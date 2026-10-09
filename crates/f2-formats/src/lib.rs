//! Parsers for every file format the Frogger 2 PC game reads.
//!
//! Pure Rust, no I/O: every parser takes bytes and returns owned data.
//! See `research/asset-formats.md` for the format documentation and the
//! original loader code each parser mirrors.

#![forbid(unsafe_code)]

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
