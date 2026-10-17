//! Side-by-side versions under `<root>/versions/<tag>/`: downloading a release archive,
//! checking it against the release's `SHA256SUMS.txt`, unpacking it, listing and removing.
//!
//! Downloads use `curl` (as the exporter does). Nothing is trusted that does not match the
//! published checksum; `F2_RELEASES_URL` points the release list at a mirror.

use crate::releases::{self, Release};
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};

/// `frogger2.exe` on Windows, `frogger2` elsewhere.
pub fn exe_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

pub fn versions_dir(root: &Path) -> PathBuf {
    root.join("versions")
}

/// A release tag that is safe as a folder name.
pub fn valid_tag(tag: &str) -> bool {
    !tag.is_empty() && tag.len() <= 64 && !tag.starts_with('.') && tag.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
}

pub fn version_dir(root: &Path, tag: &str) -> PathBuf {
    versions_dir(root).join(tag)
}

/// The engine executable of an install folder (`engine/frogger2`, else `frogger2` beside it).
pub fn engine_exe(dir: &Path) -> Option<PathBuf> {
    let name = exe_name("frogger2");
    [dir.join("engine").join(&name), dir.join(&name)].into_iter().find(|p| p.is_file())
}

/// Installed version tags, newest first.
pub fn installed_tags(root: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(versions_dir(root)) else { return Vec::new() };
    let mut tags: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|t| valid_tag(t) && engine_exe(&version_dir(root, t)).is_some())
        .collect();
    tags.sort_by(|a, b| releases::parse_version(b).cmp(&releases::parse_version(a)).then(b.cmp(a)));
    tags
}

/// `curl` the text at `url` (the releases API wants a user agent).
pub fn fetch_text(url: &str) -> Result<String, String> {
    let out = std::process::Command::new("curl")
        .args(["-fsSL", "--retry", "2", "-H", "User-Agent: frogger2-rs-launcher", "-H", "Accept: application/vnd.github+json", url])
        .output()
        .map_err(|e| format!("running curl: {e}"))?;
    if !out.status.success() {
        return Err(format!("downloading {url} failed ({}): {}", out.status, String::from_utf8_lossy(&out.stderr).trim()));
    }
    String::from_utf8(out.stdout).map_err(|e| e.to_string())
}

/// `curl` `url` into `dest`.
pub fn fetch_file(url: &str, dest: &Path) -> Result<(), String> {
    let status = std::process::Command::new("curl")
        .args(["-fL", "--retry", "3", "-H", "User-Agent: frogger2-rs-launcher", "-o"])
        .arg(dest)
        .arg(url)
        .status()
        .map_err(|e| format!("running curl: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        let _ = std::fs::remove_file(dest);
        Err(format!("downloading {url} failed ({status})"))
    }
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Downloads and unpacks `release` for `os`/`arch` into `<root>/versions/<tag>/`. The archive
/// must match the release's `SHA256SUMS.txt`. `progress(step, fraction)` reports the stages.
pub fn install(root: &Path, release: &Release, os: &str, arch: &str, progress: &dyn Fn(&str, f32)) -> Result<PathBuf, String> {
    if !valid_tag(&release.tag) {
        return Err(format!("unsafe release tag {:?}", release.tag));
    }
    let asset = release.asset_for(os, arch).ok_or_else(|| format!("{} has no build for {os}-{arch}", release.tag))?;
    let sums = release.sums_asset().ok_or_else(|| format!("{} has no {}; refusing an unverified download", release.tag, releases::SUMS_NAME))?;
    let dl = versions_dir(root).join(".download");
    std::fs::create_dir_all(&dl).map_err(|e| format!("{}: {e}", dl.display()))?;
    progress("Checking the checksum list", 0.05);
    let sums = releases::parse_sha256sums(&fetch_text(&sums.url)?);
    let want = sums.get(&asset.name).ok_or_else(|| format!("{} is not in {}", asset.name, releases::SUMS_NAME))?.clone();
    let archive = dl.join(&asset.name);
    progress(&format!("Downloading {} ({} MB)", asset.name, asset.size >> 20), 0.1);
    fetch_file(&asset.url, &archive)?;
    progress("Verifying", 0.7);
    let got = sha256_file(&archive)?;
    if got != want {
        let _ = std::fs::remove_file(&archive);
        return Err(format!("{} has SHA-256 {got}, expected {want}", asset.name));
    }
    progress("Unpacking", 0.8);
    let dest = version_dir(root, &release.tag);
    let part = versions_dir(root).join(format!(".{}.part", release.tag));
    let _ = std::fs::remove_dir_all(&part);
    std::fs::create_dir_all(&part).map_err(|e| format!("{}: {e}", part.display()))?;
    extract(&archive, &part)?;
    if engine_exe(&part).is_none() {
        let _ = std::fs::remove_dir_all(&part);
        return Err(format!("{} does not contain the game ({})", asset.name, exe_name("frogger2")));
    }
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(&part, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let _ = std::fs::remove_file(&archive);
    progress("Done", 1.0);
    Ok(dest)
}

/// Deletes an installed version.
pub fn remove(root: &Path, tag: &str) -> Result<(), String> {
    if !valid_tag(tag) {
        return Err(format!("unsafe version name {tag:?}"));
    }
    let d = version_dir(root, tag);
    std::fs::remove_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))
}

/// Unpacks `archive` into the empty folder `dest`, dropping the single top-level folder the
/// release archives have.
pub fn extract(archive: &Path, dest: &Path) -> Result<(), String> {
    let name = archive.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name.ends_with(".tar.gz") {
        run("tar", std::process::Command::new("tar").arg("-xzf").arg(archive).arg("-C").arg(dest).arg("--strip-components=1"))
    } else if cfg!(target_os = "macos") {
        run("ditto", std::process::Command::new("ditto").arg("-x").arg("-k").arg(archive).arg(dest))?;
        strip_single_dir(dest)
    } else {
        extract_zip(archive, dest)
    }
}

fn run(what: &str, cmd: &mut std::process::Command) -> Result<(), String> {
    let out = cmd.output().map_err(|e| format!("running {what}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("{what} failed ({}): {}", out.status, String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// Moves the contents of `dir`'s only sub-folder up into `dir`.
pub fn strip_single_dir(dir: &Path) -> Result<(), String> {
    let entries: Vec<_> = std::fs::read_dir(dir).map_err(|e| e.to_string())?.filter_map(|e| e.ok()).collect();
    if entries.len() != 1 || !entries[0].path().is_dir() {
        return Ok(());
    }
    let inner = entries[0].path();
    for e in std::fs::read_dir(&inner).map_err(|e| e.to_string())?.filter_map(|e| e.ok()) {
        std::fs::rename(e.path(), dir.join(e.file_name())).map_err(|e| e.to_string())?;
    }
    std::fs::remove_dir(&inner).map_err(|e| e.to_string())
}

/// Unpacks a zip into `dest` without trusting its paths (no `..`, no absolute names), dropping
/// a single top-level folder.
pub fn extract_zip(archive: &Path, dest: &Path) -> Result<(), String> {
    let f = std::fs::File::open(archive).map_err(|e| format!("{}: {e}", archive.display()))?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| format!("{}: {e}", archive.display()))?;
    let mut names = Vec::new();
    for i in 0..z.len() {
        let e = z.by_index(i).map_err(|e| e.to_string())?;
        let p = e.enclosed_name().ok_or_else(|| format!("unsafe path in the archive: {:?}", e.name()))?;
        names.push(p);
    }
    let first = |p: &Path| p.components().find_map(|c| if let Component::Normal(n) = c { Some(n.to_os_string()) } else { None });
    let top = names.first().and_then(|p| first(p)).filter(|t| names.iter().all(|p| first(p).as_ref() == Some(t)));
    for (i, rel) in names.iter().enumerate() {
        let rel: PathBuf = match &top {
            Some(t) => rel.strip_prefix(t).unwrap_or(rel).to_path_buf(),
            None => rel.clone(),
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = dest.join(&rel);
        let mut e = z.by_index(i).map_err(|e| e.to_string())?;
        if e.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
            continue;
        }
        if let Some(d) = out.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        let mut w = std::fs::File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        std::io::copy(&mut e, &mut w).map_err(|e| format!("{}: {e}", out.display()))?;
        #[cfg(unix)]
        if let Some(mode) = e.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode & 0o777));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(name: &str) -> PathBuf {
        let t = std::env::temp_dir().join(format!("f2-launcher-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        std::fs::create_dir_all(&t).unwrap();
        t
    }

    fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (n, d) in entries {
            w.start_file(*n, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(d).unwrap();
        }
        w.finish().unwrap();
    }

    #[test]
    fn zip_extraction_drops_the_top_folder() {
        let t = tmp("zip");
        make_zip(&t.join("a.zip"), &[("top/engine/frogger2.exe", b"exe"), ("top/portable.txt", b"p")]);
        std::fs::create_dir_all(t.join("out")).unwrap();
        extract_zip(&t.join("a.zip"), &t.join("out")).unwrap();
        assert_eq!(std::fs::read(t.join("out/engine/frogger2.exe")).unwrap(), b"exe");
        assert!(t.join("out/portable.txt").is_file());
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn zip_extraction_rejects_escaping_paths() {
        let t = tmp("slip");
        make_zip(&t.join("bad.zip"), &[("top/ok.txt", b"x"), ("top/../../evil.txt", b"x")]);
        std::fs::create_dir_all(t.join("out")).unwrap();
        assert!(extract_zip(&t.join("bad.zip"), &t.join("out")).is_err());
        assert!(!t.join("evil.txt").exists());
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn versions_are_listed_newest_first_and_removed() {
        let t = tmp("ver");
        for tag in ["v0.1.0", "v0.10.0", "v0.2.0"] {
            let d = version_dir(&t, tag).join("engine");
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(exe_name("frogger2")), "x").unwrap();
        }
        std::fs::create_dir_all(version_dir(&t, "v9.9.9")).unwrap(); // no engine: not a version
        assert_eq!(installed_tags(&t), ["v0.10.0", "v0.2.0", "v0.1.0"]);
        remove(&t, "v0.2.0").unwrap();
        assert_eq!(installed_tags(&t), ["v0.10.0", "v0.1.0"]);
        assert!(remove(&t, "../x").is_err());
        assert!(valid_tag("v0.1.0-rc.1") && !valid_tag("") && !valid_tag(".hidden") && !valid_tag("a/b"));
        let _ = std::fs::remove_dir_all(&t);
    }
}
