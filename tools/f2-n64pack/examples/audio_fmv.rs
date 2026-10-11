//! Runs the audio + FMV packing steps end to end on the imported data.
//!
//! `cargo run --release -p f2-n64pack --example audio_fmv -- [sfx] [music KBPS] [fmv BUDGET|max] [compare NAME] [table]`
//! Outputs go to `target/n64/pack`, cache to `target/n64/cache` (relative to the current directory).

use std::path::PathBuf;

use f2_n64pack::{audio, ctx, video, ClassReport, Ctx};

fn show(r: &ClassReport) {
    let total: u64 = r.files.iter().map(|f| f.bytes).sum();
    println!("{} files, {total} bytes", r.files.len());
    for n in &r.notes {
        println!("  {n}");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cwd = std::env::current_dir().unwrap();
    let rendition = ctx::find_rendition(&cwd).expect("no game-data/usa found");
    let c = Ctx {
        rendition,
        out: PathBuf::from("target/n64/pack"),
        cache: PathBuf::from("target/n64/cache"),
        tools: ctx::default_tools(),
        jobs: std::thread::available_parallelism().map_or(4, |n| n.get()),
        quick: false,
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "sfx" => show(&audio::pack_sfx(&c).unwrap()),
            "music" => {
                i += 1;
                show(&audio::pack_music(&c, args[i].parse().unwrap()).unwrap());
            }
            "fmv" => {
                i += 1;
                let b = if args[i] == "max" { u64::MAX } else { args[i].parse().unwrap() };
                show(&video::pack_fmv(&c, b).unwrap());
            }
            "table" => {
                for (l, b) in video::fmv_size_table(&c, &[0, 20, 40, 60, 80, 100]).unwrap() {
                    println!("{l}: {b} bytes");
                }
            }
            "compare" => {
                i += 1;
                for (l, r) in video::compare_codecs(&c, &args[i]) {
                    match r {
                        Ok(p) => println!("{l}: {} bytes ssim {:.4} psnr {:.2}", p.bytes, p.ssim, p.psnr),
                        Err(e) => println!("{l}: FAILED {}", e.lines().last().unwrap_or("")),
                    }
                }
            }
            a => panic!("unknown arg {a}"),
        }
        i += 1;
    }
}
