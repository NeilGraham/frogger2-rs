//! MDX maths (`MDX Library/mdxMath.cpp`, `mdxMath.h`).
//!
//! Matrices are `[[f32; 4]; 4]` in the original row-vector convention:
//! a point transforms as `p' = p · M` (`guMtxXFMF`), translation lives in
//! row 3, and `PushMatrix(a)` computes `a · top`.

#[cfg(not(feature = "std"))]
use crate::fmath::FloatExt;
use crate::prelude::*;

pub use f2_formats::objbank::{Mat4, Quat, Vec3};

pub const IDENTITY: Mat4 = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];

const PI2: f64 = core::f64::consts::PI * 2.0;
/// `EPSILON` (mdxMath.h).
pub const EPSILON: f32 = 0.00001;
/// `HALFPI` (mdxMath.h).
pub const HALFPI: f32 = 1.570_796_4;

struct Tables {
    sin: [f32; 4096],
    acos: [f32; 4096],
}

/// Port of `InitMaths` (mdxMath.cpp:53): the lookup tables behind `sinFast`/`acosFast`.
fn tables() -> &'static Tables {
    static T: once_cell::race::OnceBox<Tables> = once_cell::race::OnceBox::new();
    T.get_or_init(|| {
        let mut t = Box::new(Tables { sin: [0.0; 4096], acos: [0.0; 4096] });
        for i in 0..4096 {
            t.acos[i] = ((i as f64 - 2048.0) / 2048.0).acos() as f32;
            t.sin[i] = ((i as f64 * PI2) / 4096.0).sin() as f32;
        }
        t
    })
}

/// `sinFast(x)` (mdxMath.cpp:48): `sinTable[(long)(x*(4096/PI2)) & 4095]`.
pub fn sin_fast(x: f32) -> f32 {
    let i = (x as f64 * (4096.0 / PI2)) as i64;
    tables().sin[(i & 4095) as usize]
}

/// `acosFast(x)` (mdxMath.cpp:49): `acosTable[(long)(x*2048+2048) & 4095]`.
pub fn acos_fast(x: f32) -> f32 {
    let i = ((x as f64 * 2048.0) + 2048.0) as i64;
    tables().acos[(i & 4095) as usize]
}

/// Port of the `PushMatrix` macro body (mdxMath.h:86): `a · b`, forcing an affine result.
#[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_d")]
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

/// Port of `guScaleF` (mdxMath.cpp:84).
pub fn gu_scale(dx: f32, dy: f32, dz: f32) -> Mat4 {
    let mut a = IDENTITY;
    a[0][0] = dx;
    a[1][1] = dy;
    a[2][2] = dz;
    a
}

/// Port of `guTranslateF` (mdxMath.cpp:93).
pub fn gu_translate(dx: f32, dy: f32, dz: f32) -> Mat4 {
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

/// Port of `guLookAtF` (mdxMath.cpp:136), parameter names as in the original.
///
/// Note the game calls it as `guLookAtF(vMatrix, target, source, up)`
/// (`CalcViewMatrix`, drawloop.cpp:65): `eye` is the *target* and `at` the
/// camera position, so the view z axis points forward.
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

/// Port of `QuaternionToMatrix` (mdxMath.cpp:237). Quaternions are `[x, y, z, w]`.
pub fn quaternion_to_matrix(q: &Quat) -> Mat4 {
    let [x, y, z, w] = *q;
    let s = 2.0 / (x * x + y * y + z * z + w * w);
    let (xs, ys, zs) = (x * s, y * s, z * s);
    let (wx, wy, wz) = (w * xs, w * ys, w * zs);
    let (xx, xy, xz) = (x * xs, x * ys, x * zs);
    let (yy, yz, zz) = (y * ys, y * zs, z * zs);
    [[1.0 - (yy + zz), xy + wz, xz - wy, 0.0], [xy - wz, 1.0 - (xx + zz), yz + wx, 0.0], [xz + wy, yz - wx, 1.0 - (xx + yy), 0.0], [0.0, 0.0, 0.0, 1.0]]
}

/// Port of `QuatSlerp` (mdxMath.cpp:304), including the table-based trig.
pub fn quat_slerp(s1: &Quat, sp2: &Quat, t: f32) -> Quat {
    let d = |a: f32, b: f32| a - b;
    let (x, y, z, w) = (d(s1[0], sp2[0]), d(s1[1], sp2[1]), d(s1[2], sp2[2]), d(s1[3], sp2[3]));
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
        // BUG(original): w keeps the perpendicular quaternion's value (`src1->z`).
        dq
    }
}

/// `LinearInterp` macro (mdxMath.h:132).
pub fn linear_interp(from: &Vec3, to: &Vec3, t: f32) -> Vec3 {
    [from[0] + (to[0] - from[0]) * t, from[1] + (to[1] - from[1]) * t, from[2] + (to[2] - from[2]) * t]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_quat_and_cat() {
        let m = quaternion_to_matrix(&[0.0, 0.0, 0.0, 1.0]);
        assert_eq!(m, IDENTITY);
        let t = gu_translate(1.0, 2.0, 3.0);
        let s = gu_scale(2.0, 2.0, 2.0);
        // Row vectors: scale then translate.
        assert_eq!(mtx_xfm(&mtx_cat(&s, &t), [1.0, 1.0, 1.0]), [3.0, 4.0, 5.0]);
    }

    #[test]
    fn slerp_halfway() {
        let a = [0.0, 0.0, 0.0, 1.0];
        let h = core::f32::consts::FRAC_1_SQRT_2;
        let b = [0.0, h, 0.0, h];
        let q = quat_slerp(&a, &b, 0.5);
        let ang = 2.0 * q[3].acos();
        assert!((ang - core::f32::consts::FRAC_PI_4).abs() < 0.01, "{q:?}");
    }

    #[test]
    fn look_at_forward_is_positive_z() {
        // Game call order: eye = target, at = camera source.
        let v = gu_look_at([0.0, 0.0, 100.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let p = mtx_xfm(&v, [0.0, 0.0, 50.0]);
        assert!((p[2] - 50.0).abs() < 1e-4);
    }
}
