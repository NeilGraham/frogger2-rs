//! Real-data tests of the 3D frontend menus (wave G: menus.c / options.c): scripted pad
//! input drives the frog around the frontend level into story, arcade, options, the pause
//! menu and the cheat combos. Reads `F2_GAME_DIR` (default `game-data/usa/game`); skips
//! when absent.
//!
//! Frontend layout (collision tiles of FRONTEND1): the frog starts on tile 0
//! (`TILENUM_START`); left hops to the options tile (1), right to the choice tile (2);
//! from there up = book (5), left = multiplayer (3), right = arcade machine (4).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use f2_game::assets::DirData;
use f2_game::define::mode::*;
use f2_game::define::{ARCADE_MODE, STORY_MODE};
use f2_game::layout::{worldid, LEVELID_FRONTEND4, LEVEL_OPEN, WORLD_OPEN};
use f2_game::levplay::*;
use f2_game::menus::*;
use f2_game::options::*;
use f2_game::session::SessionConfig;
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

/// A fresh game in the frontend, with the training level done (as an existing save would
/// have it) so the story book starts at Garden 1 and the arcade machine is open.
fn frontend(dir: &Path) -> Game {
    let mut g = Game::new_session(SessionConfig::new(Arc::new(DirData(dir.to_path_buf()))));
    g.layout.world_visual_data[worldid::FRONTEND as usize].level_visual_data[LEVELID_FRONTEND4 as usize].level_completed = 1;
    g.start_frontend();
    g.take_level_loads();
    tick_n(&mut g, 0, 60);
    assert_eq!(g.game_state.mode, FRONTEND_MODE);
    assert_eq!(g.game.curr_tile_num, TILENUM_START);
    g
}

fn tick_n(g: &mut Game, pad: u16, n: usize) -> Vec<Effect> {
    let mut out = vec![];
    for _ in 0..n {
        g.tick(TickInput { pads: [pad, 0, 0, 0] });
        out.extend(g.take_effects());
        g.take_level_loads();
    }
    out
}

/// Presses `pad` for one tick, then waits `ticks` more.
fn press(g: &mut Game, pad: u16, ticks: usize) -> Vec<Effect> {
    let mut out = tick_n(g, pad, 1);
    out.extend(tick_n(g, 0, ticks));
    out
}

/// Ticks with no input until `cond` holds (at most `max` ticks).
fn wait(g: &mut Game, max: usize, cond: impl Fn(&Game) -> bool) -> bool {
    for _ in 0..max {
        if cond(g) {
            return true;
        }
        tick_n(g, 0, 1);
    }
    cond(g)
}

/// Hops to a frontend tile: one press, then wait for the landing.
fn hop(g: &mut Game, pad: u16, tile: u32) {
    press(g, pad, 1);
    assert!(wait(g, 200, |g| g.game.curr_tile_num == tile && g.frogger.player[0].can_jump != 0), "hop to tile {tile}: on {}", g.game.curr_tile_num);
    tick_n(g, 0, 5);
}

fn pause_combo(g: &mut Game, keys: &[u16]) {
    press(g, PAD_START, 2);
    assert_eq!(g.game_state.mode, PAUSE_MODE);
    tick_n(g, PAD_SQUARE, 1);
    for &k in keys.iter().take_while(|&&k| k != 0) {
        tick_n(g, PAD_SQUARE | k, 1);
        tick_n(g, PAD_SQUARE, 1);
    }
    tick_n(g, 0, 2);
}

#[test]
fn story_book_starts_garden1() {
    let Some(dir) = game_dir() else { return };
    let mut g = frontend(&dir);
    assert_eq!(g.options.max_page_allowed, 0);
    hop(&mut g, PAD_RIGHT, TILENUM_CHOICE);
    hop(&mut g, PAD_UP, TILENUM_BOOK);
    assert_eq!(g.options.mode, OP_BOOK);
    let fx = press(&mut g, PAD_CROSS, 1);
    assert!(fx.iter().any(|e| matches!(e, Effect::PlaySample { .. })), "BookSelect plays the superhop sample");
    assert!(wait(&mut g, 200, |g| g.game_state.mode == INGAME_MODE));
    assert_eq!(g.game_state.single, STORY_MODE);
    assert_eq!((g.frogger.player[0].world_num as u32, g.frogger.player[0].level_num), (worldid::GARDEN, 0));
    assert_eq!(g.game_state.story_sequence_level, 1);
    tick_n(&mut g, 0, 120);
    assert!(g.frogger.frog[0].is_some());
}

#[test]
fn arcade_level_and_character_select() {
    let Some(dir) = game_dir() else { return };
    let mut g = frontend(&dir);
    assert_eq!(g.menus.done_training, 1);
    hop(&mut g, PAD_RIGHT, TILENUM_CHOICE);
    hop(&mut g, PAD_RIGHT, TILENUM_ARCADE);
    assert_eq!(g.options.mode, OP_ARCADE);
    // input waits for the camera and the "Arcade mode" title
    assert!(wait(&mut g, 300, |g| g.check_cam_still() && g.textover.get(g.options.arcade_text).is_some_and(|t| t.a >= 200)));
    assert_eq!(g.menus.c_world, worldid::GARDEN);
    assert_eq!(g.options.level_num, 0);
    // Garden: level 0 = training, 1 = Garden 1
    press(&mut g, PAD_DOWN, 3);
    assert_eq!(g.options.level_num, 1);
    assert!(g.options.static_flash > 0, "the arcade screen flashes static");
    assert!(g.menus.arcade_screen_copy.ends_with("NOIS04"));
    assert!(!g.menus.level_str[1].is_empty(), "level list filled");
    press(&mut g, PAD_CROSS, 3);
    assert_eq!(g.options.mode, OP_SP_CHARSEL);
    let before = g.options.sp_char;
    press(&mut g, PAD_DOWN, 3);
    assert_ne!(g.options.sp_char, before);
    let ch = g.options.sp_char;
    assert!(g.frogger.frog_pool_active[ch as usize] != 0);
    press(&mut g, PAD_CROSS, 1);
    assert_eq!(g.menus.last_arcade, TILENUM_ARCADE);
    assert!(wait(&mut g, 200, |g| g.game_state.mode == INGAME_MODE));
    assert_eq!(g.game_state.single, ARCADE_MODE);
    assert_eq!((g.frogger.player[0].world_num as u32, g.frogger.player[0].level_num), (worldid::GARDEN, 0));
    assert_eq!(g.frogger.player[0].character as i32, ch);
}

#[test]
fn pause_quit_returns_to_frontend() {
    let Some(dir) = game_dir() else { return };
    let mut g = frontend(&dir);
    hop(&mut g, PAD_RIGHT, TILENUM_CHOICE);
    hop(&mut g, PAD_UP, TILENUM_BOOK);
    press(&mut g, PAD_CROSS, 1);
    assert!(wait(&mut g, 200, |g| g.game_state.mode == INGAME_MODE));
    tick_n(&mut g, 0, 60);

    let fx = press(&mut g, PAD_START, 1);
    assert_eq!(g.game_state.mode, PAUSE_MODE);
    assert!(fx.contains(&Effect::PauseAudio));
    assert!(g.textover.get(g.hud.continue_text).is_some_and(|t| t.draw != 0));
    // down twice: quit
    press(&mut g, PAD_DOWN, 1);
    press(&mut g, PAD_DOWN, 1);
    assert_eq!(g.menus.current_pause_selection, 2);
    press(&mut g, PAD_CROSS, 1);
    assert_eq!(g.menus.pause_confirm_mode, 1);
    assert_eq!(g.menus.current_pause_selection, 1, "confirm starts on 'no'");
    press(&mut g, PAD_UP, 1);
    assert_eq!(g.menus.current_pause_selection, 0);
    press(&mut g, PAD_CROSS, 1);
    assert!(wait(&mut g, 200, |g| g.game_state.mode == FRONTEND_MODE));
    assert_eq!(g.frogger.player[0].world_num as u32, worldid::FRONTEND);
    tick_n(&mut g, 0, 30);
    assert!(g.frogger.frog[0].is_some());

    // In the frontend, "quit" leaves the game (PostQuitMessage).
    press(&mut g, PAD_START, 1);
    assert_eq!(g.game_state.mode, PAUSE_MODE);
    press(&mut g, PAD_DOWN, 1);
    assert_eq!(g.menus.current_pause_selection, 1);
    press(&mut g, PAD_CROSS, 1);
    press(&mut g, PAD_UP, 1);
    let fx = press(&mut g, PAD_CROSS, 60);
    assert!(fx.contains(&Effect::Quit));
}

#[test]
fn cheat_combo_opens_all_levels() {
    let Some(dir) = game_dir() else { return };
    let mut g = frontend(&dir);
    assert!(g.layout.world_visual_data[worldid::SUBTERRANEAN as usize].level_visual_data[2].level_open != LEVEL_OPEN);
    pause_combo(&mut g, &CHEAT_COMBOS[CHEAT_OPEN_ALL_LEVELS].key_string);
    assert_eq!(g.game_state.mode, FRONTEND_MODE, "the combo closes the pause menu");
    assert_eq!(g.menus.cheat_combos[CHEAT_OPEN_ALL_LEVELS].state, 1);
    assert!(g.layout.world_visual_data.iter().all(|w| w.world_open == WORLD_OPEN));
    assert_eq!(g.layout.world_visual_data[worldid::SUBTERRANEAN as usize].level_visual_data[2].level_open, LEVEL_OPEN);
    assert_eq!(g.options.max_page_allowed, 9);
    // the cheat text fades in and out again
    assert!(g.menus.cheat_text.is_some());
    assert!(wait(&mut g, 400, |g| g.menus.cheat_text.is_none()));

    // all characters
    pause_combo(&mut g, &CHEAT_COMBOS[CHEAT_OPEN_ALL_CHARS].key_string);
    assert!(g.frogger.frog_pool_active.iter().all(|&a| a != 0));

    // the arcade machine now steps through the worlds
    hop(&mut g, PAD_RIGHT, TILENUM_CHOICE);
    hop(&mut g, PAD_RIGHT, TILENUM_ARCADE);
    assert!(wait(&mut g, 300, |g| g.check_cam_still() && g.textover.get(g.options.arcade_text).is_some_and(|t| t.a >= 200)));
    press(&mut g, PAD_RIGHT, 3);
    assert_eq!(g.menus.c_world, worldid::ANCIENT);
}

/// Presses `pad` on controller `pl` for one tick, then waits `ticks`.
fn press_on(g: &mut Game, pl: usize, pad: u16, ticks: usize) {
    let mut pads = [0u16; 4];
    pads[pl] = pad;
    g.tick(TickInput { pads });
    g.take_effects();
    tick_n(g, 0, ticks);
}

#[test]
fn multiplayer_setup_starts_a_race() {
    let Some(dir) = game_dir() else { return };
    let mut g = frontend(&dir);
    hop(&mut g, PAD_RIGHT, TILENUM_CHOICE);
    hop(&mut g, PAD_LEFT, TILENUM_MULTI);
    assert_eq!(g.options.mode, OP_MULTIPLAYERNUMBER);
    assert!(wait(&mut g, 300, |g| g.options.cam_still != 0));
    assert_eq!(g.options.num_players, 2);
    press(&mut g, PAD_CROSS, 2);
    assert_eq!(g.options.mode, OP_CHARSEL);
    // each player picks with their own pad (properMultiSelect)
    assert!(wait(&mut g, 300, |g| g.options.cam_still != 0));
    press_on(&mut g, 0, PAD_CROSS, 2);
    assert_eq!(g.options.current_player, 1);
    assert!(wait(&mut g, 300, |g| g.options.cam_still != 0));
    press_on(&mut g, 1, PAD_CROSS, 2);
    assert_eq!(g.options.current_player, 1, "Frogger is taken");
    press_on(&mut g, 1, PAD_RIGHT, 2);
    assert!(wait(&mut g, 300, |g| g.options.cam_still != 0));
    press_on(&mut g, 1, PAD_CROSS, 2);
    assert_eq!(g.options.player_char[..2], [0, 1]);
    assert_eq!(g.options.mode, OP_LEVELSEL);
    assert_eq!(g.options.portrait_gouraud[0], [32; 3]);
    assert!(wait(&mut g, 300, |g| g.options.cam_still != 0));
    press(&mut g, PAD_CROSS, 1);
    assert!(wait(&mut g, 200, |g| g.game_state.mode == INGAME_MODE));
    assert_eq!(g.frogger.num_frogs, 2);
    assert_eq!(g.game_state.multi, f2_game::define::MULTILOCAL);
    assert_eq!((g.frogger.player[0].world_num as u32, g.frogger.player[0].level_num), (MULTI_WORLD_NUM[0], MULTI_LEVEL_NUM[0] as u8));
    assert_eq!(g.frogger.player[1].character, 1);
    tick_n(&mut g, 0, 60);
}

#[test]
fn extras_difficulty_and_credits() {
    let Some(dir) = game_dir() else { return };
    let mut g = frontend(&dir);
    hop(&mut g, PAD_LEFT, TILENUM_OPTIONS);
    assert!(wait(&mut g, 300, |g| g.check_cam_still()));
    press(&mut g, PAD_LEFT, 2); // MenuLeft: selection 1 = extras
    assert_eq!(g.options.selection, OP_EXTRA);
    assert!(wait(&mut g, 300, |g| g.check_cam_still()));
    press(&mut g, PAD_CROSS, 2);
    assert_eq!(g.options.mode, OP_EXTRA);
    // the chest opens first
    assert!(wait(&mut g, 600, |g| g.options.chest_timer.time == 0));
    tick_n(&mut g, 0, 30);
    assert!(g.textover.get(g.options.extras[EXTRA_DIFFICULTY]).is_some_and(|t| t.draw != 0 && t.a > 0));

    let d = g.game_state.difficulty;
    press(&mut g, PAD_CROSS, 2);
    assert_eq!(g.game_state.difficulty, (d + 1) % 3);
    assert_eq!(g.textover.get(g.options.extras[EXTRA_DIFFICULTY]).unwrap().text, g.options.diff_str);

    press(&mut g, PAD_DOWN, 2);
    press(&mut g, PAD_DOWN, 2);
    assert_eq!(g.options.extra_selection, EXTRA_CREDITS as i32);
    press(&mut g, PAD_CROSS, 2);
    assert_eq!(g.options.credits_running, 1);
    assert!(g.options.credits_text[0].is_some());
    tick_n(&mut g, 0, 120);
    assert!(g.options.credits_y < 0, "credits scroll");
    press(&mut g, PAD_CROSS, 2);
    assert_eq!(g.options.ending_credits, 1);
    assert!(wait(&mut g, 300, |g| g.options.credits_running == 0));
}

#[test]
fn idle_frontend_runs_a_demo() {
    let Some(dir) = game_dir() else { return };
    let mut g = frontend(&dir);
    g.game_state.difficulty = 2;
    assert!(wait(&mut g, 2400, |g| g.game_state.mode == DEMO_MODE));
    assert_eq!(g.menus.old_diff_mode, 2);
    assert_eq!(g.game_state.difficulty, f2_game::define::DIFFICULTY_NORMAL);
}

#[test]
fn options_change_volumes() {
    let Some(dir) = game_dir() else { return };
    let mut g = frontend(&dir);
    hop(&mut g, PAD_LEFT, TILENUM_OPTIONS);
    assert_eq!(g.options.mode, OP_GLOBALMENU);
    assert!(wait(&mut g, 300, |g| g.check_cam_still()));
    assert_eq!(g.options.selection, OP_SOUND);
    press(&mut g, PAD_CROSS, 2);
    assert_eq!(g.options.mode, OP_SOUND);

    let mv = g.savegame.global_music_vol;
    let fx = tick_n(&mut g, PAD_LEFT, 5); // held: repeats every frame
    assert_eq!(g.savegame.global_music_vol, mv - 5);
    let vols: Vec<i32> = fx.iter().filter_map(|e| if let Effect::SetMusicVolume { vol, .. } = e { Some(*vol) } else { None }).collect();
    assert_eq!(vols, (mv - 5..mv).rev().collect::<Vec<_>>());
    tick_n(&mut g, 0, 2);

    press(&mut g, PAD_DOWN, 2);
    assert_eq!(g.options.sound_selection, 1);
    let sv = g.savegame.global_sound_vol;
    let fx = tick_n(&mut g, PAD_LEFT, 3);
    assert_eq!(g.savegame.global_sound_vol, sv - 3);
    assert_eq!(g.pcaudio.global_sound_vol, sv - 3);
    assert!(fx.iter().any(|e| matches!(e, Effect::SetSoundVolume { vol } if *vol == sv - 3)));
    tick_n(&mut g, 0, 2);
    let bar = g.overlays.get(g.options.sound_bar[1]).unwrap();
    assert_eq!(bar.width as i32, (sv - 3) * SOUND_SCALE);

    // back to the options menu, then off the tile
    press(&mut g, PAD_TRIANGLE, 2);
    assert_eq!(g.options.mode, OP_GLOBALMENU);
    assert!(wait(&mut g, 300, |g| g.check_cam_still()));
    press(&mut g, PAD_TRIANGLE, 1);
    assert!(wait(&mut g, 200, |g| g.game.curr_tile_num == TILENUM_START && g.frogger.player[0].can_jump != 0));
    assert_eq!(g.options.mode, -1);
}
