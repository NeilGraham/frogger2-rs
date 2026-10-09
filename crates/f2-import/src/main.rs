//! `f2-import <disc image | disc folder | install folder> [--data-root DIR] [--oracle] [--no-music] [--verify] [--as NAME]`
//! `f2-import --list [--data-root DIR]`
//! `f2-import --verify-installed [rendition] [--data-root DIR]`
//!
//! `--flac` writes music as lossless `music/trackNN.flac` instead of `.wav`.

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
            "--list" => list = true,
            "--verify-installed" => {
                let r = args.next_if(|n| !n.starts_with("--"));
                verify = Some(r);
            }
            "-h" | "--help" => {
                println!("usage: f2-import <image.cue|.zip|.bin|.iso|disc dir|install dir> [--data-root DIR] [--oracle] [--no-music] [--verify] [--flac] [--as NAME]\n       f2-import --list\n       f2-import --verify-installed [rendition]");
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
    if let Err(e) = f2_import::import(&src, &root, &opts) {
        eprintln!("import failed: {e}");
        std::process::exit(1);
    }
}
