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
    let flat = Lighting { sky: [1.0; 3], ground: [1.0; 3], sun: None, point_lights: vec![], emissive_lights: false, specular: 0.0, water: false, baked: 1.0, ..Lighting::daylight() };
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
                r.water_splash(Splash { pos: [0.0, 0.0, 0.0], radius: 25.0, strength: 6.0, above: 0.0 });
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
