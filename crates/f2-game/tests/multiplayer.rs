//! Real-data multiplayer tests (`Common/multi.c`): every multiplayer level loads with 2
//! and 4 local players in each mode (race, battle, collect) and runs 3600 ticks with
//! seeded pseudo-random pad input. The win screens belong to the frontend port, so the
//! harness plays its part: on `MULTI_WIN*_MODE` it starts the next round
//! (`ResetMultiplayer`) or a new match (`RestartMulti`) as `RunMultiWin*` would.
//! Reads `F2_GAME_DIR` (default `game-data/usa/game`); skips when absent.

use std::path::PathBuf;

use f2_formats::entity::{Entities, ENTITY_VERSION};
use f2_game::assets::{DataSource, DirData};
use f2_game::bank_tables::collision_file_for_coll_bank;
use f2_game::define::mode::{INGAME_MODE, MULTI_WINBATTLE_MODE, MULTI_WINCOLLECT_MODE, MULTI_WINRACE_MODE, PAUSE_MODE};
use f2_game::define::{INVALID_MODE, MULTILOCAL};
use f2_game::layout::{entity_file_name, ORIG_WORLD_VISUAL_DATA};
use f2_game::levplay::*;
use f2_game::multi::{MULTIMODE_BATTLE, MULTIMODE_COLLECT, MULTIMODE_RACE, MULTI_GAME_TYPES};
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

/// Multiplayer levels: those whose collision bank is a `*MULTI*` bank (not the
/// multiplayer select screen).
fn multi_levels(data: &DirData) -> Vec<(u32, u32, String)> {
    let mut out = Vec::new();
    for (w, wvd) in ORIG_WORLD_VISUAL_DATA.iter().enumerate() {
        for l in 0..wvd.num_levels as usize {
            let lvd = &wvd.level_visual_data[l];
            let Some(f) = collision_file_for_coll_bank(lvd.coll_bank) else { continue };
            if !f.contains("MULTI") || f.contains("MULTISEL") || data.read(f).is_none() {
                continue;
            }
            let ent = entity_file_name(wvd.world_id, lvd.level_id);
            if data.read(&ent).as_deref().and_then(Entities::version).is_some_and(|v| v != ENTITY_VERSION) {
                continue;
            }
            out.push((w as u32, l as u32, f.to_string()));
        }
    }
    out
}

/// xorshift32 for the scripted pads (independent of the game's `Random()`).
struct Pads(u32);

impl Pads {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }

    /// A press on roughly every 6th frame per player: mostly directions, sometimes
    /// superhop/tongue/croak. Never START (that opens the pause menu).
    fn tick(&mut self, n: usize) -> TickInput {
        const BUTTONS: [u16; 8] = [PAD_UP, PAD_UP, PAD_RIGHT, PAD_LEFT, PAD_DOWN, PAD_CROSS, PAD_CIRCLE, PAD_SQUARE];
        let mut input = TickInput::default();
        for p in input.pads.iter_mut().take(n) {
            let r = self.next();
            if r.is_multiple_of(6) {
                *p = BUTTONS[(r >> 8) as usize % BUTTONS.len()];
            }
        }
        input
    }
}

#[derive(Default, Debug)]
struct Outcome {
    rounds: u32,
    matches: u32,
    max_wins: i16,
    /// Highest `started` seen (2 = a round got running).
    started: i32,
    /// Best race progress (`check`, `lap`) and best collect/battle `score` seen.
    best_check: i16,
    best_lap: i16,
    best_score: i16,
    /// Distinct tiles frog 0 stood on.
    tiles_visited: usize,
    /// Battle: the trail cubes (`cube.obe`) exist.
    trail_cubes: bool,
}

fn play(data: &DirData, w: u32, l: u32, players: i32, mode: i32, seed: u32) -> Outcome {
    let mut g = Game::new();
    g.rng.rseed = seed;
    g.game_state.multi = MULTILOCAL;
    g.game_state.single = INVALID_MODE;
    g.game_state.mode = INGAME_MODE;
    g.frogger.num_frogs = players;
    for i in 0..players as usize {
        g.frogger.player[i].character = i as u8;
    }
    g.multi.multiplayer_mode = mode;
    g.load_level(w, l, data).unwrap_or_else(|e| panic!("level {w}-{l}: {e}"));

    let mut pads = Pads(seed | 1);
    let mut out = Outcome::default();
    let mut visited = std::collections::BTreeSet::new();
    for _ in 0..3600 {
        g.tick(pads.tick(players as usize));
        g.take_effects();
        visited.insert(g.frogmove.curr_tile[0]);
        out.started = out.started.max(g.multi.started);
        for m in g.multi.mpl.iter().take(players as usize) {
            out.best_check = out.best_check.max(m.check);
            out.best_lap = out.best_lap.max(m.lap);
            out.best_score = out.best_score.max(m.score);
        }
        match g.game_state.mode {
            INGAME_MODE => {}
            MULTI_WINRACE_MODE | MULTI_WINCOLLECT_MODE | MULTI_WINBATTLE_MODE => {
                // RunMultiWin* (frontend.c): next round, or a new match after a match win.
                out.rounds += 1;
                out.max_wins = out.max_wins.max(g.multi.mpl.iter().map(|m| m.wins).max().unwrap_or(0));
                if g.multi.match_winner != -1 {
                    out.matches += 1;
                    g.restart_multi();
                } else {
                    g.reset_multiplayer();
                }
                g.game_state.mode = INGAME_MODE;
            }
            PAUSE_MODE => g.game_state.mode = g.game_state.old_mode,
            m => panic!("level {w}-{l} mode {mode}: unexpected game mode {m}"),
        }
        assert!(g.multi.node_pool.count >= 0 && g.multi.node_pool.count as usize <= g.multi.node_pool.array.len());
        for i in 0..players as usize {
            assert!(g.frogmove.curr_tile[i].is_some(), "frog {i} lost its tile");
        }
    }
    out.tiles_visited = visited.len();
    out.trail_cubes = g.multi.node_pool.array.first().is_some_and(|n| n.fx.is_some());
    out
}

#[test]
fn multiplayer_levels_play_3600_ticks_in_every_mode() {
    let Some(dir) = game_dir() else { return };
    let data = DirData(dir);
    let levels = multi_levels(&data);
    assert!(levels.len() >= 8, "only {} multiplayer levels found", levels.len());
    let mut runs = 0;
    for (k, (w, l, coll)) in levels.iter().enumerate() {
        for players in [2, 4] {
            for mode in [MULTIMODE_RACE, MULTIMODE_BATTLE, MULTIMODE_COLLECT] {
                let seed = 0x9E37_79B9u32.wrapping_mul(k as u32 * 8 + players as u32 * 4 + mode as u32);
                let o = play(&data, *w, *l, players, mode, seed);
                eprintln!("{w}-{l} {coll} players {players} mode {mode}: {o:?}");
                // In the level's own mode (`multiGameTypes`) the round always gets going
                // (race: every frog reaches the start line by itself).
                if MULTI_GAME_TYPES[*w as usize] == mode {
                    assert_eq!(o.started, 2, "{w}-{l} mode {mode} never started");
                }
                if mode == MULTIMODE_BATTLE {
                    assert!(o.rounds > 0, "{w}-{l}: no battle round finished");
                }
                runs += 1;
            }
        }
    }
    assert_eq!(runs, levels.len() * 6);
}

#[test]
fn battle_rounds_finish_and_runs_are_deterministic() {
    let Some(dir) = game_dir() else { return };
    let data = DirData(dir);
    // The battle arena the multiplayer menu uses (Laboratory multi 1, `multiGameTypes`).
    let Some(&(w, l, _)) = multi_levels(&data).iter().find(|(w, _, c)| *w == 5 && c.contains("LABMULTI1")) else {
        eprintln!("skipping: no LABMULTI1");
        return;
    };
    let a = play(&data, w, l, 4, MULTIMODE_BATTLE, 1234);
    let b = play(&data, w, l, 4, MULTIMODE_BATTLE, 1234);
    eprintln!("battle {w}-{l}: {a:?}");
    assert!(a.rounds > 0, "no battle round finished in 3600 ticks");
    assert!(a.best_score > 0, "no trail score");
    assert!(a.trail_cubes, "cube.obe trail actors missing");
    assert_eq!((a.rounds, a.matches, a.max_wins), (b.rounds, b.matches, b.max_wins));
}

#[test]
fn collect_mode_baby_pickup_scores_and_respawns() {
    let Some(dir) = game_dir() else { return };
    let data = DirData(dir);
    // Super Retro multi 1: the collect arena (`multiGameTypes[WORLDID_SUPERRETRO]`).
    let Some(&(w, l, _)) = multi_levels(&data).iter().find(|(_, _, c)| c.contains("SRETROMULTI1")) else {
        eprintln!("skipping: no SRETROMULTI1");
        return;
    };
    let mut g = Game::new();
    g.game_state.multi = MULTILOCAL;
    g.game_state.single = INVALID_MODE;
    g.game_state.mode = INGAME_MODE;
    g.frogger.num_frogs = 2;
    g.frogger.player[1].character = 1;
    g.multi.multiplayer_mode = MULTIMODE_COLLECT;
    g.load_level(w, l, &data).unwrap();
    assert!(g.map.num_babies > 0, "collect level without babies");

    // countdown
    while g.multi.started != 2 {
        g.tick(TickInput::default());
        assert!(g.act_frame_count < 1000, "countdown never finished");
    }
    let start = g.frogmove.curr_tile[0];

    // put frog 0 on the first baby's tile
    let baby = g.babyfrog.baby_list[0].enemy.expect("baby enemy");
    let tile = g.enemies.enemy_list.array[baby.idx()].in_tile.expect("baby tile");
    let f = g.frogger.frog[0].expect("frog actor");
    g.teleport_actor_to_tile(f, tile, 0);
    for _ in 0..3 {
        g.tick(TickInput::default());
    }
    assert_eq!(g.multi.mpl[0].score, 1);
    assert_eq!(g.babyfrog.baby_list[0].is_saved, 1);
    // GTInit(dead, 2) clears lastCount, so the next GTUpdate ticks straight down to 1.
    assert_eq!(g.frogger.player[0].dead.time, 1);

    // the "dead" timer after a pickup sends the frog back to its start tile
    for _ in 0..200 {
        g.tick(TickInput::default());
    }
    assert_eq!(g.frogger.player[0].dead.time, 0);
    assert_eq!(g.frogmove.curr_tile[0], start);
}
