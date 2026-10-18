//! Float methods `core` lacks, for `no_std` builds only (`libm`). With `std` the inherent
//! `f32`/`f64` methods are used (inherent methods win over this trait), so std builds are
//! bit-for-bit unchanged.

pub(crate) trait FloatExt {
    fn sqrt(self) -> Self;
    fn sin(self) -> Self;
    fn cos(self) -> Self;
    fn acos(self) -> Self;
    fn sin_cos(self) -> (Self, Self)
    where
        Self: Sized;
    fn floor(self) -> Self;
    fn ceil(self) -> Self;
    fn round(self) -> Self;
    fn trunc(self) -> Self;
    fn atan(self) -> Self;
}

impl FloatExt for f32 {
    fn sqrt(self) -> f32 {
        libm::sqrtf(self)
    }
    fn sin(self) -> f32 {
        libm::sinf(self)
    }
    fn cos(self) -> f32 {
        libm::cosf(self)
    }
    fn acos(self) -> f32 {
        libm::acosf(self)
    }
    fn sin_cos(self) -> (f32, f32) {
        libm::sincosf(self)
    }
    fn floor(self) -> f32 {
        libm::floorf(self)
    }
    fn ceil(self) -> f32 {
        libm::ceilf(self)
    }
    fn round(self) -> f32 {
        libm::roundf(self)
    }
    fn trunc(self) -> f32 {
        libm::truncf(self)
    }
    fn atan(self) -> f32 {
        libm::atanf(self)
    }
}

impl FloatExt for f64 {
    fn sqrt(self) -> f64 {
        libm::sqrt(self)
    }
    fn sin(self) -> f64 {
        libm::sin(self)
    }
    fn cos(self) -> f64 {
        libm::cos(self)
    }
    fn acos(self) -> f64 {
        libm::acos(self)
    }
    fn sin_cos(self) -> (f64, f64) {
        libm::sincos(self)
    }
    fn floor(self) -> f64 {
        libm::floor(self)
    }
    fn ceil(self) -> f64 {
        libm::ceil(self)
    }
    fn round(self) -> f64 {
        libm::round(self)
    }
    fn trunc(self) -> f64 {
        libm::trunc(self)
    }
    fn atan(self) -> f64 {
        libm::atan(self)
    }
}
