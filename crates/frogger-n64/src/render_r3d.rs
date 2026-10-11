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
use f2_engine::{Frame, MeshSink, Renderer, TextureRegistry, MESH_SKIN};
use f2_formats::objbank::ObjectBank;

/// Per-frame counters.
#[derive(Clone, Copy, Debug, Default)]
pub struct R3dFrameStats {
    pub draws: u32,
    pub queued_tris: u32,
    pub sent_tris: u32,
    /// C0 ticks of the r3d CPU side (draw queueing + flush incl. the CPU segments).
    pub ticks: u32,
}

pub struct R3dLevel {
    pub r: R3d,
    base: R3dMark,
    /// `(Model::id, object)` → mesh of the loaded level.
    meshes: BTreeMap<(u32, u16), MeshId>,
    /// Registry image → (RenderCpu texture generation, r3d alias).
    img: Vec<(u32, Option<TexId>)>,
    pub stats: R3dFrameStats,
    bones: Vec<[[f32; 4]; 4]>,
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
        R3dLevel { r, base, meshes: BTreeMap::new(), img: Vec::new(), stats: R3dFrameStats::default(), bones: Vec::new() }
    }

    /// Frees the level's meshes and texture aliases.
    pub fn unload(&mut self) {
        self.r.reset_to(self.base);
        self.meshes.clear();
        self.img.clear();
    }

    pub fn num_meshes(&self) -> usize {
        self.meshes.len()
    }

    /// Registers bank `bank`'s F2NM meshes. `models` is the level's model index (first bank
    /// wins, like `FindObject`): meshes of models shadowed by an earlier bank are skipped.
    pub fn add_bank(&mut self, bank: usize, f2nm: Vec<u8>, banks: &[ObjectBank], models: &BTreeMap<u32, (usize, usize)>) -> Result<usize, crate::r3d::LoadError> {
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
    pub fn sync_textures(&mut self, reg: &TextureRegistry, cpu: &RenderCpu) {
        for e in &reg.entries {
            let t = e.current.and_then(|h| self.tex_for(cpu, h.0));
            self.r.map_texture(e.crc, t);
        }
    }

    /// Draws the frame: CPU segments and the recorded meshes, then presents.
    pub fn draw(&mut self, r: &mut Renderer, cpu: &mut RenderCpu, clear: [f64; 4]) {
        cpu.build(r, clear);
        let t0 = rt::ticks();
        let c = clear.map(|x| (x.clamp(0.0, 1.0) * 255.0) as u32);
        rt::begin_frame((c[0] << 24) | (c[1] << 16) | (c[2] << 8) | 0xff);
        cpu.draw_segment(0);
        let sink = r.mesh_sink.take().unwrap_or_default();
        let v = &r.view;
        let view = ViewParams {
            view: v.v_matrix,
            fov: v.fov,
            viewport: v.vp(),
            near: v.near_clip,
            far: v.far_clip,
            bilinear: r.options.linear_filter,
            guard: 6,
            fog: None,
        };
        cpu.zk = 4000.0 * view.z_scale() / 32767.0;
        let mut st = R3dFrameStats::default();
        if sink.draws.is_empty() {
            for seg in 1..=7 {
                cpu.draw_segment(seg);
            }
        } else {
            self.r.begin_frame(&view);
            self.queue(&sink, cpu, &mut st);
            self.r.flush(|l| cpu.draw_segment(segment_of(l)));
            st.queued_tris = self.r.queued_tris;
            st.sent_tris = self.r.sent_tris;
        }
        r.mesh_sink = Some(sink);
        cpu.draw_segment(8);
        rt::present();
        st.ticks = rt::ticks().wrapping_sub(t0);
        self.stats = st;
    }

    fn queue(&mut self, sink: &MeshSink, cpu: &RenderCpu, st: &mut R3dFrameStats) {
        for d in &sink.draws {
            let Some(list) = list_of(d.frame) else { continue };
            let Some(&mesh) = self.meshes.get(&(d.model, d.object)) else { continue };
            let override_tex = match d.override_tex {
                Some(h) => match self.tex_for(cpu, h.0) {
                    Some(t) => Some(t),
                    None => continue, // PC: no texture, nothing drawn
                },
                None => None,
            };
            let p = crate::r3d::DrawParams { alpha_mul: d.alpha_mul, gouraud: d.gouraud, override_tex, no_clip: d.no_clip, object_fog: d.object_fog };
            let mats = &sink.mats[d.mat_first as usize..(d.mat_first + d.mat_count) as usize];
            if d.object == MESH_SKIN {
                self.bones.clear();
                self.bones.extend_from_slice(mats);
                self.r.draw_skinned(list, mesh, &self.bones, &p);
            } else if let Some(m) = mats.first() {
                self.r.draw(list, mesh, m, &p);
            }
            st.draws += 1;
        }
    }
}
