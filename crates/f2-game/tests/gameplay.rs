//! Real-data gameplay tests (wave C): every loadable level runs 1800 ticks with a
//! scripted pad pattern, and the Garden 1 attract demo replays to completion.
//! Reads `F2_GAME_DIR` (default `game-data/usa/game`); skips when absent.

use std::path::PathBuf;

use f2_formats::entity::{Entities, ENTITY_VERSION};
use f2_game::assets::{DataSource, DirData};
use f2_game::bank_tables::collision_file_for_coll_bank;
use f2_game::define::mode::{DEMO_MODE, INGAME_MODE};
use f2_game::define::{ARCADE_MODE, SINGLEPLAYER};
use f2_game::layout::{entity_file_name, worldid, ORIG_WORLD_VISUAL_DATA};
use f2_game::levplay::*;
use f2_game::{Game, TickInput};

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

/// Pad pattern: a press every 12 frames cycling through hops, superhop, tongue, croak
/// (held 3 frames), with an occasional held direction.
fn pattern(t: u32) -> u16 {
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

#[test]
fn every_level_plays_1800_ticks() {
    let Some(dir) = game_dir() else { return };
    let data = DirData(dir);
    let mut played = 0;
    for (w, wvd) in ORIG_WORLD_VISUAL_DATA.iter().enumerate() {
        if w as u32 == worldid::FRONTEND {
            continue;
        }
        for l in 0..wvd.num_levels as usize {
            let lvd = &wvd.level_visual_data[l];
            match collision_file_for_coll_bank(lvd.coll_bank) {
                Some(f) if data.read(f).is_some() => {}
                _ => continue,
            }
            let ent = entity_file_name(wvd.world_id, lvd.level_id);
            if data.read(&ent).as_deref().and_then(Entities::version).is_some_and(|v| v != ENTITY_VERSION) {
                continue;
            }
            let mut g = Game::new();
            g.game_state.multi = SINGLEPLAYER;
            g.game_state.single = ARCADE_MODE;
            g.game_state.mode = INGAME_MODE;
            g.frogger.player[0].lives = 3;
            g.load_level(w as u32, l as u32, &data).unwrap_or_else(|e| panic!("level {w}-{l}: {e}"));
            // Enough lives that the pattern never reaches GAMEOVER_MODE (which stops the loop).
            g.frogger.player[0].lives = 100;
            let start = g.frog_position(0);
            let mut moved = false;
            for t in 0..1800u32 {
                g.tick(TickInput { pads: [pattern(t), 0, 0, 0] });
                moved |= g.frog_position(0) != start;
                g.take_effects();
            }
            eprintln!(
                "{w}-{l}: mode {} lives {} tile {:?} garibs {}/{} babies {} moved {moved}",
                g.game_state.mode,
                g.frogger.player[0].lives,
                g.frogmove.curr_tile[0],
                g.collect.garib_list.count,
                g.collect.garib_list.total,
                g.babyfrog.babies_saved
            );
            played += 1;
        }
    }
    assert!(played >= 30, "only {played} levels played");
}

#[test]
fn garden1_demo_replays_to_completion() {
    let Some(dir) = game_dir() else { return };
    let data = DirData(dir);
    if data.read("DEMOS\\RECORD-0-0.KEY").is_none() {
        eprintln!("skipping: no demo file");
        return;
    }
    let mut g = Game::new();
    g.rng.rseed = 144420441;
    g.init_demo_level(0, 0, &data).unwrap();
    g.frame_count += 1;
    assert_eq!(g.game_state.mode, DEMO_MODE);
    let garibs = g.collect.garib_list.count;
    let start_tile = g.frogmove.curr_tile[0];
    let mut ticks = 0;
    while !g.levplay.demo_finished {
        g.tick(TickInput::default());
        g.take_effects();
        ticks += 1;
        assert!(ticks < 5000, "demo did not finish");
    }
    // The demo ends after its key list (or the 25 s timeout) plus the 30-frame fade.
    assert!(ticks > 1000, "demo ended after {ticks} ticks");
    assert_ne!(g.frogmove.curr_tile[0], start_tile);
    assert!(g.collect.garib_list.count < garibs, "no garib collected");
}

/// Deaths (dead-timer starts) replaying the Garden 1 attract demo in ticks of `k` 60 Hz
/// frames, and the frog's tile after the replay.
fn garden1_demo_deaths(data: &DirData, k: u32) -> (u32, u32) {
    let mut g = Game::new();
    g.rng.rseed = 144420441;
    g.init_demo_level(0, 0, data).unwrap();
    g.frame_count += 1;
    let (mut deaths, mut was_dead, mut ticks) = (0, false, 0);
    while !g.levplay.demo_finished && ticks < 5000 {
        if k == 1 {
            g.tick(TickInput::default());
        } else {
            g.tick_frames(TickInput::default(), k);
        }
        g.take_effects();
        let dead = g.frogger.player[0].dead.time != 0;
        if dead && !was_dead {
            deaths += 1;
        }
        was_dead = dead;
        ticks += 1;
    }
    (deaths, g.frogmove.curr_tile[0].map_or(u32::MAX, |t| t.0))
}

/// The recorded demo pads are per 60 Hz frame: replayed in 60 Hz steps Frogger does not die;
/// multi-frame steps (a console's variable timestep) drift off the path, so hosts must
/// replay demos in fixed steps (frogger-n64 `App::variable_step`).
#[test]
fn garden1_demo_needs_fixed_steps() {
    let Some(dir) = game_dir() else { return };
    let data = DirData(dir);
    if data.read("DEMOS\\RECORD-0-0.KEY").is_none() {
        eprintln!("skipping: no demo file");
        return;
    }
    let fixed = garden1_demo_deaths(&data, 1);
    eprintln!("garden 1 demo, 60 Hz steps: {fixed:?} (deaths, final tile)");
    assert_eq!(fixed.0, 0, "fixed 60 Hz steps");
    for k in [2, 3, 4] {
        eprintln!("garden 1 demo, {k} frames per tick: {:?}", garden1_demo_deaths(&data, k));
    }
}
