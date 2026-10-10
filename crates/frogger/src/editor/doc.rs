//! The editor's level document: the retail-format files of one level, decoded
//! losslessly, with the editing operations (inspired by the original debug editor,
//! `FroggerProject/Editor/`) and saving to a mod folder.
//!
//! Pure data (no GPU, no game state): the UI in `editor/mod.rs` and the tests drive it.
//! Units are the files' raw units (collision/scenic positions unscaled; the game
//! multiplies by `SCALE` = 10 at load). Entity values are the decoded values of
//! `f2_formats::entity` (12-bit fixed point: 4096 = 1.0).

use f2_formats::entity::{ActorDef, ActorKind, CameraCaseDef, EntityFile, GaribDef, Item, Path, PathNode, Place, ENTITY_VERSION};
use f2_formats::level::{Collision, IQuat, SVec, SamFile, Scenic, Tile};
use f2_game::assets::DataSource;
use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};

/// Tile adjacency slot names (`tilePtrs` order).
pub const DIRS: [&str; 4] = ["n", "s", "e", "w"];

/// `TILESTATE_*` names (`define.h:86`).
pub fn tile_state_name(s: u32) -> &'static str {
    const N: [&str; 23] = [
        "NORMAL",
        "DEADLY",
        "SINK",
        "ICE",
        "SUPERHOP",
        "JOIN",
        "SAFE",
        "FINISHLINE",
        "GRAPPLE",
        "SMASH",
        "BARRED",
        "FALL",
        "FROGGER1AREA",
        "FROGGER2AREA",
        "FROGGER3AREA",
        "FROGGER4AREA",
        "DEADLYFALL",
        "HOT",
        "ELECTRIC",
        "NOSUPER",
        "PANTS",
        "NOSUPERHOT",
        "OCCUPIED",
    ];
    match s {
        0x20..=0x23 => "CONVEYOR",
        0x24..=0x27 => "CONVEYOR_MED",
        0x28..=0x2b => "CONVEYOR_FAST",
        0x2c..=0x2f => "CONVEYOR_ONEWAY",
        s => N.get(s as usize).copied().unwrap_or("?"),
    }
}

/// The states the editor cycles through (`NextTileSelectionState`, editdefs.c:666):
/// 0..=22 then the conveyor states with their direction bits.
pub fn next_tile_state(s: u32, up: bool) -> u32 {
    let all: Vec<u32> = (0..=22).chain(0x20..=0x2f).collect();
    let i = all.iter().position(|&x| x == s).unwrap_or(0);
    let n = all.len();
    all[if up { (i + 1) % n } else { (i + n - 1) % n }]
}

/// Which files changed since load/save.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dirty {
    pub col: bool,
    pub sce: bool,
    pub ent: bool,
    pub script: bool,
    pub sam: bool,
}

impl Dirty {
    pub fn any(&self) -> bool {
        self.col || self.sce || self.ent || self.script || self.sam
    }
}

/// One level's editable files.
pub struct LevelDoc {
    pub world: u32,
    pub level: u32,
    /// Virtual paths (lowercase, `/`), as the game requests them.
    pub col_path: String,
    pub sce_path: String,
    pub ent_path: String,
    pub fev_path: String,
    pub sam_path: String,
    pub col: Collision,
    pub scenics: Vec<Scenic>,
    pub ents: EntityFile,
    /// Whether the level shipped a usable (v16) entity file.
    pub had_entities: bool,
    /// Compiled script (`.fev`) as last loaded/compiled.
    pub fev: Option<Vec<u8>>,
    /// Script source (ETC), decompiled lazily or edited.
    pub script_src: Option<String>,
    pub sam: Option<SamFile>,
    /// Files changed since load (cumulative: a mod holds every edit).
    pub dirty: Dirty,
    /// The files as last saved (to report unsaved changes).
    saved: Vec<(String, Vec<u8>)>,
}

fn vpath(p: &str) -> String {
    p.replace('\\', "/").to_lowercase()
}

/// Squared distance of two raw positions.
fn dist2(a: [i32; 3], b: [i32; 3]) -> i64 {
    (0..3).map(|k| ((a[k] - b[k]) as i64).pow(2)).sum()
}

pub fn sv3(v: &SVec) -> [i32; 3] {
    [v.x as i32, v.y as i32, v.z as i32]
}

fn parse_num(s: &str) -> Result<i64, String> {
    let t = s.trim();
    let (neg, t) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t),
    };
    let v = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) { i64::from_str_radix(h, 16) } else { t.parse::<i64>() }
        .map_err(|_| format!("not a number: {s}"))?;
    Ok(if neg { -v } else { v })
}

/// Fixed-point input: `1.5` → 6144 (×4096), plain integers are raw values.
fn parse_fixed(s: &str) -> Result<i32, String> {
    if s.contains('.') {
        let f: f64 = s.trim().parse().map_err(|_| format!("not a number: {s}"))?;
        Ok((f * 4096.0).round() as i32)
    } else {
        Ok(parse_num(s)? as i32)
    }
}

impl LevelDoc {
    /// Loads `worldVisualData[world].levelVisualData[level]`'s files through `data`
    /// (the same file names `load_level` uses).
    pub fn load(data: &dyn DataSource, world: u32, level: u32) -> Result<LevelDoc, String> {
        let g = f2_game::Game::new();
        let wvd = g.layout.world_visual_data.get(world as usize).ok_or_else(|| format!("no world {world}"))?;
        let lvd = wvd.level_visual_data.get(level as usize).ok_or_else(|| format!("no level {world}-{level}"))?;
        let col_path =
            f2_game::bank_tables::collision_file_for_coll_bank(lvd.coll_bank).ok_or_else(|| format!("level {world}-{level} has no collision file"))?;
        let sce_path = f2_game::bank_tables::scenics_file_for_coll_bank(lvd.coll_bank).ok_or_else(|| format!("level {world}-{level} has no scenics file"))?;
        let (col_path, sce_path) = (vpath(col_path), vpath(sce_path));
        let ent_path = vpath(&f2_game::layout::entity_file_name(wvd.world_id, lvd.level_id));
        let fev_path = vpath(&f2_game::layout::script_file_name(wvd.world_id, lvd.level_id));
        let sam_path = vpath(&f2_game::general::sfx_mapping_file_name(world, level));

        let bytes = data.read(&col_path).ok_or_else(|| format!("missing {col_path}"))?;
        let col = Collision::parse(&bytes).map_err(|e| format!("{col_path}: {e}"))?;
        let bytes = data.read(&sce_path).ok_or_else(|| format!("missing {sce_path}"))?;
        let scenics = f2_formats::level::parse_scenics(&bytes).map_err(|e| format!("{sce_path}: {e}"))?;
        let (ents, had_entities) = match data.read(&ent_path) {
            Some(b) if f2_formats::entity::Entities::version(&b) == Some(ENTITY_VERSION) => {
                (EntityFile::parse(&b).map_err(|e| format!("{ent_path}: {e}"))?, true)
            }
            _ => (EntityFile::new(true), false),
        };
        let fev = data.read(&fev_path);
        let sam = match data.read(&sam_path) {
            Some(b) => Some(SamFile::parse(&b, col.tiles.len()).map_err(|e| format!("{sam_path}: {e}"))?),
            None => None,
        };
        Ok(LevelDoc {
            world,
            level,
            col_path,
            sce_path,
            ent_path,
            fev_path,
            sam_path,
            col,
            scenics,
            ents,
            had_entities,
            fev,
            script_src: None,
            sam,
            dirty: Dirty::default(),
            saved: Vec::new(),
        })
    }

    // ------------------------------------------------------------ geometry

    /// Raw centre of a tile.
    pub fn tile_pos(&self, t: usize) -> Option<[i32; 3]> {
        self.col.tiles.get(t).map(|t| sv3(&t.centre))
    }

    /// Typical distance between linked tiles (raw units).
    pub fn tile_spacing(&self) -> f32 {
        let (mut sum, mut n) = (0f64, 0usize);
        for t in self.col.tiles.iter().take(400) {
            for &a in &t.adj {
                if let Some(p) = (a >= 0).then(|| self.tile_pos(a as usize)).flatten() {
                    sum += (dist2(p, sv3(&t.centre)) as f64).sqrt();
                    n += 1;
                }
            }
        }
        if n == 0 {
            25.0
        } else {
            (sum / n as f64) as f32
        }
    }

    /// `FindNearestTile` on raw positions.
    pub fn nearest_tile(&self, p: [i32; 3]) -> Option<usize> {
        (0..self.col.tiles.len()).min_by_key(|&i| dist2(sv3(&self.col.tiles[i].centre), p))
    }

    pub fn place_pos(&self, p: &Place) -> Option<[i32; 3]> {
        match *p {
            Place::Tile(t) => self.tile_pos(t as usize),
            Place::Pos(v) => Some(v),
        }
    }

    pub fn place_tile(&self, p: &Place) -> Option<usize> {
        match *p {
            Place::Tile(t) => (t >= 0 && (t as usize) < self.col.tiles.len()).then_some(t as usize),
            Place::Pos(v) => self.nearest_tile(v),
        }
    }

    /// A place on tile `t` in this entity file's mode (fast: index, slow: position).
    fn place_on(&self, t: usize) -> Place {
        if self.ents.entities.fast {
            Place::Tile(t as i32)
        } else {
            Place::Pos(self.tile_pos(t).unwrap_or([0; 3]))
        }
    }

    /// Where an item sits (raw position): an actor's start node, a garib's tile,
    /// a camera case's first node.
    pub fn item_pos(&self, i: usize) -> Option<[i32; 3]> {
        match self.ents.entities.items.get(i)? {
            Item::Actor(a) => {
                let path = self.ents.entities.paths.get(a.path_index as usize)?;
                let n = path.nodes.get((a.start_node.max(0) as usize).min(path.nodes.len().saturating_sub(1)))?;
                self.place_pos(&n.place)
            }
            Item::Garib(g) => self.place_pos(&g.place),
            Item::CameraCase(c) => c.nodes.first().and_then(|p| self.place_pos(p)).or(Some(c.pos)),
        }
    }

    pub fn item_tile(&self, i: usize) -> Option<usize> {
        self.item_pos(i).and_then(|p| self.nearest_tile(p))
    }

    pub fn item_label(&self, i: usize) -> String {
        match self.ents.entities.items.get(i) {
            Some(Item::Actor(a)) => format!("{:?} {} uid {}", a.kind, a.model, a.uid),
            Some(Item::Garib(g)) => format!("Garib type {}", g.garib_type),
            Some(Item::CameraCase(c)) => format!("CameraCase flags {:#x} ({} tiles)", c.flags, c.nodes.len()),
            None => "-".into(),
        }
    }

    // ------------------------------------------------------------ tiles

    pub fn set_tile_state(&mut self, t: usize, state: u32) -> Result<(), String> {
        let tile = self.col.tiles.get_mut(t).ok_or_else(|| format!("no tile {t}"))?;
        if tile.state != state {
            tile.state = state;
            self.dirty.col = true;
        }
        Ok(())
    }

    /// Sets `tilePtrs[dir]` of `a` to `b` (`None` unlinks).
    pub fn link(&mut self, a: usize, dir: usize, b: Option<usize>) -> Result<(), String> {
        let n = self.col.tiles.len();
        if dir > 3 {
            return Err(format!("bad direction {dir}"));
        }
        if b.is_some_and(|b| b >= n) {
            return Err(format!("no tile {}", b.unwrap()));
        }
        let t = self.col.tiles.get_mut(a).ok_or_else(|| format!("no tile {a}"))?;
        t.adj[dir] = b.map_or(-1, |b| b as i32);
        self.dirty.col = true;
        Ok(())
    }

    /// The adjacency slot of `a` that points most towards `b` (by `dirVector`).
    pub fn dir_towards(&self, a: usize, b: usize) -> usize {
        let (pa, pb) = (self.tile_pos(a).unwrap_or([0; 3]), self.tile_pos(b).unwrap_or([0; 3]));
        let d = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
        let t = &self.col.tiles[a];
        (0..4)
            .max_by_key(|&k| {
                let v = sv3(&t.dir_vector[k]);
                (0..3).map(|i| v[i] as i64 * d[i] as i64).sum::<i64>()
            })
            .unwrap_or(0)
    }

    /// Adds a tile next to `from` in direction `dir` (a copy of `from` one tile
    /// spacing away) and links both ways. Returns the new tile's index.
    pub fn add_tile(&mut self, from: usize, dir: usize) -> Result<usize, String> {
        let src = self.col.tiles.get(from).ok_or_else(|| format!("no tile {from}"))?.clone();
        if dir > 3 {
            return Err(format!("bad direction {dir}"));
        }
        let c = sv3(&src.centre);
        // Spacing: distance to an existing neighbour, else 25 raw units (retail grid).
        let spacing = src
            .adj
            .iter()
            .filter(|&&a| a >= 0)
            .filter_map(|&a| self.tile_pos(a as usize))
            .map(|p| (dist2(p, c) as f64).sqrt())
            .next()
            .unwrap_or_else(|| self.tile_spacing() as f64);
        let dv = sv3(&src.dir_vector[dir]);
        let len = ((dv[0] as f64).powi(2) + (dv[1] as f64).powi(2) + (dv[2] as f64).powi(2)).sqrt().max(1.0);
        let off = |k: usize| (dv[k] as f64 / len * spacing).round() as i32;
        let mut t: Tile = src.clone();
        t.centre = SVec { x: (c[0] + off(0)) as i16, y: (c[1] + off(1)) as i16, z: (c[2] + off(2)) as i16, pad: src.centre.pad };
        t.adj = [-1; 4];
        let back = [1, 0, 3, 2][dir];
        t.adj[back] = from as i32;
        let idx = self.col.tiles.len();
        self.col.tiles.push(t);
        self.col.tiles[from].adj[dir] = idx as i32;
        self.dirty.col = true;
        Ok(idx)
    }

    /// Deletes a tile, renumbering every tile reference (links, starts, entity
    /// places, ambient sounds). References to the deleted tile are cleared or moved
    /// to the nearest remaining tile.
    pub fn delete_tile(&mut self, t: usize) -> Result<(), String> {
        let n = self.col.tiles.len();
        if t >= n {
            return Err(format!("no tile {t}"));
        }
        if n == 1 {
            return Err("cannot delete the last tile".into());
        }
        let pos = sv3(&self.col.tiles[t].centre);
        self.col.tiles.remove(t);
        let near = self.nearest_tile(pos).unwrap_or(0) as i32;
        let remap = |x: i32| -> i32 {
            match x {
                x if x < 0 => x,
                x if x as usize == t => -1,
                x if x as usize > t => x - 1,
                x => x,
            }
        };
        let remap_keep = |x: i32| -> i32 {
            match remap(x) {
                -1 if x == t as i32 => near,
                v => v,
            }
        };
        for tile in &mut self.col.tiles {
            for a in &mut tile.adj {
                *a = remap(*a);
            }
        }
        for s in &mut self.col.player_start {
            *s = remap_keep(*s);
        }
        for s in &mut self.col.baby_start {
            *s = remap_keep(*s);
        }
        let e = &mut self.ents.entities;
        let fix = |p: &mut Place| {
            if let Place::Tile(x) = p {
                *x = remap_keep(*x);
            }
        };
        for p in &mut e.paths {
            for nd in &mut p.nodes {
                fix(&mut nd.place);
            }
        }
        for it in &mut e.items {
            match it {
                Item::Garib(g) => fix(&mut g.place),
                Item::CameraCase(c) => c.nodes.iter_mut().for_each(fix),
                Item::Actor(_) => {}
            }
        }
        if let Some(sam) = &mut self.sam {
            for r in &mut sam.records {
                if let f2_formats::level::SamRecord::Ambient(a) = r {
                    a.tile = remap_keep(a.tile);
                }
            }
            self.dirty.sam = true;
        }
        self.dirty.col = true;
        self.dirty.ent = true;
        Ok(())
    }

    /// Sets player `pl`'s start tile (`gTStart`).
    pub fn set_player_start(&mut self, pl: usize, t: usize) -> Result<(), String> {
        if t >= self.col.tiles.len() || pl > 3 {
            return Err(format!("bad player/tile {pl}/{t}"));
        }
        self.col.player_start[pl] = t as i32;
        self.dirty.col = true;
        Ok(())
    }

    // ------------------------------------------------------------ entities

    fn next_uid(&self) -> i32 {
        self.ents.entities.items.iter().filter_map(|i| if let Item::Actor(a) = i { Some(a.uid) } else { None }).max().unwrap_or(0) + 1
    }

    /// Moves item `i` so that it starts on tile `t`. Paths and camera-case tile
    /// groups are translated as a whole and snapped to the nearest tiles (like
    /// dragging a selection in the original editor).
    pub fn move_item(&mut self, i: usize, t: usize) -> Result<(), String> {
        let to = self.tile_pos(t).ok_or_else(|| format!("no tile {t}"))?;
        let from = self.item_pos(i).ok_or_else(|| format!("no item {i}"))?;
        let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
        let shift = |doc: &LevelDoc, p: &Place| -> Place {
            let q = doc.place_pos(p).unwrap_or([0; 3]);
            let q = [q[0] + d[0], q[1] + d[1], q[2] + d[2]];
            doc.place_on(doc.nearest_tile(q).unwrap_or(t))
        };
        match self.ents.entities.items[i].clone() {
            Item::Actor(a) => {
                let pi = a.path_index as usize;
                // A path shared with other actors is copied first so only this one moves.
                let shared = self.ents.entities.items.iter().enumerate().any(|(j, it)| j != i && matches!(it, Item::Actor(b) if b.path_index == a.path_index));
                let mut path = self.ents.entities.paths[pi].clone();
                let start = (a.start_node.max(0) as usize).min(path.nodes.len().saturating_sub(1));
                for (k, n) in path.nodes.iter_mut().enumerate() {
                    n.place = if k == start { self.place_on(t) } else { shift(self, &n.place) };
                }
                if shared {
                    let np = self.ents.push_path(path);
                    if let Item::Actor(a) = &mut self.ents.entities.items[i] {
                        a.path_index = np as i16;
                    }
                } else {
                    for (k, n) in path.nodes.into_iter().enumerate() {
                        self.set_node_place(pi, k, n.place);
                    }
                }
            }
            Item::Garib(_) => {
                let p = self.place_on(t);
                if let Item::Garib(g) = &mut self.ents.entities.items[i] {
                    g.place = p;
                }
                self.clear_item_pos_residue(i);
            }
            Item::CameraCase(c) => {
                let nodes: Vec<Place> = c.nodes.iter().map(|p| shift(self, p)).collect();
                // Only the trigger tiles move; the camera settings stay.
                if let Item::CameraCase(c) = &mut self.ents.entities.items[i] {
                    c.nodes = nodes;
                }
                if let Some(r) = self.ents.residue.items.get_mut(i) {
                    r.nodes.iter_mut().for_each(|n| *n = [0; 3]);
                }
            }
        }
        self.dirty.ent = true;
        Ok(())
    }

    fn clear_item_pos_residue(&mut self, i: usize) {
        if let Some(r) = self.ents.residue.items.get_mut(i) {
            r.lo[..3].fill(0);
        }
    }

    fn set_node_place(&mut self, path: usize, node: usize, place: Place) {
        if let Some(n) = self.ents.entities.paths.get_mut(path).and_then(|p| p.nodes.get_mut(node)) {
            if n.place != place {
                n.place = place;
                if let Some(r) = self.ents.residue.nodes.get_mut(path).and_then(|r| r.get_mut(node)) {
                    r[..3].fill(0);
                }
            }
        }
    }

    /// Adds an enemy / platform / placeholder on tile `t` with a one-node path.
    pub fn add_actor(&mut self, kind: ActorKind, model: &str, t: usize) -> Result<usize, String> {
        if t >= self.col.tiles.len() {
            return Err(format!("no tile {t}"));
        }
        let node = PathNode { place: self.place_on(t), offset: 0, offset2: 0, speed: 4096, wait_time: 0, sample_uid: 0 };
        let pi = self.ents.push_path(Path { nodes: vec![node] });
        let a = ActorDef {
            kind,
            model: model.to_string(),
            flags: 0,
            uid: self.next_uid(),
            scale: 4096,
            radius: 0,
            anim_speed: 256,
            value1: 0,
            facing: 0,
            obj_flags: 0,
            depth_shift: 0,
            dff_clipping: 0,
            effects: 0,
            start_node: 0,
            path_index: pi as i16,
        };
        let at = self.ents.entities.items.len();
        self.ents.insert_item(at, Item::Actor(a));
        self.dirty.ent = true;
        Ok(at)
    }

    pub fn add_garib(&mut self, garib_type: u8, t: usize) -> Result<usize, String> {
        if t >= self.col.tiles.len() {
            return Err(format!("no tile {t}"));
        }
        let at = self.ents.entities.items.len();
        let place = self.place_on(t);
        self.ents.insert_item(at, Item::Garib(GaribDef { garib_type, place, height_offset: 0 }));
        self.dirty.ent = true;
        Ok(at)
    }

    /// A transition camera triggered on tile `t`: a copy of the level's last camera
    /// case's settings (or neutral defaults) with `t` as its only trigger tile.
    pub fn add_camera_case(&mut self, t: usize) -> Result<usize, String> {
        if t >= self.col.tiles.len() {
            return Err(format!("no tile {t}"));
        }
        let at = self.ents.entities.items.len();
        let place = self.place_on(t);
        let tmpl = self.ents.entities.items.iter().rev().find_map(|i| if let Item::CameraCase(c) = i { Some(c.clone()) } else { None });
        let c = match tmpl {
            Some(c) => CameraCaseDef { nodes: vec![place], ..c },
            None => CameraCaseDef { flags: 0, pos: [0, 30, -20], fov: 0, speed: 1, target: [0, 0, 0], nodes: vec![place] },
        };
        self.ents.insert_item(at, Item::CameraCase(c));
        self.dirty.ent = true;
        Ok(at)
    }

    /// Deletes item `i`; paths no longer used by any actor are removed.
    pub fn delete_item(&mut self, i: usize) -> Result<(), String> {
        if i >= self.ents.entities.items.len() {
            return Err(format!("no item {i}"));
        }
        self.ents.remove_item(i);
        self.prune_paths();
        self.dirty.ent = true;
        Ok(())
    }

    /// Removes unreferenced paths and renumbers `path_index`.
    pub fn prune_paths(&mut self) {
        let n = self.ents.entities.paths.len();
        let mut used = vec![false; n];
        for it in &self.ents.entities.items {
            if let Item::Actor(a) = it {
                if let Some(u) = used.get_mut(a.path_index as usize) {
                    *u = true;
                }
            }
        }
        if used.iter().all(|&u| u) {
            return;
        }
        let mut map = vec![0i16; n];
        let mut k = 0i16;
        for p in 0..n {
            map[p] = k;
            if used[p] {
                k += 1;
            }
        }
        for p in (0..n).rev() {
            if !used[p] {
                self.ents.entities.paths.remove(p);
                if p < self.ents.residue.nodes.len() {
                    self.ents.residue.nodes.remove(p);
                }
            }
        }
        for it in &mut self.ents.entities.items {
            if let Item::Actor(a) = it {
                a.path_index = map[a.path_index as usize];
            }
        }
    }

    /// The path of an actor item.
    pub fn item_path(&self, i: usize) -> Option<usize> {
        match self.ents.entities.items.get(i)? {
            Item::Actor(a) => Some(a.path_index as usize),
            _ => None,
        }
    }

    /// Appends (or inserts after `after`) a node on tile `t` to path `p`, copying
    /// the speed/offsets of its neighbour (`EditorAddFlag`).
    pub fn add_node(&mut self, p: usize, after: Option<usize>, t: usize) -> Result<usize, String> {
        if t >= self.col.tiles.len() {
            return Err(format!("no tile {t}"));
        }
        let path = self.ents.entities.paths.get(p).ok_or_else(|| format!("no path {p}"))?;
        let at = after.map_or(path.nodes.len(), |a| (a + 1).min(path.nodes.len()));
        let tmpl = path.nodes.get(at.saturating_sub(1)).cloned();
        let mut node = PathNode { place: self.place_on(t), offset: 0, offset2: 0, speed: 4096, wait_time: 0, sample_uid: 0 };
        if let Some(tm) = tmpl {
            node.offset = tm.offset;
            node.offset2 = tm.offset2;
            node.speed = tm.speed;
        }
        self.ents.insert_node(p, at, node);
        self.dirty.ent = true;
        Ok(at)
    }

    pub fn delete_node(&mut self, p: usize, n: usize) -> Result<(), String> {
        let path = self.ents.entities.paths.get(p).ok_or_else(|| format!("no path {p}"))?;
        if n >= path.nodes.len() {
            return Err(format!("no node {n}"));
        }
        if path.nodes.len() == 1 {
            return Err("a path needs at least one node".into());
        }
        self.ents.remove_node(p, n);
        for it in &mut self.ents.entities.items {
            if let Item::Actor(a) = it {
                if a.path_index as usize == p && a.start_node as usize >= n && a.start_node > 0 {
                    a.start_node -= 1;
                }
            }
        }
        self.dirty.ent = true;
        Ok(())
    }

    pub fn move_node(&mut self, p: usize, n: usize, t: usize) -> Result<(), String> {
        if t >= self.col.tiles.len() {
            return Err(format!("no tile {t}"));
        }
        let place = self.place_on(t);
        if self.ents.entities.paths.get(p).and_then(|x| x.nodes.get(n)).is_none() {
            return Err(format!("no node {p}/{n}"));
        }
        self.set_node_place(p, n, place);
        self.dirty.ent = true;
        Ok(())
    }

    /// Sets a path-node field: `speed`, `wait`, `offset`, `offset2`, `sample`.
    /// Fixed-point fields accept `1.5` (×4096) or raw integers.
    pub fn set_node_field(&mut self, p: usize, n: usize, field: &str, value: &str) -> Result<(), String> {
        let node = self.ents.entities.paths.get_mut(p).and_then(|x| x.nodes.get_mut(n)).ok_or_else(|| format!("no node {p}/{n}"))?;
        match field {
            "speed" => node.speed = parse_fixed(value)?,
            "wait" | "wait_time" => node.wait_time = parse_num(value)? as i32,
            "offset" => node.offset = parse_num(value)? as i32,
            "offset2" => node.offset2 = parse_num(value)? as i32,
            "sample" | "sample_uid" => node.sample_uid = parse_num(value)? as u32,
            f => return Err(format!("unknown node field {f} (speed wait offset offset2 sample)")),
        }
        self.dirty.ent = true;
        Ok(())
    }

    /// Field names per item kind (for the UI).
    pub fn item_fields(&self, i: usize) -> Vec<(&'static str, String)> {
        match self.ents.entities.items.get(i) {
            Some(Item::Actor(a)) => vec![
                ("model", a.model.clone()),
                ("flags", format!("{:#x}", a.flags)),
                ("uid", a.uid.to_string()),
                ("scale", a.scale.to_string()),
                ("radius", a.radius.to_string()),
                ("anim", a.anim_speed.to_string()),
                ("value1", a.value1.to_string()),
                ("facing", a.facing.to_string()),
                ("objflags", format!("{:#x}", a.obj_flags)),
                ("effects", format!("{:#x}", a.effects)),
                ("start", a.start_node.to_string()),
                ("path", a.path_index.to_string()),
            ],
            Some(Item::Garib(g)) => vec![("type", g.garib_type.to_string()), ("height", g.height_offset.to_string())],
            Some(Item::CameraCase(c)) => vec![
                ("flags", format!("{:#x}", c.flags)),
                ("pos", format!("{} {} {}", c.pos[0], c.pos[1], c.pos[2])),
                ("fov", c.fov.to_string()),
                ("speed", c.speed.to_string()),
                ("target", format!("{} {} {}", c.target[0], c.target[1], c.target[2])),
            ],
            None => Vec::new(),
        }
    }

    /// Sets an item field by name (see [`LevelDoc::item_fields`]).
    pub fn set_item_field(&mut self, i: usize, field: &str, value: &str) -> Result<(), String> {
        let npaths = self.ents.entities.paths.len();
        let it = self.ents.entities.items.get_mut(i).ok_or_else(|| format!("no item {i}"))?;
        let num = || parse_num(value).map(|v| v as i32);
        match it {
            Item::Actor(a) => match field {
                "model" => a.model = value.trim().to_string(),
                "flags" => a.flags = num()?,
                "uid" => a.uid = num()?,
                "scale" => a.scale = parse_fixed(value)?,
                "radius" => a.radius = num()?,
                "anim" | "anim_speed" => a.anim_speed = num()?,
                "value1" => a.value1 = parse_fixed(value)?,
                "facing" => a.facing = num()? as u8,
                "objflags" | "obj_flags" => a.obj_flags = num()?,
                "effects" => a.effects = num()?,
                "start" | "start_node" => a.start_node = num()?,
                "path" => {
                    let p = num()?;
                    if p < 0 || p as usize >= npaths {
                        return Err(format!("no path {p}"));
                    }
                    a.path_index = p as i16;
                }
                f => return Err(format!("unknown actor field {f}")),
            },
            Item::Garib(g) => match field {
                "type" => g.garib_type = num()? as u8,
                "height" => g.height_offset = num()?,
                f => return Err(format!("unknown garib field {f} (type height)")),
            },
            Item::CameraCase(c) => match field {
                "flags" => c.flags = num()?,
                "fov" => c.fov = num()?,
                "speed" => c.speed = num()?,
                "pos" | "target" => {
                    let v: Vec<i32> = value.split_whitespace().map(|s| parse_num(s).map(|v| v as i32)).collect::<Result<_, _>>()?;
                    if v.len() != 3 {
                        return Err(format!("{field} needs 3 values"));
                    }
                    if field == "pos" {
                        c.pos = [v[0], v[1], v[2]];
                    } else {
                        c.target = [v[0] as i8, v[1] as i8, v[2] as i8];
                    }
                }
                f => return Err(format!("unknown camera field {f} (flags pos fov speed target)")),
            },
        }
        self.dirty.ent = true;
        Ok(())
    }

    /// Adds / removes tile `t` to a camera case's trigger tiles.
    pub fn toggle_camera_tile(&mut self, i: usize, t: usize) -> Result<(), String> {
        let place = self.place_on(t);
        let tiles: Vec<Option<usize>> = match self.ents.entities.items.get(i) {
            Some(Item::CameraCase(c)) => c.nodes.iter().map(|p| self.place_tile(p)).collect(),
            _ => return Err("not a camera case".into()),
        };
        if let Some(Item::CameraCase(c)) = self.ents.entities.items.get_mut(i) {
            match tiles.iter().position(|&x| x == Some(t)) {
                Some(k) if c.nodes.len() > 1 => {
                    c.nodes.remove(k);
                    if let Some(r) = self.ents.residue.items.get_mut(i) {
                        if k < r.nodes.len() {
                            r.nodes.remove(k);
                        }
                    }
                }
                Some(_) => return Err("a camera case needs at least one tile".into()),
                None => c.nodes.push(place),
            }
        }
        self.dirty.ent = true;
        Ok(())
    }

    // ------------------------------------------------------------ scenics

    pub fn move_scenic(&mut self, i: usize, p: [i32; 3]) -> Result<(), String> {
        let s = self.scenics.get_mut(i).ok_or_else(|| format!("no scenic {i}"))?;
        s.pos = SVec { x: p[0] as i16, y: p[1] as i16, z: p[2] as i16, pad: s.pos.pad };
        self.dirty.sce = true;
        Ok(())
    }

    pub fn add_scenic(&mut self, name: &str, p: [i32; 3]) -> usize {
        self.scenics.push(Scenic {
            name: name.chars().take(15).collect(),
            pos: SVec { x: p[0] as i16, y: p[1] as i16, z: p[2] as i16, pad: 0 },
            rot: IQuat { x: 0, y: 0, z: 0, w: 4096 },
            name_raw: [0; 16],
        });
        self.dirty.sce = true;
        self.scenics.len() - 1
    }

    pub fn delete_scenic(&mut self, i: usize) -> Result<(), String> {
        if i >= self.scenics.len() {
            return Err(format!("no scenic {i}"));
        }
        if self.scenics.len() == 1 {
            // The game always reads at least one record.
            return Err("cannot delete the last scenic".into());
        }
        self.scenics.remove(i);
        self.dirty.sce = true;
        Ok(())
    }

    /// Sets a scenic field: `name`, `pos` (x y z), `rot` (x y z w, 4096 = 1.0).
    pub fn set_scenic_field(&mut self, i: usize, field: &str, value: &str) -> Result<(), String> {
        let s = self.scenics.get_mut(i).ok_or_else(|| format!("no scenic {i}"))?;
        let v = || -> Result<Vec<i32>, String> { value.split_whitespace().map(|s| parse_num(s).map(|v| v as i32)).collect() };
        match field {
            "name" => s.name = value.trim().chars().take(15).collect(),
            "pos" => match v()?[..] {
                [x, y, z] => s.pos = SVec { x: x as i16, y: y as i16, z: z as i16, pad: s.pos.pad },
                _ => return Err("pos needs x y z".into()),
            },
            "rot" => match v()?[..] {
                [x, y, z, w] => s.rot = IQuat { x, y, z, w },
                _ => return Err("rot needs x y z w".into()),
            },
            f => return Err(format!("unknown scenic field {f} (name pos rot)")),
        }
        self.dirty.sce = true;
        Ok(())
    }

    // ------------------------------------------------------------ script

    /// The script source (decompiled from the `.fev` on first use).
    pub fn script_source(&mut self) -> Result<&str, String> {
        if self.script_src.is_none() {
            let fev = self.fev.as_ref().ok_or_else(|| format!("level has no script ({})", self.fev_path))?;
            let d = f2_etc::decompile(fev, &self.fev_path)?;
            self.script_src = Some(String::from_utf8_lossy(&d.source).into_owned());
        }
        Ok(self.script_src.as_deref().unwrap())
    }

    /// Replaces the script source and compiles it (the `.fev` only changes when the
    /// compile succeeds). Returns warnings.
    pub fn set_script_source(&mut self, src: String) -> Result<Vec<String>, String> {
        let out = f2_etc::compile(&self.fev_path.replace(".fev", ".etc"), src.as_bytes())
            .map_err(|d| d.iter().map(|x| x.to_string()).collect::<Vec<_>>().join("\n"))?;
        self.fev = Some(out.data);
        self.script_src = Some(src);
        self.dirty.script = true;
        Ok(out.warnings.iter().map(|w| w.to_string()).collect())
    }

    // ------------------------------------------------------------ output

    /// The level's files in retail format: `(virtual path, bytes)`. With `all`
    /// false only the changed ones.
    pub fn files(&self, all: bool) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        let d = self.dirty;
        if all || d.col {
            out.push((self.col_path.clone(), self.col.write()));
        }
        if all || d.sce {
            out.push((self.sce_path.clone(), f2_formats::level::write_scenics(&self.scenics)));
        }
        if (all && self.had_entities) || d.ent {
            let mut e = self.ents.clone();
            if d.ent {
                e.entities.recount();
            }
            out.push((self.ent_path.clone(), e.write()));
        }
        if let (true, Some(f)) = (all || d.script, &self.fev) {
            out.push((self.fev_path.clone(), f.clone()));
        }
        if let (true, Some(s)) = (all || d.sam, &self.sam) {
            out.push((self.sam_path.clone(), s.write()));
        }
        out
    }

    /// Whether there are edits not yet written by [`LevelDoc::save_mod`].
    pub fn unsaved(&self) -> bool {
        self.dirty.any() && self.files(false) != self.saved
    }

    /// A data source serving the edited files over `base` (play-test).
    pub fn overlay(&self, base: std::sync::Arc<dyn DataSource + Send + Sync>) -> EditedData {
        EditedData { base, files: self.files(false).into_iter().collect() }
    }

    /// Writes the changed files (all files with `all`) into `mods_dir/<id>/game/...`,
    /// plus `mod.toml` when missing and the script source (`.etc`) next to the `.fev`.
    /// Returns the written paths.
    pub fn save_mod(&mut self, mods_dir: &FsPath, id: &str, all: bool) -> Result<Vec<PathBuf>, String> {
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(format!("invalid mod id {id:?} (use [A-Za-z0-9_-])"));
        }
        let root = mods_dir.join(id);
        let game = root.join("game");
        let mut written = Vec::new();
        let io = |p: &FsPath, e: std::io::Error| format!("{}: {e}", p.display());
        std::fs::create_dir_all(&game).map_err(|e| io(&game, e))?;
        let manifest = root.join("mod.toml");
        if !manifest.exists() {
            let text = format!(
                "id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\nengine_version = \">={}\"\ngameplay_affecting = true\ndescription = \"Level edits made with frogger --editor (level {}-{}).\"\n",
                f2_mods::ENGINE_API_VERSION,
                self.world,
                self.level
            );
            std::fs::write(&manifest, text).map_err(|e| io(&manifest, e))?;
            written.push(manifest);
        }
        let script_changed = self.dirty.script;
        for (path, bytes) in self.files(all) {
            let p = game.join(&path);
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d).map_err(|e| io(d, e))?;
            }
            std::fs::write(&p, bytes).map_err(|e| io(&p, e))?;
            written.push(p);
        }
        if script_changed || all {
            if let Some(src) = &self.script_src {
                let p = game.join(self.fev_path.replace(".fev", ".etc"));
                std::fs::write(&p, src).map_err(|e| io(&p, e))?;
                written.push(p);
            }
        }
        self.saved = self.files(false);
        Ok(written)
    }
}

/// Edited files layered over the real data (the play-test data source).
pub struct EditedData {
    pub base: std::sync::Arc<dyn DataSource + Send + Sync>,
    pub files: HashMap<String, Vec<u8>>,
}

impl DataSource for EditedData {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        match self.files.get(&vpath(path)) {
            Some(b) => Some(b.clone()),
            None => self.base.read(path),
        }
    }
}
