//! Rust-vs-original-C differential tests. The C (`Common/fixed.c`, `Common/maths.c`,
//! `Common/types.c` + macro wrappers in `tests/csrc/shim.c`) is compiled by `build.rs`.
//!
//! Known, intentional domain restrictions (host gcc/x86-64 SSE vs retail MSVC6/x87):
//! - `rsin`/`rcos` inputs limited to |x| < 2^24 (gcc rounds `x*(1.0f/4096)` to float).
//! - `arccos` inputs limited to [-4096, 4096]: for NaN, MSVC6 `_ftol` yields 0 while gcc's
//!   `cvttsd2si` yields `INT_MIN` (covered by a separate unit test of the MSVC behaviour).
#![cfg(not(f2_math_no_c))]
#![allow(non_snake_case)]

use f2_math::*;
use std::os::raw::c_char;

mod c {
    use super::*;
    extern "C" {
        pub fn FMul(x: i32, y: i32) -> i32;
        pub fn NewFDiv(x: i32, y: i32, file: *const c_char, line: i32) -> i32;
        pub fn FDiv2(x: i32, y: i32) -> i32;
        pub fn Random(lim: u32) -> i32;
        pub fn SeedRandom();
        pub fn t_set_rseed(s: u32);
        pub fn t_get_rseed() -> u32;
        pub fn t_set_vertq(q: *const IQuaternion);
        pub fn t_get_vertq(q: *mut IQuaternion);
        pub fn t_set_gamespeed(g: i32);
        pub fn arccos(c: i32) -> i32;
        pub fn t_rsin(x: i32) -> i32;
        pub fn t_rcos(x: i32) -> i32;
        pub fn t_calc_angle(a: i32, b: i32) -> i32;
        pub fn utilSqrt(x: u32) -> u32;
        pub fn t_fsqrt(x: i32) -> i32;
        pub fn t_fsqrtf(x: i32) -> i32;
        pub fn t_magnitude_v(v: *const Vector) -> i32;

        pub fn fixedRotateVectorByRotationS(r: *mut FVector, v: *const SVector, q: *const IQuaternion);
        pub fn fixedRotateVectorByRotation(r: *mut FVector, v: *const FVector, q: *const IQuaternion);
        pub fn fixedQuaternionMultiply(d: *mut IQuaternion, a: *const IQuaternion, b: *const IQuaternion);
        pub fn fixedGetQuaternionFromRotation(d: *mut IQuaternion, s: *const IQuaternion);
        pub fn fixedGetRotationFromQuaternion(d: *mut IQuaternion, s: *const IQuaternion);
        pub fn RotateVectorByQuaternionFF(r: *mut FVector, v: *const FVector, q: *const IQuaternion);
        pub fn IQuatSlerp(a: *const IQuaternion, b: *const IQuaternion, t: i32, d: *mut IQuaternion);
        pub fn DistanceBetweenPoints2D(a: *const Vector, b: *const Vector) -> i32;
        pub fn DistanceBetweenPointsVV(a: *const Vector, b: *const Vector) -> i32;
        pub fn DistanceBetweenPointsSS(a: *const SVector, b: *const SVector) -> i32;
        pub fn DistanceBetweenPointsSV(a: *const SVector, b: *const Vector) -> i32;
        pub fn DistanceBetweenPointsSF(a: *const SVector, b: *const FVector) -> i32;
        pub fn DistanceBetweenPointsFS(a: *const FVector, b: *const SVector) -> i32;
        pub fn DistanceBetweenPointsFF(a: *const FVector, b: *const FVector) -> i32;
        pub fn DistanceBetweenPointsSqrSS(a: *const SVector, b: *const SVector) -> i32;
        pub fn DistanceBetweenPointsSqrFS(a: *const FVector, b: *const SVector) -> i32;
        pub fn IsPointWithinBox(b: *const Box3D, p: *const Vector) -> i32;
        pub fn IsPointWithinBoxPlusHeight(b: *const Box3D, p: *const Vector, h: i32) -> i32;
        pub fn IsPointWithinBox2D(b: *const Box3D, p: *const Vector) -> i32;
        pub fn RotateVector2D(d: *mut FVector, s: *const FVector, a: i32);
        pub fn RotateVector2DX(d: *mut FVector, s: *const FVector, a: i32);
        pub fn FlipVector(v: *mut Vector);
        pub fn FlipVector2D(v: *mut Vector);
        pub fn FindShortestAngle(a: i32, b: i32) -> i32;
        pub fn FindShortestAngleSigned(a: i32, b: i32) -> i32;
        pub fn MagnitudeS(v: *const SVector) -> i32;
        pub fn MagnitudeF(v: *const FVector) -> i32;
        pub fn Magnitude2DF(v: *const FVector) -> i32;
        pub fn Magnitude2DV(v: *const Vector) -> i32;
        pub fn MakeUnit(v: *mut FVector);
        pub fn MakeUnit2D(v: *mut FVector);
        pub fn Aabs(a: i32) -> i32;
        pub fn SineWave(f: i32, p: i32, b: i32) -> i32;
        pub fn SineWave2(f: i32, p: i32) -> i32;
        pub fn SetVectorLength(v: *mut Vector, l: i32);
        pub fn CalculateQuatForPlane2(y: i32, q: *mut IQuaternion, n: *const FVector);
        pub fn Eul_FromQuat(d: *mut SVector, q: *const IQuaternion);
        pub fn Eul_FromHMatrix(d: *mut SVector, m: *const MatrixI);
        pub fn Eul_ToQuat(q: *mut IQuaternion, v: *const SVector);
        pub fn RotateVectorByXZRotation(r: *mut FVector, v: *const FVector, q: *const IQuaternion);
        pub fn GetQuaternionFromXZRotation(d: *mut IQuaternion, s: *const IQuaternion);
        pub fn NormalToQuaternion(q: *mut IQuaternion, n: *const SVector);
        pub fn NormaliseQuaternion(q: *mut IQuaternion);
        pub fn SlideVectorToVectorFF(v: *mut FVector, d: *const FVector, s: i32) -> i32;
        pub fn MakeVECTOR(x: i32, y: i32, z: i32) -> Vector;
        pub fn MakeFVECTOR(x: i32, y: i32, z: i32) -> FVector;
    }
}

/// Generic 3-operand macro wrappers (`t_<Name>` in shim.c).
macro_rules! cw3 { ($($n:ident: $a:ty, $b:ty, $c:ty;)*) => {
    mod cw { use super::*; extern "C" { $(pub fn $n(a: *mut $a, b: *const $b, c: *const $c);)* } }
}}
cw3! {
    t_SubVectorVVV: Vector, Vector, Vector; t_SubVectorVSV: Vector, SVector, Vector;
    t_SubVectorVSS: Vector, SVector, SVector; t_SubVectorSSS: SVector, SVector, SVector;
    t_SubVectorSSF: SVector, SVector, FVector; t_SubVectorFSS: FVector, SVector, SVector;
    t_SubVectorFSF: FVector, SVector, FVector; t_SubVectorFFS: FVector, FVector, SVector;
    t_SubVectorFFF: FVector, FVector, FVector; t_AddVectorSSS: SVector, SVector, SVector;
    t_AddVectorSFS: SVector, FVector, SVector; t_AddVectorSSF: SVector, SVector, FVector;
    t_AddVectorSFF: SVector, FVector, FVector; t_AddVectorFFS: FVector, FVector, SVector;
    t_AddVectorFFF: FVector, FVector, FVector; t_AddVectorVSF: Vector, SVector, FVector;
    t_CrossProductFFF: FVector, FVector, FVector;
}
macro_rules! cw2 { ($($n:ident: $a:ty, $b:ty;)*) => {
    mod cw2 { use super::*; extern "C" { $(pub fn $n(a: *mut $a, b: *const $b);)* } }
}}
cw2! {
    t_SubFromVectorVS: Vector, SVector; t_SubFromVectorSS: SVector, SVector;
    t_SubFromVectorSF: SVector, FVector; t_SubFromVectorFS: FVector, SVector;
    t_SubFromVectorFF: FVector, FVector; t_SetVectorVS: Vector, SVector;
    t_SetVectorSV: SVector, Vector; t_SetVectorSF: SVector, FVector;
    t_SetVectorFS: FVector, SVector; t_SetVectorFV: FVector, Vector;
    t_AddToVectorVS: Vector, SVector; t_AddToVectorSS: SVector, SVector;
    t_AddToVectorFS: FVector, SVector; t_AddToVectorFV: FVector, Vector;
    t_AddToVectorSF: SVector, FVector; t_CrossProductVertical: FVector, FVector;
}
mod cw_misc {
    use super::*;
    extern "C" {
        pub fn t_DotProductFF(a: *const FVector, b: *const FVector) -> i32;
        pub fn t_DotProductSS(a: *const SVector, b: *const SVector) -> i32;
        pub fn t_DotProduct2D(a: *const FVector, b: *const FVector) -> i32;
        pub fn t_ScaleVectorFF(v: *mut FVector, s: i32);
        pub fn t_ScaleVectorSF(v: *mut SVector, s: i32);
        pub fn t_ScaleVectorS(v: *mut SVector, s: i32);
        pub fn t_ScaleVectorF(v: *mut FVector, s: i32);
        pub fn t_LinearInterpFSSF(r: *mut FVector, a: *const SVector, b: *const SVector, t: i32);
        pub fn t_AddScaledVector2D(r: *mut Vector, a: *const Vector, s1: i32, b: *const Vector, s2: i32);
        pub fn t_AddOneScaledVector(r: *mut FVector, a: *const FVector, b: *const FVector, s: i32);
        pub fn t_AddToOneScaledVector(r: *mut FVector, b: *const FVector, s: i32);
        pub fn t_AddToOneScaledVector2D(r: *mut FVector, b: *const FVector, s: i32);
    }
}

/// C globals (rseed, vertQ, gameSpeed) are shared, so tests touching them serialise.
static C_GLOBALS: std::sync::Mutex<()> = std::sync::Mutex::new(());

const N: usize = 20_000;

/// Deterministic input generator biased toward interesting values.
struct Gen(u64);
impl Gen {
    fn new(seed: u64) -> Self {
        Gen(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn i32(&mut self) -> i32 {
        const EDGES: [i32; 16] = [0, 1, -1, 2, -2, 4095, 4096, -4096, 4097, 26754, 26755, 16_777_216, 16_777_217, i32::MAX, i32::MIN, i32::MIN + 1];
        let r = self.next();
        match r % 8 {
            0 => EDGES[(r >> 8) as usize % EDGES.len()],
            1 => ((r >> 8) as i32) % 64,
            2 => ((r >> 8) as i32) % 8192,
            3 => ((r >> 8) as i32) % (1 << 20),
            4 => ((r >> 8) as i32) % (1 << 24),
            _ => (r >> 16) as i32,
        }
    }
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next() % ((hi as i64 - lo as i64 + 1) as u64)) as i32
    }
    fn i16(&mut self) -> i16 {
        if self.next().is_multiple_of(4) {
            self.i32() as i16
        } else {
            self.range(-4096, 4096) as i16
        }
    }
    fn fv(&mut self) -> FVector {
        FVector::new(self.i32(), self.i32(), self.i32())
    }
    fn sv(&mut self) -> SVector {
        SVector { vx: self.i16(), vy: self.i16(), vz: self.i16(), pad: self.i16() }
    }
    fn v(&mut self) -> Vector {
        Vector { vx: self.i32(), vy: self.i32(), vz: self.i32(), pad: self.i32() }
    }
    fn q(&mut self) -> IQuaternion {
        IQuaternion::new(self.i32(), self.i32(), self.i32(), self.i32())
    }
    /// Small-ish quaternion components (|c| <= 4096).
    fn qs(&mut self) -> IQuaternion {
        IQuaternion::new(self.range(-4096, 4096), self.range(-4096, 4096), self.range(-4096, 4096), self.range(-4096, 4096))
    }
    /// Roughly unit quaternion (via the original axis/angle conversion).
    fn unit_q(&mut self) -> IQuaternion {
        let mut axis = FVector::new(self.range(-4096, 4096), self.range(-4096, 4096), self.range(-4096, 4096));
        make_unit(&mut axis);
        let mut q = IQuaternion::default();
        fixed_get_quaternion_from_rotation(&mut q, &IQuaternion::new(axis.vx, axis.vy, axis.vz, self.range(-8192, 8192)));
        q
    }
}

fn fdiv_c(x: i32, y: i32) -> i32 {
    unsafe { c::NewFDiv(x, y, c"rust".as_ptr(), 0) }
}

#[test]
fn fixed_point() {
    let mut g = Gen::new(1);
    for _ in 0..N * 5 {
        let (a, b) = (g.i32(), g.i32());
        assert_eq!(fmul(a, b), unsafe { c::FMul(a, b) }, "fmul({a},{b})");
        assert_eq!(fdiv2(a, b), unsafe { c::FDiv2(a, b) }, "fdiv2({a},{b})");
        if b != 0 {
            assert_eq!(fdiv(a, b), fdiv_c(a, b), "fdiv({a},{b})");
        }
    }
    // Unsigned reciprocal: negative divisors give invDiv == 0 (MSVC `unsigned long`).
    assert_eq!(fdiv_c(4096 << 12, -4096), 0);
    assert_eq!(fdiv(4096 << 12, -4096), 0);
    assert_eq!(fdiv(4096 << 12, i32::MIN), fdiv_c(4096 << 12, i32::MIN));
    assert_eq!(fdiv(8192, 4096), 8192);
    for &y in &[1, -1, 2, -2, 4096, -4096, i32::MIN, i32::MAX, 16_777_216, 16_777_217] {
        for &x in &[0, 1, -1, 4096, -4096, i32::MIN, i32::MAX] {
            assert_eq!(fdiv(x, y), fdiv_c(x, y), "fdiv({x},{y})");
            assert_eq!(fdiv2(x, y), unsafe { c::FDiv2(x, y) }, "fdiv2({x},{y})");
        }
    }
}

#[test]
fn random_sequence() {
    let _l = C_GLOBALS.lock().unwrap();
    let mut g = Gen::new(2);
    let mut rng = Rng::default();
    unsafe { c::SeedRandom() };
    assert_eq!(rng.rseed, unsafe { c::t_get_rseed() });
    for i in 0..N * 5 {
        let lim = match g.next() % 4 {
            0 => 0,
            1 => (g.next() % 16) as u32,
            2 => g.next() as u32,
            _ => (g.next() % 1000) as u32,
        };
        assert_eq!(rng.random(lim), unsafe { c::Random(lim) }, "call {i} lim {lim}");
        assert_eq!(rng.rseed, unsafe { c::t_get_rseed() });
        if i % 5000 == 4999 {
            let s = g.next() as u32;
            rng.rseed = s;
            unsafe { c::t_set_rseed(s) };
        }
    }
    // Edge states.
    for s in [0u32, 0x8000_0000, 0xC000_0000, 0xFFFF_FFFF, 0x4000_0000] {
        rng.rseed = s;
        unsafe { c::t_set_rseed(s) };
        assert_eq!(rng.random(7), unsafe { c::Random(7) });
        assert_eq!(rng.rseed, unsafe { c::t_get_rseed() });
    }
    rng.seed_random();
    unsafe { c::SeedRandom() };
    assert_eq!(rng.rseed, 398_623);
}

#[test]
fn scalar_helpers() {
    let mut g = Gen::new(3);
    for _ in 0..N {
        let x = g.i32();
        let u = g.next() as u32;
        assert_eq!(util_sqrt(u), unsafe { c::utilSqrt(u) }, "utilSqrt({u})");
        assert_eq!(util_sqrt(x as u32), unsafe { c::utilSqrt(x as u32) });
        assert_eq!(fsqrt(x), unsafe { c::t_fsqrt(x) });
        assert_eq!(fsqrt_f(x), unsafe { c::t_fsqrtf(x) });
        let a = x % (1 << 24);
        assert_eq!(rsin(a), unsafe { c::t_rsin(a) }, "rsin({a})");
        assert_eq!(rcos(a), unsafe { c::t_rcos(a) }, "rcos({a})");
        let c0 = g.range(-4096, 4096);
        assert_eq!(arccos(c0), unsafe { c::arccos(c0) }, "arccos({c0})");
        let (p, q) = (g.i32(), g.i32());
        assert_eq!(util_calc_angle(p, q), unsafe { c::t_calc_angle(p, q) });
        assert_eq!(find_shortest_angle(p, q), unsafe { c::FindShortestAngle(p, q) });
        assert_eq!(find_shortest_angle_signed(p, q), unsafe { c::FindShortestAngleSigned(p, q) });
        assert_eq!(aabs(x), unsafe { c::Aabs(x) });
        let (f, ph, b) = (g.i32(), g.i32(), g.range(-100_000, 100_000));
        if f != 0 && fdiv(to_fixed(b).wrapping_add(ph), f).unsigned_abs() < 1 << 24 {
            assert_eq!(sine_wave(f, ph, b), unsafe { c::SineWave(f, ph, b) });
        }
        if f != 0 && fdiv(fmul(ph, 12868).wrapping_mul(2), f).unsigned_abs() < 1 << 24 {
            assert_eq!(sine_wave2(f, ph), unsafe { c::SineWave2(f, ph) });
        }
    }
    for x in -4096..=4096 {
        assert_eq!(arccos(x), unsafe { c::arccos(x) }, "arccos({x})");
        assert_eq!(rsin(x), unsafe { c::t_rsin(x) });
        assert_eq!(rcos(x), unsafe { c::t_rcos(x) });
    }
}

#[test]
fn msvc_ftol_semantics() {
    // NaN / out of range -> 0x8000000000000000 -> (long) 0.
    assert_eq!(arccos(4097), 0);
    assert_eq!(arccos(i32::MIN), 0);
    assert_eq!(ftol32(4294967296.0 + 5.0), 5);
    assert_eq!(ftol32(-1.9), -1);
    assert_eq!(util_sqrt(u32::MAX), 0); // 2^32 wraps
    assert_eq!(arccos(-4096), 2047);
    assert_eq!(arccos(4096), 0);
}

#[test]
fn magnitudes_and_distances() {
    let mut g = Gen::new(4);
    for _ in 0..N {
        let (f1, f2) = (g.fv(), g.fv());
        let (s1, s2) = (g.sv(), g.sv());
        let (v1, v2) = (g.v(), g.v());
        unsafe {
            assert_eq!(magnitude_f(&f1), c::MagnitudeF(&f1), "MagnitudeF {f1:?}");
            assert_eq!(magnitude_2df(&f1), c::Magnitude2DF(&f1));
            assert_eq!(magnitude_2dv(&v1), c::Magnitude2DV(&v1));
            assert_eq!(magnitude_s(&s1), c::MagnitudeS(&s1));
            assert_eq!(magnitude_v(&v1), c::t_magnitude_v(&v1));
            assert_eq!(distance_between_points_2d(&v1, &v2), c::DistanceBetweenPoints2D(&v1, &v2));
            assert_eq!(distance_between_points_vv(&v1, &v2), c::DistanceBetweenPointsVV(&v1, &v2));
            assert_eq!(distance_between_points_ss(&s1, &s2), c::DistanceBetweenPointsSS(&s1, &s2));
            assert_eq!(distance_between_points_sv(&s1, &v2), c::DistanceBetweenPointsSV(&s1, &v2));
            assert_eq!(distance_between_points_sf(&s1, &f2), c::DistanceBetweenPointsSF(&s1, &f2));
            assert_eq!(distance_between_points_fs(&f1, &s2), c::DistanceBetweenPointsFS(&f1, &s2));
            assert_eq!(distance_between_points_ff(&f1, &f2), c::DistanceBetweenPointsFF(&f1, &f2));
            assert_eq!(distance_between_points_sqr_ss(&s1, &s2), c::DistanceBetweenPointsSqrSS(&s1, &s2));
            assert_eq!(distance_between_points_sqr_fs(&f1, &s2), c::DistanceBetweenPointsSqrFS(&f1, &s2));

            let bx = Box3D { pos: s1, size: v2 };
            let h = g.i32();
            assert_eq!(is_point_within_box(&bx, &v1), c::IsPointWithinBox(&bx, &v1) != 0);
            assert_eq!(is_point_within_box_2d(&bx, &v1), c::IsPointWithinBox2D(&bx, &v1) != 0);
            assert_eq!(is_point_within_box_plus_height(&bx, &v1, h), c::IsPointWithinBoxPlusHeight(&bx, &v1, h) != 0);
            // Boxes that actually contain the point sometimes.
            let near = Vector::new(s1.vx as i32 + g.range(-5, 50), s1.vy as i32 + g.range(-5, 50), s1.vz as i32 + g.range(-5, 50));
            let bx2 = Box3D { pos: s1, size: Vector::new(40, 40, 40) };
            assert_eq!(is_point_within_box(&bx2, &near), c::IsPointWithinBox(&bx2, &near) != 0);
            assert_eq!(is_point_within_box_plus_height(&bx2, &near, h), c::IsPointWithinBoxPlusHeight(&bx2, &near, h) != 0);

            let (mut a, mut b) = (v1, v1);
            flip_vector(&mut a);
            c::FlipVector(&mut b);
            assert_eq!(a, b);
            flip_vector_2d(&mut a);
            c::FlipVector2D(&mut b);
            assert_eq!(a, b);

            let (mut a, mut b) = (v1, v1);
            let len = g.i32();
            if len != 0 && fdiv(magnitude_v(&a), len) != 0 {
                set_vector_length(&mut a, len);
                c::SetVectorLength(&mut b, len);
                assert_eq!(a, b);
            }
        }
    }
}

#[test]
fn make_unit_and_normalise() {
    let mut g = Gen::new(5);
    let mut tested = 0;
    for i in 0..N * 2 {
        let f = if i % 2 == 0 { g.fv() } else { FVector::new(g.range(-40000, 40000), g.range(-40000, 40000), g.range(-40000, 40000)) };
        let mut a = f;
        if std::panic::catch_unwind(move || {
            let mut t = f;
            make_unit(&mut t);
        })
        .is_ok()
        {
            make_unit(&mut a);
            let mut b = f;
            unsafe { c::MakeUnit(&mut b) };
            assert_eq!(a, b, "MakeUnit {f:?}");
            tested += 1;
        }
        let mut a = f;
        let mut b = f;
        if magnitude_2df(&f) != 0 {
            make_unit_2d(&mut a);
            unsafe { c::MakeUnit2D(&mut b) };
            assert_eq!(a, b, "MakeUnit2D {f:?}");
        }
        let q = if i % 2 == 0 { g.q() } else { g.qs() };
        if std::panic::catch_unwind(move || {
            let mut t = q;
            normalise_quaternion(&mut t);
        })
        .is_ok()
        {
            let (mut a, mut b) = (q, q);
            normalise_quaternion(&mut a);
            unsafe { c::NormaliseQuaternion(&mut b) };
            assert_eq!(a, b, "NormaliseQuaternion {q:?}");
        }
    }
    assert!(tested > N, "MakeUnit cases run: {tested}");
}

#[test]
fn rotations_and_quaternions() {
    let _l = C_GLOBALS.lock().unwrap();
    let mut g = Gen::new(6);
    let mut done_slerp = 0;
    for i in 0..N {
        let f = if i % 2 == 0 { g.fv() } else { FVector::new(g.range(-1 << 20, 1 << 20), g.range(-1 << 20, 1 << 20), g.range(-1 << 20, 1 << 20)) };
        let s = g.sv();
        let mut rot = g.qs();
        rot.w = g.range(-(1 << 23), 1 << 23);
        let (mut a, mut b) = (FVector::default(), FVector::default());
        unsafe {
            fixed_rotate_vector_by_rotation(&mut a, &f, &rot);
            c::fixedRotateVectorByRotation(&mut b, &f, &rot);
            assert_eq!(a, b, "fixedRotateVectorByRotation {f:?} {rot:?}");
            fixed_rotate_vector_by_rotation_s(&mut a, &s, &rot);
            c::fixedRotateVectorByRotationS(&mut b, &s, &rot);
            assert_eq!(a, b, "fixedRotateVectorByRotationS {s:?} {rot:?}");
            rotate_vector_by_xz_rotation(&mut a, &f, &rot);
            c::RotateVectorByXZRotation(&mut b, &f, &rot);
            assert_eq!(a, b, "RotateVectorByXZRotation");
            rotate_vector_2d(&mut a, &f, rot.w);
            c::RotateVector2D(&mut b, &f, rot.w);
            assert_eq!(a, b);
            rotate_vector_2dx(&mut a, &f, rot.w);
            c::RotateVector2DX(&mut b, &f, rot.w);
            assert_eq!(a, b);

            let (q1, q2) = (g.q(), g.q());
            let (mut qa, mut qb) = (IQuaternion::default(), IQuaternion::default());
            fixed_quaternion_multiply(&mut qa, &q1, &q2);
            c::fixedQuaternionMultiply(&mut qb, &q1, &q2);
            assert_eq!(qa, qb);
            fixed_get_quaternion_from_rotation(&mut qa, &rot);
            c::fixedGetQuaternionFromRotation(&mut qb, &rot);
            assert_eq!(qa, qb);
            get_quaternion_from_xz_rotation(&mut qa, &rot);
            c::GetQuaternionFromXZRotation(&mut qb, &rot);
            assert_eq!(qa, qb);

            let uq = if i % 3 == 0 { g.qs() } else { g.unit_q() };
            fixed_get_rotation_from_quaternion(&mut qa, &uq);
            c::fixedGetRotationFromQuaternion(&mut qb, &uq);
            assert_eq!(qa, qb, "fixedGetRotationFromQuaternion {uq:?}");
            rotate_vector_by_quaternion_ff(&mut a, &f, &uq);
            c::RotateVectorByQuaternionFF(&mut b, &f, &uq);
            assert_eq!(a, b, "RotateVectorByQuaternionFF");

            let (u1, u2) = (g.unit_q(), g.unit_q());
            let t = g.range(0, 4096);
            // Skip pairs whose cosom leaves arccos's domain (x87 vs SSE NaN conversion).
            let cos_ok = |b2: IQuaternion| {
                let c = fmul(u1.x, b2.x) + fmul(u1.y, b2.y) + fmul(u1.z, b2.z) + fmul(u1.w, b2.w);
                c.abs() <= 4096
            };
            if cos_ok(u2) && cos_ok(IQuaternion::new(-u2.x, -u2.y, -u2.z, -u2.w)) {
                iquat_slerp(&u1, &u2, t, &mut qa);
                c::IQuatSlerp(&u1, &u2, t, &mut qb);
                assert_eq!(qa, qb, "IQuatSlerp {u1:?} {u2:?} {t}");
                // Opposite-quaternion branch.
                let opp = IQuaternion::new(-u1.x, -u1.y, -u1.z, -u1.w);
                iquat_slerp(&u1, &opp, t, &mut qa);
                c::IQuatSlerp(&u1, &opp, t, &mut qb);
                assert_eq!(qa, qb);
                done_slerp += 1;
            }

            let (mut ea, mut eb) = (SVector::default(), SVector::default());
            let qq = if i % 2 == 0 { g.q() } else { g.unit_q() };
            eul_from_quat(&mut ea, &qq);
            c::Eul_FromQuat(&mut eb, &qq);
            assert_eq!(ea, eb, "Eul_FromQuat {qq:?}");
            let mut m = MatrixI::default();
            for r in m.matrix.iter_mut() {
                for v in r.iter_mut() {
                    *v = if g.next().is_multiple_of(2) { g.range(-4096, 4096) } else { g.i32() };
                }
            }
            if i % 7 == 0 {
                m.matrix[2][0] = 0;
                m.matrix[2][1] = 0;
            }
            eul_from_hmatrix(&mut ea, &m);
            c::Eul_FromHMatrix(&mut eb, &m);
            assert_eq!(ea, eb);
            eul_to_quat(&mut qa, &s);
            c::Eul_ToQuat(&mut qb, &s);
            assert_eq!(qa, qb);

            let mut n = g.sv();
            n.vy = g.range(-4096, 4096) as i16;
            if i % 5 == 0 {
                n.vx = 0;
            }
            if i % 10 == 0 {
                n.vz = 0;
            }
            normal_to_quaternion(&mut qa, &n);
            c::NormalToQuaternion(&mut qb, &n);
            assert_eq!(qa, qb, "NormalToQuaternion {n:?}");

            let mut nf = FVector::new(g.range(-4096, 4096), g.range(-4096, 4096), g.range(-4096, 4096));
            if i % 6 == 0 {
                nf.vy = -4096;
            }
            if i % 11 == 0 {
                nf.vx = 0;
                nf.vz = 0;
            }
            let yr = g.range(-8192, 8192);
            let vq = IQuaternion::new(g.range(-4096, 4096), g.range(-4096, 4096), g.range(-4096, 4096), 0);
            let mut vq_r = vq;
            c::t_set_vertq(&vq);
            let init = g.qs();
            let (mut pa, mut pb) = (init, init);
            if magnitude_2df(&FVector::new(nf.vx, 0, nf.vz)) != 0 || nf.vy == -4096 {
                calculate_quat_for_plane2(yr, &mut pa, &nf, &mut vq_r);
                c::CalculateQuatForPlane2(yr, &mut pb, &nf);
                let mut vq_c = IQuaternion::default();
                c::t_get_vertq(&mut vq_c);
                assert_eq!(pa, pb, "CalculateQuatForPlane2");
                assert_eq!(vq_r, vq_c);
            }

            let (mut va, mut vb) = (f, f);
            let dest = g.fv();
            let speed = g.i32();
            let gs = g.range(0, 8192);
            c::t_set_gamespeed(gs);
            let sp = fmul(speed, gs);
            let mut d = FVector::default();
            f2_math::sub_vector_fff(&mut d, &dest, &f);
            if !(magnitude_f(&d) == 0 && sp <= 0) {
                assert_eq!(slide_vector_to_vector_ff(&mut va, &dest, speed, gs), c::SlideVectorToVectorFF(&mut vb, &dest, speed) != 0);
                assert_eq!(va, vb);
            }
        }
    }
    assert!(done_slerp > N / 2, "too few slerp cases: {done_slerp}");
}

#[test]
fn vector_macros() {
    let mut g = Gen::new(7);
    macro_rules! chk3 {
        ($rs:ident, $cf:ident, $d:expr, $a:expr, $b:expr) => {{
            let (a, b) = ($a, $b);
            let mut x = $d;
            let mut y = x;
            $rs(&mut x, &a, &b);
            unsafe { cw::$cf(&mut y, &a, &b) };
            assert_eq!(x, y, stringify!($rs));
        }};
    }
    macro_rules! chk2 {
        ($rs:ident, $cf:ident, $d:expr, $a:expr) => {{
            let a = $a;
            let mut x = $d;
            let mut y = x;
            $rs(&mut x, &a);
            unsafe { cw2::$cf(&mut y, &a) };
            assert_eq!(x, y, stringify!($rs));
        }};
    }
    for _ in 0..N {
        chk3!(sub_vector_vvv, t_SubVectorVVV, g.v(), g.v(), g.v());
        chk3!(sub_vector_vsv, t_SubVectorVSV, g.v(), g.sv(), g.v());
        chk3!(sub_vector_vss, t_SubVectorVSS, g.v(), g.sv(), g.sv());
        chk3!(sub_vector_sss, t_SubVectorSSS, g.sv(), g.sv(), g.sv());
        chk3!(sub_vector_ssf, t_SubVectorSSF, g.sv(), g.sv(), g.fv());
        chk3!(sub_vector_fss, t_SubVectorFSS, g.fv(), g.sv(), g.sv());
        chk3!(sub_vector_fsf, t_SubVectorFSF, g.fv(), g.sv(), g.fv());
        chk3!(sub_vector_ffs, t_SubVectorFFS, g.fv(), g.fv(), g.sv());
        chk3!(sub_vector_fff, t_SubVectorFFF, g.fv(), g.fv(), g.fv());
        chk3!(add_vector_sss, t_AddVectorSSS, g.sv(), g.sv(), g.sv());
        chk3!(add_vector_sfs, t_AddVectorSFS, g.sv(), g.fv(), g.sv());
        chk3!(add_vector_ssf, t_AddVectorSSF, g.sv(), g.sv(), g.fv());
        chk3!(add_vector_sff, t_AddVectorSFF, g.sv(), g.fv(), g.fv());
        chk3!(add_vector_ffs, t_AddVectorFFS, g.fv(), g.fv(), g.sv());
        chk3!(add_vector_fff, t_AddVectorFFF, g.fv(), g.fv(), g.fv());
        chk3!(add_vector_vsf, t_AddVectorVSF, g.v(), g.sv(), g.fv());
        chk3!(cross_product_fff, t_CrossProductFFF, g.fv(), g.fv(), g.fv());
        chk2!(sub_from_vector_vs, t_SubFromVectorVS, g.v(), g.sv());
        chk2!(sub_from_vector_ss, t_SubFromVectorSS, g.sv(), g.sv());
        chk2!(sub_from_vector_sf, t_SubFromVectorSF, g.sv(), g.fv());
        chk2!(sub_from_vector_fs, t_SubFromVectorFS, g.fv(), g.sv());
        chk2!(sub_from_vector_ff, t_SubFromVectorFF, g.fv(), g.fv());
        chk2!(set_vector_vs, t_SetVectorVS, g.v(), g.sv());
        chk2!(set_vector_sv, t_SetVectorSV, g.sv(), g.v());
        chk2!(set_vector_sf, t_SetVectorSF, g.sv(), g.fv());
        chk2!(set_vector_fs, t_SetVectorFS, g.fv(), g.sv());
        chk2!(set_vector_fv, t_SetVectorFV, g.fv(), g.v());
        chk2!(add_to_vector_vs, t_AddToVectorVS, g.v(), g.sv());
        chk2!(add_to_vector_ss, t_AddToVectorSS, g.sv(), g.sv());
        chk2!(add_to_vector_fs, t_AddToVectorFS, g.fv(), g.sv());
        chk2!(add_to_vector_fv, t_AddToVectorFV, g.fv(), g.v());
        chk2!(add_to_vector_sf, t_AddToVectorSF, g.sv(), g.fv());
        chk2!(cross_product_vertical, t_CrossProductVertical, g.fv(), g.fv());

        let (a, b, sc) = (g.fv(), g.fv(), g.i32());
        let (sa, sb) = (g.sv(), g.sv());
        unsafe {
            use cw_misc::*;
            assert_eq!(dot_product_ff(&a, &b), t_DotProductFF(&a, &b));
            assert_eq!(dot_product_ss(&sa, &sb), t_DotProductSS(&sa, &sb));
            assert_eq!(dot_product_2d(&a, &b), t_DotProduct2D(&a, &b));
            let (mut x, mut y) = (a, a);
            scale_vector_ff(&mut x, sc);
            t_ScaleVectorFF(&mut y, sc);
            assert_eq!(x, y);
            scale_vector(&mut x, sc);
            t_ScaleVectorF(&mut y, sc);
            assert_eq!(x, y);
            let (mut x, mut y) = (sa, sa);
            scale_vector_sf(&mut x, sc);
            t_ScaleVectorSF(&mut y, sc);
            assert_eq!(x, y);
            scale_vector_s(&mut x, sc);
            t_ScaleVectorS(&mut y, sc);
            assert_eq!(x, y);
            let (mut x, mut y) = (b, b);
            linear_interp_fssf(&mut x, &sa, &sb, sc);
            t_LinearInterpFSSF(&mut y, &sa, &sb, sc);
            assert_eq!(x, y);
            add_one_scaled_vector(&mut x, &a, &b, sc);
            t_AddOneScaledVector(&mut y, &a, &b, sc);
            assert_eq!(x, y);
            add_to_one_scaled_vector(&mut x, &b, sc);
            t_AddToOneScaledVector(&mut y, &b, sc);
            assert_eq!(x, y);
            add_to_one_scaled_vector_2d(&mut x, &a, sc);
            t_AddToOneScaledVector2D(&mut y, &a, sc);
            assert_eq!(x, y);
            let (v1, v2, s2) = (g.v(), g.v(), g.i32());
            let mut x = g.v();
            let mut y = x;
            add_scaled_vector_2d(&mut x, &v1, sc, &v2, s2);
            t_AddScaledVector2D(&mut y, &v1, sc, &v2, s2);
            assert_eq!(x, y);
            let (p, q, r) = (g.i32(), g.i32(), g.i32());
            assert_eq!(make_vector(p, q, r), c::MakeVECTOR(p, q, r));
            assert_eq!(make_fvector(p, q, r), c::MakeFVECTOR(p, q, r));
        }
    }
}
