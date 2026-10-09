//! Port of `Common/charsel.c` (wave G): a two-frog character select screen.
//!
//! Nothing in the PC (or PSX) build calls `StartCharacterSelectMode`, and the retail
//! `GameLoop` has no `CHARSELECT_MODE` case, so this mode is unreachable in the shipped game;
//! the real character selection is `options.c` (`OP_SP_CHARSEL`, `MPChar*`). It is ported
//! for completeness and runs only when the host starts it explicitly.

use f2_math::{FVector, SVector};
use serde::{Deserialize, Serialize};

use crate::define::mode::{CHARSELECT_MODE, INGAME_MODE};
use crate::effects::Effect;
use crate::frogger::{FROG_ANIM_BREATHE, FROG_ANIM_DANCE1, FROG_ANIM_DANCE2, FROG_ANIM_SOMERSAULT, FROG_ANIM_SUPERHOP, ORIG_FROG_POOL, UP_VEC};
use crate::ids::{ActorId, OverlayId, TextOverlayId};
use crate::levplay::{PAD_CROSS, PAD_LEFT, PAD_RIGHT, PAD_START};
use crate::sprite::SPRITE_SUBTRACTIVE;
use crate::textover::Font;
use crate::Game;

/// `SPOT_LEFT` / `SPOT_RIGHT` (PC).
pub const SPOT_LEFT: &str = "xxa_sptl.obe";
pub const SPOT_RIGHT: &str = "xxa_sptr.obe";

/// `charSelCamPos` / `charSelCamTarget` / `spotLightPos` (`charsel.c:52`).
pub const CHAR_SEL_CAM_POS: FVector = FVector { vx: 0, vy: 2400000, vz: 8000000 };
pub const CHAR_SEL_CAM_TARGET: FVector = FVector { vx: 0, vy: 2400000, vz: 0 };
pub const SPOT_LIGHT_POS: SVector = SVector { vx: 300, vy: 1800, vz: 0, pad: 0 };

/// `CHARSELOPTIONS` (`charsel.h:17`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharSelOptions {
    pub character: i32,
    pub world: i32,
    pub level: i32,
}

/// `CHARSELECT_MENU charSel` (`charsel.c:39`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharSelState {
    pub options: [CharSelOptions; 2],
    pub title: Option<TextOverlayId>,
    pub names: [Option<TextOverlayId>; 2],
    pub level: [Option<TextOverlayId>; 2],
    pub lights: [Option<ActorId>; 2],
    pub subtr: [Option<OverlayId>; 2],
    pub selection: i8,
    pub level_text: [String; 2],
}

impl Game {
    /// Port of `StartCharacterSelectMode` (Common/charsel.c:71).
    pub fn start_character_select_mode(&mut self, char1: CharSelOptions, char2: CharSelOptions) {
        self.effects.push(Effect::Log { text: "Starting character select mode ..".into() });
        self.game_state.mode = CHARSELECT_MODE;
        self.free_all_lists();
        // LoadTextureBank(INGAMEGENERIC_TEX_BANK) / LoadObjectBank(INGAMEGENERIC_OBJ_BANK): host
        // (the generic banks stay loaded by the session).

        self.charsel.options = [char1, char2];

        // Text overlays
        self.charsel.title = self.textover.create_and_add_text_overlay(2048, 256, "Choose your frog", true, 255, Font::Small, 0);
        let n0 = ORIG_FROG_POOL[char1.character as usize % ORIG_FROG_POOL.len()].file_name;
        let n1 = ORIG_FROG_POOL[char2.character as usize % ORIG_FROG_POOL.len()].file_name;
        self.charsel.names[0] = self.textover.create_and_add_text_overlay(512, 3600, n0, false, 255, Font::Normal, 0);
        self.charsel.names[1] = self.textover.create_and_add_text_overlay(2800, 3600, n1, false, 0x80, Font::Normal, 0);

        for (i, c) in [char1, char2].into_iter().enumerate() {
            let d = self.layout.world_visual_data[c.world as usize].level_visual_data[c.level as usize].description_str;
            self.charsel.level_text[i] = self.gamestring(d);
        }
        let (a, b) = (self.charsel.level_text[0].clone(), self.charsel.level_text[1].clone());
        self.charsel.level[0] = self.textover.create_and_add_text_overlay(625, 512, &a, false, 255, Font::Small, 0);
        self.charsel.level[1] = self.textover.create_and_add_text_overlay(3000, 512, &b, false, 255, Font::Small, 0);

        // Camera
        self.cam.fixed_pos = 1;
        self.cam.fixed_dir = 1;
        self.cam.curr_cam_target = CHAR_SEL_CAM_TARGET;
        self.cam.curr_cam_source = CHAR_SEL_CAM_POS;

        // use players to hold our actors.. tee hee
        let m0 = ORIG_FROG_POOL[char1.character as usize % ORIG_FROG_POOL.len()].single_model;
        self.create_frog_actor(None, m0, 0);
        if let Some(f) = self.frogger.frog[0] {
            self.actor2.actors[f].actor.position.vx = -625;
            if let Some(s) = self.actor2.actors[f].actor.shadow.as_mut() {
                s.draw = 0;
            }
            self.actor_animate(f, FROG_ANIM_DANCE1, true, false, 64, false);
        }
        let m1 = ORIG_FROG_POOL[char2.character as usize % ORIG_FROG_POOL.len()].single_model;
        self.create_frog_actor(None, m1, 1);
        if let Some(f) = self.frogger.frog[1] {
            self.actor2.actors[f].actor.position.vx = 625;
            self.actor2.actors[f].actor.position.vz = -512;
            // BUG(original): hides frog[0]'s shadow again instead of frog[1]'s.
            if let Some(f0) = self.frogger.frog[0] {
                if let Some(s) = self.actor2.actors[f0].actor.shadow.as_mut() {
                    s.draw = 0;
                }
            }
            self.actor_animate(f, FROG_ANIM_BREATHE, true, false, 64, false);
        }

        // spotlights (OBJECT_FLAGS_ADDITIVE is render state)
        self.charsel.lights[0] = self.create_and_add_actor(SPOT_LEFT, -SPOT_LIGHT_POS.vx, SPOT_LIGHT_POS.vy, SPOT_LIGHT_POS.vz, 0);
        self.charsel.lights[1] = self.create_and_add_actor(SPOT_RIGHT, SPOT_LIGHT_POS.vx, SPOT_LIGHT_POS.vy, SPOT_LIGHT_POS.vz, 0);

        // subtractive sprite overlays
        self.charsel.subtr[0] = self.overlays.create_and_add_sprite_overlay(0, 0, None, 2048, 4096, 0, SPRITE_SUBTRACTIVE);
        if let Some(o) = self.overlays.get_mut(self.charsel.subtr[0]) {
            o.r = 0;
            o.g = 0;
            o.b = 0;
            o.draw = 0;
        }
        self.charsel.subtr[1] = self.overlays.create_and_add_sprite_overlay(2048, 0, None, 2048, 4096, 0, SPRITE_SUBTRACTIVE);
        if let Some(o) = self.overlays.get_mut(self.charsel.subtr[1]) {
            o.r = 0xa0;
            o.g = 0xa0;
            o.b = 0xa0;
        }

        // set up some other stuff (firstTile = NULL)
        self.map.tiles.clear();
        self.charsel.selection = 0;
    }

    /// Port of `CharSelChangeSelection` (Common/charsel.c:157).
    pub fn char_sel_change_selection(&mut self) {
        for i in 0..2usize {
            let Some(f) = self.frogger.frog[i] else { continue };
            let pos = self.actor2.actors[f].actor.position;
            let mut tpos = pos;
            if self.charsel.selection as usize == i {
                if let Some(o) = self.textover.get_mut(self.charsel.names[i]) {
                    o.a = 0xff;
                }
                self.actor_animate(f, FROG_ANIM_SUPERHOP, false, false, 96, false);
                self.actor_animate(f, FROG_ANIM_DANCE2, false, true, 64, false);
                self.actor_animate(f, FROG_ANIM_DANCE1, true, true, 64, false);
                tpos.vz = 0;
                self.calculate_frog_jump(&pos, &tpos, &UP_VEC, 50, 48, i as i32);
            } else {
                if let Some(o) = self.textover.get_mut(self.charsel.names[i]) {
                    o.a = 0x80;
                }
                self.actor_animate(f, FROG_ANIM_SOMERSAULT, false, false, -96, false);
                self.actor_animate(f, FROG_ANIM_BREATHE, true, true, 64, false);
                tpos.vz = -700;
                self.calculate_frog_jump(&pos, &tpos, &UP_VEC, 100, 32, i as i32);
            }
        }
    }

    /// Port of `RunCharacterSelectMode` (Common/charsel.c:193).
    pub fn run_character_select_mode(&mut self) {
        for pl in 0..2usize {
            let s = self.charsel.subtr[pl];
            self.frogger_hop(pl as i32);
            if self.frogger.player[pl].jump_time > 4096 {
                if let Some(f) = self.frogger.frog[pl] {
                    let a = &mut self.actor2.actors[f].actor;
                    a.position.vz = if self.charsel.selection as usize == pl { 0 } else { -700 };
                    a.position.vy = 0;
                }
                self.frogger.player[pl].jump_time = -1;
            }
            if let Some(o) = self.overlays.get_mut(s) {
                if pl == self.charsel.selection as usize {
                    if o.draw != 0 {
                        if o.r < 10 {
                            o.draw = 0;
                        } else {
                            let v = o.r - 10;
                            o.r = v;
                            o.g = v;
                            o.b = v;
                        }
                    }
                } else {
                    o.draw = 1;
                    if o.r < 0xa0 {
                        let v = o.r + 10;
                        o.r = v;
                        o.g = v;
                        o.b = v;
                    }
                }
            }
        }

        let deb = self.levplay.pad_data.debounce[0];
        if deb & PAD_LEFT != 0 && self.charsel.selection > 0 {
            self.charsel.selection -= 1;
            self.char_sel_change_selection();
        }
        if deb & PAD_RIGHT != 0 && self.charsel.selection < 1 {
            self.charsel.selection += 1;
            self.char_sel_change_selection();
        }
        if deb & PAD_CROSS != 0 || deb & PAD_START != 0 {
            self.effects.push(Effect::Log { text: format!("Leaving char select mode (Selection: {})", self.charsel.selection) });
            // todo: We need a pretty effect and an animation here!
            let opt = self.charsel.options[self.charsel.selection.clamp(0, 1) as usize];
            self.game_state.mode = INGAME_MODE;
            self.frogger.player[0].character = opt.character as u8;
            self.frogger.player[0].world_num = opt.world as u8;
            self.frogger.player[0].level_num = opt.level as u8;
            self.init_level(opt.world as u32, opt.level as u32);
        }
    }
}
