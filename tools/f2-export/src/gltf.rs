//! MDX model (`f2_formats::objbank::Model`) → glTF 2.0 (`.gltf` JSON + `.bin`).
//!
//! Conventions (see `MDX Library/mdxObject.cpp` `TransformObject`, `mdxRender.cpp`):
//! - Each `MDX_OBJECT` becomes a node; its rest transform is key 0 of its
//!   move / rotate / scale tracks (what the game uses with no animation).
//! - `QuaternionToMatrix` builds the transpose of the usual column-vector
//!   rotation matrix and the engine uses row vectors, so quaternions are the
//!   standard (x, y, z, w) rotation and map to glTF unchanged.
//! - Key times are game frames at 60 Hz (`InitTiming(60)`); glTF time = frame / 60.
//!   Each `MDX_ANIMATION` clip [start, end] becomes one glTF animation (times
//!   rebased to 0, boundary values interpolated like `FindToFrom*Keys`).
//!   Clip `speed` / `loop` go in the animation's `extras`.
//! - Faces carry per-corner UVs (`value * 0.000975`) and colours, so geometry
//!   is emitted unindexed, one primitive per texture CRC.
//! - Skinned models (`isSkinned`): the root mesh is the skin; each object's
//!   `effectedVerts` lists the root-mesh vertices it moves (vertices are in
//!   that bone's local space, so inverse bind matrices are identity).
//!
//! Known approximation: the engine does not propagate a parent's *scale* to
//! its children's geometry (only to their translation); glTF does. Retail
//! models almost always have unit scale.
//! Coordinates are exported raw (Direct3D left-handed space, no axis flip).

use std::collections::BTreeMap;

use f2_formats::objbank::{Mesh, Model, Object, QKey, Quat, VKey, Vec3};
use serde_json::{json, Map, Value};

/// Game frames per second for keyframe times.
pub const FPS: f32 = 60.0;
/// Per-face texture coordinate scale (`MDX_MESH.faceTC`).
pub const UV_SCALE: f32 = 0.000975;

/// How to reference a texture CRC from a model.
#[derive(Clone, Debug)]
pub struct TexRef {
    /// URI relative to the `.gltf` file.
    pub uri: String,
    /// Whether the texture has any transparent texels.
    pub has_alpha: bool,
}

/// Result of [`model_to_gltf`].
pub struct GltfOut {
    pub json: Value,
    pub bin: Vec<u8>,
    pub stats: GltfStats,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GltfStats {
    pub nodes: usize,
    pub triangles: usize,
    pub animations: usize,
    pub missing_textures: usize,
    pub skinned: bool,
}

#[derive(Default)]
struct Builder {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
    nodes: Vec<Value>,
    meshes: Vec<Value>,
    materials: Vec<Value>,
    material_by_crc: BTreeMap<u32, usize>,
    images: Vec<Value>,
    textures: Vec<Value>,
    animations: Vec<Value>,
    stats: GltfStats,
}

const ARRAY_BUFFER: u32 = 34962;
const FLOAT: u32 = 5126;
const UNSIGNED_SHORT: u32 = 5123;

impl Builder {
    fn align(&mut self) {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
    }

    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        self.align();
        let off = self.bin.len();
        self.bin.extend_from_slice(bytes);
        let mut v = json!({"buffer": 0, "byteOffset": off, "byteLength": bytes.len()});
        if let Some(t) = target {
            v["target"] = json!(t);
        }
        self.views.push(v);
        self.views.len() - 1
    }

    /// Float accessor of `n`-component elements (`ty` = SCALAR/VEC2/…).
    fn floats(&mut self, data: &[f32], n: usize, ty: &str, target: Option<u32>, minmax: bool) -> usize {
        let bytes: Vec<u8> = data.iter().flat_map(|f| f.to_le_bytes()).collect();
        let view = self.view(&bytes, target);
        let count = data.len() / n;
        let mut a = json!({"bufferView": view, "componentType": FLOAT, "count": count, "type": ty});
        if minmax {
            let mut mn = vec![f32::INFINITY; n];
            let mut mx = vec![f32::NEG_INFINITY; n];
            for e in data.chunks_exact(n) {
                for i in 0..n {
                    mn[i] = mn[i].min(e[i]);
                    mx[i] = mx[i].max(e[i]);
                }
            }
            a["min"] = json!(mn);
            a["max"] = json!(mx);
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    fn u16s(&mut self, data: &[u16], n: usize, ty: &str) -> usize {
        let bytes: Vec<u8> = data.iter().flat_map(|f| f.to_le_bytes()).collect();
        let view = self.view(&bytes, Some(ARRAY_BUFFER));
        self.accessors.push(json!({"bufferView": view, "componentType": UNSIGNED_SHORT, "count": data.len() / n, "type": ty}));
        self.accessors.len() - 1
    }

    fn material(&mut self, crc: u32, tex: &dyn Fn(u32) -> Option<TexRef>) -> usize {
        if let Some(&m) = self.material_by_crc.get(&crc) {
            return m;
        }
        let mut m = json!({
            "name": format!("tex_{crc:08x}"),
            "doubleSided": true,
            "pbrMetallicRoughness": {"metallicFactor": 0.0, "roughnessFactor": 1.0},
            "extras": {"textureCrc": crc},
        });
        if crc != 0 {
            match tex(crc) {
                Some(t) => {
                    self.images.push(json!({"uri": t.uri}));
                    self.textures.push(json!({"source": self.images.len() - 1, "sampler": 0}));
                    m["pbrMetallicRoughness"]["baseColorTexture"] = json!({"index": self.textures.len() - 1});
                    if t.has_alpha {
                        m["alphaMode"] = json!("MASK");
                    }
                }
                None => self.stats.missing_textures += 1,
            }
        }
        self.materials.push(m);
        self.material_by_crc.insert(crc, self.materials.len() - 1);
        self.materials.len() - 1
    }

    /// Builds a glTF mesh; `skin` gives the joint index for each mesh vertex.
    fn mesh(&mut self, name: &str, mesh: &Mesh, skin: Option<&[u16]>, tex: &dyn Fn(u32) -> Option<TexRef>) -> Option<usize> {
        let nv = mesh.vertices.len();
        // Group faces by texture CRC (deterministic order).
        let mut groups: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
        for (fi, f) in mesh.faces.iter().enumerate() {
            if f.iter().any(|&i| i < 0 || i as usize >= nv) {
                continue;
            }
            groups.entry(mesh.textures.get(fi).copied().unwrap_or(0)).or_default().push(fi);
        }
        let mut prims = Vec::new();
        for (crc, faces) in groups {
            let mut pos = Vec::new();
            let mut nrm = Vec::new();
            let mut uv = Vec::new();
            let mut col = Vec::new();
            let mut joints = Vec::new();
            let colors_255 = mesh.colors.iter().any(|c| c.iter().any(|&v| v > 1.0));
            for &fi in &faces {
                let f = mesh.faces[fi];
                let fnrm = face_normal(&mesh.vertices, f);
                for (k, &vi) in f.iter().enumerate() {
                    let vi = vi as usize;
                    pos.extend_from_slice(&finite3(mesh.vertices[vi]));
                    let n = mesh.normals.get(vi).and_then(|&n| normalized(n)).unwrap_or(fnrm);
                    nrm.extend_from_slice(&n);
                    let t = mesh.face_uv.get(fi * 3 + k).copied().unwrap_or([0, 0]);
                    uv.extend_from_slice(&[t[0] as f32 * UV_SCALE, t[1] as f32 * UV_SCALE]);
                    let c = mesh.colors.get(fi * 3 + k).copied().unwrap_or([1.0; 4]);
                    for v in c {
                        let v = if colors_255 { v / 255.0 } else { v };
                        col.push(if v.is_finite() { v.clamp(0.0, 1.0) } else { 1.0 });
                    }
                    if let Some(s) = skin {
                        joints.extend_from_slice(&[s[vi], 0, 0, 0]);
                    }
                }
            }
            self.stats.triangles += faces.len();
            let mut attrs = Map::new();
            attrs.insert("POSITION".into(), json!(self.floats(&pos, 3, "VEC3", Some(ARRAY_BUFFER), true)));
            attrs.insert("NORMAL".into(), json!(self.floats(&nrm, 3, "VEC3", Some(ARRAY_BUFFER), false)));
            attrs.insert("TEXCOORD_0".into(), json!(self.floats(&uv, 2, "VEC2", Some(ARRAY_BUFFER), false)));
            attrs.insert("COLOR_0".into(), json!(self.floats(&col, 4, "VEC4", Some(ARRAY_BUFFER), false)));
            if skin.is_some() {
                attrs.insert("JOINTS_0".into(), json!(self.u16s(&joints, 4, "VEC4")));
                let w: Vec<f32> = (0..joints.len() / 4).flat_map(|_| [1.0, 0.0, 0.0, 0.0]).collect();
                attrs.insert("WEIGHTS_0".into(), json!(self.floats(&w, 4, "VEC4", Some(ARRAY_BUFFER), false)));
            }
            let material = self.material(crc, tex);
            prims.push(json!({"attributes": attrs, "material": material, "mode": 4}));
        }
        if prims.is_empty() {
            return None;
        }
        self.meshes.push(json!({"name": name, "primitives": prims}));
        Some(self.meshes.len() - 1)
    }
}

fn finite3(v: Vec3) -> [f32; 3] {
    v.map(|x| if x.is_finite() { x } else { 0.0 })
}

fn normalized(n: Vec3) -> Option<[f32; 3]> {
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    (l.is_finite() && l > 1e-6).then(|| [n[0] / l, n[1] / l, n[2] / l])
}

fn face_normal(v: &[Vec3], f: [i16; 3]) -> [f32; 3] {
    let (a, b, c) = (v[f[0] as usize], v[f[1] as usize], v[f[2] as usize]);
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    normalized([u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0]]).unwrap_or([0.0, 1.0, 0.0])
}

fn unit_quat(q: Quat) -> Quat {
    let l = (q.iter().map(|v| v * v).sum::<f32>()).sqrt();
    if l.is_finite() && l > 1e-6 {
        q.map(|v| v / l)
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

fn lerp3(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Shortest-arc slerp (as `QuatSlerp`, `mdxMath.cpp:304`).
fn slerp(a: Quat, b: Quat, t: f32) -> Quat {
    let (a, mut b) = (unit_quat(a), unit_quat(b));
    let mut d: f32 = (0..4).map(|i| a[i] * b[i]).sum();
    if d < 0.0 {
        b = b.map(|v| -v);
        d = -d;
    }
    if d > 0.9995 {
        let r = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t];
        return unit_quat(r);
    }
    let om = d.acos();
    let s = om.sin();
    let (p, q) = (((1.0 - t) * om).sin() / s, (t * om).sin() / s);
    unit_quat([a[0] * p + b[0] * q, a[1] * p + b[1] * q, a[2] * p + b[2] * q, a[3] * p + b[3] * q])
}

/// Value of a key track at `time` (clamped to the end keys).
fn sample<K, V: Copy>(keys: &[K], time: f32, t_of: impl Fn(&K) -> i32, v_of: impl Fn(&K) -> V, mix: impl Fn(V, V, f32) -> V) -> V {
    let i = keys.partition_point(|k| (t_of(k) as f32) <= time);
    if i == 0 {
        return v_of(&keys[0]);
    }
    if i >= keys.len() {
        return v_of(&keys[keys.len() - 1]);
    }
    let (a, b) = (&keys[i - 1], &keys[i]);
    let span = (t_of(b) - t_of(a)) as f32;
    let t = if span > 0.0 { (time - t_of(a) as f32) / span } else { 1.0 };
    mix(v_of(a), v_of(b), t)
}

/// Keys of one track restricted to clip [start, end], rebased to seconds.
fn clip_track<K, V: Copy>(
    keys: &[K],
    start: f32,
    end: f32,
    t_of: impl Fn(&K) -> i32 + Copy,
    v_of: impl Fn(&K) -> V + Copy,
    mix: impl Fn(V, V, f32) -> V + Copy,
) -> (Vec<f32>, Vec<V>) {
    let mut times = vec![0.0];
    let mut vals = vec![sample(keys, start, t_of, v_of, mix)];
    for k in keys {
        let t = t_of(k) as f32;
        if t > start && t < end {
            times.push((t - start) / FPS);
            vals.push(v_of(k));
        }
    }
    if end > start {
        times.push((end - start) / FPS);
        vals.push(sample(keys, end, t_of, v_of, mix));
    }
    // glTF requires strictly increasing input; drop non-increasing keys.
    let mut ot = Vec::with_capacity(times.len());
    let mut ov = Vec::with_capacity(vals.len());
    for (t, v) in times.into_iter().zip(vals) {
        if ot.last().is_none_or(|&l| t > l) {
            ot.push(t);
            ov.push(v);
        }
    }
    (ot, ov)
}

fn sorted_v(keys: &[VKey]) -> Vec<VKey> {
    let mut k = keys.to_vec();
    k.sort_by_key(|k| k.time);
    k
}
fn sorted_q(keys: &[QKey]) -> Vec<QKey> {
    let mut k = keys.to_vec();
    k.sort_by_key(|k| k.time);
    k
}

struct NodeInfo<'a> {
    obj: &'a Object,
    node: usize,
}

fn add_object<'a>(b: &mut Builder, obj: &'a Object, skinned_root: bool, tex: &dyn Fn(u32) -> Option<TexRef>, out: &mut Vec<NodeInfo<'a>>) -> usize {
    let idx = b.nodes.len();
    b.nodes.push(Value::Null);
    out.push(NodeInfo { obj, node: idx });
    let mut node = json!({"name": obj.name});
    if let Some(k) = obj.move_keys.first() {
        node["translation"] = json!(finite3(k.v));
    }
    if let Some(k) = obj.rotate_keys.first() {
        node["rotation"] = json!(unit_quat(k.q));
    }
    if let Some(k) = obj.scale_keys.first() {
        node["scale"] = json!(finite3(k.v));
    }
    // The skin mesh of a skinned model is attached elsewhere.
    if !skinned_root {
        if let Some(m) = &obj.mesh {
            if let Some(mi) = b.mesh(&obj.name, m, None, tex) {
                node["mesh"] = json!(mi);
            }
        }
    }
    let mut extras = json!({"objectId": obj.id, "flags": obj.flags, "xlu": obj.xlu});
    if obj.phong != 0 {
        extras["phongTextureCrc"] = json!(obj.phong);
    }
    if !obj.sprites.is_empty() {
        extras["sprites"] =
            obj.sprites.iter().map(|s| json!({"textureCrc": s.texture, "pos": s.pos, "size": s.size, "offset": s.offset, "flags": s.flags})).collect();
    }
    node["extras"] = extras;
    let children: Vec<usize> = obj.children.iter().map(|c| add_object(b, c, false, tex, out)).collect();
    if !children.is_empty() {
        node["children"] = json!(children);
    }
    b.nodes[idx] = node;
    idx
}

/// Converts one model. `tex` maps a texture CRC to an image reference.
pub fn model_to_gltf(model: &Model, bin_uri: &str, tex: &dyn Fn(u32) -> Option<TexRef>) -> GltfOut {
    let mut b = Builder::default();
    let skinned = model.is_skinned && model.root.mesh.is_some();
    let mut infos = Vec::new();
    let root = add_object(&mut b, &model.root, skinned, tex, &mut infos);
    let mut scene_nodes = vec![root];
    for s in &model.root_siblings {
        scene_nodes.push(add_object(&mut b, s, skinned, tex, &mut infos));
    }
    let mut skins = Vec::new();
    if skinned {
        let mesh = model.root.mesh.as_ref().unwrap();
        // Joint index per skin vertex: the last object listing it wins; unlisted → root (joint 0).
        let mut vj = vec![0u16; mesh.vertices.len()];
        for (j, info) in infos.iter().enumerate() {
            for &v in &info.obj.effected_verts {
                if v >= 0 && (v as usize) < vj.len() {
                    vj[v as usize] = j as u16;
                }
            }
        }
        if let Some(mi) = b.mesh(&format!("{}_skin", model.root.name), mesh, Some(&vj), tex) {
            skins.push(json!({"joints": infos.iter().map(|i| i.node).collect::<Vec<_>>(), "skeleton": root}));
            b.nodes.push(json!({"name": format!("{}_skin", model.root.name), "mesh": mi, "skin": 0}));
            scene_nodes.push(b.nodes.len() - 1);
            b.stats.skinned = true;
        }
    }
    b.stats.nodes = b.nodes.len();

    // Animations: one per clip, or one covering all keys if there are no clips.
    let mut clips: Vec<(String, f32, f32, Value)> = model
        .animations
        .iter()
        .enumerate()
        .map(|(i, a)| (format!("anim{i}"), a.start as f32, a.end as f32, json!({"start": a.start, "end": a.end, "speed": a.speed, "loop": a.looped})))
        .filter(|c| c.2 >= c.1)
        .collect();
    let animated = infos.iter().any(|i| i.obj.move_keys.len() > 1 || i.obj.rotate_keys.len() > 1 || i.obj.scale_keys.len() > 1);
    if clips.is_empty() && animated {
        let mut lo = i32::MAX;
        let mut hi = i32::MIN;
        for i in &infos {
            for t in i.obj.move_keys.iter().map(|k| k.time).chain(i.obj.scale_keys.iter().map(|k| k.time)).chain(i.obj.rotate_keys.iter().map(|k| k.time)) {
                lo = lo.min(t);
                hi = hi.max(t);
            }
        }
        clips.push(("all".into(), lo as f32, hi as f32, json!({"start": lo, "end": hi})));
    }
    if animated {
        for (name, start, end, extras) in clips {
            let mut samplers = Vec::new();
            let mut channels = Vec::new();
            for info in &infos {
                let o = info.obj;
                let mut add = |b: &mut Builder, path: &str, times: Vec<f32>, vals: Vec<f32>, n: usize, ty: &str| {
                    let input = b.floats(&times, 1, "SCALAR", None, true);
                    let output = b.floats(&vals, n, ty, None, false);
                    samplers.push(json!({"input": input, "output": output, "interpolation": "LINEAR"}));
                    channels.push(json!({"sampler": samplers.len() - 1, "target": {"node": info.node, "path": path}}));
                };
                if o.move_keys.len() > 1 {
                    let k = sorted_v(&o.move_keys);
                    let (t, v) = clip_track(&k, start, end, |k| k.time, |k| finite3(k.v), lerp3);
                    add(&mut b, "translation", t, v.concat(), 3, "VEC3");
                }
                if o.scale_keys.len() > 1 {
                    let k = sorted_v(&o.scale_keys);
                    let (t, v) = clip_track(&k, start, end, |k| k.time, |k| finite3(k.v), lerp3);
                    add(&mut b, "scale", t, v.concat(), 3, "VEC3");
                }
                if o.rotate_keys.len() > 1 {
                    let k = sorted_q(&o.rotate_keys);
                    let (t, mut v) = clip_track(&k, start, end, |k| k.time, |k| unit_quat(k.q), slerp);
                    // Keep consecutive quaternions on the same hemisphere so glTF's slerp takes the short arc.
                    for i in 1..v.len() {
                        if (0..4).map(|j| v[i][j] * v[i - 1][j]).sum::<f32>() < 0.0 {
                            v[i] = v[i].map(|x| -x);
                        }
                    }
                    add(&mut b, "rotation", t, v.concat(), 4, "VEC4");
                }
            }
            if !channels.is_empty() {
                b.animations.push(json!({"name": name, "samplers": samplers, "channels": channels, "extras": extras}));
            }
        }
    }
    b.stats.animations = b.animations.len();
    b.align();

    let mut root_json = json!({
        "asset": {"version": "2.0", "generator": concat!("f2-export ", env!("CARGO_PKG_VERSION"))},
        "scene": 0,
        "scenes": [{"name": model.name(), "nodes": scene_nodes}],
        "nodes": b.nodes,
        "extras": {"modelId": model.id, "radius": model.radius, "isSkinned": model.is_skinned, "fps": FPS},
    });
    let o = root_json.as_object_mut().unwrap();
    if !b.bin.is_empty() {
        o.insert("buffers".into(), json!([{"uri": bin_uri, "byteLength": b.bin.len()}]));
        o.insert("bufferViews".into(), json!(b.views));
        o.insert("accessors".into(), json!(b.accessors));
    }
    let mut put = |k: &str, v: Vec<Value>| {
        if !v.is_empty() {
            o.insert(k.into(), Value::Array(v));
        }
    };
    put("meshes", b.meshes);
    put("materials", b.materials);
    if !b.textures.is_empty() {
        // Bilinear + repeat wrap.
        put("samplers", vec![json!({"magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497})]);
    }
    put("images", b.images);
    put("textures", b.textures);
    put("skins", skins);
    put("animations", b.animations);
    GltfOut { json: root_json, bin: b.bin, stats: b.stats }
}
