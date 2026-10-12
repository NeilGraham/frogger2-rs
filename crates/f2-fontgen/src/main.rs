//! `f2-fontgen [RENDITION] [--ttf FILE] [--preview OUT.png]`: builds the modern fonts of an
//! imported rendition (default `game-data/usa`) from Gill Sans Ultra Bold. `--preview`
//! writes a side-by-side sheet of the originals and the modern glyphs (stays local: it is
//! derived from your game data).

use std::path::PathBuf;

fn main() {
    let mut rendition = PathBuf::from("game-data/usa");
    let (mut ttf, mut preview) = (None, None);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--ttf" => ttf = args.next().map(PathBuf::from),
            "--preview" => preview = args.next().map(PathBuf::from),
            "-h" | "--help" => {
                eprintln!("usage: f2-fontgen [RENDITION] [--ttf FILE] [--preview OUT.png]");
                return;
            }
            _ => rendition = PathBuf::from(a),
        }
    }
    let Some(ttf) = ttf.or_else(|| f2_fontgen::find_typeface(&[&rendition])) else {
        eprintln!("f2-fontgen: {} not found (install it, put it in <data>/fonts/, or pass --ttf)", f2_fontgen::TYPEFACE_FILE);
        std::process::exit(1);
    };
    eprintln!("f2-fontgen: typeface {}", ttf.display());
    match f2_fontgen::generate(&rendition, &ttf) {
        Ok(log) => log.iter().for_each(|l| eprintln!("f2-fontgen: {l}")),
        Err(e) => {
            eprintln!("f2-fontgen: {e}");
            std::process::exit(1);
        }
    }
    if let Some(out) = preview {
        if let Err(e) = f2_fontgen::preview::write(&rendition, &out) {
            eprintln!("f2-fontgen: preview: {e}");
            std::process::exit(1);
        }
        eprintln!("f2-fontgen: preview {}", out.display());
    }
}
