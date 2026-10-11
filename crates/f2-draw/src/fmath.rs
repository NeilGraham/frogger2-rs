//! Float methods `core` lacks, for `no_std` builds only (`libm`). With `std` the inherent
//! `f32` methods are used, so std builds are unchanged.

#[allow(dead_code)] // unused where `core` already has the methods (newer toolchains)
pub(crate) trait FloatExt {
    fn sqrt(self) -> Self;
    fn acos(self) -> Self;
    fn floor(self) -> Self;
    fn round(self) -> Self;
    fn sin_cos(self) -> (Self, Self)
    where
        Self: Sized;
}

impl FloatExt for f32 {
    fn sqrt(self) -> f32 {
        libm::sqrtf(self)
    }
    fn acos(self) -> f32 {
        libm::acosf(self)
    }
    fn floor(self) -> f32 {
        libm::floorf(self)
    }
    fn round(self) -> f32 {
        libm::roundf(self)
    }
    fn sin_cos(self) -> (f32, f32) {
        libm::sincosf(self)
    }
}
