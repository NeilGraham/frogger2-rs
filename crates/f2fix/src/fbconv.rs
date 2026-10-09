//! Adapted from theme-park-world-recomp `crates/tpwfix/src/fbconv.rs` (MIT, same owner).
//!
//! Portable framebuffer helpers for the in-process dump (`win::dd`):
//! converting a locked DirectDraw surface (any RGB bit-mask format, 15/16,
//! 24 or 32 bpp) to 8-bit RGB, and encoding that as a PNG (zlib "stored"
//! blocks, CRC-32 and Adler-32; no compression library needed).
//!
//! Kept free of Win32 so it is unit-tested on the host with synthetic
//! pixels.

/// A surface pixel format, from `DDPIXELFORMAT` (`dwRGBBitCount` and the
/// R/G/B masks).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub bpp: u32,
    pub rmask: u32,
    pub gmask: u32,
    pub bmask: u32,
}

impl Format {
    /// 16-bpp 5-6-5.
    pub const RGB565: Format = Format {
        bpp: 16,
        rmask: 0xf800,
        gmask: 0x07e0,
        bmask: 0x001f,
    };
    /// 16-bpp x-5-5-5.
    pub const RGB555: Format = Format {
        bpp: 16,
        rmask: 0x7c00,
        gmask: 0x03e0,
        bmask: 0x001f,
    };
    /// 32-bpp x8r8g8b8.
    pub const XRGB8888: Format = Format {
        bpp: 32,
        rmask: 0x00ff_0000,
        gmask: 0x0000_ff00,
        bmask: 0x0000_00ff,
    };

    /// Bytes per pixel, or `None` for an unsupported depth.
    pub fn bytes_per_pixel(&self) -> Option<usize> {
        match self.bpp {
            15 | 16 => Some(2),
            24 => Some(3),
            32 => Some(4),
            _ => None,
        }
    }
}

/// Expand the bits of `v` selected by `mask` to 0..=255 (bit replication,
/// so 5-bit 31 becomes 255 and 6-bit 63 becomes 255).
pub fn channel(v: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let bits = (mask >> shift).count_ones();
    let x = (v & mask) >> shift;
    if bits >= 8 {
        return (x >> (bits - 8)) as u8;
    }
    // Replicate the top bits into the low bits.
    let mut out = x << (8 - bits);
    let mut have = bits;
    while have < 8 {
        out |= out >> have;
        have *= 2;
    }
    (out & 0xff) as u8
}

/// Convert `h` rows of `w` pixels, `pitch` bytes apart, to packed RGB
/// (3 bytes per pixel, top row first). Returns `None` if the format or the
/// sizes do not fit `src`.
pub fn to_rgb(src: &[u8], w: usize, h: usize, pitch: usize, f: Format) -> Option<Vec<u8>> {
    let bpp = f.bytes_per_pixel()?;
    if w == 0 || h == 0 || pitch < w * bpp {
        return None;
    }
    let need = (h - 1).checked_mul(pitch)?.checked_add(w * bpp)?;
    if src.len() < need {
        return None;
    }
    let mut out = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        let row = &src[y * pitch..y * pitch + w * bpp];
        for px in row.chunks_exact(bpp) {
            let v = match bpp {
                2 => u16::from_le_bytes([px[0], px[1]]) as u32,
                3 => u32::from_le_bytes([px[0], px[1], px[2], 0]),
                _ => u32::from_le_bytes([px[0], px[1], px[2], px[3]]),
            };
            out.push(channel(v, f.rmask));
            out.push(channel(v, f.gmask));
            out.push(channel(v, f.bmask));
        }
    }
    Some(out)
}

/// Convert to packed RGBA8 (alpha 255) into `out` (cleared first; its
/// capacity is reused). Fast path for 16-bit 565 and 32-bit XRGB/ARGB.
/// False = bad geometry or format.
pub fn to_rgba_into(
    src: &[u8],
    w: usize,
    h: usize,
    pitch: usize,
    f: Format,
    out: &mut Vec<u8>,
) -> bool {
    let Some(bpp) = f.bytes_per_pixel() else {
        return false;
    };
    if w == 0 || h == 0 || pitch < w * bpp {
        return false;
    }
    let Some(need) = (h - 1)
        .checked_mul(pitch)
        .and_then(|n| n.checked_add(w * bpp))
    else {
        return false;
    };
    if src.len() < need {
        return false;
    }
    out.clear();
    out.reserve(w * h * 4);
    let is565 = bpp == 2 && f.rmask == 0xf800 && f.gmask == 0x07e0 && f.bmask == 0x001f;
    let is8888 = bpp == 4 && f.rmask == 0x00ff_0000 && f.gmask == 0xff00 && f.bmask == 0xff;
    for y in 0..h {
        let row = &src[y * pitch..y * pitch + w * bpp];
        if is565 {
            for px in row.as_chunks::<2>().0.iter() {
                let v = u16::from_le_bytes([px[0], px[1]]) as u32;
                let (r, g, b) = ((v >> 11) & 31, (v >> 5) & 63, v & 31);
                out.extend_from_slice(&[
                    ((r << 3) | (r >> 2)) as u8,
                    ((g << 2) | (g >> 4)) as u8,
                    ((b << 3) | (b >> 2)) as u8,
                    255,
                ]);
            }
        } else if is8888 {
            for px in row.as_chunks::<4>().0.iter() {
                out.extend_from_slice(&[px[2], px[1], px[0], 255]);
            }
        } else {
            for px in row.chunks_exact(bpp) {
                let v = match bpp {
                    2 => u16::from_le_bytes([px[0], px[1]]) as u32,
                    3 => u32::from_le_bytes([px[0], px[1], px[2], 0]),
                    _ => u32::from_le_bytes([px[0], px[1], px[2], px[3]]),
                };
                out.extend_from_slice(&[
                    channel(v, f.rmask),
                    channel(v, f.gmask),
                    channel(v, f.bmask),
                    255,
                ]);
            }
        }
    }
    true
}

/// CRC-32 (IEEE 802.3, as used by PNG), bitwise; fast enough for a few
/// screenshots.
pub fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    crc = !crc;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let m = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & m);
        }
    }
    !crc
}

/// Adler-32 (zlib trailer).
pub fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += x as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, ty: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(ty);
    out.extend_from_slice(data);
    let crc = crc32_update(crc32_update(0, ty), data);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// A zlib stream holding `raw` in uncompressed ("stored") deflate blocks.
pub fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    let mut z = Vec::with_capacity(raw.len() + raw.len() / 65535 * 5 + 16);
    z.extend_from_slice(&[0x78, 0x01]);
    let mut blocks = raw.chunks(65535).peekable();
    if blocks.peek().is_none() {
        z.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    while let Some(b) = blocks.next() {
        let last = blocks.peek().is_none();
        z.push(last as u8);
        let n = b.len() as u16;
        z.extend_from_slice(&n.to_le_bytes());
        z.extend_from_slice(&(!n).to_le_bytes());
        z.extend_from_slice(b);
    }
    z.extend_from_slice(&adler32(raw).to_be_bytes());
    z
}

/// Encode packed RGB (`w*h*3` bytes, top row first) as an 8-bit RGB PNG.
pub fn png_rgb(w: u32, h: u32, rgb: &[u8]) -> Option<Vec<u8>> {
    let row = (w as usize).checked_mul(3)?;
    if w == 0 || h == 0 || rgb.len() != row.checked_mul(h as usize)? {
        return None;
    }
    let mut raw = Vec::with_capacity((row + 1) * h as usize);
    for r in rgb.chunks_exact(row) {
        raw.push(0); // filter: none
        raw.extend_from_slice(r);
    }
    let mut out = Vec::with_capacity(raw.len() + 1024);
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = [0u8; 13];
    ihdr[..4].copy_from_slice(&w.to_be_bytes());
    ihdr[4..8].copy_from_slice(&h.to_be_bytes());
    ihdr[8] = 8; // bit depth
    ihdr[9] = 2; // colour type: RGB
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    chunk(&mut out, b"IEND", &[]);
    Some(out)
}

/// A cheap content digest of packed RGB (FNV-1a 32), for "did the frame
/// change" checks in logs.
pub fn digest(rgb: &[u8]) -> u32 {
    let mut h = 0x811c_9dc5u32;
    for &b in rgb {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

#[cfg(test)]
mod tests {
    #[test]
    fn rgba_matches_rgb() {
        use super::*;
        let src: Vec<u8> = (0..2 * 3 * 2).map(|i| (i * 37 + 11) as u8).collect();
        for f in [Format::RGB565] {
            let rgb = to_rgb(&src, 3, 2, 6, f).unwrap();
            let mut out = Vec::new();
            assert!(to_rgba_into(&src, 3, 2, 6, f, &mut out));
            for (a, b) in rgb.chunks(3).zip(out.chunks(4)) {
                assert_eq!(a, &b[..3]);
                assert_eq!(b[3], 255);
            }
        }
    }

    use super::*;

    #[test]
    fn channel_expands_5_and_6_bit() {
        assert_eq!(channel(0xf800, 0xf800), 255);
        assert_eq!(channel(0x07e0, 0x07e0), 255);
        assert_eq!(channel(0x001f, 0x001f), 255);
        assert_eq!(channel(0, 0xf800), 0);
        // 5-bit 16 -> 0b10000_100 = 132
        assert_eq!(channel(16 << 11, 0xf800), 132);
        // 6-bit 32 -> 0b100000_10 = 130
        assert_eq!(channel(32 << 5, 0x07e0), 130);
        assert_eq!(channel(0x12_3456, 0xff_0000), 0x12);
        assert_eq!(channel(5, 0), 0);
    }

    #[test]
    fn to_rgb_565_with_pitch() {
        // 2x2 pixels, pitch 6 (2 bytes padding per row).
        let px = |v: u16| v.to_le_bytes();
        let mut src = Vec::new();
        src.extend_from_slice(&px(0xf800)); // red
        src.extend_from_slice(&px(0x07e0)); // green
        src.extend_from_slice(&[0xaa, 0xaa]);
        src.extend_from_slice(&px(0x001f)); // blue
        src.extend_from_slice(&px(0xffff)); // white
        src.extend_from_slice(&[0xbb, 0xbb]);
        let rgb = to_rgb(&src, 2, 2, 6, Format::RGB565).unwrap();
        assert_eq!(rgb, vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255]);
    }

    #[test]
    fn to_rgb_555_and_32() {
        let src = 0x7c00u16.to_le_bytes();
        assert_eq!(
            to_rgb(&src, 1, 1, 2, Format::RGB555).unwrap(),
            vec![255, 0, 0]
        );
        let src = 0x0011_2233u32.to_le_bytes();
        assert_eq!(
            to_rgb(&src, 1, 1, 4, Format::XRGB8888).unwrap(),
            vec![0x11, 0x22, 0x33]
        );
    }

    #[test]
    fn to_rgb_rejects_short_or_bad() {
        assert!(to_rgb(&[0; 3], 2, 1, 4, Format::RGB565).is_none());
        assert!(to_rgb(&[0; 8], 2, 2, 2, Format::RGB565).is_none()); // pitch < w*bpp
        let f = Format {
            bpp: 8,
            ..Format::RGB565
        };
        assert!(to_rgb(&[0; 4], 2, 2, 2, f).is_none());
    }

    #[test]
    fn crc_and_adler_known_values() {
        assert_eq!(crc32_update(0, b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32_update(0, b"IEND"), 0xae42_6082);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
        assert_eq!(adler32(b""), 1);
    }

    #[test]
    fn zlib_stored_layout() {
        let z = zlib_stored(b"abc");
        assert_eq!(&z[..2], &[0x78, 0x01]);
        assert_eq!(&z[2..7], &[1, 3, 0, 0xfc, 0xff]);
        assert_eq!(&z[7..10], b"abc");
        assert_eq!(&z[10..], &adler32(b"abc").to_be_bytes());
        // Multi-block: 70000 bytes -> two blocks, only the second final.
        let raw = vec![7u8; 70000];
        let z = zlib_stored(&raw);
        assert_eq!(z[2], 0);
        assert_eq!(z[2 + 5 + 65535], 1);
        assert_eq!(z.len(), 2 + 5 + 65535 + 5 + (70000 - 65535) + 4);
    }

    #[test]
    fn png_structure() {
        let png = png_rgb(2, 1, &[255, 0, 0, 0, 0, 255]).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[16..20], &2u32.to_be_bytes());
        assert_eq!(&png[20..24], &1u32.to_be_bytes());
        assert_eq!(&png[png.len() - 12..], b"\0\0\0\0IEND\xaeB`\x82");
        // IHDR CRC covers type + data.
        let crc = u32::from_be_bytes(png[29..33].try_into().unwrap());
        assert_eq!(crc, crc32_update(0, &png[12..29]));
        assert!(png_rgb(2, 2, &[0; 6]).is_none());
        assert!(png_rgb(0, 1, &[]).is_none());
    }

    #[test]
    fn digest_changes() {
        assert_ne!(digest(&[1, 2, 3]), digest(&[1, 2, 4]));
    }
}
