//! Parsers for every file format the Frogger 2 PC game reads.
//!
//! Pure Rust, no I/O: every parser takes bytes and returns owned data.
//! See `research/asset-formats.md` for the format documentation and the
//! original loader code each parser mirrors.

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

/// `alloc` items the std prelude would provide (the crate also builds `no_std`).
#[allow(unused_imports)]
pub(crate) mod prelude {
    pub use alloc::borrow::ToOwned;
    pub use alloc::boxed::Box;
    pub use alloc::string::{String, ToString};
    pub use alloc::vec::Vec;
    pub use alloc::{format, vec};
}

pub mod crc;
pub mod entity;
pub mod fev;
pub mod fla;
pub mod lang;
pub mod level;
pub mod misc;
pub mod objbank;
pub mod read;
pub mod texture;
pub mod wus;

pub use crc::crc;
pub use read::{Error, Result};
