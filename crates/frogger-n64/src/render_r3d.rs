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
use f2_engine::{Frame, MeshBackend, MeshDraw, ObjDraw, ObjSphere, Renderer, TextureRegistry, View, MESH_SKIN};
use f2_formats::objbank::ObjectBank;

/// A sliding actor ([`R3dLevel::slide_textures`]): its slot, its flags and the `slides` entry
/// of its model (both fixed since its creation).
#[derive(Clone, Copy, Debug)]
struct SlideActor {
    slot: u32,
    flags: u16,
    entry: u16,
}

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

/// Testing: the next frame's draws are logged as with `F2N64_DUMP_FRAME` (set when an
/// `F2N64_REPLAY_HOLD` hold starts).
pub static DUMP_REQUEST: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub struct R3dLevel {
    pub r: R3d,
    base: R3dMark,
    /// `(Model::id, object)` → mesh of the loaded level.
    meshes: BTreeMap<(u32, u16), MeshId>,
    /// Registry image → (RenderCpu texture generation, r3d alias).
    img: Vec<(u32, Option<TexId>)>,
    pub stats: R3dFrameStats,
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
    /// `CopyActorList`'s texture sliding: per model that slides, (model id, total slide in
    /// `faceTC` units mod [`r3d::SLIDE_PERIOD`](crate::r3d::SLIDE_PERIOD), its draws'
    /// texture offset [`slide_st`](crate::r3d::slide_st)). A handful per level at most.
    slides: Vec<(u32, i32, i16)>,
    /// The sliding actors, how many actor slots were looked at, and the (slot count,
    /// `actList` count) they were updated for.
    slide_actors: Vec<SlideActor>,
    slide_scanned: usize,
    slide_sig: (usize, i32),
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
            slides: Vec::new(),
            slide_actors: Vec::new(),
            slide_scanned: 0,
            slide_sig: (0, 0),
        }
    }

    /// Frees the level's meshes and texture aliases.
    pub fn unload(&mut self) {
        self.r.reset_to(self.base);
        self.meshes.clear();
        self.img.clear();
        self.missing.clear();
        self.synced.clear();
        self.dyn_entries.clear();
        self.stat_imgs.clear();
        self.stat_users.clear();
        self.reset_slides();
    }

    /// One game frame of `CopyActorList`'s texture sliding (PC: `f2_draw::render::
    /// slide_object_textures`, which edits the meshes' `faceTC`; the F2NM meshes are static,
    /// so each sliding model's draws offset their texture coordinates instead). Call once per
    /// simulation tick, after it. `scene` names the models in the log.
    ///
    /// No actor is read in most ticks: walking `actList` costs ~500 us per tick here (cache
    /// misses on every actor), so the sliding actors are kept in a list. Within a level actor
    /// slots are only appended (a freed one stays empty) and every filled slot is in
    /// `actList`, so the actor set changed only when the slot count or `actList`'s count did:
    /// then the new slots are scanned and the freed actors dropped. An actor's slide flags and
    /// model are fixed once it is created (flags from its name; `ChangeModel` only swaps the
    /// frogs' models, which do not slide). The sum per model does not depend on `actList`
    /// order. [`reset_slides`](Self::reset_slides) at every level load.
    pub fn slide_textures(&mut self, game: &f2_game::Game, scene: Option<&f2_draw::render::LevelScene>) {
        let slots = &game.actor2.actors.slots;
        let sig = (slots.len(), game.actor2.act_list.count);
        if sig != self.slide_sig {
            self.slide_sig = sig;
            if slots.len() < self.slide_scanned {
                // A new actor list (a level load not seen by `reset_slides`).
                self.slide_scanned = 0;
                self.slide_actors.clear();
            }
            self.slide_actors.retain(|sa| slots[sa.slot as usize].is_some());
            for i in self.slide_scanned..slots.len() {
                let Some(a) = slots[i].as_ref().filter(|a| f2_draw::render::actor_slides_texture(a)) else { continue };
                let Some(model) = a.actor.actual_actor.object_controller else { continue };
                let entry = self.slide_entry(model, a.flags, game.game_speed, scene);
                self.slide_actors.push(SlideActor { slot: i as u32, flags: a.flags, entry });
            }
            self.slide_scanned = slots.len();
        }
        let gs = game.game_speed;
        if gs == 0 {
            return;
        }
        for sa in &self.slide_actors {
            let e = &mut self.slides[sa.entry as usize];
            e.1 = e.1.wrapping_add(f2_draw::render::texture_slide_speed(sa.flags, gs));
        }
        for e in &mut self.slides {
            e.1 = e.1.rem_euclid(crate::r3d::SLIDE_PERIOD);
            e.2 = crate::r3d::slide_st(e.1);
        }
    }

    /// The `slides` entry of `model` (made and logged on first use).
    #[cold]
    #[inline(never)]
    fn slide_entry(&mut self, model: u32, flags: u16, gs: i32, scene: Option<&f2_draw::render::LevelScene>) -> u16 {
        if let Some(i) = self.slides.iter().position(|e| e.0 == model) {
            return i as u16;
        }
        let name = scene.and_then(|s| s.models.get(&model).map(|&(b, m)| s.banks[b].models[m].name()));
        let speed = f2_draw::render::texture_slide_speed(flags, gs);
        crate::log!("r3d: model {model:08x} ({}) slides its texture (speed {speed})", name.unwrap_or("?"));
        self.slides.push((model, 0, 0));
        (self.slides.len() - 1) as u16
    }

    /// Forgets the slides and the sliding actors (a level load: new actors, fresh meshes).
    pub fn reset_slides(&mut self) {
        self.slides.clear();
        self.slide_actors.clear();
        self.slide_scanned = 0;
        self.slide_sig = (0, 0);
    }

    /// The texture offset of `model`'s draws if its texture slides.
    #[inline(always)]
    fn slide_of(&self, model: u32) -> Option<i16> {
        if self.slides.is_empty() {
            return None;
        }
        self.slides.iter().find(|e| e.0 == model).map(|e| e.2)
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
        if option_env!("F2N64_REPLAY_HOLD").is_some() && DUMP_REQUEST.swap(false, core::sync::atomic::Ordering::Relaxed) {
            self.dump_next = true;
            crate::log!("dump frame (requested)");
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

    /// Inside the game's draw loop: only records the draw (mesh lookup, textures and the
    /// debug filters follow in [`resolve_queue`](Self::resolve_queue), after the loop).
    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_d")]
    fn record_inner(&mut self, cpu: &RenderCpu, d: &MeshDraw, mats: &[[[f32; 4]; 4]], slot: usize) {
        let Some(mut list) = list_of(d.frame) else { return };
        if d.no_clip && list == List::Normal {
            list = List::Background;
        }
        // Every 16th draw (the C call misses the I-cache; 16 draws take ~1 ms).
        if slot % 16 == 15 {
            rt::audio_poll_due();
        }
        let flags = (d.no_clip, d.object_fog, d.object == MESH_SKIN);
        let ovr = match d.override_tex {
            None => None,
            Some(h) => match self.tex_for(cpu, h.0) {
                Some(t) => Some(t),
                None => return, // PC: no texture, nothing drawn
            },
        };
        let slide = self.slide_of(d.model);
        self.r.queue_raw(list, d.model, d.object, mats, d.alpha_mul, d.gouraud, ovr, flags, slot as u32, d.stamp, d.modge, d.modge_frame, slide);
    }

    /// [`record`](Self::record) of every draw of a rigid actor ([`MeshBackend::actor`]): the
    /// same draws, slots and queue entries, with the per-actor parts (override texture,
    /// flags) once.
    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_d")]
    fn record_actor(&mut self, cpu: &RenderCpu, base: &MeshDraw, objs: &[ObjDraw], mats: &[[[f32; 4]; 4]]) {
        let t0 = if crate::r3d::R3D_PROF { rt::ticks() } else { 0 };
        let first = self.slot;
        self.slot += objs.len() as u32;
        let ovr = match base.override_tex {
            None => None,
            Some(h) => match self.tex_for(cpu, h.0) {
                Some(t) => Some(t),
                None => {
                    // PC: no texture, nothing drawn (the slots stay used, as per draw).
                    for slot in first..self.slot {
                        if slot % 16 == 15 {
                            rt::audio_poll_due();
                        }
                    }
                    return;
                }
            },
        };
        let flags = (base.no_clip, base.object_fog, false);
        let slide = self.slide_of(base.model);
        for (k, o) in objs.iter().enumerate() {
            let slot = first + k as u32;
            let Some(mut list) = list_of(o.frame) else { continue };
            if base.no_clip && list == List::Normal {
                list = List::Background;
            }
            // Every 16th draw (the C call misses the I-cache; 16 draws take ~1 ms).
            if slot % 16 == 15 {
                rt::audio_poll_due();
            }
            let i = o.object as usize;
            self.r.queue_raw(list, base.model, o.object, &mats[i..i + 1], base.alpha_mul, base.gouraud, ovr, flags, slot, base.stamp, o.modge, base.modge_frame, slide);
        }
        if crate::r3d::R3D_PROF {
            self.r.prof.record_ticks = self.r.prof.record_ticks.wrapping_add(rt::ticks().wrapping_sub(t0));
            self.r.prof.records += objs.len() as u32;
        }
    }

    /// `n` draws of an actor culled before recording ([`MeshBackend::skip`]): their slots stay
    /// reserved, so the following draws keep last frame's slots (and r3d's per-slot caches).
    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_d")]
    fn skip_slots(&mut self, n: u32) {
        let first = self.slot;
        self.slot += n;
        // The audio poll of every 16th slot (see `record_inner`).
        if self.slot >> 4 != first >> 4 {
            rt::audio_poll_due();
        }
    }

    /// The bounding sphere of the mesh of (model, object) in model units
    /// ([`MeshBackend::object_sphere`]).
    fn object_sphere(&self, model: u32, object: u16) -> ObjSphere {
        match self.meshes.get(&(model, object)) {
            None => ObjSphere::NoMesh,
            Some(&m) => match self.r.mesh_sphere(m) {
                Some((c, r)) => ObjSphere::Sphere(c, r),
                None => ObjSphere::Unknown,
            },
        }
    }

    /// The mesh of a recorded draw ([`R3d::run_queue_with`]'s resolver: the slot memo had
    /// another model/object), applying the debug filters and logging missing meshes once.
    #[cold]
    #[inline(never)]
    fn lookup_mesh(meshes: &BTreeMap<(u32, u16), MeshId>, missing: &mut alloc::collections::BTreeSet<(u32, u16)>, d: &crate::r3d::RawDraw) -> Option<MeshId> {
        let skip = |list: &str| list.split(',').any(|m| alloc::format!("{:08x}", d.model) == m);
        if option_env!("F2N64_SKIP_MODEL").is_some_and(skip) || option_env!("F2N64_ONLY_MODEL").is_some_and(|l| !skip(l)) {
            return None;
        }
        let Some(&mesh) = meshes.get(&(d.model, d.object)) else {
            if missing.insert((d.model, d.object)) {
                crate::log!("r3d: no mesh for model {:08x} object {} (list {:?})", d.model, d.object, d.list);
            }
            return None;
        };
        Some(mesh)
    }

    #[cold]
    #[inline(never)]
    fn log_draw(seen: &mut alloc::collections::BTreeSet<u32>, dump: bool, d: &crate::r3d::RawDraw, mesh: MeshId) {
        if option_env!("F2N64_LOG_MODELS").is_some() && seen.insert(d.model) {
            crate::log!("r3d: model {:08x} first drawn ({:?}, mesh {})", d.model, d.list, mesh.0);
        }
        if dump {
            crate::log!(
                "dump r3d {:?} model {:08x} obj {} alpha {} mesh {} no_clip {}",
                d.list,
                d.model,
                d.object,
                d.alpha,
                mesh.0,
                d.no_clip
            );
        }
    }

    /// Draws the frame: CPU segments and the meshes recorded since [`begin_queue`]
    /// (Self::begin_queue), then presents.
    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_f")]
    pub fn draw(&mut self, r: &mut Renderer, cpu: &mut RenderCpu, clear: [f64; 4]) {
        // Scene depth scale first: `build` bakes it into the sprite rectangles.
        cpu.zk = 4000.0 * view_params(r).z_scale() / 32767.0;
        cpu.build(r, clear);
        if self.dump_next {
            cpu.dump(&r.textures);
        }
        let t0 = rt::ticks();
        let c = clear.map(|x| (x.clamp(0.0, 1.0) * 255.0) as u32);
        rt::begin_frame((c[0] << 24) | (c[1] << 16) | (c[2] << 8) | 0xff);
        cpu.draw_segment(0);
        let mut st = self.rec;
        if self.r.queued() == 0 {
            self.r.discard_frame();
            for seg in 1..=7 {
                cpu.draw_segment(seg);
            }
        } else {
            let tq = rt::ticks();
            let log = option_env!("F2N64_LOG_MODELS").is_some() || self.dump_next;
            self.r.resolve_all(log);
            let R3dLevel { r: rr, meshes, missing, seen, dump_next, .. } = self;
            rr.run_queue_with(&mut |d| {
                let m = Self::lookup_mesh(meshes, missing, d);
                if let (true, Some(m)) = (log, m) {
                    Self::log_draw(seen, *dump_next, d, m);
                }
                m
            });
            self.dump_next = false;
            st.queue_ticks = st.queue_ticks.wrapping_add(rt::ticks().wrapping_sub(tq));
            st.draws = self.r.drawn;
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

    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_d")]
    fn actor(&mut self, _view: &View, base: &MeshDraw, objs: &[ObjDraw], mats: &[[[f32; 4]; 4]]) {
        // SAFETY: see `new`.
        unsafe { (*self.lvl).record_actor(&*self.cpu, base, objs, mats) }
    }

    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_d")]
    fn sphere_culled(&self, c: &[f32; 3], r: f32) -> bool {
        // SAFETY: see `new`.
        unsafe { (*self.lvl).r.sphere_culled(c, r) }
    }

    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_d")]
    fn skip(&mut self, n: u32) {
        // SAFETY: see `new`.
        unsafe { (*self.lvl).skip_slots(n) }
    }

    fn object_sphere(&self, model: u32, object: u16) -> ObjSphere {
        // SAFETY: see `new`.
        unsafe { (*self.lvl).object_sphere(model, object) }
    }

    fn cull_check(&self) -> bool {
        option_env!("F2N64_CULL_CHECK").is_some()
    }

    fn mark_culled(&mut self, on: bool) {
        // SAFETY: see `new`.
        unsafe { (*self.lvl).r.extra_lf = if on { crate::r3d::Q_CHECK_CULLED } else { 0 } }
    }
}
