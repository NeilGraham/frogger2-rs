//! Level data: collision tile graph (`.col`), scenic placement (`.sce`),
//! animation→sound maps (`.sam`). Values are kept raw (unscaled) exactly as
//! stored; the loaders in the game multiply positions by `SCALE` (10).

use crate::prelude::*;

use crate::read::{Reader, Result};

/// `SVECTOR` on PC (`PC/libgte.h`): four i16.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SVec {
    pub x: i16,
    pub y: i16,
    pub z: i16,
    pub pad: i16,
}

impl SVec {
    fn read(r: &mut Reader) -> Result<SVec> {
        Ok(SVec { x: r.i16()?, y: r.i16()?, z: r.i16()?, pad: r.i16()? })
    }
}

/// `GAMETILE` (`Common/define.h`), 68 bytes on disk.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tile {
    /// Adjacent tile indices: north, south, east, west (`-1` = none).
    pub adj: [i32; 4],
    pub state: u32,
    pub centre: SVec,
    pub normal: SVec,
    pub dir_vector: [SVec; 4],
}

/// `CAM_PLANE`: fixed-point normal, `k` (×10 at load) and status flags.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CamPlane {
    pub normal: [i32; 3],
    pub k: i32,
    pub status: u32,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CamBox {
    pub num_planes: i32,
    pub first_plane: i32,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub struct Collision {
    pub player_start: [i32; 4],
    pub baby_start: Vec<i32>,
    pub tiles: Vec<Tile>,
    pub planes: Vec<CamPlane>,
    pub boxes: Vec<CamBox>,
    /// Whether the file had the (optional) camera plane/box section.
    #[cfg_attr(feature = "serde", serde(default))]
    pub has_camera_section: bool,
}

impl Collision {
    /// `LoadCollision` + `SetupCollisionTiles` + `SetupCameraBoxes` (`Common/map.c`).
    pub fn parse(data: &[u8]) -> Result<Collision> {
        let mut r = Reader::new("col", data);
        let count = r.i32()?;
        if !(0..=100_000).contains(&count) {
            return Err(r.err(format!("implausible tile count {count}")));
        }
        let mut player_start = [0i32; 4];
        for p in &mut player_start {
            *p = r.i32()?;
        }
        let nb = r.i32()?;
        if !(0..=64).contains(&nb) {
            return Err(r.err(format!("implausible baby count {nb}")));
        }
        let baby_start = (0..nb).map(|_| r.i32()).collect::<Result<Vec<_>>>()?;
        let mut tiles = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let adj = [r.i32()?, r.i32()?, r.i32()?, r.i32()?];
            let state = r.u32()?;
            let centre = SVec::read(&mut r)?;
            let normal = SVec::read(&mut r)?;
            let dir_vector = [SVec::read(&mut r)?, SVec::read(&mut r)?, SVec::read(&mut r)?, SVec::read(&mut r)?];
            tiles.push(Tile { adj, state, centre, normal, dir_vector });
        }
        for (i, t) in tiles.iter().enumerate() {
            for &a in &t.adj {
                if a != -1 && !(0..count).contains(&a) {
                    return Err(r.err(format!("tile {i} links to bad tile {a}")));
                }
            }
        }
        let (mut planes, mut boxes) = (Vec::new(), Vec::new());
        let has_camera_section = !r.is_empty();
        if has_camera_section {
            let np = r.i32()?;
            for _ in 0..np.max(0) {
                planes.push(CamPlane { normal: [r.i32()?, r.i32()?, r.i32()?], k: r.i32()?, status: r.u32()? });
            }
            let nbx = r.i32()?;
            for _ in 0..nbx.max(0) {
                boxes.push(CamBox { num_planes: r.i32()?, first_plane: r.i32()? });
            }
        }
        if !r.is_empty() {
            return Err(r.err(format!("{} trailing bytes", r.remaining())));
        }
        Ok(Collision { player_start, baby_start, tiles, planes, boxes, has_camera_section })
    }
}

/// `IQUATERNION`: four i32 (12-bit fixed point).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IQuat {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub w: i32,
}

/// `SCENIC` (`Common/define.h`), 44 bytes, chained by byte offsets.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scenic {
    pub name: String,
    pub pos: SVec,
    pub rot: IQuat,
    /// The 16 name bytes as stored (padding after the NUL kept for round-trips).
    #[cfg_attr(feature = "serde", serde(default))]
    pub name_raw: [u8; 16],
}

/// `LoadScenics` (`Common/map.c`): follows the `next` chain like the game.
pub fn parse_scenics(data: &[u8]) -> Result<Vec<Scenic>> {
    let mut r = Reader::new("sce", data);
    let count = r.i32()?;
    let base = 4usize;
    let mut out = Vec::new();
    let mut off = 0usize;
    loop {
        let mut e = Reader::at("sce", data, base + off);
        let next = e.i32()?;
        let name_raw: [u8; 16] = e.bytes(16)?.try_into().unwrap();
        let name = crate::read::cstr(&name_raw);
        let pos = SVec::read(&mut e)?;
        let rot = IQuat { x: e.i32()?, y: e.i32()?, z: e.i32()?, w: e.i32()? };
        out.push(Scenic { name, pos, rot, name_raw });
        if next == -1 {
            break;
        }
        if next <= off as i32 || out.len() > 100_000 {
            return Err(e.err(format!("scenic chain does not advance ({next})")));
        }
        off = next as usize;
    }
    if out.len() != count as usize {
        // The game ignores the count and walks the chain; report but keep the chain.
        log_mismatch(count, out.len());
    }
    Ok(out)
}

#[cfg(not(feature = "std"))]
fn log_mismatch(_count: i32, _chained: usize) {}

#[cfg(feature = "std")]
fn log_mismatch(count: i32, chained: usize) {
    if std::env::var_os("F2_FORMATS_DEBUG").is_some() {
        eprintln!("sce: header count {count} but chain has {chained} entries");
    }
}

/// One record of an animation→sound map (`LoadSfxMapping`, `Common/general.c:330`).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SfxMapping {
    pub uid: i32,
    pub kind: i32,
    pub samples: Vec<i32>,
}

/// Sample slots per record kind (`LoadSfxMapping`): frog = `NUM_FROG_ANIMS` (43),
/// multiplayer frog = `MULTI_NUM_ANIMS` (17), enemy = `NUM_NME_ANIMS` (4), scenic = 1.
pub fn sfx_mapping_slots(kind: i32) -> usize {
    match kind {
        0 => 43,
        1 => 17,
        2 => 4,
        3 | 4 => 1,
        _ => 0,
    }
}

/// Ambient sound placed on a tile (kind 4 with `uid < tileCount`).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AmbientSfx {
    pub tile: i32,
    pub sample: i32,
}

/// Parses `maps/sfxanim<world><level hex>.sam`. `tile_count` comes from the
/// level's collision file. Mirrors the loop bound `count < size - 4`.
pub fn parse_sfx_mapping(data: &[u8], tile_count: usize) -> Result<(Vec<SfxMapping>, Vec<AmbientSfx>)> {
    let mut r = Reader::new("sam", data);
    let (mut out, mut amb) = (Vec::new(), Vec::new());
    let limit = data.len().saturating_sub(4);
    while r.pos < limit {
        let uid = r.i32()?;
        let kind = r.i32()?;
        if kind == 4 && uid < tile_count as i32 {
            amb.push(AmbientSfx { tile: uid, sample: r.i32()? });
            continue;
        }
        let n = sfx_mapping_slots(kind);
        let samples = (0..n).map(|_| r.i32()).collect::<Result<Vec<_>>>()?;
        out.push(SfxMapping { uid, kind, samples });
    }
    Ok((out, amb))
}

// ---------------------------------------------------------------------------
// Writers (inverse of the parsers above; used by the level editor).

fn put_i32(o: &mut Vec<u8>, v: i32) {
    o.extend_from_slice(&v.to_le_bytes());
}

impl SVec {
    fn write(&self, o: &mut Vec<u8>) {
        for v in [self.x, self.y, self.z, self.pad] {
            o.extend_from_slice(&v.to_le_bytes());
        }
    }
}

impl Collision {
    /// Serialises to the `.col` layout read by [`Collision::parse`]. The camera
    /// plane/box section is written when present in the source or non-empty.
    pub fn write(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(24 + self.tiles.len() * 68);
        put_i32(&mut o, self.tiles.len() as i32);
        for v in self.player_start {
            put_i32(&mut o, v);
        }
        put_i32(&mut o, self.baby_start.len() as i32);
        for &v in &self.baby_start {
            put_i32(&mut o, v);
        }
        for t in &self.tiles {
            for a in t.adj {
                put_i32(&mut o, a);
            }
            put_i32(&mut o, t.state as i32);
            t.centre.write(&mut o);
            t.normal.write(&mut o);
            for d in &t.dir_vector {
                d.write(&mut o);
            }
        }
        if self.has_camera_section || !self.planes.is_empty() || !self.boxes.is_empty() {
            put_i32(&mut o, self.planes.len() as i32);
            for p in &self.planes {
                for v in p.normal {
                    put_i32(&mut o, v);
                }
                put_i32(&mut o, p.k);
                put_i32(&mut o, p.status as i32);
            }
            put_i32(&mut o, self.boxes.len() as i32);
            for b in &self.boxes {
                put_i32(&mut o, b.num_planes);
                put_i32(&mut o, b.first_plane);
            }
        }
        o
    }
}

/// Serialises scenics to the `.sce` layout: header count, then 44-byte records
/// chained by `next` byte offsets (sequential, last = `-1`), as the game walks them.
/// Names keep their original NUL padding bytes when [`Scenic::name_raw`] still
/// matches the name.
pub fn write_scenics(list: &[Scenic]) -> Vec<u8> {
    let mut o = Vec::with_capacity(4 + list.len() * 44);
    put_i32(&mut o, list.len() as i32);
    for (i, s) in list.iter().enumerate() {
        let next = if i + 1 == list.len() { -1 } else { ((i + 1) * 44) as i32 };
        put_i32(&mut o, next);
        let mut name = [0u8; 16];
        if crate::read::cstr(&s.name_raw) == s.name {
            name = s.name_raw;
        } else {
            for (d, c) in name.iter_mut().zip(s.name.chars().take(15)) {
                *d = c as u32 as u8;
            }
        }
        o.extend_from_slice(&name);
        s.pos.write(&mut o);
        for v in [s.rot.x, s.rot.y, s.rot.z, s.rot.w] {
            put_i32(&mut o, v);
        }
    }
    o
}

/// One `.sam` record, in file order.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SamRecord {
    Mapping(SfxMapping),
    Ambient(AmbientSfx),
}

/// Lossless `.sam` file: records in order plus any bytes past the loader's
/// `size - 4` loop bound.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SamFile {
    pub records: Vec<SamRecord>,
    pub trailer: Vec<u8>,
}

impl SamFile {
    /// Same walk as [`parse_sfx_mapping`], keeping record order and trailing bytes.
    pub fn parse(data: &[u8], tile_count: usize) -> Result<SamFile> {
        let mut r = Reader::new("sam", data);
        let mut records = Vec::new();
        let limit = data.len().saturating_sub(4);
        while r.pos < limit {
            let uid = r.i32()?;
            let kind = r.i32()?;
            if kind == 4 && uid < tile_count as i32 {
                records.push(SamRecord::Ambient(AmbientSfx { tile: uid, sample: r.i32()? }));
                continue;
            }
            let n = sfx_mapping_slots(kind);
            let samples = (0..n).map(|_| r.i32()).collect::<Result<Vec<_>>>()?;
            records.push(SamRecord::Mapping(SfxMapping { uid, kind, samples }));
        }
        let trailer = data[r.pos.min(data.len())..].to_vec();
        Ok(SamFile { records, trailer })
    }

    pub fn write(&self) -> Vec<u8> {
        let mut o = Vec::new();
        for rec in &self.records {
            match rec {
                SamRecord::Ambient(a) => {
                    put_i32(&mut o, a.tile);
                    put_i32(&mut o, 4);
                    put_i32(&mut o, a.sample);
                }
                SamRecord::Mapping(m) => {
                    put_i32(&mut o, m.uid);
                    put_i32(&mut o, m.kind);
                    for &s in &m.samples {
                        put_i32(&mut o, s);
                    }
                }
            }
        }
        o.extend_from_slice(&self.trailer);
        o
    }
}
