//! Bitmap fonts: port of `InitFont` / `DrawFontStringAtLoc` / `CalcStringWidth`
//! (`MDX Library/mdxFont.cpp`).
//!
//! A font is a horizontal strip (`textures/font/*.bmp`, also stored in
//! `Font.fla`); the top-left pixel is the transparent colour and glyphs are
//! separated by fully transparent columns, in `ALPHABET` order.

use crate::prelude::*;

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
}
