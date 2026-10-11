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

#[derive(Clone, Debug)]
pub struct ClassCodec {
    /// Dictionary id (unique per class, non-zero) and content size (0 = no dictionary).
    pub dict_id: u8,
    pub dict_size: usize,
    pub level: u32,
}

impl ClassCodec {
    pub fn for_class(c: Class) -> ClassCodec {
        match c {
            Class::Models => ClassCodec { dict_id: 1, dict_size: 64 << 10, level: 9 },
            Class::Textures => ClassCodec { dict_id: 2, dict_size: 64 << 10, level: 9 },
            Class::Levels => ClassCodec { dict_id: 3, dict_size: 64 << 10, level: 9 },
            _ => ClassCodec { dict_id: 4, dict_size: 32 << 10, level: 9 },
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
    c.push(Vec::new());
    c.dedup();
    c
}

/// Compresses with the best candidate; returns (stream, index of the candidate used).
pub fn compress_asset(a: &RawAsset, dict: Option<&EncDict>, opt: &Options) -> (Vec<u8>, usize) {
    let mut best: Option<(Vec<u8>, usize)> = None;
    for (i, segs) in candidates(a).iter().enumerate() {
        let z = compress(&a.data, segs, dict, opt);
        if best.as_ref().is_none_or(|b| z.len() < b.0.len()) {
            best = Some((z, i));
        }
    }
    best.unwrap()
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
pub fn pack_class(ctx: &Ctx, class: Class, assets: &[RawAsset], cfg: &ClassCodec) -> Result<ClassReport> {
    let t0 = std::time::Instant::now();
    let dict = train_dict(assets, cfg);
    let opt = Options::level(cfg.level);
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
        let z = compress(&db, &[], None, &Options::level(9));
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
