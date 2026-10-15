//! Export for Android (docs/EXPORT.md "Android"): the shell APK (the data-free app built by
//! `tools/android/build-apk.sh`, release asset `swampys-revenge-recomp-<v>-android.apk`) +
//! the player's game -> a ready-to-run, signed APK.
//!
//! What changes relative to the shell:
//! - `AndroidManifest.xml` (binary XML, string pool rewritten): a distinct package id
//!   ([`PACKAGE`], so the export installs next to the shell app), the activity class fully
//!   qualified (the dex is unchanged), launcher label = the short title, `versionName` = the
//!   app version, `versionCode` derived from it.
//! - The launcher icon PNGs (same zip paths, so the resource ids in `resources.arsc` stay
//!   valid): adaptive background = the game artwork (inset like the in-app icon, the border
//!   colour around it), adaptive foreground transparent, legacy `ic_launcher.png` = the
//!   rounded icon; every density at the shell's pixel sizes.
//! - The game data as stored, 4-byte aligned entries `assets/game-data/<rendition>/...` plus
//!   the index `assets/game-data/bundle.json` (read by `crates/frogger-android/src/bundle.rs`).
//! - The shell's v1 signature files are dropped; the APK is zip-aligned (stored `.so` on
//!   16 KiB pages) and signed with APK Signature Scheme v2 using the per-machine export key.
//!
//! `resources.arsc` is copied unchanged: its resource package keeps the shell's name, as
//! with `aapt2 --rename-manifest-package` (resources are resolved by id, 0x7f...).

mod axml;
mod sign;
mod zip;

use crate::util;
use crate::{Image, Job};
use std::io::BufWriter;
use std::path::{Path, PathBuf};

/// The shell app's package (the Java activity's package too).
#[cfg(test)]
const SHELL_PACKAGE: &str = "io.github.neilgraham.swampysrevengerecomp";
/// Package id of exported APKs.
pub const PACKAGE: &str = "io.github.neilgraham.swampysrevengerecomp.game";
/// Asset folder of the bundled game (inside the APK: `assets/` + this).
pub const BUNDLE_DIR: &str = "game-data";
/// The bundle index (`BUNDLE_DIR/bundle.json`): `{"format": 1, "id": "<sha256>",
/// "rendition": "usa", "bytes": N, "files": [["game/...", size], ...]}`.
pub const BUNDLE_INDEX: &str = "game-data/bundle.json";

pub(crate) fn export(job: &Job) -> Result<PathBuf, String> {
    let shell_path = shell_apk(job.runtime)?;
    (job.log)(&format!("Android shell: {}", shell_path.display()));
    let bytes = std::fs::read(&shell_path).map_err(|e| format!("{}: {e}", shell_path.display()))?;
    let shell = zip::Archive::parse(bytes).map_err(|e| format!("{}: {e}", shell_path.display()))?;
    let manifest_entry = shell.find("AndroidManifest.xml").ok_or("the runtime is not an APK (no AndroidManifest.xml)")?;
    let manifest = patch_manifest(&shell.read(manifest_entry)?, &job.branding.short_title, &job.branding.version)?;

    let rendition_name = job.rendition.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or("the rendition has no folder name")?;
    let files = util::rendition_files(job.rendition).map_err(|e| format!("{}: {e}", job.rendition.display()))?;
    if !files.iter().any(|(rel, _)| rel.starts_with("game/")) {
        return Err(format!("{} has no game/ folder", job.rendition.display()));
    }
    let mut sized = Vec::with_capacity(files.len());
    for (rel, path) in files {
        let len = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?.len();
        sized.push((rel, path, len));
    }
    let manifest_json = std::fs::read(job.rendition.join("manifest.json")).unwrap_or_default();
    let index = bundle_index(&rendition_name, &sized, &manifest_json);

    let key = match job.keys_dir {
        Some(dir) => sign::Key::load_or_create(dir, job.log)?,
        None => {
            (job.log)("No key directory: signing with a one-off key (a later export will not install as an update)");
            sign::Key::generate()
        }
    };

    let out = job.out_dir.join(format!("{}.apk", util::file_stem(&job.branding.short_title)));
    let part = out.with_extension("apk.part");
    let result = write_apk(job, &shell, &manifest, &rendition_name, &sized, &index, &key, &part);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    std::fs::rename(&part, &out).map_err(|e| format!("{}: {e}", out.display()))?;
    (job.log)(&format!("Wrote {} (package {PACKAGE})", out.display()));
    Ok(out)
}

/// The shell APK: the runtime itself, or the one `.apk` in a runtime folder.
fn shell_apk(runtime: &Path) -> Result<PathBuf, String> {
    if runtime.is_file() {
        return Ok(runtime.to_path_buf());
    }
    let apks: Vec<PathBuf> = std::fs::read_dir(runtime)
        .map_err(|e| format!("{}: {e}", runtime.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("apk")))
        .collect();
    match apks.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(format!("no .apk in {}", runtime.display())),
        _ => Err(format!("several .apk files in {}: pass one with --runtime", runtime.display())),
    }
}

/// "0.1.0" -> 1000 (major * 1_000_000 + minor * 1000 + patch, at least 1).
fn version_code(version: &str) -> u32 {
    let mut parts = version.split(['.', '-', '+']).map(|p| p.parse::<u32>().unwrap_or(0));
    let (a, b, c) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    (a.min(2000) * 1_000_000 + b.min(999) * 1000 + c.min(999)).max(1)
}

/// The shell's manifest with the export's package id, label and version.
fn patch_manifest(bytes: &[u8], label: &str, version: &str) -> Result<Vec<u8>, String> {
    let mut m = axml::Axml::parse(bytes).map_err(|e| format!("AndroidManifest.xml: {e}"))?;
    let shell_package = m.get_string("manifest", "package")?;
    // Relative class names resolve against the manifest package, which changes: qualify it.
    let activity = m.get_string("activity", "name")?;
    let activity = if activity.starts_with('.') {
        format!("{shell_package}{activity}")
    } else if !activity.contains('.') {
        format!("{shell_package}.{activity}")
    } else {
        activity
    };
    m.set_string("activity", "name", &activity)?;
    m.set_string("manifest", "package", PACKAGE)?;
    m.set_string("application", "label", label)?;
    m.set_string("manifest", "versionName", version)?;
    m.set_int("manifest", "versionCode", version_code(version))?;
    Ok(m.encode())
}

/// The bundle index (see [`BUNDLE_INDEX`]); its id changes whenever a file, size or the
/// import manifest does, which makes the app unpack the data again.
fn bundle_index(rendition: &str, files: &[(String, PathBuf, u64)], manifest_json: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(rendition.as_bytes());
    for (rel, _, len) in files {
        h.update(rel.as_bytes());
        h.update([0]);
        h.update(len.to_le_bytes());
    }
    h.update(manifest_json);
    let id: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    let list: Vec<serde_json::Value> = files.iter().map(|(rel, _, len)| serde_json::json!([rel, len])).collect();
    let total: u64 = files.iter().map(|f| f.2).sum();
    serde_json::to_string_pretty(&serde_json::json!({ "format": 1, "id": id, "rendition": rendition, "bytes": total, "files": list })).unwrap()
}

/// Launcher icon PNG replacements for the shell's `res/mipmap-*/ic_launcher*.png`, at the
/// shell's pixel sizes.
fn icon_png(name: &str, shell_png: &[u8], branding: &crate::Branding) -> Option<Vec<u8>> {
    let file = name.rsplit('/').next()?;
    if !name.starts_with("res/mipmap") {
        return None;
    }
    let (w, h) = png_size(shell_png)?;
    let img = match file {
        "ic_launcher.png" => util::resize(&branding.icon, w, h),
        "ic_launcher_background.png" => adaptive_background(&branding.art, w, h),
        "ic_launcher_foreground.png" => Image { width: w, height: h, rgba: vec![0; (w * h * 4) as usize] },
        _ => return None,
    };
    Some(util::png(&img))
}

fn png_size(png_bytes: &[u8]) -> Option<(u32, u32)> {
    let dec = png::Decoder::new(std::io::Cursor::new(png_bytes));
    let r = dec.read_info().ok()?;
    let i = r.info();
    Some((i.width, i.height))
}

/// The adaptive icon's background layer (108 dp): the artwork at 80 dp in the middle (the
/// launcher's mask shows the inner 72 dp, so circles and squircles cut only its edges) on
/// the average colour of the artwork's border (seen while the icon moves). Matches the
/// in-app icon (`frogger::icon::adaptive`).
fn adaptive_background(art: &Image, w: u32, h: u32) -> Image {
    let (iw, ih) = (w * 80 / 108, h * 80 / 108);
    let (ox, oy) = ((w - iw) / 2, (h - ih) / 2);
    let small = util::resize(art, iw, ih);
    let mut acc = [0u64; 3];
    let mut n = 0u64;
    for y in 0..art.height {
        for x in 0..art.width {
            if x == 0 || y == 0 || x == art.width - 1 || y == art.height - 1 {
                let i = ((y * art.width + x) * 4) as usize;
                for (a, &p) in acc.iter_mut().zip(&art.rgba[i..i + 3]) {
                    *a += p as u64;
                }
                n += 1;
            }
        }
    }
    let edge = acc.map(|a| (a / n.max(1)) as u8);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            if (ox..ox + iw).contains(&x) && (oy..oy + ih).contains(&y) {
                let i = (((y - oy) * iw + (x - ox)) * 4) as usize;
                rgba.extend_from_slice(&[small.rgba[i], small.rgba[i + 1], small.rgba[i + 2], 255]);
            } else {
                rgba.extend_from_slice(&[edge[0], edge[1], edge[2], 255]);
            }
        }
    }
    Image { width: w, height: h, rgba }
}

/// v1 (JAR) signature files of the shell, dropped (the export is v2-signed).
fn is_v1_signature(name: &str) -> bool {
    let Some(f) = name.strip_prefix("META-INF/") else { return false };
    if f.contains('/') {
        return false;
    }
    let up = f.to_ascii_uppercase();
    up == "MANIFEST.MF" || [".SF", ".RSA", ".DSA", ".EC"].iter().any(|e| up.ends_with(e)) || up.starts_with("SIG-")
}

#[allow(clippy::too_many_arguments)]
fn write_apk(
    job: &Job,
    shell: &zip::Archive,
    manifest: &[u8],
    rendition: &str,
    files: &[(String, PathBuf, u64)],
    index: &str,
    key: &sign::Key,
    out: &Path,
) -> Result<(), String> {
    let io = |e: std::io::Error| format!("{}: {e}", out.display());
    let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(out).map_err(io)?;
    let mut w = zip::Writer::new(BufWriter::with_capacity(1 << 20, file));
    w.add_stored("AndroidManifest.xml", manifest).map_err(io)?;
    let mut icons = 0;
    for e in &shell.entries {
        if e.name == "AndroidManifest.xml" || is_v1_signature(&e.name) || e.name.starts_with(&format!("assets/{BUNDLE_DIR}/")) {
            continue;
        }
        if e.name.ends_with(".png") {
            if let Some(png) = shell.read(e).ok().and_then(|b| icon_png(&e.name, &b, job.branding)) {
                w.add_stored(&e.name, &png).map_err(io)?;
                icons += 1;
                continue;
            }
        }
        w.add_raw(&e.name, e.method, e.time, e.date, e.crc, e.usize, shell.raw(e)).map_err(io)?;
    }
    if icons == 0 {
        return Err("the shell APK has no launcher icon PNGs (res/mipmap-*/ic_launcher*.png)".into());
    }
    let total: u64 = files.iter().map(|f| f.2).sum();
    (job.log)(&format!("Packing {} files ({} MB) of the game into the APK", files.len(), total >> 20));
    let mut done = 0u64;
    let mut next_log = 64 << 20;
    for (rel, path, len) in files {
        let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        w.add_stored_stream(&format!("assets/{BUNDLE_DIR}/{rendition}/{rel}"), &mut f, *len).map_err(|e| format!("{}: {e}", path.display()))?;
        done += len;
        if done >= next_log {
            (job.log)(&format!("  {} of {} MB", done >> 20, total >> 20));
            next_log = done + (64 << 20);
        }
    }
    w.add_stored(&format!("assets/{BUNDLE_INDEX}"), index.as_bytes()).map_err(io)?;
    let (buf, end, cd, eocd) = w.finish().map_err(io)?;
    let mut file = buf.into_inner().map_err(|e| io(e.into_error()))?;
    (job.log)("Signing (APK Signature Scheme v2)");
    sign::sign_v2(&mut file, end, &cd, &eocd, key).map_err(io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_codes() {
        assert_eq!(version_code("0.1.0"), 1000);
        assert_eq!(version_code("1.2.3"), 1_002_003);
        assert_eq!(version_code("0.0.0"), 1);
        assert_eq!(version_code("2.0.1-beta"), 2_000_001);
    }

    #[test]
    fn v1_files() {
        assert!(is_v1_signature("META-INF/ANDROIDD.SF"));
        assert!(is_v1_signature("META-INF/CERT.RSA"));
        assert!(is_v1_signature("META-INF/MANIFEST.MF"));
        assert!(!is_v1_signature("META-INF/services/x"));
        assert!(!is_v1_signature("res/x.RSA"));
    }

    fn branding() -> crate::Branding {
        let art = Image { width: 4, height: 4, rgba: (0..16).flat_map(|i| [i as u8 * 10, 50, 100, 255]).collect() };
        crate::Branding { title: "Frogger 2: Swampy's Revenge".into(), short_title: "Frogger 2".into(), icon: art.clone(), art, version: "0.1.0".into() }
    }

    fn png_of(w: u32, h: u32) -> Vec<u8> {
        util::png(&Image { width: w, height: h, rgba: vec![255; (w * h * 4) as usize] })
    }

    /// A fake shell: tiny manifest, icons, a library, a v1 signature file; exported with
    /// a fake rendition and checked like apksigner would.
    #[test]
    fn exports_a_signed_apk() {
        let tmp = std::env::temp_dir().join(format!("f2-android-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let rend = tmp.join("data/usa");
        std::fs::create_dir_all(rend.join("game/textures")).unwrap();
        std::fs::create_dir_all(rend.join("music")).unwrap();
        std::fs::create_dir_all(rend.join("oracle")).unwrap();
        std::fs::write(rend.join("manifest.json"), b"{\"rendition\":\"usa\"}").unwrap();
        std::fs::write(rend.join("game/textures/a.fla"), vec![1u8; 3000]).unwrap();
        std::fs::write(rend.join("music/track02.wav"), vec![2u8; 5000]).unwrap();
        std::fs::write(rend.join("oracle/frogger2.exe"), b"MZ").unwrap();

        // The shell: the test manifest with an activity element.
        let mut shell = zip::Writer::new(std::io::Cursor::new(Vec::new()));
        shell.add_stored("AndroidManifest.xml", &shell_manifest()).unwrap();
        shell.add_stored("res/mipmap-mdpi-v4/ic_launcher.png", &png_of(48, 48)).unwrap();
        shell.add_stored("res/mipmap-mdpi-v4/ic_launcher_background.png", &png_of(108, 108)).unwrap();
        shell.add_stored("res/mipmap-mdpi-v4/ic_launcher_foreground.png", &png_of(108, 108)).unwrap();
        shell.add_stored("resources.arsc", &[0u8; 64]).unwrap();
        shell.add_stored("lib/x86_64/libfrogger_android.so", &[0x7f; 1000]).unwrap();
        shell.add_stored("META-INF/CERT.RSA", b"sig").unwrap();
        let (c, _, cd, eocd) = shell.finish().unwrap();
        let shell_bytes = [c.into_inner(), cd, eocd].concat();
        let shell_path = tmp.join("shell.apk");
        std::fs::write(&shell_path, shell_bytes).unwrap();

        let b = branding();
        let out_dir = tmp.join("out");
        std::fs::create_dir_all(&out_dir).unwrap();
        let keys = tmp.join("keys");
        let job = Job {
            target: crate::Target::Android,
            rendition: &rend,
            runtime: &shell_path,
            out_dir: &out_dir,
            branding: &b,
            keys_dir: Some(&keys),
            log: &|_| {},
        };
        let out = export(&job).unwrap();
        assert_eq!(out, out_dir.join("Frogger 2.apk"));
        let apk = std::fs::read(&out).unwrap();
        let a = sign::tests::verify(&apk);
        let names: Vec<&str> = a.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names[0], "AndroidManifest.xml");
        assert!(!names.contains(&"META-INF/CERT.RSA"));
        assert!(!names.iter().any(|n| n.contains("oracle")));
        assert!(names.contains(&"assets/game-data/usa/game/textures/a.fla"));
        assert!(names.contains(&"assets/game-data/usa/music/track02.wav"));
        assert!(names.contains(&"assets/game-data/usa/manifest.json"));
        for e in &a.entries {
            assert_eq!(e.method, zip::STORED);
            assert_eq!(e.data_offset % zip::alignment(&e.name, e.method), 0, "{}", e.name);
            assert_eq!(crc32fast::hash(&a.read(e).unwrap()), e.crc, "{}", e.name);
        }
        let m = axml::Axml::parse(&a.read(a.find("AndroidManifest.xml").unwrap()).unwrap()).unwrap();
        assert_eq!(m.get_string("manifest", "package").unwrap(), PACKAGE);
        assert_eq!(m.get_string("application", "label").unwrap(), "Frogger 2");
        assert_eq!(m.get_string("activity", "name").unwrap(), format!("{SHELL_PACKAGE}.MainActivity"));
        assert_eq!(m.get_string("manifest", "versionName").unwrap(), "0.1.0");
        let bg = a.read(a.find("res/mipmap-mdpi-v4/ic_launcher_background.png").unwrap()).unwrap();
        assert_eq!(png_size(&bg), Some((108, 108)));
        let idx: serde_json::Value = serde_json::from_slice(&a.read(a.find("assets/game-data/bundle.json").unwrap()).unwrap()).unwrap();
        assert_eq!(idx["rendition"], "usa");
        assert_eq!(idx["bytes"], 3000 + 5000 + 19);
        assert_eq!(idx["files"].as_array().unwrap().len(), 3);
        // The same key next time: the certificates match (installs as an update).
        let first_cert = std::fs::read(keys.join(sign::CERT_FILE)).unwrap();
        export(&job).unwrap();
        assert_eq!(std::fs::read(keys.join(sign::CERT_FILE)).unwrap(), first_cert);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// A manifest shaped like aapt2's output for the shell.
    fn shell_manifest() -> Vec<u8> {
        use axml::tests::{build_xml, Val};
        build_xml(&[
            ("manifest", vec![(true, "versionCode", Val::Int(1)), (true, "versionName", Val::Str("0.0.1")), (false, "package", Val::Str(SHELL_PACKAGE))]),
            ("application", vec![(true, "label", Val::Str("Swampy's Revenge Recomp")), (true, "icon", Val::Int(0x7f01_0000))]),
            ("activity", vec![(true, "name", Val::Str(".MainActivity")), (true, "exported", Val::Int(1))]),
        ])
    }
}
