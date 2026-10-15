//! Export for ps5 (docs/EXPORT.md).

use crate::Job;
use std::path::PathBuf;

pub(crate) fn export(job: &Job) -> Result<PathBuf, String> {
    Err(format!("{} export is not implemented yet", job.target.name()))
}
