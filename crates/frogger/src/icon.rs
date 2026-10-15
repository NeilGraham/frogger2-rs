//! App icons. The shipped icon is our own artwork (`packaging/icon`, generated from
//! `tools/icon/make_svgs.py`). Once the player has imported their game, hosts switch to a
//! square crop of the game's own loading-screen artwork, made on the player's machine from
//! their data; it is never shipped or written anywhere but the player's own settings folder.

use crate::data::Data;

/// An RGBA8 image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// The game's loading-screen artwork (frog, logo, Swampy's jaws): 640×480.
pub const GAME_ART: &str = "textures/loading.fla";

/// Our icon at 256×256 (packaging/icon/png/256.png).
const APP_ICON_PNG: &[u8] = include_bytes!("../../../packaging/icon/png/256.png");

/// Our own icon (the default until game data exists).
pub fn app_icon() -> Rgba {
    decode_png(APP_ICON_PNG).expect("embedded app icon")
}

/// The game's artwork from the player's data, if present.
pub fn game_art(data: &Data) -> Option<Rgba> {
    let raw = data.read(GAME_ART).ok()?;
    let raw = f2_formats::fla::maybe_decompress(raw).ok()?;
    let t = f2_formats::texture::parse_backdrop(&raw).ok()?;
    Some(Rgba { width: t.width as u32, height: t.height as u32, pixels: t.to_rgba8() })
}

/// The centre square of `img` scaled to `size`×`size` (area average when shrinking,
/// bilinear when growing).
pub fn square(img: &Rgba, size: u32) -> Rgba {
    let side = img.width.min(img.height);
    let (x0, y0) = ((img.width - side) / 2, (img.height - side) / 2);
    if size > side {
        return bilinear(img, [x0, y0, side, side], size);
    }
    resample(img, [x0, y0, side, side], size, size)
}

/// `src` square of `img` scaled up to `size`×`size`, bilinear.
fn bilinear(img: &Rgba, [sx, sy, sw, sh]: [u32; 4], size: u32) -> Rgba {
    let px = |x: u32, y: u32, c: usize| img.pixels[((y * img.width + x) * 4) as usize + c] as f32;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        let fy = ((y as f32 + 0.5) * sh as f32 / size as f32 - 0.5).clamp(0.0, (sh - 1) as f32);
        let (y0, ty) = (fy.floor() as u32, fy.fract());
        let y1 = (y0 + 1).min(sh - 1);
        for x in 0..size {
            let fx = ((x as f32 + 0.5) * sw as f32 / size as f32 - 0.5).clamp(0.0, (sw - 1) as f32);
            let (x0, tx) = (fx.floor() as u32, fx.fract());
            let x1 = (x0 + 1).min(sw - 1);
            for c in 0..4 {
                let top = px(sx + x0, sy + y0, c) * (1.0 - tx) + px(sx + x1, sy + y0, c) * tx;
                let bot = px(sx + x0, sy + y1, c) * (1.0 - tx) + px(sx + x1, sy + y1, c) * tx;
                out.push((top * (1.0 - ty) + bot * ty).round() as u8);
            }
        }
    }
    Rgba { width: size, height: size, pixels: out }
}

/// `img` with rounded corners (radius `radius` × the side), transparent outside, with a
/// 4×4-supersampled edge.
pub fn rounded(img: &Rgba, radius: f32) -> Rgba {
    let (w, h) = (img.width as f32, img.height as f32);
    let r = radius * w.min(h);
    let mut out = img.clone();
    for y in 0..img.height {
        for x in 0..img.width {
            let mut cover = 0u32;
            for sy in 0..4 {
                for sx in 0..4 {
                    let (px, py) = (x as f32 + (sx as f32 + 0.5) / 4.0, y as f32 + (sy as f32 + 0.5) / 4.0);
                    let cx = px.clamp(r, w - r);
                    let cy = py.clamp(r, h - r);
                    let inside = (px - cx).powi(2) + (py - cy).powi(2) <= r * r;
                    cover += inside as u32;
                }
            }
            if cover < 16 {
                let i = ((y * img.width + x) * 4 + 3) as usize;
                out.pixels[i] = (out.pixels[i] as u32 * cover / 16) as u8;
            }
        }
    }
    out
}

/// `src` rectangle `[x, y, w, h]` of `img` resampled to `w`×`h` (area average when
/// shrinking, nearest when growing).
fn resample(img: &Rgba, [sx, sy, sw, sh]: [u32; 4], w: u32, h: u32) -> Rgba {
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        let y0 = sy + y * sh / h;
        let y1 = (sy + ((y + 1) * sh).div_ceil(h)).max(y0 + 1).min(sy + sh);
        for x in 0..w {
            let x0 = sx + x * sw / w;
            let x1 = (sx + ((x + 1) * sw).div_ceil(w)).max(x0 + 1).min(sx + sw);
            let mut acc = [0u32; 4];
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let i = ((yy * img.width + xx) * 4) as usize;
                    for (a, &p) in acc.iter_mut().zip(&img.pixels[i..i + 4]) {
                        *a += p as u32;
                    }
                }
            }
            let n = (x1 - x0) * (y1 - y0);
            out.extend(acc.map(|a| ((a + n / 2) / n) as u8));
        }
    }
    Rgba { width: w, height: h, pixels: out }
}

/// PNG file bytes.
pub fn encode_png(img: &Rgba) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, img.width, img.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    {
        let mut w = enc.write_header().expect("png header");
        w.write_image_data(&img.pixels).expect("png data");
    }
    out
}

/// Decodes an 8-bit RGBA or RGB PNG.
pub fn decode_png(bytes: &[u8]) -> Option<Rgba> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()?];
    let info = r.next_frame(&mut buf).ok()?;
    let px = &buf[..info.buffer_size()];
    let pixels = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px.chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        _ => return None,
    };
    Some(Rgba { width: info.width, height: info.height, pixels })
}

/// A Windows `.ico` holding `images` as PNG entries (Vista and later).
pub fn encode_ico(images: &[Rgba]) -> Vec<u8> {
    let pngs: Vec<Vec<u8>> = images.iter().map(encode_png).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(pngs.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * pngs.len() as u32;
    for (img, png) in images.iter().zip(&pngs) {
        out.push((img.width % 256) as u8);
        out.push((img.height % 256) as u8);
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for png in pngs {
        out.extend_from_slice(&png);
    }
    out
}

/// Android adaptive-icon bitmap (108 dp square of which launchers show the centre 72 dp):
/// the square artwork in the middle 80 units, the rest filled with the artwork's edge
/// colour so any launcher mask shows art, not a frame.
pub fn adaptive(art_square: &Rgba, size: u32) -> Rgba {
    let inner = size * 80 / 108;
    let off = (size - inner) / 2;
    let small = resample(art_square, [0, 0, art_square.width, art_square.height], inner, inner);
    // Average colour of the artwork's border.
    let (w, h) = (art_square.width, art_square.height);
    let mut acc = [0u64; 3];
    let mut n = 0u64;
    for y in 0..h {
        for x in 0..w {
            if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                let i = ((y * w + x) * 4) as usize;
                for (a, &p) in acc.iter_mut().zip(&art_square.pixels[i..i + 3]) {
                    *a += p as u64;
                }
                n += 1;
            }
        }
    }
    let edge = acc.map(|a| (a / n.max(1)) as u8);
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            if (off..off + inner).contains(&x) && (off..off + inner).contains(&y) {
                let i = (((y - off) * inner + (x - off)) * 4) as usize;
                pixels.extend_from_slice(&small.pixels[i..i + 4]);
            } else {
                pixels.extend_from_slice(&[edge[0], edge[1], edge[2], 255]);
            }
        }
    }
    Rgba { width: size, height: size, pixels }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 4]) -> Rgba {
        Rgba { width: w, height: h, pixels: (0..w * h).flat_map(|_| c).collect() }
    }

    #[test]
    fn app_icon_decodes() {
        let i = app_icon();
        assert_eq!((i.width, i.height), (256, 256));
        assert_eq!(i.pixels.len(), 256 * 256 * 4);
    }

    #[test]
    fn square_crop_is_centred() {
        // 640×480: left/right 80 px columns red, centre green.
        let mut img = solid(640, 480, [0, 255, 0, 255]);
        for y in 0..480 {
            for x in (0..80).chain(560..640) {
                let i = ((y * 640 + x) * 4) as usize;
                img.pixels[i..i + 4].copy_from_slice(&[255, 0, 0, 255]);
            }
        }
        let s = square(&img, 48);
        assert_eq!((s.width, s.height), (48, 48));
        assert!(s.pixels.chunks(4).all(|p| p == [0, 255, 0, 255]), "only the centre square");
    }

    #[test]
    fn png_and_ico_round_trip() {
        let img = solid(16, 16, [10, 20, 30, 255]);
        assert_eq!(decode_png(&encode_png(&img)).unwrap(), img);
        let ico = encode_ico(&[img.clone(), solid(32, 32, [1, 2, 3, 255])]);
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 2, 0]);
        assert_eq!(ico[6], 16);
        assert_eq!(ico[22], 32);
    }

    /// With imported data (`F2_GAME_DIR` or `game-data/usa`): the artwork is 640×480 and the
    /// icons are made from it. `F2_ICON_PREVIEW=<dir>` writes them for a look.
    #[test]
    fn game_art_icons_from_data() {
        let base = std::env::var("F2_GAME_DIR").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../game-data/usa").into());
        let Ok(data) = Data::locate(&f2_platform::Config::default(), Some(base.into())) else {
            eprintln!("no game data: skipped");
            return;
        };
        let art = game_art(&data).expect("loading-screen artwork");
        assert_eq!((art.width, art.height), (640, 480));
        let sq = square(&art, 256);
        let ad = adaptive(&square(&art, 480), 432);
        let ico = encode_ico(&[16, 32, 48, 256].map(|s| square(&art, s)));
        assert_eq!(&ico[4..6], &[4, 0]);
        if let Some(dir) = std::env::var_os("F2_ICON_PREVIEW") {
            let dir = std::path::PathBuf::from(dir);
            std::fs::write(dir.join("game-art-256.png"), encode_png(&sq)).unwrap();
            std::fs::write(dir.join("game-art-adaptive.png"), encode_png(&ad)).unwrap();
            std::fs::write(dir.join("game-art.ico"), ico).unwrap();
        }
    }

    #[test]
    fn rounded_corners() {
        let r = rounded(&solid(100, 100, [9, 9, 9, 255]), 0.2);
        assert_eq!(r.pixels[3], 0, "corner transparent");
        assert_eq!(r.pixels[((50 * 100 + 50) * 4 + 3) as usize], 255, "centre opaque");
        assert_eq!(r.pixels[((50 * 100) * 4 + 3) as usize], 255, "edge middle opaque");
        let up = square(&solid(48, 32, [1, 2, 3, 255]), 64);
        assert_eq!((up.width, up.height), (64, 64));
        assert!(up.pixels.chunks(4).all(|p| p == [1, 2, 3, 255]));
    }

    #[test]
    fn adaptive_border_is_edge_colour() {
        let a = adaptive(&solid(64, 64, [200, 30, 10, 255]), 108);
        assert_eq!((a.width, a.height), (108, 108));
        assert_eq!(&a.pixels[0..4], &[200, 30, 10, 255]);
    }
}
