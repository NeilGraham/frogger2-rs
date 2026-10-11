//! Simulation benchmark scenarios (phase 3S): the same tick sequence on the host
//! (`examples/sim_bench.rs`) and on the N64 (`frogger-n64` feature `simbench`), so per-tick
//! cost can be compared across machines and the end-of-run [`Game::checksum`] proves the
//! target computes bit-identical state. Not used by the game itself.

use alloc::sync::Arc;

use crate::assets::DataSource;
use crate::session::SessionConfig;
use crate::{Game, TickInput};

/// `rseed` when the oracle's frontend starts a demo (as in `examples/demo_trace.rs`).
const RSEED_AT_DEMO_START: u32 = 144_420_441;

/// A/B switch for performance experiments (phase 3S): code under test checks
/// [`tune`] so both variants live in one binary (same code layout). Not used by the game.
pub static TUNE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Whether experiment bit `bit` is on.
#[inline]
pub fn tune(bit: u32) -> bool {
    TUNE.load(core::sync::atomic::Ordering::Relaxed) & (1 << bit) != 0
}

/// Phase ids passed to [`Game::phase_hook`] (each marks the end of a part of the tick).
pub mod phase {
    pub const INPUT: u32 = 0;
    /// `RunGameLoop` up to the frog updates (camera, controller, frogs).
    pub const FROGS: u32 = 1;
    pub const PLATFORMS: u32 = 2;
    pub const ENEMIES: u32 = 3;
    pub const SPECFX: u32 = 4;
    pub const EVENTS: u32 = 5;
    /// The rest of `GameLoop` (ambient sounds, collectables, HUD, ...).
    pub const GAME_LOOP_REST: u32 = 6;
    pub const COPY_ACTOR_LIST: u32 = 7;
    pub const ACTOR_LIST_DRAW: u32 = 8;
    pub const DRAW_3D: u32 = 9;
    pub const DRAW_2D: u32 = 10;
    pub const COUNT: usize = 11;
    pub const NAMES: [&str; COUNT] = ["input", "frogs", "platforms", "enemies", "specfx", "events", "loop-rest", "copyactors", "actordraw", "draw3d", "draw2d"];
}

/// How a scenario starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    /// The frontend level (`start_frontend`, training marked done), idle pad.
    Frontend,
    /// An arcade level (`start_arcade`), idle pad.
    Arcade(u32, u32),
    /// The attract demo of a level (`DEMOS\RECORD-W-L.KEY`), until it ends.
    Demo(u32, u32),
}

/// One benchmark scenario: `ticks` simulation ticks from `start`.
#[derive(Clone, Copy, Debug)]
pub struct Scenario {
    pub name: &'static str,
    pub start: Start,
    pub ticks: u32,
}

/// The standard set (frontend, Garden 1, the heaviest arcade levels, the three demos).
pub const SCENARIOS: &[Scenario] = &[
    Scenario { name: "frontend", start: Start::Frontend, ticks: 600 },
    Scenario { name: "0-0", start: Start::Arcade(0, 0), ticks: 600 },
    Scenario { name: "2-0", start: Start::Arcade(2, 0), ticks: 600 },
    Scenario { name: "2-2", start: Start::Arcade(2, 2), ticks: 600 },
    Scenario { name: "4-1", start: Start::Arcade(4, 1), ticks: 600 },
    Scenario { name: "6-0", start: Start::Arcade(6, 0), ticks: 600 },
    Scenario { name: "demo-0-0", start: Start::Demo(0, 0), ticks: 1500 },
    Scenario { name: "demo-1-0", start: Start::Demo(1, 0), ticks: 1500 },
    Scenario { name: "demo-6-1", start: Start::Demo(6, 1), ticks: 1500 },
];

/// Result of [`run`].
#[derive(Clone, Copy, Debug, Default)]
pub struct Outcome {
    /// Ticks simulated (a demo stops when it ends).
    pub ticks: u32,
    /// Live actors after the level load.
    pub actors: u32,
    /// [`Game::checksum`] after the last tick.
    pub checksum: u64,
}

/// Builds the game for `s` (level loaded, nothing simulated yet).
pub fn setup(s: &Scenario, data: Arc<dyn DataSource + Send + Sync>) -> Game {
    let mut g = Game::new_session(SessionConfig::new(data.clone()));
    match s.start {
        Start::Frontend => {
            let fe = crate::layout::worldid::FRONTEND as usize;
            g.layout.world_visual_data[fe].level_visual_data[crate::layout::LEVELID_FRONTEND4 as usize].level_completed = 1;
            g.start_frontend();
        }
        Start::Arcade(w, l) => g.start_arcade(w, l),
        Start::Demo(w, l) => {
            g.rng.rseed = RSEED_AT_DEMO_START;
            g.game.mode_timer.init(1);
            if let Err(e) = g.init_demo_level(w, l, &*data) {
                g.effects.push(crate::Effect::Log { text: e });
            }
            g.frame_count += 1;
            g.after_game_loop();
        }
    }
    g.session.level_loads.clear();
    g.effects.clear();
    g
}

/// Runs `s` on a game from [`setup`]; `hook(true)` / `hook(false)` bracket every
/// [`Game::tick`] (timers, profiler).
pub fn run(s: &Scenario, g: &mut Game, hook: &mut dyn FnMut(bool)) -> Outcome {
    let actors = g.actor2.act_list.ids(&g.actor2.actors).len() as u32;
    let mut n = 0;
    while n < s.ticks {
        hook(true);
        g.tick(TickInput::default());
        hook(false);
        n += 1;
        g.effects.clear();
        g.session.level_loads.clear();
        if let Start::Demo(..) = s.start {
            if g.game_state.mode != crate::define::mode::DEMO_MODE || g.levplay.demo_finished {
                break;
            }
        }
    }
    Outcome { ticks: n, actors, checksum: g.checksum() }
}
