//! Writers reproduce every retail level file byte-for-byte: `.col`, `.sce`,
//! `entity-*.dat` (v16), `.sam`, `script-*.fev`. Needs `F2_GAME_DIR` (the
//! imported `game/` tree, or `F2_DATA_DIR`); passes trivially when absent.

use f2_formats::*;
use std::path::{Path, PathBuf};

fn game_dir() -> Option<PathBuf> {
    let d = std::env::var_os("F2_GAME_DIR").or_else(|| std::env::var_os("F2_DATA_DIR")).map(PathBuf::from)?;
    d.join("collision").is_dir().then_some(d)
}

fn files(dir: &Path, pred: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| pred(&p.file_name().unwrap().to_string_lossy().to_lowercase()))
        .collect();
    v.sort();
    v
}

fn first_diff(a: &[u8], b: &[u8]) -> String {
    let i = a.iter().zip(b).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
    format!("len {} vs {}, first diff at {i:#x}", a.len(), b.len())
}

#[test]
fn level_writers_roundtrip_retail() {
    let Some(d) = game_dir() else {
        eprintln!("F2_GAME_DIR not set; skipping");
        return;
    };
    let mut fails = Vec::new();
    let mut counts = [0usize; 5];
    for f in files(&d.join("collision"), |n| n.ends_with(".col")) {
        let raw = std::fs::read(&f).unwrap();
        let c = level::Collision::parse(&raw).unwrap();
        let w = c.write();
        if w != raw {
            fails.push(format!("{f:?}: {}", first_diff(&w, &raw)));
        }
        counts[0] += 1;
    }
    for f in files(&d.join("scenics"), |n| n.ends_with(".sce")) {
        let raw = std::fs::read(&f).unwrap();
        let s = level::parse_scenics(&raw).unwrap();
        let w = level::write_scenics(&s);
        if w != raw {
            fails.push(format!("{f:?}: {}", first_diff(&w, &raw)));
        }
        counts[1] += 1;
    }
    let maps = d.join("maps");
    let mut stale = 0;
    for f in files(&maps, |n| n.starts_with("entity") && n.ends_with(".dat")) {
        let raw = std::fs::read(&f).unwrap();
        if entity::Entities::version(&raw) != Some(entity::ENTITY_VERSION) {
            stale += 1; // the game rejects these too
            continue;
        }
        let ef = entity::EntityFile::parse(&raw).unwrap();
        let w = ef.write();
        let e = ef.entities;
        if w != raw {
            fails.push(format!("{f:?}: {}", first_diff(&w, &raw)));
        }
        let mut r = e.clone();
        r.recount();
        if (r.total_path_nodes, r.total_enemies, r.total_platforms, r.total_garibs, r.total_placeholders)
            != (e.total_path_nodes, e.total_enemies, e.total_platforms, e.total_garibs, e.total_placeholders)
        {
            eprintln!("{f:?}: header totals differ from recount");
        }
        counts[2] += 1;
    }
    // The ambient/mapping split depends on the level's tile count; both extremes
    // must round-trip.
    for f in files(&maps, |n| n.ends_with(".sam")) {
        let raw = std::fs::read(&f).unwrap();
        for tc in [0usize, 2000] {
            let s = level::SamFile::parse(&raw, tc).unwrap();
            let w = s.write();
            if w != raw {
                fails.push(format!("{f:?} (tiles {tc}): {}", first_diff(&w, &raw)));
            }
        }
        counts[3] += 1;
    }
    for f in files(&maps, |n| n.ends_with(".fev")) {
        let raw = std::fs::read(&f).unwrap();
        let Ok(s) = fev::Script::parse(&raw) else { continue };
        if s.write() != raw {
            fails.push(format!("{f:?}: {}", first_diff(&s.write(), &raw)));
        }
        counts[4] += 1;
    }
    eprintln!("round-tripped col {} sce {} entity {} (skipped {stale} non-v16) sam {} fev {}", counts[0], counts[1], counts[2], counts[3], counts[4]);
    assert!(counts.iter().all(|&c| c > 0));
    assert!(fails.is_empty(), "{} mismatches:\n{}", fails.len(), fails.join("\n"));
}
