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
