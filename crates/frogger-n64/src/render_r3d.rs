//! The game's renderer on the RSP (phase 3I): `f2-draw`'s draw loop runs unchanged with the
//! `Renderer`'s mesh sink on, so every landscape object / actor mesh it would transform on
//! the CPU is recorded as an [`f2_engine::MeshDraw`] (model, object, `xform_actor`
//! matrices, frame list, `PCRenderObject` parameters) and drawn here from the level's F2NM
//! meshes by `r3d` (MDX projection and face rejection on the RSP). Everything the engine
//! still builds on the CPU — backdrops, shadows, sprites, special effects, overlays, text —
//! is drawn by [`RenderCpu`] between r3d's frame lists, in the PC's `DrawAllFrames` order:
//!
//! ```text
//! backdrops | Normal: meshes, CPU tris | Wrap | Xlu | Additive | Late | LateAdditive | Overlay | 2D overlays
//! ```
//!
//! Inside a list the RSP meshes come before the CPU triangles of that list (the PC
//! interleaves them in submission order; only translucent overlaps can differ).
//!
//! Textures: [`RenderCpu`] converts the registry images as before; r3d aliases each image's
//! TMEM-sized surface ([`R3d::alias_texture`]), and every texture CRC is pointed at the image
//! the registry currently shows for it (animated textures follow `UpdateAnimatingTextures`,
//! procedural ones are updated in place).

use crate::r3d::{List, MeshId, R3d, R3dMark, TexId, ViewParams};
use crate::render_cpu::RenderCpu;
use crate::rt;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use f2_engine::{Frame, MeshBackend, MeshDraw, Renderer, TextureRegistry, View, MESH_SKIN};
use f2_formats::objbank::ObjectBank;

/// Per-frame counters.
#[derive(Clone, Copy, Debug, Default)]
pub struct R3dFrameStats {
    pub draws: u32,
    pub queued_tris: u32,
    pub sent_tris: u32,
    /// RSP command groups (vertex-cache batches) run and texture loads.
    pub groups: u32,
    pub tex_loads: u32,
    /// C0 ticks of the r3d CPU side (draw queueing + flush incl. the CPU segments).
    pub ticks: u32,
    /// Of which: queueing the recorded draws (matrices, culling) / flushing the lists to
    /// the RSP queue (without the CPU segments drawn between them).
    pub queue_ticks: u32,
    pub flush_ticks: u32,
    /// Draws reused from the previous frame (unchanged model matrix and view).
    pub reused: u32,
    /// Draws that kept their model-side work with a new view (camera moved).
    pub view_reused: u32,
}

pub struct R3dLevel {
    pub r: R3d,
    base: R3dMark,
    /// `(Model::id, object)` → mesh of the loaded level.
    meshes: BTreeMap<(u32, u16), MeshId>,
    /// Registry image → (RenderCpu texture generation, r3d alias).
    img: Vec<(u32, Option<TexId>)>,
    pub stats: R3dFrameStats,
    /// Per sink draw slot: the (model, object) and mesh it had last time (skips the map).
    slot_mesh: Vec<(u32, u16, MeshId)>,
    /// Recorded draws without an F2NM mesh (logged once each).
    missing: alloc::collections::BTreeSet<(u32, u16)>,
    frame_no: u32,
    dump_next: bool,
    /// Draw slot of the next recorded mesh (its index in the frame).
    slot: u32,
    /// This frame's counters while recording.
    rec: R3dFrameStats,
    /// Per registry entry: (image, its texture generation, crc) last mapped.
    synced: Vec<(u32, u32, u32, u16)>,
    /// Entries whose shown image can change (animated: 2+ frames).
    dyn_entries: Vec<u32>,
    /// Images shown by the other entries: (image, its N64 texture generation last seen, first
    /// and count of its entries in `stat_users`).
    stat_imgs: Vec<(u32, u32, u32, u32)>,
    stat_users: Vec<u32>,
    seen: alloc::collections::BTreeSet<u32>,
}

// SAFETY: the N64 runs one thread; the raw pointers inside R3d point into buffers it owns
// (the static `app::LEVEL` mutex only needs `Send` formally).
unsafe impl Send for R3dLevel {}

impl Default for R3dLevel {
    fn default() -> Self {
        Self::new()
    }
}

fn list_of(f: Frame) -> Option<List> {
    Some(match f {
        Frame::Normal => List::Normal,
        Frame::Wrap => List::Wrap,
        Frame::Xlu => List::Xlu,
        Frame::Additive => List::Additive,
        Frame::Late => List::Late,
        Frame::LateAdditive => List::LateAdditive,
        Frame::Overlay => List::Overlay,
        Frame::Glow => return None,
    })
}

/// `Renderer::build_segments` segment of a list.
fn segment_of(l: List) -> usize {
    l as usize + 1
}

impl R3dLevel {
    pub fn new() -> R3dLevel {
        let r = R3d::new();
        let base = r.mark();
        R3dLevel {
            r,
            base,
            meshes: BTreeMap::new(),
            img: Vec::new(),
            stats: R3dFrameStats::default(),
            slot_mesh: Vec::new(),
            missing: Default::default(),
            frame_no: 0,
            dump_next: false,
            slot: 0,
            rec: R3dFrameStats::default(),
            seen: Default::default(),
            synced: Vec::new(),
            dyn_entries: Vec::new(),
            stat_imgs: Vec::new(),
            stat_users: Vec::new(),
        }
    }

    /// Frees the level's meshes and texture aliases.
    pub fn unload(&mut self) {
        self.r.reset_to(self.base);
        self.meshes.clear();
        self.slot_mesh.clear();
        self.img.clear();
        self.missing.clear();
        self.synced.clear();
        self.dyn_entries.clear();
        self.stat_imgs.clear();
        self.stat_users.clear();
    }

    pub fn num_meshes(&self) -> usize {
        self.meshes.len()
    }

    /// Registers bank `bank`'s F2NM meshes. `models` is the level's model index (first bank
    /// wins, like `FindObject`): meshes of models shadowed by an earlier bank are skipped.
    pub fn add_bank(
        &mut self,
        bank: usize,
        f2nm: Vec<u8>,
        banks: &[ObjectBank],
        models: &BTreeMap<u32, (usize, usize)>,
    ) -> Result<usize, crate::r3d::LoadError> {
        let ids = self.r.load_file(f2nm)?;
        let mut n = 0;
        for id in ids {
            let (mi, obj) = f2_n64fmt::bank::split_id(self.r.mesh_packer_id(id));
            let Some(model) = banks.get(bank).and_then(|b| b.models.get(mi)) else { continue };
            if models.get(&model.id) == Some(&(bank, mi)) {
                self.meshes.insert((model.id, obj), id);
                n += 1;
            }
        }
        Ok(n)
    }

    /// The r3d texture of registry image `h` (aliased on first use, re-aliased when
    /// [`RenderCpu`] replaced the image's texture).
    #[inline(never)]
    fn tex_for(&mut self, cpu: &RenderCpu, h: u32) -> Option<TexId> {
        let (c, gen) = cpu.texture_of(h);
        if c == 0 {
            return None;
        }
        let i = h as usize;
        if self.img.len() <= i {
            self.img.resize(i + 1, (u32::MAX, None));
        }
        if self.img[i].0 != gen {
            self.img[i] = (gen, self.r.alias_texture(c));
        }
        self.img[i].1
    }

    /// Points every texture CRC at the image the registry currently shows for it.
    /// Only entries whose shown image (or that image's N64 texture) changed since the last
    /// call are looked at again (animated textures, procedural / render-to-texture images):
    /// per frame the animated entries and the generations of the images the others show
    /// (`TexEntry::current` changes only in `update_animating_textures`, for entries with
    /// 2+ frames); everything when the registry's size changed.
    pub fn sync_textures(&mut self, reg: &TextureRegistry, cpu: &RenderCpu) {
        if self.synced.len() != reg.entries.len() {
            self.sync_all(reg, cpu);
            return;
        }
        for k in 0..self.dyn_entries.len() {
            let i = self.dyn_entries[k] as usize;
            self.sync_entry(reg, cpu, i);
        }
        for k in 0..self.stat_imgs.len() {
            let (img, gen, first, n) = self.stat_imgs[k];
            let g = cpu.texture_of(img).1;
            if g != gen {
                self.stat_imgs[k].1 = g;
                for u in first..first + n {
                    let i = self.stat_users[u as usize] as usize;
                    self.sync_entry(reg, cpu, i);
                }
            }
        }
    }

    /// Every entry, and the lists of the per-frame checks.
    #[cold]
    #[inline(never)]
    fn sync_all(&mut self, reg: &TextureRegistry, cpu: &RenderCpu) {
        self.synced.clear();
        for e in &reg.entries {
            let cid = self.r.crc_id(e.crc);
            self.synced.push((u32::MAX, u32::MAX, e.crc, cid));
        }
        self.dyn_entries.clear();
        let mut stat: Vec<(u32, u32)> = Vec::new();
        for (i, e) in reg.entries.iter().enumerate() {
            self.sync_entry(reg, cpu, i);
            if e.frames.len() > 1 {
                self.dyn_entries.push(i as u32);
            } else if let Some(h) = e.current {
                stat.push((h.0, i as u32));
            }
        }
        stat.sort_unstable();
        self.stat_imgs.clear();
        self.stat_users.clear();
        for (img, i) in stat {
            match self.stat_imgs.last_mut() {
                Some(l) if l.0 == img => l.3 += 1,
                _ => self.stat_imgs.push((img, cpu.texture_of(img).1, self.stat_users.len() as u32, 1)),
            }
            self.stat_users.push(i);
        }
    }

    /// Re-points entry `i`'s CRC if its shown image or that image's texture changed.
    fn sync_entry(&mut self, reg: &TextureRegistry, cpu: &RenderCpu, i: usize) {
        let e = &reg.entries[i];
        let img = e.current.map_or(u32::MAX, |h| h.0);
        let gen = if img == u32::MAX { 0 } else { cpu.texture_of(img).1 };
        let (simg, sgen, scrc, cid) = self.synced[i];
        if simg == img && sgen == gen && scrc == e.crc {
            return;
        }
        let cid = if scrc == e.crc { cid } else { self.r.crc_id(e.crc) };
        self.synced[i] = (img, gen, e.crc, cid);
        let t = e.current.and_then(|h| self.tex_for(cpu, h.0));
        self.r.map_texture_id(cid, e.crc, t);
        rt::audio_poll_due();
    }

    /// Starts recording a frame's meshes with the renderer's view (call before
    /// `f2_draw::render::draw_loop` with the camera it will set: the draws arrive through
    /// [`R3dBackend`] while the loop runs).
    pub fn begin_queue(&mut self, r: &Renderer) {
        // F2N64_DUMP_FRAME=N (frame N of the level) or A-B (frames A..=B).
        if let Some(s) = option_env!("F2N64_DUMP_FRAME") {
            let (a, b) = s.split_once('-').unwrap_or((s, s));
            if let (Ok(a), Ok(b)) = (a.parse::<u32>(), b.parse::<u32>()) {
                self.frame_no += 1;
                self.dump_next = (a..=b).contains(&self.frame_no);
                if self.dump_next {
                    crate::log!("dump frame {}", self.frame_no);
                }
            }
        }
        self.r.begin_frame(&view_params(r));
        self.slot = 0;
        self.rec = R3dFrameStats::default();
    }

    /// Queues one recorded mesh draw ([`MeshBackend`] side of the draw loop).
    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_d")]
    fn record(&mut self, cpu: &RenderCpu, d: &MeshDraw, mats: &[[[f32; 4]; 4]]) {
        let slot = self.slot;
        self.slot += 1;
        if !crate::r3d::R3D_PROF {
            // Not timed: the draw loop's time includes recording (`r3d queue` is the batch).
            return self.record_inner(cpu, d, mats, slot as usize);
        }
        let t0 = rt::ticks();
        self.record_inner(cpu, d, mats, slot as usize);
        self.r.prof.record_ticks = self.r.prof.record_ticks.wrapping_add(rt::ticks().wrapping_sub(t0));
        self.r.prof.records += 1;
    }

    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_d")]
    fn record_inner(&mut self, cpu: &RenderCpu, d: &MeshDraw, mats: &[[[f32; 4]; 4]], slot: usize) {
        let Some(mut list) = list_of(d.frame) else { return };
        if d.no_clip && list == List::Normal {
            list = List::Background;
        }
        if let Some(skip) = option_env!("F2N64_SKIP_MODEL") {
            if skip.split(',').any(|m| alloc::format!("{:08x}", d.model) == m) {
                return;
            }
        }
        if let Some(only) = option_env!("F2N64_ONLY_MODEL") {
            if !only.split(',').any(|m| alloc::format!("{:08x}", d.model) == m) {
                return;
            }
        }
        // The draw order rarely changes between frames: try last frame's mesh of this slot.
        let mesh = match self.slot_mesh.get(slot) {
            Some(&(m, o, id)) if m == d.model && o == d.object => id,
            _ => match self.lookup_mesh(d, slot) {
                Some(m) => m,
                None => return,
            },
        };
        let override_tex = match d.override_tex {
            Some(h) => match self.tex_for(cpu, h.0) {
                Some(t) => Some(t),
                None => return, // PC: no texture, nothing drawn
            },
            None => None,
        };
        rt::audio_poll_due();

        // Recorded only: queued to the RSP lists in one batch after the draw loop
        // (`R3d::run_queue`).
        let p = crate::r3d::DrawParams { alpha_mul: d.alpha_mul, gouraud: d.gouraud, override_tex, no_clip: d.no_clip, object_fog: d.object_fog };
        self.r.queue(list, mesh, mats, &p, d.object == MESH_SKIN, slot as u32, d.stamp, d.modge, d.modge_frame);
        self.rec.draws += 1;
        if option_env!("F2N64_LOG_MODELS").is_some() || self.dump_next {
            self.log_draw(d, list, mesh);
        }
    }

    /// The mesh of a recorded draw (remembered for its slot), logging missing ones once.
    #[cold]
    #[inline(never)]
    fn lookup_mesh(&mut self, d: &MeshDraw, slot: usize) -> Option<MeshId> {
        let Some(&mesh) = self.meshes.get(&(d.model, d.object)) else {
            if self.missing.insert((d.model, d.object)) {
                crate::log!("r3d: no mesh for model {:08x} object {} (frame {:?})", d.model, d.object, d.frame);
            }
            return None;
        };
        if self.slot_mesh.len() <= slot {
            self.slot_mesh.resize(slot + 1, (0, 0, MeshId(u32::MAX)));
        }
        self.slot_mesh[slot] = (d.model, d.object, mesh);
        Some(mesh)
    }

    #[cold]
    #[inline(never)]
    fn log_draw(&mut self, d: &MeshDraw, list: List, mesh: MeshId) {
        if option_env!("F2N64_LOG_MODELS").is_some() && self.seen.insert(d.model) {
            crate::log!("r3d: model {:08x} first drawn ({:?}, {} tris)", d.model, list, self.r.mesh_tris(mesh));
        }
        if self.dump_next {
            crate::log!(
                "dump r3d {:?} model {:08x} obj {} alpha {} tris {} no_clip {}",
                list,
                d.model,
                d.object,
                d.alpha_mul,
                self.r.mesh_tris(mesh),
                d.no_clip
            );
        }
    }

    /// Draws the frame: CPU segments and the meshes recorded since [`begin_queue`]
    /// (Self::begin_queue), then presents.
    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_f")]
    pub fn draw(&mut self, r: &mut Renderer, cpu: &mut RenderCpu, clear: [f64; 4]) {
        cpu.build(r, clear);
        if self.dump_next {
            cpu.dump(&r.textures);
            self.dump_next = false;
        }
        let t0 = rt::ticks();
        let c = clear.map(|x| (x.clamp(0.0, 1.0) * 255.0) as u32);
        rt::begin_frame((c[0] << 24) | (c[1] << 16) | (c[2] << 8) | 0xff);
        cpu.draw_segment(0);
        cpu.zk = 4000.0 * view_params(r).z_scale() / 32767.0;
        let mut st = self.rec;
        if st.draws == 0 {
            self.r.discard_frame();
            for seg in 1..=7 {
                cpu.draw_segment(seg);
            }
        } else {
            let tq = rt::ticks();
            self.r.run_queue();
            st.queue_ticks = st.queue_ticks.wrapping_add(rt::ticks().wrapping_sub(tq));
            let tf = rt::ticks();
            let seg0 = cpu.stats.submit_ticks;
            self.r.flush(|l| {
                if l != List::Background {
                    cpu.draw_segment(segment_of(l));
                }
            });
            st.flush_ticks = rt::ticks().wrapping_sub(tf).wrapping_sub(cpu.stats.submit_ticks.wrapping_sub(seg0));
            st.queued_tris = self.r.queued_tris;
            st.reused = self.r.cache_hits;
            st.view_reused = self.r.view_hits;
            st.sent_tris = self.r.sent_tris;
            let c = self.r.stats();
            st.groups = c.groups;
            st.tex_loads = c.tex_loads;
        }
        cpu.draw_segment(8);
        rt::present();
        st.ticks = rt::ticks().wrapping_sub(t0);
        self.stats = st;
    }
}

/// r3d's view of the renderer's current view (camera, projection, far plane).
fn view_params(r: &Renderer) -> ViewParams {
    let v = &r.view;
    ViewParams { view: v.v_matrix, fov: v.fov, viewport: v.vp(), near: v.near_clip, far: v.far_clip, bilinear: r.options.linear_filter, guard: 6, fog: None }
}

/// The draw loop's [`MeshBackend`] on the N64: each recorded mesh goes straight to
/// [`R3dLevel`] (matrix conversion and culling while the matrices are in the D-cache).
/// Installed in the renderer's mesh sink for the duration of one draw loop.
pub struct R3dBackend {
    lvl: *mut R3dLevel,
    cpu: *const RenderCpu,
}

// SAFETY: one thread; see `R3dBackend::new`.
unsafe impl Send for R3dBackend {}

impl R3dBackend {
    /// SAFETY: `lvl` and `cpu` must stay valid, and not be accessed otherwise, while the
    /// backend is installed (`App::draw_inner` removes it right after the draw loop).
    pub unsafe fn new(lvl: *mut R3dLevel, cpu: *const RenderCpu) -> R3dBackend {
        R3dBackend { lvl, cpu }
    }
}

impl MeshBackend for R3dBackend {
    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_d")]
    fn mesh(&mut self, _view: &View, d: &MeshDraw, mats: &[[[f32; 4]; 4]]) {
        // SAFETY: see `new`.
        unsafe { (*self.lvl).record(&*self.cpu, d, mats) }
    }
}
