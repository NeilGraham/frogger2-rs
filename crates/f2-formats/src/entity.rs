//! Level entities `maps/entity-<world>-<level>.dat` (`MemLoadEntities`,
//! `Common/memload.c:107`). Packed little-endian byte stream.
//!
//! Values are decoded exactly as the loader does *before* any tile lookup:
//! `FIXED` fields are `int >> 4`, positions `int >> 16` (scaled ×10 later by
//! the game), radius `FIXED × 10`, anim speed `int >> 8`.

use crate::read::{Reader, Result};

pub const ENTITY_VERSION: u8 = 16;

/// Where a path node / garib / camera node lies: a tile index ("fast" files)
/// or a raw position to snap to the nearest tile.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Tile(i32),
    /// Position already shifted `>> 16`, NOT yet multiplied by `SCALE`.
    Pos([i32; 3]),
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathNode {
    pub place: Place,
    /// `MEMGETFIXED * SCALE` as loaded.
    pub offset: i32,
    pub offset2: i32,
    pub speed: i32,
    pub wait_time: i32,
    pub sample_uid: u32,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Path {
    pub nodes: Vec<PathNode>,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActorKind {
    Enemy,
    Platform,
    Placeholder,
}

/// Enemy / platform / placeholder record.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActorDef {
    pub kind: ActorKind,
    pub model: String,
    pub flags: i32,
    pub uid: i32,
    pub scale: i32,
    /// Already `× 10` like the loader.
    pub radius: i32,
    pub anim_speed: i32,
    pub value1: i32,
    pub facing: u8,
    pub obj_flags: i32,
    pub depth_shift: i8,
    pub dff_clipping: i8,
    pub effects: i32,
    pub start_node: i32,
    pub path_index: i16,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GaribDef {
    pub garib_type: u8,
    pub place: Place,
    pub height_offset: i32,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CameraCaseDef {
    pub flags: i32,
    /// Raw `>> 16` (×10 at load).
    pub pos: [i32; 3],
    pub fov: i32,
    pub speed: i32,
    /// Look-at direction as three signed bytes (×16 at load).
    pub target: [i8; 3],
    pub nodes: Vec<Place>,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Actor(ActorDef),
    Garib(GaribDef),
    CameraCase(CameraCaseDef),
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub struct Entities {
    pub fast: bool,
    pub total_path_nodes: i16,
    pub paths: Vec<Path>,
    pub total_enemies: i16,
    pub total_platforms: i16,
    pub total_garibs: i16,
    pub total_camera_cases: i16,
    pub total_placeholders: i16,
    pub items: Vec<Item>,
}

/// An entity file kept losslessly: the decoded [`Entities`] plus the bits the
/// loader drops, so [`EntityFile::write`] reproduces retail files byte-for-byte.
/// Edit through the helpers to keep `residue` aligned with `paths`/`items`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub struct EntityFile {
    pub entities: Entities,
    #[cfg_attr(feature = "serde", serde(default))]
    pub residue: Residue,
}

/// Low bits of fixed-point fields that the loader's shifts discard, per record.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Residue {
    /// Per path, per node: `[pos x, pos y, pos z, offset, offset2, speed]`.
    pub nodes: Vec<Vec<[u16; 6]>>,
    pub items: Vec<ItemResidue>,
}

/// Actor: `lo = [scale, radius, anim_speed, value1, ..]`, `pad` = bytes after
/// `dff_clipping`. Garib: `lo = [pos x, pos y, pos z, height_offset, ..]`.
/// Camera case: `lo = [pos x, pos y, pos z, fov, speed, target[3]]`, `nodes` per node.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemResidue {
    pub lo: [u16; 6],
    pub pad: [u8; 2],
    pub nodes: Vec<[u16; 3]>,
}

/// `MEMGETFIXED`: `int >> 4`, returning the dropped low bits too.
fn fixed(r: &mut Reader) -> Result<(i32, u16)> {
    let v = r.i32()?;
    Ok((v >> 4, (v & 0xf) as u16))
}

/// `int >> 16` with the dropped low bits.
fn hi16(r: &mut Reader) -> Result<(i32, u16)> {
    let v = r.i32()?;
    Ok((v >> 16, (v & 0xffff) as u16))
}

fn place(r: &mut Reader, fast: bool) -> Result<(Place, [u16; 3])> {
    if fast {
        Ok((Place::Tile(r.i32()?), [0; 3]))
    } else {
        let (x, a) = hi16(r)?;
        let (y, b) = hi16(r)?;
        let (z, c) = hi16(r)?;
        Ok((Place::Pos([x, y, z]), [a, b, c]))
    }
}

impl Entities {
    /// Stored format version (without the "fast" bit). The game rejects files
    /// whose version differs from [`ENTITY_VERSION`]; retail ships a few stale ones.
    pub fn version(data: &[u8]) -> Option<u8> {
        data.first().map(|v| v & !0x40)
    }

    pub fn parse(data: &[u8]) -> Result<Entities> {
        Ok(EntityFile::parse(data)?.entities)
    }
}

impl EntityFile {
    pub fn parse(data: &[u8]) -> Result<EntityFile> {
        let mut r = Reader::new("entity", data);
        let mut v = r.u8()?;
        let fast = v & 0x40 != 0;
        v &= !0x40;
        if v != ENTITY_VERSION {
            return Err(r.err(format!("entity version {v}, expected {ENTITY_VERSION}")));
        }
        let total_path_nodes = r.i16()?;
        let npaths = r.i16()?;
        let mut paths = Vec::with_capacity(npaths.max(0) as usize);
        let mut residue = Residue::default();
        for _ in 0..npaths {
            let n = r.i32()?;
            if !(0..=100_000).contains(&n) {
                return Err(r.err(format!("bad node count {n}")));
            }
            let mut nodes = Vec::with_capacity(n as usize);
            let mut nres = Vec::with_capacity(n as usize);
            for _ in 0..n {
                let (place, pl) = place(&mut r, fast)?;
                let (offset, l3) = fixed(&mut r)?;
                let (offset2, l4) = fixed(&mut r)?;
                let (speed, l5) = fixed(&mut r)?;
                let wait_time = r.i32()?;
                let sample_uid = r.u32()?;
                nodes.push(PathNode {
                    place,
                    offset: offset.wrapping_mul(10),
                    offset2: offset2.wrapping_mul(10),
                    speed,
                    wait_time,
                    sample_uid,
                });
                nres.push([pl[0], pl[1], pl[2], l3, l4, l5]);
            }
            paths.push(Path { nodes });
            residue.nodes.push(nres);
        }
        let total_enemies = r.i16()?;
        let total_platforms = r.i16()?;
        let total_garibs = r.i16()?;
        let total_camera_cases = r.i16()?;
        let total_placeholders = r.i16()?;
        let count = r.i16()?;
        let mut items = Vec::with_capacity(count.max(0) as usize);
        for _ in 0..count {
            let thing = r.u8()?;
            match thing {
                0 | 1 | 4 => {
                    let n = r.u8()? as usize;
                    let model = crate::read::cstr(r.bytes(n)?);
                    let flags = r.i32()?;
                    let uid = r.i32()?;
                    let (scale, l0) = fixed(&mut r)?;
                    let (radius, l1) = fixed(&mut r)?;
                    let radius = radius.wrapping_mul(10);
                    let a = r.i32()?;
                    let (anim_speed, l2) = (a >> 8, (a & 0xff) as u16);
                    let (value1, l3) = fixed(&mut r)?;
                    let facing = r.u8()?;
                    let obj_flags = r.i32()?;
                    let depth_shift = r.i8()?;
                    let dff_clipping = r.i8()?;
                    let pad = [r.u8()?, r.u8()?];
                    let effects = r.i32()?;
                    let start_node = r.i32()?;
                    let path_index = r.i16()?;
                    if path_index < 0 || path_index as usize >= paths.len() {
                        return Err(r.err(format!("actor {model} uses bad path {path_index}")));
                    }
                    let kind = match thing {
                        0 => ActorKind::Enemy,
                        1 => ActorKind::Platform,
                        _ => ActorKind::Placeholder,
                    };
                    items.push(Item::Actor(ActorDef {
                        kind,
                        model,
                        flags,
                        uid,
                        scale,
                        radius,
                        anim_speed,
                        value1,
                        facing,
                        obj_flags,
                        depth_shift,
                        dff_clipping,
                        effects,
                        start_node,
                        path_index,
                    }));
                    residue.items.push(ItemResidue { lo: [l0, l1, l2, l3, 0, 0], pad, nodes: Vec::new() });
                }
                2 => {
                    let garib_type = r.u8()?;
                    let (place, pl) = place(&mut r, fast)?;
                    let (height_offset, lh) = fixed(&mut r)?;
                    items.push(Item::Garib(GaribDef { garib_type, place, height_offset }));
                    residue.items.push(ItemResidue { lo: [pl[0], pl[1], pl[2], lh, 0, 0], ..Default::default() });
                }
                3 => {
                    let flags = r.i32()?;
                    let (x, l0) = hi16(&mut r)?;
                    let (y, l1) = hi16(&mut r)?;
                    let (z, l2) = hi16(&mut r)?;
                    let (fov, l3) = fixed(&mut r)?;
                    let (speed, l4) = hi16(&mut r)?;
                    let t = r.i32()?.to_le_bytes();
                    let target = [t[0] as i8, t[1] as i8, t[2] as i8];
                    let n = r.i32()?;
                    if !(0..=100_000).contains(&n) {
                        return Err(r.err(format!("bad camera node count {n}")));
                    }
                    let (nodes, nodes_lo) = (0..n).map(|_| place(&mut r, fast)).collect::<Result<(Vec<_>, Vec<_>)>>()?;
                    items.push(Item::CameraCase(CameraCaseDef {
                        flags,
                        pos: [x, y, z],
                        fov,
                        speed,
                        target,
                        nodes,
                    }));
                    residue.items.push(ItemResidue { lo: [l0, l1, l2, l3, l4, t[3] as u16], pad: [0; 2], nodes: nodes_lo });
                }
                t => return Err(r.err(format!("unknown entity type {t}"))),
            }
        }
        if !r.is_empty() {
            return Err(r.err(format!("{} trailing bytes", r.remaining())));
        }
        Ok(EntityFile { entities: Entities { fast, total_path_nodes, paths, total_enemies, total_platforms, total_garibs, total_camera_cases, total_placeholders, items }, residue })
    }
}

// ---------------------------------------------------------------------------
// Writer (inverse of `parse`; layout of the editor's `SaveCreateList`,
// `Editor/editfile.c:385`).

fn put_i32(o: &mut Vec<u8>, v: i32) {
    o.extend_from_slice(&v.to_le_bytes());
}

fn put_i16(o: &mut Vec<u8>, v: i16) {
    o.extend_from_slice(&v.to_le_bytes());
}

/// Inverse of [`fixed`]: `(v << 4) | lo`.
fn put_fixed(o: &mut Vec<u8>, v: i32, lo: u16) {
    put_i32(o, (v.wrapping_shl(4)) | (lo as i32 & 0xf));
}

fn put_hi16(o: &mut Vec<u8>, v: i32, lo: u16) {
    put_i32(o, (v.wrapping_shl(16)) | lo as i32);
}

/// Inverse of a `FIXED × 10` field: the stored value is `v / 10` (truncated
/// towards −∞ like the ×10 it undoes when `v` is a multiple of 10).
fn div10(v: i32) -> i32 {
    v.div_euclid(10)
}

fn put_place(o: &mut Vec<u8>, p: &Place, lo: [u16; 3], fast: bool) {
    match (*p, fast) {
        (Place::Tile(t), true) => put_i32(o, t),
        (Place::Pos(v), false) => {
            for k in 0..3 {
                put_hi16(o, v[k], lo[k]);
            }
        }
        // Mismatched place kind for the file mode: write what we can (a tile
        // index in a slow file cannot be resolved here; callers convert first).
        (Place::Tile(t), false) => {
            put_i32(o, t);
            put_i32(o, 0);
            put_i32(o, 0);
        }
        (Place::Pos(_), true) => put_i32(o, 0),
    }
}

impl Entities {
    /// An empty entity list (version 16, `fast` = tile-indexed places).
    pub fn new(fast: bool) -> Entities {
        Entities {
            fast,
            total_path_nodes: 0,
            paths: Vec::new(),
            total_enemies: 0,
            total_platforms: 0,
            total_garibs: 0,
            total_camera_cases: 0,
            total_placeholders: 0,
            items: Vec::new(),
        }
    }

    /// Recomputes the header totals the way the editor's `SaveCreateList` does:
    /// path nodes over all paths, items per kind (camera cases count their nodes).
    pub fn recount(&mut self) {
        self.total_path_nodes = self.paths.iter().map(|p| p.nodes.len()).sum::<usize>() as i16;
        let mut c = [0i32; 5];
        for it in &self.items {
            match it {
                Item::Actor(a) => {
                    c[match a.kind {
                        ActorKind::Enemy => 0,
                        ActorKind::Platform => 1,
                        ActorKind::Placeholder => 4,
                    }] += 1
                }
                Item::Garib(_) => c[2] += 1,
                Item::CameraCase(cc) => c[3] += cc.nodes.len() as i32,
            }
        }
        self.total_enemies = c[0] as i16;
        self.total_platforms = c[1] as i16;
        self.total_garibs = c[2] as i16;
        self.total_camera_cases = c[3] as i16;
        self.total_placeholders = c[4] as i16;
    }

    /// Serialises to the version-16 on-disk format (fast or slow per `self.fast`)
    /// with zero low bits; see [`EntityFile::write`] for lossless output.
    pub fn write(&self) -> Vec<u8> {
        self.write_with(&Residue::default())
    }

    /// Serialises with the given dropped low bits (missing entries count as zero).
    pub fn write_with(&self, residue: &Residue) -> Vec<u8> {
        let fast = self.fast;
        let mut o = Vec::new();
        o.push(ENTITY_VERSION | if fast { 0x40 } else { 0 });
        put_i16(&mut o, self.total_path_nodes);
        put_i16(&mut o, self.paths.len() as i16);
        for (pi, p) in self.paths.iter().enumerate() {
            put_i32(&mut o, p.nodes.len() as i32);
            for (ni, n) in p.nodes.iter().enumerate() {
                let lo = residue.nodes.get(pi).and_then(|r| r.get(ni)).copied().unwrap_or([0; 6]);
                put_place(&mut o, &n.place, [lo[0], lo[1], lo[2]], fast);
                put_fixed(&mut o, div10(n.offset), lo[3]);
                put_fixed(&mut o, div10(n.offset2), lo[4]);
                put_fixed(&mut o, n.speed, lo[5]);
                put_i32(&mut o, n.wait_time);
                put_i32(&mut o, n.sample_uid as i32);
            }
        }
        for v in [self.total_enemies, self.total_platforms, self.total_garibs, self.total_camera_cases, self.total_placeholders] {
            put_i16(&mut o, v);
        }
        put_i16(&mut o, self.items.len() as i16);
        let none = ItemResidue::default();
        for (ii, it) in self.items.iter().enumerate() {
            let res = residue.items.get(ii).unwrap_or(&none);
            let lo = res.lo;
            match it {
                Item::Actor(a) => {
                    o.push(match a.kind {
                        ActorKind::Enemy => 0,
                        ActorKind::Platform => 1,
                        ActorKind::Placeholder => 4,
                    });
                    let name: Vec<u8> = a.model.chars().map(|c| c as u32 as u8).take(255).collect();
                    o.push(name.len() as u8);
                    o.extend_from_slice(&name);
                    put_i32(&mut o, a.flags);
                    put_i32(&mut o, a.uid);
                    put_fixed(&mut o, a.scale, lo[0]);
                    put_fixed(&mut o, div10(a.radius), lo[1]);
                    put_i32(&mut o, a.anim_speed.wrapping_shl(8) | (lo[2] & 0xff) as i32);
                    put_fixed(&mut o, a.value1, lo[3]);
                    o.push(a.facing);
                    put_i32(&mut o, a.obj_flags);
                    o.push(a.depth_shift as u8);
                    o.push(a.dff_clipping as u8);
                    o.extend_from_slice(&res.pad);
                    put_i32(&mut o, a.effects);
                    put_i32(&mut o, a.start_node);
                    put_i16(&mut o, a.path_index);
                }
                Item::Garib(g) => {
                    o.push(2);
                    o.push(g.garib_type);
                    put_place(&mut o, &g.place, [lo[0], lo[1], lo[2]], fast);
                    put_fixed(&mut o, g.height_offset, lo[3]);
                }
                Item::CameraCase(c) => {
                    o.push(3);
                    put_i32(&mut o, c.flags);
                    for (&v, &l) in c.pos.iter().zip(&lo) {
                        put_hi16(&mut o, v, l);
                    }
                    put_fixed(&mut o, c.fov, lo[3]);
                    put_hi16(&mut o, c.speed, lo[4]);
                    o.extend_from_slice(&[c.target[0] as u8, c.target[1] as u8, c.target[2] as u8, lo[5] as u8]);
                    put_i32(&mut o, c.nodes.len() as i32);
                    for (i, n) in c.nodes.iter().enumerate() {
                        put_place(&mut o, n, res.nodes.get(i).copied().unwrap_or([0; 3]), fast);
                    }
                }
            }
        }
        o
    }
}

impl EntityFile {
    pub fn new(fast: bool) -> EntityFile {
        EntityFile { entities: Entities::new(fast), residue: Residue::default() }
    }

    /// Reproduces the parsed bytes exactly (v16).
    pub fn write(&self) -> Vec<u8> {
        self.entities.write_with(&self.residue)
    }

    /// Inserts an item (with no residue) keeping `residue` aligned.
    pub fn insert_item(&mut self, at: usize, item: Item) {
        self.residue.items.resize(self.entities.items.len(), ItemResidue::default());
        self.entities.items.insert(at, item);
        self.residue.items.insert(at, ItemResidue::default());
    }

    pub fn remove_item(&mut self, at: usize) -> Item {
        if at < self.residue.items.len() {
            self.residue.items.remove(at);
        }
        self.entities.items.remove(at)
    }

    /// Appends a path, returning its index.
    pub fn push_path(&mut self, path: Path) -> usize {
        self.residue.nodes.resize(self.entities.paths.len(), Vec::new());
        self.residue.nodes.push(vec![[0; 6]; path.nodes.len()]);
        self.entities.paths.push(path);
        self.entities.paths.len() - 1
    }

    pub fn insert_node(&mut self, path: usize, at: usize, node: PathNode) {
        self.residue.nodes.resize(self.entities.paths.len(), Vec::new());
        let r = &mut self.residue.nodes[path];
        r.resize(self.entities.paths[path].nodes.len(), [0; 6]);
        r.insert(at, [0; 6]);
        self.entities.paths[path].nodes.insert(at, node);
    }

    pub fn remove_node(&mut self, path: usize, at: usize) -> PathNode {
        if let Some(r) = self.residue.nodes.get_mut(path) {
            if at < r.len() {
                r.remove(at);
            }
        }
        self.entities.paths[path].nodes.remove(at)
    }
}
