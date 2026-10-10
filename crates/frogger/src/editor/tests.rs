//! Editor tests. The data-driven ones need the imported game (`F2_GAME_DIR`) and
//! skip otherwise.

use super::doc::LevelDoc;
use super::*;
use crate::data::Data;
use f2_formats::entity::{ActorKind, Item};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn rendition() -> Option<PathBuf> {
    let base = std::env::var("F2_GAME_DIR").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../game-data/usa").into());
    let p = PathBuf::from(base);
    let root = if p.file_name().is_some_and(|n| n == "game") { p.parent()?.to_path_buf() } else { p };
    root.join("game").is_dir().then_some(root)
}

fn data(root: &Path) -> Data {
    Data { vfs: Arc::new(f2_mods::Vfs::with_rendition(root)), rendition: root.to_path_buf() }
}

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("f2-editor-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn editor(doc: LevelDoc) -> Editor {
    Editor::new(doc, FlyCam { pos: [0.0; 3], yaw: 0.0, pitch: 0.0 })
}

/// The first enemy whose model is not a placeholder blank.
fn some_enemy(doc: &LevelDoc) -> usize {
    (0..doc.ents.entities.items.len())
        .find(|&i| matches!(&doc.ents.entities.items[i], Item::Actor(a) if a.kind == ActorKind::Enemy))
        .expect("an enemy")
}

/// Unedited documents write back every file byte-identically.
#[test]
fn unedited_level_writes_identical_files() {
    let Some(root) = rendition() else {
        eprintln!("skipping: no game data (F2_GAME_DIR)");
        return;
    };
    let d = data(&root);
    let mut n = 0;
    for (w, l) in (0..12).flat_map(|w| (0..12).map(move |l| (w, l))) {
        let Ok(doc) = LevelDoc::load(&d, w, l) else { continue };
        n += 1;
        assert!(doc.files(false).is_empty());
        for (path, bytes) in doc.files(true) {
            assert_eq!(bytes, d.read(&path).unwrap(), "{w}-{l}: {path}");
        }
    }
    assert!(n > 20, "{n} levels loaded");
}

/// Garden 1: move one entity through the editor (command API), save to a temp mod,
/// reload through a VFS with that mod and find the entity at the new position, both
/// in the parsed files and in the game after `load_level`.
#[test]
fn move_entity_save_mod_reload() {
    let Some(root) = rendition() else {
        eprintln!("skipping: no game data (F2_GAME_DIR)");
        return;
    };
    let d = data(&root);
    let doc = LevelDoc::load(&d, 0, 0).expect("Garden 1");
    let i = some_enemy(&doc);
    let old = doc.item_tile(i).unwrap();
    // Some other tile well away from the old one.
    let op = doc.tile_pos(old).unwrap();
    let new = (0..doc.col.tiles.len()).max_by_key(|&t| {
        let p = doc.tile_pos(t).unwrap();
        (0..3).map(|k| ((p[k] - op[k]) as i64).pow(2)).sum::<i64>()
    });
    let new = new.unwrap();
    // Retail uids are not unique (Garden 1's are all 0): give it one to find it again.
    let uid = 31337;

    let mods = temp_dir("mods");
    let mut ed = editor(doc);
    ed.mods_dir = mods.clone();
    ed.mod_id = "editor-test".into();
    for c in [format!("sel item {i}"), format!("set uid {uid}"), format!("move {new}"), "save".to_string()] {
        ed.exec(&c).unwrap_or_else(|e| panic!("{c}: {e}"));
    }
    assert_eq!(ed.doc.item_tile(i), Some(new));
    assert!(!ed.doc.unsaved());
    let ent_file = mods.join("editor-test/game").join(&ed.doc.ent_path);
    assert!(ent_file.is_file(), "{} written", ent_file.display());
    assert!(mods.join("editor-test/mod.toml").is_file());
    // Only the entity file changed.
    assert!(!mods.join("editor-test/game").join(&ed.doc.col_path).exists());

    // Reload through the mod VFS.
    let (vfs, loaded) = f2_mods::load_enabled_mods(&root, &mods, &["editor-test"]).expect("mod loads");
    assert_eq!(loaded.len(), 1);
    let md = Data { vfs: Arc::new(vfs), rendition: root.clone() };
    let doc2 = LevelDoc::load(&md, 0, 0).unwrap();
    assert_eq!(doc2.item_tile(i), Some(new), "entity at the new tile after reload");
    // Without the mod it is still at the old tile.
    assert_eq!(LevelDoc::load(&d, 0, 0).unwrap().item_tile(i), Some(old));

    // The game itself, loading the level through the mod VFS.
    let mut g = f2_game::Game::new();
    g.game_state.mode = f2_game::define::mode::INGAME_MODE;
    g.frogger.num_frogs = 1;
    g.load_level(0, 0, &md).expect("load_level through the mod VFS");
    let e = g.enemies.enemy_list.array.iter().find(|e| e.uid == uid && e.nme_actor.is_some()).expect("enemy created");
    let pos = g.actor2.actors.get(e.nme_actor.unwrap()).unwrap().actor.position;
    let dist = |t: usize| {
        let c = g.map.tiles[t].centre;
        ((c.vx as i64 - pos.vx as i64).pow(2) + (c.vy as i64 - pos.vy as i64).pow(2) + (c.vz as i64 - pos.vz as i64).pow(2)) as f64
    };
    let nearest = (0..g.map.tiles.len()).min_by(|&a, &b| dist(a).total_cmp(&dist(b))).unwrap();
    assert_eq!(nearest, new, "enemy {uid} spawned on the new tile (pos {pos:?})");
    let _ = std::fs::remove_dir_all(&mods);
}

/// Tile graph edits keep every reference consistent.
#[test]
fn tile_add_delete_and_links() {
    let Some(root) = rendition() else {
        eprintln!("skipping: no game data (F2_GAME_DIR)");
        return;
    };
    let mut doc = LevelDoc::load(&data(&root), 0, 0).unwrap();
    let n = doc.col.tiles.len();
    let t = doc.col.player_start[0] as usize;
    let added = doc.add_tile(t, 0).unwrap();
    assert_eq!(added, n);
    assert_eq!(doc.col.tiles[t].adj[0], n as i32);
    assert_eq!(doc.col.tiles[n].adj[1], t as i32);
    // Item positions survive deleting an unrelated earlier tile (indices shift).
    let before: Vec<_> = (0..doc.ents.entities.items.len()).map(|i| doc.item_pos(i)).collect();
    let victim = (0..n).find(|&v| v != t && (0..before.len()).all(|i| doc.item_tile(i) != Some(v))).unwrap();
    doc.delete_tile(victim).unwrap();
    assert_eq!(doc.col.tiles.len(), n);
    let after: Vec<_> = (0..doc.ents.entities.items.len()).map(|i| doc.item_pos(i)).collect();
    assert_eq!(before, after);
    for tile in &doc.col.tiles {
        assert!(tile.adj.iter().all(|&a| a < doc.col.tiles.len() as i32 && a != -2));
    }
    // The edited files parse again.
    for (path, bytes) in doc.files(false) {
        if path.ends_with(".col") {
            f2_formats::level::Collision::parse(&bytes).unwrap();
        } else if path.ends_with(".dat") {
            f2_formats::entity::Entities::parse(&bytes).unwrap();
        }
    }
}

/// Entity / path / scenic / script commands.
#[test]
fn commands() {
    let Some(root) = rendition() else {
        eprintln!("skipping: no game data (F2_GAME_DIR)");
        return;
    };
    let mut ed = editor(LevelDoc::load(&data(&root), 0, 0).unwrap());
    let n0 = ed.doc.ents.entities.items.len();
    let p0 = ed.doc.ents.entities.paths.len();
    ed.exec("new enemy bfly.obe 10").unwrap();
    assert_eq!(ed.doc.ents.entities.items.len(), n0 + 1);
    ed.exec("mode paths").unwrap();
    ed.exec("node add 11").unwrap();
    ed.exec("set speed 2.5").unwrap();
    ed.exec("set wait 30").unwrap();
    let p = ed.doc.item_path(n0).unwrap();
    let node = &ed.doc.ents.entities.paths[p].nodes[1];
    assert_eq!((node.speed, node.wait_time), (10240, 30));
    ed.exec("mode ents").unwrap();
    ed.exec("set model frog.obe").unwrap();
    ed.exec("set flags 0x10").unwrap();
    ed.exec("new garib 2 12").unwrap();
    ed.exec("new camera 13").unwrap();
    assert_eq!(ed.doc.ents.entities.items.len(), n0 + 3);
    let bytes = ed.doc.files(false).into_iter().find(|(p, _)| p.ends_with(".dat")).unwrap().1;
    let e = f2_formats::entity::Entities::parse(&bytes).unwrap();
    assert_eq!(e.items.len(), n0 + 3);
    match &e.items[n0] {
        Item::Actor(a) => assert_eq!((a.model.as_str(), a.flags), ("frog.obe", 0x10)),
        _ => panic!(),
    }
    ed.exec(&format!("sel item {n0}")).unwrap();
    ed.exec("del").unwrap();
    assert_eq!(ed.doc.ents.entities.paths.len(), p0, "unused path pruned");
    // Scenics.
    let s0 = ed.doc.scenics.len();
    ed.exec("new scenic tree 20").unwrap();
    ed.exec("set rot 0 0 0 4096").unwrap();
    assert_eq!(ed.doc.scenics.len(), s0 + 1);
    let sce = ed.doc.files(false).into_iter().find(|(p, _)| p.ends_with(".sce")).unwrap().1;
    assert_eq!(f2_formats::level::parse_scenics(&sce).unwrap().len(), s0 + 1);
    // Tiles.
    ed.exec("sel tile 5 6").unwrap();
    ed.exec("state ice").unwrap();
    assert_eq!(ed.doc.col.tiles[6].state, 3);
    // Script: decompile, recompile.
    if ed.doc.fev.is_some() {
        let before = ed.doc.fev.clone();
        ed.exec("script compile").unwrap();
        assert_eq!(ed.doc.fev, before, "decompile + compile reproduces the .fev");
        ed.doc.set_script_source("this is not a script".into()).unwrap_err();
        assert_eq!(ed.doc.fev, before, "a failed compile keeps the old .fev");
    }
    assert!(ed.exec("bogus").is_err());
}

/// Typing a command through the key handler.
#[test]
fn command_line_typing() {
    let Some(root) = rendition() else {
        eprintln!("skipping: no game data (F2_GAME_DIR)");
        return;
    };
    let mut ed = editor(LevelDoc::load(&data(&root), 0, 0).unwrap());
    ed.key(dik::RETURN);
    for k in [dik::S, dik::E, dik::L, dik::SPACE, dik::T, dik::I, dik::L, dik::E, dik::SPACE, dik::KEY_4] {
        ed.key(k);
    }
    assert_eq!(ed.prompt.as_deref(), Some("sel tile 4"));
    ed.key(dik::RETURN);
    assert!(ed.prompt.is_none());
    assert_eq!(ed.sel_tiles.iter().copied().collect::<Vec<_>>(), vec![4]);
    ed.key(dik::PRIOR);
    assert_ne!(ed.doc.col.tiles[4].state, LevelDoc::load(&data(&root), 0, 0).unwrap().col.tiles[4].state);
}
