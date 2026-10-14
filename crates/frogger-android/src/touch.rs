//! On-screen touch controls: a d-pad on the left, the four PlayStation face buttons on the
//! right, shoulder buttons in the top corners and Start/Select at the top centre. Touches
//! become a [`RawPad`] for player 1 (the app maps it like any joypad, with PlayStation
//! button prompts).
//!
//! Layout and hit testing are pure (unit-tested on the host); [`TouchPad::draw`] queues
//! 2D quads into the frame after `App::draw`, textured from a small procedurally drawn atlas.

use f2_engine::{Blend, Quad2D, Renderer, TexHandle};
use f2_platform::{PadButton, RawPad};
use std::collections::BTreeMap;

/// A circular control.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle {
    pub x: f32,
    pub y: f32,
    pub r: f32,
}

impl Circle {
    fn contains(&self, x: f32, y: f32, slack: f32) -> bool {
        let (dx, dy) = (x - self.x, y - self.y);
        dx * dx + dy * dy <= (self.r * slack) * (self.r * slack)
    }
}

/// Atlas cells (64x64 each, 4 columns x 2 rows).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cell {
    Disc,
    Ring,
    Cross,
    CircleSym,
    Square,
    Triangle,
    Arrow,
    Bars,
}

const CELL: u32 = 64;
const ATLAS_W: u32 = CELL * 4;
const ATLAS_H: u32 = CELL * 2;

/// The control positions for a screen size (physical pixels) and density (`scale_factor`,
/// physical pixels per dp).
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub dpad: Circle,
    /// South (cross), East (circle), West (square), North (triangle).
    pub face: [(PadButton, Circle); 4],
    /// L1, R1, L2, R2, Select, Start.
    pub small: [(PadButton, Circle); 6],
}

impl Layout {
    pub fn new(w: f32, h: f32, dp: f32) -> Layout {
        // Fit small screens: the controls never take more than ~45% of the height.
        let dp = dp.min(h / 330.0).max(0.5);
        let m = 20.0 * dp;
        let dr = 68.0 * dp;
        let dpad = Circle { x: m + dr, y: h - m - dr, r: dr };
        let br = 27.0 * dp;
        let gap = 40.0 * dp;
        let (cx, cy) = (w - m - gap - br, h - m - gap - br);
        let b = |dx: f32, dy: f32| Circle { x: cx + dx * gap, y: cy + dy * gap, r: br };
        let face = [(PadButton::South, b(0.0, 1.0)), (PadButton::East, b(1.0, 0.0)), (PadButton::West, b(-1.0, 0.0)), (PadButton::North, b(0.0, -1.0))];
        let sr = 22.0 * dp;
        let s = |x: f32, y: f32| Circle { x, y, r: sr };
        // Shoulders on the screen edges above the d-pad / face buttons (the top corners
        // hold the game's HUD); Select/Start at the top centre.
        let l1 = dpad.y - dr - 0.6 * m - sr;
        let r1 = cy - gap - br - 0.6 * m - sr;
        let small = [
            (PadButton::L1, s(m + sr, l1)),
            (PadButton::R1, s(w - m - sr, r1)),
            (PadButton::L2, s(m + sr, l1 - 2.6 * sr)),
            (PadButton::R2, s(w - m - sr, r1 - 2.6 * sr)),
            (PadButton::Select, s(w / 2.0 - 1.6 * sr, m + sr)),
            (PadButton::Start, s(w / 2.0 + 1.6 * sr, m + sr)),
        ];
        Layout { dpad, face, small }
    }

    /// The d-pad direction for a point relative to the d-pad (4-way; `None` in the centre).
    pub fn dpad_dir(&self, x: f32, y: f32) -> Option<PadButton> {
        let (dx, dy) = (x - self.dpad.x, y - self.dpad.y);
        if dx * dx + dy * dy < (self.dpad.r * 0.22) * (self.dpad.r * 0.22) {
            return None;
        }
        Some(if dx.abs() > dy.abs() {
            if dx > 0.0 {
                PadButton::DPadRight
            } else {
                PadButton::DPadLeft
            }
        } else if dy > 0.0 {
            PadButton::DPadDown
        } else {
            PadButton::DPadUp
        })
    }

    /// The button under a point (face buttons get some slack so thumbs between two
    /// buttons still hit one).
    pub fn button_at(&self, x: f32, y: f32) -> Option<PadButton> {
        let best = |list: &[(PadButton, Circle)], slack: f32| {
            list.iter()
                .filter(|(_, c)| c.contains(x, y, slack))
                .min_by(|a, b| {
                    let d = |c: &Circle| (x - c.x).powi(2) + (y - c.y).powi(2);
                    d(&a.1).total_cmp(&d(&b.1))
                })
                .map(|(b, _)| *b)
        };
        best(&self.face, 1.35).or_else(|| best(&self.small, 1.25))
    }
}

/// What a finger is doing.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Finger {
    /// Started on the d-pad: steers until it lifts, wherever it goes.
    DPad { x: f32, y: f32 },
    /// Started on (or slid onto) a button.
    Button { x: f32, y: f32 },
    /// Started elsewhere: ignored.
    Other,
}

/// Touch state for one window.
pub struct TouchPad {
    pub layout: Layout,
    fingers: BTreeMap<u64, Finger>,
    /// Drawn only after the screen was touched; a gamepad hides it again.
    pub visible: bool,
    /// Atlas handle in the current renderer (renderers are rebuilt on level changes).
    atlas: Option<TexHandle>,
    atlas_rgba: Vec<u8>,
}

/// Touch phase (winit's `TouchPhase` without the dependency).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Started,
    Moved,
    Ended,
}

impl TouchPad {
    pub fn new(w: f32, h: f32, dp: f32) -> TouchPad {
        TouchPad { layout: Layout::new(w, h, dp), fingers: BTreeMap::new(), visible: true, atlas: None, atlas_rgba: atlas() }
    }

    pub fn resize(&mut self, w: f32, h: f32, dp: f32) {
        self.layout = Layout::new(w, h, dp);
        self.fingers.clear();
    }

    pub fn touch(&mut self, id: u64, phase: Phase, x: f32, y: f32) {
        match phase {
            Phase::Started => {
                self.visible = true;
                let f = if self.layout.dpad.contains(x, y, 1.3) {
                    Finger::DPad { x, y }
                } else if self.layout.button_at(x, y).is_some() {
                    Finger::Button { x, y }
                } else {
                    Finger::Other
                };
                self.fingers.insert(id, f);
            }
            // Moves of pointers that never started (e.g. joystick motion events) are ignored.
            Phase::Moved => match self.fingers.get_mut(&id) {
                Some(Finger::DPad { x: fx, y: fy }) | Some(Finger::Button { x: fx, y: fy }) => (*fx, *fy) = (x, y),
                Some(f @ Finger::Other) if self.layout.button_at(x, y).is_some() => *f = Finger::Button { x, y },
                _ => {}
            },
            Phase::Ended => {
                self.fingers.remove(&id);
            }
        }
    }

    /// Lifts every finger (focus lost / suspend).
    pub fn release_all(&mut self) {
        self.fingers.clear();
    }

    /// The buttons held now.
    pub fn pad(&self) -> RawPad {
        let mut p = RawPad::default();
        for f in self.fingers.values() {
            match *f {
                Finger::DPad { x, y } => {
                    if let Some(d) = self.layout.dpad_dir(x, y) {
                        p.set_button(d, true);
                    }
                }
                Finger::Button { x, y } => {
                    if let Some(b) = self.layout.button_at(x, y) {
                        p.set_button(b, true);
                    }
                }
                Finger::Other => {}
            }
        }
        p
    }

    /// Queues the controls into the renderer's current frame (after `App::draw`).
    pub fn draw(&mut self, r: &mut Renderer) {
        if !self.visible {
            return;
        }
        let tex = self.atlas_handle(r);
        let held = self.pad();
        let quad = |r: &mut Renderer, c: &Circle, cell: Cell, scale: f32, rot: u32, color: [u8; 4]| {
            let (col, row) = (cell as u32 % 4, cell as u32 / 4);
            let (u0, v0) = ((col * CELL) as f32 / ATLAS_W as f32, (row * CELL) as f32 / ATLAS_H as f32);
            let (u1, v1) = (u0 + CELL as f32 / ATLAS_W as f32, v0 + CELL as f32 / ATLAS_H as f32);
            let s = c.r * scale;
            let mut pos = [[c.x - s, c.y - s], [c.x + s, c.y - s], [c.x + s, c.y + s], [c.x - s, c.y + s]];
            pos.rotate_right(rot as usize % 4);
            r.draw_quad(Quad2D { pos, uv: [u0, v0, u1, v1], tex: Some(tex), color, blend: Blend::Semi });
        };
        const IDLE: [u8; 4] = [255, 255, 255, 70];
        const DOWN: [u8; 4] = [255, 255, 255, 150];
        // D-pad: base disc, four arrows (lit when held).
        let d = self.layout.dpad;
        quad(r, &d, Cell::Disc, 1.0, 0, [40, 40, 40, 90]);
        quad(r, &d, Cell::Ring, 1.0, 0, IDLE);
        for (i, b) in [PadButton::DPadUp, PadButton::DPadRight, PadButton::DPadDown, PadButton::DPadLeft].into_iter().enumerate() {
            let (dx, dy) = [(0.0, -1.0), (1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)][i];
            let c = Circle { x: d.x + dx * d.r * 0.58, y: d.y + dy * d.r * 0.58, r: d.r * 0.3 };
            // `rot` turns the quad's corners counter-clockwise: up, right, down, left.
            quad(r, &c, Cell::Arrow, 1.0, [0, 3, 2, 1][i], if held.button(b) { [255, 255, 255, 220] } else { [255, 255, 255, 120] });
        }
        // Face buttons in their PlayStation colours.
        for (b, c) in self.layout.face {
            let (sym, rgb) = match b {
                PadButton::South => (Cell::Cross, [124, 178, 232]),
                PadButton::East => (Cell::CircleSym, [255, 102, 102]),
                PadButton::West => (Cell::Square, [255, 105, 248]),
                _ => (Cell::Triangle, [64, 226, 160]),
            };
            let down = held.button(b);
            quad(r, &c, Cell::Disc, 1.0, 0, if down { [90, 90, 90, 170] } else { [30, 30, 30, 100] });
            quad(r, &c, Cell::Ring, 1.0, 0, if down { DOWN } else { IDLE });
            quad(r, &c, sym, 0.62, 0, [rgb[0], rgb[1], rgb[2], if down { 255 } else { 190 }]);
        }
        for (b, c) in self.layout.small {
            let down = held.button(b);
            quad(r, &c, Cell::Disc, 1.0, 0, if down { [90, 90, 90, 170] } else { [30, 30, 30, 90] });
            quad(r, &c, Cell::Ring, 1.0, 0, if down { DOWN } else { IDLE });
            let sym = match b {
                PadButton::Start => Cell::Arrow,
                PadButton::Select => Cell::Bars,
                _ => Cell::Disc,
            };
            let rot = if b == PadButton::Start { 3 } else { 0 };
            let scale = if matches!(b, PadButton::Start | PadButton::Select) { 0.5 } else { 0.0 };
            if scale > 0.0 {
                quad(r, &c, sym, scale, rot, [255, 255, 255, if down { 230 } else { 150 }]);
            } else {
                // Shoulders: 1 or 2 dots (L1/R1 one, L2/R2 two).
                let n = if matches!(b, PadButton::L1 | PadButton::R1) { 1 } else { 2 };
                for k in 0..n {
                    let off = if n == 1 { 0.0 } else { (k as f32 - 0.5) * c.r * 0.5 };
                    let dot = Circle { x: c.x + off, y: c.y, r: c.r * 0.18 };
                    quad(r, &dot, Cell::Disc, 1.0, 0, [255, 255, 255, if down { 230 } else { 150 }]);
                }
            }
        }
    }

    fn atlas_handle(&mut self, r: &mut Renderer) -> TexHandle {
        // The app rebuilds its renderer on level changes: re-register the atlas when the
        // handle no longer points at it.
        let ok = self.atlas.is_some_and(|h| {
            r.textures.images.get(h.0 as usize).is_some_and(|i| i.width == ATLAS_W && i.height == ATLAS_H && i.rgba[..] == self.atlas_rgba[..])
        });
        if !ok {
            self.atlas = Some(r.textures.add_image(ATLAS_W, ATLAS_H, self.atlas_rgba.clone()));
        }
        self.atlas.unwrap()
    }
}

/// Draws the atlas: white shapes with anti-aliased edges (alpha = coverage).
fn atlas() -> Vec<u8> {
    let mut px = vec![0u8; (ATLAS_W * ATLAS_H * 4) as usize];
    let cells = [Cell::Disc, Cell::Ring, Cell::Cross, Cell::CircleSym, Cell::Square, Cell::Triangle, Cell::Arrow, Cell::Bars];
    for (i, cell) in cells.into_iter().enumerate() {
        let (ox, oy) = ((i as u32 % 4) * CELL, (i as u32 / 4) * CELL);
        for y in 0..CELL {
            for x in 0..CELL {
                // 4x4 supersampling; coordinates in -1..1.
                let mut cov = 0u32;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let u = ((x as f32 + (sx as f32 + 0.5) / 4.0) / CELL as f32) * 2.0 - 1.0;
                        let v = ((y as f32 + (sy as f32 + 0.5) / 4.0) / CELL as f32) * 2.0 - 1.0;
                        cov += inside(cell, u, v) as u32;
                    }
                }
                let a = (cov * 255 / 16) as u8;
                let o = (((oy + y) * ATLAS_W + ox + x) * 4) as usize;
                px[o..o + 4].copy_from_slice(&[255, 255, 255, a]);
            }
        }
    }
    px
}

fn inside(cell: Cell, u: f32, v: f32) -> bool {
    let r = (u * u + v * v).sqrt();
    let t = 0.17; // stroke width
    match cell {
        Cell::Disc => r <= 0.96,
        Cell::Ring => (0.86..=0.96).contains(&r),
        Cell::Cross => {
            let (a, b) = ((u - v).abs() / std::f32::consts::SQRT_2, (u + v).abs() / std::f32::consts::SQRT_2);
            (a <= t / 1.4 || b <= t / 1.4) && u.abs() <= 0.8 && v.abs() <= 0.8
        }
        Cell::CircleSym => ((0.8 - t)..=0.8).contains(&r),
        Cell::Square => {
            let m = u.abs().max(v.abs());
            ((0.72 - t)..=0.72).contains(&m)
        }
        Cell::Triangle => {
            // Equilateral outline, apex up: inside the outer triangle, outside the inner one.
            let tri = |s: f32, u: f32, v: f32| {
                let v = v - 0.12;
                v <= 0.5 * s && (1.732 * u.abs() - v) <= s
            };
            tri(0.85, u, v) && !tri(0.85 - 2.4 * t, u, v)
        }
        // Filled arrow pointing up.
        Cell::Arrow => (-0.7..=0.55).contains(&v) && u.abs() <= (v + 0.7) * 0.75,
        // Three horizontal bars (menu).
        Cell::Bars => u.abs() <= 0.75 && [-0.5f32, 0.0, 0.5].iter().any(|c| (v - c).abs() <= 0.12),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad() -> TouchPad {
        TouchPad::new(2400.0, 1080.0, 2.75)
    }

    #[test]
    fn controls_fit_on_screen() {
        for (w, h, dp) in [(2400.0, 1080.0, 2.75), (1280.0, 720.0, 2.0), (800.0, 480.0, 1.5), (2560.0, 1600.0, 2.0)] {
            let l = Layout::new(w, h, dp);
            let all = std::iter::once(l.dpad).chain(l.face.iter().map(|f| f.1)).chain(l.small.iter().map(|s| s.1));
            for c in all {
                assert!(c.x - c.r >= 0.0 && c.x + c.r <= w && c.y - c.r >= 0.0 && c.y + c.r <= h, "{c:?} off {w}x{h}");
            }
            // The d-pad and face buttons do not overlap.
            for (_, f) in l.face {
                assert!((f.x - l.dpad.x).abs() > f.r + l.dpad.r);
            }
        }
    }

    #[test]
    fn dpad_directions() {
        let mut t = pad();
        let d = t.layout.dpad;
        t.touch(1, Phase::Started, d.x, d.y - d.r * 0.7);
        assert!(t.pad().button(PadButton::DPadUp));
        // Sliding keeps steering, even past the d-pad.
        t.touch(1, Phase::Moved, d.x + d.r * 2.0, d.y);
        let p = t.pad();
        assert!(p.button(PadButton::DPadRight) && !p.button(PadButton::DPadUp));
        t.touch(1, Phase::Moved, d.x, d.y);
        assert_eq!(t.pad().buttons, 0, "centre is neutral");
        t.touch(1, Phase::Ended, d.x, d.y);
        assert_eq!(t.pad().buttons, 0);
    }

    #[test]
    fn multitouch_buttons() {
        let mut t = pad();
        let d = t.layout.dpad;
        let cross = t.layout.face[0].1;
        let tri = t.layout.face[3].1;
        t.touch(1, Phase::Started, d.x - d.r * 0.8, d.y);
        t.touch(2, Phase::Started, cross.x, cross.y);
        let p = t.pad();
        assert!(p.button(PadButton::DPadLeft) && p.button(PadButton::South));
        // Sliding from cross to triangle.
        t.touch(2, Phase::Moved, tri.x, tri.y);
        let p = t.pad();
        assert!(p.button(PadButton::North) && !p.button(PadButton::South));
        // A touch in empty space does nothing; moves without a start are ignored.
        t.touch(3, Phase::Started, 1200.0, 500.0);
        t.touch(9, Phase::Moved, cross.x, cross.y);
        assert_eq!(t.pad().buttons.count_ones(), 2);
        t.release_all();
        assert_eq!(t.pad().buttons, 0);
    }

    #[test]
    fn start_button() {
        let mut t = pad();
        let s = t.layout.small.iter().find(|s| s.0 == PadButton::Start).unwrap().1;
        t.touch(4, Phase::Started, s.x, s.y);
        assert!(t.pad().button(PadButton::Start));
    }

    #[test]
    fn atlas_shapes() {
        let a = atlas();
        assert_eq!(a.len(), (ATLAS_W * ATLAS_H * 4) as usize);
        let alpha = |x: u32, y: u32| a[((y * ATLAS_W + x) * 4 + 3) as usize];
        assert_eq!(alpha(32, 32), 255, "disc centre");
        assert_eq!(alpha(64 + 32, 32), 0, "ring centre is empty");
        assert_eq!(alpha(0, 0), 0, "corners are clear");
    }
}
