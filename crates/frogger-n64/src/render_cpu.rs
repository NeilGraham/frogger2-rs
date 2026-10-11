//! The current (slow) N64 renderer: `f2-engine`'s faithful CPU transform builds the frame's
//! draw list ([`Renderer::build`]: screen-space `TlVertex` triangles + [`DrawCmd`]s) and this
//! module submits it to the RDP: 3D batches through `n64_draw_tris` (`rdpq_triangle`, TMEM-sized
//! textures), screen-aligned 2D quads (backdrops, overlays, text) through `n64_draw_rects`
//! (`rdpq_tex_blit`, full-resolution textures split by libdragon).
//!
//! **Interface for the RSP renderer (phase 2R)** — the front end (`app.rs`) only calls:
//! - [`RenderCpu::reset`] when it recreates the `Renderer` (level change: all images go),
//! - [`RenderCpu::convert_image`] as the texture-bank load hook (keeps the RDRAM peak low by
//!   dropping each RGBA8 image right after conversion),
//! - [`RenderCpu::upload`] once per frame before drawing (images the game replaced/added),
//! - [`RenderCpu::submit`] with the filled `Renderer` (after `f2_draw::render::draw_loop`)
//!   and the clear colour; it begins, draws and presents the frame.
//!
//! A replacement keeps these four calls (or the same shape) and may consume the scene at a
//! higher level instead of `Renderer::build`'s triangles.

use crate::ffi::{self, Rect, State, TexDesc, Vertex};
use crate::rt::{self, ticks};
use crate::texconv::{self, N64Tex};
use alloc::vec::Vec;
use f2_engine::renderer::{Blend, DepthMode, DrawCmd};
use f2_engine::scene::TlVertex;
use f2_engine::textures::{Image, TextureRegistry};
use f2_engine::Renderer;

/// Per-frame counters (logged by the front end).
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameStats {
    pub tris: u32,
    pub rects: u32,
    pub cmds: u32,
    /// C0 ticks spent in `Renderer::build` and in submission (incl. the wait for a free
    /// framebuffer in `n64_begin_frame`).
    pub build_ticks: u32,
    pub submit_ticks: u32,
}

pub struct RenderCpu {
    /// Registry image index → N64 texture handle (0 = untextured / not uploaded) and its bytes.
    tex: Vec<u32>,
    bytes: Vec<u32>,
    verts: Vec<Vertex>,
    rects: Vec<Rect>,
    pub stats: FrameStats,
    /// Bytes of N64 texels currently allocated.
    pub tex_bytes: u32,
    /// Perspective-correct texturing of 3D batches (`RenderOptions::perspective_correct`).
    persp: bool,
}

fn desc(t: &N64Tex) -> TexDesc {
    TexDesc { width: t.w, height: t.h, fmt: t.fmt, pixels: t.data.as_ptr() }
}

impl Default for RenderCpu {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderCpu {
    pub fn new() -> RenderCpu {
        RenderCpu {
            tex: Vec::new(),
            bytes: Vec::new(),
            verts: Vec::with_capacity(4096),
            rects: Vec::with_capacity(512),
            stats: FrameStats::default(),
            tex_bytes: 0,
            persp: false,
        }
    }

    /// Frees every texture (the `Renderer` and its registry were recreated).
    pub fn reset(&mut self) {
        for h in self.tex.drain(..) {
            if h != 0 {
                rt::tex_free(h);
            }
        }
        self.bytes.clear();
        self.tex_bytes = 0;
    }

    /// Frees the textures of registry images `first..` (a level unload; fonts stay).
    pub fn free_from(&mut self, first: u32) {
        for i in first as usize..self.tex.len() {
            self.release(i);
        }
    }

    /// Converts and uploads image `idx` and frees its RGBA8 pixels (the registry keeps the
    /// size; nothing on the N64 reads the pixels back). `keep_full`: also keep the
    /// full-resolution version for 2D blits when it does not fit TMEM (fonts, screens);
    /// bank textures (mostly 3D) keep only the TMEM-sized one to save RDRAM.
    pub fn convert_image_ex(&mut self, idx: u32, img: &mut Image, keep_full: bool) {
        let i = idx as usize;
        self.release(i);
        if img.rgba.is_empty() {
            return; // already converted (pixels dropped)
        }
        // The 1x1 white image stands for "untextured".
        if img.width == 1 && img.height == 1 && img.rgba == [255, 255, 255, 255] {
            return;
        }
        let (full, small) = texconv::convert(img.width, img.height, &img.rgba);
        img.rgba = Vec::new();
        let (full, small) = match (keep_full, small) {
            (false, Some(s)) => (s, None),
            (_, s) => (full, s),
        };
        let fd = desc(&full);
        let sd = small.as_ref().map(desc);
        self.tex[i] = rt::tex_create2(&fd, sd.as_ref());
        self.bytes[i] = (full.data.len() + small.as_ref().map_or(0, |s| s.data.len())) as u32;
        self.tex_bytes += self.bytes[i];
    }

    /// Frees image `i`'s N64 texture (if any), growing the tables to cover `i`.
    fn release(&mut self, i: usize) {
        if self.tex.len() <= i {
            self.tex.resize(i + 1, 0);
            self.bytes.resize(i + 1, 0);
        }
        if self.tex[i] != 0 {
            rt::tex_free(self.tex[i]);
            self.tex[i] = 0;
            self.tex_bytes -= self.bytes[i];
            self.bytes[i] = 0;
        }
    }

    /// Texture-bank load hook: [`convert_image_ex`](Self::convert_image_ex) keeping only the
    /// TMEM-sized version.
    pub fn convert_image(&mut self, idx: u32, img: &mut Image) {
        self.convert_image_ex(idx, img, false);
    }

    /// Uses an already converted texture for registry image `idx` (whose RGBA8 pixels are a
    /// placeholder, dropped here).
    pub fn adopt(&mut self, idx: u32, img: &mut Image, t: N64Tex) {
        let i = idx as usize;
        self.release(i);
        self.bytes[i] = t.data.len() as u32;
        self.tex_bytes += self.bytes[i];
        self.tex[i] = rt::tex_create2(&desc(&t), None);
        img.rgba = Vec::new();
    }

    /// Uploads the images added or replaced since the last call.
    pub fn upload(&mut self, reg: &mut TextureRegistry) {
        for i in reg.take_dirty() {
            if let Some(img) = reg.images.get_mut(i as usize) {
                if img.rgba.is_empty() && self.tex.get(i as usize).is_some_and(|&h| h != 0) {
                    continue; // converted by the load hook
                }
                self.convert_image_ex(i, img, true);
            }
        }
    }

    fn state(&self, c: &DrawCmd, aref: f32) -> State {
        let mut flags = ffi::BILINEAR | ffi::UVNORM;
        flags |= match c.depth {
            DepthMode::TestWrite => ffi::ZTEST | ffi::ZWRITE,
            DepthMode::Test => ffi::ZTEST,
            DepthMode::Off => 0,
        };
        if c.wrap {
            flags |= ffi::WRAP;
        }
        if c.scene && self.persp {
            flags |= ffi::PERSP;
        }
        let blend = match c.blend {
            Blend::Semi => ffi::BLEND_SEMI,
            Blend::Additive => ffi::BLEND_ADD,
            Blend::Subtractive => ffi::BLEND_SUB,
        };
        // TlVertex: fragments with a*255 <= ref are discarded; RDP: alpha >= threshold passes.
        let alpha_ref = (aref.max(0.0) as u32 + 1).min(255);
        State { flags, blend, tex: self.tex.get(c.tex.0 as usize).copied().unwrap_or(0), alpha_ref }
    }

    /// Builds the frame's draw list and draws it (begin → batches → present).
    pub fn submit(&mut self, r: &mut Renderer, clear: [f64; 4]) {
        let t0 = ticks();
        self.persp = r.options.perspective_correct;
        let (verts, cmds) = r.build(clear);
        let t1 = ticks();
        let c = clear.map(|x| (x.clamp(0.0, 1.0) * 255.0) as u32);
        rt::begin_frame((c[0] << 24) | (c[1] << 16) | (c[2] << 8) | 0xff);
        let mut st = FrameStats { cmds: cmds.len() as u32, ..Default::default() };
        for (k, cmd) in cmds.iter().enumerate() {
            let tv = &verts[cmd.first as usize..(cmd.first + cmd.count) as usize];
            if tv.is_empty() {
                continue;
            }
            let state = self.state(cmd, tv[0].alpha_ref);
            if cmd.scene {
                self.tris(tv, &state, &mut st);
            } else {
                self.overlay(tv, &state, &mut st);
            }
            if k % 16 == 15 {
                rt::audio_poll();
            }
        }
        rt::present();
        st.build_ticks = t1.wrapping_sub(t0);
        st.submit_ticks = ticks().wrapping_sub(t1);
        self.stats = st;
    }

    fn tris(&mut self, tv: &[TlVertex], state: &State, st: &mut FrameStats) {
        self.verts.clear();
        self.verts.extend(tv.iter().map(vertex));
        rt::draw_tris(&self.verts, state);
        st.tris += (tv.len() / 3) as u32;
    }

    /// 2D batches: screen-aligned quads become blits, anything else triangles.
    fn overlay(&mut self, tv: &[TlVertex], state: &State, st: &mut FrameStats) {
        self.rects.clear();
        self.verts.clear();
        let flush_tris = |s: &mut Self, st: &mut FrameStats| {
            if !s.verts.is_empty() {
                let mut ts = *state;
                ts.flags &= !ffi::PERSP;
                rt::draw_tris(&s.verts, &ts);
                st.tris += (s.verts.len() / 3) as u32;
                s.verts.clear();
            }
        };
        let flush_rects = |s: &mut Self, st: &mut FrameStats| {
            if !s.rects.is_empty() {
                rt::draw_rects(&s.rects, state);
                st.rects += s.rects.len() as u32;
                s.rects.clear();
            }
        };
        let mut i = 0;
        while i < tv.len() {
            if i + 6 <= tv.len() {
                if let Some(r) = as_rect(&tv[i..i + 6]) {
                    flush_tris(self, st);
                    self.rects.push(r);
                    i += 6;
                    continue;
                }
            }
            flush_rects(self, st);
            let end = (i + 3).min(tv.len());
            self.verts.extend(tv[i..end].iter().map(vertex));
            i = end;
        }
        flush_tris(self, st);
        flush_rects(self, st);
    }
}

fn vertex(v: &TlVertex) -> Vertex {
    Vertex {
        x: v.sx,
        y: v.sy,
        z: v.sz.clamp(0.0, 1.0),
        r: v.color[0].clamp(0.0, 1.0),
        g: v.color[1].clamp(0.0, 1.0),
        b: v.color[2].clamp(0.0, 1.0),
        a: v.color[3].clamp(0.0, 1.0),
        s: v.uv[0],
        t: v.uv[1],
        inv_w: if v.rhw > 0.0 { v.rhw.min(1.0) } else { 1.0 },
    }
}

/// `Renderer::build`'s quad pattern (v0 v1 v2, v2 v3 v0; TL TR BR BL) when it is a
/// screen-aligned rectangle with one colour and aligned UVs.
pub fn as_rect(q: &[TlVertex]) -> Option<Rect> {
    let p = |v: &TlVertex| (v.sx, v.sy);
    if p(&q[3]) != p(&q[2]) || p(&q[5]) != p(&q[0]) || q[3].uv != q[2].uv || q[5].uv != q[0].uv {
        return None;
    }
    let (a, b, c, d) = (&q[0], &q[1], &q[2], &q[4]);
    let col = a.color;
    if b.color != col || c.color != col || d.color != col {
        return None;
    }
    // Edges a-b and d-c horizontal, b-c and a-d vertical.
    if a.sy != b.sy || d.sy != c.sy || b.sx != c.sx || a.sx != d.sx {
        return None;
    }
    if a.uv[1] != b.uv[1] || d.uv[1] != c.uv[1] || b.uv[0] != c.uv[0] || a.uv[0] != d.uv[0] {
        return None;
    }
    let (mut x0, mut x1, mut s0, mut s1) = (a.sx, b.sx, a.uv[0], b.uv[0]);
    if x0 > x1 {
        core::mem::swap(&mut x0, &mut x1);
        core::mem::swap(&mut s0, &mut s1);
    }
    let (mut y0, mut y1, mut t0, mut t1) = (a.sy, d.sy, a.uv[1], d.uv[1]);
    if y0 > y1 {
        core::mem::swap(&mut y0, &mut y1);
        core::mem::swap(&mut t0, &mut t1);
    }
    let c8 = col.map(|x| (x.clamp(0.0, 1.0) * 255.0 + 0.5) as u32);
    Some(Rect { x0, y0, x1, y1, s0, t0, s1, t1, rgba: (c8[0] << 24) | (c8[1] << 16) | (c8[2] << 8) | c8[3] })
}
