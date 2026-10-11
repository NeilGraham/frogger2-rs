//! Port of `Common/textover.c` (`TEXTOVERLAY` list, `StringWrap`, `fontFitToWidth`) and
//! the game text table (`GAMESTRING(n)`, `Common/lang.h`) loaded from
//! `MAPS\LANGUAGE.FLA`.
//!
//! Text overlays own their text (`String`): where the original pointed at a global
//! buffer that is rewritten with `sprintf` (HUD timers, lives, ...), the writer updates
//! the overlay text directly.
//!
//! Font metrics live in the renderer (MDX font bitmaps). Functions that measure text
//! (`CalcStringWidth`, `StringWrap`) use [`FontMetrics`], which the host may install;
//! without it a fixed advance of [`DEFAULT_CHAR_WIDTH`] pixels per character is used.
//!
//! Renderer API: [`TextOverlayState::block`]; draw entries with `used != 0 && draw != 0`
//! and (`mode != PAUSE_MODE` or `flags & TEXTOVERLAY_PAUSED`), in block order, between the
//! `num == 0` and `num == 1` sprite overlay passes. `x_pos/y_pos` are 4096-unit overlay
//! coordinates, `scale` is 20.12, `centred != 0` centres on `x_pos`;
//! `TEXTOVERLAY_SHADOW` draws a black copy offset by 6 (`font`) / 4 (`fontSmall`) pixels
//! at alpha `a*7/10` first.

use crate::prelude::*;

use f2_math::Fixed;
use serde::{Deserialize, Serialize};

use crate::assets::DataSource;
use crate::define::mode::PAUSE_MODE;
use crate::ids::TextOverlayId;
use crate::overlays::slide_step;
use crate::Game;

// `TEXTOVERLAY_*` flags (`textover.h`).
pub const TEXTOVERLAY_NORMAL: i8 = 0;
pub const TEXTOVERLAY_WAVEALL: i8 = 1 << 0;
pub const TEXTOVERLAY_WAVECHARS: i8 = 1 << 1;
pub const TEXTOVERLAY_WAVEDECAY: i8 = 1 << 2;
pub const TEXTOVERLAY_LIFE: i8 = 1 << 3;
pub const TEXTOVERLAY_SHADOW: i8 = 1 << 4;
pub const TEXTOVERLAY_PIXELS: i8 = 1 << 5;
pub const TEXTOVERLAY_PAUSED: i8 = 1 << 6;
pub const TEXTOVERLAY_LOADING: i8 = -128;

/// Fallback per-character advance (pixels at 640x480, scale 1.0).
pub const DEFAULT_CHAR_WIDTH: u8 = 12;

pub use crate::language::str_id as strs;

/// `psFont*`: `font` (normal) or `fontSmall`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Font {
    #[default]
    Normal,
    Small,
}

/// Per-character advances of the two fonts (pixels at 640x480, scale 1.0).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FontMetrics {
    pub normal: Vec<u8>,
    pub small: Vec<u8>,
}

impl FontMetrics {
    fn width(&self, font: Font, c: u8) -> u32 {
        let t = match font {
            Font::Normal => &self.normal,
            Font::Small => &self.small,
        };
        t.get(c as usize).copied().unwrap_or(DEFAULT_CHAR_WIDTH) as u32
    }
}

/// `TEXTOVERLAY` (`types2d.h`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextOverlay {
    pub text: String,
    pub flags: i8,
    pub x_pos: i16,
    pub y_pos: i16,
    pub x_pos_to: i16,
    pub y_pos_to: i16,
    pub centred: i16,
    /// Written by the renderer's `PrintText` in the original; unused by the simulation.
    pub t_width: i16,
    pub speed: Fixed,
    pub scale: Fixed,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
    pub oa: u8,
    pub draw: u8,
    pub font: Font,
    pub used: i16,
}

/// `TEXTOVERLAYLIST textOverlayList` plus the game text table.
#[derive(Clone, Debug, Default)]
pub struct TextOverlayState {
    pub num_entries: i32,
    pub alloc: i32,
    pub block: Vec<TextOverlay>,
    /// Installed font metrics (None = fixed advance).
    pub metrics: Option<FontMetrics>,
}

impl TextOverlayState {
    /// Port of `InitTextOverlayLinkedList` (Common/textover.c:324).
    pub fn init_text_overlay_linked_list(&mut self, num: i32) {
        self.num_entries = 0;
        self.alloc = num;
        self.block = vec![TextOverlay::default(); num.max(0) as usize];
    }

    /// Port of `FreeTextOverlayLinkedList` (Common/textover.c:340).
    pub fn free_text_overlay_linked_list(&mut self) {
        self.block.clear();
    }

    /// Port of `CreateAndAddTextOverlay` (Common/textover.c:41).
    #[allow(clippy::too_many_arguments)]
    pub fn create_and_add_text_overlay(&mut self, x: i16, y: i16, text: &str, centred: bool, alpha: u8, font: Font, flags: i8) -> Option<TextOverlayId> {
        if self.num_entries == self.alloc || self.block.is_empty() {
            return None;
        }
        let ne = self.num_entries as usize;
        let j = if self.block.get(ne).is_some_and(|o| o.used == 0) { ne } else { self.block.iter().position(|o| o.used == 0)? };
        self.num_entries += 1;
        let o = &mut self.block[j];
        o.text = text.to_string();
        o.flags = flags;
        o.draw = 1;
        o.font = font;
        o.scale = 4096;
        o.x_pos = x;
        o.y_pos = y;
        o.x_pos_to = x;
        o.y_pos_to = y;
        o.centred = centred as i16;
        o.r = 255;
        o.g = 255;
        o.b = 255;
        o.a = alpha;
        o.used = 1;
        Some(TextOverlayId(j as u32))
    }

    /// Port of `SubTextOverlay` (Common/textover.c:356).
    pub fn sub_text_overlay(&mut self, t: Option<TextOverlayId>) {
        if let Some(o) = t.and_then(|t| self.block.get_mut(t.idx())) {
            if o.used != 0 {
                o.used = 0;
                o.draw = 0;
                self.num_entries -= 1;
            }
        }
    }

    /// Port of `EnableTextOverlay` (Common/textover.c:296).
    pub fn enable(&mut self, t: Option<TextOverlayId>) {
        if let Some(o) = self.get_mut(t) {
            o.draw = 1;
        }
    }

    /// Port of `DisableTextOverlay` (Common/textover.c:308).
    pub fn disable(&mut self, t: Option<TextOverlayId>) {
        if let Some(o) = self.get_mut(t) {
            o.draw = 0;
        }
    }

    pub fn get(&self, t: Option<TextOverlayId>) -> Option<&TextOverlay> {
        t.and_then(|t| self.block.get(t.idx()))
    }

    pub fn get_mut(&mut self, t: Option<TextOverlayId>) -> Option<&mut TextOverlay> {
        t.and_then(|t| self.block.get_mut(t.idx()))
    }

    /// `CalcStringWidth(text, font, scale)` (MDX): pixel width at 640x480.
    pub fn calc_string_width(&self, text: &str, font: Font, scale: f32) -> i32 {
        let w: u32 = text.bytes().map(|c| self.metrics.as_ref().map_or(DEFAULT_CHAR_WIDTH as u32, |m| m.width(font, c))).sum();
        (w as f32 * scale) as i32
    }

    /// Port of `fontFitToWidth` (Common/textover.c:394): greedy word wrap of `text`
    /// into lines no wider than `width` pixels.
    pub fn font_fit_to_width(&self, font: Font, width: i32, text: &str) -> Vec<String> {
        let t: Vec<u8> = text.bytes().collect();
        let at = |i: isize| -> u8 {
            if i < 0 {
                0
            } else {
                t.get(i as usize).copied().unwrap_or(0)
            }
        };
        let mut lines = Vec::new();
        let mut buf: Vec<u8> = Vec::new();
        let mut ti: isize = 0;
        loop {
            if at(ti) >= b' ' {
                buf.push(at(ti));
            }
            let s: String = buf.iter().map(|&c| c as char).collect();
            let pix_len = self.calc_string_width(&s, font, 1.0);
            if pix_len > width || at(ti) == 0 {
                if pix_len > width {
                    if buf.contains(&b' ') {
                        // back up to the last space; it becomes the terminator
                        while buf.last().is_some_and(|&c| c != b' ') {
                            buf.pop();
                            ti -= 1;
                        }
                        buf.pop();
                    } else if buf.len() > 1 {
                        // BUG(original): no space in the line walks back before the
                        // buffer start; the port breaks the word before the overflow.
                        buf.pop();
                        ti -= 1;
                    }
                }
                lines.push(buf.iter().map(|&c| c as char).collect());
                buf.clear();
                if at(ti) == 0 {
                    break;
                }
            }
            ti += 1;
        }
        lines
    }

    /// Port of `StringWrap` (Common/textover.c:375). PC calls MDX `WrapStringToArray`
    /// (not in the preserved source) with `(maxWidth*rXRes)>>12` pixels; the port
    /// assumes a 640-pixel-wide screen and wraps with `fontFitToWidth`'s rules.
    /// Returns the lines (at most `array_size`).
    pub fn string_wrap(&self, text: &str, max_width: i32, array_size: usize, font: Font) -> Vec<String> {
        let mut l = self.font_fit_to_width(font, max_width.wrapping_mul(640) >> 12, text);
        l.truncate(array_size);
        l
    }
}

impl Game {
    /// Loads the game text (`gameTextInit`) in the current `gameTextLang` if not loaded yet.
    pub fn load_language(&mut self, data: &dyn DataSource) {
        if self.language.game_text_str.is_empty() {
            self.load_game_text(data, self.language.game_text_lang);
        }
    }

    /// The state part of `PrintTextOverlays` (Common/textover.c:198): slides moving
    /// overlays towards `xPosTo`/`yPosTo`.
    pub fn print_text_overlays_state(&mut self) {
        let gs = self.game_speed;
        let mode = self.game_state.mode;
        let l = &mut self.textover;
        if l.num_entries == 0 {
            return;
        }
        for o in l.block.iter_mut() {
            if o.used != 0 && o.draw != 0 && (mode != PAUSE_MODE || o.flags & TEXTOVERLAY_PAUSED != 0) && o.speed != 0 {
                slide_step(&mut o.x_pos, &mut o.y_pos, o.x_pos_to, o.y_pos_to, &mut o.speed, gs);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_fade_slots() {
        let mut t = TextOverlayState::default();
        t.init_text_overlay_linked_list(3);
        let a = t.create_and_add_text_overlay(10, 20, "A", true, 255, Font::Normal, TEXTOVERLAY_SHADOW).unwrap();
        let b = t.create_and_add_text_overlay(10, 20, "B", false, 128, Font::Small, 0).unwrap();
        let o = t.get(Some(a)).unwrap();
        assert_eq!((o.scale, o.centred, o.r, o.a), (4096, 1, 255, 255));
        t.sub_text_overlay(Some(a));
        // block[numEntries] (index 1) is used, so the first free slot (0) is taken
        let c = t.create_and_add_text_overlay(0, 0, "C", false, 1, Font::Normal, 0).unwrap();
        assert_eq!(c, a);
        let _ = b;
        t.create_and_add_text_overlay(0, 0, "D", false, 1, Font::Normal, 0).unwrap();
        assert!(t.create_and_add_text_overlay(0, 0, "E", false, 1, Font::Normal, 0).is_none());
    }

    #[test]
    fn wrap_greedy_words() {
        let t = TextOverlayState { metrics: Some(FontMetrics { normal: vec![10; 256], small: vec![5; 256] }), ..Default::default() };
        let l = t.font_fit_to_width(Font::Normal, 100, "the quick brown fox jumps over");
        assert_eq!(l, vec!["the quick", "brown fox", "jumps over"]);
        for s in &l {
            assert!(t.calc_string_width(s, Font::Normal, 1.0) <= 100);
        }
        assert_eq!(t.font_fit_to_width(Font::Small, 100, "the quick brown fox jumps over"), vec!["the quick brown fox", "jumps over"]);
        assert_eq!(t.font_fit_to_width(Font::Normal, 30, "abcdefgh ij"), vec!["abc", "def", "gh", "ij"]);
        assert_eq!(t.font_fit_to_width(Font::Normal, 100, ""), vec![""]);
        // StringWrap: maxWidth in 4096 units of a 640 screen
        assert_eq!(t.string_wrap("the quick brown fox jumps over", 640, 2, Font::Normal), vec!["the quick", "brown fox"]);
    }
}
