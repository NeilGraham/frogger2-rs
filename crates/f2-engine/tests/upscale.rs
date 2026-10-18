//! Headless GPU test of the upscaler inputs (`Renderer::render_upscaled`): a moving quad
//! writes its motion vectors and depth, the background stays still and empty.

use f2_engine::upscale::{UpscaleInputs, Upscaler};
use f2_engine::{Actor, Camera, GpuContext, Renderer};
use f2_formats::objbank::{Mesh, Model, Object, QKey, VKey};
use f2_formats::texture::TexType;

fn quad_model(tex: u32) -> Model {
    let h = 20.0;
    let mesh = Mesh {
        vertices: vec![[-h, -h, 0.0], [h, -h, 0.0], [h, h, 0.0], [-h, h, 0.0]],
        faces: vec![[0, 2, 1], [0, 3, 2]],
        face_uv: vec![[0, 0]; 6],
        normals: vec![],
        colors: vec![[1.0, 0.0, 0.0, 1.0]; 6],
        face_flags: vec![0; 2],
        textures: vec![tex; 2],
    };
    let root = Object {
        id: 0,
        name: "root".into(),
        xlu: 255,
        sprites: vec![],
        mesh: Some(mesh),
        effected_verts: vec![],
        scale_keys: vec![VKey { v: [1.0; 3], time: 0 }],
        move_keys: vec![VKey { v: [0.0; 3], time: 0 }],
        rotate_keys: vec![QKey { q: [0.0, 0.0, 0.0, 1.0], time: 0 }],
        bbox: None,
        phong: 0,
        flags: 0,
        matrix: f2_engine::math::IDENTITY,
        children: vec![],
    };
    Model { id: 1, radius: 30.0, object_size: 0, root, root_siblings: vec![], animations: vec![], loc_mesh: None, is_skinned: false, num_vtx: 0 }
}

/// Reads back one texel row-major texture as f32 channels.
fn read(ctx: &GpuContext, t: &wgpu::Texture, texel_bytes: u32, f: impl Fn(&[u8]) -> Vec<f32>) -> Vec<Vec<f32>> {
    let (w, h) = (t.width(), t.height());
    let bpr = (w * texel_bytes).div_ceil(256) * 256;
    let buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (bpr * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: t, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(h) } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    ctx.queue.submit([enc.finish()]);
    let s = buf.slice(..);
    s.map_async(wgpu::MapMode::Read, |_| {});
    ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let d = s.get_mapped_range().unwrap();
    let mut out = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let o = (y * bpr + x * texel_bytes) as usize;
            out.push(f(&d[o..o + texel_bytes as usize]));
        }
    }
    out
}

fn f16(b: &[u8]) -> f32 {
    let h = u16::from_le_bytes([b[0], b[1]]);
    let s = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let e = ((h >> 10) & 0x1f) as i32;
    let m = (h & 0x3ff) as f32;
    if e == 0 {
        s * m * 2f32.powi(-24)
    } else {
        s * (1.0 + m / 1024.0) * 2f32.powi(e - 15)
    }
}

/// Copies the inputs aside (the test reads them after the frame).
struct Grab<'a> {
    ctx: &'a GpuContext,
    motion: Vec<Vec<f32>>,
    depth: Vec<Vec<f32>>,
    size: (u32, u32),
    reset: bool,
}

impl Upscaler for Grab<'_> {
    fn upscale(&mut self, _d: &wgpu::Device, _q: &wgpu::Queue, i: &UpscaleInputs) -> bool {
        self.motion = read(self.ctx, i.motion, 4, |b| vec![f16(&b[0..2]), f16(&b[2..4])]);
        self.depth = read(self.ctx, i.depth, 4, |b| vec![f32::from_le_bytes(b.try_into().unwrap())]);
        self.size = i.render_size;
        self.reset = i.reset;
        false
    }
}

#[test]
fn moving_quad_writes_motion_and_depth() {
    moving_quad(false);
}

/// The same from the GPU mesh renderer (`RenderOptions::gpu_meshes`).
#[test]
fn gpu_meshes_write_motion_and_depth() {
    moving_quad(true);
}

fn moving_quad(gpu: bool) {
    let ctx = GpuContext::new_headless().expect("no GPU adapter");
    let mut r = Renderer::new(&ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm);
    r.options.gpu_meshes = gpu;
    r.options.perspective_correct = true;
    let white = f2_formats::crc("white.bmp");
    r.textures.add_rgba(white, "white.bmp", TexType::Normal, 2, 2, vec![255; 16]);
    let model = quad_model(white);
    let target = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: 320, height: 240, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut actor = Actor::default();
    let mut grab = Grab { ctx: &ctx, motion: vec![], depth: vec![], size: (0, 0), reset: false };
    let cam = Camera { source: [0.0, 0.0, -100.0], target: [0.0; 3], up: [0.0, 1.0, 0.0] };
    for (frame, x) in [(0, 0.0), (1, 4.0)] {
        r.options.motion_vectors = true;
        r.upscale_render = Some((160, 120));
        r.begin_frame(320, 240);
        r.set_camera(&cam);
        actor.pos = [x, 0.0, 0.0];
        r.draw_actor(&model, &mut actor);
        r.render_upscaled(&view, 320, 240, [0.0, 0.0, 0.0, 1.0], &mut grab);
        assert_eq!(grab.size, (160, 120));
        assert_eq!(grab.reset, frame == 0, "frame {frame}: history reset");
    }
    let at = |v: &Vec<Vec<f32>>, x: u32, y: u32| v[(y * 160 + x) as usize].clone();
    // Centre: the quad. Depth = near / (z - 10) with the quad 100 in front of the camera.
    let d = at(&grab.depth, 80, 60)[0];
    assert!((d - 10.0 / 90.0).abs() < 1e-3, "depth {d}");
    assert_eq!(at(&grab.depth, 2, 2)[0], 0.0, "background depth");
    // It moved +4 world x = left on screen (MDX projection): its previous position is to the
    // right, by 4 * 450 * (160/640) / 90 = 5 render pixels.
    let m = at(&grab.motion, 80, 60);
    assert!((m[0] - 5.0).abs() < 0.2 && m[1].abs() < 0.05, "motion {m:?}");
    assert_eq!(at(&grab.motion, 2, 2), vec![0.0, 0.0], "background motion");

    // The debug view shows the depth: grey where the quad is, black around it.
    r.upscale_debug = f2_engine::UpscaleDebug::Depth;
    r.begin_frame(320, 240);
    r.set_camera(&cam);
    r.draw_actor(&model, &mut actor);
    r.render_upscaled(&view, 320, 240, [0.0, 0.0, 0.0, 1.0], &mut grab);
    let img = read(&ctx, &target, 4, |b| b.iter().map(|&c| c as f32).collect());
    let px = |x: u32, y: u32| img[(y * 320 + x) as usize].clone();
    let g = (255.0 * (10.0f32 / 90.0).sqrt()).round();
    assert!((px(160, 120)[0] - g).abs() <= 1.0, "debug depth {:?}", px(160, 120));
    assert_eq!(px(4, 4)[0], 0.0);
}

/// A static quad under a moving camera: its motion is where the last frame's camera drew it.
#[test]
fn camera_motion_reprojects_static_geometry() {
    let ctx = GpuContext::new_headless().expect("no GPU adapter");
    let mut r = Renderer::new(&ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm);
    let white = f2_formats::crc("white.bmp");
    r.textures.add_rgba(white, "white.bmp", TexType::Normal, 2, 2, vec![255; 16]);
    let model = quad_model(white);
    let target = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: 160, height: 120, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut grab = Grab { ctx: &ctx, motion: vec![], depth: vec![], size: (0, 0), reset: false };
    let cams = [
        Camera { source: [0.0, 0.0, -100.0], target: [0.0; 3], up: [0.0, 1.0, 0.0] },
        Camera { source: [3.0, 2.0, -100.0], target: [3.0, 2.0, 0.0], up: [0.0, 1.0, 0.0] },
    ];
    for cam in &cams {
        r.options.motion_vectors = true;
        r.upscale_render = None;
        r.begin_frame(160, 120);
        r.set_camera(cam);
        r.draw_landscape(&model);
        r.render_upscaled(&view, 160, 120, [0.0, 0.0, 0.0, 1.0], &mut grab);
    }
    assert!(!grab.reset);
    // Expected: the quad centre through both cameras.
    let screen = |cam: &Camera| {
        let mut v = f2_engine::View::new(160, 120);
        v.set_camera(cam);
        let p = f2_engine::math::mtx_xfm(&v.v_matrix, [0.0, 0.0, 0.0]);
        v.project(p).unwrap()
    };
    let (a, b) = (screen(&cams[0]), screen(&cams[1]));
    let want = [a[0] - b[0], a[1] - b[1]];
    let (x, y) = (b[0].round() as u32, b[1].round() as u32);
    let m = grab.motion[(y * 160 + x) as usize].clone();
    assert!(want[0].abs() > 1.0 && want[1].abs() > 1.0, "{want:?}");
    assert!((m[0] - want[0]).abs() < 0.1 && (m[1] - want[1]).abs() < 0.1, "motion {m:?}, want {want:?}");
}
