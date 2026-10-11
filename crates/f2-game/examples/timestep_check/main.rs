//! Phase 3T host verification of the retail variable timestep (`f2_game::timing`): plays
//! scenarios render-free at fixed speeds (`Game::tick_frames(input, k)`, k frames per tick)
//! and prints invariants against k = 1 at equal game time.
//!
//! `cargo run --release -p f2-game --example timestep_check -- [--all-levels] [--speeds 1,2,3,4] [--secs 60] [name ...]`
//!
//! Scenarios: the attract demos 0-0, 1-0, 6-1 (their recorded keys are replayed by
//! `actFrameCount`, so speed > 1 may desync them, as on a slow retail PC) and arcade levels,
//! each idle (path entities vs k = 1) and with a scripted pad pattern timed in game frames.
//! Game data: `F2_GAME_DIR` (default `game-data/usa/game`).

mod check;

use std::path::PathBuf;
use std::sync::Arc;

use f2_game::assets::DirData;
use f2_game::simbench::{Scenario, Start};

use check::Input;

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn main() {
    let mut speeds = vec![1u32, 2, 3, 4];
    let mut secs = 60u32;
    let mut all = false;
    let mut names = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--speeds" => speeds = args.next().unwrap_or_default().split(',').filter_map(|s| s.parse().ok()).collect(),
            "--secs" => secs = args.next().and_then(|s| s.parse().ok()).unwrap_or(60),
            "--all-levels" => all = true,
            _ => names.push(a),
        }
    }
    let dir = std::env::var("F2_GAME_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("game-data/usa/game"));
    let data = Arc::new(DirData(dir));
    let frames = secs * 60;
    let mut list: Vec<(Scenario, Input)> = Vec::new();
    for (w, l) in [(0, 0), (1, 0), (6, 1)] {
        list.push((Scenario { name: leak(format!("demo-{w}-{l}")), start: Start::Demo(w, l), ticks: 1700 }, Input::Idle));
    }
    let levels: Vec<(u32, u32)> = if all { (0..10).flat_map(|w| (0..4).map(move |l| (w, l))).collect() } else { vec![(0, 0), (2, 0), (2, 2), (4, 1), (6, 0)] };
    for (w, l) in levels {
        for input in [Input::Idle, Input::Play] {
            let tag = if input == Input::Idle { "idle" } else { "play" };
            list.push((Scenario { name: leak(format!("{w}-{l}-{tag}")), start: Start::Arcade(w, l), ticks: frames }, input));
        }
    }
    list.retain(|(s, _)| names.is_empty() || names.iter().any(|n| s.name.starts_with(n.as_str())));

    println!("| scenario | k | ticks | game frames | panic | bad steps | path ents vs k=1: frames / max err / p99 / <=8 | all ents: max / <=8 | frog vs k=1 | frog rest max | deaths | tiles | babies | demo end | out of box |");
    println!("|---|---:|---:|---:|---|---:|---|---|---:|---:|---:|---:|---:|---:|---:|");
    for (s, input) in &list {
        let reference = check::run(s, data.clone(), 1, *input);
        for &k in &speeds {
            let r = if k == 1 { reference.clone() } else { check::run(s, data.clone(), k, *input) };
            let (nf, _n, max, p99, within) = check::compare(&reference, &r, true);
            let (_, _, all_max, _, all_within) = check::compare(&reference, &r, false);
            println!(
                "| {} | {} | {} | {} | {} | {} | {} / {} / {} / {:.1}% | {} / {:.1}% | {} | {} | {} | {} | {} | {} | {} |",
                s.name,
                k,
                r.ticks,
                r.act,
                r.panicked.as_deref().unwrap_or("-"),
                r.bad_steps,
                nf,
                max,
                p99,
                within * 100.0,
                all_max,
                all_within * 100.0,
                check::frog_err(&reference, &r),
                r.frog_rest_max,
                r.deaths,
                r.tiles_visited,
                r.babies_saved,
                r.demo_end_tick.map_or("-".to_string(), |t| t.to_string()),
                r.out_of_bounds
            );
            if std::env::var_os("F2_TS_DEBUG").is_some() && k > 1 {
                for (i, uid, act, err) in check::divergence(&reference, &r).into_iter().take(12) {
                    let kind = reference.snaps.get(&act).and_then(|s| s.kinds.get(i)).copied();
                    eprintln!("  {} k={k}: entity #{i} uid {uid} {kind:x?} first off at frame {act}, max {err}", s.name);
                }
                // F2_TS_ENT=i: entity #i's positions (k = 1 / k) over the first frames.
                if let Some(i) = std::env::var("F2_TS_ENT").ok().and_then(|s| s.parse::<usize>().ok()) {
                    let from = std::env::var("F2_TS_FROM").ok().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0) / k * k + k;
                    for act in (from..=from + 240).step_by(k as usize) {
                        let p = |run: &check::Run| run.snaps.get(&act).and_then(|s| s.ents.get(i)).map(|e| e.1);
                        eprintln!("    frame {act}: {:?} / {:?}", p(&reference), p(&r));
                    }
                }
            }
        }
    }
}
