//! Actor hierarchy transforms: port of `XformActor` (mdxActor.cpp:547),
//! `TransformObject` (mdxObject.cpp:371) and the keyframe search
//! `FindToFromVKeys`/`FindToFromQKeys` (mdxObject.cpp:229/305).

#[cfg(not(feature = "std"))]
#[allow(unused_imports)]
use crate::fmath::FloatExt;
use crate::prelude::*;

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
    /// `MDX_ACTOR.overrideTex`: every face uses this image instead of its own
    /// (`PCRenderObject`, mdxRender.cpp:2106).
    pub override_tex: Option<crate::TexHandle>,
    /// `SetObjectGouraudValues(r, g, b)` (0.0..=1.0): replaces the vertex colours' RGB
    /// (the original writes the mesh's `gouraudColors`).
    pub gouraud: Option<[f32; 3]>,
    /// `ACTOR_WRAPTC` (`wrapCoords`, mdxActor.cpp:195): the objects go to the wrapping frame
    /// (`MA_FRAME_WRAP`), whose texture coordinates repeat (sliding textures).
    pub wrap_tc: bool,
    /// `timeInfo.frameCount` for `PCCalcModgeValues` (the water/sludge wobble and wave of
    /// `OBJECT_FLAGS_MODGE` / `OBJECT_FLAGS_WAVE` objects).
    pub modge_frame: u32,
    /// The last [`xform_actor`] result and its inputs ([`xform_actor_cached`]: mesh-sink
    /// back ends skip the hierarchy walk while the pose does not change).
    pub pose: PoseCache,
    /// The recorded path's draws of the model's objects that have a mesh, in pre-order
    /// (`((model id, extra_flags, xlu fade, wrap_tc), draws)`): walked instead of the object
    /// tree every frame (same objects, same order, same frames).
    pub mesh_draws: Option<((u32, i16, bool, bool), Vec<crate::scene::ObjDraw>)>,
}

/// Inputs and result of the last [`xform_actor_cached`] of an [`Actor`].
#[derive(Clone, Debug, Default)]
pub struct PoseCache {
    /// Changes whenever `mats` change (unique across actors, never 0 once computed): a back
    /// end can reuse what it derived from matrices with the same stamp.
    pub stamp: u32,
    key: Option<PoseKey>,
    /// The hierarchy with an identity matrix-stack top (depends on the model, scale and
    /// animation time only) and its inputs.
    local_key: Option<LocalKey>,
    local: Vec<Mat4>,
    /// `local` times the actor rotation `rot_q` (row 3 without the translation).
    rot_q: Option<Quat>,
    /// The rotation of `mats`.
    last_q: Option<Quat>,
    rotl: Vec<Mat4>,
    pub mats: Vec<Mat4>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct LocalKey {
    model: u32,
    scale: Vec3,
    anim_time: f32,
    animated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PoseKey {
    model: u32,
    pos: Vec3,
    q_rot: Quat,
    scale: Vec3,
    anim_time: f32,
    animated: bool,
}

impl Default for Actor {
    fn default() -> Self {
        Actor {
            pos: [0.0; 3],
            q_rot: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
            anim_time: 0.0,
            animated: true,
            xlu: 1.0,
            extra_flags: 0,
            last_keys: Vec::new(),
            override_tex: None,
            gouraud: None,
            wrap_tc: false,
            modge_frame: 0,
            pose: PoseCache::default(),
            mesh_draws: None,
        }
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

/// [`find_to_from`] with its common case first: `time` strictly between the hinted key and
/// the next one. The original's search then ends with the same keys (it starts with
/// `low = e_key`, `high = e_key + 1` and only compares those two times), so the result is
/// identical for any key array.
#[inline(always)]
fn key_search<K: KeyTime>(keys: &[K], time: f32, e_key: i16) -> (usize, usize, f32) {
    let h = e_key as usize;
    if let (Some(a), Some(b)) = (keys.get(h), keys.get(h.wrapping_add(1))) {
        let (ta, tb) = (a.time(), b.time());
        if (ta as f32) < time && time < tb as f32 {
            let temp = (tb.wrapping_sub(ta)).unsigned_abs() as f32;
            return (h, h + 1, (time - ta as f32) / temp);
        }
    }
    find_to_from(keys, time, e_key)
}

fn count(o: &Object) -> usize {
    1 + o.children.iter().map(count).sum::<usize>()
}

/// Number of objects in a model hierarchy (pre-order indices `0..n`).
pub fn object_count(model: &Model) -> usize {
    count(&model.root) + model.root_siblings.iter().map(count).sum::<usize>()
}

/// Visits every object in pre-order (the order `TransformObject`/`DrawObject` use).
#[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_d")]
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
        let (f, t, i) = key_search(&o.move_keys, time, c.last[me][0]);
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
        let (f, t, i) = key_search(&o.scale_keys, time, c.last[me][1]);
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
        let (f, t, i) = key_search(&o.rotate_keys, time, c.last[me][2]);
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
    let s = [scale[0] * a[0], scale[1] * a[1], scale[2] * a[2]];
    // Console fast path: a unit scale matrix leaves `m1` unchanged, any other scales its
    // rows (both up to the sign of zeros: `mtx_cat` adds the zero products).
    #[cfg(feature = "lean")]
    if s == [1.0; 3] {
        c.out[me] = m1;
    } else {
        let r = |i: usize| [m1[i][0] * s[i], m1[i][1] * s[i], m1[i][2] * s[i], 0.0];
        c.out[me] = [r(0), r(1), r(2), [m1[3][0], m1[3][1], m1[3][2], 1.0]];
    }
    #[cfg(not(feature = "lean"))]
    {
        c.out[me] = mtx_cat(&gu_scale(s[0], s[1], s[2]), &m1);
    }

    // Children see the rotate/translate matrix only; the scale reaches them via
    // `parentScaleStack` (applied to their translation).
    for ch in &o.children {
        transform_object(ch, &m1, scale, c);
    }
}

/// Port of `XformActor` (mdxActor.cpp:547): returns `objMatrix` for every object in pre-order.
pub fn xform_actor(model: &Model, actor: &mut Actor) -> Vec<Mat4> {
    let mut out = Vec::new();
    let top = actor_top(actor);
    xform_into(model, actor, &top, &mut out);
    out
}

/// The matrix-stack top of `XformActor`: rotation, then translation to the position.
fn actor_top(actor: &Actor) -> Mat4 {
    let transmat = gu_translate(actor.pos[0], actor.pos[1], actor.pos[2]);
    mtx_cat(&quaternion_to_matrix(&actor.q_rot), &mtx_cat(&transmat, &IDENTITY))
}

/// [`xform_actor`] below matrix-stack top `top`, into `out` (resized to the object count).
fn xform_into(model: &Model, actor: &mut Actor, top: &Mat4, out: &mut Vec<Mat4>) {
    let n = object_count(model);
    if actor.last_keys.len() != n {
        actor.last_keys = vec![[0; 3]; n];
    }
    // Every entry is written below (each object once).
    if out.len() != n {
        out.clear();
        out.resize(n, IDENTITY);
    }
    let animated = actor.animated && !model.animations.is_empty();
    let mut c = Ctx { actor_scale: actor.scale, animated, time: actor.anim_time, last: &mut actor.last_keys, out, idx: 0 };
    transform_object(&model.root, top, [1.0; 3], &mut c);
    // `TransformObject` recurses on `obj->next` with the same matrix-stack top.
    for s in &model.root_siblings {
        transform_object(s, top, [1.0; 3], &mut c);
    }
}

/// A new matrix stamp ([`PoseCache::stamp`], `MeshDraw::stamp`; never 0).
pub fn next_stamp() -> u32 {
    use core::sync::atomic::{AtomicU32, Ordering::Relaxed};
    static NEXT: AtomicU32 = AtomicU32::new(1);
    // The N64 has one thread (and no cheap atomic read-modify-write).
    #[cfg(target_vendor = "nintendo64")]
    let s = {
        let s = NEXT.load(Relaxed);
        NEXT.store(s.wrapping_add(1), Relaxed);
        if s == 0 {
            NEXT.store(2, Relaxed);
            return 1;
        }
        s
    };
    #[cfg(not(target_vendor = "nintendo64"))]
    let s = NEXT.fetch_add(1, Relaxed);
    if s == 0 {
        NEXT.fetch_add(1, Relaxed)
    } else {
        s
    }
}

/// [`xform_actor`] for back ends that consume matrices only (the mesh sink), into
/// `actor.pose.mats` and recomputed only when an input changed:
/// - nothing changed (model, position, rotation, scale, animation time, animated flag):
///   the last result;
/// - only the position changed (moving, not turning, not re-animated actors): the cached
///   `L x R` (hierarchy below an identity matrix-stack top, times the actor rotation) plus
///   the new translation, three additions per object;
/// - only the position / rotation changed (and every actor between ticks with
///   interpolation): the cached hierarchy `L` times the new rotation, one 3x3 product per
///   object;
/// - else the whole hierarchy.
///
/// `XformActor`'s matrix-stack top is the rotation with the translation in row 3, so
/// `L x top` is `L x R` with the translation added to row 3: the same sums in the same
/// order. The keyframe search hints it skips updating only speed up the search. The cached
/// cases associate the products differently from `XformActor` (`L x top` instead of the
/// chain applied to `top`): equal up to float rounding.
#[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_d")]
pub fn xform_actor_cached<'a>(model: &Model, actor: &'a mut Actor) -> &'a [Mat4] {
    let key = PoseKey { model: model.id, pos: actor.pos, q_rot: actor.q_rot, scale: actor.scale, anim_time: actor.anim_time, animated: actor.animated };
    if actor.pose.key != Some(key) {
        pose_update(model, actor, key);
    }
    &actor.pose.mats
}

/// [`xform_actor_cached`] when an input changed.
#[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_d")]
#[inline(never)]
fn pose_update(model: &Model, actor: &mut Actor, key: PoseKey) {
    let lkey = LocalKey { model: model.id, scale: actor.scale, anim_time: actor.anim_time, animated: actor.animated };
    let fresh = actor.pose.local_key != Some(lkey);
    if fresh {
        // Also sizes `mats` and `rotl` like `local`.
        xform_local(model, actor, lkey);
    }
    let pose = &mut actor.pose;
    let t = actor.pos;
    let q = actor.q_rot;
    let add_t = |m: &mut Mat4| {
        m[3][0] += t[0];
        m[3][1] += t[1];
        m[3][2] += t[2];
    };
    if pose.rot_q == Some(q) {
        for (o, m) in pose.mats.iter_mut().zip(&pose.rotl) {
            *o = *m;
            add_t(o);
        }
    } else {
        // `L x R` straight into the result; also kept (`rotl`) once hierarchy and rotation
        // hold for a second result (moving actors that only turn now and then): then only
        // the translation changes.
        let keep = !fresh && pose.last_q == Some(q);
        let r = quaternion_to_matrix(&q);
        for ((o, c), l) in pose.mats.iter_mut().zip(pose.rotl.iter_mut()).zip(&pose.local) {
            let m = mtx_cat_rot(l, &r);
            if keep {
                *c = m;
            }
            *o = m;
            add_t(o);
        }
        if keep {
            pose.rot_q = Some(q);
        }
    }
    pose.last_q = Some(q);
    pose.key = Some(key);
    pose.stamp = next_stamp();
}

/// The [`PoseCache`] hierarchy below an identity matrix-stack top (out of line: animated
/// actors only, keeps the per-frame path small in the I-cache); `mats` and `rotl` get its
/// length.
#[inline(never)]
fn xform_local(model: &Model, actor: &mut Actor, lkey: LocalKey) {
    let mut local = core::mem::take(&mut actor.pose.local);
    xform_into(model, actor, &IDENTITY, &mut local);
    let n = local.len();
    actor.pose.local = local;
    actor.pose.local_key = Some(lkey);
    actor.pose.rot_q = None;
    actor.pose.mats.resize(n, IDENTITY);
    actor.pose.rotl.resize(n, IDENTITY);
}

/// [`mtx_cat`]`(a, b)` for a rotation `b` (row 3 of `b` zero): row 3 of the result is
/// `a`'s translation rotated, without `b`'s translation added.
#[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_d")]
fn mtx_cat_rot(a: &Mat4, b: &Mat4) -> Mat4 {
    let row = |i: usize| {
        [
            a[i][0] * b[0][0] + a[i][1] * b[1][0] + a[i][2] * b[2][0],
            a[i][0] * b[0][1] + a[i][1] * b[1][1] + a[i][2] * b[2][1],
            a[i][0] * b[0][2] + a[i][1] * b[1][2] + a[i][2] * b[2][2],
        ]
    };
    let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
    [[r0[0], r0[1], r0[2], 0.0], [r1[0], r1[1], r1[2], 0.0], [r2[0], r2[1], r2[2], 0.0], [r3[0], r3[1], r3[2], 1.0]]
}

/// The largest `tMy` of `PCCalcModgeValues` (`(sin + 1) * 5`), the height `OBJECT_FLAGS_WAVE`
/// adds to a vertex (object units).
pub const WAVE_MAX: f32 = 10.0;

/// A radius about the actor's position that encloses every vertex a back end draws for
/// `model` in any pose [`xform_actor`] can give it (any animation time, any actor rotation),
/// per unit of the actor's largest `|scale|` component; `sphere(object)` is the back end's
/// bound of an object's mesh (pre-order index).
///
/// Rotations (`QuaternionToMatrix` of any non-zero quaternion, keyframe slerps) keep lengths;
/// an interpolated translation or scale key lies within its keys' largest. `TransformObject`
/// puts a vertex `v` of object `o` at `(v * s_o * a) R_o ... R + sum_k (t_k * a * s_k-1) R_k-1
/// ... R + pos` (`a` the actor scale, `s` the scale keys, `t` the move keys of `o` and its
/// ancestors, `R` rotations), so `|p - pos| <= max|a| * (|v| max|s_o| + sum_k |t_k|
/// max|s_k-1|)`.
///
/// `OBJECT_FLAGS_WAVE` raises a vertex by `tMy` (0..10, `PCCalcModgeValues`) before its
/// matrix, for any object (the game can OR it into every object's flags): every object's
/// radius gets [`WAVE_MAX`] more.
///
/// `None` (never cull): skinned models (bones move the root mesh), an object without a known
/// bound, non-finite data, move or scale keys whose times do not increase.
pub fn model_bound(model: &Model, sphere: &dyn Fn(u16) -> crate::scene::ObjSphere) -> Option<f32> {
    use crate::scene::ObjSphere;
    if model.is_skinned {
        return None;
    }
    struct W<'a> {
        sphere: &'a dyn Fn(u16) -> ObjSphere,
        idx: usize,
        best: f32,
        ok: bool,
    }
    fn len(v: &Vec3) -> f32 {
        (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
    }
    fn walk(o: &Object, chain: f32, parent_scale: f32, w: &mut W) {
        let me = w.idx;
        w.idx += 1;
        let tmax = o.move_keys.iter().map(|k| len(&k.v)).fold(0.0f32, f32::max);
        let smax =
            if o.scale_keys.is_empty() { 1.0 } else { o.scale_keys.iter().map(|k| k.v[0].abs().max(k.v[1].abs()).max(k.v[2].abs())).fold(0.0f32, f32::max) };
        let finite = |k: &[f32]| k.iter().all(|x| x.is_finite());
        if !o.move_keys.iter().all(|k| finite(&k.v)) || !o.scale_keys.iter().all(|k| finite(&k.v)) {
            w.ok = false;
        }
        // Interpolation stays between two keys only if their times increase (else the key
        // search can extrapolate).
        let increasing = |k: &[VKey]| k.windows(2).all(|w| w[0].time < w[1].time);
        if !increasing(&o.move_keys) || !increasing(&o.scale_keys) {
            w.ok = false;
        }
        let here = chain + tmax * parent_scale;
        if o.mesh.is_some() {
            match (w.sphere)(me as u16) {
                ObjSphere::Unknown => w.ok = false,
                ObjSphere::NoMesh => {}
                ObjSphere::Sphere(c, r) => {
                    if !finite(&c) || !r.is_finite() {
                        w.ok = false;
                    }
                    w.best = w.best.max(here + (len(&c) + r + WAVE_MAX) * smax);
                }
            }
        }
        // Children: the parent's rotate/translate matrix, its scale keys on their translation.
        for c in &o.children {
            walk(c, here, smax, w);
        }
    }
    let mut w = W { sphere, idx: 0, best: 0.0, ok: true };
    walk(&model.root, 0.0, 1.0, &mut w);
    for s in &model.root_siblings {
        walk(s, 0.0, 1.0, &mut w);
    }
    (w.ok && w.best.is_finite()).then_some(w.best)
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

    /// xorshift64 for the randomised tests.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
        fn f(&mut self, lo: f32, hi: f32) -> f32 {
            lo + (hi - lo) * (self.next() % 1_000_000) as f32 / 1_000_000.0
        }
    }

    /// `key_search` (the hint fast path) returns exactly what `find_to_from` does, for any
    /// key array (unsorted and repeated times included), hint and time.
    #[test]
    fn key_search_matches_find_to_from() {
        let mut r = Rng(0x9e37_79b9_7f4a_7c15);
        for _ in 0..4000 {
            let n = 1 + r.below(12) as usize;
            let sorted = r.below(4) != 0;
            let mut t = r.below(5) as i32 - 2;
            let keys: Vec<VKey> = (0..n)
                .map(|_| {
                    t = if sorted { t + r.below(6) as i32 } else { r.below(40) as i32 - 5 };
                    vk(t, 0.0)
                })
                .collect();
            for _ in 0..50 {
                // Hints are earlier results: 0..n (`last_keys` starts at 0).
                let hint = r.below(n as u64) as i16;
                let time = if r.below(3) == 0 { (r.below(50) as i32 - 5) as f32 } else { r.f(-5.0, 45.0) };
                let a = key_search(&keys, time, hint);
                let b = find_to_from(&keys, time, hint);
                assert!(
                    a.0 == b.0 && a.1 == b.1 && a.2.to_bits() == b.2.to_bits(),
                    "keys {:?} time {time} hint {hint}: {a:?} vs {b:?}",
                    keys.iter().map(|k| k.time).collect::<Vec<_>>()
                );
            }
        }
    }

    fn rand_object(r: &mut Rng, depth: u32, idx: &mut u16, spheres: &mut Vec<(Vec3, f32)>) -> Object {
        let nk = |r: &mut Rng| 1 + r.below(4) as i32;
        let (nm, ns, nr) = (nk(r), nk(r), nk(r));
        let keys_t = |r: &mut Rng, n: i32| -> Vec<i32> { (0..n).map(|k| k * 10 + r.below(5) as i32).collect() };
        let tm = keys_t(r, nm);
        let ts = keys_t(r, ns);
        let tr = keys_t(r, nr);
        let move_keys = tm.iter().map(|&t| VKey { v: [r.f(-30.0, 30.0), r.f(-30.0, 30.0), r.f(-30.0, 30.0)], time: t }).collect();
        let scale_keys = ts.iter().map(|&t| VKey { v: [r.f(-2.0, 2.0), r.f(0.2, 2.0), r.f(0.2, 2.0)], time: t }).collect();
        let rotate_keys = tr
            .iter()
            .map(|&t| {
                let q = [r.f(-1.0, 1.0), r.f(-1.0, 1.0), r.f(-1.0, 1.0), r.f(-1.0, 1.0)];
                let l = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt().max(0.1);
                QKey { q: q.map(|x| x / l), time: t }
            })
            .collect();
        let me = *idx;
        *idx += 1;
        let has_mesh = r.below(4) != 0;
        let mesh = has_mesh.then(|| f2_formats::objbank::Mesh {
            vertices: Vec::new(),
            faces: Vec::new(),
            face_uv: Vec::new(),
            normals: Vec::new(),
            colors: Vec::new(),
            face_flags: Vec::new(),
            textures: Vec::new(),
        });
        while spheres.len() <= me as usize {
            spheres.push(([0.0; 3], -1.0));
        }
        if has_mesh {
            spheres[me as usize] = ([r.f(-20.0, 20.0), r.f(-20.0, 20.0), r.f(-20.0, 20.0)], r.f(0.0, 15.0));
        }
        let nch = if depth < 3 { r.below(3) } else { 0 };
        let flags = if r.below(5) == 0 { crate::scene::OBJECT_FLAGS_WAVE } else { 0 };
        let mut children = Vec::new();
        for _ in 0..nch {
            children.push(rand_object(r, depth + 1, idx, spheres));
        }
        Object {
            id: me as u32,
            name: String::new(),
            xlu: 0,
            sprites: Vec::new(),
            mesh,
            effected_verts: Vec::new(),
            scale_keys,
            move_keys,
            rotate_keys,
            bbox: None,
            phong: 0,
            flags,
            matrix: IDENTITY,
            children,
        }
    }

    /// [`model_bound`] encloses every point of every object's mesh sphere (plus the WAVE
    /// height) for random hierarchies, animation times, actor rotations and scales.
    #[test]
    fn model_bound_encloses_every_pose() {
        use crate::scene::ObjSphere;
        let mut r = Rng(0x2545_f491_4f6c_dd1d);
        for _ in 0..300 {
            let mut idx = 0;
            let mut spheres = Vec::new();
            let root = rand_object(&mut r, 0, &mut idx, &mut spheres);
            let root_siblings = (0..r.below(2)).map(|_| rand_object(&mut r, 1, &mut idx, &mut spheres)).collect();
            let model = Model {
                id: 7,
                radius: 0.0,
                object_size: 0,
                root,
                root_siblings,
                animations: vec![f2_formats::objbank::Animation { start: 0, end: 40, speed: 1.0, looped: 1 }],
                loc_mesh: None,
                is_skinned: false,
                num_vtx: 0,
            };
            let sph = |o: u16| match spheres[o as usize] {
                (c, rad) if rad >= 0.0 => ObjSphere::Sphere(c, rad),
                _ => ObjSphere::NoMesh,
            };
            let Some(bound) = model_bound(&model, &sph) else { panic!("no bound") };
            let mut objs = Vec::new();
            for_each_object(&model, |i, o| objs.push((i, o.mesh.is_some(), o.flags)));
            for _ in 0..40 {
                let q = [r.f(-1.0, 1.0), r.f(-1.0, 1.0), r.f(-1.0, 1.0), r.f(-1.0, 1.0)];
                let s = [r.f(-3.0, 3.0), r.f(-3.0, 3.0), r.f(-3.0, 3.0)];
                let pos = [r.f(-500.0, 500.0), r.f(-500.0, 500.0), r.f(-500.0, 500.0)];
                let mut a = Actor { pos, q_rot: q, scale: s, anim_time: r.f(-5.0, 45.0), animated: r.below(4) != 0, ..Default::default() };
                let mats = xform_actor(&model, &mut a);
                let amax = s.iter().fold(0.0f32, |m, x| m.max(x.abs()));
                for &(i, mesh, flags) in &objs {
                    if !mesh {
                        continue;
                    }
                    let (c, rad) = spheres[i];
                    for _ in 0..8 {
                        // A point on the sphere (or its centre), raised by up to the WAVE height.
                        let d = [r.f(-1.0, 1.0), r.f(-1.0, 1.0), r.f(-1.0, 1.0)];
                        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-3);
                        let wave = if flags & crate::scene::OBJECT_FLAGS_WAVE != 0 { r.f(0.0, WAVE_MAX) } else { 0.0 };
                        let v = [c[0] + d[0] / l * rad, c[1] + d[1] / l * rad + wave, c[2] + d[2] / l * rad];
                        let p = mtx_xfm(&mats[i], v);
                        let dist = ((p[0] - pos[0]).powi(2) + (p[1] - pos[1]).powi(2) + (p[2] - pos[2]).powi(2)).sqrt();
                        assert!(dist <= bound * amax * 1.001 + 0.01, "object {i}: {dist} > {} (bound {bound}, scale {amax})", bound * amax);
                    }
                }
            }
        }
    }

    /// Console path: the row-scaled object matrix equals `mtx_cat(gu_scale(s), m1)` up to
    /// the sign of zeros (what `TransformObject` computes on the PC).
    #[cfg(feature = "lean")]
    #[test]
    fn lean_transform_matches_full_products() {
        let mut r = Rng(0x0123_4567_89ab_cdef);
        for _ in 0..2000 {
            let m1 = mtx_cat(
                &quaternion_to_matrix(&[r.f(-1.0, 1.0), r.f(-1.0, 1.0), r.f(-1.0, 1.0), r.f(-1.0, 1.0)]),
                &gu_translate(r.f(-50.0, 50.0), r.f(-50.0, 50.0), r.f(-50.0, 50.0)),
            );
            let s = [r.f(-3.0, 3.0), if r.below(3) == 0 { 0.0 } else { r.f(-3.0, 3.0) }, r.f(-3.0, 3.0)];
            let full = mtx_cat(&gu_scale(s[0], s[1], s[2]), &m1);
            let row = |i: usize| [m1[i][0] * s[i], m1[i][1] * s[i], m1[i][2] * s[i], 0.0];
            let lean = [row(0), row(1), row(2), [m1[3][0], m1[3][1], m1[3][2], 1.0]];
            assert_eq!(full, lean, "scale {s:?}");
        }
    }
}
