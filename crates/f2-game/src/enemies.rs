//! Port of `Common/enemies.c` / `enemies.h`: enemies (path followers, snappers,
//! vents, homers, flappy things, ...), their collision with the frog and the
//! reactive/death animation dispatch.
//!
//! Data tables: the original `#include`s `x:/teamspirit/pcversion/nmehack.txt`
//! (`enemyAnimHack[]`, model name -> extra animation update) and `reactive.txt`
//! (`reactiveAnims[]`). Neither file is in the preserved source; the tables are
//! [`EnemiesState::enemy_anim_hack`] / [`EnemiesState::reactive_anims`], filled by
//! [`EnemiesState::load_orig_tables`] (called from `Game::new`) from the contents recovered
//! from the retail executable ([`crate::enemy_tables`]).

use crate::prelude::*;

use f2_math::{
    add_to_vector_ff, add_to_vector_fs, add_to_vector_sf, add_to_vector_ss, add_vector_sfs, add_vector_ssf, arccos, cross_product_fff,
    distance_between_points_sf, distance_between_points_ss, dot_product_ff, fabs, fdiv, fixed_get_quaternion_from_rotation, fixed_quaternion_multiply, fmul,
    iquat_slerp, magnitude_f, magnitude_s, make_unit, new_set_vector_fs, normal_to_quaternion, normalise_quaternion, rcos, rsin, scale_vector, scale_vector_ff,
    sub_vector_fff, sub_vector_ffs, sub_vector_fss, sub_vector_sss, FVector, Fixed, IQuaternion, SVector,
};
use serde::{Deserialize, Serialize};

use crate::actor2::{orientate, orientate_fs, orientate_sf, orientate_ss, ACTOR_DRAWDISTANCEOUTER};
use crate::anim::SAMPLE_VOLUME;
use crate::babyfrog::NUM_BABIES;
use crate::define::mode::INGAME_MODE;
use crate::define::{tilestate, MULTIREMOTE, SINGLEPLAYER};
use crate::effects::Effect;
use crate::frogger::{FANIM_NONE, FROG_ANIM_SOMERSAULT, IN_VEC};
use crate::frogmove::*;
use crate::general::{
    find_closest_tile_vector, find_joined_tile_by_direction, find_joined_tile_by_direction_and_type, find_nearest_tile_s, gstrcmp, gstrlwr,
    string_change_psi_to_obe,
};
use crate::ids::{ActorId, EnemyId, TileId};
use crate::layout::worldid;
use crate::list::{HasLinks, Head, Links};
use crate::map::SCALE;
use crate::path::Path;
use crate::platform::{udiv, CATCHUP_GUARD};
use crate::specfx::*;
use crate::Game;

// ----- animation bits (`enemies.h:17`) -----
pub const NMEANIM_HOMER_WALK: i32 = 0;
pub const NMEANIM_HOMER_IDLE: i32 = 1;
pub const NMEANIM_HOMER_ATTACK: i32 = 2;
pub const NMEANIM_PATH_WALK1: i32 = 0;
pub const NMEANIM_PATH_WALK2: i32 = 1;
pub const NMEANIM_SNAP_IDLE: i32 = 0;
pub const NMEANIM_SNAP_ATTACK: i32 = 1;
pub const NMEANIM_SNAP_EXTRA1: i32 = 2;
pub const NMEANIM_SNAP_EXTRA2: i32 = 3;
pub const NUM_NME_ANIMS: i32 = 4;

// ----- enemy flags (`enemies.h:38`) -----
pub const ENEMY_NEW_NONE: u32 = 0;
pub const ENEMY_NEW_FOLLOWPATH: u32 = 1 << 0;
pub const ENEMY_NEW_FORWARDS: u32 = 1 << 1;
pub const ENEMY_NEW_BACKWARDS: u32 = 1 << 2;
pub const ENEMY_NEW_PINGPONG: u32 = 1 << 3;
pub const ENEMY_NEW_CYCLE: u32 = 1 << 4;
pub const ENEMY_NEW_MOVEUP: u32 = 1 << 5;
pub const ENEMY_NEW_MOVEDOWN: u32 = 1 << 6;
pub const ENEMY_NEW_NOJUMPOVER: u32 = 1 << 7;
pub const ENEMY_NEW_RADIUSBASEDCOLLISION: u32 = 1 << 8;
pub const ENEMY_NEW_WATCHFROG: u32 = 1 << 9;
pub const ENEMY_NEW_SNAPFROG: u32 = 1 << 10;
pub const ENEMY_NEW_RANDOMSPEED: u32 = 1 << 11;
pub const ENEMY_NEW_FACEFORWARDS: u32 = 1 << 12;
pub const ENEMY_NEW_PUSHESFROG: u32 = 1 << 13;
pub const ENEMY_NEW_HOMING: u32 = 1 << 14;
pub const ENEMY_NEW_ROTATEPATH_XZ: u32 = 1 << 15;
pub const ENEMY_NEW_ROTATEPATH_XY: u32 = 1 << 16;
pub const ENEMY_NEW_ROTATEPATH_ZY: u32 = 1 << 17;
pub const ENEMY_NEW_SNAPTILES: u32 = 1 << 18;
pub const ENEMY_NEW_MOVEONMOVE: u32 = 1 << 19;
pub const ENEMY_NEW_VENT: u32 = 1 << 20;
pub const ENEMY_NEW_NODAMAGE: u32 = 1 << 21;
pub const ENEMY_NEW_FLAPPYTHING: u32 = 1 << 22;
pub const ENEMY_NEW_ONEHITKILL: u32 = 1 << 23;
pub const ENEMY_NEW_SLERPPATH: u32 = 1 << 24;
pub const ENEMY_NEW_BABYFROG: u32 = 1 << 25;
pub const ENEMY_NEW_RANDOMMOVE: u32 = 1 << 26;
pub const ENEMY_NEW_SHADOW: u32 = 1 << 27;
pub const ENEMY_NEW_TILEHOMING: u32 = 1 << 28;
pub const ENEMY_NEW_DONOTMOVE: u32 = 1 << 29;
pub const ENEMY_NEW_NOREORIENTATE: u32 = 1 << 30;
pub const ENEMY_NEW_BATTLEMODE: u32 = 1 << 31;

/// `ENEMY_UPDATEFUNCS`.
pub const ENEMY_UPDATEFUNCS: usize = 4;

// ----- reactive anim types (`enemies.c:104`) -----
pub const ANI_FACE: i32 = 1 << 0;
pub const ANI_CENTRE: i32 = 1 << 1;
pub const ANI_FIXED: i32 = 1 << 2;
pub const ANI_ANTIFACE: i32 = 1 << 3;
pub const ANI_FACECAMERA: i32 = 1 << 4;
pub const ANI_REACTIVE: i32 = 1 << 31;

// ----- `hackFuncs[]` indices (`enemies.c:70`) -----
pub const NME_RUMBLE: i16 = 0;
pub const NME_BOB: i16 = 1;
pub const NME_GALLOP: i16 = 2;
pub const NME_ROLL_Z: i16 = 3;
pub const NME_BOUNCY: i16 = 4;
pub const NME_ROLL: i16 = 5;
pub const NME_ROLL2: i16 = 6;

/// Enemy update functions (`ENEMY::Update[]` entries).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnemyUpdate {
    SlerpPath,
    Path,
    FrogWatcher,
    Snapper,
    TileSnapper,
    Vent,
    MoveVertical,
    RotatePath,
    Homing,
    MoveOnMove,
    FlappyThing,
    RandomMove,
    TileHoming,
    Battle,
    // `hackFuncs[]`
    Jiggling,
    Bobbing,
    Galloping,
    RollZ,
    Bouncing,
    Roll,
    Roll2,
}

/// `hackFuncs[]` (`enemies.c:86`), indexed by `NMEHACK::flags`.
pub const HACK_FUNCS: [EnemyUpdate; 7] =
    [EnemyUpdate::Jiggling, EnemyUpdate::Bobbing, EnemyUpdate::Galloping, EnemyUpdate::RollZ, EnemyUpdate::Bouncing, EnemyUpdate::Roll, EnemyUpdate::Roll2];

/// `NMEHACK` (`enemies.c:89`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NmeHack {
    pub modelname: String,
    pub flags: i16,
}

/// `REACTIVEANIM` (`enemies.h:120`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReactiveAnim {
    pub name: String,
    pub anim_char: i16,
    pub anim_frog: i16,
    pub type_: i32,
}

///
/// Phase 3S: `repr(C)` with the fields `UpdateEnemies` reads for every enemy first;
/// `ser_order!` keeps the serialized field order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Enemy {
    pub links: Links<EnemyId>,
    pub nme_actor: Option<ActorId>,
    pub active: u8,
    pub visible: i8,
    pub is_waiting: i16,
    pub flags: u32,
    pub in_tile: Option<TileId>,
    pub path: Path,
    /// Phase 3S3: positions of the path's from/to nodes (a cache, not state).
    pub node_pos: crate::path::NodePosCache,
    pub update: [Option<EnemyUpdate>; ENEMY_UPDATEFUNCS],
    pub speed: Fixed,
    pub curr_normal: FVector,
    pub delta_normal: FVector,
    pub is_snapping: i16,
    pub is_idle: i16,
    pub facing: u8,
    /// `next != NULL`: the enemy is in `enemyList`.
    pub linked: bool,
    pub reactive_number: i16,
    pub uid: i32,
}

crate::ser_order!(Enemy, "Enemy", EnemyWire {
    links: Links<EnemyId>,
    linked: bool,
    uid: i32,
    nme_actor: Option<ActorId>,
    flags: u32,
    reactive_number: i16,
    speed: Fixed,
    curr_normal: FVector,
    delta_normal: FVector,
    active: u8,
    is_waiting: i16,
    is_snapping: i16,
    is_idle: i16,
    facing: u8,
    visible: i8,
    in_tile: Option<TileId>,
    path: Path,
    update: [Option<EnemyUpdate>; ENEMY_UPDATEFUNCS],
} skip { node_pos });

impl HasLinks<EnemyId> for Enemy {
    fn links(&self) -> &Links<EnemyId> {
        &self.links
    }
    fn links_mut(&mut self) -> &mut Links<EnemyId> {
        &mut self.links
    }
}

/// `ENEMYLIST`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnemyList {
    pub array: Vec<Enemy>,
    pub count: i16,
    pub blocks: i16,
    pub head: Head<EnemyId>,
}

/// Globals of `enemies.c`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnemiesState {
    /// `enemyList`.
    pub enemy_list: EnemyList,
    /// `reactiveAnims[]` (data file missing from the source, see module docs).
    pub reactive_anims: Vec<ReactiveAnim>,
    /// `enemyAnimHack[]` (data file missing from the source, see module docs).
    pub enemy_anim_hack: Vec<NmeHack>,
}

impl EnemiesState {
    /// Fills `enemyAnimHack[]` / `reactiveAnims[]` with the retail tables
    /// ([`crate::enemy_tables`]).
    pub fn load_orig_tables(&mut self) {
        use crate::enemy_tables::{ORIG_ENEMY_ANIM_HACK, ORIG_REACTIVE_ANIMS};
        self.enemy_anim_hack = ORIG_ENEMY_ANIM_HACK.iter().map(|&(m, f)| NmeHack { modelname: m.to_string(), flags: f }).collect();
        self.reactive_anims =
            ORIG_REACTIVE_ANIMS.iter().map(|&(n, c, f, t)| ReactiveAnim { name: n.to_string(), anim_char: c, anim_frog: f, type_: t }).collect();
    }
}

/// The fields of an enemy `DoEnemyCollision` reads.
struct NmeCollision {
    flags: u32,
    nme_actor: Option<ActorId>,
    in_tile: Option<TileId>,
}

/// `ENEMY_RANDOMNESS`: a value from 0.5 to 1.5.
#[inline]
fn enemy_randomness(g: &mut Game) -> Fixed {
    2048 + g.rng.random(4096)
}

/// Port of `RadiusCheck` (Common/enemies.c:2827).
pub fn radius_check(a: &SVector, b: &SVector, radius: Fixed) -> bool {
    let r = radius >> 12;
    let rsqr = r.wrapping_mul(r);
    let d = |x: i16, y: i16| {
        let v = x as i32 - y as i32;
        v.wrapping_mul(v)
    };
    d(a.vx, b.vx) < rsqr && d(a.vy, b.vy) < rsqr && d(a.vz, b.vz) < rsqr
}

/// `(unsigned long)a - (unsigned long)b` as the C path-frame expressions compute it.
#[inline]
fn fr(a: u32, b: u32) -> u32 {
    a.wrapping_sub(b)
}

impl Game {
    #[inline]
    fn nme(&self, id: EnemyId) -> &Enemy {
        &self.enemies.enemy_list.array[id.idx()]
    }

    #[inline]
    fn nme_mut(&mut self, id: EnemyId) -> &mut Enemy {
        &mut self.enemies.enemy_list.array[id.idx()]
    }

    /// `nme->nmeActor` (enemies with update functions always have one).
    #[inline]
    fn nact(&self, id: EnemyId) -> ActorId {
        self.nme(id).nme_actor.expect("enemy without actor")
    }

    #[inline]
    fn npos(&self, id: EnemyId) -> SVector {
        self.actor2.actors[self.nact(id)].actor.position
    }

    #[inline]
    fn npos_mut(&mut self, id: EnemyId) -> &mut SVector {
        let a = self.nact(id);
        &mut self.actor2.actors[a].actor.position
    }

    #[inline]
    fn nq_mut(&mut self, id: EnemyId) -> &mut IQuaternion {
        let a = self.nact(id);
        &mut self.actor2.actors[a].actor.q_rot
    }

    /// `path->nodes[i].worldTile = t` (nodes are shared between entities).
    fn set_node_tile(&mut self, path: &Path, i: i32, t: Option<TileId>) {
        self.path.touch();
        let idx = path.node_index(i);
        if let Some(n) = self.path.nodes.get_mut(idx) {
            n.world_tile = t;
        }
    }

    fn node_world_tile(&self, path: &Path, i: i32) -> Option<TileId> {
        self.path_node(path, i).world_tile
    }

    /// Port of `GetReactiveAnimNumber` (Common/enemies.c:153).
    pub fn get_reactive_anim_number(&self, name: &str) -> i16 {
        let newname = string_change_psi_to_obe(&gstrlwr(name));
        for (i, r) in self.enemies.reactive_anims.iter().enumerate() {
            if r.name.is_empty() {
                break;
            }
            if gstrcmp(&newname, &gstrlwr(&r.name)) == 0 {
                return i as i16;
            }
        }
        -1
    }

    /// Port of `GetEnemyFromUID` (Common/enemies.c:179).
    pub fn get_enemy_from_uid(&self, uid: i32) -> Option<EnemyId> {
        let l = &self.enemies.enemy_list;
        let mut c = l.head.first;
        while let Some(id) = c {
            if l.array[id.idx()].uid == uid {
                return Some(id);
            }
            c = l.array[id.idx()].links.next;
        }
        None
    }

    /// Port of `DoEnemyCollision` (Common/enemies.c:192).
    #[cfg_attr(target_vendor = "nintendo64", allow(unsafe_code), link_section = ".text.f2hot_e")]
    pub fn do_enemy_collision(&mut self, cur: EnemyId) {
        // Phase 3S3: the fields used, not a clone of the whole enemy.
        let e = {
            let n = self.nme(cur);
            NmeCollision { flags: n.flags, nme_actor: n.nme_actor, in_tile: n.in_tile }
        };
        let act = self.nact(cur);
        if e.flags & ENEMY_NEW_VENT != 0 || e.flags & ENEMY_NEW_PUSHESFROG != 0 {
            return;
        }
        let apos = self.actor2.actors[act].actor.position;
        let arad = self.actor2.actors[act].radius;

        if self.game_state.multi == SINGLEPLAYER {
            let p0 = &self.frogger.player[0];
            if e.flags & ENEMY_NEW_RADIUSBASEDCOLLISION != 0 {
                let dist = distance_between_points_ss(&self.frog_position(0), &apos);
                if p0.dead.time == 0 && dist < self.frog_actor_radius(0).wrapping_add(arad) {
                    if e.flags & ENEMY_NEW_BABYFROG != 0 {
                        if self.pickup_baby_frog(e.nme_actor, e.in_tile) != 0 {
                            self.nme_mut(cur).active = 0;
                        }
                    } else if self.frogger.player[0].safe.time == 0 {
                        self.nme_damage_frog(0, Some(cur));
                    }
                }
            } else {
                // JIM: checking for lasttile causes totally unfair collision.
                if self.frogmove.curr_tile[0] == e.in_tile
                    && p0.dead.time == 0
                    && ((p0.is_super_hopping == 0 && p0.frog_state & FROGSTATUS_ISFLOATING == 0) || e.flags & ENEMY_NEW_NOJUMPOVER != 0)
                {
                    if e.flags & ENEMY_NEW_BABYFROG != 0 {
                        if self.pickup_baby_frog(e.nme_actor, e.in_tile) != 0 {
                            self.nme_mut(cur).active = 0;
                        }
                    } else if self.frogger.player[0].safe.time == 0 {
                        self.nme_damage_frog(0, Some(cur));
                    }
                }
            }
        } else {
            self.do_enemy_collision_multi(cur, &e, apos, arad);
        }
    }

    /// The multiplayer part of [`do_enemy_collision`](Self::do_enemy_collision) (phase
    /// 3S3: out of line, outside the enemy loop's I-cache region).
    #[inline(never)]
    fn do_enemy_collision_multi(&mut self, cur: EnemyId, e: &NmeCollision, apos: SVector, arad: Fixed) {
        {
            // We only want to do collision with player 1 in a REMOTE multiplayer game
            let mut i = if self.game_state.multi == MULTIREMOTE { 0 } else { self.frogger.num_frogs - 1 };
            while i >= 0 {
                let iu = i as usize;
                let p = self.frogger.player[iu].clone();
                if e.flags & ENEMY_NEW_RADIUSBASEDCOLLISION != 0 {
                    let dist = distance_between_points_ss(&self.frog_position(iu), &apos);
                    if p.dead.time == 0 && dist < self.frog_actor_radius(iu).wrapping_add(arad) {
                        if e.flags & ENEMY_NEW_BABYFROG != 0 {
                            self.pickup_baby_frog_multi(cur, i);
                        } else if p.safe.time == 0 {
                            self.kill_mp_frog(i);
                        }
                    }
                } else if self.frogmove.curr_tile[iu] == e.in_tile
                    && p.dead.time == 0
                    && (p.frog_state & FROGSTATUS_ISSUPERHOPPING == 0 || e.flags & ENEMY_NEW_NOJUMPOVER != 0)
                    && p.frog_state & FROGSTATUS_ISFLOATING == 0
                {
                    if e.flags & ENEMY_NEW_BABYFROG != 0 {
                        self.pickup_baby_frog_multi(cur, i);
                    } else if p.safe.time == 0 {
                        self.kill_mp_frog(i);
                    }
                }
                i -= 1;
            }
        }
    }

    /// Port of `NMEDamageFrog` (Common/enemies.c:261).
    pub fn nme_damage_frog(&mut self, pl: i32, nme: Option<EnemyId>) {
        let p = pl as usize;
        let Some(nme) = nme else { return };
        if self.frogger.player[p].frog_state & FROGSTATUS_ISSAFE != 0 || self.nme(nme).flags & ENEMY_NEW_NODAMAGE != 0 {
            return;
        }
        if self.game_options().invulnerable_on() {
            return; // P6-1: invulnerability assist (enhanced only).
        }

        if self.game_state.mode == INGAME_MODE {
            if self.nme(nme).flags & ENEMY_NEW_ONEHITKILL != 0 {
                self.frogger.player[p].health_points = 0;
            } else {
                self.frogger.player[p].health_points = self.frogger.player[p].health_points.wrapping_sub(1);
            }
        }

        self.frogger.player[p].frog_state |= FROGSTATUS_ISDEAD;

        // No death animations - probably swampy or roobie.
        let ch = self.frogger.player[p].character as usize;
        if crate::frogger::ORIG_FROG_POOL.get(ch).is_some_and(|f| f.anim == FANIM_NONE) {
            self.death_multi(pl); // Explode
            return;
        }

        if self.frogger.player[p].lives == 1 {
            self.call_death_anim(DEATHBY_WHACKING, pl);
            return;
        }

        let rn = self.nme(nme).reactive_number;
        if rn != -1 {
            let ra = self.enemies.reactive_anims[rn as usize].clone();
            let frog = self.frogger.frog[p];
            let nact = self.nact(nme);
            if ra.type_ & ANI_FACE != 0 {
                if let Some(f) = frog {
                    self.actor2.actors[f].actor.q_rot = self.actor2.actors[nact].actor.q_rot;
                }
            } else if ra.type_ & ANI_ANTIFACE != 0 {
                let mut fwd = FVector::default();
                sub_vector_fss(&mut fwd, &self.actor2.actors[nact].actor.position, &self.frog_position(p));
                make_unit(&mut fwd);
                let ct = self.tile_ref(self.frogmove.curr_tile[p]).clone();
                if let Some(f) = frog {
                    orientate_fs(&mut self.actor2.actors[f].actor.q_rot, &fwd, &ct.normal);
                }
            } else if ra.type_ & ANI_FACECAMERA != 0 {
                let mut fwd = FVector::default();
                sub_vector_ffs(&mut fwd, &self.cam.curr_cam_source, &self.frog_position(p));
                make_unit(&mut fwd);
                let ct = self.tile_ref(self.frogmove.curr_tile[p]).clone();
                let dv = ct.dir_vector[find_closest_tile_vector(&ct, &fwd)];
                new_set_vector_fs(&mut fwd, &dv);
                if let Some(f) = frog {
                    orientate_fs(&mut self.actor2.actors[f].actor.q_rot, &fwd, &ct.normal);
                }
            }

            if ra.type_ & ANI_CENTRE != 0 {
                if let Some(f) = frog {
                    self.actor2.actors[f].actor.position = self.actor2.actors[nact].actor.position;
                }
            }

            if ra.type_ & ANI_FIXED != 0 {
                self.nme_mut(nme).flags |= ENEMY_NEW_DONOTMOVE;
            }

            // Matts reactive animations
            if ra.type_ & ANI_REACTIVE != 0 {
                if let Some(f) = frog {
                    self.actor_animate(f, ra.anim_frog as i32, false, false, 64, false);
                }
                self.actor_animate(nact, ra.anim_char as i32, false, false, 64, false);
                self.frogger.player[p].dead.init(3);
            } else {
                // My death animations
                self.call_death_anim(ra.anim_frog as i32, pl);
                if ra.anim_char != -1 {
                    self.actor_animate(nact, ra.anim_char as i32, false, false, 64, false);
                    self.actor_animate(nact, 0, true, true, 64, false);
                }
            }
        } else {
            self.call_death_anim(DEATHBY_NORMAL, pl);
        }
    }

    /// Port of `UpdateEnemies` (Common/enemies.c:355).
    #[cfg_attr(target_vendor = "nintendo64", allow(unsafe_code), link_section = ".text.f2hot_e")]
    pub fn update_enemies(&mut self) {
        if self.enemies.enemy_list.count == 0 {
            return;
        }
        let mut cur = self.enemies.enemy_list.head.first;
        while let Some(id) = cur {
            cur = self.nme(id).links.next;
            self.update_enemy(id);
        }
        self.update_babies();
    }

    /// Loop body of `UpdateEnemies` (`continue` = return).
    #[cfg_attr(target_vendor = "nintendo64", allow(unsafe_code), link_section = ".text.f2hot_e")]
    fn update_enemy(&mut self, id: EnemyId) {
        let Some(act2) = self.nme(id).nme_actor else { return };
        // If enemy has an actor2 and it's active and not miles away
        if self.nme(id).active == 0 || self.actor2.actors[act2].distance_from_frog >= (ACTOR_DRAWDISTANCEOUTER << 2) {
            return;
        }
        let (draw, clipped) = (self.actor2.actors[act2].draw, self.actor2.actors[act2].clipped);
        self.nme_mut(id).visible = (draw != 0 && clipped == 0) as i8;

        if clipped == 0 && self.frame_count > 10 {
            self.do_enemy_collision(id);
        }

        // P7b: a mod script that claimed this enemy moves it (no-op without mod scripts).
        if !self.modscript_update_enemy(id) && !self.update_enemy_motion(id) {
            return;
        }

        // Do Special Effects attached to enemies
        let a = &self.actor2.actors[act2];
        if a.effects != 0 && a.clipped == 0 && self.nme(id).flags & ENEMY_NEW_VENT == 0 {
            self.process_attached_effects(FxEntity::Enemy(id));
        }
    }

    /// The movement part of `UpdateEnemies`' loop body (waiting at a node, then the
    /// enemy's update functions). `false` where the original `continue`s (skipping the
    /// attached effects). Split out for P7b (`enemy_run_default`).
    #[cfg_attr(target_vendor = "nintendo64", allow(unsafe_code), link_section = ".text.f2hot_e")]
    pub(crate) fn update_enemy_motion(&mut self, id: EnemyId) -> bool {
        // check if this enemy is currently 'waiting' at a node
        if self.nme(id).is_waiting != 0 {
            if self.nme(id).is_waiting == -1 {
                return false;
            }
            if self.act_frame_count > self.nme(id).path.start_frame {
                self.nme_mut(id).is_waiting = 0;
            } else {
                let e = self.nme(id);
                let flags = e.flags;
                // Distance check so it won't bother turning nicely if you can't see it :)
                if flags & ENEMY_NEW_FACEFORWARDS == 0 && e.visible != 0 {
                    if flags & ENEMY_NEW_FOLLOWPATH != 0 || flags & ENEMY_NEW_RANDOMMOVE != 0 {
                        self.rotate_waiting_nme(id);
                    } else if flags & ENEMY_NEW_FLAPPYTHING != 0 {
                        self.slerp_waiting_flappy_thing(id);
                    }
                }
                // if enemy is a pusher and currently waiting - should still 'push' away frog
                if flags & ENEMY_NEW_PUSHESFROG != 0 && self.nme(id).in_tile == self.frogmove.curr_tile[0] {
                    let prev = self.frogmove.prev_tile;
                    let c = self.tile_ref(prev).centre;
                    if let Some(f) = self.frogger.frog[0] {
                        self.actor2.actors[f].actor.position = c;
                    }
                    self.frogmove.dest_tile[0] = prev;
                    self.frogmove.curr_tile[0] = prev;
                }
                return false;
            }
        }

        // Do update functions for individual enemy types
        for k in 0..ENEMY_UPDATEFUNCS {
            let Some(u) = self.nme(id).update[k] else { break };
            self.call_enemy_update(u, id);
        }
        true
    }

    /// Port of `UpdateAllEnemies` (Common/enemies.c:454).
    pub fn update_all_enemies(&mut self) {
        if self.enemies.enemy_list.count == 0 {
            return;
        }
        let mut cur = self.enemies.enemy_list.head.first;
        while let Some(id) = cur {
            cur = self.nme(id).links.next;
            if self.nme(id).active == 0 || self.nme(id).nme_actor.is_none() {
                continue;
            }
            if self.nme(id).is_waiting != 0 {
                if self.nme(id).is_waiting == -1 {
                    continue;
                }
                if self.act_frame_count > self.nme(id).path.start_frame {
                    self.nme_mut(id).is_waiting = 0;
                } else {
                    continue;
                }
            }
            for k in 0..ENEMY_UPDATEFUNCS {
                let Some(u) = self.nme(id).update[k] else { break };
                self.call_enemy_update(u, id);
            }
        }
        self.update_babies();
    }

    /// `(*update)(cur)`.
    #[cfg_attr(target_vendor = "nintendo64", allow(unsafe_code), link_section = ".text.f2hot_e")]
    pub fn call_enemy_update(&mut self, u: EnemyUpdate, id: EnemyId) {
        match u {
            EnemyUpdate::SlerpPath => self.update_slerp_path_nme(id),
            EnemyUpdate::Path => self.update_path_nme(id),
            EnemyUpdate::FrogWatcher => self.update_frog_watcher(id),
            EnemyUpdate::Snapper => self.update_snapper(id),
            EnemyUpdate::TileSnapper => self.update_tile_snapper(id),
            EnemyUpdate::Vent => self.update_vent(id),
            EnemyUpdate::MoveVertical => self.update_move_vertical_nme(id),
            EnemyUpdate::RotatePath => self.update_rotate_path_nme(id),
            EnemyUpdate::Homing => self.update_homing_nme(id),
            EnemyUpdate::MoveOnMove => self.update_move_on_move_nme(id),
            EnemyUpdate::FlappyThing => self.update_flappy_thing(id),
            EnemyUpdate::RandomMove => self.update_random_move_nme(id),
            EnemyUpdate::TileHoming => self.update_tile_homing_nme(id),
            EnemyUpdate::Battle => self.update_battle_enemy(id),
            EnemyUpdate::Jiggling => self.update_jiggling_enemy(id),
            EnemyUpdate::Bobbing => self.update_bobbing_enemy(id),
            EnemyUpdate::Galloping => self.update_galloping_enemy(id),
            EnemyUpdate::RollZ => self.update_roll_z_enemy(id),
            EnemyUpdate::Bouncing => self.update_bouncing_enemy(id),
            EnemyUpdate::Roll => self.update_roll_enemy(id),
            EnemyUpdate::Roll2 => self.update_roll2_enemy(id),
        }
    }

    /// Port of `RotateWaitingNME` (Common/enemies.c:494).
    #[cfg_attr(target_vendor = "nintendo64", allow(unsafe_code), link_section = ".text.f2hot_e")]
    pub fn rotate_waiting_nme(&mut self, cur: EnemyId) {
        let e = self.nme(cur).clone();
        let end_t = e.path.start_frame as i32;
        let time = (e.is_waiting as i32).wrapping_mul(self.frogger.wait_scale >> 12);
        let start_t = end_t.wrapping_sub(time);

        let num = (self.act_frame_count.wrapping_sub(start_t as u32) as i32) << 12;
        let t = 4096i32.wrapping_sub(num.checked_div(time).unwrap_or(0));

        let to_position = self.node_position(&e.path, e.path.to_node as i32);
        let from_position = self.node_position(&e.path, e.path.from_node as i32);
        let mut fwd = FVector::default();
        sub_vector_fss(&mut fwd, &to_position, &from_position);
        make_unit(&mut fwd);
        let mut q = IQuaternion::default();
        orientate(&mut q, &fwd, &e.curr_normal);

        let qr = *self.nq_mut(cur);
        let mut res = IQuaternion::default();
        iquat_slerp(&q, &qr, t, &mut res);
        *self.nq_mut(cur) = res;
    }

    /// Port of `UpdateBobbingEnemy` (Common/enemies.c:529).
    #[cfg_attr(target_vendor = "nintendo64", allow(unsafe_code), link_section = ".text.f2hot_e")]
    pub fn update_bobbing_enemy(&mut self, nme: EnemyId) {
        let e = self.nme(nme);
        let p = e.path;
        if p.end_frame <= p.start_frame {
            return;
        }
        let bob = rsin(udiv(fr(self.act_frame_count, p.start_frame) << 12, fr(p.end_frame, p.start_frame))) >> 8;
        let normal = self.tile_ref(e.in_tile).normal;
        let ofs = SVector {
            vx: ((normal.vx as i32).wrapping_mul(bob) >> 10) as i16,
            vy: ((normal.vy as i32).wrapping_mul(bob) >> 10) as i16,
            vz: ((normal.vz as i32).wrapping_mul(bob) >> 10) as i16,
            pad: 0,
        };
        add_to_vector_ss(self.npos_mut(nme), &ofs);
    }

    /// Port of `UpdateJigglingEnemy` (Common/enemies.c:559), PC amplitude.
    #[cfg_attr(target_vendor = "nintendo64", allow(unsafe_code), link_section = ".text.f2hot_e")]
    pub fn update_jiggling_enemy(&mut self, nme: EnemyId) {
        let bob = ((self.act_frame_count >> 2) & 1) as i32 * 9;
        let normal = self.tile_ref(self.nme(nme).in_tile).normal;
        let ofs = SVector {
            vx: ((normal.vx as i32 * bob) >> 12) as i16,
            vy: ((normal.vy as i32 * bob) >> 12) as i16,
            vz: ((normal.vz as i32 * bob) >> 12) as i16,
            pad: 0,
        };
        add_to_vector_ss(self.npos_mut(nme), &ofs);
    }

    /// Port of `UpdateGallopingEnemy` (Common/enemies.c:588).
    // BUG(original): `fwd` is an SVECTOR, so the FVECTOR macros applied to it truncate
    // every intermediate to 16 bits; reproduced.
    pub fn update_galloping_enemy(&mut self, nme: EnemyId) {
        let e = self.nme(nme).clone();
        let p = e.path;
        let clam = (udiv(fr(self.act_frame_count, p.start_frame) << 12, fr(p.end_frame, p.start_frame)) as u32).wrapping_mul(2) as i32;

        let to_c = self.node_tile_ref(&p, p.to_node as i32).centre;
        let from_c = self.node_tile_ref(&p, p.from_node as i32).centre;
        let tr = |v: i32| v as i16;
        // SetVectorFS(&fwd, ...)
        let mut fwd = [tr((to_c.vx as i32) << 12), tr((to_c.vy as i32) << 12), tr((to_c.vz as i32) << 12)];
        // SubFromVectorFS(&fwd, ...)
        let fc = [from_c.vx, from_c.vy, from_c.vz];
        for k in 0..3 {
            fwd[k] = tr((fwd[k] as i32).wrapping_sub((fc[k] as i32) << 12));
            // ScaleVectorFF(&fwd, 4096/500)
            fwd[k] = tr(fmul(fwd[k] as i32, 4096 / 500));
        }

        let normal = self.tile_ref(e.in_tile).normal;
        let x = rsin(clam).wrapping_mul(25).wrapping_add(25);
        let y = rcos(clam).wrapping_mul(20).wrapping_add(40);
        let n = [normal.vx as i32, normal.vy as i32, normal.vz as i32];
        let c = |k: usize| n[k].wrapping_mul(x).wrapping_sub((fwd[k] as i32).wrapping_mul(y)) >> 12;
        let ofs = FVector { vx: c(0), vy: c(1), vz: c(2) };
        add_to_vector_sf(self.npos_mut(nme), &ofs);

        let a = self.nact(nme);
        self.actor2.actors[a].actor.size.vz = 4096 + (rsin(clam.wrapping_sub(1500)) >> 3);
    }

    /// Port of `UpdateBouncingEnemy` (Common/enemies.c:621).
    pub fn update_bouncing_enemy(&mut self, nme: EnemyId) {
        let e = self.nme(nme).clone();
        let p = e.path;
        let mut t = udiv(fr(self.act_frame_count, p.start_frame) << 12, fr(p.end_frame, p.start_frame));

        // vertical bounce: y = 1-((2x-1)^2)
        let x = (t << 1).wrapping_sub(4096);
        let y = (1i32 << (12 + 9)).wrapping_sub(x.wrapping_mul(x) >> (12 - 9));

        let mut v = FVector::default();
        new_set_vector_fs(&mut v, &self.tile_ref(e.in_tile).normal);
        scale_vector_ff(&mut v, y);
        add_to_vector_sf(self.npos_mut(nme), &v);

        let a = self.nact(nme);
        self.actor2.actors[a].actor.size.vy = 4096i32.wrapping_sub(x.wrapping_mul(x) >> 14);

        t >>= 2;
        t = t.wrapping_add(((p.from_node & 1) as i32) << 10);
        self.actor2.actors[a].actor.q_rot = IQuaternion { w: rcos(t), x: rsin(t), y: 0, z: 0 };
    }

    /// Port of `UpdateRollZEnemy` (Common/enemies.c:665).
    pub fn update_roll_z_enemy(&mut self, nme: EnemyId) {
        let e = self.nme(nme).clone();
        let p = e.path;
        let mut t = udiv(fr(self.act_frame_count, p.start_frame) << 11, fr(p.end_frame, p.start_frame).wrapping_mul(12));
        let k = ((p.from_node as i32 - p.start_node as i32) % 12) * 171;
        if e.flags & ENEMY_NEW_BACKWARDS != 0 {
            t = t.wrapping_sub(k);
        } else {
            t = t.wrapping_add(k);
        }
        let q = self.nq_mut(nme);
        q.w = rcos(t);
        q.x = 0;
        q.y = 0;
        q.z = if e.flags & ENEMY_NEW_BACKWARDS != 0 { rsin(t) } else { -rsin(t) };
    }

    /// Port of `UpdateRollEnemy` (Common/enemies.c:699).
    pub fn update_roll_enemy(&mut self, nme: EnemyId) {
        self.nme_mut(nme).flags |= ENEMY_NEW_NOREORIENTATE | ENEMY_NEW_FACEFORWARDS;
        let e = self.nme(nme).clone();
        let p = e.path;
        let mut fwd = SVector::default();
        sub_vector_sss(&mut fwd, &self.node_tile_ref(&p, p.to_node as i32).centre, &self.node_tile_ref(&p, p.from_node as i32).centre);

        let a = self.nact(nme);
        // A zero divisor faults in the original.
        let t = (self.game_speed << 13).checked_div(fmul(self.actor2.actors[a].radius, e.speed)).unwrap_or(0);

        // rotate around z axis with a quaternion.. oogly
        let c = rsin(t);
        let q = IQuaternion { w: rcos(t), x: c.wrapping_mul(fwd.vz as i32) / 500, y: 0, z: c.wrapping_neg().wrapping_mul(fwd.vx as i32) / 500 };
        let qr = self.actor2.actors[a].actor.q_rot;
        fixed_quaternion_multiply(&mut self.actor2.actors[a].actor.q_rot, &q, &qr);
        normalise_quaternion(&mut self.actor2.actors[a].actor.q_rot);
    }

    /// Port of `UpdateRoll2Enemy` (Common/enemies.c:744).
    pub fn update_roll2_enemy(&mut self, nme: EnemyId) {
        self.nme_mut(nme).flags |= ENEMY_NEW_NOREORIENTATE | ENEMY_NEW_FACEFORWARDS;
        let e = self.nme(nme).clone();
        let p = e.path;
        let mut t = udiv(fr(self.act_frame_count, p.start_frame) << 12, fr(p.end_frame, p.start_frame));
        let k = ((p.from_node as i32 - p.start_node as i32) % 12) * 171;
        if e.flags & ENEMY_NEW_BACKWARDS != 0 {
            t = t.wrapping_sub(k);
        } else {
            t = t.wrapping_add(k);
        }
        t >>= 2;
        t = t.wrapping_add(((p.from_node & 1) as i32) << 10);
        *self.nq_mut(nme) = IQuaternion { w: rcos(t), x: rsin(t), y: 0, z: 0 };
    }

    /// Port of `UpdatePathNME` (Common/enemies.c:781).
    #[cfg_attr(target_vendor = "nintendo64", allow(unsafe_code), link_section = ".text.f2hot_e")]
    pub fn update_path_nme(&mut self, cur: EnemyId) {
        let afc = self.act_frame_count;

        // make sure we're in the correct section of the path
        if afc >= self.nme(cur).path.end_frame {
            self.update_path_nme_arrive(cur);
        }
        self.update_path_nme_move(cur);
    }

    /// The node-arrival part of [`update_path_nme`](Self::update_path_nme) (phase 3S3: out
    /// of line, ~once per node, outside the enemy loop's I-cache region).
    #[inline(never)]
    fn update_path_nme_arrive(&mut self, cur: EnemyId) {
        let afc = self.act_frame_count;
        let ws = self.frogger.wait_scale;
        {
            // loop through updating the path until the end frame is later than the current frame
            let mut guard = 0;
            loop {
                self.update_enemy_path_nodes(cur);
                let start = {
                    let e = self.nme_mut(cur);
                    e.path.start_frame = e.path.end_frame.wrapping_add(((e.is_waiting as i32).wrapping_mul(ws) >> 12) as u32);
                    e.path.start_frame
                };
                let end = if self.nme(cur).flags & ENEMY_NEW_RANDOMSPEED != 0 {
                    start.wrapping_add(60i32.wrapping_mul(self.rng.random(4096)) as u32)
                } else {
                    start.wrapping_add((60i32.wrapping_mul(self.nme(cur).speed) >> 12) as u32)
                };
                self.nme_mut(cur).path.end_frame = end;
                guard += 1;
                if afc < end || guard >= CATCHUP_GUARD {
                    break;
                }
            }

            if self.nme(cur).flags & ENEMY_NEW_NOREORIENTATE == 0 {
                self.calc_enemy_normal_interps(cur);
            }

            let e = self.nme(cur);
            if e.flags & ENEMY_NEW_PUSHESFROG != 0 && e.flags & ENEMY_NEW_PINGPONG != 0 {
                let path = e.path;
                let mut i = 0;
                while i <= path.to_node as i32 {
                    if let Some(t) = self.node_world_tile(&path, i) {
                        if self.map.tile(t).state == tilestate::NORMAL {
                            self.map.tile_mut(t).state = tilestate::BARRED;
                        }
                    }
                    i += 1;
                }
                while i < path.num_nodes as i32 {
                    if let Some(t) = self.node_world_tile(&path, i) {
                        if self.map.tile(t).state == tilestate::BARRED {
                            self.map.tile_mut(t).state = tilestate::NORMAL;
                        }
                    }
                    i += 1;
                }
            }
        }
    }

    /// The per-frame part of [`update_path_nme`](Self::update_path_nme).
    #[cfg_attr(target_vendor = "nintendo64", allow(unsafe_code), link_section = ".text.f2hot_e")]
    fn update_path_nme_move(&mut self, cur: EnemyId) {
        let afc = self.act_frame_count;
        // BUG(original): when waiting, `fwd` is used uninitialised by the push-block code
        // below; the port uses a zero vector.
        let mut fwd = FVector::default();
        let path = self.nme(cur).path;
        if self.nme(cur).is_waiting == 0 {
            // update the enemy position
            let mut cache = self.nme(cur).node_pos;
            let (to_position, from_position) = self.path_end_positions(&path, &mut cache);
            self.nme_mut(cur).node_pos = cache;
            sub_vector_fss(&mut fwd, &to_position, &from_position);

            let progress = fr(afc, path.start_frame).max(1);
            let length = udiv(progress << 12, fr(path.end_frame, path.start_frame));
            scale_vector_ff(&mut fwd, length);

            let e = self.nme_mut(cur);
            let dn = e.delta_normal;
            add_to_vector_ff(&mut e.curr_normal, &dn);

            // Phase 3S3: the fields used below, not a clone of the whole enemy.
            let e = self.nme(cur);
            let (flags, visible, curr_normal, facing) = (e.flags, e.visible, e.curr_normal, e.facing);
            if flags & ENEMY_NEW_DONOTMOVE == 0 {
                add_vector_sfs(self.npos_mut(cur), &fwd, &from_position);
                if visible != 0 && flags & ENEMY_NEW_NOREORIENTATE == 0 {
                    if flags & ENEMY_NEW_FACEFORWARDS == 0 {
                        let mut fwd_copy = fwd;
                        make_unit(&mut fwd_copy);
                        orientate(self.nq_mut(cur), &fwd_copy, &curr_normal);
                    } else {
                        // Need to do this so normals still work
                        let dv = self.node_tile_ref(&path, 0).dir_vector[facing as usize];
                        orientate_sf(self.nq_mut(cur), &dv, &curr_normal);
                    }
                }
            }
        }

        // ------------- Push blocks, big fat mofo blocks that push ---------------
        if self.nme(cur).flags & ENEMY_NEW_PUSHESFROG != 0 {
            self.update_path_nme_push(cur, fwd);
        }

        let path = self.nme(cur).path;
        let half = fr(path.end_frame, path.start_frame) / 2;
        let t = if afc < path.start_frame.wrapping_add(half) { path.from_node } else { path.to_node };
        let tile = self.node_world_tile(&path, t as i32);
        self.nme_mut(cur).in_tile = tile;
    }

    /// The push-block part of [`update_path_nme`](Self::update_path_nme) (out of line).
    #[inline(never)]
    fn update_path_nme_push(&mut self, cur: EnemyId, mut fwd: FVector) {
        {
            make_unit(&mut fwd);
            let mut v = FVector::default();
            sub_vector_fss(&mut v, &self.frog_position(0), &self.npos(cur));
            let dist = magnitude_f(&v);
            let dot = if self.nme(cur).flags & ENEMY_NEW_PINGPONG != 0 {
                make_unit(&mut v);
                dot_product_ff(&v, &fwd)
            } else {
                4096
            };
            let arad = self.actor2.actors[self.nact(cur)].radius;
            if dot > 3900 && dist < arad.wrapping_add(self.frog_actor_radius(0)) {
                self.push_frog(&fwd, 0);
                if let Some(f) = self.frogger.frog[0] {
                    self.actor_animate(f, FROG_ANIM_SOMERSAULT, false, false, 2458, false);
                }
            }
        }
    }

    /// Port of `UpdateSlerpPathNME` (Common/enemies.c:904).
    pub fn update_slerp_path_nme(&mut self, cur: EnemyId) {
        let path = self.nme(cur).path;
        // Find the position of the current 2 nodes
        let mut to_position = self.node_position(&path, path.to_node as i32);

        // Set direction to desired point
        let mut fwd = FVector::default();
        sub_vector_fss(&mut fwd, &to_position, &self.npos(cur));
        make_unit(&mut fwd);

        let q1 = *self.nq_mut(cur);

        // Skewer a line to rotate around, and make a rotation
        let mut axis = FVector::default();
        cross_product_fff(&mut axis, &IN_VEC, &fwd);
        let t = dot_product_ff(&IN_VEC, &fwd).clamp(-4092, 4092);
        let q3 = IQuaternion { x: axis.vx, y: axis.vy, z: axis.vz, w: arccos(t) };
        let mut q2 = IQuaternion::default();
        fixed_get_quaternion_from_rotation(&mut q2, &q3);

        // Slerp between current and desired orientation
        let from_node = self.path_node(&path, path.from_node as i32);
        let speed = fmul(from_node.offset2 / SCALE, self.game_speed).min(4092);
        iquat_slerp(&q1, &q2, speed, self.nq_mut(cur));

        // Move forwards a bit in direction of facing
        let e_speed = self.nme(cur).speed;
        if self.nme(cur).flags & ENEMY_NEW_RANDOMSPEED != 0 {
            let r = self.rng.random(100);
            scale_vector_ff(&mut fwd, fmul(fmul(e_speed, self.game_speed).wrapping_mul(SCALE), fdiv(r << 12, 100 << 12)));
        } else {
            scale_vector_ff(&mut fwd, fmul(e_speed, self.game_speed).wrapping_mul(SCALE));
        }
        add_to_vector_sf(self.npos_mut(cur), &fwd);

        if distance_between_points_ss(&self.npos(cur), &to_position) < 409600 {
            self.update_enemy_path_nodes(cur);
            let afc = self.act_frame_count;
            let ws = self.frogger.wait_scale;
            let e = self.nme_mut(cur);
            e.path.start_frame = afc.wrapping_add((e.is_waiting as i32).wrapping_mul(ws >> 12) as u32);
        }

        // Set inTile
        let path = self.nme(cur).path;
        let from_position = self.node_position(&path, path.from_node as i32);
        to_position = self.node_position(&path, path.to_node as i32);
        let t = distance_between_points_ss(&from_position, &to_position) / 2;
        let tile = if distance_between_points_ss(&from_position, &self.npos(cur)) < t { path.from_node } else { path.to_node };
        let tile = self.node_world_tile(&path, tile as i32);
        self.nme_mut(cur).in_tile = tile;
    }

    /// Port of `UpdateSnapper` (Common/enemies.c:979).
    pub fn update_snapper(&mut self, cur: EnemyId) {
        let act = self.nact(cur);
        let anim_speed = self.actor2.actors[act].anim_speed;
        let afc = self.act_frame_count;
        let path = self.nme(cur).path;
        let node0 = self.path_node(&path, 0);
        let ws = self.frogger.wait_scale;

        // Idle animations
        match self.nme(cur).is_snapping {
            0 => {
                if afc < path.end_frame {
                    return;
                }
                if self.rng.random(1000) > 980 {
                    self.actor_animate(act, 2, false, false, anim_speed, false);
                }
                if self.rng.random(1000) > 950 {
                    if self.rng.random(1000) > 950 {
                        self.actor_animate(act, 3, false, true, anim_speed, false);
                    } else {
                        self.actor_animate(act, 0, false, true, anim_speed, false);
                    }
                }

                // Slerp orientation towards frog
                let mut cn = FVector::default();
                new_set_vector_fs(&mut cn, &self.node_tile_ref(&path, 0).normal);
                self.nme_mut(cur).curr_normal = cn;
                let q1 = self.actor2.actors[act].actor.q_rot;
                let mut fwd = FVector::default();
                sub_vector_fss(&mut fwd, &self.frog_position(0), &self.actor2.actors[act].actor.position);
                make_unit(&mut fwd);
                orientate(&mut self.actor2.actors[act].actor.q_rot, &fwd, &cn);
                let q2 = self.actor2.actors[act].actor.q_rot;
                let speed = fmul(node0.speed, self.game_speed).min(4092);
                iquat_slerp(&q1, &q2, speed, &mut self.actor2.actors[act].actor.q_rot);

                // If the snapper has just spotted the frog, set snap time
                if self.actor2.actors[act].distance_from_frog < 750 << 12 {
                    let ct = self.tile_ref(self.frogmove.curr_tile[0]).centre;
                    self.actor2.actors[act].actor.rotaim = ct;

                    let q1 = self.actor2.actors[act].actor.q_rot;
                    orientate(&mut self.actor2.actors[act].actor.q_rot, &fwd, &cn);
                    let q2 = self.actor2.actors[act].actor.q_rot;
                    iquat_slerp(&q1, &q2, speed, &mut self.actor2.actors[act].actor.q_rot);

                    let e = self.nme_mut(cur);
                    e.path.start_frame = afc;
                    e.path.end_frame = afc.wrapping_add((node0.wait_time as i32).wrapping_mul(ws >> 12) as u32);
                    e.is_snapping = 2;
                }
            }
            1 => {
                // Time to snap and hope
                if afc < path.end_frame {
                    return;
                }
                let e = self.nme_mut(cur);
                e.path.start_frame = afc;
                e.path.end_frame = afc.wrapping_add((node0.wait_time as i32).wrapping_mul(ws >> 12) as u32);
                e.is_snapping = 0;

                // If frog is within hitting distance
                let p0 = &self.frogger.player[0];
                if distance_between_points_ss(&self.actor2.actors[act].actor.rotaim, &self.frog_position(0)) < (30 * SCALE) << 12
                    && p0.dead.time == 0
                    && p0.safe.time == 0
                {
                    self.nme_damage_frog(0, Some(cur));
                }
            }
            2 => {
                // Snap animation
                if fr(afc, path.start_frame) < fmul(3277, fr(path.end_frame, path.start_frame) as i32) as u32 {
                    return;
                }
                self.actor_animate(act, 1, false, false, anim_speed, false);
                self.nme_mut(cur).is_snapping = 1;
            }
            _ => {}
        }
    }

    /// Port of `UpdateTileSnapper` (Common/enemies.c:1068).
    pub fn update_tile_snapper(&mut self, cur: EnemyId) {
        let act = self.nact(cur);
        let a2 = A2Snap::of(&self.actor2.actors[act]);
        let afc = self.act_frame_count;
        let ws = self.frogger.wait_scale;
        let mut path = self.nme(cur).path;

        match self.nme(cur).is_snapping {
            -2 => {
                path.start_frame = afc;
                path.end_frame = afc.wrapping_add((6i32.wrapping_mul(a2.radius) >> 12) as u32); // 60*radius, without downscaling
                self.nme_mut(cur).path = path;
                self.nme_mut(cur).is_snapping = -1;
            }
            -1 => {
                if afc >= path.end_frame {
                    self.nme_mut(cur).is_snapping = 0;
                }
            }
            0 => {
                if afc < path.end_frame {
                    return;
                }
                if path.to_node >= path.last_node() {
                    // reached end of path nodes
                    path.from_node = path.last_node();
                    path.to_node = 1; // Don't snap at the tile we're sitting on
                } else {
                    path.from_node = path.to_node;
                    path.to_node += 1;
                }
                path.start_frame = afc;
                let wt = self.path_node(&path, path.from_node as i32).wait_time as i32;
                path.end_frame = afc.wrapping_add((wt.wrapping_mul(ws) >> 12) as u32);
                self.nme_mut(cur).path = path;
                self.nme_mut(cur).is_snapping = 2;
            }
            1 => {
                if afc < path.end_frame {
                    return;
                }
                path.start_frame = afc;
                let from = self.path_node(&path, path.from_node as i32);
                path.end_frame = afc.wrapping_add(((from.wait_time as i32).wrapping_mul(ws) >> 12) as u32);
                self.nme_mut(cur).path = path;
                self.nme_mut(cur).is_snapping = 0;

                // If the frog is on our current target tile
                for i in 0..self.frogger.num_frogs.max(0) as usize {
                    let p = &self.frogger.player[i];
                    if from.world_tile == self.frogmove.curr_tile[i] && p.dead.time == 0 && p.safe.time == 0 && p.is_super_hopping == 0 {
                        self.nme_damage_frog(i as i32, Some(cur));
                    }
                }
            }
            2 => {
                let node0 = self.path_node(&path, 0);
                if fr(afc, path.start_frame) < (fr(path.end_frame, path.start_frame).wrapping_mul(10)) >> 4 {
                    let mut cn = FVector::default();
                    new_set_vector_fs(&mut cn, &self.node_tile_ref(&path, 0).normal);
                    self.nme_mut(cur).curr_normal = cn;
                    let to_position = self.node_position(&path, path.from_node as i32);
                    let mut dir = FVector::default();
                    sub_vector_fss(&mut dir, &to_position, &a2.position);
                    make_unit(&mut dir);
                    let q1 = a2.q_rot;
                    orientate(&mut self.actor2.actors[act].actor.q_rot, &dir, &cn);
                    let q2 = self.actor2.actors[act].actor.q_rot;
                    let speed = fmul(node0.speed, self.game_speed).min(4092);
                    iquat_slerp(&q1, &q2, speed, &mut self.actor2.actors[act].actor.q_rot);
                    return;
                }

                if a2.effects & EF_LIGHTNING != 0 {
                    let to_position = self.node_position(&path, path.from_node as i32);
                    let mut dir = FVector::default();
                    sub_vector_fss(&mut dir, &to_position, &a2.position);
                    if magnitude_f(&dir) >> 12 != 0 {
                        make_unit(&mut dir);
                        let mut v = self.nme(cur).curr_normal;
                        scale_vector_ff(&mut v, node0.offset2);
                        let mut source = SVector::default();
                        add_vector_sfs(&mut source, &v, &a2.position);

                        // (sic: `if FAST ...; if SLOW ... else ...` - FAST creates two effects)
                        if a2.effects & EF_FAST != 0 {
                            self.create_special_effect(FXTYPE_LIGHTNING, &source, &dir, 20480, 163840, 1024, 819);
                        }
                        let fx = if a2.effects & EF_SLOW != 0 {
                            self.create_special_effect(FXTYPE_LIGHTNING, &source, &dir, 20480, 163840, 1024, 3277)
                        } else {
                            self.create_special_effect(FXTYPE_LIGHTNING, &source, &dir, 20480, 163840, 1024, 2048)
                        };
                        if fx.is_some() {
                            self.set_fx_tilt(fx, a2.value1); // Branching factor
                            self.set_attached_fx_colour(fx, a2.effects);
                        }
                    }
                }

                // Play sound when snapping
                let sample = self.path_node(&path, path.from_node as i32).sample;
                self.play_sample(sample, Some(a2.position), 0, SAMPLE_VOLUME, -1);

                self.actor_animate(act, NMEANIM_SNAP_ATTACK, false, false, a2.anim_speed, false);
                self.actor_animate(act, NMEANIM_SNAP_IDLE, true, true, a2.anim_speed, false);
                self.nme_mut(cur).is_snapping = 1;
            }
            _ => {}
        }
    }

    /// Port of `UpdateVent` (Common/enemies.c:1195).
    pub fn update_vent(&mut self, cur: EnemyId) {
        let act = self.nact(cur);
        let afc = self.act_frame_count;
        let mut path = self.nme(cur).path;
        let node0 = self.path_node(&path, 0);
        let a2 = A2Snap::of(&self.actor2.actors[act]);
        let apos = a2.position;

        match self.nme(cur).is_snapping {
            -2 => {
                path.start_frame = afc;
                path.end_frame = afc.wrapping_add((a2.radius.wrapping_mul(6) >> 12) as u32); // Actually radius*60, but allowing for world scaling...
                self.nme_mut(cur).path = path;
                self.nme_mut(cur).is_snapping = -1;
            }
            -1 => {
                if afc >= path.end_frame {
                    self.nme_mut(cur).is_snapping = 0;
                }
            }
            0 => {
                // Start timer: delay until fire
                path.start_frame = afc;
                path.end_frame = afc.wrapping_add((node0.wait_time as i32).wrapping_mul(60) as u32);
                self.nme_mut(cur).path = path;
                self.nme_mut(cur).is_snapping = 1;
            }
            1 => {
                if afc < path.end_frame {
                    // If halway through waiting cycle, trigger some warning effect
                    if afc > path.start_frame.wrapping_add(fr(path.end_frame, path.start_frame) >> 1) {
                        let mut up = FVector::default();
                        new_set_vector_fs(&mut up, &self.node_tile_ref(&path, 0).normal);

                        if a2.effects & EF_FIERYSMOKE != 0 {
                            // Pilot light type thing
                            if self.frame_count & 1 != 0 {
                                let fx = self.create_special_effect(FXTYPE_SMOKE_GROWS, &apos, &up, 90000, 2048, 1024, 4096);
                                self.set_fx_colour(fx, 255, 255, 255);
                            }
                        } else if a2.effects & EF_SMOKEBURST != 0 {
                            if self.frame_count & 1 != 0 {
                                let fx = self.create_special_effect(FXTYPE_SMOKE_GROWS, &apos, &up, 90000, 2048, 1024, 4096);
                                self.set_attached_fx_colour(fx, a2.effects);
                            }
                        } else if a2.effects & EF_LIGHTNING != 0 && self.nme(cur).is_idle == 0 {
                            let time = ((fr(path.end_frame, afc) << 12) / 60) as i32;
                            let time = time.wrapping_add(node0.speed);
                            let last_c = self.node_tile_ref(&path, path.num_nodes as i32 - 1).centre;
                            let mut normal = FVector::default();
                            sub_vector_fss(&mut normal, &last_c, &self.node_tile_ref(&path, 0).centre);
                            make_unit(&mut normal);
                            let fx = self.create_special_effect(FXTYPE_TWINKLE, &apos, &normal, 163840, 0, 0, time);
                            if fx.is_some() {
                                self.set_fx_tilt(fx, 8192);
                                self.set_attached_fx_colour(fx, a2.effects);
                            }
                            scale_vector(&mut normal, -1);
                            let fx = self.create_special_effect(FXTYPE_TWINKLE, &last_c, &normal, 163840, 0, 0, time);
                            if fx.is_some() {
                                self.set_fx_tilt(fx, 8192);
                                self.set_attached_fx_colour(fx, a2.effects);
                            }
                            self.nme_mut(cur).is_idle = 1;
                        }
                    }
                    return;
                }

                // Delay until stop
                path.start_frame = afc;
                path.end_frame = afc.wrapping_add((node0.speed.wrapping_mul(60) >> 12) as u32);
                self.nme_mut(cur).path = path;
                self.actor2.actors[act].fx_count = 0;

                // Play sound when firing
                self.play_sample(node0.sample, Some(apos), 0, SAMPLE_VOLUME, -1);

                let e = self.nme_mut(cur);
                e.is_idle = 0;
                e.is_snapping = 2;
            }
            2 => {
                // Stop firing on timeout, and reset
                if afc >= path.end_frame {
                    self.nme_mut(cur).is_snapping = 0;
                    return;
                }

                // A zero divisor (0 < value1 < 4096) faults in the original.
                let t: u32 = if a2.value1 != 0 { fr(path.end_frame, path.start_frame).checked_div((a2.value1 >> 12) as u32).unwrap_or(0) } else { 1 };

                if fr(afc, path.start_frame) > self.actor2.actors[act].fx_count.wrapping_mul(t) {
                    let scale = if node0.offset2 != 0 { node0.offset2 / SCALE } else { 4096 };
                    self.actor2.actors[act].fx_count += 1;
                    let mut up = FVector::default();
                    new_set_vector_fs(&mut up, &self.node_tile_ref(&path, 0).normal);
                    let nn = path.num_nodes as i32;

                    if a2.effects & EF_FIERYSMOKE != 0 {
                        self.prep_for_priority_effect();
                        self.create_special_effect(FXTYPE_FIERYSMOKE, &apos, &up, 204800, (a2.anim_speed << 3).wrapping_mul(nn), scale, 8192);
                    } else if a2.effects & EF_LASER != 0 {
                        let fx = self.create_special_effect(FXTYPE_LASER, &apos, &up, 61440, a2.anim_speed << 3, 0, 2048 * nn);
                        self.set_attached_fx_colour(fx, a2.effects);
                    } else if a2.effects & EF_SMOKEBURST != 0 {
                        self.prep_for_priority_effect();
                        let fx = self.create_special_effect(FXTYPE_SMOKEBURST, &apos, &up, 204800, (a2.anim_speed << 3).wrapping_mul(nn), scale, 6963);
                        self.set_attached_fx_colour(fx, a2.effects);
                    } else if a2.effects & EF_LIGHTNING != 0 {
                        // Make a big lighning thing between first and last nodes
                        let p1 = self.node_position(&path, 0);
                        let p2 = self.node_position(&path, nn - 1);
                        let div = if a2.value1 != 0 { a2.value1 >> 12 } else { 1 };
                        let life = (60i32.wrapping_mul(node0.speed) >> 12).checked_div(div).unwrap_or(0);
                        self.create_lightning_effect(&p1, &p2, a2.effects, life, a2.depth_shift as i32);
                    }
                }

                // Lightning covers all the path at once - smoke progresses along the path
                let nn = path.num_nodes as i32;
                let progress = if a2.effects & EF_LIGHTNING != 0 {
                    nn
                } else {
                    let f = fdiv((fr(afc, path.start_frame) << 12) as i32, (fr(path.end_frame, path.start_frame) << 12) as i32);
                    (1 + (f.wrapping_mul(nn) >> 12)).min(nn)
                };

                // Check for collision with frog, and do damage
                let flags = self.nme(cur).flags;
                for j in 0..self.frogger.num_frogs.max(0) as usize {
                    for i in 0..progress {
                        let fs = self.frogger.player[j].frog_state;
                        if fs & FROGSTATUS_ISDEAD != 0 || fs & FROGSTATUS_ISSAFE != 0 {
                            continue;
                        }
                        let node = self.path_node(&path, i);
                        // Check if frog is on path
                        if flags & ENEMY_NEW_RADIUSBASEDCOLLISION == 0 {
                            if node.world_tile != self.frogmove.curr_tile[j] {
                                continue;
                            }
                        } else {
                            // Else if radius based, do a cylindrical type collision on the vent beam
                            if !(node.world_tile == self.frogmove.curr_tile[j] || node.world_tile == self.frogmove.dest_tile[j]) {
                                continue;
                            }
                            let nt = self.tile_ref(node.world_tile).clone();
                            let mut pos = FVector::default();
                            new_set_vector_fs(&mut pos, &nt.normal);
                            scale_vector_ff(&mut pos, node.offset);
                            add_to_vector_fs(&mut pos, &nt.centre);
                            if distance_between_points_sf(&self.frog_position(j), &pos) > 1638400 {
                                continue;
                            }
                        }

                        if self.game_state.multi == SINGLEPLAYER {
                            self.nme_damage_frog(j as i32, Some(cur));
                        } else {
                            self.kill_mp_frog(j as i32);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Port of `UpdateMoveVerticalNME` (Common/enemies.c:1401).
    pub fn update_move_vertical_nme(&mut self, cur: EnemyId) {
        let afc = self.act_frame_count;
        let ws = self.frogger.wait_scale;
        // check if this platform has arrived at a path node
        let mut guard = 0;
        while afc > self.nme(cur).path.end_frame && guard < CATCHUP_GUARD {
            self.update_enemy_path_nodes(cur);
            let e = self.nme_mut(cur);
            e.path.start_frame = e.path.end_frame.wrapping_add((e.is_waiting as i32).wrapping_mul(ws >> 12) as u32);
            e.path.end_frame = e.path.start_frame.wrapping_add((e.speed.wrapping_mul(60) >> 12) as u32);
            guard += 1;
        }

        if self.nme(cur).is_waiting != 0 {
            return;
        }

        let e = self.nme(cur);
        let path = e.path;
        let node0 = self.path_node(&path, 0);
        let (start_offset, end_offset) = if e.flags & ENEMY_NEW_MOVEUP != 0 { (node0.offset, node0.offset2) } else { (node0.offset2, node0.offset) };

        // get up vector for this platform
        let t0 = self.node_tile_ref(&path, 0).clone();
        let mut move_vec = FVector::default();
        new_set_vector_fs(&mut move_vec, &t0.normal);

        let t = fdiv((fr(afc, path.start_frame) as i32) << 12, (fr(path.end_frame, path.start_frame) as i32) << 12);
        scale_vector_ff(&mut move_vec, fmul(t, end_offset).wrapping_add(fmul(4096 - t, start_offset)));
        add_vector_ssf(self.npos_mut(cur), &t0.centre, &move_vec);
    }

    /// Port of `UpdateRotatePathNME` (Common/enemies.c:1445).
    pub fn update_rotate_path_nme(&mut self, cur: EnemyId) {
        let e = self.nme(cur).clone();
        let path = e.path;
        let node0 = self.path_node(&path, 0);
        let t0 = self.node_tile_ref(&path, 0).clone();
        let afc = self.act_frame_count;

        // note - our fixed-point angles go from 0-4096, where 4096 => 360degrees => 2pi radians
        let mut length = udiv(((fr(afc, path.start_frame) as i32) << 12) as u32, fr(path.end_frame, path.start_frame));
        length = length.wrapping_add(node0.offset2); // Fraction of circumference it starts at
        if e.speed < 0 {
            length = 4096i32.wrapping_sub(length);
        }
        length &= 4095; // keep in range 0-1

        let a = self.nact(cur);
        let radius = self.actor2.actors[a].radius >> 12;
        let cosx = rcos(length);
        let sinx = rsin(length);

        let mut fwd = FVector::default();
        let c = t0.centre;
        let s16 = |v: i32| v as i16;
        let add = |base: i16, d: i16| (base as i32).wrapping_add(d as i32) as i16;
        let mut to_position = SVector::default();
        if e.flags & ENEMY_NEW_ROTATEPATH_XZ != 0 {
            to_position.vx = add(c.vx, s16(radius.wrapping_mul(sinx) >> 12));
            to_position.vz = add(c.vz, s16(radius.wrapping_mul(cosx) >> 12));
            to_position.vy = add(c.vy, s16(node0.offset >> 12));
            fwd.vx = cosx;
            fwd.vz = -sinx;
        } else if e.flags & ENEMY_NEW_ROTATEPATH_XY != 0 {
            to_position.vx = add(c.vx, s16(radius.wrapping_mul(sinx) >> 12));
            to_position.vz = c.vz;
            to_position.vy = add(c.vy, s16(radius.wrapping_mul(cosx).wrapping_add(node0.offset) >> 12));
            fwd.vx = cosx;
            fwd.vy = -sinx;
        } else {
            // ZY
            to_position.vx = c.vx;
            to_position.vz = add(c.vz, s16(radius.wrapping_mul(sinx) >> 12));
            to_position.vy = add(c.vy, s16(radius.wrapping_mul(cosx).wrapping_add(node0.offset) >> 12));
            fwd.vz = cosx;
            fwd.vy = -sinx;
        }

        if e.speed < 0 {
            fwd.vx = -fwd.vx;
            fwd.vy = -fwd.vy;
            fwd.vz = -fwd.vz;
        }

        if e.visible != 0 && e.flags & ENEMY_NEW_FACEFORWARDS == 0 {
            // Look in direction of travel
            let mut fwd_copy = fwd;
            make_unit(&mut fwd_copy);
            orientate_fs(self.nq_mut(cur), &fwd_copy, &t0.normal);
        }

        *self.npos_mut(cur) = to_position;

        if afc > path.end_frame {
            let e = self.nme_mut(cur);
            e.path.start_frame = e.path.end_frame;
            e.path.end_frame = e.path.start_frame.wrapping_add((fabs(e.speed.wrapping_mul(60)) >> 12) as u32);
        }

        let t = find_nearest_tile_s(&self.map, self.npos(cur));
        self.nme_mut(cur).in_tile = t;
    }

    /// Port of `UpdateHomingNME` (Common/enemies.c:1530).
    pub fn update_homing_nme(&mut self, cur: EnemyId) {
        if self.frogger.player[0].frog_state & FROGSTATUS_ISDEAD != 0 {
            return;
        }
        let e = self.nme(cur).clone();

        // For now, screw tiles completely and just move. Make radius based if they need to collide
        let mut fwd = FVector::default();
        sub_vector_fss(&mut fwd, &self.frog_position(0), &self.npos(cur));
        make_unit(&mut fwd);

        if e.flags & ENEMY_NEW_DONOTMOVE == 0 && e.visible != 0 && e.flags & ENEMY_NEW_FACEFORWARDS == 0 {
            let node0 = self.path_node(&e.path, 0);
            let speed = fmul(node0.offset2 / SCALE, self.game_speed).min(4092);
            let mut q1 = IQuaternion::default();
            orientate(&mut q1, &fwd, &e.curr_normal);
            let q2 = *self.nq_mut(cur);
            iquat_slerp(&q2, &q1, speed, self.nq_mut(cur));
        }

        scale_vector_ff(&mut fwd, fmul(e.speed, self.game_speed).wrapping_mul(SCALE));
        add_to_vector_sf(self.npos_mut(cur), &fwd);
    }

    /// The shared "move from `nodes[1]` to `nodes[2]`" tail of the tile movers.
    fn move_between_nodes_1_2(&mut self, cur: EnemyId, length: Fixed) {
        let e = self.nme(cur).clone();
        let path = e.path;
        let t1 = self.node_tile_ref(&path, 1).clone();
        let t2 = self.node_tile_ref(&path, 2).clone();
        let mut fwd = FVector::default();
        sub_vector_fss(&mut fwd, &t2.centre, &t1.centre);
        scale_vector_ff(&mut fwd, length);
        add_vector_sfs(self.npos_mut(cur), &fwd, &t1.centre);

        // Orientate to direction of travel
        make_unit(&mut fwd);
        if e.flags & ENEMY_NEW_FACEFORWARDS == 0 && e.visible != 0 {
            orientate_fs(self.nq_mut(cur), &fwd, &t1.normal);
        }

        // Elevate above gametile
        let mut up = FVector::default();
        new_set_vector_fs(&mut up, &self.tile_ref(e.in_tile).normal);
        scale_vector_ff(&mut up, self.path_node(&path, 0).offset);
        add_to_vector_sf(self.npos_mut(cur), &up);
    }

    /// Port of `UpdateTileHomingNME` (Common/enemies.c:1565).
    pub fn update_tile_homing_nme(&mut self, cur: EnemyId) {
        let path = self.nme(cur).path;
        if path.num_nodes < 3 {
            return;
        }
        let afc = self.act_frame_count;
        let a = self.nact(cur);

        if distance_between_points_ss(&self.npos(cur), &self.frog_position(0)) > self.actor2.actors[a].radius {
            self.nme_mut(cur).is_idle = 0;
            self.set_node_tile(&path, 2, None);
            return;
        }

        if self.nme(cur).is_snapping == 0 {
            self.set_node_tile(&path, 1, None);
            let n0 = self.node_world_tile(&path, 0);
            self.set_node_tile(&path, 2, n0);
            self.nme_mut(cur).is_snapping = 1; // This is a completely random misuse of isSnapping. And I don't care.
        }

        // If enemy is on the next path node, set startnode worldtile and the next to zero
        let p = self.nme(cur).path;
        if let Some(n2) = self.node_world_tile(&p, 2) {
            if afc >= p.end_frame {
                self.set_node_tile(&p, 1, Some(n2));
                self.set_node_tile(&p, 2, None);
            } else if afc > p.start_frame.wrapping_add(fr(p.end_frame, p.start_frame) / 2) {
                self.nme_mut(cur).in_tile = Some(n2);
            }
        }

        // If we need a new destination tile, find it by the direction to the frog
        if self.node_world_tile(&p, 2).is_none() {
            let mut frog_vec = FVector::default();
            sub_vector_fss(&mut frog_vec, &self.tile_ref(self.frogmove.curr_tile[0]).centre, &self.npos(cur));
            let n1 = self.node_world_tile(&p, 1);
            let n2 = n1.and_then(|n1| find_joined_tile_by_direction(&self.map, n1, &frog_vec));
            self.set_node_tile(&p, 2, n2);

            let speed = self.path_node(&p, 0).speed;
            let e = self.nme_mut(cur);
            e.path.start_frame = afc;
            e.path.end_frame = afc.wrapping_add((speed.wrapping_mul(60) >> 12) as u32);
        }

        // Move towards next node - third condition is so fwd is not scaled to zero
        let p = self.nme(cur).path;
        if self.node_world_tile(&p, 1).is_some() && self.node_world_tile(&p, 2).is_some() && afc > p.start_frame {
            let length = fdiv((fr(afc, p.start_frame) as i32) << 12, (fr(p.end_frame, p.start_frame) as i32) << 12);
            self.move_between_nodes_1_2(cur, length);
        }
    }

    /// Port of `UpdateMoveOnMoveNME` (Common/enemies.c:1644).
    pub fn update_move_on_move_nme(&mut self, cur: EnemyId) {
        let path = self.nme(cur).path;
        if path.num_nodes < 3 {
            return;
        }
        let afc = self.act_frame_count;
        let a = self.nact(cur);

        let dist = distance_between_points_ss(&self.npos(cur), &self.frog_position(0));
        if dist > self.actor2.actors[a].radius {
            self.nme_mut(cur).is_idle = 0;
            self.set_node_tile(&path, 2, None);
            return;
        } else if dist < 1638400 && self.frogger.player[0].dead.time == 0 && self.frogger.player[0].safe.time == 0 {
            // Trial and error value of 33
            self.nme_damage_frog(0, Some(cur));
        }

        if self.nme(cur).is_snapping == 0 {
            self.set_node_tile(&path, 1, None);
            let n0 = self.node_world_tile(&path, 0);
            self.set_node_tile(&path, 2, n0);
            self.nme_mut(cur).is_snapping = 1;
        }

        // If frog has moved
        if self.frogger.player[0].frog_state & FROGSTATUS_ISDEAD != 0 {
            self.nme_mut(cur).is_idle = 0;
        } else {
            let hj = self.frogger.player[0].has_jumped as i16;
            let e = self.nme_mut(cur);
            e.is_idle = e.is_idle.wrapping_add(hj);
        }

        // If enemy is on the next path node, set startnode worldtile and the next to zero
        let p = self.nme(cur).path;
        if let Some(n2) = self.node_world_tile(&p, 2) {
            if afc > p.end_frame {
                self.nme_mut(cur).in_tile = Some(n2);
                self.set_node_tile(&p, 1, Some(n2));
                self.set_node_tile(&p, 2, None);
            }
        }

        // If we need a new destination tile, find it by the direction to the frog
        if self.nme(cur).is_idle != 0 && self.node_world_tile(&p, 2).is_none() {
            let mut frog_vec = FVector::default();
            sub_vector_fss(&mut frog_vec, &self.frog_position(0), &self.npos(cur));
            make_unit(&mut frog_vec);
            let n1 = self.node_world_tile(&p, 1);
            let n2 = n1.and_then(|n1| find_joined_tile_by_direction_and_type(&self.map, n1, &frog_vec, tilestate::NORMAL));
            self.set_node_tile(&p, 2, n2);

            let speed = self.path_node(&p, 0).speed;
            let e = self.nme_mut(cur);
            e.path.start_frame = afc;
            e.path.end_frame = afc.wrapping_add((60i32.wrapping_mul(speed) >> 12) as u32);
            e.is_idle -= 1;
            let anim_speed = self.actor2.actors[a].anim_speed;
            self.actor_animate(a, NMEANIM_HOMER_WALK, false, false, anim_speed, false);
            self.actor_animate(a, NMEANIM_HOMER_IDLE, true, true, anim_speed, false);
        }

        // Move towards next node
        let p = self.nme(cur).path;
        if self.node_world_tile(&p, 1).is_some() && self.node_world_tile(&p, 2).is_some() {
            let length = fdiv((fr(afc, p.start_frame) as i32) << 12, (fr(p.end_frame, p.start_frame) as i32) << 12);
            self.move_between_nodes_1_2(cur, length.max(1));
        }
    }

    /// Port of `UpdateRandomMoveNME` (Common/enemies.c:1737).
    pub fn update_random_move_nme(&mut self, cur: EnemyId) {
        let path = self.nme(cur).path;
        if path.num_nodes < 3 {
            return;
        }
        let afc = self.act_frame_count;
        let ws = self.frogger.wait_scale;
        let a = self.nact(cur);

        if self.nme(cur).is_snapping == 0 {
            let n1 = self.node_world_tile(&path, 1);
            self.set_node_tile(&path, 2, n1);
            let n0 = self.node_world_tile(&path, 0);
            self.set_node_tile(&path, 1, n0);
            let e = self.nme_mut(cur);
            e.is_snapping = 1;
            e.path.from_node = 1;
            e.path.to_node = 2;
        }

        // If enemy is on the next path node, set startnode worldtile and the next to zero
        let p = self.nme(cur).path;
        if let Some(n2) = self.node_world_tile(&p, 2) {
            if afc > p.end_frame {
                self.set_node_tile(&p, 1, Some(n2));

                let mut r_vec = FVector::default();
                let r = self.rng.random(4);
                match r {
                    0 => r_vec.vx = 4096,
                    1 => r_vec.vx = -4096,
                    2 => r_vec.vz = 4096,
                    _ => r_vec.vz = -4096,
                }
                let mut n2new = find_joined_tile_by_direction(&self.map, n2, &r_vec);
                if n2new.is_none() {
                    n2new = Some(n2);
                }
                self.set_node_tile(&p, 2, n2new);

                let value1 = self.actor2.actors[a].value1;
                if self.rng.random(((value1 >> 12) + 1) as u32) == 0 {
                    let wt = self.path_node(&p, 0).wait_time;
                    let w = self.rng.random(wt as i32 as u32);
                    self.nme_mut(cur).is_waiting = w as i16;
                }

                // ENEMY_NEW_BABYFROG: only commented-out animation code here.

                let iw = self.nme(cur).is_waiting as i32;
                let r = self.rng.random((iw / 2) as u32);
                let speed = self.path_node(&p, 0).speed;
                let e = self.nme_mut(cur);
                e.path.start_frame = afc.wrapping_add((r + iw / 2).wrapping_mul(ws >> 12) as u32);
                e.path.end_frame = e.path.start_frame.wrapping_add((speed.wrapping_mul(60) >> 12) as u32);
                return;
            } else if afc > p.start_frame.wrapping_add(fr(p.end_frame, p.start_frame) / 2) {
                self.nme_mut(cur).in_tile = Some(n2);
            }
        }

        // Move towards next node - third condition is so fwd is not scaled to zero
        let p = self.nme(cur).path;
        if self.node_world_tile(&p, 1).is_some() && self.node_world_tile(&p, 2).is_some() && afc > p.start_frame {
            let length = fdiv((fr(afc, p.start_frame) as i32) << 12, (fr(p.end_frame, p.start_frame) as i32) << 12);
            self.move_between_nodes_1_2(cur, length);
        }
    }

    /// Port of `UpdateFrogWatcher` (Common/enemies.c:1825).
    pub fn update_frog_watcher(&mut self, cur: EnemyId) {
        let e = self.nme(cur).clone();
        if e.visible == 0 {
            return;
        }
        let mut to = FVector::default();
        sub_vector_fss(&mut to, &self.frog_position(0), &self.npos(cur));
        make_unit(&mut to);

        let q1 = *self.nq_mut(cur);
        orientate(self.nq_mut(cur), &to, &e.curr_normal);
        let q2 = *self.nq_mut(cur);
        let speed = fmul(self.path_node(&e.path, 0).speed, self.game_speed).min(4092);
        iquat_slerp(&q1, &q2, speed, self.nq_mut(cur));
    }

    /// Port of `UpdateFlappyThing` (Common/enemies.c:1856). (Phase 3S3: not in the enemy
    /// loop's I-cache region: only Garden 1 has flappy things, and the region with the
    /// shared maths must stay under 16 KiB.)
    pub fn update_flappy_thing(&mut self, nme: EnemyId) {
        let path = self.nme(nme).path;
        let afc = self.act_frame_count;
        let a = self.nact(nme);

        // Use default values if the user can't be arsed
        if self.path_node(&path, 0).offset == self.path_node(&path, 1).offset {
            let i0 = path.node_index(0);
            let i1 = path.node_index(1);
            if i0 < self.path.nodes.len() && i1 < self.path.nodes.len() {
                self.path.nodes[i0].offset = 0;
                self.path.nodes[i1].offset = 409600;
                self.path.nodes[i0].speed = 287;
                self.path.nodes[i1].speed = 4096;
            }
        }

        // If nme is very close to destination (stored in nmeactor->actor->rotaim) then choose a new destination.
        let pos = self.actor2.actors[a].actor.position;
        let rotaim = self.actor2.actors[a].actor.rotaim;
        if distance_between_points_ss(&pos, &rotaim) < (17 * SCALE) << 12 || magnitude_s(&rotaim) >> 12 == 0 {
            // If just got to the special node we're aiming for, wait at it
            if path.start_node != 0 {
                let wt = self.path_node(&path, path.start_node as i32).wait_time as i32;
                let e = self.nme_mut(nme);
                e.path.start_frame = afc.wrapping_add(wt.wrapping_mul(60) as u32);
                e.is_waiting = 1;
                e.path.start_node = 0;
                return;
            }

            // On a random chance, aim at the closest special node (DOESN'T INCLUDE OFFSET!)
            if path.num_nodes > 2 && self.rng.random(100) > 80 {
                let n = self.rng.random((path.num_nodes as i32 - 2) as u32) + 2;
                self.nme_mut(nme).path.start_node = n as i16;
            }

            let path = self.nme(nme).path;
            if path.start_node != 0 {
                // If we just found a node to aim at, go there
                let sn = self.path_node(&path, path.start_node as i32);
                let st = self.tile_ref(sn.world_tile).clone();
                let mut up = FVector::default();
                new_set_vector_fs(&mut up, &st.normal);
                scale_vector_ff(&mut up, sn.offset);
                add_vector_sfs(&mut self.actor2.actors[a].actor.rotaim, &up, &st.centre);
            } else {
                // Else pick a random point in space
                let n0 = self.path_node(&path, 0);
                let n1 = self.path_node(&path, 1);
                let t0 = self.tile_ref(n0.world_tile).clone();
                let t1 = self.tile_ref(n1.world_tile).clone();
                let mut up = FVector::default();
                let mut p1 = SVector::default();
                let mut p2 = SVector::default();
                new_set_vector_fs(&mut up, &t0.normal);
                scale_vector_ff(&mut up, n0.offset);
                add_vector_ssf(&mut p1, &t0.centre, &up);
                new_set_vector_fs(&mut up, &t1.normal);
                scale_vector_ff(&mut up, n1.offset);
                add_vector_ssf(&mut p2, &t1.centre, &up);

                // Get dimensions of the box we can move in.
                let dx = fabs(p2.vx as i32 - p1.vx as i32);
                let dy = fabs(p2.vy as i32 - p1.vy as i32);
                let dz = fabs(p2.vz as i32 - p1.vz as i32);
                p1.vx = p2.vx.min(p1.vx);
                p1.vy = p2.vy.min(p1.vy);
                p1.vz = p2.vz.min(p1.vz);

                // Random destination within the valid box
                let rx = self.rng.random(dx as u32);
                let ry = self.rng.random(dy as u32);
                let rz = self.rng.random(dz as u32);
                let ra = &mut self.actor2.actors[a].actor.rotaim;
                ra.vx = (p1.vx as i32).wrapping_add(rx) as i16;
                ra.vy = (p1.vy as i32).wrapping_add(ry) as i16;
                ra.vz = (p1.vz as i32).wrapping_add(rz) as i16;
            }
        }

        // Store current orientation
        let q1 = self.actor2.actors[a].actor.q_rot;

        // Set direction to desired point
        let mut fwd = FVector::default();
        sub_vector_fss(&mut fwd, &self.actor2.actors[a].actor.rotaim, &self.actor2.actors[a].actor.position);
        make_unit(&mut fwd);

        // Skewer a line to rotate around, and make a rotation
        let mut axis = FVector::default();
        cross_product_fff(&mut axis, &IN_VEC, &fwd);
        make_unit(&mut axis);
        let mut t = dot_product_ff(&IN_VEC, &fwd).clamp(-4092, 4092);
        if t < 4 && t > -4 {
            // If its trying to move backwards, make it turn
            t = 410;
        }
        let q3 = IQuaternion { x: axis.vx, y: axis.vy, z: axis.vz, w: arccos(t) };
        let mut q2 = IQuaternion::default();
        fixed_get_quaternion_from_rotation(&mut q2, &q3);

        // Slerp between current and desired orientation
        let path = self.nme(nme).path;
        let speed = fmul(self.path_node(&path, 0).speed, self.game_speed).min(4092);
        iquat_slerp(&q1, &q2, speed, &mut self.actor2.actors[a].actor.q_rot);

        // Move forwards a bit in direction of facing
        scale_vector_ff(&mut fwd, fmul(self.path_node(&path, 1).speed, self.game_speed).wrapping_mul(SCALE));
        let p = self.actor2.actors[a].actor.position;
        add_vector_sfs(&mut self.actor2.actors[a].actor.position, &fwd, &p);

        self.nme_mut(nme).flags |= ENEMY_NEW_NODAMAGE;
    }

    /// Port of `UpdateBattleEnemy` (Common/enemies.c:1979).
    pub fn update_battle_enemy(&mut self, cur: EnemyId) {
        let path = self.nme(cur).path;
        if path.num_nodes < 3 {
            return;
        }
        let afc = self.act_frame_count;

        if self.nme(cur).is_snapping == 0 {
            let n1 = self.node_world_tile(&path, 1);
            self.set_node_tile(&path, 2, n1);
            let n0 = self.node_world_tile(&path, 0);
            self.set_node_tile(&path, 1, n0);
            let e = self.nme_mut(cur);
            e.is_snapping = 1;
            e.path.from_node = 1;
            e.path.to_node = 2;
        }

        if self.nme(cur).in_tile.is_none() {
            self.nme_mut(cur).in_tile = self.node_world_tile(&path, 0);
        }

        // If enemy is on the next path node, set startnode worldtile and the next to zero
        let p = self.nme(cur).path;
        if let Some(n2) = self.node_world_tile(&p, 2) {
            if afc > p.end_frame {
                let mut fwd = FVector::default();
                sub_vector_fss(&mut fwd, &self.map.tile(n2).centre, &self.node_tile_ref(&p, 1).centre);
                self.set_node_tile(&p, 1, Some(n2));

                self.map.tile_mut(n2).state = tilestate::NORMAL;
                let n2new = find_joined_tile_by_direction_and_type(&self.map, n2, &fwd, tilestate::NORMAL).unwrap_or(n2);
                self.set_node_tile(&p, 2, Some(n2new));
                self.map.tile_mut(n2new).state = tilestate::OCCUPIED;

                let speed = self.path_node(&p, 0).speed;
                let e = self.nme_mut(cur);
                e.path.start_frame = afc;
                e.path.end_frame = afc.wrapping_add((speed.wrapping_mul(60) >> 12) as u32);
                return;
            } else if afc > p.start_frame.wrapping_add(fr(p.end_frame, p.start_frame) / 2) {
                self.nme_mut(cur).in_tile = Some(n2);
            }
        }

        // Move towards next node
        let p = self.nme(cur).path;
        if self.node_world_tile(&p, 1).is_some() && self.node_world_tile(&p, 2).is_some() {
            let progress = fr(afc, p.start_frame).max(1);
            let length = udiv(progress << 12, fr(p.end_frame, p.start_frame));
            self.move_between_nodes_1_2(cur, length);
        }
    }

    /// Port of `SlerpWaitingFlappyThing` (Common/enemies.c:2064).
    pub fn slerp_waiting_flappy_thing(&mut self, cur: EnemyId) {
        let path = self.nme(cur).path;
        let q1 = *self.nq_mut(cur);
        let n = self.node_tile_ref(&path, path.start_node as i32).normal;
        let q3 = IQuaternion { x: n.vx as i32, y: n.vy as i32, z: n.vz as i32, w: 0 };
        let mut q2 = IQuaternion::default();
        fixed_get_quaternion_from_rotation(&mut q2, &q3);
        let speed = fmul(self.path_node(&path, 0).speed, self.game_speed).min(4092);
        iquat_slerp(&q1, &q2, speed, self.nq_mut(cur));
    }

    /// Port of `AddEnemyModelUpdates` (Common/enemies.c:2080): byte-for-byte name
    /// match up to the extension against `enemyAnimHack[]`.
    pub fn add_enemy_model_updates(&mut self, name: &str, nme: EnemyId) {
        let d_bytes = name.as_bytes();
        let mut set = None;
        for hack in &self.enemies.enemy_anim_hack {
            let c_bytes = hack.modelname.as_bytes();
            let at = |s: &[u8], i: usize| s.get(i).copied().unwrap_or(0);
            let mut i = 0;
            loop {
                let (c, d) = (at(c_bytes, i), at(d_bytes, i));
                if d == b'.' && (c == 0 || c == b'.') {
                    set = HACK_FUNCS.get(hack.flags as usize).copied();
                } else if c != d.to_ascii_lowercase() || c == 0 {
                    break;
                }
                i += 1;
            }
        }
        if let Some(f) = set {
            self.nme_mut(nme).update[1] = Some(f);
        }
    }

    /// Port of `CreateAndAddEnemy` (Common/enemies.c:2102).
    pub fn create_and_add_enemy(&mut self, actor_name: &str, flags: i32, id: i32, path: Path, anim_speed: Fixed, facing: u8) -> Option<EnemyId> {
        let mut init_flags = crate::mdx_actor::INIT_ANIMATION;
        let mut shadow_radius = 0;
        if flags as u32 & ENEMY_NEW_SHADOW != 0 {
            init_flags |= crate::mdx_actor::INIT_SHADOW;
            shadow_radius = 819200;
        }

        // create and add the nme actor
        let actor = self.create_and_add_actor(actor_name, 0, 0, 0, init_flags)?;

        let l = &self.enemies.enemy_list;
        if l.count >= l.blocks || l.count as usize >= l.array.len() {
            return None;
        }
        let nid = EnemyId(l.count as u32);

        let reactive = self.get_reactive_anim_number(actor_name);
        {
            let e = self.nme_mut(nid);
            e.nme_actor = Some(actor);
            e.flags = flags as u32;
            e.facing = facing;
            e.reactive_number = reactive;
        }

        // set shadow radius (if applicable)
        if shadow_radius != 0 {
            if let Some(s) = self.actor2.actors[actor].actor.shadow.as_mut() {
                s.radius = shadow_radius;
            }
        }

        self.actor_animate(actor, 0, true, false, anim_speed, false);
        self.actor2.actors[actor].anim_speed = anim_speed;

        {
            let e = self.nme_mut(nid);
            e.active = 1;
            e.path = Path::default();
            e.in_tile = None;
            e.speed = 4096;
            e.uid = id;
            e.is_snapping = 0;
        }

        self.assign_path_to_enemy(nid, path, 0);

        let flags = self.nme(nid).flags;
        let p = self.nme(nid).path;
        let t0 = self.node_tile_ref(&p, 0).clone();
        let fac = self.nme(nid).facing as usize;
        let orient = |g: &mut Game| orientate_ss(&mut g.actor2.actors[actor].actor.q_rot, &t0.dir_vector[fac], &t0.normal);
        let set = |g: &mut Game, u: EnemyUpdate| g.nme_mut(nid).update[0] = Some(u);

        if flags & ENEMY_NEW_SLERPPATH != 0 {
            set(self, EnemyUpdate::SlerpPath);
            self.actor_animate(actor, NMEANIM_PATH_WALK1, true, false, anim_speed, false);
            orient(self);
        } else if flags & ENEMY_NEW_FOLLOWPATH != 0 {
            set(self, EnemyUpdate::Path);
            self.actor_animate(actor, NMEANIM_PATH_WALK1, true, false, anim_speed, false);
            orient(self);
        } else if flags & ENEMY_NEW_WATCHFROG != 0 {
            set(self, EnemyUpdate::FrogWatcher);
        } else if flags & ENEMY_NEW_SNAPFROG != 0 {
            set(self, EnemyUpdate::Snapper);
            self.actor_animate(actor, NMEANIM_SNAP_IDLE, true, false, anim_speed, false);
        } else if flags & ENEMY_NEW_SNAPTILES != 0 {
            self.nme_mut(nid).is_snapping = -2;
            set(self, EnemyUpdate::TileSnapper);
            self.actor_animate(actor, NMEANIM_SNAP_IDLE, true, false, anim_speed, false);
        } else if flags & ENEMY_NEW_VENT != 0 {
            self.nme_mut(nid).is_snapping = -2;
            set(self, EnemyUpdate::Vent);
            orient(self);
        } else if flags & ENEMY_NEW_MOVEUP != 0 || flags & ENEMY_NEW_MOVEDOWN != 0 {
            set(self, EnemyUpdate::MoveVertical);
            orient(self);
        } else if flags & (ENEMY_NEW_ROTATEPATH_XZ | ENEMY_NEW_ROTATEPATH_XY | ENEMY_NEW_ROTATEPATH_ZY) != 0 {
            set(self, EnemyUpdate::RotatePath);
            self.actor_animate(actor, NMEANIM_PATH_WALK1, true, false, anim_speed, false);
        } else if flags & ENEMY_NEW_HOMING != 0 {
            set(self, EnemyUpdate::Homing);
            let mut cn = FVector::default();
            new_set_vector_fs(&mut cn, &t0.normal);
            self.nme_mut(nid).curr_normal = cn;
            self.actor_animate(actor, NMEANIM_HOMER_IDLE, true, false, anim_speed, false);
        } else if flags & ENEMY_NEW_MOVEONMOVE != 0 {
            set(self, EnemyUpdate::MoveOnMove);
            self.actor_animate(actor, NMEANIM_HOMER_IDLE, true, false, anim_speed, false);
        } else if flags & ENEMY_NEW_FLAPPYTHING != 0 {
            set(self, EnemyUpdate::FlappyThing);
        } else if flags & ENEMY_NEW_RANDOMMOVE != 0 {
            set(self, EnemyUpdate::RandomMove);
        } else if flags & ENEMY_NEW_TILEHOMING != 0 {
            set(self, EnemyUpdate::TileHoming);
            self.actor_animate(actor, NMEANIM_HOMER_IDLE, true, false, anim_speed, false);
        } else if flags & ENEMY_NEW_BATTLEMODE != 0 {
            set(self, EnemyUpdate::Battle);
            self.nme_mut(nid).is_waiting = -1;
        } else {
            // No update function - probably single flag nme
            orient(self);
        }

        if flags & ENEMY_NEW_BABYFROG != 0 {
            let nb = (self.map.num_babies.max(0) as usize).min(NUM_BABIES);
            let mut i = 0;
            while i < nb {
                if self.babyfrog.baby_list[i].baby.is_none() {
                    let off2 = self.path_node(&p, 0).offset2;
                    let mut n = if off2 != 0 { ((off2 >> 12) / SCALE) - 1 } else { i as i32 };
                    if n > self.map.num_babies {
                        n = 0;
                    }
                    // PC: overrideTex = GetTexEntryFromCRC(UpdateCRC("bfg0<n+1>.bmp"))
                    self.actor2.actors[actor].override_tex = Some(format!("bfg0{}.bmp", (n as u32).wrapping_add(1)));
                    self.babyfrog.baby_list[i].baby = Some(actor);
                    self.babyfrog.baby_list[i].enemy = Some(nid);
                    break;
                }
                i += 1;
            }

            self.actor2.actors[actor].effects |= EF_RIPPLE_RINGS | EF_SLOW;
            // BUG(original): with no free baby slot, i == numBabies indexes past the
            // used entries (babyList has NUM_BABIES slots); the port clamps to the array.
            if i < NUM_BABIES {
                let c = self.babyfrog.baby_list[i].fx_colour;
                if c[0] != 0 {
                    self.actor2.actors[actor].effects |= EF_TINTRED;
                }
                if c[1] != 0 {
                    self.actor2.actors[actor].effects |= EF_TINTGREEN;
                }
                if c[2] != 0 {
                    self.actor2.actors[actor].effects |= EF_TINTBLUE;
                }
                if let Some(o) = self.overlays.get_mut(self.babyfrog.baby_icons[i]) {
                    o.r = c[0];
                    o.g = c[1];
                    o.b = c[2];
                }
                // PC `!rHardware` halving dropped (hardware renderer modelled).
            }
        }

        if self.frogger.player[0].world_num as u32 == worldid::ANCIENT {
            let b = actor_name.as_bytes();
            if b.len() >= 8 && b[..8].eq_ignore_ascii_case(b"STNWHEEL") {
                self.nme_mut(nid).flags |= ENEMY_NEW_NOJUMPOVER;
            }
        }

        self.add_enemy_model_updates(actor_name, nid);
        self.add_enemy(nid);
        Some(nid)
    }

    /// Port of `AssignPathToEnemy` (Common/enemies.c:2298). `PATH_MAKENODETILEPTRS` is
    /// not supported (node tiles are already resolved in the port).
    pub fn assign_path_to_enemy(&mut self, nme: EnemyId, path: Path, _path_flags: u32) {
        let mut path = path;
        // set the start position for the enemy
        if path.start_node < path.num_nodes {
            path.from_node = path.start_node;
        } else {
            path.from_node = 0;
            path.start_node = 0;
        }

        let mut flags = self.nme(nme).flags;
        if flags & ENEMY_NEW_FORWARDS != 0 {
            // this enemy moves forward thru path nodes
            flags |= ENEMY_NEW_FOLLOWPATH;
            path.to_node = path.from_node.wrapping_add(1);
            if path.to_node > path.last_node() {
                path.to_node = 0;
            }
            if flags & ENEMY_NEW_FACEFORWARDS != 0 && check_path_orientation_safe(self, &path) != 0 {
                flags |= ENEMY_NEW_NOREORIENTATE;
            }
        } else if flags & ENEMY_NEW_BACKWARDS != 0 {
            // this enemy moves backward thru path nodes
            flags |= ENEMY_NEW_FOLLOWPATH;
            path.to_node = path.from_node.wrapping_sub(1);
            if path.to_node < 0 {
                path.to_node = path.last_node();
            }
            if flags & ENEMY_NEW_FACEFORWARDS != 0 && check_path_orientation_safe(self, &path) != 0 {
                flags |= ENEMY_NEW_NOREORIENTATE;
            }
        } else if flags & ENEMY_NEW_MOVEUP != 0 || flags & ENEMY_NEW_MOVEDOWN != 0 {
            // this enemy moves up or down
            path.from_node = 0;
            path.to_node = 0;
        }
        self.nme_mut(nme).flags = flags;
        self.nme_mut(nme).path = path;

        // set enemy position to relevant point on path
        let start = self.node_position(&path, path.from_node as i32);
        *self.npos_mut(nme) = start;
        let from = self.path_node(&path, path.from_node as i32);
        let n = self.tile_ref(from.world_tile).normal;
        normal_to_quaternion(self.nq_mut(nme), &n);

        // set enemy current 'in' tile and speeds and pause times
        let afc = self.act_frame_count;
        let e = self.nme_mut(nme);
        e.in_tile = from.world_tile;
        e.speed = from.speed;
        e.is_waiting = from.wait_time;
        e.path.start_frame = afc;
        e.path.end_frame = afc.wrapping_add((fabs(e.speed).wrapping_mul(60) >> 12) as u32);

        self.calc_enemy_normal_interps(nme);
    }

    /// Port of `UpdateEnemyPathNodes` (Common/enemies.c:2384).
    pub fn update_enemy_path_nodes(&mut self, nme: EnemyId) {
        let flags = self.nme(nme).flags;
        let mut path = self.nme(nme).path;
        let mut new_flags = flags;
        let mut sample = 0;
        let mut reset_anim = false;

        if flags & ENEMY_NEW_FORWARDS != 0 {
            // enemy moves forward through path nodes
            if path.to_node >= path.last_node() {
                // reached end of path nodes
                if flags & ENEMY_NEW_PINGPONG != 0 {
                    new_flags ^= ENEMY_NEW_FORWARDS | ENEMY_NEW_BACKWARDS; // reverse enemy path movement
                    path.from_node = path.last_node();
                    path.to_node = path.last_node() - 1;
                } else if flags & ENEMY_NEW_CYCLE != 0 {
                    path.from_node = path.last_node();
                    path.to_node = 0;
                } else {
                    path.from_node = 0;
                    path.to_node = 1;
                    reset_anim = true;
                }
            } else {
                path.from_node = path.to_node;
                path.to_node += 1;
            }
            sample = self.path_node(&path, path.from_node as i32).sample;
        } else if flags & ENEMY_NEW_BACKWARDS != 0 {
            // enemy moves backwards through path nodes
            if path.to_node <= 0 {
                if flags & ENEMY_NEW_PINGPONG != 0 {
                    new_flags ^= ENEMY_NEW_FORWARDS | ENEMY_NEW_BACKWARDS;
                    path.from_node = 0;
                    path.to_node = 1;
                } else if flags & ENEMY_NEW_CYCLE != 0 {
                    path.from_node = 0;
                    path.to_node = path.last_node();
                } else {
                    path.from_node = path.last_node();
                    path.to_node = path.last_node() - 1;
                }
            } else {
                path.from_node = path.to_node;
                path.to_node -= 1;
            }
            sample = self.path_node(&path, path.from_node as i32).sample;
        } else if flags & (ENEMY_NEW_MOVEDOWN | ENEMY_NEW_MOVEUP) != 0 && flags & ENEMY_NEW_PINGPONG != 0 {
            new_flags ^= ENEMY_NEW_MOVEUP | ENEMY_NEW_MOVEDOWN;
            sample = self.path_node(&path, 0).sample;
        }
        self.nme_mut(nme).flags = new_flags;
        self.nme_mut(nme).path = path;
        if reset_anim {
            let a = self.nact(nme);
            self.actor_reset_animation(a);
        }
        if sample != 0 {
            let pos = self.npos(nme);
            self.play_sample(sample, Some(pos), 0, SAMPLE_VOLUME, -1);
        }

        let from = self.path_node(&path, path.from_node as i32);
        {
            let e = self.nme_mut(nme);
            e.speed = from.speed;
            e.is_waiting = from.wait_time;
        }

        // Stop overshoot when waiting on a path node
        if self.nme(nme).is_waiting != 0 {
            let pos = if self.nme(nme).flags & ENEMY_NEW_MOVEDOWN != 0 {
                self.node_position_offset2(&path, path.from_node as i32)
            } else {
                self.node_position(&path, path.from_node as i32)
            };
            *self.npos_mut(nme) = pos;
        }

        if self.nme(nme).flags & ENEMY_NEW_RANDOMSPEED != 0 {
            let r = enemy_randomness(self);
            let e = self.nme_mut(nme);
            e.speed = fmul(e.speed, r);
        }
    }

    /// Port of `CalcEnemyNormalInterps` (Common/enemies.c:2498).
    pub fn calc_enemy_normal_interps(&mut self, nme: EnemyId) {
        let path = self.nme(nme).path;
        if path.num_nodes < 2 {
            return;
        }
        let from_t = self.node_tile_ref(&path, path.from_node as i32).clone();
        let to_t = self.node_tile_ref(&path, path.to_node as i32).clone();

        // set the current enemy normal to that of the 'from' node and get the dest normal
        let mut curr = FVector::default();
        let mut dest = FVector::default();
        new_set_vector_fs(&mut curr, &from_t.normal);
        new_set_vector_fs(&mut dest, &to_t.normal);

        // calculate deltas for linear interpolation of enemy normal during movement
        let mut delta = FVector::default();
        sub_vector_fff(&mut delta, &dest, &curr);

        // determine number of 'steps' over which to interpolate
        let from_pos = self.node_position(&path, path.from_node as i32);
        let to_pos = self.node_position(&path, path.to_node as i32);
        let mut num_steps = distance_between_points_ss(&from_pos, &to_pos) >> 12;
        num_steps = (num_steps.wrapping_mul(self.nme(nme).speed) >> 12).max(1);

        delta.vx /= num_steps;
        delta.vy /= num_steps;
        delta.vz /= num_steps;
        let e = self.nme_mut(nme);
        e.curr_normal = curr;
        e.delta_normal = delta;
    }

    /// Port of `InitEnemyLinkedList` (Common/enemies.c:2541). The `deathAnims[]` setup
    /// is the constant [`crate::anim::DEATH_ANIMS`].
    pub fn init_enemy_linked_list(&mut self) {
        let l = &mut self.enemies.enemy_list;
        l.array = Vec::new();
        l.count = 0;
        l.head = Head::default();
    }

    /// Port of `AddEnemy` (Common/enemies.c:2574).
    // BUG(original): the "same type" search compares `cur->Update == enemy->Update`,
    // i.e. the addresses of two different `Update[]` arrays, which never match: every
    // enemy is added at the head of the list.
    pub fn add_enemy(&mut self, enemy: EnemyId) {
        if self.nme(enemy).linked {
            return;
        }
        let l = &mut self.enemies.enemy_list;
        l.head.push_front(&mut l.array, enemy);
        l.array[enemy.idx()].linked = true;
        l.count += 1;
    }

    /// Port of `SubEnemy` (Common/enemies.c:2612).
    pub fn sub_enemy(&mut self, enemy: EnemyId) {
        if !self.nme(enemy).linked {
            return;
        }
        if let Some(a) = self.nme(enemy).nme_actor {
            self.actor2_free(a);
        }
        let l = &mut self.enemies.enemy_list;
        l.head.remove(&mut l.array, enemy);
        l.array[enemy.idx()].linked = false;
        l.array[enemy.idx()].links.next = None;
        l.count -= 1;
    }

    /// Port of `FreeEnemyLinkedList` (Common/enemies.c:2640).
    pub fn free_enemy_linked_list(&mut self) {
        while let Some(first) = self.enemies.enemy_list.head.first {
            self.sub_enemy(first);
        }
        self.free_nme_block();
        self.init_enemy_linked_list();
    }

    /// Port of `AllocNmeBlock` (Common/enemies.c:2663).
    pub fn alloc_nme_block(&mut self, num: i32) {
        let l = &mut self.enemies.enemy_list;
        if l.array.is_empty() {
            l.array = vec![Enemy::default(); num.max(0) as usize];
        }
        l.count = 0;
        l.blocks = num as i16;
    }

    /// Port of `FreeNmeBlock` (Common/enemies.c:2679).
    pub fn free_nme_block(&mut self) {
        let l = &mut self.enemies.enemy_list;
        l.array = Vec::new();
        l.count = 0;
        l.blocks = 0;
    }

    /// Port of `EnumEnemies` (Common/enemies.c:2698).
    pub fn enum_enemies(&mut self, id: i32, mut func: impl FnMut(&mut Game, EnemyId, i32) -> i32, param: i32) -> i32 {
        let mut count = 0;
        let mut cur = self.enemies.enemy_list.head.first;
        while let Some(c) = cur {
            if (id == 0 || self.nme(c).uid == id) && func(self, c, param) == 0 {
                break;
            }
            cur = self.nme(c).links.next;
            count += 1;
        }
        if count == 0 {
            self.effects.push(Effect::Log { text: "EnumEnemies found 0 enemies?".into() });
        }
        count
    }

    /// Port of `SetEnemyVisible` (Common/enemies.c:2717).
    pub fn set_enemy_visible(&mut self, nme: EnemyId, visible: bool) {
        let a = self.nact(nme);
        if visible {
            self.nme_mut(nme).active = 1;
            self.actor2.actors[a].draw = 1;
            if self.nme(nme).is_waiting != -1 {
                self.nme_mut(nme).is_waiting = -1;
                self.set_enemy_moving(nme, true);
            }
        } else {
            self.actor2.actors[a].draw = 0;
            self.nme_mut(nme).active = 0;
        }
    }

    /// Port of `SetEnemyMoving` (Common/enemies.c:2737).
    pub fn set_enemy_moving(&mut self, nme: EnemyId, moving: bool) {
        if moving {
            if self.nme(nme).is_waiting != 0 {
                let afc = self.act_frame_count;
                let e = self.nme_mut(nme);
                e.is_waiting = 0;
                e.path.start_frame = afc;
                e.path.end_frame = afc.wrapping_add((60i32.wrapping_mul(e.speed) >> 12) as u32);
            }
        } else {
            self.nme_mut(nme).is_waiting = -1;
            // JIM: This stops brain monkeys flicking when restarted. Nasty I know, but what the hell.
            let e = self.nme(nme);
            if e.flags & ENEMY_NEW_MOVEONMOVE != 0 || e.flags & ENEMY_NEW_BATTLEMODE != 0 {
                let p = e.path;
                let in_tile = e.in_tile;
                if let Some(n2) = self.node_world_tile(&p, 2) {
                    self.set_node_tile(&p, 1, Some(n2));
                    self.set_node_tile(&p, 2, None);
                } else {
                    self.set_node_tile(&p, 1, in_tile);
                    self.set_node_tile(&p, 2, None);
                }
            }
        }
    }

    /// Port of `MoveEnemyToNode` (Common/enemies.c:2777). Returns 1.
    pub fn move_enemy_to_node(&mut self, nme: EnemyId, node: i32) -> i32 {
        let e = self.nme(nme).clone();
        // Gratuitous hack to make move-on-move enemies work
        if e.flags & ENEMY_NEW_MOVEONMOVE != 0 || e.flags & ENEMY_NEW_BATTLEMODE != 0 {
            if let Some(ph) = self.get_enemy_from_uid(node) {
                let php = self.nme(ph).path;
                let p = e.path;
                let n0 = self.node_world_tile(&php, 0);
                self.set_node_tile(&p, 0, n0);
                self.nme_mut(nme).in_tile = n0;

                if e.flags & ENEMY_NEW_BATTLEMODE != 0 {
                    if let Some(n2) = self.node_world_tile(&p, 2) {
                        if self.map.tile(n2).state == tilestate::OCCUPIED {
                            self.map.tile_mut(n2).state = tilestate::NORMAL;
                        }
                    }
                    let a1 = self.node_world_tile(&php, 1);
                    let a2 = self.node_world_tile(&php, 2);
                    self.set_node_tile(&p, 1, a1);
                    self.set_node_tile(&p, 2, a2);
                }

                let pos = self.node_position(&p, 0);
                *self.npos_mut(nme) = pos;
                self.nme_mut(nme).is_snapping = 0;
            } else {
                self.effects.push(Effect::Log { text: format!("MoveEnemyToNode(): Could not find placeholder {node}") });
            }
        } else if node >= 0 && node < e.path.num_nodes as i32 {
            let t = self.node_world_tile(&e.path, node);
            let pos = self.node_position(&e.path, node);
            let afc = self.act_frame_count;
            {
                let em = self.nme_mut(nme);
                em.path.to_node = node as i16;
                em.in_tile = t;
                em.path.end_frame = afc;
            }
            *self.npos_mut(nme) = pos;
            for k in 0..ENEMY_UPDATEFUNCS {
                let Some(u) = self.nme(nme).update[k] else { break };
                self.call_enemy_update(u, nme);
            }
        } else {
            self.effects.push(Effect::Log { text: format!("MoveEnemyToNode(): Flag ({node}) out of range") });
        }
        1
    }

    /// Port of `EnemyReachedTopOrBottomPoint` (Common/enemies.c:2844).
    pub fn enemy_reached_top_or_bottom_point(&self, nme: EnemyId) -> bool {
        let e = self.nme(nme);
        let lim = (10i32.wrapping_mul(self.game_speed).wrapping_mul(SCALE)) << 12;
        if e.flags & ENEMY_NEW_MOVEUP != 0 {
            let to_pos = self.node_position_offset2(&e.path, 0);
            if distance_between_points_ss(&self.npos(nme), &to_pos) < lim {
                return true;
            }
        } else if e.flags & ENEMY_NEW_MOVEDOWN != 0 {
            let to_pos = self.node_position(&e.path, 0);
            if distance_between_points_ss(&self.npos(nme), &to_pos) < lim {
                return true;
            }
        }
        false
    }

    /// Port of `FindEnemyAtNode` (Common/enemies.c:2879). Node identity is the node's
    /// slot in the shared node block (the original compares node addresses).
    pub fn find_enemy_at_node(&self, nme: Option<EnemyId>, n: i32) -> Option<EnemyId> {
        let p = self.nme(nme?).path;
        let mut nn = n + 1;
        if nn >= p.num_nodes as i32 {
            nn = 0;
        }
        let mut pn = n - 1;
        if pn < 0 {
            pn = p.num_nodes as i32 - 1;
        }
        let l = &self.enemies.enemy_list;
        let mut c = l.head.first;
        while let Some(e) = c {
            let en = &l.array[e.idx()];
            let node = en.path.node_index(en.path.to_node as i32);
            if Some(e) != nme && en.active != 0 && (node == p.node_index(nn) || node == p.node_index(pn) || node == p.node_index(n)) {
                return Some(e);
            }
            c = en.links.next;
        }
        None
    }
}

/// `CheckPathOrientation(path)` guarding against unresolved node tiles.
pub(crate) fn check_path_orientation_safe(g: &Game, path: &Path) -> i32 {
    let nodes = g.path.nodes(path);
    if nodes.is_empty() || nodes.iter().any(|n| n.world_tile.is_none()) {
        return 0;
    }
    crate::path::check_path_orientation(&g.map, nodes)
}

/// Phase 3S: the fields of an `ACTOR2` the snapper/vent updates read, copied at their start
/// (they used to clone the whole actor, strings and vectors included, every tick).
#[derive(Clone, Copy)]
struct A2Snap {
    radius: Fixed,
    position: SVector,
    q_rot: IQuaternion,
    effects: u32,
    value1: Fixed,
    anim_speed: Fixed,
    depth_shift: i8,
}

impl A2Snap {
    fn of(a: &crate::actor2::Actor2) -> A2Snap {
        A2Snap {
            radius: a.radius,
            position: a.actor.position,
            q_rot: a.actor.q_rot,
            effects: a.effects,
            value1: a.value1,
            anim_speed: a.anim_speed,
            depth_shift: a.depth_shift,
        }
    }
}
