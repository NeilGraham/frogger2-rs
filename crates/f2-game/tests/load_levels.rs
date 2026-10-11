//! Real-data tests: load every level of `origWorldVisualData` and animate the frog.
//! Reads `F2_GAME_DIR` (default `game-data/usa/game`); skips when absent.

use std::path::PathBuf;

use f2_formats::entity::{Entities, ENTITY_VERSION};
use f2_game::assets::{DataSource, DirData};
use f2_game::bank_tables::collision_file_for_coll_bank;
use f2_game::define::SINGLEPLAYER;
use f2_game::layout::{entity_file_name, ORIG_WORLD_VISUAL_DATA};
use f2_game::{Game, TickInput};

fn game_dir() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let p = std::env::var("F2_GAME_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("game-data/usa/game"));
    let p = if p.is_absolute() { p } else { root.join(p) };
    // Accept both `.../usa` and `.../usa/game`.
    let p = if p.join("game").join("collision").is_dir() { p.join("game") } else { p };
    if p.join("collision").is_dir() {
        Some(p)
    } else {
        eprintln!("skipping: no game data at {}", p.display());
        None
    }
}

fn new_game() -> Game {
    let mut g = Game::new();
    g.game_state.multi = SINGLEPLAYER;
    g
}

#[test]
fn load_every_level() {
    let Some(dir) = game_dir() else { return };
    let data = DirData(dir);
    let mut g = new_game();
    let mut loaded = 0;
    for (w, wvd) in ORIG_WORLD_VISUAL_DATA.iter().enumerate() {
        for l in 0..wvd.num_levels as usize {
            let lvd = &wvd.level_visual_data[l];
            // Placeholder entries (collBank SYSTEM_COL, or frontend maps retail never
            // shipped such as LEVELA.COL): InitLevel would crash on them.
            match collision_file_for_coll_bank(lvd.coll_bank) {
                Some(f) if data.read(f).is_some() => {}
                f => {
                    eprintln!("skip {w}-{l}: collBank {} file {f:?} not available", lvd.coll_bank);
                    continue;
                }
            }
            let ent = entity_file_name(wvd.world_id, lvd.level_id);
            let bytes = data.read(&ent);
            if bytes.as_deref().and_then(Entities::version).is_some_and(|v| v != ENTITY_VERSION) {
                eprintln!("skip {w}-{l}: stale entity file {ent}");
                continue;
            }
            let assets = g.load_level(w as u32, l as u32, &data).unwrap_or_else(|e| panic!("level {w}-{l}: {e}"));
            assert!(!g.map.tiles.is_empty(), "{w}-{l}: no tiles");
            assert!(g.models.len() > 10, "{w}-{l}: only {} models", g.models.len());
            assert!(assets.object_banks.iter().any(|b| b.ends_with("gamegeneric.dll")));
            assert!(assets.object_banks.iter().any(|b| b.ends_with("frogger.dll")));
            assert!(assets.texture_banks.len() >= 2, "{w}-{l}: {:?}", assets.texture_banks);
            assert!(g.models.find_object("frogger.obe").is_some());
            if bytes.is_some() {
                assert!(g.level_files.entities.is_some(), "{w}-{l}: entities not parsed");
            }
            // Every tile link stays in range.
            for t in &g.map.tiles {
                assert!(t.tile_ptrs.iter().flatten().all(|p| p.idx() < g.map.tiles.len()));
            }
            // Scenic actors exist and the list is consistent.
            let ids = g.actor2.act_list.ids(&g.actor2.actors);
            assert_eq!(ids.len() as i32, g.actor2.act_list.count);
            eprintln!(
                "{w}-{l}: {} tiles, {} scenics, {} actors, {} models, missing {:?}, script {}, sfx {}",
                g.map.tiles.len(),
                g.map.scenics.len(),
                ids.len(),
                g.models.len(),
                assets.missing_object_banks,
                g.level_files.script.is_some(),
                g.level_files.sfx_mapping.len()
            );
            // Run a few ticks of the animation clock.
            for _ in 0..30 {
                g.tick(TickInput::default());
            }
            loaded += 1;
        }
    }
    assert!(loaded >= 30, "only {loaded} levels loaded");
}

#[test]
fn frog_animations_stay_in_range() {
    let Some(dir) = game_dir() else { return };
    let data = DirData(dir);
    let mut g = new_game();
    g.load_level(0, 0, &data).unwrap();
    let frog = g.create_and_add_actor("frogger.obe", 0, 0, 0, 0).expect("frog actor");
    let num = g.actor2.actors[frog].actor.actual_actor.animation.as_ref().unwrap().num_animations as i32;
    assert!(num > 20, "frog has {num} animations");
    for anim in 0..num {
        for (looped, speed) in [(true, 0), (false, 0), (true, 128), (false, -64)] {
            g.actor_animate(frog, anim, looped, false, speed, false);
            if anim + 1 < num {
                g.actor_animate(frog, anim + 1, false, true, 0, false);
            }
            for _ in 0..600 {
                g.tick(TickInput::default());
                let a = g.actor2.actors[frog].actor.actual_actor.animation.as_ref().unwrap();
                let r = a.anim(a.current_animation as i32);
                assert!(
                    a.anim_time >= r.anim_start as f32 && a.anim_time <= r.anim_end as f32,
                    "anim {} time {} outside {}..{}",
                    a.current_animation,
                    a.anim_time,
                    r.anim_start,
                    r.anim_end
                );
            }
        }
    }
    // Breathe loop never reports an end.
    g.actor_animate(frog, f2_game::pc_actor::FROG_ANIM_BREATHE, true, false, f2_game::pc_actor::FROG_BREATHE_SPEED, false);
    for _ in 0..600 {
        g.tick(TickInput::default());
        assert_eq!(g.actor2.actors[frog].actor.actual_actor.animation.as_ref().unwrap().reached_end_of_animation, 0);
    }
}
