//! Path-Traced graphics: ray-traced lighting for the GPU mesh renderer (`docs/PATH-TRACING.md`).
//!
//! On devices with ray queries ([`wgpu::Features::EXPERIMENTAL_RAY_QUERY`]) every uploaded rigid
//! mesh gets a bottom-level acceleration structure (its groups with opaque textures; cut-out
//! foliage and skins keep the shadow map), and each frame's GPU mesh draws become the instances
//! of a top-level one. The lit fragment shader then traces, per pixel:
//!
//! - soft sun / moon shadows (2 rays into a 1.7° disc),
//! - shadows of the 8 nearest point lights (1 ray each, to a jittered point on the light),
//! - ambient occlusion with one bounce of colour (2 cosine-weighted rays, 90 units; a hit
//!   surface returns its mean albedo lit by the ambient, and by the sun if a shadow ray from it
//!   reaches the sun),
//!
//! - refraction: a view ray bent by the wavy water surface traced to what it lands on (what
//!   lies under the water wavers like the original's wobbling river-bed textures),
//!
//! with per-pixel noise that changes every frame (the upscaler or TAA averages it), and
//! caustics by **photon tracing**: a compute pass refracts the sun through the simulated water
//! surface (2×2 photons per simulation cell), traces each photon to what it lands on and counts
//! the landings per cell; surfaces under the water are lit by the photon density there instead
//! of the procedural pattern.

use crate::gpu_mesh::Key;
use std::collections::HashMap;

/// The ray-traced versions of the shader's `RT-BEGIN` … `RT-END` hooks.
pub(crate) const RT_FNS: &str = r#"
@group(3) @binding(6) var acc: acceleration_structure;
@group(3) @binding(7) var<storage, read> albedo: array<vec4<f32>>;
@group(3) @binding(8) var<storage, read> photons: array<u32>;

// Two numbers in [0, 1) per pixel, sample and frame (interleaved gradient noise). `lt.eye.w`:
// the frame (a temporal upscaler averages the noise over frames), or -1 without one (a fixed
// pattern, and twice the rays).
fn rnd(seed: vec2<f32>, k: f32) -> vec2<f32> {
    let f = max(lt.eye.w, 0.0) * 5.588238 + k * 13.17;
    let a = fract(52.9829189 * fract(dot(seed + f, vec2<f32>(0.06711056, 0.00583715))));
    let b = fract(52.9829189 * fract(dot(seed.yx + f * 1.7 + 11.0, vec2<f32>(0.06711056, 0.00583715))));
    return vec2<f32>(a, b);
}

// Tangent, bitangent, n as columns.
fn frame(n: vec3<f32>) -> mat3x3<f32> {
    let up = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(n.y) > 0.9);
    let t = normalize(cross(up, n));
    return mat3x3<f32>(t, cross(n, t), n);
}

// The closest hit along a ray: distance (-1: none) and the instance's index. (Ray queries live
// in functions, never directly in loops: some drivers fail to compile that.)
struct Hit { t: f32, inst: u32 };
fn closest(o: vec3<f32>, d: vec3<f32>, tmax: f32) -> Hit {
    var rq: ray_query;
    rayQueryInitialize(&rq, acc, RayDesc(RAY_FLAG_NONE, 0xFFu, 0.01, tmax, o, d));
    rayQueryProceed(&rq);
    let h = rayQueryGetCommittedIntersection(&rq);
    if (h.kind == RAY_QUERY_INTERSECTION_NONE) { return Hit(-1.0, 0u); }
    return Hit(h.t, h.instance_custom_data);
}

fn blocked(o: vec3<f32>, d: vec3<f32>, tmax: f32) -> bool {
    var rq: ray_query;
    rayQueryInitialize(&rq, acc, RayDesc(RAY_FLAG_TERMINATE_ON_FIRST_HIT, 0xFFu, 0.01, tmax, o, d));
    rayQueryProceed(&rq);
    return rayQueryGetCommittedIntersection(&rq).kind != RAY_QUERY_INTERSECTION_NONE;
}

fn rt_sun(p: vec3<f32>, n: vec3<f32>, seed: vec2<f32>) -> f32 {
    let f = frame(lt.sun_dir.xyz);
    var lit = 0.0;
    let rays = select(2, 4, lt.eye.w < 0.0);
    for (var k = 0; k < rays; k++) {
        let r = rnd(seed, f32(k));
        let a = r.x * 6.2831853;
        let rad = sqrt(r.y) * 0.03;
        let d = normalize(f * vec3<f32>(cos(a) * rad, sin(a) * rad, 1.0));
        if (!blocked(p + n * 0.75, d, 6000.0)) { lit += 1.0; }
    }
    return lit / f32(rays);
}

fn rt_point(p: vec3<f32>, n: vec3<f32>, lp: vec3<f32>, k: u32, seed: vec2<f32>) -> f32 {
    if (k >= 8u) { return 1.0; }
    let r = rnd(seed, 7.0 + f32(k));
    let j = (vec3<f32>(r.x, r.y, fract(r.x + r.y * 1.618)) - 0.5) * 8.0;
    let o = p + n * 0.75;
    let to = lp + j - o;
    let dist = length(to);
    if (dist < 4.5 || !blocked(o, to / dist, dist - 4.0)) { return 1.0; }
    return 0.0;
}

fn rt_ambient(p: vec3<f32>, n: vec3<f32>, seed: vec2<f32>) -> vec4<f32> {
    let f = frame(n);
    var open = 0.0;
    var bounce = vec3<f32>(0.0);
    let reach = 90.0;
    let rays = select(2, 4, lt.eye.w < 0.0);
    let share = 1.0 / f32(rays);
    for (var k = 0; k < rays; k++) {
        let r = rnd(seed, 3.0 + f32(k));
        let a = r.x * 6.2831853;
        let s = sqrt(r.y);
        let d = f * vec3<f32>(cos(a) * s, sin(a) * s, sqrt(1.0 - r.y));
        let h = closest(p + n * 0.75, d, reach);
        if (h.t < 0.0) {
            open += share;
        } else {
            let w = h.t / reach;
            open += share * w * w;
            // One bounce: the hit surface's albedo lit by about half the ambient, and by half the
            // sun if a ray from it reaches the sun.
            let alb = albedo[h.inst].rgb;
            let hp = p + n * 0.75 + d * (h.t - 0.5);
            var sun = 0.0;
            if (lt.sun_dir.w > 0.5 && !blocked(hp, lt.sun_dir.xyz, 6000.0)) { sun = 0.5; }
            let lit = (lt.sky.rgb + lt.ground.rgb) * 0.25 + lt.sun_color.rgb * sun;
            bounce += share * alb * lit * (1.0 - w * w);
        }
    }
    // Softened: the baked colours already darken the corners.
    return vec4<f32>(bounce, mix(1.0, open, 0.75));
}

// The view ray refracted at the water surface, traced to what it lands on.
fn rt_refract(sp: vec3<f32>, r: vec3<f32>, depth: f32) -> vec2<f32> {
    let flat = sp.xz + r.xz * (depth / max(-r.y, 0.05));
    let h = closest(sp, r, depth * 4.0 + 20.0);
    if (h.t < 0.0) { return flat; }
    return (sp + r * h.t).xz;
}

fn photon_at(c: vec2<i32>) -> f32 {
    if (!in_window(c)) { return 1.0; }
    return f32(photons[u32(c.y) * u32(lt.water.w) + u32(c.x)]) * (1.0 / 1024.0);
}

fn rt_caustics(p: vec3<f32>, ndl: f32) -> f32 {
    let sy = surface_y(p.xz);
    let depth = sy - p.y;
    if (depth < 0.5) { return 0.0; }
    let i = vec2<i32>(floor(cell(p.xz) + 0.5));
    var sum = 0.0;
    var wsum = 0.0;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let w = f32((2 - abs(dx)) * (2 - abs(dy)));
            sum += photon_at(i + vec2<i32>(dx, dy)) * w;
            wsum += w;
        }
    }
    // The sun reaches the bottom as the photons do: 1 = evenly, more where the waves focus it.
    return (sum / wsum - 1.0) * ndl * smoothstep(0.5, 6.0, depth) * 1.4;
}
"#;

/// The photon tracer (`{LIGHTS}` and `{WATER}` are the shared WGSL).
const PHOTONS: &str = r#"
enable wgpu_ray_query;
{LIGHTS}
@group(0) @binding(0) var<uniform> lt: Lights;
@group(0) @binding(1) var<storage, read> wave: array<vec2<f32>>;
@group(0) @binding(2) var surface: texture_depth_2d;
@group(0) @binding(3) var acc: acceleration_structure;
@group(0) @binding(4) var<storage, read_write> photons: array<atomic<u32>>;
{WATER}

// Where a photon from o along d lands (-1: nowhere).
fn land(o: vec3<f32>, d: vec3<f32>) -> f32 {
    var rq: ray_query;
    rayQueryInitialize(&rq, acc, RayDesc(RAY_FLAG_NONE, 0xFFu, 0.1, 1500.0, o, d));
    rayQueryProceed(&rq);
    let h = rayQueryGetCommittedIntersection(&rq);
    if (h.kind == RAY_QUERY_INTERSECTION_NONE) { return -1.0; }
    return h.t;
}

@compute @workgroup_size(8, 8)
fn trace(@builtin(global_invocation_id) id: vec3<u32>) {
    let n = u32(lt.water.w);
    if (id.x >= n || id.y >= n || lt.sun_dir.w < 0.5 || lt.water2.y < 0.5) { return; }
    let l = lt.sun_dir.xyz;
    for (var k = 0u; k < 4u; k++) {
        let j = (vec2<f32>(f32(k & 1u), f32(k >> 1u)) + 0.5) * 0.5;
        let xz = lt.water.xy + (vec2<f32>(id.xy) + j) * lt.water.z;
        let sy = surface_y(xz);
        if (sy < -1e8) { continue; }
        let o = vec3<f32>(xz.x, sy + water_h(xz), xz.y);
        let d = refract(-l, water_n(xz), 1.0 / 1.33);
        if (dot(d, d) < 0.5) { continue; }
        let t = land(o, d);
        if (t < 0.0) { continue; }
        let c = vec2<i32>(floor(cell((o + d * t).xz) + 0.5));
        if (!in_window(c)) { continue; }
        atomicAdd(&photons[u32(c.y) * n + u32(c.x)], 256u);
    }
}
"#;

/// A TLAS instance: mesh, object matrix (row vectors, `mtx_xfm`), mean albedo.
pub(crate) type Instance = (Key, [[f32; 4]; 4], [f32; 3]);

/// One mesh's acceleration structure: the BLAS and its geometries (size, first vertex).
struct Blas {
    blas: wgpu::Blas,
    geometry: Vec<(wgpu::BlasTriangleGeometrySizeDescriptor, u32)>,
}

pub(crate) struct Rt {
    /// Per mesh: its BLAS (`None`: nothing opaque to trace).
    blas: HashMap<Key, Option<Blas>>,
    unbuilt: Vec<Key>,
    pub tlas: wgpu::Tlas,
    cap: u32,
    used: usize,
    pub albedo: wgpu::Buffer,
    pub photons: wgpu::Buffer,
    photon_pipeline: wgpu::ComputePipeline,
    pub photon_layout: wgpu::BindGroupLayout,
    pub photon_bg: Option<wgpu::BindGroup>,
    /// The TLAS or albedo buffer was replaced: the bind groups over them must be remade.
    pub rebind: bool,
}

fn tlas(device: &wgpu::Device, cap: u32) -> wgpu::Tlas {
    device.create_tlas(&wgpu::CreateTlasDescriptor {
        label: Some("scene"),
        max_instances: cap,
        flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
        update_mode: wgpu::AccelerationStructureUpdateMode::Build,
    })
}

fn albedo(device: &wgpu::Device, cap: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("instance albedo"),
        size: cap as u64 * 16,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

impl Rt {
    /// `lights` / `water`: the shared WGSL (`LIGHTS-BEGIN` … and `WATER-BEGIN` … blocks).
    pub fn new(device: &wgpu::Device, lights: &str, water: &str) -> Rt {
        let src = PHOTONS.replace("{LIGHTS}", lights).replace("{WATER}", water);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("photons"), source: wgpu::ShaderSource::Wgsl(src.into()) });
        let c = wgpu::ShaderStages::COMPUTE;
        let buf = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: c,
            ty: wgpu::BindingType::Buffer { ty, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let photon_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("photons"),
            entries: &[
                buf(0, wgpu::BufferBindingType::Uniform),
                buf(1, wgpu::BufferBindingType::Storage { read_only: true }),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: c,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: c, ty: wgpu::BindingType::AccelerationStructure { vertex_return: false }, count: None },
                buf(4, wgpu::BufferBindingType::Storage { read_only: false }),
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("photons"),
            bind_group_layouts: &[Some(&photon_layout)],
            immediate_size: 0,
        });
        let photon_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("photons"),
            layout: Some(&pl),
            module: &shader,
            entry_point: Some("trace"),
            compilation_options: Default::default(),
            cache: None,
        });
        let n = crate::water::N as u64;
        let photons = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("photon counts"),
            size: n * n * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cap = 1024;
        Rt {
            blas: HashMap::new(),
            unbuilt: Vec::new(),
            tlas: tlas(device, cap),
            cap,
            used: 0,
            albedo: albedo(device, cap),
            photons,
            photon_pipeline,
            photon_layout,
            photon_bg: None,
            rebind: true,
        }
    }

    /// Whether mesh `key` has been considered (see [`add_mesh`](Self::add_mesh)).
    pub fn has_mesh(&self, key: &Key) -> bool {
        self.blas.contains_key(key)
    }

    /// Creates `key`'s BLAS over the vertex ranges `opaque` (`(first, count)`; built with the
    /// next frame's acceleration structures).
    pub fn add_mesh(&mut self, device: &wgpu::Device, key: Key, opaque: &[(u32, u32)]) {
        let geometry: Vec<_> = opaque
            .iter()
            .filter(|r| r.1 >= 3)
            .map(|&(first, count)| {
                (
                    wgpu::BlasTriangleGeometrySizeDescriptor {
                        vertex_format: wgpu::VertexFormat::Float32x3,
                        vertex_count: count,
                        index_format: None,
                        index_count: None,
                        flags: wgpu::AccelerationStructureGeometryFlags::OPAQUE,
                    },
                    first,
                )
            })
            .collect();
        if geometry.is_empty() {
            self.blas.insert(key, None);
            return;
        }
        let blas = device.create_blas(
            &wgpu::CreateBlasDescriptor {
                label: Some("mesh"),
                flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
                update_mode: wgpu::AccelerationStructureUpdateMode::Build,
            },
            wgpu::BlasGeometrySizeDescriptors::Triangles { descriptors: geometry.iter().map(|g| g.0.clone()).collect() },
        );
        self.blas.insert(key, Some(Blas { blas, geometry }));
        self.unbuilt.push(key);
    }

    /// This frame's instances: mesh, object matrix (row vectors, `mtx_xfm`), mean albedo.
    pub fn set_instances(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, instances: &[Instance]) {
        let live: Vec<_> = instances.iter().filter(|(k, _, _)| matches!(self.blas.get(k), Some(Some(_)))).collect();
        if live.len() as u32 > self.cap {
            self.cap = (live.len() as u32).next_power_of_two();
            self.tlas = tlas(device, self.cap);
            self.albedo = albedo(device, self.cap);
            self.used = 0;
            self.rebind = true;
        }
        let mut alb = Vec::with_capacity(live.len() * 4);
        for (i, (key, m, a)) in live.iter().enumerate() {
            let Some(Some(b)) = self.blas.get(key) else { continue };
            // Row vectors (world = p·M) to the TLAS's 3×4 row-major [R | t] (world = M·p).
            let t = [m[0][0], m[1][0], m[2][0], m[3][0], m[0][1], m[1][1], m[2][1], m[3][1], m[0][2], m[1][2], m[2][2], m[3][2]];
            self.tlas[i] = Some(wgpu::TlasInstance::new(&b.blas, t, i as u32, 0xFF));
            alb.extend_from_slice(&[a[0], a[1], a[2], 1.0]);
        }
        for i in live.len()..self.used {
            self.tlas[i] = None;
        }
        self.used = live.len();
        if !alb.is_empty() {
            queue.write_buffer(&self.albedo, 0, bytemuck::cast_slice(&alb));
        }
    }

    /// Builds the new BLASes (vertex buffers from `meshes`) and this frame's TLAS.
    pub fn build(&mut self, encoder: &mut wgpu::CommandEncoder, buffer: impl Fn(&Key) -> Option<wgpu::Buffer>) {
        let keys = core::mem::take(&mut self.unbuilt);
        let bufs: Vec<_> = keys.iter().map(&buffer).collect();
        let entries: Vec<_> = keys
            .iter()
            .zip(&bufs)
            .filter_map(|(k, buf)| {
                let (Some(Some(b)), Some(buf)) = (self.blas.get(k), buf.as_ref()) else { return None };
                let geometry = b
                    .geometry
                    .iter()
                    .map(|(size, first)| wgpu::BlasTriangleGeometry {
                        size,
                        vertex_buffer: buf,
                        first_vertex: *first,
                        vertex_stride: 64,
                        index_buffer: None,
                        first_index: None,
                        transform_buffer: None,
                        transform_buffer_offset: None,
                    })
                    .collect();
                Some(wgpu::BlasBuildEntry { blas: &b.blas, geometry: wgpu::BlasGeometries::TriangleGeometries(geometry) })
            })
            .collect();
        encoder.build_acceleration_structures(entries.iter(), [&self.tlas]);
    }

    /// Clears the photon counts and traces this frame's photons.
    pub fn trace_photons(&self, encoder: &mut wgpu::CommandEncoder, run: bool) {
        encoder.clear_buffer(&self.photons, 0, None);
        let Some(bg) = self.photon_bg.as_ref().filter(|_| run) else { return };
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("photons"), timestamp_writes: None });
        pass.set_pipeline(&self.photon_pipeline);
        pass.set_bind_group(0, bg, &[]);
        let g = crate::water::N.div_ceil(8);
        pass.dispatch_workgroups(g, g, 1);
    }
}
