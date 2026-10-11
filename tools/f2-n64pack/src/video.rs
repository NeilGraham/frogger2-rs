//! FMV: the 14 Bink movies → libdragon video player format (+ `wav64` audio track).
//!
//! STUB: owned by the audio/FMV work item. Contract with the driver: [`pack_fmv`] writes
//! finished files under `video/` in `ctx.out` whose total size is at most `budget` bytes
//! (best quality that fits, capped at the decoder's limits) and reports them.

use crate::{ClassReport, Ctx, Result};

pub fn pack_fmv(_ctx: &Ctx, _budget: u64) -> Result<ClassReport> {
    Ok(ClassReport { files: Vec::new(), notes: vec!["fmv: not implemented yet".into()] })
}
