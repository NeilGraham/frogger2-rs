//! Bitmap fonts: port of `InitFont` / `DrawFontStringAtLoc` / `CalcStringWidth`
//! (`MDX Library/mdxFont.cpp`).
//!
//! A font is a horizontal strip (`textures/font/*.bmp`, also stored in
//! `Font.fla`); the top-left pixel is the transparent colour and glyphs are
//! separated by fully transparent columns, in `ALPHABET` order.

use crate::prelude::*;

#[cfg(not(feature = "std"))]
use crate::fmath::FloatExt;

use f2_formats::texture::Texture;

/// `alphabet[]` (mdxFont.cpp:66-68), Latin-1 bytes. The source file's final
/// character is `\xa9` (shown as a broken glyph in the VSS export).
pub const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!@#$%^&*()-=+[]{}:;\"'|\\,<.>/?\
\xe0\xe8\xec\xf2\xf9\xc0\xc8\xcc\xd2\xd9\xe1\xe9\xed\xf3\xfa\xfd\xc1\xc9\xcd\xd3\xda\xdd\xe2\xea\xee\xf4\xfb\xc2\xca\xce\xd4\xdb\xe3\xf1\xf5\xc3\xd1\xd5\xe4\xeb\xef\xf6\xfc\xff\xc4\xcb\xcf\xd6\xdc\xe5\xc5\xe6\xc6\xe7\xc7\xf0\xd0\xf8\xd8\xbf\xa1\xdf\xa9";

/// One glyph (`MDX_FONTCHAR`): column range in the source strip.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Glyph {
    pub left: u32,
    pub width: u32,
}

/// A parsed font: glyph table plus an RGBA8 copy of the strip
/// (transparent colour → alpha 0).
#[derive(Clone, Debug)]
pub struct Font {
    pub width: u32,
    pub height: u32,
    pub glyphs: [Option<Glyph>; 256],
    pub rgba: Vec<u8>,
}

fn c5(v: u16) -> u8 {
    let v = (v & 0x1f) as u8;
    (v << 3) | (v >> 2)
}

/// Decodes an uncompressed BMP (1/4/8/24/32 bpp) to top-down X1R5G5B5,
/// like `gelfLoad_BMP(..., GELF_IFORMAT_16BPP555, GELF_IMAGEDATA_TOPDOWN)`.
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

impl Font {
    /// Port of `InitFont` (mdxFont.cpp:85) over 555 pixels (top-down).
    pub fn from_pixels(width: u32, height: u32, data: &[u16]) -> Font {
        let (w, h) = (width as usize, height as usize);
        let transparent = data[0];
        let mut glyphs = [None; 256];
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
            // (The 256×256 page packing of the original only affects storage.)
            glyphs[ch as usize] = Some(Glyph { left: left as u32, width: width as u32 });
            if txpos >= w {
                break;
            }
        }
        // `font->characters[' '].width = font->characters['!'].width` (no image).
        let space_w = glyphs[b'!' as usize].map_or(0, |g| g.width);
        glyphs[b' ' as usize] = Some(Glyph { left: u32::MAX, width: space_w });
        let mut rgba = Vec::with_capacity(w * h * 4);
        for &p in data {
            if p == transparent {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                rgba.extend_from_slice(&[c5(p >> 10), c5(p >> 5), c5(p), 255]);
            }
        }
        Font { width, height, glyphs, rgba }
    }

    pub fn from_bmp(data: &[u8]) -> Option<Font> {
        let (w, h, px) = decode_bmp_555(data)?;
        Some(Font::from_pixels(w, h, &px))
    }

    /// From a `Font.fla` bank entry (e.g. `bigfont.bmp`).
    pub fn from_texture(t: &Texture) -> Font {
        Font::from_pixels(t.width.into(), t.height.into(), &t.pixels)
    }

    /// `GetCharWidth` (unscaled pixels).
    pub fn char_width(&self, c: u8) -> u32 {
        self.glyphs[c as usize].map_or(0, |g| g.width)
    }

    /// Port of `CalcStringWidth` (mdxFont.cpp) for Latin-1 bytes, without the
    /// `@T/@X/@S/@C` key-name substitutions (callers expand them).
    pub fn string_width(&self, text: &[u8], scale: f32) -> i32 {
        text.iter().map(|&c| (self.char_width(c) as f32 * scale) as i32).sum()
    }
}

/// High-resolution replacement image of a font (the "modern" fonts, built by `f2-fontgen` from
/// the typeface the pixel fonts were drawn from): an RGBA atlas holding each glyph's cell at
/// `scale`× the strip's pixel size. Metrics (glyph widths, height) stay the strip's, so text
/// layout is unchanged; only the image differs.
#[derive(Clone, Debug, PartialEq)]
pub struct HiRes {
    /// Atlas pixels per strip pixel.
    pub scale: f32,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Per byte: the glyph cell `[x, y, w, h]` in atlas pixels (`w == 0`: no image).
    pub rects: Vec<[f32; 4]>,
}

impl HiRes {
    /// File magic of [`encode`](Self::encode).
    pub const MAGIC: &'static [u8; 4] = b"F2MF";

    /// Texture coordinates `[u1, v1, u2, v2]` of byte `c`'s cell.
    pub fn uv(&self, c: u8) -> Option<[f32; 4]> {
        let r = self.rects.get(c as usize)?;
        (r[2] > 0.0).then(|| {
            let (w, h) = (self.width as f32, self.height as f32);
            [r[0] / w, r[1] / h, (r[0] + r[2]) / w, (r[1] + r[3]) / h]
        })
    }

    /// The atlas resampled to `scale` atlas pixels per strip pixel (area average over
    /// premultiplied alpha), so it is drawn near 1:1 at the output resolution (no mipmaps
    /// needed on any back end). Never enlarges.
    pub fn resampled(&self, scale: f32) -> HiRes {
        let f = (scale / self.scale).min(1.0);
        if f >= 0.999 {
            return self.clone();
        }
        let (sw, sh) = (self.width as usize, self.height as usize);
        let dw = ((sw as f32 * f).ceil() as usize).max(1);
        let dh = ((sh as f32 * f).ceil() as usize).max(1);
        // premultiplied float copy
        let mut pre = vec![0f32; sw * sh * 4];
        for i in 0..sw * sh {
            let a = self.rgba[i * 4 + 3] as f32 / 255.0;
            for c in 0..3 {
                pre[i * 4 + c] = self.rgba[i * 4 + c] as f32 / 255.0 * a;
            }
            pre[i * 4 + 3] = a;
        }
        // 1D box-filter weights: destination pixel d covers source [d/f, (d+1)/f)
        let taps = |n_src: usize, n_dst: usize| -> Vec<Vec<(usize, f32)>> {
            (0..n_dst)
                .map(|d| {
                    let (a, b) = (d as f32 / f, ((d + 1) as f32 / f).min(n_src as f32));
                    let mut v = Vec::new();
                    let mut x = a.floor() as usize;
                    while (x as f32) < b && x < n_src {
                        let w = (b.min(x as f32 + 1.0) - a.max(x as f32)).max(0.0);
                        if w > 0.0 {
                            v.push((x, w));
                        }
                        x += 1;
                    }
                    let t: f32 = v.iter().map(|p| p.1).sum();
                    v.iter_mut().for_each(|p| p.1 /= t.max(1e-6));
                    v
                })
                .collect()
        };
        let (tx, ty) = (taps(sw, dw), taps(sh, dh));
        let mut mid = vec![0f32; dw * sh * 4];
        for y in 0..sh {
            for (dx, t) in tx.iter().enumerate() {
                for &(sx, w) in t {
                    for c in 0..4 {
                        mid[(y * dw + dx) * 4 + c] += pre[(y * sw + sx) * 4 + c] * w;
                    }
                }
            }
        }
        let mut rgba = vec![0u8; dw * dh * 4];
        for (dy, t) in ty.iter().enumerate() {
            for dx in 0..dw {
                let mut acc = [0f32; 4];
                for &(sy, w) in t {
                    for (c, a) in acc.iter_mut().enumerate() {
                        *a += mid[(sy * dw + dx) * 4 + c] * w;
                    }
                }
                let o = (dy * dw + dx) * 4;
                let a = acc[3];
                for c in 0..3 {
                    rgba[o + c] = if a > 0.0 { (acc[c] / a * 255.0 + 0.5).min(255.0) as u8 } else { 0 };
                }
                rgba[o + 3] = (a * 255.0 + 0.5).min(255.0) as u8;
            }
        }
        let rects = self.rects.iter().map(|r| r.map(|v| v * f)).collect();
        HiRes { scale: self.scale * f, width: dw as u32, height: dh as u32, rgba, rects }
    }

    /// Raw form for `no_std` loaders (the N64 pack compresses it): magic `F2MR`, then the
    /// [`encode`](Self::encode) header, then the RGBA8 atlas uncompressed.
    pub fn encode_raw(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(20 + 256 * 16 + self.rgba.len());
        out.extend_from_slice(b"F2MR");
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&self.scale.to_le_bytes());
        out.extend_from_slice(&self.width.to_le_bytes());
        out.extend_from_slice(&self.height.to_le_bytes());
        for i in 0..256 {
            let r = self.rects.get(i).copied().unwrap_or_default();
            r.iter().for_each(|v| out.extend_from_slice(&v.to_le_bytes()));
        }
        out.extend_from_slice(&self.rgba);
        out
    }

    /// Reads [`encode_raw`](Self::encode_raw); `with_pixels: false` keeps only the metrics.
    pub fn decode_raw(data: &[u8], with_pixels: bool) -> Option<HiRes> {
        let hdr = 20 + 256 * 16;
        if data.len() < hdr || &data[0..4] != b"F2MR" {
            return None;
        }
        let u32_at = |o: usize| u32::from_le_bytes(data[o..o + 4].try_into().unwrap());
        let f32_at = |o: usize| f32::from_le_bytes(data[o..o + 4].try_into().unwrap());
        let (scale, width, height) = (f32_at(8), u32_at(12), u32_at(16));
        let n = width as usize * height as usize * 4;
        if u32_at(4) != 1 || data.len() < hdr + n {
            return None;
        }
        let rects = (0..256).map(|i| core::array::from_fn(|k| f32_at(20 + i * 16 + k * 4))).collect();
        let rgba = if with_pixels { data[hdr..hdr + n].to_vec() } else { Vec::new() };
        Some(HiRes { scale, width, height, rgba, rects })
    }

    /// `F2MF` file: magic, version (1), scale, atlas size, 256 cells (f32 x/y/w/h), then the
    /// atlas as PNG.
    #[cfg(feature = "std")]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(Self::MAGIC);
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&self.scale.to_le_bytes());
        out.extend_from_slice(&self.width.to_le_bytes());
        out.extend_from_slice(&self.height.to_le_bytes());
        for i in 0..256 {
            let r = self.rects.get(i).copied().unwrap_or_default();
            r.iter().for_each(|v| out.extend_from_slice(&v.to_le_bytes()));
        }
        let mut enc = png::Encoder::new(&mut out, self.width, self.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::High);
        let mut w = enc.write_header().expect("png header");
        w.write_image_data(&self.rgba).expect("png data");
        w.finish().expect("png finish");
        out
    }

    #[cfg(feature = "std")]
    pub fn decode(data: &[u8]) -> Result<HiRes, String> {
        let hdr = 20 + 256 * 16;
        if data.len() < hdr || &data[0..4] != Self::MAGIC {
            return Err("not an F2MF font".into());
        }
        let u32_at = |o: usize| u32::from_le_bytes(data[o..o + 4].try_into().unwrap());
        let f32_at = |o: usize| f32::from_le_bytes(data[o..o + 4].try_into().unwrap());
        if u32_at(4) != 1 {
            return Err(format!("F2MF version {} unsupported", u32_at(4)));
        }
        let (scale, width, height) = (f32_at(8), u32_at(12), u32_at(16));
        let rects = (0..256).map(|i| core::array::from_fn(|k| f32_at(20 + i * 16 + k * 4))).collect();
        let dec = png::Decoder::new(std::io::Cursor::new(&data[hdr..]));
        let mut rd = dec.read_info().map_err(|e| e.to_string())?;
        let mut rgba = vec![0u8; rd.output_buffer_size().ok_or("png too large")?];
        let info = rd.next_frame(&mut rgba).map_err(|e| e.to_string())?;
        if (info.width, info.height) != (width, height) || info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
            return Err("F2MF atlas format mismatch".into());
        }
        rgba.truncate(info.buffer_size());
        Ok(HiRes { scale, width, height, rgba, rects })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_glyphs_on_empty_columns() {
        // 0 = transparent; glyph A: cols 1-2, glyph B: col 4.
        let w = 6;
        let mut px = vec![0u16; w * 2];
        for &x in &[1usize, 2, 4] {
            px[w + x] = 0x7fff;
        }
        let f = Font::from_pixels(w as u32, 2, &px);
        assert_eq!(f.glyphs[b'A' as usize], Some(Glyph { left: 1, width: 2 }));
        assert_eq!(f.glyphs[b'B' as usize], Some(Glyph { left: 4, width: 1 }));
        assert_eq!(f.string_width(b"AB", 1.0), 3);
    }

    fn hires_2x2() -> HiRes {
        let mut rects = vec![[0.0; 4]; 256];
        rects[b'A' as usize] = [0.0, 0.0, 4.0, 4.0];
        HiRes { scale: 4.0, width: 4, height: 4, rgba: [255u8, 255, 255, 255].repeat(8).into_iter().chain([0u8; 32]).collect(), rects }
    }

    #[test]
    fn hires_resample_halves_and_keeps_uv() {
        let h = hires_2x2();
        let r = h.resampled(2.0);
        assert_eq!((r.width, r.height, r.scale), (2, 2, 2.0));
        // top half opaque white, bottom half transparent
        assert_eq!(&r.rgba[0..4], &[255, 255, 255, 255]);
        assert_eq!(r.rgba[2 * 4 + 3], 0);
        assert_eq!(r.uv(b'A'), h.uv(b'A'));
        assert_eq!(r.uv(b'B'), None);
    }

    #[test]
    fn hires_raw_roundtrip() {
        let h = hires_2x2();
        assert_eq!(HiRes::decode_raw(&h.encode_raw(), true).unwrap(), h);
        assert!(HiRes::decode_raw(&h.encode_raw(), false).unwrap().rgba.is_empty());
    }

    #[cfg(feature = "std")]
    #[test]
    fn hires_roundtrip() {
        let h = hires_2x2();
        assert_eq!(HiRes::decode(&h.encode()).unwrap(), h);
    }
}
