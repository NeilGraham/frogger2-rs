use std::path::PathBuf;
use std::process::ExitCode;

use f2_export::{Exporter, Kind};

const USAGE: &str = "usage: f2-export <rendition dir, e.g. game-data/usa> <out dir> [--kind textures|sounds|models|levels|text|all]";

fn main() -> ExitCode {
    let mut pos = Vec::new();
    let mut kind = Kind::All;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--kind" {
            match args.next().as_deref().and_then(Kind::parse) {
                Some(k) => kind = k,
                None => {
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                }
            }
        } else if let Some(k) = a.strip_prefix("--kind=") {
            match Kind::parse(k) {
                Some(k) => kind = k,
                None => {
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                }
            }
        } else if a == "-h" || a == "--help" {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        } else {
            pos.push(PathBuf::from(a));
        }
    }
    if pos.len() != 2 {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    let (src, out) = (&pos[0], &pos[1]);
    if !src.join("game").is_dir() {
        eprintln!("{}: no game/ directory (expected an imported rendition dir)", src.display());
        return ExitCode::from(2);
    }
    let mut ex = Exporter::new(src, out);
    ex.run(kind);
    println!("exported {} -> {}", src.display(), out.display());
    print!("{}", ex.report);
    if ex.report.errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        eprintln!("{} error(s)", ex.report.errors.len());
        ExitCode::FAILURE
    }
}
