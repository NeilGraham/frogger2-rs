//! Port of `Common/actor2.c` / `actor2.h` (game-side actors) together with the PC
//! `ACTOR` struct from `PC/actor.h`.
//!
//! `ACTOR2` and its `ACTOR` live in one arena slot addressed by [`ActorId`]; the
//! `MDX_ACTOR` behind `actor->actualActor` is embedded as [`MdxActor`]. `actList` is
//! a [`Head`] with insertion at the front, exactly like `CreateAndAddActor`. The MDX
//! `actorList` / `actorDrawList[2]` have the same order as `actList` (both are
//! push-front at the same moment), so draw lists are derived by filtering on
//! `MdxActor::list_num` (see [`Game::actor_draw_list`]).
//!
//! Dropped: PSX drawing (`DrawActorList`, `GetHeadMatrix`, FMA/PSI paths) and the
//! commented-out N64 helpers.

use crate::prelude::*;

use f2_math::{
    arccos, fdiv2, fixed_get_quaternion_from_rotation, fixed_quaternion_multiply, fixed_rotate_vector_by_rotation, fmul, fsqrt_f,
    get_quaternion_from_xz_rotation, new_set_vector_fs, rcos, rsin, FVector, Fixed, IQuaternion, SVector,
};
use serde::{Deserialize, Serialize};

use crate::ids::ActorId;
use crate::list::{Head, LinkStore, Links};
use crate::mdx_actor::{init_anims, mdx_flags, object_flags, MdxActor, INIT_SHADOW};
use crate::Game;

// ----- [ ACTOR2 DRAWING FLAGS ] -----
pub const ACTOR_DRAW_NEVER: u16 = 1 << 0;
pub const ACTOR_DRAW_CULLED: u16 = 1 << 1;
pub const ACTOR_DRAW_ALWAYS: u16 = 1 << 2;
pub const ACTOR_DRAW_LAST: u16 = 1 << 3;
pub const ACTOR_WATER: u16 = 1 << 4;
pub const ACTOR_SLIDYTEX: u16 = 1 << 5;
pub const ACTOR_SLIDYTEX2: u16 = 1 << 6;
pub const ACTOR_MODGETEX: u16 = 1 << 7;
pub const ACTOR_ADDITIVE: u16 = 1 << 8;
pub const ACTOR_SLUDGE: u16 = 1 << 9;
pub const ACTOR_LEAVES: u16 = 1 << 10;
pub const ACTOR_FLOWING: u16 = 1 << 11;
pub const ACTOR_SLOWSLIDE: u16 = 1 << 12;
pub const ACTOR_NEW_NOREORIENTATE: u16 = 1 << 13;
pub const ACTOR_NOANIMATION: u16 = 1 << 14;
pub const LOOKAT_ANYWHERE: i32 = 1;
pub const LOOKAT_2D: i32 = 2;

/// `ACTOR_DRAWDISTANCEINNER` / `OUTER` (`actor2.c:68`).
pub const ACTOR_DRAWDISTANCEINNER: Fixed = 12288000;
pub const ACTOR_DRAWDISTANCEOUTER: Fixed = 16384000;

/// `ACTOR_SHADOW` (`Common/types.h:313`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorShadow {
    pub radius: Fixed,
    pub alpha: u8,
    pub alpha_aim: u8,
    pub alpha_aim_speed: u8,
    pub draw: i8,
}

/// `ACTOR` (`PC/actor.h:52`). The PSX-style `ACTOR_ANIMATION animation` member is
/// unused on PC (animation lives in the MDX actor) and is not kept.
///
/// Phase 3S: `repr(C)` with the fields the per-tick passes touch first (D-cache lines on
/// the N64); `Serialize`/`Deserialize` keep the original field order (`ser_order!`).
#[derive(Clone, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Actor {
    pub position: SVector,
    pub size: FVector,
    pub q_rot: IQuaternion,
    /// `actualActor`.
    pub actual_actor: MdxActor,
    pub vel: FVector,
    pub visible: i32,
    pub old_position: SVector,
    pub rotaim: SVector,
    pub shadow: Option<ActorShadow>,
    /// The (rewritten, lower-case) model name used to create the actor.
    pub name: String,
}

crate::ser_order!(Actor, "Actor", ActorWire {
    visible: i32,
    old_position: SVector,
    position: SVector,
    rotaim: SVector,
    size: FVector,
    vel: FVector,
    shadow: Option<ActorShadow>,
    q_rot: IQuaternion,
    actual_actor: MdxActor,
    name: String,
});

/// `ACTOR2` (`actor2.h:60`).
///
/// Phase 3S: `repr(C)` with the hot fields first (see [`Actor`]); serde keeps the original
/// field order.
#[derive(Clone, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Actor2 {
    pub links: Links<ActorId>,
    pub distance_from_frog: Fixed,
    pub draw: i8,
    pub clipped: i8,
    pub miles_away: i8,
    pub depth_shift: i8,
    pub actor: Actor,
    pub flags: u16,
    pub dff_clipping: i8,
    pub effects: u32,
    pub fx_count: u32,
    pub radius: Fixed,
    pub anim_speed: Fixed,
    pub value1: Fixed,
    /// `((MDX_ACTOR*)actualActor)->overrideTex` as a texture name (baby frogs).
    pub override_tex: Option<String>,
    /// `obj->sprites[i].sprite` of the actor's own object tree (`MakeUniqueActor` copies the
    /// objects per actor), in `AddObjectsSpritesToSpriteList` order, with the model id they
    /// belong to. `TransformObject` positions them (`view.rs`).
    pub obj_sprites: Option<(u32, Vec<Option<crate::ids::SpriteId>>)>,
}

crate::ser_order!(Actor2, "Actor2", Actor2Wire {
    actor: Actor,
    links: Links<ActorId>,
    draw: i8,
    flags: u16,
    effects: u32,
    fx_count: u32,
    radius: Fixed,
    anim_speed: Fixed,
    value1: Fixed,
    distance_from_frog: Fixed,
    override_tex: Option<String>,
    clipped: i8,
    miles_away: i8,
    depth_shift: i8,
    dff_clipping: i8,
    #[serde(default)]
    obj_sprites: Option<(u32, Vec<Option<crate::ids::SpriteId>>)>,
});

/// Arena of actors; freed slots stay `None` until the next level.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Actors {
    pub slots: Vec<Option<Actor2>>,
}

impl Actors {
    pub fn get(&self, id: ActorId) -> Option<&Actor2> {
        self.slots.get(id.idx()).and_then(|s| s.as_ref())
    }
    pub fn get_mut(&mut self, id: ActorId) -> Option<&mut Actor2> {
        self.slots.get_mut(id.idx()).and_then(|s| s.as_mut())
    }
}

impl core::ops::Index<ActorId> for Actors {
    type Output = Actor2;
    fn index(&self, id: ActorId) -> &Actor2 {
        self.get(id).expect("freed actor")
    }
}

impl core::ops::IndexMut<ActorId> for Actors {
    fn index_mut(&mut self, id: ActorId) -> &mut Actor2 {
        self.get_mut(id).expect("freed actor")
    }
}

impl LinkStore<ActorId> for Actors {
    fn links(&self, id: ActorId) -> &Links<ActorId> {
        &self[id].links
    }
    fn links_mut(&mut self, id: ActorId) -> &mut Links<ActorId> {
        &mut self[id].links
    }
}

/// Globals defined in `actor2.c` (plus `backGnd` from `game.c`, set by
/// `CreateLevelObjects`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Actor2State {
    pub actors: Actors,
    /// `actList`.
    pub act_list: Head<ActorId>,
    /// `globalLevelActor`.
    pub global_level_actor: Option<ActorId>,
    /// `actorsCount`.
    pub actors_count: i32,
    /// `objectMatrix`.
    pub object_matrix: i32,
    /// `backGnd` (`Common/game.c:101`).
    pub back_gnd: Option<ActorId>,
}

/// `strnicmp(a, b, n) == 0` on byte strings.
fn strnieq(a: &[u8], b: &[u8]) -> bool {
    let n = b.len();
    a.len() >= n && a[..n].eq_ignore_ascii_case(b)
}

/// The name rewrite of `CreateAndAddActor` (PC): the three bytes after the first `.`
/// become `obe` (anything after them is kept), and `NULL_*` becomes `nothing.obe`.
pub fn actor_model_name(name: &str) -> String {
    let mut b: Vec<u8> = name.bytes().take_while(|&c| c != 0).take(31).collect();
    // Original: `for (i=0; newname[i]!='.'; i++);` runs past the string when there is
    // no dot (undefined behaviour); the port treats the end of the string as the dot.
    let i = b.iter().position(|&c| c == b'.').unwrap_or_else(|| {
        b.push(b'.');
        b.len() - 1
    });
    while b.len() < i + 4 {
        b.push(0);
    }
    b[i + 1] = b'o';
    b[i + 2] = b'b';
    b[i + 3] = b'e';
    if strnieq(&b, b"NULL_") {
        return "nothing.obe".into();
    }
    String::from_utf8_lossy(&b).into_owned()
}

/// Port of `Orientate` (Common/actor2.c:1038): builds a quaternion that points the
/// actor's z axis along `fd` with `up` as the up vector.
#[cfg_attr(target_vendor = "nintendo64", allow(unsafe_code), link_section = ".text.f2hot_e")]
pub fn orientate(me: &mut IQuaternion, fd: &FVector, up: &FVector) {
    let mut r1 = IQuaternion::default();
    let mut q1 = IQuaternion::default();
    let mut q2 = IQuaternion::default();
    let mut v1 = FVector::default();
    let mut q1_unimportant = false;
    let mut q1_flip = false;

    let dp = up.vy;

    if dp < 4055 {
        if dp > -4055 {
            r1.x = up.vz.wrapping_neg();
            r1.y = 0;
            r1.z = up.vx;
            r1.w = arccos(dp);

            // Normalise it (only need to do 2 coords!)
            let scl = fdiv2(4096, fsqrt_f(fmul(r1.x, r1.x).wrapping_add(fmul(r1.z, r1.z))));
            r1.x = fmul(r1.x, scl);
            r1.z = fmul(r1.z, scl);

            fixed_rotate_vector_by_rotation(&mut v1, fd, &r1);

            r1.w = r1.w.wrapping_neg();

            get_quaternion_from_xz_rotation(&mut q1, &r1);
        } else {
            // We're just fliping the X&Y coords!
            v1.vx = fd.vx;
            v1.vy = 0;
            v1.vz = fd.vz.wrapping_neg();
            q1.w = 0;
            q1.y = 0;
            q1.z = 0;
            q1.x = 4096;
            q1_flip = true; // We don't need the qMult later on!
        }
    } else {
        v1.vx = fd.vx;
        v1.vy = 0;
        v1.vz = fd.vz;
        q1 = IQuaternion { x: 0, y: 0, z: 0, w: 4096 };
        q1_unimportant = true;
    }

    // Again, makeunit from the two used values
    let scl = fdiv2(4096, fsqrt_f(fmul(v1.vx, v1.vx).wrapping_add(fmul(v1.vz, v1.vz))));
    let dp2 = fmul(v1.vz, scl);

    if dp2 < 4055 {
        if dp2 > -4055 {
            if q1_unimportant {
                let theta_over2 = if v1.vx > 0 { arccos(dp2) / 2 } else { arccos(dp2) / -2 };
                let sin_theta_over2 = rsin(theta_over2);
                me.x = 0;
                me.z = 0;
                me.w = rcos(theta_over2);
                me.y = sin_theta_over2;
            } else {
                r1.x = up.vx;
                r1.y = up.vy;
                r1.z = up.vz;
                r1.w = if v1.vx > 0 { arccos(dp2) } else { arccos(dp2).wrapping_neg() };

                fixed_get_quaternion_from_rotation(&mut q2, &r1);

                if q1_flip {
                    me.w = q2.x.wrapping_neg();
                    me.x = q2.w;
                    me.y = q2.z;
                    me.z = q2.y.wrapping_neg();
                } else {
                    fixed_quaternion_multiply(me, &q2, &q1);
                }
            }
        } else {
            // This could be more optimal!
            q2.w = 0;
            q2.x = up.vx.wrapping_neg();
            q2.y = up.vy.wrapping_neg();
            q2.z = up.vz.wrapping_neg();

            if q1_unimportant {
                *me = q2;
            } else if q1_flip {
                me.w = q2.x.wrapping_neg();
                me.x = q2.w;
                me.y = q2.z;
                me.z = q2.y.wrapping_neg();
            } else {
                // QuatMult of a w = 0 quaternion.
                me.w = fmul(q2.x, q1.x).wrapping_neg().wrapping_sub(fmul(q2.y, q1.y)).wrapping_sub(fmul(q2.z, q1.z));
                me.x = fmul(q2.x, q1.w).wrapping_add(fmul(q2.y, q1.z)).wrapping_sub(fmul(q2.z, q1.y));
                me.y = fmul(q2.y, q1.w).wrapping_add(fmul(q2.z, q1.x)).wrapping_sub(fmul(q2.x, q1.z));
                me.z = fmul(q2.z, q1.w).wrapping_add(fmul(q2.x, q1.y)).wrapping_sub(fmul(q2.y, q1.x));
            }
        }
    } else {
        // No change, hence no need for qmult.
        *me = q1;
    }
}

/// Port of `OrientateSS` (Common/actor2.c:1016).
pub fn orientate_ss(me: &mut IQuaternion, fd: &SVector, up: &SVector) {
    let (mut f, mut g) = (FVector::default(), FVector::default());
    new_set_vector_fs(&mut f, fd);
    new_set_vector_fs(&mut g, up);
    orientate(me, &f, &g);
}

/// Port of `OrientateFS` (Common/actor2.c:1024).
pub fn orientate_fs(me: &mut IQuaternion, fd: &FVector, up: &SVector) {
    let mut f = FVector::default();
    new_set_vector_fs(&mut f, up);
    orientate(me, fd, &f);
}

/// Port of `OrientateSF` (Common/actor2.c:1031).
pub fn orientate_sf(me: &mut IQuaternion, fd: &SVector, up: &FVector) {
    let mut f = FVector::default();
    new_set_vector_fs(&mut f, fd);
    orientate(me, &f, up);
}

impl Game {
    /// Port of `CreateAndAddActor` (Common/actor2.c:470), PC branch, including
    /// `CreateActor`/`InitActor`/`InitActorStructures` (`MDX Library/mdxActor.cpp`)
    /// for the state gameplay sees. Returns `None` when the model is not in any
    /// loaded object bank.
    pub fn create_and_add_actor(&mut self, name: &str, cx: i16, cy: i16, cz: i16, init_flags: i32) -> Option<ActorId> {
        let newname = actor_model_name(name);

        // CreateActor(newname, INIT_ANIMATION)
        let model = self.models.find_object(&newname)?.clone();
        let mut mdx = MdxActor {
            object_controller: Some(model.id),
            lod_object_controller: None,
            flags: mdx_flags::ACTOR_NOFLAGS,
            list_num: 0,
            // RestoreObjectPointers clears these bits when a bank is loaded.
            root_object_flags: model.root_flags() & !(object_flags::OBJECT_FLAGS_MODGE | object_flags::OBJECT_FLAGS_WAVE | object_flags::OBJECT_FLAGS_ADDITIVE),
            animation: Some(init_anims(&model)),
            placement: Default::default(),
        };

        let mut item = Actor2::default();

        // AddObjectsSpritesToSpriteList(object, 0) (PC/pcsprite.c:61): one pool sprite per
        // object sprite (never freed before the level ends).
        let mut obj_sprites = Vec::with_capacity(model.obj_sprites.len());
        for &(tex, xlu) in &model.obj_sprites {
            let sid = self.sprite.allocate_sprites(1);
            obj_sprites.push(sid);
            if let Some(s) = sid {
                let sp = self.sprite.get_mut(s);
                sp.texture = tex;
                sp.pos = SVector::default();
                sp.r = 255;
                sp.g = 255;
                sp.b = 255;
                sp.a = 128;
                sp.flags = if xlu < 255 { crate::sprite::SPRITE_TRANSLUCENT } else { 0 };
                sp.offset_x = -16;
                sp.offset_y = -16;
            }
        }

        // Cue sounds from animations
        let (mapping, ambient) = self.general.find_sfx_mapping(model.id);
        if let Some(anim) = mdx.animation.as_mut() {
            anim.loop_flags = [0; 5];
            anim.sfx_mapping = mapping;
        }
        // ambient: AddAmbientSound for a scenic record is made below, once the actor id exists.

        // null object - do not display
        let nb = newname.as_bytes();
        item.draw = if strnieq(nb, b"TRANSOBJ") || strnieq(nb, b"NOTHING") || strnieq(nb, b"NULL_") { 0 } else { 1 };

        let newname = crate::general::gstrlwr(&newname);
        let n = newname.as_bytes();
        let at = |i: usize| n.get(i).copied().unwrap_or(0);

        if at(0) == b'a' && at(1) == b'd' {
            mdx.flags |= mdx_flags::ACTOR_ALWAYSDRAW;
            item.flags |= ACTOR_DRAW_ALWAYS;
            if at(2) == b'a' {
                item.flags |= ACTOR_ADDITIVE;
                mdx.root_object_flags |= object_flags::OBJECT_FLAGS_ADDITIVE;
            }
        } else {
            item.flags |= ACTOR_DRAW_CULLED;
        }

        // Yes, seriously, this is the check that has been done forever and breaks if you change it
        if at(2) == b'_' && at(3) == b'g' {
            mdx.flags |= mdx_flags::ACTOR_SLIDY;
            mdx.root_object_flags |= object_flags::OBJECT_FLAGS_MODGE;
            item.flags |= ACTOR_MODGETEX;
            if at(4) == b'_' {
                item.flags |= ACTOR_SLIDYTEX;
            }
        }

        if at(0) == b's' && at(1) == b'p' {
            match at(2) {
                b'l' => {
                    item.flags |= ACTOR_SLIDYTEX;
                    mdx.flags |= mdx_flags::ACTOR_WRAPTC | mdx_flags::ACTOR_SLIDY;
                }
                b'm' => {
                    item.flags |= ACTOR_SLIDYTEX2;
                    mdx.flags |= mdx_flags::ACTOR_WRAPTC | mdx_flags::ACTOR_SLIDY;
                }
                b'f' => {
                    item.flags |= ACTOR_SLIDYTEX | ACTOR_SLIDYTEX2;
                    mdx.flags |= mdx_flags::ACTOR_WRAPTC | mdx_flags::ACTOR_SLIDY;
                }
                _ => {}
            }
        }

        // AddActorToList(tAct, 0|1)
        mdx.list_num = if (init_flags & INIT_SHADOW) != 0 || (item.flags & ACTOR_ADDITIVE) != 0 || strnieq(n, b"shadow") { 1 } else { 0 };

        // Setup a shadow
        if init_flags & INIT_SHADOW != 0 {
            item.actor.shadow = Some(ActorShadow { alpha: 255, draw: 1, ..Default::default() });
        }

        item.actor.old_position = SVector { vx: cx, vy: cy, vz: cz, pad: 0 };
        item.actor.size = FVector { vx: 4096, vy: 4096, vz: 4096 };
        item.flags |= ACTOR_DRAW_CULLED;
        item.radius = 0;
        item.anim_speed = 4096;
        item.value1 = 0;
        item.effects = 0;
        item.distance_from_frog = 0;
        item.actor.position = SVector { vx: cx, vy: cy, vz: cz, pad: 0 };
        item.clipped = 0;
        item.miles_away = 0;
        item.actor.actual_actor = mdx;
        item.actor.name = newname;

        let id = ActorId(self.actor2.actors.slots.len() as u32);
        self.actor2.actors.slots.push(Some(item));
        let a2 = &mut self.actor2;
        a2.act_list.push_front(&mut a2.actors, id);
        // ambient: FindSfxMapping type 3 -> AddAmbientSound(sample, &actor->position (still
        // zero there), 12000, AMBIENT_VOLUME, -1, 0, 0, act).
        if let Some(uid) = ambient {
            self.add_ambient_sound(uid, None, 12000, crate::pcaudio::AMBIENT_VOLUME as i16, -1, 0, 0, Some(id));
        }
        // CreateActor: XformActor(t, 1) at the origin (initial OBJECT_FLAGS_CLIPPED). It runs
        // before AddObjectsSpritesToSpriteList: the unique objects' sprite pointers are NULL.
        self.xform_new_actor(id);
        if !obj_sprites.is_empty() {
            self.actor2.actors[id].obj_sprites = Some((model.id, obj_sprites));
        }
        Some(id)
    }

    /// Port of `actor2Free` (Common/actor2.c:1200) (+ `actorFree`/`FreeActor`).
    pub fn actor2_free(&mut self, id: ActorId) {
        if self.actor2.actors.get(id).is_none() {
            return;
        }
        let a2 = &mut self.actor2;
        a2.act_list.remove(&mut a2.actors, id);
        a2.actors.slots[id.idx()] = None;
        for r in [&mut a2.global_level_actor, &mut a2.back_gnd] {
            if *r == Some(id) {
                *r = None;
            }
        }
    }

    /// Port of `FreeActorList` (Common/actor2.c:405).
    pub fn free_actor_list(&mut self) {
        self.actor2.actors.slots.clear();
        self.actor2.act_list = Head::default();
        self.actor2.global_level_actor = None;
        self.actor2.back_gnd = None;
    }

    /// Actors of `actorDrawList[list]` in draw order (= `actList` order).
    pub fn actor_draw_list(&self, list: u32) -> Vec<ActorId> {
        self.actor2.act_list.ids(&self.actor2.actors).into_iter().filter(|&id| self.actor2.actors[id].actor.actual_actor.list_num == list).collect()
    }

    // TeleportActorToTile (Common/actor2.c:1230) is ported in evenfunc.rs.
    // ResetUniqueActorList: declared only, no PC definition (nothing to port).
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_names() {
        assert_eq!(actor_model_name("logaa.psi"), "logaa.obe");
        assert_eq!(actor_model_name("Wat_x.obe"), "Wat_x.obe");
        assert_eq!(actor_model_name("null_trig.psi"), "nothing.obe");
        assert_eq!(actor_model_name("ab.p"), "ab.obe");
        assert_eq!(actor_model_name("abc"), "abc.obe");
    }

    fn fv(x: i32, y: i32, z: i32) -> FVector {
        FVector { vx: x, vy: y, vz: z }
    }

    #[test]
    fn orientate_flat_up() {
        let mut q = IQuaternion::default();
        // facing +z with up +y: identity
        orientate(&mut q, &fv(0, 0, 4096), &fv(0, 4096, 0));
        assert_eq!(q, IQuaternion { x: 0, y: 0, z: 0, w: 4096 });
        // facing +x: rotate about y by a quarter turn (half angle 512 of 4096)
        orientate(&mut q, &fv(4096, 0, 0), &fv(0, 4096, 0));
        assert_eq!((q.x, q.z), (0, 0));
        assert!((q.y - 2896).abs() <= 8 && (q.w - 2896).abs() <= 8, "{q:?}");
        // facing -z: w = 0 branch, q = -up
        orientate(&mut q, &fv(0, 0, -4096), &fv(0, 4096, 0));
        assert_eq!(q, IQuaternion { x: 0, y: -4096, z: 0, w: 0 });
    }

    #[test]
    fn orientate_tilted_and_upside_down_are_unit() {
        let mut q = IQuaternion::default();
        for (fd, up) in [(fv(0, 0, 4096), fv(4096, 0, 0)), (fv(4096, 0, 0), fv(0, -4096, 0)), (fv(0, 2896, 2896), fv(0, 2896, -2896))] {
            orientate(&mut q, &fd, &up);
            let m = (q.x as i64).pow(2) + (q.y as i64).pow(2) + (q.z as i64).pow(2) + (q.w as i64).pow(2);
            let m = (m as f64).sqrt();
            assert!((m - 4096.0).abs() < 40.0, "{fd:?} {up:?} -> {q:?}");
        }
    }
}
