//! RGBA8 image operations used by the texture converter: resampling (Lanczos-3 down,
//! bilinear up), the perceptual error metric, palette generation (median cut + k-means)
//! and dithering. Dependency-free on purpose.

use std::collections::HashMap;

/// A straight-alpha RGBA8 image, row-major, top-down.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub w: usize,
    pub h: usize,
    pub px: Vec<[u8; 4]>,
}

impl Rgba {
    pub fn new(w: usize, h: usize, px: Vec<[u8; 4]>) -> Rgba {
        assert_eq!(px.len(), w * h, "image size mismatch");
        Rgba { w, h, px }
    }

    /// From packed RGBA8 bytes.
    pub fn from_bytes(w: usize, h: usize, b: &[u8]) -> Rgba {
        Rgba::new(w, h, b.as_chunks::<4>().0.to_vec())
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.px.iter().flatten().copied().collect()
    }

    #[inline]
    pub fn at(&self, x: usize, y: usize) -> [u8; 4] {
        self.px[y * self.w + x]
    }

    /// True when every texel is fully opaque.
    pub fn opaque(&self) -> bool {
        self.px.iter().all(|p| p[3] == 255)
    }

    /// True when alpha is only ever 0 or 255.
    pub fn alpha_1bit(&self) -> bool {
        self.px.iter().all(|p| p[3] == 0 || p[3] == 255)
    }
}

/// Luma weights for R, G, B in the error metric (Rec. 601, scaled so they sum to 3:
/// a uniform grey error weighs like three independent channel errors).
pub const LUMA_W: [f64; 3] = [0.299 * 3.0, 0.587 * 3.0, 0.114 * 3.0];
/// Weight of the colour of a fully transparent texel relative to an opaque one. It is not
/// zero because bilinear filtering blends transparent neighbours' colour into edges.
pub const TRANSPARENT_RGB_W: f64 = 0.25;

/// Squared error between two texels under the module's perceptual metric (see
/// [`weighted_mse`]); the result is the sum over the 4 weighted channels.
#[inline]
pub fn texel_err(a: [f32; 4], b: [f32; 4]) -> f64 {
    let amax = f64::from(a[3].max(b[3])) / 255.0;
    let cw = TRANSPARENT_RGB_W + (1.0 - TRANSPARENT_RGB_W) * amax;
    let mut e = 0.0;
    for c in 0..3 {
        let d = f64::from(a[c] - b[c]);
        e += LUMA_W[c] * d * d * cw;
    }
    let d = f64::from(a[3] - b[3]);
    e + d * d
}

#[inline]
fn f4(p: [u8; 4]) -> [f32; 4] {
    [f32::from(p[0]), f32::from(p[1]), f32::from(p[2]), f32::from(p[3])]
}

/// Mean squared error per channel between two images of equal size:
/// `sum(texel_err) / (N * 4)` — RGB errors weighted by luma ([`LUMA_W`]) and by the
/// texel's coverage (max alpha of both, floored at [`TRANSPARENT_RGB_W`]), alpha error
/// unweighted.
pub fn weighted_mse(a: &Rgba, b: &Rgba) -> f64 {
    assert_eq!((a.w, a.h), (b.w, b.h));
    let s: f64 = a.px.iter().zip(&b.px).map(|(&x, &y)| texel_err(f4(x), f4(y))).sum();
    s / (a.px.len().max(1) as f64 * 4.0)
}

/// PSNR in dB from an MSE (infinite for identical images).
pub fn psnr_from_mse(mse: f64) -> f64 {
    if mse <= 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0f64 * 255.0 / mse).log10()
    }
}

/// Perceptual PSNR of `b` against reference `a` ([`weighted_mse`]).
pub fn psnr(a: &Rgba, b: &Rgba) -> f64 {
    psnr_from_mse(weighted_mse(a, b))
}

/// PSNR after a [1 2 1]² low-pass on both images: approximates the eye's integration at
/// viewing distance (so dithering is rewarded rather than punished).
pub fn psnr_lowpass(a: &Rgba, b: &Rgba) -> f64 {
    psnr(&blur121(a), &blur121(b))
}

fn blur121(img: &Rgba) -> Rgba {
    let (w, h) = (img.w, img.h);
    let get = |x: isize, y: isize| -> [f32; 4] { f4(img.at(x.clamp(0, w as isize - 1) as usize, y.clamp(0, h as isize - 1) as usize)) };
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h as isize {
        for x in 0..w as isize {
            let mut acc = [0f32; 4];
            for (dy, wy) in [(-1, 1.0), (0, 2.0), (1, 1.0)] {
                for (dx, wx) in [(-1, 1.0), (0, 2.0), (1, 1.0)] {
                    let p = get(x + dx, y + dy);
                    for c in 0..4 {
                        acc[c] += p[c] * wx * wy;
                    }
                }
            }
            out.push(acc.map(|v| (v / 16.0).round().clamp(0.0, 255.0) as u8));
        }
    }
    Rgba::new(w, h, out)
}

fn lanczos3(x: f64) -> f64 {
    let x = x.abs();
    if x < 1e-9 {
        1.0
    } else if x >= 3.0 {
        0.0
    } else {
        let px = std::f64::consts::PI * x;
        3.0 * px.sin() * (px / 3.0).sin() / (px * px)
    }
}

/// Per-output-sample weights for a 1D Lanczos-3 resample `n_in -> n_out` (downscale).
fn weights_1d(n_in: usize, n_out: usize, wrap: bool) -> Vec<Vec<(usize, f64)>> {
    let scale = n_in as f64 / n_out as f64;
    let support = 3.0 * scale.max(1.0);
    (0..n_out)
        .map(|o| {
            let centre = (o as f64 + 0.5) * scale - 0.5;
            let lo = (centre - support).floor() as isize;
            let hi = (centre + support).ceil() as isize;
            let mut ws: Vec<(usize, f64)> = Vec::new();
            for i in lo..=hi {
                let wgt = lanczos3((i as f64 - centre) / scale.max(1.0));
                if wgt == 0.0 {
                    continue;
                }
                let idx = if wrap { i.rem_euclid(n_in as isize) as usize } else { i.clamp(0, n_in as isize - 1) as usize };
                match ws.iter_mut().find(|(j, _)| *j == idx) {
                    Some(e) => e.1 += wgt,
                    None => ws.push((idx, wgt)),
                }
            }
            let sum: f64 = ws.iter().map(|w| w.1).sum();
            ws.iter_mut().for_each(|w| w.1 /= sum);
            ws
        })
        .collect()
}

/// Float premultiplied image used inside resampling.
type Plane = Vec<[f64; 4]>;

fn premul(img: &Rgba) -> Plane {
    img.px
        .iter()
        .map(|p| {
            let a = f64::from(p[3]) / 255.0;
            [f64::from(p[0]) * a, f64::from(p[1]) * a, f64::from(p[2]) * a, f64::from(p[3])]
        })
        .collect()
}

/// Lanczos-3 resample (meant for downscaling) in premultiplied alpha; `wrap` selects
/// toroidal edges (tiling 3D textures) instead of clamped edges. Returns float straight
/// alpha RGBA in 0..255 (unrounded, so callers can dither).
pub fn resize_f(img: &Rgba, nw: usize, nh: usize, wrap: bool) -> Vec<[f32; 4]> {
    let src = premul(img);
    let wx = weights_1d(img.w, nw, wrap);
    let wy = weights_1d(img.h, nh, wrap);
    let mut tmp: Plane = vec![[0.0; 4]; nw * img.h];
    for y in 0..img.h {
        for (x, ws) in wx.iter().enumerate() {
            let mut acc = [0.0; 4];
            for &(i, wgt) in ws {
                let p = src[y * img.w + i];
                for c in 0..4 {
                    acc[c] += p[c] * wgt;
                }
            }
            tmp[y * nw + x] = acc;
        }
    }
    let mut out = Vec::with_capacity(nw * nh);
    for ws in &wy {
        for x in 0..nw {
            let mut acc = [0.0; 4];
            for &(j, wgt) in ws {
                let p = tmp[j * nw + x];
                for c in 0..4 {
                    acc[c] += p[c] * wgt;
                }
            }
            let a = acc[3].clamp(0.0, 255.0);
            let k = if a > 1e-6 { 255.0 / a } else { 0.0 };
            out.push([(acc[0] * k).clamp(0.0, 255.0) as f32, (acc[1] * k).clamp(0.0, 255.0) as f32, (acc[2] * k).clamp(0.0, 255.0) as f32, a as f32]);
        }
    }
    out
}

/// Rounds a float image to RGBA8.
pub fn round_img(w: usize, h: usize, f: &[[f32; 4]]) -> Rgba {
    Rgba::new(w, h, f.iter().map(|p| p.map(|v| v.round().clamp(0.0, 255.0) as u8)).collect())
}

/// Lanczos-3 resample rounded to RGBA8.
pub fn resize(img: &Rgba, nw: usize, nh: usize, wrap: bool) -> Rgba {
    round_img(nw, nh, &resize_f(img, nw, nh, wrap))
}

/// Bilinear upsample (texel-centre aligned, like the RDP's bilinear filter at 1:N
/// magnification); used to compare a downscaled candidate against the original.
pub fn upsample_bilinear(img: &Rgba, nw: usize, nh: usize, wrap: bool) -> Rgba {
    let (w, h) = (img.w as isize, img.h as isize);
    let idx = |v: isize, n: isize| if wrap { v.rem_euclid(n) } else { v.clamp(0, n - 1) } as usize;
    let mut out = Vec::with_capacity(nw * nh);
    for y in 0..nh {
        let fy = (y as f64 + 0.5) * img.h as f64 / nh as f64 - 0.5;
        let y0 = fy.floor();
        let ty = fy - y0;
        let (ya, yb) = (idx(y0 as isize, h), idx(y0 as isize + 1, h));
        for x in 0..nw {
            let fx = (x as f64 + 0.5) * img.w as f64 / nw as f64 - 0.5;
            let x0 = fx.floor();
            let tx = fx - x0;
            let (xa, xb) = (idx(x0 as isize, w), idx(x0 as isize + 1, w));
            let p = [img.at(xa, ya), img.at(xb, ya), img.at(xa, yb), img.at(xb, yb)];
            let wts = [(1.0 - tx) * (1.0 - ty), tx * (1.0 - ty), (1.0 - tx) * ty, tx * ty];
            let mut acc = [0.0f64; 4];
            for k in 0..4 {
                for c in 0..4 {
                    acc[c] += f64::from(p[k][c]) * wts[k];
                }
            }
            out.push(acc.map(|v| v.round().clamp(0.0, 255.0) as u8));
        }
    }
    Rgba::new(nw, nh, out)
}

// ---------------------------------------------------------------------------------------
// Palettes

/// Weighted distance used by the quantiser (same weights as [`texel_err`]).
#[inline]
fn qdist(a: [f32; 4], b: [f32; 4]) -> f64 {
    texel_err(a, b)
}

/// Builds a palette of at most `k` colours for the weighted colour list `hist`
/// (colour, count): median cut (split the box with the largest weighted error along
/// its widest weighted axis at the weighted median) followed by `iters` k-means passes.
pub fn make_palette(hist: &[([f32; 4], f64)], k: usize, iters: usize) -> Vec<[f32; 4]> {
    if hist.is_empty() || k == 0 {
        return Vec::new();
    }
    let axis_w = [LUMA_W[0].sqrt(), LUMA_W[1].sqrt(), LUMA_W[2].sqrt(), 1.0];
    let centroid = |items: &[([f32; 4], f64)]| -> [f32; 4] {
        let mut acc = [0f64; 4];
        let mut n = 0.0;
        for (c, w) in items {
            for i in 0..4 {
                acc[i] += f64::from(c[i]) * w;
            }
            n += w;
        }
        acc.map(|v| (v / n.max(1e-12)) as f32)
    };
    let box_err = |items: &[([f32; 4], f64)]| -> f64 {
        let c = centroid(items);
        items.iter().map(|(p, w)| qdist(*p, c) * w).sum()
    };
    type Box = (Vec<([f32; 4], f64)>, f64);
    let mut boxes: Vec<Box> = vec![(hist.to_vec(), 0.0)];
    boxes[0].1 = box_err(&boxes[0].0);
    while boxes.len() < k {
        let Some((bi, _)) = boxes.iter().enumerate().filter(|(_, b)| b.0.len() > 1).max_by(|a, b| a.1 .1.total_cmp(&b.1 .1)) else { break };
        let (mut items, _) = boxes.swap_remove(bi);
        let mut best_axis = 0;
        let mut best_range = -1.0;
        for ax in 0..4 {
            let (mn, mx) = items.iter().fold((f32::MAX, f32::MIN), |(a, b), (c, _)| (a.min(c[ax]), b.max(c[ax])));
            let r = f64::from(mx - mn) * axis_w[ax];
            if r > best_range {
                best_range = r;
                best_axis = ax;
            }
        }
        items.sort_by(|a, b| a.0[best_axis].total_cmp(&b.0[best_axis]));
        let total: f64 = items.iter().map(|i| i.1).sum();
        let mut acc = 0.0;
        let mut cut = 1;
        for (i, it) in items.iter().enumerate() {
            acc += it.1;
            if acc >= total / 2.0 {
                cut = (i + 1).clamp(1, items.len() - 1);
                break;
            }
        }
        let right = items.split_off(cut);
        let el = box_err(&items);
        let er = box_err(&right);
        boxes.push((items, el));
        boxes.push((right, er));
    }
    let mut pal: Vec<[f32; 4]> = boxes.iter().map(|b| centroid(&b.0)).collect();
    for _ in 0..iters {
        let mut acc = vec![[0f64; 5]; pal.len()];
        for (c, w) in hist {
            let i = nearest(&pal, *c);
            for j in 0..4 {
                acc[i][j] += f64::from(c[j]) * w;
            }
            acc[i][4] += w;
        }
        for (p, a) in pal.iter_mut().zip(&acc) {
            if a[4] > 0.0 {
                *p = [(a[0] / a[4]) as f32, (a[1] / a[4]) as f32, (a[2] / a[4]) as f32, (a[3] / a[4]) as f32];
            }
        }
    }
    pal
}

/// Index of the nearest palette entry under the quantiser metric.
pub fn nearest(pal: &[[f32; 4]], c: [f32; 4]) -> usize {
    let mut best = 0;
    let mut bd = f64::MAX;
    for (i, p) in pal.iter().enumerate() {
        let d = qdist(*p, c);
        if d < bd {
            bd = d;
            best = i;
        }
    }
    best
}

/// Weighted histogram of an image's colours, binned to `bits` bits per colour channel
/// (alpha kept to 4 bits) to bound k-means cost; each bin holds the mean colour.
pub fn histogram(px: &[[f32; 4]], bits: u32) -> Vec<([f32; 4], f64)> {
    let sh = 8 - bits;
    let mut map: HashMap<u32, ([f64; 4], f64)> = HashMap::new();
    for p in px {
        let q = |v: f32, s: u32| (v.round().clamp(0.0, 255.0) as u32) >> s;
        let key = (q(p[0], sh) << 24) | (q(p[1], sh) << 16) | (q(p[2], sh) << 8) | q(p[3], 4);
        let e = map.entry(key).or_insert(([0.0; 4], 0.0));
        for (acc, &v) in e.0.iter_mut().zip(p) {
            *acc += f64::from(v);
        }
        e.1 += 1.0;
    }
    let mut v: Vec<_> = map.into_iter().collect();
    v.sort_by_key(|e| e.0);
    v.into_iter().map(|(_, (s, n))| (s.map(|x| (x / n) as f32), n)).collect()
}

/// 4x4 Bayer matrix (values 0..15).
pub const BAYER4: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// Dithering applied when quantising a float image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dither {
    None,
    /// 4x4 ordered (Bayer).
    Ordered,
    /// Floyd–Steinberg error diffusion, serpentine scan.
    FloydSteinberg,
}

/// Quantises a float image texel by texel with `quant` (maps a wanted colour to the
/// representable colour it becomes, returning that colour and an opaque code), with
/// optional dithering. `step` is the quantisation step used for ordered dithering.
pub fn quantise<Q: FnMut([f32; 4]) -> ([f32; 4], u32)>(w: usize, h: usize, src: &[[f32; 4]], dither: Dither, step: f32, mut quant: Q) -> Vec<u32> {
    let mut codes = vec![0u32; w * h];
    match dither {
        Dither::None | Dither::Ordered => {
            for y in 0..h {
                for x in 0..w {
                    let mut c = src[y * w + x];
                    if dither == Dither::Ordered {
                        let t = (f32::from(BAYER4[y & 3][x & 3]) + 0.5) / 16.0 - 0.5;
                        for v in c.iter_mut().take(3) {
                            *v = (*v + t * step).clamp(0.0, 255.0);
                        }
                    }
                    codes[y * w + x] = quant(c).1;
                }
            }
        }
        Dither::FloydSteinberg => {
            let mut buf: Vec<[f32; 4]> = src.to_vec();
            for y in 0..h {
                let ltr = y % 2 == 0;
                for i in 0..w {
                    let x = if ltr { i } else { w - 1 - i };
                    let want = buf[y * w + x].map(|v| v.clamp(0.0, 255.0));
                    let (got, code) = quant(want);
                    codes[y * w + x] = code;
                    // Diffuse colour error only (alpha is kept exact by the quantisers).
                    let err = [want[0] - got[0], want[1] - got[1], want[2] - got[2]];
                    let mut push = |dx: isize, dy: usize, f: f32| {
                        let nx = x as isize + if ltr { dx } else { -dx };
                        let ny = y + dy;
                        if nx >= 0 && (nx as usize) < w && ny < h {
                            let p = &mut buf[ny * w + nx as usize];
                            for c in 0..3 {
                                p[c] += err[c] * f;
                            }
                        }
                    };
                    push(1, 0, 7.0 / 16.0);
                    push(-1, 1, 3.0 / 16.0);
                    push(0, 1, 5.0 / 16.0);
                    push(1, 1, 1.0 / 16.0);
                }
            }
        }
    }
    codes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grad(w: usize, h: usize) -> Rgba {
        Rgba::new(w, h, (0..w * h).map(|i| [(i % w * 255 / (w - 1)) as u8, (i / w * 255 / (h - 1)) as u8, 128, 255]).collect())
    }

    #[test]
    fn psnr_identity_and_order() {
        let a = grad(16, 16);
        assert!(psnr(&a, &a).is_infinite());
        let mut b = a.clone();
        b.px[3][0] ^= 4;
        let mut c = a.clone();
        c.px[3][0] ^= 32;
        assert!(psnr(&a, &b) > psnr(&a, &c));
    }

    #[test]
    fn resize_constant_stays_constant() {
        let a = Rgba::new(8, 8, vec![[10, 200, 30, 255]; 64]);
        let r = resize(&a, 4, 2, true);
        assert!(r.px.iter().all(|&p| p == [10, 200, 30, 255]));
        let u = upsample_bilinear(&r, 8, 8, false);
        assert!(u.px.iter().all(|&p| p == [10, 200, 30, 255]));
    }

    #[test]
    fn palette_exact_when_few_colours() {
        let cols = [[0f32, 0.0, 0.0, 255.0], [255.0, 0.0, 0.0, 255.0], [0.0, 0.0, 255.0, 0.0]];
        let hist: Vec<_> = cols.iter().map(|&c| (c, 5.0)).collect();
        let p = make_palette(&hist, 4, 3);
        for c in cols {
            assert_eq!(p[nearest(&p, c)], c);
        }
    }
}
