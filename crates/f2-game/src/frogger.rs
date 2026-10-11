//! Port of `Common/frogger.c` / `frogger.h`: players, frog actors, the character
//! pool and the idle animations.
//!
//! `frogger.c` also defines `platformList`, `currPlatform`/`destPlatform` (kept in
//! [`crate::platform::PlatformState`] next to the code that uses them), the sprite /
//! overlay / particle lists (later waves) and a few direction constants.

use crate::prelude::*;

use f2_math::{dot_product_ff, sub_vector_fss, FVector, Fixed, IQuaternion, SVector};
use serde::{Deserialize, Serialize};

use crate::define::{tilestate, SINGLEPLAYER};
use crate::ids::{ActorId, TileId};
use crate::mdx_actor::{INIT_ANIMATION, INIT_SHADOW};
use crate::timer::Timer;
use crate::Game;

/// `MAX_IDLE_TIME` (`frogger.h:24`).
pub const MAX_IDLE_TIME: i32 = 300;
/// `SPAWN_SCOREUPTIMER`.
pub const SPAWN_SCOREUPTIMER: i32 = 15;
/// `MAX_FROGS`.
pub const MAX_FROGS: usize = 4;
/// `FROG_BREATHE_SPEED`.
pub const FROG_BREATHE_SPEED: i32 = 50;

// ----- frog animation enum (`frogger.h:34`) -----
pub const FROG_ANIM_HOP: i32 = 0;
pub const FROG_ANIM_HOPLEFT: i32 = 1;
pub const FROG_ANIM_HOPRIGHT: i32 = 2;
pub const FROG_ANIM_SUPERHOP: i32 = 3;
pub const FROG_ANIM_SOMERSAULT: i32 = 4;
pub const FROG_ANIM_HANDSPRING: i32 = 5;
pub const FROG_ANIM_FALL: i32 = 6;
pub const FROG_ANIM_TONGUE: i32 = 7;
pub const FROG_ANIM_BREATHE: i32 = 8;
pub const FROG_ANIM_LOOKAROUND: i32 = 9;
pub const FROG_ANIM_RETROPOPUP: i32 = 10;
pub const FROG_ANIM_WALLFLIP: i32 = 11;
pub const FROG_ANIM_WALLSPLAT: i32 = 12;
pub const FROG_ANIM_ICE1: i32 = 13;
pub const FROG_ANIM_ICE2: i32 = 14;
pub const FROG_ANIM_PIN: i32 = 15;
pub const FROG_ANIM_PINNED: i32 = 16;
pub const MULTI_NUM_ANIMS: i32 = 17;
pub const FROG_ANIM_ICETUBE: i32 = 17;
pub const FROG_ANIM_FALLPUFFED: i32 = 18;
pub const FROG_ANIM_FALLFLAP: i32 = 19;
pub const FROG_ANIM_FALLLAND: i32 = 20;
pub const FROG_ANIM_HOP15: i32 = 21;
pub const FROG_ANIM_HOPWHOOHOO: i32 = 22;
pub const FROG_ANIM_LOOKUP: i32 = 23;
pub const FROG_ANIM_LOOKDOWN: i32 = 24;
pub const FROG_ANIM_PREEN: i32 = 25;
pub const FROG_ANIM_DANCE1: i32 = 26;
pub const FROG_ANIM_DANCE2: i32 = 27;
pub const FROG_ANIM_DANCE3: i32 = 28;
pub const FROG_ANIM_DANCE4: i32 = 29;
pub const FROG_ANIM_BOWLEDOVER: i32 = 30;
pub const FROG_ANIM_STUNG: i32 = 31;
pub const FROG_ANIM_ASSONFIRE: i32 = 32;
pub const FROG_ANIM_SPRAWLY: i32 = 33;
pub const FROG_ANIM_DROWN: i32 = 34;
pub const FROG_ANIM_SPLATFRAME: i32 = 35;
pub const FROG_ANIM_ELECTROCUTE: i32 = 36;
pub const FROG_ANIM_BIGHEAD: i32 = 37;
pub const FROG_ANIM_SPACEFLOAT: i32 = 38;
pub const FROG_ANIM_HOOVER: i32 = 39;
pub const FROG_ANIM_SCREENSPLAT: i32 = 40;
pub const FROG_ANIM_SCREENSLIDE: i32 = 41;
pub const FROG_ANIM_VICTORY: i32 = 42;
pub const NUM_FROG_ANIMS: i32 = 43;

// ----- characters (`frogger.h:87`) -----
pub const FROG_FROGGER: u8 = 0;
pub const FROG_LILLIE: u8 = 1;
pub const FROG_BABYFROG: u8 = 2;
pub const FROG_TWEE: u8 = 3;
pub const FROG_WART: u8 = 4;
pub const FROG_ROBOFROG: u8 = 5;
pub const FROG_HOPPER: u8 = 6;
pub const FROG_SWAMPY: u8 = 7;
pub const FROG_NUMFROGS: usize = 8;

pub const FANIM_NONE: i8 = 0;
pub const FANIM_ANIMS: i8 = 1;
pub const FANIM_ALL: i8 = 2;

/// `FROGSTORE` (`frogger.h:107`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrogStore {
    pub file_name: &'static str,
    pub single_model: &'static str,
    pub multi_model: &'static str,
    pub icon: &'static str,
    /// Can this character be selected.
    pub active: i8,
    /// `FANIM_*`: which animations it can do.
    pub anim: i8,
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[allow(clippy::too_many_arguments)]
const fn fs(
    file_name: &'static str,
    single_model: &'static str,
    multi_model: &'static str,
    icon: &'static str,
    active: i8,
    anim: i8,
    r: u8,
    g: u8,
    b: u8,
) -> FrogStore {
    FrogStore { file_name, single_model, multi_model, icon, active, anim, r, g, b }
}

/// `frogPool[FROG_NUMFROGS]` (`frogger.c:93`, `FINAL_MASTER` table: the last four
/// characters start locked). The runtime copy in [`FroggerState::frog_pool`] is what
/// the unlock code changes.
pub const ORIG_FROG_POOL: [FrogStore; FROG_NUMFROGS] = [
    fs("Frogger", "frogger.obe", "mfrogger.obe", "FROGGER", 1, FANIM_ALL, 80, 255, 80),
    fs("Lillie", "lilli.obe", "mlilli.obe", "LILLIE", 1, FANIM_ALL, 255, 80, 80),
    fs("Tad", "tad.obe", "mtad.obe", "BABYFROG", 1, FANIM_ALL, 80, 80, 255),
    fs("Treeby", "twee.obe", "mtwee.obe", "TWEE", 1, FANIM_ALL, 0, 255, 255),
    fs("Wart", "toad.obe", "mtoad.obe", "WART", 0, FANIM_ALL, 110, 60, 55),
    fs("Tank", "tank.obe", "mtank.obe", "ROBOFROG", 0, FANIM_ALL, 220, 220, 220),
    fs("Roobie", "roobie.obe", "mhopper.obe", "HOPPER", 0, FANIM_ALL, 220, 220, 30),
    fs("Swampy", "swampy.obe", "mswampy.obe", "SWAMPY", 0, FANIM_ALL, 0, 100, 0),
];

/// `PLAYER` (`frogger.h:122`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Player {
    pub name: [u8; 8],
    pub character: u8,
    /// Which frog is on this one (-1: none).
    pub frogon: i16,
    /// Which frog this one is on (-1: none).
    pub frogunder: i16,
    pub num_spawn: i16,
    pub lives: i16,
    /// How many lives the player had at the start of the level.
    pub old_lives: i16,
    pub health_points: u8,
    pub death_by: u8,
    /// Used for controller input.
    pub input_pause: u8,
    pub spawn_timer: i16,
    pub spawn_score_level: i16,
    pub world_num: u8,
    pub level_num: u8,
    pub frog_state: u32,
    pub idle_time: Fixed,
    pub idle_enable: u8,
    // frog movement related stuff
    pub can_jump: u8,
    pub has_jumped: u8,
    pub has_double_jumped: u8,
    pub is_super_hopping: u8,
    pub is_sinking: u8,
    pub extended_hop_dir: u8,
    pub safe: Timer,
    pub stun: Timer,
    pub dead: Timer,
    pub is_croaking: Timer,
    pub autohop: Timer,
    pub quickhop: Timer,
    pub slowhop: Timer,
    // Parabolic curve variables (see frogmove.c)
    pub jump_start_frame: u32,
    pub jump_time: Fixed,
    pub jump_speed: Fixed,
    pub jump_multiplier: Fixed,
    pub jump_fwd_vector: FVector,
    pub jump_up_vector: FVector,
    pub jump_origin: SVector,
    pub height_jumped: Fixed,
}

/// `FOG` (`sprite.h:110`), defined in `frogger.c`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fog {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub mode: u8,
    pub min: i16,
    pub max: i16,
}

/// `FOG_OFF` / `FOG_TO_COLOUR`.
pub const FOG_OFF: u8 = 0;
pub const FOG_TO_COLOUR: u8 = 1;

/// Direction constants of `frogger.c`.
pub const UP_VEC: FVector = FVector { vx: 0, vy: 4096, vz: 0 };
pub const IN_VEC: FVector = FVector { vx: 0, vy: 0, vz: 4096 };
pub const OUT_VEC: FVector = FVector { vx: 0, vy: 0, vz: -4096 };
pub const RIGHT_VEC: FVector = FVector { vx: 4096, vy: 0, vz: 0 };
pub const ONE_VEC: FVector = FVector { vx: 4096, vy: 4096, vz: 4096 };
/// `vertQ`.
pub const VERT_Q: IQuaternion = IQuaternion { x: 0, y: 4096, z: 0, w: 0 };

/// Globals defined in `frogger.c`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FroggerState {
    /// `player[MAX_FROGS]`.
    pub player: [Player; MAX_FROGS],
    /// `frog[MAX_FROGS]`.
    pub frog: [Option<ActorId>; MAX_FROGS],
    /// `hat[MAX_FROGS]` (never created on PC).
    pub hat: [Option<ActorId>; MAX_FROGS],
    /// `NUM_FROGS`.
    pub num_frogs: i32,
    /// `globalFrogScale`.
    pub global_frog_scale: Fixed,
    /// `frogRadius`.
    pub frog_radius: Fixed,
    /// `waitScale`: frames per unit of path-node wait time (20.12).
    pub wait_scale: Fixed,
    /// `fog`.
    pub fog: Fog,
    /// `xseed`.
    pub xseed: u32,
    /// `pauseController`.
    pub pause_controller: i32,
    /// `pointOfInterest` (unused by the game: always reset to NULL).
    pub point_of_interest: Option<SVector>,
    /// `pOIDistance`.
    pub p_oi_distance: Fixed,
    /// `PFX_NUMPARTICLES`.
    pub pfx_numparticles: i32,
    /// Runtime `frogPool` (`active` flags change when characters are unlocked).
    pub frog_pool_active: [i8; FROG_NUMFROGS],
}

impl Default for FroggerState {
    fn default() -> Self {
        FroggerState {
            player: Default::default(),
            frog: [None; MAX_FROGS],
            hat: [None; MAX_FROGS],
            num_frogs: 1,
            global_frog_scale: 2457,
            frog_radius: 200 << 12,
            wait_scale: 5 << 12,
            fog: Fog { r: 120, g: 120, b: 160, mode: FOG_OFF, min: 995, max: 5000 },
            xseed: (7789 << 16) + 13399,
            pause_controller: 0,
            point_of_interest: None,
            p_oi_distance: 20000 << 12,
            pfx_numparticles: 25,
            frog_pool_active: ORIG_FROG_POOL.map(|f| f.active),
        }
    }
}

impl FroggerState {
    /// `frogPool[i]` with the runtime `active` flag.
    pub fn frog_pool(&self, i: usize) -> FrogStore {
        let mut f = ORIG_FROG_POOL[i];
        f.active = self.frog_pool_active[i];
        f
    }
}

impl Game {
    /// `frog[p]->actor->position` (the origin when the actor is missing; the original
    /// would dereference NULL).
    pub fn frog_position(&self, p: usize) -> SVector {
        self.frogger.frog[p].and_then(|f| self.actor2.actors.get(f)).map_or(SVector::default(), |a| a.actor.position)
    }

    /// `frog[p]->radius`.
    pub fn frog_actor_radius(&self, p: usize) -> Fixed {
        self.frogger.frog[p].and_then(|f| self.actor2.actors.get(f)).map_or(0, |a| a.radius)
    }

    /// Port of `CreateFrogActor` (Common/frogger.c:142).
    pub fn create_frog_actor(&mut self, where_: Option<TileId>, name: &str, p: i32) {
        let pu = p as usize;
        let me = self.create_and_add_actor(name, 0, 0, 0, INIT_ANIMATION | INIT_SHADOW);
        self.frogger.frog[pu] = me;
        let Some(me) = me else {
            // The original dereferences the NULL actor here.
            self.effects.push(Effect::Log { text: format!("CreateFrogActor: no model {name}") });
            return;
        };

        // PC: `if (!rHardware) depthShift -= 50` - the port models the hardware renderer.
        let shadow_radius = (self.frogger.global_frog_scale / 8) << 12;
        let a = &mut self.actor2.actors[me];
        if let Some(s) = a.actor.shadow.as_mut() {
            s.radius = shadow_radius;
            s.alpha = 191;
        }
        a.flags = crate::actor2::ACTOR_DRAW_ALWAYS;

        self.tongue.tongue[pu].flags = crate::tongue::TONGUE_NONE | crate::tongue::TONGUE_IDLE;

        if self.game_state.multi == SINGLEPLAYER {
            self.actor_animate(me, FROG_ANIM_DANCE1, true, false, 50, false);
        } else {
            self.actor_animate(me, FROG_ANIM_BREATHE, true, false, 50, false);
        }

        match where_ {
            Some(t) => self.set_frogger_start_pos(t, p),
            None => self.frogmove.curr_tile[pu] = None,
        }

        let pl = &mut self.frogger.player[pu];
        pl.health_points = 1;
        pl.frogon = -1;
        pl.frogunder = -1;
        pl.jump_time = -1;
        pl.num_spawn = 0;

        self.actor2.actors[me].radius = self.frogger.frog_radius;
    }

    /// Port of `CreateFrogger` (Common/frogger.c:194).
    pub fn create_frogger(&mut self, create_frog_actor: bool, _create_frog_overlays: bool) {
        if create_frog_actor {
            for i in 0..self.frogger.num_frogs.max(0) as usize {
                let mut num = self.frogger.player[i].character as usize;
                // BUG(original): `num > FROG_NUMFROGS` lets num == 8 index past frogPool;
                // the port treats that (unreachable in practice) case as inactive.
                if num >= FROG_NUMFROGS || self.frogger.frog_pool_active[num] == 0 {
                    self.frogger.player[i].character = 0;
                    num = 0;
                }

                self.platform.curr_platform[i] = None;

                let start = self.map.g_t_start[i].or(self.map.g_t_start[0]);
                let pool = self.frogger.frog_pool(num);
                let model = if self.game_state.multi == SINGLEPLAYER { pool.single_model } else { pool.multi_model };
                self.create_frog_actor(start, model, i as i32);
            }
        }
    }

    /// Port of `FroggerIdleAnim` (Common/frogger.c:228).
    pub fn frogger_idle_anim(&mut self, i: i32) {
        let iu = i as usize;
        let Some(frog) = self.frogger.frog[iu] else { return };
        if let Some(ct) = self.frogmove.curr_tile[iu] {
            let dir = (self.frogmove.frog_facing[iu].wrapping_add(self.cam.cam_facing[iu]) & 3) as usize;
            let t = self.map.tile(ct).tile_ptrs[dir];

            if let Some(t) = t {
                if self.rng.random(3) != 0 {
                    let tt = self.map.tile(t).clone();
                    // Play a looking-over-the-edge animation when we're next to a drop or deadly tile
                    if tt.state == tilestate::JOIN {
                        let cur = self.map.tile(ct).clone();
                        let mut v = FVector::default();
                        sub_vector_fss(&mut v, &tt.centre, &cur.centre);
                        let mut n = FVector::default();
                        f2_math::new_set_vector_fs(&mut n, &cur.normal);
                        let height = dot_product_ff(&v, &n);
                        if height < -10 << 12 {
                            self.actor_animate(frog, FROG_ANIM_LOOKDOWN, false, false, 40, false);
                            self.actor_animate(frog, FROG_ANIM_BREATHE, true, true, 50, false);
                            return;
                        }
                    } else if tt.state == tilestate::DEADLY {
                        self.actor_animate(frog, FROG_ANIM_LOOKDOWN, false, false, 50, false);
                        self.actor_animate(frog, FROG_ANIM_BREATHE, true, true, 50, false);
                        return;
                    }
                }
            }
        }

        // otherwise, play a normal idle
        let seq: &[(i32, bool, bool, i32)] = match self.rng.random(9) {
            0 => {
                self.actor_animate(frog, FROG_ANIM_PREEN, false, false, 50, false);
                if self.act_frame_count & 1 != 0 {
                    self.actor_animate(frog, FROG_ANIM_PREEN, false, true, 50, false);
                }
                &[(FROG_ANIM_BREATHE, true, true, 50)]
            }
            1 => &[(FROG_ANIM_DANCE1, false, false, 80), (FROG_ANIM_BREATHE, true, true, 50)],
            2 => &[(FROG_ANIM_DANCE2, false, false, 80), (FROG_ANIM_BREATHE, true, true, 50)],
            3 => &[(FROG_ANIM_DANCE3, false, false, 80), (FROG_ANIM_BREATHE, true, true, 50)],
            4 => &[(FROG_ANIM_DANCE4, false, false, 80), (FROG_ANIM_BREATHE, true, true, 50)],
            5 => &[(FROG_ANIM_LOOKAROUND, false, false, 80), (FROG_ANIM_BREATHE, true, true, 50)],
            6 => &[
                (FROG_ANIM_DANCE1, false, false, 80),
                (FROG_ANIM_DANCE2, false, true, 80),
                (FROG_ANIM_DANCE3, false, true, 80),
                (FROG_ANIM_BREATHE, true, true, 50),
            ],
            7 => &[
                (FROG_ANIM_DANCE2, false, false, 80),
                (FROG_ANIM_DANCE3, false, true, 80),
                (FROG_ANIM_DANCE4, false, true, 80),
                (FROG_ANIM_BREATHE, true, true, 50),
            ],
            _ => &[
                (FROG_ANIM_DANCE1, false, false, 80),
                (FROG_ANIM_DANCE2, false, true, 80),
                (FROG_ANIM_DANCE3, false, true, 80),
                (FROG_ANIM_DANCE4, false, true, 80),
                (FROG_ANIM_BREATHE, true, true, 50),
            ],
        };
        for &(anim, looped, queue, speed) in seq {
            self.actor_animate(frog, anim, looped, queue, speed, false);
        }
    }
}

use crate::effects::Effect;
