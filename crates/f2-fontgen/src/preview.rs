//! Side-by-side sheet: each original font strip (nearest-neighbour) above its modern
//! version, wrapped into rows, over a sky-blue background.

use std::path::Path;

use f2_engine::font::ALPHABET;
use f2_engine::{Font, HiRes, RgbaImage};

const BG: [u8; 3] = [0x2d, 0x6f, 0xa3];
const ZOOM: f32 = 3.0;
const ROW_W: usize = 1800;

fn blend(dst: &mut [u8], src: &[u8]) {
    let a = src[3] as u32;
    for c in 0..3 {
        dst[c] = ((src[c] as u32 * a + dst[c] as u32 * (255 - a)) / 255) as u8;
    }
}

/// Lays out one font: returns rows of (original band, modern band) images of `ROW_W` width.
fn font_rows(strip: &Font, hr: &HiRes) -> Vec<RgbaImage> {
    let z = ZOOM as usize;
    let hr = hr.resampled(ZOOM);
    let ch = strip.height as usize * z;
    let band =
        |w: usize| RgbaImage { width: w as u32, height: (2 * ch) as u32, pixels: BG.iter().copied().chain([255]).cycle().take(w * 2 * ch * 4).collect() };
    let mut rows = vec![band(ROW_W)];
    let mut x = 8usize;
    for &b in ALPHABET {
        let Some(g) = strip.glyphs[b as usize] else { continue };
        if g.left >= strip.width {
            continue;
        }
        let cw = g.width as usize * z;
        if x + cw + 8 > ROW_W {
            rows.push(band(ROW_W));
            x = 8;
        }
        let img = rows.last_mut().unwrap();
        for y in 0..ch {
            for dx in 0..cw {
                let si = ((y / z) * strip.width as usize + g.left as usize + dx / z) * 4;
                let di = (y * ROW_W + x + dx) * 4;
                blend(&mut img.pixels[di..di + 4], &strip.rgba[si..si + 4]);
            }
        }
        if let Some(r) = hr.rects.get(b as usize).filter(|r| r[2] > 0.0) {
            let (rx, ry) = (r[0].round() as usize, r[1].round() as usize);
            for y in 0..ch.min(r[3].round() as usize) {
                for dx in 0..cw.min(r[2].round() as usize) {
                    let si = ((ry + y) * hr.width as usize + rx + dx) * 4;
                    let di = ((ch + y) * ROW_W + x + dx) * 4;
                    blend(&mut img.pixels[di..di + 4], &hr.rgba[si..si + 4]);
                }
            }
        }
        x += cw + 4;
    }
    rows
}

/// Writes the sheet for every modern font of `rendition` to `out`.
pub fn write(rendition: &Path, out: &Path) -> Result<(), String> {
    let dir = rendition.join("game/textures/font");
    let mut rows = Vec::new();
    for (bmp, f2mf, _) in crate::FONTS {
        let strip = Font::from_bmp(&std::fs::read(dir.join(bmp)).map_err(|e| e.to_string())?).ok_or("bad bitmap")?;
        let hr = HiRes::decode(&std::fs::read(dir.join(f2mf)).map_err(|e| e.to_string())?)?;
        rows.extend(font_rows(&strip, &hr));
    }
    let h: u32 = rows.iter().map(|r| r.height + 6).sum();
    let mut sheet = RgbaImage { width: ROW_W as u32, height: h, pixels: vec![0x10; ROW_W * h as usize * 4] };
    let mut y = 0usize;
    for r in &rows {
        let n = r.pixels.len();
        sheet.pixels[y * ROW_W * 4..y * ROW_W * 4 + n].copy_from_slice(&r.pixels);
        y += r.height as usize + 6;
    }
    sheet.pixels.chunks_mut(4).for_each(|p| p[3] = 255);
    sheet.write_png(out).map_err(|e| format!("{e:?}"))
}
