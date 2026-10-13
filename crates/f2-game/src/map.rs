//! Port of the loading half of `Common/map.c`: collision tiles, camera boxes
//! and scenics. (Entities are created by `memload`.)

use crate::prelude::*;

use f2_formats::level;
use f2_math::{FVector, IQuaternion, SVector};
use serde::{Deserialize, Serialize};

use crate::ids::TileId;

/// `SCALE` (`Common/newpsx.h`): world positions in files are multiplied by this at load.
pub const SCALE: i32 = 10;

///
/// Phase 3S: `repr(C)` with `centre`/`normal` first (path positions and the nearest-tile
/// scan read only those); `ser_order!` keeps the serialized field order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct GameTile {
    pub centre: SVector,
    pub normal: SVector,
    pub state: u32,
    /// `tilePtrs[4]`: north, south, east, west.
    pub tile_ptrs: [Option<TileId>; 4],
    pub dir_vector: [SVector; 4],
}

crate::ser_order!(GameTile, "GameTile", GameTileWire {
    tile_ptrs: [Option<TileId>; 4],
    state: u32,
    centre: SVector,
    normal: SVector,
    dir_vector: [SVector; 4],
});

/// `CAM_PLANE` (`Common/Cam.h`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CamPlane {
    pub normal: FVector,
    pub k: i32,
    pub status: u32,
}

/// `CAM_BOX`: `planes` indexes `CameraBoxes::planes`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CamBox {
    pub num_planes: i32,
    pub first_plane: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CameraBoxes {
    pub planes: Vec<CamPlane>,
    pub boxes: Vec<CamBox>,
}

/// `SCENIC` (`Common/define.h`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scenic {
    pub name: String,
    pub pos: SVector,
    pub rot: IQuaternion,
}

/// Level geometry state owned by `map.c` (`firstTile`, `tileCount`, `gTStart`,
/// `bTStart`, `numBabies`, `cameraBoxes`, `firstScenic`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Map {
    pub tiles: Vec<GameTile>,
    /// `gTStart[4]`.
    pub g_t_start: [Option<TileId>; 4],
    /// `bTStart[]`.
    pub b_t_start: Vec<Option<TileId>>,
    pub num_babies: i32,
    pub camera_boxes: CameraBoxes,
    pub scenics: Vec<Scenic>,
    /// Phase 3S3: the nearest-tile search grid over `tiles` (a cache, not state).
    #[serde(skip)]
    pub(crate) grid: crate::nearest::TileGrid,
}

fn sv(v: level::SVec) -> SVector {
    SVector { vx: v.x, vy: v.y, vz: v.z, pad: v.pad }
}

/// `ScaleVector(&sv, SCALE)` on an `SVECTOR`: each `short` component multiplied
/// in `int` and truncated back to `short`.
fn scale_sv(v: SVector, s: i32) -> SVector {
    SVector { vx: (v.vx as i32).wrapping_mul(s) as i16, vy: (v.vy as i32).wrapping_mul(s) as i16, vz: (v.vz as i32).wrapping_mul(s) as i16, pad: v.pad }
}

fn tile_ref(i: i32, count: usize) -> Option<TileId> {
    (i >= 0 && (i as usize) < count).then_some(TileId(i as u32))
}

impl Map {
    pub fn tile(&self, t: TileId) -> &GameTile {
        &self.tiles[t.idx()]
    }

    pub fn tile_mut(&mut self, t: TileId) -> &mut GameTile {
        &mut self.tiles[t.idx()]
    }

    /// Port of `SetupCollisionTiles` + `SetupCameraBoxes` (`Common/map.c:410-528`)
    /// on a parsed `.col` file.
    pub fn load_collision(&mut self, c: &level::Collision) {
        let n = c.tiles.len();
        self.tiles = c
            .tiles
            .iter()
            .map(|t| GameTile {
                tile_ptrs: [tile_ref(t.adj[0], n), tile_ref(t.adj[1], n), tile_ref(t.adj[2], n), tile_ref(t.adj[3], n)],
                state: t.state,
                centre: scale_sv(sv(t.centre), SCALE),
                normal: sv(t.normal),
                dir_vector: [sv(t.dir_vector[0]), sv(t.dir_vector[1]), sv(t.dir_vector[2]), sv(t.dir_vector[3])],
            })
            .collect();
        self.grid = crate::nearest::TileGrid::build(&self.tiles);
        for (pl, s) in c.player_start.iter().enumerate() {
            self.g_t_start[pl] = tile_ref(*s, n);
        }
        self.num_babies = c.baby_start.len() as i32;
        self.b_t_start = c.baby_start.iter().map(|&b| tile_ref(b, n)).collect();
        self.camera_boxes = CameraBoxes {
            planes: c
                .planes
                .iter()
                .map(|p| CamPlane { normal: FVector { vx: p.normal[0], vy: p.normal[1], vz: p.normal[2] }, k: p.k.wrapping_mul(10), status: p.status })
                .collect(),
            boxes: c.boxes.iter().map(|b| CamBox { num_planes: b.num_planes, first_plane: b.first_plane }).collect(),
        };
    }

    /// Port of the PC branch of `LoadScenics` (`Common/map.c:34`).
    pub fn load_scenics(&mut self, s: &[level::Scenic]) {
        self.scenics = s
            .iter()
            .map(|e| Scenic { name: e.name.clone(), pos: scale_sv(sv(e.pos), SCALE), rot: IQuaternion { x: e.rot.x, y: e.rot.y, z: e.rot.z, w: e.rot.w } })
            .collect();
    }
}
