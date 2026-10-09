//! Level data: collision tile graph (`.col`), scenic placement (`.sce`),
//! animation→sound maps (`.sam`). Values are kept raw (unscaled) exactly as
//! stored; the loaders in the game multiply positions by `SCALE` (10).

use crate::read::{Reader, Result};

/// `SVECTOR` on PC (`PC/libgte.h`): four i16.
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CamPlane {
    pub normal: [i32; 3],
    pub k: i32,
    pub status: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CamBox {
    pub num_planes: i32,
    pub first_plane: i32,
}

#[derive(Clone, Debug)]
pub struct Collision {
    pub player_start: [i32; 4],
    pub baby_start: Vec<i32>,
    pub tiles: Vec<Tile>,
    pub planes: Vec<CamPlane>,
    pub boxes: Vec<CamBox>,
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
        if !r.is_empty() {
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
        Ok(Collision { player_start, baby_start, tiles, planes, boxes })
    }
}

/// `IQUATERNION`: four i32 (12-bit fixed point).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IQuat {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub w: i32,
}

/// `SCENIC` (`Common/define.h`), 44 bytes, chained by byte offsets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scenic {
    pub name: String,
    pub pos: SVec,
    pub rot: IQuat,
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
        let name = e.fixed_str(16)?;
        let pos = SVec::read(&mut e)?;
        let rot = IQuat { x: e.i32()?, y: e.i32()?, z: e.i32()?, w: e.i32()? };
        out.push(Scenic { name, pos, rot });
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

fn log_mismatch(count: i32, chained: usize) {
    if std::env::var_os("F2_FORMATS_DEBUG").is_some() {
        eprintln!("sce: header count {count} but chain has {chained} entries");
    }
}

/// One record of an animation→sound map (`LoadSfxMapping`, `Common/general.c:330`).
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
