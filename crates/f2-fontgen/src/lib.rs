//! Modern game fonts. The pixel fonts `bigfont.bmp` / `smallfont.bmp` were rasterised from
//! Monotype's Gill Sans Ultra Bold; this crate re-renders every glyph from that typeface at
//! [`SCALE`]× into a [`HiRes`] atlas, fitted to the original glyph cells (same widths, cap
//! height, baseline and horizontal centre, so layout is unchanged) and restyled like the
//! originals (white fill; `bigfont` a 1 px black outline, `smallfont` a 1 px hard drop shadow).
//!
//! The typeface is commercial (it ships with Windows/Office), so it is never bundled: it is
//! read from the user's system fonts ([`find_typeface`]) and the generated atlases live in the
//! user's data directory beside the bitmaps they replace (`game/textures/font/*.f2mf`).

pub mod preview;

use std::path::{Path, PathBuf};

use ab_glyph::{point, Font as _, FontRef, Glyph, PxScale};
use f2_engine::font::ALPHABET;
use f2_engine::{Font, HiRes};

/// Atlas pixels per original pixel (bigfont cells become 204 px tall).
pub const SCALE: u32 = 6;
/// The typeface's usual file name (Windows / Office).
pub const TYPEFACE_FILE: &str = "GILSANUB.TTF";

/// How a font's glyphs are dressed around the white fill.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    /// Outline width all around, in original pixels (0 = none).
    pub outline: f32,
    /// Drop-shadow offset (right, down) in original pixels, of the outlined glyph.
    pub shadow: (f32, f32),
}

/// `bigfont.bmp`: 1 px black outline (the drop shadow nudge matches its heavier lower-right).
pub const BIG: Style = Style { outline: 1.0, shadow: (0.35, 0.35) };
/// `smallfont.bmp`: no outline, 1 px black shadow right and below.
pub const SMALL: Style = Style { outline: 0.0, shadow: (1.0, 1.0) };

/// The fonts that get a modern version: (bitmap, generated file, style).
pub const FONTS: [(&str, &str, Style); 2] = [("bigfont.bmp", "bigfont.f2mf", BIG), ("smallfont.bmp", "smallfont.f2mf", SMALL)];

const SHADOW_RGB: u8 = 0x10;
const ATLAS_W: usize = 2048;

/// What [`build`] did besides the atlas.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Characters the typeface lacks (drawn from the upscaled original pixels).
    pub missing: Vec<char>,
    /// Characters narrowed to fit their original cell, with the horizontal scale.
    pub squeezed: Vec<(char, f32)>,
}

/// Rows/columns of an original glyph cell holding fill (white) pixels.
fn fill_at(strip: &Font, x: usize, y: usize) -> bool {
    let i = (y * strip.width as usize + x) * 4;
    let p = &strip.rgba[i..i + 4];
    p[3] > 0 && (p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000 >= 128
}

/// Squared Euclidean distance transform (Felzenszwalb & Huttenlocher), 1D.
fn edt_1d(f: &[f32], d: &mut [f32], v: &mut [usize], z: &mut [f32]) {
    let n = f.len();
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;
    for q in 1..n {
        loop {
            let p = v[k];
            let s = ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32)) / (2.0 * q as f32 - 2.0 * p as f32);
            if s <= z[k] && k > 0 {
                k -= 1;
                continue;
            }
            if s <= z[k] {
                // k == 0 and s <= -inf cannot happen; keep going
            }
            k += 1;
            v[k] = q;
            z[k] = s;
            z[k + 1] = f32::INFINITY;
            break;
        }
    }
    k = 0;
    for (q, dq) in d.iter_mut().enumerate().take(n) {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let p = v[k];
        *dq = (q as f32 - p as f32).powi(2) + f[p];
    }
}

/// Distance (pixels) from every pixel to the nearest `inside` pixel.
fn distance(inside: &[bool], w: usize, h: usize) -> Vec<f32> {
    const INF: f32 = 1e12;
    let mut g: Vec<f32> = inside.iter().map(|&b| if b { 0.0 } else { INF }).collect();
    let n = w.max(h);
    let (mut f, mut d, mut v, mut z) = (vec![0f32; n], vec![0f32; n], vec![0usize; n], vec![0f32; n + 1]);
    for x in 0..w {
        for y in 0..h {
            f[y] = g[y * w + x];
        }
        edt_1d(&f[..h], &mut d[..h], &mut v, &mut z);
        for y in 0..h {
            g[y * w + x] = d[y];
        }
    }
    for y in 0..h {
        f[..w].copy_from_slice(&g[y * w..y * w + w]);
        edt_1d(&f[..w], &mut d[..w], &mut v, &mut z);
        g[y * w..y * w + w].copy_from_slice(&d[..w]);
    }
    g.iter().map(|v| v.sqrt()).collect()
}

fn bilinear(a: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
    if x < 0.0 || y < 0.0 || x > (w - 1) as f32 || y > (h - 1) as f32 {
        return 0.0;
    }
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let top = a[y0 * w + x0] * (1.0 - fx) + a[y0 * w + x1] * fx;
    let bot = a[y1 * w + x0] * (1.0 - fx) + a[y1 * w + x1] * fx;
    top * (1.0 - fy) + bot * fy
}

/// Renders `strip`'s glyphs from the typeface `ttf` in `style`.
pub fn build(ttf: &[u8], strip: &Font, style: &Style) -> Result<(HiRes, Report), String> {
    let face = FontRef::try_from_slice(ttf).map_err(|e| format!("typeface: {e}"))?;
    let s = SCALE as usize;
    let sf = SCALE as f32;
    let (sw, sh) = (strip.width as usize, strip.height as usize);
    if strip.rgba.len() != sw * sh * 4 {
        return Err("font strip has no pixels".into());
    }
    // Cap height and baseline from the original 'H' fill.
    let hg = strip.glyphs[b'H' as usize].ok_or("font has no 'H'")?;
    let rows: Vec<usize> = (0..sh).filter(|&y| (hg.left as usize..(hg.left + hg.width) as usize).any(|x| fill_at(strip, x, y))).collect();
    let (top, bottom) = (*rows.first().ok_or("empty 'H'")?, *rows.last().unwrap());
    let cap_px = (bottom - top + 1) as f32;
    let baseline = (bottom + 1) as f32;
    let h_out = face.outline(face.glyph_id('H')).ok_or("typeface has no 'H'")?;
    let cap_units = (h_out.bounds.max.y - h_out.bounds.min.y).abs();
    let factor = cap_px * sf / cap_units; // atlas px per font unit
    let px_height = factor * face.height_unscaled();

    let pad = ((style.outline + style.shadow.0.max(style.shadow.1)) * sf).ceil() as usize + 2;
    let ch = sh * s;
    let mut report = Report::default();
    let mut cells: Vec<(u8, usize, Vec<u8>)> = Vec::new(); // (byte, cell width, rgba)
    for &b in ALPHABET {
        let Some(g) = strip.glyphs[b as usize] else { continue };
        if g.left >= strip.width {
            continue;
        }
        let (gl, gw) = (g.left as usize, (g.width as usize).min(sw - g.left as usize));
        let cw = gw * s;
        let c = b as char; // Latin-1
        let gid = face.glyph_id(c);
        let outline = (gid.0 != 0).then(|| face.outline(gid)).flatten();
        let Some(ol) = outline else {
            // Not in the typeface (or blank): nearest-neighbour upscale of the original.
            report.missing.push(c);
            let mut rgba = vec![0u8; cw * ch * 4];
            for y in 0..ch {
                for x in 0..cw {
                    let si = ((y / s) * sw + gl + x / s) * 4;
                    rgba[(y * cw + x) * 4..][..4].copy_from_slice(&strip.rgba[si..si + 4]);
                }
            }
            cells.push((b, cw, rgba));
            continue;
        };
        // Horizontal fit: centre on the original fill; narrow if wider than it.
        let cols: Vec<usize> = (0..gw).filter(|&x| (0..sh).any(|y| fill_at(strip, gl + x, y))).collect();
        let (fx0, fx1) = match (cols.first(), cols.last()) {
            (Some(&a), Some(&b)) => (a as f32, b as f32 + 1.0),
            _ => (0.0, gw as f32),
        };
        let ink_w = (ol.bounds.max.x - ol.bounds.min.x) * factor;
        // The ink may use the whole cell except the outline/shadow margins.
        // (bigfont's small shadow nudge may clip at the cell edge; smallfont's shadow is
        // its only dressing and keeps its room.)
        let m_l = style.outline * sf;
        let m_r = if style.outline > 0.0 { m_l } else { style.shadow.0 * sf };
        let target_w = (cw as f32 - m_l - m_r).max((fx1 - fx0 + 0.25) * sf);
        let sx = if ink_w > target_w { target_w / ink_w } else { 1.0 };
        if sx < 0.97 {
            report.squeezed.push((c, sx));
        }
        let (ink_x0, ink_w) = (ol.bounds.min.x * factor * sx, ink_w * sx);
        let left = ((fx0 + fx1) / 2.0 * sf - ink_w / 2.0).clamp(m_l, (cw as f32 - m_r - ink_w).max(m_l));
        let ox = left - ink_x0;
        let (ww, wh) = (cw + 2 * pad, ch + 2 * pad);
        let mut cov = vec![0f32; ww * wh];
        let glyph = Glyph { id: gid, scale: PxScale { x: px_height * sx, y: px_height }, position: point(ox + pad as f32, baseline * sf + pad as f32) };
        if let Some(og) = face.outline_glyph(glyph) {
            let bb = og.px_bounds();
            og.draw(|x, y, v| {
                let (px, py) = (bb.min.x as i32 + x as i32, bb.min.y as i32 + y as i32);
                if px >= 0 && py >= 0 && (px as usize) < ww && (py as usize) < wh {
                    cov[py as usize * ww + px as usize] = v.min(1.0);
                }
            });
        }
        // Outline (distance from the fill) and the shadow (offset copy of fill + outline).
        let ink: Vec<f32> = if style.outline > 0.0 {
            let inside: Vec<bool> = cov.iter().map(|&v| v >= 0.5).collect();
            let d = distance(&inside, ww, wh);
            let r = style.outline * sf;
            cov.iter().zip(&d).map(|(&c, &d)| c.max((r + 1.0 - d).clamp(0.0, 1.0))).collect()
        } else {
            cov.clone()
        };
        let (dx, dy) = (style.shadow.0 * sf, style.shadow.1 * sf);
        let mut rgba = vec![0u8; cw * ch * 4];
        for y in 0..ch {
            for x in 0..cw {
                let i = (y + pad) * ww + x + pad;
                let sh_a = bilinear(&ink, ww, wh, (x + pad) as f32 - dx, (y + pad) as f32 - dy);
                let o = if style.outline > 0.0 { ink[i] } else { 0.0 };
                let dark = sh_a + o - sh_a * o;
                let f = cov[i];
                let a = f + dark * (1.0 - f);
                let o4 = (y * cw + x) * 4;
                if a > 0.0 {
                    let lum = (f * 255.0 + dark * (1.0 - f) * SHADOW_RGB as f32) / a;
                    let l = (lum + 0.5).min(255.0) as u8;
                    rgba[o4..o4 + 4].copy_from_slice(&[l, l, l, (a * 255.0 + 0.5).min(255.0) as u8]);
                }
            }
        }
        cells.push((b, cw, rgba));
    }
    // Shelf-pack the cells (all `ch` tall) with a 2-original-pixel gap against bleeding when
    // the atlas is resampled and filtered.
    let gap = 2 * s;
    let mut placed = Vec::new();
    let (mut x, mut y) = (gap, gap);
    for (b, cw, _) in &cells {
        if x + cw + gap > ATLAS_W {
            x = gap;
            y += ch + gap;
        }
        placed.push((*b, x, y));
        x += cw + gap;
    }
    let ah = y + ch + gap;
    let mut atlas = vec![0u8; ATLAS_W * ah * 4];
    let mut rects = vec![[0f32; 4]; 256];
    for ((b, cw, rgba), &(_, px, py)) in cells.iter().zip(&placed) {
        for row in 0..ch {
            let d = ((py + row) * ATLAS_W + px) * 4;
            atlas[d..d + cw * 4].copy_from_slice(&rgba[row * cw * 4..(row + 1) * cw * 4]);
        }
        rects[*b as usize] = [px as f32, py as f32, *cw as f32, ch as f32];
    }
    Ok((HiRes { scale: sf, width: ATLAS_W as u32, height: ah as u32, rgba: atlas, rects }, report))
}

/// Looks for the typeface: `$F2_MODERN_FONT`, `<data>/fonts/`, then the system font folders
/// (Windows, WSL's view of them, Linux, macOS incl. Office's private fonts).
pub fn find_typeface(data_dirs: &[&Path]) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("F2_MODERN_FONT").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let mut dirs: Vec<PathBuf> = data_dirs.iter().map(|d| d.join("fonts")).collect();
    if let Some(w) = std::env::var_os("WINDIR") {
        dirs.push(Path::new(&w).join("Fonts"));
    }
    if let Some(l) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(Path::new(&l).join("Microsoft/Windows/Fonts"));
    }
    dirs.push("C:/Windows/Fonts".into());
    dirs.push("/mnt/c/Windows/Fonts".into());
    if let Some(h) = std::env::var_os("HOME") {
        let h = Path::new(&h);
        dirs.extend([h.join(".local/share/fonts"), h.join(".fonts"), h.join("Library/Fonts")]);
    }
    dirs.extend(
        [
            "/usr/share/fonts",
            "/usr/local/share/fonts",
            "/Library/Fonts",
            "/System/Library/Fonts/Supplemental",
            "/Applications/Microsoft Word.app/Contents/Resources/DFonts",
        ]
        .map(PathBuf::from),
    );
    dirs.iter().find_map(|d| search(d, 3))
}

fn search(dir: &Path, depth: u32) -> Option<PathBuf> {
    let rd = std::fs::read_dir(dir).ok()?;
    let mut subdirs = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(TYPEFACE_FILE)) && p.is_file() {
            return Some(p);
        }
        if depth > 0 && p.is_dir() {
            subdirs.push(p);
        }
    }
    subdirs.iter().find_map(|d| search(d, depth - 1))
}

/// Builds every modern font of an imported rendition (`<rendition>/game/textures/font/`)
/// from the typeface at `ttf`, writing the `.f2mf` files beside the bitmaps. Returns one
/// log line per font.
pub fn generate(rendition: &Path, ttf: &Path) -> Result<Vec<String>, String> {
    let ttf_bytes = std::fs::read(ttf).map_err(|e| format!("{}: {e}", ttf.display()))?;
    let dir = rendition.join("game/textures/font");
    let mut log = Vec::new();
    for (bmp, out, style) in FONTS {
        let p = dir.join(bmp);
        let data = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let strip = Font::from_bmp(&data).ok_or_else(|| format!("{}: not a font bitmap", p.display()))?;
        let (hr, rep) = build(&ttf_bytes, &strip, &style)?;
        let o = dir.join(out);
        std::fs::write(&o, hr.encode()).map_err(|e| format!("{}: {e}", o.display()))?;
        let mut line = format!("{}: {}x{} atlas", o.display(), hr.width, hr.height);
        if !rep.missing.is_empty() {
            line += &format!(", original pixels for {:?}", rep.missing.iter().collect::<String>());
        }
        if !rep.squeezed.is_empty() {
            let min = rep.squeezed.iter().map(|s| s.1).fold(1.0, f32::min);
            let list: String = rep.squeezed.iter().map(|(c, k)| format!(" {c}{:.0}%", k * 100.0)).collect();
            line += &format!(", {} glyphs narrowed (min {:.0}%):{list}", rep.squeezed.len(), min * 100.0);
        }
        log.push(line);
    }
    Ok(log)
}

/// Whether `rendition` already has every modern font file.
pub fn present(rendition: &Path) -> bool {
    FONTS.iter().all(|(_, out, _)| rendition.join("game/textures/font").join(out).is_file())
}

/// Generates the modern fonts for `rendition` if they are missing and the typeface can be
/// found. `Ok(None)` = nothing to do / typeface not installed.
pub fn ensure(rendition: &Path) -> Result<Option<Vec<String>>, String> {
    if present(rendition) {
        return Ok(None);
    }
    match find_typeface(&[rendition]) {
        Some(ttf) => generate(rendition, &ttf).map(Some),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_transform() {
        let (w, h) = (5, 1);
        let inside = [false, false, true, false, false];
        assert_eq!(distance(&inside, w, h), vec![2.0, 1.0, 0.0, 1.0, 2.0]);
        let inside2 = [true, false, false, false, true];
        assert_eq!(distance(&inside2, w, h), vec![0.0, 1.0, 2.0, 1.0, 0.0]);
    }
}
