//! A small 2D UI kit over [`f2_engine::Renderer`] for the launcher screens (desktop window and
//! Android): anti-aliased text from a bundled Roboto subset (Apache-2.0, `assets/`), rounded
//! rectangles, gradients, simple icons and one picture (the game's artwork), all drawn as
//! textured quads. Implements [`f2_launcher::ui::Painter`].
//!
//! Text is rasterised at the exact on-screen pixel size (one glyph atlas per size and weight,
//! made on first use), so it is crisp at any scale. Shapes come from one generated atlas:
//! a quarter disc for the corners, its inverse for clipping the picture, and the icons.

use f2_engine::{Blend, Quad2D, Renderer, TexHandle};
use f2_launcher::ui::{Icon, Painter};
use std::collections::HashMap;

const REGULAR: &[u8] = include_bytes!("../assets/Roboto-Regular.subset.ttf");
const BOLD: &[u8] = include_bytes!("../assets/Roboto-Bold.subset.ttf");

/// Side of one shape cell in the shapes atlas.
const CELL: u32 = 64;
const ICONS: [Icon; 9] = [Icon::Play, Icon::Layers, Icon::Grid, Icon::Gear, Icon::Check, Icon::Download, Icon::Refresh, Icon::Folder, Icon::Back];
/// Cells: 0 = corner (quarter disc), 1 = inverse corner, then the icons.
const CELLS: u32 = 2 + ICONS.len() as u32;

#[derive(Clone, Copy)]
struct Glyph {
    uv: [f32; 4],
    w: f32,
    h: f32,
    /// Offset of the bitmap from the pen position / the line top.
    x: f32,
    y: f32,
    advance: f32,
}

struct Atlas {
    tex: TexHandle,
    glyphs: HashMap<char, Glyph>,
}

/// The fonts, glyph atlases and pictures; create once, draw with [`UiKit::painter`].
pub struct UiKit {
    fonts: [fontdue::Font; 2],
    atlases: HashMap<(bool, u32), Atlas>,
    shapes: Option<TexHandle>,
    images: HashMap<String, TexHandle>,
}

impl Default for UiKit {
    fn default() -> Self {
        Self::new()
    }
}

impl UiKit {
    pub fn new() -> UiKit {
        let load = |b: &[u8]| fontdue::Font::from_bytes(b, fontdue::FontSettings::default()).expect("bundled font");
        UiKit { fonts: [load(REGULAR), load(BOLD)], atlases: HashMap::new(), shapes: None, images: HashMap::new() }
    }

    /// Registers (or replaces) the picture drawn for `key` (RGBA8).
    pub fn set_image(&mut self, r: &mut Renderer, key: &str, width: u32, height: u32, rgba: Vec<u8>) {
        let h = r.textures.add_image(width, height, rgba);
        self.images.insert(key.to_string(), h);
    }

    /// A painter drawing into `r`'s current frame.
    pub fn painter<'a>(&'a mut self, r: &'a mut Renderer) -> Frame<'a> {
        r.options.linear_filter = true;
        let shapes = *self.shapes.get_or_insert_with(|| r.textures.add_image(CELL * CELLS, CELL, shapes_rgba()));
        Frame { kit: self, r, shapes }
    }

    fn atlas(&mut self, r: &mut Renderer, bold: bool, px: u32) -> &Atlas {
        self.atlases.entry((bold, px)).or_insert_with(|| build_atlas(&self.fonts[bold as usize], px, r))
    }
}

fn build_atlas(font: &fontdue::Font, px: u32, r: &mut Renderer) -> Atlas {
    let pxf = px as f32;
    let ascent = font.horizontal_line_metrics(pxf).map_or(pxf * 0.93, |m| m.ascent);
    let chars: Vec<char> =
        (0x20u32..=0x7e).chain(0xa0..=0xff).chain([0x2013, 0x2014, 0x2018, 0x2019, 0x201c, 0x201d, 0x2022, 0x2026]).filter_map(char::from_u32).collect();
    let rast: Vec<(char, fontdue::Metrics, Vec<u8>)> = chars
        .iter()
        .filter(|&&c| font.lookup_glyph_index(c) != 0)
        .map(|&c| {
            let (m, bm) = font.rasterize(c, pxf);
            (c, m, bm)
        })
        .collect();
    // Shelf packing into a 1024-wide atlas, one pixel apart.
    let aw = 1024u32;
    let (mut x, mut y, mut row) = (1u32, 1u32, 0u32);
    let mut pos = Vec::new();
    for (_, m, _) in &rast {
        let (w, h) = (m.width as u32, m.height as u32);
        if x + w + 1 > aw {
            x = 1;
            y += row + 1;
            row = 0;
        }
        pos.push((x, y));
        x += w + 1;
        row = row.max(h);
    }
    let ah = (y + row + 1).max(2);
    let mut rgba = vec![0u8; (aw * ah * 4) as usize];
    let mut glyphs = HashMap::new();
    for ((c, m, bm), (gx, gy)) in rast.iter().zip(pos) {
        let (w, h) = (m.width as u32, m.height as u32);
        for row in 0..h {
            for col in 0..w {
                let o = (((gy + row) * aw + gx + col) * 4) as usize;
                rgba[o..o + 3].copy_from_slice(&[255, 255, 255]);
                rgba[o + 3] = bm[(row * w + col) as usize];
            }
        }
        let uv = [gx as f32 / aw as f32, gy as f32 / ah as f32, (gx + w) as f32 / aw as f32, (gy + h) as f32 / ah as f32];
        glyphs.insert(*c, Glyph { uv, w: w as f32, h: h as f32, x: m.xmin as f32, y: ascent - (m.ymin as f32 + h as f32), advance: m.advance_width });
    }
    Atlas { tex: r.textures.add_image(aw, ah, rgba), glyphs }
}

/// The painter for one frame.
pub struct Frame<'a> {
    kit: &'a mut UiKit,
    r: &'a mut Renderer,
    shapes: TexHandle,
}

impl Frame<'_> {
    fn solid(&mut self, [x, y, w, h]: [f32; 4], c: [u8; 4]) {
        if w > 0.0 && h > 0.0 && c[3] > 0 {
            self.r.draw_quad(Quad2D::rect(x, y, w, h, [0.0, 0.0, 1.0, 1.0], None, c, Blend::Semi));
        }
    }

    /// UV rectangle of shape cell `i`, inset half a texel against bleeding.
    fn cell_uv(i: u32, flip_u: bool, flip_v: bool) -> [f32; 4] {
        let tw = (CELL * CELLS) as f32;
        let (u1, u2) = ((i * CELL) as f32 / tw + 0.5 / tw, ((i + 1) * CELL) as f32 / tw - 0.5 / tw);
        let (v1, v2) = (0.5 / CELL as f32, 1.0 - 0.5 / CELL as f32);
        let (u1, u2) = if flip_u { (u2, u1) } else { (u1, u2) };
        let (v1, v2) = if flip_v { (v2, v1) } else { (v1, v2) };
        [u1, v1, u2, v2]
    }

    fn corners(&mut self, [x, y, w, h]: [f32; 4], rad: f32, cell: u32, c: [u8; 4]) {
        let s = Some(self.shapes);
        for (cx, cy, fu, fv) in [(x, y, false, false), (x + w - rad, y, true, false), (x, y + h - rad, false, true), (x + w - rad, y + h - rad, true, true)] {
            self.r.draw_quad(Quad2D::rect(cx, cy, rad, rad, Self::cell_uv(cell, fu, fv), s, c, Blend::Semi));
        }
    }
}

impl Painter for Frame<'_> {
    fn rect(&mut self, r: [f32; 4], color: [u8; 4]) {
        self.solid(r, color);
    }

    fn round_rect(&mut self, r: [f32; 4], radius: f32, color: [u8; 4]) {
        let [x, y, w, h] = r;
        let rad = radius.min(w / 2.0).min(h / 2.0).max(0.0);
        if rad < 1.0 {
            return self.solid(r, color);
        }
        self.solid([x + rad, y, w - 2.0 * rad, h], color);
        self.solid([x, y + rad, rad, h - 2.0 * rad], color);
        self.solid([x + w - rad, y + rad, rad, h - 2.0 * rad], color);
        self.corners(r, rad, 0, color);
    }

    fn gradient_v(&mut self, r: [f32; 4], top: [u8; 4], bottom: [u8; 4]) {
        let [x, y, w, h] = r;
        let n = ((h / 3.0).ceil() as usize).clamp(1, 64);
        for i in 0..n {
            let (y0, y1) = (y + h * i as f32 / n as f32, y + h * (i + 1) as f32 / n as f32);
            let t = (i as f32 + 0.5) / n as f32;
            let c = std::array::from_fn(|k| (top[k] as f32 + (bottom[k] as f32 - top[k] as f32) * t).round() as u8);
            self.solid([x, y0, w, y1 - y0], c);
        }
    }

    fn text(&mut self, x: f32, y: f32, s: &str, color: [u8; 4], size: f32, bold: bool) {
        let px = size.round().max(6.0) as u32;
        let a = self.kit.atlas(self.r, bold, px);
        let tex = Some(a.tex);
        let mut pen = x.round();
        let top = y.round();
        let mut quads = Vec::new();
        for ch in s.chars() {
            let Some(g) = a.glyphs.get(&ch).or_else(|| a.glyphs.get(&'?')) else { continue };
            if g.w > 0.0 {
                quads.push(Quad2D::rect((pen + g.x).round(), (top + g.y).round(), g.w, g.h, g.uv, tex, color, Blend::Semi));
            }
            pen += g.advance;
        }
        for q in quads {
            self.r.draw_quad(q);
        }
    }

    fn text_width(&mut self, s: &str, size: f32, bold: bool) -> f32 {
        let px = size.round().max(6.0) as u32;
        let a = self.kit.atlas(self.r, bold, px);
        s.chars().filter_map(|c| a.glyphs.get(&c).or_else(|| a.glyphs.get(&'?'))).map(|g| g.advance).sum()
    }

    fn icon(&mut self, icon: Icon, [x, y, w, h]: [f32; 4], color: [u8; 4]) {
        let i = 2 + ICONS.iter().position(|&i| i == icon).unwrap_or(0) as u32;
        self.r.draw_quad(Quad2D::rect(x, y, w, h, Self::cell_uv(i, false, false), Some(self.shapes), color, Blend::Semi));
    }

    fn image(&mut self, key: &str, dst: [f32; 4], uv: [f32; 4]) {
        let [x, y, w, h] = dst;
        match self.kit.images.get(key) {
            Some(&tex) => self.r.draw_quad(Quad2D::rect(x, y, w, h, uv, Some(tex), [255, 255, 255, 255], Blend::Semi)),
            None => self.solid(dst, [30, 34, 40, 255]),
        }
    }

    fn round_mask(&mut self, r: [f32; 4], radius: f32, bg: [u8; 4]) {
        let rad = radius.min(r[2] / 2.0).min(r[3] / 2.0);
        if rad >= 1.0 {
            self.corners(r, rad, 1, bg);
        }
    }
}

/// Coverage (0..=1) of `inside` over a cell, 4x4 supersampled.
fn cell_pixels(inside: impl Fn(f32, f32) -> bool, out: &mut [u8], cell: u32, stride: u32) {
    for py in 0..CELL {
        for px in 0..CELL {
            let mut n = 0;
            for sy in 0..4 {
                for sx in 0..4 {
                    let (x, y) = ((px as f32 + (sx as f32 + 0.5) / 4.0) / CELL as f32, (py as f32 + (sy as f32 + 0.5) / 4.0) / CELL as f32);
                    n += inside(x, y) as u32;
                }
            }
            let o = ((py * stride + cell * CELL + px) * 4) as usize;
            out[o..o + 3].copy_from_slice(&[255, 255, 255]);
            out[o + 3] = (n * 255 / 16) as u8;
        }
    }
}

fn seg(x: f32, y: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (dx, dy) = (bx - ax, by - ay);
    let t = (((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    ((x - ax - t * dx).powi(2) + (y - ay - t * dy).powi(2)).sqrt()
}

fn tri(x: f32, y: f32, a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> bool {
    let s = |p: (f32, f32), q: (f32, f32)| (q.0 - p.0) * (y - p.1) - (q.1 - p.1) * (x - p.0);
    let (d1, d2, d3) = (s(a, b), s(b, c), s(c, a));
    !((d1 < 0.0 || d2 < 0.0 || d3 < 0.0) && (d1 > 0.0 || d2 > 0.0 || d3 > 0.0))
}

fn rr(x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> bool {
    let dx = (x0 + r - x).max(x - (x1 - r)).max(0.0);
    let dy = (y0 + r - y).max(y - (y1 - r)).max(0.0);
    x >= x0 && x <= x1 && y >= y0 && y <= y1 && dx * dx + dy * dy <= r * r
}

fn icon_inside(i: Icon, x: f32, y: f32) -> bool {
    match i {
        Icon::Play => tri(x, y, (0.30, 0.15), (0.30, 0.85), (0.86, 0.5)),
        Icon::Layers => rr(x, y, 0.14, 0.18, 0.86, 0.34, 0.06) || rr(x, y, 0.14, 0.42, 0.86, 0.58, 0.06) || rr(x, y, 0.14, 0.66, 0.86, 0.82, 0.06),
        Icon::Grid => [(0.14, 0.14), (0.54, 0.14), (0.14, 0.54), (0.54, 0.54)].iter().any(|&(a, b)| rr(x, y, a, b, a + 0.32, b + 0.32, 0.07)),
        Icon::Gear => {
            let (dx, dy) = (x - 0.5, y - 0.5);
            let r = (dx * dx + dy * dy).sqrt();
            let a = dy.atan2(dx) / std::f32::consts::TAU * 8.0;
            let tooth = (a - a.floor() - 0.5).abs() < 0.2;
            (r < 0.31 || (r < 0.45 && tooth)) && r > 0.13
        }
        Icon::Check => seg(x, y, 0.18, 0.52, 0.42, 0.74) < 0.075 || seg(x, y, 0.42, 0.74, 0.84, 0.28) < 0.075,
        Icon::Download => {
            seg(x, y, 0.5, 0.12, 0.5, 0.58) < 0.065
                || seg(x, y, 0.27, 0.40, 0.5, 0.63) < 0.065
                || seg(x, y, 0.73, 0.40, 0.5, 0.63) < 0.065
                || seg(x, y, 0.2, 0.84, 0.8, 0.84) < 0.065
        }
        Icon::Refresh => {
            let (dx, dy) = (x - 0.5, y - 0.52);
            let r = (dx * dx + dy * dy).sqrt();
            let a = dy.atan2(dx).to_degrees();
            (r > 0.2 && r < 0.33 && !(-80.0..-5.0).contains(&a)) || tri(x, y, (0.70, 0.08), (0.94, 0.40), (0.60, 0.36))
        }
        Icon::Folder => rr(x, y, 0.10, 0.30, 0.90, 0.82, 0.07) || rr(x, y, 0.10, 0.18, 0.46, 0.38, 0.05),
        Icon::Back => seg(x, y, 0.64, 0.2, 0.34, 0.5) < 0.07 || seg(x, y, 0.34, 0.5, 0.64, 0.8) < 0.07 || seg(x, y, 0.34, 0.5, 0.84, 0.5) < 0.07,
    }
}

fn shapes_rgba() -> Vec<u8> {
    let stride = CELL * CELLS;
    let mut out = vec![0u8; (stride * CELL * 4) as usize];
    cell_pixels(|x, y| (x - 1.0).powi(2) + (y - 1.0).powi(2) <= 1.0, &mut out, 0, stride);
    cell_pixels(|x, y| (x - 1.0).powi(2) + (y - 1.0).powi(2) > 1.0, &mut out, 1, stride);
    for (i, icon) in ICONS.iter().enumerate() {
        cell_pixels(|x, y| icon_inside(*icon, x, y), &mut out, 2 + i as u32, stride);
    }
    out
}
