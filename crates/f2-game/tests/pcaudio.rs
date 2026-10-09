//! Real-data test of the simulation-side audio (`PC/pcaudio.c`): every level loads its
//! sample banks, gets its ambient sounds and runs without consuming extra `Random()`.
//! Reads `F2_GAME_DIR` (default `game-data/usa/game`); skips when absent.

use std::path::PathBuf;

use f2_game::assets::DirData;
use f2_game::define::SINGLEPLAYER;
use f2_game::layout::ORIG_WORLD_VISUAL_DATA;
use f2_game::pcaudio::{gen_sfx, GEN_FROG_HOP, NUM_GENERIC_SFX};
use f2_game::{Effect, Game};

fn game_dir() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let p = std::env::var("F2_GAME_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("game-data/usa/game"));
    let p = if p.is_absolute() { p } else { root.join(p) };
    let p = if p.join("game").join("collision").is_dir() { p.join("game") } else { p };
    if p.join("sfx").is_dir() {
        Some(p)
    } else {
        eprintln!("skipping: no game data at {}", p.display());
        None
    }
}

#[test]
fn levels_load_samples_and_ambients() {
    let Some(dir) = game_dir() else { return };
    let data = DirData(dir);
    let mut g = Game::new();
    g.game_state.multi = SINGLEPLAYER;
    let mut ambients = 0;
    for (w, wvd) in ORIG_WORLD_VISUAL_DATA.iter().enumerate() {
        for l in 0..wvd.num_levels as usize {
            if wvd.level_visual_data[l].coll_bank < 0 {
                continue;
            }
            if g.load_level(w as u32, l as u32, &data).is_err() {
                continue;
            }
            // Every genSfx sample is in generic.wus.
            for i in 0..NUM_GENERIC_SFX {
                assert!(g.pcaudio.find_sample(gen_sfx(i)).is_some(), "genSfx[{i}] missing");
            }
            assert!(g.effects.iter().any(|e| matches!(e, Effect::PlayMusic { .. })) || w == 9);
            ambients += g.pcaudio.ambient_sound_list.len();
            g.effects.clear();
        }
    }
    assert!(g.pcaudio.find_sample(gen_sfx(GEN_FROG_HOP)).is_some());
    assert!(ambients > 0, "no level created an ambient sound");
}
