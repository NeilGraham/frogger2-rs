//! Real-data tests of the game flow (wave F1): boot STARTUP -> FRONTEND, arcade completion
//! of every single-player level with progression checks, and story progression.
//! Reads `F2_GAME_DIR` (default `game-data/usa/game`); skips when absent.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use f2_game::assets::{DataSource, DirData};
use f2_game::bank_tables::collision_file_for_coll_bank;
use f2_game::define::mode::*;
use f2_game::frontend::*;
use f2_game::game::NO_MULTI_LEV;
use f2_game::language::str_id::STR_GAMEOVER;
use f2_game::layout::{worldid, LEVEL_OPEN, ORIG_WORLD_VISUAL_DATA, WORLD_OPEN};
use f2_game::levplay::{PAD_CROSS, PAD_DOWN};
use f2_game::session::SessionConfig;
use f2_game::story::STORY_SEQUENCE;
use f2_game::{Effect, Game, TickInput};

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

fn session(dir: &Path) -> Game {
    Game::new_session(SessionConfig::new(Arc::new(DirData(dir.to_path_buf()))))
}

fn tick(g: &mut Game, pad: u16) -> Vec<Effect> {
    g.tick(TickInput { pads: [pad, 0, 0, 0] });
    g.take_level_loads();
    g.take_effects()
}

#[test]
fn boot_startup_to_frontend_2000_ticks() {
    let Some(dir) = game_dir() else { return };
    let mut g = session(&dir);
    assert_eq!(g.game_state.mode, STARTUP_MODE);
    assert!(g.gamestring(STR_GAMEOVER).len() > 2, "game text loaded");
    let mut videos = vec![];
    let mut loads = 0;
    for _ in 0..2000 {
        g.tick(TickInput::default());
        loads += g.take_level_loads().len();
        for e in g.take_effects() {
            if let Effect::PlayVideo { name } = e {
                videos.push(name);
            }
        }
    }
    assert_eq!(videos, ["HASBROS", "BLITZS", "01S"]);
    assert_eq!(g.game_state.mode, FRONTEND_MODE);
    assert_eq!(loads, 1);
    assert_eq!(g.frogger.player[0].world_num as u32, worldid::FRONTEND);
    assert!(g.frogger.frog[0].is_some());
}

/// Ends the current level the way the game does: all babies saved (RunGameLoop's
/// end-level timer) or, for levels without babies, the script's EV_ENDLEVEL.
fn finish_level(g: &mut Game) {
    let w = g.frogger.player[0].world_num as usize;
    let l = g.frogger.player[0].level_num as usize;
    if g.map.num_babies > 0 && g.layout.world_visual_data[w].level_visual_data[l].multi_part_level == NO_MULTI_LEV {
        g.babyfrog.babies_saved = g.map.num_babies as u32;
    } else {
        g.script_end_level();
    }
    let mut n = 0;
    while g.game_state.mode != LEVELCOMPLETE_MODE {
        tick(g, 0);
        n += 1;
        assert!(n < 1000, "level {w}-{l} never completed (mode {})", g.game_state.mode);
    }
}

/// Runs the level-complete screen to its menu (typing a name if a record was set), then
/// picks option `opt` (0: restart / continue, 1: quit). Returns whether the game saved.
fn run_level_complete(g: &mut Game, opt: i32) -> bool {
    let mut n = 0;
    while g.frontend.lev_complete_state != LEV_COMPLETE_MENU || !g.frontend.o_text[0].as_ref().is_some_and(|o| o.x_pos == o.x_pos_to) {
        if g.frontend.lev_complete_state == LEV_COMPLETE_ENTER_NAME {
            g.name_entry_key(Some(b'T'));
            g.name_entry_key(None);
        }
        tick(g, 0);
        n += 1;
        assert!(n < 3000, "stuck in level-complete state {}", g.frontend.lev_complete_state);
    }
    if opt == 1 {
        tick(g, PAD_DOWN);
        tick(g, 0);
    }
    let saved = tick(g, PAD_CROSS).iter().any(|e| matches!(e, Effect::SaveGame { .. }));
    assert_eq!(g.frontend.lev_complete_state, LEV_COMPLETE_FADING_OUT);
    let mut n = 0;
    while g.game_state.mode == LEVELCOMPLETE_MODE {
        tick(g, 0);
        n += 1;
        assert!(n < 200, "fade out never finished");
    }
    saved
}

#[test]
fn arcade_complete_every_single_player_level() {
    let Some(dir) = game_dir() else { return };
    let data = DirData(dir.clone());
    let mut done = 0;
    for (w, wvd) in ORIG_WORLD_VISUAL_DATA.iter().enumerate() {
        if w as u32 == worldid::FRONTEND {
            continue;
        }
        for l in 0..wvd.num_single_player_levels as usize {
            match collision_file_for_coll_bank(wvd.level_visual_data[l].coll_bank) {
                Some(f) if data.read(f).is_some() => {}
                _ => continue,
            }
            let mut g = session(&dir);
            g.start_frontend();
            g.take_level_loads();
            g.start_arcade(w as u32, l as u32);
            let loads = g.take_level_loads();
            assert_eq!(loads.len(), 1, "level {w}-{l} loaded");
            assert_eq!(g.game_state.mode, INGAME_MODE);
            assert_eq!(g.frogger.player[0].lives, NUM_LIVES[1]);
            for _ in 0..30 {
                tick(&mut g, 0);
            }
            let was_completed = g.layout.world_visual_data[w].level_visual_data[l].level_completed;
            finish_level(&mut g);
            assert_eq!(g.layout.world_visual_data[w].level_visual_data[l].level_completed, 1, "{w}-{l}");
            assert_eq!(g.frontend.level_opened, (was_completed == 0) as i32);
            assert_eq!(g.game_state.story_sequence_level, 0);
            let saved = run_level_complete(&mut g, 1);
            assert!(saved, "{w}-{l}: first completion saves");
            assert_eq!(g.game_state.mode, FRONTEND_MODE, "{w}-{l}: quit goes to the frontend");
            assert_eq!(g.frogger.player[0].world_num as u32, worldid::FRONTEND);
            // The save block records the completion (worlds 0..MAX_WORLDS-1; not WORLDID_TEST).
            let block = g.make_save_game_block();
            let mut h = Game::new();
            assert_eq!(h.load_save_game_block(&block), 1);
            let expect = (w as u32 != worldid::TEST) as u16;
            assert_eq!(expect, h.layout.world_visual_data[w].level_visual_data[l].level_completed, "{w}-{l} save");
            done += 1;
        }
    }
    eprintln!("completed {done} levels");
    assert!(done >= 20, "only {done} levels");
}

#[test]
fn arcade_restart_reloads_same_level() {
    let Some(dir) = game_dir() else { return };
    let mut g = session(&dir);
    g.start_arcade(worldid::GARDEN, 0);
    g.take_level_loads();
    finish_level(&mut g);
    run_level_complete(&mut g, 0);
    assert_eq!(g.game_state.mode, INGAME_MODE);
    assert_eq!((g.frogger.player[0].world_num as u32, g.frogger.player[0].level_num), (worldid::GARDEN, 0));
}

#[test]
fn story_progression_unlocks_next_level() {
    let Some(dir) = game_dir() else { return };
    let mut g = session(&dir);
    g.start_story(1);
    let effects = g.take_effects();
    assert!(effects.contains(&Effect::PlayVideo { name: "02S".into() }), "story FMV before Garden 1");
    assert_eq!(g.take_level_loads().len(), 1);
    finish_level(&mut g);
    assert_eq!(g.game_state.story_sequence_level, 2);
    let next = STORY_SEQUENCE[2];
    assert_eq!(g.layout.world_visual_data[next.world_num as usize].world_open, WORLD_OPEN);
    assert_eq!(g.layout.world_visual_data[next.world_num as usize].level_visual_data[next.level_num as usize].level_open, LEVEL_OPEN);
    run_level_complete(&mut g, 0);
    assert_eq!(g.game_state.mode, INGAME_MODE);
    assert_eq!((g.frogger.player[0].world_num as u32, g.frogger.player[0].level_num as u32), (next.world_num, next.level_num));
    assert_eq!(g.frogger.player[0].character, next.character, "StoryStartLevel");
}
