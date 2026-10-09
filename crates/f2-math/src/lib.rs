//! Bit-exact port of the original fixed-point and vector maths (Common/fixed.c, maths.c).
//!
//! Sources: `Common/fixed.{c,h}`, `Common/maths.{c,h}`, maths types from
//! `Common/types.{h,c}`, `Common/newpsx.h`, and the PC shims `PC/libgte.h` (`rsin`/`rcos`,
//! `VECTOR`/`SVECTOR`) and `PC/islutil.{h,cpp}` (`utilSqrt`, `utilCalcAngle`).
//! All integer arithmetic wraps like MSVC 32-bit `long`; float->int conversions follow
//! MSVC 6 `_ftol` (see [`util::ftol`]).
#![forbid(unsafe_code)]

pub mod acostable;
pub mod fixed;
pub mod maths;
pub mod types;
pub mod util;
pub mod vector;

pub use acostable::BB_ACOSTABLE;
pub use fixed::{fdiv, fdiv2, fmul, to_fixed, Fixed};
pub use maths::*;
pub use types::*;
pub use util::*;
pub use vector::*;
