//! Media access for the Frogger 2 port: disc images (CUE/BIN, Redump zip, ISO),
//! ISO9660, Red Book audio tracks and InstallShield 5 cabinets.
//!
//! Nothing here is Frogger-specific except the choice of supported variants.

pub mod cue;
pub mod disc;
pub mod iscab;
pub mod iso9660;
pub mod sector;
pub mod vfs;

pub use disc::{Disc, Track, TrackKind};
pub use iscab::IsCab;
pub use iso9660::{IsoFs, Names};
pub use vfs::{DirFs, FileEntry, Vfs};
