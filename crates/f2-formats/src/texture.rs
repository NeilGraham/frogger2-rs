//! Texture banks (`textures/<Bank>.fla`) and full-screen backdrops.
//!
//! Bank payload (after FLA2 decompression), from `LoadTexBankFile`
//! (MDX `mdxTexture.cpp:680`):
//! ```text
//! u32 numTextures
//! numTextures × { TEXTURE_HEADER (24 B); u16 pixels[w*h] (X1R5G5B5, top-down) }
//! u32 numAnims
//! numAnims × { u32 numFrames; u32 baseCrc; numFrames × { u32 frameCrc; i32 time } }
//! TEXTURE_HEADER = { u32 flags; u32 crc; i16 w; i16 h; char name[12] }
//! ```
//! Backdrops are a single header + pixels with no animation table.

use crate::read::{Reader, Result};

pub const TEX_TRANSVAL: u16 = 0x7c1f;

/// `TEXTURE_*` type in the header flags (`mdxTexture.h`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TexType {
    Normal,
    /// `ai_`: white, alpha from the blue channel.
    AlphaIntensity,
    /// `prc`: procedural (uploaded like `ai_`).
    Procedural,
    /// Plain surface (non power-of-two).
    NoTexture,
    /// `ac_`: alpha = mean of channels.
    AlphaColour,
    Other(u32),
}

impl TexType {
    pub fn from_flags(f: u32) -> TexType {
        match f {
            0 => TexType::Normal,
            1 => TexType::AlphaIntensity,
            2 => TexType::Procedural,
            3 => TexType::NoTexture,
            4 => TexType::AlphaColour,
            o => TexType::Other(o),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Texture {
    pub flags: u32,
    pub crc: u32,
    pub width: u16,
    pub height: u16,
    /// Name as stored (truncated to 11 characters by the bank tool).
    pub name: String,
    pub pixels: Vec<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TexAnimFrame {
    pub crc: u32,
    /// Game frames; -1 means "advance every frame".
    pub time: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TexAnim {
    pub base_crc: u32,
    pub frames: Vec<TexAnimFrame>,
}

#[derive(Clone, Debug)]
pub struct TexBank {
    pub textures: Vec<Texture>,
    pub anims: Vec<TexAnim>,
}

fn read_texture(r: &mut Reader) -> Result<Texture> {
    let flags = r.u32()?;
    let crc = r.u32()?;
    let w = r.i16()?;
    let h = r.i16()?;
    let name = r.fixed_str(12)?;
    if w <= 0 || h <= 0 {
        return Err(r.err(format!("bad texture size {w}x{h}")));
    }
    let n = w as usize * h as usize;
    let raw = r.bytes(n * 2)?;
    let pixels = raw.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    Ok(Texture { flags, crc, width: w as u16, height: h as u16, name, pixels })
}

impl TexBank {
    /// Parses a decompressed bank.
    pub fn parse(data: &[u8]) -> Result<TexBank> {
        let mut r = Reader::new("texbank", data);
        let n = r.u32()?;
        if n > 10_000 {
            return Err(r.err(format!("implausible texture count {n}")));
        }
        let mut textures = Vec::with_capacity(n as usize);
        for _ in 0..n {
            textures.push(read_texture(&mut r)?);
        }
        let na = r.u32()?;
        let mut anims = Vec::with_capacity(na as usize);
        for _ in 0..na {
            let nf = r.u32()?;
            let base_crc = r.u32()?;
            if nf > 4096 {
                return Err(r.err(format!("implausible frame count {nf}")));
            }
            let mut frames = Vec::with_capacity(nf as usize);
            for _ in 0..nf {
                frames.push(TexAnimFrame { crc: r.u32()?, time: r.i32()? });
            }
            anims.push(TexAnim { base_crc, frames });
        }
        if !r.is_empty() {
            return Err(r.err(format!("{} trailing bytes", r.remaining())));
        }
        Ok(TexBank { textures, anims })
    }

    pub fn find(&self, crc: u32) -> Option<&Texture> {
        self.textures.iter().find(|t| t.crc == crc)
    }
}

/// A full-screen image (`title.fla`, `loading.fla`, `artworkNN.fla`): one texture record.
pub fn parse_backdrop(data: &[u8]) -> Result<Texture> {
    let mut r = Reader::new("backdrop", data);
    let t = read_texture(&mut r)?;
    if !r.is_empty() {
        return Err(r.err(format!("{} trailing bytes", r.remaining())));
    }
    Ok(t)
}

#[inline]
fn c5(v: u16) -> u8 {
    let v = (v & 0x1f) as u8;
    (v << 3) | (v >> 2)
}

#[inline]
fn c4(v: u16) -> u8 {
    let v = (v & 0xf) as u8;
    (v << 4) | v
}

impl Texture {
    pub fn tex_type(&self) -> TexType {
        TexType::from_flags(self.flags)
    }

    /// Converts to RGBA8 exactly as the D3D upload path does
    /// (`DDrawCopyToSurface`, MDX `mdxD3D.cpp:631`), including the
    /// colour bleed into transparent magenta pixels of normal textures.
    pub fn to_rgba8(&self) -> Vec<u8> {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut out = Vec::with_capacity(w * h * 4);
        let d = &self.pixels;
        match self.tex_type() {
            TexType::AlphaIntensity | TexType::Procedural => {
                // ARGB4444: ((blue5 << 11) | 0x0fff) → A = blue5 >> 1, RGB white.
                for &p in d {
                    let v = ((p & 0x1f) << 11) | 0x0fff;
                    out.extend_from_slice(&[c4(v >> 8), c4(v >> 4), c4(v), c4(v >> 12)]);
                }
            }
            TexType::AlphaColour => {
                for &p in d {
                    let r = ((p >> 10) & 0x1f) >> 1;
                    let g = ((p >> 5) & 0x1f) >> 1;
                    let b = (p & 0x1f) >> 1;
                    let a = (r + g + b) / 3;
                    out.extend_from_slice(&[c4(r), c4(g), c4(b), c4(a)]);
                }
            }
            TexType::NoTexture => {
                for &p in d {
                    out.extend_from_slice(&[c5(p >> 10), c5(p >> 5), c5(p), 255]);
                }
            }
            TexType::Normal | TexType::Other(_) => {
                let n = (w * h) as i64;
                for y in 0..h as i64 {
                    for x in 0..w as i64 {
                        let mut val = d[(x + y * w as i64) as usize];
                        let opaque = val != TEX_TRANSVAL;
                        if !opaque {
                            // Reproduces the original neighbour scan, quirks included.
                            for i in -1..=1i64 {
                                for j in -1..=1i64 {
                                    let v2 = (x + i) + (y + j) * w as i64;
                                    if v2 > 0 && v2 < n {
                                        let c = d[v2 as usize];
                                        if c != TEX_TRANSVAL {
                                            val = c;
                                        }
                                    } else {
                                        val = TEX_TRANSVAL;
                                    }
                                }
                            }
                        }
                        out.extend_from_slice(&[c5(val >> 10), c5(val >> 5), c5(val), if opaque { 255 } else { 0 }]);
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bank_bytes() -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&1u32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&0x1234u32.to_le_bytes());
        v.extend_from_slice(&2i16.to_le_bytes());
        v.extend_from_slice(&1i16.to_le_bytes());
        v.extend_from_slice(b"ab.bmp\0\0\0\0\0\0");
        v.extend_from_slice(&0x7fffu16.to_le_bytes());
        v.extend_from_slice(&TEX_TRANSVAL.to_le_bytes());
        v.extend_from_slice(&1u32.to_le_bytes());
        v.extend_from_slice(&1u32.to_le_bytes());
        v.extend_from_slice(&0x1234u32.to_le_bytes());
        v.extend_from_slice(&0x1234u32.to_le_bytes());
        v.extend_from_slice(&(-1i32).to_le_bytes());
        v
    }

    #[test]
    fn parse_and_convert() {
        let b = TexBank::parse(&bank_bytes()).unwrap();
        assert_eq!(b.textures.len(), 1);
        assert_eq!(b.textures[0].name, "ab.bmp");
        assert_eq!(b.anims[0].frames[0].time, -1);
        let rgba = b.textures[0].to_rgba8();
        assert_eq!(&rgba[0..4], &[255, 255, 255, 255]);
        // Transparent pixel: the original scan ends on an out-of-range
        // neighbour (index 0 is excluded by `val2 > 0`), so it stays magenta.
        assert_eq!(&rgba[4..8], &[255, 0, 255, 0]);
    }
}
