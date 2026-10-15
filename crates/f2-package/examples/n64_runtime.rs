//! N64 runtime tool (docs/EXPORT.md "N64"), used by `tools/runtime/build-n64.sh` and
//! `tools/n64/export-check.sh`:
//!
//! ```text
//! n64_runtime make ROM OUT.zip VERSION [KEY=VALUE]...   runtime from a tools/n64/build.sh ROM
//! n64_runtime dfs DIR OUT.dfs                           DragonFS image of a directory (= mkdfs)
//! n64_runtime rom RUNTIME DIR OUT.z64 [TITLE]           ROM from a runtime + a pack directory
//! n64_runtime info RUNTIME|ROM                          manifest / TOC
//! ```
//! `make` takes the ROM's IPL3 header and every TOC file except the DFS (n64.mk alignments:
//! 256 for the first file, 16 for the others) and checks that re-assembling them reproduces
//! the ROM byte for byte; KEY=VALUE pairs go to `runtime.json` `build`.

use f2_package::n64::{rom, runtime};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

fn run(args: &[String]) -> Result<(), String> {
    let read = |p: &str| std::fs::read(p).map_err(|e| format!("{p}: {e}"));
    match args.first().map(String::as_str) {
        Some("make") if args.len() >= 4 => {
            let rom_bytes = read(&args[1])?;
            let (_, _, toc) = rom::read_toc(&rom_bytes).ok_or("no TOC in the ROM")?;
            let aligns: Vec<usize> = (0..toc.len()).map(|i| if i == 0 { 256 } else { 16 }).collect();
            let build: BTreeMap<String, String> = args[4..].iter().filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.to_string(), v.to_string()))).collect();
            let rt = runtime::Runtime::from_rom(&rom_bytes, &args[3], &aligns, runtime::PackSpec::default_portable(), build)?;
            rt.write_zip(Path::new(&args[2]))?;
            println!("{}: {}", args[2], serde_json::to_string(&rt.manifest).map_err(|e| e.to_string())?);
            Ok(())
        }
        Some("dfs") if args.len() == 3 => {
            let img = f2_n64pack::dfs::build_dir(Path::new(&args[1]))?;
            std::fs::write(&args[2], &img).map_err(|e| format!("{}: {e}", args[2]))?;
            println!("{}: {} bytes", args[2], img.len());
            Ok(())
        }
        Some("rom") if args.len() >= 4 => {
            let rt = runtime::Runtime::load(Path::new(&args[1]))?;
            let title = args.get(4).map(String::as_str);
            let dfs = f2_n64pack::dfs::build_dir(Path::new(&args[2]))?;
            let r = rt.assemble(&dfs, title)?;
            std::fs::write(&args[3], &r).map_err(|e| format!("{}: {e}", args[3]))?;
            println!("{}: {} bytes (DFS {} bytes)", args[3], r.len(), dfs.len());
            Ok(())
        }
        Some("info") if args.len() == 2 => {
            let p = Path::new(&args[1]);
            if p.extension().is_some_and(|e| e == "z64") {
                let b = read(&args[1])?;
                let (at, cookie, toc) = rom::read_toc(&b).ok_or("no TOC")?;
                println!("{} bytes, title {:?}, TOC at {at:#x}, cookie {cookie:08x}", b.len(), String::from_utf8_lossy(&b[0x20..0x34]).trim_end_matches('\0'));
                for e in toc {
                    println!("  {:#010x} {:>10} {}", e.offset, e.size, e.name);
                }
            } else {
                let rt = runtime::Runtime::load(p)?;
                println!("{}", serde_json::to_string_pretty(&rt.manifest).map_err(|e| e.to_string())?);
            }
            Ok(())
        }
        _ => Err("usage: n64_runtime make ROM OUT.zip VERSION [K=V].. | dfs DIR OUT | rom RUNTIME DIR OUT.z64 [TITLE] | info FILE".into()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("n64_runtime: {e}");
            ExitCode::FAILURE
        }
    }
}
