use super::*;

use f2_formats::objbank::{Object, Quat, Vec3};

/// Retail `game/` dir from `F2_GAME_DIR` (default `game-data/usa/game`), if imported.
pub(crate) fn game_dir() -> Option<PathBuf> {
    let d = PathBuf::from(std::env::var("F2_GAME_DIR").unwrap_or_else(|_| "../../game-data/usa/game".into()));
    d.join("objects").is_dir().then_some(d)
}

fn put(root: &Path, rel: &str, data: &[u8]) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, data).unwrap();
}

fn png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(rgba).unwrap();
    out
}

fn tex(name: &str, crc: u32, flags: u32) -> f2_formats::texture::Texture {
    f2_formats::texture::Texture { flags, crc, width: 1, height: 1, name: name.into(), pixels: vec![0x1234] }
}

/// A synthetic rendition + two mods; checks layering and per-entry resolution.
#[test]
fn resolves_overrides_over_synthetic_banks() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path().join("usa");
    let base = TexBank { textures: vec![tex("00wate04.bmp", texture::texture_crc("00wate04"), 0), tex("ai_glow.bmp", 0xabcdef01, 1)], anims: vec![] };
    put(&r, "game/textures/Garden.fla", &texture::write_texbank(&base));
    let s = |uid| f2_formats::wus::Sample { uid, looped: true, format_tag: 1, channels: 1, sample_rate: 11025, bits: 16, pcm: vec![0, 0] };
    put(&r, "game/sfx/garden.wus", &sound::write_soundbank(&SoundBank { samples: vec![s(crc("hop")), s(0x11223344)] }));
    let lang = "LANG_NAME\tComments\tEnglish\tFrançais\tDeutsch\tItaliano\tUS\r\nSTR_A\t\ta\tb\tc\td\te\r\n";
    put(&r, "game/maps/language.fla", lang.as_bytes());
    put(&r, "music/track02.wav", &f2_audio::wav::encode_wav(1, 44100, &[0; 4]));

    let mods = t.path().join("mods");
    let manifest = |id: &str| format!("id = \"{id}\"\nname = \"{id}\"\nversion = \"1.0.0\"\nengine_version = \">=0.1\"\n");
    put(&mods, "a/mod.toml", manifest("a").as_bytes());
    put(&mods, "a/game/textures/GARDEN/00Wate04.png", &png(1, 1, &[255, 0, 0, 255]));
    put(&mods, "a/game/textures/garden/newtex.png", &png(1, 1, &[0, 0, 255, 255]));
    put(&mods, "a/game/text/language.tsv", "id\tcomment\tEnglish\r\nSTR_A\t\tmod-a\r\n".as_bytes());
    put(&mods, "b/mod.toml", manifest("b").as_bytes());
    put(&mods, "b/game/textures/garden/00wate04.png", &png(1, 1, &[0, 255, 0, 255]));
    put(&mods, "b/game/textures/garden/ai_glow.png", &png(1, 1, &[0, 0, 0, 128]));
    put(&mods, "b/game/sfx/garden/Hop.wav", &f2_audio::wav::encode_wav(2, 48000, &[1000; 9600]));
    put(&mods, "b/game/sfx/garden/11223344.wav", &f2_audio::wav::encode_wav(1, 22050, &[5; 10]));
    put(&mods, "b/game/sfx/garden/brand_new.wav", &f2_audio::wav::encode_wav(1, 11025, &[7; 3]));
    put(&mods, "b/game/text/es.toml", "name = \"Español\"\n[strings]\nSTR_A = \"hola\"\nSTR_NEW = \"nuevo\"\n".as_bytes());
    put(&mods, "b/music/track02.flac", b"not really flac");

    let (vfs, _) = f2_mods::load_enabled_mods(&r, &mods, &["a", "b"]).unwrap();
    let tb = tex_bank(&vfs, "garden").unwrap();
    assert!(tb.warnings.is_empty(), "{:?}", tb.warnings);
    assert_eq!(tb.replaced, vec!["00wate04", "ai_glow"]);
    assert_eq!(tb.added, vec!["newtex"]);
    let b = &tb.value;
    assert_eq!(b.textures.len(), 3);
    assert_eq!(b.textures[0].pixels, vec![0x03e0], "mod b (later) wins");
    assert_eq!(b.textures[0].name, "00wate04.bm");
    assert_eq!(b.textures[1].flags, 1);
    assert_eq!(b.textures[2].crc, crc("newtex.bmp"));

    let sb = sound_bank(&vfs, "garden").unwrap();
    assert_eq!(sb.replaced, vec!["11223344", "hop"]);
    assert_eq!(sb.added, vec!["brand_new"]);
    let hop = sb.value.find(crc("hop")).unwrap();
    assert_eq!((hop.sample_rate, hop.channels, hop.looped), (11025, 1, true));
    assert_eq!(hop.pcm.len(), 2 * 1102);
    assert!(!sb.value.find(crc("brand_new")).unwrap().looped);

    let st = string_table(&vfs).unwrap();
    assert!(st.warnings.is_empty(), "{:?}", st.warnings);
    assert_eq!(st.value.get("STR_A", f2_formats::lang::Language::Uk), Some("mod-a"));
    assert_eq!(st.value.languages.last().unwrap(), "Español");
    assert_eq!(st.value.rows[0].2[5], "hola");
    assert_eq!(st.value.rows[1].0, "STR_NEW");
    assert_eq!(st.value.rows[1].2[0], "", "new row: other languages stay empty when not given");

    assert!(music_track(&vfs, 2).unwrap().ends_with("b/music/track02.flac"));
    assert!(music_track(&vfs, 3).is_none());

    // Missing base bank → new bank from overrides only; broken files are warnings.
    put(&mods, "b/game/textures/newbank/x.png", b"garbage");
    put(&mods, "b/game/textures/newbank/y.png", &png(2, 2, &[9; 16]));
    let nb = tex_bank(&vfs, "newbank").unwrap();
    assert_eq!(nb.value.textures.len(), 1);
    assert_eq!(nb.warnings.len(), 1);
    assert_eq!(override_banks(&vfs, "textures"), vec!["garden", "newbank"]);

    // Validation.
    let rep = pack::validate(&mods.join("b"), Some(&r));
    assert!(rep.errors.iter().any(|e| e.contains("x.png")), "{:?}", rep.errors);
    assert!(rep.errors.iter().any(|e| e.contains("track02.flac")), "{:?}", rep.errors);
    let rep = pack::validate(&mods.join("a"), Some(&r));
    assert!(rep.ok(), "{:?}", rep.errors);
}

#[test]
fn pack_warns_on_retail_copies_and_scaffolds() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path().join("usa");
    let bank = TexBank {
        textures: vec![f2_formats::texture::Texture {
            flags: 0,
            crc: texture::texture_crc("rock"),
            width: 1,
            height: 1,
            name: "rock.bmp".into(),
            pixels: vec![0x7c00],
        }],
        anims: vec![],
    };
    let fla = texture::write_texbank(&bank);
    put(&r, "game/textures/garden.fla", &fla);
    let sha: String = <sha1::Sha1 as sha1::Digest>::digest(&fla).iter().map(|b| format!("{b:02x}")).collect();
    put(&r, "manifest.json", format!("{{\"files\": {{\"game/textures/garden.fla\": \"{sha}\"}}}}").as_bytes());

    let dir = pack::scaffold(&t.path().join("mods"), "my-mod").unwrap();
    assert!(pack::scaffold(&t.path().join("mods"), "my-mod").is_err());
    assert!(pack::scaffold(&t.path().join("mods"), "bad id").is_err());
    let rep = pack::validate(&dir, Some(&r));
    assert!(rep.ok(), "{:?}", rep.errors);
    put(&dir, "game/textures/garden.fla", &fla);
    put(&dir, "game/textures/garden/rock.png", &png(1, 1, &[255, 0, 0, 255]));
    let rep = pack::validate(&dir, Some(&r));
    assert!(rep.warnings.iter().any(|w| w.contains("byte-identical to retail asset game/textures/garden.fla")), "{:?}", rep.warnings);
    assert!(rep.warnings.iter().any(|w| w.contains("rock: identical to the retail texture")), "{:?}", rep.warnings);
    fs::write(dir.join("mod.toml"), "id = \"x\"").unwrap();
    assert!(!pack::validate(&dir, None).ok());
}

// ---- glTF round trip over every retail model -------------------------------------------

const EPS: f32 = 1e-3;

fn finite3(v: Vec3) -> Vec3 {
    v.map(|x| if x.is_finite() { x } else { 0.0 })
}

fn close3(a: Vec3, b: Vec3) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() <= EPS * (1.0 + a[i].abs()))
}

fn unit(q: Quat) -> Quat {
    let l = q.iter().map(|v| v * v).sum::<f32>().sqrt();
    if l.is_finite() && l > 1e-6 {
        q.map(|v| v / l)
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

fn close_q(a: Quat, b: Quat) -> bool {
    let (a, b) = (unit(a), unit(b));
    (0..4).map(|i| a[i] * b[i]).sum::<f32>().abs() >= 1.0 - 1e-4
}

fn preorder<'a>(o: &'a Object, out: &mut Vec<&'a Object>) {
    out.push(o);
    for c in &o.children {
        preorder(c, out);
    }
}

fn objects(m: &Model) -> Vec<&Object> {
    let mut v = Vec::new();
    preorder(&m.root, &mut v);
    for s in &m.root_siblings {
        preorder(s, &mut v);
    }
    v
}

/// Mesh faces in exporter order (stable by texture CRC, invalid faces dropped),
/// as (corner positions, uvs, colours, crc).
type Corner = (Vec3, [i16; 2], [f32; 4]);
fn face_list(m: &f2_formats::objbank::Mesh, color_norm: bool) -> Vec<([Corner; 3], u32)> {
    let nv = m.vertices.len();
    let mut idx: Vec<usize> = (0..m.faces.len()).filter(|&f| m.faces[f].iter().all(|&i| i >= 0 && (i as usize) < nv)).collect();
    idx.sort_by_key(|&f| m.textures.get(f).copied().unwrap_or(0));
    let c255 = color_norm && m.colors.iter().any(|c| c.iter().any(|&v| v > 1.0));
    idx.into_iter()
        .map(|f| {
            let corners = std::array::from_fn(|k| {
                let c = m.colors.get(f * 3 + k).copied().unwrap_or([1.0; 4]);
                let c = if color_norm { c.map(|v| if c255 { v / 255.0 } else { v }).map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 1.0 }) } else { c };
                (finite3(m.vertices[m.faces[f][k] as usize]), m.face_uv.get(f * 3 + k).copied().unwrap_or([0, 0]), c)
            });
            (corners, m.textures.get(f).copied().unwrap_or(0))
        })
        .collect()
}

fn compare_mesh(ctx: &str, a: &f2_formats::objbank::Mesh, b: &f2_formats::objbank::Mesh) -> Result<(), String> {
    let (fa, fb) = (face_list(a, true), face_list(b, false));
    if fa.len() != fb.len() {
        return Err(format!("{ctx}: {} faces vs {}", fa.len(), fb.len()));
    }
    for (i, (x, y)) in fa.iter().zip(&fb).enumerate() {
        if x.1 != y.1 {
            return Err(format!("{ctx}: face {i} texture {:08x} vs {:08x}", x.1, y.1));
        }
        for k in 0..3 {
            let (p, q) = (&x.0[k], &y.0[k]);
            if !close3(p.0, q.0) || p.1 != q.1 || (0..4).any(|c| (p.2[c] - q.2[c]).abs() > EPS) {
                return Err(format!("{ctx}: face {i} corner {k}: {p:?} vs {q:?}"));
            }
        }
    }
    if b.vertices.len() > 3 * fb.len() {
        return Err(format!("{ctx}: too many vertices"));
    }
    Ok(())
}

/// Checks that every original key inside the exported clip ranges is present (same frame, same value).
fn compare_track<V: Copy + std::fmt::Debug>(
    ctx: &str,
    orig: &[(i32, V)],
    imp: &[(i32, V)],
    ranges: &[(i32, i32)],
    close: impl Fn(V, V) -> bool,
) -> Result<(), String> {
    match orig.len() {
        0 => {
            if !imp.is_empty() {
                return Err(format!("{ctx}: {} keys from nothing", imp.len()));
            }
        }
        1 => {
            if imp.len() != 1 || !close(orig[0].1, imp[0].1) {
                return Err(format!("{ctx}: rest key {orig:?} vs {imp:?}"));
            }
        }
        _ => {
            let mut times: std::collections::BTreeMap<i32, usize> = Default::default();
            for k in orig {
                *times.entry(k.0).or_default() += 1;
            }
            for &(t, v) in orig {
                if times[&t] > 1 || !ranges.iter().any(|&(s, e)| t >= s && t <= e) {
                    continue;
                }
                let Some(&(_, w)) = imp.iter().find(|k| k.0 == t) else { return Err(format!("{ctx}: key at frame {t} missing")) };
                if !close(v, w) {
                    return Err(format!("{ctx}: frame {t}: {v:?} vs {w:?}"));
                }
            }
            if imp.len() > orig.len() + 2 * ranges.len() + 1 {
                return Err(format!("{ctx}: {} keys from {}", imp.len(), orig.len()));
            }
        }
    }
    Ok(())
}

fn compare_model(a: &Model, b: &Model) -> Result<(), String> {
    let n = a.name();
    if a.id != b.id {
        return Err(format!("{n}: id {:08x} vs {:08x}", a.id, b.id));
    }
    let (oa, ob) = (objects(a), objects(b));
    if oa.len() != ob.len() || a.root_siblings.len() != b.root_siblings.len() {
        return Err(format!("{n}: {} objects vs {}", oa.len(), ob.len()));
    }
    let animated = oa.iter().any(|o| o.move_keys.len() > 1 || o.rotate_keys.len() > 1 || o.scale_keys.len() > 1);
    let clips: Vec<_> = a.animations.iter().filter(|c| c.end >= c.start).cloned().collect();
    let mut ranges: Vec<(i32, i32)> = clips.iter().map(|c| (i32::from(c.start), i32::from(c.end))).collect();
    if animated {
        if b.animations != clips {
            return Err(format!("{n}: clips {:?} vs {:?}", clips, b.animations));
        }
        if ranges.is_empty() {
            let all = oa
                .iter()
                .flat_map(|o| o.move_keys.iter().map(|k| k.time).chain(o.scale_keys.iter().map(|k| k.time)).chain(o.rotate_keys.iter().map(|k| k.time)));
            let (lo, hi) = all.fold((i32::MAX, i32::MIN), |(l, h), t| (l.min(t), h.max(t)));
            ranges.push((lo, hi));
        }
    } else if !b.animations.is_empty() {
        return Err(format!("{n}: clips from a static model"));
    }
    let skinned = a.is_skinned && a.root.mesh.is_some();
    if b.is_skinned != skinned {
        return Err(format!("{n}: skinned {} vs {}", a.is_skinned, b.is_skinned));
    }
    for (i, (x, y)) in oa.iter().zip(&ob).enumerate() {
        let ctx = format!("{n}/{}", x.name);
        if x.name != y.name
            || x.id != y.id
            || x.flags != y.flags
            || x.xlu != y.xlu
            || x.phong != y.phong
            || x.sprites != y.sprites
            || x.children.len() != y.children.len()
        {
            return Err(format!("{ctx}: object header/shape differs"));
        }
        let is_skin = i == 0 && skinned;
        match (&x.mesh, &y.mesh) {
            (Some(ma), Some(mb)) => compare_mesh(&ctx, ma, mb)?,
            (Some(ma), None) if face_list(ma, true).is_empty() => {}
            (None, None) => {}
            _ if is_skin => {}
            _ => return Err(format!("{ctx}: mesh presence differs")),
        }
        let mv = |o: &Object| o.move_keys.iter().map(|k| (k.time, finite3(k.v))).collect::<Vec<_>>();
        let sc = |o: &Object| o.scale_keys.iter().map(|k| (k.time, finite3(k.v))).collect::<Vec<_>>();
        let rt = |o: &Object| o.rotate_keys.iter().map(|k| (k.time, k.q)).collect::<Vec<_>>();
        compare_track(&format!("{ctx} move"), &mv(x), &mv(y), &ranges, close3)?;
        compare_track(&format!("{ctx} scale"), &sc(x), &sc(y), &ranges, close3)?;
        compare_track(&format!("{ctx} rotate"), &rt(x), &rt(y), &ranges, close_q)?;
    }
    if skinned {
        // Skin vertices per bone, by position (exporter: last bone listing a vertex wins).
        let ma = a.root.mesh.as_ref().unwrap();
        let mb = b.root.mesh.as_ref().unwrap();
        let mut owner = vec![0usize; ma.vertices.len()];
        for (j, o) in oa.iter().enumerate() {
            for &v in &o.effected_verts {
                if v >= 0 && (v as usize) < owner.len() {
                    owner[v as usize] = j;
                }
            }
        }
        let used: std::collections::BTreeSet<usize> = ma.faces.iter().flat_map(|f| f.iter().map(|&i| i as usize)).filter(|&i| i < owner.len()).collect();
        for (j, o) in ob.iter().enumerate() {
            let key = |v: Vec3| finite3(v).map(f32::to_bits);
            let want: std::collections::BTreeSet<_> = used.iter().filter(|&&v| owner[v] == j).map(|&v| key(ma.vertices[v])).collect();
            let got: std::collections::BTreeSet<_> = o.effected_verts.iter().map(|&v| key(mb.vertices[v as usize])).collect();
            if want != got {
                return Err(format!("{n}/{}: skin vertices differ ({} vs {})", o.name, want.len(), got.len()));
            }
        }
    }
    Ok(())
}

#[test]
fn gltf_round_trip_all_retail_models() {
    let Some(g) = game_dir() else {
        eprintln!("skipping: no game data (set F2_GAME_DIR)");
        return;
    };
    let mut vfs = Vfs::new();
    vfs.add_layer("base", &g);
    let (mut models, mut failures, mut animated, mut skinned) = (0, Vec::new(), 0, 0);
    for e in vfs.list("objects").unwrap() {
        let Some(stem) = e.name.to_ascii_lowercase().strip_suffix(".dll").map(str::to_string) else { continue };
        let Some(data) = read_container(&vfs, &format!("objects/{}", e.name)).unwrap() else { continue };
        let Ok(bank) = ObjectBank::parse(&data) else { continue };
        for m in &bank.models {
            let out = f2_export::gltf::model_to_gltf(m, "m.bin", &|_| None);
            let doc = gltf::GltfDoc { json: out.json, buffers: vec![out.bin], base_dir: None };
            match gltf::import_model(&doc, None) {
                Ok(b) => {
                    if let Err(e) = compare_model(m, &b) {
                        failures.push(format!("{stem}: {e}"));
                    }
                    animated += usize::from(!b.animations.is_empty());
                    skinned += usize::from(b.is_skinned);
                }
                Err(e) => failures.push(format!("{stem}/{}: import failed: {e}", m.name())),
            }
            models += 1;
        }
    }
    eprintln!("round-tripped {models} models ({animated} with clips, {skinned} skinned), {} failures", failures.len());
    assert!(models > 500, "expected the retail object banks");
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.iter().take(20).cloned().collect::<Vec<_>>().join("\n"));
}

/// Real-data override flow: a mod replacing a retail model with its own export
/// (written to disk as .gltf + .bin) and a texture.
#[test]
fn retail_bank_with_file_overrides() {
    let Some(g) = game_dir() else { return };
    let t = tempfile::tempdir().unwrap();
    let mut vfs = Vfs::new();
    vfs.add_layer("base", &g);
    let base = object_bank(&vfs, "frogger").unwrap().value;
    let names = model_names(&base);
    let i = names.iter().position(|n| n == "frog").expect("frog model");
    let out = f2_export::gltf::model_to_gltf(&base.models[i], "frog.bin", &|_| None);
    put(t.path(), "objects/frogger/frog.gltf", serde_json::to_string(&out.json).unwrap().as_bytes());
    put(t.path(), "objects/frogger/frog.bin", &out.bin);
    put(t.path(), "objects/frogger/newthing.gltf", serde_json::to_string(&out.json).unwrap().as_bytes());
    put(t.path(), "objects/frogger/newthing.bin", &out.bin);
    vfs.add_layer("mod", t.path());
    let l = object_bank(&vfs, "frogger").unwrap();
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);
    assert_eq!(l.replaced, vec!["frog"]);
    assert_eq!(l.added, vec!["newthing"]);
    assert_eq!(l.value.models.len(), base.models.len() + 1);
    compare_model(&base.models[i], &l.value.models[i]).unwrap();
    assert_eq!(l.value.models.last().unwrap().id, crc("newthing.obe"));
}

fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for k in 0..4 {
            s.push(if k <= c.len() { T[(n >> (18 - 6 * k)) as usize & 63] as char } else { '=' });
        }
    }
    s
}

/// Accessors over one little-endian buffer (a test glTF's).
#[derive(Default)]
struct Bin {
    bytes: Vec<u8>,
    accessors: Vec<serde_json::Value>,
}

impl Bin {
    fn f32s(&mut self, v: &[f32], ty: &str) -> usize {
        let n = match ty {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            "VEC4" => 4,
            _ => 16,
        };
        self.accessors.push(serde_json::json!({"bufferView": 0, "byteOffset": self.bytes.len(), "componentType": 5126, "count": v.len() / n, "type": ty}));
        self.bytes.extend(v.iter().flat_map(|f| f.to_le_bytes()));
        self.accessors.len() - 1
    }
    fn u16s(&mut self, v: &[u16]) -> usize {
        self.accessors.push(serde_json::json!({"bufferView": 0, "byteOffset": self.bytes.len(), "componentType": 5123, "count": v.len() / 4, "type": "VEC4"}));
        self.bytes.extend(v.iter().flat_map(|f| f.to_le_bytes()));
        while !self.bytes.len().is_multiple_of(4) {
            self.bytes.push(0);
        }
        self.accessors.len() - 1
    }
    fn finish(self, mut doc: serde_json::Value) -> serde_json::Value {
        doc["buffers"] = serde_json::json!([{"byteLength": self.bytes.len(), "uri": format!("data:application/octet-stream;base64,{}", b64(&self.bytes))}]);
        doc["bufferViews"] = serde_json::json!([{"buffer": 0, "byteOffset": 0, "byteLength": self.bytes.len()}]);
        doc["accessors"] = serde_json::Value::Array(self.accessors);
        doc
    }
}

/// glTF PBR materials: factors and maps, images embedded (data URI) and next to the document,
/// the height map from the extras and by the `_height` name, a self-contained base colour image
/// registered as the faces' texture, and a second material on the same image made unique.
#[test]
fn gltf_pbr_materials_and_images() {
    let t = tempfile::tempdir().unwrap();
    let px = |c: [u8; 4]| png(2, 2, &c.repeat(4));
    put(t.path(), "objects/b/skin.png", &px([200, 100, 50, 255]));
    put(t.path(), "objects/b/skin_n.png", &px([128, 128, 255, 255]));
    put(t.path(), "objects/b/bump.png", &px([90, 90, 90, 255]));
    put(t.path(), "objects/b/plain_height.png", &px([20, 20, 20, 255]));
    put(t.path(), "objects/b/plain.png", &px([1, 2, 3, 255]));
    let mut bin = Bin::default();
    let pos = bin.f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0], "VEC3");
    let uv = bin.f32s(&[0.0, 0.0, 0.975, 0.0, 0.0, 0.975], "VEC2");
    let embedded = format!("data:image/png;base64,{}", b64(&px([10, 20, 30, 255])));
    let doc = serde_json::json!({
        "asset": {"version": "2.0"},
        "scenes": [{"nodes": [0]}],
        "nodes": [{"name": "thing", "mesh": 0}],
        "meshes": [{"primitives": [
            {"attributes": {"POSITION": pos, "TEXCOORD_0": uv}, "material": 0},
            {"attributes": {"POSITION": pos, "TEXCOORD_0": uv}, "material": 1},
            {"attributes": {"POSITION": pos, "TEXCOORD_0": uv}, "material": 2},
            {"attributes": {"POSITION": pos, "TEXCOORD_0": uv}, "material": 3},
            {"attributes": {"POSITION": pos, "TEXCOORD_0": uv}, "material": 4}
        ]}],
        "materials": [
            {"name": "frogskin", "pbrMetallicRoughness": {"baseColorTexture": {"index": 0}, "baseColorFactor": [1.0, 0.5, 1.0, 1.0], "metallicFactor": 0.25, "roughnessFactor": 0.5,
                "metallicRoughnessTexture": {"index": 2}},
             "normalTexture": {"index": 1, "scale": 0.8}, "occlusionTexture": {"index": 3, "strength": 0.5}, "emissiveTexture": {"index": 3},
             "emissiveFactor": [1.0, 0.5, 0.0], "extensions": {"KHR_materials_emissive_strength": {"emissiveStrength": 2.0}},
             "extras": {"heightTexture": {"index": 4}, "parallaxScale": 0.05}},
            // The exporter's: plain.
            {"pbrMetallicRoughness": {"baseColorTexture": {"index": 5}, "metallicFactor": 0.0, "roughnessFactor": 1.0}},
            // Same image as the first, plain: a CRC of its own.
            {"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}, "metallicFactor": 0.0, "roughnessFactor": 1.0}},
            // A height map by name only (plain.png → plain_height.png).
            {"pbrMetallicRoughness": {"baseColorTexture": {"index": 6}, "metallicFactor": 0.0, "roughnessFactor": 1.0}},
            // A retail texture by name, outside the directory: not read, no warning.
            {"pbrMetallicRoughness": {"baseColorTexture": {"index": 7}, "metallicFactor": 0.0, "roughnessFactor": 1.0}}
        ],
        "textures": [{"source": 0}, {"source": 1}, {"source": 2}, {"source": 3}, {"source": 4}, {"source": 5}, {"source": 6}, {"source": 7}],
        "images": [{"uri": "skin.png"}, {"uri": "skin_n.png"}, {"uri": embedded}, {"uri": "bump.png"}, {"uri": "bump.png"}, {"uri": embedded, "name": "Glow"},
            {"uri": "plain.png"}, {"uri": "../tex/00Wate04.png"}]
    });
    let doc = bin.finish(doc);
    let path = t.path().join("objects/b/thing.gltf");
    fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    let imp = gltf::import(&gltf::GltfDoc::load(&path).unwrap(), Some(7), None).unwrap();
    assert!(imp.warnings.is_empty(), "{:?}", imp.warnings);
    let mm = &imp.materials;
    let mesh = imp.model.root.mesh.as_ref().unwrap();
    let skin = crc("skin.bmp");
    let own = crc(&format!("{:08x}_mat2.bmp", 7));
    assert_eq!(mesh.textures, vec![skin, crc("glow.bmp"), own, crc("plain.bmp"), crc("00wate04.bmp")]);
    let img = |crc: u32| mm.textures.iter().find(|t| t.0 == crc).map(|t| &mm.images[t.1 as usize]);
    assert_eq!(img(skin).unwrap().rgba[..4], [200, 100, 50, 255]);
    assert_eq!(img(crc("glow.bmp")).unwrap().rgba[..4], [10, 20, 30, 255], "embedded data URI");
    assert_eq!(img(own).unwrap().rgba[..4], [200, 100, 50, 255], "a second material on the image");
    assert!(img(crc("00wate04.bmp")).is_none(), "retail textures come from the banks");
    assert_eq!(mm.materials.len(), 2);
    let p = &mm.materials[0];
    assert_eq!(p.crc, skin);
    assert_eq!((p.metallic, p.roughness, p.normal_scale, p.occlusion_strength, p.parallax_scale), (0.25, 0.5, 0.8, 0.5, 0.05));
    assert_eq!(p.base_color, [1.0, 0.5, 1.0, 1.0]);
    assert_eq!(p.emissive, [2.0, 1.0, 0.0]);
    let map = |i: Option<u32>| mm.images[i.unwrap() as usize].rgba[..4].to_vec();
    assert_eq!(map(p.normal), [128, 128, 255, 255]);
    assert_eq!(map(p.metal_rough), [10, 20, 30, 255]);
    assert_eq!(map(p.occlusion), [90, 90, 90, 255]);
    assert_eq!(p.occlusion, p.emissive_map, "one image, decoded once");
    assert_eq!(p.occlusion, p.height);
    let q = &mm.materials[1];
    assert_eq!(q.crc, crc("plain.bmp"));
    assert_eq!(map(q.height), [20, 20, 20, 255], "<stem>_height.png");
    assert_eq!(q.parallax_scale, crate::material::PARALLAX_SCALE);
    assert_eq!((q.metallic, q.roughness, q.normal), (0.0, 1.0, None));

    // The same as a .glb with the images in the binary chunk.
    let png_bytes = px([7, 8, 9, 255]);
    let mut glb_doc = serde_json::json!({
        "asset": {"version": "2.0"},
        "nodes": [{"name": "thing", "mesh": 0}],
        "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "material": 0}]}],
        "materials": [{"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}, "normalTexture": {"index": 0}}],
        "textures": [{"source": 0}],
        "images": [{"bufferView": 1, "mimeType": "image/png"}],
        "accessors": [{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"}],
    });
    let mut chunk: Vec<u8> = [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0].iter().flat_map(|f| f.to_le_bytes()).collect();
    let off = chunk.len();
    chunk.extend(&png_bytes);
    while !chunk.len().is_multiple_of(4) {
        chunk.push(0);
    }
    glb_doc["buffers"] = serde_json::json!([{"byteLength": chunk.len()}]);
    glb_doc["bufferViews"] =
        serde_json::json!([{"buffer": 0, "byteOffset": 0, "byteLength": 36}, {"buffer": 0, "byteOffset": off, "byteLength": png_bytes.len()}]);
    let mut js = serde_json::to_vec(&glb_doc).unwrap();
    while !js.len().is_multiple_of(4) {
        js.push(b' ');
    }
    let mut glb = b"glTF".to_vec();
    glb.extend(2u32.to_le_bytes());
    glb.extend(((12 + 8 + js.len() + 8 + chunk.len()) as u32).to_le_bytes());
    glb.extend((js.len() as u32).to_le_bytes());
    glb.extend(0x4E4F_534Au32.to_le_bytes());
    glb.extend(&js);
    glb.extend((chunk.len() as u32).to_le_bytes());
    glb.extend(0x004E_4942u32.to_le_bytes());
    glb.extend(&chunk);
    let imp = gltf::import(&gltf::GltfDoc::parse(&glb, None).unwrap(), Some(9), None).unwrap();
    let own = crc(&format!("{:08x}_mat0.bmp", 9));
    assert_eq!(imp.model.root.mesh.as_ref().unwrap().textures, vec![own], "an unnamed image: a CRC of its own");
    let mm = &imp.materials;
    assert_eq!(mm.textures, vec![(own, 0)]);
    assert_eq!(mm.images[0].rgba[..4], [7, 8, 9, 255]);
    assert_eq!(mm.materials[0].normal, Some(0));
    assert_eq!(mm.materials[0].metallic, 1.0, "glTF's default factors");
}

/// A synthetic skinned model like the retail frog: a root and an arm turned and moved at rest,
/// with a clip.
fn skinned_model() -> Model {
    use f2_formats::objbank::{Animation, Mesh, QKey, VKey};
    let o = |name: &str, t: Vec3, q: Quat| Object {
        id: crc(&format!("{name}.obe")),
        name: name.into(),
        xlu: 0,
        sprites: vec![],
        mesh: None,
        effected_verts: vec![],
        scale_keys: vec![VKey { v: [1.0; 3], time: 0 }],
        move_keys: vec![VKey { v: t, time: 0 }, VKey { v: [t[0], t[1] + 3.0, t[2]], time: 10 }],
        rotate_keys: vec![QKey { q, time: 0 }, QKey { q: [0.0, 0.0, 0.0, 1.0], time: 10 }],
        bbox: None,
        phong: 0,
        flags: 0,
        matrix: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]],
        children: vec![],
    };
    let s = (0.3f32).sin();
    let mut root = o("frog", [0.0, 5.0, 0.0], [0.0, s, 0.0, (1.0 - s * s).sqrt()]);
    let mut arm = o("fgshld", [4.0, 2.0, 1.0], [s, 0.0, 0.0, (1.0 - s * s).sqrt()]);
    arm.effected_verts = vec![3, 4, 5];
    root.effected_verts = vec![0, 1, 2];
    let vertices = vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.5], [0.0, 0.0, 0.0], [1.0, 3.0, 0.0], [0.0, 1.0, 2.0]];
    root.mesh = Some(Mesh {
        vertices,
        faces: vec![[0, 1, 2], [3, 4, 5], [1, 4, 2]],
        face_uv: (0..9).map(|i| [i * 100, 1000 - i * 50]).collect(),
        normals: vec![[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, -1.0, 0.0], [-1.0, 0.0, 0.0]],
        colors: (0..9).map(|i| [i as f32 / 9.0, 0.5, 1.0, 1.0]).collect(),
        face_flags: vec![0; 3],
        textures: vec![crc("a.bmp"), crc("b.bmp"), crc("a.bmp")],
    });
    root.children.push(arm);
    Model {
        id: crc("frog.obe"),
        radius: 9.0,
        object_size: 0,
        root,
        root_siblings: vec![],
        animations: vec![Animation { start: 0, end: 10, speed: 1.0, looped: 1 }],
        loc_mesh: None,
        is_skinned: true,
        num_vtx: 6,
    }
}

/// Mesh-only skin override: the exporter's own skin without its animations keeps the model and
/// reproduces its mesh exactly; a Blender-style document (bind-pose mesh in model space, inverse
/// bind matrices, bones reoriented, smooth weights) lands in the same joint spaces.
#[test]
fn gltf_mesh_only_skin_override() {
    let base = skinned_model();
    let out = f2_export::gltf::model_to_gltf(&base, "m.bin", &|_| None);
    // With its animations, the whole model is imported as before.
    let full = gltf::GltfDoc { json: out.json.clone(), buffers: vec![out.bin.clone()], base_dir: None };
    let imp = gltf::import(&full, Some(base.id), Some(&base)).unwrap();
    assert!(!imp.mesh_only);
    // Without: only the skin.
    let mut json = out.json.clone();
    json.as_object_mut().unwrap().remove("animations");
    let doc = gltf::GltfDoc { json, buffers: vec![out.bin.clone()], base_dir: None };
    let imp = gltf::import(&doc, Some(base.id), Some(&base)).unwrap();
    assert!(imp.mesh_only);
    assert!(imp.materials.skin.is_none(), "one joint per vertex: rigid");
    compare_model(&base, &imp.model).unwrap();
    let (a, b) = (base.root.mesh.as_ref().unwrap(), imp.model.root.mesh.as_ref().unwrap());
    let corners = |m: &f2_formats::objbank::Mesh| {
        let mut v: Vec<_> =
            (0..m.faces.len()).flat_map(|f| (0..3).map(move |k| (m.vertices[m.faces[f][k] as usize].map(f32::to_bits), m.face_uv[f * 3 + k]))).collect();
        v.sort();
        v
    };
    assert_eq!(corners(a), corners(b), "the skin's vertices bit for bit");
    assert_eq!(imp.model.root.children[0].move_keys, base.root.children[0].move_keys);
    assert_eq!(imp.model.animations, base.animations);

    // Blender-style: the same mesh at rest in model space, every bone turned 90° about x
    // (inverse bind matrices to match), the shoulder's vertices half on the root.
    let rest = gltf::tests_support::rest(&base);
    let turn = [[1.0, 0.0, 0.0, 0.0], [0.0, 0.0, -1.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
    // Column-vector rest matrices of the two bones, and the reoriented joints' worlds.
    let mut bin = Bin::default();
    let mut pos = Vec::new();
    let mut joints = Vec::new();
    let mut weights = Vec::new();
    for (v, p) in a.vertices.iter().enumerate() {
        let j = usize::from(v >= 3);
        let w = gltf::tests_support::xf(&rest[j], *p);
        pos.extend(w);
        joints.extend([j as u16, 0, 0, 0]);
        weights.extend(if v == 4 { [0.75, 0.25, 0.0, 0.0] } else { [1.0, 0.0, 0.0, 0.0] });
    }
    let tri: Vec<u16> = a.faces.iter().flatten().map(|&i| i as u16).collect();
    let p_acc = bin.f32s(&pos, "VEC3");
    let j_acc = bin.u16s(&joints);
    let w_acc = bin.f32s(&weights, "VEC4");
    // World of a reoriented joint: rest × turn; inverse bind: its inverse.
    let worlds: Vec<[[f64; 4]; 4]> = rest.iter().map(|m| gltf::tests_support::mul(m, &turn)).collect();
    let ibm: Vec<f32> = worlds.iter().flat_map(gltf::tests_support::inv).collect();
    let ibm_acc = bin.f32s(&ibm, "MAT4");
    // Indices: as floats-free u16 scalars.
    bin.accessors.push(serde_json::json!({"bufferView": 0, "byteOffset": bin.bytes.len(), "componentType": 5123, "count": tri.len(), "type": "SCALAR"}));
    bin.bytes.extend(tri.iter().flat_map(|i| i.to_le_bytes()));
    let i_acc = bin.accessors.len() - 1;
    let local = |w: &[[f64; 4]; 4], parent: Option<&[[f64; 4]; 4]>| {
        let l = match parent {
            Some(p) => gltf::tests_support::mul(&gltf::tests_support::inv64(p), w),
            None => *w,
        };
        (0..16).map(|i| l[i % 4][i / 4]).collect::<Vec<f64>>()
    };
    let doc = serde_json::json!({
        "asset": {"version": "2.0"},
        "scenes": [{"nodes": [0, 2]}],
        "nodes": [
            {"name": "frog", "matrix": local(&worlds[0], None), "children": [1]},
            {"name": "FGSHLD", "matrix": local(&worlds[1], Some(&worlds[0]))},
            {"name": "frog_skin", "mesh": 0, "skin": 0}
        ],
        "skins": [{"joints": [0, 1], "inverseBindMatrices": ibm_acc}],
        "meshes": [{"primitives": [{"attributes": {"POSITION": p_acc, "JOINTS_0": j_acc, "WEIGHTS_0": w_acc}, "indices": i_acc}]}],
    });
    let doc = gltf::GltfDoc { json: bin.finish(doc), buffers: vec![], base_dir: None };
    let doc = gltf::GltfDoc::parse(&serde_json::to_vec(&doc.json).unwrap(), None).unwrap();
    let imp = gltf::import(&doc, Some(base.id), Some(&base)).unwrap();
    assert!(imp.mesh_only);
    let m = imp.model.root.mesh.as_ref().unwrap();
    // Every vertex back in its object's space (as the retail one), within float error.
    for (o, obj) in [(0usize, &imp.model.root), (1, &imp.model.root.children[0])] {
        // (Without normals in the document a corner shared by two faces is welded per face.)
        assert!(obj.effected_verts.len() >= 3, "object {o}");
        for &v in &obj.effected_verts {
            let p = m.vertices[v as usize];
            assert!(a.vertices[o * 3..o * 3 + 3].iter().any(|q| close3(*q, p)), "object {o}: {p:?}");
        }
    }
    let w = imp.materials.skin.as_ref().expect("smooth weights");
    let v4 = imp.model.root.children[0].effected_verts.iter().map(|&v| v as usize).find(|&v| close3(m.vertices[v], a.vertices[4])).unwrap();
    assert_eq!((w.joints[v4], w.weights[v4]), ([1, 0, 1, 1], [191, 64, 0, 0]));
}

/// Real data: the retail frog's own export without its animations is a mesh-only override that
/// keeps the frog exactly (hierarchy, keys, clips, and the skin bit for bit).
#[test]
fn retail_frog_mesh_only() {
    let Some(g) = game_dir() else { return };
    let t = tempfile::tempdir().unwrap();
    let mut vfs = Vfs::new();
    vfs.add_layer("base", &g);
    let base = object_bank(&vfs, "frogger").unwrap().value;
    let i = model_names(&base).iter().position(|n| n == "frog").expect("frog model");
    let out = f2_export::gltf::model_to_gltf(&base.models[i], "frog.bin", &|_| None);
    let mut json = out.json;
    json.as_object_mut().unwrap().remove("animations");
    put(t.path(), "objects/frogger/frog.gltf", serde_json::to_string(&json).unwrap().as_bytes());
    put(t.path(), "objects/frogger/frog.bin", &out.bin);
    vfs.add_layer("mod", t.path());
    let l = object_bank(&vfs, "frogger").unwrap();
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);
    let (a, b) = (&base.models[i], &l.value.models[i]);
    compare_model(a, b).unwrap();
    assert_eq!(a.animations, b.animations);
    let (ma, mb) = (a.root.mesh.as_ref().unwrap(), b.root.mesh.as_ref().unwrap());
    let corners = |m: &f2_formats::objbank::Mesh| {
        let mut v: Vec<_> = (0..m.faces.len())
            .filter(|&f| m.faces[f].iter().all(|&i| i >= 0 && (i as usize) < m.vertices.len()))
            .flat_map(|f| (0..3).map(move |k| (m.vertices[m.faces[f][k] as usize].map(f32::to_bits), m.face_uv[f * 3 + k], m.textures[f])))
            .collect();
        v.sort();
        v
    };
    assert_eq!(corners(ma), corners(mb));
}

/// A mod's maps beside a bank texture (`textures/<bank>/<name>_normal.png`, `_height`, `_orm`,
/// `<name>.pbr.toml`) make that texture's material; they are not textures of their own.
#[test]
fn texture_materials_from_maps() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path().join("usa");
    let base = TexBank { textures: vec![tex("brick1.bmp", texture::texture_crc("brick1"), 0)], anims: vec![] };
    put(&r, "game/textures/ancient.fla", &texture::write_texbank(&base));
    let mods = t.path().join("mods");
    put(&mods, "m/mod.toml", b"id = \"m\"\nname = \"m\"\nversion = \"1.0.0\"\nengine_version = \"^1.0\"\n");
    put(&mods, "m/game/textures/ancient/brick1_normal.png", &png(2, 2, &[128, 128, 255, 255].repeat(4)));
    put(&mods, "m/game/textures/ancient/brick1_height.png", &png(2, 2, &[255, 255, 255, 255].repeat(4)));
    put(&mods, "m/game/textures/ancient/brick1.pbr.toml", b"parallax_scale = 0.05\nroughness = 0.4\n");
    let (vfs, _) = f2_mods::load_enabled_mods(&r, &mods, &["m"]).unwrap();
    let tb = tex_bank(&vfs, "ancient").unwrap();
    assert!(tb.added.is_empty() && tb.replaced.is_empty(), "maps are not textures: {:?} {:?}", tb.added, tb.replaced);
    let m = tex_materials(&vfs, "ancient").unwrap();
    assert!(m.warnings.is_empty(), "{:?}", m.warnings);
    let mm = &m.value;
    assert_eq!(mm.model_id, f2_formats::material::MODEL_ANY);
    assert_eq!(mm.materials.len(), 1);
    let p = &mm.materials[0];
    assert_eq!(p.crc, texture::texture_crc("brick1"));
    assert!(p.normal.is_some() && p.height.is_some() && p.metal_rough.is_none());
    assert_eq!((p.parallax_scale, p.roughness), (0.05, 0.4));
}
