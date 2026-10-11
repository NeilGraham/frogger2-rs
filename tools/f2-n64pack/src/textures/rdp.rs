//! RDP texel formats: TMEM accounting, exact and lossy encoders, and the decoder that
//! turns stored texels back into RGBA8 the way the RDP + the prescribed combiner see them.
//!
//! Texel bit layouts (big-endian in RDRAM, first texel in the high bits):
//! * `RGBA16`: `RRRRRGGG GGBBBBBA` (5551); 5→8-bit expansion `(v << 3) | (v >> 2)`.
//! * `RGBA32`: bytes `R G B A`.
//! * `CI4`: 2 indices per byte (high nibble first) into a 16-entry RGBA16 TLUT.
//! * `CI8`: 1 index per byte into an up-to-256-entry RGBA16 TLUT.
//! * `IA4`: `IIIA` per nibble (high nibble first); I expanded by bit replication.
//! * `IA8`: `IIII AAAA`; both ×17.
//! * `I4`: 4-bit intensity per nibble ×17; the RDP sets alpha = intensity.
//! * `I8`: 8-bit intensity; alpha = intensity.

use super::image::{histogram, make_palette, nearest, quantise, Dither, Rgba};

/// RDP texel format of a stored image (the `format` byte of an image descriptor).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Fmt {
    Rgba16 = 0,
    Rgba32 = 1,
    Ci4 = 2,
    Ci8 = 3,
    Ia4 = 4,
    Ia8 = 5,
    I4 = 6,
    I8 = 7,
}

impl Fmt {
    pub const ALL: [Fmt; 8] = [Fmt::Ci4, Fmt::I4, Fmt::Ia4, Fmt::Ci8, Fmt::I8, Fmt::Ia8, Fmt::Rgba16, Fmt::Rgba32];

    pub fn from_u8(v: u8) -> Option<Fmt> {
        Fmt::ALL.into_iter().find(|f| *f as u8 == v)
    }

    pub fn name(self) -> &'static str {
        match self {
            Fmt::Rgba16 => "RGBA16",
            Fmt::Rgba32 => "RGBA32",
            Fmt::Ci4 => "CI4",
            Fmt::Ci8 => "CI8",
            Fmt::Ia4 => "IA4",
            Fmt::Ia8 => "IA8",
            Fmt::I4 => "I4",
            Fmt::I8 => "I8",
        }
    }

    pub fn bits(self) -> usize {
        match self {
            Fmt::Ci4 | Fmt::Ia4 | Fmt::I4 => 4,
            Fmt::Ci8 | Fmt::Ia8 | Fmt::I8 => 8,
            Fmt::Rgba16 => 16,
            Fmt::Rgba32 => 32,
        }
    }

    pub fn is_ci(self) -> bool {
        matches!(self, Fmt::Ci4 | Fmt::Ci8)
    }

    /// Palette size for CI formats.
    pub fn palette_max(self) -> usize {
        match self {
            Fmt::Ci4 => 16,
            Fmt::Ci8 => 256,
            _ => 0,
        }
    }

    /// TMEM bytes available to texels: 2 KB for CI (the TLUT lives in the upper half)
    /// and RGBA32 (split in two 2 KB halves), else 4 KB. Same rule as libdragon
    /// `rdpq_tex_upload` / `rdpq_tex_fits`.
    pub fn tmem_limit(self) -> usize {
        match self {
            Fmt::Rgba32 | Fmt::Ci4 | Fmt::Ci8 => 2048,
            _ => 4096,
        }
    }

    /// TMEM line pitch (bytes, multiple of 8) for a row of `w` texels.
    pub fn tmem_pitch(self, w: usize) -> usize {
        let bytes = (w * self.bits()).div_ceil(8);
        let half = if self == Fmt::Rgba32 { bytes / 2 } else { bytes };
        half.div_ceil(8) * 8
    }

    pub fn tmem_bytes(self, w: usize, h: usize) -> usize {
        self.tmem_pitch(w) * h
    }

    /// Whether a `w`×`h` image (or tile) fits TMEM in one load.
    pub fn fits(self, w: usize, h: usize) -> bool {
        self.tmem_bytes(w, h) <= self.tmem_limit()
    }

    /// RDRAM row stride: row bytes rounded up to 8 (16 for RGBA32, as LOAD_BLOCK needs).
    pub fn stride(self, w: usize) -> usize {
        let a = if self == Fmt::Rgba32 { 16 } else { 8 };
        (w * self.bits()).div_ceil(8).div_ceil(a) * a
    }
}

/// Largest TMEM-sized tile for a 2D image: maximises area over tile widths
/// {w, 512, 256, …, 16} (ties keep the wider tile). `(w, h)` when the image fits whole.
pub fn tile_shape(fmt: Fmt, w: usize, h: usize) -> (usize, usize) {
    if fmt.fits(w, h) {
        return (w, h);
    }
    let mut best = (0usize, 0usize);
    let mut cands = vec![w];
    let mut p = 512;
    while p >= 8 {
        if p < w {
            cands.push(p);
        }
        p /= 2;
    }
    for tw in cands {
        let rows = (fmt.tmem_limit() / fmt.tmem_pitch(tw)).min(h);
        // 4bpp loads need an even number of rows in libdragon's TMEM arrangement only for
        // reuse; keep it simple: any row count ≥ 1.
        if rows == 0 {
            continue;
        }
        if tw * rows > best.0 * best.1 {
            best = (tw, rows);
        }
    }
    best
}

/// Tile for a horizontal strip (fonts): all rows, as many columns as fit (falls back to
/// [`tile_shape`] when even one 8-byte column of all rows does not fit).
pub fn tile_shape_full_height(fmt: Fmt, w: usize, h: usize) -> (usize, usize) {
    if fmt.fits(w, h) {
        return (w, h);
    }
    let pitch = fmt.tmem_limit() / h / 8 * 8;
    let pitch = if fmt == Fmt::Rgba32 { pitch * 2 } else { pitch };
    let tw = (pitch * 8 / fmt.bits()).min(w);
    if tw == 0 {
        return tile_shape(fmt, w, h);
    }
    (tw, h)
}

/// Stored texels of one image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Encoded {
    pub fmt: Fmt,
    pub w: usize,
    pub h: usize,
    /// Bytes per row in `data` (multiple of 8).
    pub stride: usize,
    pub data: Vec<u8>,
    /// RGBA16 TLUT (CI only; CI4 always 16 entries).
    pub tlut: Vec<u16>,
    /// Per-tile palettes (CI8 only): tile `(tw, th)`; tile `i` (row-major grid) uses
    /// `tlut[256 * i .. 256 * (i + 1)]`.
    pub palette_tile: Option<(usize, usize)>,
}

impl Encoded {
    /// Bytes this image costs in a pack file (texels + TLUT).
    pub fn bytes(&self) -> usize {
        self.data.len() + self.tlut.len() * 2
    }
}

#[inline]
pub fn expand5(v: u16) -> u8 {
    let v = (v & 31) as u8;
    (v << 3) | (v >> 2)
}

#[inline]
fn expand3(v: u8) -> u8 {
    let v = v & 7;
    (v << 5) | (v << 2) | (v >> 1)
}

#[inline]
fn to5(v: f32) -> u16 {
    (v.clamp(0.0, 255.0) * 31.0 / 255.0).round() as u16
}

/// RGBA16 code of an RGBA8 colour (rounded; alpha threshold 128).
#[inline]
pub fn rgba16_of(c: [f32; 4]) -> u16 {
    (to5(c[0]) << 11) | (to5(c[1]) << 6) | (to5(c[2]) << 1) | u16::from(c[3] >= 127.5)
}

#[inline]
pub fn rgba16_decode(v: u16) -> [u8; 4] {
    [expand5(v >> 11), expand5(v >> 6), expand5(v >> 1), if v & 1 != 0 { 255 } else { 0 }]
}

/// Exact RGBA16 code of an RGBA8 colour, if representable.
fn rgba16_exact(p: [u8; 4]) -> Option<u16> {
    let c = rgba16_of(fl(p));
    (rgba16_decode(c) == p).then_some(c)
}

#[inline]
fn fl(p: [u8; 4]) -> [f32; 4] {
    p.map(f32::from)
}

#[inline]
fn luma(c: [f32; 4]) -> f32 {
    0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]
}

/// Packs texel codes (`bits` wide) into rows padded to the format's stride.
pub fn pack(fmt: Fmt, w: usize, h: usize, codes: &[u32]) -> Encoded {
    let stride = fmt.stride(w);
    let mut data = vec![0u8; stride * h];
    for y in 0..h {
        let row = &mut data[y * stride..(y + 1) * stride];
        for x in 0..w {
            let v = codes[y * w + x];
            match fmt.bits() {
                4 => row[x / 2] |= ((v & 15) as u8) << if x.is_multiple_of(2) { 4 } else { 0 },
                8 => row[x] = v as u8,
                16 => row[x * 2..x * 2 + 2].copy_from_slice(&(v as u16).to_be_bytes()),
                _ => row[x * 4..x * 4 + 4].copy_from_slice(&v.to_be_bytes()),
            }
        }
    }
    Encoded { fmt, w, h, stride, data, tlut: Vec::new(), palette_tile: None }
}

/// Reads texel code `(x, y)` of an encoded image.
pub fn texel(fmt: Fmt, data: &[u8], stride: usize, x: usize, y: usize) -> u32 {
    let row = &data[y * stride..];
    match fmt.bits() {
        4 => u32::from((row[x / 2] >> if x.is_multiple_of(2) { 4 } else { 0 }) & 15),
        8 => u32::from(row[x]),
        16 => u32::from(u16::from_be_bytes([row[x * 2], row[x * 2 + 1]])),
        _ => u32::from_be_bytes([row[x * 4], row[x * 4 + 1], row[x * 4 + 2], row[x * 4 + 3]]),
    }
}

/// Decodes one texel code to the RGBA8 colour the renderer must produce, given the
/// image's combiner semantics: `white` = the texture is an alpha mask (colour forced to
/// 1.0, I/IA intensity used as alpha; `ai_` textures), otherwise I4/I8 are opaque grey.
pub fn decode_texel(fmt: Fmt, code: u32, tlut: &[u16], white: bool) -> [u8; 4] {
    match fmt {
        Fmt::Rgba16 => rgba16_decode(code as u16),
        Fmt::Rgba32 => code.to_be_bytes(),
        Fmt::Ci4 | Fmt::Ci8 => rgba16_decode(tlut.get(code as usize).copied().unwrap_or(0)),
        Fmt::I4 | Fmt::I8 => {
            let i = if fmt == Fmt::I4 { (code as u8 & 15) * 17 } else { code as u8 };
            if white {
                [255, 255, 255, i]
            } else {
                [i, i, i, 255]
            }
        }
        Fmt::Ia4 => {
            let i = expand3((code >> 1) as u8);
            [i, i, i, if code & 1 != 0 { 255 } else { 0 }]
        }
        Fmt::Ia8 => {
            let i = ((code >> 4) as u8 & 15) * 17;
            [i, i, i, (code as u8 & 15) * 17]
        }
    }
}

/// Decodes a whole encoded image (stored size); `palette_tile` selects per-tile palettes.
#[allow(clippy::too_many_arguments)]
pub fn decode(fmt: Fmt, w: usize, h: usize, stride: usize, data: &[u8], tlut: &[u16], white: bool, palette_tile: Option<(usize, usize)>) -> Rgba {
    let mut px = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let pal = match palette_tile {
                Some((tw, th)) => {
                    let i = (y / th) * w.div_ceil(tw) + x / tw;
                    tlut.get(256 * i..).unwrap_or(&[])
                }
                None => tlut,
            };
            px.push(decode_texel(fmt, texel(fmt, data, stride, x, y), pal, white));
        }
    }
    Rgba::new(w, h, px)
}

impl Encoded {
    pub fn decode(&self, white: bool) -> Rgba {
        decode(self.fmt, self.w, self.h, self.stride, &self.data, &self.tlut, white, self.palette_tile)
    }
}

/// Tile grid for per-tile CI8 palettes: tiles that divide the image exactly, fit TMEM
/// as CI8 (≤ 2 KB) and maximise area; ties prefer the squarest tile.
pub fn palette_tile_shape(w: usize, h: usize) -> (usize, usize) {
    let mut best = (w, h);
    let mut best_key = (0usize, usize::MAX);
    for tw in (8..=w).step_by(8).filter(|tw| w.is_multiple_of(*tw)) {
        let Some(th) = (1..=h).rev().find(|th| h.is_multiple_of(*th) && Fmt::Ci8.fits(tw, *th)) else { continue };
        let key = (tw * th, tw.abs_diff(th));
        if key.0 > best_key.0 || (key.0 == best_key.0 && key.1 < best_key.1) {
            best_key = key;
            best = (tw, th);
        }
    }
    best
}

/// CI8 with one 256-entry palette per `tile` (2D images drawn unfiltered at 1:1, e.g.
/// backdrops): close to RGBA16 quality at about 60% of its bytes.
pub fn encode_ci8_tiled(w: usize, h: usize, src: &[[f32; 4]], tile: (usize, usize), dither: Dither) -> Encoded {
    let (tw, th) = tile;
    let mut idx = vec![0u32; w * h];
    let mut tlut: Vec<u16> = Vec::new();
    for ty in (0..h).step_by(th) {
        for tx in (0..w).step_by(tw) {
            let (sw, sh) = (tw.min(w - tx), th.min(h - ty));
            let sub: Vec<[f32; 4]> = (0..sh).flat_map(|y| (0..sw).map(move |x| (x, y))).map(|(x, y)| src[(ty + y) * w + tx + x]).collect();
            let pal = ci_palette(Fmt::Ci8, &sub);
            let e = encode_ci_with(Fmt::Ci8, sw, sh, &sub, &pal, dither);
            for y in 0..sh {
                for x in 0..sw {
                    idx[(ty + y) * w + tx + x] = texel(Fmt::Ci8, &e.data, e.stride, x, y);
                }
            }
            let base = tlut.len();
            tlut.extend_from_slice(&e.tlut);
            tlut.resize(base + 256, 0);
        }
    }
    let mut e = pack(Fmt::Ci8, w, h, &idx);
    e.tlut = tlut;
    e.palette_tile = Some(tile);
    e
}

/// Builds a CI image from per-texel RGBA16 codes and a palette of codes; the palette is
/// sorted by luminance (then alpha) and indices remapped, which makes index planes
/// smoother and so compress better.
fn ci_from_codes(fmt: Fmt, w: usize, h: usize, texels: &[u16], palette: &[u16]) -> Encoded {
    let mut order: Vec<usize> = (0..palette.len()).collect();
    let key = |c: u16| {
        let d = rgba16_decode(c);
        (d[3], (luma(fl(d)) * 16.0) as u32, c)
    };
    order.sort_by_key(|&i| key(palette[i]));
    let mut remap = vec![0u32; palette.len()];
    for (new, &old) in order.iter().enumerate() {
        remap[old] = new as u32;
    }
    let mut sorted: Vec<u16> = order.iter().map(|&i| palette[i]).collect();
    let pos: std::collections::HashMap<u16, usize> = palette.iter().enumerate().map(|(i, &c)| (c, i)).collect();
    let codes: Vec<u32> = texels.iter().map(|c| remap[pos[c]]).collect();
    let mut e = pack(fmt, w, h, &codes);
    if fmt == Fmt::Ci4 {
        sorted.resize(16, 0);
    } else {
        sorted.resize(sorted.len().div_ceil(4) * 4, 0);
    }
    e.tlut = sorted;
    e
}

/// Lossless encode: `Some` only if decoding gives back `img` exactly (under the same
/// `white` semantics).
pub fn encode_exact(fmt: Fmt, img: &Rgba, white: bool) -> Option<Encoded> {
    let (w, h) = (img.w, img.h);
    let codes: Vec<u32> = match fmt {
        Fmt::Rgba16 if !white => img.px.iter().map(|&p| rgba16_exact(p).map(u32::from)).collect::<Option<_>>()?,
        Fmt::Rgba32 if !white => img.px.iter().map(|&p| u32::from_be_bytes(p)).collect(),
        Fmt::Ci4 | Fmt::Ci8 if !white => {
            let texels: Vec<u16> = img.px.iter().map(|&p| rgba16_exact(p)).collect::<Option<_>>()?;
            let mut pal = texels.clone();
            pal.sort_unstable();
            pal.dedup();
            if pal.len() > fmt.palette_max() {
                return None;
            }
            return Some(ci_from_codes(fmt, w, h, &texels, &pal));
        }
        Fmt::I4 | Fmt::I8 => img
            .px
            .iter()
            .map(|p| {
                let v = if white {
                    (p[0] == 255 && p[1] == 255 && p[2] == 255).then_some(p[3])?
                } else {
                    (p[0] == p[1] && p[1] == p[2] && p[3] == 255).then_some(p[0])?
                };
                if fmt == Fmt::I4 {
                    (v % 17 == 0).then_some(u32::from(v / 17))
                } else {
                    Some(u32::from(v))
                }
            })
            .collect::<Option<_>>()?,
        Fmt::Ia4 if !white => img
            .px
            .iter()
            .map(|p| {
                if p[0] != p[1] || p[1] != p[2] || (p[3] != 0 && p[3] != 255) {
                    return None;
                }
                let i = p[0] >> 5;
                (expand3(i) == p[0]).then_some(u32::from(i) << 1 | u32::from(p[3] == 255))
            })
            .collect::<Option<_>>()?,
        Fmt::Ia8 if !white => img
            .px
            .iter()
            .map(|p| (p[0] == p[1] && p[1] == p[2] && p[0] % 17 == 0 && p[3] % 17 == 0).then_some(u32::from(p[0] / 17) << 4 | u32::from(p[3] / 17)))
            .collect::<Option<_>>()?,
        _ => return None,
    };
    let e = pack(fmt, w, h, &codes);
    debug_assert_eq!(e.decode(white), *img);
    Some(e)
}

/// Lossy encode of a float RGBA image (straight alpha, 0..255). Returns `None` when the
/// format cannot represent the image's semantics at all (e.g. I4 for a translucent
/// colour texture, colour formats for an alpha mask).
pub fn encode_lossy(fmt: Fmt, w: usize, h: usize, src: &[[f32; 4]], white: bool, dither: Dither) -> Option<Encoded> {
    let opaque = src.iter().all(|p| p[3] >= 254.5);
    let q4 = |v: f32| (v.clamp(0.0, 255.0) / 17.0).round() as u32;
    let codes: Vec<u32> = match fmt {
        Fmt::Rgba16 if !white => quantise(w, h, src, dither, 255.0 / 31.0, |c| {
            let v = rgba16_of(c);
            (fl(rgba16_decode(v)), u32::from(v))
        }),
        Fmt::Rgba32 if !white => src.iter().map(|p| u32::from_be_bytes(p.map(|v| v.round().clamp(0.0, 255.0) as u8))).collect(),
        Fmt::Ci4 | Fmt::Ci8 if !white => return Some(encode_ci_lossy(fmt, w, h, src, dither)),
        Fmt::I4 | Fmt::I8 if white || opaque => {
            let chan = |c: [f32; 4]| if white { c[3] } else { luma(c) };
            if fmt == Fmt::I4 {
                let i: Vec<[f32; 4]> = src.iter().map(|&c| [chan(c); 4]).collect();
                quantise(w, h, &i, dither, 17.0, |c| {
                    let q = q4(c[0]);
                    ([(q * 17) as f32; 4], q)
                })
            } else {
                src.iter().map(|&c| chan(c).round().clamp(0.0, 255.0) as u32).collect()
            }
        }
        Fmt::Ia4 if !white => src
            .iter()
            .map(|&c| {
                let i = (luma(c).clamp(0.0, 255.0) * 7.0 / 255.0).round() as u32;
                i << 1 | u32::from(c[3] >= 127.5)
            })
            .collect(),
        Fmt::Ia8 if !white => src.iter().map(|&c| q4(luma(c)) << 4 | q4(c[3])).collect(),
        _ => return None,
    };
    Some(pack(fmt, w, h, &codes))
}

fn encode_ci_lossy(fmt: Fmt, w: usize, h: usize, src: &[[f32; 4]], dither: Dither) -> Encoded {
    let pal = ci_palette(fmt, src);
    encode_ci_with(fmt, w, h, src, &pal, dither)
}

/// RGBA16 palette (≤ 16 / 256 codes) for a float image: the exact rounded colours when
/// few enough, else median cut + 8 k-means passes over a 5-bit histogram.
pub fn ci_palette(fmt: Fmt, src: &[[f32; 4]]) -> Vec<u16> {
    let k = fmt.palette_max();
    let mut uniq: Vec<u16> = src.iter().map(|&c| rgba16_of(c)).collect();
    uniq.sort_unstable();
    uniq.dedup();
    if uniq.len() <= k {
        return uniq;
    }
    let hist = histogram(src, 5);
    let pal = make_palette(&hist, k, 8);
    let mut codes: Vec<u16> = pal.iter().map(|&c| rgba16_of(c)).collect();
    codes.sort_unstable();
    codes.dedup();
    codes
}

/// CI encode with a given palette (nearest entry per texel, optional dithering).
pub fn encode_ci_with(fmt: Fmt, w: usize, h: usize, src: &[[f32; 4]], pal_codes: &[u16], dither: Dither) -> Encoded {
    let pal_f: Vec<[f32; 4]> = pal_codes.iter().map(|&c| fl(rgba16_decode(c))).collect();
    let mut cache: std::collections::HashMap<[u32; 4], usize> = std::collections::HashMap::new();
    let idx = quantise(w, h, src, dither, 255.0 / 31.0, |c| {
        let key = c.map(|v| v.round() as u32);
        let i = *cache.entry(key).or_insert_with(|| nearest(&pal_f, c));
        (pal_f[i], i as u32)
    });
    let texels: Vec<u16> = idx.iter().map(|&i| pal_codes[i as usize]).collect();
    let mut used: Vec<u16> = texels.clone();
    used.sort_unstable();
    used.dedup();
    ci_from_codes(fmt, w, h, &texels, &used)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmem_limits() {
        assert!(Fmt::Rgba16.fits(32, 32));
        assert!(!Fmt::Rgba16.fits(64, 64));
        assert!(Fmt::Rgba16.fits(64, 32));
        assert!(Fmt::Ci4.fits(64, 64));
        assert!(!Fmt::Ci8.fits(64, 64));
        assert!(Fmt::Ci8.fits(64, 32));
        assert!(Fmt::I4.fits(128, 64));
        assert!(Fmt::Rgba32.fits(32, 32));
        assert!(!Fmt::Rgba32.fits(64, 32));
        // narrow rows still take a full 8-byte TMEM line
        assert_eq!(Fmt::Ci4.tmem_pitch(4), 8);
        assert_eq!(tile_shape(Fmt::Rgba16, 32, 32), (32, 32));
        let (tw, th) = tile_shape(Fmt::Rgba16, 320, 240);
        assert!(Fmt::Rgba16.fits(tw, th) && tw * th >= 1920, "{tw}x{th}");
        let (tw, th) = tile_shape_full_height(Fmt::Ci4, 3212, 34);
        assert!(Fmt::Ci4.fits(tw, th) && th == 34 && tw >= 96, "{tw}x{th}");
    }

    fn all_exact(img: &Rgba, white: bool) -> Vec<Fmt> {
        Fmt::ALL.into_iter().filter(|&f| encode_exact(f, img, white).is_some_and(|e| e.decode(white) == *img)).collect()
    }

    #[test]
    fn exact_round_trips() {
        // 3 colours incl. transparent: RGBA16, RGBA32, CI4, CI8.
        let img = Rgba::new(
            4,
            2,
            vec![[255, 0, 0, 255], [0, 0, 0, 0], [8, 132, 255, 255], [255, 0, 0, 255], [0, 0, 0, 0], [0, 0, 0, 0], [8, 132, 255, 255], [255, 0, 0, 255]],
        );
        assert_eq!(all_exact(&img, false), vec![Fmt::Ci4, Fmt::Ci8, Fmt::Rgba16, Fmt::Rgba32]);
        // Greys ×17, opaque: I4/I8/IA8 also exact.
        let g = Rgba::new(2, 1, vec![[17, 17, 17, 255], [255, 255, 255, 255]]);
        assert!(all_exact(&g, false).contains(&Fmt::I4));
        assert!(all_exact(&g, false).contains(&Fmt::Ia8));
        // Alpha mask: I4 / I8 only.
        let m = Rgba::new(2, 1, vec![[255, 255, 255, 34], [255, 255, 255, 0]]);
        assert_eq!(all_exact(&m, true), vec![Fmt::I4, Fmt::I8]);
        // CI4 limited to 16 colours.
        let many = Rgba::new(17, 1, (0..17u8).map(|i| [expand5(u16::from(i)), 0, 0, 255]).collect());
        assert!(encode_exact(Fmt::Ci4, &many, false).is_none());
        assert!(encode_exact(Fmt::Ci8, &many, false).is_some());
    }

    #[test]
    fn packing_layout_is_big_endian_high_nibble_first() {
        let img = Rgba::new(2, 1, vec![[255, 255, 255, 255], [0, 0, 0, 255]]);
        let e = encode_exact(Fmt::I4, &img, false).unwrap();
        assert_eq!(e.stride, 8);
        assert_eq!(e.data[0], 0xF0);
        let e = encode_exact(Fmt::Rgba16, &img, false).unwrap();
        assert_eq!(&e.data[..4], &[0xFF, 0xFF, 0x00, 0x01]);
    }

    #[test]
    fn lossy_ci_keeps_alpha_and_fits_palette() {
        let w = 16;
        let src: Vec<[f32; 4]> =
            (0..w * w).map(|i| [(i % 256) as f32, ((i * 7) % 256) as f32, ((i * 13) % 256) as f32, if i % 5 == 0 { 0.0 } else { 255.0 }]).collect();
        for f in [Fmt::Ci4, Fmt::Ci8] {
            let e = encode_lossy(f, w, w, &src, false, Dither::None).unwrap();
            assert!(e.tlut.len() <= f.palette_max());
            let d = e.decode(false);
            for (s, p) in src.iter().zip(&d.px) {
                assert_eq!(s[3] > 127.0, p[3] == 255);
            }
        }
    }
}
