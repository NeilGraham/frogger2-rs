//! Ports of the vector macros in `Common/maths.h`.
//!
//! Naming follows the C suffixes: `V` = `VECTOR`, `S` = `SVECTOR`, `F` = `FVECTOR`
//! (destination first). Untyped macros that the C code applies to both `VECTOR` and
//! `FVECTOR` are generic over [`LongVec`]. Output parameters are `&mut` like the C
//! pointers; when the C code passes the same pointer as source and destination, copy the
//! source first (every macro here reads each source component before writing it, so this
//! is equivalent). Stores into `short` fields truncate (`as i16`), all arithmetic wraps.

use crate::fixed::{fmul, to_fixed, Fixed};
use crate::types::{FVector, IQuaternion, SVector, Vector};
use crate::util::{fsqrt, ftol32};

/// A vector with 32-bit `vx`/`vy`/`vz` fields (`VECTOR` or `FVECTOR`).
pub trait LongVec {
    /// Returns `[vx, vy, vz]`.
    fn xyz(&self) -> [i32; 3];
    /// Mutable access to `(vx, vy, vz)`.
    fn xyz_mut(&mut self) -> (&mut i32, &mut i32, &mut i32);
}

impl LongVec for Vector {
    #[inline]
    fn xyz(&self) -> [i32; 3] {
        [self.vx, self.vy, self.vz]
    }
    #[inline]
    fn xyz_mut(&mut self) -> (&mut i32, &mut i32, &mut i32) {
        (&mut self.vx, &mut self.vy, &mut self.vz)
    }
}

impl LongVec for FVector {
    #[inline]
    fn xyz(&self) -> [i32; 3] {
        [self.vx, self.vy, self.vz]
    }
    #[inline]
    fn xyz_mut(&mut self) -> (&mut i32, &mut i32, &mut i32) {
        (&mut self.vx, &mut self.vy, &mut self.vz)
    }
}

#[inline]
fn set3<T: LongVec + ?Sized>(v: &mut T, x: i32, y: i32, z: i32) {
    let (a, b, c) = v.xyz_mut();
    *a = x;
    *b = y;
    *c = z;
}

#[inline]
fn set2<T: LongVec + ?Sized>(v: &mut T, x: i32, z: i32) {
    let (a, _, c) = v.xyz_mut();
    *a = x;
    *c = z;
}

/// Port of `Fabs` (Common/maths.h:6): `((a) > 0 ? (a) : -(a))`, wrapping.
#[inline]
pub fn fabs(a: i32) -> i32 {
    if a > 0 {
        a
    } else {
        a.wrapping_neg()
    }
}

/// Port of `Max` (Common/types.h:109): `(p1 > p2) ? p1 : p2`.
#[inline]
pub fn max<T: PartialOrd>(p1: T, p2: T) -> T {
    if p1 > p2 {
        p1
    } else {
        p2
    }
}

/// Port of `Min` (Common/types.h:110): `(p1 > p2) ? p2 : p1`.
#[inline]
pub fn min<T: PartialOrd>(p1: T, p2: T) -> T {
    if p1 > p2 {
        p2
    } else {
        p1
    }
}

/// Port of `Bound` (Common/maths.h:47): `Min(b, Max(a, x))`.
#[inline]
pub fn bound<T: PartialOrd>(a: T, x: T, b: T) -> T {
    min(b, max(a, x))
}

/// Port of `DotProduct2D` (Common/maths.h:21): plain (unscaled) `vx*vx + vz*vz`.
#[inline]
pub fn dot_product_2d<A: LongVec, B: LongVec>(v1: &A, v2: &B) -> i32 {
    let (a, b) = (v1.xyz(), v2.xyz());
    a[0].wrapping_mul(b[0]).wrapping_add(a[2].wrapping_mul(b[2]))
}

/// Port of `ScaleVector` (Common/maths.h:23) with an integer scale.
#[inline]
pub fn scale_vector<T: LongVec>(v1: &mut T, scale: i32) {
    let v = v1.xyz();
    set3(v1, v[0].wrapping_mul(scale), v[1].wrapping_mul(scale), v[2].wrapping_mul(scale));
}

/// Port of `ScaleVector` (Common/maths.h:23) on an `SVECTOR` with an integer scale
/// (`short *= int`, truncated back to `short`).
#[inline]
pub fn scale_vector_s(v1: &mut SVector, scale: i32) {
    v1.vx = (v1.vx as i32).wrapping_mul(scale) as i16;
    v1.vy = (v1.vy as i32).wrapping_mul(scale) as i16;
    v1.vz = (v1.vz as i32).wrapping_mul(scale) as i16;
}

/// Port of `ScaleVector` (Common/maths.h:23) with a floating-point scale
/// (e.g. `ScaleVector(&up,-0.5)`): `long *= double`, converted back with `_ftol`.
#[inline]
pub fn scale_vector_float<T: LongVec>(v1: &mut T, scale: f64) {
    let v = v1.xyz();
    set3(v1, ftol32(v[0] as f64 * scale), ftol32(v[1] as f64 * scale), ftol32(v[2] as f64 * scale));
}

/// Port of `ScaleVector2D` (Common/maths.h:24).
#[inline]
pub fn scale_vector_2d<T: LongVec>(v1: &mut T, scale: i32) {
    let v = v1.xyz();
    set2(v1, v[0].wrapping_mul(scale), v[2].wrapping_mul(scale));
}

/// Port of `ScaleVectorFF` (Common/maths.h:25).
#[inline]
pub fn scale_vector_ff(v1: &mut FVector, scale: Fixed) {
    v1.vx = fmul(v1.vx, scale);
    v1.vy = fmul(v1.vy, scale);
    v1.vz = fmul(v1.vz, scale);
}

/// Port of `ScaleVectorSF` (Common/maths.h:29).
#[inline]
pub fn scale_vector_sf(v1: &mut SVector, scale: Fixed) {
    v1.vx = (fmul((v1.vx as i32) << 12, scale) >> 12) as i16;
    v1.vy = (fmul((v1.vy as i32) << 12, scale) >> 12) as i16;
    v1.vz = (fmul((v1.vz as i32) << 12, scale) >> 12) as i16;
}

/// Port of `ZeroVector` / `ZeroVectorF` (Common/maths.h:37-38).
#[inline]
pub fn zero_vector<T: LongVec>(vect: &mut T) {
    set3(vect, 0, 0, 0);
}

/// Port of `ZeroVector` (Common/maths.h:37) on an `SVECTOR`.
#[inline]
pub fn zero_vector_s(vect: &mut SVector) {
    vect.vx = 0;
    vect.vy = 0;
    vect.vz = 0;
}

/// Port of `ZeroVector2D` (Common/maths.h:39): `SetVector2D(vect,&zero)`.
#[inline]
pub fn zero_vector_2d<T: LongVec>(vect: &mut T) {
    set2(vect, 0, 0);
}

/// Port of `LinearInterp` (Common/maths.h:54) with an integer `interp`
/// (`from + (to - from) * interp`, unscaled).
#[inline]
pub fn linear_interp<R: LongVec, A: LongVec, B: LongVec>(result: &mut R, from: &A, to: &B, interp: i32) {
    let (f, t) = (from.xyz(), to.xyz());
    let c = |i: usize| f[i].wrapping_add(t[i].wrapping_sub(f[i]).wrapping_mul(interp));
    set3(result, c(0), c(1), c(2));
}

/// Port of `LinearInterpFSSF` (Common/maths.h:55).
#[inline]
pub fn linear_interp_fssf(result: &mut FVector, from: &SVector, to: &SVector, interp: Fixed) {
    let c = |f: i16, t: i16| to_fixed(f as i32).wrapping_add(fmul(to_fixed((t as i32) - (f as i32)), interp));
    result.vx = c(from.vx, to.vx);
    result.vy = c(from.vy, to.vy);
    result.vz = c(from.vz, to.vz);
}

/// Port of `MagnitudeV` (Common/maths.h:105): `Fsqrt(vx*vx + vy*vy + vz*vz)`.
#[inline]
pub fn magnitude_v(v: &Vector) -> Fixed {
    fsqrt(v.vx.wrapping_mul(v.vx).wrapping_add(v.vy.wrapping_mul(v.vy)).wrapping_add(v.vz.wrapping_mul(v.vz)))
}

// ---- SubVector* -------------------------------------------------------------------------

/// Port of `SubVectorVVV` (Common/maths.h:228).
#[inline]
pub fn sub_vector_vvv(result: &mut Vector, operand1: &Vector, operand2: &Vector) {
    result.vx = operand1.vx.wrapping_sub(operand2.vx);
    result.vy = operand1.vy.wrapping_sub(operand2.vy);
    result.vz = operand1.vz.wrapping_sub(operand2.vz);
}

/// Port of `SubVectorVSV` (Common/maths.h:232).
#[inline]
pub fn sub_vector_vsv(result: &mut Vector, operand1: &SVector, operand2: &Vector) {
    result.vx = (operand1.vx as i32).wrapping_sub(operand2.vx);
    result.vy = (operand1.vy as i32).wrapping_sub(operand2.vy);
    result.vz = (operand1.vz as i32).wrapping_sub(operand2.vz);
}

/// Port of `SubVectorVSS` (Common/maths.h:236).
#[inline]
pub fn sub_vector_vss(result: &mut Vector, operand1: &SVector, operand2: &SVector) {
    result.vx = operand1.vx as i32 - operand2.vx as i32;
    result.vy = operand1.vy as i32 - operand2.vy as i32;
    result.vz = operand1.vz as i32 - operand2.vz as i32;
}

/// Port of `SubVectorSSS` (Common/maths.h:240).
#[inline]
pub fn sub_vector_sss(result: &mut SVector, operand1: &SVector, operand2: &SVector) {
    result.vx = operand1.vx.wrapping_sub(operand2.vx);
    result.vy = operand1.vy.wrapping_sub(operand2.vy);
    result.vz = operand1.vz.wrapping_sub(operand2.vz);
}

/// Port of `SubVectorSSF` (Common/maths.h:244).
#[inline]
pub fn sub_vector_ssf(result: &mut SVector, operand1: &SVector, operand2: &FVector) {
    result.vx = (operand1.vx as i32).wrapping_sub(operand2.vx >> 12) as i16;
    result.vy = (operand1.vy as i32).wrapping_sub(operand2.vy >> 12) as i16;
    result.vz = (operand1.vz as i32).wrapping_sub(operand2.vz >> 12) as i16;
}

/// Port of `SubVectorFSS` (Common/maths.h:248).
#[inline]
pub fn sub_vector_fss(result: &mut FVector, operand1: &SVector, operand2: &SVector) {
    result.vx = (operand1.vx as i32 - operand2.vx as i32) << 12;
    result.vy = (operand1.vy as i32 - operand2.vy as i32) << 12;
    result.vz = (operand1.vz as i32 - operand2.vz as i32) << 12;
}

/// Port of `SubVectorFSF` (Common/maths.h:252).
#[inline]
pub fn sub_vector_fsf(result: &mut FVector, operand1: &SVector, operand2: &FVector) {
    result.vx = ((operand1.vx as i32) << 12).wrapping_sub(operand2.vx);
    result.vy = ((operand1.vy as i32) << 12).wrapping_sub(operand2.vy);
    result.vz = ((operand1.vz as i32) << 12).wrapping_sub(operand2.vz);
}

/// Port of `SubVectorFFS` (Common/maths.h:256).
#[inline]
pub fn sub_vector_ffs(result: &mut FVector, operand1: &FVector, operand2: &SVector) {
    result.vx = operand1.vx.wrapping_sub((operand2.vx as i32) << 12);
    result.vy = operand1.vy.wrapping_sub((operand2.vy as i32) << 12);
    result.vz = operand1.vz.wrapping_sub((operand2.vz as i32) << 12);
}

/// Port of `SubVectorFFF` (Common/maths.h:260).
#[inline]
pub fn sub_vector_fff(result: &mut FVector, operand1: &FVector, operand2: &FVector) {
    result.vx = operand1.vx.wrapping_sub(operand2.vx);
    result.vy = operand1.vy.wrapping_sub(operand2.vy);
    result.vz = operand1.vz.wrapping_sub(operand2.vz);
}

/// Port of `SubVector2D` (Common/maths.h:264).
#[inline]
pub fn sub_vector_2d<R: LongVec, A: LongVec, B: LongVec>(result: &mut R, operand1: &A, operand2: &B) {
    let (a, b) = (operand1.xyz(), operand2.xyz());
    set2(result, a[0].wrapping_sub(b[0]), a[2].wrapping_sub(b[2]));
}

/// Port of `SubVector2DSSS` (Common/maths.h:267).
#[inline]
pub fn sub_vector_2d_sss(result: &mut SVector, operand1: &SVector, operand2: &SVector) {
    result.vx = operand1.vx.wrapping_sub(operand2.vx);
    result.vz = operand1.vz.wrapping_sub(operand2.vz);
}

/// Port of `SubFromVector2D` (Common/maths.h:270).
#[inline]
pub fn sub_from_vector_2d<R: LongVec, A: LongVec>(result: &mut R, operand: &A) {
    let (r, a) = (result.xyz(), operand.xyz());
    set2(result, r[0].wrapping_sub(a[0]), r[2].wrapping_sub(a[2]));
}

/// Port of `SubFromVectorVS` (Common/maths.h:273).
#[inline]
pub fn sub_from_vector_vs(result: &mut Vector, v1: &SVector) {
    result.vx = result.vx.wrapping_sub(v1.vx as i32);
    result.vy = result.vy.wrapping_sub(v1.vy as i32);
    result.vz = result.vz.wrapping_sub(v1.vz as i32);
}

/// Port of `SubFromVectorSS` (Common/maths.h:277).
#[inline]
pub fn sub_from_vector_ss(result: &mut SVector, v1: &SVector) {
    result.vx = result.vx.wrapping_sub(v1.vx);
    result.vy = result.vy.wrapping_sub(v1.vy);
    result.vz = result.vz.wrapping_sub(v1.vz);
}

/// Port of `SubFromVectorSF` (Common/maths.h:281).
#[inline]
pub fn sub_from_vector_sf(result: &mut SVector, v1: &FVector) {
    result.vx = result.vx.wrapping_sub((v1.vx >> 12) as i16);
    result.vy = result.vy.wrapping_sub((v1.vy >> 12) as i16);
    result.vz = result.vz.wrapping_sub((v1.vz >> 12) as i16);
}

/// Port of `SubFromVectorFS` (Common/maths.h:285).
#[inline]
pub fn sub_from_vector_fs(result: &mut FVector, v1: &SVector) {
    result.vx = result.vx.wrapping_sub((v1.vx as i32) << 12);
    result.vy = result.vy.wrapping_sub((v1.vy as i32) << 12);
    result.vz = result.vz.wrapping_sub((v1.vz as i32) << 12);
}

/// Port of `SubFromVectorFF` (Common/maths.h:289).
#[inline]
pub fn sub_from_vector_ff(result: &mut FVector, v1: &FVector) {
    result.vx = result.vx.wrapping_sub(v1.vx);
    result.vy = result.vy.wrapping_sub(v1.vy);
    result.vz = result.vz.wrapping_sub(v1.vz);
}

// ---- SetVector* -------------------------------------------------------------------------

/// Port of `SetVectorVS` (Common/maths.h:294).
#[inline]
pub fn set_vector_vs(v1: &mut Vector, v2: &SVector) {
    v1.vx = v2.vx as i32;
    v1.vy = v2.vy as i32;
    v1.vz = v2.vz as i32;
}

/// Port of `SetVectorSV` (Common/maths.h:298).
#[inline]
pub fn set_vector_sv(v1: &mut SVector, v2: &Vector) {
    v1.vx = v2.vx as i16;
    v1.vy = v2.vy as i16;
    v1.vz = v2.vz as i16;
}

/// Port of `SetVectorSS` (Common/maths.h:302).
#[inline]
pub fn set_vector_ss(v1: &mut SVector, v2: &SVector) {
    v1.vx = v2.vx;
    v1.vy = v2.vy;
    v1.vz = v2.vz;
}

/// Port of `SetVectorSF` (Common/maths.h:306).
#[inline]
pub fn set_vector_sf(v1: &mut SVector, v2: &FVector) {
    v1.vx = (v2.vx >> 12) as i16;
    v1.vy = (v2.vy >> 12) as i16;
    v1.vz = (v2.vz >> 12) as i16;
}

/// Port of `SetVectorFS` (Common/maths.h:310).
#[inline]
pub fn set_vector_fs(v1: &mut FVector, v2: &SVector) {
    v1.vx = (v2.vx as i32) << 12;
    v1.vy = (v2.vy as i32) << 12;
    v1.vz = (v2.vz as i32) << 12;
}

/// Port of `NewSetVectorFS` (Common/maths.h:314): plain copy, no shift.
#[inline]
pub fn new_set_vector_fs(v1: &mut FVector, v2: &SVector) {
    v1.vx = v2.vx as i32;
    v1.vy = v2.vy as i32;
    v1.vz = v2.vz as i32;
}

/// Port of `SetVectorFF` (Common/maths.h:318).
#[inline]
pub fn set_vector_ff(v1: &mut FVector, v2: &FVector) {
    v1.vx = v2.vx;
    v1.vy = v2.vy;
    v1.vz = v2.vz;
}

/// Port of `SetVectorFV` (Common/maths.h:322).
#[inline]
pub fn set_vector_fv(v1: &mut FVector, v2: &Vector) {
    v1.vx = v2.vx.wrapping_shl(12);
    v1.vy = v2.vy.wrapping_shl(12);
    v1.vz = v2.vz.wrapping_shl(12);
}

/// Port of `SetVector2D` (Common/maths.h:369).
#[inline]
pub fn set_vector_2d<A: LongVec, B: LongVec>(v1: &mut A, v2: &B) {
    let b = v2.xyz();
    set2(v1, b[0], b[2]);
}

/// Port of `SetVector2DFF` (Common/maths.h:371).
#[inline]
pub fn set_vector_2d_ff(v1: &mut FVector, v2: &FVector) {
    v1.vx = v2.vx;
    v1.vz = v2.vz;
}

/// Port of `SetQuaternion` (Common/maths.h:35).
#[inline]
pub fn set_quaternion(q1: &mut IQuaternion, q2: &IQuaternion) {
    *q1 = *q2;
}

// ---- AddVector* -------------------------------------------------------------------------

/// Port of `AddVectorSSS` (Common/maths.h:327).
#[inline]
pub fn add_vector_sss(dest: &mut SVector, v1: &SVector, v2: &SVector) {
    dest.vx = v1.vx.wrapping_add(v2.vx);
    dest.vy = v1.vy.wrapping_add(v2.vy);
    dest.vz = v1.vz.wrapping_add(v2.vz);
}

/// Port of `AddVectorSFS` (Common/maths.h:331).
#[inline]
pub fn add_vector_sfs(dest: &mut SVector, v1: &FVector, v2: &SVector) {
    dest.vx = (v1.vx >> 12).wrapping_add(v2.vx as i32) as i16;
    dest.vy = (v1.vy >> 12).wrapping_add(v2.vy as i32) as i16;
    dest.vz = (v1.vz >> 12).wrapping_add(v2.vz as i32) as i16;
}

/// Port of `AddVectorSSF` (Common/maths.h:335).
#[inline]
pub fn add_vector_ssf(dest: &mut SVector, v1: &SVector, v2: &FVector) {
    dest.vx = (v1.vx as i32).wrapping_add(v2.vx >> 12) as i16;
    dest.vy = (v1.vy as i32).wrapping_add(v2.vy >> 12) as i16;
    dest.vz = (v1.vz as i32).wrapping_add(v2.vz >> 12) as i16;
}

/// Port of `AddVectorSFF` (Common/maths.h:339).
#[inline]
pub fn add_vector_sff(dest: &mut SVector, v1: &FVector, v2: &FVector) {
    dest.vx = (v1.vx.wrapping_add(v2.vx) >> 12) as i16;
    dest.vy = (v1.vy.wrapping_add(v2.vy) >> 12) as i16;
    dest.vz = (v1.vz.wrapping_add(v2.vz) >> 12) as i16;
}

/// Port of `AddVectorFFS` (Common/maths.h:343).
#[inline]
pub fn add_vector_ffs(dest: &mut FVector, v1: &FVector, v2: &SVector) {
    dest.vx = v1.vx.wrapping_add((v2.vx as i32) << 12);
    dest.vy = v1.vy.wrapping_add((v2.vy as i32) << 12);
    dest.vz = v1.vz.wrapping_add((v2.vz as i32) << 12);
}

/// Port of `AddVectorFFF` (Common/maths.h:347).
#[inline]
pub fn add_vector_fff(dest: &mut FVector, v1: &FVector, v2: &FVector) {
    dest.vx = v1.vx.wrapping_add(v2.vx);
    dest.vy = v1.vy.wrapping_add(v2.vy);
    dest.vz = v1.vz.wrapping_add(v2.vz);
}

/// Port of `AddVectorVSF` (Common/maths.h:351).
#[inline]
pub fn add_vector_vsf(dest: &mut Vector, v1: &SVector, v2: &FVector) {
    dest.vx = (v1.vx as i32).wrapping_add(v2.vx >> 12);
    dest.vy = (v1.vy as i32).wrapping_add(v2.vy >> 12);
    dest.vz = (v1.vz as i32).wrapping_add(v2.vz >> 12);
}

/// Port of `CrossProductFFF` (Common/maths.h:356).
#[inline]
pub fn cross_product_fff(result: &mut FVector, operand1: &FVector, operand2: &FVector) {
    let (a, b) = (*operand1, *operand2);
    result.vx = fmul(a.vy, b.vz).wrapping_sub(fmul(a.vz, b.vy));
    result.vy = fmul(a.vz, b.vx).wrapping_sub(fmul(a.vx, b.vz));
    result.vz = fmul(a.vx, b.vy).wrapping_sub(fmul(a.vy, b.vx));
}

/// Port of `CrossProductVertical` (Common/maths.h:360): cross product with the up vector.
#[inline]
pub fn cross_product_vertical<R: LongVec, A: LongVec>(result: &mut R, operand2: &A) {
    let b = operand2.xyz();
    set3(result, b[2], 0, b[0].wrapping_neg());
}

/// Port of `DotProductFF` (Common/maths.h:365).
#[inline]
pub fn dot_product_ff(v1: &FVector, v2: &FVector) -> Fixed {
    fmul(v1.vx, v2.vx).wrapping_add(fmul(v1.vy, v2.vy)).wrapping_add(fmul(v1.vz, v2.vz))
}

/// Port of `DotProductSS` (Common/maths.h:367): unscaled `int` dot product.
#[inline]
pub fn dot_product_ss(v1: &SVector, v2: &SVector) -> i32 {
    (v1.vx as i32 * v2.vx as i32).wrapping_add(v1.vy as i32 * v2.vy as i32).wrapping_add(v1.vz as i32 * v2.vz as i32)
}

// ---- AddToVector* -----------------------------------------------------------------------

/// Port of `AddToVectorVS` (Common/maths.h:375).
#[inline]
pub fn add_to_vector_vs(res: &mut Vector, v1: &SVector) {
    res.vx = res.vx.wrapping_add(v1.vx as i32);
    res.vy = res.vy.wrapping_add(v1.vy as i32);
    res.vz = res.vz.wrapping_add(v1.vz as i32);
}

/// Port of `AddToVectorSS` (Common/maths.h:379).
#[inline]
pub fn add_to_vector_ss(res: &mut SVector, v1: &SVector) {
    res.vx = res.vx.wrapping_add(v1.vx);
    res.vy = res.vy.wrapping_add(v1.vy);
    res.vz = res.vz.wrapping_add(v1.vz);
}

/// Port of `AddToVectorFF` (Common/maths.h:383).
#[inline]
pub fn add_to_vector_ff(res: &mut FVector, v1: &FVector) {
    res.vx = res.vx.wrapping_add(v1.vx);
    res.vy = res.vy.wrapping_add(v1.vy);
    res.vz = res.vz.wrapping_add(v1.vz);
}

/// Port of `AddToVectorFS` (Common/maths.h:387).
#[inline]
pub fn add_to_vector_fs(res: &mut FVector, v1: &SVector) {
    res.vx = res.vx.wrapping_add((v1.vx as i32) << 12);
    res.vy = res.vy.wrapping_add((v1.vy as i32) << 12);
    res.vz = res.vz.wrapping_add((v1.vz as i32) << 12);
}

/// Port of `AddToVectorFV` (Common/maths.h:391).
#[inline]
pub fn add_to_vector_fv(res: &mut FVector, v1: &Vector) {
    res.vx = res.vx.wrapping_add(v1.vx.wrapping_shl(12));
    res.vy = res.vy.wrapping_add(v1.vy.wrapping_shl(12));
    res.vz = res.vz.wrapping_add(v1.vz.wrapping_shl(12));
}

/// Port of `AddToVectorSF` (Common/maths.h:395).
#[inline]
pub fn add_to_vector_sf(res: &mut SVector, v1: &FVector) {
    res.vx = res.vx.wrapping_add((v1.vx >> 12) as i16);
    res.vy = res.vy.wrapping_add((v1.vy >> 12) as i16);
    res.vz = res.vz.wrapping_add((v1.vz >> 12) as i16);
}

/// Port of `AddVector2D` (Common/maths.h:399).
#[inline]
pub fn add_vector_2d<R: LongVec, A: LongVec, B: LongVec>(result: &mut R, operand1: &A, operand2: &B) {
    let (a, b) = (operand1.xyz(), operand2.xyz());
    set2(result, a[0].wrapping_add(b[0]), a[2].wrapping_add(b[2]));
}

/// Port of `AddScaledVector2D` (Common/maths.h:402).
#[inline]
pub fn add_scaled_vector_2d<R: LongVec, A: LongVec, B: LongVec>(result: &mut R, operand1: &A, scale1: i32, operand2: &B, scale2: i32) {
    let (a, b) = (operand1.xyz(), operand2.xyz());
    let c = |i: usize| scale1.wrapping_mul(a[i]).wrapping_add(scale2.wrapping_mul(b[i])) >> 12;
    set2(result, c(0), c(2));
}

/// Port of `AddOneScaledVector` (Common/maths.h:405).
#[inline]
pub fn add_one_scaled_vector<R: LongVec, A: LongVec, B: LongVec>(result: &mut R, operand1: &A, operand2: &B, scale2: i32) {
    let (a, b) = (operand1.xyz(), operand2.xyz());
    let c = |i: usize| a[i].wrapping_add(scale2.wrapping_mul(b[i]) >> 12);
    set3(result, c(0), c(1), c(2));
}

/// Port of `AddToVector2D` (Common/maths.h:409).
#[inline]
pub fn add_to_vector_2d<R: LongVec, A: LongVec>(result: &mut R, operand: &A) {
    let (r, a) = (result.xyz(), operand.xyz());
    set2(result, r[0].wrapping_add(a[0]), r[2].wrapping_add(a[2]));
}

/// Port of `AddToOneScaledVector` (Common/maths.h:412) (the live, `FMul` version).
#[inline]
pub fn add_to_one_scaled_vector<R: LongVec, A: LongVec>(result: &mut R, operand2: &A, scale2: Fixed) {
    let (r, b) = (result.xyz(), operand2.xyz());
    let c = |i: usize| r[i].wrapping_add(fmul(scale2, b[i]));
    set3(result, c(0), c(1), c(2));
}

/// Port of `AddToOneScaledVector2D` (Common/maths.h:422).
#[inline]
pub fn add_to_one_scaled_vector_2d<R: LongVec, A: LongVec>(result: &mut R, operand2: &A, scale2: i32) {
    let (r, b) = (result.xyz(), operand2.xyz());
    let c = |i: usize| r[i].wrapping_add(scale2.wrapping_mul(b[i]) >> 12);
    set2(result, c(0), c(2));
}

/// Port of `AddOneScaledVector2D` (Common/maths.h:425).
#[inline]
pub fn add_one_scaled_vector_2d<R: LongVec, A: LongVec, B: LongVec>(result: &mut R, operand1: &A, operand2: &B, scale2: i32) {
    let (a, b) = (operand1.xyz(), operand2.xyz());
    let c = |i: usize| a[i].wrapping_add(scale2.wrapping_mul(b[i]) >> 12);
    set2(result, c(0), c(2));
}
