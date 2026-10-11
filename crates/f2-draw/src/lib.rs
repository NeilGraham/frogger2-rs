//! Drawing the simulation state (`PC/drawloop.cpp`) into an `f2-engine` [`Renderer`]:
//! level scene and actors ([`render`]), shadows / sprites / special effects ([`fx`]) and the
//! 2D layers ([`overlay`]). Shared by the desktop front end (`crates/frogger`) and the N64
//! port (`crates/frogger-n64`); builds `no_std` + `alloc` without the `std` feature.
//!
//! [`Renderer`]: f2_engine::Renderer

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

#[cfg(not(feature = "std"))]
mod fmath;

/// `alloc` items the std prelude would provide.
#[allow(unused_imports)]
pub(crate) mod prelude {
    pub use alloc::borrow::ToOwned;
    pub use alloc::boxed::Box;
    pub use alloc::string::{String, ToString};
    pub use alloc::vec::Vec;
    pub use alloc::{format, vec};
}

pub mod fx;
pub mod overlay;
pub mod render;
