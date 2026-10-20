//! Material maps made from a texture (Enhanced graphics: normal mapping and parallax occlusion
//! on textures that have none). From the texture's light and dark relative to its
//! surroundings: a height map (dark seams and cracks low, faces high, flattened into
//! plateaus), a tangent-space normal map (OpenGL, +Y up in the image), and ORM (R occlusion
//! from the height, G roughness, B metal 0). Everything wraps at the edges, as the texture
//! tiles. Used for the port's built-in materials (`f2-engine`) and by `f2-modkit pbr-maps`.

use alloc::vec;
use alloc::vec::Vec;

/// How to make a texture's maps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Recipe {
    /// 0: smooth relief from the texture; 1: flat-topped blocks with bevelled edges.
    pub plateau: f32,
    /// Parallax depth (texture coordinate units per full height).
    pub depth: f32,
    /// Roughness of the faces (the seams a little rougher).
    pub rough: f32,
    /// Normal map steepness.
    pub strength: f32,
    /// How much the crevices darken (0..1).
    pub ao: f32,
    /// Fine relief from the texture kept on the faces.
    pub detail: f32,
    /// Width of the slope from a face down into a seam, in texels of the texture.
    pub bevel: f32,
    /// How much darker than its surroundings a seam is (0..1 of the relief): lower, only the
    /// darkest lines are seams.
    pub seam: f32,
    /// The fewest texels a seam has (shorter dark pieces are spots in the art, not seams).
    pub min_seam: f32,
    /// The seams are light (the faces dark).
    pub invert: bool,
    /// The texture is one tile (paving, a flagstone): its rim, this many texels in from each
    /// edge, is the grout (half of the joint with the next tile, which the texture's wrap
    /// gives), and the face rises from it over `bevel` texels with rounded corners (straight,
    /// even edges whatever the art's rim looks like). 0: no rim (seams from the art only;
    /// `seam` 0: none).
    pub tile: f32,
}

impl Recipe {
    /// This recipe with `key=value` settings from `spec` (`plateau=0.8,depth=0.02`; the
    /// names of the fields; unknown keys are ignored).
    pub fn with(mut self, spec: &str) -> Recipe {
        for kv in spec.split(',') {
            let Some((k, v)) = kv.split_once('=') else { continue };
            let Ok(x) = v.trim().parse::<f32>() else { continue };
            match k.trim() {
                "plateau" => self.plateau = x,
                "depth" => self.depth = x,
                "rough" => self.rough = x,
                "strength" => self.strength = x,
                "ao" => self.ao = x,
                "detail" => self.detail = x,
                "bevel" => self.bevel = x,
                "seam" => self.seam = x,
                "min_seam" => self.min_seam = x,
                "invert" => self.invert = x != 0.0,
                "tile" => self.tile = x,
                _ => {}
            }
        }
        self
    }
}

impl Default for Recipe {
    fn default() -> Recipe {
        Recipe { plateau: 0.5, depth: 0.03, rough: 0.8, strength: 6.0, ao: 0.6, detail: 0.3, bevel: 1.0, seam: 0.15, min_seam: 3.0, invert: false, tile: 0.0 }
    }
}

/// A texture's maps, `size`² each, RGBA8 (the height in R, G, B).
pub struct Maps {
    pub size: u32,
    pub height: Vec<u8>,
    pub normal: Vec<u8>,
    pub orm: Vec<u8>,
}

/// Square grid of floats, wrapping.
struct Grid {
    n: usize,
    v: Vec<f32>,
}

impl Grid {
    fn at(&self, x: isize, y: isize) -> f32 {
        let n = self.n as isize;
        self.v[(y.rem_euclid(n) * n + x.rem_euclid(n)) as usize]
    }
    /// Gaussian blur (sigma in cells), wrapping: three box blurs of running sums each way
    /// (the cost does not grow with sigma).
    fn blur(&self, sigma: f32) -> Grid {
        if sigma < 0.3 {
            return Grid { n: self.n, v: self.v.clone() };
        }
        // Box half-widths whose three passes approximate the gaussian (Kovesi).
        let wi = (12.0 * sigma * sigma / 3.0 + 1.0).sqrt().floor() as usize;
        let wl = if wi % 2 == 0 { wi - 1 } else { wi };
        let m = ((12.0 * sigma * sigma - 3.0 * (wl * wl) as f32 - 12.0 * wl as f32 - 9.0) / (-4.0 * wl as f32 - 4.0)).round() as usize;
        let n = self.n;
        let mut v = self.v.clone();
        let mut tmp = vec![0.0f32; n * n];
        for pass in 0..3 {
            let r = (if pass < m { wl } else { wl + 2 }) / 2;
            box_pass(&v, &mut tmp, n, r, true);
            box_pass(&tmp, &mut v, n, r, false);
        }
        Grid { n, v }
    }
}

/// One box blur of radius `r` along rows (`rows`) or columns, wrapping.
fn box_pass(src: &[f32], dst: &mut [f32], n: usize, r: usize, rows: bool) {
    let idx = |line: usize, i: usize| if rows { line * n + i % n } else { (i % n) * n + line };
    let w = (2 * r + 1) as f32;
    for line in 0..n {
        let mut acc: f32 = (0..=2 * r).map(|k| src[idx(line, (k + n * (r / n + 1) - r) % n)]).sum();
        for i in 0..n {
            dst[idx(line, i)] = acc / w;
            acc += src[idx(line, i + r + 1)] - src[idx(line, (i + n * (r / n + 1) - r) % n)];
        }
    }
}

/// Distance (cells) of every cell from the nearest `true` cell, wrapping (two-pass chamfer,
/// run twice so distances carry across the edges).
fn chamfer(mask: &[bool], n: usize) -> Vec<f32> {
    let big = (n * 2) as f32;
    let mut d: Vec<f32> = mask.iter().map(|m| if *m { 0.0 } else { big }).collect();
    let w = |x: isize, y: isize| (y.rem_euclid(n as isize) as usize) * n + x.rem_euclid(n as isize) as usize;
    let (a, b) = (1.0f32, core::f32::consts::SQRT_2);
    for _ in 0..2 {
        for y in 0..n as isize {
            for x in 0..n as isize {
                let i = w(x, y);
                let v = d[i].min(d[w(x - 1, y)] + a).min(d[w(x, y - 1)] + a).min(d[w(x - 1, y - 1)] + b).min(d[w(x + 1, y - 1)] + b);
                d[i] = v;
            }
        }
        for y in (0..n as isize).rev() {
            for x in (0..n as isize).rev() {
                let i = w(x, y);
                let v = d[i].min(d[w(x + 1, y)] + a).min(d[w(x, y + 1)] + a).min(d[w(x + 1, y + 1)] + b).min(d[w(x - 1, y + 1)] + b);
                d[i] = v;
            }
        }
    }
    d
}

/// `mask` without its connected pieces (8-neighbours, wrapping) of fewer than `min` cells.
fn drop_small(mask: &[bool], w: usize, h: usize, min: usize) -> Vec<bool> {
    let mut out = mask.to_vec();
    let mut seen = vec![false; w * h];
    for start in 0..w * h {
        if !mask[start] || seen[start] {
            continue;
        }
        let (mut stack, mut piece) = (vec![start], vec![]);
        seen[start] = true;
        while let Some(i) = stack.pop() {
            piece.push(i);
            let (x, y) = ((i % w) as isize, (i / w) as isize);
            for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                let j = ((y + dy).rem_euclid(h as isize) as usize) * w + (x + dx).rem_euclid(w as isize) as usize;
                if mask[j] && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
        if piece.len() < min {
            for i in piece {
                out[i] = false;
            }
        }
    }
    out
}

/// The value at fraction `q` of the sorted values.
fn percentile(v: &[f32], q: f32) -> f32 {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    s[((s.len() - 1) as f32 * q) as usize]
}

/// Maps for an RGBA8 texture `w`×`h` (any size; resampled to `size`², bilinear, wrapping).
pub fn generate(rgba: &[u8], w: u32, h: u32, size: u32, r: &Recipe) -> Maps {
    let n = size as usize;
    let (w, h) = (w as usize, h as usize);
    let lum = |x: usize, y: usize| {
        let p = &rgba[(y * w + x) * 4..];
        let l = (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0;
        if r.invert {
            1.0 - l
        } else {
            l
        }
    };
    // Upscale: bilinear on the texel centres, wrapping (the blurs below smooth it).
    let mut l = vec![0.0; n * n];
    for y in 0..n {
        for x in 0..n {
            let fx = (x as f32 + 0.5) * w as f32 / n as f32 - 0.5;
            let fy = (y as f32 + 0.5) * h as f32 / n as f32 - 0.5;
            let (x0, y0) = (fx.floor(), fy.floor());
            let (tx, ty) = (fx - x0, fy - y0);
            let wx = |i: f32| (i as isize).rem_euclid(w as isize) as usize;
            let wy = |i: f32| (i as isize).rem_euclid(h as isize) as usize;
            let a = lum(wx(x0), wy(y0)) * (1.0 - tx) + lum(wx(x0 + 1.0), wy(y0)) * tx;
            let b = lum(wx(x0), wy(y0 + 1.0)) * (1.0 - tx) + lum(wx(x0 + 1.0), wy(y0 + 1.0)) * tx;
            l[y * n + x] = a * (1.0 - ty) + b * ty;
        }
    }
    let l = Grid { n, v: l };
    let k = n as f32 / w.max(h) as f32; // cells per texel
                                        // Relief: lighter or darker than the surroundings.
    let base = l.blur(k * 0.6);
    let wide = l.blur(k * 6.0);
    let local: Vec<f32> = base.v.iter().zip(&wide.v).map(|(a, b)| a - b).collect();
    let (lo, hi) = (percentile(&local, 0.03), percentile(&local, 0.97));
    let fine_b = l.blur(k * 1.2);
    let fine: Vec<f32> = l.v.iter().zip(&fine_b.v).map(|(a, b)| a - b).collect();
    let fmax = fine.iter().fold(1e-6f32, |m, v| m.max(v.abs()));
    let t: Vec<f32> = local.iter().map(|v| ((v - lo) / (hi - lo).max(1e-6)).clamp(0.0, 1.0)).collect();
    // Seams, found at the texture's own size: lines darker than the texels on both sides of
    // them (one or two texels wide; one-sided shading is no seam), then scaled up.
    let tex: Vec<f32> = (0..w * h).map(|i| lum(i % w, i / w)).collect();
    let tl = |x: isize, y: isize| tex[(y.rem_euclid(h as isize) as usize) * w + x.rem_euclid(w as isize) as usize];
    let (tlo, thi) = (percentile(&tex, 0.03), percentile(&tex, 0.97));
    let margin = r.seam * (thi - tlo).max(1e-3);
    let mut tseam = vec![false; w * h];
    for y in (0..h as isize).filter(|_| r.seam > 0.0) {
        for x in 0..w as isize {
            let c = tl(x, y);
            let valley = |dx: isize, dy: isize| {
                // one texel wide, or two (with the neighbour on one side)
                let one = c < tl(x - dx, y - dy) - margin && c < tl(x + dx, y + dy) - margin;
                let two_a = c < tl(x - dx, y - dy) - margin && tl(x + dx, y + dy) < tl(x + 2 * dx, y + 2 * dy) - margin;
                let two_b = c < tl(x + dx, y + dy) - margin && tl(x - dx, y - dy) < tl(x - 2 * dx, y - 2 * dy) - margin;
                one || two_a || two_b
            };
            tseam[y as usize * w + x as usize] = valley(1, 0) || valley(0, 1);
        }
    }
    // Lines, not specks: pieces of fewer than `min_seam` texels are not seams.
    let tseam = drop_small(&tseam, w, h, r.min_seam.max(1.0) as usize);
    // Up to the maps' size smoothly (the texel mask, bilinear: no staircase outlines), the
    // seam's middle at half.
    let sm = |x: isize, y: isize| if tseam[(y.rem_euclid(h as isize) as usize) * w + x.rem_euclid(w as isize) as usize] { 1.0f32 } else { 0.0 };
    let mut soft = vec![0.0f32; n * n];
    for y in 0..n {
        for x in 0..n {
            let fx = (x as f32 + 0.5) / k - 0.5;
            let fy = (y as f32 + 0.5) / k - 0.5;
            let (x0, y0) = (fx.floor(), fy.floor());
            let (tx, ty) = (fx - x0, fy - y0);
            let (xi, yi) = (x0 as isize, y0 as isize);
            let a = sm(xi, yi) * (1.0 - tx) + sm(xi + 1, yi) * tx;
            let b = sm(xi, yi + 1) * (1.0 - tx) + sm(xi + 1, yi + 1) * tx;
            soft[y * n + x] = a * (1.0 - ty) + b * ty;
        }
    }
    let soft = Grid { n, v: soft }.blur(k * 0.35);
    let seam: Vec<bool> = soft.v.iter().map(|v| *v > 0.4).collect();
    // Faces rise from the seams over `bevel` texels (straight, even edges), flat on top.
    let d = if seam.contains(&true) { chamfer(&seam, n) } else { vec![(n * 2) as f32; n * n] };
    let bw = (r.bevel * k).max(1.0);
    let mut hv = vec![0.0; n * n];
    for i in 0..n * n {
        let s = (d[i] / bw).clamp(0.0, 1.0);
        let mut plat = s * s * (3.0 - 2.0 * s);
        if r.tile > 0.0 {
            // In from the nearest edge (texels), the corners rounded (radius: the rim and the
            // bevel), then up the bevel from the rim.
            let (x, y) = ((i % n) as f32 + 0.5, (i / n) as f32 + 0.5);
            let (ex, ey) = (x.min(n as f32 - x) / k, y.min(n as f32 - y) / k);
            let rc = r.tile + r.bevel.max(0.5);
            let inward = if ex < rc && ey < rc { rc - ((rc - ex).powi(2) + (rc - ey).powi(2)).sqrt() } else { ex.min(ey) };
            let s = ((inward - r.tile) / r.bevel.max(0.25)).clamp(0.0, 1.0);
            plat = plat.min(s * s * (3.0 - 2.0 * s));
        }
        let relief = fine[i] / fmax * 0.08 * r.detail;
        hv[i] = if r.tile > 0.0 {
            // The face's own relief on top of the slab, none in the joint.
            plat * (r.plateau + t[i] * (1.0 - r.plateau) + relief)
        } else {
            t[i] * (1.0 - r.plateau) + plat * r.plateau + relief
        }
        .clamp(0.0, 1.0);
    }
    let hg = Grid { n, v: hv }.blur(k * 0.25);
    let (mn, mx) = hg.v.iter().fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
    let hv: Vec<f32> = hg.v.iter().map(|v| (v - mn) / (mx - mn).max(1e-6)).collect();
    let hg = Grid { n, v: hv };
    let cavb = hg.blur(k * 1.5);
    let s = r.strength * n as f32 / 128.0;
    let mut out = Maps { size, height: vec![0; n * n * 4], normal: vec![0; n * n * 4], orm: vec![0; n * n * 4] };
    let u8c = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    for y in 0..n {
        for x in 0..n {
            let i = y * n + x;
            let (xi, yi) = (x as isize, y as isize);
            let dx = (hg.at(xi + 1, yi) - hg.at(xi - 1, yi)) * 0.5;
            let dy = (hg.at(xi, yi + 1) - hg.at(xi, yi - 1)) * 0.5;
            let (nx, ny, nz) = (-dx * s, dy * s, 1.0);
            let ln = (nx * nx + ny * ny + nz * nz).sqrt();
            let h = hg.v[i];
            let cav = (cavb.v[i] - h).clamp(0.0, 1.0);
            let ao = (1.0 - cav * r.ao * 3.0 - (1.0 - h) * r.ao * 0.3).clamp(0.5, 1.0);
            let rough = (r.rough + cav * 0.6 + (1.0 - h) * 0.1).clamp(0.05, 1.0);
            out.height[i * 4..i * 4 + 4].copy_from_slice(&[u8c(h), u8c(h), u8c(h), 255]);
            out.normal[i * 4..i * 4 + 4].copy_from_slice(&[u8c(nx / ln * 0.5 + 0.5), u8c(ny / ln * 0.5 + 0.5), u8c(nz / ln * 0.5 + 0.5), 255]);
            out.orm[i * 4..i * 4 + 4].copy_from_slice(&[u8c(ao), u8c(rough), 0, 255]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One slab with a ragged dark rim: the joint is low all round the edge, straight (the same
    /// along each side whatever the rim's art), the face high and flat, the corners rounded.
    #[test]
    fn slab_relief() {
        let w = 16u32;
        let mut img = vec![0u8; (w * w * 4) as usize];
        for y in 0..w {
            for x in 0..w {
                // A rim one texel wide on two sides and two on the others, specks in the face.
                let rim = x == 0 || y == 0 || x >= w - 2 || y >= w - 2;
                let v = if rim {
                    70
                } else if (x * 7 + y * 3) % 11 == 0 {
                    160
                } else {
                    200
                };
                img[((y * w + x) * 4) as usize..][..4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        let n = 64usize;
        let m = generate(&img, w, w, n as u32, &Recipe { plateau: 0.9, tile: 1.0, bevel: 1.0, seam: 0.0, ..Recipe::default() });
        let h = |x: usize, y: usize| m.height[(y * n + x) * 4] as i32;
        for i in 8..n - 8 {
            // Every side's joint as low as the others, at the texture's edge (both sides of the
            // wrap), the face high.
            for (a, b) in [(h(i, 0), h(0, i)), (h(i, n - 1), h(n - 1, i)), (h(i, 0), h(i, n - 1))] {
                assert!(a < 40 && b < 40 && (a - b).abs() <= 8, "joint {a} {b} at {i}");
            }
            assert!(h(i, n / 2) > 200, "face {} at {i}", h(i, n / 2));
        }
        // The corner rounds off: lower at the diagonal than at the same distance in from a side.
        assert!(h(5, 5) + 20 < h(5, n / 2), "corner {} side {}", h(5, 5), h(5, n / 2));
    }

    /// Bricks: light faces with dark mortar lines. The faces come out high, the mortar low,
    /// the normal map tilts at the edges, and the maps tile.
    #[test]
    fn bricks_relief() {
        let w = 16u32;
        let mut img = vec![0u8; (w * w * 4) as usize];
        for y in 0..w {
            for x in 0..w {
                let mortar = y % 8 == 0 || (x + if (y / 8) % 2 == 0 { 0 } else { 4 }) % 8 == 0;
                let v = if mortar { 60 } else { 210 };
                img[((y * w + x) * 4) as usize..][..4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        let m = generate(&img, w, w, 64, &Recipe { plateau: 0.75, ..Recipe::default() });
        let h = |x: usize, y: usize| m.height[(y * 64 + x) * 4];
        assert!(h(14, 18) > 200 && h(14, 0) < 60, "face {} mortar {}", h(14, 18), h(14, 0));
        // Flat faces point straight out; the mortar's edges lean.
        let nz = |x: usize, y: usize| m.normal[(y * 64 + x) * 4 + 2];
        assert!(nz(14, 18) > 250 && nz(14, 3) < 250);
        // Tiles: the texture moved by 4 texels gives the same maps moved by 16 cells (no seam
        // at the edges).
        let mut moved = vec![0u8; img.len()];
        for y in 0..w {
            for x in 0..w {
                let from = ((y * w + (x + w - 4) % w) * 4) as usize;
                moved[((y * w + x) * 4) as usize..][..4].copy_from_slice(&img[from..from + 4]);
            }
        }
        let m2 = generate(&moved, w, w, 64, &Recipe { plateau: 0.75, ..Recipe::default() });
        for y in 0..64 {
            for x in 0..64 {
                let a = m.height[(y * 64 + (x + 64 - 16) % 64) * 4] as i32;
                assert!((a - m2.height[(y * 64 + x) * 4] as i32).abs() <= 1, "({x}, {y})");
            }
        }
    }
}
