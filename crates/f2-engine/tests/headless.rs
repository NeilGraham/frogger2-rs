//! Headless GPU test: renders a synthetic model + overlay and checks pixels.

use f2_engine::scene::OBJECT_FLAGS_XLU;
use f2_engine::{Actor, Blend, Camera, GpuContext, Renderer};
use f2_formats::objbank::{Mesh, Model, Object, QKey, VKey};
use f2_formats::texture::TexType;

fn quad(z: f32, half: f32, color: [f32; 4], tex: u32, front: bool) -> Mesh {
    let vertices = vec![[-half, -half, z], [half, -half, z], [half, half, z], [-half, half, z]];
    // Seen from a camera on -z looking +z, (0,2,1)/(0,3,2) are counter-clockwise → visible.
    let faces = if front { vec![[0, 2, 1], [0, 3, 2]] } else { vec![[0, 1, 2], [0, 2, 3]] };
    Mesh { vertices, faces, face_uv: vec![[0, 0]; 6], normals: vec![], colors: vec![color; 6], face_flags: vec![0; 2], textures: vec![tex; 2] }
}

fn object(name: &str, mesh: Mesh, flags: i16) -> Object {
    Object {
        id: 0,
        name: name.into(),
        xlu: 255,
        sprites: vec![],
        mesh: Some(mesh),
        effected_verts: vec![],
        scale_keys: vec![VKey { v: [1.0; 3], time: 0 }],
        move_keys: vec![VKey { v: [0.0; 3], time: 0 }],
        rotate_keys: vec![QKey { q: [0.0, 0.0, 0.0, 1.0], time: 0 }],
        bbox: None,
        phong: 0,
        flags,
        matrix: f2_engine::math::IDENTITY,
        children: vec![],
    }
}

#[test]
fn renders_synthetic_scene_headless() {
    let ctx = GpuContext::new_headless().expect("no GPU adapter (headless rendering must work, e.g. Vulkan llvmpipe)");
    eprintln!("adapter: {:?}", ctx.adapter.get_info());
    let mut r = Renderer::new(&ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm);
    let white = f2_formats::crc("white.bmp");
    r.textures.add_rgba(white, "white.bmp", TexType::Normal, 2, 2, vec![255; 16]);

    // Root: opaque red quad at z=0. Children: a back-facing green quad in front
    // (must be culled) and a 50% white XLU quad over the left half.
    let mut root = object("root", quad(0.0, 20.0, [1.0, 0.0, 0.0, 1.0], white, true), 0);
    root.children.push(object("back", quad(-10.0, 20.0, [0.0, 1.0, 0.0, 1.0], white, false), 0));
    let mut xlu = object("xlu", quad(-5.0, 10.0, [1.0, 1.0, 1.0, 0.5], white, true), OBJECT_FLAGS_XLU);
    xlu.move_keys[0].v = [15.0, 0.0, 0.0];
    root.children.push(xlu);
    let model = Model { id: 1, radius: 30.0, object_size: 0, root, root_siblings: vec![], animations: vec![], loc_mesh: None, is_skinned: false, num_vtx: 0 };

    r.begin_frame(320, 240);
    r.set_camera(&Camera { source: [0.0, 0.0, -100.0], target: [0.0; 3], up: [0.0, 1.0, 0.0] });
    r.draw_actor(&model, &mut Actor::default());
    // 2D overlay: blue box in the top-left 512×512 of the 4096 space.
    r.draw_sprite_overlay(0.0, 0.0, 512.0, 512.0, [0.0, 0.0, 1.0, 1.0], None, [0, 0, 255, 255], Blend::Semi, 0.0);
    let img = r.render_to_image([0.0, 0.0, 0.0, 1.0]).unwrap();
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/f2-engine-headless.png");
    img.write_png(&out).unwrap();

    let px = |x, y| img.pixel(x, y);
    // Centre: red (the green back face was culled).
    assert_eq!(px(160, 120), [255, 0, 0, 255], "centre");
    // Background corner.
    assert_eq!(px(300, 230), [0, 0, 0, 255]);
    // Overlay box: 512/4096 * 320 = 40 px wide.
    assert_eq!(px(10, 10), [0, 0, 255, 255]);
    assert_eq!(px(45, 10), [0, 0, 0, 255]);
    // World +x appears on the screen's left (MDX projection): the XLU quad at
    // x = 15 overlaps the red quad around world x = 12: 50% white over red.
    let k = 450.0 * (320.0 / 640.0) / (95.0 - 10.0);
    let sx = (160.0 - 12.0 * k) as u32;
    let p = px(sx, 120);
    assert!(p[0] == 255 && (110..=145).contains(&p[1]) && p[1] == p[2], "xlu blend {p:?} at {sx}");
}

/// Enhanced display options: Hor+, overlay re-anchoring, ultrawide clamp, MSAA and
/// supersampling.
#[test]
fn enhanced_options_render() {
    let ctx = GpuContext::new_headless().expect("no GPU adapter");
    let mut r = Renderer::new(&ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm);
    let white = f2_formats::crc("white.bmp");
    r.textures.add_rgba(white, "white.bmp", TexType::Normal, 2, 2, vec![255; 16]);
    let root = object("root", quad(0.0, 20.0, [1.0, 0.0, 0.0, 1.0], white, true), 0);
    let model = Model { id: 1, radius: 30.0, object_size: 0, root, root_siblings: vec![], animations: vec![], loc_mesh: None, is_skinned: false, num_vtx: 0 };
    let cam = Camera { source: [0.0, 0.0, -100.0], target: [0.0; 3], up: [0.0, 1.0, 0.0] };
    let frame = |r: &mut Renderer, w: u32, h: u32| {
        r.begin_frame(w, h);
        r.set_camera(&cam);
        r.draw_actor(&model, &mut Actor::default());
        // Left-anchored, centred and right-anchored 256×256 boxes.
        for x in [0.0, 1920.0, 3840.0] {
            r.draw_sprite_overlay(x, 0.0, 256.0, 256.0, [0.0, 0.0, 1.0, 1.0], None, [0, 0, 255, 255], Blend::Semi, 0.0);
        }
        r.render_to_image([0.0, 0.0, 0.0, 1.0]).unwrap()
    };
    let red_w = |img: &f2_engine::RgbaImage| (0..img.width).filter(|&x| img.pixel(x, img.height / 2)[0] == 255).count();

    // Authentic at 16:9: overlays stretched over the whole width, Vert- projection.
    let a = frame(&mut r, 640, 360);
    assert_eq!(a.pixel(639, 2), [0, 0, 255, 255]);
    assert_eq!(a.pixel(2, 2), [0, 0, 255, 255]);

    // Hor+ and anchoring: boxes keep their 4:3 size (safe width 480 → 30 px), the left and
    // right ones at the screen edges, the centre one centred.
    r.options.hor_plus = true;
    r.options.hud_anchor = true;
    let e = frame(&mut r, 640, 360);
    assert_eq!(e.pixel(2, 2), [0, 0, 255, 255]);
    assert_eq!(e.pixel(637, 2), [0, 0, 255, 255]);
    assert_eq!(e.pixel(605, 2), [0, 0, 0, 255], "right box is 30 px wide");
    assert_eq!(e.pixel(320, 2), [0, 0, 255, 255], "centred box");
    // Hor+ scale 360/480 vs Vert- 640/640: the red quad is narrower.
    assert!(red_w(&e) < red_w(&a), "Hor+ {} vs Vert- {}", red_w(&e), red_w(&a));

    // Ultrawide clamp to 4:3: black pillars, scene in the middle.
    r.options.max_aspect = 4.0 / 3.0;
    let u = frame(&mut r, 640, 360);
    assert_eq!(u.pixel(20, 200), [0, 0, 0, 255]);
    assert_eq!(u.pixel(320, 180)[0], 255);

    // MSAA + 2× supersampling + anisotropic: output-sized image, same layout, smooth edges.
    r.options.max_aspect = 0.0;
    r.options.msaa = 4;
    r.options.render_scale = 2.0;
    r.options.anisotropy = 16;
    let m = frame(&mut r, 640, 360);
    assert_eq!((m.width, m.height), (640, 360));
    assert_eq!(m.pixel(320, 180), [255, 0, 0, 255]);
    assert_eq!(m.pixel(2, 2), [0, 0, 255, 255]);
    let edge = (0..640).map(|x| m.pixel(x, 180)[0]).filter(|&c| c > 0 && c < 255).count();
    assert!(edge > 0, "antialiased edges expected");
}

/// Extended culling (`RenderOptions::clip_faces`): a floor running from behind the camera into
/// the distance is clipped at the near plane and drawn; the original drops every face of it
/// that has a corner behind the near plane.
#[test]
fn clip_faces_draws_polygons_crossing_the_near_plane() {
    let ctx = GpuContext::new_headless().expect("no GPU adapter");
    let mut r = Renderer::new(&ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm);
    let white = f2_formats::crc("white.bmp");
    r.textures.add_rgba(white, "white.bmp", TexType::Normal, 2, 2, vec![255; 16]);
    // One big floor quad, both windings (whichever faces the camera is drawn).
    let (h, y) = (200.0, -20.0);
    let vertices = vec![[-h, y, -150.0], [h, y, -150.0], [h, y, 300.0], [-h, y, 300.0]];
    let mesh = Mesh {
        vertices,
        faces: vec![[0, 1, 2], [0, 2, 3], [0, 2, 1], [0, 3, 2]],
        face_uv: vec![[0, 0]; 12],
        normals: vec![],
        colors: vec![[1.0, 0.0, 0.0, 1.0]; 12],
        face_flags: vec![0; 4],
        textures: vec![white; 4],
    };
    let model = Model {
        id: 1,
        radius: 400.0,
        object_size: 0,
        root: object("floor", mesh, 0),
        root_siblings: vec![],
        animations: vec![],
        loc_mesh: None,
        is_skinned: false,
        num_vtx: 0,
    };
    let mut red = |clip: bool| {
        r.options.clip_faces = clip;
        r.begin_frame(320, 240);
        r.set_camera(&Camera { source: [0.0, 0.0, -100.0], target: [0.0, -10.0, 0.0], up: [0.0, 1.0, 0.0] });
        r.draw_landscape(&model);
        let img = r.render_to_image([0.0, 0.0, 0.0, 1.0]).unwrap();
        img.pixels.chunks(4).filter(|p| p[0] > 200 && p[1] < 50).count()
    };
    let (original, clipped) = (red(false), red(true));
    assert_eq!(original, 0, "the original drops faces with a corner behind the near plane");
    assert!(clipped > 320 * 240 / 4, "clipped floor covers the bottom of the screen: {clipped} px");
}

/// The GPU mesh renderer (`RenderOptions::gpu_meshes`) draws the same picture as the CPU
/// path: the opaque quad from the GPU, its back-facing child culled, the translucent quad
/// still from the CPU blended over it.
#[test]
fn gpu_meshes_match_the_cpu_path() {
    let ctx = GpuContext::new_headless().expect("no GPU adapter");
    let mut r = Renderer::new(&ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm);
    let white = f2_formats::crc("white.bmp");
    r.textures.add_rgba(white, "white.bmp", TexType::Normal, 2, 2, vec![255; 16]);
    let mut root = object("root", quad(0.0, 20.0, [1.0, 0.0, 0.0, 1.0], white, true), 0);
    root.children.push(object("back", quad(-10.0, 20.0, [0.0, 1.0, 0.0, 1.0], white, false), 0));
    let mut xlu = object("xlu", quad(-5.0, 10.0, [1.0, 1.0, 1.0, 0.5], white, true), OBJECT_FLAGS_XLU);
    xlu.move_keys[0].v = [15.0, 0.0, 0.0];
    root.children.push(xlu);
    let model = Model { id: 1, radius: 30.0, object_size: 0, root, root_siblings: vec![], animations: vec![], loc_mesh: None, is_skinned: false, num_vtx: 0 };
    let mut shot = |gpu: bool| {
        r.options.gpu_meshes = gpu;
        r.options.perspective_correct = true;
        r.begin_frame(320, 240);
        r.set_camera(&Camera { source: [0.0, 0.0, -100.0], target: [0.0; 3], up: [0.0, 1.0, 0.0] });
        r.draw_actor(&model, &mut Actor::default());
        r.render_to_image([0.0, 0.0, 0.0, 1.0]).unwrap()
    };
    let (cpu, gpu) = (shot(false), shot(true));
    gpu.write_png(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/f2-engine-gpu-meshes.png")).unwrap();
    let differ = cpu.pixels.chunks(4).zip(gpu.pixels.chunks(4)).filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| x.abs_diff(*y) > 8)).count();
    // Edge pixels may round differently; the picture must be the same.
    assert!(differ < 320 * 2, "{differ} pixels differ");
    assert_eq!(gpu.pixel(160, 120), [255, 0, 0, 255], "centre: the GPU's red quad, back face culled");
}

#[test]
fn gpu_mesh_lighting_balances_and_casts_shadows() {
    use f2_engine::{DirLight, Lighting};
    let ctx = GpuContext::new_headless().expect("no GPU adapter");
    let mut r = Renderer::new(&ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm);
    let white = f2_formats::crc("white.bmp");
    r.textures.add_rgba(white, "white.bmp", TexType::Normal, 2, 2, vec![255; 16]);
    // A grey floor facing the camera and a small card in front of it.
    let mut root = object("floor", quad(20.0, 40.0, [0.5, 0.5, 0.5, 1.0], white, true), 0);
    root.children.push(object("card", quad(0.0, 5.0, [0.5, 0.5, 0.5, 1.0], white, true), 0));
    let model = Model { id: 2, radius: 60.0, object_size: 0, root, root_siblings: vec![], animations: vec![], loc_mesh: None, is_skinned: false, num_vtx: 0 };
    let mut shot = |lighting: Option<Lighting>| {
        r.options.gpu_meshes = true;
        r.options.perspective_correct = true;
        r.lighting = lighting;
        r.begin_frame(320, 240);
        r.set_camera(&Camera { source: [0.0, 0.0, -100.0], target: [0.0; 3], up: [0.0, 1.0, 0.0] });
        r.draw_actor(&model, &mut Actor::default());
        r.render_to_image([0.0, 0.0, 0.0, 1.0]).unwrap()
    };
    let unlit = shot(None);
    let flat = Lighting {
        sky: [1.0; 3],
        ground: [1.0; 3],
        sun: None,
        point_lights: vec![],
        emissive_lights: false,
        specular: 0.0,
        water: false,
        baked: 1.0,
        ..Lighting::daylight()
    };
    let balanced = shot(Some(flat.clone()));
    assert_eq!(unlit.pixel(160, 120), balanced.pixel(160, 120), "ambient 1 = the baked colours");
    let sun = |shadows| Lighting {
        sky: [0.5; 3],
        ground: [0.5; 3],
        sun: Some(DirLight { to_light: f2_engine::lighting::normalize([1.0, 0.0, -1.0]), color: [0.6; 3], shadows }),
        ..flat.clone()
    };
    let (open, shaded) = (shot(Some(sun(false))), shot(Some(sun(true))));
    shaded.write_png(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/f2-engine-gpu-shadow.png")).unwrap();
    let darker = open.pixels.chunks(4).zip(shaded.pixels.chunks(4)).filter(|(a, b)| a[0] > b[0] + 20).count();
    // The card's shadow: about 10×10 units on the floor (~ 15×15 px here).
    assert!((80..1200).contains(&darker), "{darker} pixels in shadow");
}

#[test]
fn enhanced_water_takes_splashes() {
    use f2_engine::scene::{OBJECT_FLAGS_ADDITIVE, OBJECT_FLAGS_MODGE, OBJECT_FLAGS_WAVE};
    use f2_engine::water::Splash;
    let ctx = GpuContext::new_headless().expect("no GPU adapter");
    let mut r = Renderer::new(&ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm);
    let wat = f2_formats::crc("wat0.bmp");
    r.textures.add_rgba(wat, "wat0.bmp", TexType::Normal, 2, 2, vec![60, 90, 160, 200].repeat(4));
    // A horizontal pond (both windings), 200 units across at y = 0.
    let h = 100.0;
    let vertices = vec![[-h, 0.0, -h], [h, 0.0, -h], [h, 0.0, h], [-h, 0.0, h]];
    let faces = vec![[0, 2, 1], [0, 3, 2], [0, 1, 2], [0, 2, 3]];
    let mesh = Mesh { vertices, faces, face_uv: vec![[0, 0]; 12], normals: vec![], colors: vec![[1.0; 4]; 12], face_flags: vec![0; 4], textures: vec![wat; 4] };
    let root = object("pond", mesh, OBJECT_FLAGS_MODGE | OBJECT_FLAGS_WAVE | OBJECT_FLAGS_ADDITIVE);
    let model = Model { id: 3, radius: 150.0, object_size: 0, root, root_siblings: vec![], animations: vec![], loc_mesh: None, is_skinned: false, num_vtx: 0 };
    let lighting = f2_engine::Lighting { water: true, ..f2_engine::Lighting::daylight() };
    let rt = r.ray_tracing_supported();
    let mut run = |splash: bool| {
        r.options.gpu_meshes = true;
        // Also the photon-traced caustics where the adapter has ray queries.
        r.options.ray_tracing = rt;
        r.lighting = Some(lighting.clone());
        let mut img = None;
        for f in 0..20 {
            r.water_clock = f as f64 / 60.0;
            r.begin_frame(320, 240);
            r.set_camera(&Camera { source: [0.0, 120.0, -90.0], target: [0.0; 3], up: [0.0, 1.0, 0.0] });
            if splash && f == 3 {
                r.water_splash(Splash { pos: [0.0, 0.0, 0.0], radius: 25.0, strength: 6.0, ..Default::default() });
            }
            r.draw_actor(&model, &mut Actor { modge_frame: 0, ..Actor::default() });
            img = Some(r.render_to_image([0.0, 0.0, 0.0, 1.0]).unwrap());
        }
        img.unwrap()
    };
    let (calm, rippled) = (run(false), run(true));
    rippled.write_png(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/f2-engine-water.png")).unwrap();
    let lit = calm.pixels.chunks(4).filter(|p| p[2] > 30).count();
    assert!(lit > 320 * 240 / 4, "the pond is drawn ({lit} pixels)");
    let differ = calm.pixels.chunks(4).zip(rippled.pixels.chunks(4)).filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| x.abs_diff(*y) > 6)).count();
    assert!(differ > 100, "the splash makes waves ({differ} pixels differ)");
}

/// Water whose texture slides (`Actor::slide`) is a stream: its generated surface's ripples
/// and detail are carried along v (the flow map), where the same pond standing still keeps
/// them in place.
#[test]
fn sliding_water_is_a_stream() {
    use f2_engine::scene::{OBJECT_FLAGS_ADDITIVE, OBJECT_FLAGS_MODGE};
    let ctx = GpuContext::new_headless().expect("no GPU adapter");
    let mut r = Renderer::new(&ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm);
    let wat = f2_formats::crc("wat0.bmp");
    // Stripes across the flow, so the detail shows where it is carried.
    let tex: Vec<u8> = (0..16 * 16).flat_map(|i| if (i / 16) / 4 % 2 == 0 { [40, 70, 150, 200] } else { [120, 170, 230, 200] }).collect();
    r.textures.add_rgba(wat, "wat0.bmp", TexType::Normal, 16, 16, tex);
    let h = 100.0;
    let vertices = vec![[-h, 0.0, -h], [h, 0.0, -h], [h, 0.0, h], [-h, 0.0, h]];
    let faces = [[0, 2, 1], [0, 3, 2], [0, 1, 2], [0, 2, 3]];
    // v runs along z: two repeats across the pond.
    let uv = |v: [usize; 3]| v.map(|k| [0i16, if k == 2 || k == 3 { 2048 } else { 0 }]);
    let face_uv = faces.iter().flat_map(|&f: &[usize; 3]| uv(f)).collect();
    let faces: Vec<[i16; 3]> = faces.iter().map(|f| f.map(|k| k as i16)).collect();
    let mesh = Mesh { vertices, faces, face_uv, normals: vec![], colors: vec![[1.0; 4]; 12], face_flags: vec![0; 4], textures: vec![wat; 4] };
    let root = object("pond", mesh, OBJECT_FLAGS_MODGE | OBJECT_FLAGS_ADDITIVE);
    let model = Model { id: 5, radius: 150.0, object_size: 0, root, root_siblings: vec![], animations: vec![], loc_mesh: None, is_skinned: false, num_vtx: 0 };
    let lighting = f2_engine::Lighting { water: true, ..f2_engine::Lighting::daylight() };
    let mut run = |slide: f32| {
        r.options.gpu_meshes = true;
        r.lighting = Some(lighting.clone());
        let mut img = None;
        for f in 0..40 {
            r.water_clock = f as f64 / 60.0;
            r.begin_frame(320, 240);
            r.set_camera(&Camera { source: [0.0, 120.0, -90.0], target: [0.0; 3], up: [0.0, 1.0, 0.0] });
            r.draw_actor(&model, &mut Actor { slide, ..Actor::default() });
            img = Some(r.render_to_image([0.0, 0.0, 0.0, 1.0]).unwrap());
        }
        img.unwrap()
    };
    let (still, again, stream) = (run(0.0), run(0.0), run(16.0));
    let differ = |a: &f2_engine::RgbaImage, b: &f2_engine::RgbaImage| {
        a.pixels.chunks(4).zip(b.pixels.chunks(4)).filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| x.abs_diff(*y) > 4)).count()
    };
    assert_eq!(differ(&still, &again), 0, "the same frame twice");
    let moved = differ(&still, &stream);
    assert!(moved > 320 * 240 / 20, "the stream's surface is carried along ({moved} pixels differ)");
}

/// Standing water is silky smooth: with nothing disturbing it, it looks the same from one
/// moment to the next. A butterfly fluttering low over it ruffles it (`Wake::Flyer`).
#[test]
fn still_water_is_calm_and_a_butterfly_ruffles_it() {
    use f2_engine::anim::Wake;
    use f2_engine::scene::{OBJECT_FLAGS_ADDITIVE, OBJECT_FLAGS_MODGE};
    let ctx = GpuContext::new_headless().expect("no GPU adapter");
    let mut r = Renderer::new(&ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm);
    let wat = f2_formats::crc("wat0.bmp");
    r.textures.add_rgba(wat, "wat0.bmp", TexType::Normal, 2, 2, vec![60, 90, 160, 200].repeat(4));
    let h = 100.0;
    let vertices = vec![[-h, 0.0, -h], [h, 0.0, -h], [h, 0.0, h], [-h, 0.0, h]];
    let faces = vec![[0, 2, 1], [0, 3, 2], [0, 1, 2], [0, 2, 3]];
    let mesh = Mesh { vertices, faces, face_uv: vec![[0, 0]; 12], normals: vec![], colors: vec![[1.0; 4]; 12], face_flags: vec![0; 4], textures: vec![wat; 4] };
    let pond = Model {
        id: 6,
        radius: 150.0,
        object_size: 0,
        root: object("pond", mesh, OBJECT_FLAGS_MODGE | OBJECT_FLAGS_ADDITIVE),
        root_siblings: vec![],
        animations: vec![],
        loc_mesh: None,
        is_skinned: false,
        num_vtx: 0,
    };
    // A tiny butterfly (a sliver, drawn off to the side of the view's centre).
    let wing = Mesh {
        vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
        faces: vec![[0, 1, 2]],
        face_uv: vec![[0, 0]; 3],
        normals: vec![],
        colors: vec![[1.0; 4]; 3],
        face_flags: vec![0],
        textures: vec![0],
    };
    let fly = Model {
        id: 7,
        radius: 4.0,
        object_size: 0,
        root: object("bflyhrt", wing, 0),
        root_siblings: vec![],
        animations: vec![],
        loc_mesh: None,
        is_skinned: false,
        num_vtx: 0,
    };
    let lighting = f2_engine::Lighting { water: true, ..f2_engine::Lighting::daylight() };
    let mut run = |frames: u32, flyer: bool| {
        r.options.gpu_meshes = true;
        r.lighting = Some(lighting.clone());
        let mut img = None;
        let mut bfly = Actor { wake: Wake::Flyer, ..Actor::default() };
        for f in 0..frames {
            r.water_clock = f as f64 / 60.0;
            r.begin_frame(320, 240);
            r.set_camera(&Camera { source: [0.0, 120.0, -90.0], target: [0.0; 3], up: [0.0, 1.0, 0.0] });
            r.draw_actor(&pond, &mut Actor::default());
            if flyer {
                // Fluttering about 15 units over the water.
                let a = f as f32 * 0.05;
                bfly.pos = [a.cos() * 20.0, 15.0 + (f as f32 * 0.4).sin() * 3.0, a.sin() * 20.0];
                r.draw_actor(&fly, &mut bfly);
            }
            img = Some(r.render_to_image([0.0, 0.0, 0.0, 1.0]).unwrap());
        }
        img.unwrap()
    };
    let (early, late, ruffled) = (run(60, false), run(300, false), run(300, true));
    let differ = |a: &f2_engine::RgbaImage, b: &f2_engine::RgbaImage, by: u8| {
        a.pixels.chunks(4).zip(b.pixels.chunks(4)).filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| x.abs_diff(*y) > by)).count()
    };
    let moved = differ(&early, &late, 2);
    assert!(moved < 50, "standing water stays still ({moved} pixels changed in a second)");
    ruffled.write_png(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/f2-engine-butterfly.png")).unwrap();
    let rings = differ(&late, &ruffled, 1);
    assert!(rings > 20, "the butterfly ruffles the water ({rings} pixels differ)");
}

/// A floating log that bobs (the frog landing on it) pushes the water along its whole length:
/// the waves it makes are long like it, not a ring around its centre.
#[test]
fn a_bobbing_log_makes_long_waves() {
    use f2_engine::scene::{OBJECT_FLAGS_ADDITIVE, OBJECT_FLAGS_MODGE};
    let ctx = GpuContext::new_headless().expect("no GPU adapter");
    let mut r = Renderer::new(&ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm);
    let wat = f2_formats::crc("wat0.bmp");
    r.textures.add_rgba(wat, "wat0.bmp", TexType::Normal, 2, 2, vec![60, 90, 160, 200].repeat(4));
    let h = 100.0;
    let vertices = vec![[-h, 0.0, -h], [h, 0.0, -h], [h, 0.0, h], [-h, 0.0, h]];
    let faces = vec![[0, 2, 1], [0, 3, 2], [0, 1, 2], [0, 2, 3]];
    let mesh = Mesh { vertices, faces, face_uv: vec![[0, 0]; 12], normals: vec![], colors: vec![[1.0; 4]; 12], face_flags: vec![0; 4], textures: vec![wat; 4] };
    let pond = Model {
        id: 8,
        radius: 150.0,
        object_size: 0,
        root: object("pond", mesh, OBJECT_FLAGS_MODGE | OBJECT_FLAGS_ADDITIVE),
        root_siblings: vec![],
        animations: vec![],
        loc_mesh: None,
        is_skinned: false,
        num_vtx: 0,
    };
    // A log 80 units long (x), 10 wide, its top 2 units over the water (untextured: unseen).
    let (l, w) = (40.0, 5.0);
    let lv = vec![[-l, 2.0, -w], [l, 2.0, -w], [l, 2.0, w], [-l, 2.0, w]];
    let lm = Mesh {
        vertices: lv,
        faces: vec![[0, 2, 1], [0, 3, 2]],
        face_uv: vec![[0, 0]; 6],
        normals: vec![],
        colors: vec![[1.0; 4]; 6],
        face_flags: vec![0; 2],
        textures: vec![0; 2],
    };
    let log = Model {
        id: 9,
        radius: 41.0,
        object_size: 0,
        root: object("plt4log", lm, 0),
        root_siblings: vec![],
        animations: vec![],
        loc_mesh: None,
        is_skinned: false,
        num_vtx: 0,
    };
    let lighting = f2_engine::Lighting { water: true, ..f2_engine::Lighting::daylight() };
    let mut run = |bob: bool| {
        r.options.gpu_meshes = true;
        r.lighting = Some(lighting.clone());
        let mut img = None;
        let mut a = Actor::default();
        for f in 0..18 {
            r.water_clock = f as f64 / 60.0;
            r.begin_frame(320, 240);
            // Looking straight down.
            r.set_camera(&Camera { source: [0.0, 160.0, -1.0], target: [0.0; 3], up: [0.0, 1.0, 0.0] });
            r.draw_actor(&pond, &mut Actor::default());
            // Landed on at frame 5: it dips 2 units and bobs back (a damped spring).
            let t = (f as f32 - 5.0).max(0.0);
            a.pos = [0.0, if bob { -2.0 * (-t * 0.08).exp() * (t * 0.35).sin() } else { 0.0 }, 0.0];
            r.draw_actor(&log, &mut a);
            img = Some(r.render_to_image([0.0, 0.0, 0.0, 1.0]).unwrap());
        }
        img.unwrap()
    };
    let (calm, bobbed) = (run(false), run(true));
    bobbed.write_png(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/f2-engine-log.png")).unwrap();
    // Where the waves are: one crest's extent across the screen (x: along the log) and down it.
    let (mut xs, mut ys) = (Vec::new(), Vec::new());
    for (i, (a, b)) in calm.pixels.chunks(4).zip(bobbed.pixels.chunks(4)).enumerate() {
        // One crest: the half above the log's centre line.
        if i / 320 < 120 && a.iter().zip(b.iter()).any(|(x, y)| x.abs_diff(*y) > 5) {
            xs.push((i % 320) as i32);
            ys.push((i / 320) as i32);
        }
    }
    assert!(xs.len() > 60, "the log makes waves ({} pixels differ)", xs.len());
    let spread = |v: &mut Vec<i32>| {
        v.sort();
        v[v.len() * 9 / 10] - v[v.len() / 10]
    };
    let (sx, sy) = (spread(&mut xs), spread(&mut ys));
    assert!(sx as f32 > sy as f32 * 1.3, "the waves are long like the log ({sx} across, {sy} down)");
}

#[test]
fn ray_traced_lighting_casts_shadows() {
    use f2_engine::{DirLight, Lighting};
    let ctx = GpuContext::new_headless().expect("no GPU adapter");
    let mut r = Renderer::new(&ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm);
    if !r.ray_tracing_supported() {
        eprintln!("no ray queries on this adapter: skipped");
        return;
    }
    let white = f2_formats::crc("white.bmp");
    r.textures.add_rgba(white, "white.bmp", TexType::Normal, 2, 2, vec![255; 16]);
    let mut root = object("floor", quad(20.0, 40.0, [0.5, 0.5, 0.5, 1.0], white, true), 0);
    root.children.push(object("card", quad(0.0, 5.0, [0.5, 0.5, 0.5, 1.0], white, true), 0));
    let model = Model { id: 4, radius: 60.0, object_size: 0, root, root_siblings: vec![], animations: vec![], loc_mesh: None, is_skinned: false, num_vtx: 0 };
    let sun = |shadows| Lighting {
        sky: [0.5; 3],
        ground: [0.5; 3],
        sun: Some(DirLight { to_light: f2_engine::lighting::normalize([1.0, 0.0, -1.0]), color: [0.6; 3], shadows }),
        point_lights: vec![],
        emissive_lights: false,
        specular: 0.0,
        water: true,
        baked: 1.0,
        ..Lighting::daylight()
    };
    let mut shot = |lighting: Lighting, rt: bool| {
        r.options.gpu_meshes = true;
        r.options.ray_tracing = rt;
        r.lighting = Some(lighting);
        let mut img = None;
        // A few frames: the acceleration structures are built with the first.
        for _ in 0..3 {
            r.begin_frame(320, 240);
            r.set_camera(&Camera { source: [0.0, 0.0, -100.0], target: [0.0; 3], up: [0.0, 1.0, 0.0] });
            r.draw_actor(&model, &mut Actor::default());
            img = Some(r.render_to_image([0.0, 0.0, 0.0, 1.0]).unwrap());
        }
        img.unwrap()
    };
    // No shadow map: the shadow comes from the rays alone.
    let (open, traced) = (shot(sun(false), false), shot(sun(false), true));
    traced.write_png(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/f2-engine-rt-shadow.png")).unwrap();
    let darker = open.pixels.chunks(4).zip(traced.pixels.chunks(4)).filter(|(a, b)| a[0] > b[0] + 20).count();
    assert!((80..1500).contains(&darker), "{darker} pixels in the traced shadow");
}

/// A square (one texture across) facing the camera, its texture coordinates running
/// over it (`u` to the right, `v` down on screen).
fn textured_quad(half: f32, tex: u32) -> Mesh {
    let vertices = vec![[-half, -half, 0.0], [half, -half, 0.0], [half, half, 0.0], [-half, half, 0.0]];
    // World +x is on the screen's left: u grows with -x, v with -y.
    let uv = |v: usize| -> [i16; 2] {
        let p = vertices[v];
        [((half - p[0]) / (2.0 * half) * 1024.0) as i16, ((half - p[1]) / (2.0 * half) * 1024.0) as i16]
    };
    let faces = vec![[0, 2, 1], [0, 3, 2]];
    let face_uv = faces.iter().flat_map(|f: &[i16; 3]| f.map(|v| uv(v as usize))).collect();
    Mesh { vertices, faces, face_uv, normals: vec![], colors: vec![[1.0; 4]; 6], face_flags: vec![0; 2], textures: vec![tex; 2] }
}

/// Modded materials ([`f2_engine::Pbr`], `crate::pbr`): a normal map lights a flat quad
/// differently, a height map shifts its texture with the view (parallax occlusion), and a
/// model without a material draws exactly as before one was registered (for another model).
#[test]
fn pbr_materials_bump_and_parallax() {
    use f2_formats::material::{MatImage, ModelMaterials, PbrMaterial};
    let ctx = GpuContext::new_headless().expect("no GPU adapter");
    let mut r = Renderer::new(&ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm);
    let tex = f2_formats::crc("stripes.bmp");
    // Vertical stripes, 8 px each.
    let n = 64u32;
    let stripes: Vec<u8> = (0..n * n).flat_map(|i| if ((i % n) / 8).is_multiple_of(2) { [230, 230, 230, 255] } else { [40, 90, 200, 255] }).collect();
    r.textures.add_rgba(tex, "stripes.bmp", TexType::Normal, n, n, stripes);
    let mesh = textured_quad(40.0, tex);
    let model = |id| Model {
        id,
        radius: 60.0,
        object_size: 0,
        root: object("q", mesh.clone(), 0),
        root_siblings: vec![],
        animations: vec![],
        loc_mesh: None,
        is_skinned: false,
        num_vtx: 0,
    };
    let (plain, bumpy) = (model(10), model(11));
    let cam = Camera { source: [0.0, 70.0, -90.0], target: [0.0; 3], up: [0.0, 1.0, 0.0] };
    let shot = |r: &mut Renderer, m: &Model| {
        r.options.gpu_meshes = true;
        r.options.perspective_correct = true;
        r.options.anisotropy = 0;
        let sun = f2_engine::DirLight { to_light: f2_engine::lighting::normalize([1.0, 0.5, -1.0]), color: [0.7; 3], shadows: false };
        r.lighting = Some(f2_engine::Lighting { water: false, emissive_lights: false, sun: Some(sun), ..f2_engine::Lighting::daylight() });
        r.begin_frame(320, 240);
        r.set_camera(&cam);
        r.draw_actor(m, &mut Actor::default());
        r.render_to_image([0.0, 0.0, 0.0, 1.0]).unwrap()
    };
    let before = shot(&mut r, &plain);
    let flat_img = |v: [u8; 4]| MatImage { name: String::new(), width: 4, height: 4, rgba: v.repeat(16) };
    // Normals leaning 45° towards +u; heights: the stripes' halves high and low (32 px period
    // against the texture's 16).
    let height = MatImage {
        name: "h".into(),
        width: n,
        height: n,
        rgba: (0..n * n).flat_map(|i| if ((i % n) / 16).is_multiple_of(2) { [255; 4] } else { [0, 0, 0, 255] }).collect(),
    };
    let mat = |normal: Option<u32>, h: Option<u32>, crc: u32| PbrMaterial {
        crc,
        base_color: [1.0; 4],
        metallic: 0.0,
        roughness: 0.6,
        normal_scale: 1.0,
        occlusion_strength: 1.0,
        emissive: [0.0; 3],
        parallax_scale: if h.is_some() { 0.08 } else { 0.0 },
        normal,
        metal_rough: None,
        occlusion: None,
        emissive_map: None,
        height: h,
        tiling: Default::default(),
    };
    let mm = |normal, h| ModelMaterials {
        model_id: 11,
        images: vec![flat_img([128, 128, 255, 255]), flat_img([218, 128, 218, 255]), height.clone()],
        materials: vec![mat(normal, h, tex)],
        ..Default::default()
    };
    // A material for model 11 leaves model 10 as it was.
    r.add_model_materials(&mm(Some(0), None));
    let after = shot(&mut r, &plain);
    assert!(before.pixels == after.pixels, "a model without a material draws as before");
    let flat = shot(&mut r, &bumpy);
    r.add_model_materials(&mm(Some(1), None));
    let leaning = shot(&mut r, &bumpy);
    r.add_model_materials(&mm(Some(0), Some(2)));
    let deep = shot(&mut r, &bumpy);
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    for (name, img) in [("plain", &before), ("flat", &flat), ("leaning", &leaning), ("parallax", &deep)] {
        img.write_png(dir.join(format!("f2-engine-pbr-{name}.png"))).unwrap();
    }
    let mean = |img: &f2_engine::RgbaImage| img.pixels.chunks(4).map(|p| p[0] as f64 + p[1] as f64 + p[2] as f64).sum::<f64>() / (img.pixels.len() / 4) as f64;
    let (mf, ml) = (mean(&flat), mean(&leaning));
    assert!(mf > 20.0, "the quad is drawn ({mf})");
    assert!((mf - ml).abs() > 4.0, "a leaning normal map lights the quad differently: {mf} vs {ml}");
    // Parallax: the stripes move (many pixels change colour, not just brightness).
    let moved = flat.pixels.chunks(4).zip(deep.pixels.chunks(4)).filter(|(a, b)| (a[2] as i32 - a[0] as i32 - (b[2] as i32 - b[0] as i32)).abs() > 60).count();
    assert!(moved > 500, "{moved} pixels changed stripe");
    // Path-Traced (where the adapter has ray queries): its own PBR pipeline.
    if r.ray_tracing_supported() {
        r.options.ray_tracing = true;
        let traced = shot(&mut r, &bumpy);
        traced.write_png(dir.join("f2-engine-pbr-traced.png")).unwrap();
        assert!(mean(&traced) > 20.0, "path-traced PBR draws");
    }
}

/// Smooth skins (modded, `SkinWeights`): at rest a blended skin draws like its rigid self (the
/// rest-pose round trip is exact enough); posed, the blended joint bends smoothly where the
/// rigid one tears.
#[test]
fn smooth_skin_blends_bones() {
    use f2_formats::material::{ModelMaterials, SkinWeights};
    use f2_formats::objbank::Animation;
    let ctx = GpuContext::new_headless().expect("no GPU adapter");
    let mut r = Renderer::new(&ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm);
    let white = f2_formats::crc("white.bmp");
    r.textures.add_rgba(white, "white.bmp", TexType::Normal, 2, 2, vec![255; 16]);
    // A strip of rows at y = -20, 0, 20, 40: the lower two on the body, the upper two on the arm
    // (its origin at y = 20, vertices in its space).
    let rows = [-20.0f32, 0.0, 20.0, 40.0];
    let mut vertices = Vec::new();
    for (k, &y) in rows.iter().enumerate() {
        let ly = if k >= 2 { y - 20.0 } else { y };
        vertices.extend([[-10.0, ly, 0.0], [10.0, ly, 0.0]]);
    }
    let mut faces = Vec::new();
    for k in 0..3i16 {
        let (a, b, c, d) = (2 * k, 2 * k + 1, 2 * k + 3, 2 * k + 2);
        faces.extend([[a, c, b], [a, d, c]]);
    }
    let nf = faces.len();
    let mesh = Mesh {
        vertices,
        faces,
        face_uv: vec![[0, 0]; nf * 3],
        normals: vec![],
        colors: vec![[0.9, 0.6, 0.3, 1.0]; nf * 3],
        face_flags: vec![0; nf],
        textures: vec![white; nf],
    };
    let model = |id| {
        let mut root = object("body", mesh.clone(), 0);
        root.effected_verts = vec![0, 1, 2, 3];
        let mut arm =
            object("arm", Mesh { vertices: vec![], faces: vec![], face_uv: vec![], normals: vec![], colors: vec![], face_flags: vec![], textures: vec![] }, 0);
        arm.mesh = None;
        arm.effected_verts = vec![4, 5, 6, 7];
        arm.move_keys = vec![VKey { v: [0.0, 20.0, 0.0], time: 0 }, VKey { v: [0.0, 20.0, 0.0], time: 10 }];
        let h = std::f32::consts::FRAC_PI_6;
        arm.rotate_keys = vec![QKey { q: [0.0, 0.0, 0.0, 1.0], time: 0 }, QKey { q: [0.0, 0.0, h.sin(), h.cos()], time: 10 }];
        root.children.push(arm);
        let animations = vec![Animation { start: 0, end: 10, speed: 1.0, looped: 1 }];
        Model { id, radius: 60.0, object_size: 0, root, root_siblings: vec![], animations, loc_mesh: None, is_skinned: true, num_vtx: 8 }
    };
    let (rigid, smooth) = (model(20), model(21));
    // Rows 1 and 2 halfway between the bones.
    let w = |k: usize| {
        if k == 1 || k == 2 {
            ([0, 1, 0, 0], [128, 127, 0, 0])
        } else if k < 2 {
            ([0; 4], [255, 0, 0, 0])
        } else {
            ([1; 4], [255, 0, 0, 0])
        }
    };
    let weights = SkinWeights { joints: (0..8).map(|v| w(v / 2).0).collect(), weights: (0..8).map(|v| w(v / 2).1).collect() };
    r.add_model_materials(&ModelMaterials { model_id: 21, skin: Some(weights), ..Default::default() });
    let shot = |r: &mut Renderer, m: &Model, posed: bool| {
        r.options.gpu_meshes = true;
        r.lighting = Some(f2_engine::Lighting { water: false, emissive_lights: false, sun: None, ..f2_engine::Lighting::daylight() });
        r.begin_frame(320, 240);
        r.set_camera(&Camera { source: [0.0, 10.0, -120.0], target: [0.0, 10.0, 0.0], up: [0.0, 1.0, 0.0] });
        r.draw_actor(m, &mut Actor { animated: posed, anim_time: 10.0, ..Default::default() });
        r.render_to_image([0.0, 0.0, 0.0, 1.0]).unwrap()
    };
    let lit = |img: &f2_engine::RgbaImage| img.pixels.chunks(4).filter(|p| p[0] > 20).count();
    let diff =
        |a: &f2_engine::RgbaImage, b: &f2_engine::RgbaImage| a.pixels.chunks(4).zip(b.pixels.chunks(4)).filter(|(x, y)| (x[0] > 20) != (y[0] > 20)).count();
    let (rest_r, rest_s) = (shot(&mut r, &rigid, false), shot(&mut r, &smooth, false));
    assert!(lit(&rest_r) > 1000, "the strip is drawn");
    assert!(diff(&rest_r, &rest_s) < 20, "at rest the smooth skin is the rigid one ({} pixels differ)", diff(&rest_r, &rest_s));
    let (posed_r, posed_s) = (shot(&mut r, &rigid, true), shot(&mut r, &smooth, true));
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    posed_r.write_png(dir.join("f2-engine-skin-rigid.png")).unwrap();
    posed_s.write_png(dir.join("f2-engine-skin-smooth.png")).unwrap();
    assert!(diff(&posed_r, &posed_s) > 100, "posed, the joint bends ({} pixels differ)", diff(&posed_r, &posed_s));
}
