//! n64r-export: exports one level's landscape (`world.obe`) as an F2NM test asset for the
//! N64 renderer (crates/frogger-n64 r3d) plus a scene file (camera + draw list), and renders
//! the PC reference image of exactly that geometry with f2-engine's CPU renderer
//! (`draw_landscape` + `render_soft`, authentic options) from the same camera.
//!
//! ```text
//! n64r-export --data game-data/usa --banks garden1.dll,gardenmaster.dll --tex system.fla,garden.fla \
//!     --camera sx,sy,sz,tx,ty,tz,ux,uy,uz --far 700 [--clear r,g,b] [--size 320x240] \
//!     --out target/n64/filesystem/scene [--ref target/n64/scene_ref.png]
//! ```
//! Writes `<out>.f2nm` and `<out>.f2ns`. The camera comes from
//! `F2_PRINT_CAMERA=1 frogger --headless ...` (prints the drawn camera, far clip and clear
//! colour). Outputs are derived from game data: keep them in target/ (never commit).
//!
//! `--actor bank.dll:model:x,y,z[:yaw_degrees[:anim_time]]` (repeatable) adds actors (drawn
//! like `draw_actor`: rigid objects per object frame list, skinned models as one skinned
//! mesh with one matrix per bone).
//!
//! `--fog`: the enhanced fog (linear from far/2 to far, clear colour) in both renders.
//!
//! Scene file (`F2NS`, big-endian, test-only): magic, u32 version 3, f32 view[16] (row-major
//! rows of the row-vector world→view matrix), f32 far, near, fov, u32 clear RGBA, u32 flags
//! (bit 0: fog), u32 count, count x (u32 mesh index, u32 frame list (r3d `LIST_ORDER` index,
//! | 0x100 = per-object fog), u32 n, n x f32 model[16]).

use f2_engine::anim::for_each_object;
use f2_engine::{xform_actor, Actor, Camera, Renderer, SoftTarget};
use f2_formats::objbank::{Mesh, ObjectBank};
use f2_n64fmt::encode::{encode_mesh, encode_texture, EncodedTexture, FaceIn, FileBuilder, MeshIn, TexIn};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn arg_list(v: &str) -> Vec<f32> {
    v.split(',').map(|s| s.trim().parse().expect("number")).collect()
}

struct Args {
    data: PathBuf,
    banks: Vec<String>,
    tex: Vec<String>,
    cam: Camera,
    far: f32,
    clear: [f32; 3],
    size: (u32, u32),
    out: PathBuf,
    reference: Option<PathBuf>,
    actors: Vec<(String, [f32; 3], f32, f32)>,
    fog: bool,
}

fn parse() -> Args {
    let mut a = Args {
        data: PathBuf::from(std::env::var("F2_GAME_DIR").unwrap_or_else(|_| "game-data/usa".into())),
        banks: vec!["garden1.dll".into(), "gardenmaster.dll".into()],
        tex: vec!["system.fla".into(), "garden.fla".into()],
        cam: Camera { source: [-50.0, 211.21875, 1181.2139], target: [-50.0, 0.0, 1025.0], up: [0.0, 0.1, 0.0] },
        far: 700.0,
        clear: [0.0; 3],
        size: (320, 240),
        out: PathBuf::from("target/n64/filesystem/scene"),
        reference: None,
        actors: Vec::new(),
        fog: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut v = || it.next().unwrap_or_else(|| panic!("{k} needs a value"));
        match k.as_str() {
            "--data" => a.data = PathBuf::from(v()),
            "--banks" => a.banks = v().split(',').map(String::from).collect(),
            "--tex" => a.tex = v().split(',').map(String::from).collect(),
            "--camera" => {
                let c = arg_list(&v());
                assert_eq!(c.len(), 9, "--camera sx,sy,sz,tx,ty,tz,ux,uy,uz");
                a.cam = Camera { source: [c[0], c[1], c[2]], target: [c[3], c[4], c[5]], up: [c[6], c[7], c[8]] };
            }
            "--far" => a.far = v().parse().expect("--far"),
            "--clear" => {
                let c = arg_list(&v());
                a.clear = [c[0], c[1], c[2]];
            }
            "--size" => {
                let s = v();
                let (w, h) = s.split_once('x').expect("--size WxH");
                a.size = (w.parse().unwrap(), h.parse().unwrap());
            }
            "--out" => a.out = PathBuf::from(v()),
            "--ref" => a.reference = Some(PathBuf::from(v())),
            "--fog" => a.fog = true,
            "--actor" => {
                let s = v();
                let f: Vec<&str> = s.split(':').collect();
                assert!(f.len() >= 3, "--actor bank.dll:model:x,y,z[:yaw[:anim]]");
                let p = arg_list(f[2]);
                let yaw = f.get(3).map_or(0.0, |v| v.parse().unwrap());
                let anim = f.get(4).map_or(0.0, |v| v.parse().unwrap());
                a.actors.push((f[1].to_string(), [p[0], p[1], p[2]], yaw, anim));
            }
            _ => panic!("unknown argument {k}"),
        }
    }
    a
}

fn write_png(path: &PathBuf, w: u32, h: u32, rgba: &[u8]) {
    let f = std::fs::File::create(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgba).unwrap();
}

/// The MDX mesh's drawable faces (`PCRenderObject` skips faces whose texture is missing).
fn faces_of(mesh: &Mesh, r: &Renderer, bone_of: Option<&[u8]>) -> Vec<FaceIn> {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let mut out = Vec::new();
    for (fi, f) in mesh.faces.iter().enumerate() {
        let crc = mesh.textures.get(fi).copied().unwrap_or(0);
        if crc == 0 || r.textures.current(crc).is_none() {
            continue;
        }
        if f.iter().any(|&i| i < 0 || i as usize >= mesh.vertices.len()) {
            continue;
        }
        // Skinned: vertices no bone moves stay untransformed (`vz = 0`): faces skipped.
        if let Some(b) = bone_of {
            if f.iter().any(|&i| b[i as usize] == u8::MAX) {
                continue;
            }
        }
        let corner = |k: usize| {
            let c = mesh.colors.get(fi * 3 + k).copied().unwrap_or([1.0; 4]);
            [byte(c[0]), byte(c[1]), byte(c[2]), byte(c[3])]
        };
        out.push(FaceIn {
            v: f.map(|i| i as u16),
            uv: [0, 1, 2].map(|k| mesh.face_uv.get(fi * 3 + k).copied().unwrap_or([0, 0])),
            rgba: [corner(0), corner(1), corner(2)],
            tex_crc: crc,
        });
    }
    out
}

/// Frame list of an object (`object_frame`, scene.rs) as an r3d `LIST_ORDER` index;
/// `None` for `Glow` (never drawn on retail PC).
fn object_list(flags: i16) -> Option<u32> {
    use f2_engine::scene::*;
    let mut l = 0;
    if flags & OBJECT_FLAGS_MODGE != 0 {
        l = 1;
    }
    if flags & OBJECT_FLAGS_XLU != 0 {
        l = 2;
    }
    if flags & OBJECT_FLAGS_ADDITIVE != 0 {
        l = 3;
    }
    if flags & OBJECT_FLAGS_GLOW != 0 {
        return None;
    }
    Some(l)
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--list") {
        // n64r-export --list <bank.dll>...: models with object/face counts.
        for b in std::env::args().skip(2) {
            let bank = f2_engine::load::read_object_bank(&b).unwrap_or_else(|e| panic!("{e}"));
            for m in &bank.models {
                let (mut objs, mut faces) = (0, 0);
                for_each_object(m, |_, o| {
                    objs += 1;
                    faces += o.mesh.as_ref().map_or(0, |x| x.faces.len());
                });
                println!("{b}: {} skinned={} objects={objs} faces={faces} anims={}", m.name(), m.is_skinned, m.animations.len());
            }
        }
        return;
    }
    let a = parse();
    let game = a.data.join("game");
    let banks: Vec<ObjectBank> = a
        .banks
        .iter()
        .map(|b| f2_engine::load::read_object_bank(game.join("objects").join(b)).unwrap_or_else(|e| panic!("{e}")))
        .collect();
    let world_id = f2_formats::crc("world.obe");
    let world = banks.iter().flat_map(|b| b.models.iter()).find(|m| m.id == world_id).expect("no world.obe in the banks");

    let mut r = Renderer::new_soft();
    for t in &a.tex {
        let tb = f2_engine::load::read_tex_bank(game.join("textures").join(t)).unwrap_or_else(|e| panic!("{e}"));
        r.load_tex_bank(&tb);
    }
    r.update_animating_textures(0);

    // Actors (model, engine actor).
    let mut actors: Vec<(&f2_formats::objbank::Model, Actor)> = Vec::new();
    for (name, pos, yaw, anim) in &a.actors {
        let model = banks.iter().find_map(|b| f2_engine::find_model(b, name)).unwrap_or_else(|| panic!("model {name} not found"));
        let h = yaw.to_radians() / 2.0;
        let actor = Actor { pos: *pos, q_rot: [0.0, h.sin(), 0.0, h.cos()], anim_time: *anim, animated: true, ..Default::default() };
        actors.push((model, actor));
    }

    // PC reference: f2-engine CPU renderer, authentic options.
    let (w, h) = a.size;
    r.begin_frame(w, h);
    r.set_camera(&a.cam);
    r.view.far_clip = a.far;
    if a.fog {
        r.options.fog = f2_engine::Fog { enabled: true, start: a.far * 0.5, end: a.far, color: a.clear };
    }
    r.draw_landscape(world);
    for (model, actor) in &actors {
        r.draw_actor(model, &mut actor.clone());
    }
    let queued = r.queued_tris();
    let clear = [a.clear[0] as f64, a.clear[1] as f64, a.clear[2] as f64, 1.0];
    if let Some(p) = &a.reference {
        let mut t = SoftTarget::new(w, h);
        r.render_soft(&mut t, clear, 4);
        let bytes: Vec<u8> = t.pixels.iter().flat_map(|p| p.to_le_bytes()).collect();
        write_png(p, w, h, &bytes);
        eprintln!("n64r-export: PC reference {} ({queued} triangles after MDX rejection)", p.display());
    }

    // F2NM: one mesh per world object (drawn with its key-0 matrix, like draw_landscape).
    let mats = xform_actor(world, &mut Actor { animated: false, ..Default::default() });
    let mut fb = FileBuilder::new();
    let mut draws: Vec<(u32, u32, Vec<[[f32; 4]; 4]>)> = Vec::new();
    let mut tex_crcs: BTreeMap<u32, ()> = BTreeMap::new();
    let mut total_faces = 0;
    for_each_object(world, |i, o| {
        let Some(mesh) = o.mesh.as_ref() else { return };
        let faces = faces_of(mesh, &r, None);
        if faces.is_empty() {
            return;
        }
        total_faces += faces.len();
        for f in &faces {
            tex_crcs.insert(f.tex_crc, ());
        }
        let em = encode_mesh(&MeshIn { id: o.id, positions: &mesh.vertices, faces: &faces, bone_of: None, ordered: false, wrap: false });
        draws.push((fb.meshes.len() as u32, 0, vec![mats[i]]));
        fb.meshes.push(em);
    });
    // Actors: as draw_actor (scene.rs).
    for (model, actor) in &actors {
        let mats = xform_actor(model, &mut actor.clone());
        if model.is_skinned {
            let Some(mesh) = model.root.mesh.as_ref() else { continue };
            let mut bone_of = vec![u8::MAX; mesh.vertices.len()];
            for_each_object(model, |i, o| {
                for &j in &o.effected_verts {
                    if let Some(b) = bone_of.get_mut(j as usize) {
                        *b = i as u8;
                    }
                }
            });
            let faces = faces_of(mesh, &r, Some(&bone_of));
            for f in &faces {
                tex_crcs.insert(f.tex_crc, ());
            }
            let em = encode_mesh(&MeshIn { id: model.id, positions: &mesh.vertices, faces: &faces, bone_of: Some(&bone_of), ordered: false, wrap: false });
            draws.push((fb.meshes.len() as u32, 0x100, mats.clone()));
            fb.meshes.push(em);
        } else {
            for_each_object(model, |i, o| {
                let Some(mesh) = o.mesh.as_ref() else { return };
                let Some(list) = object_list(o.flags) else { return };
                let faces = faces_of(mesh, &r, None);
                if faces.is_empty() {
                    return;
                }
                for f in &faces {
                    tex_crcs.insert(f.tex_crc, ());
                }
                let em = encode_mesh(&MeshIn { id: o.id, positions: &mesh.vertices, faces: &faces, bone_of: None, ordered: list >= 2, wrap: list >= 1 });
                draws.push((fb.meshes.len() as u32, list | 0x100, vec![mats[i]]));
                fb.meshes.push(em);
            });
        }
    }
    let mut fmt_count: BTreeMap<String, usize> = BTreeMap::new();
    for &crc in tex_crcs.keys() {
        let h = r.textures.current(crc).unwrap();
        let img = &r.textures.images[h.0 as usize];
        let t: EncodedTexture = encode_texture(&TexIn { crc, width: img.width, height: img.height, rgba: &img.rgba });
        *fmt_count.entry(format!("{:?} {}x{}", t.fmt, t.width, t.height)).or_default() += 1;
        fb.textures.push(t);
    }
    let bytes = fb.finish();
    let f2nm = a.out.with_extension("f2nm");
    if let Some(d) = f2nm.parent() {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(&f2nm, &bytes).unwrap();

    // Scene file.
    let mut s = Vec::new();
    let put_f = |s: &mut Vec<u8>, v: f32| s.extend_from_slice(&v.to_bits().to_be_bytes());
    s.extend_from_slice(b"F2NS");
    s.extend_from_slice(&3u32.to_be_bytes());
    for row in a.cam.view_matrix() {
        for v in row {
            put_f(&mut s, v);
        }
    }
    put_f(&mut s, a.far);
    put_f(&mut s, 10.0);
    put_f(&mut s, 450.0);
    let c8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    s.extend_from_slice(&(c8(a.clear[0]) << 24 | c8(a.clear[1]) << 16 | c8(a.clear[2]) << 8 | 255).to_be_bytes());
    s.extend_from_slice(&(a.fog as u32).to_be_bytes());
    s.extend_from_slice(&(draws.len() as u32).to_be_bytes());
    for (mi, list, ms) in &draws {
        s.extend_from_slice(&mi.to_be_bytes());
        s.extend_from_slice(&list.to_be_bytes());
        s.extend_from_slice(&(ms.len() as u32).to_be_bytes());
        for m in ms {
            for row in m {
                for &v in row {
                    put_f(&mut s, v);
                }
            }
        }
    }
    let f2ns = a.out.with_extension("f2ns");
    std::fs::write(&f2ns, &s).unwrap();
    let tris: u32 = fb.meshes.iter().map(|m| m.n_tris).sum();
    let verts: usize = fb.meshes.iter().map(|m| m.vertices.len()).sum();
    eprintln!(
        "n64r-export: {} ({} bytes): {} meshes, {tris} triangles ({total_faces} faces), {verts} vertices ({:.2}/tri), {} textures {:?}",
        f2nm.display(),
        bytes.len(),
        fb.meshes.len(),
        verts as f32 / tris.max(1) as f32,
        fb.textures.len(),
        fmt_count
    );
    let mut per_list = [0; 8];
    for (_, l, _) in &draws {
        per_list[(*l & 0xFF) as usize] += 1;
    }
    eprintln!("n64r-export: {} ({} draws; per list Normal/Wrap/Xlu/Additive: {:?})", f2ns.display(), draws.len(), &per_list[..4]);
}
