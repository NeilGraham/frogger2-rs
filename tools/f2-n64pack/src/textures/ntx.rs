//! The `NTX1` packed texture file: writer, reader and RGBA8 decoder.
//!
//! The byte layout is specified in the parent module's documentation
//! ([`super`]). The reader only needs `alloc` (`Vec`, `String`) and slices, so it ports
//! to the `no_std` N64 runtime as is (replace the `String` error with a static str).

use f2_formats::texture::{TexAnim, TexAnimFrame};

use super::image::Rgba;
use super::rdp::{self, Encoded, Fmt};

pub const MAGIC: [u8; 4] = *b"NTX1";
pub const VERSION: u16 = 1;
pub const HEADER_SIZE: usize = 64;
pub const IMAGE_SIZE: usize = 32;
pub const ENTRY_SIZE: usize = 24;
pub const DEP_SIZE: usize = 32;
pub const GLYPH_TABLE_SIZE: usize = 1024;
/// `aux` / `anim` / `left` "none" marker.
pub const NONE: u16 = 0xFFFF;

/// `file_kind` header field.
pub mod file_kind {
    /// A texture bank (`textures/<bank>.ntx`): images, CRC entries, animations, deps.
    pub const BANK: u16 = 0;
    /// A shared image pool (`textures/shared/*.ntx`): images only.
    pub const POOL: u16 = 1;
    /// A full-screen backdrop (`textures/backdrop/*.ntx`): one image.
    pub const BACKDROP: u16 = 2;
    /// A loose image (`textures/<dir>/<name>.ntx` from a `.bmp`): one image + one entry.
    pub const SINGLE: u16 = 3;
}

/// Image `kind` byte: the PC texture type, which selects the combiner set-up.
pub mod kind {
    pub const NORMAL: u8 = 0;
    pub const ALPHA_INTENSITY: u8 = 1;
    pub const PROCEDURAL: u8 = 2;
    pub const NO_TEXTURE: u8 = 3;
    pub const ALPHA_COLOUR: u8 = 4;
    pub const FONT: u8 = 5;
    pub const BACKDROP: u8 = 6;
}

/// Image `flags` bits.
pub mod flag {
    /// Some texel has alpha < 255 (needs alpha compare or blending).
    pub const ALPHA: u16 = 1 << 0;
    /// Alpha is only 0 or 255 (alpha compare is enough; no blending needed).
    pub const ALPHA_1BIT: u16 = 1 << 1;
    /// Alpha-mask texture: combiner colour input = 1.0, texel intensity = alpha.
    pub const WHITE: u16 = 1 << 2;
    /// The whole image fits TMEM in one load (wrap/mirror addressing allowed).
    pub const SINGLE_TILE: u16 = 1 << 3;
    /// Stored texels are an approximation of the source (see the pack notes for PSNR).
    pub const LOSSY: u16 = 1 << 4;
    /// Stored size is smaller than `orig` (scale UVs by `width/orig_width`, …).
    pub const DOWNSCALED: u16 = 1 << 5;
    /// Both stored dimensions are powers of two.
    pub const POW2: u16 = 1 << 6;
    /// RGBA16 texels are the raw PC X1R5G5B5 source (`(src << 1) | (src != 0x7C1F)`):
    /// procedural textures, whose palette the game derives from the source pixels.
    pub const SOURCE_555: u16 = 1 << 7;
    /// CI8 with one 256-entry palette per tile (`tile_w × tile_h` grid, row-major):
    /// tile `i` uses TLUT entries `tlut_first + 256·i …`. Draw unfiltered (1:1).
    pub const TLUT_PER_TILE: u16 = 1 << 8;
}

/// One image descriptor (32 bytes in the file).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageDesc {
    pub fmt: Fmt,
    pub kind: u8,
    pub flags: u16,
    pub width: u16,
    pub height: u16,
    pub orig_width: u16,
    pub orig_height: u16,
    pub stride: u16,
    pub tile_w: u16,
    pub tile_h: u16,
    /// First TLUT entry (a multiple of 4; stored in the file divided by 4).
    pub tlut_first: u32,
    pub tlut_count: u16,
    /// Glyph table index (fonts) or [`NONE`].
    pub aux: u16,
    pub pixel_offset: u32,
    pub pixel_size: u32,
}

/// One CRC entry (24 bytes in the file).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub crc: u32,
    /// Image index inside the file named by `dep`.
    pub image: u16,
    /// 0 = this file, k = `deps[k - 1]`.
    pub dep: u8,
    pub flags: u8,
    /// PC bank name, NUL padded (the procedural-texture code selects its effect from it).
    pub name: [u8; 12],
    /// Index into the animation table when this entry is an animation base, else [`NONE`].
    pub anim: u16,
}

/// Glyph table of a font image: per Latin-1 byte `(left, width)` in stored pixels;
/// `(NONE, 0)` = no glyph; `(NONE, w)` = space (advance only).
pub type GlyphTable = [(u16, u16); 256];

/// A parsed (or to-be-written) `NTX1` file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NtxFile {
    pub file_kind: u16,
    pub images: Vec<ImageDesc>,
    pub entries: Vec<Entry>,
    /// Entry indices sorted by CRC, one per distinct CRC (the last entry with that CRC
    /// in bank order, matching `LoadTexBankFile`'s replace-on-duplicate).
    pub sorted: Vec<u16>,
    pub anims: Vec<TexAnim>,
    /// Dependency pack paths relative to `textures/` (e.g. `shared/s1a2b3c4d.ntx`).
    pub deps: Vec<String>,
    pub glyphs: Vec<GlyphTable>,
    pub tlut: Vec<u16>,
    pub pixels: Vec<u8>,
}

fn align(v: usize, a: usize) -> usize {
    v.div_ceil(a) * a
}

/// Image metadata supplied when adding an image.
#[derive(Clone, Debug)]
pub struct ImageMeta {
    pub kind: u8,
    pub flags: u16,
    pub orig_width: u16,
    pub orig_height: u16,
    pub tile: (usize, usize),
    pub glyphs: Option<GlyphTable>,
}

impl NtxFile {
    pub fn new(file_kind: u16) -> NtxFile {
        NtxFile { file_kind, ..Default::default() }
    }

    /// Appends an image (texels 8-aligned in the pixel blob; identical TLUTs shared).
    pub fn add_image(&mut self, enc: &Encoded, meta: &ImageMeta) -> u16 {
        let off = align(self.pixels.len(), 8);
        self.pixels.resize(off, 0);
        self.pixels.extend_from_slice(&enc.data);
        let (tlut_first, tlut_count) = if enc.tlut.is_empty() {
            (0, 0)
        } else {
            let n = enc.tlut.len();
            let found = (0..self.tlut.len()).step_by(4).find(|&i| self.tlut.get(i..i + n) == Some(&enc.tlut[..]));
            let first = found.unwrap_or_else(|| {
                let f = align(self.tlut.len(), 4);
                self.tlut.resize(f, 0);
                self.tlut.extend_from_slice(&enc.tlut);
                f
            });
            assert!(n <= 0xFFFF && first / 4 <= 0xFFFF, "TLUT too large for the descriptor");
            (first as u32, n as u16)
        };
        let aux = match &meta.glyphs {
            Some(g) => {
                self.glyphs.push(*g);
                (self.glyphs.len() - 1) as u16
            }
            None => NONE,
        };
        let pow2 = enc.w.is_power_of_two() && enc.h.is_power_of_two();
        let per_tile = if let Some(t) = enc.palette_tile {
            assert_eq!(t, meta.tile, "per-tile palettes need the palette tile as the image tile");
            flag::TLUT_PER_TILE
        } else {
            0
        };
        self.images.push(ImageDesc {
            fmt: enc.fmt,
            kind: meta.kind,
            flags: meta.flags | per_tile | if pow2 { flag::POW2 } else { 0 },
            width: enc.w as u16,
            height: enc.h as u16,
            orig_width: meta.orig_width,
            orig_height: meta.orig_height,
            stride: enc.stride as u16,
            tile_w: meta.tile.0 as u16,
            tile_h: meta.tile.1 as u16,
            tlut_first,
            tlut_count,
            aux,
            pixel_offset: off as u32,
            pixel_size: enc.data.len() as u32,
        });
        (self.images.len() - 1) as u16
    }

    /// Rebuilds [`NtxFile::sorted`] from the entries.
    pub fn sort_entries(&mut self) {
        let mut last: std::collections::BTreeMap<u32, u16> = std::collections::BTreeMap::new();
        for (i, e) in self.entries.iter().enumerate() {
            last.insert(e.crc, i as u16);
        }
        self.sorted = last.into_values().collect();
    }

    /// Serialises to the big-endian file layout.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = vec![0u8; HEADER_SIZE];
        let pad = |out: &mut Vec<u8>| {
            let n = align(out.len(), 8);
            out.resize(n, 0);
        };
        let off_images = out.len();
        for d in &self.images {
            out.push(d.fmt as u8);
            out.push(d.kind);
            for v in [d.flags, d.width, d.height, d.orig_width, d.orig_height, d.stride, d.tile_w, d.tile_h, (d.tlut_first / 4) as u16, d.tlut_count, d.aux] {
                out.extend_from_slice(&v.to_be_bytes());
            }
            out.extend_from_slice(&d.pixel_offset.to_be_bytes());
            out.extend_from_slice(&d.pixel_size.to_be_bytes());
        }
        pad(&mut out);
        let off_entries = out.len();
        for e in &self.entries {
            out.extend_from_slice(&e.crc.to_be_bytes());
            out.extend_from_slice(&e.image.to_be_bytes());
            out.push(e.dep);
            out.push(e.flags);
            out.extend_from_slice(&e.name);
            out.extend_from_slice(&e.anim.to_be_bytes());
            out.extend_from_slice(&[0, 0]);
        }
        pad(&mut out);
        let off_sorted = out.len();
        for s in &self.sorted {
            out.extend_from_slice(&s.to_be_bytes());
        }
        pad(&mut out);
        let off_anims = out.len();
        for a in &self.anims {
            out.extend_from_slice(&a.base_crc.to_be_bytes());
            out.extend_from_slice(&(a.frames.len() as u32).to_be_bytes());
            for f in &a.frames {
                out.extend_from_slice(&f.crc.to_be_bytes());
                out.extend_from_slice(&f.time.to_be_bytes());
            }
        }
        pad(&mut out);
        let off_deps = out.len();
        for d in &self.deps {
            let mut b = [0u8; DEP_SIZE];
            let s = d.as_bytes();
            assert!(s.len() < DEP_SIZE, "dependency path too long: {d}");
            b[..s.len()].copy_from_slice(s);
            out.extend_from_slice(&b);
        }
        let off_glyphs = out.len();
        for g in &self.glyphs {
            for &(l, w) in g {
                out.extend_from_slice(&l.to_be_bytes());
                out.extend_from_slice(&w.to_be_bytes());
            }
        }
        pad(&mut out);
        let off_tlut = out.len();
        for t in &self.tlut {
            out.extend_from_slice(&t.to_be_bytes());
        }
        pad(&mut out);
        let off_pixels = out.len();
        out.extend_from_slice(&self.pixels);
        pad(&mut out);
        let size = out.len();
        let h = &mut out[..HEADER_SIZE];
        h[0..4].copy_from_slice(&MAGIC);
        let u16s = [
            VERSION,
            self.file_kind,
            self.images.len() as u16,
            self.entries.len() as u16,
            self.sorted.len() as u16,
            self.anims.len() as u16,
            self.deps.len() as u16,
            self.glyphs.len() as u16,
        ];
        for (i, v) in u16s.iter().enumerate() {
            h[4 + 2 * i..6 + 2 * i].copy_from_slice(&v.to_be_bytes());
        }
        let u32s = [off_images, off_entries, off_sorted, off_anims, off_deps, off_glyphs, off_tlut, self.tlut.len(), off_pixels, self.pixels.len(), size];
        for (i, v) in u32s.iter().enumerate() {
            h[20 + 4 * i..24 + 4 * i].copy_from_slice(&(*v as u32).to_be_bytes());
        }
        out
    }

    /// Parses a file (all offsets and sizes bounds-checked).
    pub fn parse(b: &[u8]) -> Result<NtxFile, String> {
        let u16_at = |o: usize| b.get(o..o + 2).map(|s| u16::from_be_bytes([s[0], s[1]])).ok_or_else(|| format!("ntx: truncated at {o:#x}"));
        let u32_at = |o: usize| b.get(o..o + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]])).ok_or_else(|| format!("ntx: truncated at {o:#x}"));
        if b.len() < HEADER_SIZE || b[0..4] != MAGIC {
            return Err("ntx: bad magic".into());
        }
        if u16_at(4)? != VERSION {
            return Err(format!("ntx: unsupported version {}", u16_at(4)?));
        }
        let file_kind = u16_at(6)?;
        let (n_img, n_ent, n_sorted, n_anim, n_dep, n_gly) =
            (u16_at(8)? as usize, u16_at(10)? as usize, u16_at(12)? as usize, u16_at(14)? as usize, u16_at(16)? as usize, u16_at(18)? as usize);
        let o = |i: usize| u32_at(20 + 4 * i).map(|v| v as usize);
        let (off_img, off_ent, off_sorted, off_anim, off_dep, off_gly, off_tlut, n_tlut, off_px, n_px, size) =
            (o(0)?, o(1)?, o(2)?, o(3)?, o(4)?, o(5)?, o(6)?, o(7)?, o(8)?, o(9)?, o(10)?);
        if size != b.len() {
            return Err(format!("ntx: size field {size} != {}", b.len()));
        }
        for off in [off_img, off_ent, off_sorted, off_anim, off_tlut, off_px] {
            if off % 8 != 0 || off > size {
                return Err(format!("ntx: misaligned or out-of-range section offset {off:#x}"));
            }
        }
        let mut images = Vec::with_capacity(n_img);
        for i in 0..n_img {
            let p = off_img + i * IMAGE_SIZE;
            let f = |k: usize| u16_at(p + 2 + 2 * k);
            let fmt_b = *b.get(p).ok_or("ntx: truncated image table")?;
            let d = ImageDesc {
                fmt: Fmt::from_u8(fmt_b).ok_or_else(|| format!("ntx: bad format {fmt_b}"))?,
                kind: b[p + 1],
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
                pixel_offset: u32_at(p + 24)?,
                pixel_size: u32_at(p + 28)?,
            };
            if !d.pixel_offset.is_multiple_of(8)
                || d.pixel_offset as usize + d.pixel_size as usize > n_px
                || (d.stride as usize) * (d.height as usize) != d.pixel_size as usize
            {
                return Err(format!("ntx: image {i} pixel range invalid"));
            }
            if !d.stride.is_multiple_of(8) || (d.stride as usize) < (d.width as usize * d.fmt.bits()).div_ceil(8) {
                return Err(format!("ntx: image {i} stride invalid"));
            }
            if d.tlut_count > 0 && d.tlut_first as usize + d.tlut_count as usize > n_tlut {
                return Err(format!("ntx: image {i} TLUT range invalid"));
            }
            if d.aux != NONE && d.aux as usize >= n_gly {
                return Err(format!("ntx: image {i} glyph table out of range"));
            }
            images.push(d);
        }
        let mut entries = Vec::with_capacity(n_ent);
        for i in 0..n_ent {
            let p = off_ent + i * ENTRY_SIZE;
            let raw = b.get(p..p + ENTRY_SIZE).ok_or("ntx: truncated entry table")?;
            let mut name = [0u8; 12];
            name.copy_from_slice(&raw[8..20]);
            let e = Entry { crc: u32_at(p)?, image: u16_at(p + 4)?, dep: raw[6], flags: raw[7], name, anim: u16_at(p + 20)? };
            if e.dep as usize > n_dep || (e.dep == 0 && e.image as usize >= n_img) || (e.anim != NONE && e.anim as usize >= n_anim) {
                return Err(format!("ntx: entry {i} reference out of range"));
            }
            entries.push(e);
        }
        let sorted = (0..n_sorted).map(|i| u16_at(off_sorted + 2 * i)).collect::<Result<Vec<_>, _>>()?;
        if sorted.iter().any(|&s| s as usize >= n_ent) || sorted.windows(2).any(|w| entries[w[0] as usize].crc >= entries[w[1] as usize].crc) {
            return Err("ntx: CRC index not sorted / out of range".into());
        }
        let mut anims = Vec::with_capacity(n_anim);
        let mut p = off_anim;
        for _ in 0..n_anim {
            let base_crc = u32_at(p)?;
            let n = u32_at(p + 4)? as usize;
            if n > 4096 {
                return Err("ntx: implausible animation".into());
            }
            p += 8;
            let mut frames = Vec::with_capacity(n);
            for _ in 0..n {
                frames.push(TexAnimFrame { crc: u32_at(p)?, time: u32_at(p + 4)? as i32 });
                p += 8;
            }
            anims.push(TexAnim { base_crc, frames });
        }
        let mut deps = Vec::with_capacity(n_dep);
        for i in 0..n_dep {
            let raw = b.get(off_dep + i * DEP_SIZE..off_dep + (i + 1) * DEP_SIZE).ok_or("ntx: truncated deps")?;
            let end = raw.iter().position(|&c| c == 0).unwrap_or(DEP_SIZE);
            deps.push(String::from_utf8_lossy(&raw[..end]).into_owned());
        }
        let mut glyphs = Vec::with_capacity(n_gly);
        for i in 0..n_gly {
            let mut g = [(0u16, 0u16); 256];
            for (c, slot) in g.iter_mut().enumerate() {
                let q = off_gly + i * GLYPH_TABLE_SIZE + c * 4;
                *slot = (u16_at(q)?, u16_at(q + 2)?);
            }
            glyphs.push(g);
        }
        let tlut = (0..n_tlut).map(|i| u16_at(off_tlut + 2 * i)).collect::<Result<Vec<_>, _>>()?;
        let pixels = b.get(off_px..off_px + n_px).ok_or("ntx: truncated pixels")?.to_vec();
        Ok(NtxFile { file_kind, images, entries, sorted, anims, deps, glyphs, tlut, pixels })
    }

    /// Binary search of the CRC index (what the runtime's `GetTexEntryFromCRC` does).
    pub fn find(&self, crc: u32) -> Option<&Entry> {
        let i = self.sorted.binary_search_by_key(&crc, |&s| self.entries[s as usize].crc).ok()?;
        Some(&self.entries[self.sorted[i] as usize])
    }

    pub fn texels(&self, i: usize) -> &[u8] {
        let d = &self.images[i];
        &self.pixels[d.pixel_offset as usize..(d.pixel_offset + d.pixel_size) as usize]
    }

    pub fn image_tlut(&self, i: usize) -> &[u16] {
        let d = &self.images[i];
        &self.tlut[d.tlut_first as usize..d.tlut_first as usize + d.tlut_count as usize]
    }

    /// Raw PC X1R5G5B5 pixels of a [`flag::SOURCE_555`] image.
    pub fn source_555(&self, i: usize) -> Option<Vec<u16>> {
        let d = &self.images[i];
        if d.flags & flag::SOURCE_555 == 0 || d.fmt != Fmt::Rgba16 {
            return None;
        }
        let data = self.texels(i);
        let mut v = Vec::with_capacity(d.width as usize * d.height as usize);
        for y in 0..d.height as usize {
            for x in 0..d.width as usize {
                v.push((rdp::texel(Fmt::Rgba16, data, d.stride as usize, x, y) as u16) >> 1);
            }
        }
        Some(v)
    }

    /// Decodes image `i` (stored size) to the RGBA8 the renderer must display, following
    /// the combiner rules of its kind/flags: alpha masks (`WHITE`) are white with the
    /// intensity as alpha, opaque I formats are grey, procedural images show their
    /// initial PC upload (white, alpha = blue5 >> 1 expanded from 4 bits).
    pub fn decode_rgba8(&self, i: usize) -> Rgba {
        let d = &self.images[i];
        if let Some(src) = self.source_555(i) {
            let px = src
                .iter()
                .map(|&p| {
                    let a = ((p & 0x1f) >> 1) as u8;
                    [255, 255, 255, (a << 4) | a]
                })
                .collect();
            return Rgba::new(d.width as usize, d.height as usize, px);
        }
        let pt = (d.flags & flag::TLUT_PER_TILE != 0).then_some((d.tile_w as usize, d.tile_h as usize));
        rdp::decode(d.fmt, d.width as usize, d.height as usize, d.stride as usize, self.texels(i), self.image_tlut(i), d.flags & flag::WHITE != 0, pt)
    }
}

/// Entry name field from a bank texture name.
pub fn name12(s: &str) -> [u8; 12] {
    let mut n = [0u8; 12];
    for (d, c) in n.iter_mut().zip(s.bytes()) {
        *d = c;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_parse_round_trip() {
        let img = Rgba::new(
            4,
            2,
            vec![[255, 0, 0, 255], [0, 0, 0, 0], [8, 132, 255, 255], [255, 0, 0, 255], [0, 0, 0, 0], [0, 0, 0, 0], [8, 132, 255, 255], [255, 0, 0, 255]],
        );
        let mut f = NtxFile::new(file_kind::BANK);
        for fmt in [Fmt::Ci4, Fmt::Rgba16, Fmt::Ci8, Fmt::Ci4] {
            let e = rdp::encode_exact(fmt, &img, false).unwrap();
            let mut g = [(NONE, 0u16); 256];
            g[b'A' as usize] = (1, 2);
            f.add_image(
                &e,
                &ImageMeta { kind: kind::NORMAL, flags: flag::ALPHA, orig_width: 4, orig_height: 2, tile: (4, 2), glyphs: (fmt == Fmt::Ci8).then_some(g) },
            );
        }
        // identical CI4 palettes are shared
        assert_eq!(f.images[0].tlut_first, f.images[3].tlut_first);
        f.entries.push(Entry { crc: 9, image: 1, dep: 0, flags: 0, name: name12("b.bmp"), anim: NONE });
        f.entries.push(Entry { crc: 3, image: 0, dep: 0, flags: 1, name: name12("a.bmp"), anim: 0 });
        f.entries.push(Entry { crc: 9, image: 2, dep: 0, flags: 0, name: name12("b2.bmp"), anim: NONE });
        f.anims.push(TexAnim { base_crc: 3, frames: vec![TexAnimFrame { crc: 3, time: -1 }, TexAnimFrame { crc: 9, time: 4 }] });
        f.sort_entries();
        let b = f.to_bytes();
        assert_eq!(b.len() % 8, 0);
        let g = NtxFile::parse(&b).unwrap();
        assert_eq!(g, f);
        assert_eq!(g.find(9).unwrap().image, 2, "later duplicate CRC wins");
        assert_eq!(g.find(3).unwrap().anim, 0);
        assert!(g.find(4).is_none());
        for i in 0..4 {
            assert_eq!(g.decode_rgba8(i), img);
        }
        assert_eq!(g.glyphs[0][b'A' as usize], (1, 2));
        // corruption is detected
        let mut bad = b.clone();
        bad[0] = b'X';
        assert!(NtxFile::parse(&bad).is_err());
        assert!(NtxFile::parse(&b[..b.len() - 8]).is_err());
    }
}
