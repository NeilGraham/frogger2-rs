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
        let px = rgba.as_chunks::<4>().0;
        let mut d = alloc::vec![0u8; px.len() * 2];
        for (o, p) in d.as_chunks_mut::<2>().0.iter_mut().zip(px) {
            let v = ((p[0] as u16 >> 3) << 11) | ((p[1] as u16 >> 3) << 6) | ((p[2] as u16 >> 3) << 1) | (p[3] >= 128) as u16;
            *o = v.to_be_bytes();
        }
        d
    };
    N64Tex { w, h, fmt, data }
}

/// An opaque RGBA8 image box-filtered to `sw`x`sh` (`w`, `h` multiples of them).
fn box_opaque(w: u32, h: u32, rgba: &[u8], sw: u32, sh: u32) -> Vec<u8> {
    let (fx, fy) = (w / sw, h / sh);
    let n = fx * fy;
    let mut out = Vec::with_capacity((sw * sh * 4) as usize);
    let mut acc = alloc::vec![[0u32; 3]; sw as usize];
    for oy in 0..sh {
        acc.iter_mut().for_each(|a| *a = [0; 3]);
        for sy in oy * fy..(oy + 1) * fy {
            let row = &rgba[(sy * w * 4) as usize..((sy + 1) * w * 4) as usize];
            for (ox, a) in acc.iter_mut().enumerate() {
                let base = ox * fx as usize * 4;
                for p in row[base..base + fx as usize * 4].as_chunks::<4>().0 {
                    a[0] += p[0] as u32;
                    a[1] += p[1] as u32;
                    a[2] += p[2] as u32;
                }
            }
        }
        for a in &acc {
            out.extend_from_slice(&[((a[0] + n / 2) / n) as u8, ((a[1] + n / 2) / n) as u8, ((a[2] + n / 2) / n) as u8, 255]);
        }
    }
    out
}

/// Halves an RGBA8 image (alpha-weighted colour, so colour-keyed texels do not bleed).
fn halve(w: u32, h: u32, rgba: &[u8], halve_w: bool, halve_h: bool) -> (u32, u32, Vec<u8>) {
    let nw = if halve_w { w.div_ceil(2) } else { w };
    let nh = if halve_h { h.div_ceil(2) } else { h };
    let mut out = Vec::with_capacity((nw * nh * 4) as usize);
    // Fully opaque (a composed story book page): the alpha-weighted average reduces exactly
    // to (sum + n/2) / n with n = 2 or 4 (the 255s cancel), shifts instead of divisions.
    if rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255) {
        for y in 0..nh {
            for x in 0..nw {
                let xs = if halve_w { [x * 2, (x * 2 + 1).min(w - 1)] } else { [x, x] };
                let ys = if halve_h { [y * 2, (y * 2 + 1).min(h - 1)] } else { [y, y] };
                let (sh, n) = match (halve_w, halve_h) {
                    (true, true) => (2, 4),
                    (false, false) => (0, 1),
                    _ => (1, 2),
                };
                let mut acc = [0u32; 3];
                let mut add = |sx: u32, sy: u32| {
                    let i = ((sy * w + sx) * 4) as usize;
                    for c in 0..3 {
                        acc[c] += rgba[i + c] as u32;
                    }
                };
                // The same texels as the weighted loop below (duplicates at odd edges included).
                match (halve_w, halve_h) {
                    (true, true) => {
                        for sy in ys {
                            for sx in xs {
                                add(sx, sy);
                            }
                        }
                    }
                    (true, false) => xs.iter().for_each(|&sx| add(sx, y)),
                    (false, true) => ys.iter().for_each(|&sy| add(x, sy)),
                    (false, false) => add(x, y),
                }
                for c in acc {
                    out.push(((c + n / 2) >> sh) as u8);
                }
                out.push(255);
            }
        }
        return (nw, nh, out);
    }
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
    // Opaque with an even reduction (a composed story book page, most level textures): one
    // box-filter pass to the halving loop's size instead of a pass per halving (rounded
    // once, so within 1 of the halved texels).
    if !partial && rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255) {
        let (mut sw, mut sh) = (w, h);
        while !fits(sw, sh, sfmt) {
            if sw >= sh {
                sw = sw.div_ceil(2);
            } else {
                sh = sh.div_ceil(2);
            }
        }
        if w % sw == 0 && h % sh == 0 {
            return (full, Some(encode(sw, sh, &box_opaque(w, h, rgba, sw, sh), sfmt)));
        }
    }
    // The first halving reads `rgba` itself (no whole RGBA8 copy: 256 KiB for a story book
    // page, which ran the frontend out of memory).
    let (mut sw, mut sh, mut px) = (w, h, None::<Vec<u8>>);
    while !fits(sw, sh, sfmt) {
        let wide = sw >= sh;
        let (nw, nh, n) = halve(sw, sh, px.as_deref().unwrap_or(rgba), wide, !wide);
        (sw, sh, px) = (nw, nh, Some(n));
    }
    (full, Some(encode(sw, sh, px.as_deref().unwrap_or(rgba), sfmt)))
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

    /// One box pass is within 1 of successive halvings.
    #[test]
    fn box_opaque_close_to_halving() {
        let (w, h) = (64u32, 32u32);
        let img: Vec<u8> = (0..w * h).flat_map(|i| [(i * 37) as u8, (i * 91 + 3) as u8, (i * 13 + 200) as u8, 255]).collect();
        let b = box_opaque(w, h, &img, 16, 8);
        let (w1, h1, p1) = halve(w, h, &img, true, true);
        let (_, _, p2) = halve(w1, h1, &p1, true, true);
        assert_eq!(b.len(), p2.len());
        assert!(b.iter().zip(&p2).all(|(a, c)| (*a as i32 - *c as i32).abs() <= 1));
    }

    /// The opaque shortcut of `halve` gives the alpha-weighted result exactly.
    #[test]
    fn halve_opaque_matches_weighted() {
        let (w, h) = (7u32, 5u32);
        let img: Vec<u8> = (0..w * h).flat_map(|i| [(i * 37) as u8, (i * 91 + 3) as u8, (i * 13 + 200) as u8, 255]).collect();
        // Weighted reference: the same image with one texel at alpha 254 elsewhere is not
        // opaque, so compute the reference by hand here.
        for (hw, hh) in [(true, true), (true, false), (false, true)] {
            let (nw, nh, fast) = halve(w, h, &img, hw, hh);
            for y in 0..nh {
                for x in 0..nw {
                    let xs = if hw { [x * 2, (x * 2 + 1).min(w - 1)] } else { [x, x] };
                    let ys = if hh { [y * 2, (y * 2 + 1).min(h - 1)] } else { [y, y] };
                    let mut acc = [0u32; 4];
                    let mut n = 0;
                    for sy in ys {
                        for sx in xs {
                            let i = ((sy * w + sx) * 4) as usize;
                            for c in 0..3 {
                                acc[c] += img[i + c] as u32 * 255;
                            }
                            acc[3] += 255;
                            n += 1;
                        }
                    }
                    let o = ((y * nw + x) * 4) as usize;
                    for c in 0..3 {
                        assert_eq!(fast[o + c] as u32, (acc[c] + acc[3] / 2) / acc[3], "{hw} {hh} {x} {y} {c}");
                    }
                    assert_eq!(fast[o + 3] as u32, (acc[3] + n / 2) / n);
                }
            }
        }
    }

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
