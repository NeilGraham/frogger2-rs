//! Port of `Common/babyfrog.c` / `babyfrog.h`: baby frogs (list, pickup, behaviour).
//!
//! Sprite overlays (`babyIcons[]`, `babyFlash`) are kept as the minimal state the
//! logic reads/writes; drawing is not ported here.

use f2_math::{
    add_vector_sfs, distance_between_points_ss, fdiv, fixed_rotate_vector_by_rotation, iquat_slerp, magnitude_f, make_unit, scale_vector, scale_vector_ff,
    sub_vector_fss, FVector, Fixed, IQuaternion, SVector,
};
use serde::{Deserialize, Serialize};

use crate::actor2::{orientate, orientate_fs};
use crate::anim::SAMPLE_VOLUME;
use crate::define::SINGLEPLAYER;
use crate::effects::Effect;
use crate::frogger::IN_VEC;
use crate::ids::{ActorId, EnemyId, TileId};
use crate::layout::worldid;
use crate::map::SCALE;
use crate::Game;

/// `NUM_BABIES` (`babyfrog.h:16`).
pub const NUM_BABIES: usize = 5;
/// `BABY_SCALE` (`babyfrog.c:41`).
pub const BABY_SCALE: Fixed = 1638;
/// `BABY_ACTIVE_RADIUS` (`babyfrog.h:27`).
pub const BABY_ACTIVE_RADIUS: i32 = 3000;
/// `BABY_IDLE_TIME` (`babyfrog.h:28`).
pub const BABY_IDLE_TIME: i32 = 150;

// Baby animations (`babyfrog.h:30`).
pub const BABY_ANIM_SNOOZE: i32 = 0;
pub const BABY_ANIM_HOP: i32 = 1;
pub const BABY_ANIM_WAVE: i32 = 2;
pub const BABY_ANIM_WAGGLE: i32 = 3;
pub const BABY_ANIM_FLIP: i32 = 4;
pub const BABY_ANIM_WHEEL: i32 = 5;
pub const BABY_ANIM_TWOWAVE: i32 = 6;
pub const BABY_ANIM_OI: i32 = 7;
pub const BABY_ANIM_LOOKAROUND: i32 = 8;
pub const BABY_ANIM_FLOAT: i32 = 9;
pub const BABY_ANIM_COLLECT: i32 = 10;
pub const BABY_ANIM_COLLECTHOLD: i32 = 11;

// `FXTYPE_*` used here (`specfx.h:70`). TODO(port): move to specfx.rs.
const FXTYPE_SPARKLYTRAIL: i16 = 3;
const FXTYPE_CROAK: i16 = 27;

/// `BABY` (`babyfrog.h:44`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Baby {
    pub is_saved: u32,
    pub fx_colour: [u8; 4],
    pub idle: i32,
    pub collect: i32,
    pub baby: Option<ActorId>,
    pub enemy: Option<EnemyId>,
}

/// The fields of a `SPRITEOVERLAY` (`types2d.h`) that babyfrog.c drives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BabyOverlay {
    pub x_pos: i16,
    pub y_pos: i16,
    pub width: i16,
    pub height: i16,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
    pub draw: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BabyFrogState {
    /// `babyList[NUM_BABIES]`.
    pub baby_list: [Baby; NUM_BABIES],
    /// `lastBabySaved`.
    pub last_baby_saved: i32,
    /// `babiesSaved`.
    pub babies_saved: u32,
    /// `babyIcons[i]->r/g/b` (enemies set these too).
    pub baby_icon_rgb: [[u8; 3]; NUM_BABIES],
    /// `babyIcons[i]->a`.
    pub baby_icon_a: [u8; NUM_BABIES],
    /// `babyIcons[i]` geometry (`xPos, yPos, width, height`); `None` = `NULL`.
    pub baby_icons: [Option<BabyOverlay>; NUM_BABIES],
    /// `babyFlash` (`None` = `NULL`).
    pub baby_flash: Option<BabyOverlay>,
    /// `babyFlashFrame`.
    pub baby_flash_frame: i32,
}

/// `genSfx[GEN_*]` sample uids (PC/pcaudio.c:203).
fn gen_sfx(name: &str) -> u32 {
    f2_formats::crc::crc(name)
}

/// `SetVectorFF(&f, &svector)`: plain component copy.
#[inline]
fn fvec(s: &SVector) -> FVector {
    FVector { vx: s.vx as i32, vy: s.vy as i32, vz: s.vz as i32 }
}

impl Game {
    #[inline]
    fn num_babies_clamped(&self) -> usize {
        (self.map.num_babies.max(0) as usize).min(NUM_BABIES)
    }

    /// `currTile[0]->normal` (zero when there is no current tile; C would crash).
    fn curr_tile0_normal(&self) -> SVector {
        self.frogmove.curr_tile[0].map_or(SVector::default(), |t| self.map.tiles[t.idx()].normal)
    }

    /// `babyList[i].enemy->path->nodes->offset2` icon ordering hack.
    fn baby_icon_index(&self, i: usize) -> u32 {
        let off2 = self.babyfrog.baby_list[i].enemy.map_or(0, |e| {
            let path = self.enemies.enemy_list.array[e.idx()].path;
            if path.num_nodes > 0 {
                self.path.node(&path, 0).offset2
            } else {
                0
            }
        });
        if off2 != 0 {
            (((off2 >> 12) / SCALE) - 1) as u32
        } else {
            i as u32
        }
    }

    /// Port of `InitBabyList` (Common/babyfrog.c:52). PC hardware renderer assumed
    /// (`rHardware`: icon alpha 60, no colour halving).
    pub fn init_baby_list(&mut self, create_overlays: bool) {
        const COLOURS: [[u8; 4]; NUM_BABIES] = [[60, 255, 60, 255], [255, 255, 60, 255], [60, 60, 255, 255], [255, 60, 255, 255], [255, 60, 60, 255]];
        let n = self.num_babies_clamped();
        let b = &mut self.babyfrog;
        b.last_baby_saved = -1;
        b.babies_saved = 0;
        for (i, colour) in COLOURS.iter().enumerate().take(n) {
            let e = &mut b.baby_list[i];
            e.is_saved = 0;
            e.baby = None;
            e.idle = 0;
            e.collect = 0;
            e.fx_colour = *colour;
        }

        b.baby_flash = None;
        if create_overlays {
            // CreateAndAddSpriteOverlay((230+(i*200)), 210+BORDER, "00BABYIC", ...,0, SPRITE_ADDITIVE)
            // with i == numBabies after the loop above.
            let i = self.map.num_babies;
            b.baby_flash = Some(BabyOverlay {
                x_pos: (230 + i.wrapping_mul(200)) as i16,
                y_pos: 210,
                width: ((4096 * 24) / 640) as i16,
                height: ((4096 * 24) / 480) as i16,
                r: 255,
                g: 255,
                b: 255,
                a: 0,
                draw: 1,
            });
            for i in 0..n {
                b.baby_icons[i] = Some(BabyOverlay {
                    x_pos: (230 + i * 200) as i16,
                    y_pos: 210,
                    width: ((4096 * 24) / 640) as i16,
                    height: ((4096 * 24) / 480) as i16,
                    r: 255,
                    g: 255,
                    b: 255,
                    a: 60,
                    draw: 1,
                });
                b.baby_icon_a[i] = 60;
                let c = b.baby_list[i].fx_colour;
                b.baby_icon_rgb[i] = [c[0], c[1], c[2]];
            }
        }
        b.baby_flash_frame = -1;
    }

    /// Port of `ResetBabies` (Common/babyfrog.c:147).
    pub fn reset_babies(&mut self) {
        for i in 0..self.num_babies_clamped() {
            let b = &mut self.babyfrog;
            b.baby_list[i].is_saved = 0;
            b.baby_list[i].baby = None;
            b.baby_list[i].idle = 0;
            b.baby_list[i].collect = 0;
            b.babies_saved = 0;
            b.last_baby_saved = -1;
        }
    }

    /// Port of `PickupBabyFrog` (Common/babyfrog.c:170): called when a baby frog is
    /// picked up; returns TRUE if it was collected now.
    pub fn pickup_baby_frog(&mut self, baby: Option<ActorId>, tile: Option<TileId>) -> i32 {
        let nb = self.num_babies_clamped();
        let Some(i) = (0..nb).find(|&i| self.babyfrog.baby_list[i].baby == baby) else {
            return 0;
        };

        // check if this baby has been collected
        if self.babyfrog.baby_list[i].is_saved != 0 {
            return 0;
        }

        // Hack for icon ordering. Bleh.
        let n = self.baby_icon_index(i);
        let nu = n as usize;

        // PC `!rHardware` colour doubling dropped (hardware renderer modelled).

        self.babyfrog.baby_flash_frame = self.act_frame_count as i32;
        // BUG(original): babyIcons[n] with an out-of-range n (bad offset2) reads past the
        // array; the port skips the overlay updates then.
        let icon = self.babyfrog.baby_icons.get(nu).copied().flatten();
        if let (Some(flash), Some(icon)) = (self.babyfrog.baby_flash.as_mut(), icon) {
            let rgb = self.babyfrog.baby_icon_rgb[nu];
            flash.r = rgb[0];
            flash.g = rgb[1];
            flash.b = rgb[2];
            flash.x_pos = icon.x_pos;
            flash.y_pos = icon.y_pos;
            flash.width = icon.width;
            flash.height = icon.height;
        }

        self.babyfrog.baby_list[i].is_saved = 1;
        self.babyfrog.baby_list[i].collect = 40;

        self.babyfrog.last_baby_saved = i as i32;

        if nu < NUM_BABIES {
            self.babyfrog.baby_icon_a[nu] = 255;
            if let Some(ic) = self.babyfrog.baby_icons[nu].as_mut() {
                ic.a = 255;
            }
        }
        // TODO(port): arcadeHud.babiesBack[n]->r = g = b = 255 (HUD).

        self.babyfrog.babies_saved = self.babyfrog.babies_saved.wrapping_add(1);

        // If baby references a placeholder then set the respawn position to be the placeholder
        let value1 = baby.and_then(|b| self.actor2.actors.get(b)).map_or(0, |a| a.value1);
        if value1 != 0 {
            match self.get_enemy_from_uid(value1 >> 12) {
                Some(respawn) => {
                    let path = self.enemies.enemy_list.array[respawn.idx()].path;
                    self.map.g_t_start[0] = self.path.node(&path, 0).world_tile;
                }
                None => self.map.g_t_start[0] = tile,
            }
        } else {
            // Else respawn on the current baby tile
            self.map.g_t_start[0] = tile;
        }

        self.cam.start_cam_facing = self.cam.cam_facing[0] as i16;
        self.cam.start_frog_facing = self.frogmove.frog_facing[0] as i16;

        // Make a noise (weeble weeble) and do some snazzy effects (*zing!*)
        self.effects.push(Effect::PlayVoice { player: 0, uid: f2_formats::crc::crc("frogokay") });
        self.play_sample(gen_sfx("getbabyfrog"), None, 0, SAMPLE_VOLUME, -1);

        self.baby_collect_effect(baby, tile, n as i32);

        1
    }

    // wave E: `BabyCollectEffect` moved to specfx.rs.

    /// Port of `GetNearestBabyFrog` (Common/babyfrog.c:250): index of the nearest
    /// unsaved, drawn baby, or -1.
    pub fn get_nearest_baby_frog(&self) -> i32 {
        let mut baby = -1;
        // `fixed distance = 9999999999` truncates to 32 bits.
        let mut distance: Fixed = 9_999_999_999i64 as i32;
        let mut i = self.num_babies_clamped();
        while i > 0 {
            i -= 1;
            let b = &self.babyfrog.baby_list[i];
            if b.is_saved == 0 {
                if let Some(a) = b.baby.and_then(|id| self.actor2.actors.get(id)) {
                    if a.draw != 0 {
                        let t = distance_between_points_ss(&self.frog_position(0), &a.actor.position);
                        if t < distance {
                            distance = t;
                            baby = i as i32;
                        }
                    }
                }
            }
        }
        baby
    }

    /// Port of `UpdateBabies` (Common/babyfrog.c:282): mope when far from the frog,
    /// then do a sequence of attention-grabbing animations.
    pub fn update_babies(&mut self) {
        let flash_fr = (self.act_frame_count as i32).wrapping_sub(self.babyfrog.baby_flash_frame);
        let bff = self.babyfrog.baby_flash_frame;
        if let Some(f) = self.babyfrog.baby_flash.as_mut() {
            if flash_fr < 70 && bff > 0 {
                f.draw = 1;
                f.a = (0xff - (flash_fr.wrapping_mul(0xff) / 70)) as u8;
                f.width = ((4096 * 32) / 640 + (flash_fr.wrapping_mul(4096 * 64) / 640) / 70) as i16;
                f.height = ((4096 * 32) / 480 + (flash_fr.wrapping_mul(4096 * 64) / 480) / 70) as i16;
            } else {
                f.draw = 0;
            }
        }

        let nb = self.num_babies_clamped();
        let gs = self.game_speed;
        let mut i = 0;
        while i < nb {
            let Some(baby) = self.babyfrog.baby_list[i].baby else {
                break;
            };

            if self.actor2.actors[baby].draw == 0 {
                i += 1;
                continue;
            }

            // Baby frog has been collected, so move up and spin
            if self.babyfrog.baby_list[i].collect != 0 {
                self.run_baby_collect(i);
                i += 1;
                continue;
            }

            // Special case hack for floating in space2
            if self.frogger.player[0].world_num as u32 == worldid::SPACE && self.frogger.player[0].level_num == 1 {
                self.start_animate_actor(baby, BABY_ANIM_FLOAT, true, false, 75, false);
                i += 1;
                continue;
            }

            // Otherwise, check for distance from frog and do behaviour
            let act_pos = self.actor2.actors[baby].actor.position;
            let mut frog_v = FVector::default();
            sub_vector_fss(&mut frog_v, &self.frog_position(0), &act_pos);
            let dist = magnitude_f(&frog_v) >> 12;

            if self.babyfrog.baby_list[i].idle > 0 {
                self.babyfrog.baby_list[i].idle -= gs >> 12;
            }

            if dist < BABY_ACTIVE_RADIUS {
                // Play attract animations when frog is close
                make_unit(&mut frog_v);
                let normal = self.curr_tile0_normal();
                let a = &mut self.actor2.actors[baby].actor;
                let q = a.q_rot;
                orientate_fs(&mut a.q_rot, &frog_v, &normal);
                let speed = (gs / 5).min(4090);
                let aim = a.q_rot;
                iquat_slerp(&q, &aim, speed, &mut a.q_rot);

                // Decrease twice as fast if very close
                if dist < BABY_ACTIVE_RADIUS / 2 {
                    self.babyfrog.baby_list[i].idle -= gs >> 12;
                }

                if self.babyfrog.baby_list[i].idle < 1 {
                    if dist < BABY_ACTIVE_RADIUS / 2 {
                        if self.rng.random((dist / 180).max(1) as u32) == 0 {
                            let pitch = 48 + self.rng.random(16);
                            self.play_sample(gen_sfx("babyhappy"), Some(act_pos), 0, SAMPLE_VOLUME, pitch);
                        }

                        self.start_animate_actor(baby, BABY_ANIM_HOP, true, false, 154, false);
                        self.babyfrog.baby_list[i].idle = BABY_IDLE_TIME >> 2;
                    } else {
                        // Randomly play attract animation
                        let anim = self.rng.random(7) + 2;
                        self.actor_animate(baby, anim, false, false, 102, false);
                        self.actor_animate(baby, BABY_ANIM_SNOOZE, true, true, 102, false);
                        self.babyfrog.baby_list[i].idle = BABY_IDLE_TIME + self.rng.random((dist / SCALE) as u32);
                        self.play_sample(gen_sfx("babysad"), Some(act_pos), 0, SAMPLE_VOLUME, -1);
                    }
                }
            } else if dist > BABY_ACTIVE_RADIUS + (BABY_ACTIVE_RADIUS / 2) && self.babyfrog.baby_list[i].idle > 1 {
                // If frog is out of range but the idle timer is still going then try to get him back
                self.actor_animate(baby, BABY_ANIM_TWOWAVE, false, false, 102, false);
                let in_tile = self.babyfrog.baby_list[i].enemy.and_then(|e| self.enemies.enemy_list.array[e.idx()].in_tile);
                let mut up = fvec(&in_tile.map_or(SVector::default(), |t| self.map.tiles[t.idx()].normal));
                scale_vector(&mut up, 20);
                let mut pos = SVector::default();
                add_vector_sfs(&mut pos, &up, &act_pos);
                let up = fvec(&self.curr_tile0_normal());

                if self.game_state.multi == SINGLEPLAYER {
                    self.prep_for_priority_effect();
                    let fx = self.create_special_effect_direct_pending(FXTYPE_CROAK, &pos, &up, 81920, 4096, 410, 6144);
                    if fx.is_some() {
                        let n = self.baby_icon_index(i) as usize;
                        // TODO(port): fx->spin = 25 (specfx state).
                        // BUG(original): babyList[n] with a bad offset2 reads past the array.
                        if let Some(c) = self.babyfrog.baby_list.get(n).map(|b| b.fx_colour) {
                            self.set_fx_colour(fx, c[0], c[1], c[2]);
                        }
                    }
                }

                self.play_sample(gen_sfx("babycry"), None, 0, SAMPLE_VOLUME, -1);
                self.babyfrog.baby_list[i].idle = 0;
            } else {
                self.start_animate_actor(baby, BABY_ANIM_SNOOZE, true, true, 102, false);
            }
            i += 1;
        }
    }

    /// Port of `RunBabyCollect` (Common/babyfrog.c:414): spin and move up.
    fn run_baby_collect(&mut self, bby: usize) {
        let Some(baby) = self.babyfrog.baby_list[bby].baby else {
            return;
        };
        let mut up = fvec(&self.curr_tile0_normal());
        // actFrameCount degrees converted to radians
        let w = fdiv(((self.act_frame_count.wrapping_mul(2) % 360) << 12) as i32, 235930);
        let q = IQuaternion { x: up.vx, y: up.vy, z: up.vz, w };

        let mut fwd = FVector::default();
        fixed_rotate_vector_by_rotation(&mut fwd, &IN_VEC, &q);
        orientate(&mut self.actor2.actors[baby].actor.q_rot, &fwd, &up);

        let b = &mut self.babyfrog.baby_list[bby];
        b.collect -= (self.game_speed >> 12).max(1);

        if b.collect < 1 {
            let act_pos = self.actor2.actors[baby].actor.position;
            let colour = self.babyfrog.baby_list[bby].fx_colour;
            // TODO(port): each fx: rebound->point = currTile[0]->centre; gravity = 2870.
            let fx = self.create_special_effect_direct_pending(FXTYPE_SPARKLYTRAIL, &act_pos, &up, 204800, 12288, 0, 20480);
            if fx.is_some() {
                self.set_fx_colour(fx, colour[0], colour[1], colour[2]);
            }
            let fx = self.create_special_effect_direct_pending(FXTYPE_SPARKLYTRAIL, &act_pos, &up, 163840, 1240, 0, 24576);
            if fx.is_some() {
                self.set_fx_colour(fx, 220, 220, 220);
            }
            let fx = self.create_special_effect_direct_pending(FXTYPE_SPARKLYTRAIL, &act_pos, &up, 122880, 8192, 0, 28672);
            if fx.is_some() {
                self.set_fx_colour(fx, colour[0], colour[1], colour[2]);
            }
            let fx = self.create_special_effect_direct_pending(FXTYPE_SPARKLYTRAIL, &act_pos, &up, 81920, 6144, 0, 32768);
            if fx.is_some() {
                self.set_fx_colour(fx, 220, 220, 220);
            }
            self.actor2.actors[baby].draw = 0;
            self.babyfrog.baby_list[bby].collect = 0;
        }

        scale_vector_ff(&mut up, self.game_speed.wrapping_mul(30));
        // AddToVectorSF(&act->position, &up)
        let p = &mut self.actor2.actors[baby].actor.position;
        p.vx = p.vx.wrapping_add((up.vx >> 12) as i16);
        p.vy = p.vy.wrapping_add((up.vy >> 12) as i16);
        p.vz = p.vz.wrapping_add((up.vz >> 12) as i16);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Game, ActorId) {
        let mut g = Game::new();
        g.game_state.multi = SINGLEPLAYER;
        g.map.num_babies = 2;
        g.init_baby_list(true);
        let a = g.actor2.actors.slots.len();
        g.actor2.actors.slots.push(Some(Default::default()));
        let id = ActorId(a as u32);
        g.actor2.actors[id].draw = 1;
        g.babyfrog.baby_list[0].baby = Some(id);
        (g, id)
    }

    #[test]
    fn pickup_sets_state_once() {
        let (mut g, id) = setup();
        g.act_frame_count = 100;
        g.frogmove.frog_facing[0] = 2;
        assert_eq!(g.pickup_baby_frog(Some(id), Some(TileId(7))), 1);
        let b = &g.babyfrog;
        assert_eq!(b.baby_list[0].is_saved, 1);
        assert_eq!(b.baby_list[0].collect, 40);
        assert_eq!((b.babies_saved, b.last_baby_saved, b.baby_flash_frame), (1, 0, 100));
        assert_eq!(b.baby_icon_a[0], 255);
        assert_eq!(g.map.g_t_start[0], Some(TileId(7)));
        assert_eq!(g.cam.start_frog_facing, 2);
        // Second pickup / unknown baby: FALSE.
        assert_eq!(g.pickup_baby_frog(Some(id), Some(TileId(7))), 0);
        assert_eq!(g.pickup_baby_frog(Some(ActorId(999)), None), 0);
        assert_eq!(g.get_nearest_baby_frog(), -1);
    }

    #[test]
    fn collect_runs_down_and_hides() {
        let (mut g, id) = setup();
        g.pickup_baby_frog(Some(id), None);
        for _ in 0..40 {
            g.update_babies();
        }
        assert_eq!(g.babyfrog.baby_list[0].collect, 0);
        assert_eq!(g.actor2.actors[id].draw, 0);
        g.reset_babies();
        assert_eq!(g.babyfrog.baby_list[0].baby, None);
        assert_eq!(g.babyfrog.last_baby_saved, -1);
    }

    #[test]
    fn far_baby_with_idle_cries() {
        let (mut g, id) = setup();
        g.actor2.actors[id].actor.position = SVector { vx: 5000, vy: 0, vz: 0, pad: 0 };
        g.babyfrog.baby_list[0].idle = 10;
        assert_eq!(g.get_nearest_baby_frog(), 0);
        g.update_babies();
        assert_eq!(g.babyfrog.baby_list[0].idle, 0);
        assert!(g.effects.iter().any(|e| matches!(e, Effect::PlaySample { uid, .. } if *uid == gen_sfx("babycry"))));
    }
}
