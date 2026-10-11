//! Nintendo 64 asset pack builder (N64 plan phase 1C).
//!
//! Reads an imported rendition (`game-data/<rendition>/{game,music,video}`) and writes a
//! DFS-ready tree (default `target/n64/pack/`) plus `manifest.json`, then prints the
//! cartridge budget table (`docs/N64-PLAN.md` §2) from the real output sizes.
//!
//! Pipeline: every asset class produces either
//! * [`RawAsset`]s — uncompressed runtime-ready bytes (levels, packed models, converted
//!   textures) that the driver compresses with the codec chosen per class (F2Z with a
//!   per-class shared dictionary, or a libdragon `mkasset` level), or
//! * finished files written directly into the pack (sound effects, music, FMV, which are
//!   already compressed by their media codec), reported as [`PackedFile`]s.
//!
//! Nothing produced here may be committed: it is all derived from the user's game data.

pub mod audio;
pub mod bench;
pub mod budget;
pub mod codec;
pub mod ctx;
pub mod levels;
pub mod models;
pub mod rmodels;
pub mod textures;
pub mod tools;
pub mod video;

pub use ctx::Ctx;

use serde::Serialize;

/// Budget classes (rows of the plan's §2 table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub enum Class {
    Code,
    Models,
    Textures,
    Levels,
    Sfx,
    Music,
    Fmv,
    /// Pack index, dictionaries, DFS overhead.
    Meta,
}

impl Class {
    pub const ALL: [Class; 8] = [Class::Code, Class::Models, Class::Textures, Class::Levels, Class::Sfx, Class::Music, Class::Fmv, Class::Meta];

    pub fn name(self) -> &'static str {
        match self {
            Class::Code => "boot + code (reserve)",
            Class::Models => "models",
            Class::Textures => "textures + fonts + backdrops",
            Class::Levels => "levels (maps, collision, scenics, scripts, demos, text)",
            Class::Sfx => "sound effects",
            Class::Music => "music",
            Class::Fmv => "FMV",
            Class::Meta => "pack index + dictionaries + DFS overhead",
        }
    }
}

/// Uncompressed runtime bytes for one pack file; the driver compresses it.
#[derive(Clone, Debug)]
pub struct RawAsset {
    /// Path inside the pack, `/`-separated, without the compression suffix (e.g. `models/garden1.mdl`).
    pub path: String,
    pub class: Class,
    pub data: Vec<u8>,
    /// Size of the retail source this was made from (for the "from" column).
    pub source_bytes: u64,
    /// Human-readable conversion notes (e.g. "256x256 RGBA16 -> 2x CI8 64x32 tiles").
    pub notes: Vec<String>,
    /// F2Z structure transforms describing `data` (must cover it exactly), or empty to let
    /// the codec try its generic candidates.
    pub segments: Vec<f2_n64z::Segment>,
}

/// A finished file inside the pack.
#[derive(Clone, Debug, Serialize)]
pub struct PackedFile {
    pub path: String,
    pub class: Class,
    pub bytes: u64,
    pub source_bytes: u64,
    /// Codec / format used (e.g. "f2z+dict", "vadpcm", "opus 48kbps", "h264 q80").
    pub format: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Result of one class step.
#[derive(Clone, Debug, Default, Serialize)]
pub struct ClassReport {
    pub files: Vec<PackedFile>,
    /// Class-level notes for the manifest (decisions, measured stats).
    pub notes: Vec<String>,
}

pub type Result<T> = std::result::Result<T, String>;
