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
    /// A slab ([`Recipe::tile`]) redrawn at the maps' size ([`remaster`]; [`Maps::albedo`]),
    /// its face this mottled (0..1 of the art's own light and dark apart) and its relief the
    /// same mottling. 0: not redrawn (the art's relief kept, e.g. a pattern carved in it).
    pub paint: f32,
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
                "paint" => self.paint = x,
                _ => {}
            }
        }
        self
    }
}

impl Default for Recipe {
    fn default() -> Recipe {
        Recipe {
            plateau: 0.5,
            depth: 0.03,
            rough: 0.8,
            strength: 6.0,
            ao: 0.6,
            detail: 0.3,
            bevel: 1.0,
            seam: 0.15,
            min_seam: 3.0,
            invert: false,
            tile: 0.0,
            paint: 0.0,
        }
    }
}

/// A texture's maps, `size`² each, RGBA8 (the height in R, G, B).
pub struct Maps {
    pub size: u32,
    pub height: Vec<u8>,
    pub normal: Vec<u8>,
    pub orm: Vec<u8>,
    /// A slab's colour redrawn at the maps' size ([`Recipe::tile`]; none otherwise): the art's
    /// outline, bevel and face crisp along the slab's shape, its painted light and colours
    /// kept, its mottling painted finer (see [`remaster`]).
    pub albedo: Option<Vec<u8>>,
}

/// A whole number hash.
fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

/// Value noise 0..1 at (x, y) in 0..1 of the texture, `period` cells across, wrapping (it
/// tiles with the texture).
fn tnoise(x: f32, y: f32, period: u32, seed: u32) -> f32 {
    let (fx, fy) = (x * period as f32, y * period as f32);
    let (ix, iy) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - ix, fy - iy);
    let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
    let p = period as i64;
    let at = |i: f32, j: f32| {
        let (a, b) = ((i as i64).rem_euclid(p) as u32, (j as i64).rem_euclid(p) as u32);
        (hash(seed ^ hash(a.wrapping_mul(7919) ^ b.wrapping_mul(104_729))) >> 8) as f32 / 16_777_216.0
    };
    let (a, b, c, d) = (at(ix, iy), at(ix + 1.0, iy), at(ix, iy + 1.0), at(ix + 1.0, iy + 1.0));
    (a + (b - a) * sx) * (1.0 - sy) + (c + (d - c) * sx) * sy
}

/// Painted blotches: noise of a few sizes (-1..1, about 0.35 standard deviation), tiling.
fn mottle(x: f32, y: f32, seed: u32) -> f32 {
    let v = tnoise(x, y, 6, seed) * 0.42 + tnoise(x, y, 12, seed ^ 1) * 0.35 + tnoise(x, y, 24, seed ^ 2) * 0.17 + tnoise(x, y, 64, seed ^ 3) * 0.06;
    (v - 0.5) * 2.0
}

/// How far in from the nearest edge (texels of the art, `k` cells a texel) cell (x, y) of an
/// `n`-cell map is, the corners rounded with radius `rc` (a slab's outline; the same for the
/// height and the colour).
fn inward(x: f32, y: f32, n: f32, k: f32, rc: f32) -> f32 {
    let (ex, ey) = (x.min(n - x) / k, y.min(n - y) / k);
    if ex < rc && ey < rc {
        rc - ((rc - ex).powi(2) + (rc - ey).powi(2)).sqrt()
    } else {
        ex.min(ey)
    }
}

/// How much each side (left, right, top, bottom) is the nearest to (x, y) of an `n`-cell map
/// (`k` cells a texel): mostly the nearest, blending over about a texel where two meet (the
/// diagonals).
fn side_weights(x: f32, y: f32, n: f32, k: f32) -> [f32; 4] {
    let e = [x / k, (n - x) / k, y / k, (n - y) / k];
    let m = e.iter().fold(f32::MAX, |a, b| a.min(*b));
    let w = e.map(|d| (-(d - m) * 2.5).exp());
    let t: f32 = w.iter().sum();
    w.map(|v| v / t)
}

/// A slab texture (`w`×`h` RGBA8, one slab with a rim `r.tile` texels wide) redrawn at
/// `n`² in its own style: the art is split into its rim's profile (the colours of the
/// outline, the bevel and the face by how far in they are, all round) and what is left (its
/// painted light and colour, smooth); the profile is drawn again crisply along the slab's
/// outline, what is left is scaled up smoothly and added back, and the face's mottling is
/// painted at the new size with the art's own colours and contrast.
pub fn remaster(rgba: &[u8], w: usize, h: usize, n: usize, r: &Recipe) -> Vec<u8> {
    let px = |x: usize, y: usize| {
        let p = &rgba[(y * w + x) * 4..];
        [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]
    };
    let rc = r.tile + r.bevel.clamp(0.5, 1.5); // corners rounded about as the art's, however wide the bevel
                                               // The profile: mean colours of the rings 0..1, 1..2, 2..3 texels in from each side (the
                                               // art's light falls on some sides' bevels, not others), and of the face.
    let mut ring = [[[0.0f32; 4]; 4]; 4];
    let ring_of = |d: f32| (d.floor() as usize).min(3);
    for y in 0..h {
        for x in 0..w {
            let d = inward(x as f32 + 0.5, y as f32 + 0.5, w as f32, 1.0, rc);
            let c = px(x, y);
            let sw = side_weights(x as f32 + 0.5, y as f32 + 0.5, w as f32, 1.0);
            let bin = ring_of(d);
            for (side, wt) in sw.iter().enumerate() {
                // The face is one colour, whichever side is nearest.
                let rg = &mut ring[if bin == 3 { 0 } else { side }][bin];
                for a in 0..3 {
                    rg[a] += c[a] * wt;
                }
                rg[3] += wt;
            }
        }
    }
    let face_c = ring[0][3];
    let ring: Vec<Vec<[f32; 3]>> = ring
        .iter()
        .map(|side| {
            side.iter()
                .enumerate()
                .map(|(bin, g)| {
                    let g = if bin == 3 { face_c } else { *g };
                    [g[0] / g[3].max(1e-6), g[1] / g[3].max(1e-6), g[2] / g[3].max(1e-6)]
                })
                .collect()
        })
        .collect();
    // The profile at depth d (texels) for side weights `sw`: the outline sharp at the rim's
    // edge, softer within.
    let lerp = |a: [f32; 3], b: [f32; 3], t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
    let profile = |d: f32, sw: [f32; 4], sharp: f32| -> [f32; 3] {
        let edge = ((d - r.tile) / sharp + 0.5).clamp(0.0, 1.0);
        let mut out = [0.0f32; 3];
        for (side, wt) in sw.iter().enumerate() {
            let rg = &ring[side];
            let inner = if d < 1.5 {
                rg[1]
            } else if d < 2.5 {
                lerp(rg[1], rg[2], d - 1.5)
            } else {
                lerp(rg[2], rg[3], (d - 2.5).min(1.0))
            };
            // The joint darkest in its middle (where the two slabs' outlines meet).
            let mid = 0.72 + 0.28 * (d / r.tile.max(0.1)).clamp(0.0, 1.0);
            let c = lerp(rg[0].map(|x| x * mid), inner, edge * edge * (3.0 - 2.0 * edge));
            for a in 0..3 {
                out[a] += c[a] * wt;
            }
        }
        out
    };
    // What is left of the art (its light and colour), per texel, then smoothed.
    let mut rest = vec![[0.0f32; 3]; w * h];
    for y in 0..h {
        for x in 0..w {
            let d = inward(x as f32 + 0.5, y as f32 + 0.5, w as f32, 1.0, rc);
            let (c, p) = (px(x, y), profile(d, side_weights(x as f32 + 0.5, y as f32 + 0.5, w as f32, 1.0), 1.0));
            rest[y * w + x] = [c[0] - p[0], c[1] - p[1], c[2] - p[2]];
        }
    }
    // The face's blotches: how far apart its light and dark are (the colour they run along)
    // and how strong (the art's texel-to-texel variation, which the smoothing takes away).
    let mut face: Vec<([f32; 3], f32)> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if inward(x as f32 + 0.5, y as f32 + 0.5, w as f32, 1.0, rc) >= 3.0 {
                let c = px(x, y);
                face.push((c, 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]));
            }
        }
    }
    face.sort_by(|a, b| a.1.total_cmp(&b.1));
    let third = (face.len() / 3).max(1);
    let mean = |v: &[([f32; 3], f32)]| {
        let s = v.iter().fold([0.0f32; 3], |a, (c, _)| [a[0] + c[0], a[1] + c[1], a[2] + c[2]]);
        s.map(|x| x / v.len().max(1) as f32)
    };
    let (dark, light) = (mean(&face[..third]), mean(&face[face.len() - third..]));
    let axis = [light[0] - dark[0], light[1] - dark[1], light[2] - dark[2]];
    let k = n as f32 / w.max(h) as f32;
    // Smooth upscale of what is left: bilinear, then blurred over about a texel (no blocks).
    let mut up = [vec![0.0f32; n * n], vec![0.0f32; n * n], vec![0.0f32; n * n]];
    for y in 0..n {
        for x in 0..n {
            let fx = (x as f32 + 0.5) / k - 0.5;
            let fy = (y as f32 + 0.5) / k - 0.5;
            let (x0, y0) = (fx.floor(), fy.floor());
            let (tx, ty) = (fx - x0, fy - y0);
            let at = |i: f32, j: f32| rest[(j as isize).rem_euclid(h as isize) as usize * w + (i as isize).rem_euclid(w as isize) as usize];
            for (a, ch) in up.iter_mut().enumerate() {
                let top = at(x0, y0)[a] * (1.0 - tx) + at(x0 + 1.0, y0)[a] * tx;
                let bot = at(x0, y0 + 1.0)[a] * (1.0 - tx) + at(x0 + 1.0, y0 + 1.0)[a] * tx;
                ch[y * n + x] = top * (1.0 - ty) + bot * ty;
            }
        }
    }
    let up: Vec<Grid> = up.into_iter().map(|v| Grid { n, v }.blur(k * 0.9)).collect();
    let mut out = vec![0u8; n * n * 4];
    let sharp = 0.5; // the outline's edge (texels): soft, as the art's own
    for y in 0..n {
        for x in 0..n {
            let i = y * n + x;
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let d = inward(fx, fy, n as f32, k, rc);
            let p = profile(d, side_weights(fx, fy, n as f32, k), sharp);
            // Blotches on the face and the bevel, fainter in the joint.
            let m = mottle(fx / n as f32, fy / n as f32, 0x5eed) * r.paint;
            let on_face = ((d - r.tile) / 1.5).clamp(0.0, 1.0) * 0.8 + 0.2;
            for a in 0..3 {
                out[i * 4 + a] = ((p[a] + up[a].v[i] + axis[a] * m * on_face).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
            out[i * 4 + 3] = 255;
        }
    }
    out
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
            let rc = r.tile + r.bevel.clamp(0.5, 1.5); // corners rounded about as the art's, however wide the bevel
            let inward = if ex < rc && ey < rc { rc - ((rc - ex).powi(2) + (rc - ey).powi(2)).sqrt() } else { ex.min(ey) };
            let s = ((inward - r.tile) / r.bevel.max(0.25)).clamp(0.0, 1.0);
            plat = plat.min(s * s * (3.0 - 2.0 * s));
        }
        let relief = fine[i] / fmax * 0.08 * r.detail;
        hv[i] = if r.tile > 0.0 && r.paint > 0.0 {
            // The slab, its face's relief the redrawn colour's mottling (the art's texels would
            // show as blocks), none in the joint.
            let m = mottle((i % n) as f32 / n as f32 + 0.5 / n as f32, (i / n) as f32 / n as f32 + 0.5 / n as f32, 0x5eed);
            plat * (r.plateau + (1.0 - r.plateau) * (0.5 + m * r.detail).clamp(0.0, 1.0))
        } else if r.tile > 0.0 {
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
    let albedo = (r.tile > 0.0 && r.paint > 0.0).then(|| remaster(rgba, w, h, n, r));
    let mut out = Maps { size, height: vec![0; n * n * 4], normal: vec![0; n * n * 4], orm: vec![0; n * n * 4], albedo };
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

    /// A slab redrawn at 8x: its rim dark all round and straight, its face the art's face
    /// colour (mottled about it), the light painted on one side kept on that side.
    #[test]
    fn slab_remaster() {
        let w = 16usize;
        let mut img = vec![0u8; w * w * 4];
        for y in 0..w {
            for x in 0..w {
                let rim = x == 0 || y == 0 || x == w - 1 || y == w - 1;
                // A lighter bevel along the top only (the art's light).
                let c: [u8; 3] = if rim {
                    [90, 40, 10]
                } else if y == 1 {
                    [255, 230, 160]
                } else {
                    [240, 180, 80]
                };
                img[(y * w + x) * 4..][..4].copy_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        let n = 128usize;
        let m = generate(&img, w as u32, w as u32, n as u32, &Recipe { tile: 1.0, paint: 0.5, plateau: 0.85, bevel: 1.0, seam: 0.0, ..Recipe::default() });
        let a = m.albedo.expect("a slab is redrawn");
        let px = |x: usize, y: usize| [a[(y * n + x) * 4] as i32, a[(y * n + x) * 4 + 1] as i32, a[(y * n + x) * 4 + 2] as i32];
        for i in 16..n - 16 {
            // The joint at every side: dark, and as dark as the other sides.
            for (x, y) in [(i, 1), (1, i), (i, n - 2), (n - 2, i)] {
                assert!(px(x, y)[1] < 70, "joint at ({x}, {y}): {:?}", px(x, y));
            }
        }
        // The face about the art's face colour.
        let f = px(n / 2, n / 2);
        assert!((f[0] - 240).abs() < 30 && (f[1] - 180).abs() < 40 && (f[2] - 80).abs() < 40, "face {f:?}");
        // The top bevel lighter than the bottom one (the light stays where the art put it).
        let (top, bottom) = (px(n / 2, 12)[1], px(n / 2, n - 13)[1]);
        assert!(top > bottom + 15, "top {top} bottom {bottom}");
    }

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
