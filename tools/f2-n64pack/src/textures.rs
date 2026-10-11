//! Textures, fonts and backdrops → RDP formats.
//!
//! STUB: owned by the texture work item. Contract with the driver:
//! [`convert`] returns one [`RawAsset`] per runtime file (uncompressed; the driver
//! compresses them with the class codec) under `textures/…`, plus class notes.

use crate::{Ctx, RawAsset, Result};

/// Converts every texture bank, backdrop and font of the rendition.
pub fn convert(_ctx: &Ctx) -> Result<(Vec<RawAsset>, Vec<String>)> {
    Ok((Vec::new(), vec!["textures: not implemented yet".into()]))
}
