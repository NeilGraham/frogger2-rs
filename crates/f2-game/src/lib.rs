//! Deterministic gameplay simulation of Frogger 2: Swampy's Revenge, ported
//! module-by-module from the original `FroggerProject/Common` C sources.
//!
//! Conventions (see `docs/porting-guide.md`):
//! * every former global lives in [`Game`], grouped by the C file that defined it;
//! * pointers become typed indices ([`ids`]); intrusive lists keep their order ([`list`]);
//! * calls into platform services become [`effects::Effect`]s;
//! * one call to [`Game::tick`] = one 60 Hz logical frame (`gameSpeed == 4096`).

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(not(target_vendor = "nintendo64"), forbid(unsafe_code))]
// The N64 build only: `link_section` (an "unsafe" attribute) places the per-actor pass in one
// I-cache-friendly region (`.text.f2hot_s`, tools/n64/Makefile); there is still no unsafe code.
#![cfg_attr(target_vendor = "nintendo64", deny(unsafe_code))]

extern crate alloc;

/// `alloc` items the std prelude would provide (the crate also builds `no_std`).
#[allow(unused_imports)]
pub(crate) mod prelude {
    pub use alloc::borrow::ToOwned;
    pub use alloc::boxed::Box;
    pub use alloc::string::{String, ToString};
    pub use alloc::vec::Vec;
    pub use alloc::{format, vec};
}

pub mod actor2;
pub mod anim;
pub mod assets;
pub mod babyfrog;
pub mod backdrop;
pub mod bank_tables;
pub mod bbtimer;
pub mod cam;
pub mod charsel; // wave G
pub mod collect;
pub mod defdata; // DefData.c (port-gaps agent)
pub mod define;
pub mod effects;
pub mod enemies;
pub mod enemy_tables;
pub mod enhance; // P6-1
pub mod evenfunc; // wave D
pub mod event; // wave D
pub mod fadeout;
pub mod frogger;
pub mod frogmove;
pub mod frontend;
pub mod game;
pub mod general;
pub mod hud; // wave E
pub mod ids;
pub mod language;
pub mod layout;
pub mod levels; // P7-1 mods: level registry
pub mod levplay;
pub mod list;
pub mod map;
pub mod mdx_actor;
pub mod memload;
pub mod menus; // wave G
pub mod modscript; // P7b mods: behaviour scripts
pub mod multi;
pub mod netsync; // P6-2 netplay: snapshot / checksum / resimulate
pub mod options; // wave G
pub mod overlays; // wave E
pub mod particle; // wave E
pub mod path;
pub mod pc_actor;
pub mod pc_banks;
pub mod pcaudio; // ambient
pub mod platform;
pub mod ptexture; // wave E
pub mod savegame;
pub mod script; // wave D
pub mod session;
pub mod simbench; // phase 3S: benchmark scenarios (host + N64)
pub mod specfx;
pub mod sprite; // wave E
pub mod story;
pub mod textover; // wave E
pub mod timer;
pub mod timing; // phase 3T: the retail variable timestep (opt-in)
pub mod tongue;
#[cfg(feature = "std")]
pub mod trace;
pub mod training;
pub mod view; // P3-T: MDX view model / culling
mod view_tables; // phase 3S: generated sinFast/acosFast tables

/// `Serialize`/`Deserialize` for a struct whose declaration order (memory layout) differs
/// from its serialized field order: a mirror struct `$wire` lists the fields in serialized
/// order (phase 3S keeps the netsync checksum and any serialized form unchanged).
#[macro_export]
#[doc(hidden)]
macro_rules! ser_order {
    ($ty:ident, $name:literal, $wire:ident { $( $(#[$m:meta])* $f:ident : $t:ty ),* $(,)? }) => {
        #[derive(serde::Serialize)]
        #[serde(rename = $name)]
        struct $wire<'a> {
            $( $(#[$m])* $f: &'a $t, )*
        }
        impl serde::Serialize for $ty {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                $wire { $( $f: &self.$f, )* }.serialize(s)
            }
        }
        const _: () = {
            #[derive(serde::Deserialize)]
            #[serde(rename = $name)]
            struct Owned {
                $( $(#[$m])* $f: $t, )*
            }
            impl<'de> serde::Deserialize<'de> for $ty {
                fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                    let o = Owned::deserialize(d)?;
                    Ok($ty { $( $f: o.$f, )* })
                }
            }
        };
    };
}

use f2_math::Rng;
use prelude::*;

pub use define::GameStateStruct;
pub use effects::Effect;

/// Per-player pad state for one tick (PSX button bits, `PC/islpad.h`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TickInput {
    pub pads: [u16; 4],
}

/// The whole simulation state (all former globals).
///
/// `Clone` is the netplay/rollback snapshot (`netsync.rs`): immutable data (models, level
/// files, the data source) is shared behind `Arc`s.
#[derive(Clone, Default)]
pub struct Game {
    /// `gameState` (`Common/game.c`).
    pub game_state: GameStateStruct,
    /// `frameCount` (`Common/frontend.c:89`): frames since the level (or mode) started.
    pub frame_count: i32,
    /// `actFrameCount` (`Common/layout.c`): elapsed 60 Hz frames.
    pub act_frame_count: u32,
    /// `lastActFrameCount`.
    pub last_act_frame_count: u32,
    /// `gameSpeed` (20.12): 4096 (one 60 Hz frame) under [`Game::tick`]; set per tick by
    /// `LoopFunc`'s timing under [`Game::tick_timed`] / [`Game::tick_frames`] (`timing.rs`).
    pub game_speed: i32,
    /// `rseed` and `Random()` (`Common/maths.c`).
    pub rng: Rng,
    /// Level geometry (`Common/map.c`).
    pub map: map::Map,
    /// Model facts for loaded banks.
    pub models: assets::ModelTable,
    /// Platform requests produced this tick.
    pub effects: Vec<Effect>,
    /// Actors (`Common/actor2.c`, `PC/actor.cpp`, MDX actor state).
    pub actor2: actor2::Actor2State,
    /// `Common/general.c` globals (sfx animation map).
    pub general: general::GeneralState,
    /// `Common/path.c` globals.
    pub path: path::PathState,
    /// `Common/layout.c` globals (`worldVisualData`, ...).
    pub layout: layout::LayoutState,
    /// Level files parsed by `InitLevel` for later stages (entities, script, sfx map).
    pub level_files: layout::LevelFiles,
    /// `Common/frogger.c` globals (`player[]`, `frog[]`, `NUM_FROGS`, `fog`, ...).
    pub frogger: frogger::FroggerState,
    /// `Common/frogmove.c` globals (`currTile[]`, `destTile[]`, ...).
    pub frogmove: frogmove::FrogMoveState,
    /// `Common/platform.c` (+ `platformList`/`currPlatform` from frogger.c).
    pub platform: platform::PlatformState,
    /// `Common/enemies.c` globals.
    pub enemies: enemies::EnemiesState,
    /// `Common/memload.c` globals.
    pub memload: memload::MemLoadState,
    /// `Common/babyfrog.c` globals.
    pub babyfrog: babyfrog::BabyFrogState,
    /// `Common/collect.c` globals (`garibList`, ...).
    pub collect: collect::CollectState,
    /// `Common/cam.c` globals (gameplay part; camera vectors are wave D).
    pub cam: cam::CamState,
    /// `Common/game.c` globals.
    pub game: game::GameLoopState,
    /// `Common/multi.c` globals.
    pub multi: multi::MultiState,
    /// `Common/levplay.c` globals (demo playback) and `padData` (`PC/controll.cpp`).
    pub levplay: levplay::LevPlayState,
    /// `Common/tongue.c` globals.
    pub tongue: tongue::TongueState,
    /// `PC/pcaudio.c` simulation-side globals (samples, ambient sounds, cdTrack). // ambient
    pub pcaudio: pcaudio::PcAudioState,
    // ---- wave F1 (frontend / story / save / language / fades) ----
    /// `Common/frontend.c` globals.
    pub frontend: frontend::FrontendState,
    /// `Common/training.c` globals.
    pub training: training::TrainingState,
    /// `Common/savegame.c` option globals (`globalSoundVol`, `globalMusicVol`).
    pub savegame: savegame::SaveGameState,
    /// Game text (`PC/isllocal.c`, `gameTextLang`).
    pub language: language::LanguageState,
    /// `PC/fadeout.cpp` globals (fade levels; timing in `levplay`).
    pub fadeout: fadeout::FadeoutState,
    /// `PC/backdrop.c` / tiled backdrop / `drawGame` state for the renderer.
    pub backdrop: backdrop::BackdropState,
    /// Data source and level-load queue (host link for `InitLevel`).
    pub session: session::SessionState,
    // wave E: visual effect state read by the renderer.
    /// `Common/sprite.c` (`sprList`).
    pub sprite: sprite::SpriteState,
    /// `Common/particle.c` (`partList`).
    pub particle: particle::ParticleState,
    /// `Common/specfx.c` (`sfxList`, `p2List`, ...).
    pub specfx: specfx::SpecFxState,
    /// MDX render globals the simulation culls against (`view.rs`).
    pub view: view::ViewState,
    /// `Common/overlays.c` (`spriteOverlayList`).
    pub overlays: overlays::SpriteOverlayState,
    /// `Common/textover.c` (`textOverlayList`) and the game text table.
    pub textover: textover::TextOverlayState,
    /// `Common/hud.c` globals.
    pub hud: hud::HudState,
    /// `Common/ptexture.c` (`prcTexList`).
    pub ptexture: ptexture::ProcTexState,
    /// `Common/event.c` globals (wave D: `triggerList`, `triggersReset`).
    pub event: event::EventState,
    /// `Common/script.c` globals (wave D: script flags/counters, `scriptBuffer`).
    pub script: script::ScriptState,
    // wave G: frontend menus.
    /// `Common/menus.c` globals (pause menu, cheats, frontend loop).
    pub menus: menus::MenusState,
    /// `Common/options.c` globals (`options`, credits, art viewer).
    pub options: options::OptionsState,
    /// `Common/charsel.c` (`charSel`).
    pub charsel: charsel::CharSelState,
    // P6-1: enhanced-profile gameplay options (no effect in the authentic profile).
    pub enhance: enhance::EnhanceState,
    // P7b: mod behaviour scripts (empty = authentic).
    pub modscript: modscript::ModScriptState,
    /// Phase 3S profiling glue: called with a [`simbench::phase`] id at fixed points of a
    /// tick (not game state; `None` in the game).
    pub phase_hook: Option<fn(u32)>,
    /// Phase 3S: `(uid, path.fromNode)` snapshots of the enemy/platform lists for
    /// `EnemyAtFlag`/`PlatformAtFlag` while `UpdateEvents` evaluates triggers (not game
    /// state; see `evenfunc.rs`).
    pub(crate) flag_snap: evenfunc::FlagSnap,
    /// Phase 3T: `timeInfo` and `LoopFunc`'s timing globals (variable timestep; untouched by
    /// [`Game::tick`]).
    pub timing: timing::TimingState,
    /// Phase 3T: the host's `timeGetTime()` (ms) for `InitTiming`/`InitBackdrop` under
    /// [`Game::update_timing`] (not game state; `None` = hold the last clock value).
    pub timing_clock: Option<fn() -> u32>,
}

impl Game {
    pub fn new() -> Game {
        let mut g = Game { game_speed: 4096, ..Default::default() };
        g.rng.seed_random();
        g.enemies.load_orig_tables();
        // gameState = {0,0,0,0,0,0,DIFFICULTY_NORMAL} (game.c:83); showEndLevelScreen = 1.
        g.game_state.difficulty = define::DIFFICULTY_NORMAL;
        g.game.show_end_level_screen = 1;
        g
    }

    /// Advances one logical frame: the timer update, `GameLoop` (for `INGAME_MODE`:
    /// `RunGameLoop`), then the animation step the original performs while drawing.
    pub fn tick(&mut self, input: TickInput) {
        self.tick_game_loop(input);
        self.after_game_loop();
    }

    /// The first half of [`Game::tick`]: the timer update, `ProcessUserInput` and
    /// `GameLoop`. The oracle records its trace line here (`h_game_loop`), i.e. *before*
    /// this tick's `CopyActorList`/`DrawLoop` ([`Game::after_game_loop`]).
    pub fn tick_game_loop(&mut self, input: TickInput) {
        self.last_act_frame_count = self.act_frame_count;
        self.act_frame_count = self.act_frame_count.wrapping_add(1);
        // LoopFunc (PC/Main.cpp): ProcessUserInput, then GameLoop, then DrawLoop.
        self.process_user_input(input.pads);
        self.phase(simbench::phase::INPUT);
        self.game_loop();
        self.phase(simbench::phase::GAME_LOOP_REST);
    }

    #[inline]
    pub(crate) fn phase(&self, id: u32) {
        if let Some(h) = self.phase_hook {
            h(id);
        }
    }

    /// The rest of `LoopFunc` (PC/Main.cpp:686) after `GameLoop`: `CopyActorList`, then
    /// the state changes of `DrawLoop` (view matrix, actor culling + animation, tongue,
    /// sprites/effects, overlays). Also run once after a level is started from the
    /// frontend (`InitDemoMode`/`InitLevel` happen inside the frontend's `GameLoop`).
    pub fn after_game_loop(&mut self) {
        self.copy_and_draw_actors();
        self.phase(simbench::phase::ACTOR_LIST_DRAW);
        self.draw_loop_3d_state(); // wave E
        self.phase(simbench::phase::DRAW_3D);
        self.draw_screen_transition();
        self.draw_loop_2d_state(); // wave E
        self.phase(simbench::phase::DRAW_2D);
    }

    /// `CopyActorList`, then `DrawLoop` (PC/drawloop.cpp:142) up to `UpdateFrogTongue`:
    /// `CalcViewMatrix`, `farClip`, `ActorListDraw(0/1)` (animation advances only for
    /// drawn actors).
    ///
    /// Phase 3S: one walk of `actList` runs `CopyActorList`'s body and, right after it,
    /// `ActorListDraw(0)`'s for the same actor (the actor's data is still in the cache),
    /// collecting list 1 for the second pass. Same results as the separate passes:
    /// `CalcViewMatrix` reads only the camera and fog; an actor's `ActorListDraw` reads and
    /// writes only that actor (its placement copied just before), sprites, the tongue's head
    /// matrix, sounds and `Random()` (in the same order as before, as `CopyActorList` makes
    /// none of those calls); `CopyActorList` reads only its own actor and `frog[0]`'s
    /// position (unchanged by drawing); neither creates, frees nor re-lists actors.
    #[cfg_attr(target_vendor = "nintendo64", allow(unsafe_code), link_section = ".text.f2hot_s")]
    fn copy_and_draw_actors(&mut self) {
        use define::mode::*;
        if matches!(self.game_state.mode, ARTVIEWER_MODE | STARTUP_MODE | TEASERSCREEN_MODE) {
            self.copy_actor_list();
            return;
        }
        self.calc_view_matrix();
        let view = self.specfx.view;
        let frog0 = self.frog0_position();
        let mut ids1 = Vec::new();
        let mut cur = self.actor2.act_list.first;
        while let Some(id) = cur {
            cur = self.actor2.actors[id].links.next;
            self.copy_actor(id, frog0);
            match self.actor2.actors[id].actor.actual_actor.list_num {
                0 => self.actor_list_draw_one(id, view.as_ref()),
                1 => ids1.push(id),
                _ => {}
            }
        }
        for id in ids1 {
            self.actor_list_draw_one(id, view.as_ref());
        }
        // DrawLoop (PC/drawloop.cpp:250): UpdateFrogTongue(0) after ActorListDraw(1).
        if self.game_state.multi == define::SINGLEPLAYER && self.frogger.frog[0].is_some() {
            self.update_frog_tongue(0);
            self.update_frog_croak(0); // DrawLoop (PC/drawloop.cpp:251)
        }
    }

    /// Drains platform requests produced since the last call.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        core::mem::take(&mut self.effects)
    }
}
