//! `f2-modkit`: validate and scaffold Frogger 2 mods.
//!
//! ```text
//! f2-modkit pack <mod dir> [--rendition <dir>]   validate a mod (exit 1 on errors)
//! f2-modkit new <id> [--dir <mods dir>]          create <mods dir>/<id>/ with mod.toml
//! ```
//! `--rendition` defaults to `$F2_RENDITION`, else `game-data/usa` when it exists;
//! it enables the retail-hash and unchanged-entry checks.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage:\n  f2-modkit pack <mod dir> [--rendition <dir>]\n  f2-modkit new <id> [--dir <mods dir>]";

fn opt(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn positional(args: &[String]) -> Option<&String> {
    let mut i = 0;
    while i < args.len() {
        if args[i].starts_with("--") {
            i += 2;
            continue;
        }
        return Some(&args[i]);
    }
    None
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let rest = &args[1..];
    match cmd.as_str() {
        "pack" => {
            let Some(dir) = positional(rest) else {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            };
            let rendition = opt(rest, "--rendition").or_else(|| std::env::var("F2_RENDITION").ok()).map(PathBuf::from).or_else(|| {
                let d = PathBuf::from("game-data/usa");
                d.join("manifest.json").is_file().then_some(d)
            });
            if rendition.is_none() {
                eprintln!("note: no rendition given; retail-asset checks skipped (use --rendition game-data/usa)");
            }
            let r = f2_modkit::pack::validate(Path::new(dir), rendition.as_deref());
            if let Some(m) = &r.manifest {
                println!("mod {} {} ({})", m.id, m.version, m.name);
            }
            for (k, v) in &r.counts {
                println!("  {k:<24} {v}");
            }
            for w in &r.warnings {
                println!("  warning: {w}");
            }
            for e in &r.errors {
                println!("  ERROR: {e}");
            }
            if r.ok() {
                println!("ok");
                ExitCode::SUCCESS
            } else {
                println!("{} error(s)", r.errors.len());
                ExitCode::FAILURE
            }
        }
        "new" => {
            let Some(id) = positional(rest) else {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            };
            let dir = opt(rest, "--dir").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("mods"));
            match f2_modkit::pack::scaffold(&dir, id) {
                Ok(p) => {
                    println!("created {}", p.display());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
