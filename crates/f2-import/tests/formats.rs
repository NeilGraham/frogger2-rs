//! Real-data import parity across every container format and an emulated drive.
//!
//! Converts the owner's Redump image into each supported format in a temp dir, imports
//! each and checks the manifest's file hashes equal the Redump import's (data, videos and
//! music), then deletes the output. Needs the Redump zip (`$F2_REDUMP_ZIP`, default
//! `<repo>/Redump/Frogger 2 - Swampy's Revenge (USA).zip`); skips when absent. Slow
//! (≈ several GB of I/O):
//!
//! `cargo test --release -p f2-import --test formats -- --ignored --nocapture`

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use f2_import::{import, import_opened, Audio, Manifest, Opened, Options};
use f2_media::cdrom::ImageDrive;
use f2_media::convert::{write_image, OutFormat};
use f2_media::{Disc, IsoFs, Names};

fn repo_root() -> PathBuf {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let here = here.canonicalize().unwrap_or(here);
    // Worktrees live in <repo>/.claude/worktrees/<name>: the gitignored data is in <repo>.
    let s = here.to_string_lossy().into_owned();
    match s.find("/.claude/worktrees/") {
        Some(i) => PathBuf::from(&s[..i]),
        None => here,
    }
}

fn redump_zip() -> Option<PathBuf> {
    let p = std::env::var_os("F2_REDUMP_ZIP").map(PathBuf::from).unwrap_or_else(|| repo_root().join("Redump/Frogger 2 - Swampy's Revenge (USA).zip"));
    p.exists().then_some(p)
}

fn scratch() -> PathBuf {
    let base = std::env::var_os("F2_TEST_TMP").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join(format!("f2-import-formats-{}", std::process::id()))
}

/// Files of the manifest minus nothing (oracle is off): data, videos, music.
fn files(m: &Manifest) -> &BTreeMap<String, String> {
    &m.files
}

fn diff(a: &BTreeMap<String, String>, b: &BTreeMap<String, String>) -> Vec<String> {
    let mut d = Vec::new();
    for (k, v) in a {
        if b.get(k) != Some(v) {
            d.push(format!("{k}: {v} vs {:?}", b.get(k)));
        }
    }
    for k in b.keys() {
        if !a.contains_key(k) {
            d.push(format!("{k}: only in second"));
        }
    }
    d
}

fn unzip(zip: &Path, out: &Path) {
    std::fs::create_dir_all(out).unwrap();
    let mut a = zip::ZipArchive::new(std::fs::File::open(zip).unwrap()).unwrap();
    for i in 0..a.len() {
        let mut e = a.by_index(i).unwrap();
        let name = e.enclosed_name().unwrap();
        let p = out.join(name.file_name().unwrap());
        let mut f = std::fs::File::create(&p).unwrap();
        std::io::copy(&mut e, &mut f).unwrap();
    }
}

#[test]
#[ignore = "slow; needs the Redump image"]
fn every_format_imports_identically() {
    let Some(zip) = redump_zip() else {
        eprintln!("skipping: Redump zip not found (set F2_REDUMP_ZIP)");
        return;
    };
    let tmp = scratch();
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let opts = Options { verify_track_hash: true, ..Default::default() };

    // Reference: the Redump zip itself.
    let reference = import(&zip, &tmp.join("ref-root"), &opts).expect("import zip");
    assert_eq!(reference.rendition, "usa");
    assert_eq!(reference.redump_verified, Some(true));
    assert_eq!(reference.music_tracks.len(), 11);
    let want = files(&reference).clone();
    let music_dir = tmp.join("ref-root/usa/music");

    let src_dir = tmp.join("redump");
    unzip(&zip, &src_dir);
    let cue = std::fs::read_dir(&src_dir).unwrap().flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "cue")).unwrap();
    let disc = Disc::open(&cue).unwrap();

    let mut report = Vec::new();
    for &(name, fmt) in OutFormat::ALL {
        if fmt == OutFormat::Iso {
            continue;
        }
        let dir = tmp.join(format!("img-{name}"));
        let p = write_image(&disc, fmt, &dir, "Frogger 2").unwrap();
        let root = tmp.join(format!("root-{name}"));
        let m = import(&p, &root, &opts).unwrap_or_else(|e| panic!("{name}: {e}"));
        let d = diff(&want, files(&m));
        assert!(d.is_empty(), "{name}: {} differences, e.g. {:?}", d.len(), &d[..d.len().min(5)]);
        assert_eq!(m.redump_verified, Some(true), "{name}: per-track Redump hashes");
        report.push(format!("{name} ({}): identical, Redump-verified", m.source_kind));
        // Same image inside a zip (stored members).
        if fmt == OutFormat::Ccd {
            let zp = tmp.join("ccd.zip");
            {
                let mut w = zip::ZipWriter::new(std::fs::File::create(&zp).unwrap());
                let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(true);
                for e in std::fs::read_dir(&dir).unwrap().flatten() {
                    w.start_file(format!("Frogger 2/{}", e.file_name().to_string_lossy()), o).unwrap();
                    std::io::copy(&mut std::fs::File::open(e.path()).unwrap(), &mut w).unwrap();
                }
                w.finish().unwrap();
            }
            let m = import(&zp, &tmp.join("root-zipccd"), &Options::default()).unwrap();
            assert!(diff(&want, files(&m)).is_empty(), "zip(ccd)");
            report.push(format!("zip(ccd) ({}): identical", m.source_kind));
            std::fs::remove_file(&zp).ok();
            std::fs::remove_dir_all(tmp.join("root-zipccd")).ok();
        }
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    // Data-only ISO: same data + videos, no music, then music from a folder.
    let iso = write_image(&disc, OutFormat::Iso, &tmp.join("img-iso"), "FROGGER2").unwrap();
    let m = import(&iso, &tmp.join("root-iso"), &Options::default()).unwrap();
    assert!(m.music_tracks.is_empty());
    assert_eq!(m.music_source, "none");
    assert!(m.music_note.as_deref().unwrap_or("").contains("--music"));
    let data_only: BTreeMap<_, _> = want.iter().filter(|(k, _)| !k.starts_with("music/")).map(|(k, v)| (k.clone(), v.clone())).collect();
    assert!(diff(&data_only, files(&m)).is_empty(), "iso data");
    let m = import(&iso, &tmp.join("root-iso"), &Options { music_dir: Some(music_dir.clone()), ..Default::default() }).unwrap();
    assert!(diff(&want, files(&m)).is_empty(), "iso + --music");
    assert_eq!(m.music_source, "folder");
    report.push("iso: data identical, no music + note; with --music: identical".into());
    std::fs::remove_dir_all(tmp.join("root-iso")).ok();

    // Emulated physical drive with a read offset and transient read errors.
    for offset in [6, -12, 667] {
        let mut drive = ImageDrive::new(Disc::open(&cue).unwrap()).unwrap();
        drive.read_offset = offset;
        drive.flaky.insert(150_000, 2);
        let fs = Box::new(IsoFs::open(disc.data_source().unwrap(), Names::PreferJoliet).unwrap());
        let src = Opened { fs, audio: Audio::Drive(Box::new(drive)), kind: "drive".into(), install: false, desc: "emulated".into() };
        let root = tmp.join("root-drive");
        let m = import_opened(src, &root, &Options::default()).unwrap();
        let rip = m.rip.clone().unwrap();
        assert!(rip.offset_detected, "offset {offset}");
        assert_eq!(rip.offset, offset);
        assert_eq!(rip.verified_tracks.len(), 11, "offset {offset}: {:?}", rip.verified_tracks);
        assert!(rip.retried_sectors > 0);
        let d = diff(&want, files(&m));
        assert!(d.is_empty(), "drive offset {offset}: {:?}", &d[..d.len().min(5)]);
        report.push(format!("emulated drive, read offset {offset:+}: {}, files identical", rip.status()));
        std::fs::remove_dir_all(&root).ok();
    }

    std::fs::remove_dir_all(&tmp).ok();
    for r in report {
        eprintln!("PASS {r}");
    }
}
