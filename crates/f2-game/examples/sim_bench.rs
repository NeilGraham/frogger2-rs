//! Host side of the phase-3S simulation benchmark (`f2_game::simbench`): per scenario the
//! ticks run, live actors, wall time per tick and the end-of-run state checksum (the N64
//! build with `F2N64_FEATURES=simbench` logs the same checksums).
//!
//! `cargo run --release -p f2-game --example sim_bench -- [name ...] [--reps N] [--all-levels]`
//!
//! `--all-levels` runs 600 idle ticks of every single-player arcade level (W 0..=9, L 0..=3)
//! instead, to rank levels by cost. Game data: `F2_GAME_DIR` (default `game-data/usa/game`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use f2_game::assets::DirData;
use f2_game::simbench::{self, Scenario, Start, SCENARIOS};

fn main() {
    let mut names = Vec::new();
    let mut reps = 1;
    let mut all = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--reps" => reps = args.next().and_then(|s| s.parse().ok()).unwrap_or(1),
            "--all-levels" => all = true,
            _ => names.push(a),
        }
    }
    let dir = std::env::var("F2_GAME_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("game-data/usa/game"));
    let data = Arc::new(DirData(dir));
    let list: Vec<Scenario> = if all {
        let names: Vec<&'static str> = (0..10u32).flat_map(|w| (0..4u32).map(move |l| &*Box::leak(format!("{w}-{l}").into_boxed_str()))).collect();
        names.iter().enumerate().map(|(i, n)| Scenario { name: n, start: Start::Arcade(i as u32 / 4, i as u32 % 4), ticks: 600 }).collect()
    } else {
        SCENARIOS.iter().copied().filter(|s| names.is_empty() || names.iter().any(|n| n == s.name)).collect()
    };
    for s in &list {
        let mut best = Duration::MAX;
        let mut out = Default::default();
        for _ in 0..reps {
            let mut g = simbench::setup(s, data.clone());
            let mut total = Duration::ZERO;
            let mut t0 = Instant::now();
            out = simbench::run(s, &mut g, &mut |start| {
                if start {
                    t0 = Instant::now();
                } else {
                    total += t0.elapsed();
                }
            });
            best = best.min(total);
        }
        println!(
            "{:<10} ticks {:>5} actors {:>4} checksum {:016x} host {:>6.1} us/tick",
            s.name,
            out.ticks,
            out.actors,
            out.checksum,
            best.as_secs_f64() * 1e6 / out.ticks.max(1) as f64
        );
    }
}
