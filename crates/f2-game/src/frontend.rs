//! Port of `Common/frontend.c`: the per-mode `GameLoop` state machine, the level-complete
//! sequence (times, trophies, par times and best names, coin counting, story progression
//! and level/world unlocking), game over, "the end", and the multiplayer win screens.
//!
//! Text/sprite overlays are wave E (`textover.c`/`overlays.c`); until merged, the overlays
//! this file creates are kept as [`UiOverlay`]s in [`FrontendState`] and moved by
//! [`Game::update_ui_overlays`] (the motion of `PrintTextOverlays` / `PrintSpriteOverlays`,
//! which runs while drawing). Calls into modules owned by other waves go through the
//! clearly marked hook functions at the end of this file.

use f2_math::{fmul, magnitude_f, make_unit, rcos, rsin, FVector, SVector};
use serde::{Deserialize, Serialize};

use crate::anim::SAMPLE_VOLUME;
use crate::babyfrog::{BABY_ANIM_COLLECT, BABY_ANIM_COLLECTHOLD, BABY_ANIM_HOP, BABY_ANIM_WAVE};
use crate::define::mode::*;
use crate::define::{DIFFICULTY_HARD, MULTILOCAL, SINGLEPLAYER, STORY_MODE};
use crate::effects::Effect;
use crate::frogger::*;
use crate::frogmove::FROGSTATUS_ISDEAD;
use crate::ids::{ActorId, TextOverlayId};
use crate::language::str_id::*;
use crate::language::{c_sprintf, FmtArg};
use crate::layout::{worldid, LEVELID_FRONTEND1, LEVEL_CLOSED, LEVEL_OPEN, WORLD_CLOSED, WORLD_OPEN};
use crate::levplay::{PAD_CROSS, PAD_DOWN, PAD_UP};
use crate::multi::{MULTIMODE_BATTLE, MULTIMODE_COLLECT, MULTIMODE_RACE};
use crate::story::{FMV_BLITZ_LOGO, FMV_HASBRO_LOGO, FMV_INTRO, FMV_VICTORY, NUM_STORY_LEVELS, STORY_SEQUENCE};
use crate::textover::Font;
use crate::timer::Timer;
use crate::Game;

// `levCompleteState` values (`frontend.c:73`).
pub const LEV_COMPLETE_DROPPING_FROGS: i32 = 0;
pub const LEV_COMPLETE_COUNTING_COINS: i32 = 1;
pub const LEV_COMPLETE_SLIDING_TROPHY: i32 = 2;
pub const LEV_COMPLETE_ENTER_NAME: i32 = 3;
pub const LEV_COMPLETE_MENU: i32 = 4;
pub const LEV_COMPLETE_FADING_OUT: i32 = 5;

// `MULTIGAMEWIN.mode` (`frontend.c:495`).
pub const MGW_MOVING: i8 = 0;
pub const MGW_COUNTING: i8 = 1;
pub const MGW_FLASHING: i8 = 2;
pub const MGW_FINISHED: i8 = 3;

/// `MULTI_ROUND_DRAW` (`multi.h`).
pub const MULTI_ROUND_DRAW: i32 = -2;

/// `numLives[3]` (`frontend.c:390`): EASY, NORMAL, HARD.
pub const NUM_LIVES: [i16; 3] = [10, 5, 5];

/// `NAME_LENGTH` (`general.h:31`).
pub const NAME_LENGTH: usize = 3;

/// `MAX_SPARKLES` (`frontend.c:1153`).
pub const MAX_SPARKLES: usize = 15;

/// `trophyName[3]` (`frontend.c:332`).
pub const TROPHY_NAME: [&str; 3] = ["TRPHYGOLD", "TRPHYSILV", "TRPHYBRONZ"];

/// `frogPos[6]` (`frontend.c:1118`): babies 0..5, then Frogger.
pub const FROG_POS: [SVector; 6] = [
    SVector { vx: 500, vy: 0, vz: 1000, pad: 0 },
    SVector { vx: 300, vy: 0, vz: 1200, pad: 0 },
    SVector { vx: 0, vy: 0, vz: 1500, pad: 0 },
    SVector { vx: -300, vy: 0, vz: 1200, pad: 0 },
    SVector { vx: -500, vy: 0, vz: 1000, pad: 0 },
    SVector { vx: 0, vy: 0, vz: 850, pad: 0 },
];
/// `babyFrogY[5]` / `babyFrogW[5]` (`frontend.c:1128`): baby rotations.
pub const BABY_FROG_Y: [i32; 5] = [3500, 3800, 4096, 3800, 3500];
pub const BABY_FROG_W: [i32; 5] = [-1000, -400, 0, 400, 1000];

// Level-complete tunables (`frontend.c:1137`, `1458`).
const FROG_Y: i32 = 1000;
const BABY_Y: i32 = 1000;
const FROG_Y_STEP: i32 = 500;
const BABY_SCALE: i32 = 1400;
const FROGANIMNUM: i32 = FROG_ANIM_FALL;
const FROGANIMSPEED: i32 = 100;
const FROGANIMNUM2: i32 = FROG_ANIM_FALLLAND;
const FROGANIMSPEED2: i32 = 150;
const FROGANIMNUM3: i32 = FROG_ANIM_DANCE1;
const FROGANIMSPEED3: i32 = 100;
const FROGANIMNUM4: i32 = FROG_ANIM_BREATHE;
const FROGANIMSPEED4: i32 = 100;
const DROP_SPEED: i32 = 40;
const BABYANIMSPEED: i32 = 180;
const BABYANIMSPEED2: i32 = 100;
/// `babyanimy` (PC value).
const BABYANIMY: i32 = 300;
/// `createFrames`.
const CREATE_FRAMES: u32 = 30;

/// `celebrateAnim[NUM_CELEBRATE_ANIMS]` (`frontend.c:1471`).
pub const CELEBRATE_ANIM: [i32; 7] =
    [FROG_ANIM_HOPWHOOHOO, FROG_ANIM_DANCE1, FROG_ANIM_DANCE2, FROG_ANIM_DANCE3, FROG_ANIM_DANCE4, FROG_ANIM_HANDSPRING, FROG_ANIM_VICTORY];

/// `backCharsX/Y`, `backTextX/Y` (`frontend.c:96`): multiplayer HUD home positions.
pub const BACK_CHARS_X: [i32; 4] = [64, 3520, 3520, 64];
pub const BACK_CHARS_Y: [i32; 4] = [420 + 64, 420 + 64, 3360 - 128 - 64, 3360 - 128 - 64];
pub const BACK_TEXT_X: [i32; 4] = [64 + 256 + 128, 3640, 3640, 64 + 256 + 128];
pub const BACK_TEXT_Y: [i32; 4] = [160 + 64, 160 + 64, 3640, 3640];
pub const BACK_WINS_TEXT_X: [i32; 4] = [64 + 512, 3520 - 200, 3520 - 200, 64 + 512];
pub const BACK_WINS_TEXT_Y: [i32; 4] = [420 + 128, 420 + 128, 3360 - 64, 3360 - 64];

/// `fadeSpeed` (`Common/menus.c:842`): `ToFixed(10)`.
pub const FADE_SPEED: i32 = 10 << 12;

/// `AUDIOTRK_*` song ids (`PC/pcaudio.h:87`).
pub use crate::pcaudio::{AUDIOTRK_GAMEOVER, AUDIOTRK_LEVELCOMPLETE, AUDIOTRK_LEVELCOMPLETELOOP};

/// The fields of a `TEXTOVERLAY` / `SPRITEOVERLAY` (`types2d.h`) the frontend code drives.
/// Stand-in until the overlay lists of wave E are merged.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiOverlay {
    /// Sprite overlay (texture `tex`) rather than text overlay.
    pub sprite: bool,
    pub text: String,
    pub tex: String,
    pub x_pos: i32,
    pub y_pos: i32,
    pub x_pos_to: i32,
    pub y_pos_to: i32,
    pub width: i32,
    pub height: i32,
    pub speed: i32,
    pub centred: bool,
    pub draw: bool,
    pub num: i32,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: i32,
    pub flags: u32,
    pub scale: i32,
}

impl UiOverlay {
    /// `CreateAndAddTextOverlay(x, y, text, centred, alpha, font, flags)` (Common/textover.c:52).
    /// `speed` is whatever the reused block held; the port starts at 0.
    pub fn text(x: i32, y: i32, text: &str, centred: bool, a: i32) -> UiOverlay {
        UiOverlay {
            text: text.to_string(),
            x_pos: x,
            y_pos: y,
            x_pos_to: x,
            y_pos_to: y,
            centred,
            draw: true,
            r: 255,
            g: 255,
            b: 255,
            a,
            scale: 4096,
            ..Default::default()
        }
    }

    /// `CreateAndAddSpriteOverlay(x, y, tex, w, h, alpha, flags)` (Common/overlays.c:40).
    pub fn sprite(x: i32, y: i32, tex: &str, width: i32, height: i32, a: i32) -> UiOverlay {
        UiOverlay {
            sprite: true,
            tex: tex.to_string(),
            x_pos: x,
            y_pos: y,
            x_pos_to: x,
            y_pos_to: y,
            width,
            height,
            draw: true,
            r: 255,
            g: 255,
            b: 255,
            a,
            speed: 50,
            scale: 4096,
            ..Default::default()
        }
    }

    /// The motion part of `PrintTextOverlays` (Common/textover.c:220) / `PrintSpriteOverlays`
    /// (PC/pcsprite.c:161): x first, then y, at `FMul(gameSpeed, speed)>>12` per frame.
    pub fn step(&mut self, game_speed: i32, paused: bool) {
        let active = self.draw && if self.sprite { self.a != 0 } else { !paused };
        if !active || self.speed == 0 {
            return;
        }
        let spd = fmul(game_speed, self.speed) >> 12;
        if self.x_pos_to != self.x_pos {
            self.x_pos = self.x_pos.wrapping_add(if self.x_pos_to > self.x_pos { spd } else { -spd });
            if (self.x_pos_to - self.x_pos).abs() < spd.abs() {
                self.speed = 0;
                self.x_pos = self.x_pos_to;
            }
        } else if self.y_pos_to != self.y_pos {
            self.y_pos = self.y_pos.wrapping_add(if self.y_pos_to > self.y_pos { spd } else { -spd });
            if (self.y_pos_to - self.y_pos).abs() < spd.abs() {
                self.speed = 0;
                self.y_pos = self.y_pos_to;
            }
        }
    }

    /// `DEC_ALPHA(x)` (`menus.h:41`).
    pub fn dec_alpha(&mut self, game_speed: i32) {
        let t = fmul(game_speed, FADE_SPEED) >> 12;
        if self.a > t {
            self.a -= t;
        } else {
            self.a = 0;
            self.draw = false;
        }
    }

    /// `INC_ALPHA(x, y)` (`menus.h:55`).
    pub fn inc_alpha(&mut self, y: i32, game_speed: i32) {
        let t = fmul(game_speed, FADE_SPEED) >> 12;
        self.draw = true;
        if self.a + t < y {
            self.a += t;
        } else {
            self.a = y;
        }
    }
}

/// `MULTIGAMEWIN` (`frontend.c:485`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultiGameWin {
    pub count_text_over: [Option<TextOverlayId>; 4],
    pub current: i8,
    pub mode: i8,
    pub countdown: u32,
}

/// Globals of `frontend.c` (plus the frontend-relevant globals of `PC/Main.cpp`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrontendState {
    /// `numExtra`.
    pub num_extra: i32,
    /// `extraOpenStr`.
    pub extra_open_str: String,
    /// `eolTrackComplete`.
    pub eol_track_complete: i32,
    /// `levCompleteState`.
    pub lev_complete_state: i32,
    /// `timeForLevel` (tenths of a second).
    pub time_for_level: i32,
    /// `ActiveController`.
    pub active_controller: i8,
    /// `createTime`.
    pub create_time: u32,
    /// `currentName` (enter-name prompt).
    pub current_name: String,
    /// `grade`: 0 gold (new record), 1 silver, 2 bronze, 3 training.
    pub grade: i32,
    /// `moreCoins`.
    pub more_coins: i32,
    /// `levelOpened`.
    pub level_opened: i32,
    /// `levelBeaten`.
    pub level_beaten: i32,
    /// `cOption`.
    pub c_option: i32,
    /// `goFrontend`.
    pub go_frontend: i32,
    /// `tText`, `coinText`, `newBestText`, `extraText`, `nText`, `oText[2]`, `oldBestText`.
    pub t_text: Option<UiOverlay>,
    pub coin_text: Option<UiOverlay>,
    pub new_best_text: Option<UiOverlay>,
    pub extra_text: Option<UiOverlay>,
    pub n_text: Option<UiOverlay>,
    pub o_text: [Option<UiOverlay>; 2],
    pub old_best_text: Option<UiOverlay>,
    /// `levName`.
    pub lev_name: Option<UiOverlay>,
    /// `extraIcon`, `coinIcon`, `bIcon`.
    pub extra_icon: Option<UiOverlay>,
    pub coin_icon: Option<UiOverlay>,
    pub b_icon: Option<UiOverlay>,
    /// `sparkles[MAX_SPARKLES]` around the trophy.
    pub sparkles: Vec<Option<UiOverlay>>,
    /// Coins flying off during the missed-coins count (`CreateAndAddSpriteOverlay("SCOIN0001")`).
    pub flying_coins: Vec<UiOverlay>,
    /// `coinStr`, `oldBestStr`, `levTimeText`.
    pub coin_str: String,
    pub old_best_str: String,
    pub lev_time_text: String,
    /// `coinsMissed`, `coinCounter`.
    pub coins_missed: i32,
    pub coin_counter: i32,
    /// `eolTimer`.
    pub eol_timer: Timer,
    /// `theEndText` / `theEndTimer`.
    pub the_end_text: Option<TextOverlayId>,
    pub the_end_timer: Timer,
    /// `gameOverText` (local of `StartGameOver`).
    pub game_over_text: Option<UiOverlay>,
    /// `mgWin`.
    pub mg_win: Option<MultiGameWin>,
    /// `menuText[2]` (created by the multiplayer HUD, `hud.c:364`).
    pub menu_text: [Option<TextOverlayId>; 2],
    /// `menuOption`, `optionSelected`.
    pub menu_option: i32,
    pub option_selected: i32,
    /// `textEntry` (`PC/Main.cpp`): characters still to type; 0 = not entering text.
    pub text_entry: i32,
    /// `textString[255]` (`PC/Main.cpp`): the typed name.
    pub text_string: Vec<u8>,
    /// `restartingLevel` (`Common/menus.c:61`).
    pub restarting_level: bool,
    /// `quitAllVideo` (`Common/layout.c`): set by the host when the player skipped all videos.
    pub quit_all_video: i32,
    /// `IsSongPlaying()` as last reported by the host (CD/music stream state).
    pub song_playing: bool,
    /// `checkMenuKeys` (`PC/controll.cpp`): menu keys (Esc etc.) map to pad buttons.
    pub check_menu_keys: i32,
}

impl Default for FrontendState {
    fn default() -> Self {
        FrontendState {
            num_extra: 0,
            extra_open_str: String::new(),
            eol_track_complete: 0,
            lev_complete_state: 0,
            time_for_level: 0,
            active_controller: 0,
            create_time: 0,
            current_name: String::new(),
            grade: 0,
            more_coins: 0,
            level_opened: 0,
            level_beaten: 0,
            c_option: 0,
            go_frontend: 1,
            t_text: None,
            coin_text: None,
            new_best_text: None,
            extra_text: None,
            n_text: None,
            o_text: [None, None],
            old_best_text: None,
            lev_name: None,
            extra_icon: None,
            coin_icon: None,
            b_icon: None,
            sparkles: vec![None; MAX_SPARKLES],
            flying_coins: Vec::new(),
            coin_str: String::new(),
            old_best_str: String::new(),
            lev_time_text: String::new(),
            coins_missed: 0,
            coin_counter: 0,
            eol_timer: Timer::default(),
            the_end_text: None,
            the_end_timer: Timer::default(),
            game_over_text: None,
            mg_win: None,
            menu_text: [None, None],
            menu_option: -1,
            option_selected: 0,
            text_entry: 0,
            text_string: Vec::new(),
            restarting_level: false,
            quit_all_video: 0,
            song_playing: false,
            check_menu_keys: 0,
        }
    }
}

fn nul_str(b: &[u8]) -> String {
    b.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect()
}

/// `SlideSpriteOverlayToPos` / `SlideTextOverlayToPos` (Common/frontend.c:571).
pub fn slide_overlay_to_pos(s: &mut UiOverlay, x: i32, y: i32, from_x: i32, from_y: i32, num_frames: i32, game_speed: i32) {
    let mut vect = FVector { vx: x - from_x, vy: y - from_y, vz: 0 };
    let mut speed = magnitude_f(&vect);
    speed = fmul(speed, game_speed);

    vect.vx = x - s.x_pos;
    vect.vy = y - s.y_pos;
    make_unit(&mut vect);

    if (s.x_pos - x).abs() <= speed / num_frames {
        s.x_pos = x;
    } else {
        s.x_pos += fmul(vect.vx, speed / num_frames);
    }
    if (s.y_pos - y).abs() <= speed / num_frames {
        s.y_pos = y;
    } else {
        s.y_pos += fmul(vect.vy, speed / num_frames);
    }
    s.x_pos_to = s.x_pos;
    s.y_pos_to = s.y_pos;
}

/// The pulsing highlight colour `127+((rsin(afc*k)+4096)*64)/4096`.
fn pulse(f: i32) -> u8 {
    (127 + ((f + 4096) * 64) / 4096) as u8
}

impl Game {
    /// Port of `GameLoop` (Common/frontend.c:144) for the PC build: dispatches on
    /// `gameState.mode`, then `frameCount++` and the `inputPause` reset.
    pub fn game_loop(&mut self) {
        // camControlMode toggling is #ifndef FINAL_MASTER (dropped).
        self.update_cheat_text();

        match self.game_state.mode {
            STARTUP_MODE => {
                if self.game.mode_timer.time != 0 {
                    let afc = self.act_frame_count;
                    self.game.mode_timer.update(-1, afc);
                    if self.game.mode_timer.time == 0 {
                        self.screen_fade(255, 0, 30);
                    }
                } else if !self.levplay.fading_out {
                    self.start_video_playback(FMV_HASBRO_LOGO);
                    if self.frontend.quit_all_video == 0 {
                        self.start_video_playback(FMV_BLITZ_LOGO);
                        if self.frontend.quit_all_video == 0 {
                            self.start_video_playback(FMV_INTRO);
                        }
                    }
                    self.game_state.mode = FRONTEND_MODE;
                    self.act_frame_count = 0;
                    self.game.mode_timer.init(1);
                    // The title backdrop goes when the frontend level replaces it.
                    self.free_backdrop();
                    let (w, l) = (self.frogger.player[0].world_num as u32, self.frogger.player[0].level_num as u32);
                    self.init_level(w, l);
                    self.update_ui_overlays();
                    return;
                }
            }
            INGAME_MODE => self.run_game_loop(),
            DEMO_MODE => self.run_demo_mode(),
            THEEND_MODE => self.run_the_end_mode(),
            FRONTEND_MODE => self.run_frontend_game_loop(),
            TRAINING_MODE => self.run_training_mode(),
            LEVELCOMPLETE_MODE => self.run_level_complete(),
            GAMEOVER_MODE => self.run_game_over(),
            CAMEO_MODE | PAUSE_MODE => self.run_pause_menu(),
            // BUG(original): MULTI_WINBATTLE_MODE runs RunMultiWinCollect.
            MULTI_WINBATTLE_MODE => self.run_multi_win_collect(),
            MULTI_WINCOLLECT_MODE => self.run_multi_win_collect(),
            MULTI_WINRACE_MODE => self.run_multi_win_race(),
            ARTVIEWER_MODE => self.run_art_viewer(),
            // wave G: charsel.c's mode (never entered by the retail game, see crate::charsel).
            CHARSELECT_MODE => self.run_character_select_mode(),
            // CONTROLLERVIEW_MODE / E3_LEVELSELECT_MODE are commented out, TEASERSCREEN_MODE is
            // PC_DEMO only; WORLDCOMPLETE, GAMECOMPLETE, INTRO and FMVPLAY have no
            // case in the retail GameLoop (nothing runs).
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

        self.update_ui_overlays();
    }

    /// Motion of the overlays this module owns (runs while drawing in the original).
    pub fn update_ui_overlays(&mut self) {
        let gs = self.game_speed;
        let paused = self.game_state.mode == PAUSE_MODE;
        let f = &mut self.frontend;
        for o in [
            &mut f.t_text,
            &mut f.coin_text,
            &mut f.new_best_text,
            &mut f.extra_text,
            &mut f.n_text,
            &mut f.old_best_text,
            &mut f.lev_name,
            &mut f.extra_icon,
            &mut f.coin_icon,
            &mut f.b_icon,
            &mut f.game_over_text,
        ]
        .into_iter()
        .flatten()
        {
            o.step(gs, paused);
        }
        for o in f.o_text.iter_mut().chain(f.sparkles.iter_mut()).flatten() {
            o.step(gs, paused);
        }
        for o in f.flying_coins.iter_mut() {
            o.step(gs, paused);
        }
    }

    /// Port of `RunTheEndMode` (Common/frontend.c:112).
    pub fn run_the_end_mode(&mut self) {
        if self.frontend.the_end_text.is_none() {
            self.textover.free_text_overlay_linked_list();
            self.textover.init_text_overlay_linked_list(2);
            let s = self.gamestring(STR_THE_END);
            self.frontend.the_end_text = self.textover.create_and_add_text_overlay(2048, 2000, &s, true, 255, Font::Normal, 0);
            self.frontend.the_end_timer.init(5);
            self.screen_fade(0, 255, 30);
        } else if self.frontend.the_end_timer.time != 0 && !self.levplay.fading_out {
            let afc = self.act_frame_count;
            self.frontend.the_end_timer.update(-1, afc);
            if self.frontend.the_end_timer.time == 0 || self.levplay.pad_data.debounce[0] != 0 {
                self.screen_fade(255, 0, 30);
                self.frontend.the_end_timer.time = 0;
            }
        }
        if !self.levplay.fading_out && self.frontend.the_end_timer.time == 0 {
            self.game_state.mode = FRONTEND_MODE;
            self.frontend.the_end_text = None;
            let (w, l) = (self.frogger.player[0].world_num as u32, self.frogger.player[0].level_num as u32);
            self.init_level(w, l);
            self.frame_count = 0;
        }
    }

    /// Port of `LevelCompleteProcessController` (Common/frontend.c:341).
    pub fn level_complete_process_controller(&mut self, pl: i32) {
        let button = self.levplay.pad_data.debounce[pl as usize];
        if button & PAD_UP != 0 {
            if self.frontend.c_option > 0 {
                self.frontend.c_option -= 1;
                self.play_sample(f2_formats::crc::crc("hopongrass"), None, 0, SAMPLE_VOLUME, -1);
            }
        } else if button & PAD_DOWN != 0 && self.frontend.c_option < 1 && self.frontend.o_text[1].is_some() {
            self.frontend.c_option += 1;
            self.play_sample(f2_formats::crc::crc("hopongrass"), None, 0, SAMPLE_VOLUME, -1);
        }

        if button & PAD_CROSS != 0 {
            self.play_sample(f2_formats::crc::crc("hop2"), None, 0, SAMPLE_VOLUME, -1);
            self.game.show_end_level_screen = -1;
            self.frontend.lev_complete_state = LEV_COMPLETE_FADING_OUT;
            let f = &self.frontend;
            if f.grade == 0 || f.more_coins != 0 || f.level_opened != 0 || f.level_beaten != 0 {
                self.save_game();
            }
            self.frontend.go_frontend = if self.frontend.c_option == 1 { 1 } else { 0 };
        }
    }

    /// Port of `RunGameOver` (Common/frontend.c:404).
    pub fn run_game_over(&mut self) {
        let afc = self.act_frame_count;
        self.game.mode_timer.update(-1, afc);
        if !self.fadeout.keep_fade && (self.game.mode_timer.time == 0 || self.levplay.pad_data.debounce[0] != 0) {
            self.screen_fade(255, 0, 30);
            self.fadeout.keep_fade = true;
        }
        if self.fadeout.keep_fade && !self.levplay.fading_out {
            self.game_state.mode = FRONTEND_MODE;
            self.frogger.player[0].lives = NUM_LIVES[self.game_state.difficulty as usize % 3];
            self.frogger.player[0].level_num = LEVELID_FRONTEND1 as u8;
            self.frogger.player[0].world_num = worldid::FRONTEND as u8;
            self.frogger.player[0].frog_state &= !FROGSTATUS_ISDEAD;
            self.frontend.game_over_text = None;
            self.init_level(worldid::FRONTEND, LEVELID_FRONTEND1);
            self.frame_count = 0;
        }
    }

    /// Port of `StartGameOver` (Common/frontend.c:446).
    pub fn start_game_over(&mut self) {
        self.game_state.mode = GAMEOVER_MODE;
        self.game.mode_timer.init(9);
        let mut t = UiOverlay::text(2048, 1980, &self.gamestring(STR_GAMEOVER), true, 255);
        t.flags = TEXTOVERLAY_SHADOW;
        t.g = 0;
        t.b = 0;
        self.frontend.game_over_text = Some(t);
        if let Some(f) = self.frogger.frog[0] {
            self.actor2.actors[f].draw = 0;
        }
        self.prepare_song(AUDIOTRK_GAMEOVER, false);
    }

    /// Port of `StartGameIntro` (Common/frontend.c:469).
    pub fn start_game_intro(&mut self) {
        self.frogger.player[0].world_num = worldid::FRONTEND as u8;
        self.frogger.player[0].level_num = LEVELID_FRONTEND1 as u8;
        self.game_state.mode = LEVELCOMPLETE_MODE;
        self.game_state.multi = SINGLEPLAYER;
        self.game.mode_timer.init(1);
        self.game.show_end_level_screen = 0;
    }

    /// Port of `SetTimeForLevel` (Common/frontend.c:990).
    pub fn set_time_for_level(&mut self) {
        self.frontend.time_for_level = (self.act_frame_count.wrapping_sub(self.hud_store_frame_count()) / 6) as i32;
    }

    /// Port of `CheckEOLLoopTrack` (Common/frontend.c:1484).
    pub fn check_eol_loop_track(&mut self) {
        if self.frontend.song_playing {
            if self.frontend.eol_track_complete == 0 {
                self.frontend.eol_track_complete = 1;
            }
        } else if self.frontend.eol_track_complete == 1 {
            self.prepare_song(AUDIOTRK_LEVELCOMPLETELOOP, true);
            self.frontend.eol_track_complete = 2;
        }
    }

    /// The `EV_ENDLEVEL` case of `Interpret` (Common/script.c:1210): a script ends the level.
    pub fn script_end_level(&mut self) {
        self.game_state.mode = LEVELCOMPLETE_MODE;
        self.game_state.multi = SINGLEPLAYER;
        self.game.mode_timer.init(8);
        self.set_time_for_level();
        self.prepare_song(AUDIOTRK_LEVELCOMPLETE, false);
        self.start_level_complete();
    }

    fn lvd_cur(&self) -> &crate::layout::LevelVisualData {
        let p = &self.frogger.player[0];
        &self.layout.world_visual_data[p.world_num as usize].level_visual_data[p.level_num as usize]
    }

    fn lvd_cur_mut(&mut self) -> &mut crate::layout::LevelVisualData {
        let (w, l) = (self.frogger.player[0].world_num as usize, self.frogger.player[0].level_num as usize);
        &mut self.layout.world_visual_data[w].level_visual_data[l]
    }

    /// `sprintf(oldBestStr, GAMESTRING(STR_RECORD), parName, min, sec, tenths)`.
    fn make_old_best_str(&self) -> String {
        let lv = self.lvd_cur();
        let t = lv.par_time as i32;
        c_sprintf(
            self.language.gamestring(STR_RECORD),
            &[FmtArg::Str(nul_str(&lv.par_name)), FmtArg::Int((t / 600) % 600), FmtArg::Int((t / 10) % 60), FmtArg::Int(t % 10)],
        )
    }

    /// Port of `StartLevelComplete` (Common/frontend.c:1158).
    pub fn start_level_complete(&mut self) {
        let cz = self.hud.arcade_hud.coin_zoom;
        self.overlays.disable(cz);
        self.frontend.check_menu_keys = 1;

        if let Some(f) = self.frogger.frog[0] {
            self.undo_change_model(f);
        }
        self.frontend.eol_track_complete = 0;
        if let Some(t) = self.training.train.as_mut() {
            t.bg = None;
            t.txtover = [None, None];
        }

        self.frontend.coins_missed = 0;
        self.frontend.coin_counter = 0;
        self.collect.collect_text.draw = 0;
        let ct = self.hud.arcade_hud.collect_text;
        self.textover.disable(ct);

        self.free_ambient_sound_list();
        self.screen_fade(0, 255, 20);
        self.fadeout.keep_fade = false;
        self.fadeout.flash_screen = true;

        self.frontend.level_opened = 0;
        self.frontend.level_beaten = 0;

        let timeout = self.arcade_hud_time_out_draw();
        let hard = self.game_state.difficulty == DIFFICULTY_HARD;
        if self.lvd_cur().level_completed == 0 {
            self.frontend.level_opened = 1;
        }
        if !hard || !timeout {
            self.lvd_cur_mut().level_completed = 1;
        }

        if self.game_state.single == STORY_MODE && (self.game_state.story_sequence_level as usize) < NUM_STORY_LEVELS - 1 && (!hard || !timeout) {
            self.game_state.story_sequence_level += 1;
            let sl = STORY_SEQUENCE[self.game_state.story_sequence_level as usize];
            let wv = &mut self.layout.world_visual_data[sl.world_num as usize];
            if wv.world_open == WORLD_CLOSED {
                self.frontend.level_opened = 1;
            }
            wv.world_open = WORLD_OPEN;
            if wv.level_visual_data[sl.level_num as usize].level_open == LEVEL_CLOSED {
                self.frontend.level_opened = 1;
            }
            wv.level_visual_data[sl.level_num as usize].level_open = LEVEL_OPEN;
        }

        self.frontend.c_option = 0;
        self.disable_hud();

        self.init_tiled_backdrop("LOGO");

        for i in 0..self.map.num_babies.clamp(0, crate::babyfrog::NUM_BABIES as i32) as usize {
            if let Some(o) = self.babyfrog.baby_icons[i].as_mut() {
                o.draw = 0;
            }
        }
        if let Some(o) = self.babyfrog.baby_flash.as_mut() {
            o.draw = 0;
        }

        if self.training.train.is_none() {
            self.pc_name_entry_init();
        }

        for id in self.actor2.act_list.ids(&self.actor2.actors) {
            self.actor2.actors[id].draw = 0;
        }
        self.hide_all_sprites();

        self.frontend.create_time = self.act_frame_count.wrapping_add(CREATE_FRAMES);
        self.frontend.more_coins = 0;

        if let Some(o) = self.overlays.get_mut(self.hud.arcade_hud.coins_over) {
            o.x_pos = 3400 + 4096;
        }
        self.frontend.coin_text = None;
        self.frontend.coin_icon = None;
        self.frontend.extra_text = None;
        self.frontend.extra_icon = None;
        self.frontend.flying_coins.clear();

        let max_coins = self.collect.garib_list.max_coins;
        let num_spawn = self.frogger.player[0].num_spawn;
        if max_coins != 0 {
            if num_spawn == max_coins {
                let mut ct = UiOverlay::text(2048 + 4096, 850, &self.gamestring(STR_GOT_ALL_COINS), true, 255);
                ct.x_pos_to = 2048;
                self.frontend.coin_text = Some(ct);
                let mut ci = UiOverlay::sprite(2048 - 256 - 4096, 850 + 300, "COINMEDAL", 512, 512, 255);
                ci.x_pos_to = 2048 - 256;
                self.frontend.coin_icon = Some(ci);
                if num_spawn > self.lvd_cur().max_coins as i16 {
                    self.lvd_cur_mut().max_coins = num_spawn as u8;
                    self.frontend.more_coins = 1;
                    let mut num_extra = 0;
                    for s in STORY_SEQUENCE.iter().skip(1) {
                        if self.layout.world_visual_data[s.world_num as usize].level_visual_data[s.level_num as usize].max_coins == 25 {
                            num_extra += 1;
                        }
                    }
                    self.frontend.num_extra = num_extra;
                    let mut et = if num_extra <= 9 {
                        let desc = self.layout.world_visual_data[worldid::SUPERRETRO as usize].level_visual_data[num_extra as usize].description_str;
                        self.frontend.extra_open_str = c_sprintf(self.language.gamestring(STR_OPENED_EXTRA_1), &[FmtArg::Str(self.gamestring(desc))]);
                        UiOverlay::text(2048 + 4096 + 4096, 850 + 750, &self.frontend.extra_open_str, true, 255)
                    } else {
                        UiOverlay::text(2048 + 4096 + 4096, 850 + 750, &self.gamestring(STR_OPENED_EXTRA_1 + num_extra - 9), true, 255)
                    };
                    et.x_pos_to = 2048;
                    self.frontend.extra_text = Some(et);
                    let icon = STORY_SEQUENCE.get(num_extra as usize).map_or("", |s| s.icon_name);
                    let mut ei = UiOverlay::sprite(2048 - 256 - 4096 - 4096, 850 + 1050, icon, 512, 512, 255);
                    ei.x_pos_to = 2048 - 256;
                    self.frontend.extra_icon = Some(ei);
                }
                self.overlays.disable(self.hud.arcade_hud.coins_over);
            } else {
                self.frontend.coin_str = c_sprintf(self.language.gamestring(STR_MISSED_COINS), &[FmtArg::Int(0)]);
                let mut ct = UiOverlay::text(2048 + 4096, 850, &self.frontend.coin_str, true, 255);
                ct.x_pos_to = 2048;
                self.frontend.coin_text = Some(ct);
                self.frontend.coin_icon = None;
                let mut et = UiOverlay::text(2048 + 4096 + 4096, 850 + 300, &self.gamestring(STR_NO_BONUS), true, 255);
                et.x_pos_to = 2048;
                self.frontend.extra_text = Some(et);
                self.frontend.extra_icon = None;
                if let Some(co) = self.overlays.get_mut(self.hud.arcade_hud.coins_over) {
                    co.draw = 1;
                    co.x_pos_to = 3400;
                    co.y_pos = 850;
                    co.y_pos_to = 850;
                }
            }
        }

        if num_spawn > self.lvd_cur().max_coins as i16 {
            self.lvd_cur_mut().max_coins = num_spawn as u8;
            self.frontend.more_coins = 1;
        }

        if self.training.train.is_some() {
            self.frontend.grade = 3;
            let mut ct = UiOverlay::text(2048 + 4096, 980 + 300, &self.gamestring(STR_TRAINING_COMPLETE), true, 255);
            ct.x_pos_to = 2048;
            self.frontend.coin_text = Some(ct);
        } else {
            if hard && self.lvd_cur().level_beaten == 0 && !timeout {
                self.lvd_cur_mut().level_beaten = 1;
                self.frontend.level_beaten = 1;
            }
            // timeForLevel (int) against parTime (unsigned long): unsigned comparisons.
            let t = self.frontend.time_for_level as u32;
            let par = self.lvd_cur().par_time;
            self.frontend.grade = if t < par {
                0
            } else if t < par.wrapping_mul(3) / 2 {
                1
            } else {
                2
            };
            if self.frontend.grade < 2 {
                for i in 0..MAX_SPARKLES {
                    let a = self.rng.random(0xff) as u8 as i32;
                    let mut s = UiOverlay::sprite(256, 1535, "FLASH", 200, 200, a);
                    s.flags = SPRITE_ADDITIVE;
                    s.num = 1;
                    s.draw = false;
                    s.a = self.rng.random(256);
                    self.frontend.sparkles[i] = Some(s);
                }
            }
        }

        if self.frontend.grade == 0 {
            let mut t = UiOverlay::text(2048 + 4096, 850 + 800, &self.gamestring(STR_NEW_BEST_TIME), true, 255);
            t.r = 255;
            t.g = 255;
            t.b = 0;
            t.x_pos_to = 2048;
            self.frontend.new_best_text = Some(t);
        } else {
            self.frontend.new_best_text = None;
        }

        self.frontend.lev_time_text = if self.training.train.is_some() {
            String::new()
        } else {
            let t = self.frontend.time_for_level;
            c_sprintf(self.language.gamestring(STR_YOUTOOKTIMEMIN), &[FmtArg::Int((t / 600) % 600), FmtArg::Int((t / 10) % 60), FmtArg::Int(t % 10)])
        };

        let desc = self.lvd_cur().description_str;
        let mut ln = UiOverlay::text(2048, 200 + 210, &self.gamestring(desc), true, 0xff);
        ln.r = 0;
        ln.g = 255;
        ln.b = 255;
        self.frontend.lev_name = Some(ln);

        if self.frontend.grade < 3 {
            let mut t = UiOverlay::text(2048 + 4096, 850, &self.frontend.lev_time_text, true, 0xff);
            t.x_pos_to = 2048;
            self.frontend.t_text = Some(t);
            let mut b = UiOverlay::sprite(2048 - 256 + 4096, 1150, TROPHY_NAME[self.frontend.grade as usize], 512, 512, 255);
            b.x_pos_to = 2048 - 256;
            self.frontend.b_icon = Some(b);
            self.frontend.old_best_str = self.make_old_best_str();
            let mut o = UiOverlay::text(2048 + 4096, 1950, &self.frontend.old_best_str, true, 255);
            o.x_pos_to = 2048;
            self.frontend.old_best_text = Some(o);
        } else {
            self.frontend.t_text = None;
            self.frontend.b_icon = None;
        }

        if self.game_state.single == STORY_MODE && !timeout {
            let mut o = UiOverlay::text(2048 + 4096, 3420, &self.gamestring(STR_PRESS_X_TO_CONTINUE), true, 0xff);
            o.x_pos_to = 2048;
            self.frontend.o_text = [Some(o), None];
        } else {
            let mut o0 = UiOverlay::text(2048 + 4096, 3220, &self.gamestring(STR_RESTARTLEVEL), true, 0xff);
            let mut o1 = UiOverlay::text(2048 - 4096, 3610, &self.gamestring(STR_QUIT), true, 0xff);
            o0.x_pos_to = 2048;
            o1.x_pos_to = 2048;
            self.frontend.o_text = [Some(o0), Some(o1)];
        }
        self.frontend.n_text = None;

        if self.frontend.grade == 0 {
            self.frontend.current_name = format!("{} {}", self.gamestring(STR_ENTER_NAME), nul_str(&self.frontend.text_string));
            let mut n = UiOverlay::text(4096, 3350, &self.frontend.current_name, false, 0xff);
            // w = CalcStringWidth(text, font) * 3.2 (font metrics, renderer side): the slide
            // distance is 4096 whatever w is.
            let w = 0;
            n.x_pos_to = 2048 - w;
            n.x_pos = n.x_pos_to + 4096;
            self.frontend.n_text = Some(n);
        }

        if let Some(f) = self.frogger.frog[0] {
            self.actor_animate(f, FROG_ANIM_DANCE1, true, false, 80, false);
        }

        self.frontend.lev_complete_state = LEV_COMPLETE_DROPPING_FROGS;

        self.cam.curr_cam_source = f2_math::FVector { vx: 0, vy: 500 << 12, vz: 0 };
        self.cam.curr_cam_target = f2_math::FVector { vx: 0, vy: 0, vz: 3000 << 12 };
        self.cam.cam_vect = f2_math::FVector { vx: 0, vy: 4096, vz: 0 };
        // SetCamFF(currCamSource, currCamTarget): PSX only.

        for i in 0..self.map.num_babies.clamp(0, crate::babyfrog::NUM_BABIES as i32) as usize {
            let Some(b) = self.babyfrog.baby_list[i].baby else { continue };
            let a = &mut self.actor2.actors[b];
            a.draw = 1;
            a.depth_shift = 0;
            a.actor.position = FROG_POS[i];
            a.actor.position.vy = (FROG_Y + BABY_Y + i as i32 * FROG_Y_STEP) as i16;
            a.actor.q_rot.x = 0;
            a.actor.q_rot.y = BABY_FROG_Y[i];
            a.actor.q_rot.z = 0;
            a.actor.q_rot.w = BABY_FROG_W[i];
            a.actor.size = FVector { vx: BABY_SCALE, vy: BABY_SCALE, vz: BABY_SCALE };
            self.actor_animate(b, BABY_ANIM_COLLECTHOLD, true, false, 100, false);
        }

        if let Some(f) = self.frogger.frog[0] {
            let a = &mut self.actor2.actors[f];
            a.actor.position = FROG_POS[5];
            a.actor.position.vy = FROG_Y as i16;
            a.actor.q_rot.x = 0;
            a.actor.q_rot.y = 4096;
            a.actor.q_rot.z = 0;
            a.actor.q_rot.w = 0;
            a.draw = 1;
            self.actor_animate(f, FROGANIMNUM, false, false, FROGANIMSPEED, false);
        }
        self.backdrop.draw_landscape = false;
    }

    /// Port of `RunLevelComplete` (Common/frontend.c:1500).
    pub fn run_level_complete(&mut self) {
        let dropped = fmul(DROP_SPEED, self.game_speed);
        self.frontend.old_best_str = self.make_old_best_str();
        if let Some(o) = self.frontend.old_best_text.as_mut() {
            o.text = self.frontend.old_best_str.clone();
        }
        self.backdrop.draw_landscape = false;

        self.check_eol_loop_track();

        let timeout = self.arcade_hud_time_out_draw();
        let frog = self.frogger.frog[0];
        let num_babies = self.map.num_babies.clamp(0, crate::babyfrog::NUM_BABIES as i32) as usize;
        let gs = self.game_speed;

        match self.frontend.lev_complete_state {
            LEV_COMPLETE_DROPPING_FROGS => {
                if self.levplay.fading_out {
                    return;
                }
                if let Some(f) = frog {
                    let p = &mut self.actor2.actors[f].actor.position;
                    if p.vy > 0 {
                        p.vy = (p.vy as i32 - dropped) as i16;
                    }
                    if p.vy <= 0 {
                        p.vy = 0;
                        if self.get_anim_num(f) == FROGANIMNUM && self.reached_end_of_anim(f) != 0 {
                            self.actor_animate(f, FROGANIMNUM2, false, false, FROGANIMSPEED2, false);
                            let r = self.rng.random(4);
                            self.actor_animate(f, FROGANIMNUM3 + r, false, true, FROGANIMSPEED3, false);
                            self.actor_animate(f, FROGANIMNUM4, true, true, FROGANIMSPEED4, false);
                        }
                    }
                }

                if !timeout {
                    for i in 0..num_babies {
                        let Some(b) = self.babyfrog.baby_list[i].baby else { continue };
                        let vy = self.actor2.actors[b].actor.position.vy as i32;
                        if vy > 0 {
                            let nvy = vy - dropped;
                            self.actor2.actors[b].actor.position.vy = nvy as i16;
                            if nvy <= BABYANIMY && nvy + dropped > BABYANIMY {
                                self.actor_animate(b, BABY_ANIM_COLLECT, false, false, -BABYANIMSPEED, false);
                                self.actor_animate(b, BABY_ANIM_HOP, true, true, BABYANIMSPEED2, false);
                            }
                        }
                    }
                }
            }

            LEV_COMPLETE_COUNTING_COINS => {
                let f = &mut self.frontend;
                for o in [&mut f.coin_text, &mut f.coin_icon, &mut f.extra_text, &mut f.extra_icon].into_iter().flatten() {
                    o.speed = 4096 * 75;
                }
                if let Some(co) = self.overlays.get_mut(self.hud.arcade_hud.coins_over) {
                    co.speed = 4096 * 75;
                }

                // BUG(original): coinText is NULL on a level without coins (not training); the
                // original dereferences it. The port treats a missing overlay as arrived.
                let coin_arrived = self.frontend.coin_text.as_ref().is_none_or(|c| c.x_pos == c.x_pos_to);
                if coin_arrived {
                    let max_coins = self.collect.garib_list.max_coins as i32;
                    let num_spawn = self.frogger.player[0].num_spawn as i32;
                    if num_spawn != max_coins {
                        if self.frontend.coins_missed < max_coins - num_spawn {
                            if self.levplay.pad_data.digital[0] & PAD_CROSS != 0 {
                                self.frontend.coin_counter += 10;
                            } else {
                                self.frontend.coin_counter += self.act_frame_count.wrapping_sub(self.last_act_frame_count) as i32;
                            }
                            if self.frontend.coin_counter > 10 {
                                self.frontend.coins_missed += 1;
                                self.frontend.coin_str = c_sprintf(self.language.gamestring(STR_MISSED_COINS), &[FmtArg::Int(self.frontend.coins_missed)]);
                                if let Some(c) = self.frontend.coin_text.as_mut() {
                                    c.text = self.frontend.coin_str.clone();
                                }
                                self.frontend.coin_counter = 0;
                                self.play_sample(f2_formats::crc::crc("pickupcoin"), None, 0, SAMPLE_VOLUME, -1);
                                let (cx, cy) = self.overlays.get(self.hud.arcade_hud.coins_over).map_or((0, 0), |o| (o.x_pos as i32, o.y_pos as i32));
                                let tex = if self.cheat_mad_garibs() { "RGARIB01" } else { "SCOIN0001" };
                                let mut c = UiOverlay::sprite(cx, cy, tex, 205, 273, 0xff);
                                c.x_pos_to += 10000;
                                c.speed = 4096 * 50;
                                self.frontend.flying_coins.push(c);
                            }
                        } else {
                            self.overlays.disable(self.hud.arcade_hud.coins_over);
                            if self.frontend.eol_timer.time == 0 {
                                self.frontend.eol_timer.init(2);
                            }
                        }
                    } else if self.frontend.eol_timer.time == 0 {
                        self.frontend.eol_timer.init(4);
                    }
                }

                let f = &self.frontend;
                let ready = match &f.extra_text {
                    Some(e) => e.x_pos == e.x_pos_to,
                    None => f.coin_text.as_ref().is_none_or(|c| c.x_pos == c.x_pos_to),
                };
                if ready && self.frontend.eol_timer.time != 0 {
                    let afc = self.act_frame_count;
                    self.frontend.eol_timer.update(-1, afc);
                    if self.frontend.eol_timer.time == 0 || self.levplay.pad_data.debounce[0] & PAD_CROSS != 0 {
                        self.frontend.eol_timer.time = 0;
                        if self.training.train.is_some() {
                            self.frontend.lev_complete_state = LEV_COMPLETE_MENU;
                        } else {
                            self.frontend.lev_complete_state = LEV_COMPLETE_SLIDING_TROPHY;
                            self.frontend.eol_timer.init(3);
                            let f = &mut self.frontend;
                            for o in [&mut f.coin_text, &mut f.coin_icon, &mut f.extra_text, &mut f.extra_icon].into_iter().flatten() {
                                o.speed = 4096 * 75;
                                o.x_pos_to = -4096;
                            }
                        }
                    }
                }
            }

            LEV_COMPLETE_SLIDING_TROPHY => {
                let f = &mut self.frontend;
                for o in [&mut f.t_text, &mut f.b_icon, &mut f.old_best_text, &mut f.new_best_text].into_iter().flatten() {
                    o.speed = 4096 * 75;
                }
                if let Some(n) = f.n_text.as_mut() {
                    n.speed = 4096 * 75;
                    if n.x_pos == n.x_pos_to {
                        f.lev_complete_state = LEV_COMPLETE_ENTER_NAME;
                        f.text_entry = NAME_LENGTH as i32;
                    }
                }
                let t_done = f.t_text.as_ref().is_none_or(|t| t.x_pos_to == t.x_pos);
                if f.n_text.is_none() && t_done && f.eol_timer.time != 0 {
                    let afc = self.act_frame_count;
                    self.frontend.eol_timer.update(-1, afc);
                    if self.frontend.eol_timer.time == 0 || self.levplay.pad_data.debounce[0] & PAD_CROSS != 0 {
                        self.frontend.lev_complete_state = LEV_COMPLETE_MENU;
                        self.frontend.eol_timer.time = 0;
                    }
                }
            }

            LEV_COMPLETE_ENTER_NAME => {
                if self.frontend.n_text.is_some() {
                    if self.frontend.text_entry == 0 {
                        self.frontend.n_text = None;
                        self.frontend.c_option = 0;
                        if self.frontend.grade == 0 {
                            let name = self.frontend.text_string.clone();
                            let t = self.frontend.time_for_level as u32;
                            let lv = self.lvd_cur_mut();
                            // strcpy(parName, textString): parName is char[9].
                            let n = name.iter().position(|&c| c == 0).unwrap_or(name.len()).min(8);
                            lv.par_name = [0; 9];
                            lv.par_name[..n].copy_from_slice(&name[..n]);
                            lv.par_time = t;
                        }
                        self.frontend.lev_complete_state = LEV_COMPLETE_MENU;
                    }
                    self.frontend.current_name = format!("{} {}", self.gamestring(STR_ENTER_NAME), nul_str(&self.frontend.text_string));
                    if let Some(n) = self.frontend.n_text.as_mut() {
                        n.text = self.frontend.current_name.clone();
                    }
                }
            }

            LEV_COMPLETE_MENU => {
                let f = &mut self.frontend;
                for o in f.o_text.iter_mut().flatten() {
                    o.speed = 4096 * 75;
                }
                let arrived = f.o_text[0].as_ref().is_some_and(|o| o.x_pos == o.x_pos_to);
                if arrived {
                    self.level_complete_process_controller(0);
                }
                let afc = self.act_frame_count as i32;
                let f = &mut self.frontend;
                for o in f.o_text.iter_mut().flatten() {
                    o.r = 128;
                    o.g = 128;
                    o.b = 128;
                }
                if let Some(o) = f.o_text.get_mut(f.c_option as usize).and_then(|o| o.as_mut()) {
                    o.r = pulse(rsin(afc.wrapping_mul(4200)));
                    o.g = pulse(rcos(afc.wrapping_mul(4300)));
                    o.b = pulse(rcos(afc.wrapping_mul(4400)));
                }
            }

            LEV_COMPLETE_FADING_OUT if !self.levplay.fading_out => {
                if !self.fadeout.keep_fade {
                    self.screen_fade(255, 0, 30);
                    self.fadeout.keep_fade = true;
                } else {
                    self.level_complete_fade_done();
                    return;
                }
            }
            _ => {}
        }

        if let (Some(b), true) = (self.frontend.b_icon.as_ref(), self.frontend.grade < 2) {
            let (bx, by, bw, bh) = (b.x_pos, b.y_pos, b.width, b.height);
            for i in 0..MAX_SPARKLES {
                let Some(s) = self.frontend.sparkles[i].as_ref() else { continue };
                if !s.draw {
                    let (sw, sh) = (s.width, s.height);
                    let x = bx - sw / 2 + self.rng.random(bw as u32);
                    let y = by - sh / 2 + self.rng.random(bh as u32);
                    let r = 64 + self.rng.random(192);
                    let g = 64 + self.rng.random(192);
                    let b = 64 + self.rng.random(192);
                    let a = 200 + self.rng.random(56);
                    let s = self.frontend.sparkles[i].as_mut().expect("sparkle");
                    s.x_pos = x;
                    s.y_pos = y;
                    s.r = r as u8;
                    s.g = g as u8;
                    s.b = b as u8;
                    s.a = a;
                    s.draw = true;
                } else if self.rng.random(50) < 20 {
                    self.frontend.sparkles[i].as_mut().expect("sparkle").dec_alpha(gs);
                }
            }
        }

        for i in 0..num_babies {
            let Some(b) = self.babyfrog.baby_list[i].baby else { continue };
            {
                let a = &mut self.actor2.actors[b];
                a.draw = 1;
                a.actor.q_rot.x = 0;
                a.actor.q_rot.y = BABY_FROG_Y[i];
                a.actor.q_rot.z = 0;
                a.actor.q_rot.w = BABY_FROG_W[i];
            }
            if self.actor2.actors[b].actor.position.vy <= 0 {
                if i == 4 && self.frontend.lev_complete_state == LEV_COMPLETE_DROPPING_FROGS {
                    if self.frogger.player[0].world_num as u32 == worldid::SUPERRETRO {
                        self.frontend.lev_complete_state = LEV_COMPLETE_SLIDING_TROPHY;
                        self.frontend.eol_timer.init(3);
                    } else {
                        self.frontend.lev_complete_state = LEV_COMPLETE_COUNTING_COINS;
                        self.frontend.eol_timer.init(0);
                    }
                }
                self.actor2.actors[b].actor.position.vy = 0;

                if self.get_anim_num(b) == BABY_ANIM_HOP && self.get_anim_loop(b) != 0 && self.rng.random(1000) <= 20 {
                    self.set_anim_loop(b, 0);
                    let r = self.rng.random(7);
                    self.actor_animate(b, BABY_ANIM_WAVE + r, false, true, 100, false);
                    self.actor_animate(b, BABY_ANIM_HOP, true, true, BABYANIMSPEED2, false);
                }
            }
        }

        let Some(f) = frog else { return };
        self.actor2.actors[f].draw = 1;
        if self.actor2.actors[f].actor.position.vy <= 0 {
            if timeout && self.frontend.lev_complete_state == LEV_COMPLETE_DROPPING_FROGS {
                self.frontend.lev_complete_state = LEV_COMPLETE_MENU;
                self.actor_animate(f, FROGANIMNUM2, false, false, FROGANIMSPEED2, false);
                self.actor_animate(f, FROGANIMNUM4, true, true, FROGANIMSPEED4, false);
            }
            if num_babies == 0 && self.frontend.lev_complete_state == LEV_COMPLETE_DROPPING_FROGS {
                self.frontend.lev_complete_state = LEV_COMPLETE_COUNTING_COINS;
                self.frontend.eol_timer.init(0);
                // ds - do animations from above somewhere
                self.actor_animate(f, FROGANIMNUM2, false, false, FROGANIMSPEED2, false);
                let r = self.rng.random(4);
                self.actor_animate(f, FROGANIMNUM3 + r, false, true, FROGANIMSPEED3, false);
                self.actor_animate(f, FROGANIMNUM4, true, true, FROGANIMSPEED4, false);
            }

            if !timeout && self.get_anim_num(f) == FROG_ANIM_BREATHE && self.get_anim_loop(f) != 0 && self.rng.random(1000) <= 20 {
                self.set_anim_loop(f, 0);
                let voice = match self.rng.random(3) {
                    0 => Some("frogokay"),
                    1 => Some("frogletsgo"),
                    2 => Some("frogcroak"),
                    _ => None,
                };
                if let Some(v) = voice {
                    self.effects.push(Effect::PlayVoice { player: 0, uid: f2_formats::crc::crc(v) });
                }
                let r = self.rng.random(CELEBRATE_ANIM.len() as u32);
                self.actor_animate(f, CELEBRATE_ANIM[r as usize], false, true, 70, false);
                self.actor_animate(f, FROG_ANIM_BREATHE, true, true, 75, false);
            }
        }
    }

    /// The `LEV_COMPLETE_FADING_OUT` end of `RunLevelComplete` (Common/frontend.c:1806): pick
    /// the next mode and level.
    fn level_complete_fade_done(&mut self) {
        self.training.train = None;
        self.free_tiled_backdrop();
        self.clear_level_complete_overlays();

        if self.frontend.go_frontend != 0 {
            self.game_state.mode = FRONTEND_MODE;
            self.frogger.player[0].world_num = worldid::FRONTEND as u8;
            self.frogger.player[0].level_num = LEVELID_FRONTEND1 as u8;
            self.frogger.player[0].character = FROG_FROGGER;
        } else if self.game_state.single == STORY_MODE && !self.arcade_hud_time_out_draw() {
            self.game_state.mode = INGAME_MODE;
            let last = STORY_SEQUENCE[NUM_STORY_LEVELS - 1];
            if self.frogger.player[0].world_num as u32 == last.world_num && self.frogger.player[0].level_num as u32 == last.level_num {
                self.start_video_playback(FMV_VICTORY);
                self.game_state.story_sequence_level = 0;
                self.frogger.player[0].world_num = worldid::FRONTEND as u8;
                self.frogger.player[0].level_num = LEVELID_FRONTEND1 as u8;
                self.game_state.mode = THEEND_MODE;
            } else {
                let s = STORY_SEQUENCE[self.game_state.story_sequence_level as usize];
                self.frogger.player[0].world_num = s.world_num as u8;
                self.frogger.player[0].level_num = s.level_num as u8;
            }
        } else {
            // todo: place Frogger
            self.game_state.mode = INGAME_MODE;
            self.frontend.restarting_level = true;
        }
        self.frame_count = 0;

        if self.game_state.mode != THEEND_MODE {
            let (w, l) = (self.frogger.player[0].world_num as u32, self.frogger.player[0].level_num as u32);
            self.init_level(w, l);
        }

        self.game.show_end_level_screen = 1; // Normal level progression is default
        self.frame_count = 0;
    }

    /// The level-complete overlays are freed with the text/sprite lists by `InitLevel`.
    fn clear_level_complete_overlays(&mut self) {
        let f = &mut self.frontend;
        f.t_text = None;
        f.coin_text = None;
        f.new_best_text = None;
        f.extra_text = None;
        f.n_text = None;
        f.o_text = [None, None];
        f.old_best_text = None;
        f.lev_name = None;
        f.extra_icon = None;
        f.coin_icon = None;
        f.b_icon = None;
        f.sparkles = vec![None; MAX_SPARKLES];
        f.flying_coins.clear();
    }

    /// Port of `GetAnimNum` (Common/frontend.c:2015).
    pub fn get_anim_num(&self, a: ActorId) -> i32 {
        self.actor2.actors[a].actor.actual_actor.animation.as_ref().map_or(-1, |an| an.current_animation as i32)
    }

    /// Port of `GetAnimLoop` (Common/frontend.c:2024).
    pub fn get_anim_loop(&self, a: ActorId) -> i32 {
        self.actor2.actors[a].actor.actual_actor.animation.as_ref().map_or(0, |an| an.loop_animation)
    }

    /// Port of `SetAnimLoop` (Common/frontend.c:2033).
    pub fn set_anim_loop(&mut self, a: ActorId, looped: i32) {
        if let Some(an) = self.actor2.actors[a].actor.actual_actor.animation.as_mut() {
            an.loop_animation = looped;
        }
    }

    /// Port of `ReachedEndOfAnim` (Common/frontend.c:2042).
    pub fn reached_end_of_anim(&self, a: ActorId) -> i32 {
        self.actor2.actors[a].actor.actual_actor.animation.as_ref().map_or(1, |an| an.reached_end_of_animation)
    }

    /// Port of `PcNameEntryInit` (PC/pcmisc.cpp:377): pad the name with '-'.
    pub fn pc_name_entry_init(&mut self) {
        let ts = &mut self.frontend.text_string;
        if ts.len() < NAME_LENGTH + 1 {
            ts.resize(NAME_LENGTH + 1, 0);
        }
        for c in ts.iter_mut().take(NAME_LENGTH) {
            if *c == 0 {
                *c = b'-';
            }
        }
        ts[NAME_LENGTH] = 0;
    }

    /// Host keyboard input while `textEntry != 0` (the handler lived in the PC `Main.cpp`,
    /// which is not in the preserved source): `Some(c)` types a character at the cursor
    /// position `NAME_LENGTH - textEntry` (upper-cased); `None` (Enter) finishes the entry.
    pub fn name_entry_key(&mut self, key: Option<u8>) {
        if self.frontend.text_entry <= 0 {
            return;
        }
        match key {
            Some(c) => {
                let pos = NAME_LENGTH - self.frontend.text_entry as usize;
                if self.frontend.text_string.len() < NAME_LENGTH + 1 {
                    self.frontend.text_string.resize(NAME_LENGTH + 1, 0);
                }
                self.frontend.text_string[pos] = c.to_ascii_uppercase();
                self.frontend.text_entry -= 1;
            }
            None => self.frontend.text_entry = 0,
        }
    }

    /// Pad name entry: port of `PsxNameEntryFrame` (PSX/temp_psx.c:575) for the host to call
    /// once per tick while `text_entry > 0` with player 1's debounced pad bits (the PC handler
    /// is missing from the preserved source, see [`Self::name_entry_key`]). Not called by
    /// `tick`, so traces are unaffected. // controller
    ///
    /// LEFT steps back, RIGHT/CROSS advance (CROSS on the last letter finishes), UP/DOWN cycle
    /// A..Z and space, START finishes. The PSX keeps the cursor in a global `cursPos` that is
    /// never reset between entries (BUG(original): a second entry starts on the last letter);
    /// here it is derived from `text_entry` (`cursPos = NAME_LENGTH - textEntry`), which
    /// starts every entry on the first letter like the PC keyboard path.
    pub fn psx_name_entry_frame(&mut self, debounce: u16) {
        use crate::levplay::{PAD_LEFT, PAD_RIGHT, PAD_START};
        use crate::pcaudio::{gen_sfx, GEN_COLLECT_BABY, GEN_FROG_HOP, GEN_SUPER_HOP};
        if self.frontend.text_entry <= 0 {
            return;
        }
        let ts = &mut self.frontend.text_string;
        if ts.len() < NAME_LENGTH + 1 {
            ts.resize(NAME_LENGTH + 1, 0);
        }
        // PsxNameEntryInit (temp_psx.c:560): blanks -> '-', first letter 'A'.
        for c in ts.iter_mut().take(NAME_LENGTH) {
            if *c == 0 {
                *c = b'-';
            }
        }
        let mut curs = NAME_LENGTH.saturating_sub(self.frontend.text_entry as usize).min(NAME_LENGTH - 1);
        if curs == 0 && ts[0] == b'-' {
            ts[0] = b'A';
        }
        let mut sfx = Vec::new();
        let mut done = false;
        if debounce & PAD_LEFT != 0 && curs > 0 {
            ts[curs] = b'-';
            curs -= 1;
            sfx.push(GEN_FROG_HOP);
        }
        if debounce & (PAD_RIGHT | PAD_CROSS) != 0 && (curs < NAME_LENGTH - 1 || debounce & PAD_CROSS != 0) {
            curs += 1;
            if curs == NAME_LENGTH {
                done = true;
                curs -= 1;
                sfx.push(GEN_COLLECT_BABY);
            } else {
                ts[curs] = b'A';
                ts[curs + 1] = if curs == NAME_LENGTH - 1 { 0 } else { b'-' };
                sfx.push(GEN_SUPER_HOP);
            }
        }
        if debounce & PAD_UP != 0 {
            sfx.push(GEN_FROG_HOP);
            ts[curs] = match ts[curs] {
                b' ' | b'-' => b'A',
                b'Z' => b' ',
                c if c < b'Z' => c + 1,
                c => c,
            };
        }
        if debounce & PAD_DOWN != 0 {
            sfx.push(GEN_FROG_HOP);
            ts[curs] = match ts[curs] {
                b'A' => b' ',
                b' ' | b'-' => b'Z',
                c if c > b'A' => c - 1,
                c => c,
            };
        }
        if debounce & PAD_START != 0 {
            done = true;
        }
        if done {
            for c in ts.iter_mut().take(NAME_LENGTH) {
                if *c == b'-' {
                    *c = 0;
                }
            }
            ts[NAME_LENGTH] = 0;
            self.frontend.text_entry = 0;
        } else {
            self.frontend.text_entry = (NAME_LENGTH - curs) as i32;
        }
        for s in sfx {
            self.play_sample(gen_sfx(s), None, 0, SAMPLE_VOLUME, -1);
        }
    }

    // ------------------------------------------------------------ multiplayer win screens

    /// Port of `CreateOverlaysFromLogo` (Common/frontend.c:293): one 32x32 sprite overlay
    /// per non-zero tile, texture `"%s%02d"`. Unused in the PC game.
    pub fn create_overlays_from_logo(&mut self, logo: &Logo, x: i32, y: i32) {
        let mut tex = logo.tiles.iter();
        for i in 0..logo.height as i32 {
            for j in 0..logo.width as i32 {
                let t = tex.next().copied().unwrap_or(0);
                if t != 0 {
                    let texture = format!("{}{:02}", logo.texname, t);
                    self.overlays.create_and_add_sprite_overlay((x + j * 32) as i16, (y + i * 32) as i16, Some(&texture), 32, 32, 255, 0);
                }
            }
        }
    }

    /// Port of `StartMultiWinGame` (Common/frontend.c:510).
    pub fn start_multi_win_game(&mut self) {
        self.screen_fade(255, 128, 30);
        self.fadeout.keep_fade = true;
        self.fadeout.fade_text = false;

        let mut mg = MultiGameWin::default();
        let ct = self.hud.multi_hud.centre_text;
        if let Some(o) = self.textover.get_mut(ct) {
            o.y_pos = 1600;
        }
        match self.multi.multiplayer_mode {
            MULTIMODE_RACE => {
                self.game_state.mode = MULTI_WINRACE_MODE;
                for i in 0..self.frogger.num_frogs.clamp(0, 4) as usize {
                    let s = race_time_text(self.multi.mpl[i].timer);
                    let t = self.textover.create_and_add_text_overlay(5000, (1024 + 512 * i) as i16, &s, false, 255, Font::Normal, TEXTOVERLAY_SHADOW as i8);
                    if let Some(o) = self.textover.get_mut(t) {
                        o.speed = 50 << 12;
                    }
                    mg.count_text_over[i] = t;
                }
                mg.mode = MGW_MOVING;
            }
            MULTIMODE_COLLECT | MULTIMODE_BATTLE => {
                mg.mode = MGW_MOVING;
                self.game.mode_timer.init(3);
                let s = self.multi_win_string(false);
                self.set_countdown_string(s);
                if let Some(o) = self.textover.get_mut(ct) {
                    o.a = 0;
                    o.scale = 4096;
                }
                self.game_state.mode = MULTI_WINCOLLECT_MODE;
            }
            _ => {}
        }
        self.frontend.mg_win = Some(mg);

        if self.multi.match_winner != -1 {
            self.backdrop.draw_game = false;
            self.init_tiled_backdrop("LOGO");
            self.hide_all_sprites();
        }
    }

    /// The `countdownString` of the win screens: match winner, round draw or round winner
    /// (`STR_MULTI_WINMATCH` / `STR_MULTI_DRAW` / `STR_MULTI_PLAYERWINS` with the winner's
    /// character name; `NetGetPlayerName` for `MULTIREMOTE` is not ported). `finished`
    /// selects `RunMultiWinRace`'s order (match winner first in both).
    fn multi_win_string(&self, _finished: bool) -> String {
        let gw = self.multi.game_winner;
        let name = |g: &Game| {
            let ch = if gw >= 0 { g.frogger.player[gw as usize & 3].character as i32 } else { 0 };
            g.gamestring(STR_CHAR_NAME_1 + ch)
        };
        if self.multi.match_winner != -1 {
            c_sprintf(&self.gamestring(STR_MULTI_WINMATCH), &[FmtArg::Str(name(self))])
        } else if gw as i32 == MULTI_ROUND_DRAW {
            c_sprintf(&self.gamestring(STR_MULTI_DRAW), &[])
        } else {
            c_sprintf(&self.gamestring(STR_MULTI_PLAYERWINS), &[FmtArg::Str(name(self))])
        }
    }

    /// `DEC_ALPHA` on a sprite overlay.
    fn sprite_dec_alpha(&mut self, s: Option<crate::ids::OverlayId>) {
        let gs = self.game_speed;
        if let Some(o) = self.overlays.get_mut(s) {
            crate::hud::dec_alpha(&mut o.a, &mut o.draw, gs);
        }
    }

    /// Sets the colour of `multiHud.backChars[w]`, `penaliseText[w]` and `countTextOver[w]`.
    fn multi_win_colour(&mut self, w: usize, r: u8, g: u8, b: u8) {
        let m = self.hud.multi_hud.clone();
        if let Some(o) = self.overlays.get_mut(m.back_chars[w]) {
            (o.r, o.g, o.b) = (r, g, b);
        }
        let ct = self.frontend.mg_win.as_ref().and_then(|w2| w2.count_text_over[w]);
        for t in [m.penalise_text[w], ct] {
            if let Some(o) = self.textover.get_mut(t) {
                (o.r, o.g, o.b) = (r, g, b);
            }
        }
    }

    /// The common "next round" tail of the win screens: back to `INGAME_MODE`.
    fn multi_win_next_round(&mut self) {
        self.screen_fade(128, 255, 30);
        self.fadeout.keep_fade = false;
        self.fadeout.fade_text = false;
        self.frontend.mg_win = None;
        self.frame_count = 2; // Not 0, because 0 and 1 are used to trigger initialisation stuff elsewhere
        self.reset_multiplayer();
        self.game_state.mode = INGAME_MODE;
    }

    /// Port of `RunMultiWinRace` (Common/frontend.c:637).
    pub fn run_multi_win_race(&mut self) {
        self.frontend.check_menu_keys = 1;
        let Some(mode) = self.frontend.mg_win.as_ref().map(|m| m.mode) else { return };
        let num_frogs = self.frogger.num_frogs.clamp(0, 4) as usize;
        let m = self.hud.multi_hud.clone();
        let cto = self.frontend.mg_win.as_ref().expect("mgWin").count_text_over;
        // `n = mgWin->current` at entry (the MOVING loop reuses it).
        let n = self.frontend.mg_win.as_ref().expect("mgWin").current as i32;
        if mode == MGW_MOVING {
            self.text_dec_alpha(m.time_text_hsec);
            self.text_dec_alpha(m.time_text_sec);
            self.text_dec_alpha(m.time_text_min);
            let mut num_arrived = 0;
            for n in 0..num_frogs {
                let ni = n as i32;
                self.slide_sprite_overlay_to_pos(m.back_chars[n], 700, 1024 + 512 * ni - 128, BACK_CHARS_X[n], BACK_CHARS_Y[n], 50);
                self.slide_text_overlay_to_pos(m.penalise_text[n], 1300, 1024 + 512 * ni, BACK_TEXT_X[n], BACK_TEXT_Y[n], 50);
                if let Some(t) = self.textover.get_mut(cto[n]) {
                    t.x_pos_to = 2000;
                    if t.x_pos == t.x_pos_to {
                        num_arrived += 1;
                    }
                }
            }
            // If overlays have got to proper positions, start counting
            if num_arrived == num_frogs {
                let mg = self.frontend.mg_win.as_mut().expect("mgWin");
                mg.mode = MGW_COUNTING;
                mg.countdown = 0;
                mg.current = 0;
            }
        } else if mode == MGW_COUNTING {
            let add = if self.levplay.pad_data.digital[0] & PAD_CROSS != 0 { 5 } else { self.act_frame_count.wrapping_sub(self.last_act_frame_count) };
            let mg = self.frontend.mg_win.as_mut().expect("mgWin");
            mg.countdown = mg.countdown.wrapping_add(add);
            let t = cto[mg.current as usize & 3];
            let n = n as usize & 3;
            // Every half a second, countdown a penalty point and make a noise
            if mg.countdown > 5 {
                mg.countdown = 0;
                if self.multi.mpl[n].penalty != 0 {
                    let mp = &mut self.multi.mpl[n];
                    mp.penalty = mp.penalty.wrapping_sub(60);
                    mp.timer = mp.timer.wrapping_add(60);
                    let s = race_time_text(mp.timer);
                    if let Some(o) = self.textover.get_mut(t) {
                        o.text = s;
                    }
                    self.play_sample(f2_formats::crc::crc("clocktick"), None, 0, SAMPLE_VOLUME, -1);
                } else if n + 1 < num_frogs {
                    mg.current += 1;
                } else {
                    // Else we've finished, so prepare to replay.
                    for i in 0..num_frogs {
                        let wins = (self.multi.mpl[i].wins.max(0) as usize).min(3);
                        for j in 0..wins {
                            self.overlays.enable(m.trophies[i][j]);
                        }
                    }
                    self.frontend.mg_win.as_mut().expect("mgWin").mode = MGW_FLASHING;
                    if let Some(o) = self.textover.get_mut(m.centre_text) {
                        o.a = 0;
                        o.y_pos = 900;
                    }
                    self.game.mode_timer.init(3);
                }
            }
        } else if mode == MGW_FLASHING {
            let afc = self.act_frame_count;
            self.game.mode_timer.update(-1, afc);
            let gw = self.multi.game_winner as i32;
            if gw == MULTI_ROUND_DRAW || self.game.mode_timer.time == 0 {
                self.frontend.mg_win.as_mut().expect("mgWin").mode = MGW_FINISHED;
                self.game.mode_timer.init(4);
            } else if (0..4).contains(&gw) {
                let a = afc as i32;
                self.multi_win_colour(gw as usize, pulse(rsin(a.wrapping_mul(4000))), pulse(rcos(a.wrapping_mul(4000))), 10);
            }
        } else if mode == MGW_FINISHED {
            if self.game.mode_timer.time != 0 {
                let gw = self.multi.game_winner as i32;
                if gw != MULTI_ROUND_DRAW && (0..4).contains(&gw) {
                    self.multi_win_colour(gw as usize, 255, 255, 255);
                }
                if self.multi.match_winner != -1 {
                    for i in 0..num_frogs {
                        self.slide_sprite_overlay_to_pos(m.back_chars[i], BACK_CHARS_X[i], BACK_CHARS_Y[i], 700, 1024 + 512 * n - 128, 50);
                        self.text_dec_alpha(m.penalise_text[i]);
                        if let Some(o) = self.textover.get_mut(cto[i]) {
                            o.x_pos_to = 5200;
                        }
                        self.text_dec_alpha(cto[i]);
                    }
                } else {
                    self.text_inc_alpha(m.time_text_hsec, 255);
                    self.text_inc_alpha(m.time_text_sec, 255);
                    self.text_inc_alpha(m.time_text_min, 255);
                    for i in 0..num_frogs {
                        self.slide_sprite_overlay_to_pos(m.back_chars[i], BACK_CHARS_X[i], BACK_CHARS_Y[i], 700, 1024 + 512 * n - 128, 50);
                        self.slide_text_overlay_to_pos(m.penalise_text[i], BACK_TEXT_X[i], BACK_TEXT_Y[i], 1300, 1024 + 512 * n, 50);
                        if let Some(o) = self.textover.get_mut(cto[i]) {
                            o.x_pos_to = 5200;
                        }
                        self.text_dec_alpha(cto[i]);
                    }
                }
                self.text_inc_alpha(m.centre_text, 255);
            }

            let s = self.multi_win_string(true);
            self.set_countdown_string(s);
            if let Some(o) = self.textover.get_mut(m.centre_text) {
                o.scale = 4096;
            }

            if self.game.mode_timer.time != 0 {
                let afc = self.act_frame_count;
                self.game.mode_timer.update(-1, afc);
            }
            if self.game.mode_timer.time == 0 {
                if self.multi.match_winner != -1 {
                    self.do_end_multi();
                    return;
                }
                for t in cto.iter().take(num_frogs) {
                    self.textover.sub_text_overlay(*t);
                }
                self.multi_win_next_round();
            }
        }
        self.up_date_multiplayer_info();
    }

    /// Port of `RunMultiWinCollect` (Common/frontend.c:822).
    pub fn run_multi_win_collect(&mut self) {
        self.frontend.check_menu_keys = 1;
        let Some(mode) = self.frontend.mg_win.as_ref().map(|m| m.mode) else { return };
        let afc = self.act_frame_count;
        let m = self.hud.multi_hud.clone();
        let num_frogs = self.frogger.num_frogs.clamp(0, 4) as usize;
        match mode {
            MGW_FLASHING => {
                if self.game.mode_timer.time != 0 {
                    self.game.mode_timer.update(-1, afc);
                    self.text_inc_alpha(m.centre_text, 255);
                }
                if self.game.mode_timer.time == 0 {
                    if self.multi.match_winner != -1 {
                        self.do_end_multi();
                        return;
                    }
                    if let Some(o) = self.textover.get_mut(m.centre_text) {
                        o.draw = 0;
                        o.y_pos = 900;
                    }
                    self.multi_win_next_round();
                }
            }
            MGW_MOVING | MGW_COUNTING => {
                let max_score = (0..num_frogs).map(|j| self.multi.mpl[j].score).fold(0, i16::max);
                let num_draw = (0..num_frogs).filter(|&j| self.multi.mpl[j].score == max_score).count() as i32;
                let mut num = 0;
                for j in 0..num_frogs {
                    if self.multi.mpl[j].score == max_score {
                        let cx = 2048 - 256 - (num_draw - 1) * 256 + num * 512;
                        if mode == MGW_MOVING {
                            self.slide_sprite_overlay_to_pos(m.back_chars[j], cx, 2100, BACK_CHARS_X[j], BACK_CHARS_Y[j], 30);
                        } else {
                            self.slide_sprite_overlay_to_pos(m.back_chars[j], BACK_CHARS_X[j], BACK_CHARS_Y[j], cx, 2100, 30);
                        }
                        num += 1;
                    }
                }
                self.game.mode_timer.update(-1, afc);
                self.text_inc_alpha(m.centre_text, 255);
                if self.game.mode_timer.time == 0 {
                    if mode == MGW_MOVING {
                        for i in 0..num_frogs {
                            let wins = (self.multi.mpl[i].wins.max(0) as usize).min(3);
                            for j in 0..wins {
                                self.overlays.enable(m.trophies[i][j]);
                            }
                        }
                        self.game.mode_timer.init(2);
                        self.frontend.mg_win.as_mut().expect("mgWin").mode = MGW_COUNTING;
                    } else {
                        self.frontend.mg_win.as_mut().expect("mgWin").mode = MGW_FLASHING;
                    }
                }
            }
            _ => {}
        }
    }

    /// Port of `RunMultiWinBattle` (Common/frontend.c:924) (unused: `MULTI_WINBATTLE_MODE`
    /// dispatches to `RunMultiWinCollect`).
    pub fn run_multi_win_battle(&mut self) {
        self.frontend.check_menu_keys = 1;
        let mode = self.frontend.mg_win.as_ref().map_or(MGW_FINISHED, |m| m.mode);
        let ct = self.hud.multi_hud.centre_text;
        if mode == MGW_FLASHING && self.game.mode_timer.time != 0 {
            let afc = self.act_frame_count;
            self.game.mode_timer.update(-1, afc);
            self.text_inc_alpha(ct, 255);
        }
        if self.game.mode_timer.time == 0 {
            if self.multi.match_winner != -1 {
                self.do_end_multi();
                return;
            }
            if let Some(o) = self.textover.get_mut(ct) {
                o.draw = 0;
                o.y_pos = 900;
            }
            self.multi_win_next_round();
        }
    }

    /// Port of `DoEndMulti` (Common/frontend.c:998): restart / quit menu after a match.
    pub fn do_end_multi(&mut self) {
        if self.frontend.menu_option == -1 {
            self.frontend.menu_option = 0;
            self.frontend.option_selected = 0;
        }
        let mt = self.frontend.menu_text;
        if self.frontend.option_selected == 0 {
            self.text_inc_alpha(mt[0], 255);
            if mt[1].is_some() {
                self.text_inc_alpha(mt[1], 255);
            }
            let digital = self.levplay.pad_data.digital[0];
            if self.frontend.menu_option == 1 && digital & PAD_UP != 0 {
                self.frontend.menu_option = 0;
            } else if self.frontend.menu_option == 0 && digital & PAD_DOWN != 0 && self.game_state.multi == MULTILOCAL {
                self.frontend.menu_option = 1;
            } else if digital & PAD_CROSS != 0 {
                self.frontend.option_selected = 1;
            }
            let afc = self.act_frame_count as i32;
            let mo = (self.frontend.menu_option & 1) as usize;
            if let Some(t) = self.textover.get_mut(mt[mo]) {
                t.r = pulse(rsin(afc.wrapping_mul(4000)));
                t.g = pulse(rcos(afc.wrapping_mul(4000)));
                t.b = 10;
            }
            if let Some(t) = self.textover.get_mut(mt[1 - mo]) {
                t.r = 255;
                t.g = 255;
                t.b = 255;
            }
            return;
        }

        if !self.levplay.fading_out {
            if self.fadeout.keep_fade {
                if self.frontend.menu_option == 0 {
                    // MULTIREMOTE: PostQuitMessage(0) (network play; host).
                    self.screen_fade(128, 255, 30);
                    self.fadeout.fade_text = false;
                } else {
                    self.screen_fade(128, 0, 30);
                }
                self.fadeout.keep_fade = false;
            } else {
                self.free_tiled_backdrop();
                self.backdrop.draw_game = true;
                if self.frontend.menu_option == 0 {
                    self.frontend.check_menu_keys = 0;
                    if let Some(o) = self.textover.get_mut(self.hud.multi_hud.centre_text) {
                        o.y_pos = 900;
                    }
                    self.restart_multi();
                    self.game_state.mode = INGAME_MODE;
                    self.textover.disable(mt[0]);
                    self.textover.disable(mt[1]);
                } else {
                    self.frogger.player[0].character = FROG_FROGGER;
                    self.frogger.num_frogs = 1;
                    self.game_state.multi = SINGLEPLAYER;
                    self.game_state.mode = FRONTEND_MODE;
                    self.free_multiplayer();
                    self.init_level(worldid::FRONTEND, LEVELID_FRONTEND1);
                    self.frame_count = 0;
                }
                self.frontend.menu_option = -1;
            }
        } else {
            if self.frontend.menu_option == 1 {
                let m = self.hud.multi_hud.clone();
                for t in [m.centre_text, m.time_text_hsec, m.time_text_sec, m.time_text_min] {
                    if t.is_some() {
                        self.text_dec_alpha(t);
                    }
                }
                for i in 0..self.frogger.num_frogs.clamp(0, 4) as usize {
                    self.sprite_dec_alpha(m.back_chars[i]);
                    self.text_dec_alpha(m.penalise_text[i]);
                }
            }
            self.text_dec_alpha(mt[0]);
            if mt[1].is_some() {
                self.text_dec_alpha(mt[1]);
            }
        }
    }

    // ------------------------------------------------------------ hooks into other waves

    // wave G: RunFrontendGameLoop, RunPauseMenu, UpdateCheatText and the cheat flags are in
    // `crate::menus`; RunArtViewer is in `crate::options`.

    /// `arcadeHud.timeOutText->draw` (Common/hud.c): the hard-mode time ran out.
    pub fn arcade_hud_time_out_draw(&self) -> bool {
        self.textover.get(self.hud.arcade_hud.time_out_text).is_some_and(|t| t.draw != 0)
    }

    /// `storeFrameCount` (Common/hud.c:112): frame the level clock started.
    pub fn hud_store_frame_count(&self) -> u32 {
        self.hud.store_frame_count as u32
    }

    /// `matchWinner` (Common/multi.c).
    pub fn multi_match_winner(&self) -> i32 {
        self.multi.match_winner as i32
    }

    /// `gameWinner` (Common/multi.c).
    pub fn multi_game_winner(&self) -> i32 {
        self.multi.game_winner as i32
    }
}

/// `LOGO` (`frontend.h:18`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Logo {
    pub texname: String,
    pub width: u16,
    pub height: u16,
    pub tiles: [u16; 16],
}

/// `sprintf("%d:%02d:%02d", timer/3600, (timer%3600)/60, timer%60)` of the race win screen.
fn race_time_text(timer: u32) -> String {
    format!("{}:{:02}:{:02}", timer / 3600, (timer % 3600) / 60, timer % 60)
}

/// `TEXTOVERLAY_SHADOW` (`textover.h`).
pub const TEXTOVERLAY_SHADOW: u32 = 1 << 4;
/// `SPRITE_ADDITIVE` (`sprite.h`).
pub const SPRITE_ADDITIVE: u32 = 1 << 5;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_moves_like_print_text_overlays() {
        let mut o = UiOverlay::text(2048 + 4096, 850, "x", true, 255);
        o.x_pos_to = 2048;
        o.speed = 4096 * 75;
        let mut n = 0;
        while o.x_pos != o.x_pos_to {
            o.step(4096, false);
            n += 1;
        }
        // 75 units per frame over 4096: the 54th step leaves 46 < 75 and snaps.
        assert_eq!(n, 54);
        assert_eq!(o.speed, 0);
    }

    #[test]
    fn grade_thresholds_and_progression() {
        let mut g = Game::new();
        g.game_state.single = STORY_MODE;
        g.game_state.story_sequence_level = 1; // Garden 1
        g.frogger.player[0].world_num = worldid::GARDEN as u8;
        g.frogger.player[0].level_num = 0;
        let par = g.lvd_cur().par_time as i32;
        g.frontend.time_for_level = par - 1;
        g.start_level_complete();
        assert_eq!(g.frontend.grade, 0);
        assert_eq!(g.game_state.story_sequence_level, 2);
        let next = STORY_SEQUENCE[2];
        assert_eq!(g.layout.world_visual_data[next.world_num as usize].world_open, WORLD_OPEN);
        assert_eq!(g.layout.world_visual_data[next.world_num as usize].level_visual_data[next.level_num as usize].level_open, LEVEL_OPEN);
        assert_eq!(g.lvd_cur().level_completed, 1);
        assert_eq!(g.frontend.level_opened, 1);

        let mut g = Game::new();
        g.frogger.player[0].world_num = worldid::GARDEN as u8;
        g.frontend.time_for_level = par + par / 4;
        g.start_level_complete();
        assert_eq!(g.frontend.grade, 1);
        assert_eq!(g.game_state.story_sequence_level, 0, "arcade does not advance the story");
        g.frontend.time_for_level = par * 2;
        g.start_level_complete();
        assert_eq!(g.frontend.grade, 2);
        assert_eq!(g.frontend.level_opened, 0, "already completed");
    }

    #[test]
    fn psx_name_entry_with_pad() {
        use crate::levplay::{PAD_LEFT, PAD_RIGHT, PAD_START};
        let mut g = Game::new();
        g.frontend.text_entry = NAME_LENGTH as i32;
        g.pc_name_entry_init();
        g.psx_name_entry_frame(0); // first frame: 'A' under the cursor
        assert_eq!(&g.frontend.text_string[..3], b"A--");
        g.psx_name_entry_frame(PAD_DOWN); // A -> space -> Z
        g.psx_name_entry_frame(PAD_DOWN);
        assert_eq!(g.frontend.text_string[0], b'Z');
        g.psx_name_entry_frame(PAD_UP); // Z -> space
        g.psx_name_entry_frame(PAD_UP); // space -> A
        g.psx_name_entry_frame(PAD_UP);
        assert_eq!(g.frontend.text_string[0], b'B');
        g.psx_name_entry_frame(PAD_RIGHT);
        assert_eq!((&g.frontend.text_string[..3], g.frontend.text_entry), (&b"BA-"[..], 2));
        g.psx_name_entry_frame(PAD_LEFT);
        assert_eq!((&g.frontend.text_string[..3], g.frontend.text_entry), (&b"B--"[..], 3));
        g.psx_name_entry_frame(PAD_CROSS);
        g.psx_name_entry_frame(PAD_CROSS);
        assert_eq!((&g.frontend.text_string[..4], g.frontend.text_entry), (&b"BAA\0"[..], 1));
        g.psx_name_entry_frame(PAD_RIGHT); // RIGHT does not leave the last letter
        assert_eq!(g.frontend.text_entry, 1);
        g.psx_name_entry_frame(PAD_CROSS); // CROSS there finishes
        assert_eq!((&g.frontend.text_string[..4], g.frontend.text_entry), (&b"BAA\0"[..], 0));
        // START finishes early; '-' blanks become terminators.
        let mut g = Game::new();
        g.frontend.text_entry = NAME_LENGTH as i32;
        g.pc_name_entry_init();
        g.psx_name_entry_frame(PAD_START);
        assert_eq!((&g.frontend.text_string[..4], g.frontend.text_entry), (&b"A\0\0\0"[..], 0));
        g.psx_name_entry_frame(PAD_UP); // inactive: no-op
        assert_eq!(g.frontend.text_string[0], b'A');
    }

    #[test]
    fn level_complete_runs_to_menu_and_records_best_time() {
        let mut g = Game::new();
        g.frogger.player[0].world_num = worldid::GARDEN as u8;
        g.game_state.single = crate::define::ARCADE_MODE;
        g.game_state.mode = LEVELCOMPLETE_MODE;
        g.frontend.time_for_level = 100;
        g.start_level_complete();
        assert_eq!(g.frontend.grade, 0);
        // No frog/babies in this unit test (the original needs frog[0]): skip the drop.
        g.frontend.lev_complete_state = LEV_COMPLETE_COUNTING_COINS;
        let mut ticks = 0;
        while g.frontend.lev_complete_state != LEV_COMPLETE_ENTER_NAME {
            g.tick(crate::TickInput::default());
            ticks += 1;
            assert!(ticks < 2000, "stuck in state {}", g.frontend.lev_complete_state);
        }
        g.name_entry_key(Some(b'a'));
        g.name_entry_key(Some(b'b'));
        g.name_entry_key(Some(b'c'));
        while g.frontend.lev_complete_state != LEV_COMPLETE_MENU {
            g.tick(crate::TickInput::default());
            ticks += 1;
            assert!(ticks < 4000);
        }
        assert_eq!(g.lvd_cur().par_time, 100);
        assert_eq!(&g.lvd_cur().par_name[..4], b"ABC\0");
        // Wait for the menu to slide in, choose "quit" and confirm.
        while !g.frontend.o_text[0].as_ref().is_some_and(|o| o.x_pos == o.x_pos_to) {
            g.tick(crate::TickInput::default());
        }
        g.tick(crate::TickInput { pads: [PAD_DOWN, 0, 0, 0] });
        g.tick(crate::TickInput::default());
        g.tick(crate::TickInput { pads: [PAD_CROSS, 0, 0, 0] });
        assert_eq!(g.frontend.lev_complete_state, LEV_COMPLETE_FADING_OUT);
        assert!(g.take_effects().iter().any(|e| matches!(e, Effect::SaveGame { .. })));
        for _ in 0..100 {
            g.tick(crate::TickInput::default());
        }
        assert_eq!(g.game_state.mode, FRONTEND_MODE);
        assert_eq!(g.frogger.player[0].world_num as u32, worldid::FRONTEND);
    }
}
