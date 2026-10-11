//! Phase 3T: the retail variable timestep (`Game::tick_frames`) at 2, 3 and 4 frames per tick
//! against 1, render-free: no panics, every tick advances `actFrameCount` by k with
//! `gameSpeed` = 4096·k, the attract demo ends after the same game time, clock-driven path
//! entities stay where k = 1 has them (up to the k-1 frames an enemy that stops to wait
//! may be short of its node), the frog stays on its tiles and nothing leaves the map more
//! than at k = 1. Reads `F2_GAME_DIR` (default `game-data/usa/game`); skips when absent.

#[path = "../examples/timestep_check/check.rs"]
#[allow(dead_code)]
mod check;

use std::path::PathBuf;
use std::sync::Arc;

use check::Input;
use f2_game::assets::DirData;
use f2_game::simbench::{Scenario, Start};

fn game_dir() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let p = std::env::var("F2_GAME_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("game-data/usa/game"));
    let p = if p.is_absolute() { p } else { root.join(p) };
    let p = if p.join("game").join("collision").is_dir() { p.join("game") } else { p };
    if p.join("collision").is_dir() {
        Some(p)
    } else {
        eprintln!("skipping: no game data at {}", p.display());
        None
    }
}

#[test]
fn speeds_2_to_4_keep_game_time_and_paths() {
    let Some(dir) = game_dir() else { return };
    let data = Arc::new(DirData(dir));
    let cases = [
        (Scenario { name: "demo-1-0", start: Start::Demo(1, 0), ticks: 1700 }, Input::Idle),
        (Scenario { name: "0-0-idle", start: Start::Arcade(0, 0), ticks: 1200 }, Input::Idle),
        (Scenario { name: "0-0-play", start: Start::Arcade(0, 0), ticks: 1200 }, Input::Play),
    ];
    for (s, input) in &cases {
        let reference = check::run(s, data.clone(), 1, *input);
        assert!(reference.panicked.is_none(), "{}: k=1 panicked", s.name);
        assert_eq!(reference.bad_steps, 0, "{}", s.name);
        for k in 2..=4u32 {
            let r = check::run(s, data.clone(), k, *input);
            let ctx = format!("{} k={k}", s.name);
            assert!(r.panicked.is_none(), "{ctx}: panicked: {:?}", r.panicked);
            assert_eq!(r.bad_steps, 0, "{ctx}: actFrameCount/gameSpeed not stepped by k");
            if let (Some(e1), Some(ek)) = (reference.demo_end_tick, r.demo_end_tick) {
                // Same game time, give or take GTUpdate's quantisation: the demo timeout
                // counts a "second" when actFrameCount passes lastCount+60, i.e. every 61
                // frames at k=1 but every 62/63/64 at k=2/3/4 (+1.6/3.3/4.9 %).
                assert!(ek * k >= e1 && ek * k <= e1 + 30 * k, "{ctx}: demo ended at tick {ek}, k=1 at {e1}");
            } else {
                assert_eq!(r.act, reference.act, "{ctx}: game time");
            }
            if *input == Input::Idle && s.name != "demo-1-0" {
                let (frames, n, max, _, _) = check::compare(&reference, &r, true);
                assert!(frames > 0 && n > 0, "{ctx}: nothing compared");
                assert!(max <= 30 * (k as i32 - 1) + 8, "{ctx}: path entity off by {max}");
                assert_eq!(check::frog_err(&reference, &r), 0, "{ctx}: idle frog moved");
            }
            assert!(r.frog_rest_max <= reference.frog_rest_max.max(64) * 2, "{ctx}: frog {} from its tile", r.frog_rest_max);
            assert!(r.out_of_bounds * k <= reference.out_of_bounds + k, "{ctx}: entities left the map");
        }
    }
}

/// Retail `LoopFunc`: in the pause menu `gameSpeed` is 0 and `actFrameCount` stays frozen, so
/// clock-driven path entities hold still and carry on from the same place after resuming
/// (owner-reported: with `tick()` they kept running while paused and jumped on resume).
#[test]
fn pause_freezes_game_time() {
    use f2_game::define::mode::PAUSE_MODE;
    use f2_game::levplay::PAD_START;
    use f2_game::TickInput;
    let Some(dir) = game_dir() else { return };
    let s = Scenario { name: "0-0-pause", start: Start::Arcade(0, 0), ticks: 0 };
    let mut g = f2_game::simbench::setup(&s, Arc::new(DirData(dir)));
    let tick = |g: &mut f2_game::Game, pad: u16| g.tick_frames(TickInput { pads: [pad, 0, 0, 0] }, 1);
    for _ in 0..240 {
        tick(&mut g, 0);
    }
    // Start pauses (a press = one tick down, then released).
    for i in 0..30 {
        tick(&mut g, if i == 0 { PAD_START } else { 0 });
        if g.game_state.mode == PAUSE_MODE {
            break;
        }
    }
    assert_eq!(g.game_state.mode, PAUSE_MODE, "Start did not pause");
    let act = g.act_frame_count;
    for _ in 0..180 {
        tick(&mut g, 0);
        assert_eq!(g.game_state.mode, PAUSE_MODE);
        assert_eq!(g.act_frame_count, act, "actFrameCount advanced while paused");
        assert_eq!(g.game_speed, 0, "gameSpeed not 0 while paused");
    }
}
