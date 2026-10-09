//! Port of `Common/tongue.c` / `tongue.h`: the frog's tongue (single player only:
//! `UpdateFrogTongue(0)` is called from `PC/drawloop.cpp:250` when
//! `gameState.multi == SINGLEPLAYER`, after `ActorListDraw(1)`; `game.c:449` starts a
//! search on `PAD_CIRCLE`).
//!
//! The tongue picks up power-up garibs (`TONGUE_GET_PICKUP`) and scenic "flappy
//! things" (`TONGUE_GET_SCENIC`), or waves at nothing (`TONGUE_GET_NONE`).
//! `BabyFrogIsInRange` / `FrogIsInRange` / `ThrowFrogDirection` are ported but unused
//! by the PC game (the baby/frog grabbing code was removed from `UpdateFrogTongue`).
//!
//! Rendering (`tex = FindTexture("TONGUE")`, `tongueColours[]`) is the renderer's job;
//! the texture name and colours are kept as constants.

use f2_math::{
    distance_between_points_sf, distance_between_points_ss, dot_product_ff, fmul, ftol32, linear_interp_fssf, make_unit, new_set_vector_fs,
    rotate_vector_by_quaternion_ff, scale_vector_ff, set_vector_sf, sub_vector_fss, sub_vector_sss, to_fixed, FVector, Fixed, SVector,
};
use serde::{Deserialize, Serialize};

use crate::collect::{AUTOHOP_GARIB, EXTRALIFE_GARIB, INVULNERABILITY_GARIB, QUICKHOP_GARIB, SLOWHOP_GARIB};
use crate::effects::Effect;
use crate::enemies::{ENEMY_NEW_BABYFROG, ENEMY_NEW_FLAPPYTHING, ENEMY_NEW_PUSHESFROG};
use crate::frogger::{
    FROG_ANIM_BOWLEDOVER, FROG_ANIM_BREATHE, FROG_ANIM_FALLFLAP, FROG_ANIM_TONGUE, FROG_BREATHE_SPEED, FROG_HOPPER, FROG_SWAMPY, IN_VEC, MAX_FROGS, ONE_VEC,
};
use crate::frogmove::{FROGSTATUS_ISDEAD, FROGSTATUS_ISTONGUEING};
use crate::game::LEVELID_SPACE2;
use crate::ids::{ActorId, EnemyId, GaribId, TileId};
use crate::layout::worldid;
use crate::specfx::EF_BUBBLES;
use crate::Game;

/// `MAX_TONGUENODES` (`tongue.h:22`).
pub const MAX_TONGUENODES: usize = 12;
/// `TONGUE_FRACTION` (`tongue.h:23`).
pub const TONGUE_FRACTION: Fixed = 4096 / MAX_TONGUENODES as i32;
/// `TONGUE_RADIUSNORMAL` (`tongue.h:25`).
pub const TONGUE_RADIUSNORMAL: i32 = 1800;
/// `TONGUE_RADIUSLONG` (`tongue.h:26`).
pub const TONGUE_RADIUSLONG: i32 = 2250;
/// `TONGUE_STICKYRADIUS` (`tongue.h:28`).
pub const TONGUE_STICKYRADIUS: Fixed = 409600;
/// `TONGUE_WRAPAROUNDTHRESHOLD` (`tongue.h:29`).
pub const TONGUE_WRAPAROUNDTHRESHOLD: Fixed = -3072;
/// `TONGUE_OFFSET_UP` (`tongue.c:46`).
pub const TONGUE_OFFSET_UP: i32 = 150;
/// `TONGUE_OFFSET_FWD` (`tongue.c:47`, unused).
pub const TONGUE_OFFSET_FWD: i32 = 100;
/// `SCALE` (`newpsx.h:6`).
const SCALE: i32 = 10;

// ----- tongue target types (`tongue.h:32`) -----
pub const TONGUE_GET_BABY: u8 = 0;
pub const TONGUE_GET_GARIB: u8 = 1;
pub const TONGUE_GET_FROG: u8 = 2;
pub const TONGUE_GET_SCENIC: u8 = 3;
pub const TONGUE_GET_PICKUP: u8 = 4;
pub const TONGUE_GET_NONE: u8 = 5;

// ----- frog tongue flags (`tongue.h:62`) -----
pub const TONGUE_NONE: u32 = 0;
pub const TONGUE_IDLE: u32 = 1 << 0;
pub const TONGUE_BEINGUSED: u32 = 1 << 1;
pub const TONGUE_OUTGOING: u32 = 1 << 2;
pub const TONGUE_INCOMING: u32 = 1 << 3;
pub const TONGUE_HASITEMONIT: u32 = 1 << 4;
pub const TONGUE_HASITEMINMOUTH: u32 = 1 << 5;
pub const TONGUE_THROWING: u32 = 1 << 6;
pub const TONGUE_SEARCHING: u32 = 1 << 7;
pub const TONGUE_GRAPPLE: u32 = 1 << 8;

/// Texture the renderer draws the tongue with (`FindTexture("TONGUE")`, `tongue.c:189`).
pub const TONGUE_TEXTURE: &str = "TONGUE";

/// `tongueColours[]` (`tongue.c:59`), ARGB per character.
pub const TONGUE_COLOURS: [u32; 8] = [
    0xffdd0000, // Frogger - red
    0xffdd0000, // Lilli - red
    0xffff9000, // Tad - Orange
    0xff8f00ff, // Twee - purple
    0xffaa0000, // Wart - Deep Red
    0xffbbccee, // Tank - Metallic
    0xffdd0000, // Roobie - Red
    0xff555500, // Swampy - Dark green
];

/// `void *thing` of `TONGUE`: what the tongue is trying to collect.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TongueThing {
    #[default]
    None,
    /// `GARIB*` (`GaribIsInRange`).
    Garib(GaribId),
    /// `ENEMY*` (`ScenicIsInRange`, `BabyFrogIsInRange`).
    Enemy(EnemyId),
    /// `ACTOR2*` (`FrogIsInRange`).
    Actor(ActorId),
}

impl TongueThing {
    pub fn is_some(self) -> bool {
        self != TongueThing::None
    }
}

/// `TONGUE` (`tongue.h:43`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tongue {
    pub source: SVector,
    pub target: SVector,
    pub pos: SVector,
    pub fwd: FVector,
    pub interp: FVector,
    pub flags: u32,
    pub radius: Fixed,
    pub progress: Fixed,
    /// `type` (`TONGUE_GET_*`).
    pub ty: u8,
    pub can_tongue: u8,
    /// `segment` (`MAX_TONGUENODES` entries once `StartTongue` allocated it; empty = NULL).
    pub segment: Vec<FVector>,
    pub thing: TongueThing,
}

/// Globals of `tongue.c`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TongueState {
    /// `tongue[MAX_FROGS]`.
    pub tongue: [Tongue; MAX_FROGS],
    /// `eatEverythingMode` (a cheat).
    pub eat_everything_mode: u8,
    /// `headMatrix` (`frogmove.h:126`, PC): the frog head sub-object's `objMatrix`
    /// (`float[16]`), found by `FindFrogSubObjects` (PC/actor.cpp:141) and recomputed
    /// by `XformActor` whenever frog[0] is drawn (`view.rs`); `CalcTongueSource`
    /// zeroes its translation in place.
    pub head_matrix: Option<[f32; 16]>,
    /// Name of the head sub-object `headMatrix` points into (set by `FindFrogSubObjects`).
    pub head_object: Option<String>,
}

impl Game {
    fn tongue_frog_actor(&self, pl: usize) -> Option<ActorId> {
        self.frogger.frog[pl].filter(|&f| self.actor2.actors.get(f).is_some())
    }

    /// `currTile[pl]->normal` (zero when there is no tile; the original would crash).
    fn tongue_curr_normal(&self, pl: usize) -> SVector {
        self.frogmove.curr_tile[pl].map_or(SVector::default(), |t| self.map.tiles[t.idx()].normal)
    }

    fn tongue_enemy_actor_pos(&self, e: EnemyId) -> SVector {
        self.enemies.enemy_list.array[e.idx()].nme_actor.and_then(|a| self.actor2.actors.get(a)).map_or(SVector::default(), |a| a.actor.position)
    }

    // ----- garib access -----

    /// `garibList` in list order (`for( g=garibList.head.next; g != &garibList.head; g=g->next )`).
    fn tongue_garib_list(&self) -> Vec<GaribId> {
        let l = &self.collect.garib_list;
        let mut out = Vec::new();
        let mut c = l.head.first;
        while let Some(id) = c {
            out.push(id);
            c = l.array[id.idx()].links.next;
        }
        out
    }

    /// `g->fx->act[0]` (None when the effect list is not set up, e.g. in unit tests).
    fn garib_fx_actor(&self, f: crate::ids::SpecFxId) -> Option<crate::ids::ActorId> {
        self.specfx.array.get(f.idx())?.act.as_ref()?.first().copied().flatten()
    }

    /// `(g->active, g->type, g->fx ? &g->fx->act[0]->actor->position : NULL)`.
    fn tongue_garib_info(&self, g: GaribId) -> (bool, i32, Option<SVector>) {
        let gb = &self.collect.garib_list.array[g.idx()];
        let fx_pos = gb.fx.map(|f| self.garib_fx_actor(f).map_or(gb.pos, |a| self.actor2.actors[a].actor.position));
        (gb.active != 0, gb.garib_type as i32, fx_pos)
    }

    /// `SetVectorSS(&g->fx->act[0]->actor->position, pos)`.
    fn tongue_set_garib_fx_pos(&mut self, g: GaribId, pos: SVector) {
        let Some(f) = self.collect.garib_list.array[g.idx()].fx else { return };
        if let Some(a) = self.garib_fx_actor(f) {
            self.actor2.actors[a].actor.position = pos;
        } else {
            self.collect.garib_list.array[g.idx()].pos = pos;
        }
    }

    /// `DEACTIVATEENEMY(e)` (`tongue.c:71`).
    fn deactivate_enemy(&mut self, e: EnemyId) {
        let en = &mut self.enemies.enemy_list.array[e.idx()];
        en.active = 0;
        en.visible = 0;
        if let Some(a) = en.nme_actor {
            if let Some(a) = self.actor2.actors.get_mut(a) {
                a.draw = 0;
            }
        }
    }

    /// `ACTIVATEENEMY(e)` (`tongue.c:78`).
    fn activate_enemy(&mut self, e: EnemyId) {
        let en = &mut self.enemies.enemy_list.array[e.idx()];
        en.visible = 1;
        en.active = 1;
        if let Some(a) = en.nme_actor {
            if let Some(a) = self.actor2.actors.get_mut(a) {
                a.actor.size = ONE_VEC;
            }
        }
    }

    /// Port of `InitTongues` (Common/tongue.c:91).
    pub fn init_tongues(&mut self) {
        for t in self.tongue.tongue.iter_mut() {
            t.flags = TONGUE_NONE | TONGUE_IDLE;
            t.thing = TongueThing::None;
            t.radius = to_fixed(TONGUE_RADIUSNORMAL);
            t.segment = Vec::new();
            t.ty = 0;
            t.can_tongue = 1;
        }
    }

    /// Port of `FreeTongues` (Common/tongue.c:115).
    pub fn free_tongues(&mut self) {
        for t in self.tongue.tongue.iter_mut() {
            t.flags = TONGUE_NONE | TONGUE_IDLE;
            t.thing = TongueThing::None;
            t.radius = to_fixed(TONGUE_RADIUSNORMAL);
            t.segment = Vec::new();
            t.ty = 0;
            t.can_tongue = 1;
        }
    }

    /// Port of `StartTongue` (Common/tongue.c:141).
    pub fn start_tongue(&mut self, ty: u8, dest: &SVector, pl: i32) {
        let p = pl as usize;
        if self.tongue.tongue[p].can_tongue == 0 || self.frogger.player[p].can_jump == 0 {
            self.tongue.tongue[p].flags = TONGUE_NONE | TONGUE_IDLE;
            self.tongue.tongue[p].thing = TongueThing::None;
            return;
        }
        let Some(f) = self.tongue_frog_actor(p) else {
            return;
        };
        let (fpos, q_rot) = {
            let a = &self.actor2.actors[f].actor;
            (a.position, a.q_rot)
        };
        let n = self.tongue_curr_normal(p);
        let t = &mut self.tongue.tongue[p];

        // Determine frog's forward vector
        rotate_vector_by_quaternion_ff(&mut t.fwd, &IN_VEC, &q_rot);

        // Set source, including forward and up offsets
        t.source.vx = (fpos.vx as i32).wrapping_add(((n.vx as i32) * TONGUE_OFFSET_UP) >> 12) as i16;
        t.source.vy = (fpos.vy as i32).wrapping_add(((n.vy as i32) * TONGUE_OFFSET_UP) >> 12) as i16;
        t.source.vz = (fpos.vz as i32).wrapping_add(((n.vz as i32) * TONGUE_OFFSET_UP) >> 12) as i16;
        t.pos = t.source;

        // Calculate the vector to the collectable item and its magnitude
        t.target = *dest;
        let mut to = FVector::default();
        sub_vector_fss(&mut to, &t.target, &t.source);
        make_unit(&mut to);

        t.progress = 0;

        // Calculate angle through which the tongue needs to rotate
        let no = 0;
        let dp = dot_product_ff(&t.fwd, &to);
        if dp < TONGUE_WRAPAROUNDTHRESHOLD || no != 0 {
            t.flags = TONGUE_NONE | TONGUE_IDLE;
            t.thing = TongueThing::None;
            return;
        }

        t.ty = ty;
        // tex = FindTexture("TONGUE"): renderer side (TONGUE_TEXTURE).
        if t.segment.is_empty() {
            t.segment = vec![FVector::default(); MAX_TONGUENODES];
        }
        let mut sp = FVector::default();
        f2_math::set_vector_fs(&mut sp, &t.pos);
        for s in t.segment.iter_mut() {
            *s = sp;
        }

        // Set frog mouth open animation, and the speed (Swampy has a silly tongue anim - ignore it)
        if self.frogger.player[p].character != FROG_SWAMPY {
            self.actor_animate(f, FROG_ANIM_TONGUE, false, false, 128, false);
        }

        let t = &mut self.tongue.tongue[p];
        t.flags = TONGUE_NONE | TONGUE_BEINGUSED | TONGUE_OUTGOING;
        t.can_tongue = 0;

        self.effects.push(Effect::PlayVoice { player: pl as u8, uid: f2_formats::crc::crc("tongue") });
    }

    /// Port of `UpdateFrogTongue` (Common/tongue.c:217).
    pub fn update_frog_tongue(&mut self, pl: i32) {
        let p = pl as usize;
        if self.tongue.tongue[p].flags & TONGUE_IDLE != 0 {
            return;
        }

        // Frogs tongue is ready for action
        if self.tongue.tongue[p].flags & TONGUE_BEINGUSED != 0 {
            // Check if tongue can actually be used
            if self.tongue.tongue[p].flags & TONGUE_HASITEMINMOUTH != 0 {
                return;
            }

            // Update source from matrix of head bone
            self.calc_tongue_source(pl);

            // Update the targets position
            let thing = self.tongue.tongue[p].thing;
            match self.tongue.tongue[p].ty {
                TONGUE_GET_SCENIC => {
                    if let TongueThing::Enemy(e) = thing {
                        self.tongue.tongue[p].target = self.tongue_enemy_actor_pos(e);
                    }
                }
                TONGUE_GET_PICKUP => {
                    if let TongueThing::Garib(g) = thing {
                        if let (_, _, Some(pos)) = self.tongue_garib_info(g) {
                            self.tongue.tongue[p].target = pos;
                        }
                    }
                }
                TONGUE_GET_NONE => self.tongue.tongue[p].target = self.frog_position(p),
                _ => {}
            }

            if self.tongue.tongue[p].flags & TONGUE_OUTGOING != 0 {
                let mut radcheck = TONGUE_STICKYRADIUS;
                if self.frogger.player[0].world_num as u32 == worldid::SPACE && self.frogger.player[0].level_num as u32 == LEVELID_SPACE2 {
                    if let Some(cp) = self.platform.curr_platform[p] {
                        let plat = &self.platform.platform_list.array[cp.idx()];
                        if plat.is_waiting == 0 && plat.active != 0 {
                            radcheck = radcheck.wrapping_add(fmul(plat.curr_speed, self.game_speed).wrapping_mul(256));
                        }
                    }
                }

                let game_speed = self.game_speed;
                let t = &mut self.tongue.tongue[p];
                // If tongue has not got to the target
                if distance_between_points_sf(&t.target, &t.interp) > radcheck {
                    // Extend the tongue a bit more
                    t.progress = t.progress.wrapping_add(fmul(TONGUE_FRACTION, fmul(game_speed, 6144i32.wrapping_sub(t.progress))));

                    if t.progress > 4096 {
                        t.progress = 4096;
                    }
                    let (src, tgt, prog) = (t.source, t.target, t.progress);
                    linear_interp_fssf(&mut t.interp, &src, &tgt, prog);
                    self.calculate_tongue(pl);

                    // If we have gone too far
                    let t = &mut self.tongue.tongue[p];
                    if distance_between_points_ss(&t.source, &t.pos) > t.radius {
                        t.flags = TONGUE_NONE | TONGUE_BEINGUSED | TONGUE_INCOMING;
                        return;
                    }
                } else {
                    // Got to target - return with item
                    t.flags = TONGUE_NONE | TONGUE_BEINGUSED | TONGUE_INCOMING;
                    if t.thing.is_some() {
                        t.flags |= TONGUE_HASITEMONIT;
                    }
                    return;
                }
            }

            if self.tongue.tongue[p].flags & TONGUE_INCOMING != 0 {
                let mut radcheck = TONGUE_STICKYRADIUS;
                if let Some(cp) = self.platform.curr_platform[p] {
                    radcheck = radcheck.wrapping_add(self.platform.platform_list.array[cp.idx()].curr_speed);
                }

                if self.tongue.tongue[p].flags & TONGUE_HASITEMONIT != 0 {
                    // (The TONGUE_GET_GARIB case is commented out in the original.)
                    let (ty, thing, pos) = {
                        let t = &self.tongue.tongue[p];
                        (t.ty, t.thing, t.pos)
                    };
                    if ty == TONGUE_GET_PICKUP {
                        if let TongueThing::Garib(g) = thing {
                            self.tongue_set_garib_fx_pos(g, pos);
                        }
                    } else if ty == TONGUE_GET_SCENIC {
                        if let TongueThing::Enemy(e) = thing {
                            let scale = 4096 - ((128i32.wrapping_mul(self.game_speed)) >> 12);
                            if let Some(a) = self.enemies.enemy_list.array[e.idx()].nme_actor {
                                if let Some(a) = self.actor2.actors.get_mut(a) {
                                    scale_vector_ff(&mut a.actor.size, scale);
                                    a.actor.position = pos;
                                }
                            }
                        }
                    }
                }

                // Retract tongue a bit
                let game_speed = self.game_speed;
                let t = &mut self.tongue.tongue[p];
                t.progress = t.progress.wrapping_sub(fmul(TONGUE_FRACTION, fmul(game_speed, 6144i32.wrapping_sub(t.progress))));

                if t.progress < 0 {
                    t.progress = 0;
                }
                let (src, tgt, prog) = (t.source, t.target, t.progress);
                linear_interp_fssf(&mut t.interp, &src, &tgt, prog);
                self.calculate_tongue(pl);

                // If tongue has got back to mouth
                let t = &self.tongue.tongue[p];
                if distance_between_points_sf(&t.source, &t.interp) < radcheck {
                    if t.flags & TONGUE_HASITEMONIT != 0 {
                        let (ty, thing) = (t.ty, t.thing);
                        // t = MakeTrigger( OnTimeout, (void *)(actFrameCount + 10), NULL, NULL, NULL );
                        // arg1 = (void *)FindVoice(utilStr2CRC("frogbelch1"),pl);
                        // AttachEvent( t, TRIGGER_ONCE, 0, PlaySFX, arg1, (void *)(&frog[pl]->actor->position), NULL, NULL );
                        let point = self.frogger.frog[p].map(crate::event::SfxPoint::ActorPosition);
                        let sample = crate::event::SfxSample::Voice { player: pl as u8, uid: f2_formats::crc::crc("frogbelch1") };
                        self.make_timeout_event(10, crate::event::EventFunc::PlaySfx { sample, point });

                        if ty == TONGUE_GET_PICKUP {
                            if let TongueThing::Garib(g) = thing {
                                self.pickup_collectable(g, pl);
                            }
                        } else if ty == TONGUE_GET_SCENIC {
                            if let TongueThing::Enemy(e) = thing {
                                self.deactivate_enemy(e);

                                if self.tongue.eat_everything_mode != 0 {
                                    let mut up = FVector::default();
                                    new_set_vector_fs(&mut up, &self.tongue_curr_normal(p));
                                    let fpos = self.frogger.frog[p].map(|a| self.actor2.actors[a].actor.position).unwrap_or_default();
                                    self.create_explode_effect(&fpos, &up);
                                    self.check_for_attached_nmes(e, 0);
                                }
                            }
                        }
                    }

                    self.tongue.tongue[p].thing = TongueThing::None;
                    self.remove_frog_tongue(pl);

                    // Set frog idle animation
                    if self.frogger.player[p].frog_state & FROGSTATUS_ISDEAD == 0 {
                        if let Some(f) = self.tongue_frog_actor(p) {
                            self.actor_animate(f, FROG_ANIM_BREATHE, true, true, FROG_BREATHE_SPEED, false);
                        }
                    }
                }
            }
        }

        if self.tongue.tongue[p].flags & TONGUE_SEARCHING != 0 {
            let radius = self.tongue.tongue[p].radius;
            let g = self.garib_is_in_range(radius, pl);
            self.tongue.tongue[p].thing = g.map_or(TongueThing::None, TongueThing::Garib);
            if let Some(g) = g {
                let (_, ty, fx_pos) = self.tongue_garib_info(g);
                match ty {
                    QUICKHOP_GARIB | SLOWHOP_GARIB | AUTOHOP_GARIB | INVULNERABILITY_GARIB | EXTRALIFE_GARIB => {
                        if let Some(pos) = fx_pos {
                            self.start_tongue(TONGUE_GET_PICKUP, &pos, pl);
                        }
                    }
                    _ => {
                        self.tongue.tongue[p].thing = TongueThing::None;
                        self.tongue.tongue[p].flags = TONGUE_IDLE;
                    }
                }
            }

            if !self.tongue.tongue[p].thing.is_some() {
                let radius = self.tongue.tongue[p].radius;
                let e = self.scenic_is_in_range(radius, pl);
                self.tongue.tongue[p].thing = e.map_or(TongueThing::None, TongueThing::Enemy);
                if let Some(e) = e {
                    // Stop fish making bubbles when held - butterflies can carry on doing something
                    let pos = self.tongue_enemy_actor_pos(e);
                    self.start_tongue(TONGUE_GET_SCENIC, &pos, pl);
                    if self.tongue.tongue[p].thing.is_some() {
                        let en = &mut self.enemies.enemy_list.array[e.idx()];
                        en.active = 0;
                        if let Some(a) = en.nme_actor {
                            if let Some(a) = self.actor2.actors.get_mut(a) {
                                a.effects &= !EF_BUBBLES;
                            }
                        }
                    }
                }
            }

            if !self.tongue.tongue[p].thing.is_some() {
                // Go to a point out in front and wave about a bit
                let pos = self.frog_position(p);
                self.start_tongue(TONGUE_GET_NONE, &pos, pl);
            }
        }
    }

    /// Port of `CalculateTongue` (Common/tongue.c:413): plot a Hermite curve based on
    /// the start and end points.
    pub fn calculate_tongue(&mut self, pl: i32) {
        let t = &mut self.tongue.tongue[pl as usize];
        if t.segment.is_empty() {
            // The original would write through a NULL `segment` (unreachable: StartTongue allocates).
            t.segment = vec![FVector::default(); MAX_TONGUENODES];
        }

        let mut to = SVector::default();
        sub_vector_sss(&mut to, &t.target, &t.source);

        // Set geometry components. BUG(original): `fwd.v *= 2000` scales the stored
        // forward vector itself (harmless: CalcTongueSource recomputes it each frame).
        t.fwd.vx = t.fwd.vx.wrapping_mul(2000);
        t.fwd.vy = t.fwd.vy.wrapping_mul(2000);
        t.fwd.vz = t.fwd.vz.wrapping_mul(2000);
        let gx = [to_fixed(t.source.vx as i32), t.interp.vx, t.fwd.vx, to_fixed(to.vx as i32)];
        let gy = [to_fixed(t.source.vy as i32), t.interp.vy, t.fwd.vy, to_fixed(to.vy as i32)];
        let gz = [to_fixed(t.source.vz as i32), t.interp.vz, t.fwd.vz, to_fixed(to.vz as i32)];

        let index = (t.progress.wrapping_mul(MAX_TONGUENODES as i32 - 1) >> 12) as usize;

        f2_math::set_vector_fs(&mut t.segment[0], &t.source);

        let mut tt: Fixed = 0;
        for i in 1..=index {
            tt = tt.wrapping_add(TONGUE_FRACTION);
            let t2 = fmul(tt, tt);
            let t3 = fmul(t2, tt);
            let h0 = t3.wrapping_mul(2).wrapping_sub(t2.wrapping_mul(3)).wrapping_add(4096);
            let h1 = t3.wrapping_mul(-2).wrapping_add(t2.wrapping_mul(3));
            let h2 = t3.wrapping_sub(t2.wrapping_mul(2)).wrapping_add(tt);
            let h3 = t2.wrapping_sub(t2);
            let c = |g: &[Fixed; 4]| fmul(h0, g[0]).wrapping_add(fmul(h1, g[1])).wrapping_add(fmul(h2, g[2])).wrapping_add(fmul(h3, g[3]));
            t.segment[i] = FVector { vx: c(&gx), vy: c(&gy), vz: c(&gz) };
        }

        let s = t.segment[index];
        set_vector_sf(&mut t.pos, &s);
    }

    /// Port of `RemoveFrogTongue` (Common/tongue.c:463).
    pub fn remove_frog_tongue(&mut self, pl: i32) {
        let p = pl as usize;
        self.tongue.tongue[p].flags = TONGUE_NONE | TONGUE_IDLE;
        if let TongueThing::Enemy(nme) = self.tongue.tongue[p].thing {
            if self.tongue.tongue[p].ty == TONGUE_GET_SCENIC {
                self.activate_enemy(nme);

                if self.tongue.eat_everything_mode != 0 {
                    self.check_for_attached_nmes(nme, 1);
                }
            }
        }

        let t = &mut self.tongue.tongue[p];
        t.thing = TongueThing::None;
        t.radius = to_fixed(TONGUE_RADIUSNORMAL);
        t.ty = 0;
        t.progress = 0;
        t.can_tongue = 1;

        self.frogger.player[p].frog_state &= !FROGSTATUS_ISTONGUEING;
    }

    /// Port of `GaribIsInRange` (Common/tongue.c:503): nearest active garib with an
    /// effect within `radius`.
    pub fn garib_is_in_range(&self, radius: Fixed, pl: i32) -> Option<GaribId> {
        let fpos = self.frog_position(pl as usize);
        let mut nearest = None;
        let mut best = radius;
        for g in self.tongue_garib_list() {
            let (active, _, fx_pos) = self.tongue_garib_info(g);
            let Some(pos) = fx_pos else { continue };
            if !active {
                continue;
            }
            let dist = distance_between_points_ss(&fpos, &pos);
            if dist < best {
                best = dist;
                nearest = Some(g);
            }
        }
        nearest
    }

    /// Port of `BabyFrogIsInRange` (Common/tongue.c:532).
    pub fn baby_frog_is_in_range(&self, radius: Fixed, pl: i32) -> Option<EnemyId> {
        let fpos = self.frog_position(pl as usize);
        let mut nearest = None;
        let mut best = radius;
        for i in 0..self.map.num_babies.max(0) as usize {
            // BUG(original): no bound against NUM_BABIES; the port stops at the array end.
            let Some(b) = self.babyfrog.baby_list.get(i) else { break };
            let (Some(baby), Some(enemy)) = (b.baby, b.enemy) else { continue };
            if b.is_saved != 0 {
                continue;
            }
            let Some(a) = self.actor2.actors.get(baby) else { continue };
            let dist = distance_between_points_ss(&fpos, &a.actor.position);
            if dist < best {
                best = dist;
                nearest = Some(enemy);
            }
        }
        nearest
    }

    /// Port of `ScenicIsInRange` (Common/tongue.c:562).
    pub fn scenic_is_in_range(&self, radius: Fixed, pl: i32) -> Option<EnemyId> {
        let fpos = self.frog_position(pl as usize);
        let mut nearest = None;
        let mut best = radius;
        let l = &self.enemies.enemy_list;
        let mut c = l.head.first;
        while let Some(id) = c {
            let cur = &l.array[id.idx()];
            c = cur.links.next;
            if self.tongue.eat_everything_mode == 0 {
                if cur.flags & ENEMY_NEW_FLAPPYTHING == 0 || cur.active == 0 || cur.visible == 0 {
                    continue;
                }
            } else if cur.flags & ENEMY_NEW_BABYFROG != 0 || cur.flags & ENEMY_NEW_PUSHESFROG != 0 || cur.active == 0 || cur.visible == 0 || cur.uid != 0 {
                continue;
            }

            let dist = distance_between_points_ss(&fpos, &self.tongue_enemy_actor_pos(id));
            if dist < best {
                best = dist;
                nearest = Some(id);
            }
        }
        nearest
    }

    /// Port of `FrogIsInRange` (Common/tongue.c:599): nearest other frog (multiplayer).
    pub fn frog_is_in_range(&self, radius: Fixed, pl: i32) -> Option<ActorId> {
        let n = self.frogger.num_frogs;
        if n <= 0 {
            return None;
        }
        let mut closest = -1i32;
        let mut best: Fixed = 2048000000;
        let fpos = self.frog_position(pl as usize);
        let mut i = (pl + 1) % n;
        while i != pl {
            let dist = distance_between_points_ss(&self.frog_position(i as usize), &fpos);
            if dist < radius && dist < best {
                closest = i;
                best = dist;
            }
            i = (i + 1) % n;
        }
        if closest == -1 {
            return None;
        }
        self.frogger.frog[closest as usize]
    }

    /// Port of `ThrowFrogDirection` (Common/tongue.c:628): hurl a frog along a tile
    /// direction (multiplayer).
    pub fn throw_frog_direction(&mut self, thrower: i32, throwee: i32, dir: i32) {
        let Some(start) = self.frogmove.curr_tile[thrower as usize] else {
            return; // the original dereferences NULL
        };
        let mut tile: Option<TileId>;
        let mut tile2 = start;
        let mut tile3: Option<TileId> = None;
        let mut count: Fixed = 0;
        let mut fwd = FVector::default();
        new_set_vector_fs(&mut fwd, &self.map.tiles[start.idx()].dir_vector[dir as usize]);

        // Find all tiles in the direction as far as the eye can see
        loop {
            tile = crate::general::find_joined_tile_by_direction_constrained(&self.map, tile2, &fwd, 1024);
            let Some(t) = tile else { break };
            if tile3 == Some(t) {
                // Oscillating between 2 tiles
                tile = Some(tile2);
                break;
            }
            tile3 = Some(tile2);
            tile2 = t;
            count = count.wrapping_add(4096);
        }

        let tile = tile.unwrap_or(tile2);

        if let Some(f) = self.tongue_frog_actor(throwee as usize) {
            self.actor_animate(f, FROG_ANIM_BOWLEDOVER, false, false, 128, false);
            self.actor_animate(f, FROG_ANIM_FALLFLAP, false, true, 128, false);
        }

        self.spring_frog_to_tile(tile, 204800i32.wrapping_add(fmul(count, 40960)).wrapping_mul(SCALE), 410i32.wrapping_add(fmul(count, 328)), throwee);
    }

    /// Port of `CalcTongueSource` (Common/tongue.c:660), `PC_VERSION` branch: work out
    /// the orientation and position of the tongue from the frog's head bone matrix.
    pub fn calc_tongue_source(&mut self, pl: i32) {
        let p = pl as usize;
        if let Some(mut m) = self.tongue.head_matrix {
            // Store head bone pos and set matrix translate elements to zero
            let pos = [m[12], m[13], m[14]];
            m[12] = 0.0;
            m[13] = 0.0;
            m[14] = 0.0;
            self.tongue.head_matrix = Some(m);

            // Matrix multiply to get rotation only (guMtxXFMF, mdxMath.cpp:127)
            let xf = |x: f32, y: f32, z: f32| {
                [(m[0] * x) + (m[4] * y) + (m[8] * z) + m[12], (m[1] * x) + (m[5] * y) + (m[9] * z) + m[13], (m[2] * x) + (m[6] * y) + (m[10] * z) + m[14]]
            };
            let mut fwd = xf(0.0, 0.0, 1.0);
            let mut up = xf(0.0, -1.0, 0.0);

            // JIM: Nasty special case until I can be bothered working out another way
            let ch = self.frogger.player[p].character;
            if ch == FROG_HOPPER {
                fwd.iter_mut().for_each(|v| *v *= -3.0);
            } else if ch == FROG_SWAMPY {
                fwd.iter_mut().for_each(|v| *v *= 3.0);
                up.iter_mut().for_each(|v| *v = (*v as f64 * -0.5) as f32);
            }

            // Store in tongue forward as unit vector (SetVectorFR)
            let t = &mut self.tongue.tongue[p];
            t.fwd = FVector { vx: ftol32((fwd[0] * 4096.0) as f64), vy: ftol32((fwd[1] * 4096.0) as f64), vz: ftol32((fwd[2] * 4096.0) as f64) };
            make_unit(&mut t.fwd);
            // Scale to get offset to front of head
            fwd.iter_mut().for_each(|v| *v *= 12.0);
            up.iter_mut().for_each(|v| *v *= 5.0);

            // Actual front of head position.
            // RETAIL: the shipped exe evaluates (up*5 + pos + fwd*12) * SCALE in that order
            // (FUN_00434370), not fwd += pos; fwd += up.
            for i in 0..3 {
                fwd[i] = ((up[i] + pos[i]) + fwd[i]) * SCALE as f32;
            }
            // Which is our source (SetVectorSR)
            t.source = SVector { vx: ftol32(fwd[0] as f64) as i16, vy: ftol32(fwd[1] as f64) as i16, vz: ftol32(fwd[2] as f64) as i16, pad: t.source.pad };
        } else {
            let (fpos, q_rot) = match self.tongue_frog_actor(p) {
                Some(f) => {
                    let a = &self.actor2.actors[f].actor;
                    (a.position, a.q_rot)
                }
                None => return,
            };
            let n = self.tongue_curr_normal(p);
            let t = &mut self.tongue.tongue[p];
            // Update the forward vector
            rotate_vector_by_quaternion_ff(&mut t.fwd, &IN_VEC, &q_rot);
            t.source.vx = (fpos.vx as i32).wrapping_add(((n.vx as i32) * TONGUE_OFFSET_UP) >> 12) as i16;
            t.source.vy = (fpos.vy as i32).wrapping_add(((n.vy as i32) * TONGUE_OFFSET_UP) >> 12) as i16;
            t.source.vz = (fpos.vz as i32).wrapping_add(((n.vz as i32) * TONGUE_OFFSET_UP) >> 12) as i16;
        }
    }

    /// Port of `CheckForAttachedNMEs` (Common/tongue.c:769): mainly for Sub2 - check
    /// back and forward along the path for nothing.obes until a blank node is reached.
    pub fn check_for_attached_nmes(&mut self, e: EnemyId, flag: i32) {
        let (to_node, num_nodes) = {
            let p = &self.enemies.enemy_list.array[e.idx()].path;
            (p.to_node as i32, p.num_nodes as i32)
        };
        // BUG(original): if every node of a looping path holds an enemy (and `flag`
        // re-activates them, or they have a uid), the C loops forever. The port bails
        // out after a full lap and logs it.
        let limit = num_nodes.max(1) * 4;

        for step in [-1i32, 1] {
            // Search backwards in the path, then forwards.
            let mut i = to_node;
            let mut laps = 0;
            loop {
                i += step;
                if step < 0 && i < 0 {
                    i = num_nodes - 1;
                } else if step > 0 && i >= num_nodes {
                    i = 0;
                }

                let Some(nme) = self.find_enemy_at_node(Some(e), i) else { break };

                laps += 1;
                if laps > limit {
                    self.effects.push(Effect::Log { text: "CheckForAttachedNMEs: endless loop in the original".into() });
                    break;
                }

                if self.enemies.enemy_list.array[nme.idx()].uid != 0 {
                    continue;
                }

                if flag != 0 {
                    self.activate_enemy(nme);
                } else {
                    self.deactivate_enemy(nme);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor2::Actor2;
    use crate::enemies::Enemy;
    use crate::map::GameTile;
    use f2_math::IQuaternion;

    fn sv(x: i16, y: i16, z: i16) -> SVector {
        SVector { vx: x, vy: y, vz: z, pad: 0 }
    }

    fn add_actor(g: &mut Game, pos: SVector) -> ActorId {
        let mut a = Actor2::default();
        a.actor.position = pos;
        a.actor.q_rot = IQuaternion { x: 0, y: 0, z: 0, w: 4096 };
        a.actor.size = ONE_VEC;
        a.draw = 1;
        g.actor2.actors.slots.push(Some(a));
        ActorId::from(g.actor2.actors.slots.len() - 1)
    }

    /// One flat tile, frog 0 standing on it facing +z.
    fn game() -> Game {
        let mut g = Game::new();
        g.map.tiles.push(GameTile { normal: sv(0, 4096, 0), ..Default::default() });
        g.frogmove.curr_tile[0] = Some(TileId(0));
        let f = add_actor(&mut g, sv(0, 0, 0));
        g.frogger.frog[0] = Some(f);
        g.frogger.player[0].can_jump = 1;
        g.init_tongues();
        g
    }

    fn add_scenic(g: &mut Game, pos: SVector) -> EnemyId {
        let a = add_actor(g, pos);
        let e = Enemy { nme_actor: Some(a), flags: ENEMY_NEW_FLAPPYTHING, active: 1, visible: 1, ..Default::default() };
        let l = &mut g.enemies.enemy_list;
        l.array.push(e);
        let id = EnemyId::from(l.array.len() - 1);
        l.head.push_front(&mut l.array, id);
        id
    }

    fn run_until_idle(g: &mut Game) -> usize {
        for n in 1..200 {
            g.update_frog_tongue(0);
            if g.tongue.tongue[0].flags & TONGUE_IDLE != 0 {
                return n;
            }
        }
        panic!("tongue never returned: {:?}", g.tongue.tongue[0]);
    }

    #[test]
    fn init_sets_idle() {
        let g = game();
        let t = &g.tongue.tongue[0];
        assert_eq!(t.flags, TONGUE_IDLE);
        assert_eq!(t.radius, to_fixed(TONGUE_RADIUSNORMAL));
        assert_eq!(t.can_tongue, 1);
        assert_eq!(t.thing, TongueThing::None);
    }

    #[test]
    fn search_with_nothing_waves_and_returns() {
        let mut g = game();
        g.tongue.tongue[0].flags = TONGUE_NONE | TONGUE_SEARCHING;
        g.update_frog_tongue(0);
        let t = &g.tongue.tongue[0];
        assert_eq!(t.flags, TONGUE_BEINGUSED | TONGUE_OUTGOING);
        assert_eq!(t.ty, TONGUE_GET_NONE);
        assert_eq!(t.can_tongue, 0);
        assert_eq!(t.source, sv(0, 150, 0));
        assert_eq!(t.segment.len(), MAX_TONGUENODES);
        assert!(g.effects.contains(&Effect::PlayVoice { player: 0, uid: f2_formats::crc::crc("tongue") }));
        run_until_idle(&mut g);
        let t = &g.tongue.tongue[0];
        assert_eq!((t.can_tongue, t.progress, t.thing), (1, 0, TongueThing::None));
    }

    #[test]
    fn cannot_tongue_while_unable_to_jump() {
        let mut g = game();
        g.frogger.player[0].can_jump = 0;
        g.tongue.tongue[0].flags = TONGUE_SEARCHING;
        g.update_frog_tongue(0);
        assert_eq!(g.tongue.tongue[0].flags, TONGUE_IDLE);
        assert!(g.effects.is_empty());
    }

    #[test]
    fn grabs_and_eats_scenic() {
        let mut g = game();
        let far = add_scenic(&mut g, sv(0, 0, 1700));
        let e = add_scenic(&mut g, sv(0, 0, 300));
        assert_eq!(g.scenic_is_in_range(to_fixed(TONGUE_RADIUSNORMAL), 0), Some(e));
        g.tongue.tongue[0].flags = TONGUE_SEARCHING;
        g.update_frog_tongue(0);
        assert_eq!(g.tongue.tongue[0].ty, TONGUE_GET_SCENIC);
        assert_eq!(g.tongue.tongue[0].thing, TongueThing::Enemy(e));
        assert_eq!(g.enemies.enemy_list.array[e.idx()].active, 0);

        // Goes out, sticks, comes back carrying the scenic.
        let mut saw_item = false;
        for _ in 0..200 {
            g.update_frog_tongue(0);
            saw_item |= g.tongue.tongue[0].flags & TONGUE_HASITEMONIT != 0;
            if g.tongue.tongue[0].flags & TONGUE_IDLE != 0 {
                break;
            }
        }
        assert!(saw_item);
        assert_eq!(g.tongue.tongue[0].flags, TONGUE_IDLE);
        let en = &g.enemies.enemy_list.array[e.idx()];
        assert_eq!((en.active, en.visible), (0, 0));
        assert_eq!(g.actor2.actors[en.nme_actor.unwrap()].draw, 0);
        assert_eq!(g.enemies.enemy_list.array[far.idx()].active, 1);
    }

    #[test]
    fn remove_releases_scenic() {
        let mut g = game();
        let e = add_scenic(&mut g, sv(0, 0, 300));
        g.tongue.tongue[0].flags = TONGUE_SEARCHING;
        g.update_frog_tongue(0);
        g.frogger.player[0].frog_state |= FROGSTATUS_ISTONGUEING;
        g.remove_frog_tongue(0);
        let en = &g.enemies.enemy_list.array[e.idx()];
        assert_eq!((en.active, en.visible), (1, 1));
        assert_eq!(g.tongue.tongue[0].flags, TONGUE_IDLE);
        assert_eq!(g.frogger.player[0].frog_state & FROGSTATUS_ISTONGUEING, 0);
    }

    #[test]
    fn scenic_behind_frog_is_refused() {
        let mut g = game();
        add_scenic(&mut g, sv(0, 150, -300));
        g.tongue.tongue[0].flags = TONGUE_SEARCHING;
        g.update_frog_tongue(0);
        // StartTongue(SCENIC) fails on the wrap-around threshold, then the NONE tongue starts.
        assert_eq!(g.tongue.tongue[0].ty, TONGUE_GET_NONE);
    }

    #[test]
    fn grabs_powerup_garib() {
        let mut g = game();
        g.init_spec_fx_list();
        add_scenic(&mut g, sv(0, 0, 200));
        let gb =
            crate::collect::Garib { active: 1, garib_type: QUICKHOP_GARIB as u8, fx: Some(crate::ids::SpecFxId(0)), pos: sv(0, 0, 400), ..Default::default() };
        g.collect.garib_list.array.push(gb);
        g.add_garib(GaribId(0));
        assert_eq!(g.garib_is_in_range(to_fixed(TONGUE_RADIUSNORMAL), 0), Some(GaribId(0)));
        g.tongue.tongue[0].flags = TONGUE_SEARCHING;
        g.update_frog_tongue(0);
        // Garibs take priority over the (closer) scenic.
        assert_eq!(g.tongue.tongue[0].ty, TONGUE_GET_PICKUP);
        assert_eq!(g.tongue.tongue[0].thing, TongueThing::Garib(GaribId(0)));
        run_until_idle(&mut g);
        assert_eq!(g.tongue.tongue[0].thing, TongueThing::None);
    }

    #[test]
    fn coin_garib_cancels_search() {
        let mut g = game();
        let gb = crate::collect::Garib { active: 1, garib_type: 0, fx: Some(crate::ids::SpecFxId(0)), pos: sv(0, 0, 400), ..Default::default() };
        g.collect.garib_list.array.push(gb);
        g.add_garib(GaribId(0));
        g.tongue.tongue[0].flags = TONGUE_SEARCHING;
        g.update_frog_tongue(0);
        // default: thing = NULL, flags = IDLE, then the NONE tongue still starts.
        assert_eq!(g.tongue.tongue[0].ty, TONGUE_GET_NONE);
        assert_eq!(g.tongue.tongue[0].flags, TONGUE_BEINGUSED | TONGUE_OUTGOING);
    }

    #[test]
    fn frog_in_range() {
        let mut g = game();
        assert_eq!(g.frog_is_in_range(to_fixed(1000), 0), None);
        let f1 = add_actor(&mut g, sv(500, 0, 0));
        g.frogger.frog[1] = Some(f1);
        g.frogger.num_frogs = 2;
        assert_eq!(g.frog_is_in_range(to_fixed(1000), 0), Some(f1));
        assert_eq!(g.frog_is_in_range(to_fixed(400), 0), None);
    }

    #[test]
    fn hermite_segment_ends_at_pos() {
        let mut g = game();
        let t = &mut g.tongue.tongue[0];
        t.source = sv(0, 150, 0);
        t.target = sv(0, 0, 0);
        t.fwd = IN_VEC;
        t.progress = 4096;
        linear_interp_fssf(&mut t.interp, &sv(0, 150, 0), &sv(0, 0, 0), 4096);
        g.calculate_tongue(0);
        let t = &g.tongue.tongue[0];
        assert_eq!(t.fwd.vz, 4096 * 2000);
        assert_eq!(t.segment[0], FVector { vx: 0, vy: 150 << 12, vz: 0 });
        let mut p = SVector::default();
        set_vector_sf(&mut p, &t.segment[MAX_TONGUENODES - 1]);
        assert_eq!(t.pos, p);
    }
}
