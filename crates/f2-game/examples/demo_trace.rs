//! Runs an attract-mode demo like the retail oracle (`tools/run/run.py --trace --demo W-L`)
//! and writes the per-tick trace (`tools/f2-trace` schema) as JSON lines.
//!
//! `cargo run -p f2-game --example demo_trace -- <world> <level> out.jsonl [max_ticks]`
//!
//! `F2_TICK_FRAMES=n` (phase 3T): `Game::tick_timed` with an n-frame clock instead of the
//! fixed-step `Game::tick` (n = 1 is trace-identical).
//!
//! Game data: `F2_GAME_DIR` (default `game-data/usa/game`; `.../usa` is accepted).

use std::io::Write;
use std::path::PathBuf;

use f2_game::assets::DirData;
use f2_game::define::mode::DEMO_MODE;
use f2_game::{Game, TickInput};

/// `rseed` when the oracle's frontend starts the demo (`InitDemoMode` -> `InitLevel`).
const ORACLE_RSEED_AT_DEMO_START: u32 = 144420441;

fn game_dir() -> PathBuf {
    let p = std::env::var("F2_GAME_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("game-data/usa/game"));
    if p.join("game").join("collision").is_dir() {
        p.join("game")
    } else {
        p
    }
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        return Err("usage: demo_trace <world> <level> out.jsonl [max_ticks]".into());
    }
    let world: u32 = args[0].parse().map_err(|e| format!("world: {e}"))?;
    let level: u32 = args[1].parse().map_err(|e| format!("level: {e}"))?;
    let out = PathBuf::from(&args[2]);
    let max_ticks: u32 = args.get(3).map_or(Ok(100_000), |s| s.parse()).map_err(|e| format!("max_ticks: {e}"))?;

    let data = DirData(game_dir());
    let mut g = Game::new();
    g.rng.rseed = ORACLE_RSEED_AT_DEMO_START;
    // Frontend leftovers the level never resets: GameLoop's STARTUP_MODE case ends with
    // GTInit(&modeTimer, 1) (Common/frontend.c:187) and the frontend never updates it.
    g.game.mode_timer.init(1);
    g.init_demo_level(world, level, &data)?;
    // InitDemoMode ran inside the frontend's GameLoop, which then does frameCount++.
    g.frame_count += 1;
    // ... then LoopFunc's CopyActorList + DrawLoop of that frontend iteration.
    g.after_game_loop();
    if std::env::var_os("F2_TRACE_DEBUG").is_some() {
        eprintln!("rseed after init: {}", g.rng.rseed);
    }

    let mut w = std::io::BufWriter::new(std::fs::File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?);
    let tick_frames: Option<u32> = std::env::var("F2_TICK_FRAMES").ok().and_then(|s| s.parse().ok());
    let mut n = 0u32;
    while n < max_ticks {
        if let Ok(t) = std::env::var("F2_RANDOM_LOG") {
            let on = t.split(',').any(|x| x.parse::<u32>().ok() == Some(n + 1));
            f2_math::maths::RANDOM_LOG.store(on, std::sync::atomic::Ordering::Relaxed);
            if on {
                eprintln!("--- tick {}", n + 1);
            }
        }
        // The oracle writes its line right after GameLoop, before CopyActorList/DrawLoop.
        match tick_frames {
            // Phase 3T: the retail variable timestep with an n-frame clock (n = 1 must give
            // the same trace as `tick_game_loop`).
            Some(k) => {
                g.advance_timing_frames(k);
                g.tick_timed_game_loop(TickInput::default());
            }
            None => g.tick_game_loop(TickInput::default()),
        }
        if g.game_state.mode != DEMO_MODE || g.levplay.demo_finished {
            break;
        }
        if let Ok(t) = std::env::var("F2_HEAD_DBG") {
            if t.split(',').any(|x| x.parse::<u32>().ok() == Some(n + 1)) {
                eprintln!("tick {} head {:?}", n + 1, g.tongue.head_matrix);
                if let Some(f) = g.frogger.frog[0] {
                    let a = &g.actor2.actors[f].actor;
                    let an = a.actual_actor.animation.as_ref().map(|x| (x.current_animation, x.anim_time, x.animation_speed));
                    eprintln!("  q {:?} place {:?} anim {:?}", a.q_rot, a.actual_actor.placement, an);
                }
            }
        }
        let mut f = g.trace_frame();
        f.seq = n;
        writeln!(w, "{}", f.to_json_line()).map_err(|e| e.to_string())?;
        n += 1;
        g.after_game_loop();
        g.take_effects();
    }
    w.flush().map_err(|e| e.to_string())?;
    eprintln!("{n} ticks -> {}", out.display());
    Ok(())
}
