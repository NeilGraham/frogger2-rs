//! Real-data test (wave D): every loadable level loads and runs its event script
//! (`InitLevelScript`), then 1800 ticks (30 s) with the frog hopping now and then, so
//! `UpdateEvents`, the triggers and the camera run without panicking. Reads
//! `F2_GAME_DIR` (default `game-data/usa/game`); skips when absent.

use std::path::PathBuf;

use f2_formats::entity::{Entities, ENTITY_VERSION};
use f2_game::assets::{DataSource, DirData};
use f2_game::bank_tables::collision_file_for_coll_bank;
use f2_game::define::mode::INGAME_MODE;
use f2_game::define::SINGLEPLAYER;
use f2_game::layout::{entity_file_name, script_file_name, ORIG_WORLD_VISUAL_DATA};
use f2_game::levplay::{PAD_LEFT, PAD_UP};
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

fn logs(effects: &[Effect], what: &str) -> usize {
    effects.iter().filter(|e| matches!(e, Effect::Log { text } if text.contains(what))).count()
}

#[test]
fn every_level_runs_its_script() {
    let Some(dir) = game_dir() else { return };
    let data = DirData(dir);
    let (mut loaded, mut with_script, mut with_triggers) = (0, 0, 0);
    for (w, wvd) in ORIG_WORLD_VISUAL_DATA.iter().enumerate() {
        for l in 0..wvd.num_levels as usize {
            let lvd = &wvd.level_visual_data[l];
            match collision_file_for_coll_bank(lvd.coll_bank) {
                Some(f) if data.read(f).is_some() => {}
                _ => continue,
            }
            if data.read(&entity_file_name(wvd.world_id, lvd.level_id)).as_deref().and_then(Entities::version).is_some_and(|v| v != ENTITY_VERSION) {
                continue;
            }
            let has_script = data.read(&script_file_name(wvd.world_id, lvd.level_id)).is_some();

            let mut g = Game::new();
            g.game_state.multi = SINGLEPLAYER;
            g.game_state.mode = INGAME_MODE;
            g.load_level(w as u32, l as u32, &data).unwrap_or_else(|e| panic!("level {w}-{l}: {e}"));
            g.frogger.player[0].lives = 99;
            let fx = g.take_effects();
            let init_failed = logs(&fx, "InitLevelScript() failed");
            let triggers = g.event.trigger_list.count;
            if has_script {
                with_script += 1;
                assert!(g.level_files.script.is_some(), "{w}-{l}: script did not decode");
                assert_eq!(init_failed, 0, "{w}-{l}: InitLevelScript failed");
                assert!(g.script.script_buffer.is_some(), "{w}-{l}: no scriptBuffer");
                assert!(g.event.trigger_list.active().len() as i16 <= triggers);
            }
            if triggers > 0 {
                with_triggers += 1;
            }

            let (mut errors, mut ended) = (0, None);
            let (src0, _, _) = g.camera_source_target();
            let mut cam_moved = false;
            for t in 0..1800u32 {
                let pads = match t % 90 {
                    40 => [PAD_UP, 0, 0, 0],
                    80 => [PAD_LEFT, 0, 0, 0],
                    _ => [0; 4],
                };
                g.tick(TickInput { pads });
                let fx = g.take_effects();
                errors += logs(&fx, "Error running script within trigger");
                if g.camera_source_target().0 != src0 {
                    cam_moved = true;
                }
                if g.game_state.mode != INGAME_MODE && ended.is_none() {
                    // EV_ENDLEVEL / EV_CHANGELEVEL / game over: other modes are not ported yet.
                    ended = Some((t, g.game_state.mode));
                }
            }
            let (src, tgt, up) = g.camera_source_target();
            eprintln!(
                "{w}-{l}: script {has_script}, {triggers} triggers at load, {} now, {errors} event errors, flags {:02x?}, ended {ended:?}, cam src {:?} tgt {:?} up {:?} moved {cam_moved}, fog {:?}",
                g.event.trigger_list.count,
                g.script.script_flags,
                [src.vx >> 12, src.vy >> 12, src.vz >> 12],
                [tgt.vx >> 12, tgt.vy >> 12, tgt.vz >> 12],
                [up.vx, up.vy, up.vz],
                g.frogger.fog,
            );
            assert!(g.event.trigger_list.count >= 0);
            loaded += 1;
        }
    }
    eprintln!("{loaded} levels, {with_script} with scripts, {with_triggers} with triggers");
    assert!(loaded >= 30, "only {loaded} levels loaded");
    assert!(with_script >= 25, "only {with_script} levels with scripts");
    assert!(with_triggers >= 20, "only {with_triggers} levels with triggers");
}
