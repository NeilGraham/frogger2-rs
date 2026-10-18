//! Enhanced water (Graphics: Enhanced, [`Lighting::water`](crate::Lighting); `docs/WATER.md`).
//!
//! The original animates its water on the CPU: `PCCalcModgeValues` wobbles the texture
//! coordinates of MODGE meshes and lifts the vertices of WAVE meshes by a slow swell. The
//! enhanced renderer draws those meshes on the GPU instead (`gpu_mesh.rs`): water surfaces
//! subdivided (finer geometry), with the same wobble and swell computed per vertex, plus a wave
//! simulation that objects disturb, and caustics on what lies under the water.
//!
//! The simulation is a damped wave equation on a [`N`]² height field of [`TEXEL`]-unit cells
//! in a window around the camera's target (it scrolls with the camera, whole cells at a time).
//! A top-down depth render of the water meshes (the *surface map*) says where water is and at
//! what height; cells without water stay flat and reflect waves (shores). [`Splash`]es push the
//! surface down where something moves in or lands on the water. It steps at [`STEP_HZ`] in pairs
//! (so the current state is always in the same buffer) from the host's clock, independent of
//! the frame rate.

/// Cells per side of the simulation window.
pub(crate) const N: u32 = 512;
/// World units per cell.
pub(crate) const TEXEL: f32 = 3.0;
/// Simulation steps per second.
pub(crate) const STEP_HZ: f64 = 120.0;
/// Splashes applied per step.
pub(crate) const MAX_SPLATS: usize = 64;
/// Depth range of the surface map (world units around the reference height).
pub(crate) const RANGE: f32 = 8000.0;
/// Uniform stride of the per-step parameters (dynamic offsets).
const STEP_STRIDE: u64 = 256;
const MAX_STEPS: u32 = 4;

/// Something disturbing the water at `pos` (world units): the surface within `radius` is
/// pushed down by up to `strength` units (if the water there is within `radius` of `pos`'s
/// height).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Splash {
    pub pos: [f32; 3],
    pub radius: f32,
    pub strength: f32,
}

const SIM: &str = r#"
struct Step {
    shift: vec2<i32>,
    nsplat: u32,
    n: u32,
    origin: vec2<f32>,
    texel: f32,
    ref_y: f32,
    range: f32,
    damping: f32,
    pad0: f32,
    pad1: f32,
};
@group(0) @binding(0) var<uniform> st: Step;
@group(0) @binding(1) var<storage, read> src: array<vec2<f32>>;
@group(0) @binding(2) var<storage, read_write> dst: array<vec2<f32>>;
@group(0) @binding(3) var surface: texture_depth_2d;
@group(0) @binding(4) var<storage, read> splats: array<vec4<f32>>;

fn wet(i: i32, j: i32) -> bool {
    let n = i32(st.n);
    if (i < 0 || j < 0 || i >= n || j >= n) { return false; }
    return textureLoad(surface, vec2<i32>(i, j), 0) < 0.99999;
}

// Last state at this window's cell (i, j) (the window may have moved since).
fn at(i: i32, j: i32) -> vec2<f32> {
    let n = i32(st.n);
    let si = i + st.shift.x;
    let sj = j + st.shift.y;
    if (si < 0 || sj < 0 || si >= n || sj >= n) { return vec2<f32>(0.0); }
    return src[u32(sj * n + si)];
}

// A neighbour's height; dry cells mirror the centre (waves reflect off the shore).
fn nb(i: i32, j: i32, c: f32) -> f32 {
    if (!wet(i, j)) { return c; }
    return at(i, j).x;
}

@compute @workgroup_size(8, 8)
fn step(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= st.n || id.y >= st.n) { return; }
    let i = i32(id.x);
    let j = i32(id.y);
    let idx = id.y * st.n + id.x;
    let d = textureLoad(surface, vec2<i32>(i, j), 0);
    if (d >= 0.99999) {
        dst[idx] = vec2<f32>(0.0);
        return;
    }
    let c = at(i, j);
    let sum = nb(i + 1, j, c.x) + nb(i - 1, j, c.x) + nb(i, j + 1, c.x) + nb(i, j - 1, c.x);
    var h = (2.0 * c.x - c.y + 0.3 * (sum - 4.0 * c.x)) * st.damping;
    let surf_y = st.ref_y + (0.5 - d) * st.range;
    let wp = st.origin + (vec2<f32>(f32(i), f32(j)) + 0.5) * st.texel;
    for (var k = 0u; k < st.nsplat; k++) {
        let a = splats[k * 2u];
        let b = splats[k * 2u + 1u];
        if (abs(a.y - surf_y) < a.w + 12.0) {
            let dist = distance(wp, a.xz);
            if (dist < a.w) {
                h -= b.x * (0.5 + 0.5 * cos(3.14159265 * dist / a.w));
            }
        }
    }
    dst[idx] = vec2<f32>(clamp(h, -24.0, 24.0), c.x);
}
"#;

/// Where the simulation window is and how far it stepped this frame.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct WaterFrame {
    /// World x / z of the window's corner (cell (0, 0)'s corner).
    pub origin: [f32; 2],
    /// Height the surface map's depth is measured from.
    pub ref_y: f32,
    /// Steps to run this frame (even; 0 = the surface map and state stay as they are).
    pub steps: u32,
    /// Seconds (the host's water clock), for the procedural ripples and caustics.
    pub time: f32,
}

/// The simulation's GPU state.
pub(crate) struct WaterSim {
    bufs: [wgpu::Buffer; 2],
    steps: wgpu::Buffer,
    splats: wgpu::Buffer,
    pub surface_view: wgpu::TextureView,
    pipeline: wgpu::ComputePipeline,
    /// A→B and B→A.
    groups: [wgpu::BindGroup; 2],
    /// Window corner in cells (`None`: no state yet).
    cell: Option<[i32; 2]>,
    ref_y: f32,
    last_time: Option<f64>,
    acc: f64,
    pub frame: WaterFrame,
}

impl WaterSim {
    pub fn new(device: &wgpu::Device) -> WaterSim {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("water simulation"), source: wgpu::ShaderSource::Wgsl(SIM.into()) });
        let cells = (N * N) as u64;
        let buf = |label| {
            device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: cells * 8, usage: wgpu::BufferUsages::STORAGE, mapped_at_creation: false })
        };
        let bufs = [buf("water heights A"), buf("water heights B")];
        let steps = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water steps"),
            size: STEP_STRIDE * MAX_STEPS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let splats = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water splashes"),
            size: (MAX_SPLATS * 32) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let surface = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("water surface map"),
            size: wgpu::Extent3d { width: N, height: N, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let surface_view = surface.create_view(&Default::default());
        let c = wgpu::ShaderStages::COMPUTE;
        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: c,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water simulation"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: c,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(48),
                    },
                    count: None,
                },
                storage(1, true),
                storage(2, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: c,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                storage(4, true),
            ],
        });
        let group = |a: &wgpu::Buffer, b: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("water step"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &steps, offset: 0, size: wgpu::BufferSize::new(48) }),
                    },
                    wgpu::BindGroupEntry { binding: 1, resource: a.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: b.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&surface_view) },
                    wgpu::BindGroupEntry { binding: 4, resource: splats.as_entire_binding() },
                ],
            })
        };
        let groups = [group(&bufs[0], &bufs[1]), group(&bufs[1], &bufs[0])];
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("water simulation"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("water simulation"),
            layout: Some(&pl),
            module: &shader,
            entry_point: Some("step"),
            compilation_options: Default::default(),
            cache: None,
        });
        WaterSim { bufs, steps, splats, surface_view, pipeline, groups, cell: None, ref_y: 0.0, last_time: None, acc: 0.0, frame: WaterFrame::default() }
    }

    /// Whether the simulation has a state (its first frame was planned).
    pub fn started(&self) -> bool {
        self.cell.is_some()
    }

    /// The buffer holding the current state (`vec2(height, last height)` per cell).
    pub fn heights(&self) -> &wgpu::Buffer {
        &self.bufs[0]
    }

    /// Plans this frame: how many steps the clock `time` (seconds) allows, the window around
    /// `centre`, and the parameters and `splashes` of the steps. Returns whether the splashes
    /// were used (no steps this frame: the caller keeps them for the next).
    pub fn plan(&mut self, queue: &wgpu::Queue, centre: [f32; 3], time: f64, splashes: &[Splash]) -> bool {
        let dt = match self.last_time {
            Some(t) if time >= t => (time - t).min(0.25),
            _ => 0.0,
        };
        self.last_time = Some(time);
        self.acc += dt;
        let pair = 2.0 / STEP_HZ;
        let mut pairs = (self.acc / pair).floor() as u32;
        if pairs > MAX_STEPS / 2 {
            pairs = MAX_STEPS / 2;
            self.acc = 0.0;
        } else {
            self.acc -= pairs as f64 * pair;
        }
        let first = self.cell.is_none();
        if first {
            // The first frame draws a surface map to start from.
            pairs = pairs.max(1);
        }
        let steps = pairs * 2;
        self.frame.time = time as f32;
        self.frame.steps = steps;
        if steps == 0 {
            return false;
        }
        let half = (N / 2) as i32;
        let cell = [(centre[0] / TEXEL).floor() as i32 - half, (centre[2] / TEXEL).floor() as i32 - half];
        // Far moves (a new level, a camera cut): start over.
        let shift = match self.cell {
            Some(c) if (cell[0] - c[0]).abs() < N as i32 && (cell[1] - c[1]).abs() < N as i32 => [cell[0] - c[0], cell[1] - c[1]],
            _ => [N as i32, N as i32],
        };
        if (centre[1] - self.ref_y).abs() > RANGE * 0.25 || first {
            self.ref_y = centre[1];
        }
        self.cell = Some(cell);
        let origin = [cell[0] as f32 * TEXEL, cell[1] as f32 * TEXEL];
        self.frame.origin = origin;
        self.frame.ref_y = self.ref_y;
        let n = splashes.len().min(MAX_SPLATS);
        let mut sp = vec![0.0f32; MAX_SPLATS * 8];
        for (k, s) in splashes.iter().take(n).enumerate() {
            sp[k * 8..k * 8 + 5].copy_from_slice(&[s.pos[0], s.pos[1], s.pos[2], s.radius, s.strength]);
        }
        queue.write_buffer(&self.splats, 0, bytemuck::cast_slice(&sp));
        let mut u = vec![0u8; (STEP_STRIDE * MAX_STEPS as u64) as usize];
        for k in 0..steps as usize {
            let (sh, ns) = if k == 0 { (shift, n as u32) } else { ([0, 0], 0) };
            let mut w = Vec::with_capacity(48);
            w.extend(bytemuck::cast_slice::<i32, u8>(&sh));
            w.extend(bytemuck::bytes_of(&ns));
            w.extend(bytemuck::bytes_of(&N));
            w.extend(bytemuck::cast_slice::<f32, u8>(&[origin[0], origin[1], TEXEL, self.ref_y, RANGE, 0.994, 0.0, 0.0]));
            let at = k * STEP_STRIDE as usize;
            u[at..at + 48].copy_from_slice(&w);
        }
        queue.write_buffer(&self.steps, 0, &u);
        true
    }

    /// Runs this frame's steps (after the surface map is drawn).
    pub fn run(&self, encoder: &mut wgpu::CommandEncoder) {
        if self.frame.steps == 0 {
            return;
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("water simulation"), timestamp_writes: None });
        pass.set_pipeline(&self.pipeline);
        let groups = N.div_ceil(8);
        for k in 0..self.frame.steps {
            pass.set_bind_group(0, &self.groups[(k % 2) as usize], &[(k as u64 * STEP_STRIDE) as u32]);
            pass.dispatch_workgroups(groups, groups, 1);
        }
    }
}
