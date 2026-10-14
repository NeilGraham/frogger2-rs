//! The Android overlay menu: swipe in from the left edge to open a small panel (the on-screen touch
//! controls switch and the home-screen icon). Tapping outside the panel, "Close" or Back closes
//! it; the game is paused while it is open. Layout and gestures are pure (unit-tested on the
//! host); [`Menu::draw`] queues quads and text into the host's UI renderer.

use crate::touch::Phase;
use f2_engine::{Blend, FontId, Quad2D, Renderer};

/// What a touch did to the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    /// The swipe opened the menu (the host lifts the touch controls' fingers).
    Opened,
    ToggleTouch,
    /// Offer the home-screen shortcut with the player's game artwork again.
    GameArtIcon,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    Touch,
    ArtIcon,
    Close,
}

const ROWS: [Row; 3] = [Row::Touch, Row::ArtIcon, Row::Close];

pub struct Menu {
    pub open: bool,
    w: f32,
    h: f32,
    /// Pixels per dp.
    dp: f32,
    /// A finger that went down at the left edge: (id, x, y).
    swipe: Option<(u64, f32, f32)>,
}

impl Menu {
    pub fn new(w: f32, h: f32, dp: f32) -> Menu {
        Menu { open: false, w, h, dp: dp.max(0.5), swipe: None }
    }

    pub fn resize(&mut self, w: f32, h: f32, dp: f32) {
        (self.w, self.h, self.dp) = (w, h, dp.max(0.5));
        self.swipe = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.swipe = None;
    }

    /// Panel rectangle (x, y, w, h).
    fn panel(&self) -> [f32; 4] {
        let d = self.dp;
        let pw = (260.0 * d).min(self.w * 0.6);
        let ph = self.title_h() + ROWS.len() as f32 * self.row_h() + 8.0 * d;
        [0.0, ((self.h - ph) / 2.0).max(0.0), pw, ph]
    }

    fn title_h(&self) -> f32 {
        40.0 * self.dp
    }

    fn row_h(&self) -> f32 {
        48.0 * self.dp
    }

    fn row_rect(&self, i: usize) -> [f32; 4] {
        let [x, y, w, _] = self.panel();
        [x, y + self.title_h() + i as f32 * self.row_h(), w, self.row_h()]
    }

    fn row_at(&self, x: f32, y: f32) -> Option<Row> {
        (0..ROWS.len()).find(|&i| inside(self.row_rect(i), x, y)).map(|i| ROWS[i])
    }

    /// Feeds a touch. Returns whether the menu took it (the touch controls must not see it)
    /// and what happened.
    pub fn touch(&mut self, id: u64, phase: Phase, x: f32, y: f32) -> (bool, Action) {
        if self.open {
            if phase != Phase::Started {
                return (true, Action::None);
            }
            return match self.row_at(x, y) {
                Some(Row::Touch) => (true, Action::ToggleTouch),
                Some(Row::ArtIcon) => (true, Action::GameArtIcon),
                Some(Row::Close) => {
                    self.close();
                    (true, Action::Closed)
                }
                None if inside(self.panel(), x, y) => (true, Action::None),
                None => {
                    self.close();
                    (true, Action::Closed)
                }
            };
        }
        match phase {
            Phase::Started if x <= 16.0 * self.dp => {
                self.swipe = Some((id, x, y));
                (false, Action::None)
            }
            Phase::Moved => match self.swipe {
                Some((sid, sx, sy)) if sid == id => {
                    let (dx, dy) = (x - sx, (y - sy).abs());
                    if dx >= 48.0 * self.dp && dx > 1.5 * dy {
                        self.swipe = None;
                        self.open = true;
                        (true, Action::Opened)
                    } else {
                        (false, Action::None)
                    }
                }
                _ => (false, Action::None),
            },
            Phase::Ended => {
                if self.swipe.is_some_and(|(sid, _, _)| sid == id) {
                    self.swipe = None;
                }
                (false, Action::None)
            }
            Phase::Started => (false, Action::None),
        }
    }

    /// Queues the panel (when open) into `r` (already begun at the screen size).
    pub fn draw(&self, r: &mut Renderer, font: FontId, touch_on: bool) {
        if !self.open {
            return;
        }
        let rect = |r: &mut Renderer, [x, y, w, h]: [f32; 4], c: [u8; 4]| r.draw_quad(Quad2D::rect(x, y, w, h, [0.0, 0.0, 1.0, 1.0], None, c, Blend::Semi));
        let d = self.dp;
        // Dim the game behind the panel.
        rect(r, [0.0, 0.0, self.w, self.h], [0, 0, 0, 110]);
        let p = self.panel();
        rect(r, p, [18, 40, 18, 235]);
        rect(r, [p[0] + p[2] - 3.0 * d, p[1], 3.0 * d, p[3]], [90, 200, 70, 255]);
        // Built-in font: 8 px tall glyphs.
        let s = (14.0 * d / 8.0).round().max(1.0);
        let text_y = |top: f32, h: f32| top + (h - 8.0 * s) / 2.0;
        r.draw_text(font, p[0] + 16.0 * d, text_y(p[1], self.title_h()), "FROGGER 2", [120, 230, 90, 255], s, None);
        for (i, row) in ROWS.iter().enumerate() {
            let rr = self.row_rect(i);
            rect(r, [rr[0] + 8.0 * d, rr[1] + 2.0 * d, rr[2] - 16.0 * d, rr[3] - 4.0 * d], [40, 70, 40, 255]);
            let label = match row {
                Row::Touch => format!("Touch controls: {}", if touch_on { "ON" } else { "OFF" }),
                Row::ArtIcon => "Add home-screen icon".to_string(),
                Row::Close => "Close".to_string(),
            };
            r.draw_text(font, rr[0] + 16.0 * d, text_y(rr[1], rr[3]), &label, [235, 235, 235, 255], s, None);
        }
    }
}

fn inside([x, y, w, h]: [f32; 4], px: f32, py: f32) -> bool {
    px >= x && px <= x + w && py >= y && py <= y + h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu() -> Menu {
        Menu::new(1920.0, 1080.0, 2.0)
    }

    #[test]
    fn swipe_opens() {
        let mut m = menu();
        assert_eq!(m.touch(1, Phase::Started, 10.0, 500.0), (false, Action::None));
        assert_eq!(m.touch(1, Phase::Moved, 50.0, 505.0), (false, Action::None), "not far enough yet");
        assert_eq!(m.touch(1, Phase::Moved, 140.0, 510.0), (true, Action::Opened));
        assert!(m.open);
    }

    #[test]
    fn not_from_the_middle_or_vertical() {
        let mut m = menu();
        m.touch(1, Phase::Started, 400.0, 500.0);
        m.touch(1, Phase::Moved, 700.0, 500.0);
        assert!(!m.open);
        m.touch(2, Phase::Started, 5.0, 300.0);
        m.touch(2, Phase::Moved, 110.0, 600.0);
        assert!(!m.open, "mostly vertical");
        m.touch(2, Phase::Ended, 110.0, 600.0);
        m.touch(3, Phase::Moved, 300.0, 600.0);
        assert!(!m.open, "the swipe ended");
    }

    #[test]
    fn rows_and_closing() {
        let mut m = menu();
        m.open = true;
        let t = m.row_rect(0);
        assert_eq!(m.touch(5, Phase::Started, t[0] + 20.0, t[1] + 10.0), (true, Action::ToggleTouch));
        assert!(m.open);
        assert_eq!(m.touch(5, Phase::Ended, t[0] + 20.0, t[1] + 10.0), (true, Action::None));
        assert_eq!(m.touch(8, Phase::Started, m.row_rect(1)[0] + 20.0, m.row_rect(1)[1] + 10.0), (true, Action::GameArtIcon));
        let c = m.row_rect(2);
        assert_eq!(m.touch(6, Phase::Started, c[0] + 20.0, c[1] + 10.0), (true, Action::Closed));
        assert!(!m.open);
        m.open = true;
        assert_eq!(m.touch(7, Phase::Started, 1500.0, 500.0), (true, Action::Closed), "tap outside");
        assert!(!m.open);
    }

    #[test]
    fn panel_fits() {
        for (w, h, dp) in [(1920.0, 1080.0, 2.0), (2400.0, 1080.0, 2.75), (800.0, 480.0, 1.5)] {
            let m = Menu::new(w, h, dp);
            let [x, y, pw, ph] = m.panel();
            assert!(x >= 0.0 && y >= 0.0 && x + pw <= w && y + ph <= h);
        }
    }
}
