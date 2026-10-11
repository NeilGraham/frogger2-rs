//! Port of `Common/options.c` (wave G): the frontend's menus run on the special tiles of
//! the 3D frontend level — the global options menu (sound / extras), the sound volumes,
//! extras (difficulty, FMV list, credits, reset best times / story, mini frogs, feeding
//! frenzy, art viewer), multiplayer number/character/level select, the story book, the
//! arcade machine's level and character select, and the art viewer.
//!
//! Render-facing state (the renderer must draw it):
//! * `page` (`bookpage.obe` actor, PC hardware path): while `grab_to_texture != 0` the
//!   original rendered `DrawPageB` (Common/menus.c:1858: `page32.bmp`, "Chapter N" title,
//!   the two chapter-name lines and `CHAPTERn.bmp`) into the page texture; the renderer
//!   should draw that page for `page_num` (and reset `grab_to_texture` itself if it models
//!   the countdown). Without hardware (`r_hardware == false`) the book is drawn with the
//!   `chapter_text`/`chapter_name_text`/`chapter_pic` overlays instead.
//! * `portrait_gouraud[i]` (vertex colour of the multiplayer portrait actors,
//!   `SetActorGouraudValues`) and the actors' `override_tex = "multihidden.bmp"` for locked
//!   characters.
//! * The art viewer (`ARTVIEWER_MODE`) shows `ARTWORKnn` backdrops via `crate::backdrop`.
//!
//! PC setup dialogs / key remapping (`PC/*.cpp`) are not called by options.c and stay host
//! side; volumes are reported with `Effect::SetMusicVolume` / `Effect::SetSoundVolume`.

#![allow(clippy::needless_range_loop, clippy::collapsible_match)] // C loop/switch shapes kept

use crate::prelude::*;

use f2_math::{distance_between_points_ff, fmul, rsin, slide_vector_to_vector_ff, sub_vector_fff, FVector, SVector};
use serde::{Deserialize, Serialize};

use crate::anim::SAMPLE_VOLUME;
use crate::define::mode::*;
use crate::define::{ARCADE_MODE, INVALID_MODE, MULTILOCAL, MULTIREMOTE, SINGLEPLAYER, STORY_MODE};
use crate::effects::Effect;
use crate::frogger::{FROG_FROGGER, FROG_LILLIE, FROG_NUMFROGS, MAX_IDLE_TIME, ORIG_FROG_POOL};
use crate::frogmove::*;
use crate::frontend::{BACK_CHARS_X, BACK_CHARS_Y, NUM_LIVES};
use crate::game::INPUT_POLLPAUSE;
use crate::ids::{ActorId, OverlayId, TextOverlayId};
use crate::language::{c_sprintf, FmtArg};
use crate::layout::{worldid, LEVELID_FRONTEND1, LEVELID_FRONTEND4, LEVEL_CLOSED, ORIG_WORLD_VISUAL_DATA, WORLD_CLOSED};
use crate::levplay::{PAD_CROSS, PAD_DOWN, PAD_LEFT, PAD_RIGHT, PAD_START, PAD_TRIANGLE, PAD_UP};
use crate::menus::strs::*;
use crate::menus::{pulse_g, pulse_r, srgb, sx, trgb, tx, BOOK_NUM_PAGES, CHAPTER_PIC, MAX_LEVELSTRING, MAX_SOUND_VOL, TILENUM_ARCADE, TILENUM_MULTI};
use crate::multi::{MULTIMODE_RACE, MULTI_GAME_TYPES};
use crate::pcaudio::{gen_sfx, GEN_COLLECT_COIN, GEN_FROG_HOP, GEN_SUPER_HOP};
use crate::specfx::FXTYPE_SPARKLYTRAIL;
use crate::sprite::SPRITE_SUBTRACTIVE;
use crate::story::{NUM_FMV_SEQUENCES, NUM_STORY_LEVELS, STORY_SEQUENCE};
use crate::textover::{Font, TEXTOVERLAY_PIXELS, TEXTOVERLAY_SHADOW};
use crate::timer::Timer;
use crate::Game;

// Extras (`options.h:16`).
pub const EXTRA_DIFFICULTY: usize = 0;
pub const EXTRA_VIEW_FMV: usize = 1;
pub const EXTRA_CREDITS: usize = 2;
pub const EXTRA_RESET_BEST_TIMES: usize = 3;
pub const EXTRA_RESET_STORY_MODE: usize = 4;
pub const EXTRA_MINI: usize = 5;
pub const EXTRA_VIEW_ART: usize = 6;
pub const EXTRA_FEEDING_FRENZY: usize = 7;
pub const NUM_EXTRAS: usize = 8;
/// `NUM_BASIC_EXTRAS` (`options.h:30`).
pub const NUM_BASIC_EXTRAS: usize = 5;

// Options modes (`options.h:32`).
pub const OP_SOUND: i32 = 0;
pub const OP_EXTRA: i32 = 1;
pub const OP_CONTROL: i32 = 2;
pub const OP_GLOBALMENU: i32 = 3;
pub const OP_MULTIPLAYERNUMBER: i32 = 4;
pub const OP_LEVELSEL: i32 = 5;
pub const OP_CHARSEL: i32 = 6;
pub const OP_BOOK: i32 = 7;
pub const OP_ARCADE: i32 = 8;
pub const OP_SP_CHARSEL: i32 = 9;
pub const OP_FMV: i32 = 10;
pub const OP_TOTAL: usize = 11;

/// `DEFAULT_VOLUME` (`options.h:48`).
pub const DEFAULT_VOLUME: i32 = 8;
/// `NUM_OPTIONS` (`options.h:49`).
pub const NUM_OPTIONS: usize = 2;
/// `MAX_FMV` (`options.h:76`).
pub const MAX_FMV: usize = 12;
/// `E_HEIGHT` (`options.c:110`).
pub const E_HEIGHT: i32 = 200;
/// `MAX_CREDITS` (`options.c:147`).
pub const MAX_CREDITS: usize = 250;
/// `CREDIT_SPACING` (PC).
pub const CREDIT_SPACING: i32 = 300;
/// `NUM_MULTI_LEVELS` / `NUM_MULTICHARS` (`options.c:552`, `571`).
pub const NUM_MULTI_LEVELS: usize = 8;
pub const NUM_MULTICHARS: usize = 8;
/// `MAX_ARTWORK` (`options.c:3127`).
pub const MAX_ARTWORK: i32 = 21;

/// Control slots (`options.h:66`).
pub const C_UP: usize = 0;
pub const C_DOWN: usize = 1;
pub const C_LEFT: usize = 2;
pub const C_RIGHT: usize = 3;
pub const C_SELECT: usize = 4;
pub const C_BACK: usize = 5;
pub const C_TOTAL: usize = 6;

/// The `CONTROLFUNC`s of `controlsList` (`options.c:519`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    SoundUp,
    SoundDown,
    SoundLeft,
    SoundRight,
    OptionBack,
    ExtraUp,
    ExtraDown,
    ExtraSelect,
    MenuBack,
    MenuLeft,
    MenuRight,
    MenuSelect,
    NumPSelect,
    NumPLeft,
    NumPRight,
    MPLevBack,
    MPLevLeft,
    MPLevRight,
    MPLevSelect,
    MPCharBack,
    MPCharLeft,
    MPCharRight,
    MPCharSelect,
    BookDown,
    BookLeft,
    BookRight,
    BookSelect,
    ArcadeUp,
    ArcadeDown,
    ArcadeLeft,
    ArcadeRight,
    ArcadeSelect,
    ArcadeBack,
    SPCharUp,
    SPCharDown,
    SPCharSelect,
    SPCharBack,
    FMVUp,
    FMVDown,
    FMVSelect,
    FMVBack,
}

use Control::*;

/// `controlsList[OP_TOTAL][C_TOTAL]` (`options.c:519`): up, down, left, right, select, back.
pub const CONTROLS_LIST: [[Option<Control>; C_TOTAL]; OP_TOTAL] = [
    [Some(SoundUp), Some(SoundDown), Some(SoundLeft), Some(SoundRight), None, Some(OptionBack)], // Sound
    [Some(ExtraUp), Some(ExtraDown), None, None, Some(ExtraSelect), Some(OptionBack)],           // Extra
    [None, None, None, None, None, Some(OptionBack)],                                            // Control
    [None, Some(MenuBack), Some(MenuLeft), Some(MenuRight), Some(MenuSelect), Some(MenuBack)],   // Main menu
    [Some(MenuBack), Some(NumPSelect), Some(NumPLeft), Some(NumPRight), Some(NumPSelect), Some(MenuBack)], // numplayer select
    [Some(MPLevBack), None, Some(MPLevLeft), Some(MPLevRight), Some(MPLevSelect), Some(MPLevBack)], // level select
    [Some(MPCharBack), None, Some(MPCharLeft), Some(MPCharRight), Some(MPCharSelect), Some(MPCharBack)], // char select
    [None, Some(BookDown), Some(BookLeft), Some(BookRight), Some(BookSelect), Some(BookDown)],   // book
    [Some(ArcadeUp), Some(ArcadeDown), Some(ArcadeLeft), Some(ArcadeRight), Some(ArcadeSelect), Some(ArcadeBack)], // arcade
    [Some(SPCharUp), Some(SPCharDown), None, None, Some(SPCharSelect), Some(SPCharBack)],        // sp char select
    [Some(FMVUp), Some(FMVDown), None, None, Some(FMVSelect), Some(FMVBack)],                    // fmv menu
];

const fn fv(vx: i32, vy: i32, vz: i32) -> FVector {
    FVector { vx, vy, vz }
}

/// `mpCharSelCamSource[]` / `mpCharSelCamTarget[]` (`options.c:533`).
pub const MP_CHAR_SEL_CAM_SOURCE: [FVector; NUM_MULTICHARS] = [
    fv(-10091755, 4149992, 21120782),
    fv(-9406870, 4149992, 21662182),
    fv(-10177212, 4120894, 21747884),
    fv(-10303569, 4099387, 20813268),
    fv(-9499646, 4035047, 20305478),
    fv(-8242643, 4041855, 21015220),
    fv(-9515788, 4074510, 21300772),
    fv(-9529414, 4088749, 20709322),
];
pub const MP_CHAR_SEL_CAM_TARGET: [FVector; NUM_MULTICHARS] = [
    fv(-9118355, 4149992, 25098982),
    fv(-12314170, 4149992, 24544182),
    fv(-14269212, 4120894, 21676484),
    fv(-13507269, 4099387, 18264868),
    fv(-10662746, 4035047, 16378678),
    fv(-7028143, 4041855, 17104020),
    fv(-5541588, 4074510, 20328372),
    fv(-6328414, 4088749, 23255522),
];
/// `optTargets[NUM_OPTIONS]` (`options.c:557`).
pub const OPT_TARGETS: [FVector; NUM_OPTIONS] = [fv(-7502023, 6134937, -4187053), fv(-8539837, 5777338, -3717867)];
/// `multiLevelNum[]` / `multiWorldNum[]` (`options.c:565`).
pub const MULTI_LEVEL_NUM: [u32; NUM_MULTI_LEVELS] = [3, 0, 3, 4, 5, 10, 11, 12];
pub const MULTI_WORLD_NUM: [u32; NUM_MULTI_LEVELS] = [
    worldid::ANCIENT,
    worldid::CITY,
    worldid::LABORATORY,
    worldid::LABORATORY,
    worldid::LABORATORY,
    worldid::SUPERRETRO,
    worldid::SUPERRETRO,
    worldid::SUPERRETRO,
];
/// `portraitPos[NUM_MULTICHARS]` (`options.c:581`).
pub const PORTRAIT_POS: [SVector; NUM_MULTICHARS] = [
    SVector { vx: -2170, vy: 1050, vz: 6370, pad: 0 },
    SVector { vx: -3150, vy: 1060, vz: 6130, pad: 0 },
    SVector { vx: -3780, vy: 1050, vz: 5270, pad: 0 },
    SVector { vx: -3450, vy: 1040, vz: 4330, pad: 0 },
    SVector { vx: -2660, vy: 1050, vz: 3810, pad: 0 },
    SVector { vx: -1680, vy: 1060, vz: 4030, pad: 0 },
    SVector { vx: -1120, vy: 1050, vz: 4870, pad: 0 },
    SVector { vx: -1370, vy: 1040, vz: 5810, pad: 0 },
];
/// `pageToStoryLevel[BOOK_NUM_PAGES]` (`options.c:2600`).
pub const PAGE_TO_STORY_LEVEL: [i32; BOOK_NUM_PAGES] = [0, 2, 4, 6, 8, 10, 12, 14, 15, 16];
/// `levNameColR/G[]` (`options.c:1693`).
pub const LEV_NAME_COL_R: [u8; 8] = [255, 255, 0, 0, 0, 255, 255, 255];
pub const LEV_NAME_COL_G: [u8; 8] = [0, 0, 255, 255, 255, 255, 255, 255];
/// `numberText[4]` (`options.c:513`).
pub const NUMBER_TEXT: [&str; 4] = ["1", "2", "3", "4"];
/// `mpcamspeed`, `soundwobble`, `soundwobblefac`, `SOUND_SCALE` (`options.c`).
pub const MPCAMSPEED: i32 = 4096 * 70;
pub const SOUNDWOBBLE: i32 = 40;
pub const SOUNDWOBBLEFAC: i32 = 400;
pub const SOUND_SCALE: i32 = 15;

/// `CREDIT_DATA` (`options.h:181`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CreditData {
    pub spacing: u8,
    pub r: u8,
    pub g: u8,
    pub b: u8,
}
const GREEN: (u8, u8, u8) = (60, 255, 0);
const WHITE: (u8, u8, u8) = (255, 255, 255);
const RED: (u8, u8, u8) = (255, 0, 0);
const fn cd(spacing: u8, c: (u8, u8, u8)) -> CreditData {
    CreditData { spacing, r: c.0, g: c.1, b: c.2 }
}

/// `creditData[]` (`options.c:151`): extra spacing and colour of each credits line.
pub const CREDIT_DATA: [CreditData; 249] = [
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, RED),
    cd(0, RED),
    cd(0, GREEN),
    cd(3, RED),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(0, GREEN),
    cd(3, WHITE),
    cd(1, WHITE),
    cd(0, WHITE),
    cd(1, WHITE),
    cd(0, WHITE),
    cd(0, WHITE),
    cd(0, WHITE),
    cd(1, WHITE),
    cd(1, WHITE),
    cd(0, WHITE),
];

/// `OPTIONSOBJECTS options` (`options.h:78`) plus the other globals of options.c.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OptionsState {
    pub init: i32,
    /// Current options mode (`OP_*`, -1 = none).
    pub mode: i32,
    pub selection: i32,
    pub title: Option<TextOverlayId>,
    pub sub_title: Option<TextOverlayId>,
    pub title_str: String,
    pub sub_title_str: String,
    pub sound_vol: i32,
    pub music_vol: i32,
    pub sound_icons: [Option<OverlayId>; 2],
    /// `page` (`bookpage.obe`, PC hardware path).
    pub page: Option<ActorId>,
    /// `book` (enemy uid 123's actor).
    pub book: Option<ActorId>,
    pub page_dir: i32,
    pub page_draw_count: i32,
    pub page_num: i32,
    pub max_page_allowed: i32,
    pub level_num: i32,
    pub extras: [Option<TextOverlayId>; NUM_EXTRAS],
    /// Host extension (`GameOptions::settings_menu`): the "Settings" entry after the
    /// available extras (selection index `numExtrasAvailable`).
    pub settings_text: Option<TextOverlayId>,
    pub extras_state: [i32; NUM_EXTRAS],
    pub extras_toggle: [i32; NUM_EXTRAS],
    pub extras_available: [i32; NUM_EXTRAS],
    pub num_p_text: Option<TextOverlayId>,
    pub num_p_text2: Option<TextOverlayId>,
    pub num_p_str: String,
    pub num_p_str2: String,
    pub num_players: i32,
    pub sound_selection: i32,
    pub extra_selection: i32,
    pub multi_selection: i32,
    pub level_selection: i32,
    pub current_player: i32,
    pub multi_face: [Option<OverlayId>; 8],
    /// `controls[C_TOTAL]`: the `controlsList` row copied at the start of `RunOptionsMenu`.
    pub controls: [Option<ControlSer>; C_TOTAL],
    pub player_num: [Option<TextOverlayId>; 4],
    pub world_bak: Option<OverlayId>,
    pub title_bak: Option<OverlayId>,
    pub status_bak: Option<OverlayId>,
    pub level_text: [Option<TextOverlayId>; MAX_LEVELSTRING],
    pub level_par_text: [Option<TextOverlayId>; MAX_LEVELSTRING],
    pub level_set_by_text: [Option<TextOverlayId>; MAX_LEVELSTRING],
    pub level_coin_text: [Option<TextOverlayId>; MAX_LEVELSTRING],
    pub sp_char: i32,
    pub world_text: Option<TextOverlayId>,
    pub arcade_text: Option<TextOverlayId>,
    pub select_text: Option<TextOverlayId>,
    pub par_text: [Option<TextOverlayId>; 3],
    pub fmv_num: i32,
    pub fmv_text: [Option<TextOverlayId>; NUM_FMV_SEQUENCES],
    pub sound_bar: [Option<OverlayId>; 2],
    pub chapter_text: [Option<TextOverlayId>; 2],
    pub chapter_name_text: [[Option<TextOverlayId>; 2]; 2],
    pub chapter_pic: [Option<OverlayId>; 2],
    pub mp_text: Option<TextOverlayId>,
    pub mp_str: String,
    pub yes_no_text: [Option<TextOverlayId>; 2],
    pub chest_lid: Option<ActorId>,
    pub door: Option<ActorId>,
    pub level_coin_medal: [Option<OverlayId>; MAX_LEVELSTRING],
    pub sound_bak: [Option<OverlayId>; 2],
    pub sfx_text: [Option<TextOverlayId>; 2],
    pub sound_instr_text: [Option<TextOverlayId>; 2],
    pub warn_text: [Option<TextOverlayId>; 4],
    pub beaten_icon: [Option<OverlayId>; MAX_LEVELSTRING],
    pub player_char: [i8; 4],

    // ---- options.c globals ----
    /// `warnStr` split at its NULs (`fontFitToWidth` output).
    pub warn_str: Vec<String>,
    pub diff_str: String,
    pub playing_fmv: bool,
    pub on_off_str: [String; NUM_EXTRAS],
    /// `pageDir` (global `short`, PSX clip boxes).
    pub g_page_dir: i32,
    pub sound_swap_time: i32,
    pub game_selected: i32,
    /// `confirmMode` (0 or the `EXTRA_RESET_*` being confirmed).
    pub confirm_mode: i32,
    pub last_face: i32,
    pub static_flash: i32,
    pub chest_timer: Timer,
    pub max_players: i32,
    pub going_to_art_viewer: bool,
    pub current_art: i32,
    pub chapter_str: [String; 2],
    pub sound_count: i32,
    pub num_fmv_open: i32,
    pub num_extras_available: i32,
    pub chest_open_anim: i32,
    pub art_timer: Timer,
    /// `NUM_ARCADE_WORLDS` (8; 10 with `CHEAT_EXTRA_LEVELS`).
    pub num_arcade_worlds: i32,
    pub credits_text: Vec<Option<TextOverlayId>>,
    pub cam_still: i32,
    pub credits_running: i32,
    pub ending_credits: i32,
    pub pic_offset: i32,
    pub credits_y: i32,
    /// `optionsLastButton`.
    pub options_last_button: u16,
    pub fmv_str: Vec<String>,
    /// `mpLevNameArray[2]`.
    pub mp_lev_name_array: [String; 2],
    /// `portraitActor[NUM_MULTICHARS]`.
    pub portrait_actor: [Option<ActorId>; NUM_MULTICHARS],
    /// `SetActorGouraudValues(portraitActor[i], r, g, b)` (render state).
    pub portrait_gouraud: [[u8; 3]; NUM_MULTICHARS],
    /// `properMultiSelect` (FINAL_MASTER: 1).
    pub proper_multi_select: bool,
    /// `grabToTexture` (PC): render the book page (`DrawPageB`) into the page texture.
    pub grab_to_texture: i32,
    /// `rHardware` (PC): D3D hardware rendering; the book is a 3D page then.
    pub r_hardware: bool,
}

/// Serializable stand-in for a `CONTROLFUNC` (`Control` index).
pub type ControlSer = u8;

const ALL_CONTROLS: [Control; 41] = [
    SoundUp,
    SoundDown,
    SoundLeft,
    SoundRight,
    OptionBack,
    ExtraUp,
    ExtraDown,
    ExtraSelect,
    MenuBack,
    MenuLeft,
    MenuRight,
    MenuSelect,
    NumPSelect,
    NumPLeft,
    NumPRight,
    MPLevBack,
    MPLevLeft,
    MPLevRight,
    MPLevSelect,
    MPCharBack,
    MPCharLeft,
    MPCharRight,
    MPCharSelect,
    BookDown,
    BookLeft,
    BookRight,
    BookSelect,
    ArcadeUp,
    ArcadeDown,
    ArcadeLeft,
    ArcadeRight,
    ArcadeSelect,
    ArcadeBack,
    SPCharUp,
    SPCharDown,
    SPCharSelect,
    SPCharBack,
    FMVUp,
    FMVDown,
    FMVSelect,
    FMVBack,
];

fn control_ser(c: Control) -> ControlSer {
    ALL_CONTROLS.iter().position(|&x| x == c).unwrap_or(0) as u8
}

impl Default for OptionsState {
    fn default() -> Self {
        OptionsState {
            init: 0,
            mode: -1,
            selection: 0,
            title: None,
            sub_title: None,
            title_str: String::new(),
            sub_title_str: String::new(),
            sound_vol: 0,
            music_vol: 0,
            sound_icons: [None; 2],
            page: None,
            book: None,
            page_dir: 0,
            page_draw_count: 0,
            page_num: 0,
            max_page_allowed: 0,
            level_num: 0,
            extras: [None; NUM_EXTRAS],
            settings_text: None,
            extras_state: [0; NUM_EXTRAS],
            extras_toggle: [0; NUM_EXTRAS],
            extras_available: [0; NUM_EXTRAS],
            num_p_text: None,
            num_p_text2: None,
            num_p_str: String::new(),
            num_p_str2: String::new(),
            num_players: 0,
            sound_selection: 0,
            extra_selection: 0,
            multi_selection: 0,
            level_selection: 0,
            current_player: 0,
            multi_face: [None; 8],
            controls: [None; C_TOTAL],
            player_num: [None; 4],
            world_bak: None,
            title_bak: None,
            status_bak: None,
            level_text: [None; MAX_LEVELSTRING],
            level_par_text: [None; MAX_LEVELSTRING],
            level_set_by_text: [None; MAX_LEVELSTRING],
            level_coin_text: [None; MAX_LEVELSTRING],
            sp_char: 0,
            world_text: None,
            arcade_text: None,
            select_text: None,
            par_text: [None; 3],
            fmv_num: 0,
            fmv_text: [None; NUM_FMV_SEQUENCES],
            sound_bar: [None; 2],
            chapter_text: [None; 2],
            chapter_name_text: [[None; 2]; 2],
            chapter_pic: [None; 2],
            mp_text: None,
            mp_str: String::new(),
            yes_no_text: [None; 2],
            chest_lid: None,
            door: None,
            level_coin_medal: [None; MAX_LEVELSTRING],
            sound_bak: [None; 2],
            sfx_text: [None; 2],
            sound_instr_text: [None; 2],
            warn_text: [None; 4],
            beaten_icon: [None; MAX_LEVELSTRING],
            player_char: [0; 4],
            warn_str: Vec::new(),
            diff_str: String::new(),
            playing_fmv: false,
            on_off_str: Default::default(),
            g_page_dir: 0,
            sound_swap_time: 0,
            game_selected: 0,
            confirm_mode: 0,
            last_face: 0,
            static_flash: 0,
            chest_timer: Timer::default(),
            max_players: 0,
            going_to_art_viewer: false,
            current_art: 0,
            chapter_str: Default::default(),
            sound_count: 0,
            num_fmv_open: 1,
            num_extras_available: NUM_BASIC_EXTRAS as i32,
            chest_open_anim: 1,
            art_timer: Timer::default(),
            num_arcade_worlds: 8,
            credits_text: vec![None; MAX_CREDITS],
            cam_still: 0,
            credits_running: 0,
            ending_credits: 0,
            pic_offset: 230,
            credits_y: 0,
            options_last_button: 0xffff,
            fmv_str: vec![String::new(); NUM_FMV_SEQUENCES],
            mp_lev_name_array: Default::default(),
            portrait_actor: [None; NUM_MULTICHARS],
            portrait_gouraud: [[255; 3]; NUM_MULTICHARS],
            proper_multi_select: true,
            grab_to_texture: 0,
            r_hardware: true,
        }
    }
}

impl OptionsState {
    /// A copy to read overlay ids from while mutating the game.
    pub fn clone_ids(&self) -> OptionsState {
        self.clone()
    }
}

/// `GAMESTRING(STR_CREDITS_1 + j)[0] == '*'`: the end of the credits.
fn credits_end(g: &Game, j: usize) -> bool {
    g.language.gamestring(STR_CREDITS_1 + j as i32).starts_with('*') || j >= MAX_CREDITS - 1
}

impl Game {
    fn hop_sfx(&mut self) {
        self.play_sample(gen_sfx(GEN_FROG_HOP), None, 0, SAMPLE_VOLUME, -1);
    }

    fn super_hop_sfx(&mut self) {
        self.play_sample(gen_sfx(GEN_SUPER_HOP), None, 0, SAMPLE_VOLUME, -1);
    }

    /// `PlaySample(FindSample(utilStr2CRC(name)), NULL, 0, SAMPLE_VOLUME, -1)`.
    fn named_sfx(&mut self, name: &str) {
        self.play_sample(f2_formats::crc::crc(name), None, 0, SAMPLE_VOLUME, -1);
    }

    fn set_portrait_gouraud(&mut self, i: usize, v: u8) {
        if let Some(p) = self.options.portrait_gouraud.get_mut(i) {
            *p = [v; 3];
        }
    }

    fn store_frontend_camera(&mut self) {
        let c = &self.cam;
        let m = &mut self.menus;
        m.store_cam_source = c.cam_source;
        m.store_curr_cam_source = c.curr_cam_source;
        m.store_cam_target = c.cam_target;
        m.store_curr_cam_target = c.curr_cam_target;
        m.store_cam_dist = FVector { vx: c.cam_dist.vx as i32, vy: c.cam_dist.vy as i32, vz: c.cam_dist.vz as i32 };
        m.store_curr_cam_dist = c.curr_cam_dist;
        m.store_cam_offset = c.cam_offset;
        // BUG(original): storeCamOffset is overwritten with currCamOffset (storeCurrCamOffset
        // is left alone).
        m.store_cam_offset = c.curr_cam_offset;
        m.store_cam_vect = c.cam_vect;
    }

    /// Dispatches a `CONTROLFUNC`.
    pub fn options_control(&mut self, c: Control) {
        match c {
            SoundUp => self.sound_up(),
            SoundDown => self.sound_down(),
            SoundLeft => self.sound_left(),
            SoundRight => self.sound_right(),
            OptionBack => self.option_back(),
            ExtraUp => self.extra_up(),
            ExtraDown => self.extra_down(),
            ExtraSelect => self.extra_select(),
            MenuBack => self.menu_back(),
            MenuLeft => self.menu_left(),
            MenuRight => self.menu_right(),
            MenuSelect => self.menu_select(),
            NumPSelect => self.num_p_select(),
            NumPLeft => self.num_p_left(),
            NumPRight => self.num_p_right(),
            MPLevBack => self.mp_lev_back(),
            MPLevLeft => self.mp_lev_left(),
            MPLevRight => self.mp_lev_right(),
            MPLevSelect => self.mp_lev_select(),
            MPCharBack => self.mp_char_back(),
            MPCharLeft => self.mp_char_left(),
            MPCharRight => self.mp_char_right(),
            MPCharSelect => self.mp_char_select(),
            BookDown => self.book_down(),
            BookLeft => self.book_left(),
            BookRight => self.book_right(),
            BookSelect => self.book_select(),
            ArcadeUp => self.arcade_up(),
            ArcadeDown => self.arcade_down(),
            ArcadeLeft => self.arcade_left(),
            ArcadeRight => self.arcade_right(),
            ArcadeSelect => self.arcade_select(),
            ArcadeBack => self.arcade_back(),
            SPCharUp => self.sp_char_up(),
            SPCharDown => self.sp_char_down(),
            SPCharSelect => self.sp_char_select(),
            SPCharBack => self.sp_char_back(),
            FMVUp => self.fmv_up(),
            FMVDown => self.fmv_down(),
            FMVSelect => self.fmv_select(),
            FMVBack => self.fmv_back(),
        }
    }

    /// Port of `MenuRight` (Common/options.c:601).
    pub fn menu_right(&mut self) {
        if self.options.selection != 0 {
            self.options.selection -= 1;
            self.hop_sfx();
        }
    }

    /// Port of `MenuLeft` (Common/options.c:613).
    pub fn menu_left(&mut self) {
        if self.options.selection == 0 {
            self.options.selection += 1;
            self.hop_sfx();
        }
    }

    /// Port of `NumPLeft` (Common/options.c:625).
    pub fn num_p_left(&mut self) {
        if self.options.cam_still != 0 {
            self.options.num_players -= 1;
            if self.options.num_players < 2 {
                self.options.num_players = self.options.max_players;
            }
            self.hop_sfx();
        }
    }

    /// Port of `NumPSelect` (Common/options.c:640).
    pub fn num_p_select(&mut self) {
        if self.options.cam_still != 0 && self.options.max_players > 1 {
            self.options.mode = OP_CHARSEL;
            self.super_hop_sfx();
            self.options.mode = OP_CHARSEL;
            self.options.multi_selection = 0;
            self.options.current_player = 0;
            for i in 0..(self.options.num_players.max(0) as usize).min(4) {
                tx!(self, self.options.player_num[i], |o| o.draw = 1);
            }
        }
    }

    /// Port of `MPCharLeft` (Common/options.c:658).
    pub fn mp_char_left(&mut self) {
        if self.options.cam_still != 0 {
            self.options.multi_selection = (self.options.multi_selection - 1 + NUM_MULTICHARS as i32) % NUM_MULTICHARS as i32;
            self.hop_sfx();
        }
    }

    /// Port of `MPCharRight` (Common/options.c:667).
    pub fn mp_char_right(&mut self) {
        if self.options.cam_still != 0 {
            self.options.multi_selection = (self.options.multi_selection + 1) % NUM_MULTICHARS as i32;
            self.hop_sfx();
        }
    }

    /// Port of `MPStart` (Common/options.c:676).
    pub fn mp_start(&mut self) {
        self.options.mode = -1;
        tx!(self, self.options.num_p_text, |o| o.draw = 0);
        tx!(self, self.options.num_p_text2, |o| o.draw = 0);
        self.go_multi();
    }

    /// Port of `MPCharSelect` (Common/options.c:683).
    pub fn mp_char_select(&mut self) {
        if self.options.cam_still == 0 {
            return;
        }
        let ms = self.options.multi_selection;
        let cp = self.options.current_player.clamp(0, 3) as usize;
        let allowed = (0..cp).all(|i| self.options.player_char[i] as i32 != ms);
        if allowed && self.frogger.frog_pool_active[ms as usize % FROG_NUMFROGS] != 0 {
            self.effects.push(Effect::PlayVoice { player: 0, uid: f2_formats::crc::crc("frogokay") });
            self.set_portrait_gouraud(ms as usize, 32);
            tx!(self, self.options.player_num[cp], |o| o.draw = 0);
            self.options.player_char[cp] = ms as i8;
            let mf = self.options.multi_face[ms as usize % 8];
            sx!(self, mf, |o| {
                o.draw = 1;
                o.r = 255;
                o.g = 255;
                o.b = 255;
                o.a = 255;
                o.x_pos = BACK_CHARS_X[cp] as i16;
                o.y_pos = BACK_CHARS_Y[cp] as i16;
            });
            // BUG(original): xPosTo/yPosTo are set on multiFace[currentPlayer], not on the
            // selected face.
            let mfc = self.options.multi_face[cp];
            sx!(self, mfc, |o| {
                o.x_pos_to = BACK_CHARS_X[cp] as i16;
                o.y_pos_to = BACK_CHARS_Y[cp] as i16;
            });
            self.options.current_player += 1;
            if self.options.current_player >= self.options.num_players {
                self.options.mode = OP_LEVELSEL;
            }
        } else {
            self.effects.push(Effect::PlayVoice { player: 0, uid: f2_formats::crc::crc("frogannoyed") });
        }
    }

    /// Port of `MPCharBack` (Common/options.c:731).
    pub fn mp_char_back(&mut self) {
        self.hop_sfx();
        if self.options.current_player != 0 {
            self.options.current_player -= 1;
            let cp = self.options.current_player.clamp(0, 3) as usize;
            self.options.multi_selection = self.options.player_char[cp] as i32;
            let pc = self.options.player_char[cp] as usize % 8;
            sx!(self, self.options.multi_face[pc], |o| o.draw = 0);
            tx!(self, self.options.player_num[cp], |o| o.draw = 1);
            self.set_portrait_gouraud(pc, 255);
        } else {
            self.options.mode = OP_MULTIPLAYERNUMBER;
            for i in 0..(self.options.num_players.max(0) as usize).min(4) {
                tx!(self, self.options.player_num[i], |o| o.draw = 0);
            }
        }
    }

    fn mp_level_open(&self, j: usize) -> bool {
        self.layout.world_visual_data[MULTI_WORLD_NUM[j] as usize].level_visual_data[MULTI_LEVEL_NUM[j] as usize].level_open != 0
    }

    /// Port of `MPLevLeft` (Common/options.c:757).
    pub fn mp_lev_left(&mut self) {
        if self.options.cam_still == 0 {
            return;
        }
        let n = NUM_MULTI_LEVELS as i32;
        let ls = self.options.level_selection;
        let mut j = (ls - 1 + n) % n;
        while j != ls {
            if self.mp_level_open(j as usize) {
                self.hop_sfx();
                self.options.level_selection = j;
                break;
            }
            j = (j - 1 + n) % n;
        }
    }

    /// Port of `MPLevRight` (Common/options.c:775).
    pub fn mp_lev_right(&mut self) {
        if self.options.cam_still == 0 {
            return;
        }
        let n = NUM_MULTI_LEVELS as i32;
        let ls = self.options.level_selection;
        let mut j = (ls + 1 + n) % n;
        while j != ls {
            if self.mp_level_open(j as usize) {
                self.hop_sfx();
                self.options.level_selection = j;
                break;
            }
            j = (j + 1 + n) % n;
        }
    }

    /// Port of `MPLevSelect` (Common/options.c:793).
    pub fn mp_lev_select(&mut self) {
        if self.options.cam_still != 0 {
            self.screen_fade(255, 0, 30);
            self.fadeout.keep_fade = true;
            self.options.game_selected = 1;
            self.menus.last_arcade = TILENUM_MULTI;
            self.store_frontend_camera();
        }
    }

    /// Port of `MPLevBack` (Common/options.c:814).
    pub fn mp_lev_back(&mut self) {
        self.options.mode = OP_CHARSEL;
        self.hop_sfx();
        self.mp_char_back();
    }

    /// Port of `NumPRight` (Common/options.c:824).
    pub fn num_p_right(&mut self) {
        if self.options.cam_still != 0 {
            self.options.num_players += 1;
            if self.options.num_players > self.options.max_players {
                self.options.num_players = 2;
            }
            self.hop_sfx();
        }
    }

    /// Port of `MenuBack` (Common/options.c:841): hop off the menu tile.
    pub fn menu_back(&mut self) {
        self.frogger.fog.max = self.menus.fog_store as _;

        if self.options.mode == OP_MULTIPLAYERNUMBER {
            let m = &self.menus;
            self.cam.curr_cam_source = m.store_cam_source;
            self.cam.curr_cam_target = m.store_cam_target;
            self.cam.cam_source = m.store_cam_source;
            self.cam.cam_target = m.store_cam_target;
            self.cam.curr_cam_offset = m.store_curr_cam_offset;
            self.cam.cam_offset = m.store_curr_cam_offset;
        }

        let cf = self.cam.cam_facing[0];
        if self.options.mode == OP_GLOBALMENU {
            self.frogger.player[0].extended_hop_dir = MOVE_LEFT as u8;
            self.cam.prev_cam_facing[0] = cf;
            self.frogmove.frog_facing[0] = (cf + MOVE_LEFT) & 3;
            self.frogger.player[0].frog_state |= match cf {
                0 => FROGSTATUS_ISWANTINGSUPERHOPL,
                1 => FROGSTATUS_ISWANTINGSUPERHOPU,
                2 => FROGSTATUS_ISWANTINGSUPERHOPR,
                3 => FROGSTATUS_ISWANTINGSUPERHOPD,
                _ => 0,
            };
        } else {
            self.frogger.player[0].extended_hop_dir = MOVE_RIGHT as u8;
            self.frogger.player[0].frog_state |= match cf {
                0 => FROGSTATUS_ISWANTINGSUPERHOPU,
                1 => FROGSTATUS_ISWANTINGSUPERHOPR,
                2 => FROGSTATUS_ISWANTINGSUPERHOPD,
                3 => FROGSTATUS_ISWANTINGSUPERHOPL,
                _ => 0,
            };
            self.frogmove.frog_facing[0] = (cf + MOVE_RIGHT) & 3;
        }
        let p = &mut self.frogger.player[0];
        p.can_jump = 0;
        p.is_super_hopping = 1;
        p.has_jumped = 1;

        // Hide all the overlays
        self.done_options_menu();

        // update player idleTime (Maybe not needed?)
        self.frogger.player[0].idle_time = MAX_IDLE_TIME;

        self.cam.fixed_pos = 0;
        self.cam.fixed_dir = 0;
        self.menus.reached_point = 0;
    }

    /// Port of `MenuSelect` (Common/options.c:919).
    pub fn menu_select(&mut self) {
        self.options.mode = self.options.selection;
        if self.options.mode == OP_EXTRA {
            if let Some(lid) = self.options.chest_lid {
                self.options.chest_timer.init(4);
                let anim = self.options.chest_open_anim;
                self.actor_animate(lid, anim, false, false, 40, false);
            }
            for i in 0..(self.options.num_extras_available.max(0) as usize).min(NUM_EXTRAS) {
                tx!(self, self.options.extras[i], |o| o.a = 0);
            }
            tx!(self, self.options.settings_text, |o| o.a = 0);
        }
        self.super_hop_sfx();
    }

    /// Port of `GoMulti` (Common/options.c:950).
    pub fn go_multi(&mut self) {
        self.frogger.fog.max = self.menus.fog_store as _;
        self.frogger.num_frogs = self.options.num_players;
        self.last_act_frame_count = self.act_frame_count;

        let ls = (self.options.level_selection as usize).min(NUM_MULTI_LEVELS - 1);
        self.frogger.player[0].world_num = MULTI_WORLD_NUM[ls] as u8;
        self.frogger.player[0].level_num = MULTI_LEVEL_NUM[ls] as u8;

        self.game_state.mode = INGAME_MODE;
        self.game_state.single = INVALID_MODE;
        if self.game_state.multi != MULTIREMOTE {
            self.game_state.multi = MULTILOCAL;
        }
        self.multi.multiplayer_mode = MULTI_GAME_TYPES[MULTI_WORLD_NUM[ls] as usize];

        for i in 0..(self.options.current_player.max(0) as usize).min(4) {
            self.frogger.player[i].character = self.options.player_char[i] as u8;
        }
        let (w, l) = (self.frogger.player[0].world_num as u32, self.frogger.player[0].level_num as u32);
        self.init_level(w, l);
    }

    fn sound_highlight(&mut self) {
        let sel = self.options.sound_selection.clamp(0, 1) as usize;
        let o = self.options.clone_ids();
        srgb!(self, o.sound_icons[1 - sel], 64, 64, 64);
        srgb!(self, o.sound_icons[sel], 255, 255, 255);
        srgb!(self, o.sound_bar[1 - sel], 255, 255, 255);
        srgb!(self, o.sound_bar[sel], 255, 0, 0);
    }

    /// Port of `SoundUp` (Common/options.c:978).
    pub fn sound_up(&mut self) {
        if self.options.sound_selection > 0 {
            let o = self.options.clone_ids();
            srgb!(self, o.sound_icons[0], 255, 255, 255);
            srgb!(self, o.sound_icons[1], 64, 64, 64);
            srgb!(self, o.sound_bar[0], 255, 0, 0);
            srgb!(self, o.sound_bar[1], 255, 255, 255);
            self.options.sound_selection -= 1;
            self.hop_sfx();
            self.options.sound_swap_time = self.act_frame_count as i32;
        }
    }

    /// Port of `SoundDown` (Common/options.c:1014).
    pub fn sound_down(&mut self) {
        if self.options.sound_selection < 1 {
            self.options.sound_selection += 1;
            self.sound_highlight();
            self.hop_sfx();
            self.options.sound_swap_time = self.act_frame_count as i32;
        }
    }

    fn sound_vol_changed(&mut self) {
        // globalSoundVol is read by PlaySample (PC/pcaudio.c:298).
        self.pcaudio.global_sound_vol = self.savegame.global_sound_vol;
        self.effects.push(Effect::SetSoundVolume { vol: self.savegame.global_sound_vol });
        if self.options.sound_count <= 0 {
            self.super_hop_sfx();
            self.options.sound_count = 4096 * 10;
        }
    }

    /// Port of `SoundLeft` (Common/options.c:1036).
    pub fn sound_left(&mut self) {
        match self.options.sound_selection {
            0 => {
                if self.savegame.global_music_vol != 0 {
                    self.savegame.global_music_vol -= 1;
                    self.set_music_volume();
                }
            }
            1 => {
                if self.savegame.global_sound_vol != 0 {
                    self.savegame.global_sound_vol -= 1;
                    self.sound_vol_changed();
                }
            }
            _ => {}
        }
    }

    /// Port of `SoundRight` (Common/options.c:1067).
    pub fn sound_right(&mut self) {
        match self.options.sound_selection {
            0 => {
                if self.savegame.global_music_vol < MAX_SOUND_VOL {
                    self.savegame.global_music_vol += 1;
                    self.set_music_volume();
                }
            }
            1 => {
                if self.savegame.global_sound_vol < MAX_SOUND_VOL {
                    self.savegame.global_sound_vol += 1;
                    self.sound_vol_changed();
                }
            }
            _ => {}
        }
    }

    /// Port of `ExtraUp` (Common/options.c:1102).
    pub fn extra_up(&mut self) {
        if self.options.credits_running == 0 {
            let n = self.options.num_extras_available + self.extras_settings_on() as i32;
            self.options.extra_selection =
                if self.options.confirm_mode != 0 { 1 - self.options.extra_selection } else { (self.options.extra_selection - 1 + n) % n };
            self.hop_sfx();
        }
    }

    /// Port of `ExtraDown` (Common/options.c:1117).
    pub fn extra_down(&mut self) {
        if self.options.credits_running == 0 {
            let n = self.options.num_extras_available + self.extras_settings_on() as i32;
            self.options.extra_selection =
                if self.options.confirm_mode != 0 { 1 - self.options.extra_selection } else { (self.options.extra_selection + 1) % n };
            self.hop_sfx();
        }
    }

    fn on_off_text(&self, i: usize) -> String {
        c_sprintf(&self.gamestring(STR_EXTRAS_1 + i as i32), &[FmtArg::Str(self.gamestring(STR_OFF - self.options.extras_state[i]))])
    }

    fn diff_text(&self) -> String {
        c_sprintf(&self.gamestring(STR_EXTRAS_1), &[FmtArg::Str(self.gamestring(STR_DIFFICULTY_1 + self.game_state.difficulty as i32))])
    }

    /// Port of `ExtraSelect` (Common/options.c:1132).
    pub fn extra_select(&mut self) {
        self.super_hop_sfx();
        if self.options.credits_running != 0 {
            self.options.ending_credits = 1;
            return;
        }
        if self.extras_settings_on() && self.options.confirm_mode == 0 && self.options.extra_selection == self.options.num_extras_available {
            // Host extension: open the host's settings screen; Extras stays open with
            // "Settings" selected.
            self.effects.push(crate::effects::Effect::OpenSettings);
            return;
        }
        let es = (self.options.extra_selection.max(0) as usize).min(NUM_EXTRAS - 1);
        if self.options.extras_toggle[es] != 0 {
            self.options.extras_state[es] = (self.options.extras_state[es] == 0) as i32;
            self.options.on_off_str[es] = self.on_off_text(es);
            let s = self.options.on_off_str[es].clone();
            self.set_text(self.options.extras[es], &s);
        }
        trgb!(self, self.options.extras[es], 255, 255, 255);

        let switch_val = if self.options.confirm_mode != 0 { self.options.confirm_mode as usize } else { es };
        match switch_val {
            EXTRA_DIFFICULTY => {
                self.game_state.difficulty = ((self.game_state.difficulty as i32 + 1) % 3) as _;
                self.options.diff_str = self.diff_text();
                let s = self.options.diff_str.clone();
                self.set_text(self.options.extras[EXTRA_DIFFICULTY], &s);
            }
            EXTRA_VIEW_FMV => {
                self.options.mode = OP_FMV;
                for i in 0..(self.options.num_extras_available.max(0) as usize).min(NUM_EXTRAS) {
                    tx!(self, self.options.extras[i], |o| o.draw = 0);
                }
                tx!(self, self.options.settings_text, |o| o.draw = 0);
                self.options.num_fmv_open = 2 + self.options.max_page_allowed;
                let last = STORY_SEQUENCE[NUM_STORY_LEVELS - 1];
                if self.layout.world_visual_data[last.world_num as usize].level_visual_data[last.level_num as usize].level_completed != 0 {
                    self.options.num_fmv_open += 1;
                }
                let n = self.options.num_fmv_open;
                for i in 0..(n.max(0) as usize).min(NUM_FMV_SEQUENCES) {
                    let s = if i == 0 {
                        self.gamestring(STR_FMV_1)
                    } else if i == NUM_FMV_SEQUENCES - 1 - crate::story::FMV_INTRO as usize {
                        self.gamestring(STR_FMV_2)
                    } else {
                        let k = (i as i32 - 1) * 2;
                        format!("{} - {} {}", i, self.gamestring(STR_CHAPTER_1a + k), self.gamestring(STR_CHAPTER_1b + k))
                    };
                    self.options.fmv_str[i] = s.clone();
                    let y = (2048 - (n / 2) * E_HEIGHT) + (i as i32 * E_HEIGHT);
                    self.options.fmv_text[i] = self.textover.create_and_add_text_overlay(2048, y as i16, &s, true, 255, Font::Small, TEXTOVERLAY_SHADOW);
                    trgb!(self, self.options.fmv_text[i], 200, 200, 200);
                }
            }
            EXTRA_VIEW_ART => {
                self.screen_fade(255, 0, 30);
                self.fadeout.keep_fade = true;
                self.options.going_to_art_viewer = true;
            }
            EXTRA_CREDITS => self.start_credits(),
            EXTRA_RESET_BEST_TIMES | EXTRA_RESET_STORY_MODE => {
                let which = switch_val;
                if self.options.confirm_mode == 0 {
                    self.options.confirm_mode = which as i32;
                    self.options.extra_selection = 1;
                    let y = self.textover.get(self.options.extras[which]).map_or(0, |o| o.y_pos) + 300;
                    tx!(self, self.options.yes_no_text[0], |o| {
                        o.y_pos = y;
                        o.y_pos_to = y;
                    });
                    tx!(self, self.options.yes_no_text[1], |o| {
                        o.y_pos = y + 200;
                        o.y_pos_to = y + 200;
                    });
                } else {
                    if self.options.extra_selection == 0 {
                        if which == EXTRA_RESET_BEST_TIMES {
                            self.reset_best_times();
                        } else {
                            self.reset_story_mode();
                        }
                    }
                    self.options.extra_selection = self.options.confirm_mode;
                    self.options.confirm_mode = 0;
                }
            }
            EXTRA_FEEDING_FRENZY => {
                self.tongue.eat_everything_mode = self.options.extras_state[es] as u8;
            }
            EXTRA_MINI => {
                self.frogger.global_frog_scale = if self.options.extras_state[es] != 0 { fmul(2457, 1500) } else { 2457 };
                let s = self.frogger.global_frog_scale;
                if let Some(f) = self.frogger.frog[0] {
                    let a = &mut self.actor2.actors[f].actor;
                    a.size = FVector { vx: s, vy: s, vz: s };
                    if let Some(sh) = a.shadow.as_mut() {
                        sh.radius = (s / 8) << 12;
                    }
                }
            }
            _ => {}
        }
    }

    /// The confirmed `EXTRA_RESET_BEST_TIMES` (Common/options.c:1198).
    fn reset_best_times(&mut self) {
        for (w, ow) in self.layout.world_visual_data.iter_mut().zip(ORIG_WORLD_VISUAL_DATA.iter()) {
            let n = (w.num_levels as usize).min(w.level_visual_data.len());
            for (l, ol) in w.level_visual_data.iter_mut().zip(ow.level_visual_data.iter()).take(n) {
                l.par_time = ol.par_time;
                l.par_name = ol.par_name;
                l.level_beaten = 0;
            }
        }
        self.save_game();
    }

    /// The confirmed `EXTRA_RESET_STORY_MODE` (Common/options.c:1226).
    fn reset_story_mode(&mut self) {
        self.options.level_num = 0;
        self.menus.c_world = 0;
        self.options.static_flash = 5;
        self.menus.cheat_combos[crate::menus::CHEAT_OPEN_ALL_LEVELS].state = 0;
        self.options.max_page_allowed = 0;
        self.options.page_num = 0;
        if self.options.page.is_some() {
            self.options.grab_to_texture = 1;
        } else {
            let pn = self.options.page_num;
            let t = crate::specfx::find_texture(CHAPTER_PIC[pn as usize]);
            sx!(self, self.options.chapter_pic[0], |o| o.tex = t);
            sx!(self, self.options.chapter_pic[1], |o| o.tex = t);
            self.options.chapter_str[0] = c_sprintf(&self.gamestring(STR_CHAPTER), &[FmtArg::Int(pn + 1)]);
            let s = self.options.chapter_str[0].clone();
            self.set_text(self.options.chapter_text[0], &s);
            let (a, b) = (self.gamestring(STR_CHAPTER_1a + pn * 2), self.gamestring(STR_CHAPTER_1a + pn * 2 + 1));
            self.set_text(self.options.chapter_name_text[0][0], &a);
            self.set_text(self.options.chapter_name_text[0][1], &b);
        }
        for i in 4..8 {
            self.frogger.frog_pool_active[i] = 0;
            if let Some(a) = self.options.portrait_actor[i] {
                if let Some(a) = self.actor2.actors.get_mut(a) {
                    a.override_tex = Some("multihidden.bmp".into());
                }
            }
        }
        for (w, ow) in self.layout.world_visual_data.iter_mut().zip(ORIG_WORLD_VISUAL_DATA.iter()) {
            w.world_open = ow.world_open;
            let n = (w.num_levels as usize).min(w.level_visual_data.len());
            for (l, ol) in w.level_visual_data.iter_mut().zip(ow.level_visual_data.iter()).take(n) {
                l.max_coins = 0;
                l.level_completed = ol.level_completed;
                l.level_open = ol.level_open;
            }
        }
        self.save_game();
        self.check_for_extras();
    }

    /// Port of `OptionBack` (Common/options.c:1305).
    pub fn option_back(&mut self) {
        if self.options.credits_running != 0 {
            self.options.ending_credits = 1;
        } else if self.options.confirm_mode != 0 {
            self.options.extra_selection = self.options.confirm_mode;
            self.options.confirm_mode = 0;
        } else {
            if self.options.mode == OP_EXTRA {
                if let Some(lid) = self.options.chest_lid {
                    let anim = self.options.chest_open_anim;
                    self.actor_animate(lid, anim, false, false, -40, false);
                }
            }
            self.options.mode = OP_GLOBALMENU;
            // Disable all overlays (Yes... even ones not for this menu, but not that slow and neater.)
            let o = self.options.clone_ids();
            for i in 0..2 {
                for s in [o.sound_icons[i], o.sound_bar[i], o.sound_bak[i]] {
                    sx!(self, s, |x| {
                        x.draw = 0;
                        x.a = 0;
                    });
                }
            }
            for i in 0..(self.options.num_extras_available.max(0) as usize).min(NUM_EXTRAS) {
                tx!(self, o.extras[i], |x| x.draw = 0);
            }
            tx!(self, self.options.settings_text, |x| x.draw = 0);
        }
        self.hop_sfx();
        sx!(self, self.menus.title_hud[0], |o| {
            o.a = 0;
            o.draw = 0;
        });
    }

    /// Port of `CreateOptionsObjects` (Common/options.c:1355).
    pub fn create_options_objects(&mut self) {
        for i in 0..FROG_NUMFROGS {
            self.options.multi_face[i] =
                self.overlays.create_and_add_sprite_overlay(1500, (i * 300 + 1000) as i16, Some(ORIG_FROG_POOL[i].icon), 512, 512, 255, 0);
            sx!(self, self.options.multi_face[i], |o| {
                o.num = 2;
                o.draw = 0;
            });
        }

        let (y, n) = (self.gamestring(STR_YES), self.gamestring(STR_NO));
        self.options.yes_no_text[0] = self.textover.create_and_add_text_overlay(2048, 2000, &y, true, 255, Font::Small, TEXTOVERLAY_SHADOW);
        self.options.yes_no_text[1] = self.textover.create_and_add_text_overlay(2048, 2400, &n, true, 255, Font::Small, TEXTOVERLAY_SHADOW);
        tx!(self, self.options.yes_no_text[0], |o| o.draw = 0);
        tx!(self, self.options.yes_no_text[1], |o| o.draw = 0);

        // Main title
        let s = self.options.title_str.clone();
        self.options.title = self.textover.create_and_add_text_overlay(2048, 270, &s, true, 0, Font::Normal, TEXTOVERLAY_SHADOW);
        tx!(self, self.options.title, |o| {
            o.b = 0;
            o.draw = 0;
        });

        // Sub text, initially for multiplayer, but maybe for options too.
        let s = self.options.sub_title_str.clone();
        self.options.sub_title = self.textover.create_and_add_text_overlay(2048, 4096 - 400, &s, true, 0, Font::Small, TEXTOVERLAY_SHADOW);
        tx!(self, self.options.sub_title, |o| o.draw = 0);
        let s = self.options.mp_str.clone();
        self.options.mp_text = self.textover.create_and_add_text_overlay(2048, 550, &s, true, 255, Font::Small, TEXTOVERLAY_SHADOW);

        // Number of players text
        let (a, b) = (self.options.num_p_str.clone(), self.options.num_p_str2.clone());
        self.options.num_p_text = self.textover.create_and_add_text_overlay(2048, 2048, &a, true, 255, Font::Normal, 0);
        self.options.num_p_text2 = self.textover.create_and_add_text_overlay(2048, 2198, &b, true, 255, Font::Normal, 0);

        // Sound options
        self.options.sound_icons[0] = self.overlays.create_and_add_sprite_overlay(900, 1750 - 256, Some("MUSVOL0"), 512, 512, 0, 0);
        sx!(self, self.options.sound_icons[0], |o| o.draw = 0);
        self.options.sound_icons[1] = self.overlays.create_and_add_sprite_overlay(900, 2550 - 256, Some("SFXVOL0"), 512, 512, 0, 0);
        sx!(self, self.options.sound_icons[1], |o| o.draw = 0);

        let bak_w = (100 * SOUND_SCALE + 32) as i16;
        self.options.sound_bak[0] = self.overlays.create_and_add_sprite_overlay(1600 - 16, 1750 - 17, None, bak_w, 134, 0, SPRITE_SUBTRACTIVE);
        sx!(self, self.options.sound_bak[0], |o| o.draw = 0);
        let mw = (self.savegame.global_music_vol * SOUND_SCALE) as i16;
        self.options.sound_bar[0] = self.overlays.create_and_add_sprite_overlay(1600, 1750, None, mw, 100, 0, 0);
        sx!(self, self.options.sound_bar[0], |o| {
            o.draw = 0;
            o.num = 1;
        });
        self.options.sound_bak[1] = self.overlays.create_and_add_sprite_overlay(1600 - 16, 2550 - 17, None, bak_w, 134, 0, SPRITE_SUBTRACTIVE);
        sx!(self, self.options.sound_bak[1], |o| o.draw = 0);
        let sw = (self.savegame.global_sound_vol * SOUND_SCALE) as i16;
        self.options.sound_bar[1] = self.overlays.create_and_add_sprite_overlay(1600, 2550, None, sw, 100, 0, 0);
        sx!(self, self.options.sound_bar[1], |o| {
            o.draw = 0;
            o.num = 1;
        });
        self.sound_highlight();

        let (a, b) = (self.gamestring(STR_MUS_VOL), self.gamestring(STR_SFX_VOL));
        self.options.sfx_text[0] = self.textover.create_and_add_text_overlay(2350, 1400, &a, true, 255, Font::Small, TEXTOVERLAY_SHADOW);
        self.options.sfx_text[1] = self.textover.create_and_add_text_overlay(2350, 2220, &b, true, 255, Font::Small, TEXTOVERLAY_SHADOW);
        tx!(self, self.options.sfx_text[0], |o| o.draw = 0);
        tx!(self, self.options.sfx_text[1], |o| o.draw = 0);

        let (a, b) = (self.gamestring(STR_SOUND_INSTR_2), self.gamestring(STR_SOUND_INSTR_1));
        self.options.sound_instr_text[0] = self.textover.create_and_add_text_overlay(1700, 3070, &a, false, 255, Font::Small, TEXTOVERLAY_SHADOW);
        self.options.sound_instr_text[1] = self.textover.create_and_add_text_overlay(1700, 3370, &b, false, 255, Font::Small, TEXTOVERLAY_SHADOW);
        tx!(self, self.options.sound_instr_text[0], |o| o.draw = 0);
        tx!(self, self.options.sound_instr_text[1], |o| o.draw = 0);

        for i in 0..4 {
            self.options.warn_text[i] = self.textover.create_and_add_text_overlay(2048, (800 + i * 200) as i16, "", true, 0, Font::Small, TEXTOVERLAY_SHADOW);
            tx!(self, self.options.warn_text[i], |o| o.draw = 0);
        }

        for i in 0..NUM_EXTRAS {
            let y = ((2048 - (NUM_EXTRAS as i32 / 2) * E_HEIGHT) + (i as i32 * E_HEIGHT)) as i16;
            let s = if i == 0 {
                self.options.diff_str = self.diff_text();
                self.options.diff_str.clone()
            } else if self.options.extras_toggle[i] != 0 {
                self.options.on_off_str[i] = self.on_off_text(i);
                self.options.on_off_str[i].clone()
            } else {
                self.gamestring(i as i32 + STR_EXTRAS_1)
            };
            self.options.extras[i] = self.textover.create_and_add_text_overlay(2048, y, &s, true, 255, Font::Small, TEXTOVERLAY_SHADOW);
            tx!(self, self.options.extras[i], |o| o.draw = 0);
        }
        // Host extension (settings_menu): the Extras "Settings" entry (placed each frame).
        self.options.settings_text = None;
        if self.game_options().settings_menu {
            let s = self.settings_label();
            self.options.settings_text = self.textover.create_and_add_text_overlay(2048, 2048, &s, true, 255, Font::Small, TEXTOVERLAY_SHADOW);
            tx!(self, self.options.settings_text, |o| o.draw = 0);
        }

        for i in 0..4 {
            self.options.player_num[i] = self.textover.create_and_add_text_overlay(
                (BACK_CHARS_X[i] + 200) as i16,
                (BACK_CHARS_Y[i] + 100) as i16,
                NUMBER_TEXT[i],
                false,
                255,
                Font::Normal,
                0,
            );
            tx!(self, self.options.player_num[i], |o| o.draw = 0);
        }

        // PC: find the portrait actors by position.
        for i in 0..NUM_MULTICHARS {
            for a in self.actor2.act_list.ids(&self.actor2.actors) {
                let p = self.actor2.actors[a].actor.position;
                if p.vx == PORTRAIT_POS[i].vx && p.vy == PORTRAIT_POS[i].vy && p.vz == PORTRAIT_POS[i].vz {
                    self.options.portrait_actor[i] = Some(a);
                    if self.frogger.frog_pool_active[i] == 0 {
                        self.actor2.actors[a].override_tex = Some("multihidden.bmp".into());
                    }
                }
            }
        }
    }

    /// Port of `InitOptionsMenu` (Common/options.c:1520).
    pub fn init_options_menu(&mut self) {
        // Default to sound setup, and put us in the global menu (Sound, Extra and Controller choice!)
        if self.options.init == 0 {
            self.options.current_player = 0;
            self.options.num_players = 2;
            self.options.selection = OP_SOUND;
            self.options.sound_vol = DEFAULT_VOLUME;
            self.options.music_vol = DEFAULT_VOLUME;
            self.options.init = 1;
            self.options.extra_selection = 0;
            self.options.sound_selection = 0;

            let t = &mut self.options.extras_toggle;
            t[EXTRA_VIEW_FMV] = 0;
            t[EXTRA_CREDITS] = 0;
            t[EXTRA_RESET_BEST_TIMES] = 0;
            t[EXTRA_RESET_STORY_MODE] = 0;
            t[EXTRA_FEEDING_FRENZY] = 1;
            t[EXTRA_VIEW_ART] = 0;
            t[EXTRA_MINI] = 1;
            self.create_options_objects();
            self.options.game_selected = 0;
        } else {
            self.create_options_objects();
            tx!(self, self.options.num_p_text, |o| o.draw = 0);
            tx!(self, self.options.num_p_text2, |o| o.draw = 0);
            tx!(self, self.options.mp_text, |o| o.draw = 0);
            self.options.game_selected = 0;
        }
        self.options.mode = -1;
    }

    /// Port of `DoneOptionsMenu` (Common/options.c:1560).
    pub fn done_options_menu(&mut self) {
        tx!(self, self.options.mp_text, |o| o.draw = 0);
        tx!(self, self.options.num_p_text, |o| o.draw = 0);
        tx!(self, self.options.num_p_text2, |o| o.draw = 0);
        if self.options.mode != OP_MULTIPLAYERNUMBER {
            let (s, t) = (self.cam.curr_cam_source, self.cam.curr_cam_target);
            sub_vector_fff(&mut self.cam.curr_cam_offset, &s, &t);
        }
        self.options.mode = -1;
    }

    /// Port of `OptionsProcessController` (Common/options.c:1584).
    pub fn options_process_controller(&mut self) {
        if self.cam.cam_control_mode != 0 || self.levplay.fading_out {
            return;
        }
        if self.options.mode == OP_ARCADE && (!self.check_cam_still() || self.textover.get(self.options.arcade_text).map_or(0, |o| o.a) < 200) {
            return;
        }
        if self.options.mode == OP_GLOBALMENU && !self.check_cam_still() {
            return;
        }

        let pl = if self.options.proper_multi_select && self.options.mode == OP_CHARSEL { (self.options.current_player as usize).min(3) } else { 0 };
        let button = self.levplay.pad_data.digital[pl];
        let last = self.options.options_last_button;
        let ctl = |g: &Game, c: usize| g.options.controls[c].map(|i| ALL_CONTROLS[i as usize]);
        let sound_repeat = self.options.mode == OP_SOUND && self.options.sound_selection != -1;

        let checks: [(u16, bool, usize); 6] = [
            (PAD_UP, last & PAD_UP == 0, C_UP),
            (PAD_RIGHT, last & PAD_RIGHT == 0 || sound_repeat, C_RIGHT),
            (PAD_DOWN, last & PAD_DOWN == 0, C_DOWN),
            (PAD_LEFT, last & PAD_LEFT == 0 || sound_repeat, C_LEFT),
            (PAD_CROSS, last & PAD_CROSS == 0, C_SELECT),
            (PAD_TRIANGLE, last & PAD_TRIANGLE == 0, C_BACK),
        ];
        for (bit, fresh, slot) in checks {
            if button & bit != 0 && fresh {
                if let Some(c) = ctl(self, slot) {
                    self.options_control(c);
                    self.options.options_last_button = button;
                    return;
                }
            }
        }
        self.options.options_last_button = button;
    }

    /// Port of `LookatWaterfall` (Common/options.c:1651).
    pub fn lookat_waterfall(&mut self) {
        self.cam.cam_source = FVector { vx: -8196622, vy: 4762946, vz: -8277231 };
        self.cam.cam_target = FVector { vx: -8877516, vy: 4456028, vz: -9244387 };
    }

    fn slide(&mut self, which_target: bool, dest: &FVector) -> i32 {
        let gs = self.game_speed;
        let v = if which_target { &mut self.cam.cam_target } else { &mut self.cam.cam_source };
        slide_vector_to_vector_ff(v, dest, MPCAMSPEED, gs) as i32
    }

    /// Port of `RunOptionsMenu` (Common/options.c:1698).
    pub fn run_options_menu(&mut self) {
        self.frontend.check_menu_keys = 1;
        let s = self.options.num_p_str.clone();
        self.set_text(self.options.num_p_text, &s);
        let s = self.options.num_p_str2.clone();
        self.set_text(self.options.num_p_text2, &s);

        let o = self.options.clone_ids();
        tx!(self, o.num_p_text, |x| {
            x.y_pos = 2048;
            x.font = Font::Normal;
        });
        tx!(self, o.num_p_text2, |x| x.font = Font::Normal);
        tx!(self, o.sfx_text[0], |x| x.draw = 0);
        tx!(self, o.sfx_text[1], |x| x.draw = 0);
        trgb!(self, o.num_p_text, 255, 255, 255);
        trgb!(self, o.num_p_text2, 255, 255, 255);

        self.options.cam_still = 0;

        self.options.num_extras_available = NUM_BASIC_EXTRAS as i32;
        for i in 2..NUM_EXTRAS {
            if self.options.extras_available[i] != 0 {
                self.options.num_extras_available += 1;
            }
        }

        // Sort out the title
        self.options.title_str = self.gamestring(self.options.selection + STR_OPTIONS_1);
        self.options.sub_title_str.clear();

        // Setup appropriate controls
        let row = CONTROLS_LIST.get(self.options.mode.max(0) as usize).copied().unwrap_or([None; C_TOTAL]);
        for i in 0..C_TOTAL {
            self.options.controls[i] = row[i].map(control_ser);
        }

        if self.options.mode != OP_BOOK {
            self.cam.fixed_pos = 1;
            self.cam.fixed_dir = 1;
        }

        tx!(self, o.mp_text, |x| x.draw = 0);
        if self.options.mode != OP_EXTRA {
            tx!(self, o.yes_no_text[0], |x| x.draw = 0);
            tx!(self, o.yes_no_text[1], |x| x.draw = 0);
        }
        tx!(self, o.sound_instr_text[0], |x| x.draw = 0);
        tx!(self, o.sound_instr_text[1], |x| x.draw = 0);

        if self.options.confirm_mode == 0 {
            for i in 0..4 {
                self.t_dec_alpha(o.warn_text[i]);
            }
        }

        let afc = self.act_frame_count as i32;
        let (pr, pg) = (pulse_r(afc), pulse_g(afc));
        match self.options.mode {
            OP_SOUND => {
                tx!(self, o.sound_instr_text[0], |x| x.draw = 1);
                tx!(self, o.sound_instr_text[1], |x| x.draw = 1);
                tx!(self, o.sfx_text[0], |x| x.draw = 1);
                tx!(self, o.sfx_text[1], |x| x.draw = 1);
                for i in 0..2 {
                    let c = self.overlays.get(o.sound_bar[i]).map_or((0, 0, 0), |b| (b.r, b.g, b.b));
                    trgb!(self, o.sfx_text[i], c.0, c.1, c.2);
                }
                self.t_dec_alpha(o.select_text);
                if self.options.sound_selection > 0 {
                    let sel = self.options.sound_selection.clamp(0, 1) as usize;
                    let ang = fmul(rsin((afc - self.options.sound_swap_time).wrapping_mul(SOUNDWOBBLE)), SOUNDWOBBLEFAC);
                    sx!(self, o.sound_icons[sel], |x| x.angle = ang);
                    sx!(self, o.sound_icons[1 - sel], |x| {
                        x.angle *= 4;
                        x.angle /= 5;
                    });
                }
                self.options.sub_title_str = self.gamestring(STR_TRIANGLE_BACK);
                self.t_inc_alpha(o.title, 255);
                self.s_inc_alpha(o.title_bak, 128);
                self.t_inc_alpha(o.sub_title, 255);
                self.s_inc_alpha(o.status_bak, 128);
                self.lookat_waterfall();
                self.update_sound_options();
                for i in 0..2 {
                    self.s_inc_alpha(o.sound_icons[i], 255);
                    self.s_inc_alpha(o.sound_bar[i], 255);
                    self.s_inc_alpha(o.sound_bak[i], 128);
                }
            }
            OP_CONTROL => {}
            OP_EXTRA => {
                self.t_inc_alpha(o.select_text, 0xff);
                if self.options.chest_timer.time != 0 {
                    let old_time = self.options.chest_timer.time;
                    let afcu = self.act_frame_count;
                    self.options.chest_timer.update2(-1, afcu);
                    if self.options.chest_timer.time == 2 && old_time == 3 {
                        let up = self.frogmove.curr_tile[0].map(|t| self.map.tile(t).normal).unwrap_or_default();
                        let up = FVector { vx: up.vx as i32, vy: up.vy as i32, vz: up.vz as i32 };
                        self.play_sample(gen_sfx(GEN_COLLECT_COIN), None, 0, SAMPLE_VOLUME, -1);
                        for _ in 0..5 {
                            let vx = -4371 + self.rng.random(101) - 50;
                            let vz = -1777 + self.rng.random(101) - 50;
                            let v2 = SVector { vx: vx as i16, vy: 321, vz: vz as i16, pad: 0 };
                            let fx = self.create_special_effect_direct(FXTYPE_SPARKLYTRAIL, &v2, &up, 16384 * 12, 16384 / 2, 0, 16384);
                            if fx.is_some() {
                                // MSVC evaluates the arguments right to left.
                                let b = self.rng.random(256) as u8;
                                let g = self.rng.random(256) as u8;
                                let r = self.rng.random(256) as u8;
                                self.set_fx_colour(fx, r, g, b);
                            }
                        }
                    }
                    self.sync_options_texts();
                    return;
                }
                self.options.sub_title_str = self.gamestring(STR_TRIANGLE_BACK);
                self.lookat_waterfall();
                if self.options.credits_running != 0 {
                    self.do_credits();
                    self.t_dec_alpha(o.sub_title);
                    self.s_dec_alpha(o.status_bak);
                    self.t_dec_alpha(o.select_text);
                } else {
                    self.t_inc_alpha(o.title, 255);
                    self.s_inc_alpha(o.title_bak, 128);
                    self.t_inc_alpha(o.sub_title, 255);
                    self.s_inc_alpha(o.status_bak, 128);
                    self.t_inc_alpha(o.title, 255);
                    let nea = (self.options.num_extras_available.max(0) as usize).min(NUM_EXTRAS);
                    if self.options.confirm_mode != 0 {
                        let warn = self.gamestring(STR_WARNING_1 + self.options.confirm_mode - 3);
                        self.options.warn_str = self.textover.font_fit_to_width(Font::Small, 450, &warn);
                        for i in 0..4 {
                            let s = self.options.warn_str.get(i).cloned().unwrap_or_default();
                            self.set_text(o.warn_text[i], &s);
                            self.t_inc_alpha(o.warn_text[i], 255);
                        }
                        for i in 0..nea {
                            tx!(self, o.extras[i], |x| x.draw = 1);
                            if i as i32 == self.options.confirm_mode {
                                self.t_inc_alpha(o.extras[i], 255);
                            } else {
                                self.t_dec_alpha(o.extras[i]);
                            }
                        }
                        if self.extras_settings_on() {
                            self.t_dec_alpha(self.options.settings_text);
                        }
                        for i in 0..2 {
                            if i as i32 == self.options.extra_selection {
                                self.t_inc_alpha(o.yes_no_text[i], 255);
                                trgb!(self, o.yes_no_text[i], pr, pg, 10);
                            } else {
                                trgb!(self, o.yes_no_text[i], 255, 255, 255);
                                if self.textover.get(o.yes_no_text[i]).map_or(0, |x| x.a) <= 150 {
                                    self.t_inc_alpha(o.yes_no_text[i], 150);
                                } else {
                                    self.t_dec_alpha(o.yes_no_text[i]);
                                }
                            }
                        }
                    } else {
                        for i in 0..2 {
                            self.t_dec_alpha(o.yes_no_text[i]);
                        }
                        for i in 0..nea {
                            tx!(self, o.extras[i], |x| x.draw = 1);
                            self.t_inc_alpha(o.extras[i], 255);
                            if i as i32 == self.options.extra_selection {
                                trgb!(self, o.extras[i], pr, pg, 10);
                            } else {
                                trgb!(self, o.extras[i], 255, 255, 255);
                            }
                        }
                        if self.extras_settings_on() {
                            // Host extension: the slot after the last available extra.
                            let st = self.options.settings_text;
                            let y = ((2048 - (NUM_EXTRAS as i32 / 2) * E_HEIGHT) + (nea as i32 * E_HEIGHT)) as i16;
                            tx!(self, st, |x| {
                                x.draw = 1;
                                x.y_pos = y;
                                x.y_pos_to = y;
                            });
                            self.t_inc_alpha(st, 255);
                            if nea as i32 == self.options.extra_selection {
                                trgb!(self, st, pr, pg, 10);
                            } else {
                                trgb!(self, st, 255, 255, 255);
                            }
                        }
                    }
                }
            }
            OP_GLOBALMENU => {
                let s = self.gamestring(STR_X_SELECT_OPTION);
                self.set_text(o.select_text, &s);
                self.t_inc_alpha(o.select_text, 0xff);
                self.t_inc_alpha(o.title, 255);
                self.s_inc_alpha(o.title_bak, 128);
                self.t_dec_alpha(o.sub_title);
                self.s_dec_alpha(o.status_bak);
                tx!(self, o.num_p_text, |x| x.draw = 0);
                tx!(self, o.num_p_text2, |x| x.draw = 0);
                // Some random value ;)
                self.cam.cam_source = FVector { vx: -7641900, vy: 6178404, vz: -3401826 };
                // Look at the options objects
                self.cam.cam_target = OPT_TARGETS[(self.options.selection.max(0) as usize).min(NUM_OPTIONS - 1)];
                let gs = self.game_speed;
                let (ct, cs) = (self.cam.cam_target, self.cam.cam_source);
                slide_vector_to_vector_ff(&mut self.cam.curr_cam_target, &ct, 4096 * 2, gs);
                slide_vector_to_vector_ff(&mut self.cam.curr_cam_source, &cs, 4096 * 2, gs);
                self.cam.cam_speed3 = 4096 * 10;
            }
            OP_MULTIPLAYERNUMBER => {
                // PC: maxPlayers = 4 (the PSX counts pads).
                self.options.max_players = 4;
                self.options.title_str = self.gamestring(STR_MULTIPLAYER);
                self.options.mp_str = self.gamestring(STR_SELECT_NUM_PLAYERS);
                self.options.sub_title_str = self.gamestring(STR_TRIANGLE_BACK);
                if self.options.max_players <= 1 {
                    tx!(self, o.num_p_text, |x| x.font = Font::Small);
                    tx!(self, o.num_p_text2, |x| x.font = Font::Small);
                    self.options.num_p_str = self.gamestring(STR_NEED_PADS1);
                    self.options.num_p_str2 = self.gamestring(STR_NEED_PADS2);
                } else {
                    self.options.num_players = self.options.max_players.min(self.options.num_players.max(2));
                    self.options.num_p_str = format!("{} {}", self.options.num_players, self.gamestring(STR_PLAYERS));
                    self.options.num_p_str2.clear();
                }
                let (a, b) = (self.options.num_p_str.clone(), self.options.num_p_str2.clone());
                self.set_text(o.num_p_text, &a);
                self.set_text(o.num_p_text2, &b);

                let mut cam_still = self.slide(false, &fv(-9890938, 9046405, 21231148));
                self.cam.curr_cam_source = self.cam.cam_source;
                cam_still += self.slide(true, &fv(-9890938 + 7617 * 100, 9046405, 21231148 + 41020 * 100));
                self.cam.cam_target.vy = self.cam.cam_source.vy;
                self.cam.curr_cam_target = self.cam.cam_target;
                self.options.cam_still = (cam_still == 2) as i32;

                let cs = self.options.cam_still as u8;
                tx!(self, o.num_p_text, |x| x.draw = cs);
                tx!(self, o.num_p_text2, |x| x.draw = cs);
                self.t_inc_alpha(o.title, 255);
                self.s_inc_alpha(o.title_bak, 128);
                self.t_inc_alpha(o.sub_title, 255);
                self.s_inc_alpha(o.status_bak, 128);
                tx!(self, o.mp_text, |x| x.draw = 1);
                let y = if self.options.max_players <= 1 { 1948 } else { 2048 };
                tx!(self, o.num_p_text, |x| x.y_pos = y);
            }
            OP_LEVELSEL => {
                if self.options.game_selected != 0 && !self.levplay.fading_out {
                    self.mp_start();
                    return;
                }
                self.options.title_str = self.gamestring(STR_MULTIPLAYER);
                self.options.mp_str = self.gamestring(STR_SELECT_LEVEL);
                self.options.sub_title_str = self.gamestring(STR_TRIANGLE_BACK);

                let ls = (self.options.level_selection.max(0) as usize).min(NUM_MULTI_LEVELS - 1);
                let desc = self
                    .gamestring(self.layout.world_visual_data[MULTI_WORLD_NUM[ls] as usize].level_visual_data[MULTI_LEVEL_NUM[ls] as usize].description_str);
                let lines = self.textover.string_wrap(&desc, 1800, 2, Font::Normal);
                self.options.mp_lev_name_array = [lines.first().cloned().unwrap_or_default(), lines.get(1).cloned().unwrap_or_default()];
                let [a, b] = self.options.mp_lev_name_array.clone();
                self.set_text(o.num_p_text, &a);
                self.set_text(o.num_p_text2, &b);
                if b.is_empty() {
                    tx!(self, o.num_p_text, |x| x.y_pos = 2048);
                } else {
                    tx!(self, o.num_p_text, |x| x.y_pos = 2048 - 120);
                    tx!(self, o.num_p_text2, |x| x.y_pos = 2048 + 120);
                }
                let (r, g) = (LEV_NAME_COL_R[ls], LEV_NAME_COL_G[ls]);
                trgb!(self, o.num_p_text, r, g, 0);
                trgb!(self, o.num_p_text2, r, g, 0);

                let mut cam_still = self.slide(false, &fv(-9790938, -500000, 21231148));
                self.cam.curr_cam_source = self.cam.cam_source;
                cam_still += self.slide(true, &fv(-9790938 + 7617 * 100, -500000, 21231148 + 41020 * 100));
                self.cam.cam_target.vy = self.cam.cam_source.vy;
                self.cam.curr_cam_target = self.cam.cam_target;
                self.options.cam_still = (cam_still == 2) as i32;

                self.t_inc_alpha(o.title, 255);
                self.s_inc_alpha(o.title_bak, 128);
                self.t_inc_alpha(o.sub_title, 255);
                self.s_inc_alpha(o.status_bak, 128);
                tx!(self, o.mp_text, |x| x.draw = 1);
                let cs = self.options.cam_still as u8;
                tx!(self, o.num_p_text, |x| x.draw = cs);
                tx!(self, o.num_p_text2, |x| x.draw = cs);
            }
            OP_CHARSEL => {
                let cp = self.options.current_player;
                for i in 0..4 {
                    if i == cp {
                        trgb!(self, o.player_num[i as usize], pr, pg, 10);
                    } else {
                        trgb!(self, o.player_num[i as usize], 255, 255, 255);
                    }
                }
                self.options.title_str = self.gamestring(STR_MULTIPLAYER);
                let text_buff = format!("@X - {} {} {}", self.gamestring(STR_PLAYER), cp + 1, self.gamestring(STR_SELECT_CHAR));
                self.options.sub_title_str = self.gamestring(STR_TRIANGLE_BACK);
                self.options.mp_str = text_buff;
                let ms = (self.options.multi_selection.max(0) as usize) % NUM_MULTICHARS;
                self.options.num_p_str = if self.frogger.frog_pool_active[ms] != 0 { self.gamestring(STR_CHAR_NAME_1 + ms as i32) } else { "???".into() };
                let s = self.options.num_p_str.clone();
                self.set_text(o.num_p_text, &s);

                let mut cam_still = self.slide(false, &MP_CHAR_SEL_CAM_SOURCE[ms]);
                cam_still += self.slide(true, &MP_CHAR_SEL_CAM_TARGET[ms]);
                self.cam.cam_target.vy = self.cam.cam_source.vy;
                self.cam.curr_cam_source = self.cam.cam_source;
                self.cam.curr_cam_target = self.cam.cam_target;
                self.options.cam_still = (cam_still == 2) as i32;

                self.t_inc_alpha(o.title, 255);
                self.s_inc_alpha(o.title_bak, 128);
                self.t_inc_alpha(o.sub_title, 255);
                self.s_inc_alpha(o.status_bak, 128);
                tx!(self, o.mp_text, |x| x.draw = 1);
                let cs = self.options.cam_still as u8;
                tx!(self, o.num_p_text, |x| {
                    x.draw = cs;
                    x.y_pos = 3300;
                });
                tx!(self, o.num_p_text2, |x| x.draw = 0);
            }
            OP_BOOK => {
                self.options.title_str = self.gamestring(STR_STORYMODE);
                let s = self.gamestring(STR_X_SELECT_OPTION);
                self.set_text(o.select_text, &s);
                self.t_inc_alpha(o.title, 255);
                self.s_inc_alpha(o.title_bak, 128);
                self.t_inc_alpha(o.select_text, 255);
                if !self.levplay.fading_out && self.options.game_selected != 0 {
                    self.book_start();
                    return;
                }
                if let Some(page) = self.options.page {
                    if self.reached_end_of_anim(page) != 0 {
                        if self.options.page_dir != 0 {
                            if self.options.page_dir > 0 {
                                self.options.grab_to_texture = 3;
                            } else if self.options.page_dir < 0 {
                                self.options.grab_to_texture = 1;
                            }
                            self.options.page_draw_count = 2;
                            self.options.page_dir = 0;
                        }
                        self.actor2.actors[page].actor.visible = 0;
                    }
                } else {
                    let c = &self.cam;
                    if distance_between_points_ff(&c.curr_cam_source, &c.cam_source) <= 4096
                        && distance_between_points_ff(&c.curr_cam_target, &c.cam_target) <= 4096
                    {
                        self.s_inc_alpha(o.chapter_pic[0], 255);
                        self.s_dec_alpha(o.chapter_pic[1]);
                        sx!(self, o.chapter_pic[0], |x| x.draw = 1);
                        sx!(self, o.chapter_pic[1], |x| x.draw = 1);
                        self.t_inc_alpha(o.chapter_text[0], 255);
                        self.t_dec_alpha(o.chapter_text[1]);
                        self.t_inc_alpha(o.chapter_name_text[0][0], 255);
                        self.t_inc_alpha(o.chapter_name_text[0][1], 255);
                        self.t_dec_alpha(o.chapter_name_text[1][0]);
                        self.t_dec_alpha(o.chapter_name_text[1][1]);
                    }
                    if self.options.page_dir != 0 && self.textover.get(o.chapter_text[1]).map_or(0, |x| x.a) == 0 {
                        self.options.page_dir = 0;
                    }
                }
            }
            OP_ARCADE => {
                let s = self.gamestring(STR_SELECT_LEVEL);
                self.set_text(o.select_text, &s);
                self.options.sub_title_str = self.gamestring(STR_TRIANGLE_BACK);
                self.t_dec_alpha(o.title);
                tx!(self, o.num_p_text, |x| x.draw = 0);
                tx!(self, o.num_p_text2, |x| x.draw = 0);
                self.sync_options_texts();
                self.do_arcade_menu();
            }
            OP_SP_CHARSEL => {
                self.options.sub_title_str = self.gamestring(STR_TRIANGLE_BACK);
                if self.options.game_selected != 0 && !self.levplay.fading_out {
                    self.arcade_start();
                    return;
                }
                self.t_dec_alpha(o.title);
                self.t_inc_alpha(o.arcade_text, 255);
                self.t_inc_alpha(o.select_text, 255);
                self.s_inc_alpha(o.title_bak, 128);
                self.s_inc_alpha(o.world_bak, 128);
                tx!(self, o.num_p_text, |x| x.draw = 0);
                tx!(self, o.num_p_text2, |x| x.draw = 0);
                self.t_inc_alpha(o.sub_title, 255);
                self.s_inc_alpha(o.status_bak, 128);
                for i in 0..FROG_NUMFROGS {
                    if self.frogger.frog_pool_active[i] != 0 {
                        self.t_inc_alpha(o.level_text[i], 255);
                    }
                    if i as i32 == self.options.sp_char {
                        srgb!(self, o.multi_face[i], pr, pg, 10);
                        trgb!(self, o.level_text[i], pr, pg, 10);
                    } else {
                        srgb!(self, o.multi_face[i], 255, 255, 255);
                        trgb!(self, o.level_text[i], 255, 255, 255);
                    }
                }
            }
            OP_FMV => {
                if self.options.playing_fmv && !self.levplay.fading_out {
                    self.pause_audio();
                    self.effects.push(Effect::PauseAudio);
                    self.start_video_playback(self.options.fmv_num + 3);
                    self.un_pause_audio();
                    self.effects.push(Effect::UnPauseAudio);
                    self.options.playing_fmv = false;
                    self.prepare_song(worldid::FRONTEND as i16, true);
                    self.screen_fade(0, 255, 30);
                    self.fadeout.keep_fade = false;
                }
                self.options.sub_title_str = self.gamestring(STR_TRIANGLE_BACK);
                self.t_inc_alpha(o.title, 255);
                self.s_inc_alpha(o.title_bak, 128);
                self.t_inc_alpha(o.sub_title, 255);
                self.s_inc_alpha(o.status_bak, 128);
                tx!(self, o.num_p_text, |x| x.draw = 0);
                tx!(self, o.num_p_text2, |x| x.draw = 0);
                self.options.title_str = self.gamestring(STR_EXTRAS_2);
                for i in 0..(self.options.num_fmv_open.max(0) as usize).min(NUM_FMV_SEQUENCES) {
                    if i as i32 == self.options.fmv_num {
                        trgb!(self, o.fmv_text[i], pr, pg, 10);
                    } else {
                        trgb!(self, o.fmv_text[i], 200, 200, 200);
                    }
                }
            }
            _ => {}
        }
        self.sync_options_texts();

        self.slurp_cam_position();
        if self.options.going_to_art_viewer {
            if !self.levplay.fading_out {
                self.game_state.mode = ARTVIEWER_MODE;
                self.timing.p_frame_modifier = 0; // pFrameModifier = 0 (PC timing)
                self.options.art_timer.init(10);
                self.options.current_art = 0;
                self.options.going_to_art_viewer = false;
            }
        } else if !self.options.playing_fmv {
            self.options_process_controller();
        }
    }

    /// Copies the buffers `title`, `subTitle`, `mpText` (and `numPText` when it shows
    /// `numPStr`) point at into their overlays.
    fn sync_options_texts(&mut self) {
        let o = &self.options;
        let (t, st, mp) = (o.title_str.clone(), o.sub_title_str.clone(), o.mp_str.clone());
        let (a, b, c) = (o.title, o.sub_title, o.mp_text);
        self.set_text(a, &t);
        self.set_text(b, &st);
        self.set_text(c, &mp);
    }

    /// Port of `CheckCamStill` (Common/options.c:2308).
    pub fn check_cam_still(&self) -> bool {
        let c = &self.cam;
        let d1 = distance_between_points_ff(&c.cam_source, &c.curr_cam_source);
        if d1 < 4096 * 4 && d1 > -4096 * 4 {
            let d1 = distance_between_points_ff(&c.cam_target, &c.curr_cam_target);
            if d1 < 4096 * 3 && d1 > -4096 * 3 {
                return true;
            }
        }
        false
    }

    /// Port of `UpdateSoundOptions` (Common/options.c:2323).
    pub fn update_sound_options(&mut self) {
        let (m, s) = (self.savegame.global_music_vol, self.savegame.global_sound_vol);
        sx!(self, self.options.sound_bar[0], |o| o.width = (m * SOUND_SCALE) as i16);
        sx!(self, self.options.sound_bar[1], |o| o.width = (s * SOUND_SCALE) as i16);
        if self.options.sound_count > 0 {
            self.options.sound_count -= self.game_speed;
        }
    }

    fn book_turn(&mut self, dir: i32) {
        let o = self.options.clone_ids();
        let pn = self.options.page_num;
        if let Some(page) = o.page {
            self.options.grab_to_texture = if dir < 0 { 3 } else { 1 };
            self.actor_animate(page, if dir < 0 { 2 } else { 1 }, false, false, 99, false);
            if let Some(a) = self.actor2.actors.get_mut(page) {
                a.draw = 1;
            }
            return;
        }
        let a0 = self.overlays.get(o.chapter_pic[0]).map_or(0, |x| x.a);
        let tex0 = self.overlays.get(o.chapter_pic[0]).map_or(0, |x| x.tex);
        sx!(self, o.chapter_pic[1], |x| {
            x.draw = 1;
            x.a = a0;
            x.tex = tex0;
        });
        let t = crate::specfx::find_texture(CHAPTER_PIC[(pn.max(0) as usize).min(BOOK_NUM_PAGES - 1)]);
        sx!(self, o.chapter_pic[0], |x| {
            x.a = 0;
            x.tex = t;
        });

        self.options.chapter_str[0] = c_sprintf(&self.gamestring(STR_CHAPTER), &[FmtArg::Int(pn + 1)]);
        let ta0 = self.textover.get(o.chapter_text[0]).map_or(0, |x| x.a);
        tx!(self, o.chapter_text[1], |x| {
            x.draw = 1;
            x.a = ta0;
        });
        tx!(self, o.chapter_text[0], |x| x.a = 0);
        // BookLeft shows the page after (pageNum + 2), BookRight the one before (pageNum).
        self.options.chapter_str[1] = c_sprintf(&self.gamestring(STR_CHAPTER), &[FmtArg::Int(if dir < 0 { pn + 2 } else { pn })]);
        let (c0, c1) = (self.options.chapter_str[0].clone(), self.options.chapter_str[1].clone());
        self.set_text(o.chapter_text[0], &c0);
        self.set_text(o.chapter_text[1], &c1);

        let (a, b) = (self.gamestring(STR_CHAPTER_1a + pn * 2), self.gamestring(STR_CHAPTER_1a + pn * 2 + 1));
        self.set_text(o.chapter_name_text[0][0], &a);
        self.set_text(o.chapter_name_text[0][1], &b);
        let na = self.textover.get(o.chapter_name_text[0][0]).map_or(0, |x| x.a);
        tx!(self, o.chapter_name_text[1][0], |x| {
            x.draw = 1;
            x.a = na;
        });
        tx!(self, o.chapter_name_text[1][1], |x| {
            x.draw = 1;
            x.a = na;
        });
        tx!(self, o.chapter_name_text[0][0], |x| x.a = 0);
        tx!(self, o.chapter_name_text[0][1], |x| x.a = 0);
        let other = if dir < 0 { pn + 1 } else { pn - 1 };
        let (a, b) = (self.gamestring(STR_CHAPTER_1a + other * 2), self.gamestring(STR_CHAPTER_1a + other * 2 + 1));
        self.set_text(o.chapter_name_text[1][0], &a);
        self.set_text(o.chapter_name_text[1][1], &b);
    }

    /// Port of `BookLeft` (Common/options.c:2464).
    pub fn book_left(&mut self) {
        if self.options.page_num > 0 && self.options.page_dir == 0 {
            self.named_sfx("page2");
            self.options.page_dir = -1;
            self.options.g_page_dir = -1;
            self.options.page_num -= 1;
            self.book_turn(-1);
        }
    }

    /// Port of `BookRight` (Common/options.c:2532).
    pub fn book_right(&mut self) {
        if self.options.page_num < self.options.max_page_allowed && self.options.page_dir == 0 {
            self.named_sfx("page3");
            self.options.page_dir = 1;
            self.options.g_page_dir = 1;
            self.options.page_num += 1;
            self.book_turn(1);
        }
    }

    /// Port of `BookSelect` (Common/options.c:2607).
    pub fn book_select(&mut self) {
        self.screen_fade(255, 0, 30);
        self.fadeout.keep_fade = true;
        self.options.game_selected = 1;
        self.super_hop_sfx();
        self.frogger.player[0].lives = NUM_LIVES[(self.game_state.difficulty as usize).min(2)];
    }

    /// Port of `BookStart` (Common/options.c:2616).
    pub fn book_start(&mut self) {
        self.frogger.num_frogs = 1;
        self.game_state.multi = SINGLEPLAYER;
        self.game_state.single = STORY_MODE;

        let pn = (self.options.page_num.max(0) as usize).min(BOOK_NUM_PAGES - 1);
        let mut ssl = PAGE_TO_STORY_LEVEL[pn] as usize;
        if self.options.page_num == self.options.max_page_allowed && STORY_SEQUENCE[ssl].another != 0 {
            let s = STORY_SEQUENCE[ssl];
            if self.layout.world_visual_data[s.world_num as usize].level_visual_data[s.level_num as usize].level_completed != 0 {
                ssl += 1;
            }
        }
        self.game_state.story_sequence_level = ssl as _;

        let s = STORY_SEQUENCE[ssl];
        self.frogger.player[0].world_num = s.world_num as u8;
        self.frogger.player[0].level_num = s.level_num as u8;
        self.last_act_frame_count = self.act_frame_count;
        self.game_state.mode = INGAME_MODE;
        self.init_level(s.world_num, s.level_num);
    }

    /// Port of `BookDown` (Common/options.c:2645): hop back off the book.
    pub fn book_down(&mut self) {
        let p = &mut self.frogger.player[0];
        p.input_pause = INPUT_POLLPAUSE;
        p.can_jump = 0;
        p.frog_state |= FROGSTATUS_ISWANTINGSUPERHOPD;
        self.frogmove.frog_facing[0] = (self.cam.cam_facing[0] + MOVE_DOWN) & 3;
        let p = &mut self.frogger.player[0];
        p.is_super_hopping = 1;
        p.has_jumped = 1;
        self.cam.fixed_dir = 0;
        self.cam.fixed_pos = 0;
        self.frogger.player[0].idle_time = MAX_IDLE_TIME;
        self.frogger.player[0].extended_hop_dir = MOVE_DOWN as u8;
        self.done_options_menu();
    }

    fn arcade_level_changed(&mut self, changed: bool) {
        if changed {
            self.options.static_flash = 5;
            self.named_sfx("espark1");
        }
    }

    /// Port of `ArcadeUp` (Common/options.c:2665).
    pub fn arcade_up(&mut self) {
        let old = self.options.level_num;
        let cw = self.menus.c_world as usize;
        if cw as u32 == worldid::GARDEN {
            self.options.level_num = 1 - self.options.level_num;
        } else {
            let n = self.layout.world_visual_data[cw].num_single_player_levels as i32;
            if n > 0 {
                loop {
                    self.options.level_num = (self.options.level_num + n - 1) % n;
                    if self.layout.world_visual_data[cw].level_visual_data[self.options.level_num as usize].level_open != LEVEL_CLOSED
                        || self.options.level_num == old
                    {
                        break;
                    }
                }
            }
        }
        self.arcade_level_changed(self.options.level_num != old);
    }

    /// Port of `ArcadeDown` (Common/options.c:2693).
    pub fn arcade_down(&mut self) {
        let old = self.options.level_num;
        let cw = self.menus.c_world as usize;
        if cw as u32 == worldid::GARDEN {
            self.options.level_num = 1 - self.options.level_num;
        } else {
            let n = self.layout.world_visual_data[cw].num_single_player_levels as i32;
            if n > 0 {
                loop {
                    self.options.level_num = (self.options.level_num + 1) % n;
                    if self.layout.world_visual_data[cw].level_visual_data[self.options.level_num as usize].level_open != LEVEL_CLOSED
                        || self.options.level_num == old
                    {
                        break;
                    }
                }
            }
        }
        self.arcade_level_changed(self.options.level_num != old);
    }

    fn arcade_world_step(&mut self, dir: i32) {
        let old_world = self.menus.c_world;
        let n = self.options.num_arcade_worlds.max(1) as u32;
        for _ in 0..n {
            self.menus.c_world = if dir < 0 { (self.menus.c_world + n - 1) % n } else { (self.menus.c_world + 1) % n };
            let w = &self.layout.world_visual_data[self.menus.c_world as usize];
            if !(w.world_open == WORLD_CLOSED || w.num_single_player_levels == 0) {
                break;
            }
        }
        let cw = self.menus.c_world as usize;
        if cw as u32 == worldid::GARDEN {
            if self.options.level_num > 1 {
                self.options.level_num = 1;
            }
        } else {
            let n = self.layout.world_visual_data[cw].num_single_player_levels as i32;
            if self.options.level_num > n - 1 {
                self.options.level_num = n - 1;
            }
            for _ in 0..n.max(0) {
                if self.layout.world_visual_data[cw].level_visual_data[self.options.level_num.max(0) as usize].level_open != LEVEL_CLOSED {
                    break;
                }
                self.options.level_num = (self.options.level_num + 1) % n;
            }
        }
        self.arcade_level_changed(self.menus.c_world != old_world);
    }

    /// Port of `ArcadeLeft` (Common/options.c:2719).
    pub fn arcade_left(&mut self) {
        self.arcade_world_step(-1);
    }

    /// Port of `ArcadeRight` (Common/options.c:2754).
    pub fn arcade_right(&mut self) {
        self.arcade_world_step(1);
    }

    /// Port of `SPCharSelect` (Common/options.c:2790).
    pub fn sp_char_select(&mut self) {
        self.super_hop_sfx();
        self.screen_fade(128, 0, 30);
        self.fadeout.keep_fade = true;
        self.options.game_selected = 1;
        self.menus.last_arcade = TILENUM_ARCADE;
        self.camera_set_offset();
        self.store_frontend_camera();
    }

    /// Port of `ArcadeSelect` (Common/options.c:2810): from the level list to character select.
    pub fn arcade_select(&mut self) {
        self.super_hop_sfx();
        self.options.mode = OP_SP_CHARSEL;
        let o = self.options.clone_ids();
        for t in o.par_text {
            tx!(self, t, |x| x.draw = 0);
        }
        tx!(self, o.world_text, |x| x.draw = 0);

        sx!(self, o.world_bak, |x| {
            x.x_pos = 900;
            x.x_pos_to = 900;
            x.y_pos_to = x.y_pos_to.wrapping_add(250);
            x.y_pos = x.y_pos_to;
            x.width = 4096 - 2 * x.x_pos;
        });
        let s = self.gamestring(STR_X_SELECT_CHAR);
        self.set_text(o.select_text, &s);

        for i in 0..MAX_LEVELSTRING {
            sx!(self, o.beaten_icon[i], |x| x.draw = 0);
            sx!(self, o.level_coin_medal[i], |x| x.draw = 0);
            for t in [o.level_coin_text[i], o.level_par_text[i], o.level_set_by_text[i], o.level_text[i]] {
                tx!(self, t, |x| x.draw = 0);
            }
        }
        let afc = self.act_frame_count as i32;
        for i in 0..FROG_NUMFROGS {
            if self.frogger.frog_pool_active[i] == 0 {
                continue;
            }
            self.menus.level_str[i] = self.gamestring(STR_CHAR_NAME_1 + i as i32);
            let s = self.menus.level_str[i].clone();
            self.set_text(o.level_text[i], &s);
            tx!(self, o.level_text[i], |x| x.draw = 1);
            sx!(self, o.world_bak, |x| {
                x.height = (i * 300 + 600) as i16;
                x.y_pos = 850;
                x.y_pos_to = 850;
            });
            let y = (i * 300 + 1000 + 50) as i16;
            tx!(self, o.level_text[i], |x| {
                x.y_pos = y;
                x.y_pos_to = y;
                x.x_pos = 1600;
                x.x_pos_to = 1600;
            });
            let (fx, fy) = ((1000 + (i % 2) * 1500) as i16, (i * 300 + 1000 - 128) as i16);
            sx!(self, o.multi_face[i], |x| {
                x.x_pos = fx;
                x.x_pos_to = fx;
                x.y_pos = fy;
                x.y_pos_to = fy;
                x.num = 1;
                x.draw = 1;
            });
            if i as i32 == self.options.sp_char {
                trgb!(self, o.level_text[i], pulse_r(afc), pulse_g(afc), 10);
            } else {
                trgb!(self, o.level_text[i], 255, 255, 255);
            }
        }
    }

    fn sp_char_step(&mut self, dir: i32) {
        let n = FROG_NUMFROGS as i32;
        for _ in 0..n {
            self.options.sp_char = (self.options.sp_char + if dir < 0 { n - 1 } else { 1 }) % n;
            if self.frogger.frog_pool_active[self.options.sp_char as usize] != 0 {
                break;
            }
        }
        self.hop_sfx();
    }

    /// Port of `SPCharUp` (Common/options.c:2862).
    pub fn sp_char_up(&mut self) {
        self.sp_char_step(-1);
    }

    /// Port of `SPCharDown` (Common/options.c:2871).
    pub fn sp_char_down(&mut self) {
        self.sp_char_step(1);
    }

    /// Port of `SPCharBack` (Common/options.c:2880): back to the level list.
    pub fn sp_char_back(&mut self) {
        let o = self.options.clone_ids();
        for i in 0..FROG_NUMFROGS {
            sx!(self, o.multi_face[i], |x| x.draw = 0);
        }
        self.options.mode = OP_ARCADE;
        for t in o.par_text {
            tx!(self, t, |x| x.draw = 1);
        }
        tx!(self, o.world_text, |x| x.draw = 1);
        sx!(self, o.world_bak, |x| {
            x.x_pos = 150;
            x.x_pos_to = 150;
            x.y_pos = 938;
            x.y_pos_to = 938;
            x.width = 4096 - 2 * x.x_pos;
        });
        let s = self.gamestring(STR_SELECT_LEVEL);
        self.set_text(o.select_text, &s);
        for i in 0..MAX_LEVELSTRING {
            self.menus.level_str[i].clear();
            self.set_text(o.level_text[i], "");
            for t in [o.level_coin_text[i], o.level_par_text[i], o.level_set_by_text[i], o.level_text[i]] {
                tx!(self, t, |x| x.draw = 1);
            }
            let y = (1445 + i * 170) as i16;
            tx!(self, o.level_text[i], |x| {
                x.x_pos = 306;
                x.x_pos_to = 306;
                x.y_pos = y;
                x.y_pos_to = y;
            });
        }
        self.do_arcade_menu();
        self.hop_sfx();
    }

    /// Port of `ArcadeStart` (Common/options.c:2910).
    pub fn arcade_start(&mut self) {
        self.frogger.num_frogs = 1;
        self.game_state.multi = SINGLEPLAYER;
        self.game_state.single = ARCADE_MODE;
        self.last_act_frame_count = self.act_frame_count;
        self.game_state.mode = INGAME_MODE;

        let cw = self.menus.c_world;
        // NB: THis is a heowge hack to make race mode work for internet test levels
        if cw == worldid::TEST {
            self.frogger.num_frogs = 2;
            self.frogger.player[0].world_num = worldid::TEST as u8;
            self.frogger.player[0].level_num = self.options.level_num as u8;
            self.frogger.player[0].character = FROG_FROGGER;
            self.frogger.player[1].character = FROG_LILLIE;
            self.game_state.mode = INGAME_MODE;
            self.game_state.single = INVALID_MODE;
            self.game_state.multi = MULTILOCAL;
            self.multi.multiplayer_mode = MULTIMODE_RACE;
            let (w, l) = (worldid::TEST, self.options.level_num as u32);
            self.init_level(w, l);
            self.frogger.fog.max = 25000;
            return;
        } else if cw == worldid::GARDEN {
            if self.options.level_num == 0 {
                self.frogger.player[0].world_num = worldid::FRONTEND as u8;
                self.frogger.player[0].level_num = LEVELID_FRONTEND4 as u8;
            } else {
                self.frogger.player[0].world_num = worldid::GARDEN as u8;
                self.frogger.player[0].level_num = 0;
            }
        } else {
            self.frogger.player[0].world_num = cw as u8;
            self.frogger.player[0].level_num = self.options.level_num as u8;
        }
        self.frogger.player[0].character = self.options.sp_char as u8;
        let (w, l) = (self.frogger.player[0].world_num as u32, self.frogger.player[0].level_num as u32);
        self.init_level(w, l);
    }

    /// Port of `ArcadeBack` (Common/options.c:2967): hop off the arcade machine.
    pub fn arcade_back(&mut self) {
        let p = &mut self.frogger.player[0];
        p.input_pause = INPUT_POLLPAUSE;
        p.can_jump = 0;
        p.frog_state |= FROGSTATUS_ISWANTINGSUPERHOPD;
        p.is_super_hopping = 1;
        p.has_jumped = 1;
        self.cam.fixed_dir = 0;
        self.cam.fixed_pos = 0;
        self.frogger.player[0].idle_time = MAX_IDLE_TIME;
        self.frogger.player[0].extended_hop_dir = MOVE_DOWN as u8;
        self.screen_fade(210, 255, 30);
        self.fadeout.fade_text = false;
        self.fadeout.keep_fade = false;
        self.done_options_menu();
    }

    /// Port of `StartCredits` (Common/options.c:2992).
    pub fn start_credits(&mut self) {
        let mut spacing = 0i32;
        let mut cred_font = Font::Normal;
        self.options.credits_y = 0;
        self.options.credits_running = 1;
        self.options.ending_credits = 0;

        let mut j = 0usize;
        while !credits_end(self, j) {
            let s = self.gamestring(STR_CREDITS_1 + j as i32);
            if cred_font == Font::Normal && s.starts_with('_') {
                cred_font = Font::Small;
            }
            // creditData[] has 249 entries; lines past it read beyond the array in the original.
            let d = CREDIT_DATA.get(j).copied().unwrap_or(CreditData { spacing: 0, r: 0, g: 0, b: 0 });
            spacing += d.spacing as i32;
            let y = (4096 + (j as i32 + spacing) * CREDIT_SPACING).min(10000);
            let t = self.textover.create_and_add_text_overlay(2048, y as i16, &s, true, 255, cred_font, TEXTOVERLAY_SHADOW + TEXTOVERLAY_PIXELS);
            tx!(self, t, |o| {
                o.r = d.r;
                o.g = d.g;
                o.b = d.b;
                o.speed = 4096 * 6;
            });
            self.options.credits_text[j] = t;
            j += 1;
        }
    }

    /// Port of `EndCredits` (Common/options.c:3027).
    pub fn end_credits(&mut self) {
        self.options.credits_running = 0;
        let mut j = 0;
        while !credits_end(self, j) {
            self.textover.sub_text_overlay(self.options.credits_text[j]);
            j += 1;
        }
    }

    /// Port of `DoCredits` (Common/options.c:3042).
    pub fn do_credits(&mut self) {
        let mut spacing = 0i32;
        self.options.credits_y -= (8 * self.game_speed) / 4096;

        if self.options.ending_credits != 0 {
            let mut j = 0;
            while !credits_end(self, j) {
                self.t_dec_alpha(self.options.credits_text[j]);
                j += 1;
            }
            if self.textover.get(self.options.credits_text[0]).map_or(0, |o| o.draw) == 0 {
                self.end_credits();
            }
        } else {
            self.t_dec_alpha(self.options.title);
            self.s_dec_alpha(self.options.title_bak);
            for j in 0..(self.options.num_extras_available.max(0) as usize).min(NUM_EXTRAS) {
                self.t_dec_alpha(self.options.extras[j]);
            }
            if self.options.settings_text.is_some() {
                self.t_dec_alpha(self.options.settings_text);
            }
        }
        let mut j = 0usize;
        loop {
            spacing += CREDIT_DATA.get(j).map_or(0, |d| d.spacing as i32);
            let y = (4096 + (j as i32 + spacing) * CREDIT_SPACING + self.options.credits_y).clamp(-10000, 10000) as i16;
            let t = self.options.credits_text[j];
            tx!(self, t, |o| {
                o.y_pos = y;
                o.y_pos_to = y;
            });
            if credits_end(self, j + 1) {
                if (y as i32) < -CREDIT_SPACING {
                    self.end_credits();
                }
                break;
            }
            j += 1;
        }
    }

    /// Port of `FMVUp` (Common/options.c:3085).
    pub fn fmv_up(&mut self) {
        let n = self.options.num_fmv_open;
        if n > 1 {
            self.options.fmv_num = (self.options.fmv_num + n - 1) % n;
            self.hop_sfx();
        }
    }

    /// Port of `FMVDown` (Common/options.c:3094).
    pub fn fmv_down(&mut self) {
        let n = self.options.num_fmv_open;
        if n > 1 {
            self.options.fmv_num = (self.options.fmv_num + 1) % n;
            self.hop_sfx();
        }
    }

    /// Whether the Extras list shows the host's "Settings" entry.
    pub fn extras_settings_on(&self) -> bool {
        self.options.settings_text.is_some() && self.game_options().settings_menu
    }

    /// Port of `FMVSelect` (Common/options.c:3103).
    pub fn fmv_select(&mut self) {
        self.screen_fade(255, 0, 30);
        self.options.playing_fmv = true;
        self.fadeout.keep_fade = false;
    }

    /// Port of `FMVBack` (Common/options.c:3111).
    pub fn fmv_back(&mut self) {
        self.options.mode = OP_EXTRA;
        for i in 0..(self.options.num_fmv_open.max(0) as usize).min(NUM_FMV_SEQUENCES) {
            self.textover.sub_text_overlay(self.options.fmv_text[i]);
        }
        for i in 0..(self.options.num_extras_available.max(0) as usize).min(NUM_EXTRAS) {
            tx!(self, self.options.extras[i], |o| o.draw = 1);
        }
        self.hop_sfx();
    }

    /// Port of `RunArtViewer` (Common/options.c:3152).
    pub fn run_art_viewer(&mut self) {
        if self.levplay.pad_data.debounce[0] & (PAD_START | PAD_TRIANGLE) != 0 {
            self.options.current_art = MAX_ARTWORK;
        }
        let afc = self.act_frame_count;
        self.options.art_timer.update(-1, afc);

        if !self.levplay.fading_out && self.fadeout.keep_fade {
            self.free_backdrop();
            if self.options.current_art == MAX_ARTWORK {
                self.game_state.mode = FRONTEND_MODE;
                self.init_level(worldid::FRONTEND, LEVELID_FRONTEND1);
                self.frame_count = 0;
            } else {
                let name = format!("ARTWORK{:02}", self.options.current_art);
                self.init_backdrop(&name);
                self.options.current_art += 1;
                self.screen_fade(0, 255, 30);
                self.options.art_timer.init(10);
                self.fadeout.keep_fade = false;
            }
        }
        if self.options.art_timer.time == 0 || (!self.levplay.fading_out && self.levplay.pad_data.debounce[0] != 0) {
            self.screen_fade(255, 0, 30);
            self.fadeout.keep_fade = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credit_data_matches_table() {
        assert_eq!(CREDIT_DATA.len(), 249);
        assert_eq!(CREDIT_DATA[1], CreditData { spacing: 3, r: 255, g: 0, b: 0 });
        assert_eq!(CREDIT_DATA[248], CreditData { spacing: 0, r: 255, g: 255, b: 255 });
    }

    #[test]
    fn sound_volume_controls_emit_effects() {
        let mut g = Game::new();
        g.textover.init_text_overlay_linked_list(400);
        g.overlays.init_sprite_overlay_linked_list();
        g.init_options_menu();
        let mv = g.savegame.global_music_vol;
        g.options.sound_selection = 0;
        g.sound_left();
        assert_eq!(g.savegame.global_music_vol, mv - 1);
        assert!(g.effects.iter().any(|e| matches!(e, Effect::SetMusicVolume { vol, .. } if *vol == mv - 1)));
        g.sound_down();
        assert_eq!(g.options.sound_selection, 1);
        let sv = g.savegame.global_sound_vol;
        g.sound_left();
        assert_eq!(g.savegame.global_sound_vol, sv - 1);
        assert_eq!(g.pcaudio.global_sound_vol, sv - 1);
        assert!(g.effects.iter().any(|e| matches!(e, Effect::SetSoundVolume { vol } if *vol == sv - 1)));
    }
}
