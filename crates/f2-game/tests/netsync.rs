//! Rollback support on real data (`src/netsync.rs`, `docs/NETPLAY.md` §6): a snapshot taken
//! mid-level and resimulated with the same pads reproduces the exact state (checksum and
//! oracle trace every tick), and snapshot/restore/checksum costs are reported.
//! Reads `F2_GAME_DIR` (default `game-data/usa/game`); skips when absent.

use std::path::PathBuf;
use std::time::Instant;

use f2_game::assets::DirData;
use f2_game::define::mode::INGAME_MODE;
use f2_game::define::{INVALID_MODE, MULTILOCAL};
use f2_game::levplay::*;
use f2_game::multi::MULTIMODE_RACE;
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

/// Scripted pads (xorshift32), a press on about every 6th frame per player.
fn pads(seed: u32, ticks: usize, n: usize) -> Vec<TickInput> {
    const BUTTONS: [u16; 8] = [PAD_UP, PAD_UP, PAD_RIGHT, PAD_LEFT, PAD_DOWN, PAD_CROSS, PAD_CIRCLE, PAD_SQUARE];
    let mut s = seed | 1;
    (0..ticks)
        .map(|_| {
            let mut input = TickInput::default();
            for p in input.pads.iter_mut().take(n) {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                if s.is_multiple_of(6) {
                    *p = BUTTONS[(s >> 8) as usize % BUTTONS.len()];
                }
            }
            input
        })
        .collect()
}

/// The first race level (Ancients multi), as `(world, level)`.
pub fn race_level() -> (u32, u32) {
    use f2_game::bank_tables::collision_file_for_coll_bank;
    use f2_game::layout::ORIG_WORLD_VISUAL_DATA;
    for (w, wvd) in ORIG_WORLD_VISUAL_DATA.iter().enumerate() {
        if f2_game::multi::MULTI_GAME_TYPES.get(w) != Some(&MULTIMODE_RACE) {
            continue;
        }
        for l in 0..wvd.num_levels as usize {
            if collision_file_for_coll_bank(wvd.level_visual_data[l].coll_bank).is_some_and(|f| f.contains("MULTI")) {
                return (w as u32, l as u32);
            }
        }
    }
    panic!("no race level")
}

/// The first race level with 4 frogs.
fn multi_game(data: &DirData) -> Game {
    let mut g = Game::new();
    g.game_state.multi = MULTILOCAL;
    g.game_state.single = INVALID_MODE;
    g.game_state.mode = INGAME_MODE;
    g.frogger.num_frogs = 4;
    for i in 0..4 {
        g.frogger.player[i].character = i as u8;
    }
    g.multi.multiplayer_mode = MULTIMODE_RACE;
    let (w, l) = race_level();
    g.load_level(w, l, data).unwrap_or_else(|e| panic!("level {w}-{l}: {e}"));
    g.take_effects();
    g
}

#[test]
fn snapshot_restore_resimulate_is_exact() {
    let Some(dir) = game_dir() else { return };
    let data = DirData(dir);
    let input = pads(0xC0FFEE, 600, 4);

    // Reference: 600 ticks, recording checksum + trace after every tick.
    let mut g = multi_game(&data);
    let mut snap = None;
    let mut reference = Vec::new();
    for (t, i) in input.iter().enumerate() {
        if t == 300 {
            snap = Some(g.snapshot());
        }
        g.tick(*i);
        g.take_effects();
        reference.push((g.checksum(), g.trace_frame()));
    }
    let snap = snap.unwrap();
    eprintln!("after 600: mode {} started {} frogs {:?} afc {}", g.game_state.mode, g.multi.started, g.trace_frame().players.iter().map(|p| p.curr_tile).collect::<Vec<_>>(), g.act_frame_count);
    assert!(g.multi.started >= 1, "race never started");

    // Restore the snapshot into the advanced game and resimulate 300..600.
    g.restore(&snap);
    assert_eq!(g.act_frame_count, snap.act_frame_count);
    for (t, i) in input.iter().enumerate().skip(300) {
        g.tick_resimulate(*i);
        assert!(g.effects.is_empty(), "resimulated tick {t} produced effects");
        let (cs, tr) = &reference[t];
        assert_eq!(g.trace_frame(), *tr, "trace differs at tick {t}");
        assert_eq!(g.checksum(), *cs, "checksum differs at tick {t}");
    }

    // A clone ticked in lockstep with its original stays identical (nothing outside `Game`).
    let mut a = multi_game(&data);
    let mut b = a.clone();
    for (t, i) in input.iter().enumerate().take(120) {
        a.tick(*i);
        b.tick(*i);
        assert_eq!(a.take_effects(), b.take_effects(), "effects differ at tick {t}");
        assert_eq!(a.checksum(), b.checksum(), "clone diverged at tick {t}");
    }

    // A different pad changes the state (the checksum covers the frogs).
    let mut c = snap.clone();
    let mut d = snap.clone();
    for (i, j) in input.iter().zip(pads(0xBEEF, 60, 4)) {
        c.tick(*i);
        d.tick(j);
    }
    assert_ne!(c.checksum(), d.checksum());
}

/// Snapshot / restore / checksum timings (reported; `--release --nocapture` for real numbers).
#[test]
fn snapshot_cost() {
    let Some(dir) = game_dir() else { return };
    let data = DirData(dir);
    let mut g = multi_game(&data);
    for i in pads(7, 300, 4) {
        g.tick(i);
        g.take_effects();
    }
    let n = 200;
    let t = Instant::now();
    let mut snaps = Vec::with_capacity(8);
    for k in 0..n {
        let s = g.snapshot();
        if k < 8 {
            snaps.push(s);
        }
    }
    let snap_ms = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
    let t = Instant::now();
    for k in 0..n {
        g.restore(&snaps[k % snaps.len()]);
    }
    let restore_ms = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
    let t = Instant::now();
    let mut x = 0;
    for _ in 0..20 {
        x ^= g.checksum();
    }
    let cs_ms = t.elapsed().as_secs_f64() * 1000.0 / 20.0;
    let input = pads(9, 60, 4);
    let t = Instant::now();
    for i in &input {
        g.tick_resimulate(*i);
    }
    let tick_ms = t.elapsed().as_secs_f64() * 1000.0 / input.len() as f64;
    eprintln!(
        "netsync cost ({}): snapshot {snap_ms:.3} ms, restore {restore_ms:.3} ms, checksum {cs_ms:.3} ms, tick {tick_ms:.3} ms (x={x:x})",
        if cfg!(debug_assertions) { "debug" } else { "release" }
    );
}
