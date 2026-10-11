//! Reader of the 1C pack's `NTX1` texture files (`tools/f2-n64pack/src/textures/`: the
//! byte layout is specified in `textures/mod.rs`, the host reader is `ntx.rs`). Zero-copy:
//! texels and palettes are used where they lie in the loaded file (the RDP reads them
//! directly), only the tables are parsed. Also decodes images to RGBA8 for the few places
//! the CPU composes pictures (the story book) and builds the procedural-texture source bank
//! the simulation needs (`pt_texture_load`).

use alloc::string::String;
use alloc::vec::Vec;

pub const MAGIC: [u8; 4] = *b"NTX1";
pub const HEADER_SIZE: usize = 64;
pub const IMAGE_SIZE: usize = 32;
pub const ENTRY_SIZE: usize = 24;
pub const DEP_SIZE: usize = 32;
pub const GLYPH_TABLE_SIZE: usize = 1024;
pub const NONE: u16 = 0xFFFF;

/// Texel formats (`ImageDesc::fmt`).
pub mod fmt {
    pub const RGBA16: u8 = 0;
    pub const RGBA32: u8 = 1;
    pub const CI4: u8 = 2;
    pub const CI8: u8 = 3;
    pub const IA4: u8 = 4;
    pub const IA8: u8 = 5;
    pub const I4: u8 = 6;
    pub const I8: u8 = 7;
    /// Not stored in NTX files: made on the N64 from I8 alpha masks / opaque I8.
    pub const IA16: u8 = 8;

    pub fn bits(f: u8) -> usize {
        match f {
            CI4 | IA4 | I4 => 4,
            CI8 | IA8 | I8 => 8,
            RGBA32 => 32,
            _ => 16,
        }
    }
}

pub mod kind {
    pub const NORMAL: u8 = 0;
    pub const ALPHA_INTENSITY: u8 = 1;
    pub const PROCEDURAL: u8 = 2;
    pub const NO_TEXTURE: u8 = 3;
    pub const ALPHA_COLOUR: u8 = 4;
    pub const FONT: u8 = 5;
    pub const BACKDROP: u8 = 6;
}

pub mod flag {
    pub const ALPHA: u16 = 1 << 0;
    pub const ALPHA_1BIT: u16 = 1 << 1;
    pub const WHITE: u16 = 1 << 2;
    pub const SINGLE_TILE: u16 = 1 << 3;
    pub const LOSSY: u16 = 1 << 4;
    pub const DOWNSCALED: u16 = 1 << 5;
    pub const POW2: u16 = 1 << 6;
    pub const SOURCE_555: u16 = 1 << 7;
    pub const TLUT_PER_TILE: u16 = 1 << 8;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageDesc {
    pub fmt: u8,
    pub kind: u8,
    pub flags: u16,
    pub width: u16,
    pub height: u16,
    pub orig_width: u16,
    pub orig_height: u16,
    pub stride: u16,
    pub tile_w: u16,
    pub tile_h: u16,
    pub tlut_first: u32,
    pub tlut_count: u16,
    pub aux: u16,
    pub pixel_offset: u32,
    pub pixel_size: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub crc: u32,
    pub image: u16,
    pub dep: u8,
    pub flags: u8,
    pub name: [u8; 12],
    pub anim: u16,
}

impl Entry {
    pub fn name(&self) -> &str {
        let n = self.name.iter().position(|&c| c == 0).unwrap_or(12);
        core::str::from_utf8(&self.name[..n]).unwrap_or("")
    }
}

/// Animation record (`TexAnim`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Anim {
    pub base_crc: u32,
    pub frames: Vec<(u32, i32)>,
}

/// A parsed NTX1 file over its bytes.
pub struct Ntx<'a> {
    pub data: &'a [u8],
    pub file_kind: u16,
    pub images: Vec<ImageDesc>,
    pub entries: Vec<Entry>,
    pub anims: Vec<Anim>,
    pub deps: Vec<String>,
    off_gly: usize,
    n_gly: usize,
    off_tlut: usize,
    n_tlut: usize,
    off_px: usize,
}

pub type Result<T> = core::result::Result<T, &'static str>;

fn be16(b: &[u8], o: usize) -> Result<u16> {
    b.get(o..o + 2).map(|s| u16::from_be_bytes([s[0], s[1]])).ok_or("ntx: truncated")
}
fn be32(b: &[u8], o: usize) -> Result<u32> {
    b.get(o..o + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]])).ok_or("ntx: truncated")
}

impl<'a> Ntx<'a> {
    pub fn parse(b: &'a [u8]) -> Result<Ntx<'a>> {
        if b.len() < HEADER_SIZE || b[0..4] != MAGIC {
            return Err("ntx: bad magic");
        }
        if be16(b, 4)? != 1 {
            return Err("ntx: unsupported version");
        }
        let file_kind = be16(b, 6)?;
        let n = |i: usize| be16(b, 8 + 2 * i).map(|v| v as usize);
        let (n_img, n_ent, _n_sorted, n_anim, n_dep, n_gly) = (n(0)?, n(1)?, n(2)?, n(3)?, n(4)?, n(5)?);
        let o = |i: usize| be32(b, 20 + 4 * i).map(|v| v as usize);
        let (off_img, off_ent, off_anim, off_dep, off_gly, off_tlut, n_tlut, off_px, n_px, size) = (o(0)?, o(1)?, o(3)?, o(4)?, o(5)?, o(6)?, o(7)?, o(8)?, o(9)?, o(10)?);
        if size > b.len() || off_px + n_px > b.len() || off_tlut + 2 * n_tlut > b.len() || off_gly + n_gly * GLYPH_TABLE_SIZE > b.len() {
            return Err("ntx: section out of range");
        }
        let mut images = Vec::with_capacity(n_img);
        for i in 0..n_img {
            let p = off_img + i * IMAGE_SIZE;
            let f = |k: usize| be16(b, p + 2 + 2 * k);
            let d = ImageDesc {
                fmt: *b.get(p).ok_or("ntx: truncated")?,
                kind: *b.get(p + 1).ok_or("ntx: truncated")?,
                flags: f(0)?,
                width: f(1)?,
                height: f(2)?,
                orig_width: f(3)?,
                orig_height: f(4)?,
                stride: f(5)?,
                tile_w: f(6)?,
                tile_h: f(7)?,
                tlut_first: u32::from(f(8)?) * 4,
                tlut_count: f(9)?,
                aux: f(10)?,
                pixel_offset: be32(b, p + 24)?,
                pixel_size: be32(b, p + 28)?,
            };
            if d.fmt > fmt::I8
                || d.pixel_offset as usize + d.pixel_size as usize > n_px
                || d.stride as usize * d.height as usize != d.pixel_size as usize
                || (d.tlut_count > 0 && d.tlut_first as usize + d.tlut_count as usize > n_tlut)
                || (d.aux != NONE && d.aux as usize >= n_gly)
            {
                return Err("ntx: bad image descriptor");
            }
            images.push(d);
        }
        let mut entries = Vec::with_capacity(n_ent);
        for i in 0..n_ent {
            let p = off_ent + i * ENTRY_SIZE;
            let raw = b.get(p..p + ENTRY_SIZE).ok_or("ntx: truncated")?;
            let mut name = [0u8; 12];
            name.copy_from_slice(&raw[8..20]);
            let e = Entry { crc: be32(b, p)?, image: be16(b, p + 4)?, dep: raw[6], flags: raw[7], name, anim: be16(b, p + 20)? };
            if e.dep as usize > n_dep || (e.dep == 0 && e.image as usize >= n_img) {
                return Err("ntx: bad entry");
            }
            entries.push(e);
        }
        let mut anims = Vec::with_capacity(n_anim);
        let mut p = off_anim;
        for _ in 0..n_anim {
            let base_crc = be32(b, p)?;
            let nf = be32(b, p + 4)? as usize;
            if nf > 4096 {
                return Err("ntx: bad animation");
            }
            p += 8;
            let mut frames = Vec::with_capacity(nf);
            for _ in 0..nf {
                frames.push((be32(b, p)?, be32(b, p + 4)? as i32));
                p += 8;
            }
            anims.push(Anim { base_crc, frames });
        }
        let mut deps = Vec::with_capacity(n_dep);
        for i in 0..n_dep {
            let raw = b.get(off_dep + i * DEP_SIZE..off_dep + (i + 1) * DEP_SIZE).ok_or("ntx: truncated")?;
            let end = raw.iter().position(|&c| c == 0).unwrap_or(DEP_SIZE);
            deps.push(String::from(core::str::from_utf8(&raw[..end]).map_err(|_| "ntx: bad dep")?));
        }
        Ok(Ntx { data: b, file_kind, images, entries, anims, deps, off_gly, n_gly, off_tlut, n_tlut, off_px })
    }

    pub fn texels(&self, i: usize) -> &'a [u8] {
        let d = &self.images[i];
        let s = self.off_px + d.pixel_offset as usize;
        &self.data[s..s + d.pixel_size as usize]
    }

    /// The image's TLUT bytes (big-endian RGBA16 entries).
    pub fn tlut_bytes(&self, i: usize) -> &'a [u8] {
        let d = &self.images[i];
        let s = self.off_tlut + 2 * d.tlut_first as usize;
        &self.data[s..s + 2 * d.tlut_count as usize]
    }

    pub fn tlut_entry(&self, i: usize, k: usize) -> u16 {
        let t = self.tlut_bytes(i);
        t.get(2 * k..2 * k + 2).map_or(0, |s| u16::from_be_bytes([s[0], s[1]]))
    }

    /// Glyph table `(left, width)` per byte of a font image.
    pub fn glyphs(&self, i: usize) -> Option<[(u16, u16); 256]> {
        let a = self.images[i].aux;
        if a == NONE || a as usize >= self.n_gly {
            return None;
        }
        let mut g = [(0u16, 0u16); 256];
        for (c, slot) in g.iter_mut().enumerate() {
            let q = self.off_gly + a as usize * GLYPH_TABLE_SIZE + c * 4;
            *slot = (be16(self.data, q).ok()?, be16(self.data, q + 2).ok()?);
        }
        Some(g)
    }

    /// `n_tlut` (sanity).
    pub fn tlut_len(&self) -> usize {
        self.n_tlut
    }

    pub fn texel(&self, i: usize, x: usize, y: usize) -> u32 {
        let d = &self.images[i];
        texel(d.fmt, self.texels(i), d.stride as usize, x, y)
    }

    /// RGBA8 of image `i` at its stored size (the colours the renderer shows; procedural
    /// images: their initial white upload).
    pub fn decode_rgba8(&self, i: usize) -> Vec<u8> {
        let d = self.images[i];
        let (w, h) = (d.width as usize, d.height as usize);
        let mut out = Vec::with_capacity(w * h * 4);
        let white = d.flags & flag::WHITE != 0;
        let src555 = d.flags & flag::SOURCE_555 != 0 && d.fmt == fmt::RGBA16;
        for y in 0..h {
            for x in 0..w {
                let code = self.texel(i, x, y);
                let px = if src555 {
                    let a = (((code >> 1) & 0x1f) >> 1) as u8;
                    [255, 255, 255, (a << 4) | a]
                } else {
                    let pal = |k: u32| {
                        let base = if d.flags & flag::TLUT_PER_TILE != 0 && d.tile_w > 0 && d.tile_h > 0 {
                            256 * ((y / d.tile_h as usize) * w.div_ceil(d.tile_w as usize) + x / d.tile_w as usize)
                        } else {
                            0
                        };
                        self.tlut_entry(i, base + k as usize)
                    };
                    decode_texel(d.fmt, code, white, pal)
                };
                out.extend_from_slice(&px);
            }
        }
        out
    }

    /// Raw PC X1R5G5B5 pixels of a `SOURCE_555` image (procedural textures).
    pub fn source_555(&self, i: usize) -> Option<Vec<u16>> {
        let d = &self.images[i];
        if d.flags & flag::SOURCE_555 == 0 || d.fmt != fmt::RGBA16 {
            return None;
        }
        let mut v = Vec::with_capacity(d.width as usize * d.height as usize);
        for y in 0..d.height as usize {
            for x in 0..d.width as usize {
                v.push((self.texel(i, x, y) as u16) >> 1);
            }
        }
        Some(v)
    }
}

pub fn texel(f: u8, data: &[u8], stride: usize, x: usize, y: usize) -> u32 {
    let row = &data[y * stride..];
    match fmt::bits(f) {
        4 => u32::from((row[x / 2] >> if x % 2 == 0 { 4 } else { 0 }) & 15),
        8 => u32::from(row[x]),
        16 => u32::from(u16::from_be_bytes([row[x * 2], row[x * 2 + 1]])),
        _ => u32::from_be_bytes([row[x * 4], row[x * 4 + 1], row[x * 4 + 2], row[x * 4 + 3]]),
    }
}

fn expand5(v: u16) -> u8 {
    let v = (v & 31) as u8;
    (v << 3) | (v >> 2)
}
fn expand3(v: u8) -> u8 {
    let v = v & 7;
    (v << 5) | (v << 2) | (v >> 1)
}

pub fn rgba16_decode(v: u16) -> [u8; 4] {
    [expand5(v >> 11), expand5(v >> 6), expand5(v >> 1), if v & 1 != 0 { 255 } else { 0 }]
}

/// One texel code → RGBA8 (`rdp::decode_texel` of the packer).
pub fn decode_texel(f: u8, code: u32, white: bool, pal: impl Fn(u32) -> u16) -> [u8; 4] {
    match f {
        fmt::RGBA16 => rgba16_decode(code as u16),
        fmt::RGBA32 => code.to_be_bytes(),
        fmt::CI4 | fmt::CI8 => rgba16_decode(pal(code)),
        fmt::I4 | fmt::I8 => {
            let i = if f == fmt::I4 { (code as u8 & 15) * 17 } else { code as u8 };
            if white {
                [255, 255, 255, i]
            } else {
                [i, i, i, 255]
            }
        }
        fmt::IA4 => {
            let i = expand3((code >> 1) as u8);
            [i, i, i, if code & 1 != 0 { 255 } else { 0 }]
        }
        fmt::IA8 => {
            let i = ((code >> 4) as u8 & 15) * 17;
            [i, i, i, (code as u8 & 15) * 17]
        }
        _ => {
            let c = code as u16;
            [(c >> 8) as u8, (c >> 8) as u8, (c >> 8) as u8, c as u8]
        }
    }
}

/// I-format images the RDP's TEX × SHADE combiner would show wrong (alpha masks need white
/// colour, opaque I needs alpha 1; the RDP sets I alpha = intensity): an IA16 / IA8 copy
/// with the PC's colours, or `None` when the image is fine as stored. Returns
/// `(fmt, stride, texels)`.
pub fn ia_fixup(d: &ImageDesc, texels: &[u8]) -> Option<(u8, u16, Vec<u8>)> {
    let white = d.flags & flag::WHITE != 0;
    let opaque = d.flags & flag::ALPHA == 0;
    if !(d.fmt == fmt::I4 || d.fmt == fmt::I8) || !(white || opaque) {
        return None;
    }
    let (w, h) = (d.width as usize, d.height as usize);
    let texels_n = w * h;
    // IA16 keeps 8-bit intensity where it fits TMEM (2048 texels), else IA8 (4 bits).
    let wide = d.fmt == fmt::I8 && texels_n <= 2048;
    if !wide && texels_n > 4096 {
        return None;
    }
    let (f, bpp) = if wide { (fmt::IA16, 2) } else { (fmt::IA8, 1) };
    let stride = (w * bpp).div_ceil(8) * 8;
    let mut out = alloc::vec![0u8; stride * h];
    for y in 0..h {
        for x in 0..w {
            let c = texel(d.fmt, texels, d.stride as usize, x, y);
            let i8v = if d.fmt == fmt::I4 { (c as u8 & 15) * 17 } else { c as u8 };
            let (i, a) = if white { (255u8, i8v) } else { (i8v, 255u8) };
            if wide {
                out[y * stride + 2 * x] = i;
                out[y * stride + 2 * x + 1] = a;
            } else {
                out[y * stride + x] = (i & 0xF0) | (a >> 4);
            }
        }
    }
    Some((f, stride as u16, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ia_fixup_alpha_mask() {
        let d = ImageDesc {
            fmt: fmt::I4,
            kind: kind::ALPHA_INTENSITY,
            flags: flag::WHITE | flag::ALPHA,
            width: 4,
            height: 1,
            orig_width: 4,
            orig_height: 1,
            stride: 8,
            tile_w: 4,
            tile_h: 1,
            tlut_first: 0,
            tlut_count: 0,
            aux: NONE,
            pixel_offset: 0,
            pixel_size: 8,
        };
        let tex = [0x0F, 0x80, 0, 0, 0, 0, 0, 0];
        let (f, stride, out) = ia_fixup(&d, &tex).unwrap();
        assert_eq!((f, stride), (fmt::IA8, 8));
        assert_eq!(&out[..4], &[0xF0, 0xFF, 0xF8, 0xF0]);
        assert_eq!(decode_texel(fmt::I4, 8, true, |_| 0), [255, 255, 255, 136]);
    }

    /// Every NTX file of a built pack parses (needs `F2N64_PACK_DIR`; skipped otherwise).
    #[test]
    fn real_pack_files_parse() {
        let Some(dir) = std::env::var_os("F2N64_PACK_DIR") else { return };
        let t = std::path::Path::new(&dir).join("textures");
        let mut n = 0;
        let mut stack = alloc::vec![t];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "ntx") {
                    let raw = std::fs::read(&p).unwrap();
                    // mkasset (DCA) files cannot be read on the host without libdragon; only raw ones.
                    if raw.starts_with(b"NTX1") {
                        let f = Ntx::parse(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
                        for i in 0..f.images.len() {
                            assert_eq!(f.decode_rgba8(i).len(), f.images[i].width as usize * f.images[i].height as usize * 4);
                        }
                        n += 1;
                    }
                }
            }
        }
        std::eprintln!("parsed {n} NTX files");
    }
}
