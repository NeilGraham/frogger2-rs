//! GPU mesh renderer (enhanced, [`RenderOptions::gpu_meshes`](crate::RenderOptions)).
//!
//! The original projects every vertex on the CPU and hands the GPU screen-space triangles
//! (`scene.rs`, the MDX pipeline). Here the opaque meshes (`Normal` / `Wrap` frames, no
//! MODGE/WAVE animation) stay on the GPU as model data instead: each mesh is expanded once
//! into a vertex buffer (position, bone, uv, normal, colour; faces grouped by texture) and
//! drawn every frame with its object matrices (all bones for a skin) and the camera. That keeps
//! the whole scene's geometry available to the GPU (lighting, shadows, ray tracing) and leaves
//! clipping and culling to the hardware.
//!
//! The vertex shader uses the game's projection (`PCPrepareObject` with the exact depth,
//! [`View::exact_depth`](crate::View), as a matrix: [`crate::upscale::projection`]) and the same
//! linear depth as the CPU path (`vz × 0.00025`), so meshes, CPU-projected sprites, shadows and
//! effects keep occluding each other as before. Translucent and wobbling meshes stay on the CPU
//! path.
//!
//! With [`Lighting`] the fragment shader lights the meshes (`crate::lighting`): hemisphere
//! ambient, a sun / moon with a shadow map (rendered from the light over the meshes around the
//! camera's target, on screen or not) and point lights; emissive surfaces are drawn unlit.

use crate::lighting::{is_emissive, Lighting, PointLight};
use crate::prelude::*;
use crate::scene::{Frame, GpuObj, View, MESH_SKIN, UV_SCALE};
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

/// Faces of one texture: `count` vertices from `first`; where a glowing group's lights go
/// (model space): the centres and sizes of its faces clustered in [`SPOT_CELL`] cells.
#[derive(Clone, Debug)]
struct Group {
    crc: u32,
    first: u32,
    count: u32,
    spots: Vec<([f32; 3], f32)>,
}

/// Cell size of a glowing surface's lights (a lava lake gets one light per cell).
const SPOT_CELL: f32 = 220.0;

/// Mesh key: model id and pre-order object index (or [`MESH_SKIN`]).
type Key = (u32, u16);

/// Per-draw record in the storage buffer (32 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Rec {
    /// First matrix (this frame's and last frame's lists are parallel).
    mat: u32,
    /// Bit 0: no far clip (sky); bit 1: Gouraud override; bit 2: landscape.
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
    sky: bool,
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
    /// Meshes the CPU draws, for the lights of their glowing faces: key and matrix.
    light_only: Vec<(Key, [[f32; 4]; 4])>,
}

impl Sink {
    pub fn clear(&mut self) {
        self.draws.clear();
        self.recs.clear();
        self.mats.clear();
        self.prev.clear();
        self.light_only.clear();
    }

    /// Takes `o` (see [`GpuObj`]); false = the CPU draws it.
    pub fn offer(&mut self, o: &GpuObj, no_clip: bool) -> bool {
        let key = (o.model.id, o.object);
        if !self.known.contains(&key) {
            let (verts, groups) = expand(o);
            self.known.insert(key);
            self.pending.push((key, verts, groups));
        }
        if o.light_only {
            if o.override_tex.is_none() {
                self.light_only.push((key, o.mats[0]));
            }
            return false;
        }
        let mat = self.mats.len() as u32;
        self.mats.extend(o.mats.iter().copied());
        // Bones without last-frame matrices (a new pose): this frame's.
        self.prev.extend((0..o.mats.len()).map(|i| *o.prev.get(i).unwrap_or(&o.mats[i])));
        let flags = no_clip as u32 | (o.gouraud.is_some() as u32) << 1 | (o.landscape as u32) << 2;
        let g = o.gouraud.unwrap_or([1.0; 3]);
        self.recs.push(Rec { mat, flags, _pad: [0; 2], gouraud: [g[0], g[1], g[2], 1.0] });
        self.draws.push(Draw {
            key,
            rec: self.recs.len() as u32 - 1,
            wrap: o.frame == Frame::Wrap,
            landscape: o.landscape,
            sky: no_clip,
            override_tex: o.override_tex,
        });
        true
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Vertex normals: the mesh's own (turned to face the way its faces do), else averaged from the
/// faces (`cross(b - a, c - a)` points out of a drawn face).
fn normals(mesh: &f2_formats::objbank::Mesh) -> Vec<[f32; 3]> {
    let nv = mesh.vertices.len();
    let mut acc = vec![[0.0f32; 3]; nv];
    for f in &mesh.faces {
        let i = f.map(|v| v as usize);
        if i.iter().any(|&k| k >= nv) {
            continue;
        }
        let v = |k: usize| mesh.vertices[i[k]];
        let n = cross(sub(v(1), v(0)), sub(v(2), v(0)));
        for &k in &i {
            acc[k] = [acc[k][0] + n[0], acc[k][1] + n[1], acc[k][2] + n[2]];
        }
    }
    let face_normals: Vec<[f32; 3]> = acc.iter().map(|&n| crate::lighting::normalize(n)).collect();
    if mesh.normals.len() == nv {
        let agree: f32 = mesh.normals.iter().zip(&face_normals).map(|(a, b)| dot(*a, *b)).sum();
        let s = if agree < 0.0 { -1.0 } else { 1.0 };
        // Zero normals (some meshes have them): the faces'.
        return mesh
            .normals
            .iter()
            .zip(&face_normals)
            .map(|(n, f)| if dot(*n, *n) < 1e-6 { *f } else { crate::lighting::normalize([n[0] * s, n[1] * s, n[2] * s]) })
            .collect();
    }
    face_normals
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
    let norm = normals(mesh);
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
            groups.push(Group { crc, first: verts.len() as u32, count: 0, spots: Vec::new() });
        }
        for (k, &i) in idx.iter().enumerate() {
            let uv = mesh.face_uv.get(fi * 3 + k).copied().unwrap_or([0, 0]);
            let n = norm[i];
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
    for g in &mut groups {
        // Skins move their vertices bone by bone: one spot (their lights are not used).
        let vs = &verts[g.first as usize..(g.first + g.count) as usize];
        let mut cells: Vec<([i32; 3], [f32; 3], f32)> = Vec::new();
        for v in vs {
            let cell = v.pos.map(|c| (c / SPOT_CELL).floor() as i32);
            match cells.iter_mut().find(|c| c.0 == cell) {
                Some(c) => {
                    c.1 = [c.1[0] + v.pos[0], c.1[1] + v.pos[1], c.1[2] + v.pos[2]];
                    c.2 += 1.0;
                }
                None => cells.push((cell, v.pos, 1.0)),
            }
        }
        g.spots = cells
            .iter()
            .map(|(cell, sum, n)| {
                let c = [sum[0] / n, sum[1] / n, sum[2] / n];
                let ext = vs
                    .iter()
                    .filter(|v| v.pos.map(|x| (x / SPOT_CELL).floor() as i32) == *cell)
                    .map(|v| dot(sub(v.pos, c), sub(v.pos, c)).sqrt())
                    .fold(0.0, f32::max);
                (c, ext)
            })
            .collect();
    }
    (verts, groups)
}

const SHADER: &str = r#"
override EMISSIVE: bool = false;

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
struct Lights {
    sky: vec4<f32>,       // w: lighting on
    ground: vec4<f32>,    // w: number of point lights
    sun_dir: vec4<f32>,   // to the light; w: sun on
    sun_color: vec4<f32>, // w: specular strength
    sh_centre: vec4<f32>, // w: half extent
    sh_right: vec4<f32>,  // w: depth range
    sh_up: vec4<f32>,     // w: shadows on
    sh_dir: vec4<f32>,    // to the light; w: texel size (uv)
    eye: vec4<f32>,       // camera position
};
struct Point { pos: vec4<f32>, color: vec4<f32> };
@group(0) @binding(0) var<uniform> cam: Cam;
@group(1) @binding(0) var t: texture_2d<f32>;
@group(1) @binding(1) var s: sampler;
@group(2) @binding(0) var<storage, read> recs: array<Rec>;
@group(2) @binding(1) var<storage, read> mats: array<mat4x4<f32>>;
@group(2) @binding(2) var<storage, read> prevs: array<mat4x4<f32>>;
@group(3) @binding(0) var<uniform> lt: Lights;
@group(3) @binding(1) var<storage, read> points: array<Point>;
@group(3) @binding(2) var shadow_map: texture_depth_2d;
@group(3) @binding(3) var shadow_s: sampler_comparison;

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
    @location(7) world: vec3<f32>,
    @location(8) normal: vec3<f32>,
};

// `mtx_xfm`: row-vector affine transform.
fn xfm(m: mat4x4<f32>, p: vec3<f32>) -> vec3<f32> {
    return p.x * m[0].xyz + p.y * m[1].xyz + p.z * m[2].xyz + m[3].xyz;
}
fn xfm_dir(m: mat4x4<f32>, d: vec3<f32>) -> vec3<f32> {
    return d.x * m[0].xyz + d.y * m[1].xyz + d.z * m[2].xyz;
}

// View-space point to clip space without jitter (camera space X = -x, Y = y, Z = z - 10).
fn clip(pr: vec4<f32>, p: vec3<f32>) -> vec4<f32> {
    let z = p.z - 10.0;
    return vec4<f32>(-pr.x * p.x + pr.z * z, pr.y * p.y + pr.w * z, 0.0, z);
}

// World point in the sun's shadow map: (u-ish x, y in -1..1, depth 0..1).
fn light_space(p: vec3<f32>) -> vec3<f32> {
    let d = p - lt.sh_centre.xyz;
    return vec3<f32>(dot(d, lt.sh_right.xyz) / lt.sh_centre.w, dot(d, lt.sh_up.xyz) / lt.sh_centre.w, 0.5 - dot(d, lt.sh_dir.xyz) / lt.sh_right.w);
}

@vertex fn vs(i: VIn, @builtin(instance_index) inst: u32) -> VOut {
    let r = recs[inst];
    let m = mats[r.mat + i.bone];
    let world = xfm(m, i.pos);
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
    o.world = world;
    o.normal = xfm_dir(m, i.normal.xyz);
    return o;
}

// Depth only, from the sun.
@vertex fn vs_shadow(i: VIn, @builtin(instance_index) inst: u32) -> @builtin(position) vec4<f32> {
    let r = recs[inst];
    let l = light_space(xfm(mats[r.mat + i.bone], i.pos));
    return vec4<f32>(l.xy, clamp(l.z, 0.0, 1.0), 1.0);
}

fn shadow(p: vec3<f32>) -> f32 {
    if (lt.sh_up.w < 0.5) { return 1.0; }
    let l = light_space(p);
    let uv = vec2<f32>(l.x * 0.5 + 0.5, 0.5 - l.y * 0.5);
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) || l.z > 1.0) { return 1.0; }
    let ts = lt.sh_dir.w;
    var sum = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            sum += textureSampleCompareLevel(shadow_map, shadow_s, uv + vec2<f32>(f32(x), f32(y)) * ts, l.z - 0.002);
        }
    }
    return sum / 9.0;
}

fn light(i: VOut, base: vec3<f32>) -> vec3<f32> {
    var n = normalize(i.normal);
    let v = normalize(lt.eye.xyz - i.world);
    if (dot(n, v) < 0.0) { n = -n; }
    var l = mix(lt.ground.rgb, lt.sky.rgb, n.y * 0.5 + 0.5);
    var spec = vec3<f32>(0.0);
    if (lt.sun_dir.w > 0.5) {
        let sh = shadow(i.world + n * 2.0);
        let ndl = max(dot(n, lt.sun_dir.xyz), 0.0);
        l += lt.sun_color.rgb * ndl * sh;
        let h = normalize(lt.sun_dir.xyz + v);
        spec = lt.sun_color.rgb * pow(max(dot(n, h), 0.0), 48.0) * lt.sun_color.w * sh * step(0.0, ndl);
    }
    let np = u32(lt.ground.w);
    for (var k = 0u; k < np; k++) {
        let p = points[k];
        let d = p.pos.xyz - i.world;
        let dist = length(d);
        let att = clamp(1.0 - dist / p.pos.w, 0.0, 1.0);
        // Half-wrapped so lights also reach surfaces seen edge-on (sprites' flames, lava).
        l += p.color.rgb * att * att * clamp(dot(n, d / max(dist, 0.001)) * 0.75 + 0.25, 0.0, 1.0);
    }
    // The baked colours are the art's occlusion: dark corners get no highlight either.
    return base * l + spec * dot(i.color.rgb, vec3<f32>(1.0 / 3.0));
}

fn shade(i: VOut) -> vec4<f32> {
    // The far plane (`PCPrepareObject` drops faces past it; here per pixel, no popping).
    if ((i.flags & 1u) == 0u && i.vz - 10.0 >= cam.fog.w) { discard; }
    var c = textureSample(t, s, i.uv) * i.color;
    if (floor(c.a * 255.0 + 0.5) <= cam.depth.y) { discard; }
    if (lt.sky.w > 0.5) {
        if (EMISSIVE) {
            c = vec4<f32>(min(c.rgb * 1.15, vec3<f32>(1.0)), c.a);
        } else if ((i.flags & 1u) == 0u) {
            c = vec4<f32>(light(i, c.rgb), c.a);
        }
    }
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

/// Size of `Cam` and `Lights`.
const CAM_SIZE: u64 = 64 * 2 + 16 * 6;
const LIGHTS_SIZE: u64 = 16 * 9;
/// Point lights drawn at once (the nearest to the camera's target).
pub const MAX_POINT_LIGHTS: usize = 32;
/// Shadow map size.
const SHADOW_SIZE: u32 = 2048;

/// The GPU side: uploaded meshes, per-frame buffers, pipelines.
pub(crate) struct Gpu {
    meshes: HashMap<Key, (wgpu::Buffer, Vec<Group>)>,
    shader: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    store_layout: wgpu::BindGroupLayout,
    cam: wgpu::Buffer,
    cam_bg: wgpu::BindGroup,
    lights: wgpu::Buffer,
    points: wgpu::Buffer,
    lights_bg: wgpu::BindGroup,
    shadow_ubo_bg: wgpu::BindGroup,
    shadow_view: wgpu::TextureView,
    shadow_pipeline: wgpu::RenderPipeline,
    /// recs, mats, prev, and the bind group over them.
    store: Option<(wgpu::Buffer, wgpu::Buffer, wgpu::Buffer, wgpu::BindGroup)>,
    pipelines: HashMap<(wgpu::TextureFormat, u32, bool, bool), wgpu::RenderPipeline>,
    /// This frame's draw list: (record, mesh key, vertex range, texture, wrap, emissive).
    list: Vec<(u32, Key, core::ops::Range<u32>, TexHandle, bool, bool)>,
    /// Emissive and mean colour per texture CRC.
    tex_info: HashMap<u32, (bool, [f32; 3])>,
    /// The shadow map needs drawing this frame.
    shadows: bool,
}

/// The lights of glowing group `g` drawn with matrix `m`: at its spots, the texture's colour
/// pushed warm, reaching further for larger surfaces.
fn point_lights(m: &[[f32; 4]; 4], g: &Group, color: [f32; 3], out: &mut Vec<PointLight>) {
    for &(c, ext) in &g.spots {
        // Above the surface (lava lakes light the rocks standing in them).
        let p = crate::math::mtx_xfm(m, c);
        let pos = [p[0], p[1] + 30.0 + ext * 0.15, p[2]];
        out.push(PointLight { pos, color: [color[0] * 1.7, color[1] * 1.4, color[2] * 1.1], radius: 140.0 + ext * 1.5 });
    }
}

/// Whether texture `crc` glows, and its mean colour (`None`: not loaded), cached in `info`.
fn emissive(info: &mut HashMap<u32, (bool, [f32; 3])>, textures: &TextureRegistry, crc: u32) -> Option<(bool, [f32; 3])> {
    if let Some(i) = info.get(&crc) {
        return Some(*i);
    }
    let e = textures.entry(crc)?;
    let col = textures.images.get(e.image.0 as usize).map_or([1.0; 3], |img| mean_color(&img.rgba));
    let i = (is_emissive(&e.name), col);
    info.insert(crc, i);
    Some(i)
}

/// Mean colour of an image (`rgba`, weighted by alpha).
fn mean_color(rgba: &[u8]) -> [f32; 3] {
    let mut acc = [0.0f64; 4];
    for p in rgba.as_chunks::<4>().0 {
        let a = p[3] as f64 / 255.0;
        for c in 0..3 {
            acc[c] += p[c] as f64 / 255.0 * a;
        }
        acc[3] += a;
    }
    if acc[3] <= 0.0 {
        return [1.0; 3];
    }
    [(acc[0] / acc[3]) as f32, (acc[1] / acc[3]) as f32, (acc[2] / acc[3]) as f32]
}

impl Gpu {
    pub fn new(device: &wgpu::Device, tex_layout: &wgpu::BindGroupLayout) -> Gpu {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("gpu meshes"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let ubo = |binding, vis| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: vis,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let storage = |binding, vis| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: vis,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let vf = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let cam_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("gpu mesh camera"), entries: &[ubo(0, vf)] });
        let v = wgpu::ShaderStages::VERTEX;
        let store_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gpu mesh draws"),
            entries: &[storage(0, v), storage(1, v), storage(2, v)],
        });
        let lights_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gpu mesh lights"),
            entries: &[
                ubo(0, vf),
                storage(1, wgpu::ShaderStages::FRAGMENT),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        });
        // The shadow pass reads only the lights' uniform block (the map is its target).
        let shadow_ubo_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("gpu mesh shadow"), entries: &[ubo(0, vf)] });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gpu meshes"),
            bind_group_layouts: &[Some(&cam_layout), Some(tex_layout), Some(&store_layout), Some(&lights_layout)],
            immediate_size: 0,
        });
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gpu mesh shadow"),
            bind_group_layouts: &[None, None, Some(&store_layout), Some(&shadow_ubo_layout)],
            immediate_size: 0,
        });
        let buf = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage: usage | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
        };
        let cam = buf("gpu mesh camera", CAM_SIZE, wgpu::BufferUsages::UNIFORM);
        let lights = buf("gpu mesh lights", LIGHTS_SIZE, wgpu::BufferUsages::UNIFORM);
        let points = buf("gpu mesh point lights", (MAX_POINT_LIGHTS * 32) as u64, wgpu::BufferUsages::STORAGE);
        let cam_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gpu mesh camera"),
            layout: &cam_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: cam.as_entire_binding() }],
        });
        let shadow_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sun shadow map"),
            size: wgpu::Extent3d { width: SHADOW_SIZE, height: SHADOW_SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shadow_view = shadow_tex.create_view(&Default::default());
        let shadow_s = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let lights_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gpu mesh lights"),
            layout: &lights_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: lights.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: points.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&shadow_view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&shadow_s) },
            ],
        });
        let shadow_ubo_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gpu mesh shadow"),
            layout: &shadow_ubo_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: lights.as_entire_binding() }],
        });
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sun shadow"),
            layout: Some(&shadow_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_shadow"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_layout())],
            },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: wgpu::DepthBiasState { constant: 2, slope_scale: 2.0, clamp: 0.0 },
            }),
            multisample: Default::default(),
            fragment: None,
            multiview_mask: None,
            cache: None,
        });
        Gpu {
            meshes: HashMap::new(),
            shader,
            layout,
            store_layout,
            cam,
            cam_bg,
            lights,
            points,
            lights_bg,
            shadow_ubo_bg,
            shadow_view,
            shadow_pipeline,
            store: None,
            pipelines: HashMap::new(),
            list: Vec::new(),
            tex_info: HashMap::new(),
            shadows: false,
        }
    }

    /// Uploads new meshes and this frame's records, matrices, camera and lights; resolves the
    /// textures of every draw. Returns the (texture, wrap) pairs whose bind groups the
    /// renderer must have before [`draw`](Self::draw). `extra_lights`: point lights the
    /// renderer found elsewhere (emissive sprites).
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
        lighting: Option<&Lighting>,
        extra_lights: &[PointLight],
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
        self.shadows = false;
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
        queue.write_buffer(&self.cam, 0, bytemuck::cast_slice(&cam));
        // Draw list with resolved textures (`PCRenderObject`: objects skip faces whose
        // texture is not loaded; the landscape draws them untextured), and the point lights
        // of emissive groups.
        let lit = lighting.is_some();
        let auto_lights = lighting.is_some_and(|l| l.emissive_lights);
        let mut found: Vec<PointLight> = Vec::new();
        let mut need_bg = Vec::new();
        for d in &sink.draws {
            let Some((_, groups)) = self.meshes.get(&d.key) else { continue };
            for g in groups {
                let tex = match d.override_tex.or_else(|| textures.current(g.crc)) {
                    Some(t) => t,
                    None if d.landscape => white,
                    None => continue,
                };
                let (emissive, color) = emissive(&mut self.tex_info, textures, g.crc).unwrap_or((false, [1.0; 3]));
                let emissive = lit && emissive && d.override_tex.is_none() && !d.sky;
                if emissive && auto_lights && d.key.1 != MESH_SKIN {
                    point_lights(&sink.mats[sink.recs[d.rec as usize].mat as usize], g, color, &mut found);
                }
                self.list.push((d.rec, d.key, g.first..g.first + g.count, tex, d.wrap, emissive));
                need_bg.push((tex, d.wrap));
            }
        }
        if auto_lights {
            for (key, m) in &sink.light_only {
                let Some((_, groups)) = self.meshes.get(key) else { continue };
                for g in groups {
                    if let Some((true, color)) = emissive(&mut self.tex_info, textures, g.crc) {
                        point_lights(m, g, color, &mut found);
                    }
                }
            }
        }
        self.write_lights(queue, view, lighting, found, extra_lights);
        need_bg
    }

    /// The lights' uniform block and point light list (the nearest to the camera's target).
    fn write_lights(&mut self, queue: &wgpu::Queue, view: &View, lighting: Option<&Lighting>, mut found: Vec<PointLight>, extra: &[PointLight]) {
        let mut u = [0.0f32; (LIGHTS_SIZE / 4) as usize];
        let Some(l) = lighting else {
            queue.write_buffer(&self.lights, 0, bytemuck::cast_slice(&u));
            return;
        };
        let cam = view.camera.unwrap_or(crate::scene::Camera { source: [0.0, 400.0, -300.0], target: [0.0; 3], up: [0.0, 1.0, 0.0] });
        let focus = cam.target;
        let mut lights: Vec<PointLight> = l.point_lights.clone();
        if l.emissive_lights {
            found.extend_from_slice(extra);
            // Merge lights closer than 40 units (a lava pool's many groups, a torch's sprites).
            let mut merged: Vec<(PointLight, f32)> = Vec::new();
            for p in found {
                match merged.iter_mut().find(|(m, _)| dot(sub(m.pos, p.pos), sub(m.pos, p.pos)) < 40.0 * 40.0) {
                    Some((m, n)) => {
                        let k = 1.0 / (*n + 1.0);
                        m.pos = [m.pos[0] + (p.pos[0] - m.pos[0]) * k, m.pos[1] + (p.pos[1] - m.pos[1]) * k, m.pos[2] + (p.pos[2] - m.pos[2]) * k];
                        m.radius = m.radius.max(p.radius);
                        *n += 1.0;
                    }
                    None => merged.push((p, 1.0)),
                }
            }
            lights.extend(merged.into_iter().map(|(p, _)| p));
        }
        let d2 = |p: &PointLight| dot(sub(p.pos, focus), sub(p.pos, focus));
        lights.sort_by(|a, b| d2(a).total_cmp(&d2(b)));
        lights.truncate(MAX_POINT_LIGHTS);
        let mut pts = vec![0.0f32; MAX_POINT_LIGHTS * 8];
        for (k, p) in lights.iter().enumerate() {
            pts[k * 8..k * 8 + 8].copy_from_slice(&[p.pos[0], p.pos[1], p.pos[2], p.radius, p.color[0], p.color[1], p.color[2], 1.0]);
        }
        queue.write_buffer(&self.points, 0, bytemuck::cast_slice(&pts));
        // Shadow map frame: centred on the camera's target, sized by the camera distance,
        // snapped to whole texels so it does not shimmer as the camera moves.
        let sun = l.sun;
        let to_light = sun.map_or([0.0, 1.0, 0.0], |s| crate::lighting::normalize(s.to_light));
        let up_hint = if to_light[1].abs() > 0.95 { [0.0, 0.0, 1.0] } else { [0.0, 1.0, 0.0] };
        let right = crate::lighting::normalize(cross(up_hint, to_light));
        let up = cross(to_light, right);
        let dist = dot(sub(cam.source, cam.target), sub(cam.source, cam.target)).sqrt();
        let half = (dist * 1.6).clamp(500.0, 3000.0);
        let texel = 2.0 * half / SHADOW_SIZE as f32;
        let snap = |v: f32| (v / texel).round() * texel;
        let (cx, cy, cz) = (dot(focus, right), dot(focus, up), dot(focus, to_light));
        let (cx, cy) = (snap(cx), snap(cy));
        let centre =
            [right[0] * cx + up[0] * cy + to_light[0] * cz, right[1] * cx + up[1] * cy + to_light[1] * cz, right[2] * cx + up[2] * cy + to_light[2] * cz];
        let depth_range = 8000.0;
        self.shadows = sun.is_some_and(|s| s.shadows);
        let (sd, sc) = sun.map_or(([0.0; 3], [0.0; 3]), |s| (to_light, s.color));
        u.copy_from_slice(&[
            l.sky[0],
            l.sky[1],
            l.sky[2],
            1.0,
            l.ground[0],
            l.ground[1],
            l.ground[2],
            lights.len() as f32,
            sd[0],
            sd[1],
            sd[2],
            sun.is_some() as u32 as f32,
            sc[0],
            sc[1],
            sc[2],
            l.specular,
            centre[0],
            centre[1],
            centre[2],
            half,
            right[0],
            right[1],
            right[2],
            depth_range,
            up[0],
            up[1],
            up[2],
            self.shadows as u32 as f32,
            to_light[0],
            to_light[1],
            to_light[2],
            1.0 / SHADOW_SIZE as f32,
            cam.source[0],
            cam.source[1],
            cam.source[2],
            1.0,
        ]);
        queue.write_buffer(&self.lights, 0, bytemuck::cast_slice(&u));
    }

    /// Renders the sun's shadow map (every GPU mesh draw, on screen or not).
    pub fn shadow_pass(&self, encoder: &mut wgpu::CommandEncoder) {
        let clear_only = !self.shadows || self.list.is_empty();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sun shadow"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.shadow_view,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let Some((_, _, _, store_bg)) = &self.store else { return };
        if clear_only {
            return;
        }
        pass.set_pipeline(&self.shadow_pipeline);
        pass.set_bind_group(2, store_bg, &[]);
        pass.set_bind_group(3, &self.shadow_ubo_bg, &[]);
        let mut cur: Option<Key> = None;
        for (rec, key, range, _, _, _) in &self.list {
            if cur != Some(*key) {
                let Some((buf, _)) = self.meshes.get(key) else { continue };
                pass.set_vertex_buffer(0, buf.slice(..));
                cur = Some(*key);
            }
            pass.draw(range.clone(), *rec..*rec + 1);
        }
    }

    fn pipeline(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat, samples: u32, mrt: bool, emissive: bool) {
        if self.pipelines.contains_key(&(format, samples, mrt, emissive)) {
            return;
        }
        let comp = |src, dst| wgpu::BlendComponent { src_factor: src, dst_factor: dst, operation: wgpu::BlendOperation::Add };
        use wgpu::BlendFactor as F;
        let blend = wgpu::BlendState { color: comp(F::SrcAlpha, F::OneMinusSrcAlpha), alpha: comp(F::One, F::OneMinusSrcAlpha) };
        let targets = [
            Some(wgpu::ColorTargetState { format, blend: Some(blend), write_mask: wgpu::ColorWrites::ALL }),
            Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rg16Float, blend: None, write_mask: wgpu::ColorWrites::ALL }),
            Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::R32Float, blend: None, write_mask: wgpu::ColorWrites::ALL }),
        ];
        let constants = [("EMISSIVE", if emissive { 1.0 } else { 0.0 })];
        let p = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gpu meshes"),
            layout: Some(&self.layout),
            vertex: wgpu::VertexState {
                module: &self.shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_layout())],
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
                compilation_options: wgpu::PipelineCompilationOptions { constants: &constants, ..Default::default() },
                targets: &targets[..if mrt { 3 } else { 1 }],
            }),
            multiview_mask: None,
            cache: None,
        });
        self.pipelines.insert((format, samples, mrt, emissive), p);
    }

    /// Creates the pipelines [`draw`](Self::draw) will use.
    pub fn ensure_pipeline(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat, samples: u32, mrt: bool) {
        self.pipeline(device, format, samples, mrt, false);
        self.pipeline(device, format, samples, mrt, true);
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
        let Some((_, _, _, store_bg)) = &self.store else { return };
        let (Some(lit), Some(emi)) = (self.pipelines.get(&(format, samples, mrt, false)), self.pipelines.get(&(format, samples, mrt, true))) else { return };
        if self.list.is_empty() {
            return;
        }
        pass.set_bind_group(0, &self.cam_bg, &[]);
        pass.set_bind_group(2, store_bg, &[]);
        pass.set_bind_group(3, &self.lights_bg, &[]);
        let mut cur: Option<Key> = None;
        let mut cur_emissive = None;
        for (rec, key, range, tex, wrap, emissive) in &self.list {
            if cur_emissive != Some(*emissive) {
                pass.set_pipeline(if *emissive { emi } else { lit });
                cur_emissive = Some(*emissive);
            }
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

/// `Vert` as vertex attributes (uv at 16, normal at 32, colour at 48).
fn vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRS: [wgpu::VertexAttribute; 5] = [
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Uint32, offset: 12, shader_location: 1 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 16, shader_location: 2 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 32, shader_location: 3 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 48, shader_location: 4 },
    ];
    wgpu::VertexBufferLayout { array_stride: 64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &ATTRS }
}
