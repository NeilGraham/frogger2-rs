//! Codec comparison on the real data (`f2-n64pack --bench`), per asset class:
//! libdragon `mkasset` levels 1/2/3 (LZ4 / aPLib / Shrinkler, 256 KiB window), host
//! reference points zstd -19 (with and without a trained 64 KiB dictionary), xz -9e and
//! brotli -q11, and F2Z variants (plain, + structure transforms, + shared dictionary).
//! Every codec compresses each file separately (as the pack stores them). Results go to
//! stdout and `target/n64/bench.md`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use f2_n64z::dict::EncDict;
use f2_n64z::enc::{compress, Options};
use f2_n64z::{header, Decoder, Dict};

use crate::codec::{candidates, compress_asset, train_dict, ClassCodec};
use crate::{levels, models, textures, Class, Ctx, RawAsset, Result};

struct Row {
    name: String,
    bytes: u64,
    note: String,
}

fn write_files(dir: &Path, assets: &[RawAsset]) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    assets
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let p = dir.join(format!("{i:04}.bin"));
            std::fs::write(&p, &a.data).map_err(|e| e.to_string())?;
            Ok(p)
        })
        .collect()
}

fn tool_sizes(files: &[PathBuf], jobs: usize, mk: impl Fn(&Path) -> Command + Sync) -> Result<u64> {
    let r = crate::tools::par_map(files, jobs, |f: &PathBuf| -> Result<u64> { Ok(crate::tools::run(&mut mk(f))?.stdout.len() as u64) });
    r.into_iter().sum()
}

fn mkasset(ctx: &Ctx, files: &[PathBuf], level: u32, scratch: &Path) -> Result<u64> {
    let out = scratch.join(format!("mkasset{level}"));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    for chunk in files.chunks(200) {
        crate::tools::run(Command::new(ctx.tool("mkasset")).args(["-c", &level.to_string(), "-w", "256", "-o"]).arg(&out).args(chunk))?;
    }
    let mut t = 0;
    for f in files {
        t += std::fs::metadata(out.join(f.file_name().unwrap())).map_err(|e| e.to_string())?.len();
    }
    Ok(t)
}

/// Host decode throughput of a set of F2Z streams (MB/s of output), best of 3.
fn f2z_decode_speed(streams: &[Vec<u8>], dict: Option<&EncDict>) -> f64 {
    let dbytes = dict.map(EncDict::to_bytes);
    let d = dbytes.as_ref().map(|b| Dict::parse(b).unwrap());
    let mut dec = Box::new(Decoder::new());
    let bufs: Vec<(Vec<u8>, Vec<u8>)> = streams
        .iter()
        .map(|z| {
            let h = header(z).unwrap();
            (vec![0u8; h.raw_size as usize], vec![0u8; if h.needs_scratch() { h.raw_size as usize } else { 0 }])
        })
        .collect();
    let total: usize = bufs.iter().map(|b| b.0.len()).sum();
    let mut best = f64::INFINITY;
    let mut bufs = bufs;
    for _ in 0..3 {
        let t = Instant::now();
        for (z, (o, tmp)) in streams.iter().zip(bufs.iter_mut()) {
            dec.decode(z, d.as_ref(), o, tmp).unwrap();
        }
        best = best.min(t.elapsed().as_secs_f64());
    }
    total as f64 / best / 1e6
}

fn bench_class(ctx: &Ctx, class: Class, assets: &[RawAsset], md: &mut String) -> Result<()> {
    if assets.is_empty() {
        return Ok(());
    }
    let scratch = ctx.cache_dir(&format!("bench/{}", crate::codec::dict_name(class))).map_err(|e| e.to_string())?;
    let files = write_files(&scratch.join("raw"), assets)?;
    let raw: u64 = assets.iter().map(|a| a.data.len() as u64).sum();
    let mut rows = vec![Row { name: "raw (runtime bytes)".into(), bytes: raw, note: String::new() }];
    eprintln!("[bench] {:?}: {} files, {raw} B", class, assets.len());
    for l in 1..=3 {
        let n = ["", "LZ4", "aPLib", "Shrinkler"][l as usize];
        rows.push(Row { name: format!("mkasset -c {l} ({n}, 256 KiB window)"), bytes: mkasset(ctx, &files, l, &scratch)?, note: String::new() });
    }
    rows.push(Row {
        name: "zstd -19 --ultra".into(),
        bytes: tool_sizes(&files, ctx.jobs, |f| {
            let mut c = Command::new("zstd");
            c.args(["-19", "--ultra", "-q", "-c"]).arg(f);
            c
        })?,
        note: "host reference".into(),
    });
    let zd = scratch.join("zstd.dict");
    let zstd_dict = crate::tools::run(Command::new("zstd").args(["--train", "-q", "--maxdict=65536", "-o"]).arg(&zd).args(&files)).ok().map(|_| ());
    if zstd_dict.is_some() {
        let dsz = std::fs::metadata(&zd).map_err(|e| e.to_string())?.len();
        let b = tool_sizes(&files, ctx.jobs, |f| {
            let mut c = Command::new("zstd");
            c.args(["-19", "--ultra", "-q", "-c", "-D"]).arg(&zd).arg(f);
            c
        })?;
        rows.push(Row { name: "zstd -19 + trained 64 KiB dictionary".into(), bytes: b + dsz, note: format!("incl. {dsz} B dictionary; host reference") });
    }
    rows.push(Row {
        name: "xz -9e (.lzma)".into(),
        bytes: tool_sizes(&files, ctx.jobs, |f| {
            let mut c = Command::new("xz");
            c.args(["--format=lzma", "-9e", "-c"]).arg(f);
            c
        })?,
        note: "host reference".into(),
    });
    rows.push(Row {
        name: "brotli -q 11 -w 24".into(),
        bytes: tool_sizes(&files, ctx.jobs, |f| {
            let mut c = Command::new("brotli");
            c.args(["-q", "11", "-w", "24", "-c"]).arg(f);
            c
        })?,
        note: "host reference".into(),
    });

    let opt = Options::level(9);
    let plain: Vec<Vec<u8>> = crate::tools::par_map(assets, ctx.jobs, |a: &RawAsset| compress(&a.data, &[], None, &opt));
    let pb: u64 = plain.iter().map(|z| z.len() as u64).sum();
    rows.push(Row {
        name: "F2Z (LZ + rANS, no transform, no dictionary)".into(),
        bytes: pb,
        note: format!("host decode {:.0} MB/s", f2z_decode_speed(&plain, None)),
    });
    let tr: Vec<Vec<u8>> = crate::tools::par_map(assets, ctx.jobs, |a: &RawAsset| compress_asset(a, None, &opt).0);
    let tb: u64 = tr.iter().map(|z| z.len() as u64).sum();
    rows.push(Row { name: "F2Z + structure transforms".into(), bytes: tb, note: format!("host decode {:.0} MB/s", f2z_decode_speed(&tr, None)) });
    // xz on the transformed bytes: how much of the gain is the transform itself
    let tfiles: Vec<PathBuf> = assets
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let segs = candidates(a).into_iter().next().unwrap_or_default();
            let t = if segs.is_empty() { a.data.clone() } else { f2_n64z::transform::forward(&segs, &a.data) };
            let p = scratch.join("raw").join(format!("{i:04}.t"));
            std::fs::write(&p, t).unwrap();
            p
        })
        .collect();
    rows.push(Row {
        name: "xz -9e on the transformed bytes".into(),
        bytes: tool_sizes(&tfiles, ctx.jobs, |f| {
            let mut c = Command::new("xz");
            c.args(["--format=lzma", "-9e", "-c"]).arg(f);
            c
        })?,
        note: "host reference".into(),
    });
    for size in [32usize << 10, 64 << 10, 128 << 10] {
        let cfg = ClassCodec { dict_size: size, ..ClassCodec::for_class(class) };
        let dict = train_dict(assets, &cfg);
        let zs: Vec<Vec<u8>> = crate::tools::par_map(assets, ctx.jobs, |a: &RawAsset| compress_asset(a, dict.as_ref(), &opt).0);
        let db = dict.as_ref().map_or(0, |d| compress(&d.to_bytes(), &[], None, &opt).len() as u64);
        let b: u64 = zs.iter().map(|z| z.len() as u64).sum::<u64>() + db;
        rows.push(Row {
            name: format!("F2Z + transforms + {} KiB class dictionary", size >> 10),
            bytes: b,
            note: format!("incl. {db} B compressed dictionary; host decode {:.0} MB/s", f2z_decode_speed(&zs, dict.as_ref())),
        });
    }
    let _ = writeln!(md, "\n### {:?} ({} files)\n\n| Codec | Bytes | Ratio | Notes |\n|---|---:|---:|---|", class, assets.len());
    for r in &rows {
        let _ = writeln!(md, "| {} | {} | {:.3} | {} |", r.name, r.bytes, r.bytes as f64 / raw as f64, r.note);
    }
    Ok(())
}

pub fn run(ctx: &Ctx) -> Result<()> {
    let mut md = String::from("## Codec benchmark (generated by `f2-n64pack --bench`)\n");
    let (lv, _) = levels::convert(ctx)?;
    bench_class(ctx, Class::Levels, &lv, &mut md)?;
    let (mo, _) = models::convert(ctx)?;
    bench_class(ctx, Class::Models, &mo, &mut md)?;
    let (tx, _) = textures::convert(ctx)?;
    bench_class(ctx, Class::Textures, &tx, &mut md)?;
    println!("{md}");
    let p = ctx.out.parent().unwrap_or(Path::new(".")).join("bench.md");
    std::fs::write(&p, &md).map_err(|e| e.to_string())?;
    eprintln!("wrote {}", p.display());
    Ok(())
}
