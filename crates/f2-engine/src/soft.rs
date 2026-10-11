//! CPU back end: rasterises the same TL triangle lists the wgpu back end draws
//! (`Renderer::build`), with the same render states (blend, depth, alpha test,
//! fog, wrap/clamp, nearest/bilinear), into an RGBA8 framebuffer.
//!
//! For platforms without a wgpu backend (homebrew consoles). It matches the
//! wgpu shader's maths: D3D7 TL pixel centres at integer coordinates, depth =
//! `clamp(sz, 0, 1)` interpolated in screen space, colour/uv/fog affine unless
//! `perspective_correct` (then weighted by `rhw`), `a*255 <= aref` discards.
//! Not used by the desktop build (MSAA / render scale / anisotropy are ignored here).
//!
//! Rows are split into horizontal bands rasterised in parallel; each band
//! replays every command in order, so per-pixel draw order is preserved.

use crate::renderer::{Blend, DepthMode, DrawCmd, Renderer};
use crate::scene::TlVertex;
use crate::textures::Image;

/// A CPU colour + depth target. Pixels are RGBA8 packed little-endian (`r` in the
/// low byte), i.e. `pixels` reinterpreted as bytes is `R, G, B, A, ...`.
#[derive(Default)]
pub struct SoftTarget {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
    depth: Vec<f32>,
}

impl SoftTarget {
    pub fn new(width: u32, height: u32) -> SoftTarget {
        let mut t = SoftTarget::default();
        t.resize(width, height);
        t
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        let n = (width.max(1) * height.max(1)) as usize;
        self.width = width.max(1);
        self.height = height.max(1);
        self.pixels.resize(n, 0);
        self.depth.resize(n, 1.0);
    }
}

/// Per-draw constant state.
#[derive(Clone, Copy)]
struct State {
    blend: Blend,
    test: bool,
    write: bool,
    wrap: bool,
    linear: bool,
    persp: bool,
    fog: Option<[f32; 3]>,
    /// Scissor (x0, y0, x1, y1) inclusive-exclusive, in pixels.
    clip: [i32; 4],
}

#[inline]
fn pack(c: [f32; 4]) -> u32 {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    b(c[0]) | b(c[1]) << 8 | b(c[2]) << 16 | b(c[3]) << 24
}

#[inline]
fn unpack(p: u32) -> [f32; 4] {
    const K: f32 = 1.0 / 255.0;
    [(p & 255) as f32 * K, (p >> 8 & 255) as f32 * K, (p >> 16 & 255) as f32 * K, (p >> 24) as f32 * K]
}

#[inline]
fn texel(img: &Image, x: i32, y: i32, wrap: bool) -> [f32; 4] {
    let (w, h) = (img.width as i32, img.height as i32);
    let (x, y) = if wrap { (x.rem_euclid(w), y.rem_euclid(h)) } else { (x.clamp(0, w - 1), y.clamp(0, h - 1)) };
    let i = ((y * w + x) * 4) as usize;
    let p = &img.rgba[i..i + 4];
    const K: f32 = 1.0 / 255.0;
    [p[0] as f32 * K, p[1] as f32 * K, p[2] as f32 * K, p[3] as f32 * K]
}

#[inline]
fn sample(img: &Image, u: f32, v: f32, st: &State) -> [f32; 4] {
    let (w, h) = (img.width as f32, img.height as f32);
    if !st.linear {
        return texel(img, (u * w).floor() as i32, (v * h).floor() as i32, st.wrap);
    }
    let (x, y) = (u * w - 0.5, v * h - 0.5);
    let (fx, fy) = (x.floor(), y.floor());
    let (ax, ay) = (x - fx, y - fy);
    let (ix, iy) = (fx as i32, fy as i32);
    let t00 = texel(img, ix, iy, st.wrap);
    let t10 = texel(img, ix + 1, iy, st.wrap);
    let t01 = texel(img, ix, iy + 1, st.wrap);
    let t11 = texel(img, ix + 1, iy + 1, st.wrap);
    let mut o = [0.0; 4];
    for c in 0..4 {
        let top = t00[c] + (t10[c] - t00[c]) * ax;
        let bot = t01[c] + (t11[c] - t01[c]) * ax;
        o[c] = top + (bot - top) * ay;
    }
    o
}

/// Attributes interpolated across a triangle: z, r, g, b, a, u, v, fog, q (rhw weight).
const NA: usize = 9;

fn attrs(v: &TlVertex, persp: bool) -> [f32; NA] {
    let q = if persp && v.rhw > 0.0 { v.rhw } else { 1.0 };
    [v.sz.clamp(0.0, 1.0), v.color[0] * q, v.color[1] * q, v.color[2] * q, v.color[3] * q, v.uv[0] * q, v.uv[1] * q, v.fog * q, q]
}

/// Rasterises one triangle into the band starting at row `y0` (`rows` rows).
#[allow(clippy::too_many_arguments)]
fn tri(px: &mut [u32], dp: &mut [f32], width: i32, y0: i32, rows: i32, v: &[TlVertex], img: &Image, st: &State, aref: f32) {
    let (x0, y0v, x1, y1, x2, y2) = (v[0].sx, v[0].sy, v[1].sx, v[1].sy, v[2].sx, v[2].sy);
    let area = (x1 - x0) * (y2 - y0v) - (x2 - x0) * (y1 - y0v);
    if area == 0.0 || !area.is_finite() {
        return;
    }
    // Bounding box (sample points at integer coordinates), clipped to scissor and band.
    let minx = x0.min(x1).min(x2).ceil().max(st.clip[0] as f32) as i32;
    let maxx = (x0.max(x1).max(x2).floor() as i32).min(st.clip[2] - 1).min(width - 1);
    let miny = y0v.min(y1).min(y2).ceil().max(st.clip[1].max(y0) as f32) as i32;
    let maxy = (y0v.max(y1).max(y2).floor() as i32).min(st.clip[3] - 1).min(y0 + rows - 1);
    if minx > maxx || miny > maxy {
        return;
    }
    // Attribute planes: a(x, y) = a0 + ax * (x - x0) + ay * (y - y0).
    let (a0, a1, a2) = (attrs(&v[0], st.persp), attrs(&v[1], st.persp), attrs(&v[2], st.persp));
    let inv = 1.0 / area;
    let mut pdx = [0.0f32; NA];
    let mut pdy = [0.0f32; NA];
    for i in 0..NA {
        let (d1, d2) = (a1[i] - a0[i], a2[i] - a0[i]);
        pdx[i] = (d1 * (y2 - y0v) - d2 * (y1 - y0v)) * inv;
        pdy[i] = (d2 * (x1 - x0) - d1 * (x2 - x0)) * inv;
    }
    // Edge functions, oriented so the inside is >= 0; top-left fill rule via a bias.
    let s = if area > 0.0 { 1.0 } else { -1.0 };
    let edge = |ax: f32, ay: f32, bx: f32, by: f32| {
        let (a, b) = ((ay - by) * s, (bx - ax) * s);
        let c = -(a * ax + b * ay);
        // Top edge (horizontal, pointing left in y-down after orientation) or left edge.
        let top_left = (a == 0.0 && b < 0.0) || a > 0.0;
        (a, b, c, top_left)
    };
    let es = [edge(x1, y1, x2, y2), edge(x2, y2, x0, y0v), edge(x0, y0v, x1, y1)];
    let w = width as usize;
    for y in miny..=maxy {
        let fy = y as f32;
        // Exact span for this row: intersect the half-planes along x.
        let (mut lo, mut hi) = (minx as f32, maxx as f32);
        let mut empty = false;
        for &(a, b, c, tl) in &es {
            let r = b * fy + c; // e(x) = a*x + r
            if a == 0.0 {
                if r < 0.0 || (r == 0.0 && !tl) {
                    empty = true;
                }
            } else {
                let x = -r / a;
                if a > 0.0 {
                    // e >= 0 for x >= x_cross (strict when not top-left).
                    let mut l = x.ceil();
                    if !tl && l == x {
                        l += 1.0;
                    }
                    lo = lo.max(l);
                } else {
                    let mut h = x.floor();
                    if !tl && h == x {
                        h -= 1.0;
                    }
                    hi = hi.min(h);
                }
            }
        }
        if empty || lo > hi {
            continue;
        }
        let (xs, xe) = (lo as i32, hi as i32);
        let (dx, dy) = (xs as f32 - x0, fy - y0v);
        let mut a = [0.0f32; NA];
        for i in 0..NA {
            a[i] = a0[i] + pdx[i] * dx + pdy[i] * dy;
        }
        let row = (y - y0) as usize * w;
        for x in xs..=xe {
            let i = row + x as usize;
            let z = a[0];
            let pass = !st.test || z <= dp[i];
            if pass {
                let q = if st.persp { 1.0 / a[8] } else { 1.0 };
                let t = sample(img, a[5] * q, a[6] * q, st);
                let mut c = [t[0] * a[1] * q, t[1] * a[2] * q, t[2] * a[3] * q, t[3] * a[4] * q];
                if (c[3] * 255.0 + 0.5).floor() > aref {
                    if let Some(fc) = st.fog {
                        let f = a[7] * q;
                        for k in 0..3 {
                            c[k] = fc[k] + (c[k] - fc[k]) * f;
                        }
                    }
                    let d = unpack(px[i]);
                    let o = match st.blend {
                        Blend::Semi => {
                            let sa = c[3].clamp(0.0, 1.0);
                            [c[0] * sa + d[0] * (1.0 - sa), c[1] * sa + d[1] * (1.0 - sa), c[2] * sa + d[2] * (1.0 - sa), sa + d[3] * (1.0 - sa)]
                        }
                        Blend::Additive => {
                            let sa = c[3].clamp(0.0, 1.0);
                            [d[0] + c[0] * sa, d[1] + c[1] * sa, d[2] + c[2] * sa, d[3]]
                        }
                        Blend::Subtractive => {
                            [d[0] * (1.0 - c[0].clamp(0.0, 1.0)), d[1] * (1.0 - c[1].clamp(0.0, 1.0)), d[2] * (1.0 - c[2].clamp(0.0, 1.0)), d[3]]
                        }
                    };
                    px[i] = pack(o);
                    if st.write {
                        dp[i] = z;
                    }
                }
            }
            for k in 0..NA {
                a[k] += pdx[k];
            }
        }
    }
}

impl Renderer {
    /// Renders the queued frame into `target` on the CPU (resized to the internal view
    /// size). `threads` = number of row bands rasterised in parallel (>= 1).
    pub fn render_soft(&mut self, target: &mut SoftTarget, clear: [f64; 4], threads: usize) {
        let (w, h) = (self.view.width as u32, self.view.height as u32);
        target.resize(w, h);
        self.textures.dirty.clear();
        let (verts, cmds) = self.build(clear);
        let clear = if self.view.viewport.is_some() { [0.0, 0.0, 0.0, 1.0] } else { clear };
        let cc = pack(clear.map(|c| c as f32));
        target.pixels.fill(cc);
        target.depth.fill(1.0);
        let full = [0, 0, w as i32, h as i32];
        let scissor = self.view.viewport.map(|[x, y, vw, vh]| [x as i32, y as i32, (x + vw) as i32, (y + vh) as i32]);
        let fog = self.options.fog;
        let linear = self.options.linear_filter;
        let persp = self.options.perspective_correct;
        let images = &self.textures.images;
        let states: Vec<State> = cmds
            .iter()
            .map(|c: &DrawCmd| State {
                blend: c.blend,
                test: c.depth != DepthMode::Off,
                write: c.depth == DepthMode::TestWrite,
                wrap: c.wrap,
                linear,
                persp,
                fog: fog.enabled.then_some(fog.color),
                clip: if c.scene { scissor.unwrap_or(full) } else { full },
            })
            .collect();
        let threads = threads.clamp(1, 16).min(h as usize);
        let band = (h as usize).div_ceil(threads);
        let width = w as i32;
        let run = |y0: usize, px: &mut [u32], dp: &mut [f32]| {
            let rows = (px.len() / w as usize) as i32;
            for (c, st) in cmds.iter().zip(&states) {
                let img = &images[c.tex.0 as usize];
                if img.rgba.len() as u32 != img.width * img.height * 4 || img.width == 0 {
                    continue;
                }
                let vs = &verts[c.first as usize..(c.first + c.count) as usize];
                for t in vs.as_chunks::<3>().0 {
                    tri(px, dp, width, y0 as i32, rows, t, img, st, t[0].alpha_ref);
                }
            }
        };
        if threads == 1 {
            run(0, &mut target.pixels, &mut target.depth);
            return;
        }
        let n = band * w as usize;
        std::thread::scope(|s| {
            for (i, (px, dp)) in target.pixels.chunks_mut(n).zip(target.depth.chunks_mut(n)).enumerate() {
                let run = &run;
                s.spawn(move || run(i * band, px, dp));
            }
        });
    }
}
