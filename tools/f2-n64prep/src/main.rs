//! `f2-n64prep <game dir> <dfs out dir> <wav out dir> [bank ...]`: see the library docs.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!("usage: f2-n64prep <game dir> <dfs out dir> <wav out dir> [sound bank stem ...]");
        return ExitCode::from(2);
    }
    let (game, out, wav) = (PathBuf::from(&args[0]), PathBuf::from(&args[1]), PathBuf::from(&args[2]));
    match f2_n64prep::prepare(&game, &out, &wav, &args[3..]) {
        Ok(r) => {
            println!(
                "f2-n64prep: {} raw files ({} KiB), {} object banks ({} KiB DLL -> {} KiB postcard), {} sound banks ({} samples, {} KiB PCM)",
                r.raw_files,
                r.raw_bytes / 1024,
                r.object_banks,
                r.object_bytes_in / 1024,
                r.object_bytes_out / 1024,
                r.sound_banks,
                r.samples,
                r.sample_bytes / 1024
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("f2-n64prep: {e}");
            ExitCode::FAILURE
        }
    }
}
