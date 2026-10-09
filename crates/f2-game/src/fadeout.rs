//! Port of the state of `PC/fadeout.cpp` (screen fades). Drawing (`DrawScreenFade`'s flat
//! rectangle) is the renderer's job: it reads [`Game::screen_fade_colour`] and
//! [`FadeoutState::flash_screen`] (additive instead of subtractive blend).
//!
//! `fadingOut` / `fadeoutStart` / `fadeoutLength` live in `levplay` (ported first, wave C).

use serde::{Deserialize, Serialize};

use crate::Game;

/// Globals of `fadeout.cpp` other than the timing ones.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FadeoutState {
    /// `fadeProc != NULL` (the only `fadeProc` is `DrawScreenFade`).
    pub fade_proc: bool,
    /// `keepFade`: keep drawing the end intensity once the fade is over.
    pub keep_fade: bool,
    /// `fadeText`: text overlays are faded too.
    pub fade_text: bool,
    /// `flashScreen`: additive (white flash) instead of subtractive fade.
    pub flash_screen: bool,
    /// `startIntensity` (already inverted: `255 - start`).
    pub start_intensity: i32,
    /// `endIntensity` (already inverted: `255 - end`).
    pub end_intensity: i32,
}

impl Default for FadeoutState {
    fn default() -> Self {
        FadeoutState { fade_proc: false, keep_fade: false, fade_text: true, flash_screen: false, start_intensity: 0, end_intensity: 255 }
    }
}

impl Game {
    /// Port of `ScreenFade(start, end, time)` (PC/fadeout.cpp:75): fade from brightness
    /// `start` to `end` (0-255) over `time` frames.
    pub fn screen_fade(&mut self, start: i32, end: i32, time: i32) {
        self.fadeout.fade_proc = true;
        self.levplay.fadeout_start = self.act_frame_count;
        self.levplay.fadeout_length = time;
        self.levplay.fading_out = true;
        self.fadeout.flash_screen = false;
        // cause we're using subtractive
        self.fadeout.start_intensity = 255 - start;
        self.fadeout.end_intensity = 255 - end;
        self.fadeout.fade_text = true;
    }

    /// The colour `DrawScreenFade` (PC/fadeout.cpp:34) would draw this frame (subtracted
    /// from, or with `flash_screen` added to, the frame), or `None` when no fade is active.
    pub fn screen_fade_colour(&self) -> Option<i32> {
        if !self.fadeout.fade_proc {
            return None;
        }
        let l = &self.levplay;
        let f = &self.fadeout;
        let afc = self.act_frame_count as i64;
        Some(if afc >= l.fadeout_start as i64 + l.fadeout_length as i64 {
            f.end_intensity
        } else {
            (f.start_intensity as i64 + ((f.end_intensity - f.start_intensity) as i64 * (afc - l.fadeout_start as i64)) / l.fadeout_length as i64) as i32
        })
    }

    /// Port of `DrawScreenTransition` (PC/fadeout.cpp:95): runs while drawing, after
    /// `GameLoop`. `DrawScreenFade` returns `actFrameCount > fadeoutStart + fadeoutLength`.
    pub fn draw_screen_transition(&mut self) {
        if self.fadeout.fade_proc && self.act_frame_count as i64 > self.levplay.fadeout_start as i64 + self.levplay.fadeout_length as i64 {
            self.levplay.fading_out = false;
            if !self.fadeout.keep_fade {
                self.fadeout.fade_proc = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fade_runs_and_keeps() {
        let mut g = Game::new();
        g.act_frame_count = 10;
        g.screen_fade(255, 0, 30);
        assert_eq!(g.screen_fade_colour(), Some(0));
        g.act_frame_count = 25;
        assert_eq!(g.screen_fade_colour(), Some(127));
        g.fadeout.keep_fade = true;
        g.act_frame_count = 40;
        g.draw_screen_transition();
        assert!(g.levplay.fading_out);
        g.act_frame_count = 41;
        g.draw_screen_transition();
        assert!(!g.levplay.fading_out);
        assert_eq!(g.screen_fade_colour(), Some(255));
        g.fadeout.keep_fade = false;
        g.draw_screen_transition();
        assert_eq!(g.screen_fade_colour(), None);
    }
}
