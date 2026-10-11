//! RGBA8 images (the `f2-engine` texture registry) → RDP texel formats, respecting TMEM.
//!
//! Every image keeps a **full** version (RGBA16 5551, or RGBA32 when it has partial alpha
//! and fits TMEM as RGBA32) for 2D blits, which libdragon splits into TMEM-sized pieces; and
//! when the full version does not fit the 4 KiB TMEM, a **small** box-filtered version
//! (halving the larger side until it fits) that triangles sample. Phase-1C/2R replace this
//! with host-converted CI4/CI8 textures; until then large 3D textures lose resolution.

use alloc::vec::Vec;

pub const FMT_RGBA16: u32 = 0;
pub const FMT_RGBA32: u32 = 1;
/// TMEM bytes usable by one texture (4 KiB).
pub const TMEM: u32 = 4096;

/// Texels in the RDP's memory order (big-endian 16-bit words / RGBA bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct N64Tex {
    pub w: u32,
    pub h: u32,
    pub fmt: u32,
    pub data: Vec<u8>,
}

impl N64Tex {
    pub fn fits_tmem(&self) -> bool {
        fits(self.w, self.h, self.fmt)
    }
}

fn fits(w: u32, h: u32, fmt: u32) -> bool {
    let bpp = if fmt == FMT_RGBA32 { 4 } else { 2 };
    w * h * bpp <= TMEM
}

/// Whether any texel is neither transparent nor opaque (RGBA16 would lose it).
pub fn has_partial_alpha(rgba: &[u8]) -> bool {
    rgba.as_chunks::<4>().0.iter().any(|p| p[3] != 0 && p[3] != 255)
}

fn encode(w: u32, h: u32, rgba: &[u8], fmt: u32) -> N64Tex {
    let data = if fmt == FMT_RGBA32 {
        rgba.to_vec()
    } else {
        let mut d = Vec::with_capacity((w * h * 2) as usize);
        for p in rgba.as_chunks::<4>().0 {
            let v = ((p[0] as u16 >> 3) << 11) | ((p[1] as u16 >> 3) << 6) | ((p[2] as u16 >> 3) << 1) | (p[3] >= 128) as u16;
            d.extend_from_slice(&v.to_be_bytes());
        }
        d
    };
    N64Tex { w, h, fmt, data }
}

/// Halves an RGBA8 image (alpha-weighted colour, so colour-keyed texels do not bleed).
fn halve(w: u32, h: u32, rgba: &[u8], halve_w: bool, halve_h: bool) -> (u32, u32, Vec<u8>) {
    let nw = if halve_w { w.div_ceil(2) } else { w };
    let nh = if halve_h { h.div_ceil(2) } else { h };
    let mut out = Vec::with_capacity((nw * nh * 4) as usize);
    for y in 0..nh {
        for x in 0..nw {
            let mut acc = [0u32; 4];
            let mut n = 0;
            let xs = if halve_w { [x * 2, (x * 2 + 1).min(w - 1)] } else { [x, x] };
            let ys = if halve_h { [y * 2, (y * 2 + 1).min(h - 1)] } else { [y, y] };
            for sy in ys {
                for sx in xs {
                    let i = ((sy * w + sx) * 4) as usize;
                    let a = rgba[i + 3] as u32;
                    for c in 0..3 {
                        acc[c] += rgba[i + c] as u32 * a;
                    }
                    acc[3] += a;
                    n += 1;
                }
            }
            for c in 0..3 {
                out.push((acc[c] + acc[3] / 2).checked_div(acc[3]).unwrap_or(0) as u8);
            }
            out.push(((acc[3] + n / 2) / n) as u8);
        }
    }
    (nw, nh, out)
}

/// An RGBA8 image at half size in both directions (alpha-weighted box filter).
pub fn halve_rgba8(w: u32, h: u32, rgba: &[u8]) -> (u32, u32, Vec<u8>) {
    let (nw, nh, px) = halve(w, h, rgba, w > 1, h > 1);
    (nw, nh, px)
}

/// Full and (if the full one does not fit TMEM) small RDP versions of an RGBA8 image.
pub fn convert(w: u32, h: u32, rgba: &[u8]) -> (N64Tex, Option<N64Tex>) {
    assert_eq!(rgba.len(), (w * h * 4) as usize);
    let partial = has_partial_alpha(rgba);
    let fmt = if partial && fits(w, h, FMT_RGBA32) { FMT_RGBA32 } else { FMT_RGBA16 };
    let full = encode(w, h, rgba, fmt);
    if full.fits_tmem() {
        return (full, None);
    }
    let sfmt = if partial { FMT_RGBA32 } else { FMT_RGBA16 };
    let (mut sw, mut sh, mut px) = (w, h, rgba.to_vec());
    while !fits(sw, sh, sfmt) {
        let wide = sw >= sh;
        (sw, sh, px) = halve(sw, sh, &px, wide, !wide);
    }
    (full, Some(encode(sw, sh, &px, sfmt)))
}

/// RGBA16 version of a large bank-format texture (backdrops), converted 16 rows at a time
/// with the D3D upload rules of `Texture::to_rgba8` (so no whole RGBA8 copy is made; the
/// transparent-texel colour bleed does not cross strip edges, which only changes the colour
/// of texels that are invisible anyway).
pub fn convert_strips(t: &f2_formats::texture::Texture) -> N64Tex {
    let (w, h) = (t.width as u32, t.height as u32);
    let mut data = Vec::with_capacity((w * h * 2) as usize);
    let mut y = 0;
    while y < h {
        let n = 16.min(h - y);
        let strip = f2_formats::texture::Texture {
            flags: t.flags,
            crc: t.crc,
            width: t.width,
            height: n as u16,
            name: alloc::string::String::new(),
            pixels: t.pixels[(y * w) as usize..((y + n) * w) as usize].to_vec(),
        };
        data.extend_from_slice(&encode(w, n, &strip.to_rgba8(), FMT_RGBA16).data);
        y += n;
    }
    N64Tex { w, h, fmt: FMT_RGBA16, data }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_textures_stay() {
        let px = alloc::vec![255u8; 32 * 32 * 4];
        let (f, s) = convert(32, 32, &px);
        assert_eq!((f.w, f.h, f.fmt, f.data.len()), (32, 32, FMT_RGBA16, 2048));
        assert!(s.is_none());
        assert_eq!(&f.data[..2], &[0xff, 0xff]);
    }

    #[test]
    fn big_textures_get_a_tmem_version() {
        let px = alloc::vec![200u8; 64 * 64 * 4]; // partial alpha (200)
        let (f, s) = convert(64, 64, &px);
        assert_eq!(f.fmt, FMT_RGBA16);
        let s = s.unwrap();
        assert!(s.fits_tmem());
        assert_eq!((s.w, s.h, s.fmt), (32, 32, FMT_RGBA32));
        let opaque = alloc::vec![255u8; 256 * 32 * 4];
        let (_, s) = convert(256, 32, &opaque);
        let s = s.unwrap();
        assert_eq!((s.w, s.h, s.fmt), (64, 32, FMT_RGBA16));
    }

    #[test]
    fn rgba16_packing() {
        let (f, _) = convert(1, 1, &[255, 0, 0, 255]);
        assert_eq!(f.data, alloc::vec![0xf8, 0x01]);
        let (f, _) = convert(1, 1, &[0, 0, 255, 0]);
        assert_eq!(f.data, alloc::vec![0x00, 0x3e]);
    }
}
