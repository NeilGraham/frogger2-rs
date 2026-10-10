//! Addresses and struct offsets inside the retail USA `frogger2.exe`
//! (image base 0x400000, no relocations), matched against the original
//! source. The evidence for each entry is in `symbols/frogger2_usa.csv`.
//!
//! Only addresses, names and layouts live here, never code from the exe.
//! Struct offsets are the MSVC (x86, default packing) layouts of the source
//! structs, computed with `clang --target=i686-pc-windows-msvc
//! -fdump-record-layouts` and confirmed against field accesses in the exe.

// ---- functions --------------------------------------------------------------

/// `GameLoop` (Common/frontend.c), `void(void)`.
pub const GAME_LOOP: u32 = 0x0041_0ae0;
/// `LoopFunc` (PC/Main.cpp): the `AppLoop` of `RunWindowsLoop`.
pub const LOOP_FUNC: u32 = 0x0043_cc90;
/// `call GameLoop` inside `LoopFunc` (the non-network branch).
pub const LOOP_FUNC_CALL_GAME_LOOP: u32 = 0x0043_cdd0;
/// `UpdateTiming` (MDX Library/mdxTiming.cpp), `void(void)`.
pub const UPDATE_TIMING: u32 = 0x0044_b430;
/// First bytes of `UpdateTiming` (`sub esp,0xc; fld dword [timeInfo.frameSpeed]`).
pub const UPDATE_TIMING_PROLOGUE: [u8; 8] = [0x83, 0xec, 0x0c, 0xd9, 0x05, 0x5c, 0xc1, 0x48];
/// `call UpdateTiming` at the end of each `RunWindowsLoop` iteration (mdxWindows.cpp).
pub const RUN_WINDOWS_LOOP_CALL_UPDATE_TIMING: u32 = 0x0045_5b6e;
/// `InitLevel(unsigned long worldID, unsigned long levelID)` (Common/layout.c), cdecl.
pub const INIT_LEVEL: u32 = 0x0041_78b0;

// ---- globals ----------------------------------------------------------------

/// `MDX_TIMING_VAL timeInfo` (mdxTiming.cpp).
pub const TIME_INFO: u32 = 0x0048_c148;
/// Offsets in `MDX_TIMING_VAL`.
pub mod time_info {
    pub const FIRST_TICKS: u32 = 0;
    pub const TICK_COUNT: u32 = 4;
    pub const FRAME_COUNT: u32 = 8;
    pub const SPEED: u32 = 24;
}
/// `unsigned long actFrameCount` (Common/layout.c).
pub const ACT_FRAME_COUNT: u32 = 0x0048_9954;
/// `fixed gameSpeed` (Common/layout.c).
pub const GAME_SPEED: u32 = 0x0047_4070;
/// `int frameCount` (Common/frontend.c).
pub const FRAME_COUNT: u32 = 0x0048_9734;
/// `struct gameStateStruct gameState` (Common/game.c).
pub const GAME_STATE: u32 = 0x0047_2108;
/// Offsets in `gameStateStruct`.
pub mod game_state {
    pub const MULTI: u32 = 0;
    pub const SINGLE: u32 = 1;
    pub const MODE: u32 = 4;
    pub const DIFFICULTY: u32 = 20;
}
/// `ULONG rseed` (Common/maths.c), initial value 398623.
pub const RSEED: u32 = 0x0047_9d70;
/// `PLAYER player[4]` (Common/frogger.c), `sizeof(PLAYER) == 156`.
pub const PLAYER: u32 = 0x036c_1f00;
pub const PLAYER_SIZE: u32 = 156;
/// Offsets in `PLAYER`.
pub mod player {
    pub const CHARACTER: u32 = 8;
    pub const NUM_SPAWN: u32 = 14;
    pub const LIVES: u32 = 16;
    pub const HEALTH_POINTS: u32 = 20;
    pub const DEATH_BY: u32 = 21;
    pub const WORLD_NUM: u32 = 28;
    pub const LEVEL_NUM: u32 = 29;
    pub const FROG_STATE: u32 = 32;
    pub const CAN_JUMP: u32 = 41;
    pub const HAS_JUMPED: u32 = 42;
    pub const IS_SUPER_HOPPING: u32 = 44;
    pub const IS_SINKING: u32 = 45;
    pub const SAFE_TIME: u32 = 48;
    pub const DEAD_TIME: u32 = 64;
    pub const JUMP_TIME: u32 = 108;
    pub const HEIGHT_JUMPED: u32 = 152;
}
/// `long NUM_FROGS` (Common/frogger.c): active frogs.
pub const NUM_FROGS: u32 = 0x0047_1df0;
/// `ACTOR2 *frog[4]` (Common/frogger.c).
pub const FROG: u32 = 0x0048_9698;
/// `GAMETILE *currTile[4]` (Common/frogmove.c).
pub const CURR_TILE: u32 = 0x0048_96dc;
/// `GAMETILE *firstTile` (Common/map.c).
pub const FIRST_TILE: u32 = 0x036b_c650;
/// `int tileCount` (Common/map.c).
pub const TILE_COUNT: u32 = 0x0048_997c;
pub const GAMETILE_SIZE: u32 = 68;
/// `ACTOR2.actor` is at 0; `ACTOR.position` (SVECTOR) at 12.
pub const ACTOR_POSITION: u32 = 12;
/// `ENEMYLIST enemyList` (Common/enemies.c): `{ENEMY *array; short count, blocks; ENEMY head;}`.
pub const ENEMY_LIST: u32 = 0x036c_23e0;
/// Offsets in `ENEMY`.
pub mod enemy {
    pub const NEXT: u32 = 0;
    pub const UID: u32 = 8;
    pub const NME_ACTOR: u32 = 12;
    pub const FLAGS: u32 = 16;
    pub const SPEED: u32 = 24;
    pub const ACTIVE: u32 = 52;
    pub const IS_WAITING: u32 = 54;
    pub const IN_TILE: u32 = 64;
}
/// `PLATFORMLIST platformList` (Common/platform.c), same shape as `ENEMYLIST`.
pub const PLATFORM_LIST: u32 = 0x036c_21a0;
/// Offsets in `PLATFORM`.
pub mod platform {
    pub const NEXT: u32 = 0;
    pub const UID: u32 = 24;
    pub const PLT_ACTOR: u32 = 124;
    pub const ACTIVE: u32 = 128;
    pub const FLAGS: u32 = 132;
    pub const CURR_SPEED: u32 = 136;
    pub const IS_WAITING: u32 = 140;
    pub const IN_TILE: u32 = 184;
}
/// Offset of `head` in `ENEMYLIST`/`PLATFORMLIST`; `count` is at 4.
pub const LIST_HEAD: u32 = 8;
/// `GARIBLIST garibList` (Common/collect.c): `{GARIB *array; GARIB head; short blocks, count, total, maxCoins;}`.
pub const GARIB_LIST: u32 = 0x036c_2460;
pub mod garib_list {
    pub const HEAD: u32 = 4;
    pub const COUNT: u32 = 38;
    pub const TOTAL: u32 = 40;
    pub const MAX_COINS: u32 = 42;
}
/// Offsets in `GARIB`.
pub mod garib {
    pub const NEXT: u32 = 4;
    pub const ACTIVE: u32 = 8;
    pub const TYPE: u32 = 9;
    pub const POS: u32 = 20;
}
/// `int babiesSaved` / `int numBabies`.
pub const BABIES_SAVED: u32 = 0x0048_91a4;
pub const NUM_BABIES: u32 = 0x0048_9978;
/// `FVECTOR currCamSource` / `currCamTarget` (Common/cam.c).
pub const CURR_CAM_SOURCE: u32 = 0x0047_1160;
pub const CURR_CAM_TARGET: u32 = 0x0048_91b0;
/// `PADDATA padData` (PC/islpad.h); `digital[8]` at +18, `debounce[8]` at +34.
pub const PAD_DATA: u32 = 0x0048_a7c8;
pub const PAD_DIGITAL: u32 = PAD_DATA + 18;
pub const PAD_DEBOUNCE: u32 = PAD_DATA + 34;
/// `int demoLevels[3][2]` (Common/levplay.c).
pub const DEMO_LEVELS: u32 = 0x0047_93bc;
/// `int nextDemo` (Common/levplay.c).
pub const NEXT_DEMO: u32 = 0x0048_9970;
/// `char goingToDemo` (Common/menus.c).
pub const GOING_TO_DEMO: u32 = 0x0048_9df0;
/// `int fadingOut` (PC/fadeout.cpp).
pub const FADING_OUT: u32 = 0x0048_ab98;
/// `unsigned long curPlayKey` / `playKeyCount` / `unsigned long *playKeyList` (levplay.c).
pub const CUR_PLAY_KEY: u32 = 0x036b_c678;
pub const PLAY_KEY_COUNT: u32 = 0x036b_c67c;
pub const PLAY_KEY_LIST: u32 = 0x036b_c688;
/// `int quittingDemo` (levplay.c).
pub const QUITTING_DEMO: u32 = 0x0048_9974;
/// `TIMER endLevelTimer` (Common/game.c): `.time` (u16).
pub const END_LEVEL_TIMER: u32 = 0x036c_1c68;

// ---- schema v2 (trace.version=2); layouts in symbols/frogger2_structs.md ----

/// More `PLAYER` offsets (Common/frogger.h).
pub mod player_ext {
    pub const FROGON: u32 = 10;
    pub const FROGUNDER: u32 = 12;
    pub const OLD_LIVES: u32 = 18;
    pub const INPUT_PAUSE: u32 = 22;
    pub const SPAWN_TIMER: u32 = 24;
    pub const SPAWN_SCORE_LEVEL: u32 = 26;
    pub const IDLE_TIME: u32 = 36;
    pub const IDLE_ENABLE: u32 = 40;
    pub const HAS_DOUBLE_JUMPED: u32 = 43;
    pub const EXTENDED_HOP_DIR: u32 = 46;
    pub const STUN_TIME: u32 = 56;
    pub const CROAK_TIME: u32 = 72;
    pub const AUTOHOP_TIME: u32 = 80;
    pub const QUICKHOP_TIME: u32 = 88;
    pub const SLOWHOP_TIME: u32 = 96;
    pub const JUMP_START_FRAME: u32 = 104;
    pub const JUMP_SPEED: u32 = 112;
    pub const JUMP_MULTIPLIER: u32 = 116;
    pub const JUMP_FWD: u32 = 120;
    pub const JUMP_UP: u32 = 132;
    pub const JUMP_ORIGIN: u32 = 144;
}
/// `GARIB.dropSpeed` (short); `garibList.blocks` at +36.
pub const GARIB_DROP_SPEED: u32 = 28;
pub const GARIB_LIST_BLOCKS: u32 = 36;
/// collect.c `int pauseFlag` / `TIMER pauseTimer`.
pub const GARIB_PAUSE_FLAG: u32 = 0x036c_2440;
pub const GARIB_PAUSE_TIMER: u32 = 0x036c_2448;
/// `BABY babyList[NUM_BABIES]` (Common/babyfrog.c), `sizeof(BABY) == 24`.
pub const BABY_LIST: u32 = 0x036c_24e0;
pub const BABY_SIZE: u32 = 24;
pub const NUM_BABIES_MAX: u32 = 5;
pub mod baby {
    pub const IS_SAVED: u32 = 0;
    pub const IDLE: u32 = 8;
    pub const COLLECT: u32 = 12;
    pub const BABY: u32 = 16;
}
/// `unsigned char scriptFlags[8]` / `short scriptCounters[16]` (Common/script.c).
pub const SCRIPT_FLAGS: u32 = 0x0048_a238;
pub const SCRIPT_COUNTERS: u32 = 0x0048_a240;
/// `TRIGGERLIST triggerList` (Common/event.c): `head` at 0 (a TRIGGER), `count` (short) at 0x80.
pub const TRIGGER_LIST: u32 = 0x036c_22e0;
pub const TRIGGER_LIST_COUNT: u32 = 0x80;
/// Offsets in `TRIGGER` (`sizeof == 60`).
pub mod trigger {
    pub const NEXT: u32 = 0;
    pub const FLAGS: u32 = 0x30;
    pub const COUNT: u32 = 0x34;
    pub const DELAY: u32 = 0x38;
}
/// `SPECFXLIST sfxList` (Common/specfx.c): `count` (short) at 4, `head` (a SPECFX) at 0x14.
pub const SFX_LIST: u32 = 0x036b_b9c0;
pub const SFX_LIST_COUNT: u32 = 4;
pub const SFX_LIST_HEAD: u32 = 0x14;
/// Offsets in `SPECFX` (`sizeof == 136`).
pub mod specfx {
    pub const NEXT: u32 = 0;
    pub const ORIGIN: u32 = 0x2c;
    pub const TYPE: u32 = 0x4c;
    pub const LIFETIME: u32 = 0x68;
    pub const UPDATE_TYPE: u32 = 0x7c;
}
/// `SPRITELIST sprList` / `PARTICLELIST partList`: `count` (short) at 4.
pub const SPR_LIST: u32 = 0x036c_1ea0;
pub const PART_LIST: u32 = 0x036c_2280;
pub const POOL_COUNT: u32 = 4;
/// `TONGUE tongue[4]` (Common/tongue.c), `sizeof(TONGUE) == 76`.
pub const TONGUE: u32 = 0x036b_b880;
pub const TONGUE_SIZE: u32 = 76;
pub mod tongue {
    pub const SOURCE: u32 = 0;
    pub const TARGET: u32 = 8;
    pub const POS: u32 = 0x10;
    pub const FLAGS: u32 = 0x30;
    pub const RADIUS: u32 = 0x34;
    pub const PROGRESS: u32 = 0x38;
    pub const TYPE: u32 = 0x3c;
    pub const CAN_TONGUE: u32 = 0x3d;
    pub const THING: u32 = 0x48;
}
/// `TIMER scoreTimer` / `modeTimer` (Common/game.c), `goTimer` (hud.c), `demoTimeout` (levplay.c): `.time` (u16) at 0.
pub const SCORE_TIMER: u32 = 0x036c_1c40;
pub const MODE_TIMER: u32 = 0x036c_1c60;
pub const GO_TIMER: u32 = 0x036c_1908;
pub const DEMO_TIMEOUT: u32 = 0x036b_c680;
/// Camera globals (Common/cam.c): `camVect`, `currCamDist`, `camDist` (SVECTOR),
/// `currCamOffset`, `camLookOfs`, `transCamSpeedMult`, `yFOVNew`, `fixedDir`/`fixedPos`/`fixedUp` (char),
/// `cam_shakiness`, `int camFacing[4]`.
pub const CAM_SOURCE: u32 = 0x0047_1180;
pub const CAM_TARGET: u32 = 0x036c_24a0;
pub const CAM_VECT: u32 = 0x036c_24d0;
pub const CURR_CAM_DIST: u32 = 0x0047_1170;
pub const CAM_DIST: u32 = 0x0047_11b8;
pub const CURR_CAM_OFFSET: u32 = 0x036c_24b0;
pub const CAM_LOOK_OFS: u32 = 0x0047_11b0;
pub const TRANS_CAM_SPEED_MULT: u32 = 0x0047_11d0;
pub const Y_FOV_NEW: u32 = 0x0047_11a0;
pub const FIXED_DIR: u32 = 0x0048_91d1;
pub const FIXED_POS: u32 = 0x0048_91d2;
pub const FIXED_UP: u32 = 0x0048_91d4;
pub const CAM_SHAKINESS: u32 = 0x0048_91fc;
pub const CAM_FACING: u32 = 0x0048_91bc;
/// `TRANSCAMERALIST tcList` (cam.c): `{TRANSCAMERA *array; short count, blocks;}`; `TRANSCAMERA.tile` at 0, sizeof 36.
pub const TC_LIST: u32 = 0x036c_24c0;
pub const TC_LIST_COUNT: u32 = 4;
pub const TRANSCAMERA_SIZE: u32 = 36;
/// `int frogFacing[4]` (Common/frogmove.c).
pub const FROG_FACING: u32 = 0x0048_96f0;
/// `FOG fog` (Common/frogger.c): `{u8 r,g,b,mode; short min,max;}`.
pub const FOG: u32 = 0x0047_1b50;
/// hud.c `handAngle` / `storeFrameCount`.
pub const HAND_ANGLE: u32 = 0x036c_1ab0;
pub const STORE_FRAME_COUNT: u32 = 0x0048_994c;

/// `gameState.mode` values (Common/define.h).
pub mod mode {
    pub const FRONTEND: u32 = 0;
    pub const INGAME: u32 = 2;
    pub const LEVELCOMPLETE: u32 = 3;
    pub const GAMEOVER: u32 = 6;
    pub const PAUSE: u32 = 9;
    pub const DEMO: u32 = 10;
    pub const STARTUP: u32 = 19;
}

// ---- local multiplayer (Common/multi.c, Common/options.c) --------------------

/// `GoMulti` (Common/options.c:950), `void(void)`: the frontend's multiplayer start
/// (`NUM_FROGS = options.numPlayers`, level from `multiWorldNum/multiLevelNum[levelSelection]`,
/// `gameState = {MULTILOCAL, INVALID_MODE, INGAME}`, `multiplayerMode = multiGameTypes[world]`,
/// `player[i].character = options.playerChar[i]` for `i < currentPlayer`, `InitLevel`).
/// The retail build also clears `options.mode` (0x47a1ec = -1) and the `draw` flag (+0x21)
/// of two option overlays (pointers at 0x47a328 / 0x47a32c).
pub const GO_MULTI: u32 = 0x0042_2e40;
/// `options` fields read by `GoMulti` (Common/options.c `OPTIONSOBJECTS options`).
pub mod options {
    /// `options.mode` (int; GoMulti writes -1).
    pub const MODE: u32 = 0x0047_a1ec;
    /// Two `TEXTOVERLAY *` whose `draw` (+0x21) GoMulti clears.
    pub const OVERLAY_A: u32 = 0x0047_a328;
    pub const OVERLAY_B: u32 = 0x0047_a32c;
    pub const OVERLAY_DRAW: u32 = 0x21;
    /// `long numPlayers` -> `NUM_FROGS`.
    pub const NUM_PLAYERS: u32 = 0x0047_a370;
    /// `long levelSelection` (index into `multiWorldNum`/`multiLevelNum`).
    pub const LEVEL_SELECTION: u32 = 0x0047_a380;
    /// `long currentPlayer` (players whose character was picked).
    pub const CURRENT_PLAYER: u32 = 0x0047_a384;
    /// `char playerChar[4]`.
    pub const PLAYER_CHAR: u32 = 0x0047_a5c0;
}
/// `long multiLevelNum[8]` / `long multiWorldNum[8]` (Common/options.c:548).
pub const MULTI_LEVEL_NUM: u32 = 0x0047_aba8;
pub const MULTI_WORLD_NUM: u32 = 0x0047_abc8;
pub const NUM_MULTI_LEVELS: u32 = 8;
/// `char multiGameTypes[MAX_WORLDS]` (Common/multi.c:75).
pub const MULTI_GAME_TYPES: u32 = 0x0047_a13c;
/// `int multiplayerMode` (Common/multi.c).
pub const MULTIPLAYER_MODE: u32 = 0x036b_c500;
/// `long started`, `unsigned long numMultiItems`, `fixed initialCamHeight` (Common/multi.c).
pub const MP_STARTED: u32 = 0x0048_9df8;
pub const NUM_MULTI_ITEMS: u32 = 0x0048_9dfc;
pub const INITIAL_CAM_HEIGHT: u32 = 0x0048_9e00;
/// `signed char gameWinner, matchWinner` (Common/multi.c).
pub const GAME_WINNER: u32 = 0x0047_a12b;
pub const MATCH_WINNER: u32 = 0x0047_a12c;
/// `TIMER multiTimer`, `TIMER endTimer` (Common/multi.c).
pub const MULTI_TIMER: u32 = 0x036b_c420;
pub const END_TIMER: u32 = 0x036b_c508;
/// `MPINFO mpl[4]` (Common/multi.h), `sizeof(MPINFO) == 44`.
pub const MPL: u32 = 0x036b_c440;
pub const MPINFO_SIZE: u32 = 44;
pub mod mpinfo {
    pub const WINS: u32 = 0;
    pub const LAP: u32 = 2;
    pub const TIMER: u32 = 4;
    pub const PENALTY: u32 = 8;
    pub const CHECK: u32 = 12;
    pub const SCORE: u32 = 14;
    pub const TRAIL: u32 = 16;
    pub const READY: u32 = 18;
    pub const LASTTILE: u32 = 20;
}
/// `fogStore` (Common/menus.c; `GoMulti`: `fog.max = fogStore`), `lastActFrameCount`.
pub const FOG_STORE: u32 = 0x036b_c61c;
pub const LAST_ACT_FRAME_COUNT: u32 = 0x0048_9958;
/// `gameState.multi` values (Common/define.h).
pub const MULTILOCAL: u8 = 2;
