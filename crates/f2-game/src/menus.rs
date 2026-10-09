//! Port of `Common/menus.c` (wave G): the pause menu, the pause-mode cheat combos, the 3D
//! frontend's per-frame loop (`RunFrontendGameLoop`: title HUD arrows, arcade-machine screen,
//! sparkles, demo idle timer, the options/multiplayer/arcade/book tiles), the arcade level
//! list (`DoArcadeMenu`) and `CheckForExtras`.
//!
//! Overlays live in the real overlay lists (`crate::textover`, `crate::overlays`). Where the
//! C points a `TEXTOVERLAY.text` at a buffer it later rewrites (`worldStr`, `levelStr[]`,
//! `cheatStr`, ...), the port keeps the buffer in the state and copies it into the overlay
//! where the C writes the buffer.
//!
//! Render-facing state the renderer must use: `arcade_screen_copy` (the texture currently
//! copied onto the arcade machine's `TEST5` screen, `CopyTexture`), and everything in
//! `crate::options` (book page / portraits).

#![allow(clippy::needless_range_loop, clippy::collapsible_match)] // C loop/switch shapes kept

use f2_math::{fmul, rcos, rsin, FVector, SVector};
use serde::{Deserialize, Serialize};

use crate::anim::SAMPLE_VOLUME;
use crate::define::mode::*;
use crate::define::{DIFFICULTY_NORMAL, MULTIREMOTE, SINGLEPLAYER, STORY_MODE};
use crate::effects::Effect;
use crate::frogger::{FROG_FROGGER, FROG_NUMFROGS};
use crate::frogmove::FROGSTATUS_ISDEAD;
use crate::frontend::{BACK_CHARS_X, BACK_CHARS_Y, FADE_SPEED};
use crate::ids::{OverlayId, TextOverlayId};
use crate::language::{c_sprintf, FmtArg};
use crate::layout::{worldid, LEVELID_FRONTEND1, LEVELID_FRONTEND4, LEVEL_OPEN, WORLD_OPEN};
use crate::levplay::{PAD_CROSS, PAD_DOWN, PAD_LEFT, PAD_RIGHT, PAD_SQUARE, PAD_START, PAD_TRIANGLE, PAD_UP};
use crate::options::*;
use crate::pcaudio::{gen_sfx, GEN_DEATHDROWN};
use crate::sprite::{SPRITE_ADDITIVE, SPRITE_SUBTRACTIVE};
use crate::story::{NUM_STORY_LEVELS, STORY_SEQUENCE};
use crate::textover::{Font, TEXTOVERLAY_PAUSED, TEXTOVERLAY_SHADOW};
use crate::timer::Timer;
use crate::Game;

/// `STR_*` ids used by menus.c/options.c (`lang.h`, PC build numbering).
pub mod strs {
    pub use crate::language::str_id::*;
    pub const STR_CHEAT_1: i32 = 170;
    pub const STR_ON: i32 = 168;
    pub const STR_OFF: i32 = 169;
    pub const STR_YES: i32 = 183;
    pub const STR_NO: i32 = 184;
    pub const STR_CHAPTER: i32 = 142;
    #[allow(non_upper_case_globals)]
    pub const STR_CHAPTER_1a: i32 = 119;
    #[allow(non_upper_case_globals)]
    pub const STR_CHAPTER_1b: i32 = 120;
    pub const STR_OPTIONS: i32 = 248;
    pub const STR_START_GAME: i32 = 249;
    pub const STR_OPTIONS_1: i32 = 103;
    pub const STR_OPTIONS_2: i32 = 104;
    pub const STR_STORYMODE: i32 = 80;
    pub const STR_MULTIPLAYER: i32 = 114;
    pub const STR_ARCADEMODE: i32 = 79;
    pub const STR_SELECT_LEVEL: i32 = 116;
    pub const STR_PAR: i32 = 139;
    pub const STR_SET_BY: i32 = 140;
    pub const STR_COINS: i32 = 141;
    pub const STR_EXTRAS_1: i32 = 106;
    pub const STR_EXTRAS_2: i32 = 107;
    pub const STR_DIFFICULTY_1: i32 = 244;
    pub const STR_FMV_1: i32 = 153;
    pub const STR_FMV_2: i32 = 154;
    pub const STR_WARNING_1: i32 = 234;
    pub const STR_MUS_VOL: i32 = 227;
    pub const STR_SFX_VOL: i32 = 228;
    pub const STR_SOUND_INSTR_1: i32 = 232;
    pub const STR_SOUND_INSTR_2: i32 = 233;
    pub const STR_TRIANGLE_BACK: i32 = 177;
    pub const STR_SELECT_NUM_PLAYERS: i32 = 115;
    pub const STR_NEED_PADS1: i32 = 221;
    pub const STR_NEED_PADS2: i32 = 222;
    pub const STR_PLAYERS: i32 = 88;
    pub const STR_PLAYER: i32 = 87;
    pub const STR_SELECT_CHAR: i32 = 117;
    pub const STR_X_SELECT_CHAR: i32 = 118;
    pub const STR_CREDITS_1: i32 = 251;
}
use strs::*;

/// `BOOK_NUM_PAGES` (`menus.h:24`).
pub const BOOK_NUM_PAGES: usize = 10;
// Frontend tile numbers (`menus.h:26`).
pub const TILENUM_START: u32 = 204;
pub const TILENUM_OPTIONS: u32 = 1;
pub const TILENUM_CHOICE: u32 = 2;
pub const TILENUM_MULTI: u32 = 3;
pub const TILENUM_ARCADE: u32 = 4;
pub const TILENUM_BOOK: u32 = 5;
/// `MAX_LEVELSTRING` (`menus.h:57`).
pub const MAX_LEVELSTRING: usize = 10;
/// `MAX_SOUND_VOL` (`menus.h:58`).
pub const MAX_SOUND_VOL: i32 = 100;
/// `LONG_DEMO_WAIT` (`menus.c:56`).
pub const LONG_DEMO_WAIT: i32 = 300;
/// `CHEAT_TEXT_TIME` (`menus.c:114`).
pub const CHEAT_TEXT_TIME: u16 = 2;
/// `MAX_SPARKLES` (`menus.c:832`).
pub const MAX_SPARKLES: usize = 15;

// Cheats (`menus.h:80`).
pub const CHEAT_OPEN_ALL_LEVELS: usize = 0;
pub const CHEAT_INFINITE_LIVES: usize = 1;
pub const CHEAT_OPEN_ALL_CHARS: usize = 2;
pub const CHEAT_OPEN_ALL_EXTRAS: usize = 3;
pub const CHEAT_INVULNERABILITY: usize = 4;
pub const CHEAT_SKIP_LEVEL: usize = 5;
pub const CHEAT_MAD_GARIBS: usize = 6;
pub const CHEAT_EXTRA_LEVELS: usize = 7;
pub const NUMCHEATCOMBOS: usize = 8;

/// `CHEAT_COMBO` (`menus.h:68`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheatCombo {
    pub key_string: [u16; 12],
    pub state: i8,
    pub toggle: i8,
}

const fn cc(k: [u16; 8], state: i8, toggle: i8) -> CheatCombo {
    let mut key_string = [0u16; 12];
    let mut i = 0;
    while i < 8 {
        key_string[i] = k[i];
        i += 1;
    }
    CheatCombo { key_string, state, toggle }
}

const U: u16 = PAD_UP;
const D: u16 = PAD_DOWN;
const L: u16 = PAD_LEFT;
const R: u16 = PAD_RIGHT;

/// `cheatCombos[NUMCHEATCOMBOS]` (`menus.c:122`): hold square in the pause menu, press the
/// d-pad sequence, release square.
pub const CHEAT_COMBOS: [CheatCombo; NUMCHEATCOMBOS] = [
    cc([U, D, L, R, R, R, D, L], 0, -1), // CHEAT_OPEN_ALL_LEVELS
    cc([D, D, U, D, R, D, U, U], 0, 1),  // CHEAT_INFINITE_LIVES
    cc([L, R, L, L, L, U, L, L], 0, 0),  // CHEAT_OPEN_ALL_CHARS
    cc([R, U, U, D, R, R, D, R], 0, 0),  // CHEAT_OPEN_ALL_EXTRAS
    cc([L, L, U, L, D, R, R, R], 0, 0),  // CHEAT_INVULNERABILITY
    cc([R, L, U, U, U, R, L, L], 0, 0),  // CHEAT_SKIP_LEVEL
    cc([R, L, R, L, U, U, L, R], 0, 1),  // CHEAT_MAD_GARIBS
    cc([U, U, D, D, L, R, 0, 0], 0, 0),  // CHEAT_EXTRA_LEVELS
];

/// `titleHudX/Y[2][4]` (`menus.c:156`, PC).
pub const TITLE_HUD_X: [[i16; 4]; 2] = [[1898, 1898, 0, 3818], [1898, 1898, 128, 3818 - 128]];
pub const TITLE_HUD_Y: [[i16; 4]; 2] = [[0, 3818, 1898, 1898], [128, 3818 - 128, 1898, 1898]];
/// `titleHudOn[7][4]` (`menus.c:164`).
pub const TITLE_HUD_ON: [[u8; 4]; 7] = [[0, 0, 1, 1], [0, 1, 1, 0], [1, 1, 1, 1], [0, 1, 1, 1], [0, 0, 1, 1], [1, 1, 1, 1], [0, 1, 0, 1]];
/// `titleHudName[4]` (`menus.c:175`).
pub const TITLE_HUD_NAME: [&str; 4] = ["BUT_UP", "BUT_DOWN", "BUT_LEFT", "BUT_RIGH"];
/// `chapterPic[BOOK_NUM_PAGES]` (`menus.c:1843`).
pub const CHAPTER_PIC: [&str; BOOK_NUM_PAGES] =
    ["CHAPTER1", "CHAPTER2", "CHAPTER3", "CHAPTER4", "CHAPTER5", "CHAPTER6", "CHAPTER7", "CHAPTER8", "CHAPTER9", "CHAPTER10"];

/// Globals of `menus.c`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenusState {
    /// `fogStore`.
    pub fog_store: i32,
    /// `quittingLevel`: 1 restart, 2 quit (after the confirm).
    pub quitting_level: i32,
    /// `doneTraining`.
    pub done_training: i32,
    /// `lastArcade`: tile to put the frog back on when returning to the frontend.
    pub last_arcade: u32,
    /// `storeCamSource` ... `storeCamVect`: frontend camera kept across a game.
    pub store_cam_source: FVector,
    pub store_curr_cam_source: FVector,
    pub store_cam_target: FVector,
    pub store_curr_cam_target: FVector,
    pub store_cam_dist: FVector,
    pub store_curr_cam_dist: FVector,
    pub store_cam_offset: FVector,
    pub store_curr_cam_offset: FVector,
    pub store_cam_vect: FVector,
    /// `oldDiffMode` (restored by `RunDemoMode`, Common/levplay.c:138).
    pub old_diff_mode: i32,
    /// `pauseConfirmMode`.
    pub pause_confirm_mode: i32,
    /// `pauseGameSpeed` (never assigned in the original: 0).
    pub pause_game_speed: i32,
    /// `frogLogo`.
    pub frog_logo: Option<OverlayId>,
    /// `fadingLogos`.
    pub fading_logos: bool,
    /// `playDemos` (FINAL_MASTER: 1).
    pub play_demos: bool,
    /// `cheatActivated`.
    pub cheat_activated: i32,
    /// `cheatTimer`.
    pub cheat_timer: Timer,
    /// `cheatText` / `cheatStr`.
    pub cheat_text: Option<TextOverlayId>,
    pub cheat_str: String,
    /// `cheatCombos[]`.
    pub cheat_combos: [CheatCombo; NUMCHEATCOMBOS],
    /// `currentCheatCombo[12]`.
    pub current_cheat_combo: [i32; 12],
    /// `currCheat`.
    pub curr_cheat: i32,
    /// `currentPauseSelection`.
    pub current_pause_selection: u32,
    /// `titleHud[4]` / `titleHudText[4]`: the d-pad arrows around the frontend.
    pub title_hud: [Option<OverlayId>; 4],
    pub title_hud_text: [Option<TextOverlayId>; 4],
    /// `pauseFrameCount`.
    pub pause_frame_count: i32,
    /// `pauseFaded` (Dreamcast screensaver; stays 0).
    pub pause_faded: i32,
    /// `yesText` / `noText` of the pause confirm.
    pub yes_text: Option<TextOverlayId>,
    pub no_text: Option<TextOverlayId>,
    /// `sparkles[MAX_SPARKLES]` (menus.c's own, around the highlighted list entry).
    pub sparkles: [Option<OverlayId>; MAX_SPARKLES],
    /// `frontendTimer`: idle time before a demo runs.
    pub frontend_timer: Timer,
    /// `worldStr`, `levelStr[]`, `levelParStr[]`, `levelSetByStr[]`, `levelCoinStr[]`.
    pub world_str: String,
    pub level_str: [String; MAX_LEVELSTRING],
    pub level_par_str: [String; MAX_LEVELSTRING],
    pub level_set_by_str: [String; MAX_LEVELSTRING],
    pub level_coin_str: [String; MAX_LEVELSTRING],
    /// `cWorld`: world shown on the arcade machine.
    pub c_world: u32,
    /// `reachedPoint`.
    pub reached_point: i32,
    /// `arcadeScreenTex = FindTexture("TEST5")`: the arcade machine's screen texture.
    pub arcade_screen_tex: String,
    /// The texture last `CopyTexture`d onto `arcadeScreenTex` (render state).
    pub arcade_screen_copy: String,
    /// `arcadeTexNum`.
    pub arcade_tex_num: i32,
    /// `bounce`, `bounceDir`, `bounceSpeed`.
    pub bounce: i32,
    pub bounce_dir: i32,
    pub bounce_speed: i32,
    /// `goingToDemo`.
    pub going_to_demo: bool,
    /// `tileNum` (`Common/game.c:74`, created by the frontend loop; reset by FreeAllLists).
    pub tile_num: Option<TextOverlayId>,
}

impl Default for MenusState {
    fn default() -> Self {
        MenusState {
            fog_store: 0,
            quitting_level: 0,
            done_training: 0,
            last_arcade: 0,
            store_cam_source: FVector::default(),
            store_curr_cam_source: FVector::default(),
            store_cam_target: FVector::default(),
            store_curr_cam_target: FVector::default(),
            store_cam_dist: FVector::default(),
            store_curr_cam_dist: FVector::default(),
            store_cam_offset: FVector::default(),
            store_curr_cam_offset: FVector::default(),
            store_cam_vect: FVector::default(),
            old_diff_mode: 0,
            pause_confirm_mode: 0,
            pause_game_speed: 0,
            frog_logo: None,
            fading_logos: false,
            play_demos: true,
            cheat_activated: -1,
            cheat_timer: Timer::default(),
            cheat_text: None,
            cheat_str: String::new(),
            cheat_combos: CHEAT_COMBOS,
            current_cheat_combo: [0; 12],
            curr_cheat: -1,
            current_pause_selection: 0,
            title_hud: [None; 4],
            title_hud_text: [None; 4],
            pause_frame_count: 0,
            pause_faded: 0,
            yes_text: None,
            no_text: None,
            sparkles: [None; MAX_SPARKLES],
            frontend_timer: Timer::default(),
            world_str: String::new(),
            level_str: Default::default(),
            level_par_str: Default::default(),
            level_set_by_str: Default::default(),
            level_coin_str: Default::default(),
            c_world: 0,
            reached_point: 0,
            arcade_screen_tex: "TEST5".into(),
            arcade_screen_copy: String::new(),
            arcade_tex_num: 0,
            bounce: 0,
            bounce_dir: 1,
            bounce_speed: 5,
            going_to_demo: false,
            tile_num: None,
        }
    }
}

/// `127+((rsin(n*4000)+4096)*64)/4096`: the pulsing red of a highlighted entry.
pub fn pulse_r(n: i32) -> u8 {
    (127 + ((rsin(n.wrapping_mul(4000)) + 4096) * 64) / 4096) as u8
}

/// `127+((rcos(n*4000)+4096)*64)/4096`: the pulsing green of a highlighted entry.
pub fn pulse_g(n: i32) -> u8 {
    (127 + ((rcos(n.wrapping_mul(4000)) + 4096) * 64) / 4096) as u8
}

/// `strlen`-terminated byte name (`parName`, `texCRC`) as a string.
pub(crate) fn cstr(b: &[u8]) -> String {
    b.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect()
}

/// Sets `r,g,b` of a text overlay.
macro_rules! trgb {
    ($g:expr, $id:expr, $r:expr, $gg:expr, $b:expr) => {
        if let Some(o) = $g.textover.get_mut($id) {
            o.r = $r as u8;
            o.g = $gg as u8;
            o.b = $b as u8;
        }
    };
}
/// Sets `r,g,b` of a sprite overlay.
macro_rules! srgb {
    ($g:expr, $id:expr, $r:expr, $gg:expr, $b:expr) => {
        if let Some(o) = $g.overlays.get_mut($id) {
            o.r = $r as u8;
            o.g = $gg as u8;
            o.b = $b as u8;
        }
    };
}
/// Runs `$body` with `$o` bound to the text overlay, if any.
macro_rules! tx {
    ($g:expr, $id:expr, |$o:ident| $body:expr) => {
        if let Some($o) = $g.textover.get_mut($id) {
            $body;
        }
    };
}
/// Runs `$body` with `$o` bound to the sprite overlay, if any.
macro_rules! sx {
    ($g:expr, $id:expr, |$o:ident| $body:expr) => {
        if let Some($o) = $g.overlays.get_mut($id) {
            $body;
        }
    };
}
pub(crate) use {srgb, sx, trgb, tx};

impl Game {
    /// `tValue = FMul(gameSpeed,fadeSpeed)>>12`.
    fn t_value(&self) -> i32 {
        fmul(self.game_speed, FADE_SPEED) >> 12
    }

    /// `DEC_ALPHA(x)` (`menus.h:41`) on a text overlay.
    pub fn t_dec_alpha(&mut self, t: Option<TextOverlayId>) {
        let tv = self.t_value();
        tx!(self, t, |o| if (o.a as i32) > tv {
            o.a = (o.a as i32 - tv) as u8;
        } else {
            o.a = 0;
            o.draw = 0;
        });
    }

    /// `INC_ALPHA(x, y)` (`menus.h:55`) on a text overlay.
    pub fn t_inc_alpha(&mut self, t: Option<TextOverlayId>, y: i32) {
        let tv = self.t_value();
        tx!(self, t, |o| {
            o.draw = 1;
            if (o.a as i32) + tv < y {
                o.a = (o.a as i32 + tv) as u8;
            } else {
                o.a = y as u8;
            }
        });
    }

    /// `DEC_ALPHA(x)` on a sprite overlay.
    pub fn s_dec_alpha(&mut self, s: Option<OverlayId>) {
        let tv = self.t_value();
        sx!(self, s, |o| if (o.a as i32) > tv {
            o.a = (o.a as i32 - tv) as u8;
        } else {
            o.a = 0;
            o.draw = 0;
        });
    }

    /// `INC_ALPHA(x, y)` on a sprite overlay.
    pub fn s_inc_alpha(&mut self, s: Option<OverlayId>, y: i32) {
        let tv = self.t_value();
        sx!(self, s, |o| {
            o.draw = 1;
            if (o.a as i32) + tv < y {
                o.a = (o.a as i32 + tv) as u8;
            } else {
                o.a = y as u8;
            }
        });
    }

    /// `x->text = str` / writing the buffer a text overlay points at.
    pub fn set_text(&mut self, t: Option<TextOverlayId>, s: &str) {
        tx!(self, t, |o| o.text = s.to_string());
    }

    fn t_draw(&self, t: Option<TextOverlayId>) -> u8 {
        self.textover.get(t).map_or(0, |o| o.draw)
    }

    fn t_set_draw(&mut self, t: Option<TextOverlayId>, d: u8) {
        tx!(self, t, |o| o.draw = d);
    }

    fn s_set_draw(&mut self, s: Option<OverlayId>, d: u8) {
        sx!(self, s, |o| o.draw = d);
    }

    fn t_ypos(&self, t: Option<TextOverlayId>) -> i16 {
        self.textover.get(t).map_or(0, |o| o.y_pos)
    }

    /// `cheatCombos[CHEAT_SKIP_LEVEL].state` (read by `RunGameLoop`).
    pub fn cheat_skip_level(&self) -> bool {
        self.menus.cheat_combos[CHEAT_SKIP_LEVEL].state != 0
    }

    /// `cheatCombos[CHEAT_MAD_GARIBS].state` (read by collect.c, hud.c, frontend.c).
    pub fn cheat_mad_garibs(&self) -> bool {
        self.menus.cheat_combos[CHEAT_MAD_GARIBS].state != 0
    }

    /// `cheatCombos[CHEAT_INFINITE_LIVES].state` (set by the combo; nothing in the PC
    /// source reads it).
    pub fn cheat_infinite_lives(&self) -> bool {
        self.menus.cheat_combos[CHEAT_INFINITE_LIVES].state != 0
    }

    /// Port of `StartPauseMenu` (Common/menus.c:188).
    pub fn start_pause_menu(&mut self) {
        self.frontend.check_menu_keys = 1;
        self.menus.pause_frame_count = 0;
        self.menus.quitting_level = 0;
        self.menus.pause_confirm_mode = 0;
        self.frontend.restarting_level = false;

        self.game_state.old_mode = self.game_state.mode;
        self.game_state.mode = PAUSE_MODE;
        self.specfx.pause_mode = 1;

        self.menus.pause_faded = 0;
        self.pause_audio();
        self.effects.push(Effect::PauseAudio); // wave G

        self.menus.current_pause_selection = 0;
        self.textover.enable(self.hud.continue_text);
        self.textover.enable(self.hud.xselect_text);

        self.hud.pause_title_string = if self.frogger.num_frogs == 1 {
            self.gamestring(STR_PAUSE_MODE)
        } else {
            let c = self.frogger.player[self.frogger.pause_controller.clamp(0, 3) as usize].character as i32;
            format!("{} {}", self.gamestring(STR_CHAR_NAME_1 + c), self.gamestring(STR_PAUSE_MODE))
        };
        let s = self.hud.pause_title_string.clone();
        self.set_text(self.hud.pause_title_text, &s);
        self.textover.enable(self.hud.pause_title_text);
        trgb!(self, self.hud.continue_text, 255, 255, 255);
        let (quit, restart) = (self.hud.quit_text, self.hud.restart_text);
        if self.game_state.old_mode == FRONTEND_MODE || self.game_state.multi == MULTIREMOTE {
            let y = self.t_ypos(restart);
            tx!(self, quit, |o| {
                o.y_pos = y;
                o.y_pos_to = y;
            });
        } else {
            tx!(self, quit, |o| {
                o.y_pos = 2180;
                o.y_pos_to = 2180;
            });
            trgb!(self, restart, 64, 160, 64);
            self.textover.enable(restart);
        }
        trgb!(self, quit, 64, 160, 64);
        self.textover.enable(quit);

        self.disable_hud();

        for i in 0..(self.map.num_babies.max(0) as usize).min(self.babyfrog.baby_icons.len()) {
            if let Some(b) = self.babyfrog.baby_icons[i].as_mut() {
                b.draw = 0;
            }
        }
    }

    /// Port of `CheatAllowed` (Common/menus.c:265).
    pub fn cheat_allowed(&self, cheat: usize) -> bool {
        match cheat {
            CHEAT_INVULNERABILITY => self.frogger.num_frogs == 1,
            CHEAT_SKIP_LEVEL => self.frogger.num_frogs == 1 && self.game_state.mode != FRONTEND_MODE,
            // CHEAT_EXTRA_LEVELS: TRUE on PC.
            _ => true,
        }
    }

    /// Port of `ComboCheat` (Common/menus.c:301).
    pub fn combo_cheat(&mut self, cheat: usize) {
        if !self.cheat_allowed(cheat) {
            return;
        }
        self.play_sample(gen_sfx(GEN_DEATHDROWN), None, 0, SAMPLE_VOLUME, -1);
        let c = &mut self.menus.cheat_combos[cheat];
        if c.toggle > 0 {
            c.state = 1 - c.state;
        } else {
            c.state = 1;
        }
        let on = c.state != 0;

        self.menus.cheat_activated = cheat as i32;
        self.menus.cheat_timer.init(CHEAT_TEXT_TIME as i32);

        self.menus.cheat_str = format!("{} {}", self.gamestring(STR_CHEAT_1 + cheat as i32), self.gamestring(if on { STR_ON } else { STR_OFF }));
        let s = self.menus.cheat_str.clone();
        self.set_text(self.menus.cheat_text, &s);

        match cheat {
            CHEAT_OPEN_ALL_LEVELS => {
                for w in self.layout.world_visual_data.iter_mut() {
                    w.world_open = WORLD_OPEN;
                    let n = (w.num_levels as usize).min(w.level_visual_data.len());
                    for l in w.level_visual_data.iter_mut().take(n) {
                        l.level_open = LEVEL_OPEN;
                    }
                }
                self.options.max_page_allowed = BOOK_NUM_PAGES as i32 - 1;
            }
            CHEAT_OPEN_ALL_CHARS => {
                for a in self.frogger.frog_pool_active.iter_mut() {
                    *a = 1;
                }
            }
            CHEAT_OPEN_ALL_EXTRAS => self.check_for_extras(),
            CHEAT_INVULNERABILITY => self.frogger.player[0].safe.init(20),
            CHEAT_SKIP_LEVEL => {
                self.game_state.mode = LEVELCOMPLETE_MODE;
                self.babyfrog.babies_saved = self.map.num_babies as u32;
            }
            CHEAT_MAD_GARIBS => {
                let tex = if on { "RGARIB01" } else { "SCOIN0001" };
                let mut c = self.collect.garib_list.head.first;
                while let Some(id) = c {
                    let g = &mut self.collect.garib_list.array[id.idx()];
                    c = g.links.next;
                    if g.garib_type as i32 == crate::collect::SILVERCOIN_GARIB {
                        if let Some(s) = g.sprite.as_mut() {
                            s.texture = tex.to_string();
                        }
                    }
                }
                let t = crate::specfx::find_texture(tex);
                sx!(self, self.hud.arcade_hud.coins_over, |o| o.tex = t);
                sx!(self, self.hud.arcade_hud.coin_zoom, |o| o.tex = t);
            }
            CHEAT_EXTRA_LEVELS => self.options.num_arcade_worlds = 10,
            _ => {} // CHEAT_INFINITE_LIVES: nothing else.
        }
    }

    /// Port of `RunPauseMenu` (Common/menus.c:395).
    pub fn run_pause_menu(&mut self) {
        let mut exit_pause = false;
        let pc = self.frogger.pause_controller.clamp(0, 3) as usize;

        self.menus.pause_frame_count = self.menus.pause_frame_count.wrapping_add((self.menus.pause_game_speed >> 12).max(1));
        if self.menus.quitting_level != 0 && self.menus.pause_confirm_mode == 0 {
            if !self.levplay.fading_out {
                self.frontend.check_menu_keys = 0;
                if self.menus.quitting_level == 1 {
                    if let Some(f) = self.frogger.frog[0] {
                        if self.undo_change_model(f) != 0 {
                            self.frogger.player[0].idle_enable = 1;
                        }
                    }
                    if self.game_state.multi != SINGLEPLAYER {
                        self.reset_multiplayer();
                    }
                    let p = &self.frogger.player[0];
                    if p.world_num as u32 == worldid::FRONTEND && p.level_num as u32 == LEVELID_FRONTEND1 {
                        self.game_state.mode = FRONTEND_MODE;
                    } else {
                        self.game_state.mode = INGAME_MODE;
                    }
                    if self.game_state.single == STORY_MODE {
                        self.frogger.player[0].lives = self.frogger.player[0].old_lives;
                    }
                    self.frontend.restarting_level = true;
                    let (w, l) = (self.frogger.player[0].world_num as u32, self.frogger.player[0].level_num as u32);
                    self.init_level(w, l);
                    self.frame_count = 0;
                } else if self.menus.quitting_level == 2 {
                    if self.game_state.old_mode == FRONTEND_MODE || self.game_state.multi == MULTIREMOTE {
                        // PostQuitMessage(0)
                        self.effects.push(Effect::Quit); // wave G
                    } else {
                        if let Some(f) = self.frogger.frog[0] {
                            if self.undo_change_model(f) != 0 {
                                self.frogger.player[0].idle_enable = 1;
                            }
                        }
                        self.frogger.player[0].character = FROG_FROGGER;
                        self.frogger.num_frogs = 1;
                        if self.game_state.multi != SINGLEPLAYER {
                            self.free_multiplayer();
                        }
                        if self.game_state.multi != MULTIREMOTE {
                            self.game_state.multi = SINGLEPLAYER;
                        }
                        if self.game_state.single == STORY_MODE {
                            self.frogger.player[0].lives = self.frogger.player[0].old_lives;
                        }
                        self.game_state.mode = FRONTEND_MODE;
                        self.frogger.player[0].character = FROG_FROGGER;
                        self.init_level(worldid::FRONTEND, LEVELID_FRONTEND1);
                    }
                    self.frame_count = 0;
                }
            }
            return;
        }

        let deb = self.levplay.pad_data.debounce[pc];
        let dig = self.levplay.pad_data.digital[pc];

        if deb & PAD_SQUARE != 0 {
            self.menus.curr_cheat = 0;
            self.menus.current_cheat_combo = [0; 12];
        }

        if dig & PAD_SQUARE != 0 {
            if deb & !PAD_SQUARE != 0 {
                // currCheat is -1 until square is first pressed in this pause; the original
                // then writes currentCheatCombo[-1] (the port drops that key).
                if self.menus.curr_cheat >= 0 {
                    self.menus.current_cheat_combo[self.menus.curr_cheat as usize] = deb as i32;
                }
                self.menus.curr_cheat += 1;
                if self.menus.curr_cheat == 12 {
                    self.menus.curr_cheat = 0;
                }
            }
        } else if self.menus.curr_cheat > 0 {
            // if we've stored a cheat, check it
            for cheat in 0..NUMCHEATCOMBOS {
                let c = self.menus.cheat_combos[cheat];
                let mut i = 0;
                while i < 12 && c.key_string[i] != 0 && self.menus.current_cheat_combo[i] == c.key_string[i] as i32 {
                    i += 1;
                }
                if (i == 12 || c.key_string[i] == 0) && (c.state == 0 || c.toggle >= 0) {
                    self.combo_cheat(cheat);
                    self.menus.curr_cheat = 0;
                    exit_pause = true;
                    self.menus.current_pause_selection = 0;
                    break;
                }
            }
            self.menus.curr_cheat = 0;
        }

        let short_menu = self.game_state.old_mode == FRONTEND_MODE || self.game_state.multi == MULTIREMOTE;
        if !exit_pause {
            if deb & PAD_UP != 0 {
                if self.menus.current_pause_selection == 0 {
                    self.menus.current_pause_selection = if short_menu || self.menus.pause_confirm_mode != 0 { 1 } else { 2 };
                } else {
                    self.menus.current_pause_selection -= 1;
                }
            }
            if deb & PAD_DOWN != 0 {
                if ((short_menu || self.menus.pause_confirm_mode != 0) && self.menus.current_pause_selection == 1) || self.menus.current_pause_selection == 2 {
                    self.menus.current_pause_selection = 0;
                } else {
                    self.menus.current_pause_selection += 1;
                }
            }
        }

        let (cont, restart, quit) = (self.hud.continue_text, self.hud.restart_text, self.hud.quit_text);
        if self.menus.pause_confirm_mode != 0 && deb & PAD_TRIANGLE != 0 {
            self.menus.current_pause_selection = self.menus.quitting_level as u32;
            self.menus.pause_confirm_mode = 0;
            self.textover.sub_text_overlay(self.menus.yes_text);
            self.textover.sub_text_overlay(self.menus.no_text);
            self.menus.quitting_level = 0;
            if self.game_state.old_mode != FRONTEND_MODE && self.game_state.multi != MULTIREMOTE {
                self.t_set_draw(restart, 1);
            }
            self.t_set_draw(quit, 1);
            self.t_set_draw(cont, 1);
        }

        if deb & PAD_CROSS != 0 || deb & PAD_START != 0 || exit_pause {
            self.specfx.pause_mode = 0;

            if deb & PAD_START != 0 {
                self.menus.current_pause_selection = 0;
                if self.menus.pause_confirm_mode != 0 {
                    self.menus.pause_confirm_mode = 0;
                    self.textover.sub_text_overlay(self.menus.yes_text);
                    self.textover.sub_text_overlay(self.menus.no_text);
                }
                self.menus.quitting_level = 0;
            }

            let mut sel = self.menus.current_pause_selection;
            if sel == 0 {
                // continue game
                self.textover.disable(cont);
                self.textover.disable(self.hud.xselect_text);
                self.textover.disable(self.hud.pause_title_text);
                self.textover.disable(restart);
                self.textover.disable(quit);
                if self.menus.pause_confirm_mode != 0 {
                    self.menus.pause_confirm_mode = 0;
                    self.screen_fade(255, 0, 30);
                    self.textover.sub_text_overlay(self.menus.yes_text);
                    self.textover.sub_text_overlay(self.menus.no_text);
                } else {
                    self.frontend.check_menu_keys = 0;
                    let p = &self.frogger.player[0];
                    if p.world_num as u32 == worldid::FRONTEND && p.level_num as u32 == LEVELID_FRONTEND1 {
                        self.game_state.mode = FRONTEND_MODE;
                    } else if p.world_num as u32 == worldid::FRONTEND && p.level_num as u32 == LEVELID_FRONTEND4 {
                        self.game_state.mode = TRAINING_MODE;
                        for i in 0..(self.map.num_babies.max(0) as usize).min(6) {
                            sx!(self, self.hud.arcade_hud.babies_back[i], |o| o.draw = 1);
                            if let Some(b) = self.babyfrog.baby_icons.get_mut(i).and_then(|b| b.as_mut()) {
                                b.draw = 1;
                            }
                        }
                    } else {
                        self.game_state.mode = INGAME_MODE;
                        // gameSpeed = pauseGameSpeed (0): the PC timer recomputes gameSpeed
                        // every frame; the port's gameSpeed is fixed (4096), so this is dropped.
                        self.enable_hud();
                    }
                    self.un_pause_audio();
                    self.effects.push(Effect::UnPauseAudio); // wave G
                }
                return;
            }
            if sel == 1 {
                if self.menus.pause_confirm_mode != 0 {
                    self.menus.pause_confirm_mode = 0;
                    self.textover.sub_text_overlay(self.menus.yes_text);
                    self.textover.sub_text_overlay(self.menus.no_text);
                    if short_menu {
                        self.menus.current_pause_selection = 1;
                    } else {
                        self.t_set_draw(restart, 1);
                        self.menus.current_pause_selection = self.menus.quitting_level as u32;
                    }
                    self.t_set_draw(cont, 1);
                    self.t_set_draw(quit, 1);
                    self.menus.quitting_level = 0;
                    sel = u32::MAX; // break
                } else if !short_menu {
                    self.menus.quitting_level = 1;
                    self.menus.pause_confirm_mode = 1;
                    trgb!(self, restart, 255, 255, 255);
                    let y = self.t_ypos(restart);
                    let (ys, ns) = (self.gamestring(STR_YES), self.gamestring(STR_NO));
                    self.menus.yes_text = self.textover.create_and_add_text_overlay(
                        2048,
                        y.wrapping_add(400),
                        &ys,
                        true,
                        255,
                        Font::Normal,
                        TEXTOVERLAY_SHADOW | TEXTOVERLAY_PAUSED,
                    );
                    trgb!(self, self.menus.yes_text, 64, 160, 64);
                    self.menus.no_text = self.textover.create_and_add_text_overlay(
                        2048,
                        y.wrapping_add(700),
                        &ns,
                        true,
                        255,
                        Font::Normal,
                        TEXTOVERLAY_SHADOW | TEXTOVERLAY_PAUSED,
                    );
                    trgb!(self, self.menus.no_text, 64, 160, 64);
                    self.fadeout.keep_fade = true;
                    self.t_set_draw(cont, 0);
                    self.t_set_draw(quit, 0);
                    self.menus.current_pause_selection = 1;
                    return;
                } else {
                    sel = 2; // deliberate drop-through!!!!!!!
                }
            }
            if sel == 2 {
                // quit game
                if self.menus.pause_confirm_mode == 0 {
                    self.menus.pause_confirm_mode = 1;
                    self.menus.current_pause_selection = 1;
                    trgb!(self, quit, 255, 255, 255);
                    let y = self.t_ypos(quit);
                    let (ys, ns) = (self.gamestring(STR_YES), self.gamestring(STR_NO));
                    self.menus.yes_text = self.textover.create_and_add_text_overlay(
                        2048,
                        y.wrapping_add(400),
                        &ys,
                        true,
                        255,
                        Font::Normal,
                        TEXTOVERLAY_SHADOW | TEXTOVERLAY_PAUSED,
                    );
                    self.menus.no_text = self.textover.create_and_add_text_overlay(
                        2048,
                        y.wrapping_add(700),
                        &ns,
                        true,
                        255,
                        Font::Normal,
                        TEXTOVERLAY_SHADOW | TEXTOVERLAY_PAUSED,
                    );
                    self.t_set_draw(cont, 0);
                    self.t_set_draw(restart, 0);
                }
                self.menus.quitting_level = 2;
                self.fadeout.keep_fade = true;
            }
        }

        let pfc = self.menus.pause_frame_count;
        let (pr, pg) = (pulse_r(pfc), pulse_g(pfc));
        if self.menus.pause_confirm_mode != 0 {
            let (yes, no) = (self.menus.yes_text, self.menus.no_text);
            match self.menus.current_pause_selection {
                0 => {
                    trgb!(self, yes, pr, pg, 10);
                    trgb!(self, no, 64, 160, 64);
                }
                1 => {
                    trgb!(self, no, pr, pg, 10);
                    trgb!(self, yes, 64, 160, 64);
                }
                _ => {}
            }
        } else {
            let mut s = self.menus.current_pause_selection;
            if s == 0 {
                trgb!(self, cont, pr, pg, 10);
                trgb!(self, restart, 64, 160, 64);
                trgb!(self, quit, 64, 160, 64);
            }
            if s == 1 {
                if self.game_state.old_mode != FRONTEND_MODE && self.game_state.multi != MULTIREMOTE {
                    trgb!(self, restart, pr, pg, 10);
                    trgb!(self, cont, 64, 160, 64);
                    trgb!(self, quit, 64, 160, 64);
                } else {
                    s = 2; // deliberate drop-through!!!!!!!
                }
            }
            if s == 2 {
                trgb!(self, quit, pr, pg, 10);
                trgb!(self, cont, 64, 160, 64);
                trgb!(self, restart, 64, 160, 64);
            }
        }
    }

    /// The frontend tile number of `currTile[0]` as `RunFrontendGameLoop` counts it
    /// (Common/menus.c:1153): the index into `firstTile` for tiles 1.., `tileCount` when
    /// the frog is on tile 0 (or nowhere).
    pub fn frontend_tile_num(&self) -> u32 {
        match self.frogmove.curr_tile[0] {
            Some(t) if t.idx() >= 1 && t.idx() < self.map.tiles.len() => t.idx() as u32,
            _ => self.map.tiles.len() as u32,
        }
    }

    /// Port of `RunFrontendGameLoop` (Common/menus.c:863), PC build.
    pub fn run_frontend_game_loop(&mut self) {
        // setup for frogger point of interest
        self.frogger.p_oi_distance = 20000 << 12;
        self.frogger.point_of_interest = None;

        self.frontend.check_menu_keys = 0;
        if self.menus.going_to_demo {
            if !self.levplay.fading_out {
                self.menus.old_diff_mode = self.game_state.difficulty as i32;
                self.game_state.difficulty = DIFFICULTY_NORMAL;
                self.menus.going_to_demo = false;
                // InitDemoMode() through the session's data source.
                if let Some(data) = self.session.data.clone() {
                    match self.init_demo_mode(&*data) {
                        Ok(assets) => {
                            let (w, l) = (self.frogger.player[0].world_num as u32, self.frogger.player[0].level_num as u32);
                            self.session.level_loads.push(crate::session::LevelLoad { world: w, level: l, assets });
                        }
                        Err(e) => self.effects.push(Effect::Log { text: format!("InitDemoMode failed: {e}") }),
                    }
                } else {
                    self.effects.push(Effect::Log { text: "InitDemoMode: no data source".into() });
                }
            }
            return;
        }

        let gs = self.game_speed;
        if self.menus.bounce_dir > 0 {
            self.menus.bounce += fmul(self.menus.bounce_speed, gs);
            if self.menus.bounce >= 128 {
                self.menus.bounce_dir = -1;
                self.menus.bounce = 128;
            }
        } else {
            self.menus.bounce -= fmul(self.menus.bounce_speed, gs);
            if self.menus.bounce <= 0 {
                self.menus.bounce_dir = 1;
                self.menus.bounce = 0;
            }
        }

        self.cam.cam_speed3 = 4096 * 20;
        if self.frame_count == 1 {
            self.frontend_first_frame();
        }

        let mut curr_tile_num = self.frontend_tile_num();
        self.game.curr_tile_num = curr_tile_num;

        if self.levplay.pad_data.debounce[0] != 0 {
            if curr_tile_num == TILENUM_START {
                self.menus.frontend_timer.init(30);
            } else {
                self.menus.frontend_timer.init(LONG_DEMO_WAIT);
            }
        }

        if self.menus.frontend_timer.time > 0
            && self.menus.play_demos
            && self.options.credits_running == 0
            && self.options.mode != OP_GLOBALMENU
            && self.options.mode != OP_EXTRA
        {
            if curr_tile_num == TILENUM_START && self.menus.frontend_timer.time > 30 {
                self.menus.frontend_timer.init(30);
            }
            let afc = self.act_frame_count;
            self.menus.frontend_timer.update(-1, afc);
            if self.menus.frontend_timer.time == 0 {
                self.effects.push(Effect::Log { text: "Front-end idle timer ran out.. run demo mode!".into() });
                // run a demo here
                self.menus.going_to_demo = true;
                self.screen_fade(255, 0, 30);
                self.fadeout.keep_fade = false;
            }
        }

        if self.menus.done_training == 0 || self.options.static_flash != 0 {
            if self.options.static_flash != 0 {
                self.options.static_flash -= 1;
                if self.options.static_flash == 0 {
                    let wvd = &self.layout.world_visual_data;
                    let name = if self.menus.c_world == worldid::GARDEN {
                        if self.options.level_num == 0 {
                            cstr(&wvd[worldid::FRONTEND as usize].level_visual_data[LEVELID_FRONTEND4 as usize].tex_crc)
                        } else {
                            cstr(&wvd[worldid::GARDEN as usize].level_visual_data[0].tex_crc)
                        }
                    } else {
                        cstr(&wvd[self.menus.c_world as usize].level_visual_data[self.options.level_num as usize].tex_crc)
                    };
                    self.menus.arcade_screen_copy = name;
                }
            }
            if self.menus.done_training == 0 || self.options.static_flash != 0 {
                let texname = format!("0{}NOIS04", self.menus.arcade_tex_num);
                self.menus.arcade_tex_num = (self.menus.arcade_tex_num + 1) % 3;
                self.menus.arcade_screen_copy = texname;
            }
        }

        let spk_level_text = |g: &Game| -> Option<TextOverlayId> {
            let spk_lev = if g.options.mode == OP_ARCADE { g.options.level_num } else { g.options.sp_char };
            g.options.level_text.get(spk_lev as usize).copied().flatten()
        };
        for i in 0..MAX_SPARKLES {
            let sp = self.menus.sparkles[i];
            let dec = (gs * 3) / 4096;
            let a = self.overlays.get(sp).map_or(0, |o| o.a as i32);
            if a > dec {
                sx!(self, sp, |o| o.a = (a - dec) as u8);
            } else if self.frogger.player[0].can_jump != 0 {
                let lt = spk_level_text(self);
                let (ly, lw, lc, lx) = self.textover.get(lt).map_or((0, 0, 0, 0), |o| (o.y_pos as i32, o.t_width as i32, o.centred, o.x_pos as i32));
                let y = self.rng.random(128) + ly - 64;
                let na = self.rng.random(160);
                let x = if lc != 0 { (2000 - lw * 3) + self.rng.random((lw * 6) as u32) } else { lx + self.rng.random((lw * 6) as u32) };
                sx!(self, sp, |o| {
                    o.y_pos = y as i16;
                    o.y_pos_to = y as i16;
                    o.a = na as u8;
                    o.x_pos = x as i16;
                    o.x_pos_to = x as i16;
                });
            } else {
                sx!(self, sp, |o| o.a = 0);
            }
            let d = self.t_draw(self.options.level_text[self.options.level_num as usize % MAX_LEVELSTRING]);
            sx!(self, sp, |o| {
                o.speed = 0;
                o.draw = d;
            });
        }

        // Take this out for release
        if self.menus.tile_num.is_none() {
            self.menus.tile_num = self.textover.create_and_add_text_overlay(2048, 1024, "", true, 255, Font::Normal, 0);
        }

        if self.frame_count == 1 && self.menus.last_arcade != 0 {
            self.frontend_return_from_game();
            curr_tile_num = self.game.curr_tile_num;
        }

        if !self.menus.fading_logos && curr_tile_num != TILENUM_START {
            self.menus.fading_logos = true;
        }
        if self.menus.fading_logos {
            self.s_dec_alpha(self.menus.frog_logo);
        }

        self.frontend_title_hud(curr_tile_num);

        if curr_tile_num != TILENUM_BOOK
            && curr_tile_num != TILENUM_MULTI
            && curr_tile_num != TILENUM_OPTIONS
            && curr_tile_num != TILENUM_ARCADE
            && self.frame_count > 15
        {
            self.game_process_controller(0);
        }

        let can_jump = self.frogger.player[0].can_jump != 0;
        let o = &self.options;
        if (curr_tile_num != TILENUM_BOOK || !can_jump) && o.page.is_none() {
            let ids = [
                o.chapter_name_text[0][0],
                o.chapter_name_text[0][1],
                o.chapter_name_text[1][0],
                o.chapter_name_text[1][1],
                o.chapter_text[0],
                o.chapter_text[1],
            ];
            let pics = o.chapter_pic;
            for t in ids {
                self.t_dec_alpha(t);
            }
            for s in pics {
                self.s_dec_alpha(s);
            }
        }

        if curr_tile_num != TILENUM_ARCADE || !can_jump {
            let o = &self.options;
            let ids = [o.arcade_text, o.par_text[0], o.par_text[1], o.par_text[2]];
            for t in ids {
                self.t_dec_alpha(t);
            }
            if (curr_tile_num != TILENUM_BOOK || !can_jump) && (curr_tile_num != TILENUM_OPTIONS || self.options.credits_running != 0 || !can_jump) {
                self.t_dec_alpha(self.options.select_text);
            }
            self.t_dec_alpha(self.options.world_text);
            self.s_dec_alpha(self.options.world_bak);
            for i in 0..MAX_LEVELSTRING {
                let o = &self.options;
                let (a, b, c, d, e, f) =
                    (o.level_text[i], o.level_par_text[i], o.level_set_by_text[i], o.level_coin_text[i], o.level_coin_medal[i], o.beaten_icon[i]);
                self.t_dec_alpha(a);
                self.t_dec_alpha(b);
                self.t_dec_alpha(c);
                self.t_dec_alpha(d);
                self.s_dec_alpha(e);
                self.s_dec_alpha(f);
            }
        }

        if self.options.mode == -1 {
            let o = &self.options;
            let (a, b, c, d) = (o.title, o.sub_title, o.title_bak, o.status_bak);
            self.t_dec_alpha(a);
            self.t_dec_alpha(b);
            self.s_dec_alpha(c);
            self.s_dec_alpha(d);
        }

        // options tile by waterfall
        if curr_tile_num == TILENUM_OPTIONS && self.frogger.player[0].has_jumped == 0 {
            self.frogmove.frog_facing[0] = 1;
            if self.menus.reached_point == 0 {
                self.menus.reached_point = 1;
            }
            if self.options.mode == -1 {
                self.options.mode = OP_GLOBALMENU;
                self.menus.frontend_timer.init(LONG_DEMO_WAIT);
            }
            self.run_options_menu();
            if self.game_state.mode == ARTVIEWER_MODE {
                return;
            }
        }

        // multiplayer tile
        if curr_tile_num == TILENUM_MULTI && self.frogger.player[0].has_jumped == 0 {
            if self.menus.reached_point == 0 {
                self.menus.store_cam_source = self.cam.curr_cam_source;
                self.menus.store_cam_target = self.cam.curr_cam_target;
                self.menus.store_curr_cam_offset = self.cam.curr_cam_offset;

                self.cam.curr_cam_source = FVector { vx: -9591334, vy: 2500000, vz: 20425392 };
                self.cam.curr_cam_target = FVector { vx: -9593916, vy: 0, vz: 20430628 };
                self.cam.cam_target = self.cam.curr_cam_target;
                self.cam.cam_source = self.cam.curr_cam_source;

                self.menus.reached_point = 1;
                self.menus.fog_store = self.frogger.fog.max as i32;
                self.frogger.fog.max = 2500;
            }
            if self.menus.reached_point != 0 {
                if self.options.mode == -1 {
                    self.options.mode = OP_MULTIPLAYERNUMBER;
                    self.menus.frontend_timer.init(LONG_DEMO_WAIT);
                }
                self.run_options_menu();
                if self.game_state.mode == INGAME_MODE {
                    self.menus.reached_point = 0;
                    return;
                }
            }
        }

        if curr_tile_num == TILENUM_ARCADE && self.frogger.player[0].has_jumped == 0 {
            if self.options.mode == -1 {
                self.options.mode = OP_ARCADE;
                self.menus.frontend_timer.init(LONG_DEMO_WAIT);
                self.screen_fade(255, 210, 30);
                self.fadeout.keep_fade = true;
                self.fadeout.fade_text = false;
            }
            self.run_options_menu();
            if self.game_state.mode != FRONTEND_MODE {
                return;
            }
        }

        if curr_tile_num == TILENUM_BOOK && self.frogger.player[0].has_jumped == 0 {
            if self.options.mode == -1 {
                self.options.mode = OP_BOOK;
                self.menus.frontend_timer.init(LONG_DEMO_WAIT);
            }
            self.run_options_menu();
            if self.game_state.mode != FRONTEND_MODE {
                return;
            }
        }

        if let Some(page) = self.options.page {
            if self.options.page_dir == 0 && self.options.page_draw_count != 0 {
                self.options.page_draw_count -= 1;
                if self.options.page_draw_count == 0 {
                    if let Some(a) = self.actor2.actors.get_mut(page) {
                        a.draw = 0;
                    }
                }
            }
        }

        if self.menus.reached_point == 0 {
            let p0 = &self.frogger.player[0];
            let (cj, ehd) = (p0.can_jump != 0, p0.extended_hop_dir);
            if cj {
                let t = self.frogmove.curr_tile[0];
                self.check_for_dynamic_camera_change(t, 0);
            }
            if (curr_tile_num != TILENUM_OPTIONS || !cj) && curr_tile_num != TILENUM_MULTI && (curr_tile_num != TILENUM_CHOICE || cj || ehd != 1) {
                self.update_camera_position();
            }
        }
        // SetCamFF(currCamSource, currCamTarget): PSX only.

        for i in 0..self.frogger.num_frogs.max(0) as usize {
            let Some(f) = self.frogger.frog[i] else { continue };
            self.update_frogger_pos(i as i32);
            if self.frogger.player[i].jump_multiplier != 0 {
                self.frogger.player[i].jump_multiplier = 1024;
            }
            if self.frogger.player[i].frog_state & FROGSTATUS_ISDEAD == 0 {
                self.actor2.actors[f].draw = 1;
                if self.frogger.player[i].safe.time != 0 {
                    let afc = self.act_frame_count;
                    self.frogger.player[i].safe.update(-1, afc);
                }
            }
            let p = &self.frogger.player[i];
            if p.can_jump != 0 && p.idle_enable != 0 && self.game_state.multi == SINGLEPLAYER {
                let p = &mut self.frogger.player[i];
                p.idle_time = p.idle_time.wrapping_sub(self.game_speed);
                if p.idle_time < 4096 {
                    self.frogger_idle_anim(i as i32);
                    let r = self.rng.random(400);
                    self.frogger.player[i].idle_time = (500 + r) << 12;
                    if self.rng.random(10) == 0 {
                        self.effects.push(Effect::PlayVoice { player: i as u8, uid: f2_formats::crc::crc("frogbored") });
                    }
                }
            }
        }

        self.update_platforms();
        self.update_enemies();
        self.update_special_effects();
        self.update_events();
        if self.game_state.mode != PAUSE_MODE {
            self.update_ambient_sounds();
        }
        self.process_collectables();
        self.up_date_on_screen_info();
    }

    /// The `frameCount == 1` block of `RunFrontendGameLoop` (Common/menus.c:947): creates the
    /// frontend's overlays and finds the book, chest and door.
    fn frontend_first_frame(&mut self) {
        self.set_music_volume();

        self.menus.fading_logos = false;
        self.menus.frog_logo = self.overlays.create_and_add_sprite_overlay(1024, 2800, Some("FROGLOGO"), 2048, 2048, 255, 0);

        self.menus.fog_store = self.frogger.fog.max as i32;

        self.check_for_extras();
        self.init_options_menu();
        self.screen_fade(0, 255, 30);
        self.fadeout.keep_fade = false;

        for i in 0..4 {
            self.menus.title_hud[i] =
                self.overlays.create_and_add_sprite_overlay(TITLE_HUD_X[0][i], TITLE_HUD_Y[0][i], Some(TITLE_HUD_NAME[i]), 300, 300, 0, 0);
            self.menus.title_hud_text[i] = self.textover.create_and_add_text_overlay(0, 0, "", i < 2, 255, Font::Small, TEXTOVERLAY_SHADOW);
            tx!(self, self.menus.title_hud_text[i], |o| {
                o.b = 0;
                o.draw = 0;
            });
        }

        for i in (0..NUM_STORY_LEVELS).rev() {
            let s = STORY_SEQUENCE[i];
            let w = &self.layout.world_visual_data[s.world_num as usize];
            if w.world_open != 0 && w.level_visual_data[s.level_num as usize].level_open != 0 {
                self.game_state.story_sequence_level = i as _;
                break;
            }
        }

        for (i, &lev) in PAGE_TO_STORY_LEVEL.iter().enumerate() {
            if self.game_state.story_sequence_level as i64 >= lev as i64 {
                self.options.max_page_allowed = i as i32;
                self.options.page_num = i as i32;
            }
        }

        self.options.page = None;

        let enemy_actor = |g: &Game, uid: i32| g.get_enemy_from_uid(uid).and_then(|e| g.enemies.enemy_list.array[e.idx()].nme_actor);
        self.options.book = enemy_actor(self, 123);
        self.options.chest_lid = enemy_actor(self, 124);
        self.options.door = enemy_actor(self, 125);

        if let Some(d) = self.options.door {
            self.actor_animate(d, 1, false, false, 4000, false);
        }
        if self.layout.world_visual_data[worldid::FRONTEND as usize].level_visual_data[LEVELID_FRONTEND4 as usize].level_completed != 0 {
            self.menus.done_training = 1;
            self.options.static_flash = 5;
        }

        if self.options.r_hardware {
            if let Some(book) = self.options.book {
                self.options.page = self.create_and_add_actor("bookpage.obe", -3200, 800, -2800, crate::mdx_actor::INIT_ANIMATION);
                if let Some(page) = self.options.page {
                    let b = self.actor2.actors[book].actor.clone();
                    let a = &mut self.actor2.actors[page].actor;
                    a.position = b.position;
                    a.position.vy = a.position.vy.wrapping_add(5);
                    a.q_rot = b.q_rot;
                    a.size = b.size;
                    self.options.page_dir = 1;
                    self.options.page_draw_count = 0;
                    self.actor_animate(page, 1, false, false, 9999, false);
                    self.options.grab_to_texture = 1;
                }
            }
        }

        if self.options.page.is_none() {
            let pn = self.options.page_num;
            self.options.chapter_str[0] = c_sprintf(&self.gamestring(STR_CHAPTER), &[FmtArg::Int(pn + 1)]);
            let pic = CHAPTER_PIC[(pn as usize).min(BOOK_NUM_PAGES - 1)];
            for i in 0..2 {
                let s = self.options.chapter_str[i].clone();
                self.options.chapter_text[i] = self.textover.create_and_add_text_overlay(2150, 800, &s, false, 0, Font::Normal, TEXTOVERLAY_SHADOW);
                self.t_set_draw(self.options.chapter_text[i], 0);
                let (a, b) = (self.gamestring(STR_CHAPTER_1a + pn * 2), self.gamestring(STR_CHAPTER_1a + pn * 2 + 1));
                self.options.chapter_name_text[i][0] = self.textover.create_and_add_text_overlay(2150, 2800, &a, false, 0, Font::Small, TEXTOVERLAY_SHADOW);
                self.options.chapter_name_text[i][1] = self.textover.create_and_add_text_overlay(2150, 3000, &b, false, 0, Font::Small, TEXTOVERLAY_SHADOW);
                self.t_set_draw(self.options.chapter_name_text[i][0], 0);
                self.t_set_draw(self.options.chapter_name_text[i][1], 0);
                let clip = if i == 1 { crate::sprite::SPRITE_CLIP1 } else { crate::sprite::SPRITE_CLIP2 };
                self.options.chapter_pic[i] = self.overlays.create_and_add_sprite_overlay(2400, 1500, Some(pic), 1024, 1024, 0, clip);
                self.s_set_draw(self.options.chapter_pic[i], 0);
            }
            self.frogger.player[0].has_jumped = 0;
        }

        self.disable_hud();
        self.menus.reached_point = 0;

        for i in 0..MAX_SPARKLES {
            let a = self.rng.random(0xff) as u8;
            self.menus.sparkles[i] = self.overlays.create_and_add_sprite_overlay(256, 1535, Some("FLASH"), 102, 137, a, SPRITE_ADDITIVE);
            sx!(self, self.menus.sparkles[i], |o| o.num = 1);
        }

        let s = self.gamestring(STR_ARCADEMODE);
        self.options.arcade_text = self.textover.create_and_add_text_overlay(2048, 270, &s, true, 255, Font::Normal, 0);
        tx!(self, self.options.arcade_text, |o| o.b = 0);
        let s = self.gamestring(STR_SELECT_LEVEL);
        self.options.select_text = self.textover.create_and_add_text_overlay(2048, 576, &s, true, 255, Font::Small, 0);
        let s = self.menus.world_str.clone();
        self.options.world_text = self.textover.create_and_add_text_overlay(306, 1105, &s, false, 255, Font::Small, TEXTOVERLAY_SHADOW);

        let (a, b, c) = (self.gamestring(STR_PAR), self.gamestring(STR_SET_BY), self.gamestring(STR_COINS));
        self.options.par_text[0] = self.textover.create_and_add_text_overlay(1950, 1105, &a, false, 255, Font::Small, TEXTOVERLAY_SHADOW);
        self.options.par_text[1] = self.textover.create_and_add_text_overlay(2750, 1105, &b, false, 255, Font::Small, TEXTOVERLAY_SHADOW);
        self.options.par_text[2] = self.textover.create_and_add_text_overlay(3450, 1105, &c, false, 255, Font::Small, TEXTOVERLAY_SHADOW);
        for t in [self.options.world_text, self.options.par_text[0], self.options.par_text[1], self.options.par_text[2]] {
            trgb!(self, t, 255, 200, 0);
        }

        for i in 0..MAX_LEVELSTRING {
            let y = (1445 + i * 170) as i16;
            let (ls, lp, lsb, lc) = (
                self.menus.level_str[i].clone(),
                self.menus.level_par_str[i].clone(),
                self.menus.level_set_by_str[i].clone(),
                self.menus.level_coin_str[i].clone(),
            );
            self.options.level_text[i] = self.textover.create_and_add_text_overlay(306, y, &ls, false, 0, Font::Small, TEXTOVERLAY_SHADOW);
            self.t_set_draw(self.options.level_text[i], 0);
            self.options.level_par_text[i] = self.textover.create_and_add_text_overlay(1950, y, &lp, false, 0, Font::Small, TEXTOVERLAY_SHADOW);
            self.t_set_draw(self.options.level_par_text[i], 0);
            self.options.level_set_by_text[i] = self.textover.create_and_add_text_overlay(2750, y, &lsb, false, 0, Font::Small, TEXTOVERLAY_SHADOW);
            self.t_set_draw(self.options.level_set_by_text[i], 0);
            self.options.level_coin_text[i] = self.textover.create_and_add_text_overlay(3600, y, &lc, false, 0, Font::Small, TEXTOVERLAY_SHADOW);
            self.t_set_draw(self.options.level_coin_text[i], 0);
            let y2 = (1477 + i * 170) as i16;
            self.options.level_coin_medal[i] = self.overlays.create_and_add_sprite_overlay(3582, y2, Some("COINMEDAL"), 160, 160, 0, 0);
            self.s_set_draw(self.options.level_coin_medal[i], 0);
            self.options.beaten_icon[i] = self.overlays.create_and_add_sprite_overlay(150, y2 - 32, Some("FLASH"), 160, 160, 0, SPRITE_ADDITIVE);
            self.s_set_draw(self.options.beaten_icon[i], 0);
        }

        self.options.world_bak = self.overlays.create_and_add_sprite_overlay(150, 938, Some("WBACK"), 3796, 1365, 0, 0);
        srgb!(self, self.options.world_bak, 10, 200, 10);

        self.menus.arcade_screen_tex = "TEST5".into();
        self.menus.arcade_screen_copy = "GARDEN1".into();

        self.options.title_bak = self.overlays.create_and_add_sprite_overlay(0, 0, Some(""), 4096, 800, 0, SPRITE_SUBTRACTIVE);
        self.options.status_bak = self.overlays.create_and_add_sprite_overlay(0, 3660, Some(""), 4096, 500, 0, SPRITE_SUBTRACTIVE);

        let o = &self.options;
        let (a, b, c, d, e, f) = (o.arcade_text, o.select_text, o.world_text, o.world_bak, o.title_bak, o.status_bak);
        let pt = o.par_text;
        self.t_set_draw(a, 0);
        self.t_set_draw(b, 0);
        self.t_set_draw(c, 0);
        self.s_set_draw(d, 0);
        self.s_set_draw(e, 0);
        self.s_set_draw(f, 0);
        for t in pt {
            self.t_set_draw(t, 0);
        }

        let t = self.frogmove.curr_tile[0];
        self.check_for_dynamic_camera_change(t, 0); // TEMPORARY FIX!!
        self.last_act_frame_count = 0;

        self.menus.frontend_timer.init(30); // 30 seconds
    }

    /// The `frameCount == 1 && lastArcade` block of `RunFrontendGameLoop`
    /// (Common/menus.c:1298): puts the frog back on the tile it left from.
    fn frontend_return_from_game(&mut self) {
        let la = self.menus.last_arcade as usize;
        if la >= 1 && la < self.map.tiles.len() {
            let t = crate::ids::TileId(la as u32);
            self.frogmove.curr_tile[0] = Some(t);
            self.cam.last_tile[0] = Some(t);
            self.game.curr_tile_num = la as u32;
        } else {
            self.game.curr_tile_num = self.map.tiles.len() as u32;
        }

        if let (Some(f), Some(t)) = (self.frogger.frog[0], self.frogmove.curr_tile[0]) {
            let c: SVector = self.map.tile(t).centre;
            self.actor2.actors[f].actor.position = c;
        }
        let m = &self.menus;
        self.cam.curr_cam_source = m.store_curr_cam_source;
        self.cam.cam_source = m.store_cam_source;
        self.cam.curr_cam_target = m.store_curr_cam_target;
        self.cam.cam_target = m.store_cam_target;
        self.cam.curr_cam_dist = m.store_curr_cam_dist;
        self.cam.cam_dist = SVector { vx: m.store_cam_dist.vx as i16, vy: m.store_cam_dist.vy as i16, vz: m.store_cam_dist.vz as i16, pad: 0 };
        self.cam.curr_cam_offset = m.store_curr_cam_offset;
        self.cam.cam_offset = m.store_cam_offset;
        self.cam.cam_vect = m.store_cam_vect;
        if self.menus.last_arcade == TILENUM_ARCADE {
            self.cam.cam_facing[0] = 1;
        } else {
            for i in 0..(self.options.current_player.max(0) as usize).min(4) {
                let mf = self.options.multi_face[self.options.player_char[i] as usize % FROG_NUMFROGS];
                sx!(self, mf, |o| {
                    o.draw = 1;
                    o.r = 255;
                    o.g = 255;
                    o.b = 255;
                    o.a = 255;
                    o.x_pos = BACK_CHARS_X[i] as i16;
                    o.x_pos_to = BACK_CHARS_X[i] as i16;
                    o.y_pos = BACK_CHARS_Y[i] as i16;
                    o.y_pos_to = BACK_CHARS_Y[i] as i16;
                });
            }
            self.options.mode = OP_LEVELSEL;
            self.menus.reached_point = 1;
            self.menus.store_cam_source = FVector { vx: -3884265, vy: 3653915, vz: 2103810 };
            self.menus.store_cam_target = FVector { vx: 6143995, vy: 409600, vz: -3686400 };
            self.menus.store_curr_cam_offset = FVector { vx: -10028260, vy: 3244315, vz: 5790205 };
            self.frogger.player[0].has_jumped = 0;
        }

        self.menus.last_arcade = 0;
        self.menus.fading_logos = true;
        self.s_set_draw(self.menus.frog_logo, 0);
    }

    /// The title HUD part of `RunFrontendGameLoop` (Common/menus.c:1376): which d-pad arrows
    /// and labels show on each frontend tile.
    fn frontend_title_hud(&mut self, curr_tile_num: u32) {
        let mut max_hud = 4usize;
        let th = self.menus.title_hud;
        let tht = self.menus.title_hud_text;
        let hud_num: i32;

        if self.frogger.player[0].can_jump != 0 {
            match curr_tile_num {
                TILENUM_START => {
                    hud_num = 0;
                    self.t_set_draw(tht[0], 0);
                    self.t_set_draw(tht[1], 0);
                    self.t_set_draw(tht[2], 1);
                    self.t_set_draw(tht[3], 1);
                    let (a, b) = (self.gamestring(STR_OPTIONS), self.gamestring(STR_START_GAME));
                    self.set_text(tht[2], &a);
                    self.set_text(tht[3], &b);
                }
                TILENUM_OPTIONS => {
                    if self.options.mode == OP_GLOBALMENU {
                        self.t_set_draw(tht[0], 0);
                        self.t_set_draw(tht[1], 1);
                        let s = self.gamestring(STR_START_GAME);
                        self.set_text(tht[1], &s);
                        if self.options.selection == 0 {
                            hud_num = 1;
                            self.t_set_draw(tht[2], 1);
                            self.t_set_draw(tht[3], 0);
                            let s = self.gamestring(STR_OPTIONS_2);
                            self.set_text(tht[2], &s);
                        } else {
                            hud_num = 6;
                            self.t_set_draw(tht[2], 0);
                            self.t_set_draw(tht[3], 1);
                            let s = self.gamestring(STR_OPTIONS_1);
                            self.set_text(tht[3], &s);
                        }
                        let y = TITLE_HUD_Y[0][2];
                        sx!(self, th[2], |o| {
                            o.y_pos = y;
                            o.y_pos_to = y;
                        });
                        sx!(self, th[3], |o| {
                            o.y_pos = y;
                            o.y_pos_to = y;
                        });
                        let x = TITLE_HUD_X[0][0];
                        sx!(self, th[0], |o| {
                            o.x_pos = x;
                            o.x_pos_to = x;
                        });
                        sx!(self, th[1], |o| {
                            o.x_pos = x;
                            o.x_pos_to = x;
                        });
                    } else if self.options.mode == OP_SOUND {
                        for t in tht {
                            self.t_set_draw(t, 0);
                        }
                        hud_num = 5;
                        for s in th {
                            sx!(self, s, |o| o.a = 255);
                        }
                    } else {
                        hud_num = -1;
                    }
                }
                TILENUM_CHOICE => {
                    // close off multiplayer and arcade machine if training not completed
                    self.t_set_draw(tht[0], 1);
                    self.t_set_draw(tht[1], 1);
                    self.t_set_draw(tht[2], 1);
                    self.t_set_draw(tht[3], self.menus.done_training as u8);
                    for (i, s) in [STR_STORYMODE, STR_OPTIONS, STR_MULTIPLAYER, STR_ARCADEMODE].into_iter().enumerate() {
                        let s = self.gamestring(s);
                        self.set_text(tht[i], &s);
                    }
                    let y = TITLE_HUD_Y[0][2];
                    sx!(self, th[2], |o| {
                        o.y_pos = y;
                        o.y_pos_to = y;
                    });
                    sx!(self, th[3], |o| {
                        o.y_pos = y;
                        o.y_pos_to = y;
                    });
                    // FINAL_MASTER
                    if self.menus.done_training == 0 {
                        if let Some(t) = self.frogmove.curr_tile[0] {
                            self.map.tile_mut(t).tile_ptrs[2] = None;
                        }
                        max_hud = 3;
                    }
                    self.game.mode_timer.init(5);
                    hud_num = 2;
                }
                TILENUM_BOOK => {
                    hud_num = 3;
                    for t in tht {
                        self.t_set_draw(t, 0);
                    }
                }
                TILENUM_MULTI => {
                    for t in tht {
                        self.t_set_draw(t, 0);
                    }
                    hud_num = if self.options.mode == OP_MULTIPLAYERNUMBER && self.options.max_players <= 2 { -1 } else { 4 };
                }
                TILENUM_ARCADE => {
                    for t in tht {
                        self.t_set_draw(t, 0);
                    }
                    hud_num = if self.options.mode == OP_ARCADE { 4 } else { -1 };
                }
                _ => {
                    for t in tht {
                        self.t_set_draw(t, 0);
                    }
                    hud_num = -1;
                }
            }
        } else {
            hud_num = -1;
        }

        tx!(self, tht[0], |o| o.x_pos = 2048);
        tx!(self, tht[1], |o| o.x_pos = 2048);
        let x2 = TITLE_HUD_X[0][2] + 164;
        let y2 = self.overlays.get(th[2]).map_or(0, |o| o.y_pos) + 300;
        let y3 = self.overlays.get(th[3]).map_or(0, |o| o.y_pos) + 300;
        tx!(self, tht[2], |o| {
            o.x_pos = x2;
            o.y_pos = y2;
        });
        tx!(self, tht[3], |o| o.y_pos = y3);
        tx!(self, tht[0], |o| o.y_pos = TITLE_HUD_Y[0][0] + 400);
        tx!(self, tht[1], |o| o.y_pos = TITLE_HUD_Y[0][1] - 400);
        // CalcStringWidth(text, fontSmall, 1) * 6.4 * ((!rHardware && rXRes < 640) + 1)
        let t3 = self.textover.get(tht[3]).map(|o| o.text.clone()).unwrap_or_default();
        let w = self.textover.calc_string_width(&t3, Font::Small, 1.0);
        let x3 = (TITLE_HUD_X[0][3] as i32 + 100) as f64 - (w as f64) * 6.4 * if self.options.r_hardware { 1.0 } else { 2.0 };
        tx!(self, tht[3], |o| o.x_pos = f2_math::ftol(x3) as i16);

        if hud_num == 5 {
            let pos = [(1000, 3000), (1000 + 300, 3000), (1000, 3300), (1000 + 300, 3300)];
            for (i, (x, y)) in pos.into_iter().enumerate() {
                sx!(self, th[i], |o| {
                    o.x_pos = x;
                    o.y_pos = y;
                });
            }
        } else {
            let b = self.menus.bounce as i16;
            let h0 = self.overlays.get(th[0]).map_or(0, |o| o.height);
            let w3 = self.overlays.get(th[3]).map_or(0, |o| o.width);
            sx!(self, th[0], |o| o.y_pos = b);
            sx!(self, th[1], |o| o.y_pos = 4096 - h0 - b);
            sx!(self, th[2], |o| o.x_pos = b);
            sx!(self, th[3], |o| o.x_pos = 4096 - w3 - b);
        }

        if hud_num == -1 {
            for i in 0..4 {
                self.s_dec_alpha(th[i]);
                self.t_dec_alpha(tht[i]);
            }
        } else {
            for i in 0..4 {
                if self.t_draw(tht[i]) != 0 {
                    self.t_inc_alpha(tht[i], 255);
                } else {
                    self.t_dec_alpha(tht[i]);
                }
                if TITLE_HUD_ON[hud_num as usize][i] == 0 {
                    self.s_dec_alpha(th[i]);
                } else if i < max_hud {
                    if curr_tile_num == TILENUM_BOOK {
                        if i == 2 {
                            if self.options.page_num == 0 {
                                self.s_dec_alpha(th[i]);
                            } else {
                                self.s_inc_alpha(th[i], 255);
                            }
                        } else if i == 3 {
                            if self.options.page_num == self.options.max_page_allowed {
                                self.s_dec_alpha(th[i]);
                            } else {
                                self.s_inc_alpha(th[i], 255);
                            }
                        } else {
                            self.s_inc_alpha(th[i], 255);
                        }
                    } else {
                        self.s_inc_alpha(th[i], 255);
                    }
                    let t = crate::specfx::find_texture(TITLE_HUD_NAME[i]);
                    sx!(self, th[i], |o| o.tex = t);
                }
            }
        }
    }

    /// Port of `DoArcadeMenu` (Common/menus.c:1882): the arcade machine's world/level list.
    pub fn do_arcade_menu(&mut self) {
        self.t_inc_alpha(self.options.sub_title, 255);
        self.s_inc_alpha(self.options.status_bak, 128);
        let y = self.t_ypos(self.options.sub_title);
        for i in 0..2 {
            sx!(self, self.menus.title_hud[i + 2], |o| {
                o.y_pos = y;
                o.y_pos_to = y;
            });
        }

        if !self.check_cam_still() {
            return;
        }

        let cw = self.menus.c_world as usize;
        let wvd = self.layout.world_visual_data;
        self.menus.world_str = self.gamestring(wvd[cw].description_str);
        let s = self.menus.world_str.clone();
        self.set_text(self.options.world_text, &s);

        for i in 0..MAX_LEVELSTRING {
            self.menus.level_str[i].clear();
            self.menus.level_par_str[i].clear();
            self.menus.level_set_by_str[i].clear();
            self.menus.level_coin_str[i].clear();
        }

        let h = if cw as u32 == worldid::GARDEN { 2 * 170 + 682 } else { (wvd[cw].num_single_player_levels as i32 * 170 + 682) as i16 };
        sx!(self, self.options.world_bak, |o| o.height = h);

        let par_str = |t: u32| format!("{}:{:02}.{}0", t / 600, (t / 10) % 60, t % 10);
        let o = self.options.clone_ids();
        if cw as u32 == worldid::GARDEN {
            let f4 = wvd[worldid::FRONTEND as usize].level_visual_data[LEVELID_FRONTEND4 as usize];
            let g1 = wvd[worldid::GARDEN as usize].level_visual_data[0];
            self.menus.level_str[0] = self.gamestring(f4.description_str);
            trgb!(self, o.level_text[0], 255, 255, 255);
            self.menus.level_str[1] = self.gamestring(g1.description_str);
            trgb!(self, o.level_text[1], 255, 255, 255);
            self.menus.level_set_by_str[1] = cstr(&g1.par_name);
            trgb!(self, o.level_set_by_text[1], 255, 255, 255);
            self.menus.level_par_str[1] = par_str(g1.par_time);
            trgb!(self, o.level_par_text[1], 255, 255, 255);
            self.menus.level_coin_str[1] = format!("{}", g1.max_coins);
            srgb!(self, o.level_coin_medal[1], 255, 255, 255);
            trgb!(self, o.level_coin_text[1], 255, 255, 255);
        } else {
            for i in 0..(wvd[cw].num_single_player_levels as usize).min(MAX_LEVELSTRING) {
                let l = wvd[cw].level_visual_data[i];
                self.menus.level_str[i] = self.gamestring(l.description_str);
                self.menus.level_par_str[i] = par_str(l.par_time);
                self.menus.level_set_by_str[i] = cstr(&l.par_name);
                if cw as u32 == worldid::SUPERRETRO {
                    self.menus.level_coin_str[i].clear();
                } else {
                    self.menus.level_coin_str[i] = format!("{}", l.max_coins);
                }
                let (r, g, b) = if l.level_open == LEVEL_OPEN { (255, 255, 255) } else { (30, 60, 0) };
                trgb!(self, o.level_text[i], r, g, b);
                trgb!(self, o.level_par_text[i], r, g, b);
                trgb!(self, o.level_set_by_text[i], r, g, b);
                trgb!(self, o.level_coin_text[i], r, g, b);
                srgb!(self, o.level_coin_medal[i], r, g, b);
            }
        }
        self.sync_level_texts();

        let ln = (self.options.level_num as usize).min(MAX_LEVELSTRING - 1);
        let afc = self.act_frame_count as i32;
        let (pr, pg) = (pulse_r(afc), pulse_g(afc));
        trgb!(self, o.level_set_by_text[ln], pr, pg, 10);
        srgb!(self, o.level_coin_medal[ln], pr, pg, 10);
        trgb!(self, o.level_coin_text[ln], pr, pg, 10);
        trgb!(self, o.level_par_text[ln], pr, pg, 10);
        trgb!(self, o.level_text[ln], pr, pg, 10);

        self.frogmove.frog_facing[0] = 3;
        if let (Some(f), Some(t)) = (self.frogger.frog[0], self.frogmove.curr_tile[0]) {
            let tile = self.map.tile(t);
            let (dv, n) = (tile.dir_vector[3], tile.normal);
            crate::actor2::orientate_ss(&mut self.actor2.actors[f].actor.q_rot, &dv, &n);
        }

        self.t_inc_alpha(o.arcade_text, 0xff);
        self.t_inc_alpha(o.par_text[0], 0xff);
        self.t_inc_alpha(o.par_text[1], 0xff);
        if cw as u32 == worldid::SUPERRETRO {
            self.t_set_draw(o.par_text[2], 0);
        } else {
            self.t_inc_alpha(o.par_text[2], 0xff);
        }
        self.t_inc_alpha(o.select_text, 0xff);
        self.t_inc_alpha(o.world_text, 0xff);
        self.s_inc_alpha(o.world_bak, 128);
        self.s_inc_alpha(o.title_bak, 128);
        self.s_inc_alpha(o.status_bak, 128);

        for i in 0..MAX_LEVELSTRING {
            self.t_inc_alpha(o.level_text[i], 0xff);
            self.t_inc_alpha(o.level_par_text[i], 0xff);
            self.t_inc_alpha(o.level_set_by_text[i], 0xff);
            let lvd = if cw as u32 == worldid::GARDEN {
                match i {
                    0 => Some((wvd[worldid::FRONTEND as usize].level_visual_data[LEVELID_FRONTEND4 as usize], false)),
                    1 => Some((wvd[worldid::GARDEN as usize].level_visual_data[0], true)),
                    _ => None,
                }
            } else {
                Some((wvd[cw].level_visual_data[i], true))
            };
            match lvd {
                Some((l, beaten_shown)) => {
                    if beaten_shown && l.level_beaten != 0 {
                        self.s_inc_alpha(o.beaten_icon[i], 254);
                    } else {
                        self.s_set_draw(o.beaten_icon[i], 0);
                    }
                    if l.max_coins == 25 {
                        self.s_inc_alpha(o.level_coin_medal[i], 0xff);
                        self.t_set_draw(o.level_coin_text[i], 0);
                    } else {
                        self.s_set_draw(o.level_coin_medal[i], 0);
                        self.t_inc_alpha(o.level_coin_text[i], 0xff);
                    }
                }
                None => {
                    self.s_set_draw(o.level_coin_medal[i], 0);
                    self.s_set_draw(o.beaten_icon[i], 0);
                    self.t_inc_alpha(o.level_coin_text[i], 0xff);
                }
            }
        }
    }

    /// Copies `levelStr[]` & co. into the overlays that point at them.
    pub(crate) fn sync_level_texts(&mut self) {
        for i in 0..MAX_LEVELSTRING {
            let m = &self.menus;
            let (a, b, c, d) = (m.level_str[i].clone(), m.level_par_str[i].clone(), m.level_set_by_str[i].clone(), m.level_coin_str[i].clone());
            let o = &self.options;
            let (ta, tb, tc, td) = (o.level_text[i], o.level_par_text[i], o.level_set_by_text[i], o.level_coin_text[i]);
            self.set_text(ta, &a);
            self.set_text(tb, &b);
            self.set_text(tc, &c);
            self.set_text(td, &d);
        }
    }

    /// Port of `CheckForExtras` (Common/menus.c:2108): every level with all 25 coins opens
    /// a retro level, then a character, then an extra.
    pub fn check_for_extras(&mut self) {
        let mut num = 0;
        for s in STORY_SEQUENCE.iter().take(NUM_STORY_LEVELS).skip(1) {
            if self.layout.world_visual_data[s.world_num as usize].level_visual_data[s.level_num as usize].max_coins == 25 {
                num += 1;
            }
        }

        for i in NUM_BASIC_EXTRAS..NUM_EXTRAS {
            self.options.extras_available[i] = 0;
            self.t_set_draw(self.options.extras[i], 0);
        }

        for i in 0..num {
            if i < 9 {
                self.layout.world_visual_data[worldid::SUPERRETRO as usize].level_visual_data[i + 1].level_open = 1;
            } else if i < 13 {
                self.frogger.frog_pool_active[i - 9 + 4] = 1;
            } else {
                self.options.extras_available[i - 13 + NUM_BASIC_EXTRAS] = 1;
            }
        }
        if self.menus.cheat_combos[CHEAT_OPEN_ALL_EXTRAS].state != 0 {
            for i in NUM_BASIC_EXTRAS..NUM_EXTRAS {
                self.options.extras_available[i] = 1;
            }
        }
    }

    /// Port of `UpdateCheatText` (Common/menus.c:2153).
    pub fn update_cheat_text(&mut self) {
        if self.menus.cheat_activated == -1 {
            return;
        }
        if self.menus.cheat_text.is_none() {
            let s = self.menus.cheat_str.clone();
            self.menus.cheat_text = self.textover.create_and_add_text_overlay(2048, 2000, &s, true, 0, Font::Normal, TEXTOVERLAY_SHADOW);
        }
        let afc = self.act_frame_count;
        self.menus.cheat_timer.update(-1, afc);
        if self.menus.cheat_timer.time >= CHEAT_TEXT_TIME / 2 {
            self.t_inc_alpha(self.menus.cheat_text, 255);
        } else {
            self.t_dec_alpha(self.menus.cheat_text);
            if self.t_draw(self.menus.cheat_text) == 0 {
                self.textover.sub_text_overlay(self.menus.cheat_text);
                self.menus.cheat_text = None;
                self.menus.cheat_activated = -1;
            }
        }
    }

    /// Port of `SetMusicVolume` (Common/options.c:3115), PC: `SetCDVolume((globalMusicVol *
    /// 65535)/MAX_SOUND_VOL)`.
    pub fn set_music_volume(&mut self) {
        let v = self.savegame.global_music_vol;
        self.effects.push(Effect::SetMusicVolume { vol: v, cd_volume: (v.wrapping_mul(65535) / MAX_SOUND_VOL) as u32 });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cheat_combo_opens_all_levels_and_toggles_mad_garibs() {
        let mut g = Game::new();
        g.textover.init_text_overlay_linked_list(40);
        g.overlays.init_sprite_overlay_linked_list();
        g.frogger.num_frogs = 1;
        g.game_state.mode = INGAME_MODE;
        g.start_pause_menu();
        assert_eq!(g.game_state.mode, PAUSE_MODE);
        let press = |g: &mut Game, deb: u16, dig: u16| {
            g.levplay.pad_data.debounce[0] = deb;
            g.levplay.pad_data.digital[0] = dig;
            g.run_pause_menu();
        };
        press(&mut g, PAD_SQUARE, PAD_SQUARE);
        for k in CHEAT_COMBOS[CHEAT_OPEN_ALL_LEVELS].key_string.iter().take_while(|&&k| k != 0) {
            press(&mut g, *k, PAD_SQUARE | *k);
        }
        press(&mut g, 0, 0);
        assert_eq!(g.menus.cheat_combos[CHEAT_OPEN_ALL_LEVELS].state, 1);
        assert!(g.layout.world_visual_data.iter().all(|w| w.world_open == WORLD_OPEN));
        assert_eq!(g.options.max_page_allowed, 9);
        // the combo closes the pause menu
        assert_eq!(g.game_state.mode, INGAME_MODE);
        assert_eq!(g.menus.cheat_activated, CHEAT_OPEN_ALL_LEVELS as i32);

        // mad garibs toggles
        g.start_pause_menu();
        for _ in 0..2 {
            g.game_state.mode = PAUSE_MODE;
            press(&mut g, PAD_SQUARE, PAD_SQUARE);
            for k in CHEAT_COMBOS[CHEAT_MAD_GARIBS].key_string.iter().take_while(|&&k| k != 0) {
                press(&mut g, *k, PAD_SQUARE | *k);
            }
            press(&mut g, 0, 0);
        }
        assert!(!g.cheat_mad_garibs());
    }

    #[test]
    fn pause_menu_quit_confirm_flow() {
        let mut g = Game::new();
        g.textover.init_text_overlay_linked_list(40);
        g.overlays.init_sprite_overlay_linked_list();
        g.init_in_game_text_overlays(0, 0);
        g.frogger.num_frogs = 1;
        g.game_state.mode = INGAME_MODE;
        g.start_pause_menu();
        let press = |g: &mut Game, deb: u16| {
            g.levplay.pad_data.debounce[0] = deb;
            g.levplay.pad_data.digital[0] = deb;
            g.run_pause_menu();
        };
        press(&mut g, PAD_DOWN);
        press(&mut g, PAD_DOWN);
        assert_eq!(g.menus.current_pause_selection, 2);
        press(&mut g, PAD_CROSS);
        assert_eq!(g.menus.pause_confirm_mode, 1);
        assert_eq!(g.menus.quitting_level, 2);
        assert!(g.menus.yes_text.is_some());
        // back out with triangle
        press(&mut g, PAD_TRIANGLE);
        assert_eq!(g.menus.pause_confirm_mode, 0);
        assert_eq!(g.menus.current_pause_selection, 2);
        // continue resumes
        press(&mut g, PAD_START);
        assert_eq!(g.game_state.mode, INGAME_MODE);
    }
}
