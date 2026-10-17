//! Export for macOS (docs/EXPORT.md "macOS"): `<out>/<file stem>.app`, a copy of the
//! running `.app` with its `Info.plist` renamed (`CFBundleName`/`CFBundleDisplayName` = the
//! title, its own `CFBundleIdentifier`), a `.icns` made from the branding icon, and the game
//! data at `Contents/Resources/game-data/<rendition>/`, which the exe finds from
//! `Contents/MacOS/` (`frogger::data::Data::locate`).
//!
//! Editing a bundle breaks its code-signature seal: on macOS the copy is re-signed ad hoc
//! (`codesign --force --deep -s -`); elsewhere the player does that once on the Mac (see the
//! docs).

use crate::util;
use crate::{Image, Job};
use std::path::{Path, PathBuf};

/// Bundle identifier of exported builds (the port app itself is
/// `io.github.NeilGraham.Frogger2rs`).
pub(crate) const BUNDLE_ID: &str = "io.github.NeilGraham.Frogger2rs.game";

/// Icon file in `Contents/Resources/` (`CFBundleIconFile`).
const ICON_NAME: &str = "game-icon";

pub(crate) fn export(job: &Job) -> Result<PathBuf, String> {
    let src = job.runtime;
    if !src.join("Contents/Info.plist").is_file() {
        return Err(format!("{} is not an app bundle (the macOS export is made from the macOS app)", src.display()));
    }
    let app = job.out_dir.join(format!("{}.app", job.branding.file_stem()));
    if app.exists() {
        std::fs::remove_dir_all(&app).map_err(|e| format!("{}: {e}", app.display()))?;
    }
    (job.log)(&format!("Copying {}", src.display()));
    copy_bundle(src, &app)?;
    let contents = app.join("Contents");
    // An exported app exported again: its data is replaced below.
    let _ = std::fs::remove_dir_all(contents.join("Resources/game-data"));

    let plist_path = contents.join("Info.plist");
    let mut plist = plist::Value::from_file(&plist_path).map_err(|e| format!("{}: {e}", plist_path.display()))?;
    let old_icon = rebrand_plist(&mut plist, &job.branding.title, &job.branding.version)?;
    if let Some(old) = old_icon {
        let old = if old.ends_with(".icns") { old } else { format!("{old}.icns") };
        if old != format!("{ICON_NAME}.icns") {
            let _ = std::fs::remove_file(contents.join("Resources").join(old));
        }
    }
    plist.to_file_xml(&plist_path).map_err(|e| format!("{}: {e}", plist_path.display()))?;
    std::fs::create_dir_all(contents.join("Resources")).map_err(|e| e.to_string())?;
    let icns_path = contents.join("Resources").join(format!("{ICON_NAME}.icns"));
    std::fs::write(&icns_path, encode_icns(&job.branding.icon)).map_err(|e| format!("{}: {e}", icns_path.display()))?;

    copy_data(job, &contents.join("Resources/game-data"))?;
    sign(&app, job.log);
    Ok(app)
}

/// Sets the names, identifier and icon in an app's `Info.plist`; returns the old
/// `CFBundleIconFile`.
pub(crate) fn rebrand_plist(plist: &mut plist::Value, title: &str, version: &str) -> Result<Option<String>, String> {
    let dict = plist.as_dictionary_mut().ok_or("Info.plist is not a dictionary")?;
    let old_icon = dict.get("CFBundleIconFile").and_then(|v| v.as_string()).map(str::to_string);
    let s = |v: &str| plist::Value::String(v.to_string());
    // CFBundleName is limited to 15 characters by convention; macOS shows
    // CFBundleDisplayName (the Finder name follows the bundle's file name).
    dict.insert("CFBundleName".into(), s(title));
    dict.insert("CFBundleDisplayName".into(), s(title));
    dict.insert("CFBundleIdentifier".into(), s(BUNDLE_ID));
    dict.insert("CFBundleIconFile".into(), s(ICON_NAME));
    dict.remove("CFBundleIconName"); // an asset-catalog icon would win over the .icns
    if !dict.contains_key("CFBundleShortVersionString") {
        dict.insert("CFBundleShortVersionString".into(), s(version));
    }
    Ok(old_icon)
}

/// Apple icon image (`.icns`) with PNG entries from 32 to 1024 px (macOS 10.7+).
pub(crate) fn encode_icns(icon: &Image) -> Vec<u8> {
    // (OSType, pixel size): 16/32/128/256/512 pt at 1x and 2x.
    const ENTRIES: [(&[u8; 4], u32); 8] =
        [(b"ic11", 32), (b"ic12", 64), (b"ic07", 128), (b"ic13", 256), (b"ic08", 256), (b"ic14", 512), (b"ic09", 512), (b"ic10", 1024)];
    let mut body = Vec::new();
    for (ty, size) in ENTRIES {
        let png = util::png(&util::resize(icon, size, size));
        body.extend_from_slice(ty);
        body.extend_from_slice(&(8 + png.len() as u32).to_be_bytes());
        body.extend_from_slice(&png);
    }
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&(8 + body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

/// Ad-hoc signature for the edited bundle (macOS only; elsewhere it is left to the player).
fn sign(app: &Path, log: &dyn Fn(&str)) {
    if !cfg!(target_os = "macos") {
        log("Not on macOS: the app's signature no longer matches (sign it on the Mac with `codesign --force --deep -s - <app>`)");
        return;
    }
    match std::process::Command::new("codesign").args(["--force", "--deep", "-s", "-"]).arg(app).status() {
        Ok(s) if s.success() => log("Signed (ad hoc)"),
        Ok(s) => log(&format!("warning: codesign failed ({s}); the app may not launch until signed")),
        Err(e) => log(&format!("warning: codesign not available ({e}); the app may not launch until signed")),
    }
}

/// Copies the bundle, keeping permissions and symlinks.
fn copy_bundle(src: &Path, dst: &Path) -> Result<(), String> {
    let err = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    std::fs::create_dir_all(dst).map_err(|e| err(dst, e))?;
    for e in std::fs::read_dir(src).map_err(|e| err(src, e))? {
        let e = e.map_err(|e| err(src, e))?;
        let (from, to) = (e.path(), dst.join(e.file_name()));
        let ft = e.file_type().map_err(|e| err(&from, e))?;
        if ft.is_dir() {
            copy_bundle(&from, &to)?;
        } else if ft.is_symlink() {
            let target = std::fs::read_link(&from).map_err(|e| err(&from, e))?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &to).map_err(|e| err(&to, e))?;
            #[cfg(not(unix))]
            {
                let _ = target;
                std::fs::copy(&from, &to).map_err(|e| err(&from, e))?;
            }
        } else {
            std::fs::copy(&from, &to).map_err(|e| err(&from, e))?;
        }
    }
    Ok(())
}

/// Copies the rendition's game files to `<dest>/<rendition name>/`.
fn copy_data(job: &Job, dest: &Path) -> Result<(), String> {
    let name = job.rendition.file_name().ok_or("the rendition has no name")?;
    let dest = dest.join(name);
    let files = util::rendition_files(job.rendition).map_err(|e| format!("{}: {e}", job.rendition.display()))?;
    let total: u64 = files.iter().filter_map(|(_, p)| p.metadata().ok()).map(|m| m.len()).sum();
    (job.log)(&format!("Copying the game data ({} files, {} MB)", files.len(), total / 1_000_000));
    for (rel, src) in files {
        let to = dest.join(&rel);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::copy(&src, &to).map_err(|e| format!("{} -> {}: {e}", src.display(), to.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Frogger2rs</string>
  <key>CFBundleDisplayName</key><string>Frogger 2: Swampy's Revenge (Rust)</string>
  <key>CFBundleIdentifier</key><string>io.github.NeilGraham.Frogger2rs</string>
  <key>CFBundleExecutable</key><string>frogger2</string>
  <key>CFBundleIconFile</key><string>frogger2-rs</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>"#;

    fn icon() -> Image {
        Image { width: 4, height: 4, rgba: [10u8, 200, 30, 255].repeat(16) }
    }

    #[test]
    fn icns_entries() {
        let icns = encode_icns(&icon());
        assert_eq!(&icns[0..4], b"icns");
        assert_eq!(u32::from_be_bytes(icns[4..8].try_into().unwrap()) as usize, icns.len());
        let mut types = Vec::new();
        let mut off = 8;
        while off < icns.len() {
            let ty = String::from_utf8(icns[off..off + 4].to_vec()).unwrap();
            let len = u32::from_be_bytes(icns[off + 4..off + 8].try_into().unwrap()) as usize;
            let png = &icns[off + 8..off + len];
            let dec = png::Decoder::new(std::io::Cursor::new(png)).read_info().unwrap();
            let size = dec.info().width;
            assert_eq!(dec.info().height, size);
            types.push((ty, size));
            off += len;
        }
        assert_eq!(off, icns.len());
        assert!(types.contains(&("ic10".into(), 1024)));
        assert!(types.contains(&("ic07".into(), 128)));
        assert!(types.contains(&("ic11".into(), 32)));
    }

    #[test]
    fn exports_a_bundle() {
        let tmp = std::env::temp_dir().join(format!("f2pkg-mac-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let rt = tmp.join("Frogger2rs.app");
        let c = rt.join("Contents");
        std::fs::create_dir_all(c.join("MacOS")).unwrap();
        std::fs::create_dir_all(c.join("Resources")).unwrap();
        std::fs::write(c.join("Info.plist"), PLIST).unwrap();
        std::fs::write(c.join("MacOS/frogger2"), "bin").unwrap();
        std::fs::write(c.join("Resources/frogger2-rs.icns"), "old").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(c.join("MacOS/frogger2"), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let rend = tmp.join("data/usa");
        std::fs::create_dir_all(rend.join("music")).unwrap();
        std::fs::write(rend.join("music/track02.flac"), "m").unwrap();
        std::fs::write(rend.join("manifest.json"), "{}").unwrap();
        let b = crate::Branding {
            title: "Frogger 2: Swampy's Revenge".into(),
            short_title: "Frogger 2".into(),
            art: icon(),
            icon: icon(),
            version: "0.1.0".into(),
        };
        let out = tmp.join("out");
        let job = Job { target: crate::Target::MacOs, rendition: &rend, runtime: &rt, out_dir: &out, branding: &b, keys_dir: None, log: &|_| {} };
        let app = crate::export(&job).unwrap();
        assert_eq!(app, out.join("Frogger 2 - Swampy's Revenge.app"));
        let c = app.join("Contents");
        let p = plist::Value::from_file(c.join("Info.plist")).unwrap();
        let d = p.as_dictionary().unwrap();
        let get = |k: &str| d.get(k).and_then(|v| v.as_string()).unwrap().to_string();
        assert_eq!(get("CFBundleName"), "Frogger 2: Swampy's Revenge");
        assert_eq!(get("CFBundleDisplayName"), "Frogger 2: Swampy's Revenge");
        assert_eq!(get("CFBundleIdentifier"), BUNDLE_ID);
        assert_eq!(get("CFBundleExecutable"), "frogger2");
        assert_eq!(get("CFBundleIconFile"), ICON_NAME);
        assert_eq!(d.get("NSHighResolutionCapable").and_then(|v| v.as_boolean()), Some(true));
        assert_eq!(&std::fs::read(c.join("Resources/game-icon.icns")).unwrap()[0..4], b"icns");
        assert!(!c.join("Resources/frogger2-rs.icns").exists());
        assert!(c.join("Resources/game-data/usa/music/track02.flac").is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(c.join("MacOS/frogger2")).unwrap().permissions().mode() & 0o777, 0o755);
        }
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
