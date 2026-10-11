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
use crate::{audio, levels, models, textures, video, Class, ClassReport, Ctx, PackedFile, RawAsset, Result};

pub const MIB: f64 = 1_048_576.0;

#[derive(Clone, Debug)]
pub struct BudgetConfig {
    pub cart_bytes: u64,
    /// Space kept for IPL3 + the compressed ELF.
    pub code_reserve: u64,
    /// Restrict to some steps (development).
    pub only: Option<Vec<String>>,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        BudgetConfig { cart_bytes: 64 << 20, code_reserve: (2.5 * MIB) as u64, only: None }
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

fn compressed(ctx: &Ctx, class: Class, step: (Vec<RawAsset>, Vec<String>)) -> Result<ClassReport> {
    let (assets, notes) = step;
    let mut r = pack_class(ctx, class, &assets, &ClassCodec::for_class(class))?;
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
        add(compressed(ctx, Class::Levels, levels::convert(ctx)?)?, &mut files);
    }
    if cfg.wants("models") {
        step("models");
        add(compressed(ctx, Class::Models, models::convert(ctx)?)?, &mut files);
    }
    if cfg.wants("textures") {
        step("textures");
        add(compressed(ctx, Class::Textures, textures::convert(ctx)?)?, &mut files);
    }
    if cfg.wants("sfx") {
        step("sound effects");
        add(audio::pack_sfx(ctx)?, &mut files);
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
