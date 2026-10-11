use std::path::PathBuf;
use std::process::ExitCode;

use f2_n64pack::budget::{self, BudgetConfig};
use f2_n64pack::ctx::{default_tools, find_rendition};
use f2_n64pack::Ctx;

const USAGE: &str = "usage: f2-n64pack [--rendition DIR] [--out DIR] [--cache DIR] [--tools DIR] [--jobs N]
                  [--reserve-mib X] [--cart-mib N] [--size-weight W|inf] [--codec CLASS=NAME]... [--quick] [--only levels,models,textures,sfx,music,fmv]
                  [--bench]

Builds the N64 asset pack (default target/n64/pack) from an imported rendition (default: the
first game-data/usa found upwards) and prints the cartridge budget. Fails if the pack plus the
code reserve (default 2.5 MiB) exceeds the cartridge (default 64 MiB). --bench writes the
codec comparison (research/n64-compression.md numbers) to target/n64/bench.md.
--size-weight: F2Z space-speed trade-off, decode cycles per stored byte (19 = minimum
load time on the VR4300; inf = smallest pack, the default).
--codec: per-class codec, e.g. --codec models=f2z (defaults: models/textures aplib, levels f2z).";

fn main() -> ExitCode {
    let mut rendition: Option<PathBuf> = None;
    let mut out = PathBuf::from("target/n64/pack");
    let mut cache = PathBuf::from("target/n64/cache");
    let mut tools = default_tools();
    let mut jobs = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut cfg = BudgetConfig::default();
    let mut quick = false;
    let mut bench = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = || args.next().unwrap_or_default();
        match a.as_str() {
            "--rendition" => rendition = Some(val().into()),
            "--out" => out = val().into(),
            "--cache" => cache = val().into(),
            "--tools" => tools = val().into(),
            "--jobs" => jobs = val().parse().unwrap_or(jobs),
            "--reserve-mib" => cfg.code_reserve = (val().parse::<f64>().unwrap_or(2.5) * 1048576.0) as u64,
            "--cart-mib" => cfg.cart_bytes = val().parse::<u64>().unwrap_or(64) << 20,
            "--size-weight" => cfg.size_weight = val().parse().unwrap_or(f64::INFINITY),
            "--codec" => {
                let v = val();
                let parsed = v.split_once('=').and_then(|(c, k)| {
                    let class = match c {
                        "models" => f2_n64pack::Class::Models,
                        "textures" => f2_n64pack::Class::Textures,
                        "levels" => f2_n64pack::Class::Levels,
                        _ => return None,
                    };
                    Some((class, f2_n64pack::codec::Codec::parse(k)?))
                });
                match parsed {
                    Some(p) => cfg.codecs.push(p),
                    None => {
                        eprintln!("bad --codec {v} (CLASS=f2z|lz4|aplib|shrinkler|none, CLASS = models|textures|levels)");
                        return ExitCode::from(2);
                    }
                }
            }
            "--only" => cfg.only = Some(val().split(',').map(str::to_string).collect()),
            "--quick" => quick = true,
            "--bench" => bench = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ => {
                eprintln!("unknown argument {a}\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let Some(rendition) = rendition.or_else(|| find_rendition(&std::env::current_dir().unwrap_or_default())) else {
        eprintln!("no rendition found (pass --rendition game-data/usa)");
        return ExitCode::from(2);
    };
    let ctx = Ctx { rendition, out, cache, tools, jobs, quick };
    let r = if bench { f2_n64pack::bench::run(&ctx).map(|_| true) } else { budget::build(&ctx, &cfg) };
    match r {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
