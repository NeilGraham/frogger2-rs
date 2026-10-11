//! Bit-exact port of the original fixed-point and vector maths (Common/fixed.c, maths.c).
//!
//! Sources: `Common/fixed.{c,h}`, `Common/maths.{c,h}`, maths types from
//! `Common/types.{h,c}`, `Common/newpsx.h`, and the PC shims `PC/libgte.h` (`rsin`/`rcos`,
//! `VECTOR`/`SVECTOR`) and `PC/islutil.{h,cpp}` (`utilSqrt`, `utilCalcAngle`).
//! All integer arithmetic wraps like MSVC 32-bit `long`; float->int conversions follow
//! MSVC 6 `_ftol` (see [`util::ftol`]).
//!
//! `no_std` (core only, no allocator) when the default `std` feature is off, for future
//! console/embedded targets (docs/PLATFORMS.md). Float maths goes through [`detfloat`] (libm),
//! never the host C library, so results are identical with and without `std`.
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

pub mod acostable;
pub mod detfloat;
pub mod fixed;
pub mod maths;
mod tables;
pub mod types;
pub mod util;
pub mod vector;

pub use acostable::BB_ACOSTABLE;
pub use fixed::{fdiv, fdiv2, fmul, to_fixed, Fixed};
pub use maths::*;
pub use types::*;
pub use util::*;
pub use vector::*;
