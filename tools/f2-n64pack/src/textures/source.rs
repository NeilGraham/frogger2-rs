//! Reading the PC sources: texture banks, backdrops, loose BMPs, the object banks' texture
//! references, the per-type "what the PC displays" targets, and the font glyph scan.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use f2_formats::objbank::{Mesh, Object, ObjectBank};
use f2_formats::texture::{parse_backdrop, TexBank, TexType, Texture, TEX_TRANSVAL};

use super::image::Rgba;
use super::ntx::{kind, GlyphTable, NONE};
use crate::Result;

/// Everything the converter reads, already decoded.
#[derive(Clone, Debug, Default)]
pub struct Sources {
    /// Bank stem (lower case, e.g. `garden`) → (bank, `.fla` size).
    pub banks: BTreeMap<String, (TexBank, u64)>,
    /// Backdrop stem (`title`, `artwork00`, …) → (image, `.fla` size).
    pub backdrops: BTreeMap<String, (Texture, u64)>,
    /// Loose BMPs: (`dir/name.bmp`, decoded as a texture named `name.bmp`, file size).
    pub loose: Vec<(String, Texture, u64)>,
    /// CRCs referenced by model faces / environment maps (UV-mapped 3D: one TMEM tile).
    pub used_3d: HashSet<u32>,
    /// CRCs referenced by object sprites (billboards; informative).
    pub used_sprite: HashSet<u32>,
    pub notes: Vec<String>,
}

/// Banks whose images are fonts (`InitFont` strips: transparent colour = top-left pixel).
pub const FONT_BANK: &str = "font";

impl Sources {
    /// Loads `game/textures` and scans `game/objects` of a rendition.
    pub fn load(game: &Path) -> Result<Sources> {
        let tex_dir = game.join("textures");
        let mut s = Sources::default();
        let mut names: Vec<_> =
            std::fs::read_dir(&tex_dir).map_err(|e| format!("{}: {e}", tex_dir.display()))?.filter_map(|e| e.ok().map(|e| e.path())).collect();
        names.sort();
        for p in &names {
            let Some(stem) = p.file_stem().and_then(|s| s.to_str()).map(str::to_lowercase) else { continue };
            if p.is_dir() {
                let mut files: Vec<_> = std::fs::read_dir(p).map_err(|e| format!("{}: {e}", p.display()))?.filter_map(|e| e.ok().map(|e| e.path())).collect();
                files.sort();
                for f in files {
                    if f.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("bmp")) {
                        let fname = f.file_name().unwrap().to_string_lossy().to_lowercase();
                        let data = std::fs::read(&f).map_err(|e| format!("{}: {e}", f.display()))?;
                        match decode_bmp_555(&data) {
                            Some((w, h, px)) if w <= 0x7fff && h <= 0x7fff => {
                                let t = Texture { flags: 0, crc: f2_formats::crc(&fname), width: w as u16, height: h as u16, name: fname.clone(), pixels: px };
                                s.loose.push((format!("{stem}/{fname}"), t, data.len() as u64));
                            }
                            _ => s.notes.push(format!("{stem}/{fname}: unsupported BMP, skipped")),
                        }
                    }
                }
                continue;
            }
            if !p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("fla")) {
                continue;
            }
            let raw = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
            let size = raw.len() as u64;
            let data = f2_formats::fla::maybe_decompress(raw).map_err(|e| format!("{}: {e}", p.display()))?;
            match TexBank::parse(&data) {
                Ok(b) => {
                    s.banks.insert(stem, (b, size));
                }
                Err(e) => match parse_backdrop(&data) {
                    Ok(t) => {
                        s.backdrops.insert(stem, (t, size));
                    }
                    Err(_) => return Err(format!("{}: neither a texture bank nor a backdrop ({e})", p.display())),
                },
            }
        }
        let obj_dir = game.join("objects");
        let (u3, us, n, bad) = scan_objects(&obj_dir);
        s.used_3d = u3;
        s.used_sprite = us;
        s.notes.push(format!(
            "objects: {n} banks scanned ({bad} unparsable), {} face/env-map texture CRCs, {} sprite CRCs",
            s.used_3d.len(),
            s.used_sprite.len()
        ));
        Ok(s)
    }
}

/// Collects texture CRCs used by meshes (faces, `phong` environment maps, LOC meshes) and
/// by object sprites, over every `*.dll` object bank in `dir`.
pub fn scan_objects(dir: &Path) -> (HashSet<u32>, HashSet<u32>, usize, usize) {
    let mut faces = HashSet::new();
    let mut sprites = HashSet::new();
    let (mut n, mut bad) = (0, 0);
    let Ok(rd) = std::fs::read_dir(dir) else { return (faces, sprites, 0, 0) };
    let mut paths: Vec<_> =
        rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("dll"))).collect();
    paths.sort();
    fn mesh(m: &Mesh, f: &mut HashSet<u32>) {
        f.extend(m.textures.iter().copied().filter(|&c| c != 0));
    }
    fn walk(o: &Object, f: &mut HashSet<u32>, s: &mut HashSet<u32>) {
        if let Some(m) = &o.mesh {
            mesh(m, f);
        }
        if o.phong != 0 {
            f.insert(o.phong);
        }
        s.extend(o.sprites.iter().map(|sp| sp.texture).filter(|&c| c != 0));
        for c in &o.children {
            walk(c, f, s);
        }
    }
    for p in paths {
        let Ok(data) = std::fs::read(&p) else { continue };
        match ObjectBank::parse(&data) {
            Ok(b) => {
                n += 1;
                for m in &b.models {
                    walk(&m.root, &mut faces, &mut sprites);
                    for o in &m.root_siblings {
                        walk(o, &mut faces, &mut sprites);
                    }
                    if let Some(l) = &m.loc_mesh {
                        mesh(l, &mut faces);
                    }
                }
            }
            Err(_) => bad += 1,
        }
    }
    (faces, sprites, n, bad)
}

/// Image kind of a bank texture.
pub fn kind_of(t: &Texture, font: bool) -> u8 {
    if font {
        return kind::FONT;
    }
    match t.tex_type() {
        TexType::Normal | TexType::Other(_) => kind::NORMAL,
        TexType::AlphaIntensity => kind::ALPHA_INTENSITY,
        TexType::Procedural => kind::PROCEDURAL,
        TexType::NoTexture => kind::NO_TEXTURE,
        TexType::AlphaColour => kind::ALPHA_COLOUR,
    }
}

#[inline]
fn c5(v: u16) -> u8 {
    let v = (v & 0x1f) as u8;
    (v << 3) | (v >> 2)
}

/// The RGBA8 the PC renders for a texture of `kind` (the conversion target):
/// D3D upload rules (`Texture::to_rgba8`, incl. the colour bleed into transparent texels)
/// for bank textures, `InitFont`'s colour key (top-left pixel → transparent black) for
/// fonts, opaque 5→8 expansion for backdrops.
pub fn target(t: &Texture, kind: u8) -> Rgba {
    let (w, h) = (t.width as usize, t.height as usize);
    match kind {
        kind::FONT => {
            let key = t.pixels[0];
            Rgba::new(w, h, t.pixels.iter().map(|&p| if p == key { [0, 0, 0, 0] } else { [c5(p >> 10), c5(p >> 5), c5(p), 255] }).collect())
        }
        kind::BACKDROP => Rgba::new(w, h, t.pixels.iter().map(|&p| [c5(p >> 10), c5(p >> 5), c5(p), 255]).collect()),
        _ => Rgba::from_bytes(w, h, &t.to_rgba8()),
    }
}

/// RGBA16 texel codes carrying the raw X1R5G5B5 source (procedural textures).
pub fn source_codes(t: &Texture) -> Vec<u32> {
    t.pixels.iter().map(|&p| u32::from(((p & 0x7fff) << 1) | u16::from(p != TEX_TRANSVAL))).collect()
}

/// `alphabet[]` of `mdxFont.cpp:66-68` (Latin-1), as in `f2-engine::font::ALPHABET`.
pub const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!@#$%^&*()-=+[]{}:;\"'|\\,<.>/?\
\xe0\xe8\xec\xf2\xf9\xc0\xc8\xcc\xd2\xd9\xe1\xe9\xed\xf3\xfa\xfd\xc1\xc9\xcd\xd3\xda\xdd\xe2\xea\xee\xf4\xfb\xc2\xca\xce\xd4\xdb\xe3\xf1\xf5\xc3\xd1\xd5\xe4\xeb\xef\xf6\xfc\xff\xc4\xcb\xcf\xd6\xdc\xe5\xc5\xe6\xc6\xe7\xc7\xf0\xd0\xf8\xd8\xbf\xa1\xdf\xa9";

/// Port of `InitFont`'s column scan (`mdxFont.cpp:85`, as `f2-engine::font::Font::from_pixels`):
/// glyphs in `ALPHABET` order separated by columns without ink; space gets `'!'`'s width.
pub fn glyph_table(t: &Texture) -> GlyphTable {
    let (w, h) = (t.width as usize, t.height as usize);
    let data = &t.pixels;
    let transparent = data[0];
    let mut g: GlyphTable = [(NONE, 0); 256];
    let col_has_ink = |x: usize| (0..h).any(|y| data[y * w + x] != transparent);
    let mut txpos = 0usize;
    for &ch in ALPHABET {
        while txpos < w && !col_has_ink(txpos) {
            txpos += 1;
        }
        let left = txpos;
        let mut width = 1;
        txpos += 1;
        while txpos < w && col_has_ink(txpos) {
            width += 1;
            txpos += 1;
        }
        g[ch as usize] = (left as u16, width as u16);
        if txpos >= w {
            break;
        }
    }
    let space_w = g[b'!' as usize].1;
    g[b' ' as usize] = (NONE, space_w);
    g
}

/// Decodes an uncompressed BMP (1/4/8/24/32 bpp) to top-down X1R5G5B5 (copy of
/// `f2-engine::font::decode_bmp_555`, which this crate does not depend on).
pub fn decode_bmp_555(data: &[u8]) -> Option<(u32, u32, Vec<u16>)> {
    let u32_at = |o: usize| data.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let u16_at = |o: usize| data.get(o..o + 2).map(|b| u16::from_le_bytes(b.try_into().unwrap()));
    if data.get(0..2)? != b"BM" {
        return None;
    }
    let off = u32_at(10)? as usize;
    let hsize = u32_at(14)? as usize;
    let w = u32_at(18)? as i32;
    let h = u32_at(22)? as i32;
    let bpp = u16_at(28)?;
    let comp = u32_at(30)?;
    if comp != 0 || w <= 0 || h == 0 || w > 16384 || h.abs() > 16384 {
        return None;
    }
    let (w, top_down, h) = (w as usize, h < 0, h.unsigned_abs() as usize);
    let ncol = match u32_at(46)? {
        0 if bpp <= 8 => 1usize << bpp,
        n => n as usize,
    };
    let pal_off = 14 + hsize;
    let pal = |i: usize| -> Option<(u8, u8, u8)> {
        let p = data.get(pal_off + 4 * i..pal_off + 4 * i + 3)?;
        Some((p[2], p[1], p[0]))
    };
    let stride = (w * bpp as usize).div_ceil(32) * 4;
    let mut out = vec![0u16; w * h];
    let to555 = |r: u8, g: u8, b: u8| ((r as u16 >> 3) << 10) | ((g as u16 >> 3) << 5) | (b as u16 >> 3);
    for y in 0..h {
        let src_row = if top_down { y } else { h - 1 - y };
        let row = data.get(off + src_row * stride..off + src_row * stride + stride)?;
        for x in 0..w {
            let (r, g, b) = match bpp {
                24 => (row[x * 3 + 2], row[x * 3 + 1], row[x * 3]),
                32 => (row[x * 4 + 2], row[x * 4 + 1], row[x * 4]),
                8 => pal(row[x] as usize % ncol.max(1))?,
                4 => pal(((row[x / 2] >> if x % 2 == 0 { 4 } else { 0 }) & 15) as usize)?,
                1 => pal(((row[x / 8] >> (7 - x % 8)) & 1) as usize)?,
                _ => return None,
            };
            out[y * w + x] = to555(r, g, b);
        }
    }
    Some((w as u32, h as u32, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_scan_matches_engine_rules() {
        let w = 6;
        let mut px = vec![0u16; w * 2];
        for &x in &[1usize, 2, 4] {
            px[w + x] = 0x7fff;
        }
        let t = Texture { flags: 0, crc: 0, width: w as u16, height: 2, name: "f.bmp".into(), pixels: px };
        let g = glyph_table(&t);
        assert_eq!(g[b'A' as usize], (1, 2));
        assert_eq!(g[b'B' as usize], (4, 1));
        assert_eq!(g[b'Z' as usize], (NONE, 0));
        let f = target(&t, kind::FONT);
        assert_eq!(f.px[0], [0, 0, 0, 0]);
        assert_eq!(f.px[w + 1], [255, 255, 255, 255]);
    }

    #[test]
    fn bmp_24bit_bottom_up() {
        // 2x2, 24 bpp, bottom-up: rows padded to 8 bytes.
        let mut b = vec![0u8; 54];
        b[0..2].copy_from_slice(b"BM");
        b[10..14].copy_from_slice(&54u32.to_le_bytes());
        b[14..18].copy_from_slice(&40u32.to_le_bytes());
        b[18..22].copy_from_slice(&2u32.to_le_bytes());
        b[22..26].copy_from_slice(&2u32.to_le_bytes());
        b[28..30].copy_from_slice(&24u16.to_le_bytes());
        // bottom row: blue, green; top row: red, white (BGR order)
        b.extend_from_slice(&[255, 0, 0, 0, 255, 0, 0, 0]);
        b.extend_from_slice(&[0, 0, 255, 255, 255, 255, 0, 0]);
        let (w, h, px) = decode_bmp_555(&b).unwrap();
        assert_eq!((w, h), (2, 2));
        assert_eq!(px, vec![0x7c00, 0x7fff, 0x001f, 0x03e0]);
    }
}
