//! Export for PS4: a fake package (fPKG) of the player's game, written in pure Rust from the
//! prebuilt runtime (docs/EXPORT.md "PS4").
//!
//! The package is what `tools/ps4/build-pkg.sh --data` makes with OpenOrbis' `PkgTool.Core`
//! (LibOrbisPkg): app files at `/app0` (`eboot.bin`, `sce_module/`, `sce_sys/about/right.sprx`,
//! the generated `sce_sys/keystone`), the game data at `/app0/data` (where the PS4 host looks
//! first), and `param.sfo`, `icon0.png`, `pic0.png`, `pic1.png` as package entries. The PKG/PFS
//! format is reimplemented after LibOrbisPkg (OpenOrbis, LGPL-2.1, C#; no code copied) in
//! `ps4/pkg.rs`, `ps4/pfs.rs`, `ps4/crypto.rs`.

pub(crate) mod art;
mod crypto;
mod keys;
mod pfs;
mod pkg;
#[cfg(test)]
mod reader;
pub(crate) mod runtime;
mod sfo;
#[cfg(test)]
pub(crate) mod tests;

use crate::{util, Job};
use runtime::Runtime;
use std::path::PathBuf;

/// The passcode of homebrew fake packages (all zeroes, as create-gp4 writes it).
const PASSCODE: &str = "00000000000000000000000000000000";

pub(crate) fn export(job: &Job) -> Result<PathBuf, String> {
    let log = job.log;
    let rt = Runtime::load(job.runtime, "ps4", log)?;
    let content_id = rt.str("content_id")?.to_string();
    let title_id = rt.str("title_id")?.to_string();
    if content_id.len() != 36 || title_id.len() != 9 || content_id[7..16] != title_id {
        return Err(format!("runtime.json: content ID {content_id:?} / title ID {title_id:?} malformed"));
    }
    let data_dir = rt.json.get("data_dir").and_then(|v| v.as_str()).unwrap_or("data").trim_matches('/').to_string();

    // param.sfo (the fields tools/ps4/build-pkg.sh sets)
    let app_ver = rt.json.get("app_ver").and_then(|v| v.as_str()).unwrap_or("01.00").to_string();
    let mut sfo = sfo::Sfo::default();
    sfo.set_int("APP_TYPE", rt.int("app_type", 1) as i32);
    sfo.set_str("APP_VER", &app_ver, 8);
    sfo.set_int("ATTRIBUTE", rt.int("attribute", 0) as i32);
    sfo.set_str("CATEGORY", "gd", 4);
    sfo.set_str("CONTENT_ID", &content_id, 48);
    sfo.set_int("DOWNLOAD_DATA_SIZE", rt.int("download_data_size", 64) as i32);
    sfo.set_int("SYSTEM_VER", rt.int("system_ver", 0) as i32);
    sfo.set_str("TITLE", &truncate(&job.branding.title, 127), 128);
    sfo.set_str("TITLE_ID", &title_id, 12);
    sfo.set_str("VERSION", &app_ver, 8);

    log("Making the artwork (icon0 512x512, pic0/pic1 1920x1080)...");
    let icon0 = art::png_rgb(&art::icon(&job.branding.art, 512));
    let pic = art::png_rgb(&art::backdrop(&job.branding.art, 1920, 1080));

    // the inner image: runtime files in their order, the staged sce_sys files, the data
    let time = runtime::now();
    let mut inner = pfs::Pfs::new(pfs::Props { time, min_blocks: 0x55, crypt: None });
    let mut files: Vec<(String, pfs::Content, u64)> = Vec::new();
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    for (path, bytes) in rt.ordered_files() {
        if path.starts_with(&format!("{data_dir}/")) || path == "sce_sys/param.sfo" {
            continue;
        }
        if let Some(name) = path.strip_prefix("sce_sys/").filter(|n| pkg::entry_id(n).is_some()) {
            entries.push((name.to_string(), bytes.to_vec()));
        }
        files.push((path.to_string(), pfs::Content::Bytes(bytes.to_vec()), bytes.len() as u64));
    }
    if !files.iter().any(|(p, ..)| p == "eboot.bin") {
        return Err("the PS4 runtime has no eboot.bin".into());
    }
    let sfo_bytes = sfo.to_bytes();
    files.insert(
        files.iter().position(|(p, ..)| p.starts_with("sce_sys/")).map_or(files.len(), |i| i + 1),
        ("sce_sys/param.sfo".into(), pfs::Content::Bytes(sfo_bytes.clone()), sfo_bytes.len() as u64),
    );
    for (name, bytes) in [("icon0.png", icon0), ("pic0.png", pic.clone()), ("pic1.png", pic)] {
        if entries.iter().any(|(n, _)| n == name) {
            continue; // the runtime brings its own
        }
        entries.push((name.to_string(), bytes.clone()));
        files.push((format!("sce_sys/{name}"), pfs::Content::Bytes(bytes.clone()), bytes.len() as u64));
    }
    let data = util::rendition_files(job.rendition).map_err(|e| format!("{}: {e}", job.rendition.display()))?;
    if !data.iter().any(|(p, _)| p.starts_with("game/")) {
        return Err(format!("{} has no game/ folder (not an imported rendition)", job.rendition.display()));
    }
    let mut data_bytes = 0u64;
    for (rel, full) in &data {
        if rel.bytes().any(|b| !b.is_ascii() || b.is_ascii_control()) {
            return Err(format!("{rel}: only ASCII file names can be packed"));
        }
        let len = std::fs::metadata(full).map_err(|e| format!("{}: {e}", full.display()))?.len();
        data_bytes += len;
        files.push((format!("{data_dir}/{rel}"), pfs::Content::Path(full.clone()), len));
    }
    log(&format!("Packing {} files ({} MB of game data) as {content_id}", files.len() + 1, data_bytes >> 20));
    // directories first, in sorted order (as the staged tree is listed), then the files
    let mut dirs: Vec<String> = files.iter().filter_map(|(p, ..)| p.rsplit_once('/').map(|(d, _)| d.to_string())).collect();
    dirs.push("sce_sys".into());
    let mut all_dirs: Vec<String> = Vec::new();
    for d in dirs {
        let mut acc = String::new();
        for part in d.split('/') {
            acc = if acc.is_empty() { part.to_string() } else { format!("{acc}/{part}") };
            all_dirs.push(acc.clone());
        }
    }
    all_dirs.sort();
    all_dirs.dedup();
    for d in &all_dirs {
        inner.dir(d);
    }
    for (p, c, len) in files {
        inner.add_file(&p, c, len);
    }
    inner.add_file("sce_sys/keystone", pfs::Content::Bytes(crypto::keystone(PASSCODE)), 96);
    inner.finish(&|name| pkg::entry_id(name).is_some());

    let (y, m, d, ..) = runtime::civil(time);
    let input = pkg::PkgInput {
        content_id: content_id.clone(),
        passcode: PASSCODE.into(),
        sfo,
        sce_sys_entries: entries,
        c_date: format!("{y:04}{m:02}{d:02}"),
        inner,
        time,
    };
    let out = job.out_dir.join(format!("{}.pkg", job.branding.file_stem()));
    let tmp = out.with_extension("pkg.part");
    let written = pkg::write(&tmp, input, log).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    std::fs::rename(&tmp, &out).map_err(|e| format!("{}: {e}", out.display()))?;
    log(&format!("PS4 package: {} ({} MB, {content_id}, version {app_ver})", out.display(), written.package_size >> 20));
    Ok(out)
}

/// `s` cut to at most `max` bytes on a character boundary.
fn truncate(s: &str, max: usize) -> String {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}
