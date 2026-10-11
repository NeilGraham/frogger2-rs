//! Phase 3T: runs a scenario with the retail variable timestep at a fixed speed `k`
//! (`Game::tick_frames(input, k)`: `gameSpeed` = 4096·k, `actFrameCount` += k) and collects
//! invariants, compared against the same scenario at `k` = 1 at equal game time
//! (`actFrameCount`). Shared by `examples/timestep_check` and `tests/timestep.rs`.

use std::collections::HashMap;
use std::sync::Arc;

use f2_game::assets::DataSource;
use f2_game::define::mode::{DEMO_MODE, INGAME_MODE, PAUSE_MODE};
use f2_game::levplay::{PAD_CIRCLE, PAD_CROSS, PAD_DOWN, PAD_LEFT, PAD_RIGHT, PAD_SQUARE, PAD_UP};
use f2_game::simbench::{self, Scenario, Start};
use f2_game::TickInput;

/// Input of a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// No buttons (path entities can be compared with `k` = 1 exactly in game time).
    Idle,
    /// A press every 12 game frames (hops, superhop, tongue, croak), timed by
    /// `actFrameCount` so every speed sees the same presses at the same game time.
    Play,
}

/// The gameplay test's pad pattern (`tests/gameplay.rs`), indexed by game frame.
pub fn pattern(t: u32) -> u16 {
    const SEQ: [u16; 10] = [PAD_UP, PAD_UP, PAD_RIGHT, PAD_DOWN, PAD_LEFT, PAD_CROSS, PAD_UP | PAD_CROSS, PAD_CIRCLE, PAD_SQUARE, PAD_LEFT];
    let k = (t / 12) as usize;
    if t % 12 < 3 {
        SEQ[k % SEQ.len()]
    } else if k % 7 == 3 {
        PAD_UP
    } else {
        0
    }
}

/// Per game frame: positions of the enemies and platforms (list order, uid) and the frog.
#[derive(Clone, Debug, Default)]
pub struct Snap {
    pub ents: Vec<(i32, [i16; 3])>,
    pub frog: Option<[i16; 3]>,
}

/// What one run produced.
#[derive(Clone, Debug, Default)]
pub struct Run {
    pub ticks: u32,
    /// `actFrameCount` at the end (game frames simulated in this level).
    pub act: u32,
    pub panicked: Option<String>,
    /// Ticks whose `actFrameCount` delta or `gameSpeed` was not k (outside pause / reloads).
    pub bad_steps: u32,
    /// `scoreTimer.time` at the start and end (the arcade clock, 1 per 61 frames at 60 Hz).
    pub score_timer: (u16, u16),
    /// Lives lost (play).
    pub deaths: u32,
    /// Max distance of the resting frog from its tile centre (fell off / through the map).
    pub frog_rest_max: i32,
    /// Distinct tiles the frog stood on.
    pub tiles_visited: usize,
    pub babies_saved: i32,
    /// Demo: tick at which the demo ended.
    pub demo_end_tick: Option<u32>,
    /// Enemy/platform positions per game frame.
    pub snaps: HashMap<u32, Snap>,
    /// Entities whose position was not finite / left the map's box (+ margin).
    pub out_of_bounds: u32,
}

fn dist(a: [i16; 3], b: [i16; 3]) -> i32 {
    let d = |i: usize| (a[i] as i64 - b[i] as i64).pow(2);
    (((d(0) + d(1) + d(2)) as f64).sqrt()) as i32
}

/// Runs `s` (with `s.ticks` game frames) at `k` frames per tick.
pub fn run(s: &Scenario, data: Arc<dyn DataSource + Send + Sync>, k: u32, input: Input) -> Run {
    let mut g = simbench::setup(s, data);
    let mut r = Run::default();
    // Map box (tile centres) for the bounds check.
    let (mut lo, mut hi) = ([i16::MAX; 3], [i16::MIN; 3]);
    for t in &g.map.tiles {
        let c = [t.centre.vx, t.centre.vy, t.centre.vz];
        for i in 0..3 {
            lo[i] = lo[i].min(c[i]);
            hi[i] = hi[i].max(c[i]);
        }
    }
    const MARGIN: i32 = 4000;
    r.score_timer.0 = g.game.score_timer.time;
    let mut lives = g.frogger.player[0].lives;
    let mut tiles = std::collections::HashSet::new();
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        while g.act_frame_count < s.ticks {
            let before = (g.act_frame_count, g.game_state.mode, g.frogger.player[0].level_num);
            let pad = match input {
                Input::Idle => 0,
                Input::Play => pattern(g.act_frame_count + k),
            };
            g.tick_frames(TickInput { pads: [pad, 0, 0, 0] }, k);
            g.effects.clear();
            g.session.level_loads.clear();
            r.ticks += 1;
            let same_level = before.2 == g.frogger.player[0].level_num && g.act_frame_count > before.0;
            if before.1 != PAUSE_MODE && g.game_state.mode != PAUSE_MODE && same_level {
                if g.act_frame_count.wrapping_sub(before.0) != k || g.game_speed != 4096 * k as i32 {
                    r.bad_steps += 1;
                }
            }
            let f = g.trace_frame();
            if let Some(p) = f.players.first() {
                if p.lives < lives {
                    r.deaths += (lives - p.lives) as u32;
                }
                lives = p.lives;
                if p.curr_tile >= 0 {
                    tiles.insert(p.curr_tile);
                }
                // At rest: can jump, not jumping, alive, not riding a platform.
                let on_platform = g.platform.curr_platform.first().is_some_and(|c| c.is_some());
                if let (Some(pos), Some(t)) = (p.pos, g.frogmove.curr_tile[0]) {
                    if p.can_jump != 0 && p.has_jumped == 0 && p.dead_time == 0 && !on_platform && g.game_state.mode == INGAME_MODE {
                        let c = g.map.tiles[t.0 as usize].centre;
                        r.frog_rest_max = r.frog_rest_max.max(dist(pos, [c.vx, c.vy, c.vz]));
                    }
                }
            }
            let mut snap = Snap { frog: f.players.first().and_then(|p| p.pos), ..Default::default() };
            for (uid, pos) in
                f.enemies.iter().filter(|e| e.active != 0).map(|e| (e.uid, e.pos)).chain(f.platforms.iter().filter(|p| p.active != 0).map(|p| (p.uid, p.pos)))
            {
                if let Some(p) = pos {
                    if (0..3).any(|i| (p[i] as i32) < lo[i] as i32 - MARGIN || (p[i] as i32) > hi[i] as i32 + MARGIN) {
                        r.out_of_bounds += 1;
                    }
                    snap.ents.push((uid, p));
                }
            }
            r.snaps.insert(g.act_frame_count, snap);
            r.babies_saved = g.babyfrog.babies_saved as i32;
            if let Start::Demo(..) = s.start {
                if g.game_state.mode != DEMO_MODE || g.levplay.demo_finished {
                    r.demo_end_tick = Some(r.ticks);
                    break;
                }
            }
        }
    }));
    if let Err(e) = res {
        r.panicked = Some(e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default());
    }
    r.act = g.act_frame_count;
    r.score_timer.1 = g.game.score_timer.time;
    r.tiles_visited = tiles.len();
    r
}

/// Max distance between the frog of `run` and of the `k` = 1 `reference` at equal game time.
pub fn frog_err(reference: &Run, run: &Run) -> i32 {
    run.snaps.iter().filter_map(|(act, s)| Some(dist(s.frog?, reference.snaps.get(act)?.frog?))).max().unwrap_or(0)
}

/// Path-entity position error of `run` against the `k` = 1 `reference` at equal game time:
/// `(frames compared, entity samples, max error, 99th percentile, share within 8 units)`.
/// Entities are matched by list position when both lists have the same uids.
pub fn compare(reference: &Run, run: &Run) -> (u32, usize, i32, i32, f64) {
    let mut errs = Vec::new();
    let mut frames = 0;
    for (act, s) in &run.snaps {
        let Some(r) = reference.snaps.get(act) else { continue };
        if r.ents.len() != s.ents.len() || r.ents.iter().zip(&s.ents).any(|(a, b)| a.0 != b.0) {
            continue;
        }
        frames += 1;
        errs.extend(r.ents.iter().zip(&s.ents).map(|(a, b)| dist(a.1, b.1)));
    }
    errs.sort_unstable();
    let n = errs.len();
    if n == 0 {
        return (frames, 0, 0, 0, 1.0);
    }
    let within = errs.iter().filter(|&&e| e <= 8).count() as f64 / n as f64;
    (frames, n, errs[n - 1], errs[(n * 99 / 100).min(n - 1)], within)
}
