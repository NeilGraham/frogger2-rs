//! PNG → `f2_formats::texture::Texture` (X1R5G5B5, the format of `.fla` banks).
//!
//! The texture type is chosen from the name prefix like the original bank
//! tool: `ai_` → alpha-intensity, `ac_` → alpha-colour, `prc` → procedural,
//! anything else → normal (non power-of-two sizes → plain surface). Encodings
//! are the inverse of [`Texture::to_rgba8`]:
//!
//! - normal / plain: RGB → 5:5:5; texels with alpha < 128 become the magenta
//!   key `0x7c1f` (normal only); an opaque texel that would encode to the key
//!   is nudged to `0x7c1e`.
//! - `ai_` / `prc`: the game draws white with alpha = blue channel; we store
//!   the PNG alpha (or the luminance when the PNG is fully opaque) as grey.
//! - `ac_`: alpha is derived by the game from the mean of R, G, B; RGB is
//!   stored as is.

use f2_formats::crc::crc;
use f2_formats::texture::{TexType, Texture, TEX_TRANSVAL};

use crate::Error;

/// A decoded RGBA8 image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Decodes any PNG (grey, grey+alpha, RGB, RGBA, palette; 1–16 bit) to RGBA8.
pub fn decode_png(data: &[u8]) -> Result<Rgba, Error> {
    let mut dec = png::Decoder::new(data);
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut r = dec.read_info().map_err(|e| Error::new(format!("png: {e}")))?;
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).map_err(|e| Error::new(format!("png: {e}")))?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width, info.height);
    let n = (w * h) as usize;
    let pixels = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf.as_chunks::<3>().0.iter().flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf.as_chunks::<2>().0.iter().flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err(Error::new("png: palette not expanded")),
    };
    if pixels.len() != n * 4 {
        return Err(Error::new("png: unexpected buffer size"));
    }
    Ok(Rgba { width: w, height: h, pixels })
}

/// Texture type for a texture name (`ai_`, `ac_`, `prc` prefixes).
pub fn tex_type_for_name(name: &str) -> TexType {
    let n = name.to_ascii_lowercase();
    if n.starts_with("ai_") {
        TexType::AlphaIntensity
    } else if n.starts_with("ac_") {
        TexType::AlphaColour
    } else if n.starts_with("prc") {
        TexType::Procedural
    } else {
        TexType::Normal
    }
}

fn flags_of(t: TexType) -> u32 {
    match t {
        TexType::Normal => 0,
        TexType::AlphaIntensity => 1,
        TexType::Procedural => 2,
        TexType::NoTexture => 3,
        TexType::AlphaColour => 4,
        TexType::Other(o) => o,
    }
}

/// CRC the game uses to address texture `name` (`crc("<name>.bmp")`).
pub fn texture_crc(name: &str) -> u32 {
    crc(&format!("{}.bmp", name.to_ascii_lowercase()))
}

#[inline]
fn rgb555(r: u8, g: u8, b: u8) -> u16 {
    (u16::from(r >> 3) << 10) | (u16::from(g >> 3) << 5) | u16::from(b >> 3)
}

/// Encodes RGBA8 pixels as a bank texture of type `ty`.
pub fn encode(img: &Rgba, ty: TexType) -> Vec<u16> {
    let px = img.pixels.as_chunks::<4>().0.iter();
    match ty {
        TexType::AlphaIntensity | TexType::Procedural => {
            let has_alpha = img.pixels.as_chunks::<4>().0.iter().any(|p| p[3] != 255);
            px.map(|p| {
                let a = if has_alpha { p[3] } else { ((u32::from(p[0]) * 77 + u32::from(p[1]) * 150 + u32::from(p[2]) * 29) >> 8) as u8 };
                rgb555(a, a, a)
            })
            .collect()
        }
        TexType::AlphaColour | TexType::NoTexture => px.map(|p| rgb555(p[0], p[1], p[2])).collect(),
        TexType::Normal | TexType::Other(_) => px
            .map(|p| {
                if p[3] < 128 {
                    TEX_TRANSVAL
                } else {
                    let v = rgb555(p[0], p[1], p[2]);
                    if v == TEX_TRANSVAL {
                        TEX_TRANSVAL - 1
                    } else {
                        v
                    }
                }
            })
            .collect(),
    }
}

/// Builds a texture named `name` from PNG bytes. `crc_override` keeps the CRC
/// of the entry being replaced (retail names may be truncated).
pub fn texture_from_png(name: &str, data: &[u8], crc_override: Option<u32>) -> Result<Texture, Error> {
    let img = decode_png(data)?;
    texture_from_rgba(name, &img, crc_override)
}

pub fn texture_from_rgba(name: &str, img: &Rgba, crc_override: Option<u32>) -> Result<Texture, Error> {
    texture_from_rgba_typed(name, img, crc_override, None)
}

/// Like [`texture_from_rgba`] with an explicit type (`None` = from the name).
pub fn texture_from_rgba_typed(name: &str, img: &Rgba, crc_override: Option<u32>, ty: Option<TexType>) -> Result<Texture, Error> {
    if img.width == 0 || img.height == 0 || img.width > i16::MAX as u32 || img.height > i16::MAX as u32 {
        return Err(Error::new(format!("texture {name}: size {}x{} out of range", img.width, img.height)));
    }
    let mut ty = ty.unwrap_or_else(|| tex_type_for_name(name));
    if ty == TexType::Normal && !(img.width.is_power_of_two() && img.height.is_power_of_two()) {
        ty = TexType::NoTexture;
    }
    let lower = name.to_ascii_lowercase();
    // TEXTURE_HEADER.name is char[12]: the bank tool keeps 11 characters + NUL.
    let stored: String = format!("{lower}.bmp").chars().take(11).collect();
    Ok(Texture {
        flags: flags_of(ty),
        crc: crc_override.unwrap_or_else(|| texture_crc(&lower)),
        width: img.width as u16,
        height: img.height as u16,
        name: stored,
        pixels: encode(img, ty),
    })
}

/// Serialises a bank in the decompressed `.fla` layout ([`TexBank::parse`] inverse).
pub fn write_texbank(bank: &f2_formats::texture::TexBank) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&(bank.textures.len() as u32).to_le_bytes());
    for t in &bank.textures {
        v.extend_from_slice(&t.flags.to_le_bytes());
        v.extend_from_slice(&t.crc.to_le_bytes());
        v.extend_from_slice(&(t.width as i16).to_le_bytes());
        v.extend_from_slice(&(t.height as i16).to_le_bytes());
        let mut name = [0u8; 12];
        for (d, s) in name.iter_mut().zip(t.name.bytes().take(11)) {
            *d = s;
        }
        v.extend_from_slice(&name);
        for p in &t.pixels {
            v.extend_from_slice(&p.to_le_bytes());
        }
    }
    v.extend_from_slice(&(bank.anims.len() as u32).to_le_bytes());
    for a in &bank.anims {
        v.extend_from_slice(&(a.frames.len() as u32).to_le_bytes());
        v.extend_from_slice(&a.base_crc.to_le_bytes());
        for f in &a.frames {
            v.extend_from_slice(&f.crc.to_le_bytes());
            v.extend_from_slice(&f.time.to_le_bytes());
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut e = png::Encoder::new(&mut out, w, h);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header().unwrap().write_image_data(rgba).unwrap();
        out
    }

    #[test]
    fn normal_round_trip_and_key() {
        let px = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 0, 255, 0, 255, 255];
        let t = texture_from_png("00wate04", &png(2, 2, &px), None).unwrap();
        assert_eq!(t.tex_type(), TexType::Normal);
        assert_eq!(t.crc, crc("00wate04.bmp"));
        assert_eq!(t.pixels, vec![0x7c00, 0x03e0, TEX_TRANSVAL, TEX_TRANSVAL - 1]);
        let back = t.to_rgba8();
        assert_eq!(&back[0..4], &[255, 0, 0, 255]);
        assert_eq!(back[11], 0);
    }

    #[test]
    fn prefixes_and_alpha_intensity() {
        let px = [0, 0, 0, 0, 0, 0, 0, 255];
        let t = texture_from_png("AI_glow", &png(2, 1, &px), None).unwrap();
        assert_eq!(t.tex_type(), TexType::AlphaIntensity);
        assert_eq!(t.name, "ai_glow.bmp");
        let back = t.to_rgba8();
        assert_eq!((back[3], back[7]), (0, 255));
        assert_eq!(tex_type_for_name("ac_x"), TexType::AlphaColour);
        assert_eq!(tex_type_for_name("prcfire"), TexType::Procedural);
        let t = texture_from_png("odd", &png(3, 1, &[9; 12]), None).unwrap();
        assert_eq!(t.tex_type(), TexType::NoTexture);
    }

    #[test]
    fn bank_serialisation_round_trips() {
        let t = texture_from_png("averyverylongname", &png(1, 1, &[1, 2, 3, 255]), Some(0x1234)).unwrap();
        assert_eq!(t.name, "averyverylo");
        let b = f2_formats::texture::TexBank { textures: vec![t], anims: vec![] };
        let p = f2_formats::texture::TexBank::parse(&write_texbank(&b)).unwrap();
        assert_eq!(p.textures[0].crc, 0x1234);
        assert_eq!(p.textures[0].pixels, b.textures[0].pixels);
    }
}
