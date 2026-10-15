//! Helpers shared by the exporters.

use crate::Image;
use std::path::Path;

/// A title as a file name on every OS: "Frogger 2: Swampy's Revenge" -> "Frogger 2 - Swampy's Revenge".
pub fn file_stem(title: &str) -> String {
    let s = title.replace(": ", " - ");
    let s: String = s.chars().map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() { '-' } else { c }).collect();
    s.trim_end_matches(['.', ' ']).to_string()
}

/// PNG file bytes of `img`.
pub fn png(img: &Image) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, img.width, img.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    {
        let mut w = enc.write_header().expect("png header");
        w.write_image_data(&img.rgba).expect("png data");
    }
    out
}

/// `img` resampled to `w`×`h` (area average when shrinking, nearest when growing).
pub fn resize(img: &Image, w: u32, h: u32) -> Image {
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        let y0 = y * img.height / h;
        let y1 = ((y + 1) * img.height).div_ceil(h).max(y0 + 1).min(img.height);
        for x in 0..w {
            let x0 = x * img.width / w;
            let x1 = ((x + 1) * img.width).div_ceil(w).max(x0 + 1).min(img.width);
            let mut acc = [0u32; 4];
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let i = ((yy * img.width + xx) * 4) as usize;
                    for (a, &p) in acc.iter_mut().zip(&img.rgba[i..i + 4]) {
                        *a += p as u32;
                    }
                }
            }
            let n = (x1 - x0) * (y1 - y0);
            out.extend(acc.map(|a| ((a + n / 2) / n) as u8));
        }
    }
    Image { width: w, height: h, rgba: out }
}

/// Every regular file under `dir` as (path relative to `dir` with `/` separators, full path),
/// sorted.
pub fn walk(dir: &Path) -> std::io::Result<Vec<(String, std::path::PathBuf)>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)? {
            let e = e?;
            let p = e.path();
            if e.file_type()?.is_dir() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(dir).unwrap().components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
                out.push((rel, p));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// The game files of a rendition to ship: `game/`, `music/`, `video/` and `manifest.json`
/// (never the retail exe the developer oracle may have extracted into `oracle/`).
pub fn rendition_files(rendition: &Path) -> std::io::Result<Vec<(String, std::path::PathBuf)>> {
    Ok(walk(rendition)?.into_iter().filter(|(rel, _)| rel == "manifest.json" || ["game/", "music/", "video/"].iter().any(|p| rel.starts_with(p))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems() {
        assert_eq!(file_stem("Frogger 2: Swampy's Revenge"), "Frogger 2 - Swampy's Revenge");
        assert_eq!(file_stem("a/b?c."), "a-b-c");
    }

    #[test]
    fn resizing() {
        let img = Image { width: 2, height: 2, rgba: vec![0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 255] };
        let r = resize(&img, 1, 1);
        assert_eq!(r.rgba, vec![128, 128, 128, 255]);
    }
}
