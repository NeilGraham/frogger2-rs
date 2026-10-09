//! Parses every retail file of each kind. Runs only when `F2_DATA_DIR` points
//! at an extracted `Data files` tree (never committed); otherwise passes trivially.

use f2_formats::*;
use std::path::{Path, PathBuf};

fn data_dir() -> Option<PathBuf> {
    std::env::var_os("F2_DATA_DIR").map(PathBuf::from)
}

fn files(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|e| e.to_string_lossy().eq_ignore_ascii_case(ext)).unwrap_or(false))
        .collect();
    v.sort();
    v
}

#[test]
fn all_formats_parse() {
    let Some(d) = data_dir() else {
        eprintln!("F2_DATA_DIR not set; skipping");
        return;
    };
    let mut n = 0;
    for f in files(&d.join("textures"), "fla") {
        let raw = fla::decompress(&std::fs::read(&f).unwrap()).unwrap();
        let name = f.file_name().unwrap().to_string_lossy().to_lowercase();
        if name.starts_with("artwork") || name == "title.fla" || name == "loading.fla" {
            let t = texture::parse_backdrop(&raw).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!((t.width, t.height), (640, 480));
        } else {
            let b = texture::TexBank::parse(&raw).unwrap_or_else(|e| panic!("{name}: {e}"));
            for t in &b.textures {
                assert_eq!(t.to_rgba8().len(), t.width as usize * t.height as usize * 4);
            }
        }
        n += 1;
    }
    for f in files(&d.join("sfx"), "wus") {
        let b = wus::SoundBank::parse(&std::fs::read(&f).unwrap()).unwrap_or_else(|e| panic!("{f:?}: {e}"));
        assert!(!b.samples.is_empty());
        n += 1;
    }
    let mut tiles_total = 0;
    for f in files(&d.join("collision"), "col") {
        let c = level::Collision::parse(&std::fs::read(&f).unwrap()).unwrap_or_else(|e| panic!("{f:?}: {e}"));
        tiles_total += c.tiles.len();
        n += 1;
    }
    assert!(tiles_total > 10_000);
    for f in files(&d.join("scenics"), "sce") {
        level::parse_scenics(&std::fs::read(&f).unwrap()).unwrap_or_else(|e| panic!("{f:?}: {e}"));
        n += 1;
    }
    let maps = d.join("maps");
    for f in files(&maps, "dat") {
        let raw = std::fs::read(&f).unwrap();
        if entity::Entities::version(&raw) != Some(entity::ENTITY_VERSION) {
            eprintln!("stale entity file (game rejects it): {f:?}");
            continue;
        }
        entity::Entities::parse(&std::fs::read(&f).unwrap()).unwrap_or_else(|e| panic!("{f:?}: {e}"));
        n += 1;
    }
    for f in files(&maps, "sam") {
        level::parse_sfx_mapping(&std::fs::read(&f).unwrap(), 0).unwrap_or_else(|e| panic!("{f:?}: {e}"));
        n += 1;
    }
    for f in files(&maps, "fev") {
        fev::Script::parse(&std::fs::read(&f).unwrap()).unwrap_or_else(|e| panic!("{f:?}: {e}"));
        n += 1;
    }
    let lang = lang::StringTable::parse(&fla::decompress(&std::fs::read(maps.join("language.fla")).unwrap()).unwrap()).unwrap();
    assert!(lang.rows.len() > 300, "{} rows", lang.rows.len());
    assert_eq!(lang.languages.len(), 5);
    let mut models = 0;
    for f in files(&d.join("objects"), "dll") {
        let b = objbank::ObjectBank::parse(&std::fs::read(&f).unwrap()).unwrap_or_else(|e| panic!("{f:?}: {e}"));
        models += b.models.len();
        n += 1;
    }
    assert!(models > 500, "{models} models");
    for f in files(&d.join("demos"), "key") {
        misc::parse_key_file(&std::fs::read(&f).unwrap()).unwrap();
        n += 1;
    }
    eprintln!("parsed {n} files, {models} models, {tiles_total} tiles");
}

#[cfg(feature = "serde")]
#[test]
fn serde_roundtrip_real_data() {
    let Some(d) = data_dir() else { return };
    let f = files(&d.join("collision"), "col").remove(0);
    let c = level::Collision::parse(&std::fs::read(&f).unwrap()).unwrap();
    let j = serde_json::to_string(&c).unwrap();
    let back: level::Collision = serde_json::from_str(&j).unwrap();
    assert_eq!(back.tiles, c.tiles);
    let f = files(&d.join("maps"), "fev").remove(0);
    let s = fev::Script::parse(&std::fs::read(&f).unwrap()).unwrap();
    let back: fev::Script = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(back.write(), std::fs::read(&f).unwrap());
    let f = files(&d.join("objects"), "dll").remove(0);
    let b = objbank::ObjectBank::parse(&std::fs::read(&f).unwrap()).unwrap();
    serde_json::to_string(&b).unwrap();
}
