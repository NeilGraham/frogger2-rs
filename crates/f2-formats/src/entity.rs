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

fn fixed(r: &mut Reader) -> Result<i32> {
    Ok(r.i32()? >> 4)
}

fn place(r: &mut Reader, fast: bool) -> Result<Place> {
    if fast {
        Ok(Place::Tile(r.i32()?))
    } else {
        Ok(Place::Pos([r.i32()? >> 16, r.i32()? >> 16, r.i32()? >> 16]))
    }
}

impl Entities {
    /// Stored format version (without the "fast" bit). The game rejects files
    /// whose version differs from [`ENTITY_VERSION`]; retail ships a few stale ones.
    pub fn version(data: &[u8]) -> Option<u8> {
        data.first().map(|v| v & !0x40)
    }

    pub fn parse(data: &[u8]) -> Result<Entities> {
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
        for _ in 0..npaths {
            let n = r.i32()?;
            if !(0..=100_000).contains(&n) {
                return Err(r.err(format!("bad node count {n}")));
            }
            let mut nodes = Vec::with_capacity(n as usize);
            for _ in 0..n {
                let place = place(&mut r, fast)?;
                let offset = fixed(&mut r)?.wrapping_mul(10);
                let offset2 = fixed(&mut r)?.wrapping_mul(10);
                let speed = fixed(&mut r)?;
                let wait_time = r.i32()?;
                let sample_uid = r.u32()?;
                nodes.push(PathNode { place, offset, offset2, speed, wait_time, sample_uid });
            }
            paths.push(Path { nodes });
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
                    let scale = fixed(&mut r)?;
                    let radius = fixed(&mut r)?.wrapping_mul(10);
                    let anim_speed = r.i32()? >> 8;
                    let value1 = fixed(&mut r)?;
                    let facing = r.u8()?;
                    let obj_flags = r.i32()?;
                    let depth_shift = r.i8()?;
                    let dff_clipping = r.i8()?;
                    r.skip(2)?;
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
                }
                2 => {
                    let garib_type = r.u8()?;
                    let place = place(&mut r, fast)?;
                    let height_offset = fixed(&mut r)?;
                    items.push(Item::Garib(GaribDef { garib_type, place, height_offset }));
                }
                3 => {
                    let flags = r.i32()?;
                    let pos = [r.i32()? >> 16, r.i32()? >> 16, r.i32()? >> 16];
                    let fov = fixed(&mut r)?;
                    let speed = r.i32()? >> 16;
                    let t = r.i32()?.to_le_bytes();
                    let target = [t[0] as i8, t[1] as i8, t[2] as i8];
                    let n = r.i32()?;
                    if !(0..=100_000).contains(&n) {
                        return Err(r.err(format!("bad camera node count {n}")));
                    }
                    let nodes = (0..n).map(|_| place(&mut r, fast)).collect::<Result<Vec<_>>>()?;
                    items.push(Item::CameraCase(CameraCaseDef { flags, pos, fov, speed, target, nodes }));
                }
                t => return Err(r.err(format!("unknown entity type {t}"))),
            }
        }
        if !r.is_empty() {
            return Err(r.err(format!("{} trailing bytes", r.remaining())));
        }
        Ok(Entities { fast, total_path_nodes, paths, total_enemies, total_platforms, total_garibs, total_camera_cases, total_placeholders, items })
    }
}
