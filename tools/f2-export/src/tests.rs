use super::*;
use f2_formats::objbank::{Animation, Mesh, Model, Object, QKey, VKey};

fn obj(name: &str, mesh: Option<Mesh>, children: Vec<Object>) -> Object {
    Object {
        id: f2_formats::crc(name),
        name: name.into(),
        xlu: 255,
        sprites: vec![],
        mesh,
        effected_verts: vec![],
        scale_keys: vec![VKey { v: [1.0; 3], time: 0 }],
        move_keys: vec![VKey { v: [0.0, 0.0, 0.0], time: 0 }, VKey { v: [0.0, 10.0, 0.0], time: 30 }],
        rotate_keys: vec![
            QKey { q: [0.0, 0.0, 0.0, 1.0], time: 0 },
            QKey { q: [0.0, std::f32::consts::FRAC_1_SQRT_2, 0.0, std::f32::consts::FRAC_1_SQRT_2], time: 30 },
        ],
        bbox: None,
        phong: 0,
        flags: 0,
        matrix: [[0.0; 4]; 4],
        children,
    }
}

fn quad(tex: [u32; 2]) -> Mesh {
    Mesh {
        vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0]],
        faces: vec![[0, 1, 2], [0, 2, 3]],
        face_uv: vec![[0, 0], [1024, 0], [1024, 1024], [0, 0], [1024, 1024], [0, 1024]],
        normals: vec![[0.0, 2.0, 0.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
        colors: vec![[255.0, 128.0, 0.0, 255.0]; 6],
        face_flags: vec![0, 0],
        textures: tex.to_vec(),
    }
}

fn model(skinned: bool) -> Model {
    let mut child = obj("arm", if skinned { None } else { Some(quad([7, 7])) }, vec![]);
    child.effected_verts = vec![2, 3];
    let mut root = obj("body", Some(quad([7, 0])), vec![child, obj("leg", None, vec![])]);
    root.effected_verts = vec![0, 1];
    Model {
        id: 1,
        radius: 1.0,
        object_size: 0,
        root,
        root_siblings: vec![],
        animations: vec![Animation { start: 0, end: 15, speed: 1.0, looped: 1 }, Animation { start: 15, end: 30, speed: 0.5, looped: 0 }],
        loc_mesh: None,
        is_skinned: skinned,
        num_vtx: 4,
    }
}

/// Writes the model + a 1×1 texture to a temp dir and reloads it with the `gltf` crate.
fn roundtrip(m: &Model) -> (::gltf::Document, Vec<::gltf::buffer::Data>, gltf::GltfStats) {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("t.png"), encode_png(1, 1, &[255, 0, 0, 255])).unwrap();
    let tex = |crc: u32| (crc == 7).then(|| gltf::TexRef { uri: "t.png".into(), has_alpha: false });
    let g = gltf::model_to_gltf(m, "m.bin", &tex);
    fs::write(dir.path().join("m.gltf"), serde_json::to_vec_pretty(&g.json).unwrap()).unwrap();
    fs::write(dir.path().join("m.bin"), &g.bin).unwrap();
    let (doc, bufs, _imgs) = ::gltf::import(dir.path().join("m.gltf")).expect("gltf reload");
    (doc, bufs, g.stats)
}

#[test]
fn gltf_structure_roundtrip() {
    let (doc, bufs, stats) = roundtrip(&model(false));
    assert_eq!(doc.nodes().count(), 3);
    let root = doc.default_scene().unwrap().nodes().next().unwrap();
    assert_eq!(root.name(), Some("body"));
    assert_eq!(root.children().count(), 2);
    assert_eq!(doc.animations().count(), 2);
    assert_eq!(stats.triangles, 4);
    assert_eq!(stats.missing_textures, 0);
    // Root mesh: 2 primitives (texture 0 and texture 7).
    let mesh = root.mesh().unwrap();
    assert_eq!(mesh.primitives().count(), 2);
    let textured = mesh.primitives().find(|p| p.material().pbr_metallic_roughness().base_color_texture().is_some()).unwrap();
    let r = textured.reader(|b| Some(&bufs[b.index()]));
    let uv: Vec<[f32; 2]> = r.read_tex_coords(0).unwrap().into_f32().collect();
    assert!((uv[1][0] - 1024.0 * gltf::UV_SCALE).abs() < 1e-6);
    let col: Vec<[f32; 4]> = r.read_colors(0).unwrap().into_rgba_f32().collect();
    assert!((col[0][1] - 128.0 / 255.0).abs() < 1e-6);
    for n in r.read_normals().unwrap() {
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        assert!((l - 1.0).abs() < 1e-4);
    }
    // Second clip: times rebased, 15 frames at 60 fps.
    let a = doc.animations().nth(1).unwrap();
    let ch = a.channels().next().unwrap();
    let times: Vec<f32> = ch.reader(|b| Some(&bufs[b.index()])).read_inputs().unwrap().collect();
    assert_eq!(times.first().copied(), Some(0.0));
    assert!((times.last().unwrap() - 0.25).abs() < 1e-6);
    // Clip boundary sample at frame 15 = halfway translation.
    if let Some(::gltf::animation::util::ReadOutputs::Translations(mut t)) = ch.reader(|b| Some(&bufs[b.index()])).read_outputs() {
        assert!((t.next().unwrap()[1] - 5.0).abs() < 1e-4);
    }
}

#[test]
fn gltf_skinned_roundtrip() {
    let (doc, bufs, stats) = roundtrip(&model(true));
    assert!(stats.skinned);
    assert_eq!(doc.skins().count(), 1);
    assert_eq!(doc.skins().next().unwrap().joints().count(), 3);
    let skin_node = doc.nodes().find(|n| n.skin().is_some()).unwrap();
    let p = skin_node.mesh().unwrap().primitives().next().unwrap();
    let joints: Vec<[u16; 4]> = p.reader(|b| Some(&bufs[b.index()])).read_joints(0).unwrap().into_u16().collect();
    assert!(joints.iter().any(|j| j[0] == 1), "arm joint used");
}

#[test]
fn naming_helpers() {
    assert!(is_backdrop("artwork07"));
    assert!(is_backdrop("title"));
    assert!(!is_backdrop("titles"));
    assert_eq!(sanitize("a b/c"), "a_b_c");
    let t = Texture { flags: 0, crc: 0xab, width: 1, height: 1, name: "00Wate04.bm".into(), pixels: vec![0] };
    assert_eq!(texture_stem(&t), "00wate04");
    assert_eq!(Kind::parse("SOUNDS"), Some(Kind::Sounds));
}

/// Exports a real object bank and reloads every glTF (skips without game data).
#[test]
fn real_data_models_reload() {
    let dir = PathBuf::from(std::env::var("F2_GAME_DIR").unwrap_or_else(|_| "../../game-data/usa".into()));
    if !dir.join("game").is_dir() {
        eprintln!("skipping: no game data at {}", dir.display());
        return;
    }
    let out = tempfile::tempdir().unwrap();
    let mut ex = Exporter::new(&dir, out.path());
    ex.textures(true);
    let bank = ObjectBank::parse(&maybe_decompress(ex.vfs.read("objects/frogger.dll").unwrap()).unwrap()).unwrap();
    for m in &bank.models {
        let idx = &ex.tex_index;
        let tex = |crc: u32| idx.get(&crc).map(|t| gltf::TexRef { uri: format!("textures/{}/{}", t.bank, t.file), has_alpha: t.has_alpha });
        let g = gltf::model_to_gltf(m, "m.bin", &tex);
        fs::write(out.path().join("m.gltf"), serde_json::to_vec(&g.json).unwrap()).unwrap();
        fs::write(out.path().join("m.bin"), &g.bin).unwrap();
        ::gltf::import(out.path().join("m.gltf")).unwrap_or_else(|e| panic!("{}: {e}", m.name()));
    }
}
