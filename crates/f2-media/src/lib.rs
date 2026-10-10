//! Media access for the Frogger 2 port: disc images (CUE/BIN, Redump zip, ISO, CloneCD,
//! Alcohol, Nero, cdrdao), optical drives (TOC + CD-DA ripping), ISO9660, Red Book audio
//! tracks and InstallShield 5 cabinets.
//!
//! Nothing here is Frogger-specific except the choice of supported variants.

pub mod ccd;
pub mod cdrom;
pub mod convert;
pub mod cue;
pub mod disc;
pub mod iscab;
pub mod iso9660;
pub mod mds;
pub mod nrg;
pub mod sector;
pub mod toc;
pub mod vfs;

pub use disc::{Disc, Track, TrackKind};
pub use iscab::IsCab;
pub use iso9660::{IsoFs, Names};
pub use vfs::{DirFs, FileEntry, Vfs};
