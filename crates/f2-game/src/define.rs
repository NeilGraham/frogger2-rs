//! Port of `Common/define.h`: game modes, tile states, global game state.

use serde::{Deserialize, Serialize};

/// `enum gameModes` (`define.h:19`). Stored as `u32` like `gameState.mode`.
pub mod mode {
    pub const FRONTEND_MODE: u32 = 0;
    pub const TRAINING_MODE: u32 = 1;
    pub const INGAME_MODE: u32 = 2;
    pub const LEVELCOMPLETE_MODE: u32 = 3;
    pub const WORLDCOMPLETE_MODE: u32 = 4;
    pub const GAMECOMPLETE_MODE: u32 = 5;
    pub const GAMEOVER_MODE: u32 = 6;
    pub const INTRO_MODE: u32 = 7;
    pub const CAMEO_MODE: u32 = 8;
    pub const PAUSE_MODE: u32 = 9;
    pub const DEMO_MODE: u32 = 10;
    pub const CHARSELECT_MODE: u32 = 11;
    pub const FMVPLAY_MODE: u32 = 12;
    pub const CONTROLLERVIEW_MODE: u32 = 13;
    pub const E3_LEVELSELECT_MODE: u32 = 14;
    pub const MULTI_WINRACE_MODE: u32 = 15;
    pub const MULTI_WINCOLLECT_MODE: u32 = 16;
    pub const MULTI_WINBATTLE_MODE: u32 = 17;
    pub const ARTVIEWER_MODE: u32 = 18;
    pub const STARTUP_MODE: u32 = 19;
    pub const TEASERSCREEN_MODE: u32 = 20;
    pub const THEEND_MODE: u32 = 21;
}

pub const SINGLEPLAYER: u8 = 1;
pub const MULTILOCAL: u8 = 2;
pub const MULTIREMOTE: u8 = 3;

pub const INVALID_MODE: u8 = 0;
pub const ARCADE_MODE: u8 = 1;
pub const STORY_MODE: u8 = 2;

pub const DIFFICULTY_EASY: u8 = 0;
pub const DIFFICULTY_NORMAL: u8 = 1;
pub const DIFFICULTY_HARD: u8 = 2;

/// `struct gameStateStruct` (`define.h:73`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameStateStruct {
    pub multi: u8,
    pub single: u8,
    pub mode: u32,
    pub old_mode: u32,
    pub menu_mode: u32,
    pub story_sequence_level: u32,
    pub difficulty: u8,
}

/// `TILESTATE_*` (`define.h:86`).
pub mod tilestate {
    pub const NORMAL: u32 = 0;
    pub const DEADLY: u32 = 1;
    pub const SINK: u32 = 2;
    pub const ICE: u32 = 3;
    pub const SUPERHOP: u32 = 4;
    pub const JOIN: u32 = 5;
    pub const SAFE: u32 = 6;
    pub const FINISHLINE: u32 = 7;
    pub const GRAPPLE: u32 = 8;
    pub const SMASH: u32 = 9;
    pub const BARRED: u32 = 10;
    pub const FALL: u32 = 11;
    pub const FROGGER1AREA: u32 = 12;
    pub const FROGGER2AREA: u32 = 13;
    pub const FROGGER3AREA: u32 = 14;
    pub const FROGGER4AREA: u32 = 15;
    pub const DEADLYFALL: u32 = 16;
    pub const HOT: u32 = 17;
    pub const ELECTRIC: u32 = 18;
    pub const NOSUPER: u32 = 19;
    pub const PANTS: u32 = 20;
    pub const NOSUPERHOT: u32 = 21;
    pub const OCCUPIED: u32 = 22;
    pub const CONVEYOR: u32 = 0x20;
    pub const CONVEYOR_MED: u32 = 0x24;
    pub const CONVEYOR_FAST: u32 = 0x28;
    pub const CONVEYOR_ONEWAY: u32 = 0x2C;
}
