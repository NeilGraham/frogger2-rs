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

/// Testing (`F2N64_TRI_DIAG`): the replay tick, for the off-screen triangle log.
pub static DIAG_TICK: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

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
    built: Built,
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
        const IMAGE_SLOTS: usize = 1024;
        RenderCpu {
            // Per registry image; reserved at boot so the lists do not grow during a level
            // load (they live for the whole run: main heap, not the level heap).
            tex: Vec::with_capacity(IMAGE_SLOTS),
            bytes: Vec::with_capacity(IMAGE_SLOTS),
            // `submit`'s buffers (the render-cpu build; the r3d build fills `built`): 184 KiB.
            verts: Vec::with_capacity(if cfg!(feature = "render-cpu") { 4096 } else { 0 }),
            rects: Vec::with_capacity(if cfg!(feature = "render-cpu") { 512 } else { 0 }),
            stats: FrameStats::default(),
            tex_bytes: 0,
            persp: false,
            skip_once: Vec::new(),
            gen: Vec::with_capacity(IMAGE_SLOTS),
            ext: Vec::with_capacity(IMAGE_SLOTS),
            built: Built::default(),
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

    /// Refills registry image `idx`'s texture in place from a procedural texture's
    /// `side` x `side` ARGB4444 surface: the texels `convert_image_ex` makes from its RGBA8
    /// expansion (`* 17`), without that copy. `false` if the texture cannot be updated in
    /// place (none yet, an NTX placeholder, another format): use the registry path.
    pub fn update_argb4444(&mut self, idx: u32, side: u32, s: &[u16]) -> bool {
        let h = self.tex.get(idx as usize).copied().unwrap_or(0);
        if h == 0 || s.len() != (side * side) as usize {
            return false;
        }
        let partial = s.iter().any(|&c| (0x1000..0xf000).contains(&c));
        let fmt = if partial && side * side * 4 <= texconv::TMEM { texconv::FMT_RGBA32 } else { texconv::FMT_RGBA16 };
        if fmt == texconv::FMT_RGBA16 && side * side * 2 > texconv::TMEM {
            return false;
        }
        // Straight into the texture's (uncached) texels: one store per texel.
        let desc = TexDesc { width: side, height: side, fmt, pixels: core::ptr::null() };
        // Whole-word stores: through `[u8; N]` chunks (alignment 1) the compiler wrote each
        // texel as an swl/swr pair (RGBA32) or two byte stores (RGBA16), each one an uncached
        // RDRAM access. The texels are 64-byte aligned (`surface_alloc`); the fallback keeps
        // any other buffer correct.
        rt::tex_update_with(h, &desc, |out| {
            if fmt == texconv::FMT_RGBA32 {
                // SAFETY: plain integers, any bit pattern is valid.
                let (head, words, _) = unsafe { out.align_to_mut::<u32>() };
                if head.is_empty() {
                    for (o, &c) in words.iter_mut().zip(s) {
                        *o = texconv::argb4444_to_rgba32(c).to_be();
                    }
                } else {
                    for (o, &c) in out.as_chunks_mut::<4>().0.iter_mut().zip(s) {
                        *o = texconv::argb4444_to_rgba32(c).to_be_bytes();
                    }
                }
            } else {
                // Two texels per store.
                // SAFETY: as above.
                let (head, words, tail) = unsafe { out.align_to_mut::<u32>() };
                if head.is_empty() {
                    let (pairs, last) = s.as_chunks::<2>();
                    for (o, c) in words.iter_mut().zip(pairs) {
                        let w = (texconv::argb4444_to_rgba16(c[0]) as u32) << 16 | texconv::argb4444_to_rgba16(c[1]) as u32;
                        *o = w.to_be();
                    }
                    if let (Some(&c), Some(t)) = (last.first(), tail.first_chunk_mut::<2>()) {
                        *t = texconv::argb4444_to_rgba16(c).to_be_bytes();
                    }
                } else {
                    for (o, &c) in out.as_chunks_mut::<2>().0.iter_mut().zip(s) {
                        *o = texconv::argb4444_to_rgba16(c).to_be_bytes();
                    }
                }
            }
        })
    }

    /// Registry image `idx` shows `rgba` (a render-to-texture result: the story book page),
    /// converted straight from the caller's pixels (the registry keeps none).
    /// Replaces image `idx` with `rgba`; false if there was no memory for the N64 texture
    /// (the image then has none).
    pub fn grab_rgba(&mut self, idx: u32, w: u32, h: u32, rgba: &[u8]) -> bool {
        let i = idx as usize;
        let (mut full, small) = texconv::convert(w, h, rgba);
        if small.is_some() {
            full.fmt |= ffi::TEX_TILE3D;
        }
        self.release(i);
        let sd = small.as_ref().map(desc);
        self.tex[i] = rt::tex_create2(&desc(&full), sd.as_ref());
        self.gen[i] = self.gen[i].wrapping_add(1);
        if self.tex[i] == 0 {
            return false;
        }
        self.bytes[i] = (full.data.len() + small.as_ref().map_or(0, |s| s.data.len())) as u32;
        self.tex_bytes += self.bytes[i];
        true
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
    /// Frees image `idx`'s N64 texture now (before a large replacement is converted).
    pub fn free_image(&mut self, idx: u32) {
        self.release(idx as usize);
    }

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
            rt::audio_poll_due();
        }
    }

    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_f")]
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

    /// Builds the frame's CPU draw list for [`draw_segment`](Self::draw_segment): the
    /// [`Renderer::frame_view`] parts converted straight into RDP batches (the batches
    /// `Renderer::build_segments` + [`quads`](Self::quads) / [`tris`](Self::tris) make,
    /// without the intermediate vertex list).
    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_f")]
    pub fn build(&mut self, r: &mut Renderer, clear: [f64; 4]) {
        let t0 = ticks();
        self.persp = r.options.perspective_correct;
        let fv = r.frame_view(clear);
        let mut b = core::mem::take(&mut self.built);
        b.clear();
        let white = fv.white;
        // Segment 0: viewport clear + backdrops; 8: overlays (2D quads).
        for q in fv.viewport_clear.iter().chain(fv.backdrops) {
            self.add_quad(&mut b, 0, q, white);
        }
        for (k, (ps, tris)) in fv.passes.iter().enumerate() {
            let seg = k as u8 + 1;
            // Rect-eligible: sprites (screen-aligned quads at one depth) in a Z-tested,
            // non-writing, clamped pass.
            let rects = ps.depth != DepthMode::TestWrite && !ps.wrap;
            let mut i = 0;
            while i < tris.len() {
                // A run of one texture (`build_segments` merges these into one command;
                // quads pair up from its first triangle).
                let tex = tris[i].tex.unwrap_or(white);
                let mut j = i + 1;
                while j < tris.len() && tris[j].tex.unwrap_or(white) == tex {
                    j += 1;
                }
                let c = DrawCmd { blend: ps.blend, depth: ps.depth, tex, wrap: ps.wrap, scene: true, first: 0, count: 0 };
                let state = self.state(&c, ps.alpha_ref);
                self.add_scene_run(&mut b, seg, &state, &tris[i..j], rects);
                i = j;
            }
        }
        for q in fv.overlays {
            self.add_quad(&mut b, 8, q, white);
        }
        // Testing (F2N64_TRI_DIAG): per frame, the number of scene triangles with a vertex
        // outside main.c's guard band (clipped or rejected there) and the one reaching
        // farthest, with the replay tick (`tridiag tick T n N pass P ext E tex X v ...`).
        if option_env!("F2N64_TRI_DIAG").is_some() {
            let mut n = 0u32;
            let mut worst: Option<(usize, f32, &f2_engine::Tri)> = None;
            for (k, (_, tris)) in fv.passes.iter().enumerate() {
                for t in tris.iter() {
                    if t.v.iter().all(|v| (-1024.0..=1344.0).contains(&v.sx) && (-1024.0..=1264.0).contains(&v.sy)) {
                        continue;
                    }
                    n += 1;
                    let e = t.v.iter().map(|v| (v.sx - 160.0).abs().max((v.sy - 120.0).abs())).fold(0.0f32, f32::max);
                    if worst.is_none_or(|w| e > w.1) {
                        worst = Some((k, e, t));
                    }
                }
            }
            if let Some((k, e, t)) = worst {
                let v = &t.v;
                crate::log!(
                    "tridiag tick {} n {} pass {} ext {} tex {:?} v ({} {} {}) ({} {} {}) ({} {} {}) rgba {:.2} {:.2} {:.2} {:.2}",
                    DIAG_TICK.load(core::sync::atomic::Ordering::Relaxed),
                    n, k, e as i32, t.tex.map(|h| h.0),
                    v[0].sx, v[0].sy, v[0].sz, v[1].sx, v[1].sy, v[1].sz, v[2].sx, v[2].sy, v[2].sz,
                    v[0].color[0], v[0].color[1], v[0].color[2], v[0].color[3]
                );
            }
        }
        self.stats = FrameStats { cmds: b.batches.len() as u32, build_ticks: ticks().wrapping_sub(t0), ..Default::default() };
        self.built = b;
    }

    /// One 2D quad of segment `seg` (as `build_segments` + [`quads`](Self::quads)).
    fn add_quad(&self, b: &mut Built, seg: u8, q: &f2_engine::Quad2D, white: f2_engine::TexHandle) {
        let c = DrawCmd { blend: q.blend, depth: DepthMode::Off, tex: q.tex.unwrap_or(white), wrap: false, scene: false, first: 0, count: 0 };
        let state = self.state(&c, f2_engine::renderer::ALPHA_NONZERO);
        let conv = colour_conv(state.blend);
        let p = &q.pos;
        // `as_rect` of `Quad2D::tl_vertices` without building them: the quad is a
        // screen-aligned rectangle when its edges are (UVs and colour always line up).
        if p[0][1] == p[1][1] && p[3][1] == p[2][1] && p[1][0] == p[2][0] && p[0][0] == p[3][0] {
            let r = rect_of(p[0][0], p[1][0], q.uv[0], q.uv[2], p[0][1], p[3][1], q.uv[1], q.uv[3]);
            b.rect(seg, &state, Rect { rgba: pack_colour(q.color.map(|c| c as f32 / 255.0), conv), ..r });
            return;
        }
        let mut tv = q.tl_vertices();
        if let Some(f) = conv {
            for v in &mut tv {
                *v = converted(v, f);
            }
        }
        let mut ts = state;
        ts.flags &= !ffi::PERSP;
        for v in &tv {
            b.vert(seg, &ts, vertex(v));
        }
    }

    /// A run of one pass's triangles with one texture: sprite quads as Z-tested
    /// rectangles when `rects`, the rest as triangles at their depth (`sz * zk`).
    fn add_scene_run(&self, b: &mut Built, seg: u8, state: &State, tris: &[f2_engine::Tri], rects: bool) {
        let conv = colour_conv(state.blend);
        let mut i = 0;
        while i < tris.len() {
            if rects && i + 1 < tris.len() {
                let (p, q) = (&tris[i].v, &tris[i + 1].v);
                let six = [&p[0], &p[1], &p[2], &q[0], &q[1], &q[2]];
                if six[1].sz == six[0].sz && six[2].sz == six[0].sz && six[4].sz == six[0].sz {
                    // (One colour before the split is one colour after it.)
                    if let Some(r) = rect_geometry(six) {
                        let z = (six[0].sz * self.zk).clamp(0.0, 1.0);
                        b.rect(seg, state, Rect { rgba: pack_colour(six[0].color, conv), z, ..r });
                        i += 2;
                        continue;
                    }
                }
            }
            // Converted by C at submission (`n64_draw_tl_tris`, as `tris` converts them).
            b.tl_tri(seg, state, &tris[i]);
            i += 1;
        }
    }

    /// Logs the built frame's CPU batches (debug aid: `F2N64_DUMP_FRAME`): segment, state,
    /// triangle / rectangle count and the name of a texture entry showing it.
    pub fn dump(&self, reg: &TextureRegistry) {
        for bt in &self.built.batches {
            let name = reg.entries.iter().find(|e| e.current.is_some_and(|h| self.tex.get(h.0 as usize) == Some(&bt.state.tex))).map_or("", |e| e.name.as_str());
            crate::log!(
                "dump cpu seg {} flags {:x} blend {} aref {} n64 tex {} {} {} {}",
                bt.seg,
                bt.state.flags,
                bt.state.blend,
                bt.state.alpha_ref,
                bt.state.tex,
                name,
                match bt.kind {
                    Kind::Rects => "rects",
                    _ => "tris",
                },
                if bt.kind == Kind::Verts { bt.count / 3 } else { bt.count }
            );
            if bt.kind == Kind::Tl {
                // SAFETY: as in `draw_segment` (the frame is unchanged since `build`).
                let tris = unsafe { core::slice::from_raw_parts(bt.first as *const f2_engine::Tri, bt.count as usize) };
                for t in tris {
                    let v = &t.v;
                    crate::log!(
                        "dump cpu  tri ({:.0} {:.0} {:.4}) ({:.0} {:.0} {:.4}) ({:.0} {:.0} {:.4}) rgba {:.2} {:.2} {:.2} {:.2}",
                        v[0].sx, v[0].sy, v[0].sz, v[1].sx, v[1].sy, v[1].sz, v[2].sx, v[2].sy, v[2].sz,
                        v[0].color[0], v[0].color[1], v[0].color[2], v[0].color[3]
                    );
                }
            }
        }
    }

    /// Draws segment `seg` of the built frame (0 = backdrops, 1..=7 = the 3D passes in
    /// `DrawAllFrames` order, 8 = 2D overlays).
    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_f")]
    pub fn draw_segment(&mut self, seg: usize) {
        let t0 = ticks();
        let b = &self.built;
        let mut st = self.stats;
        let mut n = 0;
        for bt in b.batches.iter().filter(|bt| bt.seg as usize == seg) {
            let (a, e) = (bt.first as usize, bt.first as usize + bt.count as usize);
            let tb = ticks();
            match bt.kind {
                Kind::Rects => {
                    rt::draw_rects(&b.rects[a..e], &bt.state);
                    st.rects += bt.count;
                }
                Kind::Verts => {
                    rt::draw_tris(&b.verts[a..e], &bt.state);
                    st.tris += bt.count / 3;
                }
                Kind::Tl => {
                    // SAFETY: `bt.first` points at `count` consecutive triangles of the
                    // renderer's frame lists, unchanged since `build` (the frame is drawn
                    // before the next `Renderer::begin_frame`).
                    let tris = unsafe { core::slice::from_raw_parts(bt.first as *const f2_engine::Tri, bt.count as usize) };
                    rt::draw_tl_tris(tris, self.zk, &bt.state);
                    st.tris += bt.count;
                }
            }
            let dt = ticks().wrapping_sub(tb);
            if dt > rt::TICKS_PER_SEC / 500 {
                crate::log!("slow cpu batch (Rust side): seg {seg} {:?} x{} tex {} flags {:x} blend {}: {} us", bt.kind, bt.count, bt.state.tex, bt.state.flags, bt.state.blend, rt::ticks_to_us(dt));
            }
            n += 1;
            // Only when due (40 ms): an unconditional poll here ran the mixer whenever the AI
            // wanted data, which put ~1 ms of audio into this segment's submit time.
            if n % 16 == 0 {
                rt::audio_poll_due();
            }
        }
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

    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_f")]
    fn tris(&mut self, tv: &[TlVertex], state: &State, st: &mut FrameStats) {
        self.verts.clear();
        let zk = self.zk;
        let (add, sub) = (state.blend == ffi::BLEND_ADD, state.blend == ffi::BLEND_SUB);
        self.verts.extend(tv.iter().map(|v| {
            let mut o = Vertex { z: (v.sz * zk).clamp(0.0, 1.0), ..vertex(v) };
            if add {
                additive_colour(&mut o);
            } else if sub {
                subtractive_colour(&mut o);
            }
            o
        }));
        rt::draw_tris(&self.verts, state);
        st.tris += (tv.len() / 3) as u32;
    }

    /// 2D batches: screen-aligned quads become blits, anything else triangles.
    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_f")]
    fn overlay(&mut self, tv: &[TlVertex], state: &State, st: &mut FrameStats) {
        self.quads(tv, state, false, st);
    }

    /// Screen-aligned quads become texture rectangles, anything else triangles. `scene`:
    /// a 3D pass (sprites): a quad must lie at one depth, which becomes the rectangle's
    /// primitive depth, and the triangles keep their depth (as [`tris`](Self::tris)).
    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_f")]
    fn quads(&mut self, tv: &[TlVertex], state: &State, scene: bool, st: &mut FrameStats) {
        self.rects.clear();
        self.verts.clear();
        // Additive: the colour split of `additive_colour` (main.c draws the rects and
        // triangles from the texture's additive variant); subtractive: `subtractive_colour`.
        let split;
        let conv: Option<fn(&mut Vertex)> = match state.blend {
            ffi::BLEND_ADD => Some(additive_colour),
            ffi::BLEND_SUB => Some(subtractive_colour),
            _ => None,
        };
        let tv = if let Some(f) = conv {
            split = tv
                .iter()
                .map(|v| {
                    let mut o = vertex(v);
                    f(&mut o);
                    TlVertex { color: [o.r, o.g, o.b, o.a], ..*v }
                })
                .collect::<Vec<_>>();
            &split[..]
        } else {
            tv
        };
        let zk = self.zk;
        let flush_tris = |s: &mut Self, st: &mut FrameStats| {
            if !s.verts.is_empty() {
                let mut ts = *state;
                if !scene {
                    ts.flags &= !ffi::PERSP;
                }
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
                let q = &tv[i..i + 6];
                let flat = !scene || (q[1].sz == q[0].sz && q[2].sz == q[0].sz && q[4].sz == q[0].sz);
                if let Some(mut r) = as_rect(q).filter(|_| flat) {
                    if scene {
                        r.z = (q[0].sz * zk).clamp(0.0, 1.0);
                    }
                    flush_tris(self, st);
                    self.rects.push(r);
                    i += 6;
                    continue;
                }
            }
            flush_rects(self, st);
            let end = (i + 3).min(tv.len());
            if scene {
                self.verts.extend(tv[i..end].iter().map(|v| Vertex { z: (v.sz * zk).clamp(0.0, 1.0), ..vertex(v) }));
            } else {
                self.verts.extend(tv[i..end].iter().map(vertex));
            }
            i = end;
        }
        flush_tris(self, st);
        flush_rects(self, st);
    }
}

/// RDP batches of a frame ([`RenderCpu::build`]): vertices / rectangles and runs of them.
#[derive(Default)]
struct Built {
    verts: Vec<Vertex>,
    rects: Vec<Rect>,
    batches: Vec<Batch>,
}

/// What a [`Batch`] draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// `count` vertices (whole triangles) of `Built::verts` from `first`.
    Verts,
    /// `count` rectangles of `Built::rects` from `first`.
    Rects,
    /// `count` consecutive `f2_engine::Tri`s at address `first` (the renderer's frame list).
    Tl,
}

/// A run of one kind of primitive with one state.
struct Batch {
    seg: u8,
    kind: Kind,
    state: State,
    first: usize,
    count: u32,
}

impl Built {
    fn clear(&mut self) {
        self.verts.clear();
        self.rects.clear();
        self.batches.clear();
    }

    /// The batch to append to (a new one unless the last has this segment, kind and state
    /// and ends at `first`).
    fn open(&mut self, seg: u8, kind: Kind, state: &State, first: usize, unit: usize) -> &mut Batch {
        let same = self.batches.last().is_some_and(|l| l.seg == seg && l.kind == kind && l.state == *state && l.first + l.count as usize * unit == first);
        if !same {
            self.batches.push(Batch { seg, kind, state: *state, first, count: 0 });
        }
        self.batches.last_mut().unwrap()
    }

    fn vert(&mut self, seg: u8, state: &State, v: Vertex) {
        let n = self.verts.len();
        self.open(seg, Kind::Verts, state, n, 1).count += 1;
        self.verts.push(v);
    }

    fn rect(&mut self, seg: u8, state: &State, r: Rect) {
        let n = self.rects.len();
        self.open(seg, Kind::Rects, state, n, 1).count += 1;
        self.rects.push(r);
    }

    fn tl_tri(&mut self, seg: u8, state: &State, t: &f2_engine::Tri) {
        self.open(seg, Kind::Tl, state, t as *const _ as usize, core::mem::size_of::<f2_engine::Tri>()).count += 1;
    }
}

/// The vertex colour split of an additive / subtractive batch.
fn colour_conv(blend: u32) -> Option<fn(&mut Vertex)> {
    match blend {
        ffi::BLEND_ADD => Some(additive_colour),
        ffi::BLEND_SUB => Some(subtractive_colour),
        _ => None,
    }
}

/// `v` with the colour split `f` applied (as [`RenderCpu::quads`] converts a batch).
fn converted(v: &TlVertex, f: fn(&mut Vertex)) -> TlVertex {
    let mut o = vertex(v);
    f(&mut o);
    TlVertex { color: [o.r, o.g, o.b, o.a], ..*v }
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

/// Subtractive batches (D3D `ZERO, INVSRCCOLOR`: `dst * (1 - colour)`, alpha unused) are
/// drawn with the alpha blender as black at alpha = the colour's brightest channel:
/// `dst * (1 - a)`, exact for grey (the screen fades; drawn opaque grey before, so a fade
/// from black flashed white).
fn subtractive_colour(v: &mut Vertex) {
    v.a = v.r.max(v.g).max(v.b);
    v.r = 0.0;
    v.g = 0.0;
    v.b = 0.0;
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
    let r = rect_geometry([&q[0], &q[1], &q[2], &q[3], &q[4], &q[5]])?;
    Some(Rect { rgba: pack_colour(q[0].color, None), ..r })
}

/// [`as_rect`]'s test and rectangle (without the colour) on six vertex references.
fn rect_geometry(q: [&TlVertex; 6]) -> Option<Rect> {
    let p = |v: &TlVertex| (v.sx, v.sy);
    if p(q[3]) != p(q[2]) || p(q[5]) != p(q[0]) || q[3].uv != q[2].uv || q[5].uv != q[0].uv {
        return None;
    }
    let (a, b, c, d) = (q[0], q[1], q[2], q[4]);
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
    Some(rect_of(a.sx, b.sx, a.uv[0], b.uv[0], a.sy, d.sy, a.uv[1], d.uv[1]))
}

/// The rectangle with x / s from `(x0, s0)` to `(x1, s1)` and y / t likewise, ordered so
/// x0 < x1 and y0 < y1 (the texture coordinates follow, so mirrored quads flip).
#[allow(clippy::too_many_arguments)]
fn rect_of(mut x0: f32, mut x1: f32, mut s0: f32, mut s1: f32, mut y0: f32, mut y1: f32, mut t0: f32, mut t1: f32) -> Rect {
    if x0 > x1 {
        core::mem::swap(&mut x0, &mut x1);
        core::mem::swap(&mut s0, &mut s1);
    }
    if y0 > y1 {
        core::mem::swap(&mut y0, &mut y1);
        core::mem::swap(&mut t0, &mut t1);
    }
    Rect { x0, y0, x1, y1, s0, t0, s1, t1, rgba: 0, z: 0.0 }
}

/// A vertex colour (0..1) with the batch's colour split `conv` as 0xRRGGBBAA.
fn pack_colour(col: [f32; 4], conv: Option<fn(&mut Vertex)>) -> u32 {
    let col = match conv {
        Some(f) => {
            let mut o = Vertex { r: col[0].clamp(0.0, 1.0), g: col[1].clamp(0.0, 1.0), b: col[2].clamp(0.0, 1.0), a: col[3].clamp(0.0, 1.0), ..Default::default() };
            f(&mut o);
            [o.r, o.g, o.b, o.a]
        }
        None => col,
    };
    let c8 = col.map(|x| (x.clamp(0.0, 1.0) * 255.0 + 0.5) as u32);
    (c8[0] << 24) | (c8[1] << 16) | (c8[2] << 8) | c8[3]
}
