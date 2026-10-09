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
