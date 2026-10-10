//! Port of `Common/training.c`: the training level (`WORLDID_FRONTEND`/`LEVELID_FRONTEND4`)
//! with its sliding hint panel. The hints come from the level script (`EV_HINT`,
//! [`crate::script`]): `TrainingHint(GAMESTRING(str + STR_TRAINING1))`.

use serde::{Deserialize, Serialize};

use crate::define::mode::TRAINING_MODE;
use crate::define::SINGLEPLAYER;
use crate::frogger::UP_VEC;
use crate::frontend::NUM_LIVES;
use crate::ids::{OverlayId, TextOverlayId};
use crate::specfx::FXTYPE_SPARKLYTRAIL;
use crate::textover::{Font, TEXTOVERLAY_SHADOW};
use crate::Game;

/// `MAX_LINES` of `TrainingHint`.
const MAX_LINES: usize = 4;

/// `TRAININGMODE` (`training.h:18`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrainingMode {
    /// `bg` (sprite overlay `SHEEN`).
    pub bg: Option<OverlayId>,
    /// `txtover[2]` (small font, shadowed).
    pub txtover: [Option<TextOverlayId>; 2],
    /// `text[1024]`: the hint being shown (the overlays hold its first two wrapped lines).
    pub text: String,
}

/// Globals of `training.c`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrainingState {
    /// `train` (`None` = `NULL`).
    pub train: Option<TrainingMode>,
    /// `lastStr` (the original compares `GAMESTRING` pointers, one per string id; the port
    /// compares the text).
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

        train.bg = self.overlays.create_and_add_sprite_overlay(0, 4096, Some("SHEEN"), 4096, 900, 180, 0);
        if let Some(bg) = self.overlays.get_mut(train.bg) {
            bg.speed = 75 << 12;
            bg.r = 128;
            bg.g = 128;
            bg.b = 128;
        }
        for i in 0..2 {
            let t = self.textover.create_and_add_text_overlay(2048, (4096 + 204 + i * 250) as i16, "", true, 255, Font::Small, TEXTOVERLAY_SHADOW);
            if let Some(o) = self.textover.get_mut(t) {
                o.speed = 75 << 12;
            }
            train.txtover[i] = t;
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

    /// Port of `TrainingHint(str)` (Common/training.c:72).
    pub fn training_hint(&mut self, s: Option<&str>) {
        if self.training.curr_str.is_none() {
            self.training.curr_str = s.map(str::to_string);
        }
        // The original dereferences `train` unconditionally (NULL outside the training level).
        let Some(t) = self.training.train.clone() else { return };
        let Some(s) = s else {
            self.training_slide(&t, 4096, 4096 + 204);
            return;
        };
        if t.bg.is_none() {
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
            let Some((y, y_to)) = self.overlays.get(t.bg).map(|o| (o.y_pos, o.y_pos_to)) else { return };
            if y_to == y {
                if y >= 4096 {
                    self.training_set_text(s);
                    self.training.last_str = Some(s.to_string());
                    self.training_slide(&t, 3296, 3500);
                } else {
                    let last = self.training.last_str.clone().unwrap_or_default();
                    self.training_set_text(&last);
                    self.training_slide(&t, 4096, 4096 + 204);
                }
            }
        } else {
            self.training_set_text(s);
        }
    }

    /// `StringWrap(str, 4096-128, train->text, 1024, lines, MAX_LINES, fontSmall)` then
    /// `txtover[0..2]->text = lines[0..2]`.
    fn training_set_text(&mut self, s: &str) {
        let lines = self.textover.string_wrap(s, 4096 - 128, MAX_LINES, Font::Small);
        let Some(t) = self.training.train.as_mut() else { return };
        t.text = s.to_string();
        let ids = t.txtover;
        for (i, id) in ids.iter().enumerate() {
            let line = lines.get(i).cloned().unwrap_or_default();
            if let Some(o) = self.textover.get_mut(*id) {
                o.text = line;
            }
        }
    }

    /// Sets `bg->yPosTo = bg_y`, `txtover[i]->yPosTo = text_y + i*250`, all with speed `75<<12`.
    fn training_slide(&mut self, t: &TrainingMode, bg_y: i32, text_y: i32) {
        if let Some(bg) = self.overlays.get_mut(t.bg) {
            bg.y_pos_to = bg_y as i16;
            bg.speed = 75 << 12;
        }
        for (i, id) in t.txtover.iter().enumerate() {
            if let Some(o) = self.textover.get_mut(*id) {
                o.y_pos_to = (text_y + i as i32 * 250) as i16;
                o.speed = 75 << 12;
            }
        }
    }
}
