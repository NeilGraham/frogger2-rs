//! Port of `Common/cam.c` / `Cam.h`: the in-game camera (source/target interpolation,
//! camera boxes, transition cameras, shake).
//!
//! `currCamSource` / `currCamTarget` / `camVect` are what the PC renderer uses
//! (`PC/drawloop.cpp:68`); [`Game::camera_source_target`] exposes them. `SetCamFF`
//! only has a PSX body and is not ported. `ControlCamera` is `#ifndef FINAL_MASTER`
//! (debug fly camera) and dropped.

use f2_math::{
    add_to_vector_ff, add_vector_fff, distance_between_points_ff, dot_product_ff, fmul, magnitude_s, make_unit, scale_vector, scale_vector_ff, set_vector_fs,
    sub_vector_fff, sub_vector_sss, FVector, Fixed, SVector,
};
use serde::{Deserialize, Serialize};

use crate::define::{MULTILOCAL, SINGLEPLAYER};
use crate::effects::Effect;
use crate::frogger::UP_VEC;
use crate::frogmove::FROGSTATUS_ISDEAD;
use crate::ids::TileId;
use crate::map::{CamBox, CamPlane, SCALE};
use crate::Game;

/// One `CreateAndAddTransCamera(tile, dirCamMustFace, &v, flags)` call of
/// `MemLoadEntities` plus the fields it sets on the result (`FOV`, `speed`, raw
/// look-at bytes), recorded in creation order.
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

/// `TRANSCAMERA` (`Cam.h:35`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransCamera {
    pub tile: Option<TileId>,
    pub dir_cam_must_face: u8,
    pub flags: u8,
    pub cam_offset: SVector,
    pub cam_look_at: FVector,
    pub fov: Fixed,
    pub speed: Fixed,
}

/// `TRANSCAMERALIST` (`Cam.h:49`): `array` holds the `count` cameras created so far.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransCameraList {
    pub array: Vec<TransCamera>,
    pub count: i16,
    pub blocks: i16,
}

// Transition camera kinds (`Cam.h:21`).
pub const STATIC_CAMERA: u8 = 0;
pub const FIXED_SOURCE: u8 = 1;
pub const LOOK_AT_FROG: u8 = 2;
pub const LOOK_AT_FROG_NOTILT: u8 = 3;
pub const LOOK_AT_TILE: u8 = 4;
pub const LOOK_IN_DIR: u8 = 5;

/// Globals of `cam.c` (initial values as in the original).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CamState {
    /// `camControlMode`.
    pub cam_control_mode: i32,
    /// `cameraUpVect`.
    pub camera_up_vect: FVector,
    /// `camVect`: the camera up vector.
    pub cam_vect: FVector,
    /// `currCamSource`.
    pub curr_cam_source: FVector,
    /// `currCamTarget`.
    pub curr_cam_target: FVector,
    /// `currCamDist`.
    pub curr_cam_dist: FVector,
    /// `camSource`.
    pub cam_source: FVector,
    /// `camTarget`.
    pub cam_target: FVector,
    /// `currCamOffset`, `camOffset`: in floaty camera mode, the offset of the camera
    /// from the player.
    pub curr_cam_offset: FVector,
    pub cam_offset: FVector,
    /// `xFOVNew`, `yFOVNew`, `xFOV`, `yFOV`, `precScaleFactor`.
    pub x_fov_new: Fixed,
    pub y_fov_new: Fixed,
    pub x_fov: Fixed,
    pub y_fov: Fixed,
    pub prec_scale_factor: Fixed,
    /// `camLookOfs`, `camLookOfsNew`.
    pub cam_look_ofs: Fixed,
    pub cam_look_ofs_new: Fixed,
    /// `camDist`.
    pub cam_dist: SVector,
    /// `camSpeed`, `camSpeed2`, `camSpeed3` (source & target interp.), `fovSpd`.
    pub cam_speed: Fixed,
    pub cam_speed2: Fixed,
    pub cam_speed3: Fixed,
    pub fov_spd: Fixed,
    /// `transCamSpeedMult`.
    pub trans_cam_speed_mult: Fixed,
    /// `camFacing[4]`.
    pub cam_facing: [i32; 4],
    /// `startCamFacing`, `startFrogFacing`.
    pub start_cam_facing: i16,
    pub start_frog_facing: i16,
    /// `scaleV`.
    pub scale_v: Fixed,
    /// `controlCamera`, `fixedDir`, `fixedPos`, `firstPerson`, `fixedUp`.
    pub control_camera: i8,
    pub fixed_dir: i8,
    pub fixed_pos: i8,
    pub first_person: i8,
    pub fixed_up: i8,
    /// `camSideOfs`.
    pub cam_side_ofs: Fixed,
    /// `lastTile[4]`.
    pub last_tile: [Option<TileId>; 4],
    /// `prevCamFacing[4]`.
    pub prev_cam_facing: [i32; 4],
    /// `currCamBox`: index into `map.camera_boxes.boxes`.
    pub curr_cam_box: Option<usize>,
    /// `cam_edge_spacing`.
    pub cam_edge_spacing: Fixed,
    /// `cam_shakiness`, `cam_shake_falloff`.
    pub cam_shakiness: Fixed,
    pub cam_shake_falloff: Fixed,
    /// `playerFocus`.
    pub player_focus: i8,
    /// `tcList`.
    pub tc_list: TransCameraList,
    /// `AllocCamBlock(num)` (= `tcList.blocks`).
    pub cam_blocks: i32,
    /// Every `CreateAndAddTransCamera` call of `MemLoadEntities`, in order (including
    /// ones the full list rejected).
    pub trans_camera_defs: Vec<TransCameraDef>,
}

impl Default for CamState {
    fn default() -> Self {
        CamState {
            cam_control_mode: 0,
            camera_up_vect: FVector { vx: 0, vy: 4096, vz: 0 },
            cam_vect: FVector::default(),
            curr_cam_source: FVector { vx: 0, vy: 0, vz: -100 << 12 },
            curr_cam_target: FVector::default(),
            curr_cam_dist: FVector { vx: 0, vy: 0, vz: 10 << 12 },
            cam_source: FVector { vx: 0, vy: 569 << 12, vz: -200 << 12 },
            cam_target: FVector::default(),
            curr_cam_offset: FVector::default(),
            cam_offset: FVector { vx: 0, vy: 100 << 12, vz: 100 << 12 },
            x_fov_new: 5461,
            y_fov_new: 184320,
            x_fov: 5461,
            y_fov: 184320,
            prec_scale_factor: 4096,
            cam_look_ofs: 50 << 12,
            cam_look_ofs_new: 50 << 12,
            cam_dist: SvDist::INITIAL,
            cam_speed: 15 << 12,
            cam_speed2: 20 << 12,
            cam_speed3: 20 << 12,
            fov_spd: 2 << 12,
            trans_cam_speed_mult: 4096,
            cam_facing: [0; 4],
            start_cam_facing: 0,
            start_frog_facing: 0,
            scale_v: 4506,
            control_camera: 0,
            fixed_dir: 0,
            fixed_pos: 0,
            first_person: 0,
            fixed_up: 0,
            cam_side_ofs: 0,
            last_tile: [None; 4],
            prev_cam_facing: [0; 4],
            curr_cam_box: None,
            cam_edge_spacing: 100 << 12,
            cam_shakiness: 0,
            cam_shake_falloff: 410,
            player_focus: 0,
            tc_list: TransCameraList::default(),
            cam_blocks: 0,
            trans_camera_defs: Vec::new(),
        }
    }
}

struct SvDist;
impl SvDist {
    /// `camDist = {0, 680*SCALE, 192*SCALE}`.
    const INITIAL: SVector = SVector { vx: 0, vy: (680 * SCALE) as i16, vz: (192 * SCALE) as i16, pad: 0 };
}

/// `SetVectorFF(&f, &svector)`: the macro copies the shorts unshifted.
#[inline]
fn fv(s: &SVector) -> FVector {
    FVector { vx: s.vx as i32, vy: s.vy as i32, vz: s.vz as i32 }
}

/// The planes of `bx` (`box->planes[0..numPlanes]`).
fn box_planes<'a>(planes: &'a [CamPlane], bx: &CamBox) -> impl Iterator<Item = &'a CamPlane> {
    let first = bx.first_plane.max(0) as usize;
    planes.iter().skip(first).take(bx.num_planes.max(0) as usize)
}

/// Port of `GetCamLimitVector` (Common/cam.c:111): the push back inside `box` for a
/// point closer than `edge` to its planes. Returns the number of planes involved.
pub fn get_cam_limit_vector(out: &mut FVector, v: &FVector, planes: &[CamPlane], bx: Option<&CamBox>, edge: Fixed) -> i32 {
    let Some(bx) = bx else { return 0 };
    let mut count = 0;
    let mut res = FVector::default();
    for plane in box_planes(planes, bx) {
        if plane.status == 0 {
            continue;
        }
        let mut dist = dot_product_ff(v, &plane.normal).wrapping_sub(plane.k);
        if dist < edge {
            dist = dist.wrapping_sub(edge);
            let mut a = plane.normal;
            scale_vector_ff(&mut a, dist.wrapping_neg());
            add_to_vector_ff(&mut res, &a);
            count += 1;
        }
    }
    *out = res;
    count
}

/// Port of `IsPointInCameraBox` (Common/cam.c:150).
pub fn is_point_in_camera_box(v: &FVector, planes: &[CamPlane], bx: &CamBox) -> bool {
    box_planes(planes, bx).all(|plane| dot_product_ff(v, &plane.normal) >= plane.k)
}

impl Game {
    /// The camera for the renderer: (`currCamSource`, `currCamTarget`, `camVect`), 20.12
    /// world units (`PC/drawloop.cpp:68` scales them by `CAMVECTSCALE`).
    pub fn camera_source_target(&self) -> (FVector, FVector, FVector) {
        (self.cam.curr_cam_source, self.cam.curr_cam_target, self.cam.cam_vect)
    }

    /// Port of `CheckCameraBoxes` (Common/cam.c:173): checks the current camera box,
    /// handles transitions between boxes.
    pub fn check_camera_boxes(&mut self) {
        let cb = &self.map.camera_boxes;
        let inside = self.cam.curr_cam_box.and_then(|b| cb.boxes.get(b)).is_some_and(|b| is_point_in_camera_box(&self.cam.cam_target, &cb.planes, b));
        if !inside {
            self.cam.curr_cam_box = cb.boxes.iter().position(|b| is_point_in_camera_box(&self.cam.cam_target, &cb.planes, b));
        }
    }

    /// Port of `CreateAndAddTransCamera` (Common/cam.c:207) plus the field setup of
    /// `MemLoadEntities` (Common/memload.c:459): returns false (`NULL`) when the list
    /// is full.
    pub fn create_and_add_trans_camera(&mut self, def: TransCameraDef) -> bool {
        self.cam.trans_camera_defs.push(def);
        let l = &mut self.cam.tc_list;
        if l.count >= l.blocks {
            return false;
        }
        l.count += 1;
        // memload: camLookAt = target bytes << 4, MakeUnit, ScaleVector(1000)
        let mut look = FVector { vx: (def.target[0] as i32) << 4, vy: (def.target[1] as i32) << 4, vz: (def.target[2] as i32) << 4 };
        make_unit(&mut look);
        scale_vector(&mut look, 1000);
        l.array.push(TransCamera {
            tile: def.tile,
            dir_cam_must_face: def.dir_cam_must_face,
            flags: def.flags,
            cam_offset: def.pos,
            cam_look_at: look,
            // CreateAndAddTransCamera sets FOV 184320 / speed 4096; memload overwrites.
            fov: def.fov,
            speed: def.speed,
        });
        true
    }

    /// Port of `AllocCamBlock` (Common/cam.c:240).
    pub fn alloc_cam_block(&mut self, num: i32) {
        self.cam.tc_list.array = Vec::with_capacity(num.max(0) as usize);
        self.cam.tc_list.count = 0;
        self.cam.tc_list.blocks = num as i16;
        self.cam.cam_blocks = num;
    }

    /// Port of `CheckForDynamicCameraChange` (Common/cam.c:256): the transition camera
    /// of `tile` (the most recently created one wins) changes the camera mode.
    pub fn check_for_dynamic_camera_change(&mut self, tile: Option<TileId>, pl: i32) {
        if self.cam.cam_control_mode != 0 {
            return;
        }
        let p = pl as usize;
        for i in (0..self.cam.tc_list.count.max(0) as usize).rev() {
            let cur = self.cam.tc_list.array[i];
            if cur.tile != tile {
                continue;
            }
            let c = &mut self.cam;
            match cur.flags {
                LOOK_AT_FROG | LOOK_AT_FROG_NOTILT => {
                    c.cam_dist = cur.cam_offset;
                    if c.fixed_pos != 0 {
                        sub_vector_fff(&mut c.curr_cam_offset, &c.curr_cam_source, &c.curr_cam_target);
                    }
                    c.fixed_dir = 0;
                    c.fixed_pos = 0;
                    c.fixed_up = (cur.flags == LOOK_AT_FROG_NOTILT) as i8;
                }
                STATIC_CAMERA => {
                    set_vector_fs(&mut c.cam_source, &cur.cam_offset);
                    (c.fixed_dir, c.fixed_pos, c.fixed_up) = (1, 1, 0);
                }
                FIXED_SOURCE => {
                    set_vector_fs(&mut c.cam_source, &cur.cam_offset);
                    (c.fixed_dir, c.fixed_pos, c.fixed_up) = (0, 1, 0);
                }
                LOOK_AT_TILE => {
                    set_vector_fs(&mut c.cam_source, &cur.cam_offset);
                    if let Some(t) = tile {
                        set_vector_fs(&mut c.cam_target, &self.map.tile(t).centre);
                    }
                    (c.fixed_dir, c.fixed_pos, c.fixed_up) = (1, 1, 0);
                }
                LOOK_IN_DIR => {
                    set_vector_fs(&mut c.cam_source, &cur.cam_offset);
                    c.cam_target = cur.cam_look_at;
                    let src = c.cam_source;
                    add_to_vector_ff(&mut c.cam_target, &src);
                    (c.fixed_dir, c.fixed_pos, c.fixed_up) = (1, 1, 1);
                }
                _ => {}
            }

            if cur.dir_cam_must_face != 0 {
                self.cam.cam_facing[p] = cur.dir_cam_must_face as i32 - 1;
                self.frogger.player[p].extended_hop_dir = ((self.frogmove.frog_facing[p] - self.cam.cam_facing[p]) & 3) as u8;
            }
            self.cam.trans_cam_speed_mult = if cur.speed != 0 { cur.speed } else { 4096 };
            self.cam.y_fov_new = cur.fov;
            break;
        }
    }

    /// Port of `FreeTransCameraList` (Common/cam.c:368).
    pub fn free_trans_camera_list(&mut self) {
        let c = &mut self.cam;
        c.tc_list = TransCameraList::default();
        c.cam_blocks = 0;
        c.trans_camera_defs.clear();
        c.last_tile = [None; 4];
        c.fixed_dir = 0;
        c.fixed_pos = 0;
        c.first_person = 0;
        c.start_cam_facing = 0;
        c.start_frog_facing = 0;
    }

    /// Port of `CameraLookAtFrog` (Common/cam.c:403): sets `camTarget` to the frog, or
    /// an average position.
    pub fn camera_look_at_frog(&mut self) {
        if self.cam.cam_control_mode != 0 {
            return;
        }
        if self.frogger.frog[0].is_some() && self.cam.fixed_dir == 0 && self.cam.control_camera == 0 {
            if self.game_state.multi != MULTILOCAL {
                self.cam.cam_target = self.calc_sp_camera();
            } else {
                let mut t = self.cam.cam_target;
                self.calc_mp_camera(&mut t);
                self.cam.cam_target = t;
            }
        }
    }

    /// Port of `CalcSPCamera` (Common/cam.c:434).
    pub fn calc_sp_camera(&self) -> FVector {
        let p = &self.frogger.player[0];
        let t = p.jump_time;
        let mut target = FVector::default();
        if t > 0 && t < 4096 {
            // jumping; calculate linear position
            set_vector_fs(&mut target, &p.jump_origin);
            // horizontal
            let mut v = p.jump_fwd_vector;
            scale_vector_ff(&mut v, t);
            add_to_vector_ff(&mut target, &v);
        } else if let Some(f) = self.frogger.frog[0] {
            set_vector_fs(&mut target, &self.actor2.actors[f].actor.position);
        }
        target
    }

    /// `CalcMPCamera` (Common/multi.c:1385). TODO(port): multi.c camera modes (race,
    /// battle, collect); leaves the target unchanged until multiplayer is ported.
    pub fn calc_mp_camera(&mut self, _target: &mut FVector) {
        // TODO(port): CalcMPCamera (multi wave).
    }

    /// Port of `CameraSetOffset` (Common/cam.c:460).
    pub fn camera_set_offset(&mut self) {
        let c = &self.cam;
        if self.game_state.multi != MULTILOCAL {
            let Some(ct) = self.frogmove.curr_tile[0] else { return };
            let tile = self.map.tile(ct);
            let n_c = ((c.cam_facing[0] + 1) & 3) as usize;

            let mut off = c.cam_vect;
            scale_vector_ff(&mut off, c.curr_cam_dist.vy);

            let mut v = fv(&tile.dir_vector[c.cam_facing[0] as usize & 3]);
            scale_vector_ff(&mut v, c.curr_cam_dist.vz.wrapping_neg());
            add_to_vector_ff(&mut off, &v);

            let mut v = fv(&tile.dir_vector[n_c]);
            scale_vector_ff(&mut v, c.cam_side_ofs);
            add_to_vector_ff(&mut off, &v);
            self.cam.cam_offset = off;
        } else {
            let (mut afx, mut afy, mut afz, mut l) = (0i32, 0i32, 0i32, 0i32);
            for i in 0..self.frogger.num_frogs.clamp(0, 4) as usize {
                let n_c = ((c.cam_facing[i] + 1) & 3) as usize;
                if self.frogger.player[i].frog_state & FROGSTATUS_ISDEAD == 0 {
                    let Some(ct) = self.frogmove.curr_tile[i] else { continue };
                    let t = self.map.tile(ct);
                    let d = t.dir_vector[c.cam_facing[i] as usize & 3];
                    let dz = c.curr_cam_dist.vz;
                    afx = afx.wrapping_add(0i32.wrapping_sub(fmul(d.vx as i32, dz)));
                    afy = afy.wrapping_add(c.curr_cam_dist.vy.wrapping_sub(fmul(d.vy as i32, dz)));
                    afz = afz.wrapping_add(0i32.wrapping_sub(fmul(d.vz as i32, dz)));

                    let s = t.dir_vector[n_c];
                    afx = afx.wrapping_add(fmul(s.vx as i32, c.cam_side_ofs));
                    afy = afy.wrapping_add(fmul(s.vy as i32, c.cam_side_ofs));
                    afz = afz.wrapping_add(fmul(s.vz as i32, c.cam_side_ofs));
                    l += 1;
                }
            }
            if l != 0 {
                self.cam.cam_offset = FVector { vx: afx, vy: afy, vz: afz };
                scale_vector_ff(&mut self.cam.cam_offset, 4096 / l);
            }
        }
    }

    /// `min(4096, (gameSpeed<<12) / FMul(speed, transCamSpeedMult))`. A zero divisor
    /// raises a divide-by-zero in the original; the port logs and uses 4096.
    fn slurp_factor(&mut self, speed: Fixed) -> Fixed {
        let d = fmul(speed, self.cam.trans_cam_speed_mult);
        if d == 0 {
            self.effects.push(Effect::Log { text: "SlurpCamPosition: division by zero".into() });
            return 4096;
        }
        4096.min(self.game_speed.wrapping_shl(12).wrapping_div(d))
    }

    /// Port of `SlurpCamPosition` (Common/cam.c:522): interpolates the current camera
    /// towards its goal.
    pub fn slurp_cam_position(&mut self) {
        if self.cam.cam_control_mode != 0 {
            return;
        }
        let s1 = self.slurp_factor(self.cam.cam_speed);
        let s2 = self.slurp_factor(self.cam.cam_speed2);
        let s3 = self.slurp_factor(self.cam.cam_speed3);

        let mut v = FVector::default();
        sub_vector_fff(&mut v, &self.cam.cam_target, &self.cam.curr_cam_target);
        scale_vector_ff(&mut v, s3);
        add_to_vector_ff(&mut self.cam.curr_cam_target, &v);

        if self.game_state.multi != MULTILOCAL && self.cam.fixed_dir == 0 {
            let cb = &self.map.camera_boxes;
            let bx = self.cam.curr_cam_box.and_then(|b| cb.boxes.get(b));
            if get_cam_limit_vector(&mut v, &self.cam.curr_cam_target, &cb.planes, bx, self.cam.cam_edge_spacing) != 0 {
                scale_vector_ff(&mut v, s3);
                add_to_vector_ff(&mut self.cam.curr_cam_target, &v);
            }
        }

        if self.cam.fixed_pos == 0 {
            // use offset from target..
            self.camera_set_offset();
            let c = &mut self.cam;
            sub_vector_fff(&mut v, &c.cam_offset, &c.curr_cam_offset);
            scale_vector_ff(&mut v, s3);
            add_to_vector_ff(&mut c.curr_cam_offset, &v);
            add_vector_fff(&mut c.curr_cam_source, &c.curr_cam_target, &c.curr_cam_offset);
        } else {
            // .. otherwise interpolate source independantly
            let c = &mut self.cam;
            sub_vector_fff(&mut v, &c.cam_source, &c.curr_cam_source);
            scale_vector_ff(&mut v, s3);
            add_to_vector_ff(&mut c.curr_cam_source, &v);
        }

        // camDist
        let c = &mut self.cam;
        c.curr_cam_dist.vx = c.scale_v.wrapping_mul(c.cam_dist.vx as i32);
        c.curr_cam_dist.vy = c.scale_v.wrapping_mul(c.cam_dist.vy as i32);
        c.curr_cam_dist.vz = c.scale_v.wrapping_mul(c.cam_dist.vz as i32);

        // camVect (up vector)
        if self.game_state.multi != SINGLEPLAYER {
            // In multiplayer, average out camera 'up' vectors
            let mut t = FVector::default();
            for i in 0..self.frogger.num_frogs.clamp(0, 4) as usize {
                if let Some(ct) = self.frogmove.curr_tile[i] {
                    let n = self.map.tile(ct).normal;
                    t.vx = t.vx.wrapping_add(n.vx as i32);
                    t.vy = t.vy.wrapping_add(n.vy as i32);
                    t.vz = t.vz.wrapping_add(n.vz as i32);
                }
            }
            make_unit(&mut t);
            let c = &mut self.cam;
            if c.fixed_up == 0 {
                c.cam_vect.vx = c.cam_vect.vx.wrapping_sub(fmul(s2, c.cam_vect.vx.wrapping_sub(t.vx)));
                c.cam_vect.vy = c.cam_vect.vy.wrapping_sub(fmul(s2, c.cam_vect.vy.wrapping_sub(t.vy)));
                c.cam_vect.vz = c.cam_vect.vz.wrapping_sub(fmul(s2, c.cam_vect.vz.wrapping_sub(t.vz)));
            }
        } else if self.cam.fixed_up == 0 {
            if let Some(ct) = self.frogmove.curr_tile[0] {
                let n = fv(&self.map.tile(ct).normal);
                let c = &mut self.cam;
                sub_vector_fff(&mut v, &n, &c.cam_vect);
                scale_vector_ff(&mut v, s2);
                add_to_vector_ff(&mut c.cam_vect, &v);
                make_unit(&mut c.cam_vect); // fake a vaguely spherical interpolation
            }
        }

        let c = &mut self.cam;
        c.cam_look_ofs = c.cam_look_ofs.wrapping_sub(fmul(s1, c.cam_look_ofs.wrapping_sub(c.cam_look_ofs_new)));
    }

    /// Port of `UpdateCameraPosition` (Common/cam.c:636).
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
            // This causes lots of problems with camera transitions..
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

        self.camera_look_at_frog();
        self.check_camera_boxes();
        self.slurp_cam_position();

        // Handle shaky camera nonsense
        if self.cam.cam_shakiness > 0 {
            let s = self.cam.cam_shakiness;
            let h = s >> 1;
            let mut r = || self.rng.random(s as u32).wrapping_sub(h);
            let (tx, sx, ty, sy, tz, sz) = (r(), r(), r(), r(), r(), r());
            let c = &mut self.cam;
            c.curr_cam_target.vx = c.curr_cam_target.vx.wrapping_add(tx);
            c.curr_cam_source.vx = c.curr_cam_source.vx.wrapping_add(sx);
            c.curr_cam_target.vy = c.curr_cam_target.vy.wrapping_add(ty);
            c.curr_cam_source.vy = c.curr_cam_source.vy.wrapping_add(sy);
            c.curr_cam_target.vz = c.curr_cam_target.vz.wrapping_add(tz);
            c.curr_cam_source.vz = c.curr_cam_source.vz.wrapping_add(sz);
            c.cam_shakiness = c.cam_shakiness.wrapping_sub(fmul(c.cam_shake_falloff, self.game_speed));
        }

        // edge spacing should probably related to tan(FOV/2)
        let c = &mut self.cam;
        c.cam_edge_spacing = fmul(distance_between_points_ff(&c.curr_cam_source, &c.curr_cam_target), 3072);
    }

    /// Port of `ResetCamera` (Common/cam.c:719).
    pub fn reset_camera(&mut self) {
        let initial_cam_offset = FVector { vx: 0, vy: 100 << 12, vz: 100 << 12 };
        let (initial_cam_source, initial_curr_cam_source) = match self.frogger.frog[0] {
            Some(f) => {
                let p = self.actor2.actors[f].actor.position;
                let (x, y, z) = (p.vx as i32, p.vy as i32, p.vz as i32);
                (FVector { vx: x << 12, vy: (y + 569) << 12, vz: (z - 200) << 12 }, FVector { vx: x << 12, vy: (y + 100) << 12, vz: (z + 100) << 12 })
            }
            None => {
                // BUG(original): the else branch declares shadowing locals and leaves the
                // outer vectors uninitialised (and frog[0] is dereferenced first anyway).
                self.effects.push(Effect::Log { text: "ResetCamera without frog[0]".into() });
                (FVector::default(), FVector::default())
            }
        };
        let c = &mut self.cam;
        c.cam_source = initial_cam_source;
        c.curr_cam_source = initial_curr_cam_source;
        c.cam_offset = initial_cam_offset;
        c.curr_cam_offset = initial_cam_offset;
        c.cam_dist = SvDist::INITIAL;
        c.curr_cam_target = FVector::default();
        c.camera_up_vect = UP_VEC;
        c.scale_v = 4506;
    }

    /// Port of `InitCamera` (Common/cam.c:767).
    pub fn init_camera(&mut self) {
        self.cam.curr_cam_box = None;

        if self.game_state.multi != MULTILOCAL {
            self.cam.last_tile[0] = None;
            self.check_for_dynamic_camera_change(self.frogmove.curr_tile[0], 0);
        } else {
            for i in 0..self.frogger.num_frogs.clamp(0, 4) as usize {
                self.cam.cam_facing[i] = 0;
                self.cam.last_tile[i] = None;
                self.check_for_dynamic_camera_change(self.frogmove.curr_tile[i], i as i32);
            }
        }

        self.cam.cam_shakiness = 0;
        self.cam.control_camera = 0;

        let c = &mut self.cam;
        sub_vector_fff(&mut c.curr_cam_offset, &c.cam_target, &c.cam_source);
        self.update_camera_position();

        let c = &mut self.cam;
        c.curr_cam_source = c.cam_source;
        c.curr_cam_target = c.cam_target;
        match self.frogmove.curr_tile[0] {
            Some(ct) => self.cam.cam_vect = fv(&self.map.tile(ct).normal),
            None => self.effects.push(Effect::Log { text: "InitCamera without currTile[0]".into() }),
        }
        self.slurp_cam_position();
        self.cam.curr_cam_offset = self.cam.cam_offset;
        for _ in 0..50 {
            self.update_camera_position();
        }
        // SetCamFF(currCamSource, currCamTarget): PSX only.
    }

    /// Port of `FindMaxInterFrogDistance` (Common/cam.c:810).
    pub fn find_max_inter_frog_distance(&self) -> Fixed {
        let mut best: Fixed = -100000 << 12;
        let n = self.frogger.num_frogs.clamp(0, 4) as usize;
        for i in 0..n {
            for j in i + 1..n {
                let (Some(a), Some(b)) = (self.frogger.frog[i], self.frogger.frog[j]) else { continue };
                let mut sep = SVector::default();
                sub_vector_sss(&mut sep, &self.actor2.actors[a].actor.position, &self.actor2.actors[b].actor.position);
                let dist = magnitude_s(&sep);
                if dist > best {
                    best = dist;
                }
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::{CameraBoxes, GameTile};

    fn plane(n: [i32; 3], k: i32) -> CamPlane {
        CamPlane { normal: FVector { vx: n[0], vy: n[1], vz: n[2] }, k, status: 1 }
    }

    #[test]
    fn camera_box_membership_and_limit() {
        // box: x >= 0 and x <= 1000 (as -x >= -1000)
        let planes = vec![plane([4096, 0, 0], 0), plane([-4096, 0, 0], -1000 << 12)];
        let bx = CamBox { num_planes: 2, first_plane: 0 };
        assert!(is_point_in_camera_box(&FVector { vx: 500 << 12, vy: 0, vz: 0 }, &planes, &bx));
        assert!(!is_point_in_camera_box(&FVector { vx: -1, vy: 0, vz: 0 }, &planes, &bx));
        // 10 units from the x=0 plane with a 100-unit edge: pushed 90 units inwards
        let mut out = FVector::default();
        let n = get_cam_limit_vector(&mut out, &FVector { vx: 10 << 12, vy: 0, vz: 0 }, &planes, Some(&bx), 100 << 12);
        assert_eq!(n, 1);
        assert_eq!(out, FVector { vx: 90 << 12, vy: 0, vz: 0 });
        assert_eq!(get_cam_limit_vector(&mut out, &FVector::default(), &planes, None, 100), 0);
    }

    #[test]
    fn slurp_interpolates_towards_goal() {
        let mut g = Game::new();
        g.game_state.multi = SINGLEPLAYER;
        g.cam.fixed_pos = 1;
        g.cam.fixed_up = 1;
        g.cam.fixed_dir = 1;
        g.cam.cam_target = FVector { vx: 1000 << 12, vy: 0, vz: 0 };
        g.cam.cam_source = FVector { vx: 0, vy: 2000 << 12, vz: 0 };
        g.cam.curr_cam_target = FVector::default();
        g.cam.curr_cam_source = FVector::default();
        // s3 = (4096<<12) / FMul(20<<12, 4096) = 204: 1/20 of the way per frame
        g.slurp_cam_position();
        assert_eq!(g.cam.curr_cam_target.vx, fmul(1000 << 12, 204));
        assert_eq!(g.cam.curr_cam_source.vy, fmul(2000 << 12, 204));
        for _ in 0..400 {
            g.slurp_cam_position();
        }
        assert!((g.cam.curr_cam_target.vx - (1000 << 12)).abs() < 100 << 12);
        // faster transition speed multiplier -> bigger step (capped at 4096)
        g.cam.trans_cam_speed_mult = 1;
        assert_eq!(g.slurp_factor(g.cam.cam_speed3), 4096);
        // camDist scaling
        assert_eq!(g.cam.curr_cam_dist.vy, 4506 * 6800);
    }

    #[test]
    fn trans_camera_list_and_dynamic_change() {
        let mut g = Game::new();
        g.map.tiles = vec![GameTile { centre: SVector { vx: 10, vy: 20, vz: 30, pad: 0 }, ..Default::default() }; 2];
        g.map.camera_boxes = CameraBoxes::default();
        g.alloc_cam_block(2);
        let def = |tile: u32, flags: u8, speed: Fixed| TransCameraDef {
            tile: Some(TileId(tile)),
            dir_cam_must_face: 3,
            pos: SVector { vx: 1, vy: 2, vz: 3, pad: 0 },
            flags,
            fov: 1234,
            speed,
            target: [0, 0, 64],
        };
        assert!(g.create_and_add_trans_camera(def(0, LOOK_AT_TILE, 0)));
        assert!(g.create_and_add_trans_camera(def(0, LOOK_IN_DIR, 8192)));
        assert!(!g.create_and_add_trans_camera(def(1, STATIC_CAMERA, 0)), "list full");
        assert_eq!(g.cam.trans_camera_defs.len(), 3);
        assert_eq!(g.cam.tc_list.array[1].cam_look_at, FVector { vx: 0, vy: 0, vz: 4096 * 1000 });

        // the last created camera on the tile wins
        g.check_for_dynamic_camera_change(Some(TileId(0)), 0);
        assert_eq!((g.cam.fixed_dir, g.cam.fixed_pos, g.cam.fixed_up), (1, 1, 1));
        assert_eq!(g.cam.cam_source, FVector { vx: 1 << 12, vy: 2 << 12, vz: 3 << 12 });
        assert_eq!(g.cam.cam_target, FVector { vx: 1 << 12, vy: 2 << 12, vz: (3 << 12) + 4096 * 1000 });
        assert_eq!((g.cam.cam_facing[0], g.cam.trans_cam_speed_mult, g.cam.y_fov_new), (2, 8192, 1234));
        // no camera on tile 1
        g.cam.fixed_dir = 9;
        g.check_for_dynamic_camera_change(Some(TileId(1)), 0);
        assert_eq!(g.cam.fixed_dir, 9);
    }
}
