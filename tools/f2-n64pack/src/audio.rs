//! Sound effects (deduplicated `.wus` samples) and music → libdragon `wav64`.
//!
//! STUB: owned by the audio/FMV work item. Contract with the driver: both functions write
//! finished files into `ctx.out` (under `sfx/` and `music/`) and report them.

use crate::{ClassReport, Ctx, Result};

/// Deduplicates and encodes every sample of the 19 sound banks; writes the index.
pub fn pack_sfx(_ctx: &Ctx) -> Result<ClassReport> {
    Ok(ClassReport { files: Vec::new(), notes: vec!["sfx: not implemented yet".into()] })
}

/// Encodes the music tracks at (about) `kbps` per stereo track.
pub fn pack_music(_ctx: &Ctx, _kbps: u32) -> Result<ClassReport> {
    Ok(ClassReport { files: Vec::new(), notes: vec!["music: not implemented yet".into()] })
}
