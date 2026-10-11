//! Textures, fonts and backdrops → RDP formats in `NTX1` files.
//!
//! [`convert`] reads `game/textures` (14 banks, 23 backdrops, loose BMPs) and the object
//! banks (to learn which textures are UV-mapped), and returns one uncompressed
//! [`RawAsset`] per runtime file (the driver compresses them with the class codec):
//!
//! | pack path | content |
//! |---|---|
//! | `textures/<bank>.ntx` | one per PC bank (`garden`, `system`, `font`, …): images used only by this bank, the CRC entries in bank order, the animation table, and references to shared pools |
//! | `textures/shared/s<hash>.ntx` | **shared pools**: images used by ≥ 2 banks, one file per distinct *set of banks* using them (see "Dedupe") |
//! | `textures/backdrop/<name>.ntx` | full-screen backdrops (`title`, `loading`, `artwork00`–`20`) at 320×240 |
//! | `textures/<dir>/<name>.ntx` | loose `.bmp` files (`font/`, `procdata/`) that are *not* identical to the same-CRC texture of the matching bank (identical ones are aliases: load the bank entry) |
//!
//! # `NTX1` file layout
//! Big-endian (the N64's VR4300 runs big-endian). All section offsets are from the start of
//! the file and 8-aligned; every image's texels are 8-aligned (DMA and `LOAD_BLOCK`/
//! `LOAD_TILE` friendly) and every TLUT starts on a 4-entry (8-byte) boundary. Sections
//! appear in this order; palette-index texels sit together at the end, apart from the TLUTs,
//! which helps the compressor.
//!
//! **Header** (64 bytes):
//! ```text
//! 0x00 [u8;4] magic "NTX1"          0x14 u32 off_images   (n_images × 32 B)
//! 0x04 u16 version = 1              0x18 u32 off_entries  (n_entries × 24 B)
//! 0x06 u16 file_kind                0x1C u32 off_sorted   (n_sorted × u16)
//!          0 bank, 1 shared pool,   0x20 u32 off_anims    (variable)
//!          2 backdrop, 3 single     0x24 u32 off_deps     (n_deps × 32 B)
//! 0x08 u16 n_images                 0x28 u32 off_glyphs   (n_glyph_tables × 1024 B)
//! 0x0A u16 n_entries                0x2C u32 off_tlut     (u16 RGBA16 entries)
//! 0x0C u16 n_sorted                 0x30 u32 tlut_entries
//! 0x0E u16 n_anims                  0x34 u32 off_pixels
//! 0x10 u16 n_deps                   0x38 u32 pixel_bytes
//! 0x12 u16 n_glyph_tables           0x3C u32 file_size
//! ```
//! **Image descriptor** (32 bytes):
//! ```text
//! 0x00 u8  format   0 RGBA16, 1 RGBA32, 2 CI4, 3 CI8, 4 IA4, 5 IA8, 6 I4, 7 I8
//! 0x01 u8  kind     0 normal, 1 alpha-intensity (ai_), 2 procedural (prc), 3 no-texture,
//!                   4 alpha-colour (ac_), 5 font, 6 backdrop      (→ combiner, below)
//! 0x02 u16 flags    bit0 ALPHA (some texel alpha < 255)   bit1 ALPHA_1BIT (alpha ∈ {0,255})
//!                   bit2 WHITE (alpha mask: colour = 1.0, intensity = alpha)
//!                   bit3 SINGLE_TILE (whole image fits TMEM: wrap/mirror OK)
//!                   bit4 LOSSY  bit5 DOWNSCALED  bit6 POW2 (both dims powers of two)
//!                   bit7 SOURCE_555 (RGBA16 texels = raw PC X1R5G5B5 << 1 | opaque)
//!                   bit8 TLUT_PER_TILE (CI8, one 256-entry palette per tile, row-major)
//! 0x04 u16 width, 0x06 u16 height             stored size
//! 0x08 u16 orig_width, 0x0A u16 orig_height   PC size (UV scale = width / orig_width)
//! 0x0C u16 stride   bytes per stored row (multiple of 8; 16 for RGBA32)
//! 0x0E u16 tile_w, 0x10 u16 tile_h   largest TMEM load (= width/height if SINGLE_TILE)
//! 0x12 u16 tlut_first4  first TLUT entry / 4 (palettes are 8-byte aligned)
//! 0x14 u16 tlut_count   TLUT entries (0: none; 256 × tiles with TLUT_PER_TILE)
//! 0x16 u16 aux      glyph table index, 0xFFFF = none
//! 0x18 u32 pixel_offset (from off_pixels, 8-aligned)   0x1C u32 pixel_size = stride × height
//! ```
//! **Entry** (24 bytes; in PC bank order, duplicates kept):
//! ```text
//! 0x00 u32 crc      CRC of the lower-case "name.bmp" (f2_formats::crc)
//! 0x04 u16 image    image index inside the file selected by `dep`
//! 0x06 u8  dep      0 = this file, k = deps[k-1]
//! 0x07 u8  flags    bit0 = animation base (anim != 0xFFFF)
//! 0x08 [u8;12] name PC bank name, NUL padded (procedural effects are chosen from it)
//! 0x14 u16 anim     last animation record whose base_crc == crc, else 0xFFFF
//! 0x16 u16 0
//! ```
//! **Sorted index**: `u16` entry indices ordered by CRC, one per distinct CRC — the *last*
//! entry with that CRC (the PC loader replaces on duplicates). The runtime's
//! `GetTexEntryFromCRC` is a binary search over it (padded to 8 bytes).
//!
//! **Animations**: the PC table verbatim, in order: `{u32 base_crc; u32 n_frames;
//! n_frames × {u32 frame_crc; i32 time}}` (`time` in game frames, −1 = every frame). The
//! runtime must apply `LoadTexBankFile`'s rule against its *global* registry (all banks
//! loaded so far): for each record in order, look the base up; stop at the first unknown
//! base; frames with unknown CRCs show nothing. `UpdateAnimatingTextures` is unchanged.
//!
//! **Deps**: 32-byte NUL-padded paths relative to `textures/` (e.g. `shared/s1a2b3c4d.ntx`).
//!
//! **Glyph tables** (fonts): 256 × `{u16 left; u16 width}` per Latin-1 byte, in stored
//! pixels: `(0xFFFF, 0)` = no glyph, `(0xFFFF, w)` = space (advance `w`, nothing drawn).
//! This is `InitFont`'s column scan precomputed (`source::glyph_table`), so the runtime
//! never scans; draw glyph `c` by loading the sub-rectangle `[left, left+width) × [0, h)`
//! (4bpp loads round `left` down / the end up to even texels, as libdragon does).
//!
//! **TLUT blob**: RGBA16 entries; CI4 palettes are always 16 entries (load at palette 0..15
//! of the upper TMEM half), CI8 palettes `tlut_count` (≤ 256, padded to a multiple of 4)
//! entries. Identical palettes inside a file are stored once. Palettes are sorted by
//! luminance (smoother index planes).
//!
//! # RDP formats and the combiner per kind
//! Texel encodings are listed in [`rdp`]. Decoding rules (what [`NtxFile::decode_rgba8`]
//! implements and the renderer must reproduce):
//! * **normal / no-texture / font / backdrop** (`RGBA16`, `CI4`/`CI8` + RGBA16 TLUT,
//!   `RGBA32`, `IA4`/`IA8`, or opaque `I4`/`I8`): `colour = TEX0 × SHADE`,
//!   `alpha = TEX0 × SHADE` — except opaque `I4`/`I8` (no `ALPHA` flag), whose alpha input
//!   is `SHADE` only (the RDP sets I-format alpha = intensity). `ALPHA_1BIT` images (the PC
//!   magenta colour key `0x7C1F`) need only alpha compare (threshold 0.5); the PC bleeds a
//!   neighbour colour into transparent texels and so do the stored texels, so bilinear
//!   filtering shows no magenta fringes. No-texture images are opaque.
//! * **alpha-intensity (`ai_`, flag `WHITE`)**: the PC uploads white with
//!   `A = (blue5 >> 1) × 17`; stored as `I4` (exact: 16 levels). Combiner:
//!   `colour = SHADE` (texel colour replaced by 1.0), `alpha = TEX0 × SHADE`.
//! * **procedural (`prc`, flag `SOURCE_555`)**: stored as RGBA16 carrying the exact PC
//!   pixels (`src = texel >> 1`). The game builds its 256-entry ARGB4444 palette from them
//!   (`create_procedural_texture`) and simulates an 8-bit 32×32 buffer; the runtime blits
//!   it through the palette into a dynamic 32×32 `RGBA32` surface (exactly 4 KB of TMEM)
//!   or, cheaper, a CI8 + RGBA16-TLUT surface (1-bit alpha). Until the first update it
//!   shows the PC's initial upload: white with `A = (blue5 >> 1) × 17` (as `ai_`).
//! * **alpha-colour (`ac_`)**: PC uses ARGB4444 with `A = mean(r4, g4, b4)`; stored
//!   `RGBA32` (exact) when it fits, `TEX0 × SHADE`. (No `ac_` texture exists in the retail
//!   data.)
//!
//! # Format choice (per unique image)
//! The *target* is what the PC displays (`source::target`: the D3D upload rules incl. the
//! transparent-texel colour bleed, `InitFont`'s colour key for fonts). Then:
//! 1. **Lossless first**: every format whose decode equals the target bit-exactly is a
//!    candidate (RGBA16 always is for 5551-representable sources; CI4/CI8 when ≤ 16/256
//!    distinct RGBA16 colours; I/IA for exact greys; I4 for `ai_`); UV-mapped images must
//!    also fit TMEM whole. The smallest (texels + TLUT bytes) wins.
//! 2. **Otherwise lossy** (only UV-mapped images that no lossless format fits whole):
//!    every format at every power-of-two downscale `(w >> sx, h >> sy)`, `sx, sy ≤ 3`,
//!    that fits TMEM (only the least-downscaled fitting sizes per format) is encoded
//!    (Lanczos-3 downscale in premultiplied alpha with wrapped edges; palettes by median
//!    cut + k-means; no dithering on tiling textures), decoded, bilinear-upsampled back and
//!    scored; the **highest PSNR wins** (TMEM, not ROM, is the binding constraint).
//! 3. [`Policy::near_lossless_db`] (off by default) lets lossy candidates at or above a
//!    PSNR compete with lossless ones on size, for budget tuning.
//!
//! **Metric** ([`image::weighted_mse`]): per texel `Σ_rgb k_c·Δc²·cov + Δa²`, with
//! `k = 3·(0.299, 0.587, 0.114)` (luma weights) and `cov = 0.25 + 0.75·max(a, a')/255`
//! (a transparent texel's colour still matters a little through bilinear filtering);
//! PSNR = `10·log10(255² / (Σ / 4N))`. Backdrops are additionally scored after a [1 2 1]²
//! low-pass ([`image::psnr_lowpass`]), which approximates viewing distance and is what
//! decides dithering.
//!
//! # TMEM, downscaling, tiles
//! TMEM is 4 KB; CI formats (TLUT in the upper half) and RGBA32 (split halves) get 2 KB of
//! texel space; each row takes `ceil(row bytes / 8) × 8` bytes (libdragon `rdpq_tex_fits`).
//! * UV-mapped textures (model faces, environment maps — found by scanning every
//!   `objects/*.dll`) must fit as one tile (`SINGLE_TILE`); a downscaled one keeps its
//!   `orig_*` size: the runtime maps PC UVs (0..1 over `orig`) to texels with the *stored*
//!   size (`s = u × width`), so UVs stay valid.
//! * Everything else (2D, frontend, HUD, sprites, particles on quads, fonts, backdrops)
//!   may be larger than TMEM: texels stay one contiguous surface and `tile_w × tile_h` is
//!   the largest rectangle that fits one load (`rdp::tile_shape`). Draw it as a grid of
//!   `ceil(w / tile_w) × ceil(h / tile_h)` sub-rectangles (each an affine piece of the
//!   quad, so rotated/scaled sprites work too); with bilinear filtering overlap tiles by one
//!   texel (libdragon's `rdpq_tex_blit` does all of this from the surface).
//!
//! # Dedupe
//! Images are keyed by `(kind, width, height, PC pixels)`; a key used by several CRCs or
//! banks is converted once (as UV-mapped if any user is). An image used by one bank lives
//! in that bank; an image used by ≥ 2 banks lives in the pool for *exactly that set of
//! banks* (`shared/s<fnv32 of the sorted bank names>.ntx`). A bank lists the pools it needs
//! in `deps`; entries point into them. Loading a bank = load the bank file + its deps (each
//! pool once, ref-counted); no pool contains an image the bank does not use, so the RAM cost
//! equals the undeduplicated one while the ROM stores every image once.
//!
//! # Backdrops
//! 640×480 → 320×240 (Lanczos-3, clamped edges), then seven encodings are measured against
//! the 8-bit downscaled reference ([`backdrop_variants`]): RGBA16 with no / ordered 4×4 /
//! Floyd–Steinberg dither, CI8 with one global k-means palette (no / FS) and CI8 with one
//! 256-entry palette per 40×48 tile (`TLUT_PER_TILE`; no / FS). The smallest variant whose
//! low-pass PSNR is within [`Policy::backdrop_margin_db`] (1 dB) of the best is kept.
//! Measured on the USA data (23 backdrops; mean PSNR / mean low-pass PSNR; total raw bytes
//! and, as a compressibility proxy, `zstd -19` bytes):
//!
//! | variant | PSNR | low-pass | raw | zstd -19 |
//! |---|---|---|---|---|
//! | RGBA16 / none | 44.2 | 51.1 | 3.53 MB | 1.28 MB |
//! | RGBA16 / ordered | 40.9 | 50.8 | 3.53 MB | 1.39 MB |
//! | RGBA16 / FS | 43.0 | 54.1 | 3.53 MB | 1.38 MB |
//! | CI8 global / none | 39.5 | 45.8 | 1.78 MB | 0.77 MB |
//! | CI8 global / FS | 37.6 | 47.9 | 1.78 MB | 0.85 MB |
//! | CI8 per-tile / none | 43.2 | 50.2 | 2.24 MB | 1.18 MB |
//! | CI8 per-tile / FS | 42.0 | 52.3 | 2.24 MB | 1.23 MB |
//!
//! Dithering costs ~1 dB of plain PSNR but gains ~3 dB low-pass (no banding in skies) and
//! matches the console's 16-bit look, so FS wins; a global 256-colour palette is clearly
//! worse; per-tile palettes reach RGBA16 quality on most images at ~60% of the raw bytes.
//! Result: 17 × RGBA16/FS, 6 × CI8 per-tile/FS. Backdrops are 2D tiles; per-tile-palette
//! images must be drawn unfiltered at 1:1 (a neighbour tile's indices use another palette),
//! which is how backdrops are drawn anyway.
//!
//! # Fonts and loose BMPs
//! `font.fla` holds the six `InitFont` strips (identical to `textures/font/*.bmp`, which
//! are therefore aliases). Each strip is converted losslessly under the font colour key
//! (CI4 when ≤ 16 colours incl. transparent, else CI8/RGBA16) with its glyph table and a
//! full-height tile (`tile_h = height`; every glyph is narrower than `tile_w`, checked
//! by [`verify`]). `procdata/*.bmp` are identical to `procdata.fla` entries (aliases).

pub mod image;
pub mod ntx;
pub mod rdp;
pub mod source;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use f2_formats::texture::Texture;

use crate::{tools, Class, Ctx, RawAsset, Result};
use image::{Dither, Rgba};
pub use ntx::NtxFile;
use ntx::{file_kind, flag, kind, Entry, ImageMeta, NONE};
use rdp::{Encoded, Fmt};
use source::Sources;

/// Tunables of the conversion.
#[derive(Clone, Debug)]
pub struct Policy {
    /// When set, lossy candidates at or above this PSNR (dB) compete with lossless ones
    /// on size. `None` (default): lossless whenever a lossless format fits.
    pub near_lossless_db: Option<f64>,
    /// Maximum power-of-two downscale per axis for UV-mapped textures that do not fit.
    pub max_downscale_log2: u32,
    /// Backdrops: keep the smallest variant whose low-pass PSNR is at most this far below
    /// the best variant's.
    pub backdrop_margin_db: f64,
    /// Backdrop output size (larger backdrops are Lanczos-downscaled to it).
    pub backdrop_size: (usize, usize),
}

impl Default for Policy {
    fn default() -> Self {
        Policy { near_lossless_db: None, max_downscale_log2: 3, backdrop_margin_db: 1.0, backdrop_size: (320, 240) }
    }
}

/// Measured statistics of one conversion.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    /// Texture instances in banks (incl. duplicates).
    pub textures: usize,
    /// Distinct images after dedupe.
    pub unique: usize,
    pub per_format: BTreeMap<&'static str, usize>,
    /// Raw PC pixel bytes (16 bpp) of every bank texture, backdrop and loose BMP.
    pub raw_before: u64,
    /// Bytes of every produced file (uncompressed).
    pub raw_after: u64,
    pub downscaled: usize,
    /// 2D images larger than one TMEM load (drawn as tiles).
    pub split: usize,
    /// PSNR (dB) of every lossy image (bank textures and backdrops).
    pub lossy_psnr: Vec<(String, f64)>,
    /// Texture instances that reused an image stored once.
    pub dedupe_refs: usize,
    /// Converted bytes those instances would have cost.
    pub dedupe_saved: u64,
    pub pools: usize,
    pub loose_aliased: usize,
}

/// Result of [`convert_sources`].
#[derive(Clone, Debug)]
pub struct Output {
    pub assets: Vec<RawAsset>,
    pub notes: Vec<String>,
    pub stats: Stats,
}

/// Converts every texture bank, backdrop and font of the rendition.
pub fn convert(ctx: &Ctx) -> Result<(Vec<RawAsset>, Vec<String>)> {
    let src = Sources::load(&ctx.game())?;
    let out = convert_sources(&src, &Policy::default(), ctx.jobs)?;
    Ok((out.assets, out.notes))
}

/// One converted image.
#[derive(Clone, Debug)]
pub struct Converted {
    pub enc: Encoded,
    pub meta: ImageMeta,
    pub lossy: bool,
    /// PSNR against the full-size target (infinite when lossless).
    pub psnr: f64,
    /// log2 downscale per axis.
    pub scale: (u32, u32),
}

impl Converted {
    pub fn tiles(&self) -> usize {
        self.enc.w.div_ceil(self.meta.tile.0) * self.enc.h.div_ceil(self.meta.tile.1)
    }

    fn describe(&self) -> String {
        let mut s = format!("{} {}x{}", self.enc.fmt.name(), self.enc.w, self.enc.h);
        if self.scale != (0, 0) {
            s += &format!(" (from {}x{})", self.meta.orig_width, self.meta.orig_height);
        }
        if self.tiles() > 1 {
            s += &format!(" {} tiles of {}x{}", self.tiles(), self.meta.tile.0, self.meta.tile.1);
        }
        if self.lossy {
            s += &format!(" lossy {:.1} dB", self.psnr);
        }
        s
    }
}

/// Picks a format (and size) for a target image; see the module docs.
pub fn choose(target: &Rgba, white: bool, single_tile: bool, policy: &Policy) -> (Encoded, f64, (u32, u32)) {
    let (w, h) = (target.w, target.h);
    let mut lossless: Option<Encoded> = None;
    for fmt in Fmt::ALL {
        if single_tile && !fmt.fits(w, h) {
            continue;
        }
        if let Some(e) = rdp::encode_exact(fmt, target, white) {
            if lossless.as_ref().is_none_or(|b| e.bytes() < b.bytes()) {
                lossless = Some(e);
            }
        }
    }
    if policy.near_lossless_db.is_none() {
        if let Some(e) = lossless {
            return (e, f64::INFINITY, (0, 0));
        }
    }
    let src_f: Vec<[f32; 4]> = target.px.iter().map(|p| p.map(f32::from)).collect();
    let maxs = if single_tile { policy.max_downscale_log2 } else { 0 };
    let mut scaled: HashMap<(u32, u32), Vec<[f32; 4]>> = HashMap::new();
    let mut cands: Vec<(Encoded, f64, (u32, u32))> = Vec::new();
    for fmt in Fmt::ALL {
        // Least-downscaled fitting sizes only (Pareto-minimal (sx, sy)).
        let mut fitting: Vec<(u32, u32)> = Vec::new();
        for sx in 0..=maxs {
            for sy in 0..=maxs {
                let (nw, nh) = (w >> sx, h >> sy);
                if nw == 0 || nh == 0 || ((sx, sy) != (0, 0) && (nw < 4 || nh < 4)) {
                    continue;
                }
                if !single_tile || fmt.fits(nw, nh) {
                    fitting.push((sx, sy));
                }
            }
        }
        let minimal: Vec<(u32, u32)> = fitting.iter().copied().filter(|&(a, b)| !fitting.iter().any(|&(c, d)| (c, d) != (a, b) && c <= a && d <= b)).collect();
        for s in minimal {
            let (nw, nh) = (w >> s.0, h >> s.1);
            let img = scaled.entry(s).or_insert_with(|| if s == (0, 0) { src_f.clone() } else { image::resize_f(target, nw, nh, single_tile) });
            let Some(e) = rdp::encode_lossy(fmt, nw, nh, img, white, Dither::None) else { continue };
            let dec = e.decode(white);
            let cmp = if s == (0, 0) { dec } else { image::upsample_bilinear(&dec, w, h, single_tile) };
            let p = image::psnr(target, &cmp);
            cands.push((e, p, s));
        }
    }
    if let Some(t) = policy.near_lossless_db {
        let mut pool: Vec<(Encoded, f64, (u32, u32))> = cands.iter().filter(|c| c.1 >= t).cloned().collect();
        if let Some(e) = lossless {
            pool.push((e, f64::INFINITY, (0, 0)));
        }
        if let Some(best) = pool.into_iter().min_by(|a, b| a.0.bytes().cmp(&b.0.bytes()).then(b.1.total_cmp(&a.1))) {
            return best;
        }
    }
    cands.into_iter().max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.bytes().cmp(&a.0.bytes()))).expect("no candidate format: image too small or unsupported")
}

fn alpha_flags(target: &Rgba, white: bool) -> u16 {
    let mut f = 0;
    if white || !target.opaque() {
        f |= flag::ALPHA;
    }
    if target.alpha_1bit() {
        f |= flag::ALPHA_1BIT;
    }
    if white {
        f |= flag::WHITE;
    }
    f
}

/// Converts one bank/loose image of `kind` (`single_tile`: UV-mapped).
pub fn convert_image(t: &Texture, kind: u8, single_tile: bool, policy: &Policy) -> Converted {
    let (w, h) = (t.width as usize, t.height as usize);
    if kind == kind::PROCEDURAL {
        let enc = rdp::pack(Fmt::Rgba16, w, h, &source::source_codes(t));
        let mut flags = flag::SOURCE_555 | flag::ALPHA;
        if enc.fmt.fits(w, h) {
            flags |= flag::SINGLE_TILE;
        }
        let tile = rdp::tile_shape(enc.fmt, w, h);
        let meta = ImageMeta { kind, flags, orig_width: t.width, orig_height: t.height, tile, glyphs: None };
        return Converted { enc, meta, lossy: false, psnr: f64::INFINITY, scale: (0, 0) };
    }
    let target = source::target(t, kind);
    let white = kind == kind::ALPHA_INTENSITY;
    let single_tile = single_tile && kind != kind::FONT;
    let (enc, psnr, scale) = choose(&target, white, single_tile, policy);
    let lossy = psnr.is_finite();
    let mut flags = alpha_flags(&target, white);
    if lossy {
        flags |= flag::LOSSY;
    }
    if scale != (0, 0) {
        flags |= flag::DOWNSCALED;
    }
    if enc.fmt.fits(enc.w, enc.h) {
        flags |= flag::SINGLE_TILE;
    }
    let tile = if kind == kind::FONT { rdp::tile_shape_full_height(enc.fmt, enc.w, enc.h) } else { rdp::tile_shape(enc.fmt, enc.w, enc.h) };
    let glyphs = (kind == kind::FONT).then(|| source::glyph_table(t));
    let meta = ImageMeta { kind, flags, orig_width: t.width, orig_height: t.height, tile, glyphs };
    Converted { enc, meta, lossy, psnr, scale }
}

/// Measurements of one backdrop conversion: `(variant, PSNR, low-pass PSNR, bytes)`.
pub type BackdropReport = Vec<(String, f64, f64, usize)>;

/// One measured backdrop encoding.
#[derive(Clone, Debug)]
pub struct BackdropVariant {
    pub name: String,
    pub enc: Encoded,
    pub psnr: f64,
    pub psnr_lowpass: f64,
}

/// Encodes a backdrop every candidate way and scores each against the 8-bit downscaled
/// reference: RGBA16 (no / ordered / Floyd–Steinberg dither), CI8 with one global palette
/// (no / FS) and CI8 with per-tile palettes (no / FS).
pub fn backdrop_variants(t: &Texture, policy: &Policy) -> ((usize, usize), Vec<BackdropVariant>) {
    let target = source::target(t, kind::BACKDROP);
    let (bw, bh) = policy.backdrop_size;
    let (nw, nh) = if target.w > bw || target.h > bh { (bw, bh) } else { (target.w, target.h) };
    let reff = if (nw, nh) == (target.w, target.h) { target.px.iter().map(|p| p.map(f32::from)).collect() } else { image::resize_f(&target, nw, nh, false) };
    let ref8 = image::round_img(nw, nh, &reff);
    let mut variants: Vec<(String, Encoded)> = Vec::new();
    for (name, d) in [("none", Dither::None), ("ordered", Dither::Ordered), ("fs", Dither::FloydSteinberg)] {
        variants.push((format!("RGBA16/{name}"), rdp::encode_lossy(Fmt::Rgba16, nw, nh, &reff, false, d).unwrap()));
    }
    let pal = rdp::ci_palette(Fmt::Ci8, &reff);
    let ptile = rdp::palette_tile_shape(nw, nh);
    for (name, d) in [("none", Dither::None), ("fs", Dither::FloydSteinberg)] {
        variants.push((format!("CI8/{name}"), rdp::encode_ci_with(Fmt::Ci8, nw, nh, &reff, &pal, d)));
        variants.push((format!("CI8-tiles/{name}"), rdp::encode_ci8_tiled(nw, nh, &reff, ptile, d)));
    }
    let out = variants
        .into_iter()
        .map(|(name, enc)| {
            let dec = enc.decode(false);
            let (psnr, psnr_lowpass) = (image::psnr(&ref8, &dec), image::psnr_lowpass(&ref8, &dec));
            BackdropVariant { name, enc, psnr, psnr_lowpass }
        })
        .collect();
    ((target.w, target.h), out)
}

/// Converts a full-screen backdrop: the smallest variant whose low-pass PSNR is within
/// [`Policy::backdrop_margin_db`] of the best one (see the module docs).
pub fn convert_backdrop(t: &Texture, policy: &Policy) -> (Converted, BackdropReport) {
    let ((ow, oh), vars) = backdrop_variants(t, policy);
    let best_lp = vars.iter().map(|v| v.psnr_lowpass).fold(f64::MIN, f64::max);
    let pick = vars
        .iter()
        .filter(|v| v.psnr_lowpass >= best_lp - policy.backdrop_margin_db)
        .min_by(|a, b| a.enc.bytes().cmp(&b.enc.bytes()).then(b.psnr_lowpass.total_cmp(&a.psnr_lowpass)))
        .unwrap();
    let report = vars.iter().map(|v| (v.name.clone(), v.psnr, v.psnr_lowpass, v.enc.bytes())).collect();
    let enc = pick.enc.clone();
    let (nw, nh) = (enc.w, enc.h);
    let mut flags = flag::LOSSY;
    if (nw, nh) != (ow, oh) {
        flags |= flag::DOWNSCALED;
    }
    if enc.fmt.fits(nw, nh) {
        flags |= flag::SINGLE_TILE;
    }
    let tile = enc.palette_tile.unwrap_or_else(|| rdp::tile_shape(enc.fmt, nw, nh));
    let meta = ImageMeta { kind: kind::BACKDROP, flags, orig_width: t.width, orig_height: t.height, tile, glyphs: None };
    let scale = ((ow / nw).trailing_zeros(), (oh / nh).trailing_zeros());
    (Converted { enc, meta, lossy: true, psnr: pick.psnr, scale }, report)
}

/// Location of a unique image in the output.
#[derive(Clone, Debug)]
enum Home {
    Bank,
    Pool(String),
}

struct Unique<'a> {
    kind: u8,
    tex: &'a Texture,
    single_tile: bool,
    banks: BTreeSet<String>,
    refs: usize,
}

fn pool_path(banks: &BTreeSet<String>) -> String {
    let joined = banks.iter().cloned().collect::<Vec<_>>().join("+");
    format!("shared/s{:08x}.ntx", tools::fnv64(joined.as_bytes()) as u32)
}

/// Converts already-loaded sources (the testable core of [`convert`]).
pub fn convert_sources(src: &Sources, policy: &Policy, jobs: usize) -> Result<Output> {
    let mut stats = Stats::default();
    let mut notes = src.notes.clone();

    // 1. Unique images over all banks.
    let mut uniq: Vec<Unique> = Vec::new();
    let mut by_key: HashMap<(u8, u16, u16, &[u16]), usize> = HashMap::new();
    let mut bank_refs: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (bname, (bank, _)) in &src.banks {
        let font = bname == source::FONT_BANK;
        let refs = bank_refs.entry(bname).or_default();
        for t in &bank.textures {
            let k = source::kind_of(t, font);
            stats.textures += 1;
            stats.raw_before += t.pixels.len() as u64 * 2;
            let key = (k, t.width, t.height, &t.pixels[..]);
            let i = *by_key.entry(key).or_insert_with(|| {
                uniq.push(Unique { kind: k, tex: t, single_tile: false, banks: BTreeSet::new(), refs: 0 });
                uniq.len() - 1
            });
            let u = &mut uniq[i];
            u.single_tile |= src.used_3d.contains(&t.crc);
            u.banks.insert(bname.clone());
            u.refs += 1;
            refs.push(i);
        }
    }
    stats.unique = uniq.len();

    // 2. Convert them.
    let conv: Vec<Converted> = tools::par_map(&uniq, jobs, |u| convert_image(u.tex, u.kind, u.single_tile, policy));
    for (u, c) in uniq.iter().zip(&conv) {
        *stats.per_format.entry(c.enc.fmt.name()).or_default() += 1;
        if c.scale != (0, 0) {
            stats.downscaled += 1;
        }
        if c.tiles() > 1 {
            stats.split += 1;
        }
        if c.lossy {
            stats.lossy_psnr.push((u.tex.name.clone(), c.psnr));
        }
        if u.refs > 1 {
            stats.dedupe_refs += u.refs - 1;
            stats.dedupe_saved += (u.refs as u64 - 1) * c.enc.bytes() as u64;
        }
    }

    // 3. Homes: single-bank images stay local; others go to the pool of their bank set.
    let homes: Vec<Home> = uniq.iter().map(|u| if u.banks.len() == 1 { Home::Bank } else { Home::Pool(pool_path(&u.banks)) }).collect();
    let mut pools: BTreeMap<String, (NtxFile, Vec<String>)> = BTreeMap::new();
    let mut pool_index: HashMap<usize, u16> = HashMap::new();
    for (i, h) in homes.iter().enumerate() {
        if let Home::Pool(p) = h {
            let e = pools.entry(p.clone()).or_insert_with(|| (NtxFile::new(file_kind::POOL), Vec::new()));
            pool_index.insert(i, e.0.add_image(&conv[i].enc, &conv[i].meta));
            e.1.push(format!("{} {}", uniq[i].tex.name, conv[i].describe()));
        }
    }
    let mut assets: Vec<RawAsset> = Vec::new();
    for (path, (file, lines)) in &pools {
        let users = uniq
            .iter()
            .zip(&homes)
            .find(|(_, h)| matches!(h, Home::Pool(p) if p == path))
            .map(|(u, _)| u.banks.iter().cloned().collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        let mut n = vec![format!("shared pool for banks [{users}]: {} images", file.images.len())];
        n.extend(lines.iter().cloned());
        assets.push(RawAsset { path: format!("textures/{path}"), class: Class::Textures, data: file.to_bytes(), source_bytes: 0, notes: n });
    }
    stats.pools = pools.len();

    // 4. Bank files.
    for (bname, (bank, src_size)) in &src.banks {
        let refs = &bank_refs[bname.as_str()];
        let mut file = NtxFile::new(file_kind::BANK);
        let mut local: HashMap<usize, u16> = HashMap::new();
        let mut deps: Vec<String> =
            refs.iter().filter_map(|&i| if let Home::Pool(p) = &homes[i] { Some(p.clone()) } else { None }).collect::<BTreeSet<_>>().into_iter().collect();
        deps.sort();
        let mut lines = Vec::new();
        let mut fmt_count: BTreeMap<&str, usize> = BTreeMap::new();
        for (t, &i) in bank.textures.iter().zip(refs) {
            let (image, dep, where_) = match &homes[i] {
                Home::Bank => {
                    let idx = *local.entry(i).or_insert_with(|| {
                        *fmt_count.entry(conv[i].enc.fmt.name()).or_default() += 1;
                        file.add_image(&conv[i].enc, &conv[i].meta)
                    });
                    (idx, 0u8, String::new())
                }
                Home::Pool(p) => (pool_index[&i], (deps.iter().position(|d| d == p).unwrap() + 1) as u8, format!(" [{p}]")),
            };
            file.entries.push(Entry { crc: t.crc, image, dep, flags: 0, name: ntx::name12(&t.name), anim: NONE });
            let use_ = if uniq[i].single_tile { "uv" } else { "2d" };
            lines.push(format!("{:08x} {:<12} {use_} {}x{} -> {}{where_}", t.crc, t.name, t.width, t.height, conv[i].describe()));
        }
        file.anims = bank.anims.clone();
        for (ai, a) in bank.anims.iter().enumerate() {
            for e in file.entries.iter_mut().filter(|e| e.crc == a.base_crc) {
                e.anim = ai as u16;
                e.flags |= 1;
            }
        }
        file.deps = deps;
        file.sort_entries();
        let mut n = vec![format!(
            "{} textures, {} local images ({}), {} shared via {} pools, {} animations",
            bank.textures.len(),
            file.images.len(),
            fmt_count.iter().map(|(k, v)| format!("{v} {k}")).collect::<Vec<_>>().join(", "),
            file.entries.iter().filter(|e| e.dep != 0).count(),
            file.deps.len(),
            bank.anims.len()
        )];
        n.extend(lines);
        assets.push(RawAsset { path: format!("textures/{bname}.ntx"), class: Class::Textures, data: file.to_bytes(), source_bytes: *src_size, notes: n });
    }

    // 5. Backdrops.
    let bd: Vec<(&String, &(Texture, u64))> = src.backdrops.iter().collect();
    let bconv = tools::par_map(&bd, jobs, |(_, (t, _))| convert_backdrop(t, policy));
    let mut bd_report: BTreeMap<String, (f64, f64, f64, usize)> = BTreeMap::new();
    for ((name, (t, size)), (c, report)) in bd.iter().zip(&bconv) {
        stats.raw_before += t.pixels.len() as u64 * 2;
        *stats.per_format.entry(c.enc.fmt.name()).or_default() += 1;
        stats.downscaled += usize::from(c.scale != (0, 0));
        stats.split += usize::from(c.tiles() > 1);
        stats.lossy_psnr.push((format!("backdrop/{name}"), c.psnr));
        for (v, p, lp, b) in report {
            let e = bd_report.entry(v.clone()).or_insert((f64::INFINITY, 0.0, 0.0, 0));
            e.0 = e.0.min(*lp);
            e.1 += p;
            e.2 += lp;
            e.3 += b;
        }
        let mut file = NtxFile::new(file_kind::BACKDROP);
        file.add_image(&c.enc, &c.meta);
        let mut n = vec![format!("{}x{} -> {}", t.width, t.height, c.describe())];
        n.extend(report.iter().map(|(v, p, lp, b)| format!("  {v}: PSNR {p:.2} dB, low-pass {lp:.2} dB, {b} B")));
        assets.push(RawAsset { path: format!("textures/backdrop/{name}.ntx"), class: Class::Textures, data: file.to_bytes(), source_bytes: *size, notes: n });
    }

    // 6. Loose BMPs: aliases of identical bank entries, else single-image files.
    for (rel, t, size) in &src.loose {
        let dir = rel.split('/').next().unwrap_or("");
        stats.raw_before += t.pixels.len() as u64 * 2;
        let same = src
            .banks
            .get(dir)
            .and_then(|(b, _)| b.textures.iter().rev().find(|bt| bt.crc == t.crc))
            .is_some_and(|bt| bt.width == t.width && bt.height == t.height && bt.pixels == t.pixels);
        if same {
            stats.loose_aliased += 1;
            notes.push(format!("{rel}: identical to the {:08x} entry of textures/{dir}.ntx (alias, no file)", t.crc));
            continue;
        }
        let k = if dir == source::FONT_BANK { kind::FONT } else { kind::NORMAL };
        let c = convert_image(t, k, src.used_3d.contains(&t.crc), policy);
        *stats.per_format.entry(c.enc.fmt.name()).or_default() += 1;
        let mut file = NtxFile::new(file_kind::SINGLE);
        let image = file.add_image(&c.enc, &c.meta);
        file.entries.push(Entry { crc: t.crc, image, dep: 0, flags: 0, name: ntx::name12(&t.name), anim: NONE });
        file.sort_entries();
        let stem = rel.trim_end_matches(".bmp");
        assets.push(RawAsset {
            path: format!("textures/{stem}.ntx"),
            class: Class::Textures,
            data: file.to_bytes(),
            source_bytes: *size,
            notes: vec![format!("{}x{} -> {}", t.width, t.height, c.describe())],
        });
    }

    // 7. Class notes.
    stats.raw_after = assets.iter().map(|a| a.data.len() as u64).sum();
    let lossy_bank: Vec<f64> = stats.lossy_psnr.iter().filter(|(n, _)| !n.starts_with("backdrop/")).map(|p| p.1).collect();
    let lossy_bd: Vec<f64> = stats.lossy_psnr.iter().filter(|(n, _)| n.starts_with("backdrop/")).map(|p| p.1).collect();
    let mm = |v: &[f64]| {
        if v.is_empty() {
            "n/a".to_string()
        } else {
            format!("min {:.1} / mean {:.1} dB", v.iter().copied().fold(f64::INFINITY, f64::min), v.iter().sum::<f64>() / v.len() as f64)
        }
    };
    notes.push(format!(
        "textures: {} bank textures -> {} unique images ({} instances deduplicated, {} B of converted texels saved, {} shared pools)",
        stats.textures, stats.unique, stats.dedupe_refs, stats.dedupe_saved, stats.pools
    ));
    notes.push(format!(
        "formats (unique images + backdrops + loose): {}",
        stats.per_format.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ")
    ));
    notes.push(format!(
        "raw bytes: {} (PC 16 bpp) -> {} (NTX, before compression), {:.1}%",
        stats.raw_before,
        stats.raw_after,
        100.0 * stats.raw_after as f64 / stats.raw_before.max(1) as f64
    ));
    notes.push(format!("downscaled {} (UV-mapped textures over TMEM + backdrops), split into TMEM tiles {}", stats.downscaled, stats.split));
    notes.push(format!(
        "lossy bank textures: {} ({}); backdrops: {} ({}) [perceptual PSNR, module docs]",
        lossy_bank.len(),
        mm(&lossy_bank),
        lossy_bd.len(),
        mm(&lossy_bd)
    ));
    for (n, p) in stats.lossy_psnr.iter().filter(|(n, _)| !n.starts_with("backdrop/")) {
        notes.push(format!("  lossy: {n} {p:.1} dB"));
    }
    if !bd_report.is_empty() {
        let n = src.backdrops.len() as f64;
        notes.push("backdrop variants (mean PSNR / mean low-pass PSNR / min low-pass / total bytes):".into());
        for (v, (minlp, p, lp, b)) in &bd_report {
            notes.push(format!("  {v}: {:.2} / {:.2} / {:.2} dB, {b} B", p / n, lp / n, minlp));
        }
    }
    if stats.loose_aliased > 0 {
        notes.push(format!("loose BMPs aliased to bank entries: {}", stats.loose_aliased));
    }
    Ok(Output { assets, notes, stats })
}

/// Counts from [`verify`].
#[derive(Clone, Debug, Default)]
pub struct VerifyReport {
    pub entries: usize,
    pub exact: usize,
    pub lossy: usize,
    pub single_tile_checked: usize,
}

/// Re-reads produced files and checks them against the sources: every bank texture
/// resolves by CRC (through deps) to an image of the right kind/size; lossless images
/// decode bit-exactly to the PC target (procedural ones to the exact source pixels);
/// UV-mapped images fit TMEM whole; every image's tile fits TMEM; animation tables are
/// preserved; backdrops parse.
pub fn verify(src: &Sources, assets: &[RawAsset]) -> Result<VerifyReport> {
    let mut files: HashMap<String, NtxFile> = HashMap::new();
    for a in assets {
        let f = NtxFile::parse(&a.data).map_err(|e| format!("{}: {e}", a.path))?;
        for (i, d) in f.images.iter().enumerate() {
            if !d.fmt.fits(d.tile_w as usize, d.tile_h as usize) || d.tile_w == 0 || d.tile_h == 0 {
                return Err(format!("{} image {i}: tile {}x{} does not fit TMEM as {}", a.path, d.tile_w, d.tile_h, d.fmt.name()));
            }
            if (d.flags & flag::SINGLE_TILE != 0) != d.fmt.fits(d.width as usize, d.height as usize) {
                return Err(format!("{} image {i}: SINGLE_TILE flag inconsistent", a.path));
            }
        }
        files.insert(a.path.trim_start_matches("textures/").to_string(), f);
    }
    let mut rep = VerifyReport::default();
    for (bname, (bank, _)) in &src.banks {
        let f = files.get(&format!("{bname}.ntx")).ok_or_else(|| format!("missing textures/{bname}.ntx"))?;
        if f.anims != bank.anims {
            return Err(format!("{bname}: animation table differs"));
        }
        if f.entries.len() != bank.textures.len() {
            return Err(format!("{bname}: entry count differs"));
        }
        let font = bname == source::FONT_BANK;
        for (t, e) in bank.textures.iter().zip(&f.entries) {
            rep.entries += 1;
            if e.crc != t.crc || e.name != ntx::name12(&t.name) {
                return Err(format!("{bname}/{}: entry mismatch", t.name));
            }
            let last = bank.textures.iter().rposition(|x| x.crc == t.crc).unwrap();
            if f.find(t.crc) != Some(&f.entries[last]) {
                return Err(format!("{bname}/{}: CRC lookup does not return the last duplicate", t.name));
            }
            let (img_file, idx) = if e.dep == 0 {
                (f, e.image as usize)
            } else {
                (files.get(&f.deps[e.dep as usize - 1]).ok_or_else(|| format!("{bname}: missing dep {}", f.deps[e.dep as usize - 1]))?, e.image as usize)
            };
            let d = img_file.images.get(idx).ok_or_else(|| format!("{bname}/{}: image index out of range", t.name))?;
            let k = source::kind_of(t, font);
            if d.kind != k || d.orig_width != t.width || d.orig_height != t.height {
                return Err(format!("{bname}/{}: kind/size mismatch", t.name));
            }
            if src.used_3d.contains(&t.crc) && k != kind::FONT {
                rep.single_tile_checked += 1;
                if d.flags & flag::SINGLE_TILE == 0 {
                    return Err(format!("{bname}/{}: UV-mapped but not a single TMEM tile", t.name));
                }
            }
            if d.flags & flag::LOSSY != 0 {
                rep.lossy += 1;
                continue;
            }
            if (d.width, d.height) != (t.width, t.height) {
                return Err(format!("{bname}/{}: lossless image with a different size", t.name));
            }
            if k == kind::PROCEDURAL {
                let s = img_file.source_555(idx).ok_or_else(|| format!("{bname}/{}: procedural without source texels", t.name))?;
                if s.iter().zip(&t.pixels).any(|(a, b)| *a != b & 0x7fff) {
                    return Err(format!("{bname}/{}: procedural source differs", t.name));
                }
            }
            if img_file.decode_rgba8(idx) != source::target(t, k) {
                return Err(format!("{bname}/{}: lossless image does not decode to the PC target", t.name));
            }
            if k == kind::FONT {
                let g = img_file.glyphs.get(d.aux as usize).ok_or_else(|| format!("{bname}/{}: font without glyph table", t.name))?;
                if *g != source::glyph_table(t) {
                    return Err(format!("{bname}/{}: glyph table differs", t.name));
                }
                if let Some(c) = g.iter().position(|e| e.0 != NONE && e.1 > d.tile_w) {
                    return Err(format!("{bname}/{}: glyph {c} wider than one TMEM tile", t.name));
                }
            }
            rep.exact += 1;
        }
    }
    for name in src.backdrops.keys() {
        let f = files.get(&format!("backdrop/{name}.ntx")).ok_or_else(|| format!("missing backdrop {name}"))?;
        if f.images.len() != 1 {
            return Err(format!("backdrop {name}: expected one image"));
        }
    }
    Ok(rep)
}

/// Debug aid: writes every image of `assets` as a PAM (RGBA) file under `dir` for
/// eyeballing (never commit the output: it is derived from game data). Returns the count.
pub fn write_previews(assets: &[RawAsset], dir: &Path) -> Result<usize> {
    let mut n = 0;
    for a in assets {
        let f = NtxFile::parse(&a.data).map_err(|e| format!("{}: {e}", a.path))?;
        let stem = a.path.trim_start_matches("textures/").trim_end_matches(".ntx").replace('/', "_");
        for i in 0..f.images.len() {
            let img = f.decode_rgba8(i);
            let mut out = format!("P7\nWIDTH {}\nHEIGHT {}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n", img.w, img.h).into_bytes();
            out.extend(img.to_bytes());
            let p = dir.join(format!("{stem}_{i:03}.pam"));
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            std::fs::write(&p, out).map_err(|e| format!("{}: {e}", p.display()))?;
            n += 1;
        }
    }
    Ok(n)
}
