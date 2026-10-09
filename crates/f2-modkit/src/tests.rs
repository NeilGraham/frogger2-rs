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
            let doc = gltf::GltfDoc { json: out.json, buffers: vec![out.bin] };
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
