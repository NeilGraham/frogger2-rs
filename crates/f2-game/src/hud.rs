//! Port of `Common/hud.c`: the single-player arcade HUD (`arcadeHud`: lives, coins,
//! babies, clock, hard-mode time bar, "GO" countdown and sparkles), the in-game text
//! overlays (`InitInGameTextOverlays`) and the per-frame `UpDateOnScreenInfo`.
//!
//! Everything is made of [`crate::overlays`] sprite overlays and [`crate::textover`]
//! text overlays; [`HudState`] keeps their ids (the `arcadeHud` pointers). The HUD's
//! `sprintf` buffers are written straight into the overlays' text.
//!
//! The multiplayer HUD (`multiHud`: `InitMultiHUD`, `EnableMultiHUD`, `DisableMultiHUD`,
//! `UpDateMultiplayerInfo`) is at the end of this file.

use f2_math::{fmul, rsin};
use serde::{Deserialize, Serialize};

use crate::anim::SAMPLE_VOLUME;
use crate::define::mode::{DEMO_MODE, FRONTEND_MODE, INGAME_MODE};
use crate::define::{ARCADE_MODE, DIFFICULTY_HARD, SINGLEPLAYER, STORY_MODE};
use crate::ids::{OverlayId, TextOverlayId};
use crate::layout::worldid;
use crate::sprite::{SPRITE_ADDITIVE, SPRITE_SUBTRACTIVE};
use crate::textover::{strs, Font, TEXTOVERLAY_PAUSED, TEXTOVERLAY_SHADOW};
use crate::timer::Timer;
use crate::Game;

/// `MAX_HUD_SPARKLES`.
pub const MAX_HUD_SPARKLES: usize = 20;
/// `timeBarWidth` / `timeBarStep`.
pub const TIME_BAR_WIDTH: i32 = 1600;
pub const TIME_BAR_STEP: i32 = 16;
/// `fadeSpeed` (`Common/menus.c:842`) used by `INC_ALPHA`/`DEC_ALPHA`.
pub const FADE_SPEED: i32 = 10 << 12;
/// `spSpeeds[20]` (hud.c:329).
const SP_SPEEDS: [i32; 20] = [8, 3, 6, 7, 8, 3, 4, 6, 8, 7, 9, 6, 3, 9, 8, 6, 3, 8, 7, 6];
/// `storySequence[NUM_STORY_LEVELS - 1]` world / level (Halloween 3).
const LAST_STORY_WORLD: u32 = worldid::HALLOWEEN;
const LAST_STORY_LEVEL: u32 = 2;

/// `ARCADE_HUD arcadeHud` (`hud.h:27`), PC fields.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArcadeHud {
    pub s_x: u32,
    pub s_y: u32,
    pub s_w: u32,
    pub s_h: u32,
    pub s_time: u32,
    pub lives_over: Option<OverlayId>,
    pub lives_text: Option<TextOverlayId>,
    pub go_text: Option<TextOverlayId>,
    pub go_sparkles: [Option<OverlayId>; MAX_HUD_SPARKLES],
    pub coins_over: Option<OverlayId>,
    pub coins_back: Option<OverlayId>,
    pub babies_back: [Option<OverlayId>; 6],
    pub coins_text: Option<TextOverlayId>,
    pub auto_hop_over: Option<OverlayId>,
    pub quick_hop_over: Option<OverlayId>,
    pub time_face_over: Option<OverlayId>,
    pub time_head_over: Option<OverlayId>,
    pub time_hand_over: Option<OverlayId>,
    pub time_text_min: Option<TextOverlayId>,
    pub time_text_sec: Option<TextOverlayId>,
    pub time_text_hsec: Option<TextOverlayId>,
    pub collect_text: Option<TextOverlayId>,
    pub timed_out: u32,
    pub coin_zoom: Option<OverlayId>,
    pub time_bar: Option<OverlayId>,
    pub time_bak: Option<OverlayId>,
    pub time_bar_text: Option<TextOverlayId>,
    pub time_out_text: Option<TextOverlayId>,
}

/// `MULTI_HUD multiHud` (`hud.h:81`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultiHud {
    pub back_chars: [Option<OverlayId>; 4],
    pub penalise_text: [Option<TextOverlayId>; 4],
    pub time_text_min: Option<TextOverlayId>,
    pub time_text_sec: Option<TextOverlayId>,
    pub time_text_hsec: Option<TextOverlayId>,
    pub centre_text: Option<TextOverlayId>,
    pub trophies: [[Option<OverlayId>; 3]; 4],
    pub penal_over: [Option<OverlayId>; 4],
}

/// Globals of `hud.c`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HudState {
    pub arcade_hud: ArcadeHud,
    /// `multiHud`.
    pub multi_hud: MultiHud,
    /// `countdownString` (the text of `multiHud.centreText`; overlays copy their text, so
    /// writers call [`Game::set_countdown_string`]).
    pub countdown_string: String,
    /// `handAngle`.
    pub hand_angle: i32,
    /// `goTimer`.
    pub go_timer: Timer,
    /// `storeFrameCount`.
    pub store_frame_count: i32,
    /// `static int lastSecs` of `UpDateOnScreenInfo`.
    pub last_secs: i32,
    /// `countdownTimer`.
    pub countdown_timer: i8,
    /// `coinZoomX` / `coinZoomY` (set by the coin pickup, collect.c).
    pub coin_zoom_x: i32,
    pub coin_zoom_y: i32,
    /// `goText` points at `goStr` (hard / arcade) instead of a fixed string.
    pub go_text_is_go_str: bool,
    /// `pauseTitleString`.
    pub pause_title_string: String,
    pub xselect_text: Option<TextOverlayId>,
    pub pause_title_text: Option<TextOverlayId>,
    pub continue_text: Option<TextOverlayId>,
    pub controller_text: Option<TextOverlayId>,
    pub restart_text: Option<TextOverlayId>,
    pub quit_text: Option<TextOverlayId>,
    pub pos_text: Option<TextOverlayId>,
    pub levelname_text: Option<TextOverlayId>,
    pub garib_count: Option<TextOverlayId>,
    pub credit_count: Option<TextOverlayId>,
    pub baby_saved_text: Option<TextOverlayId>,
}

/// `INC_ALPHA(x, y)` (`menus.h:55`) on an overlay's `a`/`draw`.
pub(crate) fn inc_alpha(a: &mut u8, draw: &mut u8, y: i32, gs: i32) {
    let t = fmul(gs, FADE_SPEED) >> 12;
    *draw = 1;
    if (*a as i32) + t < y {
        *a = (*a as i32 + t) as u8;
    } else {
        *a = y as u8;
    }
}

/// `DEC_ALPHA(x)` (`menus.h:41`).
pub(crate) fn dec_alpha(a: &mut u8, draw: &mut u8, gs: i32) {
    let t = fmul(gs, FADE_SPEED) >> 12;
    if (*a as i32) > t {
        *a = (*a as i32 - t) as u8;
    } else {
        *a = 0;
        *draw = 0;
    }
}

impl Game {
    fn hud_text(&mut self, t: Option<TextOverlayId>, s: String) {
        if let Some(o) = self.textover.get_mut(t) {
            o.text = s;
        }
    }

    fn is_last_story_level(&self) -> bool {
        self.frogger.player[0].world_num as u32 == LAST_STORY_WORLD && self.frogger.player[0].level_num as u32 == LAST_STORY_LEVEL
    }

    fn level_times(&self) -> (u32, i32) {
        let w = self.frogger.player[0].world_num as usize;
        let l = self.frogger.player[0].level_num as usize;
        self.layout.world_visual_data.get(w).and_then(|wv| wv.level_visual_data.get(l)).map_or((0, 0), |lv| (lv.par_time, lv.difficult_time))
    }

    /// Port of `InitArcadeHUD` (Common/hud.c:114), PC branch.
    pub fn init_arcade_hud(&mut self) {
        let hard = self.game_state.difficulty == DIFFICULTY_HARD;
        let arcade = self.game_state.single == ARCADE_MODE;
        let frontend_world = self.frogger.player[0].world_num as u32 == worldid::FRONTEND;
        let (_, dt) = self.level_times();
        let mut h = ArcadeHud::default();
        let ov = &mut self.overlays;

        h.time_bar = ov.create_and_add_sprite_overlay(1200, 3600, None, 0, 100, 255, 0);
        if let Some(o) = ov.get_mut(h.time_bar) {
            o.num = 1;
        }
        h.time_bak = ov.create_and_add_sprite_overlay(1200 - 8, 3600 - 17, None, (TIME_BAR_WIDTH + 24) as i16, 100 + 34, 255, SPRITE_SUBTRACTIVE);
        if let Some(o) = ov.get_mut(h.time_bak) {
            o.a = 254;
            o.r = 128;
            o.g = 128;
            o.b = 128;
        }
        let time_bar_str = format!("{}:{:02}.{}0", dt / 600, (dt / 10) % 60, dt % 10);
        h.time_bar_text = self.textover.create_and_add_text_overlay(2048, 3300, &time_bar_str, true, 255, Font::Normal, TEXTOVERLAY_SHADOW);
        if self.game_state.mode == FRONTEND_MODE || self.game_state.mode == DEMO_MODE {
            self.hud.go_timer.init(0);
        } else {
            self.hud.go_timer.init(5);
        }
        if !hard || frontend_world {
            if let Some(o) = self.overlays.get_mut(h.time_bar) {
                o.draw = 0;
            }
            if let Some(o) = self.overlays.get_mut(h.time_bak) {
                o.draw = 0;
            }
            self.textover.disable(h.time_bar_text);
        }

        let ch = (self.frogger.player[0].character as usize).min(crate::frogger::FROG_NUMFROGS - 1);
        let icon = self.frogger.frog_pool(ch).icon;
        let ov = &mut self.overlays;
        h.lives_over = ov.create_and_add_sprite_overlay(100, 3332, Some(icon), 410, 546, 0xff, 0);

        if self.game_state.single != STORY_MODE {
            h.time_face_over = ov.create_and_add_sprite_overlay(3600, 3432, Some("CLK_FACE"), 410, 546, 0xff, 0);
            if let Some(o) = ov.get_mut(h.time_face_over) {
                o.num = 0;
            }
            h.time_hand_over = ov.create_and_add_sprite_overlay(3600, 3432, Some("CLK_HAND"), 410, 546, 0xff, 0);
            if let Some(o) = ov.get_mut(h.time_hand_over) {
                o.num = 1;
            }
            h.time_head_over = ov.create_and_add_sprite_overlay(3600, 3432, Some("CLK_HEAD"), 410, 546, 0xff, 0);
            if let Some(o) = ov.get_mut(h.time_head_over) {
                o.num = 2;
            }
            let t = &mut self.textover;
            h.time_text_min = t.create_and_add_text_overlay(3100, 3700, "00:", false, 255, Font::Normal, 0);
            h.time_text_sec = t.create_and_add_text_overlay(3100 + 200, 3700, "00:", false, 255, Font::Normal, 0);
            h.time_text_hsec = t.create_and_add_text_overlay(3100 + 450, 3700, "00", false, 255, Font::Normal, 0);
            if let Some(o) = t.get_mut(h.time_text_hsec) {
                o.scale = 3072;
            }
        }

        let ov = &mut self.overlays;
        h.auto_hop_over = ov.create_and_add_sprite_overlay(100, 3200, Some("AHOVER"), 180, 180, 0, 0);
        ov.disable(h.auto_hop_over);
        h.quick_hop_over = ov.create_and_add_sprite_overlay(350, 3200, Some("QHOVER"), 180, 180, 0, 0);
        ov.disable(h.quick_hop_over);

        h.coins_back = ov.create_and_add_sprite_overlay(3520, 130, Some("HUD_BGR"), 450, 460, 0xff, 0);
        if let Some(o) = ov.get_mut(h.coins_back) {
            o.num = 0;
        }
        let lives = format!("{}", self.frogger.player[0].lives as i32 as u32);
        h.lives_text = self.textover.create_and_add_text_overlay(520, 3650, &lives, false, 255, Font::Normal, TEXTOVERLAY_SHADOW);
        let coin_tex = if self.cheat_mad_garibs() { "RGARIB01" } else { "SCOIN0001" };
        let ov = &mut self.overlays;
        h.coins_over = ov.create_and_add_sprite_overlay(3624, 130, Some(coin_tex), 205, 273, 0xff, 0);
        if let Some(o) = ov.get_mut(h.coins_over) {
            o.num = 1;
        }
        h.coin_zoom = ov.create_and_add_sprite_overlay(3624, 130, Some(coin_tex), 205, 273, 0xff, 0);
        if let Some(o) = ov.get_mut(h.coin_zoom) {
            o.draw = 0;
            o.num = 1;
        }
        let nb = self.map.num_babies.clamp(0, 6) as usize;
        for i in 0..nb {
            h.babies_back[i] =
                ov.create_and_add_sprite_overlay((200 + i * 200) as i16, 160, Some("HUD_BGR"), ((4096 * 32) / 640) as i16, ((4096 * 32) / 480) as i16, 0xff, 0);
            if let Some(o) = ov.get_mut(h.babies_back[i]) {
                o.r = 64;
                o.g = 64;
                o.b = 64;
            }
        }

        h.collect_text = self.textover.create_and_add_text_overlay(4096 + 2048, 2048, "", true, 255, Font::Normal, TEXTOVERLAY_SHADOW);
        if let Some(o) = self.textover.get_mut(h.collect_text) {
            o.b = 0;
            o.g = 128;
            o.draw = 1;
        }
        // The collect.c view of arcadeHud.collectText.
        {
            let c = &mut self.collect.collect_text;
            c.text.clear();
            c.x_pos = 4096 + 2048;
            c.x_pos_to = 4096 + 2048;
            c.speed = 0;
            c.draw = 1;
        }
        self.collect.pause_timer.init(0);
        self.collect.pause_flag = 0;

        if (hard || arcade) && !frontend_world {
            h.go_text = self.textover.create_and_add_text_overlay(2048, 2048, "", true, 255, Font::Normal, TEXTOVERLAY_SHADOW);
            self.hud.go_text_is_go_str = true;
        } else {
            self.hud.go_text_is_go_str = false;
            if self.is_last_story_level() {
                h.go_text = self.textover.create_and_add_text_overlay(2048, 2048, "", true, 255, Font::Normal, 0);
            } else {
                let s = self.gamestring(strs::STR_COLLECTBABIES);
                h.go_text = self.textover.create_and_add_text_overlay(2048, 2048, &s, true, 255, Font::Normal, TEXTOVERLAY_SHADOW);
            }
        }

        let coins = format!("{:2}", self.frogger.player[0].num_spawn);
        h.coins_text = self.textover.create_and_add_text_overlay(3680, 370, &coins, false, 255, Font::Normal, TEXTOVERLAY_SHADOW);

        let s = self.gamestring(strs::STR_OUTOFTIME);
        h.time_out_text = self.textover.create_and_add_text_overlay(2048, 1900, &s, true, 255, Font::Normal, TEXTOVERLAY_SHADOW);
        if let Some(o) = self.textover.get_mut(h.time_out_text) {
            o.r = 0xff;
            o.g = 0;
            o.b = 0;
            o.a = 0;
        }
        self.textover.disable(h.time_out_text);
        h.timed_out = 0;

        if (!hard && !arcade) || frontend_world {
            let last = self.is_last_story_level();
            for i in 0..MAX_HUD_SPARKLES {
                // MSVC evaluates the call's arguments right to left: Random(350) first.
                let y = 1900 + self.rng.random(350);
                let x = 900 - self.rng.random(1800) + 2048;
                h.go_sparkles[i] = self.overlays.create_and_add_sprite_overlay(x as i16, y as i16, Some("FLASH2"), 100, 100, 0xff, SPRITE_ADDITIVE);
                if let Some(o) = self.overlays.get_mut(h.go_sparkles[i]) {
                    o.num = 1;
                    if last {
                        o.draw = 0;
                    }
                }
            }
        }
        self.hud.arcade_hud = h;
        if self.game_state.mode == FRONTEND_MODE {
            self.disable_hud();
        }
        if self.frogger.player[0].world_num as u32 == worldid::SUPERRETRO {
            let h = self.hud.arcade_hud.clone();
            self.overlays.disable(h.coins_back);
            self.overlays.disable(h.coins_over);
            self.textover.disable(h.coins_text);
        }
    }

    /// Port of `InitHUD` (Common/hud.c:459).
    pub fn init_hud(&mut self) {
        self.hud.store_frame_count = 0;
        self.hud.hand_angle = 0;
        if self.game_state.multi == SINGLEPLAYER {
            self.init_arcade_hud();
        } else {
            self.hud.arcade_hud = ArcadeHud::default();
            self.init_multi_hud();
        }
    }

    /// Port of `SparkleCoins` (Common/hud.c:469).
    pub fn sparkle_coins(&mut self) {
        let h = &mut self.hud.arcade_hud;
        h.s_x = 1100;
        h.s_y = 3800;
        h.s_w = 640;
        h.s_h = 192;
        h.s_time = self.act_frame_count.wrapping_add(30);
    }

    /// Port of `SparkleBabies` (Common/hud.c:478).
    pub fn sparkle_babies(&mut self) {
        let nb = self.map.num_babies;
        let h = &mut self.hud.arcade_hud;
        h.s_x = (3000 - nb * 192) as u32;
        h.s_y = 3864;
        h.s_w = ((nb - 1) * 192) as u32;
        h.s_h = 128;
        h.s_time = self.act_frame_count.wrapping_add(30);
    }

    fn set_overlay_draw(&mut self, ids: &[Option<OverlayId>], v: u8) {
        for &i in ids {
            if let Some(o) = self.overlays.get_mut(i) {
                o.draw = v;
            }
        }
    }

    fn set_text_draw(&mut self, ids: &[Option<TextOverlayId>], v: u8) {
        for &i in ids {
            if let Some(o) = self.textover.get_mut(i) {
                o.draw = v;
            }
        }
    }

    fn set_baby_icons_draw(&mut self, v: u8) {
        let nb = self.map.num_babies.max(0) as usize;
        for ic in self.babyfrog.baby_icons.iter_mut().take(nb).flatten() {
            ic.draw = v;
        }
    }

    /// Port of `DisableHUD` (Common/hud.c:487).
    pub fn disable_hud(&mut self) {
        if self.game_state.multi != SINGLEPLAYER {
            self.disable_multi_hud();
            return;
        }
        let h = self.hud.arcade_hud.clone();
        let nb = self.map.num_babies.clamp(0, 6) as usize;
        self.set_overlay_draw(&h.babies_back[..nb], 0);
        let hard = self.game_state.difficulty == DIFFICULTY_HARD;
        let frontend_world = self.frogger.player[0].world_num as u32 == worldid::FRONTEND;
        if (!hard && self.game_state.single != ARCADE_MODE) || frontend_world {
            self.set_overlay_draw(&h.go_sparkles, 0);
        }
        self.set_overlay_draw(&[h.coins_back, h.coins_over, h.lives_over, h.auto_hop_over, h.quick_hop_over], 0);
        self.set_text_draw(&[h.go_text, h.lives_text, h.coins_text, h.collect_text], 0);
        self.collect.collect_text.draw = 0;
        if self.game_state.single != STORY_MODE {
            self.set_overlay_draw(&[h.time_head_over, h.time_hand_over, h.time_face_over], 0);
            self.set_text_draw(&[h.time_text_min, h.time_text_sec, h.time_text_hsec], 0);
        }
        self.set_baby_icons_draw(0);
        if hard && !frontend_world {
            self.set_overlay_draw(&[h.time_bar, h.time_bak], 0);
            self.set_text_draw(&[h.time_bar_text], 0);
        }
    }

    /// Port of `EnableHUD` (Common/hud.c:528).
    pub fn enable_hud(&mut self) {
        if self.game_state.multi != SINGLEPLAYER {
            self.enable_multi_hud();
            return;
        }
        let h = self.hud.arcade_hud.clone();
        let nb = self.map.num_babies.clamp(0, 6) as usize;
        self.set_overlay_draw(&h.babies_back[..nb], 1);
        let hard = self.game_state.difficulty == DIFFICULTY_HARD;
        let w = self.frogger.player[0].world_num as u32;
        let l = self.frogger.player[0].level_num as u32;
        let frontend_world = w == worldid::FRONTEND;
        // NB: `worldNum != last.worldNum && levelNum != last.levelNum` (sic).
        if ((!hard && self.game_state.single != ARCADE_MODE) || frontend_world) && w != LAST_STORY_WORLD && l != LAST_STORY_LEVEL {
            self.set_overlay_draw(&h.go_sparkles, 1);
        }
        self.set_overlay_draw(&[h.coins_back, h.coins_over, h.lives_over, h.auto_hop_over, h.quick_hop_over], 1);
        self.set_text_draw(&[h.go_text, h.lives_text, h.coins_text, h.collect_text], 1);
        self.collect.collect_text.draw = 1;
        if self.game_state.single != STORY_MODE && !frontend_world {
            self.set_overlay_draw(&[h.time_head_over, h.time_hand_over, h.time_face_over], 1);
            self.set_text_draw(&[h.time_text_min, h.time_text_sec], 1);
        }
        self.set_baby_icons_draw(1);
        if w == worldid::SUPERRETRO {
            self.set_overlay_draw(&[h.coins_back, h.coins_over], 0);
            self.set_text_draw(&[h.coins_text], 0);
        }
        if hard && !frontend_world {
            self.set_overlay_draw(&[h.time_bar, h.time_bak], 1);
            if self.textover.get(h.time_bar_text).is_some_and(|t| t.a != 0) {
                self.set_text_draw(&[h.time_bar_text], 1);
            }
        }
    }

    /// The coin colour pulse at the top of `RunGameLoop` (Common/game.c:533).
    pub fn hud_coin_colours(&mut self) {
        if self.frogger.player[0].num_spawn != 25 {
            return;
        }
        let afc = self.act_frame_count as i32;
        let r = (127 + ((rsin(afc.wrapping_mul(4000)) + 4096) * 64) / 4096) as u8;
        let g = (127 + ((rcos_(afc.wrapping_mul(4000)) + 4096) * 64) / 4096) as u8;
        let h = self.hud.arcade_hud.clone();
        if let Some(o) = self.overlays.get_mut(h.coins_over) {
            o.r = r;
            o.g = g;
            o.b = 0;
        }
        if let Some(o) = self.textover.get_mut(h.coins_text) {
            o.r = r;
            o.g = g;
            o.b = 0;
        }
    }

    /// Port of `InitInGameTextOverlays` (Common/hud.c:1131).
    pub fn init_in_game_text_overlays(&mut self, _world_id: u32, _level_id: u32) {
        let s = self.gamestring(strs::STR_X_SELECT_OPTION);
        self.hud.xselect_text = self.textover.create_and_add_text_overlay(2048, 3400, &s, true, 255, Font::Small, TEXTOVERLAY_SHADOW | TEXTOVERLAY_PAUSED);
        self.hud.pause_title_string = self.gamestring(strs::STR_PAUSE_MODE);
        let s = self.hud.pause_title_string.clone();
        self.hud.pause_title_text = self.textover.create_and_add_text_overlay(2048, 400, &s, true, 255, Font::Small, TEXTOVERLAY_SHADOW | TEXTOVERLAY_PAUSED);
        let s = self.gamestring(strs::STR_CONTINUE);
        self.hud.continue_text = self.textover.create_and_add_text_overlay(2048, 1540, &s, true, 255, Font::Normal, TEXTOVERLAY_SHADOW | TEXTOVERLAY_PAUSED);
        let s = self.gamestring(strs::STR_RESTARTLEVEL);
        self.hud.restart_text = self.textover.create_and_add_text_overlay(2048, 1860, &s, true, 255, Font::Normal, TEXTOVERLAY_SHADOW | TEXTOVERLAY_PAUSED);
        let s = if self.game_state.mode == FRONTEND_MODE { self.gamestring(strs::STR_QUIT_TO_WINDOWS) } else { self.gamestring(strs::STR_QUIT) };
        self.hud.quit_text = self.textover.create_and_add_text_overlay(2048, 2180, &s, true, 255, Font::Normal, TEXTOVERLAY_SHADOW | TEXTOVERLAY_PAUSED);
        self.hud.pos_text = self.textover.create_and_add_text_overlay(2048, 640, "-----------------------", true, 255, Font::Small, 0);
        self.hud.levelname_text = self.textover.create_and_add_text_overlay(2048, 800, "Levelname Goes in here", true, 255, Font::Small, 0);
        let h = &self.hud;
        let ids = [h.controller_text, h.continue_text, h.xselect_text, h.pause_title_text, h.restart_text, h.quit_text, h.pos_text, h.levelname_text];
        for i in ids {
            self.textover.disable(i);
        }

        // Lives and score
        self.init_hud();

        // Garib and Credit counters
        self.hud.garib_count = self.textover.create_and_add_text_overlay(100, 205, "", false, 255, Font::Normal, 0);
        self.hud.credit_count = self.textover.create_and_add_text_overlay(150, 205, "", false, 255, Font::Normal, 0);

        // Baby display
        self.hud.baby_saved_text = self.textover.create_and_add_text_overlay(2048, 110, "BABY SAVED", true, 255, Font::Normal, 0);
        self.textover.disable(self.hud.baby_saved_text);
    }

    /// Hides the counters for the frontend / multiplayer (`InitLevel`, layout.c:907).
    pub fn hide_level_counters(&mut self) {
        let (g, c) = (self.hud.garib_count, self.hud.credit_count);
        self.set_text_draw(&[g, c], 0);
        self.hud.countdown_timer = 0;
        self.set_baby_icons_draw(0);
    }

    /// Port of `UpDateOnScreenInfo` (Common/hud.c:669).
    pub fn up_date_on_screen_info(&mut self) {
        let gs = self.game_speed;
        let afc = self.act_frame_count;
        let (par, dt) = self.level_times();
        let mut time_frames = (par.wrapping_mul(6)) as i32;
        let mut hard_time_frames = time_frames;
        let hard = self.game_state.difficulty == DIFFICULTY_HARD;
        let arcade = self.game_state.single == ARCADE_MODE;
        let frontend_world = self.frogger.player[0].world_num as u32 == worldid::FRONTEND;
        let h = self.hud.arcade_hud.clone();

        if self.game_state.mode == INGAME_MODE {
            let mut alpha = (afc as i32).wrapping_sub(100);

            if self.overlays.get(h.coin_zoom).is_some_and(|o| o.draw != 0) {
                let (cx, cy) = self.overlays.get(h.coins_over).map_or((0, 0), |o| (o.x_pos as i32, o.y_pos as i32));
                let (zx, zy) = (self.hud.coin_zoom_x, self.hud.coin_zoom_y);
                self.slide_sprite_overlay_to_pos(h.coin_zoom, cx, cy, zx, zy, 20);
                if let Some(z) = self.overlays.get_mut(h.coin_zoom) {
                    if z.x_pos as i32 == cx && z.y_pos as i32 == cy {
                        z.draw = 0;
                    }
                }
            }

            if hard && !frontend_world {
                if self.hud.go_timer.time == 0 {
                    if let Some(t) = self.textover.get_mut(h.go_text) {
                        t.a = 0;
                        t.draw = 0;
                    }
                }
            } else if afc > 100 {
                alpha = alpha.wrapping_mul(3);
                if alpha > 255 {
                    alpha = 255;
                    if let Some(t) = self.textover.get_mut(h.go_text) {
                        t.draw = 0;
                    }
                }
                if let Some(t) = self.textover.get_mut(h.go_text) {
                    t.a = (0xff - alpha) as u8;
                }
            }

            // Show and update powerup icons
            let pulse = ((127 + ((rsin((afc as i32).wrapping_mul(4000)) + 4096) << 6)) >> 12) as u8;
            let autohop = self.frogger.player[0].autohop.time;
            if let Some(o) = self.overlays.get_mut(h.auto_hop_over) {
                if autohop != 0 {
                    if autohop > 3 {
                        inc_alpha(&mut o.a, &mut o.draw, 0xff, gs);
                        o.draw = 1;
                    } else {
                        o.a = pulse;
                    }
                } else {
                    dec_alpha(&mut o.a, &mut o.draw, gs);
                    o.a = 0;
                    o.draw = 0;
                }
            }
            let quickhop = self.frogger.player[0].quickhop.time;
            if let Some(o) = self.overlays.get_mut(h.quick_hop_over) {
                if quickhop != 0 {
                    if quickhop > 5 {
                        inc_alpha(&mut o.a, &mut o.draw, 0xff, gs);
                        o.draw = 1;
                    } else {
                        o.a = pulse;
                    }
                } else {
                    dec_alpha(&mut o.a, &mut o.draw, gs);
                    o.draw = 0;
                }
            }

            // Sparklies
            if (!hard && !arcade) || frontend_world {
                for (i, sp) in h.go_sparkles.iter().enumerate() {
                    let Some(sa) = self.overlays.get(*sp).map(|o| o.a as i32) else { continue };
                    let a = (sa * 4096).wrapping_sub(gs.wrapping_mul(SP_SPEEDS[i]).wrapping_mul(4));
                    if a < 0 {
                        let go_a = self.textover.get(h.go_text).map_or(0, |t| t.a);
                        if go_a > 100 {
                            let x = 1000 - self.rng.random(2000) + 2048;
                            let y = 1900 + self.rng.random(350);
                            let o = self.overlays.get_mut(*sp).expect("sparkle");
                            o.a = go_a;
                            o.x_pos = x as i16;
                            o.y_pos = y as i16;
                        } else if let Some(o) = self.overlays.get_mut(*sp) {
                            o.draw = 0;
                        }
                    } else if let Some(o) = self.overlays.get_mut(*sp) {
                        o.a = (a / 4096) as u8;
                    }
                }
            }
            // (frameCheck colouring only fed the removed polysText overlay.)
        }

        time_frames = time_frames.wrapping_sub(afc as i32);
        hard_time_frames = hard_time_frames.wrapping_sub(afc as i32);
        if (hard || arcade) && !frontend_world && self.game_state.mode != DEMO_MODE {
            if self.hud.go_timer.time != 0 {
                if self.hud.go_timer.time > 1 {
                    self.hud.store_frame_count = afc as i32;
                    self.frogger.player[0].can_jump = 0;
                }
                let old_time = self.hud.go_timer.time;
                self.hud.go_timer.update2(-1, afc);
                if old_time != self.hud.go_timer.time {
                    match self.hud.go_timer.time {
                        2..=4 => self.play_sample(f2_formats::crc::crc("racehorn"), None, 0, SAMPLE_VOLUME, -1),
                        1 => {
                            self.frogger.player[0].can_jump = 1;
                            self.play_sample(f2_formats::crc::crc("racehorngo"), None, 0, SAMPLE_VOLUME, -1);
                        }
                        _ => {}
                    }
                }
                let go = match self.hud.go_timer.time {
                    3 => "2".to_string(),
                    2 => "1".to_string(),
                    1 | 0 => self.gamestring(strs::STR_GO),
                    _ => "3".to_string(),
                };
                if self.hud.go_text_is_go_str {
                    self.hud_text(h.go_text, go);
                }
            }
            time_frames = time_frames.wrapping_add(self.hud.store_frame_count);
            hard_time_frames = hard_time_frames.wrapping_add(self.hud.store_frame_count);
        }

        if self.game_state.single != STORY_MODE && !frontend_world {
            let (hsec, sec, min);
            if time_frames < 0 {
                time_frames = 0;
                if self.hud.arcade_hud.timed_out == 0 {
                    self.hud.last_secs = 0;
                    self.set_text_draw(&[h.time_text_hsec], 0);
                    self.play_sample(f2_formats::crc::crc("alarm"), None, 0, SAMPLE_VOLUME, -1);
                }
                self.hud.arcade_hud.timed_out = 1;
                for t in [h.time_text_min, h.time_text_sec] {
                    if let Some(o) = self.textover.get_mut(t) {
                        o.r = 255;
                        o.g = 0;
                        o.b = 0;
                    }
                }
                if let Some(o) = self.textover.get_mut(h.time_text_sec) {
                    o.a = (127 + ((4096 + rsin((afc as i32).wrapping_mul(150))) / (4096 / 64))) as u8;
                    o.scale = 4096;
                    o.x_pos = 3500;
                    o.y_pos = 3700;
                }
                hsec = format!("{:02}", ((time_frames * 100) / 60) % 100);
                sec = "0".to_string();
                min = String::new();
                for o in [h.time_hand_over, h.time_head_over] {
                    if let Some(o) = self.overlays.get_mut(o) {
                        o.angle /= 2;
                    }
                }
            } else {
                if self.game_state.mode != DEMO_MODE {
                    self.set_text_draw(&[h.time_text_hsec], 1);
                }
                if time_frames / 60 < 10 {
                    let value = time_frames % 60;
                    let alpha = (value * 0xff) / 60;
                    let scale = 4096 + (4096 - ((value * 4096) / 60));
                    if let Some(o) = self.textover.get_mut(h.time_text_sec) {
                        o.scale = scale;
                    }
                    for t in [h.time_text_min, h.time_text_sec] {
                        if let Some(o) = self.textover.get_mut(t) {
                            o.r = 255;
                            o.g = 0;
                            o.b = 0;
                        }
                    }
                    if let Some(o) = self.textover.get_mut(h.time_text_sec) {
                        o.a = alpha as u8;
                    }
                    let secs = (time_frames / 60) % 60;
                    if self.hud.last_secs != secs {
                        self.play_sample(f2_formats::crc::crc("clocktick"), None, 0, SAMPLE_VOLUME, -1);
                        self.hud.last_secs = secs;
                    }
                    hsec = format!("{:02}", (time_frames.wrapping_mul(100) / 60) % 100);
                    sec = format!("{secs:02}");
                    min = String::new();
                } else {
                    hsec = format!("{:02}", (time_frames.wrapping_mul(100) / 60) % 100);
                    sec = format!("{:02}", (time_frames / 60) % 60);
                    let m = (time_frames / (60 * 60)) % 60;
                    min = if m > 0 { format!("{m:2}:") } else { String::new() };
                }

                if self.hud.go_timer.time <= 1 {
                    self.hud.hand_angle = self.hud.hand_angle.wrapping_add(gs);
                    if self.hud.hand_angle > 4096 * 60 {
                        self.hud.hand_angle -= 4096 * 60;
                    }
                    let ha = self.hud.hand_angle / 60;
                    if let Some(o) = self.overlays.get_mut(h.time_hand_over) {
                        o.angle = ha;
                    }
                    let hand = self.overlays.get(h.time_hand_over).map_or(0, |o| o.angle);
                    if let Some(o) = self.overlays.get_mut(h.time_head_over) {
                        o.angle = rsin(hand) >> 2;
                    }
                }
                if hard_time_frames < 0 {
                    hard_time_frames = 0;
                }
            }
            self.hud_text(h.time_text_hsec, hsec);
            self.hud_text(h.time_text_sec, sec);
            self.hud_text(h.time_text_min, min);
            // timeTextMin->xPos = timeTextSec->xPos - (CalcStringWidth(min,font,scale)+2)*6.4
            if let (Some(sec_o), Some(min_o)) = (self.textover.get(h.time_text_sec).cloned(), self.textover.get(h.time_text_min).cloned()) {
                let w = self.textover.calc_string_width(&min_o.text, min_o.font, min_o.scale as f32 / 4096.0);
                let x = sec_o.x_pos as f32 - (w + 2) as f32 * 6.4;
                if let Some(o) = self.textover.get_mut(h.time_text_min) {
                    o.x_pos = f2_math::ftol32(x as f64) as i16;
                }
            }
        }

        let lives = format!("{}", self.frogger.player[0].lives as i32 as u32);
        self.hud_text(h.lives_text, lives);
        let coins = format!("{:2}", self.frogger.player[0].num_spawn);
        self.hud_text(h.coins_text, coins);

        if hard && !frontend_world {
            if self.hud.go_timer.time > 1 {
                let step = fmul(gs, TIME_BAR_STEP);
                if let Some(o) = self.overlays.get_mut(h.time_bar) {
                    if (o.width as i32) < TIME_BAR_WIDTH - step {
                        o.width = (o.width as i32 + step) as i16;
                    } else {
                        o.width = TIME_BAR_WIDTH as i16;
                    }
                }
            } else {
                if let Some(t) = self.textover.get_mut(h.time_bar_text) {
                    dec_alpha(&mut t.a, &mut t.draw, gs);
                }
                hard_time_frames = hard_time_frames.wrapping_add((6i32.wrapping_mul(dt)).wrapping_sub((par.wrapping_mul(6)) as i32));
                if hard_time_frames > 0 {
                    let d = 6i32.wrapping_mul(dt);
                    let w = if d != 0 { (hard_time_frames.wrapping_mul(TIME_BAR_WIDTH) / d).max(0) } else { 0 };
                    if let Some(o) = self.overlays.get_mut(h.time_bar) {
                        o.width = w as i16;
                    }
                } else {
                    self.frogger.player[0].can_jump = 0;
                    if let Some(t) = self.textover.get_mut(h.time_out_text) {
                        t.a = 255;
                        t.draw = 1;
                    }
                    self.game.end_level_timer.init(4);
                    self.set_time_for_level();
                    self.play_sample(f2_formats::crc::crc("alarm"), None, 0, SAMPLE_VOLUME, -1);
                    self.prepare_song(crate::pcaudio::AUDIOTRK_GAMEOVER, false);
                }
            }
        }
    }

    /// Copies the collect.c view of `arcadeHud.collectText` into the overlay before
    /// `PrintTextOverlays`, and the slide result back afterwards.
    pub(crate) fn sync_collect_text(&mut self, to_overlay: bool) {
        let id = self.hud.arcade_hud.collect_text;
        if id.is_none() {
            return;
        }
        if to_overlay {
            let c = self.collect.collect_text.clone();
            let text = if c.text.is_empty() { String::new() } else { self.language.gamestring_by_id(&c.text).to_string() };
            if let Some(o) = self.textover.get_mut(id) {
                o.text = text;
                o.x_pos = c.x_pos;
                o.x_pos_to = c.x_pos_to;
                o.speed = c.speed;
                o.draw = c.draw;
            }
        } else if let Some(o) = self.textover.get(id) {
            let (x, s) = (o.x_pos, o.speed);
            self.collect.collect_text.x_pos = x;
            self.collect.collect_text.speed = s;
        }
    }

    /// The overlay half of `DrawLoop` (PC/drawloop.cpp:322): `PrintSpriteOverlays(0)`,
    /// `PrintTextOverlays()`, `PrintSpriteOverlays(1)`, `PrintSpriteOverlays(2)`.
    pub fn print_overlays_state(&mut self) {
        self.print_sprite_overlays_state(0);
        self.sync_collect_text(true);
        self.print_text_overlays_state();
        self.sync_collect_text(false);
        self.print_sprite_overlays_state(1);
        self.print_sprite_overlays_state(2);
    }
}

// ------------------------------------------------------------------ multiplayer HUD

pub use crate::frontend::{BACK_CHARS_X, BACK_CHARS_Y, BACK_TEXT_X, BACK_TEXT_Y, BACK_WINS_TEXT_X, BACK_WINS_TEXT_Y};

impl Game {
    #[inline]
    fn num_frogs_hud(&self) -> usize {
        self.frogger.num_frogs.clamp(0, 4) as usize
    }

    /// `sprintf(countdownString, ...)`: sets the string and the centre text that shows it.
    pub fn set_countdown_string(&mut self, s: String) {
        if let Some(o) = self.textover.get_mut(self.hud.multi_hud.centre_text) {
            o.text = s.clone();
        }
        self.hud.countdown_string = s;
    }

    /// `INC_ALPHA(t, y)` on a text overlay.
    pub fn text_inc_alpha(&mut self, t: Option<TextOverlayId>, y: i32) {
        let gs = self.game_speed;
        if let Some(o) = self.textover.get_mut(t) {
            inc_alpha(&mut o.a, &mut o.draw, y, gs);
        }
    }

    /// `DEC_ALPHA(t)` on a text overlay.
    pub fn text_dec_alpha(&mut self, t: Option<TextOverlayId>) {
        let gs = self.game_speed;
        if let Some(o) = self.textover.get_mut(t) {
            dec_alpha(&mut o.a, &mut o.draw, gs);
        }
    }

    /// Port of `InitMultiHUD` (Common/hud.c:334).
    pub fn init_multi_hud(&mut self) {
        use crate::define::MULTIREMOTE;
        use crate::language::str_id::{STR_QUIT, STR_RESTARTLEVEL};
        use crate::multi::{MULTIMODE_BATTLE, MULTIMODE_COLLECT, MULTIMODE_RACE};

        let mut m = MultiHud::default();
        let overlay_name = match self.multi.multiplayer_mode {
            MULTIMODE_BATTLE => "",
            MULTIMODE_COLLECT => "00BABYIC",
            MULTIMODE_RACE => {
                let t = &mut self.textover;
                m.time_text_min = t.create_and_add_text_overlay(1536 - 102, 160 + 64, "00:", false, 255, Font::Normal, TEXTOVERLAY_SHADOW);
                m.time_text_sec = t.create_and_add_text_overlay(1536 + 410 - 102, 160 + 64, "00:", false, 255, Font::Normal, TEXTOVERLAY_SHADOW);
                m.time_text_hsec = t.create_and_add_text_overlay(1536 + 410 + 410 - 102, 160 + 64, "00", false, 255, Font::Normal, TEXTOVERLAY_SHADOW);
                if let Some(o) = t.get_mut(m.time_text_hsec) {
                    o.scale = 3072;
                }
                "HUD_PENALTY"
            }
            // `overlayName` is uninitialised for other modes; treated as none.
            _ => "",
        };

        if self.game_state.multi == MULTIREMOTE {
            let q = self.gamestring(STR_QUIT);
            self.frontend.menu_text[0] = self.textover.create_and_add_text_overlay(2048, 2400, &q, true, 0, Font::Normal, TEXTOVERLAY_SHADOW);
            self.frontend.menu_text[1] = None;
        } else {
            let r = self.gamestring(STR_RESTARTLEVEL);
            let q = self.gamestring(STR_QUIT);
            self.frontend.menu_text[0] = self.textover.create_and_add_text_overlay(2048, 2400, &r, true, 0, Font::Normal, TEXTOVERLAY_SHADOW);
            self.frontend.menu_text[1] = self.textover.create_and_add_text_overlay(2048, 2700, &q, true, 0, Font::Normal, TEXTOVERLAY_SHADOW);
            self.textover.disable(self.frontend.menu_text[1]);
        }
        self.textover.disable(self.frontend.menu_text[0]);

        let cs = self.hud.countdown_string.clone();
        m.centre_text = self.textover.create_and_add_text_overlay(2048, 900, &cs, true, 255, Font::Normal, TEXTOVERLAY_SHADOW);

        for i in 0..self.num_frogs_hud() {
            // The text is `penalString[i]`, refreshed below.
            m.penalise_text[i] = self.textover.create_and_add_text_overlay(BACK_TEXT_X[i] as i16, BACK_TEXT_Y[i] as i16, "00", false, 255, Font::Normal, TEXTOVERLAY_SHADOW);
            let ov = &mut self.overlays;
            if !overlay_name.is_empty() {
                // PC size 256x256 (PSX: 4096x256).
                m.penal_over[i] = ov.create_and_add_sprite_overlay((BACK_TEXT_X[i] - 256 - 64) as i16, BACK_TEXT_Y[i] as i16, Some(overlay_name), 256, 256, 255, 0);
                if self.multi.multiplayer_mode == MULTIMODE_COLLECT {
                    if let Some(o) = ov.get_mut(m.penal_over[i]) {
                        o.r = 60;
                        o.g = 255;
                        o.b = 60;
                    }
                }
            } else {
                m.penal_over[i] = None;
            }
            let ch = (self.frogger.player[i].character as usize).min(crate::frogger::FROG_NUMFROGS - 1);
            let icon = self.frogger.frog_pool(ch).icon;
            let ov = &mut self.overlays;
            m.back_chars[i] = ov.create_and_add_sprite_overlay(BACK_CHARS_X[i] as i16, BACK_CHARS_Y[i] as i16, Some(icon), 512, 512, 0xff, 0);
            for j in 0..3 {
                let x = if i == 0 || i == 3 { BACK_WINS_TEXT_X[i] + j as i32 * 220 } else { BACK_WINS_TEXT_X[i] - j as i32 * 220 };
                m.trophies[i][j] = ov.create_and_add_sprite_overlay(x as i16, BACK_WINS_TEXT_Y[i] as i16, Some("TRPHYGOLD"), 200, 200, 255, 0);
                ov.disable(m.trophies[i][j]);
            }
        }
        self.hud.multi_hud = m;
        // The overlays pointed at the static strings: copy their current values.
        self.up_date_multiplayer_info_text();
    }

    /// Port of `DisableMultiHUD` (Common/hud.c:408).
    pub fn disable_multi_hud(&mut self) {
        let m = self.hud.multi_hud.clone();
        if self.multi.multiplayer_mode == crate::multi::MULTIMODE_RACE {
            self.set_text_draw(&[m.time_text_min, m.time_text_sec, m.time_text_hsec], 0);
        }
        self.textover.disable(m.centre_text);
        for i in 0..self.num_frogs_hud() {
            self.overlays.disable(m.back_chars[i]);
            self.overlays.disable(m.penal_over[i]);
            self.textover.disable(m.penalise_text[i]);
            self.set_overlay_draw(&m.trophies[i], 0);
        }
    }

    /// Port of `EnableMultiHUD` (Common/hud.c:431).
    pub fn enable_multi_hud(&mut self) {
        let m = self.hud.multi_hud.clone();
        if self.multi.multiplayer_mode == crate::multi::MULTIMODE_RACE {
            self.set_text_draw(&[m.time_text_min, m.time_text_sec, m.time_text_hsec], 1);
        }
        if self.multi.multi_timer.time > 1 {
            self.textover.enable(m.centre_text);
        }
        for i in 0..self.num_frogs_hud() {
            self.overlays.enable(m.back_chars[i]);
            self.overlays.enable(m.penal_over[i]);
            self.textover.enable(m.penalise_text[i]);
            if let Some(o) = self.textover.get_mut(m.penalise_text[i]) {
                o.a = 255;
                o.x_pos = BACK_TEXT_X[i] as i16;
                o.x_pos_to = BACK_TEXT_X[i] as i16;
                o.y_pos = BACK_TEXT_Y[i] as i16;
                o.y_pos_to = BACK_TEXT_Y[i] as i16;
            }
            let wins = (self.multi.mpl[i].wins.max(0) as usize).min(3);
            self.set_overlay_draw(&m.trophies[i][..wins], 1);
        }
    }

    /// Port of `UpDateMultiplayerInfo` (Common/hud.c:581): countdown / round / "GO" centre
    /// text, per-player penalty or score text and the race clock. Display only.
    pub fn up_date_multiplayer_info(&mut self) {
        use crate::language::str_id::{STR_GO, STR_ROUND_NUM};
        use crate::language::{c_sprintf, FmtArg};
        let ct = self.hud.multi_hud.centre_text;
        let time = self.multi.multi_timer.time as u32;
        if time > 1 {
            if time < 5 {
                self.set_countdown_string(format!("{}", time - 1));
                if let Some(o) = self.textover.get_mut(ct) {
                    o.scale = 6144;
                    match time {
                        4 => (o.r, o.g, o.b) = (0xff, 0, 0),
                        3 => (o.r, o.g, o.b) = (0xff, 180, 30),
                        2 => (o.r, o.g, o.b) = (0, 0xff, 0),
                        _ => {}
                    }
                }
            } else {
                let mut round_num = 1i32;
                for i in 0..self.num_frogs_hud() {
                    round_num = round_num.wrapping_add(self.multi.mpl[i].wins as i32);
                }
                let s = c_sprintf(&self.gamestring(STR_ROUND_NUM), &[FmtArg::Int(round_num)]);
                self.set_countdown_string(s);
                if let Some(o) = self.textover.get_mut(ct) {
                    o.scale = 4096;
                }
            }
            self.textover.enable(ct);
        } else {
            if let Some(o) = self.textover.get_mut(ct) {
                (o.r, o.g, o.b) = (0xff, 0xff, 0xff);
            }
            if time == 1 {
                let s = c_sprintf(&self.gamestring(STR_GO), &[]);
                self.set_countdown_string(s);
            } else if self.multi.game_winner == -1 {
                self.textover.disable(ct);
            }
        }
        self.up_date_multiplayer_info_text();
    }

    /// The `switch (multiplayerMode)` half of `UpDateMultiplayerInfo`: `penalString` (race
    /// penalty seconds, collect score) and the race clock `timeString*`.
    fn up_date_multiplayer_info_text(&mut self) {
        use crate::multi::{MULTIMODE_BATTLE, MULTIMODE_COLLECT, MULTIMODE_RACE};
        let m = self.hud.multi_hud.clone();
        let n = self.num_frogs_hud();
        match self.multi.multiplayer_mode {
            MULTIMODE_RACE => {
                let mut time_frames: u32 = 0;
                for i in 0..n {
                    let s = format!("{:02}", (self.multi.mpl[i].penalty as i32) / 60);
                    self.hud_text(m.penalise_text[i], s);
                    if self.multi.mpl[i].timer > time_frames {
                        time_frames = self.multi.mpl[i].timer;
                    }
                }
                let tf = time_frames as i32;
                self.hud_text(m.time_text_hsec, format!("{:02}", (tf.wrapping_mul(100) / 60) % 100));
                self.hud_text(m.time_text_sec, format!("{:02}:", (tf / 60) % 60));
                self.hud_text(m.time_text_min, format!("{:2}:", (tf / (60 * 60)) % 60));
                // PC: xPos = sec.xPos +/- (CalcStringWidth(text, font, scale/4096) + 2) * 6.4
                let width = |g: &Game, t: Option<TextOverlayId>| {
                    g.textover.get(t).map_or(0.0, |o| (g.textover.calc_string_width(&o.text, o.font, o.scale as f32 / 4096.0) + 2) as f32 * 6.4)
                };
                let sec_x = self.textover.get(m.time_text_sec).map_or(0, |o| o.x_pos);
                let ws = width(self, m.time_text_sec);
                let wm = width(self, m.time_text_min);
                if let Some(o) = self.textover.get_mut(m.time_text_hsec) {
                    o.x_pos = (sec_x as f32 + ws) as i32 as i16;
                }
                if let Some(o) = self.textover.get_mut(m.time_text_min) {
                    o.x_pos = (sec_x as f32 - wm) as i32 as i16;
                }
            }
            MULTIMODE_BATTLE | MULTIMODE_COLLECT => {
                for i in 0..n {
                    let s = if self.multi.multiplayer_mode == MULTIMODE_BATTLE { String::new() } else { format!("{:02}", self.multi.mpl[i].score) };
                    self.hud_text(m.penalise_text[i], s);
                }
            }
            _ => {}
        }
    }
}

#[inline]
fn rcos_(x: i32) -> i32 {
    f2_math::rcos(x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::define::mode::INGAME_MODE;

    fn game(single: u8, difficulty: u8) -> Game {
        let mut g = Game::new();
        g.game_state.multi = SINGLEPLAYER;
        g.game_state.single = single;
        g.game_state.difficulty = difficulty;
        g.game_state.mode = INGAME_MODE;
        g.textover.init_text_overlay_linked_list(40);
        g.overlays.init_sprite_overlay_linked_list();
        g.frogger.player[0].lives = 3;
        g
    }

    #[test]
    fn arcade_hud_layout_and_sparkles() {
        let mut g = game(STORY_MODE, crate::define::DIFFICULTY_NORMAL);
        let seed = g.rng;
        g.init_in_game_text_overlays(0, 0);
        // 20 sparkles x 2 Random() calls
        let mut r = seed;
        let mut ys = vec![];
        for _ in 0..20 {
            let y = 1900 + r.random(350);
            let x = 900 - r.random(1800) + 2048;
            ys.push((x, y));
        }
        assert_eq!(g.rng, r);
        let h = g.hud.arcade_hud.clone();
        let s0 = g.overlays.get(h.go_sparkles[0]).unwrap();
        assert_eq!((s0.x_pos as i32, s0.y_pos as i32), ys[0]);
        // story mode: no clock
        assert!(h.time_text_sec.is_none());
        assert_eq!(g.textover.get(h.lives_text).unwrap().text, "3");
        assert_eq!(g.textover.get(h.coins_text).unwrap().text, " 0");
        assert!(g.textover.get(g.hud.pause_title_text).unwrap().draw == 0);
        // go text fades out after frame 100
        for f in 0..200 {
            g.act_frame_count = f;
            g.up_date_on_screen_info();
        }
        let go = g.textover.get(h.go_text).unwrap();
        assert_eq!((go.a, go.draw), (0, 0));
    }

    #[test]
    fn arcade_countdown_and_clock() {
        let mut g = game(ARCADE_MODE, crate::define::DIFFICULTY_NORMAL);
        g.layout.world_visual_data[0].level_visual_data[0].par_time = 300; // 30 s
        g.init_in_game_text_overlays(0, 0);
        let h = g.hud.arcade_hud.clone();
        assert!(g.hud.go_text_is_go_str);
        assert_eq!(g.hud.go_timer.time, 5);
        g.act_frame_count = 1;
        g.up_date_on_screen_info();
        assert_eq!(g.frogger.player[0].can_jump, 0);
        assert_eq!(g.textover.get(h.go_text).unwrap().text, "3");
        let mut f = 1;
        while g.hud.go_timer.time > 1 {
            f += 1;
            g.act_frame_count = f;
            g.up_date_on_screen_info();
            assert!(f < 1000);
        }
        assert_eq!(g.frogger.player[0].can_jump, 1);
        // 30 s on the clock after the countdown
        assert_eq!(g.textover.get(h.time_text_sec).unwrap().text, "30");
        for _ in 0..61 {
            f += 1;
            g.act_frame_count = f;
            g.up_date_on_screen_info();
        }
        assert_eq!(g.textover.get(h.time_text_sec).unwrap().text, "28");
        // run out of time
        for _ in 0..2000 {
            f += 1;
            g.act_frame_count = f;
            g.up_date_on_screen_info();
        }
        assert_eq!(g.hud.arcade_hud.timed_out, 1);
        assert_eq!(g.textover.get(h.time_text_sec).unwrap().text, "0");
        assert_eq!(g.textover.get(h.time_text_hsec).unwrap().draw, 0);
    }

    #[test]
    fn powerup_icon_fades() {
        let mut g = game(STORY_MODE, crate::define::DIFFICULTY_NORMAL);
        g.init_in_game_text_overlays(0, 0);
        let h = g.hud.arcade_hud.clone();
        g.frogger.player[0].autohop.time = 10;
        for f in 0..10 {
            g.act_frame_count = f;
            g.up_date_on_screen_info();
        }
        let o = *g.overlays.get(h.auto_hop_over).unwrap();
        assert_eq!((o.a, o.draw), (100, 1));
        g.frogger.player[0].autohop.time = 0;
        g.up_date_on_screen_info();
        let o = *g.overlays.get(h.auto_hop_over).unwrap();
        assert_eq!((o.a, o.draw), (0, 0));
    }
}
