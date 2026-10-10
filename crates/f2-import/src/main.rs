//! `f2-import <disc image | drive (D: or /dev/sr0) | disc folder | install folder> [--data-root DIR] [--oracle] [--no-music] [--verify] [--as NAME] [--music DIR] [--drive-offset N]`
//! `f2-import --list-drives`
//! `f2-import --list [--data-root DIR]`
//! `f2-import --verify-installed [rendition] [--data-root DIR]`
//! `f2-import --convert cue|ccd|mds|mds-sub|nrg|nrg1|toc|iso <image> <out dir>` (developer tool)
//!
//! `--flac` writes music as lossless `music/trackNN.flac` instead of `.wav`; `--music DIR`
//! takes missing tracks 02–12 from WAV/FLAC files; `--drive-offset N` forces a drive read
//! offset (samples) instead of detecting it.

use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1).peekable();
    let mut source: Option<PathBuf> = None;
    let mut root = f2_import::default_data_root();
    let mut opts = f2_import::Options::default();
    let mut list = false;
    let mut verify: Option<Option<String>> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--data-root" => root = PathBuf::from(args.next().expect("--data-root needs a value")),
            "--oracle" => opts.with_oracle = true,
            "--no-music" => opts.skip_music = true,
            "--verify" => opts.verify_track_hash = true,
            "--as" => opts.rendition = Some(args.next().expect("--as needs a value")),
            "--flac" => opts.flac = true,
            "--music" => opts.music_dir = Some(PathBuf::from(args.next().expect("--music needs a folder"))),
            "--drive-offset" => opts.drive_offset = Some(args.next().and_then(|v| v.parse().ok()).expect("--drive-offset needs a number of samples")),
            "--list-drives" => {
                list_drives();
                return;
            }
            "--convert" => {
                let (f, img, out) = (args.next(), args.next(), args.next());
                let (Some(f), Some(img), Some(out)) = (f, img, out) else {
                    eprintln!("usage: f2-import --convert FORMAT IMAGE OUTDIR");
                    std::process::exit(2);
                };
                let Some(fmt) = f2_media::convert::OutFormat::parse(&f) else {
                    eprintln!("unknown format {f}");
                    std::process::exit(2);
                };
                let img = PathBuf::from(img);
                let stem = img.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                match f2_media::Disc::open(&img).and_then(|d| f2_media::convert::write_image(&d, fmt, std::path::Path::new(&out), &stem)) {
                    Ok(p) => println!("{}", p.display()),
                    Err(e) => {
                        eprintln!("convert failed: {e}");
                        std::process::exit(1);
                    }
                }
                return;
            }
            "--list" => list = true,
            "--verify-installed" => {
                let r = args.next_if(|n| !n.starts_with("--"));
                verify = Some(r);
            }
            "-h" | "--help" => {
                println!(
                    "usage: f2-import <SOURCE> [--data-root DIR] [--oracle] [--no-music] [--verify] [--flac] [--as NAME] [--music DIR] [--drive-offset N]\n\
                     \x20      f2-import --list-drives | --list | --verify-installed [rendition]\n\
                     \x20      f2-import --convert cue|ccd|mds|mds-sub|nrg|nrg1|toc|iso IMAGE OUTDIR\n\
                     SOURCE: .cue/.bin, .ccd/.img, .mds/.mdf, .nrg, .toc, .iso, .zip of any of these,\n\
                     \x20       a drive (D: or /dev/sr0), a disc folder or an install folder"
                );
                return;
            }
            _ => source = Some(PathBuf::from(a)),
        }
    }
    if let Some(r) = verify {
        match f2_import::verify_installed(&root, r.as_deref()) {
            Ok(reports) => {
                let mut bad = false;
                for rep in reports {
                    println!(
                        "{}: {} files checked, {} missing, {} modified, {} extra",
                        rep.rendition,
                        rep.checked,
                        rep.missing.len(),
                        rep.mismatched.len(),
                        rep.extra.len()
                    );
                    for f in &rep.missing {
                        println!("  MISSING  {f}");
                    }
                    for f in &rep.mismatched {
                        println!("  MODIFIED {f}");
                    }
                    for f in &rep.extra {
                        println!("  extra    {f}");
                    }
                    bad |= !rep.ok();
                }
                std::process::exit(if bad { 1 } else { 0 });
            }
            Err(e) => {
                eprintln!("verify failed: {e}");
                std::process::exit(2);
            }
        }
    }
    if list {
        for m in f2_import::installed(&root) {
            println!(
                "{:10} {} ({} files, {} music tracks, {} videos){}",
                m.rendition,
                m.title,
                m.files.len(),
                m.music_tracks.len(),
                m.videos.len(),
                if m.verified { "" } else { " [unverified]" }
            );
        }
        return;
    }
    let Some(src) = source else {
        eprintln!("no source given (try --help)");
        std::process::exit(2);
    };
    match f2_import::import(&src, &root, &opts) {
        Ok(m) => {
            eprintln!("Imported {} ({} files, {} videos, {} music tracks, music: {})", m.title, m.files.len(), m.videos.len(), m.music_tracks.len(), m.music_source);
            if let Some(r) = &m.rip {
                eprintln!("Drive rip: {}", r.status());
            }
        }
        Err(e) => {
            eprintln!("import failed: {e}");
            std::process::exit(1);
        }
    }
}

fn list_drives() {
    let drives = f2_media::cdrom::list_drives();
    if drives.is_empty() {
        println!("no CD/DVD drives found");
    }
    for d in drives {
        println!("{}", d.summary());
        if !d.media {
            continue;
        }
        match f2_media::cdrom::open_drive(&d.device).and_then(|mut r| r.read_toc()) {
            Ok(toc) => {
                let audio = toc.audio_count();
                println!("    TOC: {} tracks ({} data, {} audio), lead-out at LBA {}", toc.tracks.len(), toc.tracks.len() - audio, audio, toc.leadout);
            }
            Err(e) => println!("    TOC unavailable: {e}"),
        }
    }
}
