//! Actor hierarchy transforms: port of `XformActor` (mdxActor.cpp:547),
//! `TransformObject` (mdxObject.cpp:371) and the keyframe search
//! `FindToFromVKeys`/`FindToFromQKeys` (mdxObject.cpp:229/305).

use crate::math::*;
use f2_formats::objbank::{Model, Object, QKey, VKey};

/// `ANIMSNAP_THRESHHOLD` (mdxObject.cpp:41).
pub const ANIMSNAP_THRESHHOLD: f32 = 0.25;
/// `ROTATESNAP_MULT` (mdxObject.cpp:42).
pub const ROTATESNAP_MULT: f32 = 2.0;

/// The parts of `MDX_ACTOR` the renderer needs.
#[derive(Clone, Debug)]
pub struct Actor {
    pub pos: Vec3,
    /// Orientation quaternion `[x, y, z, w]`.
    pub q_rot: Quat,
    /// `actor->scale`.
    pub scale: Vec3,
    /// `animation->animTime` (key time units, i.e. game frames × speed).
    pub anim_time: f32,
    /// Whether keyframes are interpolated (`currentDrawActor->animation` non-null);
    /// otherwise key 0 is used everywhere.
    pub animated: bool,
    /// `xluOverride / 100` (1.0 = opaque). Not used by the PC object path
    /// (`PCRenderObject` ignores `globalXLU2`); values < 1 are an enhancement that
    /// fades the actor through the XLU frame.
    pub xlu: f32,
    /// OR-ed into every object's flags at draw time (the game sets
    /// e.g. `OBJECT_FLAGS_ADDITIVE` at run time; the loader clears it).
    pub extra_flags: i16,
    /// `lastMKey`, `lastSKey`, `lastRKey` per object (pre-order), search hints.
    pub last_keys: Vec<[i16; 3]>,
}

impl Default for Actor {
    fn default() -> Self {
        Actor { pos: [0.0; 3], q_rot: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3], anim_time: 0.0, animated: true, xlu: 1.0, extra_flags: 0, last_keys: Vec::new() }
    }
}

impl Actor {
    pub fn at(pos: Vec3) -> Actor {
        Actor { pos, ..Default::default() }
    }
}

trait KeyTime {
    fn time(&self) -> i32;
}
impl KeyTime for VKey {
    fn time(&self) -> i32 {
        self.time
    }
}
impl KeyTime for QKey {
    fn time(&self) -> i32 {
        self.time
    }
}

/// Port of `FindToFromVKeys`/`FindToFromQKeys` (identical bodies).
/// Returns `(from, to, interp)`.
fn find_to_from<K: KeyTime>(keys: &[K], time: f32, e_key: i16) -> (usize, usize, f32) {
    let num_keys = keys.len() as i32;
    let t = |i: i32| keys[i.clamp(0, num_keys - 1) as usize].time() as f32;
    let mut low = e_key as i32;
    // BUG(original): when eKey is the last key, keys[eKey+1] is read out of
    // bounds; we clamp the read to the last key.
    let mut high = e_key as i32 + 1;
    if t(high) < time {
        high = num_keys - 1;
    }
    if t(low) > time {
        low = 0;
    }
    let mut from_key: i32 = -1;
    let mut to_key: i32 = 0;
    while low <= high {
        let i = (low + high) >> 1;
        if t(i) == time {
            if i == num_keys - 1 {
                from_key = -2;
            } else {
                from_key = i;
                to_key = i + 1;
            }
            break;
        } else if t(i) > time {
            high = i - 1;
        } else {
            low = i + 1;
        }
    }
    if from_key == -1 && high + 1 < num_keys && t(high + 1) > time {
        from_key = high;
        to_key = high + 1;
    }
    let interp;
    if from_key >= 0 {
        let temp = (keys[to_key as usize].time().wrapping_sub(keys[from_key as usize].time())).unsigned_abs() as f32;
        interp = (time - keys[from_key as usize].time() as f32) / temp;
    } else {
        from_key = num_keys - 1;
        to_key = from_key;
        interp = 1.0;
    }
    (from_key as usize, to_key as usize, interp)
}

fn count(o: &Object) -> usize {
    1 + o.children.iter().map(count).sum::<usize>()
}

/// Number of objects in a model hierarchy (pre-order indices `0..n`).
pub fn object_count(model: &Model) -> usize {
    count(&model.root) + model.root_siblings.iter().map(count).sum::<usize>()
}

/// Visits every object in pre-order (the order `TransformObject`/`DrawObject` use).
pub fn for_each_object<'a>(model: &'a Model, mut f: impl FnMut(usize, &'a Object)) {
    fn walk<'a>(o: &'a Object, idx: &mut usize, f: &mut dyn FnMut(usize, &'a Object)) {
        f(*idx, o);
        *idx += 1;
        for c in &o.children {
            walk(c, idx, f);
        }
    }
    let mut i = 0;
    walk(&model.root, &mut i, &mut f);
    for s in &model.root_siblings {
        walk(s, &mut i, &mut f);
    }
}

struct Ctx<'a> {
    actor_scale: Vec3,
    animated: bool,
    time: f32,
    last: &'a mut [[i16; 3]],
    out: &'a mut [Mat4],
    idx: usize,
}

/// Port of `TransformObject` (mdxObject.cpp:371) for one object and its subtree.
/// `top` is the matrix-stack top on entry, `parent_scale` = `parentScaleStack[level]`.
fn transform_object(o: &Object, top: &Mat4, parent_scale: Vec3, c: &mut Ctx) {
    let me = c.idx;
    c.idx += 1;
    let (a, time) = (c.actor_scale, c.time);

    let translation = if o.move_keys.len() > 1 && c.animated {
        let (f, t, i) = find_to_from(&o.move_keys, time, c.last[me][0]);
        c.last[me][0] = f as i16;
        if (o.move_keys[t].time as f32 - time) < ANIMSNAP_THRESHHOLD {
            o.move_keys[t].v
        } else {
            linear_interp(&o.move_keys[f].v, &o.move_keys[t].v, i)
        }
    } else {
        o.move_keys.first().map_or([0.0; 3], |k| k.v)
    };

    let scale = if o.scale_keys.len() > 1 && c.animated {
        let (f, t, i) = find_to_from(&o.scale_keys, time, c.last[me][1]);
        c.last[me][1] = f as i16;
        if (o.scale_keys[t].time as f32 - time) < ANIMSNAP_THRESHHOLD {
            o.scale_keys[t].v
        } else {
            linear_interp(&o.scale_keys[f].v, &o.scale_keys[t].v, i)
        }
    } else {
        o.scale_keys.first().map_or([1.0; 3], |k| k.v)
    };

    let mut rotmat = if o.rotate_keys.len() > 1 && c.animated {
        let (f, t, i) = find_to_from(&o.rotate_keys, time, c.last[me][2]);
        c.last[me][2] = f as i16;
        if (o.rotate_keys[t].time as f32 - time) < ANIMSNAP_THRESHHOLD * ROTATESNAP_MULT {
            quaternion_to_matrix(&o.rotate_keys[t].q)
        } else {
            quaternion_to_matrix(&quat_slerp(&o.rotate_keys[f].q, &o.rotate_keys[t].q, i))
        }
    } else {
        quaternion_to_matrix(&o.rotate_keys.first().map_or([0.0, 0.0, 0.0, 1.0], |k| k.q))
    };

    rotmat[3][0] = translation[0] * a[0] * parent_scale[0];
    rotmat[3][1] = translation[1] * a[1] * parent_scale[1];
    rotmat[3][2] = translation[2] * a[2] * parent_scale[2];

    let m1 = mtx_cat(&rotmat, top);
    let scalemat = gu_scale(scale[0] * a[0], scale[1] * a[1], scale[2] * a[2]);
    c.out[me] = mtx_cat(&scalemat, &m1);

    // Children see the rotate/translate matrix only; the scale reaches them via
    // `parentScaleStack` (applied to their translation).
    for ch in &o.children {
        transform_object(ch, &m1, scale, c);
    }
}

/// Port of `XformActor` (mdxActor.cpp:547): returns `objMatrix` for every object in pre-order.
pub fn xform_actor(model: &Model, actor: &mut Actor) -> Vec<Mat4> {
    let n = object_count(model);
    if actor.last_keys.len() != n {
        actor.last_keys = vec![[0; 3]; n];
    }
    let mut out = vec![IDENTITY; n];
    let transmat = gu_translate(actor.pos[0], actor.pos[1], actor.pos[2]);
    let top = mtx_cat(&quaternion_to_matrix(&actor.q_rot), &mtx_cat(&transmat, &IDENTITY));
    let animated = actor.animated && !model.animations.is_empty();
    let mut c = Ctx { actor_scale: actor.scale, animated, time: actor.anim_time, last: &mut actor.last_keys, out: &mut out, idx: 0 };
    transform_object(&model.root, &top, [1.0; 3], &mut c);
    // `TransformObject` recurses on `obj->next` with the same matrix-stack top.
    for s in &model.root_siblings {
        transform_object(s, &top, [1.0; 3], &mut c);
    }
    out
}

/// World-space bounding box `(min, max)` of every mesh vertex of a posed actor
/// (skinned models: the root mesh as deformed by the bones).
pub fn actor_bounds(model: &Model, actor: &mut Actor) -> Option<(Vec3, Vec3)> {
    let mats = xform_actor(model, actor);
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    let mut add = |p: Vec3| {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    };
    if model.is_skinned {
        if let Some(mesh) = &model.root.mesh {
            for_each_object(model, |i, o| {
                for &j in &o.effected_verts {
                    if let Some(v) = mesh.vertices.get(j as usize) {
                        add(mtx_xfm(&mats[i], *v));
                    }
                }
            });
        }
    } else {
        for_each_object(model, |i, o| {
            if let Some(m) = &o.mesh {
                for v in &m.vertices {
                    add(mtx_xfm(&mats[i], *v));
                }
            }
        });
    }
    (lo[0] <= hi[0]).then_some((lo, hi))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vk(t: i32, x: f32) -> VKey {
        VKey { v: [x, 0.0, 0.0], time: t }
    }

    #[test]
    fn key_search_matches_original() {
        let keys = [vk(0, 0.0), vk(10, 1.0), vk(20, 2.0)];
        assert_eq!(find_to_from(&keys, 5.0, 0), (0, 1, 0.5));
        assert_eq!(find_to_from(&keys, 15.0, 0), (1, 2, 0.5));
        assert_eq!(find_to_from(&keys, 10.0, 0), (1, 2, 0.0));
        // Exactly on / past the last key → last key.
        assert_eq!(find_to_from(&keys, 20.0, 1), (2, 2, 1.0));
        assert_eq!(find_to_from(&keys, 25.0, 1), (2, 2, 1.0));
        // Before the first key wraps to the last key (original behaviour).
        assert_eq!(find_to_from(&keys, -1.0, 0), (2, 2, 1.0));
    }
}
