//! Starts an arcade level like the retail oracle's scenario mode
//! (`tools/run/run.py --trace --level W-L --keys FILE.key`, `crates/f2fix/src/win/oracle.rs`)
//! and writes the per-tick trace (`tools/f2-trace` schema) as JSON lines.
//!
//! ```text
//! cargo run -p f2-game --example scenario_trace -- <world> <level> <keys.key|-> out.jsonl [max_ticks]
//! cargo run -p f2-game --example scenario_trace -- --list-levels
//! ```
//!
//! `--list-levels` prints `W-L NAME` for every single-player level of the PC level table
//! (`ORIG_WORLD_VISUAL_DATA`, worlds other than FRONTEND/TEST). Used by `tools/f2-parity`.
//!
//! Start (mirrors f2fix `h_game_loop` with `start.level`): at the frontend iteration where
//! the start fires, f2fix writes `gameState = {multi 1, single ARCADE, mode INGAME,
//! difficulty NORMAL}`, replaces that iteration's `GameLoop` by `InitLevel(W, L)` and does
//! `frameCount++`; `LoopFunc` then runs `CopyActorList` + `DrawLoop`.
//!
//! Input (f2fix `keys::Player`): before each `GameLoop` of the level, `padData.digital[p]`
//! = buttons of the last record with `frame < actFrameCount`, `debounce = ~prev & digital`.
//!
//! Game data: `F2_GAME_DIR` (default `game-data/usa/game`; `.../usa` is accepted).
//! `F2_START_RSEED` overrides the `rseed` at the start (default: the oracle's logged value).
//!
//! Local multiplayer (f2fix `start.multi`, `run.py --level W-L --multi N`):
//!
//! ```text
//! scenario_trace --multi N [--chars a,b,c,d] [--mp-mode M] <world> <level> <keys|-> out.jsonl [max_ticks]
//! scenario_trace --list-multi
//! ```
//!
//! The start mirrors the frontend's `GoMulti` (Common/options.c:950) as f2fix calls it:
//! `options.{numPlayers, currentPlayer} = N`, `playerChar[] = chars` (default 0,1,2,3),
//! `levelSelection` = the level's index in `multiWorldNum/multiLevelNum`; `fog.max =
//! fogStore` (the oracle's frontend value, `F2_FOG_STORE`, default 10000), `NUM_FROGS = N`,
//! `gameState = {MULTILOCAL, INVALID_MODE, INGAME}` (difficulty: the frontend's NORMAL),
//! `multiplayerMode = multiGameTypes[world]` (or `--mp-mode`), characters, `InitLevel`.
//! Each line then carries the `mp` section (multi.c globals, `mpl[]`) while
//! `gameState.multi == MULTILOCAL`. `--list-multi` prints `W-L MODE` for the table.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use f2_game::assets::DirData;
use f2_game::define::mode::FRONTEND_MODE;
use f2_game::define::{ARCADE_MODE, DIFFICULTY_NORMAL, INVALID_MODE, MULTILOCAL, SINGLEPLAYER};
use f2_game::layout::{worldid, ORIG_WORLD_VISUAL_DATA};
use f2_game::multi::MULTI_GAME_TYPES;
use f2_game::options::{MULTI_LEVEL_NUM, MULTI_WORLD_NUM};
use f2_game::{Game, TickInput};
use f2_trace::{MpPlayerState, MpState};

/// `rseed` when f2fix fires `start.level` (frontend `frameCount` 33, before `InitLevel`):
/// "oracle: start level W-L (..., rseed before InitLevel N)" in `f2fix.log`.
const ORACLE_RSEED_AT_LEVEL_START: u32 = 144420441;

fn game_dir() -> PathBuf {
    let p = std::env::var("F2_GAME_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("game-data/usa/game"));
    if p.join("game").join("collision").is_dir() {
        p.join("game")
    } else {
        p
    }
}

/// `.key` records `(frame, buttons, player)` (PC/controll.cpp `RecordButtons`).
fn parse_keys(b: &[u8]) -> Result<Vec<(u32, u16, usize)>, String> {
    if b.len() % 12 != 0 {
        return Err(format!("key file length {} is not a multiple of 12", b.len()));
    }
    let w = |c: &[u8], k: usize| u32::from_le_bytes([c[k], c[k + 1], c[k + 2], c[k + 3]]);
    b.chunks_exact(12)
        .map(|c| {
            let p = w(c, 8);
            if p >= 4 {
                return Err(format!("player {p} >= 4"));
            }
            Ok((w(c, 0), w(c, 4) as u16, p as usize))
        })
        .collect()
}

/// Held-state replay, as f2fix `keys::Player::step`.
struct KeyPlayer {
    records: Vec<(u32, u16, usize)>,
    next: usize,
    held: [u16; 4],
}

impl KeyPlayer {
    fn step(&mut self, act_frame_count: u32) -> ([u16; 4], [u16; 4]) {
        let prev = self.held;
        while let Some(&(frame, buttons, p)) = self.records.get(self.next) {
            if frame >= act_frame_count {
                break;
            }
            self.held[p] = buttons;
            self.next += 1;
        }
        (prev, self.held)
    }
}

fn list_levels() {
    for w in ORIG_WORLD_VISUAL_DATA.iter() {
        if w.world_id == worldid::FRONTEND || w.world_id == worldid::TEST {
            continue;
        }
        for l in 0..w.num_single_player_levels as usize {
            let lv = &w.level_visual_data[l];
            let name: String = lv.tex_crc.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect();
            println!("{}-{} {}", w.world_id, l, name);
        }
    }
}

/// Local multiplayer start options (`--multi`, `--chars`, `--mp-mode`).
struct MultiOpts {
    players: u32,
    chars: [u8; 4],
    mp_mode: i32,
}

/// `--list-multi`: the frontend's multiplayer level table (`multiWorldNum/multiLevelNum`)
/// with each level's `multiGameTypes` mode.
fn list_multi() {
    for i in 0..MULTI_WORLD_NUM.len() {
        let w = MULTI_WORLD_NUM[i];
        let mode = match MULTI_GAME_TYPES[w as usize] {
            1 => "race",
            2 => "battle",
            3 => "collect",
            _ => "none",
        };
        println!("{}-{} {}", w, MULTI_LEVEL_NUM[i], mode);
    }
}

/// The `mp` trace section from `multi.c`'s globals (f2fix `read_mp`).
fn mp_section(g: &Game) -> MpState {
    let m = &g.multi;
    let n = g.frogger.num_frogs.clamp(0, 4) as usize;
    MpState {
        mode: m.multiplayer_mode,
        started: m.started,
        num_multi_items: m.num_multi_items,
        game_winner: m.game_winner,
        match_winner: m.match_winner,
        multi_timer: m.multi_timer.time,
        end_timer: m.end_timer.time,
        initial_cam_height: m.initial_cam_height,
        mpl: m.mpl[..n]
            .iter()
            .map(|p| MpPlayerState {
                wins: p.wins,
                lap: p.lap,
                timer: p.timer,
                penalty: p.penalty,
                check: p.check,
                score: p.score,
                trail: p.trail,
                ready: p.ready,
                lasttile: p.lasttile,
            })
            .collect(),
    }
}

fn main() -> Result<(), String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--list-levels") {
        list_levels();
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("--list-multi") {
        list_multi();
        return Ok(());
    }
    let mut mp = MultiOpts { players: 0, chars: [0, 1, 2, 3], mp_mode: 0 };
    while args.first().is_some_and(|a| a.starts_with("--")) {
        let flag = args.remove(0);
        if args.is_empty() {
            return Err(format!("{flag} needs a value"));
        }
        let val = args.remove(0);
        match flag.as_str() {
            "--multi" => mp.players = val.parse().ok().filter(|n| (2..=4).contains(n)).ok_or("--multi takes 2..4")?,
            "--chars" => {
                for (i, c) in val.split(',').take(4).enumerate() {
                    mp.chars[i] = c.trim().parse().map_err(|e| format!("--chars: {e}"))?;
                }
            }
            "--mp-mode" => mp.mp_mode = val.parse().ok().filter(|m| (0..=3).contains(m)).ok_or("--mp-mode takes 0..3")?,
            _ => return Err(format!("unknown option {flag}")),
        }
    }
    if args.len() < 4 {
        return Err("usage: scenario_trace [--multi N [--chars a,b,..] [--mp-mode M]] <world> <level> <keys.key|-> out.jsonl [max_ticks] | --list-levels | --list-multi".into());
    }
    let world: u32 = args[0].parse().map_err(|e| format!("world: {e}"))?;
    let level: u32 = args[1].parse().map_err(|e| format!("level: {e}"))?;
    let records = if args[2] == "-" { Vec::new() } else { parse_keys(&std::fs::read(&args[2]).map_err(|e| format!("{}: {e}", args[2]))?)? };
    let out = PathBuf::from(&args[3]);
    let max_ticks: u32 = args.get(4).map_or(Ok(100_000), |s| s.parse()).map_err(|e| format!("max_ticks: {e}"))?;
    let rseed: u32 = match std::env::var("F2_START_RSEED") {
        Ok(s) => s.parse().map_err(|e| format!("F2_START_RSEED: {e}"))?,
        Err(_) => ORACLE_RSEED_AT_LEVEL_START,
    };

    let data = Arc::new(DirData(game_dir()));
    let mut g = Game::new();
    // In-level InitLevel calls (restart, next level) need the data source.
    g.session.data = Some(data.clone());
    g.rng.rseed = rseed;
    // Frontend leftover (see demo_trace): STARTUP_MODE ends with GTInit(&modeTimer, 1).
    g.game.mode_timer.init(1);
    if mp.players >= 2 {
        // f2fix start_multi -> GoMulti (Common/options.c:950); difficulty stays the
        // frontend's (NORMAL in the oracle log).
        g.game_state.difficulty = DIFFICULTY_NORMAL;
        let n = mp.players as i32;
        g.options.num_players = n;
        g.options.current_player = n;
        for i in 0..4 {
            g.options.player_char[i] = mp.chars[i] as i8;
        }
        if let Some(i) = (0..MULTI_WORLD_NUM.len()).find(|&i| MULTI_WORLD_NUM[i] == world && MULTI_LEVEL_NUM[i] == level) {
            g.options.level_selection = i as i32;
        }
        let fog_store: i32 = std::env::var("F2_FOG_STORE").ok().and_then(|s| s.parse().ok()).unwrap_or(10000);
        g.menus.fog_store = fog_store;
        g.frogger.fog.max = fog_store as i16;
        g.frogger.num_frogs = n;
        g.last_act_frame_count = g.act_frame_count;
        g.frogger.player[0].world_num = world as u8;
        g.frogger.player[0].level_num = level as u8;
        g.game_state.mode = f2_game::define::mode::INGAME_MODE;
        g.game_state.single = INVALID_MODE;
        g.game_state.multi = MULTILOCAL;
        g.multi.multiplayer_mode = if mp.mp_mode != 0 { mp.mp_mode } else { MULTI_GAME_TYPES.get(world as usize).copied().unwrap_or(0) };
        for i in 0..mp.players as usize {
            g.frogger.player[i].character = mp.chars[i];
        }
    } else {
        g.game_state.multi = SINGLEPLAYER;
        g.game_state.single = ARCADE_MODE;
        g.game_state.mode = f2_game::define::mode::INGAME_MODE;
        g.game_state.difficulty = DIFFICULTY_NORMAL;
    }
    g.load_level(world, level, &*data)?;
    // f2fix: GameLoop's epilogue for the start iteration.
    g.frame_count += 1;
    // ... then LoopFunc's CopyActorList + DrawLoop of that frontend iteration.
    g.after_game_loop();
    let load_fx = g.take_effects();
    if std::env::var_os("F2_LOG").is_some() {
        for e in &load_fx {
            if let f2_game::effects::Effect::Log { text } = e {
                eprintln!("{text}");
            }
        }
    }
    g.take_level_loads();

    let mut keys = KeyPlayer { records, next: 0, held: [0; 4] };
    let mut w = std::io::BufWriter::new(std::fs::File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?);
    let mut n = 0u32;
    while n < max_ticks {
        // LoopFunc sets actFrameCount before the (hooked) GameLoop, where f2fix writes padData.
        let (prev, held) = keys.step(g.act_frame_count.wrapping_add(1));
        // process_user_input derives debounce = ~padData.digital & live.
        g.levplay.pad_data.digital = prev;
        g.tick_game_loop(TickInput { pads: held });
        if g.game_state.mode == FRONTEND_MODE {
            break; // f2fix: level left -> no line, run ends (exit.level_end)
        }
        let mut f = g.trace_frame();
        f.seq = n;
        if g.game_state.multi == MULTILOCAL {
            f.mp = Some(mp_section(&g));
        }
        writeln!(w, "{}", f.to_json_line()).map_err(|e| e.to_string())?;
        n += 1;
        g.after_game_loop();
        g.take_effects();
        g.take_level_loads();
    }
    w.flush().map_err(|e| e.to_string())?;
    eprintln!("{n} ticks -> {}", out.display());
    Ok(())
}
