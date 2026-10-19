//! `.f2mod` packages (`f2-modkit pack --out`, the launcher's "Import mod"): a zip of a mod
//! folder, `mod.toml` at the zip root or inside exactly one top-level folder (`pack` writes
//! `<id>/...`). Installing unpacks into the mods folder as `<mods>/<id>/` after the same
//! checks `pack` makes without a rendition; a mod it replaces is kept in `<mods>/.backup/<id>/`.
//!
//! Reading trusts nothing: absolute names, `..`, `\`, `:`, symlinks and oversized contents
//! are refused before anything is written.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use f2_mods::ModManifest;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

use crate::Error;

/// File extension of a mod package.
pub const EXT: &str = "f2mod";
/// Where replaced and removed mods are kept (one copy per id), inside the mods folder.
pub const BACKUP_DIR: &str = ".backup";
const MAX_ENTRIES: usize = 100_000;
/// Per file and in total (uncompressed): music and models make big mods, not this big.
const MAX_FILE: u64 = 1 << 30;
const MAX_TOTAL: u64 = 4 << 30;

fn io(p: &Path) -> impl Fn(std::io::Error) -> Error + '_ {
    move |e| Error::new(format!("{}: {e}", p.display()))
}

/// Files of `dir` relative to it with `/` separators, sorted; dot-files and dot-folders are
/// left out, symlinks refused.
fn mod_files(dir: &Path) -> Result<Vec<(String, PathBuf)>, Error> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<(), Error> {
        let mut v: Vec<fs::DirEntry> = fs::read_dir(dir).map_err(io(dir))?.flatten().collect();
        v.sort_by_key(|e| e.file_name());
        for e in v {
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let p = e.path();
            let ft = e.file_type().map_err(io(&p))?;
            if ft.is_symlink() {
                return Err(Error::new(format!("{}: symbolic links are not allowed in a mod", p.display())));
            }
            if ft.is_dir() {
                walk(root, &p, out)?;
            } else {
                let rel = p.strip_prefix(root).unwrap_or(&p).components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
                out.push((rel, p));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out)?;
    Ok(out)
}

/// Writes the mod in `mod_dir` to the package `out` as `<id>/...` (sorted, deflated, fixed
/// timestamps: the same folder always makes the same bytes). Returns the number of files.
pub fn write(mod_dir: &Path, out: &Path) -> Result<usize, Error> {
    let m = ModManifest::load(&mod_dir.join("mod.toml")).map_err(|e| Error::new(e.to_string()))?;
    let out_abs = fs::canonicalize(out.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new(".")))
        .ok()
        .map(|d| d.join(out.file_name().unwrap_or_default()));
    let files: Vec<_> = mod_files(mod_dir)?.into_iter().filter(|(_, p)| fs::canonicalize(p).ok() != out_abs).collect();
    let tmp = out.with_extension("f2mod.part");
    let res = (|| {
        let mut z = ZipWriter::new(std::io::BufWriter::new(fs::File::create(&tmp).map_err(io(&tmp))?));
        let zerr = |e: zip::result::ZipError| Error::new(format!("{}: {e}", tmp.display()));
        for (rel, p) in &files {
            let data = fs::read(p).map_err(io(p))?;
            let opts = SimpleFileOptions::default()
                .compression_method(CompressionMethod::Deflated)
                .last_modified_time(DateTime::default())
                .unix_permissions(0o644)
                .large_file(data.len() as u64 >= u32::MAX as u64);
            z.start_file(format!("{}/{rel}", m.id), opts).map_err(zerr)?;
            z.write_all(&data).map_err(io(&tmp))?;
        }
        z.finish().map_err(zerr)?.flush().map_err(io(&tmp))?;
        fs::rename(&tmp, out).map_err(io(out))
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res.map(|_| files.len())
}

/// A zip entry name as path parts, or `None` when it could leave the folder it is unpacked
/// into (absolute, `..`, drive letters, backslashes).
fn safe_parts(name: &str) -> Option<Vec<&str>> {
    if name.starts_with('/') || name.contains('\\') || name.contains(':') || name.contains('\0') {
        return None;
    }
    let parts: Vec<&str> = name.split('/').filter(|s| !s.is_empty() && *s != ".").collect();
    (!parts.contains(&"..")).then_some(parts)
}

/// Unpacks the package `archive` into `dest` (created; should not exist yet), dropping the
/// top-level folder when `mod.toml` is inside one.
pub fn unpack(archive: &Path, dest: &Path) -> Result<(), Error> {
    let f = fs::File::open(archive).map_err(io(archive))?;
    let bad = |m: String| Error::new(format!("{}: {m}", archive.display()));
    let mut z = ZipArchive::new(std::io::BufReader::new(f)).map_err(|e| bad(format!("not a mod package ({e})")))?;
    if z.len() > MAX_ENTRIES {
        return Err(bad(format!("{} entries (at most {MAX_ENTRIES})", z.len())));
    }
    // (index, parts, is dir) of every entry, checked before anything is written.
    let mut entries = Vec::with_capacity(z.len());
    let mut total = 0u64;
    for i in 0..z.len() {
        let e = z.by_index(i).map_err(|e| bad(e.to_string()))?;
        let name = e.name().to_string();
        let parts = safe_parts(&name).filter(|_| e.enclosed_name().is_some()).ok_or_else(|| bad(format!("unsafe path {name:?}")))?;
        if e.is_symlink() || e.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            return Err(bad(format!("{name}: symbolic links are not allowed in a mod")));
        }
        if e.size() > MAX_FILE {
            return Err(bad(format!("{name}: {} bytes (at most {MAX_FILE})", e.size())));
        }
        total += e.size();
        if parts.is_empty() {
            continue;
        }
        entries.push((i, parts.into_iter().map(str::to_string).collect::<Vec<_>>(), e.is_dir()));
    }
    if total > MAX_TOTAL {
        return Err(bad(format!("{total} bytes unpacked (at most {MAX_TOTAL})")));
    }
    let is_manifest = |p: &[String], d: bool| !d && p.len() == 1 && p[0] == "mod.toml";
    let strip = if entries.iter().any(|(_, p, d)| is_manifest(p, *d)) {
        0
    } else {
        let top = entries.first().map(|(_, p, _)| p[0].clone());
        let one_top = top.as_ref().is_some_and(|t| entries.iter().all(|(_, p, _)| &p[0] == t));
        if !one_top || !entries.iter().any(|(_, p, d)| is_manifest(&p[1..], *d)) {
            return Err(bad("no mod.toml at the top of the package (or inside its one top-level folder)".into()));
        }
        1
    };
    fs::create_dir_all(dest).map_err(io(dest))?;
    let mut written = 0u64;
    for (i, parts, dir) in entries {
        let out = parts[strip..].iter().fold(dest.to_path_buf(), |p, s| p.join(s));
        if dir {
            fs::create_dir_all(&out).map_err(io(&out))?;
            continue;
        }
        if let Some(d) = out.parent() {
            fs::create_dir_all(d).map_err(io(d))?;
        }
        let e = z.by_index(i).map_err(|e| bad(e.to_string()))?;
        let name = e.name().to_string();
        let mut w = fs::File::create(&out).map_err(io(&out))?;
        // Sizes in the headers can lie: count what really comes out.
        let n = std::io::copy(&mut e.take(MAX_FILE + 1), &mut w).map_err(|e| bad(format!("{name}: {e}")))?;
        written += n;
        if n > MAX_FILE || written > MAX_TOTAL {
            return Err(bad(format!("{name}: unpacks to more than it says")));
        }
    }
    Ok(())
}

/// Copies the mod folder `src` to `dest` (dot-files left out, symlinks refused).
fn copy_mod(src: &Path, dest: &Path) -> Result<(), Error> {
    for (rel, p) in mod_files(src)? {
        let out = rel.split('/').fold(dest.to_path_buf(), |d, s| d.join(s));
        if let Some(d) = out.parent() {
            fs::create_dir_all(d).map_err(io(d))?;
        }
        fs::copy(&p, &out).map_err(io(&p))?;
    }
    Ok(())
}

/// Result of [`install`].
#[derive(Debug)]
pub struct Installed {
    pub manifest: ModManifest,
    /// `<mods>/<id>`.
    pub dir: PathBuf,
    /// The version of the mod it replaced (`Some("")` when that one had none or was unreadable).
    pub replaced: Option<String>,
    /// Where the replaced mod was kept.
    pub backup: Option<PathBuf>,
    /// `pack`'s warnings.
    pub warnings: Vec<String>,
}

/// The installed mod folders in `mods_dir` whose `mod.toml` has `id` (any folder name).
fn dirs_with_id(mods_dir: &Path, id: &str) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(mods_dir) else { return Vec::new() };
    let mut v: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
        .filter(|p| p.is_dir() && ModManifest::load(&p.join("mod.toml")).is_ok_and(|m| m.id == id))
        .collect();
    v.sort();
    v
}

/// Moves the mod folder `dir` to `<mods>/.backup/<id>` (replacing an older backup).
fn back_up(mods_dir: &Path, dir: &Path, id: &str) -> Result<PathBuf, Error> {
    let b = mods_dir.join(BACKUP_DIR).join(id);
    if b.exists() {
        fs::remove_dir_all(&b).map_err(io(&b))?;
    }
    fs::create_dir_all(mods_dir.join(BACKUP_DIR)).map_err(io(mods_dir))?;
    fs::rename(dir, &b).map_err(io(dir))?;
    Ok(b)
}

/// Installs a mod package (`.f2mod` or `.zip`) or a mod folder into `mods_dir` as
/// `<mods_dir>/<id>/`: refused when the manifest is bad, the engine is not one the mod
/// accepts, or `pack` finds errors. An installed mod with the same id is replaced (kept in
/// `.backup/<id>`). Turning it on is up to the caller (`[mods] enabled`).
pub fn install(src: &Path, mods_dir: &Path) -> Result<Installed, Error> {
    fs::create_dir_all(mods_dir).map_err(io(mods_dir))?;
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    // Unpacked next to its destination (one rename to install), in a dot-folder the game skips.
    let staging = mods_dir.join(format!(".import-{}-{nonce}", std::process::id()));
    let r = install_via(src, mods_dir, &staging);
    let _ = fs::remove_dir_all(&staging);
    r
}

fn install_via(src: &Path, mods_dir: &Path, staging: &Path) -> Result<Installed, Error> {
    let unpacked = staging.join("unpacked");
    if src.is_dir() {
        if !src.join("mod.toml").is_file() {
            return Err(Error::new(format!("{}: no mod.toml in this folder", src.display())));
        }
        copy_mod(src, &unpacked)?;
    } else {
        unpack(src, &unpacked)?;
    }
    let m = ModManifest::load(&unpacked.join("mod.toml"))
        .map_err(|e| Error::new(e.to_string().replace(&unpacked.display().to_string(), &src.display().to_string())))?;
    m.check_compatible().map_err(|e| Error::new(e.to_string()))?;
    // Validated under its own name, as it will be installed.
    let staged = staging.join(&m.id);
    fs::rename(&unpacked, &staged).map_err(io(&unpacked))?;
    let rep = crate::pack::validate(&staged, None);
    if !rep.ok() {
        let more = if rep.errors.len() > 3 { format!(" (and {} more)", rep.errors.len() - 3) } else { String::new() };
        return Err(Error::new(format!("{} has errors: {}{more}", m.id, rep.errors.iter().take(3).cloned().collect::<Vec<_>>().join("; "))));
    }
    let dir = mods_dir.join(&m.id);
    let mut old = dirs_with_id(mods_dir, &m.id);
    if dir.exists() && !old.contains(&dir) {
        old.insert(0, dir.clone());
    }
    let (mut replaced, mut backup) = (None, None);
    for o in &old {
        let v = ModManifest::load(&o.join("mod.toml")).map(|m| m.version).unwrap_or_default();
        replaced.get_or_insert(v);
        backup = Some(back_up(mods_dir, o, &m.id)?);
    }
    fs::rename(&staged, &dir).map_err(io(&dir))?;
    Ok(Installed { manifest: m, dir, replaced, backup, warnings: rep.warnings })
}

/// Removes the installed mod `id` from `mods_dir` (kept in `.backup/<id>`); the backup's path.
pub fn remove(mods_dir: &Path, id: &str) -> Result<PathBuf, Error> {
    if id.is_empty() || id.starts_with('.') || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(Error::new(format!("invalid mod id {id:?}")));
    }
    let mut dirs = dirs_with_id(mods_dir, id);
    let named = mods_dir.join(id);
    if dirs.is_empty() && named.join("mod.toml").is_file() {
        dirs.push(named);
    }
    let mut backup = None;
    for d in dirs {
        backup = Some(back_up(mods_dir, &d, id)?);
    }
    backup.ok_or_else(|| Error::new(format!("mod {id} is not installed in {}", mods_dir.display())))
}

/// Whether `path` looks like something [`install`] takes: a `.f2mod`/`.zip` file or a folder
/// with a `mod.toml` (a `mod.toml` itself stands for its folder, see [`source_of`]).
pub fn is_mod_source(path: &Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
    (path.is_file() && matches!(ext.as_deref(), Some(EXT) | Some("zip"))) || source_of(path).join("mod.toml").is_file()
}

/// A picked `mod.toml` means its folder (file dialogs pick files).
pub fn source_of(path: &Path) -> PathBuf {
    match (path.file_name(), path.parent()) {
        (Some(n), Some(d)) if n.eq_ignore_ascii_case("mod.toml") => d.to_path_buf(),
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(root: &Path, rel: &str, data: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, data).unwrap();
    }

    /// A mod with a manifest, a text override and a new texture (nothing retail).
    fn sample(dir: &Path, id: &str, version: &str) -> PathBuf {
        let d = dir.join(id);
        put(&d, "mod.toml", crate::pack::manifest_template(id).replace("0.1.0", version).as_bytes());
        put(&d, "game/text/uk.toml", b"[strings]\nSTR_HELLO = \"Hi\"\n");
        let mut png = Vec::new();
        let mut e = png::Encoder::new(&mut png, 1, 1);
        e.set_color(png::ColorType::Rgba);
        e.write_header().unwrap().write_image_data(&[255, 0, 0, 255]).unwrap();
        put(&d, "game/textures/garden/newtex.png", &png);
        put(&d, ".git/HEAD", b"ref");
        d
    }

    fn zip_of(path: &Path, files: &[(&str, &[u8])]) {
        let mut z = ZipWriter::new(fs::File::create(path).unwrap());
        for (n, d) in files {
            z.start_file(*n, SimpleFileOptions::default()).unwrap();
            z.write_all(d).unwrap();
        }
        z.finish().unwrap();
    }

    fn manifest(id: &str) -> Vec<u8> {
        crate::pack::manifest_template(id).into_bytes()
    }

    #[test]
    fn pack_and_install_round_trip() {
        let t = tempfile::tempdir().unwrap();
        let src = sample(&t.path().join("src"), "round-trip", "1.0.0");
        let pkg = t.path().join("round-trip.f2mod");
        assert_eq!(write(&src, &pkg).unwrap(), 3, "dot-files left out");
        let bytes = fs::read(&pkg).unwrap();
        write(&src, &pkg).unwrap();
        assert_eq!(fs::read(&pkg).unwrap(), bytes, "deterministic");
        let names: Vec<String> = ZipArchive::new(fs::File::open(&pkg).unwrap()).unwrap().file_names().map(str::to_string).collect();
        assert!(names.contains(&"round-trip/mod.toml".to_string()), "{names:?}");

        let mods = t.path().join("mods");
        let i = install(&pkg, &mods).unwrap();
        assert_eq!((i.manifest.id.as_str(), i.replaced.clone(), i.dir.clone()), ("round-trip", None, mods.join("round-trip")));
        for f in ["mod.toml", "game/text/uk.toml", "game/textures/garden/newtex.png"] {
            assert_eq!(fs::read(mods.join("round-trip").join(f)).unwrap(), fs::read(src.join(f)).unwrap(), "{f}");
        }
        assert!(!mods.join("round-trip/.git").exists());
        assert_eq!(fs::read_dir(&mods).unwrap().count(), 1, "no staging left behind");
        assert_eq!(f2_mods::scan_mods(&mods).unwrap().len(), 1);

        // Again, newer, from a folder: the old one goes to the backup.
        let src2 = sample(&t.path().join("src2"), "round-trip", "1.1.0");
        let i = install(&src2, &mods).unwrap();
        assert_eq!(i.replaced.as_deref(), Some("1.0.0"));
        assert_eq!(i.backup.as_deref(), Some(mods.join(".backup/round-trip").as_path()));
        assert!(fs::read_to_string(mods.join("round-trip/mod.toml")).unwrap().contains("1.1.0"));
        assert!(fs::read_to_string(mods.join(".backup/round-trip/mod.toml")).unwrap().contains("1.0.0"));
        assert_eq!(f2_mods::scan_mods(&mods).unwrap().len(), 1, "the backup is not a mod");

        assert_eq!(remove(&mods, "round-trip").unwrap(), mods.join(".backup/round-trip"));
        assert!(!mods.join("round-trip").exists());
        assert!(remove(&mods, "round-trip").is_err());
        assert!(remove(&mods, "../x").is_err());
    }

    #[test]
    fn installs_root_layout_and_replaces_by_id() {
        let t = tempfile::tempdir().unwrap();
        let mods = t.path().join("mods");
        // Installed by hand under another folder name.
        put(&mods.join("by-hand"), "mod.toml", &manifest("flat"));
        let pkg = t.path().join("flat.zip");
        zip_of(&pkg, &[("mod.toml", &manifest("flat")), ("game/text/uk.toml", b"[strings]\nSTR_A = \"a\"\n")]);
        let i = install(&pkg, &mods).unwrap();
        assert_eq!(i.replaced.as_deref(), Some("0.1.0"));
        assert!(mods.join("flat/game/text/uk.toml").is_file() && !mods.join("by-hand").exists());
        assert!(is_mod_source(&pkg) && is_mod_source(&mods.join("flat")) && is_mod_source(&mods.join("flat/mod.toml")));
        assert_eq!(source_of(&mods.join("flat/MOD.TOML")), mods.join("flat"));
        assert!(!is_mod_source(t.path()));
    }

    #[test]
    fn refuses_unsafe_and_broken_packages() {
        let t = tempfile::tempdir().unwrap();
        let mods = t.path().join("mods");
        let refused = |files: &[(&str, &[u8])], why: &str| {
            let pkg = t.path().join("bad.f2mod");
            zip_of(&pkg, files);
            let e = install(&pkg, &mods).unwrap_err().0;
            assert!(e.contains(why), "{e}");
        };
        let m = manifest("evil");
        refused(&[("mod.toml", &m), ("../escape.txt", b"x")], "unsafe path");
        refused(&[("mod.toml", &m), ("/abs.txt", b"x")], "unsafe path");
        refused(&[("mod.toml", &m), ("game\\..\\..\\x", b"x")], "unsafe path");
        refused(&[("mod.toml", &m), ("C:/x", b"x")], "unsafe path");
        refused(&[("a/mod.toml", &m), ("b/x.txt", b"x")], "no mod.toml");
        refused(&[("a/b/mod.toml", &m)], "no mod.toml");
        refused(&[("mod.toml", b"id = \"../x\"\nname = \"x\"\nversion = \"1\"\nengine_version = \"*\"\n")], "invalid mod id");
        refused(&[("mod.toml", &m), ("music/song.wav", b"x")], "has errors");
        refused(&[("mod.toml", b"id = \"old\"\nname = \"old\"\nversion = \"1.0.0\"\nengine_version = \">=99\"\n")], "old");
        // A symlink entry.
        let pkg = t.path().join("link.f2mod");
        let mut z = ZipWriter::new(fs::File::create(&pkg).unwrap());
        z.start_file("mod.toml", SimpleFileOptions::default()).unwrap();
        z.write_all(&m).unwrap();
        z.add_symlink("game/x", "/etc/passwd", SimpleFileOptions::default()).unwrap();
        z.finish().unwrap();
        assert!(install(&pkg, &mods).unwrap_err().0.contains("symbolic links"));
        assert!(!t.path().join("escape.txt").exists());
        assert!(fs::read_dir(&mods).map_or(0, |d| d.count()) == 0, "nothing installed, no staging left");
        assert!(install(&t.path().join("missing.f2mod"), &mods).is_err());
        fs::write(t.path().join("junk.f2mod"), b"not a zip").unwrap();
        assert!(install(&t.path().join("junk.f2mod"), &mods).unwrap_err().0.contains("not a mod package"));
    }
}
