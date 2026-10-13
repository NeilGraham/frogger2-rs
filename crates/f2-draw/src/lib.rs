//! Drawing the simulation state (`PC/drawloop.cpp`) into an `f2-engine` [`Renderer`]:
//! level scene and actors ([`render`]), shadows / sprites / special effects ([`fx`]) and the
//! 2D layers ([`overlay`]). Shared by the desktop front end (`crates/frogger`) and the N64
//! port (`crates/frogger-n64`); builds `no_std` + `alloc` without the `std` feature.
//!
//! [`Renderer`]: f2_engine::Renderer

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(not(target_vendor = "nintendo64"), forbid(unsafe_code))]
// The N64 build only: `link_section` (an "unsafe" attribute) places the per-actor draw path in
// the I-cache-friendly hot region (tools/n64/Makefile); there is still no unsafe code.
#![cfg_attr(target_vendor = "nintendo64", deny(unsafe_code))]

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

pub mod book;
pub mod fx;
pub mod overlay;
pub mod render;
