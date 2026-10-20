//! glTF materials → face texture CRCs + [`ModelMaterials`] (the document's own images and
//! physically based materials for the Enhanced GPU mesh pass; see `docs/modding.md` §6).
//!
//! - The base-colour image of a material, when the document has it (embedded, data URI, or a
//!   PNG next to the document), becomes the faces' texture: CRC from the material as before
//!   (`extras.textureCrc`, `tex_XXXXXXXX`, `crc("<image stem>.bmp")`), or a CRC of its own
//!   when nothing names it. Images outside the model's directory (`..`, the exporter's shared
//!   `tex/`) are not read: the texture comes from the banks by name, as before.
//! - A material is physically based when it has a normal, metallic-roughness, occlusion,
//!   emissive or height map, an emissive factor, or explicit metallic / roughness factors
//!   other than the exporter's (0, 1); `extras.pbr` (bool) forces it either way.
//! - Height (parallax occlusion): `extras.heightTexture` = `{"index": <texture>}`, a texture
//!   index, or a PNG file name next to the document; else `<base colour stem>_height.png`
//!   next to the document when there is one. Depth: `extras.parallaxScale` (default 0.03).
//! - Two materials naming the same texture but looking different: the second gets a CRC of
//!   its own (showing the same image).

use std::collections::HashMap;

use f2_formats::crc::crc;
use f2_formats::material::{MatImage, ModelMaterials, PbrMaterial};
use serde_json::Value;

use crate::gltf::{arr, base64_decode, err, f3, idx, material_crc, percent_decode, rel_path, GltfDoc};
use crate::Error;

/// Default `extras.parallaxScale`.
pub const PARALLAX_SCALE: f32 = 0.03;

/// The decoded images, each once.
#[derive(Default)]
struct Images {
    by_index: HashMap<usize, Option<u32>>,
    by_file: HashMap<String, Option<u32>>,
    list: Vec<MatImage>,
}

pub(crate) fn png_image(name: String, bytes: &[u8]) -> Result<MatImage, Error> {
    if bytes.starts_with(&[0xFF, 0xD8]) {
        return Err(err("JPEG images are not supported (export PNG)"));
    }
    let img = crate::texture::decode_png(bytes)?;
    Ok(MatImage { name, width: img.width, height: img.height, rgba: img.pixels })
}

/// File stem of a URI or file name (`""`: none).
fn stem_of(u: &str) -> String {
    let u = percent_decode(u);
    let file = u.rsplit(['/', '\\']).next().unwrap_or(&u);
    match file.rsplit_once('.') {
        Some((s, _)) if !s.is_empty() => s.to_string(),
        _ => file.to_string(),
    }
}

/// glTF image `gi`'s name: its `name`, else its file's stem.
fn image_stem(doc: &GltfDoc, gi: usize) -> Option<String> {
    let img = doc.json["images"].get(gi)?;
    img["name"].as_str().map(stem_of).or_else(|| img["uri"].as_str().filter(|u| !u.starts_with("data:")).map(stem_of)).filter(|s| !s.is_empty())
}

impl Images {
    /// A PNG next to the document (`None`: not there, or outside its directory).
    fn file(&mut self, doc: &GltfDoc, rel: &str, warnings: &mut Vec<String>, quiet_missing: bool) -> Option<u32> {
        if let Some(&r) = self.by_file.get(rel) {
            return r;
        }
        let r = match rel_path(doc.base_dir.as_deref(), rel, "image") {
            Ok(p) => match std::fs::read(&p) {
                Ok(bytes) => match png_image(stem_of(rel), &bytes) {
                    Ok(img) => {
                        self.list.push(img);
                        Some(self.list.len() as u32 - 1)
                    }
                    Err(e) => {
                        warnings.push(format!("image {rel:?}: {e}"));
                        None
                    }
                },
                Err(e) => {
                    if !quiet_missing {
                        warnings.push(format!("image {}: {e}", p.display()));
                    }
                    None
                }
            },
            // Outside the model's directory (the exporter's `../tex/`), or no directory: the
            // banks' texture of that name is used.
            Err(_) => None,
        };
        self.by_file.insert(rel.to_string(), r);
        r
    }

    /// glTF image `gi`.
    fn get(&mut self, doc: &GltfDoc, gi: usize, warnings: &mut Vec<String>) -> Option<u32> {
        if let Some(&r) = self.by_index.get(&gi) {
            return r;
        }
        let Some(img) = doc.json["images"].get(gi) else {
            warnings.push(format!("image {gi} missing"));
            self.by_index.insert(gi, None);
            return None;
        };
        let name = image_stem(doc, gi).unwrap_or_default();
        let r = if let Some(vi) = idx(&img["bufferView"]) {
            let v = &doc.json["bufferViews"][vi];
            let (o, l) = (v["byteOffset"].as_u64().unwrap_or(0) as usize, v["byteLength"].as_u64().unwrap_or(0) as usize);
            let bytes = doc.buffers.get(v["buffer"].as_u64().unwrap_or(0) as usize).and_then(|b| b.get(o..o + l));
            match bytes.ok_or_else(|| err("image buffer view beyond its buffer")).and_then(|b| png_image(name, b)) {
                Ok(img) => {
                    self.list.push(img);
                    Some(self.list.len() as u32 - 1)
                }
                Err(e) => {
                    warnings.push(format!("image {gi}: {e}"));
                    None
                }
            }
        } else {
            match img["uri"].as_str() {
                Some(u) if u.starts_with("data:") => {
                    let r = u.split_once(";base64,").ok_or_else(|| err("only base64 data URIs are supported")).and_then(|(_, p)| base64_decode(p));
                    match r.and_then(|b| png_image(name, &b)) {
                        Ok(img) => {
                            self.list.push(img);
                            Some(self.list.len() as u32 - 1)
                        }
                        Err(e) => {
                            warnings.push(format!("image {gi}: {e}"));
                            None
                        }
                    }
                }
                Some(u) => self.file(doc, u, warnings, false),
                None => None,
            }
        };
        self.by_index.insert(gi, r);
        r
    }
}

/// Image of a texture reference (`{"index": t}`, or a bare texture index).
fn tex_source(doc: &GltfDoc, info: &Value) -> Option<usize> {
    let t = idx(&info["index"]).or_else(|| idx(info))?;
    idx(&doc.json["textures"].get(t)?["source"])
}

/// What a material's PBR factors and maps are, when it is physically based.
fn pbr_of(doc: &GltfDoc, m: &Value, images: &mut Images, warnings: &mut Vec<String>) -> Option<PbrMaterial> {
    let pmr = &m["pbrMetallicRoughness"];
    let ex = &m["extras"];
    let set = |v: &Value| !v.is_null();
    let num = |v: &Value, d: f32| v.as_f64().map_or(d, |x| x as f32);
    let (metallic, roughness) = (num(&pmr["metallicFactor"], 1.0), num(&pmr["roughnessFactor"], 1.0));
    let strength = num(&m["extensions"]["KHR_materials_emissive_strength"]["emissiveStrength"], 1.0);
    let emissive = f3(&m["emissiveFactor"]).unwrap_or([0.0; 3]).map(|c| c * strength);
    // The height map: named in the extras, or next to the base colour image.
    let base_gi = tex_source(doc, &pmr["baseColorTexture"]);
    let height_ref = &ex["heightTexture"];
    let has_height = set(height_ref)
        || base_gi
            .and_then(|g| image_stem(doc, g))
            .is_some_and(|s| doc.base_dir.as_ref().is_some_and(|d| rel_path(Some(d), &format!("{s}_height.png"), "image").is_ok_and(|p| p.is_file())));
    let factors = (set(&pmr["metallicFactor"]) || set(&pmr["roughnessFactor"])) && (metallic, roughness) != (0.0, 1.0);
    let maps = set(&m["normalTexture"]) || set(&m["occlusionTexture"]) || set(&m["emissiveTexture"]) || set(&pmr["metallicRoughnessTexture"]);
    if !ex["pbr"].as_bool().unwrap_or(maps || factors || has_height || emissive.iter().any(|&c| c > 0.0)) {
        return None;
    }
    let mut map = |info: &Value, what: &str, warnings: &mut Vec<String>| -> Option<u32> {
        if !set(info) {
            return None;
        }
        if info["texCoord"].as_u64().unwrap_or(0) != 0 {
            warnings.push(format!("{what}: only texCoord 0 is supported"));
        }
        let gi = tex_source(doc, info);
        if gi.is_none() {
            warnings.push(format!("{what}: no PNG image"));
        }
        gi.and_then(|g| images.get(doc, g, warnings))
    };
    let normal = map(&m["normalTexture"], "normalTexture", warnings);
    let metal_rough = map(&pmr["metallicRoughnessTexture"], "metallicRoughnessTexture", warnings);
    let occlusion = map(&m["occlusionTexture"], "occlusionTexture", warnings);
    let emissive_map = map(&m["emissiveTexture"], "emissiveTexture", warnings);
    let height = match height_ref {
        Value::String(f) => images.file(doc, f, warnings, false),
        Value::Null => base_gi.and_then(|g| image_stem(doc, g)).and_then(|s| images.file(doc, &format!("{s}_height.png"), warnings, true)),
        r => map(r, "heightTexture", warnings),
    };
    Some(PbrMaterial {
        crc: 0,
        base_color: pmr["baseColorFactor"]
            .as_array()
            .filter(|a| a.len() == 4)
            .map_or([1.0; 4], |a| std::array::from_fn(|i| a[i].as_f64().unwrap_or(1.0) as f32)),
        metallic: metallic.clamp(0.0, 1.0),
        roughness: roughness.clamp(0.0, 1.0),
        normal_scale: num(&m["normalTexture"]["scale"], 1.0),
        occlusion_strength: num(&m["occlusionTexture"]["strength"], 1.0).clamp(0.0, 1.0),
        emissive,
        parallax_scale: if height.is_some() { num(&ex["parallaxScale"], PARALLAX_SCALE) } else { 0.0 },
        normal,
        metal_rough,
        occlusion,
        emissive_map,
        height,
    })
}

/// The face texture CRC of every glTF material, and what the model brings for them.
pub(crate) fn resolve(doc: &GltfDoc, model_id: u32, warnings: &mut Vec<String>) -> (Vec<u32>, ModelMaterials) {
    let mut images = Images::default();
    let mut out = ModelMaterials { model_id, ..Default::default() };
    let mut crcs = Vec::new();
    // Per face CRC: the material look it has (`None`: plain).
    let mut seen: HashMap<u32, Option<PbrMaterial>> = HashMap::new();
    for (mi, m) in arr(&doc.json, "materials").iter().enumerate() {
        let own = || crc(&format!("{model_id:08x}_mat{mi}.bmp"));
        let mut c = material_crc(doc, Some(mi));
        let base = tex_source(doc, &m["pbrMetallicRoughness"]["baseColorTexture"]).and_then(|g| images.get(doc, g, warnings));
        if c == 0 && base.is_some() {
            c = own();
        }
        let pbr = pbr_of(doc, m, &mut images, warnings);
        if let Some(prev) = seen.get(&c) {
            if *prev != pbr && c != 0 {
                let n = own();
                if base.is_none() {
                    out.aliases.push((n, c));
                }
                c = n;
            }
        }
        seen.entry(c).or_insert_with(|| pbr.clone());
        if let Some(i) = base {
            if !out.textures.contains(&(c, i)) {
                out.textures.push((c, i));
            }
        }
        if let Some(mut p) = pbr {
            p.crc = c;
            if !out.materials.iter().any(|x| x.crc == c) {
                out.materials.push(p);
            }
        }
        crcs.push(c);
    }
    out.images = images.list;
    (crcs, out)
}

/// Starting material maps for the texture in `tex` (a PNG; e.g. your own export of a retail
/// texture), made like the port's built-in ones ([`f2_formats::pbrgen`]), written to `out` as
/// `<name>_normal.png`, `<name>_height.png`, `<name>_orm.png` and `<name>.pbr.toml`: the files
/// a mod puts beside a bank texture (`textures/<bank>/`). Returns `<name>`.
pub fn write_pbr_maps(tex: &std::path::Path, out: &std::path::Path, size: u32, recipe: &f2_formats::pbrgen::Recipe) -> Result<String, Error> {
    let data = std::fs::read(tex).map_err(|e| err(format!("{}: {e}", tex.display())))?;
    let img = crate::texture::decode_png(&data)?;
    let maps = f2_formats::pbrgen::generate(&img.pixels, img.width, img.height, size, recipe);
    let name = tex.file_stem().and_then(|s| s.to_str()).unwrap_or("texture").to_ascii_lowercase();
    std::fs::create_dir_all(out).map_err(|e| err(format!("{}: {e}", out.display())))?;
    for (kind, px) in [("normal", &maps.normal), ("height", &maps.height), ("orm", &maps.orm)] {
        let path = out.join(format!("{name}_{kind}.png"));
        let f = std::fs::File::create(&path).map_err(|e| err(format!("{}: {e}", path.display())))?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(f), size, size);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().and_then(|mut w| w.write_image_data(px)).map_err(|e| err(format!("{}: {e}", path.display())))?;
    }
    let toml = format!(
        "# Made by f2-modkit pbr-maps (edit freely). The maps: <name>_normal.png (tangent space, OpenGL +Y),\n\
         # <name>_height.png (white = high), <name>_orm.png (R occlusion, G roughness, B metal).\n\
         parallax_scale = {}\nroughness = 1.0\nmetallic = 1.0\nnormal_scale = 1.0\nocclusion_strength = 1.0\n",
        recipe.depth
    );
    std::fs::write(out.join(format!("{name}.pbr.toml")), toml).map_err(|e| err(e.to_string()))?;
    Ok(name)
}
