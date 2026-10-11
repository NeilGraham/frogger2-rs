//! PC shims the maths depend on: `utilSqrt` (PC/islutil.cpp), `Fsqrt`/`FsqrtF`
//! (Common/newpsx.h), `rsin`/`rcos` (PC/libgte.h), `utilCalcAngle` (PC/islutil.h), and the
//! MSVC6 float->int conversion (`_ftol`).
//!
//! Floats: the retail exe (MSVC 6, x87) runs under Direct3D's 24-bit precision control.
//! `rsin`/`rcos`/`utilCalcAngle` (and `arccos` in maths.rs) emulate that exactly with
//! IEEE `f64` operations and the pure-Rust [`crate::detfloat`] functions, so they are
//! bit-identical on every OS/CPU and match the host's real x87 on all audited inputs
//! (all 2^32 `rsin`/`rcos` arguments; see docs/DECISIONS.md, docs/NETPLAY.md §9).

use crate::detfloat;
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
    // Phase 3S: `retval` is a positive normal float in [2^-63, 65536] for every `num`, so
    // `retval * 65536` is exact in `f32` and lies in [0, 2^32]: the original's `double`
    // product and `_ftol` (`ftol`) give the same integer as an `f32` multiply and a
    // truncating `f32 -> i64` conversion (no `f64` work on the VR4300). Tested against
    // [`util_sqrt_f64`] on a dense sweep of `u32`.
    (retval * 65536.0) as i64 as u32
}

/// [`util_sqrt`] as written in the original (`double` product, `_ftol`), for tests.
#[inline]
pub fn util_sqrt_f64(num: u32) -> u32 {
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

// ---- x87 precision-control emulation ----------------------------------------------------
//
// RETAIL: the exe runs with Direct3D's x87 control word (0x007F: 24-bit precision, round to
// nearest). Under it every x87 `fadd`/`fsub`/`fmul`/`fdiv`/`fsqrt` rounds its exact result
// once to a 24-bit significand, while `fsin`/`fcos`/`fpatan` still deliver 64-bit results.
// The helpers below reproduce that with plain IEEE `f64` operations (no FMA, no platform
// libm), so the results are identical on every OS/CPU. Verified against the host's real x87
// (`tests/x87.rs`, `examples/x87_check.rs`); see docs/DECISIONS.md.

/// Exact product `a*b = p + e` (Dekker/Veltkamp, no FMA). Exact while nothing overflows.
#[inline]
fn two_prod(a: f64, b: f64) -> (f64, f64) {
    #[inline]
    fn split(x: f64) -> (f64, f64) {
        let c = 134_217_729.0 * x; // 2^27 + 1
        let hi = c - (c - x);
        (hi, x - hi)
    }
    let p = a * b;
    let ((ah, al), (bh, bl)) = (split(a), split(b));
    let e = ((ah * bh - p) + ah * bl + al * bh) + al * bl;
    (p, e)
}

/// Rounds the exact value `p + e` (`|e|` ≤ half an `f64` ulp of `p`) to the nearest `f32`
/// (ties to even) with a single rounding — `p as f32` alone double-rounds when `p` lands
/// exactly on an `f32` midpoint.
#[inline]
fn round_f32_tail(p: f64, e: f64) -> f32 {
    let f = p as f32;
    let d = p - f as f64; // exact (Sterbenz)
    if d == 0.0 || e == 0.0 || (d > 0.0) != (e > 0.0) {
        return f;
    }
    // `p` sits between `f` and its neighbour towards `p`; the exact value lies further out.
    let up = d > 0.0;
    let next = if f == 0.0 {
        f32::from_bits(1).copysign(if up { 1.0 } else { -1.0 })
    } else if (f > 0.0) == up {
        f32::from_bits(f.to_bits() + 1)
    } else {
        f32::from_bits(f.to_bits() - 1)
    };
    if d == (next as f64 - f as f64) * 0.5 {
        next
    } else {
        f
    }
}

/// x87 PC24 `fmul`: `a*b` rounded once to `f32` precision.
#[inline]
pub fn mul_pc24(a: f64, b: f64) -> f32 {
    let (p, e) = two_prod(a, b);
    round_f32_tail(p, e)
}

/// x87 PC24 `fdiv`: `a/b` rounded once to `f32` precision.
#[inline]
pub fn div_pc24(a: f64, b: f64) -> f32 {
    let q = a / b;
    // Remainder a - q*b, exact: q*b = p + e exactly and a - p is exact (Sterbenz).
    let (p, e) = two_prod(q, b);
    let r = (a - p) - e;
    // True quotient = q + r/b; only the sign of the tail matters for the rounding.
    let tail = if r == 0.0 {
        0.0
    } else if (r > 0.0) == (b > 0.0) {
        f64::MIN_POSITIVE
    } else {
        -f64::MIN_POSITIVE
    };
    round_f32_tail(q, tail)
}

/// `(x)*(1.0f/4096.0f)*PI2` as the retail exe evaluates it: `fild` (exact), `fmul` by the
/// float constant and `fmul` by the double `PI2`, each rounded to 24 bits.
#[inline]
fn rtrig_arg(x: i32) -> f64 {
    let a = x as f32 * (1.0 / 4096.0); // round24(x) * 2^-12, exact scaling
    mul_pc24(a as f64, PI2) as f64
}

/// Port of `rsin` (PC/libgte.h:6): `(fixed)(sin(((x)*(1.0f/4096.0f))*PI2)*4096)`.
/// Angle is 4096 per turn; result is 4096-scaled. Evaluated with retail x87 PC24 semantics
/// (argument and `*4096` product rounded to 24 bits); the 53-bit evaluation is
/// [`rsin_f64`]. Both agree for |x| ≤ 10683.
///
/// Phase 3S: a lookup in the exact table of [`rsin_float`] for |x| ≤ 10683 (where the
/// result is 4096-periodic and odd-symmetric, checked exhaustively by the tests), the
/// float evaluation elsewhere. Same results for every `x`; ~20× cheaper on the VR4300.
#[inline]
pub fn rsin(x: i32) -> Fixed {
    if (-10683..=10683).contains(&x) {
        sin_lookup(x)
    } else {
        rsin_float(x)
    }
}

/// Port of `rcos` (PC/libgte.h:7): `(fixed)(cos(((x)*(1.0f/4096.0f))*PI2)*4096)`, with
/// retail x87 PC24 semantics (see [`rsin`]); 53-bit evaluation: [`rcos_f64`].
/// Phase 3S: `rcos(x) == rsin_float((x + 1024) & 4095)` for |x| ≤ 10723 (tested), else the
/// float evaluation.
#[inline]
pub fn rcos(x: i32) -> Fixed {
    if (-10723..=10723).contains(&x) {
        sin_lookup(x.wrapping_add(1024))
    } else {
        rcos_float(x)
    }
}

/// `rsin_float(x & 4095)` from the half-period table.
#[inline(always)]
fn sin_lookup(x: i32) -> Fixed {
    let v = crate::tables::SIN_HALF[(x & 2047) as usize] as i32;
    if x & 2048 != 0 {
        -v
    } else {
        v
    }
}

/// The float evaluation of [`rsin`] (the reference the table is generated from).
#[inline]
pub fn rsin_float(x: i32) -> Fixed {
    ftol32((detfloat::sin(rtrig_arg(x)) as f32) as f64 * 4096.0)
}

/// The float evaluation of [`rcos`].
#[inline]
pub fn rcos_float(x: i32) -> Fixed {
    ftol32((detfloat::cos(rtrig_arg(x)) as f32) as f64 * 4096.0)
}

/// `rsin` evaluated in double precision (the macro compiled for an SSE host), for difftests.
#[inline]
pub fn rsin_f64(x: i32) -> Fixed {
    ftol32(detfloat::sin(x as f64 * (1.0 / 4096.0) * PI2) * 4096.0)
}

/// `rcos` evaluated in double precision (the macro compiled for an SSE host), for difftests.
#[inline]
pub fn rcos_f64(x: i32) -> Fixed {
    ftol32(detfloat::cos(x as f64 * (1.0 / 4096.0) * PI2) * 4096.0)
}

/// Port of `utilCalcAngle` (PC/islutil.h:23):
/// `((int)((atan2(adj,opp) / 6.283185308) * 4096.0))`, with retail x87 PC24 semantics: the
/// division is rounded to 24 bits (so `utilCalcAngle(1,0)` is 1024, not 1023).
#[inline]
pub fn util_calc_angle(adj: i32, opp: i32) -> i32 {
    let q = div_pc24(detfloat::atan2(adj as f64, opp as f64), 6.283185308);
    ftol32(q as f64 * 4096.0)
}

/// `utilCalcAngle` evaluated in double precision (SSE host), for difftests.
#[inline]
pub fn util_calc_angle_f64(adj: i32, opp: i32) -> i32 {
    ftol32((detfloat::atan2(adj as f64, opp as f64) / 6.283185308) * 4096.0)
}

#[cfg(test)]
mod tests {
    use crate::{arccos, arccos_float, rcos, rcos_float, rsin, rsin_float};

    /// The lookups equal the float evaluations everywhere: exhaustively around the table
    /// domains, and on a dense stride over all of `i32`.
    #[test]
    fn tables_match_float_reference() {
        let check = |x: i32| {
            assert_eq!(rsin(x), rsin_float(x), "rsin({x})");
            assert_eq!(rcos(x), rcos_float(x), "rcos({x})");
            assert_eq!(arccos(x), arccos_float(x), "arccos({x})");
        };
        (-40_000..=40_000).for_each(check);
        // util_sqrt: every u32 below 2^22, near every power of two, and a dense stride.
        let us = |x: u32| assert_eq!(crate::util_sqrt(x), crate::util_sqrt_f64(x), "util_sqrt({x})");
        (0..1u32 << 22).for_each(us);
        for b in 0..32 {
            let p = 1u64 << b;
            for d in 0..2048u64 {
                us((p + d).min(u32::MAX as u64) as u32);
                us(p.saturating_sub(d) as u32);
            }
        }
        (0..=u32::MAX).step_by(1009).for_each(us);
        us(u32::MAX);
        let mut x = i32::MIN;
        while let Some(n) = x.checked_add(9_973) {
            check(x);
            x = n;
        }
        check(i32::MAX);
    }
}
