//! Export for the Nintendo 64 (docs/EXPORT.md "N64"): a `.z64` ROM of the player's game from
//! the data-free runtime (`runtime-n64-<v>.zip`, [`runtime`]), in pure Rust:
//!
//! 1. the asset pack: `f2-n64pack`'s pipeline in its portable profile (pure-Rust encoders,
//!    the same bytes as `tools/n64/build.sh --portable`; music VADPCM at the best rate that
//!    fits, no FMV), into a work directory next to the output;
//! 2. the DragonFS image of the pack (`f2_n64pack::dfs`, = libdragon `mkdfs`);
//! 3. the ROM: the runtime's IPL3/header, compressed ELF and symbols with the DFS, laid out
//!    like `n64tool --toc` ([`rom`]), the title = the disc's short title in upper case
//!    ("FROGGER 2");
//! 4. the 64 MiB size check `build.sh` does; if the full ROM does not fit, the pack is redone
//!    without music (the `--small` variant) and the export says so.

pub mod rom;
pub mod runtime;

use crate::Job;
use std::path::{Path, PathBuf};

/// Builds the portable pack of `rendition` into `work/pack` (f2-n64pack `--portable`,
/// optionally `--only levels,models,textures,sfx` = small); `Ok(false)` when it does not fit.
pub fn build_pack(rendition: &Path, work: &Path, spec: &runtime::PackSpec, small: bool) -> Result<bool, String> {
    if spec.profile != "portable" {
        return Err(format!("runtime pack profile {:?} is not supported (this exporter builds \"portable\" packs)", spec.profile));
    }
    let ctx = f2_n64pack::Ctx {
        rendition: rendition.to_path_buf(),
        out: work.join("pack"),
        cache: work.join("cache"),
        tools: PathBuf::new(),
        jobs: std::thread::available_parallelism().map_or(4, |n| n.get()),
        quick: false,
        portable: true,
    };
    let mut cfg = f2_n64pack::budget::BudgetConfig { cart_bytes: spec.cart_bytes, code_reserve: spec.code_reserve, ..Default::default() };
    if small {
        cfg.only = Some(["levels", "models", "textures", "sfx"].map(String::from).to_vec());
    }
    f2_n64pack::budget::build(&ctx, &cfg)
}

/// The ROM of a pack directory with a runtime: DFS + assembly + title; errors if it exceeds
/// the runtime's size limit.
pub fn rom_from_pack(rt: &runtime::Runtime, pack: &Path, title: &str) -> Result<Vec<u8>, String> {
    let dfs = f2_n64pack::dfs::build_dir(pack)?;
    rt.assemble(&dfs, Some(title))
}

pub(crate) fn export(job: &Job) -> Result<PathBuf, String> {
    let log = job.log;
    let rt = runtime::Runtime::load(job.runtime)?;
    if rt.manifest.version != job.branding.version {
        log(&format!("Note: N64 runtime {} with app {} (use the runtime of the same release)", rt.manifest.version, job.branding.version));
    }
    if !job.rendition.join("game").is_dir() {
        return Err(format!("{}: no game/ folder (not an imported game)", job.rendition.display()));
    }
    let title = rom::rom_title(&job.branding.short_title);
    let stem = job.branding.file_stem();
    let out = job.out_dir.join(format!("{stem}.z64"));
    let work = job.out_dir.join(format!(".{stem}.n64-work"));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
    let result = (|| {
        let max = rt.manifest.rom.max_size;
        for small in [false, true] {
            log(&format!("N64: building the asset pack{} (several minutes)", if small { " without music (small ROM)" } else { "" }));
            let fits = build_pack(job.rendition, &work, &rt.manifest.pack, small)?;
            if fits {
                log("N64: writing the DragonFS image and the ROM");
                let rom = rom_from_pack(&rt, &work.join("pack"), &title)?;
                if rom.len() as u64 <= max {
                    std::fs::write(&out, &rom).map_err(|e| format!("{}: {e}", out.display()))?;
                    log(&format!(
                        "N64: {} ({} bytes of {max}), title {title:?}{}",
                        out.display(),
                        rom.len(),
                        if small { "; no music: the full ROM does not fit the cartridge" } else { "" }
                    ));
                    return Ok(out.clone());
                }
                log(&format!("N64: the ROM is {} bytes, over the {max}-byte cartridge", rom.len()));
            } else {
                log("N64: the pack does not fit the cartridge");
            }
        }
        Err(format!("the game does not fit a {max}-byte N64 cartridge even without music"))
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}
