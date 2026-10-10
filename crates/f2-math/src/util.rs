//! PC shims the maths depend on: `utilSqrt` (PC/islutil.cpp), `Fsqrt`/`FsqrtF`
//! (Common/newpsx.h), `rsin`/`rcos` (PC/libgte.h), `utilCalcAngle` (PC/islutil.h), and the
//! MSVC6 float->int conversion (`_ftol`).
//!
//! Float caveat: the retail exe (MSVC 6, x87) evaluates these in x87 registers. We model the
//! default MSVC CRT precision control (53-bit, identical to `f64`). If Direct3D switched
//! the FPU to 24-bit precision at runtime, or where the CRT `sin`/`cos`/`acos`/`atan2`
//! (x87 `fsin`/`fpatan`) differ from libm by an ulp, the truncated results may differ in
//! rare cases; the trace oracle is authoritative for those.

use crate::fixed::Fixed;

/// Port of `PI2` (MDX Library/mdxMath.h:12): `(3.141592654*2.0F)`, a `double`.
#[allow(clippy::approx_constant)] // the original's truncated constant, intentionally
pub const PI2: f64 = 3.141592654 * 2.0;

const TWO63: f64 = 9_223_372_036_854_775_808.0; // 2^63, exact

/// MSVC 6 `_ftol`: truncates toward zero into a 64-bit integer via `fistp qword`.
/// NaN / out-of-range values give the x87 "integer indefinite" `0x8000_0000_0000_0000`.
/// A C cast to `long` / `unsigned long` keeps the low 32 bits of this (so `(long)NaN == 0`).
#[inline]
pub fn ftol(x: f64) -> i64 {
    if x.is_nan() || !(-TWO63..TWO63).contains(&x) {
        i64::MIN
    } else {
        x as i64
    }
}

/// `(long)x` for a `double` under MSVC 6 (low 32 bits of [`ftol`]).
#[inline]
pub fn ftol32(x: f64) -> i32 {
    ftol(x) as i32
}

/// Port of `utilSqrt` (PC/islutil.cpp:36): approximate sqrt via float exponent halving,
/// returned in 16.16. `num` is first rounded to `float`; the bit hack is
/// `((bits - 0x3F800000) >> 1) + 0x3F800000` (arithmetic shift); result `(ULONG)(r*65536)`.
#[inline]
pub fn util_sqrt(num: u32) -> u32 {
    let n = num as f32;
    let bits = ((n.to_bits() as i32).wrapping_sub(0x3F80_0000) >> 1).wrapping_add(0x3F80_0000);
    let retval = f32::from_bits(bits as u32);
    ftol(retval as f64 * 65536.0) as u32
}

/// Port of `Fsqrt(x)` (Common/newpsx.h:10): `utilSqrt(x)>>4`, stored into a `fixed`.
#[inline]
pub fn fsqrt(x: i32) -> Fixed {
    (util_sqrt(x as u32) >> 4) as i32
}

/// Port of `FsqrtF(x)` (Common/newpsx.h:11): `utilSqrt(x)>>10`, stored into a `fixed`.
#[inline]
pub fn fsqrt_f(x: i32) -> Fixed {
    (util_sqrt(x as u32) >> 10) as i32
}

/// Port of `rsin` (PC/libgte.h:6): `(fixed)(sin(((x)*(1.0f/4096.0f))*PI2)*4096)`.
/// Angle is 4096 per turn; result is 4096-scaled.
///
/// RETAIL: the exe runs with Direct3D's x87 control word (24-bit precision), so
/// `x/4096`, the product with `PI2` and `sin*4096` are each rounded to `float` before the
/// truncating `_ftol` (see `arccos`). This matters for large angles (e.g. the unsigned
/// `clam` of `UpdateGallopingEnemy` while an enemy waits), where the float argument is
/// off by up to 1/64 rad. The difftest build rewrites the C macro the same way.
#[inline]
pub fn rsin(x: i32) -> Fixed {
    let b = x87_angle(x);
    ftol32(((b as f64).sin() * 4096.0) as f32 as f64)
}

/// Port of `rcos` (PC/libgte.h:7): `(fixed)(cos(((x)*(1.0f/4096.0f))*PI2)*4096)`,
/// with the retail 24-bit x87 rounding of [`rsin`].
#[inline]
pub fn rcos(x: i32) -> Fixed {
    let b = x87_angle(x);
    ftol32(((b as f64).cos() * 4096.0) as f32 as f64)
}

/// `(x*(1.0f/4096.0f))*PI2` on the retail 24-bit x87 FPU.
#[inline]
fn x87_angle(x: i32) -> f32 {
    let a = (x as f64 * (1.0 / 4096.0)) as f32;
    (a as f64 * PI2) as f32
}

/// Port of `utilCalcAngle` (PC/islutil.h:23):
/// `((int)((atan2(adj,opp) / 6.283185308) * 4096.0))`.
#[inline]
pub fn util_calc_angle(adj: i32, opp: i32) -> i32 {
    ftol32(((adj as f64).atan2(opp as f64) / 6.283185308) * 4096.0)
}
