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
//!
//! With the RSP renderer (`render_r3d.rs`, the default) this module still owns the textures
//! (r3d aliases their TMEM-sized surfaces) and draws everything `f2-engine` builds on the CPU
//! (backdrops, sprites, effects, shadows, overlays, text): [`RenderCpu::build`] once per frame,
//! then [`RenderCpu::draw_segment`] for each `Renderer::build_segments` segment between the
//! RSP's frame lists.

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
    /// Images the load hook converted while keeping their pixels (skip their dirty entry).
    skip_once: Vec<u32>,
    /// Per image: bumped whenever its N64 texture handle changes (r3d aliases follow it).
    gen: Vec<u32>,
    /// Per image: the buffer an NTX texture's texels live in (a pack file shared by the
    /// bank's images, or a converted copy), kept alive while the texture exists.
    ext: Vec<Option<alloc::sync::Arc<Vec<u8>>>>,
    /// The frame built by [`build`](Self::build).
    fverts: Vec<TlVertex>,
    fcmds: Vec<DrawCmd>,
    marks: [u32; 10],
    /// Depth of scene triangles: RDP z = `sz * zk` (1 = the faithful 0..1 `sz`; r3d maps its
    /// linear depth differently, see `r3d::ViewParams::tl_z`).
    pub zk: f32,
}

/// Whether a bank texture is read back by the CPU (`f2_draw::book::draw_page_b`).
fn book_source(crc: u32) -> bool {
    crc == f2_formats::crc("page32.bmp") || f2_game::menus::CHAPTER_PIC.iter().any(|p| crc == f2_formats::crc(&alloc::format!("{p}.bmp").to_lowercase()))
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
            skip_once: Vec::new(),
            gen: Vec::new(),
            ext: Vec::new(),
            fverts: Vec::new(),
            fcmds: Vec::new(),
            marks: [0; 10],
            zk: 1.0,
        }
    }

    /// N64 texture of registry image `idx` (0 = none) and its generation.
    pub fn texture_of(&self, idx: u32) -> (u32, u32) {
        let i = idx as usize;
        (self.tex.get(i).copied().unwrap_or(0), self.gen.get(i).copied().unwrap_or(0))
    }

    /// Frees every texture (the `Renderer` and its registry were recreated).
    pub fn reset(&mut self) {
        for h in self.tex.drain(..) {
            if h != 0 {
                rt::tex_free(h);
            }
        }
        self.bytes.clear();
        self.ext.clear();
        self.skip_once.clear();
        self.tex_bytes = 0;
        for g in &mut self.gen {
            *g = g.wrapping_add(1);
        }
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
        self.convert_image_keep(idx, img, keep_full, false);
    }

    /// [`convert_image_ex`](Self::convert_image_ex); `keep_pixels` leaves the RGBA8 image in
    /// the registry (sources the CPU composes from: the story book page's `page32.bmp` and
    /// chapter pictures, read by `f2_draw::book::draw_page_b`).
    pub fn convert_image_keep(&mut self, idx: u32, img: &mut Image, keep_full: bool, keep_pixels: bool) {
        let i = idx as usize;
        if img.rgba.is_empty() {
            self.release(i);
            return; // already converted (pixels dropped)
        }
        self.grow(i);
        // The 1x1 white image stands for "untextured".
        if img.width == 1 && img.height == 1 && img.rgba == [255, 255, 255, 255] {
            self.release(i);
            return;
        }
        let (mut full, small) = texconv::convert(img.width, img.height, &img.rgba);
        // Same size and format as the current texture (procedural textures every frame):
        // update in place, keeping the handle (and r3d's alias of it).
        if small.is_none() && self.tex.get(i).is_some_and(|&h| h != 0) && rt::tex_update(self.tex[i], &desc(&full)) {
            if !keep_pixels {
                img.rgba = Vec::new();
            }
            return;
        }
        if keep_pixels {
            self.skip_once.push(idx);
        } else {
            img.rgba = Vec::new();
        }
        let (full, small) = match (keep_full, small) {
            (false, Some(s)) => (s, None),
            (_, s) => {
                if s.is_some() {
                    // Too big for TMEM: triangles draw it in TMEM tiles (render-to-texture
                    // pages, screens) rather than from the downscaled copy.
                    full.fmt |= ffi::TEX_TILE3D;
                }
                (full, s)
            }
        };
        self.release(i);
        let fd = desc(&full);
        let sd = small.as_ref().map(desc);
        self.tex[i] = rt::tex_create2(&fd, sd.as_ref());
        self.gen[i] = self.gen[i].wrapping_add(1);
        self.bytes[i] = (full.data.len() + small.as_ref().map_or(0, |s| s.data.len())) as u32;
        self.tex_bytes += self.bytes[i];
    }

    fn grow(&mut self, i: usize) {
        if self.tex.len() <= i {
            self.tex.resize(i + 1, 0);
            self.bytes.resize(i + 1, 0);
        }
        if self.gen.len() <= i {
            self.gen.resize(i + 1, 0);
        }
        if self.ext.len() <= i {
            self.ext.resize(i + 1, None);
        }
    }

    /// Registry image `idx` shows `rgba` (a render-to-texture result: the story book page),
    /// converted straight from the caller's pixels (the registry keeps none).
    pub fn grab_rgba(&mut self, idx: u32, w: u32, h: u32, rgba: &[u8]) {
        let i = idx as usize;
        let (mut full, small) = texconv::convert(w, h, rgba);
        if small.is_some() {
            full.fmt |= ffi::TEX_TILE3D;
        }
        self.release(i);
        let sd = small.as_ref().map(desc);
        self.tex[i] = rt::tex_create2(&desc(&full), sd.as_ref());
        self.gen[i] = self.gen[i].wrapping_add(1);
        self.bytes[i] = (full.data.len() + small.as_ref().map_or(0, |s| s.data.len())) as u32;
        self.tex_bytes += self.bytes[i];
    }

    /// Registry image `idx` shows an NTX1 image of the pack (`desc` points into `owner`).
    pub fn adopt_ntx(&mut self, idx: u32, owner: alloc::sync::Arc<Vec<u8>>, desc: &ffi::NtxDesc) -> bool {
        let i = idx as usize;
        self.release(i);
        let h = unsafe { ffi::n64_tex_create_ntx(desc) };
        if h == 0 {
            return false;
        }
        self.tex[i] = h;
        self.gen[i] = self.gen[i].wrapping_add(1);
        self.ext[i] = Some(owner);
        self.bytes[i] = desc.stride * desc.height;
        self.tex_bytes += self.bytes[i];
        true
    }

    /// Frees image `i`'s N64 texture (if any), growing the tables to cover `i`.
    fn release(&mut self, i: usize) {
        self.grow(i);
        if self.tex[i] != 0 {
            self.gen[i] = self.gen[i].wrapping_add(1);
            self.ext[i] = None;
            rt::tex_free(self.tex[i]);
            self.tex[i] = 0;
            self.tex_bytes -= self.bytes[i];
            self.bytes[i] = 0;
        }
    }

    /// Texture-bank load hook: [`convert_image_ex`](Self::convert_image_ex) keeping only the
    /// TMEM-sized version (and the RGBA8 pixels of the book page sources).
    pub fn convert_image(&mut self, idx: u32, img: &mut Image, t: &f2_formats::texture::Texture) {
        self.convert_image_keep(idx, img, false, book_source(t.crc));
    }

    /// Uses an already converted texture for registry image `idx` (whose RGBA8 pixels are a
    /// placeholder, dropped here).
    pub fn adopt(&mut self, idx: u32, img: &mut Image, t: N64Tex) {
        let i = idx as usize;
        self.release(i);
        self.bytes[i] = t.data.len() as u32;
        self.tex_bytes += self.bytes[i];
        self.tex[i] = rt::tex_create2(&desc(&t), None);
        self.gen[i] = self.gen[i].wrapping_add(1);
        img.rgba = Vec::new();
    }

    /// Uploads the images added or replaced since the last call.
    pub fn upload(&mut self, reg: &mut TextureRegistry) {
        for i in reg.take_dirty() {
            if let Some(k) = self.skip_once.iter().position(|&s| s == i) {
                self.skip_once.swap_remove(k);
                continue; // converted by the load hook, pixels kept on purpose
            }
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

    /// Builds the frame's CPU draw list (`Renderer::build_segments`) for
    /// [`draw_segment`](Self::draw_segment).
    pub fn build(&mut self, r: &mut Renderer, clear: [f64; 4]) {
        let t0 = ticks();
        self.persp = r.options.perspective_correct;
        let (verts, cmds, marks) = r.build_segments(clear);
        self.fverts = verts;
        self.fcmds = cmds;
        self.marks = marks;
        self.stats = FrameStats { cmds: self.fcmds.len() as u32, build_ticks: ticks().wrapping_sub(t0), ..Default::default() };
    }

    /// Logs the built frame's CPU commands (debug aid: `F2N64_DUMP_FRAME`): segment, state,
    /// triangle count, registry image and the name of a texture entry showing it.
    pub fn dump(&self, reg: &TextureRegistry) {
        for (k, c) in self.fcmds.iter().enumerate() {
            let seg = self.marks.iter().rposition(|&m| m as usize <= k).unwrap_or(0).min(8);
            let name = reg.entries.iter().find(|e| e.current == Some(c.tex)).map_or("", |e| e.name.as_str());
            let img = reg.images.get(c.tex.0 as usize).map_or((0, 0), |i| (i.width, i.height));
            let tv = &self.fverts[c.first as usize];
            crate::log!(
                "dump cpu seg {seg} {:?} {:?} wrap {} scene {} tris {} tex {} {:?} {} n64 {} col {:?} aref {}",
                c.blend,
                c.depth,
                c.wrap,
                c.scene,
                c.count / 3,
                c.tex.0,
                img,
                name,
                self.tex.get(c.tex.0 as usize).copied().unwrap_or(0),
                tv.color,
                tv.alpha_ref
            );
        }
    }

    /// Draws segment `seg` of the built frame (0 = backdrops, 1..=7 = the 3D passes in
    /// `DrawAllFrames` order, 8 = 2D overlays).
    pub fn draw_segment(&mut self, seg: usize) {
        let t0 = ticks();
        let (a, b) = (self.marks[seg] as usize, self.marks[seg + 1] as usize);
        let cmds = core::mem::take(&mut self.fcmds);
        let verts = core::mem::take(&mut self.fverts);
        let mut st = self.stats;
        for (k, cmd) in cmds[a..b].iter().enumerate() {
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
        self.fcmds = cmds;
        self.fverts = verts;
        st.submit_ticks += ticks().wrapping_sub(t0);
        self.stats = st;
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
        let zk = self.zk;
        let add = state.blend == ffi::BLEND_ADD;
        self.verts.extend(tv.iter().map(|v| {
            let mut o = Vertex { z: (v.sz * zk).clamp(0.0, 1.0), ..vertex(v) };
            if add {
                additive_colour(&mut o);
            }
            o
        }));
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

/// Additive batches are drawn with the alpha blender from each texture's additive variant
/// (main.c `make_add_variant`: colour / max, alpha * max); the vertex colour gets the same
/// split so the blended term `colour * alpha` equals D3D's `tex * diffuse * alpha` exactly.
fn additive_colour(v: &mut Vertex) {
    let m = v.r.max(v.g).max(v.b);
    if m > 0.0 {
        v.r /= m;
        v.g /= m;
        v.b /= m;
        v.a *= m;
    } else {
        v.a = 0.0;
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
