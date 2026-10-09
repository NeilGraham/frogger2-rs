//! Port of `Common/training.c`: the training level (`WORLDID_FRONTEND`/`LEVELID_FRONTEND4`)
//! with its sliding hint panel.

use serde::{Deserialize, Serialize};

use crate::define::mode::TRAINING_MODE;
use crate::define::SINGLEPLAYER;
use crate::frogger::UP_VEC;
use crate::frontend::{UiOverlay, NUM_LIVES};
use crate::specfx::FXTYPE_SPARKLYTRAIL;
use crate::Game;

/// `TRAININGMODE` (`training.h:18`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrainingMode {
    /// `bg` (sprite overlay `SHEEN`).
    pub bg: Option<UiOverlay>,
    /// `txtover[2]`.
    pub txtover: [Option<UiOverlay>; 2],
    /// `text[1024]`: the wrapped hint.
    pub text: String,
}

/// Globals of `training.c`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrainingState {
    /// `train` (`None` = `NULL`).
    pub train: Option<TrainingMode>,
    /// `lastStr` (the original compares string pointers; strings come from `GAMESTRING`).
    pub last_str: Option<String>,
    /// `currStr`.
    pub curr_str: Option<String>,
}

impl Game {
    /// Port of `InitTrainingMode` (Common/training.c:31).
    pub fn init_training_mode(&mut self) {
        let mut train = TrainingMode::default();

        self.game_state.mode = TRAINING_MODE;
        self.game_state.multi = SINGLEPLAYER;

        self.disable_hud();
        let nb = (self.map.num_babies.max(0) as usize).min(self.hud.arcade_hud.babies_back.len());
        for i in 0..nb {
            let bb = self.hud.arcade_hud.babies_back[i];
            self.overlays.enable(bb);
            if let Some(Some(ic)) = self.babyfrog.baby_icons.get_mut(i) {
                ic.draw = 1;
            }
        }

        let mut bg = UiOverlay::sprite(0, 4096, "SHEEN", 4096, 900, 180);
        bg.speed = 75 << 12;
        bg.r = 128;
        bg.g = 128;
        bg.b = 128;
        train.bg = Some(bg);
        for i in 0..2 {
            let mut t = UiOverlay::text(2048, 4096 + 204 + i as i32 * 250, "", true, 255);
            t.speed = 75 << 12;
            train.txtover[i] = Some(t);
        }
        self.training.train = Some(train);
        self.training.curr_str = None;
        self.training.last_str = None;
        self.frogger.player[0].lives = NUM_LIVES[self.game_state.difficulty as usize % 3];
    }

    /// Port of `RunTrainingMode` (Common/training.c:63).
    pub fn run_training_mode(&mut self) {
        self.run_game_loop();
        if self.training.train.is_some() {
            let s = self.training.curr_str.clone();
            self.training_hint(s.as_deref());
        }
    }

    /// Port of `TrainingHint(str)` (Common/training.c:72). `StringWrap` (font metrics) is the
    /// renderer's job: the hint text goes to `text` / `txtover[0]` unwrapped.
    pub fn training_hint(&mut self, s: Option<&str>) {
        if self.training.curr_str.is_none() {
            self.training.curr_str = s.map(str::to_string);
        }
        let Some(s) = s else {
            if let Some(t) = self.training.train.as_mut() {
                slide(t, 4096, 4096 + 204);
            }
            return;
        };
        let Some(has_bg) = self.training.train.as_ref().map(|t| t.bg.is_some()) else { return };
        if !has_bg {
            return;
        }
        if self.training.last_str.as_deref() != Some(s) {
            self.training.curr_str = Some(s.to_string());
            if let Some(ct) = self.frogmove.curr_tile[0] {
                let centre = self.map.tile(ct).centre;
                let fx = self.create_special_effect(FXTYPE_SPARKLYTRAIL, &centre, &UP_VEC, 81920, 8192 * 2, 0, 12288);
                if fx.is_some() {
                    let g = self.rng.random(256) as u8;
                    let b = self.rng.random(256) as u8;
                    self.set_fx_colour(fx, 255, g, b);
                    self.set_fx_rebound(fx, &centre, Some(&UP_VEC));
                    self.set_fx_gravity(fx, 4100);
                }
            }
            let last = self.training.last_str.clone();
            let Some(t) = self.training.train.as_mut() else { return };
            let bg = t.bg.as_ref().expect("bg");
            if bg.y_pos_to == bg.y_pos {
                if bg.y_pos >= 4096 {
                    set_text(t, s);
                    self.training.last_str = Some(s.to_string());
                    slide(t, 3296, 3500);
                } else {
                    set_text(t, last.as_deref().unwrap_or(""));
                    slide(t, 4096, 4096 + 204);
                }
            }
        } else if let Some(t) = self.training.train.as_mut() {
            set_text(t, s);
        }
    }
}

/// `StringWrap(str, ...)` + `txtover[0..2]->text = lines[0..2]` (wrapping left to the renderer).
fn set_text(t: &mut TrainingMode, s: &str) {
    t.text = s.to_string();
    if let Some(o) = t.txtover[0].as_mut() {
        o.text = s.to_string();
    }
    if let Some(o) = t.txtover[1].as_mut() {
        o.text.clear();
    }
}

/// Sets `bg->yPosTo = bg_y`, `txtover[i]->yPosTo = text_y + i*250`, all with speed `75<<12`.
fn slide(t: &mut TrainingMode, bg_y: i32, text_y: i32) {
    if let Some(bg) = t.bg.as_mut() {
        bg.y_pos_to = bg_y;
        bg.speed = 75 << 12;
    }
    for (i, o) in t.txtover.iter_mut().enumerate() {
        if let Some(o) = o.as_mut() {
            o.y_pos_to = text_y + i as i32 * 250;
            o.speed = 75 << 12;
        }
    }
}
