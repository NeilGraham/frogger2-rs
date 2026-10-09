//! Port of the in-game frame of `Common/game.c` (`RunGameLoop`) and the frame
//! bookkeeping of `GameLoop` (`Common/frontend.c:144`), for the modules ported so far.

use serde::{Deserialize, Serialize};

use crate::actor2::orientate_ss;
use crate::anim::SAMPLE_VOLUME;
use crate::define::mode::{DEMO_MODE, FRONTEND_MODE, GAMEOVER_MODE, INGAME_MODE, LEVELCOMPLETE_MODE, PAUSE_MODE};
use crate::define::tilestate;
use crate::define::SINGLEPLAYER;
use crate::effects::Effect;
use crate::frogger::{
    FROG_ANIM_BIGHEAD, FROG_ANIM_BREATHE, FROG_ANIM_DANCE4, FROG_ANIM_FALL, FROG_ANIM_FALLLAND, FROG_ANIM_SOMERSAULT, FROG_ANIM_VICTORY, FROG_BREATHE_SPEED,
    FROG_HOPPER, MAX_IDLE_TIME,
};
use crate::frogmove::*;
use crate::layout::worldid;
use crate::levplay::*;
use crate::multi::MULTIMODE_BATTLE;
use crate::timer::Timer;
use crate::tongue::{TONGUE_IDLE, TONGUE_NONE, TONGUE_SEARCHING};
use crate::Game;

/// `LEVELID_SPACE2` (`layout.h:275`).
pub const LEVELID_SPACE2: u32 = 1;
/// `NO_MULTI_LEV` (`layout.h:20`).
pub const NO_MULTI_LEV: u32 = 0;

/// Globals of `game.c` used by the ported part of the loop.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameLoopState {
    /// `endLevelTimer`.
    pub end_level_timer: Timer,
    /// `startCam`.
    pub start_cam: i32,
    /// `scoreTimer`.
    pub score_timer: Timer,
    /// `modeTimer`.
    pub mode_timer: Timer,
    /// `levelTime`.
    pub level_time: u32,
    /// `doubleQueue[4]`: a double jump requested before the superhop apex.
    pub double_queue: [i8; 4],
    /// `GameProcessController`'s `static u16 button[4]`.
    pub button: [u16; 4],
}

/// `INPUT_POLLPAUSE` (`game.c:79`).
pub const INPUT_POLLPAUSE: u8 = 10;

impl Game {
    /// Port of the per-mode dispatch of `GameLoop` (Common/frontend.c:144) for the
    /// modes ported so far, plus its `frameCount++` / `inputPause` bookkeeping.
    pub fn game_loop(&mut self) {
        // TODO(port): UpdateCheatText; other modes (frontend, pause, level complete, ...).
        match self.game_state.mode {
            INGAME_MODE => self.run_game_loop(),
            DEMO_MODE => self.run_demo_mode(),
            _ => {}
        }

        self.frame_count = self.frame_count.wrapping_add(1);

        // ds - what the hell?
        let mut i = self.frogger.num_frogs;
        while i > 0 {
            i -= 1;
            let p = &mut self.frogger.player[i as usize];
            if p.input_pause > 0 {
                p.input_pause = 0;
            }
        }
    }

    /// Port of `RunGameLoop` (Common/game.c:529), parts ported so far.
    pub fn run_game_loop(&mut self) {
        // TODO(port): arcadeHud coin colours when numSpawn == 25 (overlays wave).

        // setup for frogger point of interest
        self.frogger.p_oi_distance = 500000 << 12;
        self.frogger.point_of_interest = None;

        if self.game.end_level_timer.time != 0 {
            // TODO(port): CheckEOLLoopTrack (audio).
            let afc = self.act_frame_count;
            self.game.end_level_timer.update(-1, afc);
            if self.game.end_level_timer.time == 0 {
                // TODO(port): cheatCombos[CHEAT_SKIP_LEVEL].state = 0; camDist (cam wave);
                // StartLevelComplete (level-complete wave).
                self.game_state.mode = LEVELCOMPLETE_MODE;
                self.game.level_time = self.act_frame_count;
                self.game.mode_timer.init(8);
                return;
            }

            self.update_babies();
            if self.frogger.player[0].world_num as u32 != worldid::SPACE || self.frogger.player[0].level_num as u32 != LEVELID_SPACE2 {
                self.update_platforms();
                self.update_enemies();
            }
            self.update_special_effects();
            return;
        }

        // TODO(port): cheatCombos[CHEAT_SKIP_LEVEL].state (cheats).
        let w = self.frogger.player[0].world_num as usize;
        let l = self.frogger.player[0].level_num as usize;
        let multi_part = self.layout.world_visual_data.get(w).and_then(|wv| wv.level_visual_data.get(l)).map_or(NO_MULTI_LEV, |lv| lv.multi_part_level);
        let nb = self.map.num_babies;
        if self.babyfrog.babies_saved as i32 == nb && nb != 0 && multi_part == NO_MULTI_LEV {
            self.game.end_level_timer.init(4);
            // TODO(port): SetTimeForLevel (story/records wave).
            self.prepare_song(crate::pcaudio::AUDIOTRK_LEVELCOMPLETE, false); // ambient
            if let Some(f) = self.frogger.frog[0] {
                self.actor_animate(f, FROG_ANIM_VICTORY, false, false, 100, false);
                self.actor_animate(f, FROG_ANIM_DANCE4, true, true, 70, false);
            }
            return;
        }

        if self.game.start_cam != 0 || self.frame_count < 2 {
            // TODO(port wave D): SetVectorFF(&currCamSource,&camSource); currCamTarget.
            self.game.start_cam = 0;
        }
        self.update_camera_position();
        // TODO(port wave D): SetCamFF(currCamSource, currCamTarget).

        if self.frame_count > 15 {
            for i in 0..self.frogger.num_frogs.max(0) {
                self.game_process_controller(i);
            }
        }

        for i in 0..self.frogger.num_frogs.max(0) as usize {
            let Some(f) = self.frogger.frog[i] else { continue };
            self.update_frogger_pos(i as i32);

            if self.frogger.player[i].frog_state & FROGSTATUS_ISDEAD == 0 {
                self.actor2.actors[f].draw = 1;
                if self.frogger.player[i].safe.time != 0 {
                    if self.act_frame_count & 1 != 0 {
                        self.create_ring_effect(i as i32);
                    }
                    let afc = self.act_frame_count;
                    self.frogger.player[i].safe.update(-1, afc);
                } else if self.frogger.player[i].frog_state & FROGSTATUS_ISTELEPORTING != 0 && self.act_frame_count & 1 != 0 {
                    self.create_ring_effect(i as i32);
                }

                if self.frogger.player[i].idle_enable != 0 && self.game_state.multi == SINGLEPLAYER {
                    let p = &mut self.frogger.player[i];
                    p.idle_time = p.idle_time.wrapping_sub(self.game_speed);
                    if p.idle_time < 4096 {
                        self.frogger_idle_anim(i as i32);
                        let r = self.rng.random(300);
                        self.frogger.player[i].idle_time = (400 + r) << 12;
                        if self.rng.random(4) == 0 {
                            self.effects.push(Effect::PlayVoice { player: i as u8, uid: f2_formats::crc::crc("frogbored") });
                        }
                    }
                }
            }
        }

        self.update_platforms();
        self.update_enemies();
        self.update_special_effects();
        // TODO(port wave D): UpdateEvents().
        if self.game_state.mode != PAUSE_MODE {
            self.update_ambient_sounds(); // ambient
        }
        self.process_collectables();

        if self.game_state.multi == SINGLEPLAYER {
            if self.game_state.mode != LEVELCOMPLETE_MODE {
                // TODO(port): UpDateOnScreenInfo() (overlays wave).
            }
        } else {
            self.run_multiplayer();
        }
    }

    /// Port of `StartPauseMenu` (Common/menus.c:188), the game-state part.
    pub fn start_pause_menu(&mut self) {
        self.game_state.old_mode = self.game_state.mode;
        self.game_state.mode = PAUSE_MODE;
        // TODO(port): pause menu state, PauseAudio, text overlays (menus wave).
    }

    /// Port of `StartGameOver` (Common/frontend.c:446), the game-state part.
    pub fn start_game_over(&mut self) {
        self.game_state.mode = GAMEOVER_MODE;
        self.game.mode_timer.init(9);
        // TODO(port): game-over text overlays, music (frontend wave).
    }

    /// Port of `GameProcessController` (Common/game.c:115): pad input -> frog movement
    /// requests. `debugKeys` (camera turning, baby warp) is off in retail and dropped.
    pub fn game_process_controller(&mut self, pl: i32) {
        let p = pl as usize;
        if self.cam.cam_control_mode != 0 {
            return;
        }
        if self.game_state.multi != SINGLEPLAYER && self.multi.multiplayer_mode == MULTIMODE_BATTLE {
            return;
        }

        let pad = self.levplay.pad_data;
        self.game.button[p] = pad.digital[p];
        let button = self.game.button[p];

        // The only thing we can do when dead is press start
        if (self.game_state.multi == SINGLEPLAYER || self.frogger.player[p].frog_state & FROGSTATUS_ISDEAD == 0) && pad.debounce[p] & PAD_START != 0 {
            self.frogger.pause_controller = pl;
            self.start_pause_menu();
            return;
        }

        if self.frogger.player[p].stun.time != 0 {
            let afc = self.act_frame_count;
            self.frogger.player[p].stun.update(-1, afc);
            return;
        } else if self.frogger.player[p].dead.time != 0 || self.frogger.player[p].frog_state & FROGSTATUS_ISDEAD != 0 {
            return;
        }

        self.frogger.player[p].has_jumped = 0;

        // check if frog is using extended hop ability
        if self.frogger.player[p].is_super_hopping != 0 {
            self.frogger.player[p].can_jump = 0;
        }

        if self.frogger.player[p].has_double_jumped != 0 {
            let pp = &self.frogger.player[p];
            if button & PAD_CROSS != 0 && pp.jump_time > pp.jump_multiplier / 2 {
                if let Some(f) = self.frogger.frog[p] {
                    self.start_animate_actor(f, FROG_ANIM_FALL, true, false, 256, true);
                }
                self.frogger.player[p].frog_state |= FROGSTATUS_ISFLOATING;
            } else {
                if self.frogger.player[p].frog_state & FROGSTATUS_ISFLOATING != 0 {
                    if let Some(f) = self.frogger.frog[p] {
                        self.actor_animate(f, FROG_ANIM_FALLLAND, false, false, 128, false);
                    }
                }
                self.frogger.player[p].frog_state &= !FROGSTATUS_ISFLOATING;
            }
        }

        // u/r/d/l in the original's order
        const DIRS: [(u16, u32, u32, i32); 4] = [
            (PAD_UP, FROGSTATUS_ISWANTINGSUPERHOPU, FROGSTATUS_ISWANTINGU, MOVE_UP),
            (PAD_RIGHT, FROGSTATUS_ISWANTINGSUPERHOPR, FROGSTATUS_ISWANTINGR, MOVE_RIGHT),
            (PAD_DOWN, FROGSTATUS_ISWANTINGSUPERHOPD, FROGSTATUS_ISWANTINGD, MOVE_DOWN),
            (PAD_LEFT, FROGSTATUS_ISWANTINGSUPERHOPL, FROGSTATUS_ISWANTINGL, MOVE_LEFT),
        ];
        for (bit, super_flag, flag, mv) in DIRS {
            let pp = &self.frogger.player[p];
            if (pad.debounce[p] & bit != 0 || (pad.digital[p] & bit != 0 && pp.autohop.time != 0)) && pp.can_jump != 0 {
                if self.frogger.player[p].input_pause == 0 {
                    let frontend = self.game_state.mode == FRONTEND_MODE;
                    let pp = &mut self.frogger.player[p];
                    pp.input_pause = INPUT_POLLPAUSE;
                    pp.can_jump = 0;
                    pp.has_jumped = 1;
                    if frontend {
                        pp.frog_state |= super_flag;
                        pp.is_super_hopping = 1;
                        self.frogmove.old_frog_facing[p] = self.frogmove.frog_facing[p];
                        self.frogmove.frog_facing[p] = (self.cam.cam_facing[p] + mv) & 3;
                    } else {
                        pp.frog_state |= flag;
                    }
                    // update player idleTime
                    let pp = &mut self.frogger.player[p];
                    pp.idle_time = MAX_IDLE_TIME << 12;
                    pp.extended_hop_dir = mv as u8;
                }
                break;
            }
        }

        // Do not allow superhop if in retro mode
        let pp = &self.frogger.player[p];
        if ((self.game.double_queue[p] != 0 && pp.jump_time > 2048) || pad.debounce[p] & PAD_CROSS != 0)
            && self.frogger.player[0].world_num as u32 != worldid::SUPERRETRO
            && self.game_state.mode != FRONTEND_MODE
            && !self.frogmove.cameo_mode
        {
            let mut dir = pp.extended_hop_dir as i32;
            if button & PAD_UP != 0 {
                dir = MOVE_UP;
            } else if button & PAD_DOWN != 0 {
                dir = MOVE_DOWN;
            } else if button & PAD_LEFT != 0 {
                dir = MOVE_LEFT;
            } else if button & PAD_RIGHT != 0 {
                dir = MOVE_RIGHT;
            }

            // update player idleTime
            self.frogger.player[p].idle_time = MAX_IDLE_TIME << 12;

            // ------------------- DOUBLE JUMP -------------------------
            let pp = &self.frogger.player[p];
            let dest_state = self.frogmove.dest_tile[p].map(|t| self.map.tile(t).state);
            if pp.is_super_hopping != 0 && pp.has_double_jumped == 0 && dest_state.is_some_and(|s| s != tilestate::NOSUPER && s != tilestate::NOSUPERHOT) {
                if self.game.double_queue[p] == 0 && pp.jump_time < pp.jump_multiplier {
                    self.game.double_queue[p] = 1;
                    return;
                }
                self.game.double_queue[p] = 0;

                self.play_sample(f2_formats::crc::crc("doublehop"), None, 0, SAMPLE_VOLUME, -1);

                // we have to keep track here of the previous tile so if it fails we don't
                // just sit in mid-air...
                let old = self.frogmove.curr_tile[p];
                let old_cam_facing = self.cam.cam_facing[p];
                let (ct, dt) = (self.frogmove.curr_tile[p].expect("currTile"), self.frogmove.dest_tile[p].expect("destTile"));
                self.cam.cam_facing[p] = get_tiles_matching_direction(self.map.tile(ct), self.cam.cam_facing[p], self.map.tile(dt));
                self.frogmove.curr_tile[p] = Some(dt);
                self.frogmove.frog_facing[p] = (self.cam.cam_facing[p] + dir) & 3;

                // player is superhopping - make frog double jump in desired direction
                if self.move_to_requested_destination(dir, pl) {
                    self.frogger.player[p].has_double_jumped = 1;
                    self.frogger.player[p].can_jump = 0;
                    let cur = self.map.tile(self.frogmove.curr_tile[p].expect("currTile")).dir_vector[self.frogmove.frog_facing[p] as usize & 3];
                    let old_normal = self.map.tile(old.expect("currTile")).normal;
                    if let Some(f) = self.frogger.frog[p] {
                        orientate_ss(&mut self.actor2.actors[f].actor.q_rot, &cur, &old_normal);
                    }
                    self.frogger.player[p].extended_hop_dir = dir as u8;
                    if let Some(f) = self.frogger.frog[p] {
                        self.actor_animate(f, FROG_ANIM_SOMERSAULT, false, false, 75, false);
                    }
                } else {
                    self.frogger.player[p].is_super_hopping = 1;
                    // Restore old state :oP
                    // ds - hAck! Jump to the platform
                    if self.platform.curr_platform[p].is_some() {
                        self.platform.dest_platform[p] = self.platform.curr_platform[p];
                        self.frogger.player[p].frog_state |= FROGSTATUS_ISJUMPINGTOPLATFORM;
                    }
                    self.frogmove.dest_tile[p] = self.frogmove.curr_tile[p];
                    self.frogmove.curr_tile[p] = old;
                    self.cam.cam_facing[p] = old_cam_facing;
                    self.frogger.player[p].can_jump = 0;
                }
            } else if pp.is_super_hopping == 0 && pp.input_pause == 0 && self.game_state.mode != FRONTEND_MODE {
                // Check if the player can jump.. if not, check if we're just hopping..
                // .. but only if we're not sliding! Good heavens.
                if pp.can_jump != 0
                    || (pp.frog_state & (FROGSTATUS_ISJUMPINGTOTILE | FROGSTATUS_ISJUMPINGTOPLATFORM) != 0 && pp.frog_state & FROGSTATUS_ISSLIDING == 0)
                {
                    // frog is wanting superhop
                    let pp = &mut self.frogger.player[p];
                    pp.is_super_hopping = 1;
                    pp.has_jumped = 1;
                    pp.has_double_jumped = 0;
                    pp.input_pause = INPUT_POLLPAUSE;
                    match pp.extended_hop_dir as i32 {
                        MOVE_UP => pp.frog_state |= FROGSTATUS_ISWANTINGSUPERHOPU,
                        MOVE_LEFT => pp.frog_state |= FROGSTATUS_ISWANTINGSUPERHOPL,
                        MOVE_DOWN => pp.frog_state |= FROGSTATUS_ISWANTINGSUPERHOPD,
                        MOVE_RIGHT => pp.frog_state |= FROGSTATUS_ISWANTINGSUPERHOPR,
                        _ => {}
                    }
                }
            }
        }

        if self.game_state.multi == SINGLEPLAYER && self.frogger.player[p].can_jump != 0 {
            if pad.debounce[p] & PAD_CIRCLE != 0 && self.tongue.tongue[p].flags & TONGUE_IDLE != 0 {
                // want to use tongue
                self.tongue.tongue[p].flags = TONGUE_NONE | TONGUE_SEARCHING;
                self.frogger.player[p].idle_time = MAX_IDLE_TIME << 12;
            }

            if pad.debounce[p] & PAD_SQUARE != 0 && self.frogger.player[p].is_croaking.time == 0 {
                self.frogger.player[p].frog_state |= FROGSTATUS_ISCROAKING;
                self.frogger.player[p].is_croaking.init(2);
                if self.frogger.player[p].character != FROG_HOPPER {
                    if let Some(f) = self.frogger.frog[p] {
                        self.actor_animate(f, FROG_ANIM_BIGHEAD, false, false, 64, false);
                        self.actor_animate(f, FROG_ANIM_BREATHE, true, true, FROG_BREATHE_SPEED, false);
                    }
                }
                self.frogger.player[p].idle_time = MAX_IDLE_TIME << 12;
            }
        }
        // debugKeys (PAD_L2/R2 camera turn, PAD_R1 baby warp) dropped: off in retail.
    }
}
