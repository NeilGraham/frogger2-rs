//! The renderer's view model, inside the deterministic simulation.
//!
//! The original decides several gameplay-relevant things while *drawing*: special effects
//! are only created/updated when `IsPointVisible` (Common/multi.c:1694) says their origin
//! is on screen, `ACTOR2::clipped` comes from the MDX object's bounding-box test
//! (`OBJECT_FLAGS_CLIPPED`, set by `TransformObject`), `distanceFromFrog` is refreshed by
//! `CopyActorList` (PC/Main.cpp:621), and actors only animate when `ActorListDraw`
//! (MDX `mdxActor.cpp:139`) transforms them. This module ports that part of the MDX
//! library (view matrix `guLookAtF`, `XfmPoint`, `CheckBoundingBox`, `XformActor` for
//! the root object) with the oracle's 640x480 projection, so these decisions do not depend
//! on the host's display.
//!
//! Matrices are `[[f32; 4]; 4]` in MDX's row-vector convention (`p' = p · M`,
//! translation in row 3, `PushMatrix(a)` = `a · top`). Float maths is `f32` throughout:
//! the retail exe runs with Direct3D's single-precision x87 control word.

use f2_formats::objbank::{Mat4, Model, Object, QKey, Quat, VKey, Vec3};
use f2_math::{distance_between_points_ss, SVector};

use crate::ids::ActorId;
use crate::mdx_actor::mdx_flags;
use crate::specfx::ViewXform;
use crate::Game;

/// `OBJECT_FLAGS_CLIPPED` (`mdxObject.h:273`, `1<<15`).
pub const OBJECT_FLAGS_CLIPPED: i16 = i16::MIN;

/// Oracle resolution (`f2fix.ini reg.resolution=640x480`): `rXRes`/`rYRes`.
pub const VIEW_X_RES: f32 = 640.0;
pub const VIEW_Y_RES: f32 = 480.0;
/// `nearClip`, `horizClip`, `vertClip` (mdxRender.cpp:58), `DIST`, `FOV` (after `SetupRenderer`).
pub const NEAR_CLIP: f32 = 10.0;
pub const HORIZ_CLIP: f32 = 3000.0;
pub const VERT_CLIP: f32 = 2500.0;
pub const DIST: i32 = -10;
pub const FOV: i32 = 450;
/// Initial `farClip` (mdxRender.cpp:59); `DrawLoop` sets `fog.max*0.1` every frame.
pub const FAR_CLIP_INIT: f32 = 1200.0;

/// `CAMVECTSCALE` (PC/drawloop.cpp:63).
const CAMVECTSCALE: f32 = 1.0 / 40960.0;

pub const IDENTITY: Mat4 = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];

// ---- mdxMath.cpp -------------------------------------------------------------------

const PI2: f64 = std::f64::consts::PI * 2.0;
const EPSILON: f32 = 0.00001;
const HALFPI: f32 = 1.570_796_4;

struct Tables {
    sin: [f32; 4096],
    acos: [f32; 4096],
}

/// `InitMaths` (mdxMath.cpp:53): tables behind `sinFast`/`acosFast`.
fn tables() -> &'static Tables {
    static T: std::sync::OnceLock<Box<Tables>> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut t = Box::new(Tables { sin: [0.0; 4096], acos: [0.0; 4096] });
        for i in 0..4096 {
            t.acos[i] = f2_math::detfloat::acos((i as f64 - 2048.0) / 2048.0) as f32;
            t.sin[i] = f2_math::detfloat::sin((i as f64 * PI2) / 4096.0) as f32;
        }
        t
    })
}

fn sin_fast(x: f32) -> f32 {
    let i = (x as f64 * (4096.0 / PI2)) as i64;
    tables().sin[(i & 4095) as usize]
}

fn acos_fast(x: f32) -> f32 {
    let i = ((x as f64 * 2048.0) + 2048.0) as i64;
    tables().acos[(i & 4095) as usize]
}

/// The `PushMatrix` macro body (mdxMath.h:86) / `guMtxCatF`: `a · b`, affine.
pub fn mtx_cat(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut r = [[0f32; 4]; 4];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    for j in 0..3 {
        r[3][j] = a[3][0] * b[0][j] + a[3][1] * b[1][j] + a[3][2] * b[2][j] + b[3][j];
    }
    r[3][3] = 1.0;
    r
}

fn gu_scale(dx: f32, dy: f32, dz: f32) -> Mat4 {
    let mut a = IDENTITY;
    a[0][0] = dx;
    a[1][1] = dy;
    a[2][2] = dz;
    a
}

fn gu_translate(dx: f32, dy: f32, dz: f32) -> Mat4 {
    let mut a = IDENTITY;
    a[3][0] = dx;
    a[3][1] = dy;
    a[3][2] = dz;
    a
}

/// Port of `guMtxXFMF` (mdxMath.cpp:127).
pub fn mtx_xfm(m: &Mat4, v: Vec3) -> Vec3 {
    [
        m[0][0] * v[0] + m[1][0] * v[1] + m[2][0] * v[2] + m[3][0],
        m[0][1] * v[0] + m[1][1] * v[1] + m[2][1] * v[2] + m[3][1],
        m[0][2] * v[0] + m[1][2] * v[1] + m[2][2] * v[2] + m[3][2],
    ]
}

fn normalise(v: &mut Vec3) {
    let m = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if m != 0.0 {
        let r = 1.0 / m;
        v[0] *= r;
        v[1] *= r;
        v[2] *= r;
    }
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Port of `guLookAtF` (mdxMath.cpp:136). `CalcViewMatrix` passes the camera *target*
/// as `eye` and the camera source as `at`.
pub fn gu_look_at(eye: Vec3, at: Vec3, up: Vec3) -> Mat4 {
    let mut m = IDENTITY;
    let mut view_dir = [eye[0] - at[0], eye[1] - at[1], eye[2] - at[2]];
    normalise(&mut view_dir);
    let mut right = cross(up, view_dir);
    let mut up2 = cross(view_dir, right);
    normalise(&mut right);
    normalise(&mut up2);
    m[0][0] = right[0];
    m[1][0] = right[1];
    m[2][0] = right[2];
    m[0][1] = up2[0];
    m[1][1] = up2[1];
    m[2][1] = up2[2];
    m[0][2] = view_dir[0];
    m[1][2] = view_dir[1];
    m[2][2] = view_dir[2];
    m[3][0] = -dot(right, at);
    m[3][1] = -dot(up2, at);
    m[3][2] = -dot(view_dir, at);
    m
}

/// Port of `QuaternionToMatrix` (mdxMath.cpp:237); quaternions are `[x, y, z, w]`.
pub fn quaternion_to_matrix(q: &Quat) -> Mat4 {
    let [x, y, z, w] = *q;
    let s = 2.0 / (x * x + y * y + z * z + w * w);
    let (xs, ys, zs) = (x * s, y * s, z * s);
    let (wx, wy, wz) = (w * xs, w * ys, w * zs);
    let (xx, xy, xz) = (x * xs, x * ys, x * zs);
    let (yy, yz, zz) = (y * ys, y * zs, z * zs);
    [[1.0 - (yy + zz), xy + wz, xz - wy, 0.0], [xy - wz, 1.0 - (xx + zz), yz + wx, 0.0], [xz + wy, yz - wx, 1.0 - (xx + yy), 0.0], [0.0, 0.0, 0.0, 1.0]]
}

/// Port of `QuatSlerp` (mdxMath.cpp:304).
fn quat_slerp(s1: &Quat, sp2: &Quat, t: f32) -> Quat {
    let (x, y, z, w) = (s1[0] - sp2[0], s1[1] - sp2[1], s1[2] - sp2[2], s1[3] - sp2[3]);
    let pdist = x * x + y * y + z * z + w * w;
    let (x, y, z, w) = (s1[0] + sp2[0], s1[1] + sp2[1], s1[2] + sp2[2], s1[3] + sp2[3]);
    let ndist = x * x + y * y + z * z + w * w;
    let s2 = if pdist < ndist { *sp2 } else { [-sp2[0], -sp2[1], -sp2[2], -sp2[3]] };
    let cosom = s1[0] * s2[0] + s1[1] * s2[1] + s1[2] * s2[2] + s1[3] * s2[3];
    if (1.0 + cosom) > EPSILON {
        let (sclp, sclq) = if (1.0 - cosom) > EPSILON {
            let omega = acos_fast(cosom);
            let sinom = 1.0 / sin_fast(omega);
            (sin_fast((1.0 - t) * omega) * sinom, sin_fast(t * omega) * sinom)
        } else {
            (1.0 - t, t)
        };
        [sclp * s1[0] + sclq * s2[0], sclp * s1[1] + sclq * s2[1], sclp * s1[2] + sclq * s2[2], sclp * s1[3] + sclq * s2[3]]
    } else {
        let mut dq = [-s1[1], s1[0], -s1[3], s1[2]];
        let sclp = sin_fast((1.0 - t) * HALFPI);
        let sclq = sin_fast(t * HALFPI);
        dq[0] = sclp * s1[0] + sclq * dq[0];
        dq[1] = sclp * s1[1] + sclq * dq[1];
        dq[2] = sclp * s1[2] + sclq * dq[2];
        // BUG(original): w keeps the perpendicular quaternion's value.
        dq
    }
}

fn linear_interp(from: &Vec3, to: &Vec3, t: f32) -> Vec3 {
    [from[0] + (to[0] - from[0]) * t, from[1] + (to[1] - from[1]) * t, from[2] + (to[2] - from[2]) * t]
}

// ---- mdxObject.cpp: keyframes ------------------------------------------------------

/// `ANIMSNAP_THRESHHOLD` / `ROTATESNAP_MULT` (mdxObject.cpp:41).
const ANIMSNAP_THRESHHOLD: f32 = 0.25;
const ROTATESNAP_MULT: f32 = 2.0;

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

/// Port of `FindToFromVKeys`/`FindToFromQKeys` (mdxObject.cpp:229/305): `(from, to, interp)`.
fn find_to_from<K: KeyTime>(keys: &[K], time: f32, e_key: i16) -> (usize, usize, f32) {
    let num_keys = keys.len() as i32;
    let t = |i: i32| keys[i.clamp(0, num_keys - 1) as usize].time() as f32;
    let mut low = e_key as i32;
    // BUG(original): keys[eKey+1] is read out of bounds for the last key (clamped here).
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
    if from_key >= 0 {
        let temp = (keys[to_key as usize].time().wrapping_sub(keys[from_key as usize].time())).unsigned_abs() as f32;
        let interp = (time - keys[from_key as usize].time() as f32) / temp;
        (from_key as usize, to_key as usize, interp)
    } else {
        let k = (num_keys - 1).max(0) as usize;
        (k, k, 1.0)
    }
}

/// The keyframes and bounding box of a model's root object (`objectController->object`),
/// which is all `CopyActorList` needs (`OBJECT_FLAGS_CLIPPED` of the root).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RootObject {
    pub move_keys: Vec<VKey>,
    pub scale_keys: Vec<VKey>,
    pub rotate_keys: Vec<QKey>,
    pub bbox: Option<[Vec3; 8]>,
}

impl RootObject {
    pub fn from_object(o: &Object) -> RootObject {
        RootObject { move_keys: o.move_keys.clone(), scale_keys: o.scale_keys.clone(), rotate_keys: o.rotate_keys.clone(), bbox: o.bbox }
    }
}

/// The local part of `TransformObject` (mdxObject.cpp:371): the object's `rotmat` (with
/// the scaled translation in row 3) and its `scale` vector at `time`. `last` holds
/// `lastMKey`, `lastSKey`, `lastRKey`.
fn object_local(mk: &[VKey], sk: &[VKey], rk: &[QKey], animated: bool, time: f32, last: &mut [i16; 3], actor_scale: Vec3, parent_scale: Vec3) -> (Mat4, Vec3) {
    let translation = if mk.len() > 1 && animated {
        let (f, t, i) = find_to_from(mk, time, last[0]);
        last[0] = f as i16;
        if (mk[t].time as f32 - time) < ANIMSNAP_THRESHHOLD {
            mk[t].v
        } else {
            linear_interp(&mk[f].v, &mk[t].v, i)
        }
    } else {
        mk.first().map_or([0.0; 3], |k| k.v)
    };
    let scale = if sk.len() > 1 && animated {
        let (f, t, i) = find_to_from(sk, time, last[1]);
        last[1] = f as i16;
        if (sk[t].time as f32 - time) < ANIMSNAP_THRESHHOLD {
            sk[t].v
        } else {
            linear_interp(&sk[f].v, &sk[t].v, i)
        }
    } else {
        sk.first().map_or([1.0; 3], |k| k.v)
    };
    let mut rotmat = if rk.len() > 1 && animated {
        let (f, t, i) = find_to_from(rk, time, last[2]);
        last[2] = f as i16;
        if (rk[t].time as f32 - time) < ANIMSNAP_THRESHHOLD * ROTATESNAP_MULT {
            quaternion_to_matrix(&rk[t].q)
        } else {
            quaternion_to_matrix(&quat_slerp(&rk[f].q, &rk[t].q, i))
        }
    } else {
        quaternion_to_matrix(&rk.first().map_or([0.0, 0.0, 0.0, 1.0], |k| k.q))
    };
    rotmat[3][0] = translation[0] * actor_scale[0] * parent_scale[0];
    rotmat[3][1] = translation[1] * actor_scale[1] * parent_scale[1];
    rotmat[3][2] = translation[2] * actor_scale[2] * parent_scale[2];
    (rotmat, scale)
}

/// The matrix-stack top `XformActor` (mdxActor.cpp:547) pushes before the root object:
/// `qRot · translate(pos)`.
fn actor_top(pos: Vec3, q_rot: &Quat) -> Mat4 {
    let top = mtx_cat(&gu_translate(pos[0], pos[1], pos[2]), &IDENTITY);
    mtx_cat(&quaternion_to_matrix(q_rot), &top)
}

fn flatten(m: &Mat4) -> [f32; 16] {
    let mut o = [0f32; 16];
    for i in 0..4 {
        for j in 0..4 {
            o[i * 4 + j] = m[i][j];
        }
    }
    o
}

/// `FindActorCentre` (mdxActor.cpp:896) as `CreateActor` runs it: the whole hierarchy is
/// transformed at the origin (scale 1, identity rotation, `animTime` 0) and the bounds of
/// every object's mesh give `trueCentre` and `radius` (× `RADIUS_SCALE` 0.95). A model
/// constant, so it is computed once per model.
pub fn find_actor_centre(model: &Model, animated: bool) -> (Vec3, f32) {
    const BIGVAL: f32 = 10000.0;
    let mut mx = [-BIGVAL; 3];
    let mut mn = [BIGVAL; 3];
    fn walk(o: &Object, top: &Mat4, parent_scale: Vec3, animated: bool, mx: &mut Vec3, mn: &mut Vec3) {
        let mut last = [0i16; 3];
        let (rotmat, scale) = object_local(&o.move_keys, &o.scale_keys, &o.rotate_keys, animated, 0.0, &mut last, [1.0; 3], parent_scale);
        let m1 = mtx_cat(&rotmat, top);
        let obj = mtx_cat(&gu_scale(scale[0], scale[1], scale[2]), &m1);
        if let Some(mesh) = &o.mesh {
            for v in &mesh.vertices {
                let t = mtx_xfm(&obj, *v);
                for k in 0..3 {
                    mx[k] = if t[k] > mx[k] { t[k] } else { mx[k] };
                    mn[k] = if t[k] < mn[k] { t[k] } else { mn[k] };
                }
            }
        }
        for c in &o.children {
            walk(c, &m1, scale, animated, mx, mn);
        }
    }
    let top = actor_top([0.0; 3], &[0.0, 0.0, 0.0, 1.0]);
    walk(&model.root, &top, [1.0; 3], animated, &mut mx, &mut mn);
    for s in &model.root_siblings {
        walk(s, &top, [1.0; 3], animated, &mut mx, &mut mn);
    }
    let c = [(mn[0] + mx[0]) / 2.0, (mn[1] + mx[1]) / 2.0, (mn[2] + mx[2]) / 2.0];
    let r = [mx[0] - c[0], mx[1] - c[1], mx[2] - c[2]];
    let mag = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt();
    (c, (mag as f64 * 0.95) as f32)
}

/// A model object's name and keys (`MDX_OBJECT`), for transforming sub-objects.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct XNode {
    pub name: String,
    pub keys: RootObject,
    pub children: Vec<XNode>,
}

impl XNode {
    pub fn from_object(o: &Object) -> XNode {
        XNode { name: o.name.clone(), keys: RootObject::from_object(o), children: o.children.iter().map(XNode::from_object).collect() }
    }
}

/// `objMatrix` of the first object named `name` (case-insensitive, `FindObj` order:
/// the object, its `next` siblings, then its children) after `XformActor` with the
/// given placement; the object's `postMatrix` is the identity (`headPostMatrix`).
pub fn sub_object_matrix(chain: &[XNode], name: &str, p: &MdxPlacement, animated: bool, time: f32) -> Option<Mat4> {
    // FindObj order: chain[i], then chain[i+1..] (recursively), then chain[i].children.
    fn find(chain: &[XNode], i: usize, name: &str, top: &Mat4, ps: Vec3, a: &Ctx) -> Option<Mat4> {
        let me = &chain[i];
        let mut last = [0i16; 3];
        let (rotmat, scale) = object_local(&me.keys.move_keys, &me.keys.scale_keys, &me.keys.rotate_keys, a.animated, a.time, &mut last, a.scale, ps);
        let m1 = mtx_cat(&rotmat, top);
        if me.name.eq_ignore_ascii_case(name) {
            let s = gu_scale(scale[0] * a.scale[0], scale[1] * a.scale[1], scale[2] * a.scale[2]);
            return Some(mtx_cat(&IDENTITY, &mtx_cat(&s, &m1)));
        }
        if i + 1 < chain.len() {
            if let Some(m) = find(chain, i + 1, name, top, ps, a) {
                return Some(m);
            }
        }
        if !me.children.is_empty() {
            return find(&me.children, 0, name, &m1, scale, a);
        }
        None
    }
    struct Ctx {
        animated: bool,
        time: f32,
        scale: Vec3,
    }
    if chain.is_empty() {
        return None;
    }
    let top = actor_top(p.pos, &p.q_rot);
    find(chain, 0, name, &top, [1.0; 3], &Ctx { animated, time, scale: p.scale })
}

// ---- mdxRender.cpp / mdxActor.cpp: projection and clipping --------------------------

impl ViewXform {
    /// The oracle's projection with view matrix `m` and `farClip`.
    pub fn oracle(matrix: Mat4, far_clip: f32) -> ViewXform {
        ViewXform {
            matrix,
            x_res: VIEW_X_RES,
            y_res: VIEW_Y_RES,
            near_clip: NEAR_CLIP,
            far_clip,
            horiz_clip: HORIZ_CLIP,
            vert_clip: VERT_CLIP,
            fov: FOV,
            dist: DIST,
        }
    }

    /// `-FOV * oneOver[fftol(z) + DIST]` (`oneOver[i] = (rXRes/640)/i`, `fftol` truncates).
    fn oozd_f(&self, z: f32) -> f32 {
        let i = (z as i32).wrapping_add(self.dist);
        if i <= 0 {
            return 0.0;
        }
        let one_over = (self.x_res / 640.0) / i as f32;
        -(self.fov as f32) * one_over
    }

    /// Port of `XfmPoint` (mdxRender.cpp:331) for an MDX-space point, `d == NULL`.
    pub fn xfm_point_f(&self, p: Vec3) -> Vec3 {
        let [x, y, z] = mtx_xfm(&self.matrix, p);
        let zd = z + self.dist as f32;
        if zd > self.near_clip && zd < self.far_clip && x > -self.horiz_clip && x < self.horiz_clip && y > -self.vert_clip && y < self.vert_clip {
            let o = self.oozd_f(z);
            [self.x_res / 2.0 + x * o, self.y_res / 2.0 + y * o, z]
        } else {
            [x, y, 0.0]
        }
    }

    /// Port of `CheckBoundingBox` (mdxActor.cpp:465): 1 when the box is off screen.
    pub fn check_bounding_box(&self, bbox: &[Vec3; 8], m: &Mat4) -> bool {
        const TOP: u32 = 0x1;
        const BOTTOM: u32 = 0x2;
        const LEFT: u32 = 0x4;
        const RIGHT: u32 = 0x8;
        const INWARD: u32 = 0x10;
        const OUTWARD: u32 = 0x20;
        let mtx = mtx_cat(m, &self.matrix);
        let mut ocs = [0u32; 8];
        for (k, b) in bbox.iter().enumerate() {
            let mut oc = 0;
            let [mut x, mut y, z] = mtx_xfm(&mtx, *b);
            if z < self.near_clip {
                oc |= INWARD;
            } else if z > self.far_clip {
                oc |= OUTWARD;
            } else if z >= self.near_clip && z <= self.far_clip {
                let o = self.oozd_f(z);
                x = self.x_res / 2.0 + x * o;
                y = self.y_res / 2.0 + y * o;
                if x < 0.0 {
                    oc |= LEFT;
                } else if x > self.x_res {
                    oc |= RIGHT;
                }
                if y < 0.0 {
                    oc |= TOP;
                } else if y > self.y_res {
                    oc |= BOTTOM;
                }
            }
            // bBox[k] fills ocs[7-k]
            ocs[7 - k] = oc;
            if oc == 0 {
                return false;
            }
        }
        let o = &ocs;
        (o[0] & o[1]) != 0
            && (o[0] & o[3]) != 0
            && (o[0] & o[4]) != 0
            && (o[1] & o[2]) != 0
            && (o[1] & o[5]) != 0
            && (o[2] & o[3]) != 0
            && (o[2] & o[6]) != 0
            && (o[3] & o[7]) != 0
            && (o[4] & o[5]) != 0
            && (o[4] & o[7]) != 0
            && (o[5] & o[6]) != 0
            && (o[6] & o[7]) != 0
            && (o[4] & o[6]) != 0
            && (o[3] & o[5]) != 0
            && (o[4] & o[2]) != 0
            && (o[6] & o[0]) != 0
    }
}

// ---- game side ------------------------------------------------------------------------

/// `MDX_ACTOR` placement and visibility (`pos`, `qRot`, `scale`, `visible`, `draw`), as
/// `CopyActorList` copies them from the game actor, plus the root object's key hints.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MdxPlacement {
    pub pos: Vec3,
    pub q_rot: Quat,
    pub scale: Vec3,
    pub visible: u8,
    pub draw: u8,
    /// Root object's `lastMKey`, `lastSKey`, `lastRKey`.
    pub last_keys: [i16; 3],
}

impl Default for MdxPlacement {
    fn default() -> Self {
        MdxPlacement { pos: [0.0; 3], q_rot: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3], visible: 1, draw: 1, last_keys: [0; 3] }
    }
}

/// MDX render globals that live across frames (`noClip` is always 0 between frames).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ViewState {
    /// `farClip` (`fog.max * 0.1` after the first `DrawLoop`).
    pub far_clip: f32,
    /// Whether `DrawLoop` has computed `vMatrix` yet (before that nothing is culled).
    pub valid: bool,
}

impl Default for ViewState {
    fn default() -> Self {
        ViewState { far_clip: FAR_CLIP_INIT, valid: false }
    }
}

impl Game {
    /// Port of `CalcViewMatrix` (PC/drawloop.cpp:65) + `farClip = fog.max*0.1`
    /// (drawloop.cpp:198): the view the next tick's logic culls against.
    pub fn calc_view_matrix(&mut self) {
        let c = &self.cam;
        let s = |v: i32| v as f32 * CAMVECTSCALE;
        let m = gu_look_at(
            [s(c.curr_cam_target.vx), s(c.curr_cam_target.vy), s(c.curr_cam_target.vz)],
            [s(c.curr_cam_source.vx), s(c.curr_cam_source.vy), s(c.curr_cam_source.vz)],
            [s(c.cam_vect.vx), s(c.cam_vect.vy), s(c.cam_vect.vz)],
        );
        self.view.far_clip = (self.frogger.fog.max as f64 * 0.1) as f32;
        self.view.valid = true;
        self.specfx.view = Some(ViewXform::oracle(m, self.view.far_clip));
    }

    /// The root object's `OBJECT_FLAGS_CLIPPED` as `TransformObject` sets it (only the
    /// root's flag is ever read, by `CopyActorList`).
    fn transform_root_object(&mut self, id: ActorId, no_clip: bool) {
        let Some(view) = self.specfx.view else { return };
        let a = &self.actor2.actors[id].actor.actual_actor;
        let Some(model) = a.object_controller.and_then(|m| self.models.get(m)).cloned() else { return };
        let Some(root) = model.root_object.as_ref() else { return };
        let animated = a.animation.is_some();
        let time = a.animation.as_ref().map_or(0.0, |x| x.anim_time);
        let p = a.placement;
        let mut last = p.last_keys;
        let (rotmat, scale) = object_local(&root.move_keys, &root.scale_keys, &root.rotate_keys, animated, time, &mut last, p.scale, [1.0; 3]);
        let top = actor_top(p.pos, &p.q_rot);
        let m1 = mtx_cat(&rotmat, &top);
        let obj = mtx_cat(&gu_scale(scale[0] * p.scale[0], scale[1] * p.scale[1], scale[2] * p.scale[2]), &m1);
        let clipped = root.bbox.as_ref().is_some_and(|b| view.check_bounding_box(b, &obj)) && !no_clip;
        let a = &mut self.actor2.actors[id].actor.actual_actor;
        a.placement.last_keys = last;
        if clipped {
            a.root_object_flags |= OBJECT_FLAGS_CLIPPED;
        } else {
            a.root_object_flags &= !OBJECT_FLAGS_CLIPPED;
        }
    }

    /// Port of `XformActor` (mdxActor.cpp:547): `UpdateAnims`, then the object transform.
    pub fn xform_actor(&mut self, id: ActorId, no_clip: bool) {
        if self.actor2.actors[id].actor.actual_actor.object_controller.is_none() {
            return;
        }
        self.update_actor_anim(id);
        self.transform_root_object(id, no_clip);
        // `headMatrix` points at frog[0]'s head object's objMatrix (FindFrogSubObjects).
        if let Some(name) = self.tongue.head_object.clone() {
            if self.frogger.frog[0] == Some(id) {
                if let Some(m) = self.actor_sub_object_matrix(id, &name) {
                    self.tongue.head_matrix = Some(flatten(&m));
                }
            }
        }
    }

    /// The `objMatrix` a sub-object of `id` gets from `XformActor` now.
    pub fn actor_sub_object_matrix(&self, id: ActorId, name: &str) -> Option<Mat4> {
        let a = &self.actor2.actors[id].actor.actual_actor;
        let model = a.object_controller.and_then(|m| self.models.get(m))?;
        let animated = a.animation.is_some();
        let time = a.animation.as_ref().map_or(0.0, |x| x.anim_time);
        sub_object_matrix(&model.xform_tree, name, &a.placement, animated, time)
    }

    /// Port of `FindFrogSubObjects` (PC/actor.cpp:129), head part: `headMatrix` =
    /// `&FindActorSubObject(frog[0], headNames[character])->objMatrix` (its current value is
    /// the pose of `CreateActor`'s transform at the origin until the frog is drawn).
    pub fn find_frog_sub_objects(&mut self, p: usize) {
        if self.tongue.head_object.is_some() {
            return;
        }
        let Some(f0) = self.frogger.frog[0] else { return };
        let ch = self.frogger.player[p].character as usize;
        let name = crate::frogmove::HEAD_NAMES[ch.min(7)];
        let a = &self.actor2.actors[f0].actor.actual_actor;
        let Some(model) = a.object_controller.and_then(|m| self.models.get(m)) else { return };
        if model.find_sub_object(name).is_none() {
            return;
        }
        let animated = a.animation.is_some();
        let m = sub_object_matrix(&model.xform_tree, name, &MdxPlacement::default(), animated, 0.0);
        self.tongue.head_object = Some(name.to_string());
        self.tongue.head_matrix = m.map(|m| flatten(&m));
    }

    /// `CreateActor`'s `XformActor(t, 1)` at the origin (mdxActor.cpp:953): sets the
    /// root object's initial clip flag against the current view.
    pub fn xform_new_actor(&mut self, id: ActorId) {
        self.actor2.actors[id].actor.actual_actor.placement = MdxPlacement::default();
        if self.view.valid {
            self.xform_actor(id, false);
        }
    }

    /// Port of `CopyActorList` (PC/Main.cpp:621), minus texture sliding (render only).
    pub fn copy_actor_list(&mut self) {
        let frog0 = self.frogger.frog[0].and_then(|f| self.actor2.actors.get(f)).map(|a| a.actor.position);
        for id in self.actor2.act_list.ids(&self.actor2.actors) {
            let c = &mut self.actor2.actors[id];
            if let Some(f0) = frog0 {
                c.distance_from_frog = distance_between_points_ss(&c.actor.position, &f0);
            }
            if c.actor.actual_actor.object_controller.is_none() {
                continue;
            }
            c.clipped = (c.actor.actual_actor.root_object_flags & OBJECT_FLAGS_CLIPPED != 0) as i8;
            let draw = c.draw != 0;
            let pos = c.actor.position;
            let size = c.actor.size;
            let q = c.actor.q_rot;
            let p = &mut c.actor.actual_actor.placement;
            if !draw {
                p.visible = 0;
                p.draw = 0;
                continue;
            }
            p.visible = 1;
            p.draw = 1;
            p.pos = [pos.vx as f32 * 0.1, pos.vy as f32 * 0.1, pos.vz as f32 * 0.1];
            const S: f32 = 1.0 / 4096.0;
            p.scale = [size.vx as f32 * S, size.vy as f32 * S, size.vz as f32 * S];
            if q.w != 0 || q.x != 0 || q.y != 0 || q.z != 0 {
                p.q_rot = [q.x as f32 * S, q.y as f32 * S, q.z as f32 * S, q.w as f32 * S];
            }
        }
    }

    /// Port of `ActorListDraw` (mdxActor.cpp:139): the sphere test that decides whether
    /// an actor is transformed (animated, bounding box clipped) this frame.
    pub fn actor_list_draw(&mut self, list: u32) {
        let Some(view) = self.specfx.view else {
            for id in self.actor_draw_list(list) {
                self.update_actor_anim(id);
            }
            return;
        };
        for id in self.actor_draw_list(list) {
            let a = &self.actor2.actors[id].actor.actual_actor;
            let Some(model) = a.object_controller.and_then(|m| self.models.get(m)) else { continue };
            let p = a.placement;
            let draw_override = a.flags & mdx_flags::ACTOR_ALWAYSDRAW != 0;
            let visible = if draw_override {
                true
            } else {
                let tc = model.true_centre;
                let t_pos = [tc[0] * p.scale[0], tc[1] * p.scale[1], tc[2] * p.scale[2]];
                let rotmat = quaternion_to_matrix(&p.q_rot);
                let t2 = mtx_xfm(&rotmat, t_pos);
                let t_pos = [p.pos[0] + t2[0], p.pos[1] + t2[1], p.pos[2] + t2[2]];
                let wh = view.xfm_point_f(t_pos);
                let t2 = mtx_xfm(&view.matrix, t_pos);
                let scale = p.scale[0].max(p.scale[1].max(p.scale[2]));
                let mut radius = model.actor_radius * scale;
                let depth = t2[2] + view.dist as f32;
                if depth + radius > view.near_clip && depth - radius < view.far_clip {
                    if wh[2] > view.near_clip {
                        radius = (view.fov as f32 * radius * (view.x_res * (1.0 / 640.0))) / (wh[2] + view.dist as f32);
                    }
                    (wh[0] > -radius && wh[0] < view.x_res + radius) && (wh[1] > -radius && wh[1] < view.y_res + radius)
                } else {
                    false
                }
            };
            if visible {
                self.xform_actor(id, draw_override);
            }
        }
    }

    /// `IsPointVisible` for a world point (`None` view: everything visible).
    pub fn view_point_visible(&self, p: &SVector) -> bool {
        self.is_point_visible(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn look_at_matches_oracle_layout() {
        // Looking down -z from +z: right = -x (row-vector MDX convention, as the oracle's vMatrix).
        let m = gu_look_at([0.0, 0.0, 0.0], [0.0, 0.0, 100.0], [0.0, 1.0, 0.0]);
        assert_eq!(m[0][0], -1.0);
        let p = mtx_xfm(&m, [0.0, 0.0, 50.0]);
        assert!((p[2] - 50.0).abs() < 1e-4);
    }

    #[test]
    fn bbox_in_front_is_visible() {
        let v = ViewXform::oracle(gu_look_at([0.0, 0.0, 100.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]), 700.0);
        let mut b = [[0.0; 3]; 8];
        for (i, c) in b.iter_mut().enumerate() {
            *c = [if i & 1 != 0 { 5.0 } else { -5.0 }, if i & 2 != 0 { 5.0 } else { -5.0 }, 50.0 + if i & 4 != 0 { 5.0 } else { -5.0 }];
        }
        assert!(!v.check_bounding_box(&b, &IDENTITY));
        let behind = gu_translate(0.0, 0.0, -500.0);
        assert!(v.check_bounding_box(&b, &behind));
    }
}
