//! Deterministic gameplay simulation of Frogger 2: Swampy's Revenge, ported
//! module-by-module from the original `FroggerProject/Common` C sources.
//!
//! Conventions (see `docs/porting-guide.md`):
//! * every former global lives in [`Game`], grouped by the C file that defined it;
//! * pointers become typed indices ([`ids`]); intrusive lists keep their order ([`list`]);
//! * calls into platform services become [`effects::Effect`]s;
//! * one call to [`Game::tick`] = one 60 Hz logical frame (`gameSpeed == 4096`).

#![forbid(unsafe_code)]

pub mod actor2;
pub mod anim;
pub mod assets;
pub mod babyfrog;
pub mod backdrop;
pub mod bank_tables;
pub mod bbtimer;
pub mod cam;
pub mod collect;
pub mod define;
pub mod effects;
pub mod enemies;
pub mod enemy_tables;
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
pub mod levplay;
pub mod list;
pub mod map;
pub mod mdx_actor;
pub mod memload;
pub mod multi;
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
pub mod specfx;
pub mod sprite; // wave E
pub mod story;
pub mod textover; // wave E
pub mod timer;
pub mod tongue;
pub mod trace;
pub mod training;
pub mod view; // P3-T: MDX view model / culling

use f2_math::Rng;

pub use define::GameStateStruct;
pub use effects::Effect;

/// Per-player pad state for one tick (PSX button bits, `PC/islpad.h`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TickInput {
    pub pads: [u16; 4],
}

/// The whole simulation state (all former globals).
#[derive(Default)]
pub struct Game {
    /// `gameState` (`Common/game.c`).
    pub game_state: GameStateStruct,
    /// `frameCount` (`Common/frontend.c:89`): frames since the level (or mode) started.
    pub frame_count: i32,
    /// `actFrameCount` (`Common/layout.c`): elapsed 60 Hz frames.
    pub act_frame_count: u32,
    /// `lastActFrameCount`.
    pub last_act_frame_count: u32,
    /// `gameSpeed` (20.12): fixed at 4096 in the port (one frame per tick).
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
        self.game_loop();
    }

    /// The rest of `LoopFunc` (PC/Main.cpp:686) after `GameLoop`: `CopyActorList`, then
    /// the state changes of `DrawLoop` (view matrix, actor culling + animation, tongue,
    /// sprites/effects, overlays). Also run once after a level is started from the
    /// frontend (`InitDemoMode`/`InitLevel` happen inside the frontend's `GameLoop`).
    pub fn after_game_loop(&mut self) {
        self.copy_actor_list();
        self.draw_loop_actors();
        self.draw_loop_3d_state(); // wave E
        self.draw_screen_transition();
        self.draw_loop_2d_state(); // wave E
    }

    /// `DrawLoop` (PC/drawloop.cpp:142) up to `UpdateFrogTongue`: `CalcViewMatrix`,
    /// `farClip`, `ActorListDraw(0/1)` (animation advances only for drawn actors).
    fn draw_loop_actors(&mut self) {
        use define::mode::*;
        if matches!(self.game_state.mode, ARTVIEWER_MODE | STARTUP_MODE | TEASERSCREEN_MODE) {
            return;
        }
        self.calc_view_matrix();
        self.actor_list_draw(0);
        self.actor_list_draw(1);
        // DrawLoop (PC/drawloop.cpp:250): UpdateFrogTongue(0) after ActorListDraw(1).
        if self.game_state.multi == define::SINGLEPLAYER && self.frogger.frog[0].is_some() {
            self.update_frog_tongue(0);
        }
    }

    /// Drains platform requests produced since the last call.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }
}
