//! Port of the functions in `Common/maths.c` (PC build).
//!
//! Output parameters are `&mut` like the C pointers. Where the C code is called with the
//! destination aliasing a source (e.g. `RotateVectorByQuaternionFF(&v, &v, q)`), pass a
//! copy of the source; each function documents whether that is equivalent.

use serde::{Deserialize, Serialize};

use crate::acostable::BB_ACOSTABLE;
use crate::fixed::{fdiv, fdiv2, fmul, to_fixed, Fixed};
use crate::types::{Box3D, FVector, IQuaternion, MatrixI, SVector, Vector, X, Y, Z};
use crate::util::{fsqrt, fsqrt_f, ftol32, rcos, rsin, util_calc_angle, util_sqrt, PI2};
use crate::vector::{
    cross_product_fff, cross_product_vertical, fabs, magnitude_v, scale_vector_ff, set_vector_vs, sub_vector_fff, sub_vector_ffs, sub_vector_fsf,
    sub_vector_vss, sub_vector_vsv, sub_vector_vvv,
};

/// Seed value of `rseed` (Common/maths.c:249, reset by `SeedRandom`).
pub const RSEED_INIT: u32 = 398_623;

/// Debug aid: log every `Random()` call site to stderr (trace parity work).
pub static RANDOM_LOG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The game's RNG state: global `ULONG rseed` (Common/maths.c:249).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rng {
    pub rseed: u32,
}

impl Default for Rng {
    fn default() -> Self {
        Self { rseed: RSEED_INIT }
    }
}

impl Rng {
    /// Port of `Random` (Common/maths.c:251): shift-xor generator, returns `rseed % lim`.
    /// `lim == 0` returns 0 without advancing the state.
    #[track_caller]
    pub fn random(&mut self, lim: u32) -> i32 {
        if lim == 0 {
            return 0;
        }
        if RANDOM_LOG.load(std::sync::atomic::Ordering::Relaxed) {
            eprintln!("Random({lim}) at {}", std::panic::Location::caller());
        }
        self.rseed ^= self.rseed << 1;
        if self.rseed & 0x8000_0000 == 0 {
            self.rseed = self.rseed.wrapping_add(1);
        } else if self.rseed == 0x8000_0000 {
            self.rseed = 0;
        }
        (self.rseed % lim) as i32
    }

    /// Port of `SeedRandom` (Common/maths.c:274).
    pub fn seed_random(&mut self) {
        self.rseed = RSEED_INIT;
    }
}

/// Port of `arccos` (Common/maths.c:189), PC path:
/// `(long)(acos(cos/4096.0F)*(4096/PI2))`. Returns a 4096-per-turn angle.
///
/// For `|cos| > 4096` `acos` yields NaN, which MSVC 6 `_ftol` turns into
/// `0x8000000000000000`, i.e. **0** after truncation to `long`.
pub fn arccos(cos: i32) -> i32 {
    // RETAIL: the exe runs with Direct3D's x87 control word (24-bit precision), so
    // `cos/4096.0F`, the `acos` result and the product are each rounded to `float`.
    // With 53-bit maths arccos(0) would be 1023 (PI2 = 6.283185308 > 2*pi); the
    // retail game gets 1024 (exact 90-degree turns: Orientate -> (0,-2896,0,2896)).
    let c = cos as f32 / 4096.0f32;
    let a = (c as f64).acos() as f32;
    let r = (a as f64 * (4096.0 / PI2)) as f32;
    ftol32(r as f64)
}

/// `arccos` evaluated in double precision (the source compiled on a 53-bit FPU), for
/// difftests against the C on the host.
pub fn arccos_f64(cos: i32) -> i32 {
    ftol32((cos as f64 / 4096.0).acos() * (4096.0 / PI2))
}

/// Port of `arccos` (Common/maths.c:189), PSX path (table lookup with clamping). Unused
/// by the PC build; provided for reference/enhanced profiles.
pub fn arccos_psx(mut cos: i32) -> i32 {
    cos = cos.clamp(-4096, 4096);
    if cos >= 0 {
        BB_ACOSTABLE[cos as usize] as i32
    } else {
        2048 - BB_ACOSTABLE[(-cos) as usize] as i32
    }
}

/// Port of `fixedRotateVectorByRotationS` (Common/maths.c:83). `result` may not alias.
pub fn fixed_rotate_vector_by_rotation_s(result: &mut FVector, vect: &SVector, rot: &IQuaternion) {
    let (vx, vy, vz) = (vect.vx as i32, vect.vy as i32, vect.vz as i32);
    let mut m = vx.wrapping_mul(rot.x) >> 12;
    m = m.wrapping_add(vy.wrapping_mul(rot.y) >> 12);
    m = m.wrapping_add(vz.wrapping_mul(rot.z) >> 12);

    let m_vec = FVector { vx: m.wrapping_mul(rot.x) >> 12, vy: m.wrapping_mul(rot.y) >> 12, vz: m.wrapping_mul(rot.z) >> 12 };
    let mut p_vec = FVector::default();
    sub_vector_fsf(&mut p_vec, vect, &m_vec);
    rotate_tail(result, &m_vec, &p_vec, rot, || FVector { vx, vy, vz }, fdiv);
}

/// Shared tail of `fixedRotateVectorByRotation{,S}` (Common/maths.c:108-144, 332-367).
fn rotate_tail(result: &mut FVector, m_vec: &FVector, p_vec: &FVector, rot: &IQuaternion, copy: impl FnOnce() -> FVector, div: fn(Fixed, Fixed) -> Fixed) {
    let mut v_vec = FVector::default();
    let rot_v = FVector::new(rot.x, rot.y, rot.z);
    // This version does scale down
    cross_product_fff(&mut v_vec, &rot_v, p_vec);

    let mut m = magnitude_f(p_vec);
    if m == 0 {
        // COPYVECTOR(result, vect). BUG(original): for the S variant this copies the
        // short components without converting them to fixed point.
        *result = copy();
    } else {
        let n = magnitude_f(&v_vec);
        if n != 0 {
            m = div(m, n);
            scale_vector_ff(&mut v_vec, m);
        }
        let cos_theta = rcos(rot.w);
        let sin_theta = rsin(rot.w);
        let c = |mv: i32, pv: i32, vv: i32| mv.wrapping_add(fmul(cos_theta, pv)).wrapping_add(fmul(sin_theta, vv));
        result.vx = c(m_vec.vx, p_vec.vx, v_vec.vx);
        result.vy = c(m_vec.vy, p_vec.vy, v_vec.vy);
        result.vz = c(m_vec.vz, p_vec.vz, v_vec.vz);
    }
}

/// Port of `fixedRotateVectorByRotation` (Common/maths.c:308): rotate by axis (`x,y,z`,
/// 4096 = 1) and angle `w`. Equivalent to the C when `result` aliases `vect`.
pub fn fixed_rotate_vector_by_rotation(result: &mut FVector, vect: &FVector, rot: &IQuaternion) {
    let v = *vect;
    let mut m = v.vx.wrapping_mul(rot.x) >> 12;
    m = m.wrapping_add(v.vy.wrapping_mul(rot.y) >> 12);
    m = m.wrapping_add(v.vz.wrapping_mul(rot.z) >> 12);

    let m_vec = FVector { vx: m.wrapping_mul(rot.x) >> 12, vy: m.wrapping_mul(rot.y) >> 12, vz: m.wrapping_mul(rot.z) >> 12 };
    let mut p_vec = FVector::default();
    sub_vector_fff(&mut p_vec, &v, &m_vec);
    rotate_tail(result, &m_vec, &p_vec, rot, || v, fdiv);
}

/// Port of `fixedQuaternionMultiply` (Common/maths.c:150). Destination may equal a source
/// in C; here pass copies.
pub fn fixed_quaternion_multiply(dest: &mut IQuaternion, src1: &IQuaternion, src2: &IQuaternion) {
    let (a, b) = (*src1, *src2);
    let m = |p: i32, q: i32| p.wrapping_mul(q);
    let dp = m(a.x, b.x).wrapping_add(m(a.y, b.y)).wrapping_add(m(a.z, b.z));
    let x = m(a.w, b.x).wrapping_add(m(b.w, a.x)).wrapping_add(m(a.y, b.z)).wrapping_sub(m(a.z, b.y)) >> 12;
    let y = m(a.w, b.y).wrapping_add(m(b.w, a.y)).wrapping_add(m(a.z, b.x)).wrapping_sub(m(a.x, b.z)) >> 12;
    let z = m(a.w, b.z).wrapping_add(m(b.w, a.z)).wrapping_add(m(a.x, b.y)).wrapping_sub(m(a.y, b.x)) >> 12;
    dest.w = m(a.w, b.w).wrapping_sub(dp) >> 12;
    dest.x = x;
    dest.y = y;
    dest.z = z;
}

/// Port of `fixedGetQuaternionFromRotation` (Common/maths.c:291): axis/angle -> quaternion.
pub fn fixed_get_quaternion_from_rotation(dest_q: &mut IQuaternion, src_q: &IQuaternion) {
    let s = *src_q;
    let theta_over2 = s.w / 2;
    let sin_theta_over2 = rsin(theta_over2);
    dest_q.w = rcos(theta_over2);
    dest_q.x = sin_theta_over2.wrapping_mul(s.x) >> 12;
    dest_q.y = sin_theta_over2.wrapping_mul(s.y) >> 12;
    dest_q.z = sin_theta_over2.wrapping_mul(s.z) >> 12;
}

/// Port of `DistanceBetweenPoints2D` (Common/maths.c:384).
pub fn distance_between_points_2d(v1: &Vector, v2: &Vector) -> Fixed {
    let x = v1.vx.wrapping_sub(v2.vx);
    let z = v1.vz.wrapping_sub(v2.vz);
    magnitude_2dv(&Vector::new(x, 0, z))
}

/// Port of `DistanceBetweenPointsVV` (Common/maths.c:407).
pub fn distance_between_points_vv(v1: &Vector, v2: &Vector) -> Fixed {
    let mut t = Vector::default();
    sub_vector_vvv(&mut t, v1, v2);
    magnitude_v(&t)
}

/// Port of `DistanceBetweenPointsSS` (Common/maths.c:415).
pub fn distance_between_points_ss(v1: &SVector, v2: &SVector) -> Fixed {
    let mut t = Vector::default();
    sub_vector_vss(&mut t, v1, v2);
    magnitude_v(&t)
}

/// Port of `DistanceBetweenPointsSV` (Common/maths.c:428).
pub fn distance_between_points_sv(v1: &SVector, v2: &Vector) -> Fixed {
    let mut t = Vector::default();
    sub_vector_vsv(&mut t, v1, v2);
    magnitude_v(&t)
}

/// Port of `DistanceBetweenPointsSF` (Common/maths.c:441).
pub fn distance_between_points_sf(v1: &SVector, v2: &FVector) -> Fixed {
    let mut t = FVector::default();
    sub_vector_fsf(&mut t, v1, v2);
    magnitude_f(&t)
}

/// Port of `DistanceBetweenPointsFS` (Common/maths.c:450).
pub fn distance_between_points_fs(v1: &FVector, v2: &SVector) -> Fixed {
    let mut t = FVector::default();
    sub_vector_ffs(&mut t, v1, v2);
    magnitude_f(&t)
}

/// Port of `DistanceBetweenPointsFF` (Common/maths.c:458).
pub fn distance_between_points_ff(v1: &FVector, v2: &FVector) -> Fixed {
    let mut t = FVector::default();
    sub_vector_fff(&mut t, v1, v2);
    magnitude_f(&t)
}

/// Port of `DistanceBetweenPointsSqrSS` (Common/maths.c:474).
pub fn distance_between_points_sqr_ss(v1: &SVector, v2: &SVector) -> i32 {
    let mut t = Vector::default();
    sub_vector_vss(&mut t, v1, v2);
    t.vx.wrapping_mul(t.vx).wrapping_add(t.vy.wrapping_mul(t.vy)).wrapping_add(t.vz.wrapping_mul(t.vz))
}

/// Port of `DistanceBetweenPointsSqrFS` (Common/maths.c:493).
pub fn distance_between_points_sqr_fs(v1: &FVector, v2: &SVector) -> i32 {
    let mut t = FVector::default();
    sub_vector_ffs(&mut t, v1, v2);
    (fmul(t.vx, t.vx) >> 12).wrapping_add(fmul(t.vy, t.vy) >> 12).wrapping_add(fmul(t.vz, t.vz) >> 12)
}

#[inline]
fn in_range(p: i32, lo: i16, size: i32) -> bool {
    p >= lo as i32 && p <= (lo as i32).wrapping_add(size)
}

/// Port of `IsPointWithinBox` (Common/maths.c:519).
pub fn is_point_within_box(bx: &Box3D, pos: &Vector) -> bool {
    in_range(pos.vx, bx.pos.vx, bx.size.vx) && in_range(pos.vy, bx.pos.vy, bx.size.vy) && in_range(pos.vz, bx.pos.vz, bx.size.vz)
}

/// Port of `IsPointWithinBoxPlusHeight` (Common/maths.c:536).
pub fn is_point_within_box_plus_height(bx: &Box3D, pos: &Vector, height: Fixed) -> bool {
    in_range(pos.vx, bx.pos.vx, bx.size.vx)
        && pos.vy >= bx.pos.vy as i32
        && pos.vy <= (bx.pos.vy as i32).wrapping_add(bx.size.vy).wrapping_add(height >> 12)
        && in_range(pos.vz, bx.pos.vz, bx.size.vz)
}

/// Port of `IsPointWithinBox2D` (Common/maths.c:553).
pub fn is_point_within_box_2d(bx: &Box3D, pos: &Vector) -> bool {
    in_range(pos.vx, bx.pos.vx, bx.size.vx) && in_range(pos.vz, bx.pos.vz, bx.size.vz)
}

/// Port of `RotateVector2D` (Common/maths.c:569): rotate X/Z about the Y axis.
pub fn rotate_vector_2d(dest: &mut FVector, source: &FVector, angle: Fixed) {
    let s = *source;
    let angle = angle.wrapping_neg();
    let cosangle = rcos(angle);
    let sinangle = rsin(angle);
    dest.vy = s.vy;
    dest.vx = fmul(cosangle, s.vx).wrapping_add(fmul(sinangle.wrapping_neg(), s.vz));
    dest.vz = fmul(cosangle, s.vz).wrapping_add(fmul(sinangle, s.vx));
}

/// Port of `RotateVector2DX` (Common/maths.c:598): rotate Y/Z about the X axis.
pub fn rotate_vector_2dx(dest: &mut FVector, source: &FVector, angle: Fixed) {
    let s = *source;
    let cosangle = rcos(angle);
    let sinangle = rsin(angle);
    dest.vx = s.vx;
    dest.vy = fmul(cosangle, s.vy).wrapping_add(fmul(sinangle.wrapping_neg(), s.vz));
    dest.vz = fmul(cosangle, s.vz).wrapping_add(fmul(sinangle, s.vy));
}

/// Port of `FlipVector` (Common/maths.c:628).
pub fn flip_vector(vector: &mut Vector) {
    vector.vx = vector.vx.wrapping_neg();
    vector.vy = vector.vy.wrapping_neg();
    vector.vz = vector.vz.wrapping_neg();
}

/// Port of `FlipVector2D` (Common/maths.c:642).
pub fn flip_vector_2d(vector: &mut Vector) {
    vector.vx = vector.vx.wrapping_neg();
    vector.vz = vector.vz.wrapping_neg();
}

/// Port of `FindShortestAngle` (Common/maths.c:655). Uses the old radian constants
/// (12868 = PI, 51472 = 4*PI) on fixed angles, as the original does.
pub fn find_shortest_angle(val1: Fixed, val2: Fixed) -> Fixed {
    if fabs(val1.wrapping_sub(val2)) > 12868 {
        let temp = if val1 > val2 { val2.wrapping_sub(val1) } else { val1.wrapping_sub(val2) };
        temp.wrapping_add(51472)
    } else {
        fabs(val1.wrapping_sub(val2))
    }
}

/// Port of `FindShortestAngleSigned` (Common/maths.c:693).
pub fn find_shortest_angle_signed(ang1: Fixed, ang2: Fixed) -> Fixed {
    let temp = ang1.wrapping_sub(ang2);
    if temp > -2048 && temp <= 2048 {
        temp
    } else if temp > 2048 {
        ang1.wrapping_sub(ang2).wrapping_sub(4096)
    } else {
        ang1.wrapping_add(4096).wrapping_sub(ang2)
    }
}

/// Port of `MagnitudeS` (Common/maths.c:758).
pub fn magnitude_s(vect: &SVector) -> Fixed {
    let mut tmp = Vector::default();
    set_vector_vs(&mut tmp, vect);
    magnitude_v(&tmp)
}

/// Shared range-reduced magnitude used by `MagnitudeF`/`Magnitude2DF`/`MakeUnit`/
/// `NormaliseQuaternion`: returns `(utilSqrt(calc), s)` with components pre-shifted by
/// `s` so that their OR is <= 26754. `n_sq` is how many of `comps` are squared (all of
/// them except for the `NormaliseQuaternion` quirk).
#[inline]
fn reduced_sqrt(comps: &[u32], shift_count: usize) -> (u32, u32) {
    let mut num = comps.iter().fold(0u32, |a, &c| a | c);
    let mut s: u32 = 0;
    while num > 26754 {
        num >>= 2;
        s += 2;
    }
    let calc = comps.iter().enumerate().fold(0u32, |acc, (i, &c)| {
        let c = if i < shift_count { c >> s } else { c };
        acc.wrapping_add(c.wrapping_mul(c))
    });
    (util_sqrt(calc), s)
}

/// Port of `MagnitudeF` (Common/maths.c:771). `16-s` may underflow; the x86 shift masks
/// the count to 5 bits, reproduced with `wrapping_shr`.
pub fn magnitude_f(v: &FVector) -> Fixed {
    let c = [v.vx.wrapping_abs() as u32, v.vy.wrapping_abs() as u32, v.vz.wrapping_abs() as u32];
    let (l, s) = reduced_sqrt(&c, 3);
    l.wrapping_shr(16u32.wrapping_sub(s)) as i32
}

/// Port of `Magnitude2DF` (Common/maths.c:814).
pub fn magnitude_2df(v: &FVector) -> Fixed {
    let c = [v.vx.wrapping_abs() as u32, v.vz.wrapping_abs() as u32];
    let (l, s) = reduced_sqrt(&c, 2);
    l.wrapping_shr(16u32.wrapping_sub(s)) as i32
}

/// Port of `Magnitude2DV` (Common/maths.c:851).
pub fn magnitude_2dv(vect: &Vector) -> Fixed {
    fsqrt(vect.vx.wrapping_mul(vect.vx).wrapping_add(vect.vz.wrapping_mul(vect.vz)))
}

/// Shared normalise step of `MakeUnit` / `NormaliseQuaternion` (Common/maths.c:925-955).
fn normalise_components(comps: &mut [i32], num: u32, l: u32, s: u32) {
    let m = l.wrapping_shr(16u32.wrapping_sub(s)) as i32;
    if m == 0 {
        return;
    }
    // find top bit
    let mut n = num as i32;
    let mut tb: i32 = 31;
    while (n as u32) & 0x8000_0000 == 0 {
        n = n.wrapping_shl(1);
        tb -= 1;
    }
    if tb > 18 {
        let s = (30 - tb) as u32;
        let d = (tb - 18) as u32;
        let div = m >> d;
        for c in comps.iter_mut() {
            *c = c.wrapping_shl(s).wrapping_div(div);
        }
    } else {
        for c in comps.iter_mut() {
            *c = c.wrapping_shl(12).wrapping_div(m);
        }
    }
}

/// Port of `MakeUnit` (Common/maths.c:868): normalise to length 4096.
///
/// # Panics
/// Only where the original would raise a divide-by-zero (not reachable for `m != 0`).
pub fn make_unit(vect: &mut FVector) {
    let c = [fabs(vect.vx) as u32, fabs(vect.vy) as u32, fabs(vect.vz) as u32];
    let num = c[0] | c[1] | c[2];
    let (l, s) = reduced_sqrt(&c, 3);
    let mut comps = [vect.vx, vect.vy, vect.vz];
    normalise_components(&mut comps, num, l, s);
    [vect.vx, vect.vy, vect.vz] = comps;
}

/// Port of `MakeUnit2D` (Common/maths.c:964) (declared as `MakeUnit2DF` in maths.h).
pub fn make_unit_2d(vect: &mut FVector) {
    let m = magnitude_2df(vect);
    if m != to_fixed(0) {
        vect.vx = fdiv(vect.vx, m);
        vect.vz = fdiv(vect.vz, m);
    }
}

/// Port of `Aabs` (Common/maths.c:982): wrap an angle into 0..=4096.
pub fn aabs(mut angle: Fixed) -> Fixed {
    if angle > 4096 {
        angle -= 4096;
    }
    if angle < 0 {
        angle += 4096;
    }
    angle
}

/// Port of `SineWave` (Common/maths.c:1002).
pub fn sine_wave(freq: Fixed, phase: Fixed, base: i32) -> Fixed {
    let temp = to_fixed(base).wrapping_add(phase);
    rsin(fdiv(temp, freq))
}

/// Port of `SineWave2` (Common/maths.c:1010).
pub fn sine_wave2(freq: Fixed, phase: Fixed) -> Fixed {
    rsin(fdiv(fmul(phase, 12868).wrapping_mul(2), freq))
}

/// Port of `SetVectorLength` (Common/maths.c:1019).
pub fn set_vector_length(vect: &mut Vector, length: Fixed) {
    let m = fdiv(magnitude_v(vect), length);
    if m != to_fixed(0) {
        vect.vx = fdiv(to_fixed(vect.vx), m) >> 12;
        vect.vy = fdiv(to_fixed(vect.vy), m) >> 12;
        vect.vz = fdiv(to_fixed(vect.vz), m) >> 12;
    }
}

/// Port of `CalculateQuatForPlane2` (Common/maths.c:1202). `vert_q` is the global `vertQ`
/// (initially [`crate::VERT_Q`]), whose `w` this function overwrites.
pub fn calculate_quat_for_plane2(y_rot: Fixed, q_aim: &mut IQuaternion, normal: &FVector, vert_q: &mut IQuaternion) {
    let mut temp_rot = IQuaternion::default();
    if normal.vy == -4096 {
        vert_q.w = aabs(4096i32.wrapping_sub(y_rot));
        temp_rot = IQuaternion::new(4096, 0, 0, 2048);
    } else {
        vert_q.w = 0;
        let mut temp_vect = FVector::new(normal.vx, 0, normal.vz);
        make_unit_2d(&mut temp_vect);
        let mut r = FVector::default();
        cross_product_vertical(&mut r, &temp_vect);
        temp_rot.x = r.vx;
        temp_rot.y = r.vy;
        temp_rot.z = r.vz;
        temp_rot.w = arccos(normal.vy);
    }
    let mut temp_q = IQuaternion::default();
    fixed_get_quaternion_from_rotation(q_aim, vert_q);
    fixed_get_quaternion_from_rotation(&mut temp_q, &temp_rot);
    let a = *q_aim;
    fixed_quaternion_multiply(q_aim, &temp_q, &a);
}

/// Port of `IQuatSlerp` (Common/maths.c:1248). Equivalent to the C when `dquat` aliases
/// `sp2` (as in babyfrog.c); the C never calls it with `dquat` aliasing `src1`.
pub fn iquat_slerp(src1: &IQuaternion, sp2: &IQuaternion, t: Fixed, dquat: &mut IQuaternion) {
    let (a, b) = (*src1, *sp2);
    let sq4 = |x: i32, y: i32, z: i32, w: i32| fmul(x, x).wrapping_add(fmul(y, y)).wrapping_add(fmul(z, z)).wrapping_add(fmul(w, w));
    let pdist = sq4(a.x.wrapping_sub(b.x), a.y.wrapping_sub(b.y), a.z.wrapping_sub(b.z), a.w.wrapping_sub(b.w));
    let ndist = sq4(a.x.wrapping_add(b.x), a.y.wrapping_add(b.y), a.z.wrapping_add(b.z), a.w.wrapping_add(b.w));

    let src2 = if pdist < ndist {
        // +ve arc is shorter, so interpolate along that
        b
    } else {
        // -ve arc is shorter, so interpolate along that
        IQuaternion::new(b.x.wrapping_neg(), b.y.wrapping_neg(), b.z.wrapping_neg(), b.w.wrapping_neg())
    };

    let cosom = fmul(a.x, src2.x).wrapping_add(fmul(a.y, src2.y)).wrapping_add(fmul(a.z, src2.z)).wrapping_add(fmul(a.w, src2.w));

    if 4096i32.wrapping_add(cosom) > 1 {
        let (sclp, sclq);
        if 4096i32.wrapping_sub(cosom) > 1 {
            let omega = arccos(cosom);
            let sinom = fdiv2(4096, rsin(omega)); // ????? Look at..........
            sclp = fmul(rsin(fmul(4096i32.wrapping_sub(t), omega)), sinom);
            sclq = fmul(rsin(fmul(t, omega)), sinom);
        } else {
            sclp = 4096i32.wrapping_sub(t);
            sclq = t;
        }
        dquat.x = fmul(sclp, a.x).wrapping_add(fmul(sclq, src2.x));
        dquat.y = fmul(sclp, a.y).wrapping_add(fmul(sclq, src2.y));
        dquat.z = fmul(sclp, a.z).wrapping_add(fmul(sclq, src2.z));
        dquat.w = fmul(sclp, a.w).wrapping_add(fmul(sclq, src2.w));
    } else {
        dquat.x = a.y.wrapping_neg();
        dquat.y = a.x;
        dquat.z = a.w.wrapping_neg();
        dquat.w = a.z;
        let sclp = rsin(4096i32.wrapping_sub(t) / 4);
        let sclq = rsin(t / 4);
        dquat.x = fmul(sclp, a.x).wrapping_add(fmul(sclq, dquat.x));
        dquat.y = fmul(sclp, a.y).wrapping_add(fmul(sclq, dquat.y));
        dquat.z = fmul(sclp, a.z).wrapping_add(fmul(sclq, dquat.z));
    }
}

/// Port of `RotateVectorByQuaternionFF` (Common/maths.c:1329). Equivalent to the C when
/// `result` aliases `vect`.
pub fn rotate_vector_by_quaternion_ff(result: &mut FVector, vect: &FVector, q: &IQuaternion) {
    let mut rot = IQuaternion::default();
    fixed_get_rotation_from_quaternion(&mut rot, q);
    fixed_rotate_vector_by_rotation(result, vect, &rot);
}

/// Port of `fixedGetRotationFromQuaternion` (Common/maths.c:1352): quaternion -> axis/angle.
pub fn fixed_get_rotation_from_quaternion(dest_q: &mut IQuaternion, src_q: &IQuaternion) {
    let s = *src_q;
    let theta = arccos(s.w).wrapping_mul(2);
    let sin_theta_over2 = rsin(theta / 2);

    dest_q.w = theta;
    if sin_theta_over2 != 0 {
        dest_q.x = fdiv(s.x, sin_theta_over2);
        dest_q.y = fdiv(s.y, sin_theta_over2);
        dest_q.z = fdiv(s.z, sin_theta_over2);

        let m = fsqrt_f(fmul(dest_q.x, dest_q.x).wrapping_add(fmul(dest_q.y, dest_q.y)).wrapping_add(fmul(dest_q.z, dest_q.z)));
        if m != 0 {
            dest_q.x = fdiv(dest_q.x, m);
            dest_q.y = fdiv(dest_q.y, m);
            dest_q.z = fdiv(dest_q.z, m);
        }
    } else {
        dest_q.x = 0;
        dest_q.y = 4096;
        dest_q.z = 0;
    }
}

/// Euler order fields unpacked by `EulGetOrd` (Common/maths.c:43).
#[derive(Clone, Copy)]
struct EulOrd {
    i: usize,
    j: usize,
    k: usize,
    n: u32,
    s: u32,
    f: u32,
}

/// Port of `EulGetOrd` (Common/maths.c:43).
const fn eul_get_ord(ord: u32) -> EulOrd {
    const EUL_SAFE: [usize; 4] = [0, 1, 2, 0];
    const EUL_NEXT: [usize; 4] = [1, 2, 0, 1];
    let mut o = ord;
    let f = o & 1;
    o >>= 1;
    let s = o & 1;
    o >>= 1;
    let n = o & 1;
    o >>= 1;
    let i = EUL_SAFE[(o & 3) as usize];
    let j = EUL_NEXT[i + n as usize];
    let k = EUL_NEXT[i + 1 - n as usize];
    EulOrd { i, j, k, n, s, f }
}

/// Port of `EulOrd` (Common/maths.c:46).
const fn eul_ord(i: u32, p: u32, r: u32, f: u32) -> u32 {
    (((((i << 1) + p) << 1) + r) << 1) + f
}

/// Port of `EulOrdZYZr` (Common/maths.c:73).
const EUL_ORD_ZYZR: u32 = eul_ord(Z as u32, 1, 1, 1);

/// Port of `Eul_FromQuat` (Common/maths.c:1391): quaternion -> Euler angles (ZYZr).
pub fn eul_from_quat(dest: &mut SVector, q: &IQuaternion) {
    let nq = fmul(q.x, q.x).wrapping_add(fmul(q.y, q.y)).wrapping_add(fmul(q.z, q.z)).wrapping_add(fmul(q.w, q.w));
    let s = if nq > 0 { fdiv(8192, nq) } else { 0 };
    let (xs, ys, zs) = (fmul(q.x, s), fmul(q.y, s), fmul(q.z, s));
    let (wx, wy, wz) = (fmul(q.w, xs), fmul(q.w, ys), fmul(q.w, zs));
    let (xx, xy, xz) = (fmul(q.x, xs), fmul(q.x, ys), fmul(q.x, zs));
    let (yy, yz, zz) = (fmul(q.y, ys), fmul(q.y, zs), fmul(q.z, zs));

    let mut m = MatrixI::default();
    m.matrix[0][0] = 4096i32.wrapping_sub(yy.wrapping_add(zz));
    m.matrix[0][1] = xy.wrapping_sub(wz);
    m.matrix[0][2] = xz.wrapping_add(wy);
    m.matrix[1][0] = xy.wrapping_add(wz);
    m.matrix[1][1] = 4096i32.wrapping_sub(xx.wrapping_add(zz));
    m.matrix[1][2] = yz.wrapping_sub(wx);
    m.matrix[2][0] = xz.wrapping_sub(wy);
    m.matrix[2][1] = yz.wrapping_add(wx);
    m.matrix[2][2] = 4096i32.wrapping_sub(xx.wrapping_add(yy));
    m.matrix[3][3] = 4096;

    eul_from_hmatrix(dest, &m);
}

/// Port of `Eul_FromHMatrix` (Common/maths.c:1421) for order `EulOrdZYZr`.
pub fn eul_from_hmatrix(dest: &mut SVector, mm: &MatrixI) {
    let EulOrd { i, j, k, n, s, f } = eul_get_ord(EUL_ORD_ZYZR);
    let m = &mm.matrix;
    let ang = |a: i32, b: i32| util_calc_angle(a, b) as i16;

    if s == 1 {
        let sy = (util_sqrt(m[i][j].wrapping_mul(m[i][j]).wrapping_add(m[i][k].wrapping_mul(m[i][k])) as u32) >> 4) as i32;
        if sy > 1 {
            dest.vx = ang(m[i][j], m[i][k]);
            dest.vy = ang(sy, m[i][i]);
            dest.vz = ang(m[j][i], m[k][i].wrapping_neg());
        } else {
            dest.vx = ang(m[j][k].wrapping_neg(), m[j][j]);
            dest.vy = ang(sy, m[i][i]);
            dest.vz = 0;
            dest.vy = dest.vy.wrapping_add(2048);
        }
    } else {
        let cy = (util_sqrt(m[i][i].wrapping_mul(m[i][i]).wrapping_add(m[j][i].wrapping_mul(m[j][i])) as u32) >> 4) as i32;
        if cy > 1 {
            dest.vx = ang(m[k][j], m[k][k]);
            dest.vy = ang(m[k][i].wrapping_neg(), cy);
            dest.vz = ang(m[j][i], m[i][i]);
        } else {
            dest.vx = ang(m[j][k].wrapping_neg(), m[j][j]);
            dest.vy = ang(m[k][i].wrapping_neg(), cy);
            dest.vz = 0;
        }
    }
    if n == 1 {
        dest.vx = dest.vx.wrapping_neg();
        dest.vy = dest.vy.wrapping_neg();
        dest.vz = dest.vz.wrapping_neg();
    }
    if f == 1 {
        core::mem::swap(&mut dest.vx, &mut dest.vz);
    }
}

/// Port of `Eul_ToQuat` (Common/maths.c:1479): Euler angles (ZYZr) -> quaternion.
pub fn eul_to_quat(qu: &mut IQuaternion, src: &SVector) {
    let mut v = *src; // bb - work on copy
    let EulOrd { i, j, k, n, s, f } = eul_get_ord(EUL_ORD_ZYZR);
    if f == 1 {
        core::mem::swap(&mut v.vx, &mut v.vz);
    }
    if n == 1 {
        v.vy = v.vy.wrapping_neg();
    }
    let (ti, tj, th) = ((v.vx as i32) >> 1, (v.vy as i32) >> 1, (v.vz as i32) >> 1);
    let (ci, cj, ch) = (rcos(ti), rcos(tj), rcos(th));
    let (si, sj, sh) = (rsin(ti), rsin(tj), rsin(th));
    let (cc, cs, sc, ss) = (fmul(ci, ch), fmul(ci, sh), fmul(si, ch), fmul(si, sh));
    let mut a = [0i32; 3];
    if s == 1 {
        a[i] = fmul(cj, cs.wrapping_add(sc)); // Could speed up with
        a[j] = fmul(sj, cc.wrapping_add(ss)); // trig identities.
        a[k] = fmul(sj, cs.wrapping_sub(sc));
        qu.w = fmul(cj, cc.wrapping_sub(ss));
    } else {
        a[i] = fmul(cj, sc).wrapping_sub(fmul(sj, cs));
        a[j] = fmul(cj, ss).wrapping_add(fmul(sj, cc));
        a[k] = fmul(cj, cs).wrapping_sub(fmul(sj, sc));
        qu.w = fmul(cj, cc).wrapping_add(fmul(sj, ss));
    }
    if n == 1 {
        a[j] = a[j].wrapping_neg();
    }
    qu.x = a[X];
    qu.y = a[Y];
    qu.z = a[Z];
}

/// Port of `RotateVectorByXZRotation` (Common/maths.c:1523). Equivalent to the C when
/// `result` aliases `vect`.
pub fn rotate_vector_by_xz_rotation(result: &mut FVector, vect: &FVector, rot: &IQuaternion) {
    let v = *vect;
    let mut m = fmul(v.vx, rot.x);
    m = m.wrapping_add(fmul(v.vz, rot.z));
    let m_vec = FVector::new(fmul(m, rot.x), 0, fmul(m, rot.z));
    let mut p_vec = FVector::default();
    sub_vector_fff(&mut p_vec, &v, &m_vec);

    let mut v_vec =
        FVector { vx: fmul(rot.z, p_vec.vx).wrapping_neg(), vy: fmul(rot.z, p_vec.vy).wrapping_sub(fmul(rot.x, p_vec.vz)), vz: fmul(rot.x, p_vec.vz) };

    m = magnitude_f(&p_vec);
    if m == 0 {
        *result = v;
    } else {
        let n = magnitude_f(&v_vec);
        if n != 0 {
            m = fdiv2(m, n);
            scale_vector_ff(&mut v_vec, m);
        }
        let cos_theta = rcos(rot.w);
        let sin_theta = rsin(rot.w);
        result.vx = m_vec.vx.wrapping_add(fmul(cos_theta, p_vec.vx)).wrapping_add(fmul(sin_theta, v_vec.vx));
        result.vy = fmul(cos_theta, p_vec.vy).wrapping_add(fmul(sin_theta, v_vec.vy));
        result.vz = m_vec.vz.wrapping_add(fmul(cos_theta, p_vec.vz)).wrapping_add(fmul(sin_theta, v_vec.vz));
    }
}

/// Port of `GetQuaternionFromXZRotation` (Common/maths.c:1569).
pub fn get_quaternion_from_xz_rotation(dest_q: &mut IQuaternion, src_q: &IQuaternion) {
    let s = *src_q;
    let theta_over2 = s.w >> 1;
    let sin_theta_over2 = rsin(theta_over2);
    dest_q.w = rcos(theta_over2);
    dest_q.x = fmul(sin_theta_over2, s.x);
    dest_q.y = 0;
    dest_q.z = fmul(sin_theta_over2, s.z);
}

/// Port of `NormalToQuaternion` (Common/maths.c:1595).
pub fn normal_to_quaternion(q: &mut IQuaternion, normal: &SVector) {
    let (nx, ny, nz) = (normal.vx as i32, normal.vy as i32, normal.vz as i32);
    if nx != 0 {
        q.z = fsqrt_f(fdiv2(4096, 4096i32.wrapping_add(fdiv2(fmul(nz, nz), fmul(nx, nx)))));
        if nx > 0 {
            q.z = q.z.wrapping_neg();
        }
        q.x = nz.wrapping_mul(q.z).wrapping_neg().wrapping_div(nx);
        q.w = arccos(ny) / 2;

        q.x = q.x.wrapping_mul(rsin(q.w));
        q.z = q.z.wrapping_mul(rsin(q.w));
        q.w = rcos(q.w);
    } else if nz != 0 {
        q.z = 0;
        q.w = arccos(ny) / 2;
        q.x = if nz > 0 { rsin(q.w) } else { rsin(q.w).wrapping_neg() };
        q.w = rcos(q.w);
    } else if ny > 0 {
        q.x = 0;
        q.z = 0;
        q.w = 4096;
    } else {
        q.x = 0;
        q.z = -4096;
        q.w = 0;
    }
    q.y = 0;
}

/// Port of `NormaliseQuaternion` (Common/maths.c:1643): `MakeUnit` in 4D.
///
/// BUG(original): `w` is never pre-shifted by `s` before squaring (only x, y, z are).
pub fn normalise_quaternion(vect: &mut IQuaternion) {
    let c = [fabs(vect.x) as u32, fabs(vect.y) as u32, fabs(vect.z) as u32, fabs(vect.w) as u32];
    let num = c[0] | c[1] | c[2] | c[3];
    let (l, s) = reduced_sqrt(&c, 3);
    let mut comps = [vect.x, vect.y, vect.z, vect.w];
    normalise_components(&mut comps, num, l, s);
    [vect.x, vect.y, vect.z, vect.w] = comps;
}

/// Port of `SlideVectorToVectorFF` (Common/maths.c:1792). `game_speed` is the global
/// `gameSpeed`. Returns `TRUE` when `vect` reached `dest`.
pub fn slide_vector_to_vector_ff(vect: &mut FVector, dest: &FVector, speed: Fixed, game_speed: Fixed) -> bool {
    let speed = fmul(speed, game_speed);
    let mut temp_vect = FVector::default();
    sub_vector_fff(&mut temp_vect, dest, vect);
    let mut m = magnitude_f(&temp_vect);
    if m < speed {
        *vect = *dest;
        true
    } else {
        m = fdiv(speed, m);
        scale_vector_ff(&mut temp_vect, m);
        vect.vx = vect.vx.wrapping_add(temp_vect.vx);
        vect.vy = vect.vy.wrapping_add(temp_vect.vy);
        vect.vz = vect.vz.wrapping_add(temp_vect.vz);
        false
    }
}
