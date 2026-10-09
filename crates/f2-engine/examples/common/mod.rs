// Shared by the examples (included with `mod common;`).
#![allow(dead_code)]

use f2_engine::load::{default_tex_banks, game_dir, read_tex_bank};
use f2_engine::Renderer;
use std::path::{Path, PathBuf};

/// Splits `--tex <bank>` options from positional args.
pub fn parse_args() -> (Vec<String>, Vec<PathBuf>) {
    let mut pos = Vec::new();
    let mut tex = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        if a == "--tex" {
            tex.push(PathBuf::from(it.next().expect("--tex needs a path")));
        } else {
            pos.push(a);
        }
    }
    (pos, tex)
}

/// Loads explicit `--tex` banks, or the defaults for `bank` from the bank's
/// sibling `textures/` directory (falling back to `$F2_GAME_DIR/game/textures`).
pub fn load_textures(r: &mut Renderer, bank: &Path, explicit: &[PathBuf]) {
    let banks: Vec<PathBuf> = if explicit.is_empty() {
        let dir = bank.parent().and_then(|p| p.parent()).map(|p| p.join("textures")).filter(|p| p.is_dir()).unwrap_or_else(|| game_dir().join("textures"));
        default_tex_banks(&bank.to_string_lossy()).iter().map(|b| dir.join(b)).collect()
    } else {
        explicit.to_vec()
    };
    for b in banks {
        match read_tex_bank(&b) {
            Ok(tb) => {
                eprintln!("loaded {} ({} textures, {} anims)", b.display(), tb.textures.len(), tb.anims.len());
                r.load_tex_bank(&tb);
            }
            Err(e) => eprintln!("warning: {e}"),
        }
    }
}
