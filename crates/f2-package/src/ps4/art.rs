//! Console artwork from the branding art (shared by PS4 and PS5): the launcher icon and the
//! background picture, as `tools/ps4/make-art.sh` makes them with ffmpeg: the icon is the
//! square art, the background the art fitted to the height over a blurred, darkened copy of
//! itself.

use crate::{util, Image};

/// The icon: the (square) art at `size`×`size`, opaque.
pub(crate) fn icon(art: &Image, size: u32) -> Image {
    let sq = square(art);
    let mut img = if sq.width >= size { util::resize(&sq, size, size) } else { bilinear(&sq, size, size) };
    for p in img.rgba.as_chunks_mut::<4>().0 {
        p[3] = 255;
    }
    img
}

/// The centre square of `img`.
fn square(img: &Image) -> Image {
    let s = img.width.min(img.height);
    crop(img, (img.width - s) / 2, (img.height - s) / 2, s, s)
}

fn crop(img: &Image, x0: u32, y0: u32, w: u32, h: u32) -> Image {
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in y0..y0 + h {
        let i = ((y * img.width + x0) * 4) as usize;
        rgba.extend_from_slice(&img.rgba[i..i + (w * 4) as usize]);
    }
    Image { width: w, height: h, rgba }
}

/// Bilinear resampling (for enlarging).
pub(crate) fn bilinear(img: &Image, w: u32, h: u32) -> Image {
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    let sx = img.width as f32 / w as f32;
    let sy = img.height as f32 / h as f32;
    let px = |x: u32, y: u32| ((y * img.width + x) * 4) as usize;
    for y in 0..h {
        let fy = ((y as f32 + 0.5) * sy - 0.5).clamp(0.0, (img.height - 1) as f32);
        let (y0, ty) = (fy.floor() as u32, fy.fract());
        let y1 = (y0 + 1).min(img.height - 1);
        for x in 0..w {
            let fx = ((x as f32 + 0.5) * sx - 0.5).clamp(0.0, (img.width - 1) as f32);
            let (x0, tx) = (fx.floor() as u32, fx.fract());
            let x1 = (x0 + 1).min(img.width - 1);
            for c in 0..4 {
                let a = img.rgba[px(x0, y0) + c] as f32 * (1.0 - tx) + img.rgba[px(x1, y0) + c] as f32 * tx;
                let b = img.rgba[px(x0, y1) + c] as f32 * (1.0 - tx) + img.rgba[px(x1, y1) + c] as f32 * tx;
                out.push((a * (1.0 - ty) + b * ty).round() as u8);
            }
        }
    }
    Image { width: w, height: h, rgba: out }
}

/// `w`×`h` background: the art fitted to the height, centred over a blurred copy that covers
/// the whole picture, darkened.
pub(crate) fn backdrop(art: &Image, w: u32, h: u32) -> Image {
    // background: cover-crop, blur (shrink to 1/24 and back), darken
    let sq = square(art);
    let cover = if sq.width >= w { util::resize(&sq, w, w) } else { bilinear(&sq, w, w) };
    let cover = crop(&cover, 0, (w - h.min(w)) / 2, w, h.min(w));
    let small = util::resize(&cover, (w / 24).max(1), (h / 24).max(1));
    let mut bg = bilinear(&small, w, h);
    for p in bg.rgba.as_chunks_mut::<4>().0 {
        for c in &mut p[..3] {
            *c = (*c as f32 * 0.8 - 12.0).clamp(0.0, 255.0) as u8;
        }
        p[3] = 255;
    }
    // foreground: the art at full height
    let fg = if sq.height >= h { util::resize(&sq, h, h) } else { bilinear(&sq, h, h) };
    let x0 = (w.saturating_sub(h)) / 2;
    for y in 0..h {
        for x in 0..fg.width.min(w) {
            let s = ((y * fg.width + x) * 4) as usize;
            let d = ((y * w + x0 + x) * 4) as usize;
            let a = fg.rgba[s + 3] as u32;
            for c in 0..3 {
                bg.rgba[d + c] = ((fg.rgba[s + c] as u32 * a + bg.rgba[d + c] as u32 * (255 - a) + 127) / 255) as u8;
            }
        }
    }
    bg
}

/// An 8-bit RGB PNG (what the consoles' own tools write for icon0/pic files).
pub(crate) fn png_rgb(img: &Image) -> Vec<u8> {
    let rgb: Vec<u8> = img.rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, img.width, img.height);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Balanced);
    {
        let mut w = enc.write_header().expect("png header");
        w.write_image_data(&rgb).expect("png data");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_art(s: u32) -> Image {
        let mut rgba = Vec::new();
        for y in 0..s {
            for x in 0..s {
                rgba.extend([(x * 255 / s) as u8, (y * 255 / s) as u8, 128, 255]);
            }
        }
        Image { width: s, height: s, rgba }
    }

    #[test]
    fn sizes() {
        let art = test_art(64);
        let i = icon(&art, 32);
        assert_eq!((i.width, i.height), (32, 32));
        let b = backdrop(&art, 192, 108);
        assert_eq!((b.width, b.height, b.rgba.len()), (192, 108, 192 * 108 * 4));
        // the centre column shows the art itself (undarkened)
        let c = ((54 * 192 + 96) * 4) as usize;
        assert!(b.rgba[c + 2] >= 126);
        let png = png_rgb(&i);
        assert_eq!(&png[1..4], b"PNG");
    }
}
