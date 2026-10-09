//! `f2-import <disc image | disc folder | install folder> [--data-root DIR] [--oracle] [--no-music] [--verify] [--as NAME]`
//! `f2-import --list [--data-root DIR]`

use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut source: Option<PathBuf> = None;
    let mut root = f2_import::default_data_root();
    let mut opts = f2_import::Options::default();
    let mut list = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--data-root" => root = PathBuf::from(args.next().expect("--data-root needs a value")),
            "--oracle" => opts.with_oracle = true,
            "--no-music" => opts.skip_music = true,
            "--verify" => opts.verify_track_hash = true,
            "--as" => opts.rendition = Some(args.next().expect("--as needs a value")),
            "--list" => list = true,
            "-h" | "--help" => {
                println!("usage: f2-import <image.cue|.zip|.bin|.iso|disc dir|install dir> [--data-root DIR] [--oracle] [--no-music] [--verify] [--as NAME]\n       f2-import --list");
                return;
            }
            _ => source = Some(PathBuf::from(a)),
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
