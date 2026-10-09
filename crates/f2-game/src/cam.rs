//! `Common/cam.c`: camera and transition cameras. **Wave D.** This wave records the
//! `CreateAndAddTransCamera` calls of `MemLoadEntities` (one per camera-case node).

use f2_math::{FVector, Fixed, SVector};
use serde::{Deserialize, Serialize};

use crate::ids::TileId;
use crate::Game;

/// One `CreateAndAddTransCamera(tile, dirCamMustFace, &v, flags)` call plus the
/// fields memload sets on the result (`FOV`, `speed`, raw look-at bytes; the
/// `camLookAt` computation `<<4, MakeUnit, ScaleVector(1000)` is wave D).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransCameraDef {
    pub tile: Option<TileId>,
    pub dir_cam_must_face: u8,
    pub pos: SVector,
    pub flags: u8,
    pub fov: Fixed,
    /// `ToFixed(speed)`.
    pub speed: Fixed,
    pub target: [i8; 3],
}

// Transition camera kinds (`Cam.h:21`).
pub const STATIC_CAMERA: u8 = 0;
pub const FIXED_SOURCE: u8 = 1;
pub const LOOK_AT_FROG: u8 = 2;
pub const LOOK_AT_FROG_NOTILT: u8 = 3;
pub const LOOK_AT_TILE: u8 = 4;
pub const LOOK_IN_DIR: u8 = 5;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CamState {
    /// `camFacing[4]`.
    pub cam_facing: [i32; 4],
    /// `currCamSource`.
    pub curr_cam_source: FVector,
    /// `currCamTarget`.
    pub curr_cam_target: FVector,
    /// `camSource`.
    pub cam_source: FVector,
    /// `camTarget`.
    pub cam_target: FVector,
    /// `camControlMode`.
    pub cam_control_mode: i32,
    /// `startCamFacing`, `startFrogFacing`.
    pub start_cam_facing: i16,
    pub start_frog_facing: i16,
    /// `controlCamera`.
    pub control_camera: i8,
    /// `fixedPos`, `fixedDir`.
    pub fixed_pos: i8,
    pub fixed_dir: i8,
    /// `lastTile[4]`.
    pub last_tile: [Option<TileId>; 4],
    /// `prevCamFacing[4]`.
    pub prev_cam_facing: [i32; 4],
    /// `fixedUp`.
    pub fixed_up: i8,
    /// `playerFocus`.
    pub player_focus: i8,
    /// `transCamSpeedMult`, `yFOVNew`.
    pub trans_cam_speed_mult: Fixed,
    pub y_fov_new: Fixed,
    /// `AllocCamBlock(num)`.
    pub cam_blocks: i32,
    /// Transition cameras created by `MemLoadEntities` (wave D builds `TRANSCAMERA`s).
    pub trans_camera_defs: Vec<TransCameraDef>,
}

impl Game {
    /// `AllocCamBlock` (Common/cam.c).
    pub fn alloc_cam_block(&mut self, num: i32) {
        // TODO(port wave D)
        self.cam.cam_blocks = num;
    }

    /// `CreateAndAddTransCamera` (Common/cam.c) + the field setup in `MemLoadEntities`.
    pub fn create_and_add_trans_camera(&mut self, def: TransCameraDef) {
        // TODO(port wave D)
        self.cam.trans_camera_defs.push(def);
    }

    /// Port of `CheckForDynamicCameraChange` (Common/cam.c:262): the transition camera
    /// of `tile` (the most recently created one wins) sets the camera mode flags and
    /// `camFacing`. TODO(port wave D): the camera vectors (`camDist`, `camSource`,
    /// `camTarget`, `currCamOffset`).
    pub fn check_for_dynamic_camera_change(&mut self, tile: Option<TileId>, pl: i32) {
        if self.cam.cam_control_mode != 0 {
            return;
        }
        let p = pl as usize;
        let n = (self.cam.trans_camera_defs.len() as i32).min(self.cam.cam_blocks.max(0)) as usize;
        for i in (0..n).rev() {
            let cur = self.cam.trans_camera_defs[i];
            if cur.tile != tile {
                continue;
            }
            let c = &mut self.cam;
            match cur.flags {
                LOOK_AT_FROG => (c.fixed_dir, c.fixed_pos, c.fixed_up) = (0, 0, 0),
                LOOK_AT_FROG_NOTILT => (c.fixed_dir, c.fixed_pos, c.fixed_up) = (0, 0, 1),
                STATIC_CAMERA => (c.fixed_dir, c.fixed_pos, c.fixed_up) = (1, 1, 0),
                FIXED_SOURCE => (c.fixed_dir, c.fixed_pos, c.fixed_up) = (0, 1, 0),
                LOOK_AT_TILE => (c.fixed_dir, c.fixed_pos, c.fixed_up) = (1, 1, 0),
                LOOK_IN_DIR => (c.fixed_dir, c.fixed_pos, c.fixed_up) = (1, 1, 1),
                _ => {}
            }
            if cur.dir_cam_must_face != 0 {
                c.cam_facing[p] = cur.dir_cam_must_face as i32 - 1;
                self.frogger.player[p].extended_hop_dir = ((self.frogmove.frog_facing[p] - self.cam.cam_facing[p]) & 3) as u8;
            }
            self.cam.trans_cam_speed_mult = if cur.speed != 0 { cur.speed } else { 4096 };
            self.cam.y_fov_new = cur.fov;
            break;
        }
    }

    /// Port of `InitCamera` (Common/cam.c:749), the gameplay part (`camFacing`,
    /// `lastTile`, `controlCamera`). TODO(port wave D): camera vectors, SlurpCamPosition.
    pub fn init_camera(&mut self) {
        if self.game_state.multi != crate::define::MULTILOCAL {
            self.cam.last_tile[0] = None;
            self.check_for_dynamic_camera_change(self.frogmove.curr_tile[0], 0);
        } else {
            for i in 0..self.frogger.num_frogs.clamp(0, 4) as usize {
                self.cam.cam_facing[i] = 0;
                self.cam.last_tile[i] = None;
                self.check_for_dynamic_camera_change(self.frogmove.curr_tile[i], i as i32);
            }
        }
        self.cam.control_camera = 0;
        self.update_camera_position();
        for _ in 0..50 {
            self.update_camera_position();
        }
    }

    /// Port of `UpdateCameraPosition` (Common/cam.c:636), the gameplay part: following
    /// the frog's tile changes with `camFacing` and the transition cameras.
    /// TODO(port wave D): CameraLookAtFrog, CheckCameraBoxes, camera interpolation.
    pub fn update_camera_position(&mut self) {
        if self.cam.cam_control_mode != 0 {
            return;
        }
        let pf = self.cam.player_focus as usize & 3;
        if self.frogger.frog[pf].is_none() || self.frogmove.curr_tile[pf].is_none() || self.cam.control_camera != 0 {
            return;
        }
        for i in 0..self.frogger.num_frogs.clamp(0, 4) as usize {
            let ct = self.frogmove.curr_tile[i];
            if ct != self.cam.last_tile[i] {
                if let (Some(c), Some(l)) = (ct, self.cam.last_tile[i]) {
                    if self.cam.cam_facing[i] == self.cam.prev_cam_facing[i] {
                        self.cam.cam_facing[i] = crate::frogmove::get_tiles_matching_direction(self.map.tile(l), self.cam.cam_facing[i], self.map.tile(c));
                    }
                }
                self.cam.prev_cam_facing[i] = self.cam.cam_facing[i];
                self.cam.last_tile[i] = ct;
                self.check_for_dynamic_camera_change(ct, i as i32);
            }
        }
    }
}
