//! Port of `Common/frogmove.c` / `frogmove.h`: frog movement (hops, superhops,
//! double jumps, platform landing, tile states, falling and respawning).
//!
//! PC notes: the port models the hardware renderer (`rHardware == 1`), so the
//! `if (!rHardware) shadow->draw = 0` branches are dropped. `FindFrogSubObjects`
//! (head/breast matrices, PC/actor.cpp) is rendering-only and not ported.

use f2_math::{
    add_to_vector_ff, add_to_vector_fs, add_to_vector_sf, add_vector_sfs, add_vector_ssf, cross_product_fff, distance_between_points_fs,
    distance_between_points_sqr_ss, distance_between_points_ss, dot_product_ff, dot_product_ss, fdiv, fmul, fsqrt_f, magnitude_f, make_unit, new_set_vector_fs,
    rotate_vector_by_quaternion_ff, scale_vector, scale_vector_ff, set_vector_sf, sub_from_vector_fs, sub_from_vector_sf, sub_vector_fff, sub_vector_fss,
    sub_vector_sss, FVector, Fixed, SVector,
};
use serde::{Deserialize, Serialize};

use crate::actor2::{orientate, orientate_fs, orientate_ss};
use crate::anim::SAMPLE_VOLUME;
use crate::define::mode::{FRONTEND_MODE, INGAME_MODE};
use crate::define::{tilestate, MULTILOCAL, SINGLEPLAYER};
use crate::enemies::{ENEMY_NEW_DONOTMOVE, ENEMY_NEW_FLAPPYTHING};
use crate::frogger::*;
use crate::general::find_joined_tile_by_direction_constrained;
use crate::ids::{PlatformId, TileId};
use crate::map::{GameTile, SCALE};
use crate::multi::MULTIMODE_BATTLE;
use crate::platform::{PLATFORM_NEW_CARRYINGFROG, PLATFORM_NEW_NOWALKUNDER};
use crate::specfx::{FXTYPE_BUBBLES, FXTYPE_DECAL, FXTYPE_FIERYSMOKE, FXTYPE_SPLASH, FXTYPE_WATERRIPPLE};
use crate::Game;

pub const NUM_PREV_TILES: usize = 5;
pub const FROG_SAFEDROPHEIGHT: i32 = 60 * SCALE;
pub const FROG_NORMALJUMPHEIGHT: i32 = 25 * SCALE;
pub const FROG_SUPERJUMPHEIGHT: i32 = 50 * SCALE;
pub const NOINIT_VELOCITY: i32 = -100000;

// ----- frog movement / action flags (`frogmove.h:30`) -----
pub const FROGSTATUS_NONE: u32 = 0;
pub const FROGSTATUS_ISDEAD: u32 = 1 << 0;
pub const FROGSTATUS_ISWANTINGU: u32 = 1 << 1;
pub const FROGSTATUS_ISWANTINGD: u32 = 1 << 2;
pub const FROGSTATUS_ISWANTINGL: u32 = 1 << 3;
pub const FROGSTATUS_ISWANTINGR: u32 = 1 << 4;
pub const FROGSTATUS_ISWANTINGSUPERHOPU: u32 = 1 << 5;
pub const FROGSTATUS_ISWANTINGSUPERHOPD: u32 = 1 << 6;
pub const FROGSTATUS_ISWANTINGSUPERHOPL: u32 = 1 << 7;
pub const FROGSTATUS_ISWANTINGSUPERHOPR: u32 = 1 << 8;
pub const FROGSTATUS_ISSUPERHOPPING: u32 = 1 << 9;
pub const FROGSTATUS_ISJUMPINGTOTILE: u32 = 1 << 10;
pub const FROGSTATUS_ISJUMPINGTOPLATFORM: u32 = 1 << 11;
pub const FROGSTATUS_ISTONGUEING: u32 = 1 << 12;
pub const FROGSTATUS_ISONMOVINGPLATFORM: u32 = 1 << 13;
pub const FROGSTATUS_ISFLOATING: u32 = 1 << 14;
pub const FROGSTATUS_ISSPRINGING: u32 = 1 << 15;
pub const FROGSTATUS_ISCROAKING: u32 = 1 << 16;
pub const FROGSTATUS_ISTELEPORTING: u32 = 1 << 17;
pub const FROGSTATUS_ISSAFE: u32 = 1 << 18;
pub const FROGSTATUS_ISSLIDING: u32 = 1 << 19;

pub const FROGSTATUS_HOPFLAGS: u32 = FROGSTATUS_ISWANTINGU | FROGSTATUS_ISWANTINGD | FROGSTATUS_ISWANTINGL | FROGSTATUS_ISWANTINGR;
pub const FROGSTATUS_SUPERHOPFLAGS: u32 =
    FROGSTATUS_ISWANTINGSUPERHOPU | FROGSTATUS_ISWANTINGSUPERHOPD | FROGSTATUS_ISWANTINGSUPERHOPL | FROGSTATUS_ISWANTINGSUPERHOPR;
pub const FROGSTATUS_ALLHOPFLAGS: u32 = FROGSTATUS_HOPFLAGS | FROGSTATUS_SUPERHOPFLAGS;

// ----- `MOVE_*` (`frogmove.h:62`) -----
pub const MOVE_UP: i32 = 0;
pub const MOVE_LEFT: i32 = 1;
pub const MOVE_DOWN: i32 = 2;
pub const MOVE_RIGHT: i32 = 3;

// ----- `DEATHBY_*` (`frogmove.h:80`) -----
pub const DEATHBY_NORMAL: i32 = 0;
pub const DEATHBY_RUNOVER: i32 = 1;
pub const DEATHBY_DROWNING: i32 = 2;
pub const DEATHBY_SQUASHED: i32 = 3;
pub const DEATHBY_FIRE: i32 = 4;
pub const DEATHBY_ELECTRIC: i32 = 5;
pub const DEATHBY_FALLING: i32 = 6;
pub const DEATHBY_WHACKING: i32 = 7;
pub const DEATHBY_INFLATION: i32 = 8;
pub const DEATHBY_POISON: i32 = 9;
pub const DEATHBY_SLICING: i32 = 10;
pub const DEATHBY_EXPLOSION: i32 = 11;
pub const DEATHBY_GIBBING: i32 = 12;
pub const DEATHBY_INCINERATE: i32 = 13;
pub const DEATHBY_BURSTING: i32 = 14;
pub const DEATHBY_SPIKED: i32 = 15;
pub const DEATHBY_LIGHTNING: i32 = 16;
pub const DEATHBY_FALLINGFOREVER: i32 = 17;
pub const NUM_DEATHTYPES: usize = 18;

// ----- hop timing (`frogmove.c:62`) -----
pub const STANDARD_HOP_FRAMES: i32 = 7;
pub const BATTLE_HOP_FRAMES: i32 = 15;
pub const QUICK_SUPER_FRAMES: i32 = 24;
pub const SUPER_HOP_FRAMES: i32 = 32;
pub const QUICK_DOUBLE_FRAMES: i32 = 32;
pub const DOUBLE_HOP_FRAMES: i32 = 44;
pub const QUICK_HOP_FRAMES: i32 = 4;
pub const SLOW_HOP_FRAMES: i32 = 14;
pub const FLOAT_FRAMES: i32 = 30;
pub const ICE_FRAMES: i32 = 15;
pub const CONVEYOR_FRAMES: [i32; 3] = [60, 30, 15];

pub const MAX_HOP_HEIGHT: Fixed = (26 * SCALE) << 12; // +1 for rounding :o)
pub const MAX_SUPERHOP_HEIGHT: Fixed = (51 * SCALE) << 12;
pub const DROP_HURT_HEIGHT: Fixed = (100 * SCALE) << 12;
pub const DROP_KILL_HEIGHT: Fixed = (150 * SCALE) << 12;

pub const HOP_HEIGHT: Fixed = (30 * SCALE) << 12;
pub const SUPERHOP_HEIGHT: Fixed = (50 * SCALE) << 12;
pub const DOUBLEHOP_HEIGHT: Fixed = (75 * SCALE) << 12;
pub const FLOAT_MULTIPLY: Fixed = 1024;
pub const FROG_ANIM_SPEED: i32 = 200;
pub const FROG_ANIM_SPEED2: i32 = 400;

/// `THROWFROG_FRAMES`.
pub const THROWFROG_FRAMES: i32 = 12;
/// `screenDist` (PC value).
pub const SCREEN_DIST: Fixed = 2200000;

/// `headNames[]` (`frogmove.c:103`), per character.
pub const HEAD_NAMES: [&str; 8] = ["FGHED", "FMHED", "TADHED", "TWHED", "TDHED", "THED", "ROOBHED", "SSPYHED"];

/// The `frogPitch` struct (`frogmove.c:77`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrogPitch {
    pub last_hop_on: i32,
    pub freq: i32,
    pub time: i32,
    pub addr: i32,
    pub subr: i32,
    pub max: i32,
    pub min: i32,
}

impl Default for FrogPitch {
    fn default() -> Self {
        FrogPitch { last_hop_on: 0, freq: 128, time: 15, addr: 2, subr: 8, max: 64, min: 50 }
    }
}

/// Globals of `frogmove.c`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrogMoveState {
    /// `currTile[]`.
    pub curr_tile: [Option<TileId>; MAX_FROGS],
    /// `destTile[]`.
    pub dest_tile: [Option<TileId>; MAX_FROGS],
    /// `prevTile`.
    pub prev_tile: Option<TileId>,
    /// `frogFacing[4]`, `oldFrogFacing[4]`, `nextFrogFacing[4]`.
    pub frog_facing: [i32; 4],
    pub old_frog_facing: [i32; 4],
    pub next_frog_facing: [i32; 4],
    /// `cameoMode`.
    pub cameo_mode: bool,
    /// `hopCount`, `hopTimer`.
    pub hop_count: i32,
    pub hop_timer: i32,
    /// `numHops_TOTAL`, `speedHops_TOTAL`, `numHealth_TOTAL`, `speedFrameCount`.
    pub num_hops_total: i32,
    pub speed_hops_total: i32,
    pub num_health_total: i32,
    pub speed_frame_count: i32,
    /// `frogPitch`.
    pub frog_pitch: FrogPitch,
}

/// `DotProductFF` / `SetVectorFF` on an `SVECTOR` operand: the macros read the shorts
/// unshifted.
#[inline]
fn fv(s: &SVector) -> FVector {
    let mut v = FVector::default();
    new_set_vector_fs(&mut v, s);
    v
}

/// Port of `GetTilesMatchingDirection` (Common/frogmove.c:863).
///
/// `dirNumber` is uninitialised in the original when no dot product is `>= 0`; the port
/// returns 0 for that (unreachable for connected tiles) case.
pub fn get_tiles_matching_direction(me: &GameTile, direction: i32, next: &GameTile) -> i32 {
    let mut dir_number = 0;
    let mut distance: Fixed = 0;
    let d = fv(&me.dir_vector[(direction & 3) as usize]);
    for i in 0..4 {
        let t = dot_product_ff(&d, &fv(&next.dir_vector[i]));
        if t >= distance {
            distance = t;
            dir_number = i as i32;
        }
    }
    dir_number
}

/// Port of `GetTilesMatchingVector` (Common/frogmove.c:881).
pub fn get_tiles_matching_vector(dir: &SVector, next: &GameTile) -> i32 {
    let mut dir_number = 0;
    let mut distance: Fixed = 0;
    let d = fv(dir);
    for i in 0..4 {
        let t = dot_product_ff(&d, &fv(&next.dir_vector[i]));
        if t >= distance {
            distance = t;
            dir_number = i as i32;
        }
    }
    dir_number
}

/// `actorAnimate(frog[pl]->actor, ...)` shorthand.
macro_rules! frog_anim {
    ($g:expr, $p:expr, $anim:expr, $looped:expr, $queue:expr, $speed:expr) => {
        if let Some(f) = $g.frogger.frog[$p] {
            $g.actor_animate(f, $anim, $looped, $queue, $speed, false);
        }
    };
}

impl Game {
    fn frog_pos(&self, p: usize) -> SVector {
        self.frog_position(p)
    }

    fn set_frog_pos(&mut self, p: usize, v: SVector) {
        if let Some(f) = self.frogger.frog[p] {
            let a = &mut self.actors_mut(f).actor.position;
            a.vx = v.vx;
            a.vy = v.vy;
            a.vz = v.vz;
        }
    }

    fn actors_mut(&mut self, f: crate::ids::ActorId) -> &mut crate::actor2::Actor2 {
        &mut self.actor2.actors[f]
    }

    fn set_frog_shadow_draw(&mut self, p: usize, d: i8) {
        if let Some(f) = self.frogger.frog[p] {
            if let Some(s) = self.actor2.actors[f].actor.shadow.as_mut() {
                s.draw = d;
            }
        }
    }

    fn orientate_frog_ss(&mut self, p: usize, fd: &SVector, up: &SVector) {
        if let Some(f) = self.frogger.frog[p] {
            orientate_ss(&mut self.actor2.actors[f].actor.q_rot, fd, up);
        }
    }

    /// `currTile[p]` (the original dereferences it unconditionally).
    fn ctile(&self, p: usize) -> GameTile {
        self.tile_or_default(self.frogmove.curr_tile[p])
    }

    /// `plat->pltActor->actor->position`.
    pub(crate) fn plat_pos(&self, id: PlatformId) -> SVector {
        let pl = &self.platform.platform_list.array[id.idx()];
        pl.plt_actor.and_then(|a| self.actor2.actors.get(a)).map_or(SVector::default(), |a| a.actor.position)
    }

    /// `OrientateSS(&frog[p]->actor->qRot, &currTile[p]->dirVector[frogFacing[p]], &currTile[p]->normal)`.
    fn orientate_frog_to_curr_tile(&mut self, p: usize) {
        let t = self.ctile(p);
        let ff = (self.frogmove.frog_facing[p] & 3) as usize;
        self.orientate_frog_ss(p, &t.dir_vector[ff], &t.normal);
    }

    /// Port of `SetFroggerStartPos` (Common/frogmove.c:147).
    pub fn set_frogger_start_pos(&mut self, start_tile: TileId, p: i32) {
        let pu = p as usize;
        let mut c = self.enemies.enemy_list.head.first;
        while let Some(id) = c {
            let next = self.enemies.enemy_list.array[id.idx()].links.next;
            let e = &mut self.enemies.enemy_list.array[id.idx()];
            e.flags &= !ENEMY_NEW_DONOTMOVE;
            if e.flags & ENEMY_NEW_FLAPPYTHING != 0 {
                e.active = 1;
                e.visible = 1;
                let path = e.path;
                let actor = e.nme_actor;
                let centre = self.tile_or_default(self.path_node(&path, 0).world_tile).centre;
                if let Some(a) = actor {
                    let a = &mut self.actor2.actors[a];
                    a.draw = 1;
                    a.actor.position.vx = centre.vx;
                    a.actor.position.vy = centre.vy;
                    a.actor.position.vz = centre.vz;
                    a.actor.size = ONE_VEC;
                }
            }
            c = next;
        }

        let Some(f) = self.frogger.frog[pu] else { return };
        // Change frog's position to reflect that of the start tile
        let centre = self.map.tile(start_tile).centre;
        self.set_frog_pos(pu, centre);

        self.actor_animate(f, FROG_ANIM_BREATHE, true, false, 64, false);
        // PC: if (rHardware) shadow->draw = 1
        self.set_frog_shadow_draw(pu, 1);

        if let Some(cp) = self.platform.curr_platform[pu] {
            let plat = &mut self.platform.platform_list.array[cp.idx()];
            plat.carrying = None;
            plat.flags &= !PLATFORM_NEW_CARRYINGFROG;
        }

        self.frogmove.curr_tile[pu] = Some(start_tile);
        self.frogmove.dest_tile[pu] = None;
        self.platform.curr_platform[pu] = None;
        self.platform.dest_platform[pu] = None;

        let pl = &mut self.frogger.player[pu];
        pl.health_points = 1;
        pl.is_croaking.init(0);
        pl.stun.init(0);
        pl.safe.init(0);
        pl.dead.init(0);
        pl.autohop.init(0);
        pl.quickhop.init(0);
        pl.slowhop.init(0);

        // set frog action movement variables
        self.actor2.actors[f].actor.vel = FVector::default();

        let pl = &mut self.frogger.player[pu];
        pl.death_by = 0;
        pl.can_jump = 1;
        pl.frog_state = 0;
        pl.is_super_hopping = 0;
        pl.is_sinking = 0;
        pl.idle_time = MAX_IDLE_TIME;
        pl.idle_enable = 1;
        pl.height_jumped = 0;
        pl.jump_time = -1;

        self.cam.fixed_pos = 0;
        self.cam.fixed_dir = 0;

        // JIM: I hope this doesn'y break owt...
        self.frogmove.cameo_mode = false;

        let scale = self.frogger.global_frog_scale;
        let a = &mut self.actor2.actors[f];
        a.draw = 1;
        a.clipped = 0;
        a.miles_away = 0;
        a.actor.size.vx = scale;
        a.actor.size.vy = scale;
        a.actor.size.vz = scale;
        if self.frogger.player[pu].character == FROG_BABYFROG {
            scale_vector_ff(&mut self.actor2.actors[f].actor.size, 3072);
        }

        self.cam.cam_facing[pu] = self.cam.start_cam_facing as i32;
        self.frogmove.frog_facing[pu] = self.cam.start_frog_facing as i32;
        self.frogger.player[pu].extended_hop_dir = ((self.cam.start_frog_facing as i32 - self.cam.start_cam_facing as i32) & 3) as u8;
        self.orientate_frog_to_curr_tile(pu);

        // FindFrogSubObjects(p) (PC/actor.cpp:129): the head matrix CalcTongueSource uses
        // (breast post-matrix: rendering only).
        self.find_frog_sub_objects(pu);

        if !(self.game_state.multi != SINGLEPLAYER && self.multi.multiplayer_mode == MULTIMODE_BATTLE) {
            self.reactivate_powerups();
        }
    }

    /// Port of `SetFroggerOnTile` (Common/frogmove.c:272).
    pub fn set_frogger_on_tile(&mut self, tile: TileId, pl: i32) {
        let p = pl as usize;
        if let Some(cp) = self.platform.curr_platform[p] {
            let plat = &mut self.platform.platform_list.array[cp.idx()];
            plat.carrying = None;
            plat.flags &= !PLATFORM_NEW_CARRYINGFROG;
            self.platform.curr_platform[p] = None;
        }
        self.frogmove.curr_tile[p] = Some(tile);
        self.check_tile_state(Some(tile), pl);
    }

    /// Port of `UpdateFroggerControls` (Common/frogmove.c:293).
    pub fn update_frogger_controls(&mut self, pl: i32) -> bool {
        let p = pl as usize;
        let fs = self.frogger.player[p].frog_state;

        // ----------------------- Frog wants to HOP u/d/l/r -----------------------------
        if fs & FROGSTATUS_HOPFLAGS != 0 {
            let dir = if fs & FROGSTATUS_ISWANTINGU != 0 {
                MOVE_UP
            } else if fs & FROGSTATUS_ISWANTINGD != 0 {
                MOVE_DOWN
            } else if fs & FROGSTATUS_ISWANTINGL != 0 {
                MOVE_LEFT
            } else {
                MOVE_RIGHT
            };

            self.frogger.player[p].frog_state &= !FROGSTATUS_ALLHOPFLAGS;

            let fp = &mut self.frogmove.frog_pitch;
            if self.act_frame_count.wrapping_sub(fp.last_hop_on as u32) < fp.time as u32 {
                if fp.freq < fp.max {
                    fp.freq += fp.addr;
                }
            } else {
                fp.freq = fp.min;
            }
            let act_f = self.frogmove.frog_pitch.freq + self.rng.random(10);
            self.frogmove.frog_pitch.last_hop_on = self.act_frame_count as i32;

            self.frogmove.prev_tile = self.frogmove.curr_tile[p];

            let cf = self.cam.cam_facing[p];
            self.animate_frog_hop((dir - cf) & 3, pl);
            let old_facing = self.frogmove.frog_facing[p];
            self.frogmove.frog_facing[p] = (cf + dir) & 3;
            let jump = self.move_to_requested_destination(dir, pl);

            if jump {
                let vol = 100 - self.rng.random(15);
                self.play_sample(f2_formats::crc::crc("hopongrass"), None, 0, vol, act_f);
                self.frogmove.hop_count += 1;
                self.frogmove.hop_timer = 30;
            } else {
                match (((dir - cf) & 3) - old_facing) & 3 {
                    1 => frog_anim!(self, p, FROG_ANIM_HOPLEFT, false, false, FROG_ANIM_SPEED2),
                    3 => frog_anim!(self, p, FROG_ANIM_HOPRIGHT, false, false, FROG_ANIM_SPEED2),
                    _ => {
                        if self.frogmove.hop_count >= 3 {
                            frog_anim!(self, p, FROG_ANIM_WALLSPLAT, false, false, 154);
                            self.play_voice(pl, "frogannoyed");
                        } else {
                            frog_anim!(self, p, FROG_ANIM_LOOKUP, false, false, 154);
                        }
                    }
                }
                frog_anim!(self, p, FROG_ANIM_BREATHE, true, true, FROG_BREATHE_SPEED);
                self.frogmove.hop_count = 0;
                self.frogmove.hop_timer = 0;
            }
        }
        // ----------------------- Frog wants to SUPERHOP u/d/l/r -----------------------------
        else if fs & FROGSTATUS_SUPERHOPFLAGS != 0 {
            let dir = if fs & FROGSTATUS_ISWANTINGSUPERHOPU != 0 {
                MOVE_UP
            } else if fs & FROGSTATUS_ISWANTINGSUPERHOPD != 0 {
                MOVE_DOWN
            } else if fs & FROGSTATUS_ISWANTINGSUPERHOPL != 0 {
                MOVE_LEFT
            } else {
                MOVE_RIGHT
            };

            self.frogmove.hop_count = 0;
            self.frogmove.hop_timer = 0;
            self.frogger.player[p].frog_state &= !FROGSTATUS_ALLHOPFLAGS;

            let cstate = self.ctile(p).state;
            if (self.platform.curr_platform[p].is_none() && cstate == tilestate::NOSUPER) || cstate == tilestate::NOSUPERHOT {
                self.frogger.player[p].is_super_hopping = 0;
                self.frogger.player[p].can_jump = 1;
                frog_anim!(self, p, FROG_ANIM_LOOKUP, false, false, 154);
                frog_anim!(self, p, FROG_ANIM_BREATHE, true, true, FROG_BREATHE_SPEED);
                return true;
            }

            self.frogmove.prev_tile = self.frogmove.curr_tile[p];
            let cf = self.cam.cam_facing[p];
            self.frogmove.frog_facing[p] = (cf + dir) & 3;

            self.frogger.player[p].frog_state |= FROGSTATUS_ISSUPERHOPPING;
            let jump = self.move_to_requested_destination(dir, pl);

            if !jump {
                self.frogger.player[p].frog_state &= !(FROGSTATUS_ALLHOPFLAGS | FROGSTATUS_ISJUMPINGTOTILE | FROGSTATUS_ISJUMPINGTOPLATFORM);
                if self.game_state.mode != FRONTEND_MODE {
                    let pp = &mut self.frogger.player[p];
                    pp.has_jumped = 1;
                    pp.is_super_hopping = 1;
                    pp.can_jump = 0;
                    self.frogmove.next_frog_facing[p] = self.frogmove.frog_facing[p]; // ds

                    let t = if self.frogger.player[p].quickhop.time != 0 { QUICK_SUPER_FRAMES } else { SUPER_HOP_FRAMES };

                    let ct = self.ctile(p);
                    // ds - hAck! Jump to the platform
                    if let Some(cp) = self.platform.curr_platform[p] {
                        self.platform.dest_platform[p] = Some(cp);
                        self.frogger.player[p].frog_state |= FROGSTATUS_ISJUMPINGTOPLATFORM;
                        let (from, to) = (self.frog_pos(p), self.plat_pos(cp));
                        self.calculate_frog_jump_s(&from, &to, &ct.normal, SUPERHOP_HEIGHT, t, pl);
                    } else {
                        self.frogmove.dest_tile[p] = self.frogmove.curr_tile[p];
                        let from = self.frog_pos(p);
                        self.calculate_frog_jump_s(&from, &ct.centre, &ct.normal, SUPERHOP_HEIGHT, t, pl);
                    }
                } else {
                    self.frogmove.frog_facing[p] = self.frogmove.old_frog_facing[p];
                    self.orientate_frog_to_curr_tile(p);
                }
            }

            if jump || self.game_state.mode != FRONTEND_MODE {
                self.animate_frog_hop((dir - cf) & 3, pl);
                self.play_sample(f2_formats::crc::crc("hop2"), None, 0, 255, -1);
            }
        } else {
            if self.frogmove.hop_timer != 0 {
                self.frogmove.hop_timer -= (self.game_speed >> 12).max(1);
            }
            if self.frogmove.hop_timer <= 0 {
                self.frogmove.hop_timer = 0;
                self.frogmove.hop_count = 0;
            }
            return false; // nope, we didn't do nuffink
        }

        true // yep, we did somefink
    }

    /// `PlayVoice(pl, name)`.
    pub fn play_voice(&mut self, pl: i32, name: &str) {
        self.effects.push(crate::Effect::PlayVoice { player: pl as u8, uid: f2_formats::crc::crc(name) });
    }

    /// Port of `FroggerHop` (Common/frogmove.c:487). See the notes on frog movement
    /// there: `y = h(1-(2mt-1)^2)`, horizontal motion linear in `t`.
    ///
    /// Note that we also check for falling through platforms here!
    pub fn frogger_hop(&mut self, pl: i32) {
        // static landingRadius = 220 (just less than the radius of one tile),
        // landingHeight = 200 (how 'thick' platforms are)
        const LANDING_RADIUS: Fixed = 220;
        const LANDING_HEIGHT: Fixed = 200;
        let p = pl as usize;

        if self.frogger.player[p].jump_time < 0 {
            return;
        }

        let mut delta = fmul(self.frogger.player[p].jump_speed, self.game_speed);
        if self.frogger.player[p].frog_state & FROGSTATUS_ISFLOATING != 0 {
            delta = fmul(delta, FLOAT_MULTIPLY);
        }
        let pp = &mut self.frogger.player[p];
        pp.jump_time = pp.jump_time.wrapping_add(delta);
        let t = pp.jump_time;

        // horizontal (parallel to plane)
        let mut fwd = pp.jump_fwd_vector;
        fwd.vx = fmul(fwd.vx, t);
        fwd.vy = fmul(fwd.vy, t);
        fwd.vz = fmul(fwd.vz, t);
        let mut pos = SVector::default();
        add_vector_sfs(&mut pos, &fwd, &pp.jump_origin);

        // vertical
        if pp.jump_multiplier != 0 {
            if self.game_state.multi == SINGLEPLAYER {
                // Stop looking at the frog if it's going to die from falling
                let pp = &self.frogger.player[p];
                let dest_deadly = self.frogmove.dest_tile[p].is_some_and(|d| self.map.tile(d).state == tilestate::DEADLY);
                if (pp.height_jumped < -DROP_KILL_HEIGHT && pp.jump_time > 2048)
                    || (dest_deadly && pp.jump_time > 2600 && self.platform.dest_platform[p].is_none())
                {
                    self.cam.control_camera = 1;
                } else if pp.death_by as i32 != DEATHBY_WHACKING {
                    self.cam.control_camera = 0;
                }
            }

            let pp = &self.frogger.player[p];
            let mut pv = fmul(t, pp.jump_multiplier).wrapping_mul(2).wrapping_sub(4096);
            pv = 4096i32.wrapping_sub(fmul(pv, pv));
            let mut up = pp.jump_up_vector;
            up.vx = fmul(up.vx, pv);
            up.vy = fmul(up.vy, pv);
            up.vz = fmul(up.vz, pv);
            add_to_vector_sf(&mut pos, &up);

            let nearest_plat_dist: i32 = 1000;

            // And now... fun with platforms (again)
            // While we're jumping, we need to check if we've fallen through a platform
            // We also need to check if we're trying to jump under a 'don't jump under' platform
            // (since checking beforehand is unreliable)
            if self.platform.dest_platform[p].is_none() && self.frogger.player[p].frog_state & FROGSTATUS_ISDEAD == 0 {
                let mut c = self.platform.platform_list.head.first;
                while let Some(cur) = c {
                    let next = self.platform.platform_list.array[cur.idx()].links.next;
                    c = next;
                    let plat = self.platform.platform_list.array[cur.idx()].clone();
                    if plat.active == 0 || self.tile_or_default(plat.in_tile[0]).state == tilestate::BARRED {
                        continue;
                    }

                    let ppos = self.plat_pos(cur);
                    let mut sv = SVector::default();
                    sub_vector_sss(&mut sv, &ppos, &pos);
                    // Make sv positive..
                    if sv.vx < 0 {
                        sv.vx = sv.vx.wrapping_neg();
                    }
                    if sv.vy < 0 {
                        sv.vy = sv.vy.wrapping_neg();
                    }
                    if sv.vz < 0 {
                        sv.vz = sv.vz.wrapping_neg();
                    }
                    // Get maximum..
                    let mut check = sv.vx as i32;
                    if sv.vy as i32 > check {
                        check = sv.vy as i32;
                    }
                    if sv.vz as i32 > check {
                        check = sv.vz as i32;
                    }
                    // bounding-box discard
                    if check > nearest_plat_dist {
                        continue;
                    }

                    // 'kay, now check to see if we've fallen through it...
                    let plt = ppos;
                    let ctn = fv(&self.ctile(p).normal);
                    let mut v = FVector::default();
                    sub_vector_fss(&mut v, &pos, &plt);
                    let after = dot_product_ff(&v, &ctn);
                    let fpos = self.frog_pos(p);
                    sub_vector_fss(&mut v, &fpos, &plt);
                    let before = dot_product_ff(&v, &ctn);

                    let dest = self.frogmove.dest_tile[p];
                    if before > -LANDING_HEIGHT && 0 >= after {
                        // we've passed through the platform's plane (with a bit of a hacked threshold)

                        // if the platform's tile and our destination are the same
                        // then it's a pretty safe bet we've landed
                        if plat.in_tile[0] == dest {
                            self.land_on_platform_mid_hop(p, cur, &up);
                            break;
                        }

                        // Project our 'after' position onto the tile's normal and subtract that from
                        // the vector from the frog to the platform: the magnitude is the radius of an
                        // infinite cylinder centred on the platform, along its normal.
                        let mut v = ctn;
                        scale_vector_ff(&mut v, after.wrapping_neg());
                        sub_from_vector_fs(&mut v, &plt);
                        add_to_vector_fs(&mut v, &pos);

                        let radius = magnitude_f(&v) >> 12;
                        if radius < LANDING_RADIUS {
                            self.land_on_platform_mid_hop(p, cur, &up);
                            self.cam.control_camera = 0;
                            break;
                        }
                    } else if plat.flags & PLATFORM_NEW_NOWALKUNDER != 0
                        && self.frogger.player[p].jump_time > 2048
                        && after < 0
                        // BUG(original): indexes inTile[] by player number.
                        && plat.in_tile.get(p).copied().flatten() == dest
                    {
                        // Fix for accidentally jumping underneath moving platforms - ds
                        let jf = self.frogger.player[p].jump_fwd_vector;
                        let dir = FVector { vx: jf.vx.wrapping_neg(), vy: jf.vy.wrapping_neg(), vz: jf.vz.wrapping_neg() };
                        self.frogmove.curr_tile[p] = dest;
                        self.push_frog(&dir, pl);
                    }
                }
            }
        }

        if self.frogger.player[p].death_by as i32 == DEATHBY_FALLINGFOREVER || t < (1 << 12) {
            // MY GOD this is a stupid hack
            let cur = self.frog_pos(p);
            if !(cur.vy < -30000 && pos.vy > 30000) {
                // Stop wrapping
                self.set_frog_pos(p, pos);
            } else if let Some(f) = self.frogger.frog[p] {
                self.actor2.actors[f].draw = 0;
            }
        }

        if self.frogger.player[p].frog_state & FROGSTATUS_ISDEAD == 0 {
            self.set_frog_shadow_draw(p, 1);
        }
        // currTile >= CONVEYOR || ICE: shadow off only without rHardware (PC).
    }

    fn land_on_platform_mid_hop(&mut self, p: usize, cur: PlatformId, up: &FVector) {
        self.platform.dest_platform[p] = Some(cur);
        let pp = &mut self.frogger.player[p];
        pp.frog_state &= !FROGSTATUS_ALLHOPFLAGS;
        pp.frog_state |= FROGSTATUS_ISJUMPINGTOPLATFORM;
        pp.jump_time = 4096; // aaand land.
        let n = fv(&self.tile_or_default(self.platform.platform_list.array[cur.idx()].in_tile[0]).normal);
        self.frogger.player[p].height_jumped = dot_product_ff(up, &n);
    }

    /// `DEC_POWERUP(x, pl)` (Common/frogmove.c:701).
    fn dec_powerup(&mut self, p: usize, which: u8) {
        let afc = self.act_frame_count;
        let pp = &mut self.frogger.player[p];
        let t = match which {
            0 => &mut pp.autohop,
            1 => &mut pp.quickhop,
            _ => &mut pp.slowhop,
        };
        let old_time = t.time;
        t.update(-1, afc);
        let (now, changed) = (t.time, t.time != old_time);
        if now == 0 {
            self.play_voice(p as i32, "frogannoyed");
        }
        if self.game_state.multi == SINGLEPLAYER && changed {
            self.play_sample(f2_formats::crc::crc("puptick"), None, 0, SAMPLE_VOLUME, -1);
        }
    }

    /// Port of `UpdateFroggerPos` (Common/frogmove.c:717).
    pub fn update_frogger_pos(&mut self, pl: i32) {
        let p = pl as usize;
        if self.frogger.frog[p].is_none() || self.frogmove.curr_tile[p].is_none() {
            return;
        }

        if self.frogmove.cameo_mode {
            self.frogger.player[p].frog_state &= !FROGSTATUS_ALLHOPFLAGS;
        }

        if self.frogger.player[p].autohop.time != 0 {
            self.dec_powerup(p, 0);
        }
        if self.frogger.player[p].quickhop.time != 0 {
            self.dec_powerup(p, 1);
        }
        if self.frogger.player[p].slowhop.time != 0 {
            self.dec_powerup(p, 2);
        }

        // But first... platforms
        if let Some(plat) = self.platform.curr_platform[p] {
            let pl_data = self.platform.platform_list.array[plat.idx()].clone();
            let tile_id = pl_data.in_tile[0];
            let tile = self.tile_or_default(tile_id);

            self.cam.control_camera = 0;

            if tile_id != self.frogmove.curr_tile[p] {
                // mm, try this..?
                let ct = self.ctile(p);
                self.frogmove.frog_facing[p] = get_tiles_matching_direction(&ct, self.frogmove.frog_facing[p], &tile);
                self.frogmove.curr_tile[p] = tile_id;
                let ff = (self.frogmove.frog_facing[p] & 3) as usize;
                self.orientate_frog_ss(p, &tile.dir_vector[ff], &tile.normal);
                self.check_tile_for_collectable(tile_id, pl);
            }

            if tile.state == tilestate::BARRED {
                // if we're moving onto a barred tile, push the frog in the other direction
                let from = self.node_tile(&pl_data.path, pl_data.path.from_node as i32).centre;
                let to = self.node_tile(&pl_data.path, pl_data.path.to_node as i32).centre;
                let mut v = FVector::default();
                sub_vector_fss(&mut v, &from, &to);
                make_unit(&mut v);
                self.push_frog(&v, pl);
            } else if tile.state == tilestate::DEADLY {
                // test for platforms going down through deadly tiles
                let mut v = SVector::default();
                sub_vector_sss(&mut v, &self.plat_pos(plat), &tile.centre);
                if dot_product_ss(&v, &tile.normal) < 0 {
                    if let Some(t) = tile_id {
                        self.set_frogger_on_tile(t, pl);
                    }
                }
            } // hur hur, I typed 'going down'
        } else {
            let ct = self.ctile(p);
            if ct.state == tilestate::SINK {
                let fpos = self.frog_pos(p);
                let mut dir = FVector::default();
                sub_vector_fss(&mut dir, &fpos, &ct.centre);
                make_unit(&mut dir);
                let j = dot_product_ff(&fv(&ct.normal), &dir);
                let mut dist = distance_between_points_sqr_ss(&fpos, &ct.centre);
                if j < 0 {
                    dist = dist.wrapping_mul(-1);
                }

                if self.frogger.player[p].frog_state & FROGSTATUS_ALLHOPFLAGS != 0 {
                    let mut move_vec = fv(&ct.normal);
                    scale_vector_ff(&mut move_vec, self.game_speed.wrapping_mul(2));
                    if let Some(f) = self.frogger.frog[p] {
                        add_to_vector_sf(&mut self.actor2.actors[f].actor.position, &move_vec);
                    }
                    // Frog has broken free!
                    if dist >= 0 {
                        self.frogger.player[p].is_sinking = 0;
                    }
                } else if self.frogger.player[p].is_sinking != 0 {
                    let mut move_vec = fv(&ct.normal);
                    scale_vector_ff(&mut move_vec, self.game_speed / -5);
                    if let Some(f) = self.frogger.frog[p] {
                        add_to_vector_sf(&mut self.actor2.actors[f].actor.position, &move_vec);
                    }
                    // Frog is dead
                    if dist < -900 && self.frogger.player[p].dead.time == 0 {
                        if self.frogger.frog_pool(self.frogger.player[p].character as usize % FROG_NUMFROGS).anim != 0 {
                            self.call_death_anim(DEATHBY_DROWNING, pl);
                        } else {
                            self.death_multi(pl);
                        }
                    }
                }
            } else if ct.state == tilestate::SAFE {
                self.frogger.player[p].frog_state |= FROGSTATUS_ISSAFE;
            } else if ct.state & tilestate::CONVEYOR != 0 {
                // If we're on a conveyor and can jump - i.e. we're not already jumping -
                // consider the player to be on its destination tile once it goes past half-way
                // along its 'jump'.
                if self.frogmove.dest_tile[p].is_some() && self.frogger.player[p].can_jump != 0 && self.frogger.player[p].jump_time > 2048 {
                    self.frogmove.curr_tile[p] = self.frogmove.dest_tile[p];
                }
            }
        }

        // Calculate frog hop
        self.frogger_hop(pl);

        if self.frogger.player[p].frog_state & FROGSTATUS_ISDEAD != 0 {
            self.check_for_frogger_landing(pl);
            if self.game_state.multi == SINGLEPLAYER {
                self.kill_frog(pl);
            }
            return;
        }

        if self.frogmove.cameo_mode || !self.update_frogger_controls(pl) {
            self.check_for_frogger_landing(pl);
        }
    }

    /// Port of `GetNextTile` (Common/frogmove.c:905).
    pub fn get_next_tile(&mut self, pdir: &mut i32, pl: i32) -> Option<TileId> {
        let p = pl as usize;
        let ct_id = self.frogmove.curr_tile[p]?;
        let ct = self.map.tile(ct_id).clone();
        let mut direction = *pdir;
        let mut dest;
        // wave G: on the frontend's options tile every hop goes to tilePtrs[1].
        if self.game_state.mode == crate::define::mode::FRONTEND_MODE && self.game.curr_tile_num == crate::menus::TILENUM_OPTIONS {
            dest = ct.tile_ptrs[1];
            // BUG(original): `i` is uninitialised here; the port leaves `*pdir` unchanged.
        } else {
            let i = (direction + self.cam.cam_facing[p] + 2) & 3;
            dest = ct.tile_ptrs[i as usize];
            *pdir = i;
        }

        let d = dest?;
        if self.map.tile(d).state == tilestate::BARRED {
            return None;
        }

        if self.map.tile(d).state == tilestate::JOIN {
            let joining = self.map.tile(d).clone();
            // Find the direction closest to "up" or "down" in the opposite direction to where we are now
            let vec_up = fv(&ct.normal);
            let mut distance: Fixed = -1000 << 12;
            for (k, tp) in joining.tile_ptrs.iter().enumerate() {
                if let Some(tp) = tp {
                    let t2 = dot_product_ff(&vec_up, &fv(&joining.dir_vector[k]));
                    let t = f2_math::fabs(t2);
                    if t > distance && Some(ct_id) != Some(*tp) {
                        distance = t;
                        direction = k as i32;
                    }
                }
            }
            dest = joining.tile_ptrs[(direction & 3) as usize];
            let d = dest?;
            if self.map.tile(d).state == tilestate::BARRED {
                return None;
            }
        }
        dest
    }

    /// Port of `AnimateFrogHop` (Common/frogmove.c:971).
    pub fn animate_frog_hop(&mut self, direction: i32, pl: i32) {
        let p = pl as usize;
        if self.frogger.player[p].is_super_hopping != 0 {
            // play animation for superhopping
            let anim_speed = self.frogger.player[p].jump_speed.wrapping_mul(15) >> 4; // play 15 frames of anim before landing
            frog_anim!(self, p, FROG_ANIM_SUPERHOP, false, false, anim_speed);
            return;
        }
        match (direction - self.frogmove.frog_facing[p]) & 3 {
            1 => frog_anim!(self, p, FROG_ANIM_HOPLEFT, false, false, FROG_ANIM_SPEED2),
            3 => frog_anim!(self, p, FROG_ANIM_HOPRIGHT, false, false, FROG_ANIM_SPEED2),
            _ => frog_anim!(self, p, FROG_ANIM_HOP, false, false, FROG_ANIM_SPEED),
        }
    }

    /// Port of `MoveToRequestedDestination` (Common/frogmove.c:1009): TRUE if we did a hop.
    pub fn move_to_requested_destination(&mut self, dir: i32, pl: i32) -> bool {
        let p = pl as usize;
        // clear all movement flags
        self.frogger.player[p].frog_state &= !(FROGSTATUS_ALLHOPFLAGS | FROGSTATUS_ISJUMPINGTOTILE | FROGSTATUS_ISJUMPINGTOPLATFORM);
        self.orientate_frog_to_curr_tile(p);

        let mut tiledir = dir;
        let dest = self.get_next_tile(&mut tiledir, pl);

        let Some(dest) = dest.filter(|_| self.frogger.player[p].is_sinking == 0) else {
            self.frogger.player[p].can_jump = 1;
            self.frogger.player[p].is_super_hopping = 0;
            return false;
        };

        // check destination tile
        if self.game_tile_too_high(dest, pl) && self.game_state.mode != FRONTEND_MODE {
            // gametile is too high
            self.frogger.player[p].can_jump = 1;
            self.frogger.player[p].is_super_hopping = 0;
            return false;
        } else if self.map.tile(dest).state == tilestate::SMASH {
            // smash tiles
            if self.frogger.player[p].frog_state & FROGSTATUS_ISSUPERHOPPING != 0 {
                self.map.tile_mut(dest).state = tilestate::NORMAL;
            }
            self.frogger.player[p].can_jump = 1;
            self.frogger.player[p].is_super_hopping = 0;
            return false;
        }

        // If we get this far, it's a valid jump to a tile
        self.frogger.player[p].frog_state &= !(FROGSTATUS_ALLHOPFLAGS | FROGSTATUS_ISJUMPINGTOTILE | FROGSTATUS_ISJUMPINGTOPLATFORM);

        let from = self.frogmove.curr_tile[p];
        self.platform.dest_platform[p] = None;
        self.frogger.player[p].has_jumped = 1;

        // Platform tests
        let dest_tile = self.map.tile(dest).clone();
        let mut n = self.platform.platform_list.count;
        let mut c = self.platform.platform_list.head.first;
        while n != 0 {
            let Some(id) = c else { break };
            c = self.platform.platform_list.array[id.idx()].links.next;
            n -= 1;
            let plat = &self.platform.platform_list.array[id.idx()];
            if plat.active == 0 {
                continue;
            }
            let (flags, in0, path) = (plat.flags, plat.in_tile[0], plat.path);
            let ppos = self.plat_pos(id);
            let fpos = self.frog_pos(p);
            let super_hop = self.frogger.player[p].is_super_hopping != 0;

            if in0 == Some(dest) {
                let mut w = FVector::default();
                sub_vector_fss(&mut w, &ppos, &fpos);
                let height = dot_product_ff(&w, &fv(&dest_tile.normal));

                if flags & PLATFORM_NEW_NOWALKUNDER != 0 && height > (if super_hop { MAX_SUPERHOP_HEIGHT } else { MAX_HOP_HEIGHT }) {
                    self.frogger.player[p].can_jump = 1;
                    self.frogger.player[p].is_super_hopping = 0;
                    return false;
                }

                if !super_hop && height <= MAX_HOP_HEIGHT && self.path_node(&path, path.to_node as i32).offset >= 0 {
                    self.platform.dest_platform[p] = Some(id);
                    break;
                }
            } else if !super_hop && self.platform.curr_platform[p].is_some() {
                // platform->platform hopping: find roughly where we're trying to hop to by
                // scaling the direction vector by the size of a normal tile
                let mut v = fv(&self.ctile(p).dir_vector[(tiledir & 3) as usize]);
                scale_vector(&mut v, -500); // ~ one tile
                add_to_vector_fs(&mut v, &fpos);

                // 400 is some completely arbitrary value that seems to work
                let dist = distance_between_points_fs(&v, &ppos);
                if dist < 400 * 4096 && self.path_node(&path, path.to_node as i32).offset >= 0 {
                    let mut w = FVector::default();
                    sub_vector_fss(&mut w, &ppos, &fpos);
                    let height = dot_product_ff(&w, &fv(&dest_tile.normal));
                    // Check we're not trying to jump too high!
                    if height <= MAX_HOP_HEIGHT && self.frogger.player[p].is_super_hopping == 0 {
                        self.platform.dest_platform[p] = Some(id);
                    }
                    break;
                }
            }
        }

        self.frogmove.dest_tile[p] = Some(dest);

        if let Some(cp) = self.platform.curr_platform[p] {
            // Matt - PSX fix, as in single player frogon is not -1
            let frogon = self.frogger.player[p].frogon;
            if frogon == -1 || self.frogger.num_frogs == 1 {
                let plat = &mut self.platform.platform_list.array[cp.idx()];
                plat.carrying = None;
                plat.flags &= !PLATFORM_NEW_CARRYINGFROG;
            } else {
                let fo = frogon as usize & 3;
                self.platform.platform_list.array[cp.idx()].carrying = self.frogger.frog[fo];
                self.platform.curr_platform[fo] = Some(cp);
            }
            self.platform.curr_platform[p] = None;
        }

        self.frogger.player[p].frog_state |= if self.platform.dest_platform[p].is_some() { FROGSTATUS_ISJUMPINGTOPLATFORM } else { FROGSTATUS_ISJUMPINGTOTILE };

        let from_tile = self.tile_or_default(from);
        self.frogmove.next_frog_facing[p] = get_tiles_matching_direction(&from_tile, self.frogmove.frog_facing[p], &dest_tile);

        let pp = &self.frogger.player[p];
        let (t, h) = if self.game_state.multi != SINGLEPLAYER && self.multi.multiplayer_mode == MULTIMODE_BATTLE {
            let t = if pp.quickhop.time != 0 {
                STANDARD_HOP_FRAMES
            } else if pp.slowhop.time != 0 {
                SUPER_HOP_FRAMES
            } else {
                BATTLE_HOP_FRAMES
            };
            (t, HOP_HEIGHT)
        } else if pp.is_super_hopping != 0 {
            (if pp.quickhop.time != 0 { QUICK_SUPER_FRAMES } else { SUPER_HOP_FRAMES }, SUPERHOP_HEIGHT)
        } else if pp.quickhop.time != 0 {
            (QUICK_HOP_FRAMES, HOP_HEIGHT)
        } else if pp.slowhop.time != 0 {
            (SLOW_HOP_FRAMES, HOP_HEIGHT)
        } else {
            (STANDARD_HOP_FRAMES, HOP_HEIGHT)
        };

        // determine where frog is going to land...
        let fpos = self.frog_pos(p);
        let cn = self.ctile(p).normal;
        if self.frogger.player[p].frog_state & FROGSTATUS_ISJUMPINGTOPLATFORM != 0 {
            let to = self.plat_pos(self.platform.dest_platform[p].expect("destPlatform"));
            self.calculate_frog_jump_s(&fpos, &to, &cn, h, t, pl);
        } else {
            self.calculate_frog_jump_s(&fpos, &dest_tile.centre, &cn, h, t, pl);
        }

        self.frogmove.num_hops_total += 1;
        if self.frogmove.speed_frame_count != 0 {
            self.frogmove.speed_hops_total += self.frame_count - self.frogmove.speed_frame_count;
        }
        self.frogmove.speed_frame_count = self.frame_count;
        true
    }

    /// The splat-on-landing death of `CheckForFroggerLanding`.
    fn splat_frog(&mut self, p: usize, tile: &GameTile, anim_speed: i32) {
        if self.frogger.player[p].dead.time != 0 {
            return;
        }
        if self.game_state.multi != SINGLEPLAYER {
            self.kill_mp_frog(p as i32);
        } else {
            let n = fv(&tile.normal);
            self.create_special_effect(FXTYPE_DECAL, &tile.centre, &n, 102400, 4096, 410, 3277);
            self.frogger.player[p].death_by = DEATHBY_NORMAL as u8;
            frog_anim!(self, p, FROG_ANIM_SPLATFRAME, false, false, anim_speed);
            self.frogger.player[p].frog_state |= FROGSTATUS_ISDEAD;
            self.frogger.player[p].dead.init(3);
            self.set_frog_shadow_draw(p, 0);
        }
        self.play_sample(f2_formats::crc::crc("frogfall"), None, 0, SAMPLE_VOLUME, -1);
    }

    /// Port of `CheckForFroggerLanding` (Common/frogmove.c:1238).
    pub fn check_for_frogger_landing(&mut self, pl: i32) {
        let p = pl as usize;
        let pp = &self.frogger.player[p];
        if pp.jump_time < 4096 || pp.death_by as i32 == DEATHBY_FALLINGFOREVER {
            return; // we haven't landed yet.
        }

        // in case we have to keep jumping, keep track of how far past the tile we've jumped this frame
        let overrun = pp.jump_time;

        let pp = &mut self.frogger.player[p];
        pp.can_jump = 1;
        pp.is_super_hopping = 0;
        pp.has_double_jumped = 0;
        pp.jump_time = -4096;
        self.game.double_queue[p] = 0;

        // Assume for now that if we've landed after being on another frogs head then we're no longer on it.
        if self.game_state.multi == MULTILOCAL && self.multi.multiplayer_mode != MULTIMODE_BATTLE && self.frogger.player[p].frogon != -1 {
            let f = self.frogger.player[p].frogon as usize & 3;
            if self.frogger.player[f].frogunder == pl as i16 {
                self.frogger.player[f].frogunder = -1;
                self.frogger.player[f].stun.init(1);
                self.frogger.player[f].can_jump = 1;
            }
            self.frogger.player[p].frogon = -1;
            self.frogger.player[p].idle_enable = 1;
            if self.frogger.player[f].frogon == -1 {
                if self.frogger.player[f].frogunder == -1 {
                    self.frogger.player[f].idle_enable = 1;
                    frog_anim!(self, f, FROG_ANIM_BREATHE, true, false, FROG_BREATHE_SPEED);
                }
            } else {
                frog_anim!(self, f, FROG_ANIM_PIN, true, false, 128);
            }
        }

        if let Some(cp) = self.platform.curr_platform[p] {
            self.platform.platform_list.array[cp.idx()].carrying = None;
            self.platform.curr_platform[p] = None;
        }

        // Finish anims when floating
        let cur_anim =
            self.frogger.frog[p].and_then(|f| self.actor2.actors[f].actor.actual_actor.animation.as_ref().map(|a| a.current_animation as i32)).unwrap_or(-1);
        if self.frogger.player[p].frog_state & FROGSTATUS_ISFLOATING != 0 || (self.game_state.multi != SINGLEPLAYER && cur_anim == FROG_ANIM_FALL) {
            if self.frogger.frog_pool(self.frogger.player[p].character as usize % FROG_NUMFROGS).anim != 0 && self.game_state.multi == SINGLEPLAYER {
                frog_anim!(self, 0, FROG_ANIM_FALLLAND, false, false, 128);
                frog_anim!(self, p, FROG_ANIM_BREATHE, true, true, FROG_BREATHE_SPEED);
            } else {
                frog_anim!(self, p, FROG_ANIM_BREATHE, true, false, FROG_BREATHE_SPEED);
            }
        }

        let mut tile: Option<TileId> = None;
        if self.frogger.player[p].death_by as i32 == DEATHBY_WHACKING {
            // Do some simplified frog jump maths to stick Frogger to the screen
            let pp = &self.frogger.player[p];
            let mut pos = FVector::default();
            if pp.jump_multiplier != 0 {
                let mut mult = pp.jump_multiplier.wrapping_mul(2).wrapping_sub(4096);
                mult = 4096i32.wrapping_sub(mult.wrapping_mul(mult) >> 12);
                pos = pp.jump_up_vector;
                scale_vector_ff(&mut pos, mult);
            }
            add_to_vector_fs(&mut pos, &pp.jump_origin);
            add_to_vector_ff(&mut pos, &pp.jump_fwd_vector);
            let mut s = SVector::default();
            set_vector_sf(&mut s, &pos);
            self.set_frog_pos(p, s);
        } else if self.frogger.player[p].frog_state & FROGSTATUS_ISJUMPINGTOPLATFORM != 0 {
            let dp = self.platform.dest_platform[p].expect("destPlatform");
            let t = self.platform.platform_list.array[dp.idx()].in_tile[0];
            let tt = self.tile_or_default(t);

            // ok - frog has landed
            let ppos = self.plat_pos(dp);
            self.set_frog_pos(p, ppos);
            let ct = self.ctile(p);
            self.frogmove.frog_facing[p] = get_tiles_matching_direction(&ct, self.frogmove.frog_facing[p], &tt);

            let plat = &mut self.platform.platform_list.array[dp.idx()];
            plat.flags |= PLATFORM_NEW_CARRYINGFROG;
            plat.carrying = self.frogger.frog[p];
            let pp = &mut self.frogger.player[p];
            pp.frog_state |= FROGSTATUS_ISONMOVINGPLATFORM;
            pp.frog_state &= !(FROGSTATUS_ISJUMPINGTOTILE
                | FROGSTATUS_ISFLOATING
                | FROGSTATUS_ISSLIDING
                | FROGSTATUS_ISJUMPINGTOPLATFORM
                | FROGSTATUS_ISSUPERHOPPING
                | FROGSTATUS_ISSAFE);

            self.platform.curr_platform[p] = Some(dp);
            self.frogmove.curr_tile[p] = t;
            self.platform.dest_platform[p] = None;

            if self.frogger.player[p].height_jumped < -DROP_KILL_HEIGHT {
                // BUG(original): uses destTile (may be stale/NULL) for the decal.
                let dt = self.tile_or_default(self.frogmove.dest_tile[p]);
                self.splat_frog(p, &dt, 64);
            } else {
                frog_anim!(self, p, FROG_ANIM_BREATHE, true, true, FROG_BREATHE_SPEED);
            }
            self.check_tile_for_collectable(t, pl);
            tile = t;
        } else {
            if self.frogmove.dest_tile[p].is_none() {
                tile = self.frogmove.curr_tile[p];
            } else {
                let ct = self.ctile(p);
                let dt_id = self.frogmove.dest_tile[p];
                let dt = self.tile_or_default(dt_id);
                if ct.state >= tilestate::CONVEYOR_ONEWAY && dt.state < tilestate::CONVEYOR_ONEWAY {
                    let cam = get_tiles_matching_direction(&ct, self.cam.cam_facing[p], &dt);
                    frog_anim!(self, p, FROG_ANIM_BREATHE, true, false, FROG_BREATHE_SPEED);
                    let mut fwd = FVector::default();
                    sub_vector_fss(&mut fwd, &dt.centre, &ct.centre);
                    make_unit(&mut fwd);
                    // SetVectorFF(&dir, &fwd) into an SVECTOR: plain truncating copy
                    let dir = SVector { vx: fwd.vx as i16, vy: fwd.vy as i16, vz: fwd.vz as i16, pad: 0 };
                    let ff = get_tiles_matching_vector(&dir, &dt);
                    self.frogmove.frog_facing[p] = ff;
                    self.frogmove.next_frog_facing[p] = ff;
                    self.frogger.player[p].extended_hop_dir = ((ff - cam) & 3) as u8;
                }
                self.frogmove.curr_tile[p] = dt_id;
                tile = dt_id;
                self.frogmove.dest_tile[p] = None;
            }

            let tt = self.tile_or_default(tile);
            let state = tt.state;

            if self.frogger.player[p].death_by as i32 != DEATHBY_WHACKING {
                self.frogmove.frog_facing[p] = self.frogmove.next_frog_facing[p];
                let ff = (self.frogmove.frog_facing[p] & 3) as usize;
                self.orientate_frog_ss(p, &tt.dir_vector[ff], &tt.normal);
                // DON'T set frog to centre of tile - ds
                if state != tilestate::FALL {
                    self.set_frog_pos(p, tt.centre);
                }
            }

            self.frogger.player[p].frog_state &= !(FROGSTATUS_ISJUMPINGTOTILE
                | FROGSTATUS_ISJUMPINGTOPLATFORM
                | FROGSTATUS_ISSLIDING
                | FROGSTATUS_ISONMOVINGPLATFORM
                | FROGSTATUS_ISSUPERHOPPING
                | FROGSTATUS_ISFLOATING
                | FROGSTATUS_ISSAFE);

            if self.frogger.player[p].frog_state & FROGSTATUS_ISDEAD != 0 {
                // We're bouncing along the ground
                if self.frogger.player[p].death_by as i32 == DEATHBY_FIRE {
                    self.bounce_frog(pl, 2048000, 25);
                    frog_anim!(self, p, FROG_ANIM_ASSONFIRE, false, false, 128);
                }
            } else {
                // check tile to see if frog has jumped onto a certain tile type
                self.check_tile_state(tile, pl);

                if self.frogger.player[p].height_jumped < -DROP_KILL_HEIGHT || state == tilestate::DEADLYFALL {
                    self.splat_frog(p, &tt, 256);
                }
                if state == tilestate::FALL {
                    self.frogger.player[p].jump_time = overrun;
                }
            }

            // Next, check if frog has landed on a collectable
            self.check_tile_for_collectable(tile, pl);
        }

        self.check_for_frog_on(pl, tile);
    }

    /// Port of `CheckForFrogOn` (Common/frogmove.c:1501): multiplayer frog stacking.
    pub fn check_for_frog_on(&mut self, pl: i32, tile: Option<TileId>) {
        let p = pl as usize;
        let n = self.frogger.num_frogs;
        if !(self.frogger.player[p].frogunder == -1 && self.game_state.multi == MULTILOCAL && self.multi.multiplayer_mode != MULTIMODE_BATTLE) || n <= 0 {
            return;
        }
        // If we've landed on another frog, sit on his head
        let mut i = (pl + 1) % n;
        while i != pl {
            let iu = i as usize;
            let next = (i + 1) % n;
            let same_plat = self.platform.curr_platform[iu].is_some() && self.platform.curr_platform[iu] == self.platform.curr_platform[p];
            if (tile.is_some() && self.frogmove.curr_tile[iu] == tile) || same_plat {
                let far = distance_between_points_ss(&self.frog_pos(p), &self.frog_pos(iu)) > (20 * SCALE) << 12;
                if (self.frogmove.dest_tile[iu] != tile && self.frogmove.dest_tile[iu].is_some() && tile.is_some())
                    || self.frogger.player[p].frog_state & FROGSTATUS_ISDEAD != 0
                    || far
                {
                    i = next;
                    continue;
                }

                // Face all lower frogs to our direction
                self.frogmove.frog_facing[iu] = self.frogmove.frog_facing[p];
                self.frogmove.next_frog_facing[iu] = self.frogmove.frog_facing[p];
                if let (Some(fi), Some(fp)) = (self.frogger.frog[iu], self.frogger.frog[p]) {
                    self.actor2.actors[fi].actor.q_rot = self.actor2.actors[fp].actor.q_rot;
                }
                self.orientate_frog_to_curr_tile(iu);

                let under = self.frogger.player[iu].frogunder;
                if under != -1 {
                    let u = under as usize & 3;
                    if self.frogger.player[u].can_jump != 0 || self.frogger.player[u].frogunder != -1 {
                        i = next;
                        continue;
                    }
                }

                self.play_sample(f2_formats::crc::crc("frogcrush"), None, 0, SAMPLE_VOLUME, -1);
                self.frogger.player[iu].can_jump = 0;
                self.frogger.player[p].frogon = i as i16;
                self.frogger.player[iu].frogunder = pl as i16;
                self.frogger.player[iu].frog_state &= !FROGSTATUS_ALLHOPFLAGS;
                self.frogger.player[p].idle_enable = 0;
                self.frogger.player[iu].idle_enable = 0;
                frog_anim!(self, p, FROG_ANIM_PIN, true, false, 128);

                let on = self.frogger.player[iu].frogon;
                if on != -1 {
                    let mut up = fv(&self.ctile(iu).normal);
                    scale_vector(&mut up, 10);
                    let base = self.frog_pos(on as usize & 3);
                    let mut np = SVector::default();
                    add_vector_ssf(&mut np, &base, &up);
                    self.set_frog_pos(iu, np);
                }
                let ip = self.frog_pos(iu);
                self.set_frog_pos(p, ip);
                frog_anim!(self, iu, FROG_ANIM_PINNED, true, false, 128);

                if same_plat {
                    let cp = self.platform.curr_platform[iu].expect("same platform");
                    self.platform.platform_list.array[cp.idx()].carrying = self.frogger.frog[iu];
                }
            }
            i = next;
        }
    }

    /// The jump setup shared by the conveyor and ice branches of `CheckTileState`.
    fn slide_to_dest(&mut self, p: usize, tile: &GameTile, frames: i32) -> FVector {
        let fpos = self.frog_pos(p);
        let to = if self.frogger.player[p].frog_state & FROGSTATUS_ISJUMPINGTOPLATFORM != 0 {
            self.plat_pos(self.platform.dest_platform[p].expect("destPlatform"))
        } else {
            self.tile_or_default(self.frogmove.dest_tile[p]).centre
        };
        self.calculate_frog_jump_s(&fpos, &to, &tile.normal, -1, frames, p as i32);
        let fpos = self.frog_pos(p);
        let mut fwd = FVector::default();
        sub_vector_fss(&mut fwd, &to, &fpos);
        fwd
    }

    /// Port of `CheckTileState` (Common/frogmove.c:1561): when landing on a tile, check
    /// what state it is and do stuff.
    pub fn check_tile_state(&mut self, tile: Option<TileId>, pl: i32) {
        let p = pl as usize;
        let Some(tile_id) = tile else { return };
        let tt = self.map.tile(tile_id).clone();
        let state = tt.state;
        let last_state = self.cam.last_tile[p].map(|t| self.map.tile(t).state);

        if state & tilestate::CONVEYOR != 0 {
            // -------------------------------- Conveyors ----------------------------
            let res = self.move_to_requested_destination(((state & (tilestate::CONVEYOR - 1)) as i32 - self.cam.cam_facing[p]) & 3, pl);
            if res {
                let speed;
                if state >= tilestate::CONVEYOR_ONEWAY {
                    speed = 2;
                    if let Some(f) = self.frogger.frog[p] {
                        self.start_animate_actor(f, FROG_ANIM_ICE2, true, false, 64, false);
                    }
                    // lastTile[pl] is NULL until the camera has run (wave D): no voice then.
                    if last_state.is_some_and(|s| s < tilestate::CONVEYOR_ONEWAY) {
                        self.play_voice(pl, "frogslide");
                    }
                    self.frogger.player[p].can_jump = 0;
                } else {
                    speed = if state >= tilestate::CONVEYOR_FAST {
                        2
                    } else if state >= tilestate::CONVEYOR_MED {
                        1
                    } else {
                        0
                    };
                    if let Some(f) = self.frogger.frog[p] {
                        self.start_animate_actor(f, FROG_ANIM_BREATHE, true, true, FROG_BREATHE_SPEED, false);
                    }
                }

                let mut fwd = self.slide_to_dest(p, &tt, CONVEYOR_FRAMES[speed]);
                self.frogger.player[p].frog_state |= FROGSTATUS_ISSLIDING;

                if state >= tilestate::CONVEYOR_ONEWAY {
                    make_unit(&mut fwd);
                    let n = self.ctile(p).normal;
                    if let Some(f) = self.frogger.frog[p] {
                        orientate_fs(&mut self.actor2.actors[f].actor.q_rot, &fwd, &n);
                    }
                }
            }
            return;
        }

        match state {
            tilestate::DEADLY => {
                if self.frogger.player[p].dead.time == 0 {
                    let n = fv(&self.ctile(p).normal);
                    self.create_special_effect(FXTYPE_WATERRIPPLE, &tt.centre, &n, 81920, 3277, 410, 2458);
                    let fpos = self.frog_pos(p);
                    if let Some(fx) = self.create_special_effect(FXTYPE_SPLASH, &fpos, &n, 40960, 16384, 0, 8192) {
                        self.specfx.fx_mut(fx).gravity = 8190;
                        if let Some(rb) = self.fx_alloc_rebound(fx) {
                            let pl2 = self.specfx.p2_mut(rb);
                            f2_math::set_vector_ff(&mut pl2.normal, &n);
                            // BUG(original): SetVectorFF(&rebound->point, &frog->actor->position)
                            // copies 12 bytes into the 8-byte SVECTOR (clobbering normal.vx);
                            // the port copies the position only.
                            pl2.point = fpos;
                        }
                    }

                    if self.game_state.multi == SINGLEPLAYER {
                        if self.frogger.frog_pool(self.frogger.player[p].character as usize % FROG_NUMFROGS).anim != 0 {
                            self.call_death_anim(DEATHBY_DROWNING, pl);
                        } else {
                            self.death_multi(pl);
                        }
                    } else {
                        self.kill_mp_frog(pl);
                    }
                }
            }
            tilestate::PANTS => {
                if self.frogger.player[p].dead.time == 0 {
                    if self.game_state.multi == SINGLEPLAYER {
                        self.frogger.player[p].frog_state |= FROGSTATUS_ISDEAD;
                        self.call_death_anim(DEATHBY_POISON, pl);
                    } else {
                        self.kill_mp_frog(pl);
                    }
                }
            }
            tilestate::FALL => {
                if self.frogger.player[p].dead.time == 0 {
                    if self.game_state.multi == SINGLEPLAYER {
                        let pp = &mut self.frogger.player[p];
                        pp.death_by = DEATHBY_FALLINGFOREVER as u8;
                        pp.frog_state |= FROGSTATUS_ISDEAD;
                        pp.dead.init(2);
                        self.set_frog_shadow_draw(p, 0);
                        self.frogger.player[p].jump_speed = 75;
                        // DOn't follow frog cos it exposes nasty things
                        self.cam.fixed_pos = 1;
                        self.cam.fixed_dir = 1;
                        self.cam.cam_source = self.cam.curr_cam_source;
                    } else {
                        self.kill_mp_frog(pl);
                    }
                    self.play_sample(f2_formats::crc::crc("frogfall"), None, 0, SAMPLE_VOLUME, -1);
                }
            }
            tilestate::NOSUPERHOT | tilestate::HOT => {
                if self.game_state.multi != SINGLEPLAYER {
                    self.kill_mp_frog(pl);
                } else {
                    self.frogger.player[p].frog_state |= FROGSTATUS_ISDEAD;
                    if self.rng.random(2) != 0 {
                        self.call_death_anim(DEATHBY_FIRE, pl);
                    } else {
                        self.call_death_anim(DEATHBY_INCINERATE, pl);
                    }
                }
            }
            tilestate::ELECTRIC => {
                if self.game_state.multi != SINGLEPLAYER {
                    self.kill_mp_frog(pl);
                } else {
                    self.frogger.player[p].frog_state |= FROGSTATUS_ISDEAD;
                    self.call_death_anim(DEATHBY_ELECTRIC, pl);
                }
            }
            tilestate::SINK => {
                self.frogger.player[p].is_sinking = 1;
            }
            tilestate::ICE => {
                let res = self.move_to_requested_destination((self.frogmove.frog_facing[p] - self.cam.cam_facing[p]) & 3, pl);
                if res {
                    let mut fwd = self.slide_to_dest(p, &tt, ICE_FRAMES);
                    if let Some(f) = self.frogger.frog[p] {
                        self.start_animate_actor(f, FROG_ANIM_ICETUBE, true, false, 64, false);
                    }
                    if last_state.is_some_and(|s| s != tilestate::ICE) {
                        self.play_voice(pl, "frogslide");
                    }
                    self.frogger.player[p].can_jump = 0;
                    self.frogger.player[p].frog_state |= FROGSTATUS_ISSLIDING;
                    make_unit(&mut fwd);
                    let n = self.ctile(p).normal;
                    if let Some(f) = self.frogger.frog[p] {
                        orientate_fs(&mut self.actor2.actors[f].actor.q_rot, &fwd, &n);
                    }
                }
            }
            _ => {
                // thank heavens, we've landed on a normal tile!
                frog_anim!(self, p, FROG_ANIM_BREATHE, true, true, FROG_BREATHE_SPEED);
            }
        }
    }

    /// Port of `GameTileTooHigh` (Common/frogmove.c:1777).
    pub fn game_tile_too_high(&self, tile: TileId, pl: i32) -> bool {
        let p = pl as usize;
        let mut diff = FVector::default();
        sub_vector_fss(&mut diff, &self.map.tile(tile).centre, &self.frog_pos(p));
        let height = dot_product_ff(&diff, &fv(&self.ctile(p).normal));
        height > if self.frogger.player[p].is_super_hopping != 0 { MAX_SUPERHOP_HEIGHT } else { MAX_HOP_HEIGHT }
    }

    /// Port of `KillFrog` (Common/frogmove.c:1797): runs the death sequence; TRUE while
    /// it is running.
    pub fn kill_frog(&mut self, pl: i32) -> bool {
        let p = pl as usize;
        self.remove_frog_tongue(pl);

        let afc = self.act_frame_count;
        self.frogger.player[p].dead.update(-1, afc);
        if self.frogger.player[p].dead.time == 0 {
            self.frogmove.num_health_total += 1;

            // If substitute frog model was used then swap them back here
            if let Some(f) = self.frogger.frog[p] {
                if self.undo_change_model(f) != 0 {
                    self.frogger.player[p].idle_enable = 1;
                }
            }

            // cheatCombos[CHEAT_INFINITE_LIVES].state is 0 (cheats not ported).
            if self.game_state.mode == INGAME_MODE {
                if !self.game_options().infinite_lives_on() {
                    // P6-1: infinite lives assist (enhanced only).
                    self.frogger.player[p].lives -= 1;
                }
                if self.frogger.player[p].lives <= 0 {
                    self.start_game_over();
                    return false;
                }
            }

            self.frogger.player[p].frog_state &= !FROGSTATUS_ISDEAD;

            if let Some(t) = self.map.g_t_start[0] {
                self.set_frogger_start_pos(t, pl);
            }
            self.frogger.player[0].safe.init(2);
            self.init_camera();
            self.play_voice(pl, "frogletsgo");
            self.reset_platform_flags();
            return false;
        }

        match self.frogger.player[p].death_by as i32 {
            DEATHBY_DROWNING => {
                if self.act_frame_count.is_multiple_of(5) {
                    let n = fv(&self.ctile(p).normal);
                    let fpos = self.frog_pos(p);
                    if let Some(fx) = self.create_special_effect(FXTYPE_BUBBLES, &fpos, &n, 32768, 3277, 0, 2458) {
                        if let Some(rb) = self.fx_alloc_rebound(fx) {
                            let mut up = n;
                            let pl2 = self.specfx.p2_mut(rb);
                            f2_math::set_vector_ff(&mut pl2.normal, &up);
                            f2_math::scale_vector(&mut up, 30);
                            f2_math::add_vector_ssf(&mut pl2.point, &fpos, &up);
                        }
                    }
                }
            }
            DEATHBY_FIRE => {
                if self.act_frame_count.is_multiple_of(2) {
                    let n = fv(&self.ctile(p).normal);
                    let fpos = self.frog_pos(p);
                    self.create_special_effect(FXTYPE_FIERYSMOKE, &fpos, &n, 204800, 2048, 0, 6144);
                }
            }
            DEATHBY_ELECTRIC => {
                if let Some(f) = self.frogger.frog[p] {
                    let dx = (-2048 + (self.rng.random(2) << 12)) >> 12;
                    let a = &mut self.actor2.actors[f].actor.position;
                    a.vx = (a.vx as i32).wrapping_add(dx) as i16;
                    let dy = (-2048 + (self.rng.random(2) << 12)) >> 12;
                    let a = &mut self.actor2.actors[f].actor.position;
                    a.vy = (a.vy as i32).wrapping_add(dy) as i16;
                    let dz = (-2048 + (self.rng.random(2) << 12)) >> 12;
                    let a = &mut self.actor2.actors[f].actor.position;
                    a.vz = (a.vz as i32).wrapping_add(dz) as i16;
                }
            }
            _ => {}
        }
        true
    }

    /// Port of `HopFrogToTile` (Common/frogmove.c:1896).
    pub fn hop_frog_to_tile(&mut self, tile: TileId, pl: i32) {
        let p = pl as usize;
        self.frogger.player[p].frog_state |= FROGSTATUS_ISJUMPINGTOTILE;
        self.frogger.player[p].can_jump = 0;
        self.frogmove.dest_tile[p] = Some(tile);

        let ct = self.ctile(p);
        let dt = self.map.tile(tile).clone();
        let mut v = FVector::default();
        sub_vector_fss(&mut v, &ct.centre, &dt.centre);
        let mut maxdot: Fixed = 0;
        let mut dir = 0;
        for j in 0..4 {
            let dot = dot_product_ff(&fv(&ct.dir_vector[j]), &v);
            if dot > maxdot {
                maxdot = dot;
                dir = j as i32;
            }
        }

        self.animate_frog_hop(dir, pl);
        self.frogmove.frog_facing[p] = (dir + 2) & 3;
        self.frogmove.next_frog_facing[p] = (dir + 2) & 3;
        self.orientate_frog_to_curr_tile(p);

        let fpos = self.frog_pos(p);
        self.calculate_frog_jump_s(&fpos, &dt.centre, &ct.normal, HOP_HEIGHT, STANDARD_HOP_FRAMES, pl);
    }

    /// Port of `SpringFrogToTile` (Common/frogmove.c:1935). `time` is in seconds (20.12).
    pub fn spring_frog_to_tile(&mut self, tile: TileId, height: Fixed, time: Fixed, pl: i32) {
        let p = pl as usize;
        let t = time.wrapping_mul(60) >> 12;
        self.frogger.player[p].frog_state |= FROGSTATUS_ISJUMPINGTOTILE;
        self.frogger.player[p].can_jump = 0;
        self.frogmove.dest_tile[p] = Some(tile);

        if let Some(cp) = self.platform.curr_platform[p] {
            let plat = &mut self.platform.platform_list.array[cp.idx()];
            plat.carrying = None;
            plat.flags &= !PLATFORM_NEW_CARRYINGFROG;
            self.platform.curr_platform[p] = None;
        }
        self.frogmove.dest_tile[p] = Some(tile);

        let fpos = self.frog_pos(p);
        let centre = self.map.tile(tile).centre;
        let n = self.ctile(p).normal;
        self.calculate_frog_jump_s(&fpos, &centre, &n, height, t, pl);
    }

    /// The direction loop of `PushFrog`: the tile direction closest to `direction`.
    fn push_dir(&self, p: usize, direction: &FVector) -> i32 {
        let ct = self.ctile(p);
        let mut distance: Fixed = -100000;
        // `res` is uninitialised in the original if no dot product beats -100000.
        let mut res = 0;
        for i in 0..4 {
            let t = dot_product_ff(direction, &fv(&ct.dir_vector[i]));
            if t > distance {
                distance = t;
                res = i as i32;
            }
        }
        res
    }

    /// Port of `PushFrog` (Common/frogmove.c:1962): push the frog in a given direction.
    pub fn push_frog(&mut self, direction: &FVector, pl: i32) {
        let p = pl as usize;
        if self.platform.curr_platform[p].is_some() {
            let res = self.push_dir(p, direction);
            self.move_to_requested_destination((res - self.cam.cam_facing[p]) & 3, pl);
        }

        if self.frogger.player[p].frog_state & FROGSTATUS_ISJUMPINGTOPLATFORM != 0 {
            let dp = self.platform.dest_platform[p];
            let fpos = self.frog_pos(p);
            let to = dp.map_or(SVector::default(), |d| self.plat_pos(d));
            let n = self.ctile(p).normal;
            self.calculate_frog_jump_s(&fpos, &to, &n, 819200, STANDARD_HOP_FRAMES, pl);
            self.platform.curr_platform[p] = dp;
        } else {
            let res = self.push_dir(p, direction);
            self.move_to_requested_destination((res - self.cam.cam_facing[p]) & 3, pl);
        }

        if self.frogmove.dest_tile[p].is_some() {
            self.frogmove.curr_tile[p] = self.frogmove.dest_tile[p];
        } else {
            self.frogger.player[p].frog_state |= FROGSTATUS_ISDEAD;
            self.death_normal(pl);
        }
    }

    /// Port of `ThrowFrogAtScreen` (Common/frogmove.c:2029).
    pub fn throw_frog_at_screen(&mut self, pl: i32) {
        let p = pl as usize;
        if self.frogmove.dest_tile[p].is_some() {
            self.frogmove.curr_tile[p] = self.frogmove.dest_tile[p];
            self.frogmove.dest_tile[p] = None;
        }

        self.frogger.player[p].frog_state &= !(FROGSTATUS_ALLHOPFLAGS | FROGSTATUS_ISFLOATING);
        let ct = self.ctile(p);
        self.set_frog_pos(p, ct.centre);

        // Calculate the point to throw the frog to
        let mut v = FVector::default();
        sub_vector_fff(&mut v, &self.cam.curr_cam_source, &self.cam.curr_cam_target); // v points from screen target to screen
        make_unit(&mut v);

        let mut w = v;
        scale_vector_ff(&mut w, -SCREEN_DIST);
        let mut target = SVector::default();
        f2_math::add_vector_sff(&mut target, &w, &self.cam.curr_cam_source);

        let cross = fv(&ct.dir_vector[((self.cam.cam_facing[p] + 1) & 3) as usize]);
        let mut up = FVector::default();
        cross_product_fff(&mut up, &cross, &v);
        make_unit(&mut up);
        if let Some(f) = self.frogger.frog[p] {
            orientate(&mut self.actor2.actors[f].actor.q_rot, &v, &up);
        }

        let mut v = up;
        scale_vector(&mut v, -150);
        add_to_vector_sf(&mut target, &v);

        let fpos = self.frog_pos(p);
        self.calculate_frog_jump_s(&fpos, &target, &ct.normal, 0, 80, pl);

        self.frogmove.frog_facing[p] = (self.cam.cam_facing[p] + 2) & 3;

        self.cam.cam_source = self.cam.curr_cam_source;
        self.cam.cam_target = self.cam.curr_cam_target;
        self.cam.fixed_pos = 1;
        self.cam.fixed_dir = 1;
        self.cam.control_camera = 1;

        self.frogger.player[p].idle_enable = 0;
    }

    /// Port of `CalculateFrogJump` (Common/frogmove.c:2085): jump curve variables.
    pub fn calculate_frog_jump(&mut self, start_pos: &SVector, end_pos: &SVector, normal: &FVector, mut height: Fixed, time: i32, p: i32) {
        let pl = &mut self.frogger.player[p as usize];
        pl.jump_origin.vx = start_pos.vx;
        pl.jump_origin.vy = start_pos.vy;
        pl.jump_origin.vz = start_pos.vz;
        let mut v = FVector::default();
        sub_vector_fss(&mut v, end_pos, start_pos);
        let div = |a: i32, b: i32| if b == 0 { 0 } else { a.wrapping_div(b) };

        if height < 0 {
            // just *slide*, force linear movement with no curve
            pl.jump_fwd_vector = v;
            pl.jump_multiplier = 0;
            pl.height_jumped = 0;
            pl.jump_speed = div(4096, time);
        } else {
            // Forward vector
            let diff = dot_product_ff(normal, &v); // projection of V onto normal
            let mut q = *normal;
            scale_vector_ff(&mut q, diff);
            let mut f = FVector::default();
            sub_vector_fff(&mut f, &v, &q); // F = V - N(N.V)
            pl.jump_fwd_vector = f;

            if diff > 0 {
                height = height.wrapping_add(diff); // When we're jumping UP, add height
            }

            let m;
            if height != 0 {
                // Up vector
                let mut up = *normal;
                scale_vector_ff(&mut up, height);
                pl.jump_up_vector = up;
                // m = 0.5f * (1 + sqrtf(1 - diff/height));
                m = 4096i32.wrapping_add(fsqrt_f(4096i32.wrapping_sub(fdiv(diff, height)))) >> 1;
                pl.jump_speed = fdiv(4096, m.wrapping_mul(time));
                pl.jump_time = 0;
            } else if diff < 0 {
                // Up vector
                let mut up = *normal;
                scale_vector_ff(&mut up, diff.wrapping_neg());
                pl.jump_up_vector = up;
                sub_from_vector_sf(&mut pl.jump_origin, &up);
                m = 4096;
                pl.jump_speed = div(2048, time);
                pl.jump_time = 2048;
            } else {
                m = 0; // Probably only happens on conveyors/ice, but still.
                pl.jump_speed = div(4096, time);
                pl.jump_time = 0;
            }
            pl.jump_multiplier = m;
            pl.height_jumped = diff;
        }
    }

    /// Port of `CalculateFrogJumpS` (Common/frogmove.c:2155).
    pub fn calculate_frog_jump_s(&mut self, start_pos: &SVector, end_pos: &SVector, normal: &SVector, height: Fixed, time: i32, p: i32) {
        let f = fv(normal);
        self.calculate_frog_jump(start_pos, end_pos, &f, height, time, p);
    }

    /// Port of `BounceFrog` (Common/frogmove.c:2169): jump in the current direction.
    pub fn bounce_frog(&mut self, pl: i32, height: Fixed, time: i32) {
        let p = pl as usize;
        let Some(f) = self.frogger.frog[p] else { return };
        // Frog forward vector
        let mut dir = FVector::default();
        rotate_vector_by_quaternion_ff(&mut dir, &IN_VEC, &self.actor2.actors[f].actor.q_rot);

        // And a tile to bounce to
        let ct = self.frogmove.curr_tile[p];
        let bounce_to = ct.and_then(|c| find_joined_tile_by_direction_constrained(&self.map, c, &dir, 1024)).or(ct);

        // (the original also computes an unused `pos` = position + dir*300)
        let fpos = self.frog_pos(p);
        let to = self.tile_or_default(bounce_to).centre;
        let n = self.ctile(p).normal;
        self.calculate_frog_jump_s(&fpos, &to, &n, height, time, pl);
        self.frogmove.dest_tile[p] = bounce_to;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::GameTile;

    /// A flat strip of `n` tiles along +z (500 units apart), linked up/down.
    /// Tile directions: dirVector[0] = +z, [1] = +x, [2] = -z, [3] = -x (unit 4096),
    /// tilePtrs[i] the neighbour in direction i.
    fn strip(g: &mut Game, states: &[u32]) {
        let n = states.len();
        g.map.tiles = (0..n)
            .map(|i| GameTile {
                tile_ptrs: [(i + 1 < n).then(|| TileId((i + 1) as u32)), None, (i > 0).then(|| TileId((i - 1) as u32)), None],
                state: states[i],
                centre: SVector { vx: 0, vy: 0, vz: (i as i16) * 500, pad: 0 },
                normal: SVector { vx: 0, vy: 4096, vz: 0, pad: 0 },
                dir_vector: [
                    SVector { vx: 0, vy: 0, vz: 4096, pad: 0 },
                    SVector { vx: 4096, vy: 0, vz: 0, pad: 0 },
                    SVector { vx: 0, vy: 0, vz: -4096, pad: 0 },
                    SVector { vx: -4096, vy: 0, vz: 0, pad: 0 },
                ],
            })
            .collect();
        g.map.g_t_start[0] = Some(TileId(0));
        g.game_state.multi = SINGLEPLAYER;
        g.game_state.mode = INGAME_MODE;
        let anims = (0..NUM_FROG_ANIMS)
            .map(|i| crate::assets::MdxAnimation { anim_start: (i * 10) as i16, anim_end: (i * 10 + 5) as i16, speed: 1.0, looped: 0 })
            .collect();
        let id = f2_formats::crc::crc("frogger.obe");
        g.models.models.insert(id, std::sync::Arc::new(crate::assets::ModelInfo { id, name: "frogger".into(), animations: anims, ..Default::default() }));
        let a = g.create_and_add_actor("frogger.obe", 0, 0, 0, crate::mdx_actor::INIT_ANIMATION);
        g.frogger.frog[0] = a;
        g.actor2.actors[a.unwrap()].actor.shadow = Some(Default::default());
        g.set_frogger_start_pos(TileId(0), 0);
        g.frogger.player[0].lives = 3;
    }

    #[test]
    fn standard_hop_arc() {
        let mut g = Game::new();
        strip(&mut g, &[0, 0, 0]);
        // cam facing 0: MOVE_UP looks at tilePtrs[(0+0+2)&3] = [2]; use MOVE_DOWN for +z.
        g.frogger.player[0].frog_state |= FROGSTATUS_ISWANTINGD;
        g.frogger.player[0].can_jump = 0;
        let mut ys = vec![];
        let mut zs = vec![];
        for f in 1..=10u32 {
            g.act_frame_count = f;
            g.update_frogger_pos(0);
            let p = g.frog_position(0);
            ys.push(p.vy);
            zs.push(p.vz);
        }
        // CalculateFrogJump: flat hop, height 300, 7 frames, m = 1.0.
        assert_eq!(g.frogger.player[0].jump_multiplier, 4096);
        assert_eq!(g.frogmove.curr_tile[0], Some(TileId(1)));
        // Arc rises then lands on the tile centre.
        assert!(ys[2] > 200, "{ys:?}");
        assert_eq!(*zs.last().unwrap(), 500);
        assert_eq!(*ys.last().unwrap(), 0);
        assert_eq!(g.frogger.player[0].can_jump, 1);
        // the z motion is monotonic
        assert!(zs.windows(2).all(|w| w[0] <= w[1]), "{zs:?}");
    }

    #[test]
    fn calculate_jump_up_and_down() {
        let mut g = Game::new();
        strip(&mut g, &[0, 0]);
        let n = FVector { vx: 0, vy: 4096, vz: 0 };
        let a = SVector { vx: 0, vy: 0, vz: 0, pad: 0 };
        let b = SVector { vx: 0, vy: 100, vz: 500, pad: 0 };
        g.calculate_frog_jump(&a, &b, &n, HOP_HEIGHT, 7, 0);
        let p = &g.frogger.player[0];
        assert_eq!(p.height_jumped, 100 << 12);
        assert_eq!(p.jump_up_vector.vy, HOP_HEIGHT + (100 << 12));
        assert!(p.jump_multiplier > 2048 && p.jump_multiplier < 4096);
        assert_eq!(p.jump_time, 0);
        // dropping down with height 0: start half-way along a full parabola
        let c = SVector { vx: 0, vy: -200, vz: 500, pad: 0 };
        g.calculate_frog_jump(&a, &c, &n, 0, 8, 0);
        let p = &g.frogger.player[0];
        assert_eq!(p.jump_multiplier, 4096);
        assert_eq!(p.jump_time, 2048);
        assert_eq!(p.jump_speed, 256);
        assert_eq!(p.jump_origin.vy, -200);
        // slide
        g.calculate_frog_jump(&a, &b, &n, -1, 15, 0);
        assert_eq!(g.frogger.player[0].jump_multiplier, 0);
        assert_eq!(g.frogger.player[0].jump_speed, 4096 / 15);
    }

    fn hop_onto(state: u32) -> Game {
        let mut g = Game::new();
        strip(&mut g, &[0, state, 0, 0]);
        g.frogger.player[0].frog_state |= FROGSTATUS_ISWANTINGD;
        for f in 1..=9u32 {
            g.act_frame_count = f;
            g.update_frogger_pos(0);
        }
        g
    }

    #[test]
    fn deadly_tile_drowns() {
        let g = hop_onto(tilestate::DEADLY);
        assert_eq!(g.frogger.player[0].death_by as i32, DEATHBY_DROWNING);
        assert!(g.frogger.player[0].frog_state & FROGSTATUS_ISDEAD != 0 || g.frogger.player[0].dead.time != 0);
    }

    #[test]
    fn sink_tile_sets_sinking() {
        let g = hop_onto(tilestate::SINK);
        assert_eq!(g.frogger.player[0].is_sinking, 1);
    }

    #[test]
    fn fall_tile_falls_forever() {
        let g = hop_onto(tilestate::FALL);
        assert_eq!(g.frogger.player[0].death_by as i32, DEATHBY_FALLINGFOREVER);
        assert_eq!(g.frogger.player[0].dead.time, 2);
    }

    #[test]
    fn barred_and_smash_tiles_block() {
        let mut g = Game::new();
        strip(&mut g, &[0, tilestate::BARRED]);
        assert!(!g.move_to_requested_destination(MOVE_DOWN, 0));
        let mut g = Game::new();
        strip(&mut g, &[0, tilestate::SMASH]);
        assert!(!g.move_to_requested_destination(MOVE_DOWN, 0));
        assert_eq!(g.map.tiles[1].state, tilestate::SMASH);
        g.frogger.player[0].frog_state |= FROGSTATUS_ISSUPERHOPPING;
        assert!(!g.move_to_requested_destination(MOVE_DOWN, 0));
        assert_eq!(g.map.tiles[1].state, tilestate::NORMAL);
    }

    #[test]
    fn conveyor_carries_frog() {
        // conveyor direction 2 (MOVE_DOWN: tilePtrs[0], +z) at slow speed
        let g = hop_onto(tilestate::CONVEYOR | 2);
        assert!(g.frogger.player[0].frog_state & FROGSTATUS_ISSLIDING != 0);
        assert_eq!(g.frogger.player[0].jump_speed, 4096 / CONVEYOR_FRAMES[0]);
        assert_eq!(g.frogmove.dest_tile[0], Some(TileId(2)));
    }

    #[test]
    fn ice_slides_forward() {
        let g = hop_onto(tilestate::ICE);
        assert_eq!(g.frogger.player[0].can_jump, 0);
        assert_eq!(g.frogmove.dest_tile[0], Some(TileId(2)));
        assert_eq!(g.frogger.player[0].jump_speed, 4096 / ICE_FRAMES);
    }

    #[test]
    fn superhop_in_place_and_too_high() {
        let mut g = Game::new();
        strip(&mut g, &[0, 0]);
        // raise tile 1 beyond hop height but within superhop height
        g.map.tiles[1].centre.vy = 400;
        g.frogger.player[0].frog_state |= FROGSTATUS_ISWANTINGD;
        g.act_frame_count = 1;
        g.update_frogger_pos(0);
        assert_eq!(g.frogmove.dest_tile[0], None, "normal hop refused");
        g.frogger.player[0].is_super_hopping = 1;
        g.frogger.player[0].frog_state |= FROGSTATUS_ISWANTINGSUPERHOPD;
        g.act_frame_count = 2;
        g.update_frogger_pos(0);
        assert_eq!(g.frogmove.dest_tile[0], Some(TileId(1)));
        assert_eq!(g.frogger.player[0].height_jumped, 400 << 12);
    }

    #[test]
    fn kill_frog_respawns_after_timer() {
        let mut g = Game::new();
        strip(&mut g, &[0, 0, 0]);
        g.set_frog_pos(0, SVector { vx: 0, vy: 0, vz: 1000, pad: 0 });
        g.frogmove.curr_tile[0] = Some(TileId(2));
        g.frogger.player[0].frog_state |= FROGSTATUS_ISDEAD;
        g.frogger.player[0].dead.init(1);
        let mut f = 1;
        while g.frogger.player[0].frog_state & FROGSTATUS_ISDEAD != 0 && f < 200 {
            g.act_frame_count = f;
            g.kill_frog(0);
            f += 1;
        }
        assert_eq!(g.frogmove.curr_tile[0], Some(TileId(0)));
        assert_eq!(g.frogger.player[0].lives, 2);
        assert_eq!(g.frogger.player[0].safe.time, 2);
    }

    /// P6-1: the infinite-lives assist keeps the life count (enhanced only).
    #[test]
    fn kill_frog_infinite_lives_assist() {
        use crate::enhance::{GameOptions, GameProfile};
        for (profile, want) in [(GameProfile::Authentic, 2), (GameProfile::Enhanced, 3)] {
            let mut g = Game::new();
            g.game_state.mode = INGAME_MODE;
            g.enhance.opts = GameOptions { profile, infinite_lives: true, ..GameOptions::AUTHENTIC };
            strip(&mut g, &[0, 0, 0]);
            g.frogmove.curr_tile[0] = Some(TileId(2));
            g.frogger.player[0].lives = 3;
            g.frogger.player[0].frog_state |= FROGSTATUS_ISDEAD;
            g.frogger.player[0].dead.init(1);
            let mut f = 1;
            while g.frogger.player[0].frog_state & FROGSTATUS_ISDEAD != 0 && f < 200 {
                g.act_frame_count = f;
                g.kill_frog(0);
                f += 1;
            }
            assert_eq!(g.frogger.player[0].lives, want, "{profile:?}");
        }
    }
}
