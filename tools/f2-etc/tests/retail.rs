//! Every retail `.fev` must decompile and recompile to the identical bytes.
//! Reads `$F2_GAME_DIR/maps` (default `game-data/usa/game`); skips if absent.

use std::path::PathBuf;

use f2_etc::Roundtrip;

fn game_dir() -> PathBuf {
    match std::env::var_os("F2_GAME_DIR") {
        Some(d) => PathBuf::from(d),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../game-data/usa/game"),
    }
}

#[test]
fn retail_fev_roundtrip() {
    let maps = game_dir().join("maps");
    let Ok(rd) = std::fs::read_dir(&maps) else {
        eprintln!("skipping: {} not found", maps.display());
        return;
    };
    let mut files: Vec<PathBuf> =
        rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("fev"))).collect();
    files.sort();
    assert!(!files.is_empty(), "no .fev files in {}", maps.display());

    let mut failures = Vec::new();
    for f in &files {
        let data = std::fs::read(f).unwrap();
        match f2_etc::roundtrip(&data) {
            Ok((dec, Roundtrip::Identical)) => assert!(!dec.lossy),
            Ok((_, r)) => failures.push(format!("{}: {r:?}", f.display())),
            Err(e) => failures.push(format!("{}: {e}", f.display())),
        }
    }
    eprintln!("{} of {} retail .fev files round-trip", files.len() - failures.len(), files.len());
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
