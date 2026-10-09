//! Port of `Common/particle.c`: the `PARTICLE` pool (`MAX_PARTICLES = 512` on PC) and the
//! particle-system special effects (`PTYPE_*`, `CreateParticleEffect`, `UpdatePFXJet`).
//!
//! The pool mirrors `partList`: a free stack plus an intrusive list where each allocation
//! is pushed at the head, so an effect's `n` particles form a contiguous run starting at
//! the returned particle. Deallocation zeroes the particle (`memset`).

use f2_math::{
    add_to_vector_sf, dot_product_ff, fmul, scale_vector, scale_vector_ff, set_vector_ff, set_vector_fs, set_vector_ss, sub_from_vector_ff, FVector, SVector,
};
use serde::{Deserialize, Serialize};

use crate::ids::SpecFxId;
use crate::specfx::{FxUpdate, FXTYPE_NUMTYPES, FXTYPE_WAKE};
use crate::sprite::{SPRITE_ADDITIVE, SPRITE_TRANSLUCENT};
use crate::Game;

/// `MAX_PARTICLES` (PC).
pub const MAX_PARTICLES: usize = 512;

// `PTYPE_*` (`particle.h:20`).
pub const PTYPE_TELEPORTIDLE: i16 = FXTYPE_NUMTYPES;
pub const PTYPE_TELEPORTACTIVE: i16 = FXTYPE_NUMTYPES + 1;
pub const PTYPE_JET: i16 = FXTYPE_NUMTYPES + 2;
pub const PTYPE_WATERJET: i16 = FXTYPE_NUMTYPES + 3;
pub const PTYPE_NUMTYPES: i16 = FXTYPE_NUMTYPES + 4;

/// `PFX_NUMPARTICLES` (`Common/frogger.c:61`).
pub const PFX_NUMPARTICLES: i32 = 25;

const NIL: u32 = u32::MAX;

/// Index into the particle pool (`PARTICLE*`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ParticleId(pub u32);

/// `PARTICLE` (`particle.h:30`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Particle {
    pub pos: SVector,
    pub vel: FVector,
    pub poly: [SVector; 2],
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
    pub bounce: u8,
}

/// `PARTICLELIST partList`. Index `MAX_PARTICLES` in `next`/`prev` is the static head.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParticleState {
    pub array: Vec<Particle>,
    next: Vec<u32>,
    prev: Vec<u32>,
    stack: Vec<u32>,
    pub stack_ptr: i32,
    pub count: i32,
}

const HEAD: u32 = MAX_PARTICLES as u32;

impl ParticleState {
    /// Port of `InitParticleList` (Common/particle.c:348).
    pub fn init_particle_list(&mut self) {
        let n = MAX_PARTICLES;
        self.array = vec![Particle::default(); n];
        self.next = vec![NIL; n + 1];
        self.prev = vec![NIL; n + 1];
        self.next[n] = HEAD;
        self.prev[n] = HEAD;
        self.stack = (0..n as u32).collect();
        self.count = 0;
        self.stack_ptr = n as i32 - 1;
    }

    /// Port of `FreeParticleList` (Common/particle.c:310).
    pub fn free_particle_list(&mut self) {
        self.array.clear();
        self.next.clear();
        self.prev.clear();
        self.stack.clear();
        self.count = 0;
        self.stack_ptr = 0;
    }

    /// Port of `AllocateParticles` (Common/particle.c:254).
    pub fn allocate_particles(&mut self, number: i32) -> Option<ParticleId> {
        if self.array.is_empty() || number <= 0 || self.stack_ptr - number < 0 || number >= MAX_PARTICLES as i32 - self.count {
            return None;
        }
        let mut n = number;
        while n > 0 {
            n -= 1;
            let p = self.stack[self.stack_ptr as usize];
            self.stack_ptr -= 1;
            let first = self.next[HEAD as usize];
            self.prev[p as usize] = HEAD;
            self.next[p as usize] = first;
            self.prev[first as usize] = p;
            self.next[HEAD as usize] = p;
            self.count += 1;
        }
        Some(ParticleId(self.stack[(self.stack_ptr + 1) as usize]))
    }

    /// Port of `DeallocateParticles` (Common/particle.c:281).
    pub fn deallocate_particles(&mut self, head: Option<ParticleId>, number: i32) {
        let Some(h) = head else { return };
        let mut p = h.0;
        if self.array.is_empty() || self.next[p as usize] == NIL || number <= 0 || self.stack_ptr + number >= MAX_PARTICLES as i32 {
            return;
        }
        let mut n = number;
        while n > 0 {
            n -= 1;
            if p == HEAD || p == NIL {
                return;
            }
            let t = self.next[p as usize];
            let (pv, nx) = (self.prev[p as usize], self.next[p as usize]);
            self.next[pv as usize] = nx;
            self.prev[nx as usize] = pv;
            self.next[p as usize] = NIL;
            self.prev[p as usize] = NIL;
            self.array[p as usize] = Particle::default();
            self.count -= 1;
            self.stack_ptr += 1;
            self.stack[self.stack_ptr as usize] = p;
            p = t;
        }
    }

    /// `p->next` (None at the head).
    pub fn next_of(&self, p: ParticleId) -> Option<ParticleId> {
        let n = *self.next.get(p.0 as usize)?;
        (n != NIL && n != HEAD).then_some(ParticleId(n))
    }

    /// `p->prev` (None at the head).
    pub fn prev_of(&self, p: ParticleId) -> Option<ParticleId> {
        let n = *self.prev.get(p.0 as usize)?;
        (n != NIL && n != HEAD).then_some(ParticleId(n))
    }

    /// The `n` particles of a block starting at `first`.
    pub fn chain(&self, first: Option<ParticleId>, n: i32) -> Vec<ParticleId> {
        let mut v = Vec::new();
        let mut c = first;
        for _ in 0..n.max(0) {
            let Some(p) = c else { break };
            v.push(p);
            c = self.next_of(p);
        }
        v
    }

    pub fn get(&self, p: ParticleId) -> &Particle {
        &self.array[p.0 as usize]
    }

    pub fn get_mut(&mut self, p: ParticleId) -> &mut Particle {
        &mut self.array[p.0 as usize]
    }
}

/// `(Random(30)-10)*(speed/100)` added to each velocity component (C evaluates the three
/// statements in order).
fn jet_velocity(g: &mut Game, normal: &FVector, speed: i32) -> FVector {
    let mut vel = FVector::default();
    set_vector_ff(&mut vel, normal);
    scale_vector_ff(&mut vel, speed);
    let k = speed / 100;
    vel.vx = vel.vx.wrapping_add((g.rng.random(30) - 10).wrapping_mul(k));
    vel.vy = vel.vy.wrapping_add((g.rng.random(30) - 10).wrapping_mul(k));
    vel.vz = vel.vz.wrapping_add((g.rng.random(30) - 10).wrapping_mul(k));
    vel
}

impl Game {
    /// Port of `CreateParticleEffect` (Common/particle.c:44). `fx` is already allocated.
    pub(crate) fn create_particle_effect(&mut self, fx: SpecFxId) -> Option<SpecFxId> {
        let num_p = PFX_NUMPARTICLES;
        self.specfx.fx_mut(fx).num_p = num_p;
        let Some(parts) = self.particle.allocate_particles(num_p) else {
            self.deallocate_fx(Some(fx), 1);
            return None;
        };
        self.specfx.fx_mut(fx).particles = Some(parts);

        let e = self.specfx.fx(fx).clone();
        let mut up = FVector::default();
        set_vector_ff(&mut up, &e.normal);
        scale_vector(&mut up, 200);
        add_to_vector_sf(&mut self.specfx.fx_mut(fx).origin, &up);

        match e.ty {
            PTYPE_TELEPORTIDLE | PTYPE_TELEPORTACTIVE => {}
            PTYPE_WATERJET | PTYPE_JET => {
                let Some(spr) = self.sprite.allocate_sprites(num_p) else {
                    self.deallocate_fx(Some(fx), 1);
                    return None;
                };
                self.specfx.fx_mut(fx).sprites = Some(spr);
                let origin = self.specfx.fx(fx).origin;
                if let Some(p2) = self.specfx.allocate_p2() {
                    let pl = self.specfx.p2_mut(p2);
                    set_vector_ss(&mut pl.point, &origin);
                    set_vector_ff(&mut pl.normal, &e.normal);
                    self.specfx.fx_mut(fx).rebound = Some(p2);
                }
                let tex = self.specfx.textures.txtr_solid_ring;
                self.specfx.fx_mut(fx).tex = tex;
                let sprites = self.sprite.chain(Some(spr), num_p);
                let parts = self.particle.chain(Some(parts), num_p);
                let (scale, speed) = (e.scale, e.speed);
                for (s, p) in sprites.iter().zip(parts.iter()) {
                    {
                        let sp = self.sprite.get_mut(*s);
                        set_vector_ss(&mut sp.pos, &origin);
                        sp.r = e.r;
                        sp.g = e.g;
                        sp.b = e.b;
                        sp.a = 255;
                        sp.offset_x = -16;
                        sp.offset_y = -16;
                        sp.flags = SPRITE_TRANSLUCENT | SPRITE_ADDITIVE;
                    }
                    let vel = jet_velocity(self, &e.normal, speed);
                    self.particle.get_mut(*p).vel = vel;
                    let sp = self.sprite.get_mut(*s);
                    sp.scale_x = ((scale.vx >> 12) / crate::map::SCALE) as i16;
                    sp.scale_y = ((scale.vy >> 12) / crate::map::SCALE) as i16;
                    sp.texture = tex;
                }
                let f = self.specfx.fx_mut(fx);
                f.update = Some(FxUpdate::PfxJet);
                f.draw = None;
            }
            _ => {}
        }
        Some(fx)
    }

    /// Port of `UpdatePFXJet` (Common/particle.c:149).
    pub(crate) fn update_pfx_jet(&mut self, fx: SpecFxId) {
        let gs = self.game_speed;
        if let Some(a) = self.specfx.fx(fx).follow.and_then(|a| self.actor2.actors.get(a)) {
            let p = a.actor.position;
            set_vector_ss(&mut self.specfx.fx_mut(fx).origin, &p);
        }
        let e = self.specfx.fx(fx).clone();
        let mut i = e.num_p;
        let mut alive = 0;
        let mut s = e.sprites;
        let mut p = e.particles;
        while i > 0 {
            i -= 1;
            let (Some(sid), Some(pid)) = (s, p) else { break };
            // BUG(original): `continue` without advancing s/p, so one dead particle
            // stalls the rest of the loop on itself.
            if self.particle.get(pid).bounce != 0 {
                continue;
            }
            alive += 1;
            if e.gravity != 0 {
                let mut up = FVector::default();
                set_vector_ff(&mut up, &e.normal);
                scale_vector_ff(&mut up, fmul(e.gravity, gs));
                sub_from_vector_ff(&mut self.particle.get_mut(pid).vel, &up);
            }
            if let Some(rb) = self.specfx.fx(fx).rebound {
                let (point, normal) = {
                    let pl = self.specfx.p2(rb);
                    (pl.point, pl.normal)
                };
                let mut frp = FVector::default();
                set_vector_fs(&mut frp, &point);
                let j = dot_product_ff(&frp, &normal).wrapping_neg();
                self.specfx.p2_mut(rb).j = j;
                let mut fsp = FVector::default();
                set_vector_fs(&mut fsp, &self.sprite.get(sid).pos);
                let dist = dot_product_ff(&fsp, &normal).wrapping_add(j).wrapping_neg();
                if dist > 0 {
                    self.particle.get_mut(pid).bounce = 1;
                    let sp = self.sprite.get_mut(sid);
                    sp.a = 0;
                    sp.draw = 0;
                    if e.ty == PTYPE_WATERJET && dist < 204800 {
                        let pos = self.sprite.get(sid).pos;
                        self.create_special_effect(FXTYPE_WAKE, &pos, &normal, 20480, 819, 410, 1229);
                    }
                }
            }
            let vel = self.particle.get(pid).vel;
            {
                let sp = self.sprite.get_mut(sid);
                sp.pos.vx = (sp.pos.vx as i32).wrapping_add(fmul(vel.vx, gs) >> 12) as i16;
                sp.pos.vy = (sp.pos.vy as i32).wrapping_add(fmul(vel.vy, gs) >> 12) as i16;
                sp.pos.vz = (sp.pos.vz as i32).wrapping_add(fmul(vel.vz, gs) >> 12) as i16;
            }

            // particle has died, respawn if the entire effect is alive
            let lifetime = self.specfx.fx(fx).lifetime;
            if self.particle.get(pid).bounce != 0 && (lifetime == -1 || self.act_frame_count < lifetime as u32) {
                self.particle.get_mut(pid).bounce = 0;
                let f = self.specfx.fx(fx).clone();
                {
                    let sp = self.sprite.get_mut(sid);
                    set_vector_ss(&mut sp.pos, &f.origin);
                    sp.r = f.r;
                    sp.g = f.g;
                    sp.b = f.b;
                    sp.a = 255;
                    sp.draw = 1;
                }
                let vel = jet_velocity(self, &f.normal, f.speed);
                self.particle.get_mut(pid).vel = vel;
            }
            s = self.sprite.next_of(sid);
            p = self.particle.next_of(pid);
        }
        let lifetime = self.specfx.fx(fx).lifetime;
        if alive == 0 && lifetime != -1 && self.act_frame_count > lifetime as u32 {
            self.deallocate_fx(Some(fx), 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn particle_pool_zeroes_on_free() {
        let mut p = ParticleState::default();
        p.init_particle_list();
        let a = p.allocate_particles(4).unwrap();
        let c = p.chain(Some(a), 4);
        assert_eq!(c.len(), 4);
        assert_eq!(p.prev_of(c[1]), Some(c[0]));
        p.get_mut(c[2]).bounce = 2;
        p.deallocate_particles(Some(a), 4);
        assert_eq!(p.count, 0);
        assert!(p.array.iter().all(|x| *x == Particle::default()));
        assert!(p.allocate_particles(512).is_none());
        assert!(p.allocate_particles(511).is_some());
    }
}
