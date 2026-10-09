//! Real-data test: every loadable level, single player in-game, entities created by
//! `MemLoadEntities`, then 1800 ticks (30 s) of `Game::tick`. Moving enemies and
//! platforms must move. Reads `F2_GAME_DIR` (default `game-data/usa/game`); skips
//! when absent.

use std::path::PathBuf;

use f2_formats::entity::{Entities, ENTITY_VERSION};
use f2_game::assets::{DataSource, DirData};
use f2_game::bank_tables::collision_file_for_coll_bank;
use f2_game::define::mode::INGAME_MODE;
use f2_game::define::SINGLEPLAYER;
use f2_game::enemies::EnemyUpdate;
use f2_game::ids::ActorId;
use f2_game::layout::{entity_file_name, ORIG_WORLD_VISUAL_DATA};
use f2_game::platform::PlatformUpdate;
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

/// Actors of enemies/platforms whose update function moves them along a path.
fn movers(g: &Game) -> Vec<ActorId> {
    let mut v = Vec::new();
    let el = &g.enemies.enemy_list;
    for id in el.head.ids(&el.array) {
        let e = &el.array[id.idx()];
        let moving = matches!(e.update[0], Some(EnemyUpdate::Path | EnemyUpdate::SlerpPath | EnemyUpdate::MoveVertical | EnemyUpdate::RotatePath));
        if moving && e.active != 0 && e.path.num_nodes >= 2 {
            v.extend(e.nme_actor);
        }
    }
    let pl = &g.platform.platform_list;
    for id in pl.head.ids(&pl.array) {
        let p = &pl.array[id.idx()];
        if p.update == Some(PlatformUpdate::Path) && p.active != 0 && p.path.num_nodes >= 2 {
            v.extend(p.plt_actor);
        }
    }
    v
}

#[test]
fn entities_move_in_every_level() {
    let Some(dir) = game_dir() else { return };
    let data = DirData(dir);
    let mut loaded = 0;
    let mut levels_with_movers = 0;
    for (w, wvd) in ORIG_WORLD_VISUAL_DATA.iter().enumerate() {
        for l in 0..wvd.num_levels as usize {
            let lvd = &wvd.level_visual_data[l];
            match collision_file_for_coll_bank(lvd.coll_bank) {
                Some(f) if data.read(f).is_some() => {}
                _ => continue,
            }
            let ent = entity_file_name(wvd.world_id, lvd.level_id);
            let bytes = data.read(&ent);
            if bytes.as_deref().and_then(Entities::version).is_some_and(|v| v != ENTITY_VERSION) {
                continue;
            }

            let mut g = Game::new();
            g.game_state.multi = SINGLEPLAYER;
            g.game_state.mode = INGAME_MODE;
            g.load_level(w as u32, l as u32, &data).unwrap_or_else(|e| panic!("level {w}-{l}: {e}"));

            let el = &g.enemies.enemy_list;
            let pl = &g.platform.platform_list;
            let n_enemies = el.head.ids(&el.array).len();
            let n_platforms = pl.head.ids(&pl.array).len();
            assert_eq!(n_enemies as i16, el.count);
            assert_eq!(n_platforms as i16, pl.count);
            if let Some(e) = &g.level_files.entities {
                assert!(el.count <= e.total_enemies + e.total_placeholders);
                assert!(pl.count <= e.total_platforms);
            }
            assert!(g.frogger.frog[0].is_some(), "{w}-{l}: no frog");

            let movers = movers(&g);
            let start: Vec<_> = movers.iter().map(|&a| g.actor2.actors[a].actor.position).collect();
            let mut moved = vec![false; movers.len()];
            for t in 0..1800 {
                g.tick(TickInput::default());
                if t % 10 == 0 {
                    for (k, &a) in movers.iter().enumerate() {
                        if g.actor2.actors[a].actor.position != start[k] {
                            moved[k] = true;
                        }
                    }
                }
                g.take_effects();
            }
            let n_moved = moved.iter().filter(|&&m| m).count();
            eprintln!(
                "{w}-{l}: {n_enemies} enemies, {n_platforms} platforms, {} garibs, {} trans cams; {}/{} path movers moved",
                g.collect.garib_list.total,
                g.cam.trans_camera_defs.len(),
                n_moved,
                movers.len()
            );
            if !movers.is_empty() {
                levels_with_movers += 1;
                assert!(n_moved > 0, "{w}-{l}: none of {} moving entities moved", movers.len());
                assert!(n_moved * 2 >= movers.len(), "{w}-{l}: only {n_moved}/{} moving entities moved", movers.len());
            }
            loaded += 1;
        }
    }
    eprintln!("{loaded} levels, {levels_with_movers} with path movers");
    assert!(loaded >= 30, "only {loaded} levels loaded");
    assert!(levels_with_movers >= 20, "only {levels_with_movers} levels with moving entities");
}
