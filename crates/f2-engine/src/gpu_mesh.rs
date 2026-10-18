//! GPU mesh renderer (enhanced, [`RenderOptions::gpu_meshes`](crate::RenderOptions)).
//!
//! The original projects every vertex on the CPU and hands the GPU screen-space triangles
//! (`scene.rs`, the MDX pipeline). Here the opaque meshes (`Normal` / `Wrap` frames, no
//! MODGE/WAVE animation) stay on the GPU as model data instead: each mesh is expanded once
//! into a vertex buffer (position, bone, uv, normal, colour; faces grouped by texture) and
//! drawn every frame with its object matrices (all bones for a skin) and the camera. That keeps
//! the whole scene's geometry available to the GPU (lighting, shadows, ray tracing later) and
//! leaves clipping and culling to the hardware.
//!
//! The vertex shader uses the game's projection (`PCPrepareObject` with the exact depth,
//! [`View::exact_depth`](crate::View), as a matrix: [`crate::upscale::projection`]) and the same
//! linear depth as the CPU path (`vz × 0.00025`), so meshes, CPU-projected sprites, shadows and
//! effects keep occluding each other as before. Translucent and wobbling meshes stay on the CPU
//! path.

use crate::math::Mat4;
use crate::prelude::*;
use crate::scene::{Frame, GpuObj, View, MESH_SKIN, SZ_SCALE, UV_SCALE};
use crate::textures::{TexHandle, TextureRegistry};
use std::collections::{HashMap, HashSet};

/// One expanded mesh corner (64 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Vert {
    pos: [f32; 3],
    /// Matrix of the draw's bone list (a skin's object index; 0 for rigid meshes).
    bone: u32,
    uv: [f32; 2],
    _pad: [f32; 2],
    normal: [f32; 4],
    color: [f32; 4],
}

/// Faces of one texture: `count` vertices from `first`.
#[derive(Clone, Copy, Debug)]
struct Group {
    crc: u32,
    first: u32,
    count: u32,
}

/// Mesh key: model id and pre-order object index (or [`MESH_SKIN`]).
type Key = (u32, u16);

/// Per-draw record in the storage buffer (32 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Rec {
    /// First matrix (this frame's and last frame's lists are parallel).
    mat: u32,
    /// Bit 0: no far clip (sky); bit 1: Gouraud override; bit 2: landscape (fog per vertex).
    flags: u32,
    _pad: [u32; 2],
    gouraud: [f32; 4],
}

#[derive(Clone, Copy, Debug)]
struct Draw {
    key: Key,
    rec: u32,
    wrap: bool,
    landscape: bool,
    override_tex: Option<TexHandle>,
}

/// One frame's GPU mesh draws (filled while the frame is built).
#[derive(Default)]
pub(crate) struct Sink {
    draws: Vec<Draw>,
    recs: Vec<Rec>,
    mats: Vec<[[f32; 4]; 4]>,
    prev: Vec<[[f32; 4]; 4]>,
    /// Meshes seen for the first time, to upload.
    pending: Vec<(Key, Vec<Vert>, Vec<Group>)>,
    known: HashSet<Key>,
}

impl Sink {
    pub fn clear(&mut self) {
        self.draws.clear();
        self.recs.clear();
        self.mats.clear();
        self.prev.clear();
    }

    /// Takes `o` (see [`GpuObj`]); false = the CPU draws it.
    pub fn offer(&mut self, o: &GpuObj, no_clip: bool) -> bool {
        let key = (o.model.id, o.object);
        if !self.known.contains(&key) {
            let (verts, groups) = expand(o);
            self.known.insert(key);
            self.pending.push((key, verts, groups));
        }
        let mat = self.mats.len() as u32;
        self.mats.extend(o.mats.iter().map(mat4));
        // Bones without last-frame matrices (a new pose): this frame's.
        self.prev.extend((0..o.mats.len()).map(|i| mat4(o.prev.get(i).unwrap_or(&o.mats[i]))));
        let flags = no_clip as u32 | (o.gouraud.is_some() as u32) << 1 | (o.landscape as u32) << 2;
        let g = o.gouraud.unwrap_or([1.0; 3]);
        self.recs.push(Rec { mat, flags, _pad: [0; 2], gouraud: [g[0], g[1], g[2], 1.0] });
        self.draws.push(Draw { key, rec: self.recs.len() as u32 - 1, wrap: o.frame == Frame::Wrap, landscape: o.landscape, override_tex: o.override_tex });
        true
    }
}

fn mat4(m: &Mat4) -> [[f32; 4]; 4] {
    *m
}

/// The corners of `o.mesh` grouped by texture, as `PCRenderObject` would draw them (skins:
/// the bone moving each vertex; faces with an unmoved vertex left out like the CPU path).
fn expand(o: &GpuObj) -> (Vec<Vert>, Vec<Group>) {
    let mesh = o.mesh;
    let nv = mesh.vertices.len();
    let mut bone = vec![if o.object == MESH_SKIN { u32::MAX } else { 0 }; nv];
    if o.object == MESH_SKIN {
        crate::anim::for_each_object(o.model, |i, obj| {
            for &j in &obj.effected_verts {
                if let Some(b) = bone.get_mut(j as usize) {
                    *b = i as u32;
                }
            }
        });
    }
    let mut order: Vec<usize> = (0..mesh.faces.len()).collect();
    order.sort_by_key(|&fi| mesh.textures.get(fi).copied().unwrap_or(0));
    let (mut verts, mut groups) = (Vec::with_capacity(mesh.faces.len() * 3), Vec::<Group>::new());
    for fi in order {
        let face = mesh.faces[fi];
        let idx = face.map(|v| v as usize);
        if idx.iter().any(|&i| i >= nv || bone[i] == u32::MAX) {
            continue;
        }
        let crc = mesh.textures.get(fi).copied().unwrap_or(0);
        if groups.last().is_none_or(|g| g.crc != crc) {
            groups.push(Group { crc, first: verts.len() as u32, count: 0 });
        }
        for (k, &i) in idx.iter().enumerate() {
            let uv = mesh.face_uv.get(fi * 3 + k).copied().unwrap_or([0, 0]);
            let n = mesh.normals.get(i).copied().unwrap_or([0.0; 3]);
            verts.push(Vert {
                pos: mesh.vertices[i],
                bone: bone[i],
                uv: [uv[0] as f32 * UV_SCALE, uv[1] as f32 * UV_SCALE],
                _pad: [0.0; 2],
                normal: [n[0], n[1], n[2], 0.0],
                color: mesh.colors.get(fi * 3 + k).copied().unwrap_or([1.0; 4]),
            });
        }
        groups.last_mut().unwrap().count += 3;
    }
    (verts, groups)
}

const SHADER: &str = r#"
struct Cam {
    v: mat4x4<f32>,      // rows of the game's view matrix (row vectors)
    pv: mat4x4<f32>,     // last frame's
    proj: vec4<f32>,     // a, b, ox, oy (crate::upscale::projection)
    pproj: vec4<f32>,
    size: vec4<f32>,     // width, height, jitter x, jitter y
    fog: vec4<f32>,      // on, start, end, far clip
    fog_color: vec4<f32>,
    depth: vec4<f32>,    // near (upscaler depth), alpha reference, -, -
};
struct Rec { mat: u32, flags: u32, pad0: u32, pad1: u32, gouraud: vec4<f32> };
@group(0) @binding(0) var<uniform> cam: Cam;
@group(1) @binding(0) var t: texture_2d<f32>;
@group(1) @binding(1) var s: sampler;
@group(2) @binding(0) var<storage, read> recs: array<Rec>;
@group(2) @binding(1) var<storage, read> mats: array<mat4x4<f32>>;
@group(2) @binding(2) var<storage, read> prevs: array<mat4x4<f32>>;

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) bone: u32,
    @location(2) uv: vec2<f32>,
    @location(3) normal: vec4<f32>,
    @location(4) color: vec4<f32>,
};
struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) fog: f32,
    @location(3) vz: f32,
    @location(4) @interpolate(flat) flags: u32,
    @location(5) cur: vec4<f32>,
    @location(6) prev: vec4<f32>,
};

// `mtx_xfm`: row-vector affine transform.
fn xfm(m: mat4x4<f32>, p: vec3<f32>) -> vec3<f32> {
    return p.x * m[0].xyz + p.y * m[1].xyz + p.z * m[2].xyz + m[3].xyz;
}

// View-space point to clip space without jitter (camera space X = -x, Y = y, Z = z - 10).
fn clip(pr: vec4<f32>, p: vec3<f32>) -> vec4<f32> {
    let z = p.z - 10.0;
    return vec4<f32>(-pr.x * p.x + pr.z * z, pr.y * p.y + pr.w * z, 0.0, z);
}

@vertex fn vs(i: VIn, @builtin(instance_index) inst: u32) -> VOut {
    let r = recs[inst];
    let world = xfm(mats[r.mat + i.bone], i.pos);
    let vp = xfm(cam.v, world);
    let c = clip(cam.proj, vp);
    var o: VOut;
    // The CPU path's linear depth, so CPU and GPU geometry occlude each other as before.
    let zl = clamp(vp.z * 0.00025, 0.0, 1.0);
    let j = vec2<f32>(cam.size.z * 2.0 / cam.size.x, -cam.size.w * 2.0 / cam.size.y);
    o.pos = vec4<f32>(c.xy + j * c.w, zl * c.w, c.w);
    var col = i.color;
    if ((r.flags & 2u) != 0u) { col = vec4<f32>(r.gouraud.rgb, col.a); }
    o.color = clamp(col, vec4<f32>(0.0), vec4<f32>(1.0));
    o.uv = i.uv;
    o.fog = 1.0;
    if (cam.fog.x > 0.5 && (r.flags & 1u) == 0u) {
        o.fog = clamp(1.0 - (vp.z - cam.fog.y) / (cam.fog.z - cam.fog.y), 0.0, 1.0);
    }
    o.vz = vp.z;
    o.flags = r.flags;
    o.cur = c;
    o.prev = clip(cam.pproj, xfm(cam.pv, xfm(prevs[r.mat + i.bone], i.pos)));
    return o;
}

fn shade(i: VOut) -> vec4<f32> {
    // The far plane (`PCPrepareObject` drops faces past it; here per pixel, no popping).
    if ((i.flags & 1u) == 0u && i.vz - 10.0 >= cam.fog.w) { discard; }
    var c = textureSample(t, s, i.uv) * i.color;
    if (floor(c.a * 255.0 + 0.5) <= cam.depth.y) { discard; }
    if (cam.fog.x > 0.5) { c = vec4<f32>(mix(cam.fog_color.rgb, c.rgb, i.fog), c.a); }
    return c;
}

@fragment fn fs(i: VOut) -> @location(0) vec4<f32> {
    return shade(i);
}

struct FOut { @location(0) c: vec4<f32>, @location(1) mv: vec2<f32>, @location(2) d: f32 };
@fragment fn fs_mrt(i: VOut) -> FOut {
    var o: FOut;
    o.c = shade(i);
    let half = vec2<f32>(cam.size.x * 0.5, -cam.size.y * 0.5);
    o.mv = (i.prev.xy / i.prev.w - i.cur.xy / i.cur.w) * half;
    o.d = clamp(cam.depth.x / (i.vz - 10.0), 0.0, 1.0);
    return o;
}
"#;

/// Size of `Cam`.
const CAM_SIZE: u64 = 64 * 2 + 16 * 6;

/// The GPU side: uploaded meshes, per-frame buffers, pipelines.
pub(crate) struct Gpu {
    meshes: HashMap<Key, (wgpu::Buffer, Vec<Group>)>,
    shader: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    store_layout: wgpu::BindGroupLayout,
    cam: wgpu::Buffer,
    cam_bg: wgpu::BindGroup,
    /// recs, mats, prev, and the bind group over them.
    store: Option<(wgpu::Buffer, wgpu::Buffer, wgpu::Buffer, wgpu::BindGroup)>,
    pipelines: HashMap<(wgpu::TextureFormat, u32, bool), wgpu::RenderPipeline>,
    /// This frame's draw list: (record, mesh key, vertex range, texture, wrap).
    list: Vec<(u32, Key, core::ops::Range<u32>, TexHandle, bool)>,
}

impl Gpu {
    pub fn new(device: &wgpu::Device, tex_layout: &wgpu::BindGroupLayout) -> Gpu {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("gpu meshes"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let cam_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gpu mesh camera"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let storage = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let store_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("gpu mesh draws"), entries: &[storage(0), storage(1), storage(2)] });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gpu meshes"),
            bind_group_layouts: &[Some(&cam_layout), Some(tex_layout), Some(&store_layout)],
            immediate_size: 0,
        });
        let cam = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu mesh camera"),
            size: CAM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cam_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gpu mesh camera"),
            layout: &cam_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: cam.as_entire_binding() }],
        });
        Gpu { meshes: HashMap::new(), shader, layout, store_layout, cam, cam_bg, store: None, pipelines: HashMap::new(), list: Vec::new() }
    }

    /// Uploads new meshes and this frame's records, matrices and camera; resolves the
    /// textures of every draw. Returns the (texture, wrap) pairs whose bind groups the
    /// renderer must have before [`draw`](Self::draw).
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sink: &mut Sink,
        textures: &TextureRegistry,
        white: TexHandle,
        view: &View,
        prev: Option<&View>,
        jitter: [f32; 2],
        fog: &crate::scene::Fog,
    ) -> Vec<(TexHandle, bool)> {
        use wgpu::util::DeviceExt;
        for (key, verts, groups) in sink.pending.drain(..) {
            if verts.is_empty() {
                self.meshes.insert(
                    key,
                    (
                        device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 64, usage: wgpu::BufferUsages::VERTEX, mapped_at_creation: false }),
                        Vec::new(),
                    ),
                );
                continue;
            }
            let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("gpu mesh"),
                contents: bytemuck::cast_slice(&verts),
                usage: wgpu::BufferUsages::VERTEX,
            });
            self.meshes.insert(key, (buf, groups));
        }
        self.list.clear();
        if sink.draws.is_empty() {
            return Vec::new();
        }
        // Records and matrices: storage buffers, grown as needed.
        let need = |n: usize, each: usize| ((n.max(1) * each) as u64).next_power_of_two().max(256);
        let (rn, mn) = (need(sink.recs.len(), 32), need(sink.mats.len(), 64));
        if self.store.as_ref().is_none_or(|(r, m, _, _)| r.size() < rn || m.size() < mn) {
            let mk = |label, size| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            };
            let (r, m, p) = (mk("gpu mesh records", rn), mk("gpu mesh matrices", mn), mk("gpu mesh previous matrices", mn));
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("gpu mesh draws"),
                layout: &self.store_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: r.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: m.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: p.as_entire_binding() },
                ],
            });
            self.store = Some((r, m, p, bg));
        }
        let (r, m, p, _) = self.store.as_ref().unwrap();
        queue.write_buffer(r, 0, bytemuck::cast_slice(&sink.recs));
        queue.write_buffer(m, 0, bytemuck::cast_slice(&sink.mats));
        queue.write_buffer(p, 0, bytemuck::cast_slice(&sink.prev));
        // Camera.
        let pv = prev.unwrap_or(view);
        let pr = |v: &View| {
            let p = crate::upscale::projection(v);
            [p[0][0], p[1][1], p[2][0], p[2][1]]
        };
        let mut cam: Vec<f32> = Vec::with_capacity((CAM_SIZE / 4) as usize);
        cam.extend(view.v_matrix.iter().flatten());
        cam.extend(pv.v_matrix.iter().flatten());
        cam.extend(pr(view));
        cam.extend(pr(pv));
        cam.extend([view.width, view.height, jitter[0], jitter[1]]);
        cam.extend([if fog.enabled { 1.0 } else { 0.0 }, fog.start, fog.end, view.far_clip]);
        cam.extend([fog.color[0], fog.color[1], fog.color[2], 1.0]);
        // Normal / Wrap passes: `tightAlphaCmpRS` (> 0x64).
        cam.extend([crate::upscale::DEPTH_NEAR, 100.0, 0.0, 0.0]);
        let _ = SZ_SCALE;
        queue.write_buffer(&self.cam, 0, bytemuck::cast_slice(&cam));
        // Draw list with resolved textures (`PCRenderObject`: objects skip faces whose
        // texture is not loaded; the landscape draws them untextured).
        let mut need_bg = Vec::new();
        for d in &sink.draws {
            let Some((_, groups)) = self.meshes.get(&d.key) else { continue };
            for g in groups {
                let tex = match d.override_tex.or_else(|| textures.current(g.crc)) {
                    Some(t) => t,
                    None if d.landscape => white,
                    None => continue,
                };
                self.list.push((d.rec, d.key, g.first..g.first + g.count, tex, d.wrap));
                need_bg.push((tex, d.wrap));
            }
        }
        need_bg
    }

    fn pipeline(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat, samples: u32, mrt: bool) {
        if self.pipelines.contains_key(&(format, samples, mrt)) {
            return;
        }
        let attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Uint32, 2 => Float32x2, 3 => Float32x4, 4 => Float32x4];
        // uv at 16, normal at 32, colour at 48 (`Vert`).
        let attrs = [
            attrs[0],
            attrs[1],
            wgpu::VertexAttribute { offset: 16, ..attrs[2] },
            wgpu::VertexAttribute { offset: 32, ..attrs[3] },
            wgpu::VertexAttribute { offset: 48, ..attrs[4] },
        ];
        let comp = |src, dst| wgpu::BlendComponent { src_factor: src, dst_factor: dst, operation: wgpu::BlendOperation::Add };
        use wgpu::BlendFactor as F;
        let blend = wgpu::BlendState { color: comp(F::SrcAlpha, F::OneMinusSrcAlpha), alpha: comp(F::One, F::OneMinusSrcAlpha) };
        let targets = [
            Some(wgpu::ColorTargetState { format, blend: Some(blend), write_mask: wgpu::ColorWrites::ALL }),
            Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rg16Float, blend: None, write_mask: wgpu::ColorWrites::ALL }),
            Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::R32Float, blend: None, write_mask: wgpu::ColorWrites::ALL }),
        ];
        let p = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gpu meshes"),
            layout: Some(&self.layout),
            vertex: wgpu::VertexState {
                module: &self.shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout { array_stride: 64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attrs })],
            },
            // `D3DCULL_CW` on a y-down screen: the faces drawn are counter-clockwise on screen.
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: &self.shader,
                entry_point: Some(if mrt { "fs_mrt" } else { "fs" }),
                compilation_options: Default::default(),
                targets: &targets[..if mrt { 3 } else { 1 }],
            }),
            multiview_mask: None,
            cache: None,
        });
        self.pipelines.insert((format, samples, mrt), p);
    }

    /// Creates the pipeline [`draw`](Self::draw) will use.
    pub fn ensure_pipeline(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat, samples: u32, mrt: bool) {
        self.pipeline(device, format, samples, mrt);
    }

    /// Records this frame's mesh draws into `pass` (`bind_group(tex, wrap)` = the texture's
    /// bind group, prepared by the caller).
    pub fn draw<'b>(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        format: wgpu::TextureFormat,
        samples: u32,
        mrt: bool,
        bind_group: impl Fn(TexHandle, bool) -> &'b wgpu::BindGroup,
    ) {
        let (Some((_, _, _, store_bg)), Some(p)) = (&self.store, self.pipelines.get(&(format, samples, mrt))) else { return };
        if self.list.is_empty() {
            return;
        }
        pass.set_pipeline(p);
        pass.set_bind_group(0, &self.cam_bg, &[]);
        pass.set_bind_group(2, store_bg, &[]);
        let mut cur: Option<Key> = None;
        for (rec, key, range, tex, wrap) in &self.list {
            if cur != Some(*key) {
                let Some((buf, _)) = self.meshes.get(key) else { continue };
                pass.set_vertex_buffer(0, buf.slice(..));
                cur = Some(*key);
            }
            pass.set_bind_group(1, bind_group(*tex, *wrap), &[]);
            pass.draw(range.clone(), *rec..*rec + 1);
        }
    }

    pub fn has_draws(&self) -> bool {
        !self.list.is_empty()
    }
}
