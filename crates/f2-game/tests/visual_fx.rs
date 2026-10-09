//! Real-data tests (wave E): special effects, sprites, overlays, HUD and procedural
//! textures while playing every loadable level for 1800 ticks.
//! Reads `F2_GAME_DIR` (default `game-data/usa/game`); skips when absent.

use std::path::PathBuf;

use f2_formats::entity::{Entities, ENTITY_VERSION};
use f2_game::assets::{DataSource, DirData};
use f2_game::bank_tables::collision_file_for_coll_bank;
use f2_game::define::mode::INGAME_MODE;
use f2_game::define::{ARCADE_MODE, SINGLEPLAYER};
use f2_game::layout::{entity_file_name, worldid, ORIG_WORLD_VISUAL_DATA};
use f2_game::levplay::*;
use f2_game::textover::strs;
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

fn pattern(t: u32) -> u16 {
    const SEQ: [u16; 8] = [PAD_UP, PAD_UP, PAD_RIGHT, PAD_DOWN, PAD_LEFT, PAD_CROSS, PAD_CIRCLE, PAD_SQUARE];
    let k = (t / 12) as usize;
    if t % 12 < 3 {
        SEQ[k % SEQ.len()]
    } else {
        0
    }
}

fn new_game(data: &DirData, w: u32, l: u32) -> Game {
    let mut g = Game::new();
    g.game_state.multi = SINGLEPLAYER;
    g.game_state.single = ARCADE_MODE;
    g.game_state.mode = INGAME_MODE;
    g.frogger.player[0].lives = 3;
    g.load_level(w, l, data).unwrap_or_else(|e| panic!("level {w}-{l}: {e}"));
    g.frogger.player[0].lives = 100;
    g
}

/// Structural invariants of the effect pools.
fn check(g: &Game, ctx: &str) {
    let fx: Vec<_> = g.specfx.iter_list().collect();
    assert_eq!(fx.len() as i32, g.specfx.count, "{ctx}: fx list/count");
    assert_eq!(g.sprite.iter_list().count() as i32, g.sprite.count, "{ctx}: sprite list/count");
    let mut fx_sprites = 0;
    for (id, e) in &fx {
        assert!(e.update.is_some(), "{ctx}: fx without update");
        let s = g.fx_sprites(*id);
        if e.sprites.is_some() {
            assert_eq!(s.len() as i32, e.num_p, "{ctx}: fx sprite block");
            assert!(s.iter().all(|&s| g.sprite.is_linked(s)));
        }
        fx_sprites += s.len();
    }
    assert!(fx_sprites as i32 <= g.sprite.count);
    let used = g.textover.block.iter().filter(|t| t.used != 0).count() as i32;
    assert_eq!(used, g.textover.num_entries, "{ctx}: text overlays");
    let used = g.overlays.block.iter().filter(|t| t.used != 0).count() as i32;
    assert_eq!(used, g.overlays.num_entries, "{ctx}: sprite overlays");
}

#[test]
fn language_table_strings() {
    let Some(dir) = game_dir() else { return };
    let mut g = Game::new();
    g.load_language(&DirData(dir));
    assert!(!g.language.game_text_str.is_empty());
    let go = g.gamestring(strs::STR_GO);
    assert!(go.to_ascii_uppercase().contains("GO"), "STR_GO = {go:?}");
    let q = g.gamestring(strs::STR_QUIT);
    assert!(q.to_ascii_uppercase().contains("QUIT"), "STR_QUIT = {q:?}");
    let c = g.gamestring(strs::STR_CONTINUE);
    assert!(c.to_ascii_uppercase().contains("CONTINUE"), "STR_CONTINUE = {c:?}");
    eprintln!("GO={go:?} QUIT={q:?} OUTOFTIME={:?} COLLECTBABIES={:?}", g.gamestring(strs::STR_OUTOFTIME), g.gamestring(strs::STR_COLLECTBABIES));
}

#[test]
fn garden1_effects_hud_and_determinism() {
    let Some(dir) = game_dir() else { return };
    let data = DirData(dir);
    let run = || {
        let mut g = new_game(&data, worldid::GARDEN, 0);
        assert!(g.hud.arcade_hud.lives_text.is_some());
        assert!(g.hud.arcade_hud.go_text.is_some());
        let mut max_fx = 0;
        let mut max_sprites = 0;
        for t in 0..1800u32 {
            g.tick(TickInput { pads: [pattern(t), 0, 0, 0] });
            g.take_effects();
            max_fx = max_fx.max(g.specfx.count);
            max_sprites = max_sprites.max(g.sprite.count);
            if t % 60 == 0 {
                check(&g, &format!("garden1 t{t}"));
            }
        }
        (g, max_fx, max_sprites)
    };
    let (g1, fx1, sp1) = run();
    let (g2, fx2, sp2) = run();
    assert_eq!(g1.rng, g2.rng);
    assert_eq!((fx1, sp1), (fx2, sp2));
    assert_eq!(g1.specfx.array, g2.specfx.array);
    assert_eq!(g1.sprite.array, g2.sprite.array);
    assert_eq!(g1.textover.block, g2.textover.block);
    let lives = g1.textover.get(g1.hud.arcade_hud.lives_text).unwrap().text.clone();
    assert_eq!(lives, g1.frogger.player[0].lives.to_string());
    let go = g1.textover.get(g1.hud.arcade_hud.go_text).unwrap();
    eprintln!("garden1: max fx {fx1}, max sprites {sp1}, go text {:?}, proc textures {}", go.text, g1.ptexture.list.len());
    assert!(fx1 > 0, "no special effects were created");
}

#[test]
fn every_level_effects_1800_ticks() {
    let Some(dir) = game_dir() else { return };
    let data = DirData(dir);
    let mut played = 0;
    let mut with_ptex = 0;
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
            let mut g = new_game(&data, w as u32, l as u32);
            let mut max_fx = 0;
            for t in 0..1800u32 {
                g.tick(TickInput { pads: [pattern(t), 0, 0, 0] });
                g.take_effects();
                max_fx = max_fx.max(g.specfx.count);
                if t % 300 == 0 {
                    check(&g, &format!("{w}-{l} t{t}"));
                }
            }
            check(&g, &format!("{w}-{l} end"));
            if !g.ptexture.list.is_empty() {
                with_ptex += 1;
                assert!(g.ptexture.list.iter().filter(|p| p.active != 0 && p.update.is_some()).all(|p| p.surface.is_some()));
            }
            eprintln!(
                "{w}-{l}: max fx {max_fx}, sprites {}, particles {}, overlays {}, texts {}, proc textures {}",
                g.sprite.count,
                g.particle.count,
                g.overlays.num_entries,
                g.textover.num_entries,
                g.ptexture.list.len()
            );
            played += 1;
        }
    }
    assert!(played >= 30, "only {played} levels played");
    assert!(with_ptex > 0, "no level has procedural textures");
}
