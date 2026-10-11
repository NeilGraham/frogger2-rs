//! Pack driver and cartridge budget (`docs/N64-PLAN.md` §2).
//!
//! Order: fixed-quality classes first (levels, models, textures, sound effects), then the
//! remaining space is split between music and FMV, which are tuned to fill the cartridge:
//! FMV is capped by the N64 H.264 decoder's bitrate limit, music by audioconv64's Opus
//! ceiling, so when both fit at maximum the rest is reported as headroom.

use std::collections::BTreeMap;
use std::process::Command;

use serde::Serialize;

use crate::codec::{pack_class, ClassCodec};
use crate::{audio, levels, rmodels, textures, video, Class, ClassReport, Ctx, PackedFile, RawAsset, Result};

pub const MIB: f64 = 1_048_576.0;

#[derive(Clone, Debug)]
pub struct BudgetConfig {
    pub cart_bytes: u64,
    /// Space kept for IPL3 + the compressed ELF.
    pub code_reserve: u64,
    /// Restrict to some steps (development).
    pub only: Option<Vec<String>>,
    /// F2Z space–speed weight for every F2Z class.
    pub size_weight: f64,
    /// Per-class codec overrides (`--codec models=f2z`).
    pub codecs: Vec<(Class, crate::codec::Codec)>,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        BudgetConfig { cart_bytes: 64 << 20, code_reserve: (2.5 * MIB) as u64, only: None, size_weight: crate::codec::DEFAULT_SIZE_WEIGHT, codecs: Vec::new() }
    }
}

impl BudgetConfig {
    fn wants(&self, step: &str) -> bool {
        self.only.as_ref().is_none_or(|o| o.iter().any(|s| s == step))
    }
}

/// Plan §2 targets (bytes), for the comparison column.
pub fn plan_target(c: Class) -> u64 {
    let mb = |x: f64| (x * 1e6) as u64;
    match c {
        Class::Code => mb(2.0),
        Class::Models => mb(5.0),
        Class::Textures => mb(6.0),
        Class::Levels => mb(1.5),
        Class::Sfx => mb(8.0),
        Class::Music => mb(7.5),
        Class::Fmv => mb(25.0),
        Class::Meta => 0,
    }
}

#[derive(Serialize)]
struct Manifest<'a> {
    rendition: String,
    cart_bytes: u64,
    code_reserve: u64,
    dfs_bytes: u64,
    total_bytes: u64,
    fits: bool,
    classes: BTreeMap<String, ClassSummary>,
    notes: &'a [String],
    files: &'a [PackedFile],
}

#[derive(Serialize, Default, Clone)]
struct ClassSummary {
    bytes: u64,
    files: usize,
    source_bytes: u64,
    plan_target: u64,
}

fn compressed(ctx: &Ctx, cfg: &BudgetConfig, class: Class, step: (Vec<RawAsset>, Vec<String>)) -> Result<ClassReport> {
    let (assets, notes) = step;
    let codec = cfg.codecs.iter().rev().find(|c| c.0 == class).map_or(crate::codec::Codec::default_for(class), |c| c.1);
    let mut r = pack_class(ctx, class, &assets, &ClassCodec { size_weight: cfg.size_weight, codec, ..ClassCodec::for_class(class) })?;
    r.notes.splice(0..0, notes);
    Ok(r)
}

fn sum(files: &[PackedFile], c: Class) -> u64 {
    files.iter().filter(|f| f.class == c).map(|f| f.bytes).sum()
}

/// DFS image size and per-file overhead, measured with libdragon's `mkdfs`.
fn measure_dfs(ctx: &Ctx) -> Result<u64> {
    let img = ctx.out.with_extension("dfs");
    crate::tools::run(Command::new(ctx.tool("mkdfs")).arg(&img).arg(&ctx.out))?;
    Ok(std::fs::metadata(&img).map_err(|e| e.to_string())?.len())
}

/// Host-side manifest, next to (not inside) the DFS tree.
pub fn manifest_path(ctx: &Ctx) -> std::path::PathBuf {
    ctx.out.with_extension("manifest.json")
}

/// Builds the pack and prints the budget; `Ok(false)` when it does not fit.
pub fn build(ctx: &Ctx, cfg: &BudgetConfig) -> Result<bool> {
    if ctx.out.exists() {
        let ours = manifest_path(ctx).exists() || std::fs::read_dir(&ctx.out).map(|mut d| d.next().is_none()).unwrap_or(false);
        if !ours {
            return Err(format!("{} exists and is not a pack directory; refusing to overwrite", ctx.out.display()));
        }
        std::fs::remove_dir_all(&ctx.out).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(&ctx.out).map_err(|e| e.to_string())?;
    let mut files: Vec<PackedFile> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut add = |r: ClassReport, files: &mut Vec<PackedFile>| {
        for n in &r.notes {
            eprintln!("  {n}");
        }
        notes.extend(r.notes);
        files.extend(r.files);
    };
    let step = |name: &str| eprintln!("[f2-n64pack] {name}");
    if cfg.wants("levels") {
        step("levels");
        add(compressed(ctx, cfg, Class::Levels, levels::convert(ctx)?)?, &mut files);
    }
    if cfg.wants("models") {
        step("models");
        add(compressed(ctx, cfg, Class::Models, rmodels::convert(ctx)?)?, &mut files);
    }
    if cfg.wants("textures") {
        step("textures");
        add(compressed(ctx, cfg, Class::Textures, textures::convert(ctx)?)?, &mut files);
        add(compressed(ctx, cfg, Class::Textures, rmodels::fonts(ctx)?)?, &mut files);
    }
    if cfg.wants("sfx") {
        step("sound effects");
        // All VADPCM from the start: the N64 front end plays one opened sample on several
        // voices at once, which libdragon's Opus wav64 (one decoder state per file) cannot do.
        add(audio::pack_sfx_with(ctx, &audio::SfxOpts { opus_max_ratio: 0.0 })?, &mut files);
    }
    // Space left for music + FMV (DFS overhead estimated as 64 B per file + 0.1%).
    let fixed: u64 = files.iter().map(|f| f.bytes).sum();
    let overhead_est = 64 * (files.len() as u64 + 64) + fixed / 1000 + 64 * 1024;
    let avail = cfg.cart_bytes.saturating_sub(cfg.code_reserve + fixed + overhead_est);
    if cfg.wants("music") || cfg.wants("fmv") {
        step("music + FMV (filling the remaining space)");
        let (m, f) = media(ctx, cfg, avail)?;
        add(m, &mut files);
        add(f, &mut files);
    }
    if cfg.wants("sfx") && cfg.wants("fmv") {
        step("quality upgrades with the remaining space");
        for n in fill(ctx, cfg, &mut files)? {
            eprintln!("  {n}");
            notes.push(n);
        }
    }
    let dfs = measure_dfs(ctx)?;
    let total = dfs + cfg.code_reserve;
    let fits = total <= cfg.cart_bytes;
    let classes = summarize(&files, dfs, cfg);
    print_table(&classes, dfs, total, cfg);
    let m = Manifest {
        rendition: ctx.rendition.display().to_string(),
        cart_bytes: cfg.cart_bytes,
        code_reserve: cfg.code_reserve,
        dfs_bytes: dfs,
        total_bytes: total,
        fits,
        classes: classes.iter().map(|(c, s)| (format!("{c:?}"), s.clone())).collect(),
        notes: &notes,
        files: &files,
    };
    std::fs::write(manifest_path(ctx), serde_json::to_string_pretty(&m).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    if !fits {
        eprintln!("error: pack ({dfs} B) + code reserve ({} B) = {total} B exceeds the {} B cartridge", cfg.code_reserve, cfg.cart_bytes);
    }
    Ok(fits)
}

/// Spends leftover space on quality, in priority order, re-measuring the real DFS image
/// after each step and reverting a step that does not fit: (1) all sound effects VADPCM
/// (no Opus: better fidelity, near-free RSP decode, exact loops), (2) FMV audio Opus mono
/// 48 kbps / stereo 96 kbps (from 32/64). Music and FMV video are already at their caps.
fn fill(ctx: &Ctx, cfg: &BudgetConfig, files: &mut Vec<PackedFile>) -> Result<Vec<String>> {
    let mut notes = Vec::new();
    let fits = |ctx: &Ctx| -> Result<(bool, u64)> {
        let d = measure_dfs(ctx)?;
        Ok((d + cfg.code_reserve <= cfg.cart_bytes, d))
    };
    let (ok, _) = fits(ctx)?;
    if !ok {
        return Ok(notes);
    }
    let fmv_info = |files: &[PackedFile]| files.iter().find(|f| f.class == Class::Fmv && f.path.ends_with(".h264")).map(|f| f.format.clone());
    let replace = |files: &mut Vec<PackedFile>, c: Class, r: ClassReport| {
        files.retain(|f| f.class != c);
        files.extend(r.files);
        r.notes
    };
    // (1) sound effects: all VADPCM
    let before = sum(files, Class::Sfx);
    let r = audio::pack_sfx_with(ctx, &audio::SfxOpts { opus_max_ratio: 0.0 })?;
    let after = sum(&r.files, Class::Sfx);
    let n = replace(files, Class::Sfx, r);
    if fits(ctx)?.0 {
        notes.push(format!("fill: sound effects all VADPCM ({before} -> {after} B)"));
        notes.extend(n);
    } else {
        let r = audio::pack_sfx(ctx)?;
        replace(files, Class::Sfx, r);
        notes.push(format!("fill: all-VADPCM sound effects do not fit ({after} B), kept Opus for long samples"));
        return Ok(notes);
    }
    // (2) FMV audio bitrate, keeping the chosen video rung
    let Some(label) = fmv_info(files) else { return Ok(notes) };
    let video_budget = sum(files, Class::Fmv);
    for kbps in [(48u32, 96u32), (40, 80)] {
        let extra = video::pack_fmv_with(ctx, u64::MAX, kbps)?;
        let same_rung = fmv_info(&extra.files).as_deref() == Some(label.as_str());
        let new_total = sum(&extra.files, Class::Fmv);
        let n = replace(files, Class::Fmv, extra);
        if same_rung && fits(ctx)?.0 {
            notes.push(format!("fill: FMV audio Opus {} kbps mono / {} kbps stereo ({video_budget} -> {new_total} B)", kbps.0, kbps.1));
            notes.extend(n);
            return Ok(notes);
        }
        let r = video::pack_fmv(ctx, video_budget)?;
        replace(files, Class::Fmv, r);
    }
    Ok(notes)
}

/// Music + FMV: the best quality that fits `avail` bytes.
fn media(ctx: &Ctx, cfg: &BudgetConfig, avail: u64) -> Result<(ClassReport, ClassReport)> {
    const MUSIC_KBPS: [u32; 7] = [99, 96, 80, 64, 48, 40, 32];
    const FMV_FLOOR: u64 = 10_000_000;
    let want_music = cfg.wants("music");
    let want_fmv = cfg.wants("fmv");
    let fmv_max = if want_fmv { video::pack_fmv(ctx, u64::MAX)? } else { ClassReport::default() };
    let fmv_max_bytes = sum(&fmv_max.files, Class::Fmv);
    if !want_music {
        return Ok((ClassReport::default(), if fmv_max_bytes <= avail { fmv_max } else { video::pack_fmv(ctx, avail)? }));
    }
    for (i, &kbps) in MUSIC_KBPS.iter().enumerate() {
        let music = audio::pack_music(ctx, kbps)?;
        let m = sum(&music.files, Class::Music);
        let left = avail.saturating_sub(m);
        let last = i + 1 == MUSIC_KBPS.len();
        if !want_fmv {
            if m <= avail || last {
                return Ok((music, ClassReport::default()));
            }
            continue;
        }
        if fmv_max_bytes <= left {
            let mut f = fmv_max;
            f.notes.push(format!("FMV at maximum quality; {} B of headroom left", left - fmv_max_bytes));
            return Ok((music, f));
        }
        if (left >= FMV_FLOOR && kbps <= 64) || last {
            return Ok((music, video::pack_fmv(ctx, left)?));
        }
    }
    unreachable!()
}

fn summarize(files: &[PackedFile], dfs: u64, cfg: &BudgetConfig) -> Vec<(Class, ClassSummary)> {
    let mut v = Vec::new();
    let mut payload = 0;
    for c in Class::ALL {
        let mut s = ClassSummary { plan_target: plan_target(c), ..Default::default() };
        for f in files.iter().filter(|f| f.class == c) {
            s.bytes += f.bytes;
            s.files += 1;
            s.source_bytes += f.source_bytes;
        }
        payload += s.bytes;
        if c == Class::Code {
            s.bytes = cfg.code_reserve;
        }
        if c == Class::Meta {
            s.bytes += dfs.saturating_sub(payload);
        }
        v.push((c, s));
    }
    v
}

fn print_table(classes: &[(Class, ClassSummary)], dfs: u64, total: u64, cfg: &BudgetConfig) {
    println!("| Asset class | Files | From (retail) | Packed bytes | MiB | Plan target |");
    println!("|---|---:|---:|---:|---:|---:|");
    for (c, s) in classes {
        println!("| {} | {} | {} | {} | {:.2} | {:.1} MB |", c.name(), s.files, s.source_bytes, s.bytes, s.bytes as f64 / MIB, s.plan_target as f64 / 1e6);
    }
    let head = cfg.cart_bytes.saturating_sub(total);
    println!("| **total** (DFS image {dfs} B + code reserve) | | | **{total}** | **{:.2}** | 64 MiB |", total as f64 / MIB);
    println!("| headroom | | | {head} | {:.2} | |", head as f64 / MIB);
}
