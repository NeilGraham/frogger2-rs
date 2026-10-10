//! Platform-independent transcendental functions for every float path that can influence
//! game state.
//!
//! `f64::sin`/`cos`/`atan2`/`acos` call the platform C library (glibc, the Windows UCRT,
//! Apple libm), whose last-ulp results differ between OSes. These wrappers call the
//! pure-Rust [`libm`] crate (a port of musl's fdlibm-derived code) instead: it uses only
//! IEEE `+ − × ÷` (and correctly rounded `sqrt`), without FMA, so it returns the same bits
//! on every OS and on x86-64/aarch64.
//!
//! Retail parity: on every input the game can produce, the ported formulas give the same
//! integer results with these functions as with glibc (exhaustive / representative audit in
//! `examples/float_audit.rs`, decision in docs/DECISIONS.md, policy in docs/NETPLAY.md §9).

/// Deterministic `sin` (radians).
#[inline]
pub fn sin(x: f64) -> f64 {
    libm::sin(x)
}

/// Deterministic `cos` (radians).
#[inline]
pub fn cos(x: f64) -> f64 {
    libm::cos(x)
}

/// Deterministic `atan2(y, x)` (radians).
#[inline]
pub fn atan2(y: f64, x: f64) -> f64 {
    libm::atan2(y, x)
}

/// Deterministic `acos` (radians); NaN outside `[-1, 1]`.
#[inline]
pub fn acos(x: f64) -> f64 {
    libm::acos(x)
}
