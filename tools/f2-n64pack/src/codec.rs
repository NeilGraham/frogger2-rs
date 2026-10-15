//! Per-class compression: trains the class dictionary, compresses every [`RawAsset`] with
//! F2Z (best of its transform candidates), verifies the round trip with the `no_std`
//! decoder and writes the files into the pack.
//!
//! Pack conventions the N64 loader must follow:
//! * Every file written here is an F2Z stream (`"F2Z1"` magic) of the asset's runtime
//!   bytes; `header()` gives the decoded size, `dict_id` names the dictionary.
//! * Dictionaries live at `dict/<class>.f2zd`, themselves F2Z-compressed without a
//!   dictionary; decode once at boot and keep resident (`Dict::parse` borrows the bytes).

use f2_n64z::dict::EncDict;
use f2_n64z::enc::{compress, Options};
use f2_n64z::train::{train, Sample};
use f2_n64z::{header, Decoder, Dict, Segment};

use crate::{Class, ClassReport, Ctx, PackedFile, RawAsset, Result};

/// Default F2Z space–speed weight: smallest output (F2Z is used where size wins; see
/// `research/n64-compression.md` §5 for the measured trade-off curve).
pub const DEFAULT_SIZE_WEIGHT: f64 = f64::INFINITY;

/// Codec of a class's files in the pack. The N64 loader dispatches on the file magic:
/// `"F2Z1"` → F2Z (`f2_n64z::Decoder`), `"DCA"` → libdragon `asset_load`, else raw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    F2z,
    /// libdragon `mkasset -c N -w 256` (0 = stored, 1 = LZ4, 2 = aPLib, 3 = Shrinkler).
    Mkasset(u8),
}

impl Codec {
    pub fn parse(s: &str) -> Option<Codec> {
        Some(match s {
            "f2z" => Codec::F2z,
            "none" => Codec::Mkasset(0),
            "lz4" => Codec::Mkasset(1),
            "aplib" => Codec::Mkasset(2),
            "shrinkler" => Codec::Mkasset(3),
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Codec::F2z => "f2z",
            Codec::Mkasset(0) => "none",
            Codec::Mkasset(1) => "lz4",
            Codec::Mkasset(2) => "aplib",
            Codec::Mkasset(_) => "shrinkler",
        }
    }

    /// Measured winners (ratio among codecs decoding ≥ ~3.5 MB/s on the VR4300 in ares, see
    /// `research/n64-compression.md` §5): aPLib for models and textures (libdragon's MIPS
    /// assembly decoder: 5.6 / 3.5 MB/s incl. ROM read); F2Z (smallest) for level data, where
    /// it is 31 % smaller than aPLib and a level's files decode in ~30 ms.
    pub fn default_for(c: Class) -> Codec {
        match c {
            Class::Models | Class::Textures => Codec::Mkasset(2),
            _ => Codec::F2z,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ClassCodec {
    /// Dictionary id (unique per class, non-zero) and content size (0 = no dictionary).
    pub dict_id: u8,
    pub dict_size: usize,
    pub level: u32,
    /// F2Z space–speed weight (`f2_n64z::enc::Options::size_weight`): decode cycles a stored
    /// byte is worth. `LOAD_TIME_WEIGHT` minimises ROM read + decode time; ∞ = smallest.
    pub size_weight: f64,
    pub codec: Codec,
}

impl ClassCodec {
    pub fn for_class(c: Class) -> ClassCodec {
        match c {
            Class::Models => ClassCodec { dict_id: 1, dict_size: 128 << 10, level: 9, size_weight: DEFAULT_SIZE_WEIGHT, codec: Codec::default_for(c) },
            Class::Textures => ClassCodec { dict_id: 2, dict_size: 128 << 10, level: 9, size_weight: DEFAULT_SIZE_WEIGHT, codec: Codec::default_for(c) },
            Class::Levels => ClassCodec { dict_id: 3, dict_size: 64 << 10, level: 9, size_weight: DEFAULT_SIZE_WEIGHT, codec: Codec::default_for(c) },
            _ => ClassCodec { dict_id: 4, dict_size: 32 << 10, level: 9, size_weight: DEFAULT_SIZE_WEIGHT, codec: Codec::default_for(c) },
        }
    }
}

pub fn dict_name(c: Class) -> &'static str {
    match c {
        Class::Models => "models",
        Class::Textures => "textures",
        Class::Levels => "levels",
        _ => "misc",
    }
}

/// Transform candidates for an asset: its own segments, plus class-generic ones.
pub fn candidates(a: &RawAsset) -> Vec<Vec<Segment>> {
    let mut c = Vec::new();
    if !a.segments.is_empty() {
        c.push(a.segments.clone());
    }
    if a.class == Class::Models {
        c.extend(crate::models::candidate_segments(&a.data));
    }
    if a.class == Class::Textures {
        c.extend(ntx_segments(&a.data));
    }
    c.push(Vec::new());
    c.dedup();
    c
}

/// Transform candidates for an NTX1 texture file: record arrays as byte planes, TLUT
/// entries split into high/low byte planes; pixel data raw or as planes of 2 (RGBA16 rows).
pub fn ntx_segments(d: &[u8]) -> Vec<Vec<Segment>> {
    let be32 = |o: usize| d.get(o..o + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()) as usize);
    if d.get(..4) != Some(b"NTX1") {
        return Vec::new();
    }
    let offs: Option<Vec<usize>> = [0x14, 0x18, 0x1C, 0x20, 0x24, 0x28, 0x2C, 0x34].iter().map(|&o| be32(o)).collect();
    let Some(o) = offs else { return Vec::new() };
    // sections in file order: images, entries, sorted, anims, deps, glyphs, tlut, pixels
    let mut bounds = o.clone();
    bounds.push(d.len());
    if bounds[0] < 64 || bounds.windows(2).any(|w| w[0] > w[1]) || *bounds.last().unwrap() > d.len() {
        return Vec::new();
    }
    let strides: [u16; 8] = [32, 24, 2, 4, 1, 4, 2, 1];
    let mk = |pix_planes: bool| {
        let mut v = vec![Segment::raw(bounds[0] as u32)];
        for i in 0..8 {
            let len = (bounds[i + 1] - bounds[i]) as u32;
            let st = if i == 7 && pix_planes { 2 } else { strides[i] };
            v.push(if st > 1 { Segment::planes(st, len) } else { Segment::raw(len) });
        }
        v
    };
    vec![mk(false), mk(true)]
}

/// Compresses with the best candidate; returns (stream, index of the candidate used).
/// With a finite space–speed weight the inverse-transform decode cost counts too.
pub fn compress_asset(a: &RawAsset, dict: Option<&EncDict>, opt: &Options) -> (Vec<u8>, usize) {
    let mut best: Option<(Vec<u8>, usize, f64)> = None;
    for (i, segs) in candidates(a).iter().enumerate() {
        let z = compress(&a.data, segs, dict, opt);
        let score = if opt.size_weight.is_finite() {
            let tbytes: u64 = segs.iter().filter(|s| s.kind != f2_n64z::transform::Kind::Raw).map(|s| u64::from(s.len)).sum();
            z.len() as f64 * opt.size_weight + tbytes as f64 * f2_n64z::enc::TRANSFORM_CYCLES_PER_BYTE
        } else {
            z.len() as f64
        };
        if best.as_ref().is_none_or(|b| score < b.2) {
            best = Some((z, i, score));
        }
    }
    let (z, i, _) = best.unwrap();
    (z, i)
}

/// Decodes `z` with the `no_std` decoder and checks it equals `data`.
pub fn verify(z: &[u8], dict: Option<&EncDict>, data: &[u8]) -> Result<()> {
    let h = header(z).map_err(|e| e.to_string())?;
    let dbytes = dict.map(EncDict::to_bytes);
    let d = dbytes.as_ref().map(|b| Dict::parse(b)).transpose().map_err(|e| e.to_string())?;
    let mut out = vec![0u8; h.raw_size as usize];
    let mut tmp = vec![0u8; if h.needs_scratch() { h.raw_size as usize } else { 0 }];
    let mut dec = Box::new(Decoder::new());
    dec.decode(z, d.as_ref(), &mut out, &mut tmp).map_err(|e| e.to_string())?;
    if out != data {
        return Err("F2Z round trip mismatch".into());
    }
    Ok(())
}

/// Trains a dictionary for the class from its assets.
pub fn train_dict(assets: &[RawAsset], cfg: &ClassCodec) -> Option<EncDict> {
    if cfg.dict_size == 0 || assets.is_empty() {
        return None;
    }
    let segs: Vec<Vec<Segment>> = assets.iter().map(|a| candidates(a).into_iter().next().unwrap_or_default()).collect();
    let samples: Vec<Sample> = assets.iter().zip(&segs).map(|(a, s)| Sample { data: &a.data, segs: s }).collect();
    Some(train(&samples, cfg.dict_size, cfg.dict_id, &Options::level(5)))
}

/// Compresses a whole class into the pack.
/// Compresses each asset with libdragon `mkasset` (the runtime loads it with `asset_load`).
fn pack_mkasset(ctx: &Ctx, class: Class, assets: &[RawAsset], level: u8) -> Result<ClassReport> {
    let t0 = std::time::Instant::now();
    let tmp = ctx.cache_dir(&format!("mkasset/{}", dict_name(class))).map_err(|e| e.to_string())?;
    let idx: Vec<usize> = (0..assets.len()).collect();
    // aPLib (apultra) and Shrinkler need hundreds of MB per process: bound the parallelism
    // to stay inside the 8 GB memory cap.
    let jobs = match level {
        3 => ctx.jobs.min(6),
        // in process (portable): ~0.4 GB each, inside the desktop app
        2 if ctx.portable => ctx.jobs.min(4),
        2 => ctx.jobs.min(8),
        _ => ctx.jobs,
    };
    let results = crate::tools::par_map(&idx, jobs, |&i| -> Result<Vec<u8>> {
        let a = &assets[i];
        if ctx.portable {
            // The same bytes as `mkasset -c 2 -w 256`, in process (crate::dca).
            return match level {
                2 => Ok(crate::dca::mkasset_aplib(&a.data, 256 * 1024)),
                _ => Err(format!("{}: mkasset level {level} has no pure-Rust encoder (portable pack: aPLib only)", a.path)),
            };
        }
        let dir = tmp.join(format!("{i:04}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("out")).map_err(|e| e.to_string())?;
        let src = dir.join("asset.bin");
        std::fs::write(&src, &a.data).map_err(|e| e.to_string())?;
        crate::tools::run(std::process::Command::new(ctx.tool("mkasset")).args(["-c", &level.to_string(), "-w", "256", "-o"]).arg(dir.join("out")).arg(&src))
            .map_err(|e| format!("{}: {e}", a.path))?;
        let z = std::fs::read(dir.join("out/asset.bin")).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_dir_all(&dir);
        Ok(z)
    });
    let mut rep = ClassReport::default();
    let (mut raw, mut comp) = (0u64, 0u64);
    let name = Codec::Mkasset(level).name();
    for (a, z) in assets.iter().zip(results) {
        let z = z?;
        std::fs::write(ctx.out_path(&a.path).map_err(|e| e.to_string())?, &z).map_err(|e| e.to_string())?;
        raw += a.data.len() as u64;
        comp += z.len() as u64;
        let mut notes = a.notes.clone();
        notes.push(format!("raw {} B", a.data.len()));
        rep.files.push(PackedFile {
            path: a.path.clone(),
            class,
            bytes: z.len() as u64,
            source_bytes: a.source_bytes,
            format: format!("mkasset {name}"),
            notes,
        });
    }
    rep.notes.push(format!(
        "{}: {} files, {raw} B runtime bytes -> {comp} B mkasset {name} = {:.1}%, {:.1}s",
        dict_name(class),
        assets.len(),
        100.0 * comp as f64 / raw.max(1) as f64,
        t0.elapsed().as_secs_f64()
    ));
    Ok(rep)
}

pub fn pack_class(ctx: &Ctx, class: Class, assets: &[RawAsset], cfg: &ClassCodec) -> Result<ClassReport> {
    if let Codec::Mkasset(l) = cfg.codec {
        return pack_mkasset(ctx, class, assets, l);
    }
    let t0 = std::time::Instant::now();
    let dict = train_dict(assets, cfg);
    let opt = Options { size_weight: cfg.size_weight, ..Options::level(cfg.level) };
    let results = crate::tools::par_map(assets, ctx.jobs, |a: &RawAsset| -> Result<(Vec<u8>, usize)> {
        let (z, cand) = compress_asset(a, dict.as_ref(), &opt);
        verify(&z, dict.as_ref(), &a.data).map_err(|e| format!("{}: {e}", a.path))?;
        Ok((z, cand))
    });
    let mut rep = ClassReport::default();
    let (mut raw, mut comp) = (0u64, 0u64);
    for (a, r) in assets.iter().zip(results) {
        let (z, cand) = r?;
        std::fs::write(ctx.out_path(&a.path).map_err(|e| e.to_string())?, &z).map_err(|e| e.to_string())?;
        raw += a.data.len() as u64;
        comp += z.len() as u64;
        let mut notes = a.notes.clone();
        notes.push(format!("raw {} B, transform candidate {cand}", a.data.len()));
        rep.files.push(PackedFile { path: a.path.clone(), class, bytes: z.len() as u64, source_bytes: a.source_bytes, format: "f2z".into(), notes });
    }
    if let Some(d) = &dict {
        let db = d.to_bytes();
        let z = compress(&db, &[], None, &opt);
        verify(&z, None, &db)?;
        let path = format!("dict/{}.f2zd", dict_name(class));
        std::fs::write(ctx.out_path(&path).map_err(|e| e.to_string())?, &z).map_err(|e| e.to_string())?;
        rep.files.push(PackedFile {
            path,
            class,
            bytes: z.len() as u64,
            source_bytes: 0,
            format: "f2z dictionary".into(),
            notes: vec![format!("{} B content + default tables = {} B resident in RAM", d.content.len(), db.len())],
        });
        comp += z.len() as u64;
    }
    rep.notes.push(format!(
        "{}: {} files, {raw} B runtime bytes -> {comp} B F2Z (incl. dictionary) = {:.1}%, {:.1}s",
        dict_name(class),
        assets.len(),
        100.0 * comp as f64 / raw.max(1) as f64,
        t0.elapsed().as_secs_f64()
    ));
    Ok(rep)
}
