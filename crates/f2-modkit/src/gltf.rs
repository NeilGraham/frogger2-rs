//! glTF 2.0 (`.gltf` + buffers, or `.glb`) → `f2_formats::objbank::Model`.
//!
//! The inverse of `f2-export`'s writer (`tools/f2-export/src/gltf.rs`), and
//! general enough for Blender exports:
//!
//! - Scene root nodes become the root object and its `root_siblings`; node
//!   children become object children. Nodes carrying a `skin` are skin-mesh
//!   holders, not objects.
//! - Coordinates are taken raw (Direct3D left-handed, no axis flip), like the
//!   exporter writes them. UV = value / 0.000975. Colours are RGBA floats.
//! - Triangles are welded into MDX vertices by (position, normal, joint); per
//!   face corners keep their own UVs and colours, as in `MDX_MESH`.
//! - Texture CRC per face from the material: `extras.textureCrc`, a material
//!   named `tex_XXXXXXXX`, or the base-colour image name/URI stem → `crc("<stem>.bmp")`.
//! - Animations: key times are seconds × 60 game frames, offset by the clip's
//!   `extras.start`. Animations with `extras.speed` (or no extras) become
//!   `MDX_ANIMATION` clips (`start`/`end`/`speed`/`loop` from extras, else laid
//!   out back to back with speed 1 and loop 1); the exporter's keys-only "all"
//!   animation (start/end but no speed) adds keys but no clip.
//! - Objects without a channel for a path get a single key (time 0) from the
//!   node's rest transform when the node specifies one.
//! - Skins: every skin vertex is assigned to its highest-weight joint
//!   (`effectedVerts`); vertices are moved into that joint's local space with
//!   the inverse bind matrix (identity in exporter output).
//! - Per-object `extras` (`objectId`, `flags`, `xlu`, `phongTextureCrc`,
//!   `sprites`) and root `extras` (`modelId`, `radius`, `objectSize`) are honoured.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use f2_formats::crc::crc;
use f2_formats::objbank::{Animation, Mesh, Model, ObjSprite, Object, QKey, VKey, Vec3};
use serde_json::Value;

use crate::texture::texture_crc;
use crate::Error;

/// Game frames per second of key times.
pub const FPS: f32 = 60.0;
/// `MDX_MESH.faceTC` scale.
pub const UV_SCALE: f32 = 0.000975;

/// A parsed glTF document with its buffers loaded.
pub struct GltfDoc {
    pub json: Value,
    pub buffers: Vec<Vec<u8>>,
}

fn err(m: impl Into<String>) -> Error {
    Error::new(format!("gltf: {}", m.into()))
}

fn base64_decode(s: &str) -> Result<Vec<u8>, Error> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut n) = (0u32, 0);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return Err(err("bad base64 in data URI")),
        };
        acc = (acc << 6) | u32::from(v);
        n += 6;
        if n >= 8 {
            n -= 8;
            out.push((acc >> n) as u8);
        }
    }
    Ok(out)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl GltfDoc {
    /// Parses `.gltf` JSON or `.glb`. External buffer URIs are resolved
    /// against `base_dir` and must be relative paths without `..`.
    pub fn parse(data: &[u8], base_dir: Option<&Path>) -> Result<GltfDoc, Error> {
        let (json_bytes, blob): (&[u8], Option<&[u8]>) = if data.starts_with(b"glTF") {
            let rd = |o: usize| data.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize).ok_or_else(|| err("truncated glb"));
            let total = rd(8)?.min(data.len());
            let mut o = 12;
            let (mut js, mut bin) = (None, None);
            while o + 8 <= total {
                let len = rd(o)?;
                let ty = rd(o + 4)?;
                let body = data.get(o + 8..o + 8 + len).ok_or_else(|| err("glb chunk beyond file"))?;
                match ty {
                    0x4E4F_534A => js = Some(body),
                    0x004E_4942 => bin = Some(body),
                    _ => {}
                }
                o += 8 + len;
            }
            (js.ok_or_else(|| err("glb without JSON chunk"))?, bin)
        } else {
            (data, None)
        };
        let json: Value = serde_json::from_slice(json_bytes).map_err(|e| err(format!("json: {e}")))?;
        let mut buffers = Vec::new();
        for (i, b) in arr(&json, "buffers").iter().enumerate() {
            let len = b["byteLength"].as_u64().unwrap_or(0) as usize;
            let data = match b["uri"].as_str() {
                None => blob.ok_or_else(|| err(format!("buffer {i} has no uri and there is no GLB chunk")))?.to_vec(),
                Some(u) if u.starts_with("data:") => {
                    let (_, payload) = u.split_once(";base64,").ok_or_else(|| err("only base64 data URIs are supported"))?;
                    base64_decode(payload)?
                }
                Some(u) => {
                    let rel = percent_decode(u);
                    let comps = f2_mods::normalize(&rel).ok_or_else(|| err(format!("buffer uri {u:?} escapes the model directory")))?;
                    if rel.starts_with('/') || rel.starts_with('\\') {
                        return Err(err(format!("buffer uri {u:?} is absolute")));
                    }
                    let dir = base_dir.ok_or_else(|| err(format!("external buffer {u:?} needs a base directory")))?;
                    let p = comps.iter().fold(dir.to_path_buf(), |p, c| p.join(c));
                    std::fs::read(&p).map_err(|e| err(format!("{}: {e}", p.display())))?
                }
            };
            if data.len() < len {
                return Err(err(format!("buffer {i}: {} bytes, byteLength says {len}", data.len())));
            }
            buffers.push(data);
        }
        Ok(GltfDoc { json, buffers })
    }

    /// Reads a file and its relative buffers.
    pub fn load(path: &Path) -> Result<GltfDoc, Error> {
        let data = std::fs::read(path).map_err(|e| err(format!("{}: {e}", path.display())))?;
        GltfDoc::parse(&data, path.parent())
    }

    /// Accessor elements as f32 (normalized integers mapped to [0,1]/[-1,1]).
    /// Returns (values, components per element).
    fn floats(&self, idx: usize) -> Result<(Vec<f32>, usize), Error> {
        let a = self.json["accessors"].get(idx).ok_or_else(|| err(format!("accessor {idx} missing")))?;
        let comps = match a["type"].as_str().unwrap_or("") {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            "VEC4" => 4,
            "MAT4" => 16,
            t => return Err(err(format!("accessor {idx}: unsupported type {t}"))),
        };
        let count = a["count"].as_u64().unwrap_or(0) as usize;
        let ct = a["componentType"].as_u64().unwrap_or(0);
        let norm = a["normalized"].as_bool().unwrap_or(false);
        let csize = match ct {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            5125 | 5126 => 4,
            _ => return Err(err(format!("accessor {idx}: bad componentType {ct}"))),
        };
        if a.get("sparse").is_some() {
            return Err(err(format!("accessor {idx}: sparse accessors are not supported")));
        }
        let mut out = vec![0f32; count * comps];
        let Some(vi) = a["bufferView"].as_u64() else { return Ok((out, comps)) };
        let v = self.json["bufferViews"].get(vi as usize).ok_or_else(|| err(format!("bufferView {vi} missing")))?;
        let buf = self.buffers.get(v["buffer"].as_u64().unwrap_or(0) as usize).ok_or_else(|| err("buffer missing"))?;
        let base = v["byteOffset"].as_u64().unwrap_or(0) as usize + a["byteOffset"].as_u64().unwrap_or(0) as usize;
        let stride = v["byteStride"].as_u64().map(|s| s as usize).unwrap_or(comps * csize);
        let vlen = v["byteLength"].as_u64().unwrap_or(0) as usize;
        let vend = v["byteOffset"].as_u64().unwrap_or(0) as usize + vlen;
        if count > 0 && (base + (count - 1) * stride + comps * csize > vend.min(buf.len())) {
            return Err(err(format!("accessor {idx} overruns its buffer view")));
        }
        for i in 0..count {
            for c in 0..comps {
                let o = base + i * stride + c * csize;
                let b = &buf[o..o + csize];
                out[i * comps + c] = match ct {
                    5126 => f32::from_le_bytes(b.try_into().unwrap()),
                    5121 if norm => f32::from(b[0]) / 255.0,
                    5121 => f32::from(b[0]),
                    5120 if norm => (f32::from(b[0] as i8) / 127.0).max(-1.0),
                    5120 => f32::from(b[0] as i8),
                    5123 if norm => f32::from(u16::from_le_bytes([b[0], b[1]])) / 65535.0,
                    5123 => f32::from(u16::from_le_bytes([b[0], b[1]])),
                    5122 if norm => (f32::from(i16::from_le_bytes([b[0], b[1]])) / 32767.0).max(-1.0),
                    5122 => f32::from(i16::from_le_bytes([b[0], b[1]])),
                    _ => u32::from_le_bytes(b.try_into().unwrap()) as f32,
                };
            }
        }
        Ok((out, comps))
    }
}

fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v[k].as_array().map(|a| a.as_slice()).unwrap_or(&[])
}

fn idx(v: &Value) -> Option<usize> {
    v.as_u64().map(|x| x as usize)
}

fn f3(v: &Value) -> Option<Vec3> {
    let a = v.as_array()?;
    (a.len() == 3).then(|| [a[0].as_f64().unwrap_or(0.0) as f32, a[1].as_f64().unwrap_or(0.0) as f32, a[2].as_f64().unwrap_or(0.0) as f32])
}

fn f4(v: &Value) -> Option<[f32; 4]> {
    let a = v.as_array()?;
    (a.len() == 4).then(|| std::array::from_fn(|i| a[i].as_f64().unwrap_or(0.0) as f32))
}

/// Rest TRS of a node; each component is `None` when the node leaves it at the default.
fn node_trs(n: &Value) -> (Option<Vec3>, Option<[f32; 4]>, Option<Vec3>) {
    if let Some(m) = n["matrix"].as_array().filter(|m| m.len() == 16) {
        let m: Vec<f32> = m.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect();
        let col = |c: usize| [m[c * 4], m[c * 4 + 1], m[c * 4 + 2]];
        let len = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        let s = [len(col(0)), len(col(1)), len(col(2))];
        let r: [[f32; 3]; 3] = std::array::from_fn(|c| {
            let v = col(c);
            let l = if s[c] > 1e-12 { s[c] } else { 1.0 };
            [v[0] / l, v[1] / l, v[2] / l]
        });
        // r[c][row]: column-major rotation matrix → quaternion.
        let (m00, m11, m22) = (r[0][0], r[1][1], r[2][2]);
        let q = if m00 + m11 + m22 > 0.0 {
            let t = (1.0 + m00 + m11 + m22).sqrt() * 2.0;
            [(r[1][2] - r[2][1]) / t, (r[2][0] - r[0][2]) / t, (r[0][1] - r[1][0]) / t, 0.25 * t]
        } else if m00 > m11 && m00 > m22 {
            let t = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
            [0.25 * t, (r[1][0] + r[0][1]) / t, (r[2][0] + r[0][2]) / t, (r[1][2] - r[2][1]) / t]
        } else if m11 > m22 {
            let t = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
            [(r[1][0] + r[0][1]) / t, 0.25 * t, (r[2][1] + r[1][2]) / t, (r[2][0] - r[0][2]) / t]
        } else {
            let t = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
            [(r[2][0] + r[0][2]) / t, (r[2][1] + r[1][2]) / t, 0.25 * t, (r[0][1] - r[1][0]) / t]
        };
        return (Some([m[12], m[13], m[14]]), Some(q), Some(s));
    }
    (f3(&n["translation"]), f4(&n["rotation"]), f3(&n["scale"]))
}

fn material_crc(doc: &GltfDoc, mat: Option<usize>) -> u32 {
    let Some(m) = mat.and_then(|m| doc.json["materials"].get(m)) else { return 0 };
    if let Some(c) = m["extras"]["textureCrc"].as_u64() {
        return c as u32;
    }
    if let Some(h) = m["name"].as_str().and_then(|n| n.strip_prefix("tex_")).filter(|h| h.len() == 8) {
        if let Ok(c) = u32::from_str_radix(h, 16) {
            return c;
        }
    }
    let img = idx(&m["pbrMetallicRoughness"]["baseColorTexture"]["index"])
        .and_then(|t| doc.json["textures"].get(t))
        .and_then(|t| idx(&t["source"]))
        .and_then(|i| doc.json["images"].get(i));
    let Some(img) = img else { return 0 };
    let name = img["name"].as_str().map(str::to_string).or_else(|| {
        img["uri"].as_str().filter(|u| !u.starts_with("data:")).map(|u| {
            let u = percent_decode(u);
            let file = u.rsplit(['/', '\\']).next().unwrap_or(&u).to_string();
            file
        })
    });
    match name {
        Some(n) => {
            let stem = match n.rsplit_once('.') {
                Some((s, _)) if !s.is_empty() => s.to_string(),
                _ => n,
            };
            texture_crc(&stem)
        }
        None => 0,
    }
}

/// Column-major 4×4 × point.
fn xform(m: &[f32], v: Vec3) -> Vec3 {
    std::array::from_fn(|i| m[i] * v[0] + m[4 + i] * v[1] + m[8 + i] * v[2] + m[12 + i])
}

fn xform_dir(m: &[f32], v: Vec3) -> Vec3 {
    std::array::from_fn(|i| m[i] * v[0] + m[4 + i] * v[1] + m[8 + i] * v[2])
}

fn face_normal(a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0]];
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if l > 1e-12 {
        n.map(|x| x / l)
    } else {
        [0.0, 1.0, 0.0]
    }
}

/// Skinning info for [`convert_mesh`]: joint node per joint index and its inverse bind matrix.
struct SkinInfo {
    ibm: Vec<Vec<f32>>,
}

/// Converts a glTF mesh. Returns the MDX mesh and the joint (skin joint
/// index) of each MDX vertex when `skin` is given.
fn convert_mesh(doc: &GltfDoc, mesh_idx: usize, skin: Option<&SkinInfo>) -> Result<(Mesh, Vec<u16>), Error> {
    let m = doc.json["meshes"].get(mesh_idx).ok_or_else(|| err(format!("mesh {mesh_idx} missing")))?;
    let mut mesh = Mesh { vertices: vec![], faces: vec![], face_uv: vec![], normals: vec![], colors: vec![], face_flags: vec![], textures: vec![] };
    let mut joints_out = Vec::new();
    let mut weld: HashMap<([u32; 3], [u32; 3], u16), i16> = HashMap::new();
    for (pi, p) in arr(m, "primitives").iter().enumerate() {
        let mode = p["mode"].as_u64().unwrap_or(4);
        if mode != 4 {
            return Err(err(format!("mesh {mesh_idx} primitive {pi}: mode {mode} (only triangle lists are supported)")));
        }
        let at = &p["attributes"];
        let pos_i = idx(&at["POSITION"]).ok_or_else(|| err(format!("mesh {mesh_idx} primitive {pi}: no POSITION")))?;
        let (pos, _) = doc.floats(pos_i)?;
        let nv = pos.len() / 3;
        let get = |k: &str| -> Result<Option<(Vec<f32>, usize)>, Error> { idx(&at[k]).map(|i| doc.floats(i)).transpose() };
        let nrm = get("NORMAL")?;
        let uv = get("TEXCOORD_0")?;
        let col = get("COLOR_0")?;
        let (jts, wts) = (get("JOINTS_0")?, get("WEIGHTS_0")?);
        let indices: Vec<usize> = match idx(&p["indices"]) {
            Some(i) => doc.floats(i)?.0.into_iter().map(|v| v as usize).collect(),
            None => (0..nv).collect(),
        };
        if indices.iter().any(|&i| i >= nv) {
            return Err(err(format!("mesh {mesh_idx} primitive {pi}: index out of range")));
        }
        let crc = material_crc(doc, idx(&p["material"]));
        let flag = p["extras"]["faceFlags"]
            .as_u64()
            .or_else(|| idx(&p["material"]).and_then(|mi| doc.json["materials"][mi]["extras"]["faceFlags"].as_u64()))
            .unwrap_or(0) as u8;
        for tri in indices.as_chunks::<3>().0.iter() {
            let corner_pos = |i: usize| -> Vec3 { [pos[3 * i], pos[3 * i + 1], pos[3 * i + 2]] };
            let fnrm = face_normal(corner_pos(tri[0]), corner_pos(tri[1]), corner_pos(tri[2]));
            let mut face = [0i16; 3];
            for (k, &vi) in tri.iter().enumerate() {
                let mut v = corner_pos(vi);
                let mut n = nrm.as_ref().map(|(d, c)| [d[c * vi], d[c * vi + 1], d[c * vi + 2]]).unwrap_or(fnrm);
                let joint = match (&jts, skin) {
                    (Some((j, jc)), Some(s)) => {
                        let w = wts.as_ref().map(|(w, wc)| &w[wc * vi..wc * vi + (*wc).min(*jc)]);
                        let best = match w {
                            Some(w) => (0..w.len()).max_by(|&a, &b| w[a].total_cmp(&w[b])).unwrap_or(0),
                            None => 0,
                        };
                        let jn = j[jc * vi + best] as u16;
                        if let Some(m) = s.ibm.get(jn as usize) {
                            v = xform(m, v);
                            n = xform_dir(m, n);
                        }
                        jn
                    }
                    _ => 0,
                };
                let key = (v.map(f32::to_bits), n.map(f32::to_bits), joint);
                let id = match weld.get(&key) {
                    Some(&i) => i,
                    None => {
                        if mesh.vertices.len() >= i16::MAX as usize {
                            return Err(err(format!("mesh {mesh_idx}: more than {} vertices", i16::MAX)));
                        }
                        let i = mesh.vertices.len() as i16;
                        mesh.vertices.push(v);
                        mesh.normals.push(n);
                        joints_out.push(joint);
                        weld.insert(key, i);
                        i
                    }
                };
                face[k] = id;
                let t = uv.as_ref().map(|(d, c)| [d[c * vi], d[c * vi + 1]]).unwrap_or([0.0, 0.0]);
                mesh.face_uv.push(t.map(|x| (x / UV_SCALE).round().clamp(i16::MIN as f32, i16::MAX as f32) as i16));
                let c = col
                    .as_ref()
                    .map(
                        |(d, c)| {
                            if *c == 4 {
                                [d[4 * vi], d[4 * vi + 1], d[4 * vi + 2], d[4 * vi + 3]]
                            } else {
                                [d[3 * vi], d[3 * vi + 1], d[3 * vi + 2], 1.0]
                            }
                        },
                    )
                    .unwrap_or([1.0; 4]);
                mesh.colors.push(c);
            }
            mesh.faces.push(face);
            mesh.textures.push(crc);
            mesh.face_flags.push(flag);
        }
    }
    Ok((mesh, joints_out))
}

/// Per-object track being assembled: frame → value.
#[derive(Default)]
struct Tracks {
    mv: BTreeMap<i32, Vec3>,
    sc: BTreeMap<i32, Vec3>,
    rot: BTreeMap<i32, [f32; 4]>,
}

fn obj_extras(o: &mut Object, e: &Value) {
    if let Some(v) = e["objectId"].as_u64() {
        o.id = v as u32;
    }
    if let Some(v) = e["flags"].as_i64() {
        o.flags = v as i16;
    }
    if let Some(v) = e["xlu"].as_u64() {
        o.xlu = v as u8;
    }
    if let Some(v) = e["phongTextureCrc"].as_u64() {
        o.phong = v as u32;
    }
    let i16s = |v: &Value| -> Vec<i16> { v.as_array().map(|a| a.iter().map(|x| x.as_i64().unwrap_or(0) as i16).collect()).unwrap_or_default() };
    for s in arr(e, "sprites") {
        let (p, sz, off) = (i16s(&s["pos"]), i16s(&s["size"]), i16s(&s["offset"]));
        o.sprites.push(ObjSprite {
            texture: s["textureCrc"].as_u64().unwrap_or(0) as u32,
            pos: std::array::from_fn(|i| p.get(i).copied().unwrap_or(0)),
            size: std::array::from_fn(|i| sz.get(i).copied().unwrap_or(0)),
            offset: std::array::from_fn(|i| off.get(i).copied().unwrap_or(0)),
            flags: s["flags"].as_i64().unwrap_or(0) as i16,
        });
    }
}

fn bbox_of(m: &Mesh) -> Option<[Vec3; 8]> {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for v in &m.vertices {
        for i in 0..3 {
            lo[i] = lo[i].min(v[i]);
            hi[i] = hi[i].max(v[i]);
        }
    }
    if !lo[0].is_finite() {
        return None;
    }
    Some(std::array::from_fn(|i| [if i & 1 != 0 { hi[0] } else { lo[0] }, if i & 2 != 0 { hi[1] } else { lo[1] }, if i & 4 != 0 { hi[2] } else { lo[2] }]))
}

const IDENTITY: [[f32; 4]; 4] = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];

/// Imports a model. `model_id` is used unless the document's root `extras.modelId`
/// is wanted instead (pass `None`); falls back to `crc("<scene name>.obe")`.
pub fn import_model(doc: &GltfDoc, model_id: Option<u32>) -> Result<Model, Error> {
    let j = &doc.json;
    let nodes = arr(j, "nodes");
    if nodes.is_empty() {
        return Err(err("no nodes"));
    }
    let scene_i = idx(&j["scene"]).unwrap_or(0);
    let scene_nodes: Vec<usize> = match j["scenes"].get(scene_i) {
        Some(s) => arr(s, "nodes").iter().filter_map(idx).collect(),
        None => {
            let mut is_child = vec![false; nodes.len()];
            for n in nodes {
                for c in arr(n, "children").iter().filter_map(idx) {
                    if c < is_child.len() {
                        is_child[c] = true;
                    }
                }
            }
            (0..nodes.len()).filter(|&i| !is_child[i]).collect()
        }
    };
    // Flat pre-order object list.
    let mut objs: Vec<Object> = Vec::new();
    let mut obj_node: Vec<usize> = Vec::new();
    let mut kids: Vec<Vec<usize>> = Vec::new();
    let mut node_obj: HashMap<usize, usize> = HashMap::new();
    let mut skin_holders: Vec<usize> = Vec::new();
    let mut top: Vec<usize> = Vec::new();

    #[allow(clippy::too_many_arguments)]
    fn visit(
        doc: &GltfDoc,
        ni: usize,
        depth: u32,
        objs: &mut Vec<Object>,
        obj_node: &mut Vec<usize>,
        kids: &mut Vec<Vec<usize>>,
        node_obj: &mut HashMap<usize, usize>,
        skin_holders: &mut Vec<usize>,
    ) -> Result<Option<usize>, Error> {
        if depth > 256 {
            return Err(err("node hierarchy too deep"));
        }
        let n = doc.json["nodes"].get(ni).ok_or_else(|| err(format!("node {ni} missing")))?;
        if n.get("skin").is_some_and(|s| !s.is_null()) {
            skin_holders.push(ni);
            return Ok(None);
        }
        if node_obj.contains_key(&ni) {
            return Err(err(format!("node {ni} used twice")));
        }
        let name = n["name"].as_str().map(str::to_string).unwrap_or_else(|| format!("node{ni}"));
        let mut o = Object {
            id: crc(&format!("{}.obe", name.to_ascii_lowercase())),
            name,
            xlu: 0,
            sprites: vec![],
            mesh: None,
            effected_verts: vec![],
            scale_keys: vec![],
            move_keys: vec![],
            rotate_keys: vec![],
            bbox: None,
            phong: 0,
            flags: 0,
            matrix: IDENTITY,
            children: vec![],
        };
        obj_extras(&mut o, &n["extras"]);
        if let Some(mi) = idx(&n["mesh"]) {
            let (m, _) = convert_mesh(doc, mi, None)?;
            if !m.faces.is_empty() {
                o.bbox = bbox_of(&m);
                o.mesh = Some(m);
            }
        }
        let me = objs.len();
        objs.push(o);
        obj_node.push(ni);
        kids.push(vec![]);
        node_obj.insert(ni, me);
        for c in arr(n, "children").iter().filter_map(idx) {
            if let Some(k) = visit(doc, c, depth + 1, objs, obj_node, kids, node_obj, skin_holders)? {
                kids[me].push(k);
            }
        }
        Ok(Some(me))
    }

    for &ni in &scene_nodes {
        if let Some(o) = visit(doc, ni, 0, &mut objs, &mut obj_node, &mut kids, &mut node_obj, &mut skin_holders)? {
            top.push(o);
        }
    }
    if top.is_empty() {
        return Err(err("scene has no object nodes"));
    }

    // Skin.
    let mut is_skinned = false;
    let mut num_vtx = 0i16;
    if let Some(&h) = skin_holders.first() {
        let hn = &nodes[h];
        let si = idx(&hn["skin"]).ok_or_else(|| err("bad skin index"))?;
        let skin = j["skins"].get(si).ok_or_else(|| err(format!("skin {si} missing")))?;
        let joints: Vec<usize> = arr(skin, "joints").iter().filter_map(idx).collect();
        let ibm = match idx(&skin["inverseBindMatrices"]) {
            Some(a) => {
                let (d, c) = doc.floats(a)?;
                if c != 16 {
                    return Err(err("inverseBindMatrices must be MAT4"));
                }
                d.as_chunks::<16>().0.iter().map(|m| m.to_vec()).collect()
            }
            None => vec![],
        };
        let mi = idx(&hn["mesh"]).ok_or_else(|| err("skin holder without mesh"))?;
        let (m, vj) = convert_mesh(doc, mi, Some(&SkinInfo { ibm }))?;
        for (v, &jn) in vj.iter().enumerate() {
            let node = *joints.get(jn as usize).ok_or_else(|| err(format!("joint {jn} out of range")))?;
            let o = *node_obj.get(&node).ok_or_else(|| err(format!("joint node {node} is not part of the object hierarchy")))?;
            objs[o].effected_verts.push(v as i32);
        }
        num_vtx = m.vertices.len() as i16;
        let root = top[0];
        objs[root].bbox = bbox_of(&m);
        objs[root].mesh = Some(m);
        is_skinned = true;
    }

    // Animations.
    let mut tracks: Vec<Tracks> = (0..objs.len()).map(|_| Tracks::default()).collect();
    let mut clips = Vec::new();
    let mut cursor = 0i32;
    for (ai, a) in arr(j, "animations").iter().enumerate() {
        let ex = &a["extras"];
        let start = ex["start"].as_i64().map(|v| v as i32).unwrap_or(cursor);
        let mut max_t = 0f32;
        let samplers = arr(a, "samplers");
        for ch in arr(a, "channels") {
            let Some(&o) = idx(&ch["target"]["node"]).and_then(|n| node_obj.get(&n)) else { continue };
            let path = ch["target"]["path"].as_str().unwrap_or("");
            let s = idx(&ch["sampler"]).and_then(|s| samplers.get(s)).ok_or_else(|| err(format!("animation {ai}: bad sampler")))?;
            let (times, _) = doc.floats(idx(&s["input"]).ok_or_else(|| err("sampler without input"))?)?;
            let (vals, c) = doc.floats(idx(&s["output"]).ok_or_else(|| err("sampler without output"))?)?;
            let cubic = s["interpolation"].as_str() == Some("CUBICSPLINE");
            let want = match path {
                "translation" | "scale" => 3,
                "rotation" => 4,
                _ => continue,
            };
            if c != want {
                return Err(err(format!("animation {ai}: {path} output has {c} components")));
            }
            for (k, &t) in times.iter().enumerate() {
                max_t = max_t.max(t);
                let e = if cubic { (3 * k + 1) * c } else { k * c };
                let Some(v) = vals.get(e..e + c) else { return Err(err(format!("animation {ai}: output shorter than input"))) };
                let f = start + (t * FPS).round() as i32;
                let tr = &mut tracks[o];
                match path {
                    "translation" => {
                        tr.mv.insert(f, [v[0], v[1], v[2]]);
                    }
                    "scale" => {
                        tr.sc.insert(f, [v[0], v[1], v[2]]);
                    }
                    _ => {
                        tr.rot.insert(f, [v[0], v[1], v[2], v[3]]);
                    }
                }
            }
        }
        let end = ex["end"].as_i64().map(|v| v as i32).unwrap_or(start + (max_t * FPS).round() as i32);
        let keys_only = ex.is_object() && ex.get("start").is_some() && ex.get("speed").is_none();
        if !keys_only {
            clips.push(Animation {
                start: start as i16,
                end: end as i16,
                speed: ex["speed"].as_f64().unwrap_or(1.0) as f32,
                looped: ex["loop"].as_i64().unwrap_or(1) as i32,
            });
        }
        cursor = cursor.max(end + 1);
    }
    for (o, tr) in tracks.into_iter().enumerate() {
        let (t, r, s) = node_trs(&nodes[obj_node[o]]);
        let obj = &mut objs[o];
        let rest_v = |map: BTreeMap<i32, Vec3>, rest: Option<Vec3>| -> Vec<VKey> {
            let mut keys: Vec<VKey> = map.into_iter().map(|(time, v)| VKey { v, time }).collect();
            if let Some(rv) = rest {
                if keys.first().is_none_or(|k| k.time > 0) {
                    keys.insert(0, VKey { v: rv, time: 0 });
                }
            }
            keys
        };
        obj.move_keys = rest_v(tr.mv, t);
        obj.scale_keys = rest_v(tr.sc, s);
        obj.rotate_keys = tr.rot.into_iter().map(|(time, q)| QKey { q, time }).collect();
        if let Some(rq) = r {
            if obj.rotate_keys.first().is_none_or(|k| k.time > 0) {
                obj.rotate_keys.insert(0, QKey { q: rq, time: 0 });
            }
        }
    }

    // Assemble the tree.
    let mut slots: Vec<Option<Object>> = objs.into_iter().map(Some).collect();
    fn build(i: usize, slots: &mut [Option<Object>], kids: &[Vec<usize>]) -> Object {
        let mut o = slots[i].take().expect("object used once");
        o.children = kids[i].iter().map(|&k| build(k, slots, kids)).collect();
        o
    }
    let mut tops: Vec<Object> = top.iter().map(|&t| build(t, &mut slots, &kids)).collect();
    let root = tops.remove(0);

    let ex = &j["extras"];
    let id = model_id.or_else(|| ex["modelId"].as_u64().map(|v| v as u32)).unwrap_or_else(|| {
        let name = j["scenes"][scene_i]["name"].as_str().unwrap_or(&root.name).to_ascii_lowercase();
        crc(&format!("{name}.obe"))
    });
    let radius = ex["radius"].as_f64().map(|r| r as f32).unwrap_or_else(|| compute_radius(&root, &tops));
    let object_size = ex["objectSize"].as_i64().map(|v| v as i16).unwrap_or(0);
    Ok(Model { id, radius, object_size, root, root_siblings: tops, animations: clips, loc_mesh: None, is_skinned, num_vtx })
}

/// Bounding radius around the model origin (rest pose translations, no rotation/scale).
fn compute_radius(root: &Object, sibs: &[Object]) -> f32 {
    fn walk(o: &Object, off: Vec3, r: &mut f32) {
        let t = o.move_keys.first().map(|k| k.v).unwrap_or([0.0; 3]);
        let p = [off[0] + t[0], off[1] + t[1], off[2] + t[2]];
        if let Some(m) = &o.mesh {
            for v in &m.vertices {
                let d = ((v[0] + p[0]).powi(2) + (v[1] + p[1]).powi(2) + (v[2] + p[2]).powi(2)).sqrt();
                if d.is_finite() {
                    *r = r.max(d);
                }
            }
        }
        for c in &o.children {
            walk(c, p, r);
        }
    }
    let mut r = 0.0;
    walk(root, [0.0; 3], &mut r);
    for s in sibs {
        walk(s, [0.0; 3], &mut r);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_and_percent() {
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(percent_decode("a%20b.bin"), "a b.bin");
    }

    /// A minimal embedded glTF: one textured triangle under a child node, one clip.
    #[test]
    fn imports_minimal_document() {
        let pos: [f32; 9] = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let uv: [f32; 6] = [0.0, 0.0, 0.975, 0.0, 0.0, 0.4875];
        let times: [f32; 2] = [0.0, 0.5];
        let tr: [f32; 6] = [0.0, 0.0, 0.0, 0.0, 10.0, 0.0];
        let bytes: Vec<u8> = pos.iter().chain(&uv).chain(&times).chain(&tr).flat_map(|f| f.to_le_bytes()).collect();
        let b64 = {
            const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let mut s = String::new();
            for c in bytes.chunks(3) {
                let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
                for k in 0..4 {
                    s.push(if k <= c.len() { T[(n >> (18 - 6 * k)) as usize & 63] as char } else { '=' });
                }
            }
            s
        };
        let doc = serde_json::json!({
            "asset": {"version": "2.0"},
            "scene": 0,
            "scenes": [{"name": "Thing", "nodes": [0]}],
            "nodes": [{"name": "thing", "children": [1]}, {"name": "arm", "mesh": 0, "translation": [1.0, 2.0, 3.0]}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0, "TEXCOORD_0": 1}, "material": 0}]}],
            "materials": [{"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}}],
            "textures": [{"source": 0}],
            "images": [{"uri": "../tex/00Wate04.png"}],
            "buffers": [{"byteLength": bytes.len(), "uri": format!("data:application/octet-stream;base64,{b64}")}],
            "bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": bytes.len()}],
            "accessors": [
                {"bufferView": 0, "byteOffset": 0, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": 0, "byteOffset": 36, "componentType": 5126, "count": 3, "type": "VEC2"},
                {"bufferView": 0, "byteOffset": 60, "componentType": 5126, "count": 2, "type": "SCALAR"},
                {"bufferView": 0, "byteOffset": 68, "componentType": 5126, "count": 2, "type": "VEC3"}
            ],
            "animations": [{"samplers": [{"input": 2, "output": 3}], "channels": [{"sampler": 0, "target": {"node": 0, "path": "translation"}}]}]
        });
        let d = GltfDoc::parse(serde_json::to_string(&doc).unwrap().as_bytes(), None).unwrap();
        let m = import_model(&d, None).unwrap();
        assert_eq!(m.id, crc("thing.obe"));
        assert_eq!(m.root.children.len(), 1);
        let arm = &m.root.children[0];
        assert_eq!(arm.move_keys, vec![VKey { v: [1.0, 2.0, 3.0], time: 0 }]);
        let mesh = arm.mesh.as_ref().unwrap();
        assert_eq!(mesh.faces, vec![[0, 1, 2]]);
        assert_eq!(mesh.face_uv, vec![[0, 0], [1000, 0], [0, 500]]);
        assert_eq!(mesh.textures, vec![crc("00wate04.bmp")]);
        assert_eq!(m.root.move_keys.iter().map(|k| k.time).collect::<Vec<_>>(), vec![0, 30]);
        assert_eq!(m.animations, vec![Animation { start: 0, end: 30, speed: 1.0, looped: 1 }]);
        assert!(m.radius > 3.0);
    }
}
