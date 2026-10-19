//! GPU mesh renderer (enhanced, [`RenderOptions::gpu_meshes`](crate::RenderOptions)).
//!
//! The original projects every vertex on the CPU and hands the GPU screen-space triangles
//! (`scene.rs`, the MDX pipeline). Here the opaque meshes (`Normal` / `Wrap` frames, no
//! MODGE/WAVE animation) stay on the GPU as model data instead: each mesh is expanded once
//! into a vertex buffer (position, bone, uv, normal, colour; faces grouped by texture) and
//! drawn every frame with its object matrices (all bones for a skin) and the camera. That keeps
//! the whole scene's geometry available to the GPU (lighting, shadows, ray tracing) and leaves
//! clipping and culling to the hardware.
//!
//! The vertex shader uses the game's projection (`PCPrepareObject` with the exact depth,
//! [`View::exact_depth`](crate::View), as a matrix: [`crate::upscale::projection`]) and the same
//! linear depth as the CPU path (`vz × 0.00025`), so meshes, CPU-projected sprites, shadows and
//! effects keep occluding each other as before. Translucent and wobbling meshes stay on the CPU
//! path.
//!
//! With [`Lighting`] the fragment shader lights the meshes (`crate::lighting`): hemisphere
//! ambient, a sun / moon with a shadow map (rendered from the light over the meshes around the
//! camera's target, on screen or not) and point lights; emissive surfaces are drawn unlit.

use crate::lighting::{is_emissive, Lighting, PointLight};
use crate::prelude::*;
use crate::scene::{Frame, GpuObj, View, MESH_SKIN, UV_SCALE};
use crate::textures::{TexHandle, TextureRegistry};
use std::collections::HashMap;

/// One expanded mesh corner (64 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Vert {
    pos: [f32; 3],
    /// Matrix of the draw's bone list (a skin's object index; 0 for rigid meshes).
    bone: u32,
    uv: [f32; 2],
    _pad: [f32; 2],
    normal: [f32; 4],
    color: [f32; 4],
}

/// Faces of one texture: `count` vertices from `first`; where a glowing group's lights go
/// (model space): the centres and sizes of its faces clustered in [`SPOT_CELL`] cells.
#[derive(Clone, Debug)]
struct Group {
    crc: u32,
    first: u32,
    count: u32,
    spots: Vec<([f32; 3], f32)>,
    /// Water: steep faces (waterfalls), still drawn from the mesh when the generated surface
    /// replaces the flat ones.
    steep: bool,
    /// A waterfall's lowest points (model space, one per [`FOOT_CELL`]): where it lands.
    feet: Vec<[f32; 3]>,
}

/// Spacing of a waterfall's landing points.
const FOOT_CELL: f32 = 8.0;
/// Landing points per waterfall group at most.
const MAX_FEET: usize = 12;

/// Cell size of a glowing surface's lights (a lava lake gets one light per cell).
const SPOT_CELL: f32 = 220.0;

/// Mesh key: model id and pre-order object index (or [`MESH_SKIN`]).
pub(crate) type Key = (u32, u16);

/// Per-draw record in the storage buffer (64 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Rec {
    /// First matrix (this frame's and last frame's lists are parallel).
    mat: u32,
    /// Bit 0: no far clip (sky); bit 1: Gouraud override; bit 2: landscape; bit 3: MODGE
    /// (texture wobble); bit 4: WAVE (swell, water alpha); bit 5: a river bed (drawn on the CPU,
    /// after the opaque meshes, by the original: pushed back in depth so the banks it runs
    /// under win where they meet); bit 6: an opaque liquid ([`Class::Liquid`]).
    flags: u32,
    /// `PCCalcModgeValues`' frame count; spare.
    modge_frame: u32,
    _pad: u32,
    gouraud: [f32; 4],
    /// A sliding texture ([`GpuObj::slide`]): x: what to add to the uploaded `v` (meshes
    /// uploaded once: subdivided water); y: how fast it slides (`v` per second, the texture
    /// moving to +v); z: the liquid ([`Liquid`] as a number; 0: water or none); w: lava: how
    /// much of its texture is painted crust ([`painted_crust`]); a waterfall: its lip's height
    /// (world y; [`Liquid::Fall`]).
    slide: [f32; 4],
    /// A floating thing ([`GpuObj::float_at`]): xyz: where it rides the water; w: 1 (0: it
    /// does not float).
    anchor: [f32; 4],
}

#[derive(Clone, Copy, Debug)]
struct Draw {
    key: Key,
    rec: u32,
    wrap: bool,
    landscape: bool,
    sky: bool,
    override_tex: Option<TexHandle>,
}

/// One frame's GPU mesh draws (filled while the frame is built).
#[derive(Default)]
pub(crate) struct Sink {
    draws: Vec<Draw>,
    recs: Vec<Rec>,
    mats: Vec<[[f32; 4]; 4]>,
    prev: Vec<[[f32; 4]; 4]>,
    /// Meshes seen for the first time, to upload.
    pending: Vec<(Key, Vec<Vert>, Vec<Group>)>,
    /// The first texture coordinate of each mesh as uploaded: sliding textures
    /// (`slide_object_textures`) move every face's, and the mesh is uploaded again.
    known: HashMap<Key, Option<[i16; 2]>>,
    /// Uploaded meshes whose texture coordinates slid, to write again (same layout).
    slid: Vec<(Key, Vec<Vert>)>,
    /// Meshes the CPU draws, for the lights of their glowing faces: key and matrix.
    light_only: Vec<(Key, [[f32; 4]; 4])>,
    /// [`Lighting::water`]: water and MODGE meshes are taken too (set per frame).
    pub water_on: bool,
    /// What each non-opaque mesh is (see [`Class`]).
    class: HashMap<Key, Class>,
    /// This frame's water draws: mesh, record, additive (else alpha blended).
    water: Vec<(Key, u32, bool)>,
    /// Subdivided water meshes seen for the first time, to upload.
    water_pending: Vec<(Key, Vec<Vert>, Vec<Group>)>,
    /// The first texture coordinate of each subdivided water mesh as uploaded: a sliding one
    /// is not uploaded again, its draws move `v` instead ([`Rec::slide`]).
    water_uv0: HashMap<Key, [i16; 2]>,
    /// Where each opaque waterfall ([`Liquid::Fall`]) lands (model space; see [`Group::feet`]).
    fall_feet: HashMap<Key, Vec<[f32; 3]>>,
    /// How much of each lava mesh's texture is painted crust ([`painted_crust`]).
    lava_crust: HashMap<Key, f32>,
    /// The highest point of each opaque waterfall's steep faces (model space): its lip.
    fall_tops: HashMap<Key, Option<[f32; 3]>>,
    /// The way each of [`Self::fall_feet`] faces (model space, out of the drawn side).
    fall_out: HashMap<Key, Vec<[f32; 3]>>,
}

/// A mesh the original animates or blends ([`GpuObj::light_only`]), with the enhanced water.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Class {
    /// Left to the CPU.
    Cpu,
    /// Mostly water textures: the water pass (subdivided, waves).
    Water,
    /// An opaque MODGE surface (river beds): the GPU mesh pass, with caustics.
    Modge,
    /// An opaque liquid (acid, lava, a waterfall drawn opaque): its own surface on the GPU
    /// mesh pass, flowing as in the original where its texture slides; the wave simulation
    /// runs on it too, as thick as the liquid is.
    Liquid(Liquid),
}

/// What an opaque liquid is ([`Class::Liquid`]; the number is [`Rec::slide`]`.z` and the flow
/// map's material, which sets how the waves move on it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Liquid {
    /// Acid, sludge, toxic glop: thick, glowing a little, bubbling.
    Acid = 1,
    /// Molten rock: very thick, crusted, glowing.
    Lava = 2,
    /// Water drawn opaque whose texture slides: a stream and its waterfall (the frontend's).
    Fall = 3,
}

/// Whether colour `c` is a liquid's green (acid: the Subterranean water textures are blue,
/// their vertex colours turn them green).
fn is_green(c: [f32; 3]) -> bool {
    c[1] > 0.12 && c[1] > c[0] * 1.5 && c[1] > c[2] * 1.3
}

/// [`Class`] of `o` (texture names: [`crate::lighting::is_water`]).
fn classify(o: &GpuObj, textures: &TextureRegistry) -> Class {
    let n = o.mesh.textures.len();
    let named =
        |f: fn(&str) -> bool| o.mesh.textures.iter().filter(|&&crc| textures.entry(crc).is_some_and(|e| f(&e.name.to_ascii_lowercase()))).count() * 2 > n;
    let wet = named(crate::lighting::is_water);
    let opaque = matches!(o.frame, Frame::Normal | Frame::Wrap);
    if n == 0 {
        return Class::Cpu;
    }
    if opaque && named(|n| n.contains("lava")) {
        return Class::Liquid(Liquid::Lava);
    }
    if opaque && (named(|n| n.contains("glop") || n.contains("toxic")) || wet && is_green(seen_color(o, textures))) {
        return Class::Liquid(Liquid::Acid);
    }
    if wet && opaque && o.slide != 0.0 {
        // Sludge keeps the liquid's look; water is a stream (and its fall).
        let sludge = named(|n| n.starts_with("sludge") || n.starts_with("slu_"));
        return Class::Liquid(if sludge { Liquid::Acid } else { Liquid::Fall });
    }
    if wet {
        Class::Water
    } else if o.modge & 3 == 1 && opaque {
        Class::Modge
    } else {
        Class::Cpu
    }
}

/// How much of lava mesh `o`'s texture is crust already (0..1): some artists painted molten
/// rock as dark plates between glowing cracks (`00lava04`, `anclava`: a fifth to a half of
/// their texels dark), others as bright melt (`lavafall`: none). Painted crust keeps its
/// plates (lit, in relief) and gets none of ours over it.
fn painted_crust(o: &GpuObj, textures: &TextureRegistry) -> f32 {
    let (mut dark, mut all) = (0usize, 0usize);
    for &crc in o.mesh.textures.iter().collect::<std::collections::BTreeSet<_>>() {
        let Some(img) = textures.entry(crc).filter(|e| e.name.to_ascii_lowercase().contains("lava")).and_then(|e| textures.images.get(e.image.0 as usize))
        else {
            continue;
        };
        let px = img.rgba.as_chunks::<4>().0;
        dark += px.iter().filter(|p| p[0].max(p[1]).max(p[2]) < 120).count();
        all += px.len();
    }
    if all == 0 {
        0.0
    } else {
        (dark as f32 / all as f32 * 4.0).min(1.0)
    }
}

/// The colour `o` shows: its textures' mean colour times its vertex colours, over its faces.
fn seen_color(o: &GpuObj, textures: &TextureRegistry) -> [f32; 3] {
    let mut means: HashMap<u32, [f32; 3]> = HashMap::new();
    let mut acc = [0.0f32; 3];
    let mut n = 0.0;
    for (fi, &crc) in o.mesh.textures.iter().enumerate() {
        let m = *means
            .entry(crc)
            .or_insert_with(|| textures.entry(crc).and_then(|e| textures.images.get(e.image.0 as usize)).map_or([1.0; 3], |img| mean_color(&img.rgba).0));
        for k in 0..3 {
            let c = o.mesh.colors.get(fi * 3 + k).copied().unwrap_or([1.0; 4]);
            for j in 0..3 {
                acc[j] += m[j] * c[j];
            }
            n += 1.0;
        }
    }
    acc.map(|a| a / f32::max(n, 1.0))
}

impl Sink {
    pub fn clear(&mut self) {
        self.draws.clear();
        self.recs.clear();
        self.mats.clear();
        self.prev.clear();
        self.light_only.clear();
        self.water.clear();
    }

    /// Takes `o` (see [`GpuObj`]); false = the CPU draws it.
    pub fn offer(&mut self, o: &GpuObj, no_clip: bool, textures: &TextureRegistry) -> bool {
        let key = (o.model.id, o.object);
        let uv0 = o.mesh.face_uv.first().copied();
        // A flowing liquid is not uploaded again as it slides: its draws move `v` instead
        // (every face together: smooth, where each face's own jumps by 8192 now and then).
        let flowing = o.light_only && matches!(self.class.get(&key), Some(Class::Liquid(_)));
        let mut dv = 0.0;
        match self.known.get_mut(&key) {
            None => {
                let (verts, groups) = expand(o);
                self.known.insert(key, uv0);
                self.pending.push((key, verts, groups));
            }
            Some(k) if flowing => dv = (uv0.map_or(0, |u| u[1]) as i32 - k.map_or(0, |u| u[1]) as i32) as f32 * UV_SCALE,
            Some(k) if *k != uv0 => {
                *k = uv0;
                self.slid.push((key, expand(o).0));
            }
            Some(_) => {}
        }
        let mut modge = o.modge;
        let (mut bed, mut liquid) = (false, None);
        if o.light_only {
            if o.override_tex.is_none() {
                self.light_only.push((key, o.mats[0]));
            }
            if !self.water_on || o.override_tex.is_some() || o.object == MESH_SKIN || no_clip {
                return false;
            }
            let class = *self.class.entry(key).or_insert_with(|| classify(o, textures));
            match class {
                Class::Cpu => return false,
                // The wobble is the original's stand-in for caustics; the real ones replace it.
                Class::Modge => {
                    modge = 0;
                    bed = true;
                }
                Class::Liquid(l) => {
                    liquid = Some(l);
                    if l == Liquid::Lava && !self.lava_crust.contains_key(&key) {
                        self.lava_crust.insert(key, painted_crust(o, textures));
                    }
                    if l == Liquid::Fall && !self.fall_feet.contains_key(&key) {
                        let (verts, groups) = expand(o);
                        let (sv, sg) = subdivide(&verts, &groups);
                        let steep: Vec<Group> = sg.into_iter().filter(|g| g.steep).collect();
                        let top =
                            steep.iter().flat_map(|g| &sv[g.first as usize..(g.first + g.count) as usize]).map(|v| v.pos).max_by(|a, b| a[1].total_cmp(&b[1]));
                        self.fall_tops.insert(key, top);
                        self.fall_out.insert(key, steep.iter().flat_map(|g| foot_normals(&sv, g)).collect());
                        self.fall_feet.insert(key, steep.into_iter().flat_map(|g| g.feet).collect());
                    }
                }
                Class::Water => {
                    if let std::collections::hash_map::Entry::Vacant(e) = self.class.entry((key.0, key.1 ^ WATER_KEY)) {
                        e.insert(Class::Water);
                        let (verts, groups) = expand(o);
                        let (verts, groups) = subdivide(&verts, &groups);
                        self.water_pending.push((key, verts, groups));
                        self.water_uv0.insert(key, uv0.unwrap_or([0, 0]));
                    }
                    let mat = self.mats.len() as u32;
                    self.mats.push(o.mats[0]);
                    self.prev.push(*o.prev.first().unwrap_or(&o.mats[0]));
                    let flags = (o.modge as u32 & 3) << 3;
                    // A face's `v` jumps by 8192 when it slides far (`slide_object`): the first
                    // face's moves every face, which keeps them together.
                    let up = self.water_uv0.get(&key).map_or(0, |u| u[1]);
                    let dv = (uv0.map_or(0, |u| u[1]) as i32 - up as i32) as f32 * UV_SCALE;
                    let slide = [dv, o.slide * UV_SCALE * 60.0, 0.0, 0.0];
                    self.recs.push(Rec { mat, flags, modge_frame: o.modge_frame, _pad: 0, gouraud: [1.0; 4], slide, anchor: [0.0; 4] });
                    self.water.push((key, self.recs.len() as u32 - 1, o.frame == Frame::Additive));
                    return true;
                }
            }
        }
        let mat = self.mats.len() as u32;
        self.mats.extend(o.mats.iter().copied());
        // Bones without last-frame matrices (a new pose): this frame's.
        self.prev.extend((0..o.mats.len()).map(|i| *o.prev.get(i).unwrap_or(&o.mats[i])));
        let flags = no_clip as u32 | (o.gouraud.is_some() as u32) << 1 | (o.landscape as u32) << 2;
        let g = o.gouraud.unwrap_or([1.0; 3]);
        let flags = flags | (modge as u32 & 3) << 3 | (bed as u32) << 5 | (liquid.is_some() as u32) << 6;
        let crust = if liquid == Some(Liquid::Lava) { self.lava_crust.get(&key).copied().unwrap_or(0.0) } else { 0.0 };
        // A waterfall's lip height (world y; none: -1e9).
        let top = self.fall_tops.get(&key).copied().flatten().map_or(-1e9, |p| crate::math::mtx_xfm(&o.mats[0], p)[1]);
        let w = if liquid == Some(Liquid::Fall) { top } else { crust };
        let slide = [dv, o.slide * UV_SCALE * 60.0, liquid.map_or(0.0, |l| l as u32 as f32), w];
        let anchor = o.float_at.filter(|_| liquid.is_none() && !bed).map_or([0.0; 4], |a| [a[0], a[1], a[2], 1.0]);
        self.recs.push(Rec { mat, flags, modge_frame: o.modge_frame, _pad: 0, gouraud: [g[0], g[1], g[2], 1.0], slide, anchor });
        self.draws.push(Draw {
            key,
            rec: self.recs.len() as u32 - 1,
            wrap: o.frame == Frame::Wrap,
            landscape: o.landscape,
            sky: no_clip,
            override_tex: o.override_tex,
        });
        true
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Quads per side of the generated water surface (2×2 simulation cells each).
const GRID: u32 = 512;

/// Marks a water mesh's entry in [`Sink::class`] (its subdivided upload).
const WATER_KEY: u16 = 0x8000;

/// Longest edge of a subdivided water triangle (model units).
const WATER_EDGE: f32 = 12.0;

/// `verts` with every triangle split into n² (n ≤ 8) so no edge is longer than about
/// [`WATER_EDGE`]: the swell and the waves get vertices to move.
fn subdivide(verts: &[Vert], groups: &[Group]) -> (Vec<Vert>, Vec<Group>) {
    let lerp = |a: &Vert, b: &Vert, c: &Vert, u: f32, v: f32| {
        let w = 1.0 - u - v;
        let mix3 = |x: [f32; 3], y: [f32; 3], z: [f32; 3]| [0, 1, 2].map(|k| x[k] * w + y[k] * u + z[k] * v);
        let mix4 = |x: [f32; 4], y: [f32; 4], z: [f32; 4]| [0, 1, 2, 3].map(|k| x[k] * w + y[k] * u + z[k] * v);
        Vert {
            pos: mix3(a.pos, b.pos, c.pos),
            bone: a.bone,
            uv: [a.uv[0] * w + b.uv[0] * u + c.uv[0] * v, a.uv[1] * w + b.uv[1] * u + c.uv[1] * v],
            _pad: [0.0; 2],
            normal: mix4(a.normal, b.normal, c.normal),
            color: mix4(a.color, b.color, c.color),
        }
    };
    let (mut out, mut gs) = (Vec::new(), Vec::new());
    for g in groups {
        for steep in [false, true] {
            let first = out.len() as u32;
            let mut feet = Vec::new();
            for t in verts[g.first as usize..(g.first + g.count) as usize].as_chunks::<3>().0 {
                // Steep and tall: a waterfall (short skirts under the banks count as flat).
                let n = crate::lighting::normalize(cross(sub(t[1].pos, t[0].pos), sub(t[2].pos, t[0].pos)));
                let ys = t.map(|v| v.pos[1]);
                let tall = ys.iter().copied().fold(f32::MIN, f32::max) - ys.iter().copied().fold(f32::MAX, f32::min) > 24.0;
                if (n[1].abs() < 0.5 && tall) != steep {
                    continue;
                }
                if steep {
                    let low = t.iter().map(|v| v.pos).fold([0.0, f32::MAX, 0.0], |a, p| if p[1] < a[1] { p } else { a });
                    let q = |p: [f32; 3]| [(p[0] / FOOT_CELL).round(), (p[1] / FOOT_CELL).round(), (p[2] / FOOT_CELL).round()];
                    if feet.len() < MAX_FEET && !feet.iter().any(|&f| q(f) == q(low)) {
                        feet.push(low);
                    }
                }
                let len = |a: &Vert, b: &Vert| dot(sub(a.pos, b.pos), sub(a.pos, b.pos)).sqrt();
                let e = len(&t[0], &t[1]).max(len(&t[1], &t[2])).max(len(&t[2], &t[0]));
                let n = ((e / WATER_EDGE).ceil() as usize).clamp(1, 8);
                let f = 1.0 / n as f32;
                let p = |i: usize, j: usize| lerp(&t[0], &t[1], &t[2], i as f32 * f, j as f32 * f);
                for i in 0..n {
                    for j in 0..n - i {
                        out.extend([p(i, j), p(i + 1, j), p(i, j + 1)]);
                        if i + j + 1 < n {
                            out.extend([p(i + 1, j), p(i + 1, j + 1), p(i, j + 1)]);
                        }
                    }
                }
            }
            if out.len() as u32 > first {
                gs.push(Group { crc: g.crc, first, count: out.len() as u32 - first, spots: Vec::new(), steep, feet });
            }
        }
    }
    (out, gs)
}

/// The way the face each of `g`'s feet comes from faces (out of its drawn side).
fn foot_normals(verts: &[Vert], g: &Group) -> Vec<[f32; 3]> {
    let tris = verts[g.first as usize..(g.first + g.count) as usize].as_chunks::<3>().0;
    g.feet
        .iter()
        .map(|&f| {
            tris.iter()
                .find(|t| t.iter().any(|v| v.pos == f) && t.iter().all(|v| v.pos[1] >= f[1]))
                .map_or([0.0; 3], |t| crate::lighting::normalize(cross(sub(t[1].pos, t[0].pos), sub(t[2].pos, t[0].pos))))
        })
        .collect()
}

/// Vertex normals: the mesh's own (turned to face the way its faces do), else averaged from the
/// faces (`cross(b - a, c - a)` points out of a drawn face).
fn normals(mesh: &f2_formats::objbank::Mesh) -> Vec<[f32; 3]> {
    let nv = mesh.vertices.len();
    let mut acc = vec![[0.0f32; 3]; nv];
    for f in &mesh.faces {
        let i = f.map(|v| v as usize);
        if i.iter().any(|&k| k >= nv) {
            continue;
        }
        let v = |k: usize| mesh.vertices[i[k]];
        let n = cross(sub(v(1), v(0)), sub(v(2), v(0)));
        for &k in &i {
            acc[k] = [acc[k][0] + n[0], acc[k][1] + n[1], acc[k][2] + n[2]];
        }
    }
    let face_normals: Vec<[f32; 3]> = acc.iter().map(|&n| crate::lighting::normalize(n)).collect();
    if mesh.normals.len() == nv {
        let agree: f32 = mesh.normals.iter().zip(&face_normals).map(|(a, b)| dot(*a, *b)).sum();
        let s = if agree < 0.0 { -1.0 } else { 1.0 };
        // Zero normals (some meshes have them): the faces'.
        return mesh
            .normals
            .iter()
            .zip(&face_normals)
            .map(|(n, f)| if dot(*n, *n) < 1e-6 { *f } else { crate::lighting::normalize([n[0] * s, n[1] * s, n[2] * s]) })
            .collect();
    }
    face_normals
}

/// The corners of `o.mesh` grouped by texture, as `PCRenderObject` would draw them (skins:
/// the bone moving each vertex; faces with an unmoved vertex left out like the CPU path).
fn expand(o: &GpuObj) -> (Vec<Vert>, Vec<Group>) {
    let mesh = o.mesh;
    let nv = mesh.vertices.len();
    let mut bone = vec![if o.object == MESH_SKIN { u32::MAX } else { 0 }; nv];
    if o.object == MESH_SKIN {
        crate::anim::for_each_object(o.model, |i, obj| {
            for &j in &obj.effected_verts {
                if let Some(b) = bone.get_mut(j as usize) {
                    *b = i as u32;
                }
            }
        });
    }
    let norm = normals(mesh);
    let mut order: Vec<usize> = (0..mesh.faces.len()).collect();
    order.sort_by_key(|&fi| mesh.textures.get(fi).copied().unwrap_or(0));
    let (mut verts, mut groups) = (Vec::with_capacity(mesh.faces.len() * 3), Vec::<Group>::new());
    for fi in order {
        let face = mesh.faces[fi];
        let idx = face.map(|v| v as usize);
        if idx.iter().any(|&i| i >= nv || bone[i] == u32::MAX) {
            continue;
        }
        let crc = mesh.textures.get(fi).copied().unwrap_or(0);
        if groups.last().is_none_or(|g| g.crc != crc) {
            groups.push(Group { crc, first: verts.len() as u32, count: 0, spots: Vec::new(), steep: false, feet: Vec::new() });
        }
        for (k, &i) in idx.iter().enumerate() {
            let uv = mesh.face_uv.get(fi * 3 + k).copied().unwrap_or([0, 0]);
            let n = norm[i];
            verts.push(Vert {
                pos: mesh.vertices[i],
                bone: bone[i],
                uv: [uv[0] as f32 * UV_SCALE, uv[1] as f32 * UV_SCALE],
                _pad: [0.0; 2],
                normal: [n[0], n[1], n[2], 0.0],
                color: mesh.colors.get(fi * 3 + k).copied().unwrap_or([1.0; 4]),
            });
        }
        groups.last_mut().unwrap().count += 3;
    }
    for g in &mut groups {
        // Skins move their vertices bone by bone: one spot (their lights are not used).
        let vs = &verts[g.first as usize..(g.first + g.count) as usize];
        let mut cells: Vec<([i32; 3], [f32; 3], f32)> = Vec::new();
        for v in vs {
            let cell = v.pos.map(|c| (c / SPOT_CELL).floor() as i32);
            match cells.iter_mut().find(|c| c.0 == cell) {
                Some(c) => {
                    c.1 = [c.1[0] + v.pos[0], c.1[1] + v.pos[1], c.1[2] + v.pos[2]];
                    c.2 += 1.0;
                }
                None => cells.push((cell, v.pos, 1.0)),
            }
        }
        g.spots = cells
            .iter()
            .map(|(cell, sum, n)| {
                let c = [sum[0] / n, sum[1] / n, sum[2] / n];
                let ext = vs
                    .iter()
                    .filter(|v| v.pos.map(|x| (x / SPOT_CELL).floor() as i32) == *cell)
                    .map(|v| dot(sub(v.pos, c), sub(v.pos, c)).sqrt())
                    .fold(0.0, f32::max);
                (c, ext)
            })
            .collect();
    }
    (verts, groups)
}

const SHADER: &str = r#"
override EMISSIVE: bool = false;
override ADDITIVE: bool = false;

struct Cam {
    v: mat4x4<f32>,      // rows of the game's view matrix (row vectors)
    pv: mat4x4<f32>,     // last frame's
    proj: vec4<f32>,     // a, b, ox, oy (crate::upscale::projection)
    pproj: vec4<f32>,
    size: vec4<f32>,     // width, height, jitter x, jitter y
    fog: vec4<f32>,      // on, start, end, far clip
    fog_color: vec4<f32>,
    depth: vec4<f32>,    // near (upscaler depth), alpha reference, -, -
};
struct Rec { mat: u32, flags: u32, mframe: u32, pad1: u32, gouraud: vec4<f32>, slide: vec4<f32>, anchor: vec4<f32> };
// LIGHTS-BEGIN
struct Lights {
    sky: vec4<f32>,       // w: lighting on
    ground: vec4<f32>,    // w: number of point lights
    sun_dir: vec4<f32>,   // to the light; w: sun on
    sun_color: vec4<f32>, // w: specular strength
    sh_centre: vec4<f32>, // w: half extent
    sh_right: vec4<f32>,  // w: depth range
    sh_up: vec4<f32>,     // w: shadows on
    sh_dir: vec4<f32>,    // to the light; w: texel size (uv)
    eye: vec4<f32>,       // camera position
    water: vec4<f32>,     // simulation window corner x, z; cell size; cells per side
    water2: vec4<f32>,    // clock (s); water on; surface map reference height, depth range
    extra: vec4<f32>,     // baked shading kept, -, -, -
    water3: vec4<f32>,    // generated water: tint rgb; w: on
    water4: vec4<f32>,    // flow x / z (units per second), swell (1: the full swell), -
};
// LIGHTS-END
struct Point { pos: vec4<f32>, color: vec4<f32> };
@group(0) @binding(0) var<uniform> cam: Cam;
// The generated water's: the frame drawn before it (x of `scene_on`: there is one).
@group(0) @binding(1) var scene_c: texture_2d<f32>;
@group(0) @binding(2) var scene_d: texture_depth_2d;
@group(0) @binding(3) var scene_s: sampler;
@group(0) @binding(4) var<uniform> scene_on: vec4<f32>;
// Where the waterfalls land (`Gpu::falls`): how many (x), the points, and the way each fall
// faces there (horizontal; zero: either way).
struct Falls { n: vec4<f32>, p: array<vec4<f32>, 32>, o: array<vec4<f32>, 32> };
@group(0) @binding(5) var<uniform> falls: Falls;
@group(1) @binding(0) var t: texture_2d<f32>;
@group(1) @binding(1) var s: sampler;
@group(2) @binding(0) var<storage, read> recs: array<Rec>;
@group(2) @binding(1) var<storage, read> mats: array<mat4x4<f32>>;
@group(2) @binding(2) var<storage, read> prevs: array<mat4x4<f32>>;
@group(3) @binding(0) var<uniform> lt: Lights;
@group(3) @binding(1) var<storage, read> points: array<Point>;
@group(3) @binding(2) var shadow_map: texture_depth_2d;
@group(3) @binding(3) var shadow_s: sampler_comparison;
@group(3) @binding(4) var<storage, read> wave: array<vec2<f32>>;
@group(3) @binding(5) var surface: texture_depth_2d;
@group(3) @binding(9) var bed: texture_depth_2d;
@group(3) @binding(10) var flow: texture_2d<f32>;
@group(3) @binding(11) var obstacles: texture_2d<f32>;
@group(3) @binding(12) var obstacle_s: sampler;

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) bone: u32,
    @location(2) uv: vec2<f32>,
    @location(3) normal: vec4<f32>,
    @location(4) color: vec4<f32>,
};
struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) fog: f32,
    @location(3) vz: f32,
    @location(4) @interpolate(flat) flags: u32,
    @location(5) cur: vec4<f32>,
    @location(6) prev: vec4<f32>,
    @location(7) world: vec3<f32>,
    @location(8) normal: vec3<f32>,
    // How fast the texture slides (`v` per second; `Rec::slide`).
    @location(9) @interpolate(flat) slide: f32,
    // The liquid (`Rec::slide` z: `Liquid`; 0: none).
    @location(10) @interpolate(flat) mat: f32,
    // Lava: how much of its texture is painted crust (`Rec::slide` w).
    @location(11) @interpolate(flat) crust: f32,
    // A waterfall's lip height (`Rec::slide` w).
    @location(12) @interpolate(flat) top: f32,
};

// `mtx_xfm`: row-vector affine transform.
fn xfm(m: mat4x4<f32>, p: vec3<f32>) -> vec3<f32> {
    return p.x * m[0].xyz + p.y * m[1].xyz + p.z * m[2].xyz + m[3].xyz;
}
fn xfm_dir(m: mat4x4<f32>, d: vec3<f32>) -> vec3<f32> {
    return d.x * m[0].xyz + d.y * m[1].xyz + d.z * m[2].xyz;
}

// View-space point to clip space without jitter (camera space X = -x, Y = y, Z = z - 10).
fn clip(pr: vec4<f32>, p: vec3<f32>) -> vec4<f32> {
    let z = p.z - 10.0;
    return vec4<f32>(-pr.x * p.x + pr.z * z, pr.y * p.y + pr.w * z, 0.0, z);
}

// The vertex colour with its baked shading faded to `lt.extra.x` (the dynamic lights shade
// instead): brightness lifted towards 0.85 (about the art's average), hue kept, near-black (pits, fade-outs) kept.
fn delight(c: vec3<f32>) -> vec3<f32> {
    let m = max(max(c.r, c.g), c.b);
    if (m <= 0.0) { return c; }
    let lifted = mix(0.85, m, lt.extra.x);
    return c / m * mix(m, lifted, smoothstep(0.02, 0.25, m));
}

// `PCCalcModgeValues` at model-space point p, frame t: texture wobble x / z, swell.
fn modge(p: vec3<f32>, t: f32) -> vec3<f32> {
    let mx = (sin(p.z * (1.5 / 35.0) + sin(p.x * (1.3 / 49.0) + t * (2.0 / 56.0)) + t * (1.5 / 70.0)) + 1.0) * 0.1;
    let mz = (cos(p.x * (1.5 / 49.0) + cos(p.z * (1.3 / 29.0) + t * (2.0 / 66.0)) + t * (1.5 / 66.0)) + 1.0) * 0.1;
    let my = (sin(p.z * (1.2 / 26.0) + cos(p.x * (1.3 / 49.0) + t * (1.0 / 26.0)) + t * (1.0 / 26.0)) + 1.0) * 5.0;
    return vec3<f32>(mx, mz, my);
}

// WATER-BEGIN
// The simulation cell of world x / z (may be outside the window).
fn cell(xz: vec2<f32>) -> vec2<f32> {
    return (xz - lt.water.xy) / lt.water.z - 0.5;
}
fn in_window(c: vec2<i32>) -> bool {
    let n = i32(lt.water.w);
    return c.x >= 0 && c.y >= 0 && c.x < n && c.y < n;
}
fn wave_at(c: vec2<i32>) -> f32 {
    if (!in_window(c)) { return 0.0; }
    return wave[u32(c.y) * u32(lt.water.w) + u32(c.x)].x;
}
// Simulated wave height at world x / z (bilinear).
fn wave_h(xz: vec2<f32>) -> f32 {
    if (lt.water2.y < 0.5) { return 0.0; }
    let c = cell(xz);
    let i = vec2<i32>(floor(c));
    let f = c - floor(c);
    let a = mix(wave_at(i), wave_at(i + vec2<i32>(1, 0)), f.x);
    let b = mix(wave_at(i + vec2<i32>(0, 1)), wave_at(i + vec2<i32>(1, 1)), f.x);
    return mix(a, b, f.y);
}
// Water surface height at world x / z from the surface map (none: -1e9).
fn surface_y(xz: vec2<f32>) -> f32 {
    if (lt.water2.y < 0.5) { return -1e9; }
    let c = vec2<i32>(floor(cell(xz) + 0.5));
    if (!in_window(c)) { return -1e9; }
    let d = textureLoad(surface, c, 0);
    if (d >= 0.99999) { return -1e9; }
    return lt.water2.z + (0.5 - d) * lt.water2.w;
}
// A stream's flow at simulation cell c (the flow map, bilinear).
fn flow_map(c: vec2<f32>) -> vec2<f32> {
    let i = vec2<i32>(floor(c));
    let f = c - floor(c);
    let n = vec2<i32>(i32(lt.water.w) - 1);
    let z = vec2<i32>(0);
    let a = mix(textureLoad(flow, clamp(i, z, n), 0).xy, textureLoad(flow, clamp(i + vec2<i32>(1, 0), z, n), 0).xy, f.x);
    let b = mix(textureLoad(flow, clamp(i + vec2<i32>(0, 1), z, n), 0).xy, textureLoad(flow, clamp(i + vec2<i32>(1, 1), z, n), 0).xy, f.x);
    return mix(a, b, f.y);
}
// The liquid at world x / z (the flow map's; 0: water, else `Liquid`).
fn material_at(xz: vec2<f32>) -> f32 {
    let c = vec2<i32>(floor(cell(xz) + 0.5));
    if (!in_window(c)) { return 0.0; }
    return textureLoad(flow, c, 0).z;
}
// How fast the water at world x / z flows (x / z per second): the level's flow plus a
// stream's, smoothed over about 12 units (the art's texture density, so the speed, changes
// from face to face, and a stream runs into still water: carried things would tear there).
fn flow_at(xz: vec2<f32>) -> vec2<f32> {
    let c = cell(xz);
    let sum = flow_map(c) + flow_map(c + vec2<f32>(4.0, 4.0)) + flow_map(c + vec2<f32>(-4.0, 4.0))
        + flow_map(c + vec2<f32>(4.0, -4.0)) + flow_map(c + vec2<f32>(-4.0, -4.0));
    return lt.water4.xy + sum * 0.2;
}
// Something carried by the flow, seen at x / z: where it was at time 0 is x / z - flow · t,
// which stretches it out wherever the flow changes. So two copies are kept, each carried
// along for one period and then put back, half a period apart, crossfaded (flow mapping):
// x: how far the first has been carried (time), y: the second's, z: the first's weight.
fn flow_phases(t: f32) -> vec3<f32> {
    let p0 = fract(t / FLOW_PERIOD);
    let p1 = fract(t / FLOW_PERIOD + 0.5);
    return vec3<f32>(p0 * FLOW_PERIOD, p1 * FLOW_PERIOD, 1.0 - abs(1.0 - 2.0 * p0));
}
const FLOW_PERIOD: f32 = 2.5;
// How much the water at flow f moves (0: standing, 1: a stream): standing water is silky
// smooth, only what disturbs it shows (the simulation); a stream ripples as it runs.
fn moving(f: vec2<f32>) -> f32 {
    return smoothstep(1.5, 10.0, length(f));
}
// The ripples of running water (world units), carried by the flow; none on standing water.
fn ripples(xz0: vec2<f32>, t: f32, f: vec2<f32>) -> f32 {
    let m = moving(f);
    if (m <= 0.0) { return 0.0; }
    let ph = flow_phases(t);
    let d = f / max(length(f), 1e-3);
    return (ripples_at(xz0 - f * ph.x, t, d) * ph.z + ripples_at(xz0 - f * ph.y + vec2<f32>(37.0, 19.0), t, d) * (1.0 - ph.z)) * m;
}
// A stream's surface at time t (d: the way it runs): a slow swell, riffles across the flow
// (the current tumbling over its bed: short crests, long along the flow's width) and fine
// ripples (they bend the light into caustics and catch the sun).
fn ripples_at(xz: vec2<f32>, t: f32, d: vec2<f32>) -> f32 {
    let swell = 0.9 * sin(dot(xz, vec2<f32>(0.031, 0.017)) + t * 1.1)
         + 0.6 * sin(dot(xz, vec2<f32>(-0.022, 0.035)) + t * 0.9)
         + 0.35 * sin(dot(xz, vec2<f32>(0.11, 0.05)) + t * 1.9)
         + 0.25 * sin(dot(xz, vec2<f32>(-0.06, 0.13)) + t * 2.3)
         + 0.15 * sin(dot(xz, vec2<f32>(0.19, -0.16)) + t * 3.1);
    // In the stream's own frame: a along the flow, b across it.
    let q = vec2<f32>(dot(xz, d), dot(xz, vec2<f32>(-d.y, d.x)));
    let riffle = 0.12 * sin(q.x * 0.42 + sin(q.y * 0.07) * 2.0 + t * 1.7) * (0.6 + 0.4 * sin(q.y * 0.05 + q.x * 0.03))
         + 0.07 * sin(q.x * 0.71 - q.y * 0.18 + sin(q.y * 0.11 + t) * 1.5 + t * 2.3);
    var fine = 0.0;
    let ks = array<vec3<f32>, 5>(vec3<f32>(0.27, 0.11, 2.1), vec3<f32>(-0.18, 0.31, 2.6), vec3<f32>(0.41, -0.29, 3.3), vec3<f32>(-0.52, -0.37, 3.9), vec3<f32>(0.08, 0.66, 4.4));
    for (var k = 0; k < 5; k++) {
        let w = ks[k];
        fine += (0.035 / length(w.xy)) * sin(dot(w.xy, xz) + t * w.z);
    }
    return swell * max(lt.water4.z, 0.3) + riffle + fine;
}
fn water_h(xz: vec2<f32>) -> f32 {
    return wave_h(xz) + ripples(xz, lt.water2.x, flow_at(xz));
}
// The water's normal at world x / z.
fn water_n(xz: vec2<f32>) -> vec3<f32> {
    let e = lt.water.z;
    let f = flow_at(xz);
    let t = lt.water2.x;
    let dx = vec2<f32>(e, 0.0);
    let dz = vec2<f32>(0.0, e);
    // The simulated waves' slope, steepened where it is gentle (a soft curve: a butterfly's
    // ripple on calm water catches the light; a splash's stays as it is).
    var w = vec2<f32>(wave_h(xz + dx) - wave_h(xz - dx), wave_h(xz + dz) - wave_h(xz - dz)) / (2.0 * e);
    let wl = length(w);
    if (wl > 1e-6) { w *= WAVE_GAIN / (1.0 + wl * (WAVE_GAIN - 1.0) / 0.35) ; }
    let hx = w.x * 2.0 * e + ripples(xz + dx, t, f) - ripples(xz - dx, t, f);
    let hz = w.y * 2.0 * e + ripples(xz + dz, t, f) - ripples(xz - dz, t, f);
    return normalize(vec3<f32>(-hx, 2.0 * e, -hz));
}
const WAVE_GAIN: f32 = 1.6;
// WATER-END
// The river bed's height at world x / z (none: -1e9): the highest level surface below 15
// units over the water (above the water: a bank's lip hanging over it).
fn bed_y(xz: vec2<f32>) -> f32 {
    let c = vec2<i32>(floor(cell(xz) + 0.5));
    if (!in_window(c)) { return -1e9; }
    let d = textureLoad(bed, c, 0);
    if (d >= 0.99999) { return -1e9; }
    return lt.water2.z + (0.5 - d) * lt.water2.w;
}
// The deepest river bed (below sy) on the nearest ring around x / z that has one, out to 12
// units in 8 directions. Cells with no bed are under tall land as often as open water, so
// they count only when no ring has a bed (then -1e9: deep); nothing at all: sy.
fn bed_beside(xz: vec2<f32>, sy: f32) -> f32 {
    var open = false;
    for (var r = 1; r <= 4; r++) {
        var best = 1e9;
        for (var k = 0; k < 8; k++) {
            let a = f32(k) * 0.7853982;
            let y = bed_y(xz + vec2<f32>(cos(a), sin(a)) * (f32(r) * 3.0));
            if (y < -1e8) { open = true; } else if (y < sy - 0.5) { best = min(best, y); }
        }
        if (best < 1e8) { return best; }
    }
    return select(sy, -1e9, open);
}

// Caustic network at world x / z: bright lines where three warped travelling waves cancel
// (cells about 25 units across, two layers drifting apart).
fn caustic_layer(p: vec2<f32>, t: f32) -> f32 {
    let w = vec2<f32>(sin(p.y * 0.11 + t * 0.9), sin(p.x * 0.097 - t * 0.8)) * 3.5;
    let q = p + w;
    let a = sin(dot(q, vec2<f32>(0.25, 0.04)) + t * 1.3);
    let b = sin(dot(q, vec2<f32>(-0.12, 0.22)) - t * 1.1);
    let c = sin(dot(q, vec2<f32>(-0.14, -0.19)) + t * 0.9);
    let s = (a + b + c) * (1.0 / 3.0);
    return pow(1.0 - abs(s), 5.0);
}
fn caustic(p: vec2<f32>, t: f32) -> f32 {
    return caustic_layer(p, t) * 0.65 + caustic_layer(p * 1.37 + vec2<f32>(17.0, 5.0), t * 1.21) * 0.55;
}
// Light focused by the water above p (0 above water): the caustic network shifted along the
// sun's direction by the depth, brightened where the simulated surface curves into a lens.
fn caustics(p: vec3<f32>, to_light: vec3<f32>) -> f32 {
    let sy = surface_y(p.xz);
    let depth = sy - p.y;
    if (depth < 0.5 || material_at(p.xz) > 0.5) { return 0.0; }
    let q = p.xz + to_light.xz / max(to_light.y, 0.3) * depth;
    let e = lt.water.z;
    let lap = wave_h(q + vec2<f32>(e, 0.0)) + wave_h(q - vec2<f32>(e, 0.0)) + wave_h(q + vec2<f32>(0.0, e)) + wave_h(q - vec2<f32>(0.0, e)) - 4.0 * wave_h(q);
    let focus = clamp(1.0 + lap * 1.5, 0.3, 3.0);
    let fade = exp(-depth / 240.0) * smoothstep(0.5, 6.0, depth);
    let f = flow_at(q);
    let m = moving(f);
    // Standing water: only its waves focus the light (rings of light under a ripple).
    // (Soft-limited, so a faint ripple still draws a ring of light and a splash no glare.)
    let still = 0.8 * (1.0 - exp(-max(lap, 0.0) * 6.0));
    if (m <= 0.0) { return still * fade; }
    let ph = flow_phases(lt.water2.x);
    let c = caustic(q - f * ph.x, lt.water2.x) * ph.z + caustic(q - f * ph.y + vec2<f32>(37.0, 19.0), lt.water2.x) * (1.0 - ph.z);
    return mix(still, c * focus, m) * fade;
}

// World point in the sun's shadow map: (u-ish x, y in -1..1, depth 0..1).
fn light_space(p: vec3<f32>) -> vec3<f32> {
    let d = p - lt.sh_centre.xyz;
    return vec3<f32>(dot(d, lt.sh_right.xyz) / lt.sh_centre.w, dot(d, lt.sh_up.xyz) / lt.sh_centre.w, 0.5 - dot(d, lt.sh_dir.xyz) / lt.sh_right.w);
}

// How far a floating thing (`Rec::anchor`: a, riding the water there) is moved up at world
// point p: the original made it float at the original surface; it rides the generated one
// (which rests lower, and moves), a little sunk in (`FLOAT_DRAFT`), rising and falling with
// the waves and tilting with their slope. On acid only with its waves; over lava (stepping
// stones hover over it) and away from any water not at all.
fn float_lift(a: vec3<f32>, p: vec3<f32>) -> f32 {
    let sy = surface_y(a.xz);
    if (sy < -1e8) { return 0.0; }
    let m = material_at(a.xz);
    if (m > 1.5) { return 0.0; }
    let above = a.y - sy;
    let w = smoothstep(-30.0, -18.0, above) * (1.0 - smoothstep(22.0, 36.0, above));
    if (w <= 0.0) { return 0.0; }
    let f = flow_at(a.xz);
    let rest = select(-REST - FLOAT_DRAFT, -FLOAT_DRAFT * 0.5, m > 0.5);
    let e = 3.0;
    let h = wave_h(a.xz) + ripples(a.xz, lt.water2.x, f);
    let hx = wave_h(a.xz + vec2<f32>(e, 0.0)) + ripples(a.xz + vec2<f32>(e, 0.0), lt.water2.x, f) - h;
    let hz = wave_h(a.xz + vec2<f32>(0.0, e)) + ripples(a.xz + vec2<f32>(0.0, e), lt.water2.x, f) - h;
    // The slope felt by something this big: gentler than the water's own.
    let tilt = clamp(vec2<f32>(hx, hz) / e, vec2<f32>(-0.15), vec2<f32>(0.15)) * 0.6;
    return w * (rest + h * 0.8 + dot(p.xz - a.xz, tilt));
}
const FLOAT_DRAFT: f32 = 1.5;

@vertex fn vs(i: VIn, @builtin(instance_index) inst: u32) -> VOut {
    let r = recs[inst];
    let m = mats[r.mat + i.bone];
    var world = xfm(m, i.pos);
    var lift = 0.0;
    if (r.anchor.w > 0.5) {
        lift = float_lift(r.anchor.xyz, world);
        world.y += lift;
    }
    let vp = xfm(cam.v, world);
    let c = clip(cam.proj, vp);
    var o: VOut;
    // The CPU path's linear depth, so CPU and GPU geometry occlude each other as before.
    var zl = clamp(vp.z * 0.00025, 0.0, 1.0);
    if ((r.flags & 32u) != 0u) { zl = min(zl + 0.5 * 0.00025, 1.0); }
    let j = vec2<f32>(cam.size.z * 2.0 / cam.size.x, -cam.size.w * 2.0 / cam.size.y);
    o.pos = vec4<f32>(c.xy + j * c.w, zl * c.w, c.w);
    var col = i.color;
    if ((r.flags & 2u) != 0u) {
        col = vec4<f32>(r.gouraud.rgb, col.a);
    } else if (lt.sky.w > 0.5 && !EMISSIVE && (r.flags & 1u) == 0u) {
        col = vec4<f32>(delight(col.rgb), col.a);
    }
    o.color = clamp(col, vec4<f32>(0.0), vec4<f32>(1.0));
    o.uv = i.uv + vec2<f32>(0.0, r.slide.x);
    if ((r.flags & 8u) != 0u) {
        o.uv += modge(i.pos, f32(r.mframe)).xy * 0.4;
    }
    o.fog = 1.0;
    if (cam.fog.x > 0.5 && (r.flags & 1u) == 0u) {
        o.fog = clamp(1.0 - (vp.z - cam.fog.y) / (cam.fog.z - cam.fog.y), 0.0, 1.0);
    }
    o.vz = vp.z;
    o.flags = r.flags;
    o.cur = c;
    o.prev = clip(cam.pproj, xfm(cam.pv, xfm(prevs[r.mat + i.bone], i.pos) + vec3<f32>(0.0, lift, 0.0)));
    o.world = world;
    o.normal = xfm_dir(m, i.normal.xyz);
    o.slide = r.slide.y;
    o.mat = r.slide.z;
    o.crust = r.slide.w;
    o.top = r.slide.w;
    return o;
}

// Depth only, from the sun.
fn shadow_pos(i: VIn, inst: u32) -> vec4<f32> {
    let r = recs[inst];
    let l = light_space(xfm(mats[r.mat + i.bone], i.pos));
    return vec4<f32>(l.xy, clamp(l.z, 0.0, 1.0), 1.0);
}
@vertex fn vs_shadow(i: VIn, @builtin(instance_index) inst: u32) -> @builtin(position) vec4<f32> {
    return shadow_pos(i, inst);
}
// Cut-out textures (leaves, butterfly wings): their see-through pixels cast no shadow (the
// alpha test of the drawn pixels, `tightAlphaCmpRS`).
struct ShadowCut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs_shadow_cut(i: VIn, @builtin(instance_index) inst: u32) -> ShadowCut {
    var o: ShadowCut;
    o.pos = shadow_pos(i, inst);
    o.uv = i.uv;
    return o;
}
@fragment fn fs_shadow_cut(i: ShadowCut) {
    if (floor(textureSample(t, s, i.uv).a * 255.0 + 0.5) <= 100.0) { discard; }
}

// Water surface: the mesh's own swell (WAVE) plus the simulated waves and ripples.
@vertex fn vs_water(i: VIn, @builtin(instance_index) inst: u32) -> VOut {
    let r = recs[inst];
    let m = mats[r.mat];
    let md = modge(i.pos, f32(r.mframe));
    var lp = i.pos;
    if ((r.flags & 16u) != 0u) { lp.y += md.z; }
    var world = xfm(m, lp);
    world.y += water_h(world.xz);
    let vp = xfm(cam.v, world);
    let c = clip(cam.proj, vp);
    var o: VOut;
    let zl = clamp(vp.z * 0.00025, 0.0, 1.0);
    let j = vec2<f32>(cam.size.z * 2.0 / cam.size.x, -cam.size.w * 2.0 / cam.size.y);
    o.pos = vec4<f32>(c.xy + j * c.w, zl * c.w, c.w);
    var col = clamp(i.color, vec4<f32>(0.0), vec4<f32>(1.0));
    o.uv = i.uv + vec2<f32>(0.0, r.slide.x);
    if ((r.flags & 8u) != 0u) {
        o.uv += md.xy * 0.4;
        if ((r.flags & 16u) != 0u) {
            // `50+(unsigned long)((m1x+m1z)*400)<<24`
            col.a = (50.0 + floor((md.x + md.y) * 400.0)) / 255.0;
        }
    }
    o.color = col;
    o.fog = 1.0;
    if (cam.fog.x > 0.5) {
        o.fog = clamp(1.0 - (vp.z - cam.fog.y) / (cam.fog.z - cam.fog.y), 0.0, 1.0);
    }
    o.vz = vp.z;
    o.flags = r.flags;
    o.cur = c;
    o.prev = c;
    o.world = world;
    o.normal = vec3<f32>(0.0, 1.0, 0.0);
    o.slide = r.slide.y;
    return o;
}

// The lowest water within r cells of c (none: 1e9).
fn grid_ymin_within(c: vec2<i32>, r: i32) -> f32 {
    var best = 1e9;
    for (var dy = -r; dy <= r; dy++) {
        for (var dx = -r; dx <= r; dx++) {
            let q = c + vec2<i32>(dx, dy);
            if (!in_window(q)) { continue; }
            let d = textureLoad(surface, q, 0);
            if (d < 0.99999) { best = min(best, lt.water2.z + (0.5 - d) * lt.water2.w); }
        }
    }
    return best;
}

fn water_color(i: VOut, additive: bool) -> vec4<f32> {
    if (i.vz - 10.0 >= cam.fog.w) { discard; }
    // With the generated surface the mesh draws only its steep faces: waterfalls, and the
    // skirts the art hangs under the surface along banks and islands (hiding seams). Under
    // the water (the lowest water about it: a fall's pool, not the stream it falls from)
    // they are no surface: the bed and the banks seen through the water bound it.
    if (lt.water3.w > 0.5) {
        let c = vec2<i32>(floor(cell(i.world.xz) + 0.5));
        if (i.world.y < grid_ymin_within(c, 3) - REST) { discard; }
    }
    let n = water_n(i.world.xz);
    // The waves bend the texture (a cheap refraction).
    var base = textureSample(t, s, i.uv + n.xz * 0.04) * i.color;
    base = fall_sheet(i, base);
    let v = normalize(lt.eye.xyz - i.world);
    let fres = 0.03 + 0.97 * pow(1.0 - max(dot(n, v), 0.0), 5.0);
    let refl = lt.sky.rgb * 1.1;
    var spec = vec3<f32>(0.0);
    if (lt.sun_dir.w > 0.5) {
        let h = normalize(lt.sun_dir.xyz + v);
        spec = lt.sun_color.rgb * (pow(max(dot(n, h), 0.0), 220.0) * 2.5 + pow(max(dot(n, h), 0.0), 30.0) * 0.12) * shadow(i.world);
    }
    var c: vec4<f32>;
    if (additive) {
        // Added with its alpha (`SrcAlpha, One`): the reflection and glint are scaled back up.
        let a = max(base.a, 0.15);
        c = vec4<f32>(base.rgb + (refl * fres * 0.45 + spec) / a, base.a);
    } else {
        c = vec4<f32>(mix(base.rgb, refl, fres * 0.6) + spec, clamp(max(base.a, fres), 0.0, 1.0));
    }
    if (cam.fog.x > 0.5) {
        let f = select(cam.fog_color.rgb, vec3<f32>(0.0), additive);
        c = vec4<f32>(mix(f, c.rgb, i.fog), c.a);
    }
    return c;
}

@fragment fn fs_water(i: VOut) -> @location(0) vec4<f32> {
    return water_color(i, ADDITIVE);
}
@fragment fn fs_water_mrt(i: VOut) -> FOut {
    var o: FOut;
    o.c = water_color(i, ADDITIVE);
    return o;
}

// The generated water surface: a grid of 2×2-cell quads over the simulation window, at the
// surface map's height, moved by the waves; drawn where the surface map has water.
const GRID: u32 = 512u;
// How far below the original water surface the generated one rests.
const REST: f32 = 2.5;
// The highest water within r cells of c (none: -1e9).
fn grid_y_within(c: vec2<i32>, r: i32) -> f32 {
    var best = -1e9;
    for (var dy = -r; dy <= r; dy++) {
        for (var dx = -r; dx <= r; dx++) {
            let q = c + vec2<i32>(dx, dy);
            if (!in_window(q)) { continue; }
            let d = textureLoad(surface, q, 0);
            if (d < 0.99999) { best = max(best, lt.water2.z + (0.5 - d) * lt.water2.w); }
        }
    }
    return best;
}
const CORNERS = array<vec2<u32>, 6>(vec2<u32>(0u, 0u), vec2<u32>(1u, 0u), vec2<u32>(0u, 1u), vec2<u32>(1u, 0u), vec2<u32>(1u, 1u), vec2<u32>(0u, 1u));
fn grid_cell(vi: u32) -> vec2<i32> {
    let q = vi / 6u;
    return vec2<i32>((vec2<u32>(q % GRID, q / GRID) + CORNERS[vi % 6u]) * 2u);
}
@vertex fn vs_grid(@builtin(vertex_index) vi: u32) -> VOut {
    let c = grid_cell(vi);
    var o: VOut;
    // A corner's height is the water within 2 cells (its quads reach 2 cells). A triangle is
    // drawn when any corner has water so near; its dry corners take the water within 4 cells
    // (which holds the wet corner's search), so edge triangles stay in place and their dry
    // part is discarded per pixel. Decided per triangle: a corner dropped alone would drag
    // its triangles across the screen.
    var y = grid_y_within(c, 2);
    if (y < -1e8) {
        let t = vi - vi % 3u;
        var wet = false;
        for (var k = 0u; k < 3u; k++) {
            let ck = grid_cell(t + k);
            if (any(ck != c) && grid_y_within(ck, 2) > -1e8) { wet = true; }
        }
        if (wet) { y = grid_y_within(c, 4); }
    }
    if (y < -1e8 || lt.water3.w < 0.5) {
        o.pos = vec4<f32>(0.0, 0.0, 0.0, 1.0);
        return o;
    }
    let xz = lt.water.xy + (vec2<f32>(c) + 0.5) * lt.water.z;
    // Next to anything the crests could reach (a bank's lip, a platform's top just over the
    // water) the water lies still, at rest below it: waves would poke through.
    var lip = false;
    for (var dy = -2; dy <= 2; dy += 2) {
        for (var dx = -2; dx <= 2; dx += 2) {
            let q = c + vec2<i32>(dx, dy);
            if (in_window(q)) {
                let d = textureLoad(bed, q, 0);
                if (d < 0.99999 && lt.water2.z + (0.5 - d) * lt.water2.w > y - 3.5) { lip = true; }
            }
        }
    }
    // At rest 2.5 below the original surface (`REST`): crests reach it, troughs go below; the
    // banks were made for that height (the original's swell only lifted the water).
    let world = vec3<f32>(xz.x, y + select(water_h(xz), 0.0, lip) - REST, xz.y);
    let vp = xfm(cam.v, world);
    let cl = clip(cam.proj, vp);
    let zl = clamp(vp.z * 0.00025, 0.0, 1.0);
    let j = vec2<f32>(cam.size.z * 2.0 / cam.size.x, -cam.size.w * 2.0 / cam.size.y);
    o.pos = vec4<f32>(cl.xy + j * cl.w, zl * cl.w, cl.w);
    o.color = vec4<f32>(1.0);
    o.uv = xz;
    o.fog = 1.0;
    if (cam.fog.x > 0.5) {
        o.fog = clamp(1.0 - (vp.z - cam.fog.y) / (cam.fog.z - cam.fog.y), 0.0, 1.0);
    }
    o.vz = vp.z;
    o.cur = cl;
    o.prev = cl;
    o.world = world;
    o.normal = vec3<f32>(0.0, 1.0, 0.0);
    return o;
}

// How thick the water is over cell c (no water: 0).
fn cell_thick(c: vec2<i32>) -> f32 {
    if (!in_window(c)) { return 0.0; }
    let d = textureLoad(surface, c, 0);
    if (d >= 0.99999) { return 0.0; }
    let sy = lt.water2.z + (0.5 - d) * lt.water2.w;
    let xz = lt.water.xy + (vec2<f32>(c) + 0.5) * lt.water.z;
    var by = bed_y(xz);
    // Under a bank's lip (a stone rim, grass hanging over the water) the water goes on as
    // deep as the open water beside it, up to the bank, which hides it (`vs_grid` keeps it
    // still there, below the lip).
    if (by > sy - 0.5) {
        by = bed_beside(xz, sy);
    }
    // No bed: open water, deep. Measured from where the water rests: a surface between the
    // rest and the original height is dry (no water drawn over a platform's top).
    if (by < -1e8) { return 200.0; }
    return max(sy - REST - by, 0.0);
}
// World point p on the screen: uv and view depth (behind the camera: uv -1).
fn screen_uv(p: vec3<f32>) -> vec3<f32> {
    let vp = xfm(cam.v, p);
    let c = clip(cam.proj, vp);
    if (c.w < 1.0) { return vec3<f32>(-1.0, -1.0, vp.z); }
    let j = vec2<f32>(cam.size.z * 2.0 / cam.size.x, -cam.size.w * 2.0 / cam.size.y);
    let ndc = c.xy / c.w + j;
    return vec3<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5, vp.z);
}
// View depth of the frame drawn before the water at uv (the depth buffer is linear).
fn scene_vz(uv: vec2<f32>) -> f32 {
    let dims = vec2<i32>(textureDimensions(scene_d));
    let px = clamp(vec2<i32>(uv * vec2<f32>(dims)), vec2<i32>(0), dims - 1);
    return textureLoad(scene_d, px, 0) / 0.00025;
}
// Screen-space reflection: what the ray from p along r hits in the frame drawn before the
// water (rgb) and how sure that is (a; 0: off the screen or nothing hit).
fn ssr(p: vec3<f32>, r: vec3<f32>, jitter: f32) -> vec4<f32> {
    // The first step jittered per pixel: fixed steps would land on the same things along a
    // line (dotted seams).
    var len = 3.0;
    var a = p + r * (len * jitter);
    for (var k = 0; k < 32; k++) {
        let b = a + r * len;
        let sb = screen_uv(b);
        if (any(sb.xy < vec2<f32>(0.0)) || any(sb.xy > vec2<f32>(1.0))) { return vec4<f32>(0.0); }
        let d = sb.z - scene_vz(sb.xy);
        // Behind what the frame shows there, not far behind it (else it passed behind a thin
        // thing and goes on).
        if (d > 0.0 && d < len * 2.0 + 20.0) {
            var lo = a;
            var hi = b;
            for (var m = 0; m < 5; m++) {
                let mid = (lo + hi) * 0.5;
                let sm = screen_uv(mid);
                if (sm.z - scene_vz(sm.xy) > 0.0) { hi = mid; } else { lo = mid; }
            }
            let sh = screen_uv(hi);
            // Only a surface the ray really reaches (close behind the refined point).
            if (sh.z - scene_vz(sh.xy) > 4.0 + len * 0.25) { return vec4<f32>(0.0); }
            let border = min(min(sh.x, 1.0 - sh.x), min(sh.y, 1.0 - sh.y));
            let sure = smoothstep(0.0, 0.06, border) * (1.0 - f32(k) / 32.0);
            return vec4<f32>(textureSampleLevel(scene_c, scene_s, sh.xy, 0.0).rgb, sure);
        }
        a = b;
        len *= 1.15;
    }
    return vec4<f32>(0.0);
}
// The sky seen along r: the level's backdrop colour (its fog) where there is one, else a
// blue as bright as the sky light; deeper overhead, paler towards the horizon.
fn sky_along(r: vec3<f32>) -> vec3<f32> {
    var sky = vec3<f32>(0.42, 0.62, 0.9) * dot(lt.sky.rgb, vec3<f32>(0.333));
    if (cam.fog.x > 0.5 && dot(cam.fog_color.rgb, vec3<f32>(1.0)) > 0.05) { sky = cam.fog_color.rgb; }
    let horizon = mix(sky, vec3<f32>(1.0), 0.2);
    return mix(horizon, sky * vec3<f32>(0.75, 0.85, 1.0), sqrt(clamp(r.y, 0.0, 1.0)));
}
// How much the simulated surface is moving at x / z (churn: wakes, splashes, waterfalls).
fn churn_at(c: vec2<i32>) -> f32 {
    if (!in_window(c)) { return 0.0; }
    let w = wave[u32(c.y) * u32(lt.water.w) + u32(c.x)];
    return abs(w.x - w.y);
}
fn churn(xz: vec2<f32>) -> f32 {
    if (lt.water2.y < 0.5) { return 0.0; }
    let c = cell(xz);
    let i = vec2<i32>(floor(c));
    let f = c - floor(c);
    return mix(mix(churn_at(i), churn_at(i + vec2<i32>(1, 0)), f.x), mix(churn_at(i + vec2<i32>(0, 1)), churn_at(i + vec2<i32>(1, 1)), f.x), f.y);
}

// Whether any of the four cells about x / z holds another liquid (an opaque stream at the top
// of its fall: the generated water's corners there would take its height).
fn liquid_near(xz: vec2<f32>) -> bool {
    let c = vec2<i32>(floor(cell(xz)));
    for (var k = 0; k < 4; k++) {
        let q = c + vec2<i32>(k & 1, k >> 1);
        if (in_window(q) && textureLoad(flow, q, 0).z > 0.5) { return true; }
    }
    return false;
}

fn grid_color(i: VOut) -> vec4<f32> {
    // A triangle hanging from water at the top of a waterfall to its pool (all but upright:
    // waves never are): none.
    let tn = cross(dpdx(i.world), dpdy(i.world));
    if (tn.y * tn.y < 0.0025 * dot(tn, tn)) { discard; }
    if (i.vz - 10.0 >= cam.fog.w) { discard; }
    // ... or lying along its top edge over the pool (corners take the highest water near them).
    let own = surface_y(i.world.xz);
    if (own > -1e8 && i.world.y > own + 20.0) { discard; }
    if (liquid_near(i.world.xz)) { discard; }
    // The water's thickness, blended between the four nearest cells (smooth shores).
    let cf = cell(i.world.xz);
    let c0 = vec2<i32>(floor(cf));
    let f = cf - floor(cf);
    let thick = mix(mix(cell_thick(c0), cell_thick(c0 + vec2<i32>(1, 0)), f.x),
                    mix(cell_thick(c0 + vec2<i32>(0, 1)), cell_thick(c0 + vec2<i32>(1, 1)), f.x), f.y);
    // Over another liquid (acid, lava: drawn by their own meshes) no water.
    if (thick <= 0.0 || material_at(i.world.xz) > 0.5) { discard; }
    let tm = lt.water2.x;
    // The waves and ripples; sparkles fade out with distance (no shimmer).
    let near = 1.0 - smoothstep(250.0, 1200.0, i.vz);
    let n = water_n(i.world.xz);
    let v = normalize(lt.eye.xyz - i.world);
    // Fresnel, lifted a little (the game's cameras look down steeply, where real water
    // reflects little).
    let fres = 0.07 + 0.93 * pow(1.0 - max(dot(n, v), 0.0), 4.0);
    // The original water texture as detail, in world space, drifting and bent by the waves.
    let fl = flow_at(i.world.xz);
    let mv = moving(fl);
    let ph = flow_phases(tm);
    let dq = vec2<f32>(tm * 0.012, tm * 0.007) * mv + n.xz * 0.05;
    let streamed = textureSample(t, s, (i.world.xz - fl * ph.x) / 48.0 + dq).rgb * ph.z
        + textureSample(t, s, (i.world.xz - fl * ph.y) / 48.0 + dq + vec2<f32>(0.37, 0.19)).rgb * (1.0 - ph.z);
    // Standing water: its texture lies still, softened (larger, fainter): a calm, deep body.
    let calm = mix(vec3<f32>(0.5), textureSample(t, s, i.world.xz / 160.0 + n.xz * 0.05).rgb, 0.35);
    let detail = mix(calm, streamed, mv);
    let body = lt.water3.rgb;
    var sh = 1.0;
    var spec = vec3<f32>(0.0);
    var lit = mix(lt.ground.rgb, lt.sky.rgb, n.y * 0.5 + 0.5) * 0.8;
    if (lt.sun_dir.w > 0.5) {
        sh = shadow(i.world);
        // Lit by the waves' slopes (the water's surface scatters a little).
        lit += lt.sun_color.rgb * max(dot(n, lt.sun_dir.xyz), 0.0) * sh * 1.2;
        let h = normalize(lt.sun_dir.xyz + v);
        let nh = max(dot(n, h), 0.0);
        // A sharp glint, sparkles off the fine ripples, a soft sheen.
        spec = lt.sun_color.rgb * (pow(nh, 260.0) * 3.0 + pow(nh, 900.0) * 6.0 * near + pow(nh, 40.0) * 0.1) * sh;
    }
    // In the water: its colour (the original texture's) lit, with the texture as detail.
    let scatter = body * lit * (0.75 + 0.5 * detail);
    // Reflection: the frame itself where the reflected ray finds it, else the sky.
    var r = reflect(-v, n);
    r = normalize(vec3<f32>(r.x, max(r.y, 0.02), r.z));
    var refl = sky_along(r);
    let on = scene_on.x > 0.5;
    if (on) {
        let hit = ssr(i.world, r, fract(52.9829189 * fract(dot(i.pos.xy, vec2<f32>(0.06711056, 0.00583715)))));
        refl = mix(refl, hit.rgb, hit.a);
    }
    // Shore foam, and foam where the simulation churns (waterfalls landing, wakes).
    var edge = smoothstep(0.0, 4.0, thick);
    let shore = (1.0 - smoothstep(0.0, 5.0, thick)) * 0.35;
    let stir = smoothstep(0.12, 0.6, churn(i.world.xz)) * 0.5;
    let foam = max(clamp((shore + stir) * (0.4 + 0.6 * caustic_layer(i.world.xz * 0.7, tm * 1.2)), 0.0, 1.0), fall_foam(i.world, tm, near));
    let foam_rgb = vec3<f32>(foam) * max(lit, vec3<f32>(0.6));
    var rgb: vec3<f32>;
    var a: f32;
    if (on) {
        // What lies under the water (bent a little by the waves, never something in front of
        // the water), dimmed and tinted along the path through it (red goes first).
        let px = i.pos.xy / vec2<f32>(textureDimensions(scene_c));
        var uv = px + n.xz * vec2<f32>(-1.0, 1.0) * (min(thick, 30.0) * 0.6 / max(i.vz, 40.0));
        if (scene_vz(uv) < i.vz) { uv = px; }
        let under = textureSampleLevel(scene_c, scene_s, uv, 0.0).rgb;
        // How far the view ray runs through the water, to whatever it meets (the bed, a wall,
        // a rock under the water: the frame's depth, exact per pixel, where the bed map has
        // 1.5-unit cells and sees no walls); the shore fades where it is shallow, along the
        // banks' own shape.
        let ray = max(scene_vz(px) - i.vz, 0.0) * length(i.world - lt.eye.xyz) / max(i.vz, 1.0);
        let path = min(ray, thick / max(v.y, 0.2) + 40.0);
        edge = smoothstep(0.0, 2.5, ray * max(v.y, 0.1));
        let tr = exp(-path * vec3<f32>(0.024, 0.010, 0.007));
        let water = under * tr + scatter * (vec3<f32>(1.0) - tr);
        rgb = mix(water, refl, fres) + spec + foam_rgb;
        a = 1.0;
    } else {
        // No copy of the frame: the water is blended over it.
        let opacity = 1.0 - exp(-thick / 45.0);
        rgb = scatter * opacity * (1.0 - fres) + refl * fres + spec + foam_rgb;
        a = clamp(1.0 - (1.0 - opacity) * (1.0 - fres) + foam, 0.0, 1.0);
    }
    // Premultiplied; the shore fades out.
    rgb *= edge;
    a *= edge;
    if (cam.fog.x > 0.5) { rgb = mix(cam.fog_color.rgb * a, rgb, i.fog); }
    return vec4<f32>(rgb, a);
}
// White water where the waterfalls land (`falls`): foam churned out from under them (carried
// outwards, flow-mapped), broken at its edges, and bubbles welling up, swelling and popping
// (`near`: close enough for them to show).
fn fall_foam(p: vec3<f32>, t: f32, near: f32) -> f32 {
    var w = 0.0;
    var away = vec2<f32>(0.0);
    let n = u32(falls.n.x);
    for (var k = 0u; k < n; k++) {
        let f = falls.p[k].xyz;
        // Feet on this water (a fall's mesh may go on far under its pool).
        if (f.y > p.y + 30.0 || f.y < p.y - 400.0) { continue; }
        let d = p.xz - f.xz;
        let g = exp(-dot(d, d) / (FALL_POOL * FALL_POOL));
        w += g;
        away += (d / max(length(d), 0.5) + falls.o[k].xz * 0.6) * g;
    }
    if (w < 0.02) { return 0.0; }
    w = min(w, 1.0);
    // Carried outwards at up to 16 units a second.
    let vel = away / max(length(away), 1e-3) * 16.0 * w;
    let ph = flow_phases(t);
    let a = fall_foam_at(p.xz - vel * ph.x, t);
    let b = fall_foam_at(p.xz - vel * ph.y + vec2<f32>(23.0, 41.0), t);
    let fn_ = mix(b, a, ph.z);
    let th = 0.55 - w * 1.05;
    var foam = smoothstep(th, th + 0.3, fn_) * smoothstep(0.02, 0.25, w);
    // Bubbles (cells 4 units across): each swells for a moment and pops into a spreading
    // ring; more of them nearer the fall.
    let q = p.xz / 4.0;
    let ci = vec2<i32>(floor(q));
    if (lhash(ci, 41u) < w * 0.8 && near > 0.0) {
        let life = 0.5 + lhash(ci, 42u) * 0.7;
        let bp = fract(t / life + lhash(ci, 43u));
        let d = length(fract(q) - (vec2<f32>(0.3) + vec2<f32>(lhash(ci, 44u), lhash(ci, 45u)) * 0.4));
        let rmax = 0.12 + lhash(ci, 46u) * 0.14;
        var b = 0.0;
        if (bp < 0.8) {
            let r = rmax * sqrt(bp / 0.8);
            // The dome's bright rim.
            b = smoothstep(r * 0.55, r, d) * (1.0 - smoothstep(r, r + 0.04, d));
        } else {
            let k = (bp - 0.8) / 0.2;
            let rr = rmax * (1.0 + k * 1.5);
            b = exp(-pow((d - rr) / 0.035, 2.0)) * (1.0 - k);
        }
        foam = max(foam, b * near * w);
    }
    return foam;
}
fn fall_foam_at(xz: vec2<f32>, t: f32) -> f32 {
    // Churning: two layers of blobs turning over (drifting against each other).
    let n1 = lnoise(xz * 0.13 + vec2<f32>(t * 0.35, -t * 0.25), vec2<i32>(4096), 47u).x;
    let n2 = lnoise(xz * 0.31 + vec2<f32>(-t * 0.6, t * 0.45), vec2<i32>(4096), 48u).x;
    return n1 * 0.65 + n2 * 0.35;
}
// How far a waterfall's foam reaches from where it lands (world units).
const FALL_POOL: f32 = 26.0;

@fragment fn fs_grid(i: VOut) -> @location(0) vec4<f32> {
    return grid_color(i);
}
@fragment fn fs_grid_mrt(i: VOut) -> FOut {
    var o: FOut;
    o.c = grid_color(i);
    return o;
}

// Mist rising where the waterfalls land, and spray thrown up there (`falls`): camera-facing
// quads, each foot's first `MIST_PUFFS` soft puffs that rise, swell, drift out over the pool
// and fade, the rest drops thrown out and falling back. Made from the water's clock alone.
struct MistOut {
    @builtin(position) pos: vec4<f32>,
    // The quad's corner (-1..1).
    @location(0) q: vec2<f32>,
    // Opacity, a seed, 1 for a drop, the pool's height.
    @location(1) @interpolate(flat) a: vec4<f32>,
    @location(2) vz: f32,
    @location(3) fog: f32,
    @location(4) y: f32,
};
const MIST_PUFFS: u32 = 10u;
const MIST_PER_FOOT: u32 = 18u;
@vertex fn vs_mist(@builtin(vertex_index) vi: u32) -> MistOut {
    var o: MistOut;
    o.pos = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    let id = vi / 6u;
    let foot = id / MIST_PER_FOOT;
    let k = id % MIST_PER_FOOT;
    if (foot >= u32(falls.n.x)) { return o; }
    let f = falls.p[foot].xyz;
    let out = falls.o[foot].xz;
    // Along the fall (either way when it is not known).
    let side = select(vec2<f32>(1.0, 0.0), vec2<f32>(-out.y, out.x), dot(out, out) > 0.5);
    // The pool it lands in (the fall's mesh may go on under it).
    var base = f.y;
    let sy = max(surface_y(f.xz + out * 6.0), surface_y(f.xz));
    if (sy > f.y - 30.0 && sy < f.y + 400.0) { base = sy - REST; }
    let t = lt.water2.x;
    let seed = vec2<i32>(i32(foot), i32(k));
    var c: vec3<f32>;
    var size: f32;
    var a: f32;
    var drop = 0.0;
    if (k < MIST_PUFFS) {
        let life = 3.5 + lhash(seed, 31u) * 2.5;
        let ph0 = t / life + lhash(seed, 32u);
        let ph = fract(ph0);
        // A new place each time round.
        let s = vec2<i32>(i32(foot * 64u + k), i32(floor(ph0)));
        let h1 = lhash(s, 33u);
        let h2 = lhash(s, 34u);
        let h3 = lhash(s, 35u);
        var start = f.xz + side * (h1 * 2.0 - 1.0) * 22.0 + out * (6.0 + h2 * 16.0);
        if (dot(out, out) < 0.5) { start += vec2<f32>(h2 - 0.5, h3 - 0.5) * 24.0; }
        let wind = vec2<f32>(lhash(s, 36u) - 0.5, lhash(s, 37u) - 0.5) * 16.0;
        let xz = start + (out * 24.0 + wind) * ph;
        // Rising, slowing as it goes; swelling.
        let rise = 5.0 + (1.0 - (1.0 - ph) * (1.0 - ph)) * (26.0 + h3 * 26.0);
        c = vec3<f32>(xz.x, base + rise, xz.y);
        size = 10.0 + 20.0 * sqrt(ph) + h1 * 6.0;
        // As thick wherever feet crowd together.
        var crowd = 0.0;
        for (var m = 0u; m < u32(falls.n.x); m++) {
            crowd += 1.0 - smoothstep(20.0, 45.0, distance(falls.p[m].xz, f.xz));
        }
        a = smoothstep(0.0, 0.15, ph) * (1.0 - smoothstep(0.35, 1.0, ph)) * 0.65 / max(crowd, 1.0);
    } else {
        // A drop thrown out of the churn, falling back.
        let life = 0.55 + lhash(seed, 51u) * 0.5;
        let ph0 = t / life + lhash(seed, 52u);
        let ph = fract(ph0);
        let s = vec2<i32>(i32(foot * 64u + k), i32(floor(ph0)));
        let h1 = lhash(s, 53u);
        let h2 = lhash(s, 54u);
        let h3 = lhash(s, 55u);
        let tau = ph * life;
        var v = out * (20.0 + h2 * 40.0) + side * (h3 - 0.5) * 40.0;
        if (dot(out, out) < 0.5) { v = vec2<f32>(h2 - 0.5, h3 - 0.5) * 70.0; }
        let start = f.xz + side * (h1 * 2.0 - 1.0) * 26.0;
        let xz = start + v * tau;
        let y = base + 2.0 + (30.0 + h2 * 45.0) * tau - 120.0 * tau * tau;
        c = vec3<f32>(xz.x, y, xz.y);
        size = 0.7 + h3 * 0.7;
        a = (1.0 - smoothstep(0.6, 1.0, ph)) * step(base, y) * 0.8;
        drop = 1.0;
    }
    let corner = vec2<f32>(CORNERS[vi % 6u]) * 2.0 - 1.0;
    let vp = xfm(cam.v, c);
    // Not in the camera's face.
    a *= smoothstep(20.0, 70.0, vp.z);
    if (a <= 0.002) { return o; }
    let vq = vp + vec3<f32>(corner * size, 0.0);
    let cl = clip(cam.proj, vq);
    let zl = clamp(vq.z * 0.00025, 0.0, 1.0);
    let j = vec2<f32>(cam.size.z * 2.0 / cam.size.x, -cam.size.w * 2.0 / cam.size.y);
    o.pos = vec4<f32>(cl.xy + j * cl.w, zl * cl.w, cl.w);
    o.q = corner;
    o.a = vec4<f32>(a, lhash(seed, 38u) * 64.0, drop, base);
    o.vz = vq.z;
    // The quad's corner in the world, for its height over the pool.
    o.y = c.y + corner.y * size;
    o.fog = 1.0;
    if (cam.fog.x > 0.5) {
        o.fog = clamp(1.0 - (vp.z - cam.fog.y) / (cam.fog.z - cam.fog.y), 0.0, 1.0);
    }
    return o;
}
fn mist_color(i: MistOut) -> vec4<f32> {
    let r2 = dot(i.q, i.q);
    if (r2 >= 1.0 || i.vz - 10.0 >= cam.fog.w) { discard; }
    let soft = (1.0 - r2) * (1.0 - r2);
    var a: f32;
    if (i.a.z > 0.5) {
        a = soft * i.a.x;
    } else {
        // A soft puff, curling slowly (noise in the quad, its own per puff).
        let t = lt.water2.x;
        let o = vec2<f32>(i.a.y, i.a.y * 0.7);
        let n = lnoise(i.q * 1.6 + o + vec2<f32>(t * 0.12, -t * 0.08), vec2<i32>(4096), 61u).x * 0.6
              + lnoise(i.q * 3.7 + o * 1.3 + vec2<f32>(-t * 0.2, t * 0.15), vec2<i32>(4096), 62u).x * 0.4;
        a = i.a.x * soft * smoothstep(-0.45, 0.55, n + soft * 0.4);
        // Thin at the pool (the water is not in the depth buffer).
        a *= smoothstep(i.a.w - 2.0, i.a.w + 10.0, i.y);
    }
    // Soft where it meets the scene.
    if (scene_on.x > 0.5) {
        let d = scene_vz(i.pos.xy / vec2<f32>(textureDimensions(scene_d))) - i.vz;
        a *= smoothstep(0.0, 8.0, d);
    }
    // Lit by the sky and the sun (bright: the mist scatters it), a little of the water's hue.
    var l = mix(lt.ground.rgb, lt.sky.rgb, 0.8);
    if (lt.sun_dir.w > 0.5) { l += lt.sun_color.rgb * 0.6; }
    if (lt.sky.w < 0.5) { l = vec3<f32>(0.9); }
    var rgb = min(l, vec3<f32>(1.1)) * mix(vec3<f32>(1.0), lt.water3.rgb * 1.6 + 0.3, 0.2 * lt.water3.w);
    if (i.a.z > 0.5) { rgb = max(rgb, vec3<f32>(0.9)) * 1.2; }
    if (cam.fog.x > 0.5) { rgb = mix(cam.fog_color.rgb, rgb, i.fog); }
    return vec4<f32>(rgb * a, a);
}
@fragment fn fs_mist(i: MistOut) -> @location(0) vec4<f32> {
    return mist_color(i);
}
@fragment fn fs_mist_mrt(i: MistOut) -> FOut {
    var o: FOut;
    o.c = mist_color(i);
    return o;
}

// The opaque meshes seen from above into the bed map (the highest surface under the water).
struct BedOut { @builtin(position) pos: vec4<f32>, @location(0) world: vec3<f32>, @location(1) @interpolate(flat) keep: u32 };
@vertex fn vs_bed(i: VIn, @builtin(instance_index) inst: u32) -> BedOut {
    let r = recs[inst];
    let w = xfm(mats[r.mat + i.bone], i.pos);
    let size = lt.water.z * lt.water.w;
    var o: BedOut;
    o.pos = vec4<f32>((w.x - lt.water.x) / size * 2.0 - 1.0, 1.0 - (w.z - lt.water.y) / size * 2.0, clamp(0.5 - (w.y - lt.water2.z) / lt.water2.w, 0.0, 1.0), 1.0);
    o.world = w;
    // The level and the river beds only (not fish, logs and other things in the water).
    o.keep = r.flags & (4u | 32u);
    return o;
}
// Up to 15 units over the water: a bank's lip hanging over it (`grid_color`).
@fragment fn fs_bed(i: BedOut) -> @builtin(frag_depth) f32 {
    let sy = surface_y(i.world.xz);
    if (i.keep == 0u || i.world.y > sy + 15.0) { discard; }
    return i.pos.z;
}

// The opaque meshes seen from above into the obstacle map: what stands in the water (from just
// under its surface to 40 units over it; the highest counts less). The sky is left out, and
// the liquids but where they stand out of the surface (a volcano's lava-covered cone). Where
// the liquid stops at what stands in it, the surface beside it is taken.
struct ObstOut { @builtin(position) pos: vec4<f32>, @location(0) world: vec3<f32>, @location(1) @interpolate(flat) flags: u32 };
@vertex fn vs_obstacle(i: VIn, @builtin(instance_index) inst: u32) -> ObstOut {
    let r = recs[inst];
    let w = xfm(mats[r.mat + i.bone], i.pos);
    let size = lt.water.z * lt.water.w;
    var o: ObstOut;
    o.pos = vec4<f32>((w.x - lt.water.x) / size * 2.0 - 1.0, 1.0 - (w.z - lt.water.y) / size * 2.0, 0.5, 1.0);
    o.world = w;
    o.flags = r.flags;
    return o;
}
@fragment fn fs_obstacle(i: ObstOut) -> @location(0) vec4<f32> {
    if ((i.flags & 1u) != 0u) { discard; }
    var sy = surface_y(i.world.xz);
    for (var k = 0; k < 4; k++) {
        let a = f32(k) * 1.5707963 + 0.7853982;
        sy = max(sy, surface_y(i.world.xz + vec2<f32>(cos(a), sin(a)) * 4.5));
    }
    let above = i.world.y - sy;
    let low = select(-0.5, 1.5, (i.flags & 64u) != 0u);
    if (sy < -1e8 || above < low || above > 40.0) { discard; }
    return vec4<f32>(1.0 - smoothstep(24.0, 40.0, above), above / 64.0, 0.0, 0.0);
}

// The water meshes seen from above into the surface map (depth = height) and the flow map.
struct SurfOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) xz: vec2<f32>,
    @location(1) v: f32,
    @location(2) @interpolate(flat) slide: f32,
    @location(3) y: f32,
    @location(4) @interpolate(flat) material: f32,
};
@vertex fn vs_surface(i: VIn, @builtin(instance_index) inst: u32) -> SurfOut {
    let r = recs[inst];
    let w = xfm(mats[r.mat], i.pos);
    let size = lt.water.z * lt.water.w;
    let x = (w.x - lt.water.x) / size * 2.0 - 1.0;
    let y = 1.0 - (w.z - lt.water.y) / size * 2.0;
    var o: SurfOut;
    o.pos = vec4<f32>(x, y, clamp(0.5 - (w.y - lt.water2.z) / lt.water2.w, 0.0, 1.0), 1.0);
    o.xz = w.xz;
    o.v = i.uv.y;
    o.slide = r.slide.y;
    o.y = w.y;
    o.material = r.slide.z;
    return o;
}
// A sliding texture is a stream: the texture moves to +v at `slide` (v per second), so the
// water flows along v's gradient in x / z, at slide / |gradient|.
// The flow map's z is the liquid there (`Liquid`; 0: water), w is 1 where there is any.
@fragment fn fs_surface(i: SurfOut) -> @location(0) vec4<f32> {
    let gx = dpdx(i.xz);
    let gy = dpdy(i.xz);
    let vx = dpdx(i.v);
    let vy = dpdy(i.v);
    let det = gx.x * gy.y - gx.y * gy.x;
    if (abs(det) < 1e-9) { return vec4<f32>(0.0, 0.0, i.material, 1.0); }
    // [gx; gy] · g = [vx; vy]
    let g = vec2<f32>(vx * gy.y - vy * gx.y, vy * gx.x - vx * gy.x) / det;
    // A liquid's steep faces (its falls) are no surface: they would lift the pool they land in.
    let yx = dpdx(i.y);
    let yy = dpdy(i.y);
    let gh = vec2<f32>(yx * gy.y - yy * gx.y, yy * gx.x - yx * gy.x) / det;
    if (i.material > 0.5 && dot(gh, gh) > 1.0) { discard; }
    let gg = dot(g, g);
    if (i.slide == 0.0 || gg < 1e-12) { return vec4<f32>(0.0, 0.0, i.material, 1.0); }
    var f = g * (i.slide / gg);
    let sp = length(f);
    if (sp > 150.0) { f *= 150.0 / sp; }
    return vec4<f32>(f, i.material, 1.0);
}

fn shadow(p: vec3<f32>) -> f32 {
    if (lt.sh_up.w < 0.5) { return 1.0; }
    let l = light_space(p);
    let uv = vec2<f32>(l.x * 0.5 + 0.5, 0.5 - l.y * 0.5);
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) || l.z > 1.0) { return 1.0; }
    let ts = lt.sh_dir.w;
    var sum = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            sum += textureSampleCompareLevel(shadow_map, shadow_s, uv + vec2<f32>(f32(x), f32(y)) * ts, l.z - 0.002);
        }
    }
    return sum / 9.0;
}

// RT-BEGIN
// The ray-traced terms; the Path-Traced shader (`crate::rt`) replaces these.
// Sun visibility at p (1 = lit).
fn rt_sun(p: vec3<f32>, n: vec3<f32>, seed: vec2<f32>) -> f32 { return 1.0; }
// Point light k's visibility from p.
fn rt_point(p: vec3<f32>, n: vec3<f32>, lp: vec3<f32>, k: u32, seed: vec2<f32>) -> f32 { return 1.0; }
// Light bounced onto p (rgb) and the ambient's openness (a).
fn rt_ambient(p: vec3<f32>, n: vec3<f32>, seed: vec2<f32>) -> vec4<f32> { return vec4<f32>(0.0, 0.0, 0.0, 1.0); }
// Where a view ray refracted at water surface point sp along r lands, `depth` below (x / z).
fn rt_refract(sp: vec3<f32>, r: vec3<f32>, depth: f32) -> vec2<f32> { return sp.xz + r.xz * (depth / max(-r.y, 0.05)); }
// Extra sunlight focused onto p by the water above (× the sun's colour).
fn rt_caustics(p: vec3<f32>, ndl: f32) -> f32 { return caustics(p, lt.sun_dir.xyz) * 2.6; }
// RT-END

// `face`: the triangle's own normal, facing the camera.
fn light(i: VOut, base: vec3<f32>, face: vec3<f32>) -> vec3<f32> {
    // The art's smoothed normals often lean far from their faces (the book's curled pages), or
    // point out of the side the camera sees (two-sided faces): a normal behind the face is
    // mirrored in its plane, which turns it smoothly (no edge where it crosses the face).
    var n = normalize(i.normal);
    let behind = dot(n, face);
    if (behind < 0.0) { n = normalize(n - face * (2.0 * behind)); }
    let v = normalize(lt.eye.xyz - i.world);
    let seed = i.pos.xy;
    let amb = rt_ambient(i.world, n, seed);
    var l = mix(lt.ground.rgb, lt.sky.rgb, n.y * 0.5 + 0.5) * amb.a + amb.rgb;
    var spec = vec3<f32>(0.0);
    if (lt.sun_dir.w > 0.5) {
        let sh = min(shadow(i.world + n * 2.0), rt_sun(i.world, n, seed));
        let ndl = max(dot(n, lt.sun_dir.xyz), 0.0);
        l += lt.sun_color.rgb * ndl * sh;
        l += lt.sun_color.rgb * rt_caustics(i.world, ndl) * sh;
        let h = normalize(lt.sun_dir.xyz + v);
        spec = lt.sun_color.rgb * pow(max(dot(n, h), 0.0), 48.0) * lt.sun_color.w * sh * step(0.0, ndl);
    }
    if (lt.sun_dir.w < 0.5) {
        l += lt.sky.rgb * caustics(i.world, vec3<f32>(0.0, 1.0, 0.0)) * 0.8;
    }
    let np = u32(lt.ground.w);
    for (var k = 0u; k < np; k++) {
        let p = points[k];
        let d = p.pos.xyz - i.world;
        let dist = length(d);
        let att = clamp(1.0 - dist / p.pos.w, 0.0, 1.0);
        if (att <= 0.0) { continue; }
        // Half-wrapped so lights also reach surfaces seen edge-on (sprites' flames, lava).
        let w = clamp(dot(n, d / max(dist, 0.001)) * 0.75 + 0.25, 0.0, 1.0);
        l += p.color.rgb * att * att * w * rt_point(i.world, n, p.pos.xyz, k, seed);
    }
    // The baked colours are the art's occlusion: dark corners get no highlight either.
    return base * l + spec * dot(i.color.rgb, vec3<f32>(1.0 / 3.0));
}

// Texture offset making what lies under the water waver as the waves bend the view rays (the
// original wobbled river-bed textures for this): where the refracted ray lands minus where it
// would land under flat water, turned into texture space by the surface's uv / world
// derivatives (`d`: dpdx / dpdy of world x / z and of uv).
fn refraction_uv(i: VOut, d: mat4x2<f32>) -> vec2<f32> {
    if (lt.water2.y < 0.5 || (i.flags & 65u) != 0u || material_at(i.world.xz) > 0.5) { return vec2<f32>(0.0); }
    let sy = surface_y(i.world.xz);
    let depth = sy - i.world.y;
    let eye = lt.eye.xyz;
    if (depth < 0.5 || eye.y <= sy) { return vec2<f32>(0.0); }
    let sp = i.world + (eye - i.world) * (depth / (eye.y - i.world.y));
    let v = normalize(i.world - eye);
    let r = refract(v, water_n(sp.xz), 1.0 / 1.33);
    let r0 = refract(v, vec3<f32>(0.0, 1.0, 0.0), 1.0 / 1.33);
    let shift = rt_refract(sp, r, depth) - (sp.xz + r0.xz * (depth / max(-r0.y, 0.05)));
    // world x / z -> uv: [du/dx du/dy] · inverse([dw/dx dw/dy]).
    let det = d[0].x * d[1].y - d[1].x * d[0].y;
    if (abs(det) < 1e-6) { return vec2<f32>(0.0); }
    let inv = mat2x2<f32>(vec2<f32>(d[1].y, -d[0].y), vec2<f32>(-d[1].x, d[0].x)) * (1.0 / det);
    let duv = mat2x2<f32>(d[2], d[3]) * (inv * shift);
    return clamp(duv, vec2<f32>(-0.2), vec2<f32>(0.2)) * smoothstep(0.5, 4.0, depth);
}

// A flowing liquid ([`Class::Flowing`]: the acid rivers): its surface is a height field in
// its texture's coordinates, which slide, so everything on it flows with the texture. Every
// pattern repeats with the texture: the art's faces meet with their coordinates whole
// repeats apart.

// The in-plane world gradient of something whose screen derivatives are dx / dy.
fn plane_grad(sx: vec3<f32>, sy: vec3<f32>, dx: f32, dy: f32) -> vec3<f32> {
    let a = dot(sx, sx);
    let b = dot(sx, sy);
    let c = dot(sy, sy);
    let det = a * c - b * b;
    if (abs(det) < 1e-12) { return vec3<f32>(0.0); }
    return (sx * (dx * c - dy * b) + sy * (dy * a - dx * b)) / det;
}
fn lhash(c: vec2<i32>, salt: u32) -> f32 {
    var h = u32(c.x) * 374761393u + u32(c.y) * 668265263u + salt * 2246822519u;
    h = (h ^ (h >> 13u)) * 1274126177u;
    return f32(h ^ (h >> 16u)) / 4294967295.0;
}
fn lwrap(c: vec2<i32>, per: vec2<i32>) -> vec2<i32> {
    return ((c % per) + per) % per;
}
// Value noise repeating every `per` cells: value (-1..1) and its gradient.
fn lnoise(p: vec2<f32>, per: vec2<i32>, salt: u32) -> vec3<f32> {
    let i = vec2<i32>(floor(p));
    let f = p - floor(p);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let du = 30.0 * f * f * (f * (f - 2.0) + 1.0);
    let a = lhash(lwrap(i, per), salt);
    let b = lhash(lwrap(i + vec2<i32>(1, 0), per), salt);
    let c = lhash(lwrap(i + vec2<i32>(0, 1), per), salt);
    let d = lhash(lwrap(i + vec2<i32>(1, 1), per), salt);
    let k = a - b - c + d;
    return vec3<f32>(a + (b - a) * u.x + (c - a) * u.y + k * u.x * u.y, du * vec2<f32>(b - a + k * u.y, c - a + k * u.x)) * 2.0 - vec3<f32>(1.0, 0.0, 0.0);
}

// A flowing liquid's surface ([`liquid_surface`]).
struct LiquidSurf {
    // The height's gradient in uv (world units per uv).
    g: vec2<f32>,
    // Height (about -1..2: troughs to crests).
    h: f32,
    // The rapids' foaming crests (0..1).
    crest: f32,
    // Bubbles: their domes (0..1) and the rings they leave when they pop.
    dome: f32,
    ring: f32,
};

// The liquid's surface at texture coordinates uv, time t (s); `steep` 0..1 (rapids); `fp`: a
// pixel's footprint in uv (detail finer than a few pixels fades out, no glitter far away).
fn liquid_surface(uv: vec2<f32>, t: f32, steep: f32, fp: f32) -> LiquidSurf {
    var o: LiquidSurf;
    // Streaks along the flow (+v): narrow across, long along it, drifting a little against
    // the flow so the liquid shears; ridged, higher on the rapids.
    let sf1 = vec2<f32>(7.0, 2.0);
    let sf2 = vec2<f32>(13.0, 3.0);
    let s1 = lnoise(uv * sf1 + vec2<f32>(0.0, -t * 0.45), vec2<i32>(7, 2), 1u);
    let s2 = lnoise(uv * sf2 + vec2<f32>(0.0, -t * 0.8), vec2<i32>(13, 3), 5u);
    let sn = s1.x * 0.65 + s2.x * 0.35;
    let sg = s1.yz * sf1 * 0.65 + s2.yz * sf2 * 0.35;
    // Ridged with a softened crease.
    let sq = sqrt(sn * sn + 0.004);
    let ridge = 1.0 - sq;
    let sa = mix(0.8, 1.6, steep);
    o.g = sa * 2.0 * ridge * (-sn / sq) * sg;
    o.h = (ridge * ridge - 0.45) * sa;
    // Ripples, boiling: two layers drifting across each other.
    let r1 = lnoise(uv * 12.0 + vec2<f32>(t * 0.35, t * 0.6), vec2<i32>(12), 2u);
    let r2 = lnoise(uv * 25.0 + vec2<f32>(-t * 0.7, t * 0.2), vec2<i32>(25), 3u);
    let f1 = 1.0 - smoothstep(0.1, 0.3, fp * 12.0);
    let f2 = 1.0 - smoothstep(0.1, 0.3, fp * 25.0);
    o.g = o.g * (1.0 - smoothstep(0.1, 0.3, fp * 7.0)) + r1.yz * 12.0 * 0.5 * f1 + r2.yz * 25.0 * 0.15 * f2;
    o.h += r1.x * 0.5 * f1 + r2.x * 0.15 * f2;
    o.crest = smoothstep(0.8, 0.98, ridge) * steep;
    // Bubbles: some cells (5.6 units across) have one, which swells and pops, leaving a ring
    // that spreads and fades.
    let cells = 9.0;
    let q = uv * cells;
    let ci = lwrap(vec2<i32>(floor(q)), vec2<i32>(9));
    let f = q - floor(q);
    let fb = 1.0 - smoothstep(0.15, 0.4, fp * cells);
    if (lhash(ci, 4u) < 0.3 * (1.0 - steep) && fb > 0.0) {
        let life = 2.4 + lhash(ci, 5u) * 1.6;
        let ph = fract(t / life + lhash(ci, 6u));
        let ctr = vec2<f32>(0.3, 0.3) + vec2<f32>(lhash(ci, 7u), lhash(ci, 8u)) * 0.4;
        let rmax = 0.1 + lhash(ci, 9u) * 0.16;
        let dv = f - ctr;
        let d = length(dv);
        if (ph < 0.85) {
            let r = rmax * sqrt(ph / 0.85);
            if (d < r) {
                // A dome: its slope (into world units per uv, about 0.4 of a sphere's).
                let s = sqrt(max(r * r - d * d, r * r * 0.02));
                o.g += -dv / s * 20.0 * fb;
                o.dome = smoothstep(r, r * 0.8, d) * fb;
            }
        } else {
            let k = (ph - 0.85) / 0.15;
            let rr = rmax * (1.0 + k * 1.8);
            o.ring = exp(-pow((d - rr) / 0.03, 2.0)) * (1.0 - k) * fb;
        }
    }
    return o;
}

// Cell noise repeating every `per` cells: distances to the nearest and the second nearest of
// the cells' points, which wander about with time t.
fn lcells(p: vec2<f32>, per: i32, t: f32, salt: u32) -> vec2<f32> {
    let i = vec2<i32>(floor(p));
    let f = p - floor(p);
    var d1 = 9.0;
    var d2 = 9.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let w = lwrap(i + vec2<i32>(x, y), vec2<i32>(per));
            let a = lhash(w, salt) * 6.2831853;
            let r = lhash(w, salt + 1u);
            let o = vec2<f32>(0.5) + vec2<f32>(cos(a + t * (0.2 + r * 0.3)), sin(a * 1.3 + t * (0.15 + r * 0.25))) * 0.32;
            let d = length(vec2<f32>(f32(x), f32(y)) + o - f);
            if (d < d1) { d2 = d1; d1 = d; } else if (d < d2) { d2 = d; }
        }
    }
    return vec2<f32>(d1, d2);
}

// An acid's bubble in the cell of uv (9 cells a repeat): its dome's height (0..1, while it
// swells) and the ring it leaves when it pops (0..1).
fn acid_bubble(uv: vec2<f32>, t: f32, calm: f32) -> vec2<f32> {
    let q = uv * 9.0;
    let ci = lwrap(vec2<i32>(floor(q)), vec2<i32>(9));
    if (lhash(ci, 4u) > 0.12 + 0.18 * calm) { return vec2<f32>(0.0); }
    let life = 3.2 + lhash(ci, 5u) * 2.4;
    let ph = fract(t / life + lhash(ci, 6u));
    let ctr = vec2<f32>(0.3) + vec2<f32>(lhash(ci, 7u), lhash(ci, 8u)) * 0.4;
    let rmax = 0.16 + lhash(ci, 9u) * 0.14;
    let d = length(q - floor(q) - ctr);
    if (ph < 0.88) {
        // Slow to swell (the acid is thick), then it pops.
        let r = rmax * pow(ph / 0.88, 0.6);
        return vec2<f32>(sqrt(max(1.0 - d * d / max(r * r, 1e-5), 0.0)) * r / rmax, 0.0);
    }
    let k = (ph - 0.88) / 0.12;
    return vec2<f32>(0.0, exp(-pow((d - rmax * (1.0 + k * 1.6)) / 0.035, 2.0)) * (1.0 - k));
}

// An acid's height (world units, about -1.5..1.5) at texture coordinates uv: slow, gloopy
// swells (thick: no fine chop), streaks along the flow where it runs steep, bubbles' domes.
// Periodic in uv with the texture (its faces meet whole repeats apart).
fn acid_height(uv: vec2<f32>, t: f32, steep: f32, calm: f32, fp: f32) -> f32 {
    let w = vec2<f32>(t * 0.05, -t * 0.035);
    var h = lnoise(uv * 3.0 + w, vec2<i32>(3), 11u).x * 1.0 + lnoise(uv * 6.0 - w * 1.7, vec2<i32>(6), 12u).x * 0.4;
    h += lnoise(uv * 12.0 + vec2<f32>(-w.y, w.x) * 3.0, vec2<i32>(12), 13u).x * 0.12 * (1.0 - smoothstep(0.1, 0.3, fp * 12.0));
    if (steep > 0.0) {
        let s = lnoise(uv * vec2<f32>(9.0, 2.0) + vec2<f32>(0.0, -t * 0.6), vec2<i32>(9, 2), 14u).x;
        h += (0.6 - abs(s)) * 1.6 * steep;
    }
    let b = acid_bubble(uv, t, calm);
    return h + b.x * 1.4 * (1.0 - steep) * (1.0 - smoothstep(0.15, 0.4, fp * 9.0));
}

// An acid ([`Liquid::Acid`]: Subterranean's rivers, falls and vats, sludge, toxic glop) with
// texture colour `tex` (the original's, kept as its palette), face normal `face`, screen
// derivatives sx / sy of the world position and duvx / duvy of the texture coordinates:
// a thick, glossy, faintly glowing liquid, emerald in its troughs and lime on its swells,
// a skin of scum drifting on it (with the texture: where the original's slides, it flows),
// bubbles that swell slowly and pop, the simulated waves (the frog's landings, things moving
// through it: slow, short-lived), and fizzing froth where they stir it.
fn liquid(i: VOut, tex: vec3<f32>, face: vec3<f32>, sx: vec3<f32>, sy: vec3<f32>, duvx: vec2<f32>, duvy: vec2<f32>) -> vec3<f32> {
    let gu = plane_grad(sx, sy, duvx.x, duvy.x);
    let gv = plane_grad(sx, sy, duvx.y, duvy.y);
    let steep = smoothstep(0.25, 0.7, 1.0 - abs(face.y));
    // Standing acid (its texture does not slide) bubbles more.
    let calm = select(0.0, 1.0, i.slide == 0.0);
    let fp = max(length(duvx), length(duvy));
    let t = lt.water2.x;
    let uv = i.uv;
    let e = 1.0 / 256.0;
    let h = acid_height(uv, t, steep, calm, fp);
    let g = vec2<f32>(acid_height(uv + vec2<f32>(e, 0.0), t, steep, calm, fp) - h, acid_height(uv + vec2<f32>(0.0, e), t, steep, calm, fp) - h) / e;
    // The height's gradient in world space (uv's gradients × the slopes), plus the waves'.
    let ws = wave_slope(i.world.xz);
    var hw = (g.x * gu + g.y * gv) * ACID_BUMP + vec3<f32>(ws.x, 0.0, ws.y) * 2.5;
    // On the smoothed normal (the faces of the art's coarse meshes tilt this way and that).
    var n0 = normalize(i.normal);
    if (dot(n0, face) < 0.0) { n0 = -n0; }
    let n = normalize(n0 - (hw - n0 * dot(hw, n0)));
    let v = normalize(lt.eye.xyz - i.world);
    // How stirred it is (the simulation): it fizzes and brightens.
    let wh = wave_h(i.world.xz);
    let stir = clamp(abs(wh) * 0.5 + churn(i.world.xz) * 4.0, 0.0, 1.0);
    // Its body: emerald in the troughs, lime on the swells and bubbles, brighter where stirred.
    let b = acid_bubble(uv, t, calm);
    let up = clamp(smoothstep(-1.4, 1.2, h - wh * 0.6) + stir * 0.25, 0.0, 1.0);
    let deep = tex * vec3<f32>(0.16, 0.36, 0.17);
    let bright = min(tex * vec3<f32>(1.35, 1.45, 1.1) + vec3<f32>(0.12, 0.16, 0.0), vec3<f32>(1.0));
    let body = mix(deep, bright, up);
    var j = i;
    j.normal = n;
    var c = light(j, body, face);
    // Its own glow, breathing slowly through the swells.
    c += bright * (ACID_GLOW * (0.75 + 0.25 * sin(t * 1.1 + h * 2.0)) + stir * 0.15);
    // The light about it (the sun, else a light from above in a cave) across its swells.
    let key = select(normalize(vec3<f32>(0.25, 1.0, 0.3)), lt.sun_dir.xyz, lt.sun_dir.w > 0.5);
    c *= clamp(1.0 + (dot(n, key) - dot(n0, key)) * 2.0, 0.65, 1.45);
    // Scum: foamy, yellowish clusters drifting on it (pitted with tiny bubbles), each where
    // the cells' points gather it, some cells clear.
    let fs = 1.0 - smoothstep(0.1, 0.3, fp * 6.0);
    let cl = lcells(uv * 6.0, 6, t * (0.35 + calm * 0.3), 21u);
    let clump = smoothstep(0.42, 0.12, cl.x + lnoise(uv * 18.0, vec2<i32>(18), 24u).x * 0.08) * smoothstep(-0.2, 0.3, lnoise(uv * 3.0 + vec2<f32>(t * 0.02, 0.0), vec2<i32>(3), 22u).x);
    let pits = smoothstep(0.2, 0.6, lnoise(uv * 48.0, vec2<i32>(48), 25u).x) * (1.0 - smoothstep(0.1, 0.3, fp * 48.0));
    let film = clump * (1.0 - pits * 0.5) * fs * (1.0 - steep);
    let scum = min(tex * vec3<f32>(1.1, 1.0, 0.55) + vec3<f32>(0.2, 0.18, 0.02), vec3<f32>(1.0)) * max(light(i, vec3<f32>(1.0), face), vec3<f32>(0.35));
    c = mix(c, scum, film * 0.55);
    // The crests of the simulated waves catch the light: rings where something disturbed it.
    c += bright * clamp(length(ws) * 4.0 - 0.1, 0.0, 1.0) * 0.45;
    // Froth: fizz where it is stirred, the rings of popped bubbles, foam on the rapids.
    let fz = lnoise(uv * 40.0 + vec2<f32>(t * 0.7, -t * 0.5), vec2<i32>(40), 23u).x * (1.0 - smoothstep(0.1, 0.3, fp * 40.0));
    let froth = clamp(smoothstep(0.35, 0.75, fz) * stir + b.y * 0.6 + smoothstep(0.85, 1.4, h) * steep * 0.8, 0.0, 1.0);
    c = mix(c, min(bright * 1.25 + vec3<f32>(0.15), vec3<f32>(1.0)), froth * 0.55);
    // Gloss: highlights of the key light and the point lights, sharp (a wet, smooth surface;
    // duller on the scum), and a reflection (the sky; in a cave, its own glow on the rock).
    let gloss = 1.0 - film * 0.6;
    let fres = 0.04 + 0.96 * pow(1.0 - max(dot(n, v), 0.0), 5.0);
    var spec = mix(bright * 0.3, lt.sky.rgb * 0.5, lt.sun_dir.w) * fres;
    let sh = select(1.0, shadow(i.world + face * 2.0), lt.sun_dir.w > 0.5);
    let kc = select(vec3<f32>(0.8, 0.95, 0.75) * 0.6, lt.sun_color.rgb * 2.0, lt.sun_dir.w > 0.5);
    let nh = max(dot(n, normalize(key + v)), 0.0);
    spec += kc * (pow(nh, 220.0) * 1.6 + pow(nh, 40.0) * 0.08) * sh;
    let np = u32(lt.ground.w);
    for (var k = 0u; k < np; k++) {
        let p = points[k];
        let d = p.pos.xyz - i.world;
        let dist = length(d);
        let att = clamp(1.0 - dist / p.pos.w, 0.0, 1.0);
        let hp = pow(max(dot(n, normalize(d / max(dist, 0.001) + v)), 0.0), 160.0);
        spec += p.color.rgb * att * att * hp * 1.5;
    }
    return c + spec * gloss;
}
const ACID_BUMP: f32 = 0.5;
const ACID_GLOW: f32 = 0.16;
const LIQUID_BUMP: f32 = 0.6;
const LIQUID_GLOW: f32 = 0.08;

// The simulated waves' slope at world x / z (world units of height per unit).
fn wave_slope(xz: vec2<f32>) -> vec2<f32> {
    let e = lt.water.z;
    return vec2<f32>(wave_h(xz + vec2<f32>(e, 0.0)) - wave_h(xz - vec2<f32>(e, 0.0)), wave_h(xz + vec2<f32>(0.0, e)) - wave_h(xz - vec2<f32>(0.0, e))) / (2.0 * e);
}

// Molten rock ([`Liquid::Lava`]): rafts of dark crust, cracked into plates, drifting on the
// glowing melt (the original texture's colours). The crust is laid out in world space (the
// art's lava faces do not all meet whole texture repeats apart: a pattern in its coordinates
// would break at their edges) and carried along the flow map like the water's ripples: two
// copies, each carried for a while and put back, crossfaded, the rafts' outlines morphing
// between them (crust melting and forming). Thick: the simulated waves bulge it slowly and
// tear its crust open; it folds against what stands in it and trails behind it.

// What stands in the liquid at world x / z (the obstacle map, bilinear): solid (0..1), at
// mip level `lod` (each one twice as smooth: 3, 6, 12, 24 units).
fn obstacle_at(xz: vec2<f32>, lod: f32) -> f32 {
    let uv = (xz - lt.water.xy) / (lt.water.z * lt.water.w);
    return textureSampleLevel(obstacles, obstacle_s, uv, lod).r;
}

// A plate's centre in its cell (0..1), wandering slowly (the crust churns).
fn plate_centre(c: vec2<i32>, t: f32, salt: u32) -> vec2<f32> {
    let a = lhash(c, salt) * 6.2831853;
    let w = 0.12 + 0.12 * lhash(c, salt + 1u);
    return vec2<f32>(0.5) + 0.3 * vec2<f32>(sin(a + t * w), cos(a * 1.7 + t * w * 0.8));
}
// Crust plates: the cells of a Voronoi diagram. x: distance to the nearest crack (cell
// units), yz: the direction across it (away from the plate), w: the plate's own random value.
fn plates(p: vec2<f32>, t: f32, salt: u32) -> vec4<f32> {
    let i = vec2<i32>(floor(p));
    let f = p - floor(p);
    var best = 1e9;
    var mo = vec2<i32>(0);
    var mr = vec2<f32>(0.0);
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let o = vec2<i32>(x, y);
            let r = vec2<f32>(o) + plate_centre(i + o, t, salt) - f;
            let d = dot(r, r);
            if (d < best) { best = d; mo = o; mr = r; }
        }
    }
    var e = 1e9;
    var n = vec2<f32>(0.0, 1.0);
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            if (x == 0 && y == 0) { continue; }
            let o = mo + vec2<i32>(x, y);
            let r = vec2<f32>(o) + plate_centre(i + o, t, salt) - f;
            let k = normalize(r - mr);
            let d = dot(0.5 * (mr + r), k);
            if (d < e) { e = d; n = k; }
        }
    }
    return vec4<f32>(e, n, lhash(i + mo, salt + 2u));
}
// The slope of smoothstep(a, b, x).
fn smooth_d(a: f32, b: f32, x: f32) -> f32 {
    let t = clamp((x - a) / (b - a), 0.0, 1.0);
    return 6.0 * t * (1.0 - t) / (b - a);
}
const LAVA_NOWRAP: vec2<i32> = vec2<i32>(65536);

// Where crust has formed at world x / z (one carried copy): a slow temperature field, crust
// where it is high, and its gradient (per unit). Mostly large rafts (about 35 units across,
// warped a little so they do not line up); `fine` (0..1) breaks them into smaller pieces
// (crust torn and crumpled against what stands in the lava).
fn crust_temp(p: vec2<f32>, t: f32, fine: f32) -> vec3<f32> {
    let wq = vec2<f32>(lnoise(p * 0.02, LAVA_NOWRAP, 18u).x, lnoise(p * 0.02 + vec2<f32>(31.7, 11.3), LAVA_NOWRAP, 19u).x) * 0.45;
    let a = lnoise(p * (1.0 / 38.0) + wq + vec2<f32>(t * 0.02, -t * 0.015), LAVA_NOWRAP, 11u);
    let b = lnoise(p * (1.0 / 13.0) + wq * 2.0 + vec2<f32>(-t * 0.03, t * 0.02), LAVA_NOWRAP, 16u);
    let kb = mix(0.12, 0.4, fine);
    let ka = 1.0 - kb;
    return vec3<f32>(a.x * ka + b.x * kb, a.yz * (ka / 38.0) + b.yz * (kb / 13.0));
}
// The cracks across the crust at world x / z (one carried copy): plates about 16 units across
// (their cracks wider by `edge`, near a raft's edge where it breaks up) and fine crazing,
// fading out where a pixel (`fp` units) is too coarse for them. x: how much of a crack (0..1),
// yz: the slope it makes (the crust dips into it), w: the plate's shade.
fn crust_cracks(p: vec2<f32>, t: f32, edge: f32, fp: f32) -> vec4<f32> {
    let sc = 1.0 / 16.0;
    let fz = 1.0 - smoothstep(0.15, 0.4, fp * sc);
    if (fz <= 0.0) { return vec4<f32>(0.0, 0.0, 0.0, 0.5); }
    let pl = plates(p * sc, t, 21u);
    let w = 0.08 + 0.06 * edge;
    var o = vec4<f32>((1.0 - smoothstep(0.0, w, pl.x)) * fz, pl.yz * sc * smooth_d(0.0, w, pl.x) * 0.6 * fz, pl.w);
    let sf = 1.0 / 4.5;
    let fc = 1.0 - smoothstep(0.06, 0.15, fp * sf);
    if (fc > 0.0) {
        let pz = plates(p * sf + vec2<f32>(0.37, 0.61), t * 0.5, 31u);
        let cz = (1.0 - smoothstep(0.0, 0.05, pz.x)) * fc;
        o.x = max(o.x, cz * 0.15);
        o = vec4<f32>(o.x, o.yz + pz.yz * sf * smooth_d(0.0, 0.05, pz.x) * 0.25 * fc, o.w);
    }
    return o;
}

// Lava's flow at world x / z, smooth: the flow map (each art face slides its own way, some
// 30 degrees apart) averaged about points of a 32-unit lattice, blended between the four
// around x / z. The crust is carried far along it: a flow with steps in it would tear it.
fn lava_flow(xz: vec2<f32>) -> vec2<f32> {
    let g = xz / 32.0 - 0.5;
    let i = floor(g);
    let f = smoothstep(vec2<f32>(0.0), vec2<f32>(1.0), g - i);
    var corner: array<vec2<f32>, 4>;
    for (var k = 0; k < 4; k++) {
        let c = cell((i + vec2<f32>(f32(k & 1), f32(k >> 1)) + 0.5) * 32.0);
        corner[k] = (flow_map(c + vec2<f32>(6.0, 6.0)) + flow_map(c + vec2<f32>(-6.0, 6.0)) + flow_map(c + vec2<f32>(6.0, -6.0)) + flow_map(c + vec2<f32>(-6.0, -6.0))) * 0.25;
    }
    return lt.water4.xy + mix(mix(corner[0], corner[1], f.x), mix(corner[2], corner[3], f.x), f.y);
}

// How much a texel of painted crust is plate (dark), not crack (0..1).
fn lava_plate(c: vec3<f32>) -> f32 {
    return 1.0 - smoothstep(0.35, 0.75, max(c.r, max(c.g, c.b)));
}

fn lava(i: VOut, tex: vec3<f32>, face: vec3<f32>, sx: vec3<f32>, sy: vec3<f32>, duvx: vec2<f32>, duvy: vec2<f32>) -> vec3<f32> {
    let tm = lt.water2.x;
    let xz = i.world.xz;
    // (From the smoothed normal: the faces' own would cut the crust along their edges.)
    let steep = smoothstep(0.25, 0.7, 1.0 - abs(normalize(i.normal).y));
    // A pixel's footprint (world units, and in the texture's coordinates).
    let fpw = max(length(sx), length(sy));
    let fp = max(length(duvx), length(duvy));
    // The flow, and what disturbs the surface (splashes, wakes): it bulges and tears open
    // (only on the surface the waves run on: not on a volcano's lava-covered cone in it).
    let fl = lava_flow(xz);
    let speed = length(fl);
    let fd = select(vec2<f32>(0.0), fl / max(speed, 1e-4), speed > 0.5);
    let fast = smoothstep(2.0, 20.0, speed);
    let on = (1.0 - steep) * (1.0 - smoothstep(2.0, 6.0, abs(i.world.y - surface_y(xz))));
    let wh = wave_h(xz) * on;
    let ws = wave_slope(xz) * on;
    let stir = clamp(abs(wh) * 0.4 + length(ws) * 4.0 + churn(xz) * 4.0 * on, 0.0, 1.0);
    // What stands in it: touching it, how near (a field reaching about 10 units out), the way
    // to it, and whether it lies upstream (the wake behind it, some 40 units long).
    let touch = clamp(obstacle_at(xz, 0.5) * 2.0, 0.0, 1.0);
    let near = clamp(obstacle_at(xz, 1.5) * 2.5, 0.0, 1.0);
    let e = 8.0;
    let og = vec2<f32>(obstacle_at(xz + vec2<f32>(e, 0.0), 2.5) - obstacle_at(xz - vec2<f32>(e, 0.0), 2.5),
                       obstacle_at(xz + vec2<f32>(0.0, e), 2.5) - obstacle_at(xz - vec2<f32>(0.0, e), 2.5)) / (2.0 * e);
    let ogn = og / max(length(og), 1e-4);
    var pile = near * 0.3;
    var wake = 0.0;
    if (speed > 0.5) {
        pile += near * smoothstep(-0.2, 0.7, dot(fd, ogn)) * fast * 0.7;
        wake = clamp(max(obstacle_at(xz - fd * 14.0, 2.0), obstacle_at(xz - fd * 34.0, 3.0) * 0.8) * 2.0 * (1.0 - near) * fast, 0.0, 1.0);
    }
    // Heat: steep faces, fast flow, stirring and the wake melt the crust away.
    let open = clamp(steep * 0.85 + fast * 0.1 + stir * 0.9 + wake * 0.4, 0.0, 1.0);
    // The crust, carried by the flow; around what stands in it, it slips by to the side.
    let q = xz + (fd * dot(fd, og) - og) * 50.0 * fast;
    var pa = vec2<f32>(0.0);
    var pb = vec2<f32>(0.0);
    var wa = 1.0;
    if (speed > 0.5) {
        let ph = fract(tm / LAVA_PERIOD);
        pa = fl * (ph * LAVA_PERIOD);
        pb = fl * (fract(ph + 0.5) * LAVA_PERIOD) - vec2<f32>(57.0, 23.0);
        wa = 1.0 - abs(1.0 - 2.0 * ph);
    }
    // (Blended keeping the field's contrast: a plain mix would thin the crust mid-way.)
    let fine = clamp(pile + wake, 0.0, 1.0);
    var temp = crust_temp(q - pa, tm, fine);
    if (wa < 1.0) { temp = mix(crust_temp(q - pb, tm, fine), temp, wa) / sqrt(wa * wa + (1.0 - wa) * (1.0 - wa)); }
    let d = temp.x - mix(0.2, 0.9, open);
    // A raft's edge is sharp, but never finer than a pixel or two; the melt glows brightest
    // right at it (thin, hot crust just forming), bleeding a few units out.
    let tl = max(length(temp.yz), 1e-4);
    let pc = i.crust;
    let cover = smoothstep(0.0, max(0.04, fpw * tl * 1.5), d) * (1.0 - smoothstep(0.0, 0.6, pc));
    var crack = exp(-max(-d, 0.0) / tl / max(3.5, fpw * 2.0)) * (1.0 - cover) * smoothstep(0.9, 0.6, open) * (1.0 - pc);
    // Rafts stand a little proud, bevelled at their edges, cracked into plates.
    var h = smoothstep(0.0, 0.18, d) * 1.2;
    var g = temp.yz * smooth_d(0.0, 0.18, d) * 2.5;
    let bevel = (1.0 - smoothstep(0.0, 0.2, d)) * cover;
    var shade = 0.5;
    var lines = 0.0;
    if (cover > 0.0) {
        let edge = 1.0 - smoothstep(0.0, 0.3, d);
        var ck = crust_cracks(q - pa, tm, edge, fpw);
        if (wa < 1.0) {
            let cb = crust_cracks(q - pb, tm, edge, fpw);
            ck = vec4<f32>(mix(cb.xyz, ck.xyz, wa), select(cb.w, ck.w, wa > 0.5));
        }
        lines = ck.x;
        h -= lines * 0.6 * cover;
        g -= ck.yz * cover;
        shade = ck.w;
    }
    var crust = cover * (1.0 - lines);
    crack = max(crack, lines * cover);
    // Painted crust: the texture's dark plates stand proud of its glowing cracks (the slope
    // from two more texels a pixel or so away in x and z); stirring melts them.
    let gu = plane_grad(sx, sy, duvx.x, duvy.x);
    let gv = plane_grad(sx, sy, duvx.y, duvy.y);
    if (pc > 0.0) {
        let dl = max(fpw * 1.5, 0.75);
        let l0 = lava_plate(textureSampleGrad(t, s, i.uv, duvx, duvy).rgb);
        let lx = lava_plate(textureSampleGrad(t, s, i.uv + vec2<f32>(gu.x, gv.x) * dl, duvx, duvy).rgb);
        let lz = lava_plate(textureSampleGrad(t, s, i.uv + vec2<f32>(gu.z, gv.z) * dl, duvx, duvy).rgb);
        let plate = l0 * pc * (1.0 - stir);
        h += plate * 1.5;
        g += vec2<f32>(lx - l0, lz - l0) / dl * 1.5 * pc;
        crust = max(crust, plate);
    }
    // The melt's slow rolls along the flow (in the texture's coordinates, with it): brighter
    // and dimmer bands, a gentle swell.
    let rf = vec2<f32>(4.0, 2.0);
    let fr = 1.0 - smoothstep(0.15, 0.4, fp * 8.0);
    let r1 = lnoise(i.uv * rf + vec2<f32>(tm * 0.05, -tm * 0.25), vec2<i32>(4, 2), 12u);
    let r2 = lnoise(i.uv * rf * 2.0 + vec2<f32>(-tm * 0.1, -tm * 0.4), vec2<i32>(8, 4), 13u) * fr;
    let roll = 1.0 + r1.x * 0.16 + r2.x * 0.08;
    let rg = (r1.yz * rf * 0.6 + r2.yz * rf * 2.0 * 0.25) * (1.0 - cover);
    // Against what stands in it the crust piles up in folds that creep in (glowing between
    // them) into a collar of crust under it, with a hot seam where the melt touches it;
    // behind it the crust is torn away, leaving a glowing trail.
    let folds = 0.5 + 0.5 * sin(near * 9.0 - tm * 0.9);
    let fold = pile * smoothstep(0.2, 0.7, folds);
    let under = smoothstep(0.6, 1.0, touch);
    let seam = touch * (1.0 - touch) * 4.0;
    crust = max(crust, max(fold * 0.9, under * 0.85));
    h = max(h, max(fold, under) * 1.5);
    crack = max(crack * (1.0 - pile * 0.5), max(pile * (1.0 - smoothstep(0.0, 0.25, folds)) * 0.7, seam * 0.8));
    crust *= 1.0 - wake * 0.7;
    crack = max(crack, wake * 0.35);
    // The normal: bumped crust and melt on the smoothed normal, the slow bulges of the
    // simulated surface and the folds.
    let hw = vec3<f32>(g.x, 0.0, g.y) * LAVA_BUMP + (rg.x * gu + rg.y * gv) * LAVA_BUMP
        + vec3<f32>(ws.x, 0.0, ws.y) * 0.8 - vec3<f32>(og.x, 0.0, og.y) * pile * 4.0;
    var n0 = normalize(i.normal);
    if (dot(n0, face) < 0.0) { n0 = -n0; }
    let n = normalize(n0 - (hw - n0 * dot(hw, n0)));
    let v = normalize(lt.eye.xyz - i.world);
    // The melt: the original's colours, brighter in its rolls, swelling and stirred parts and
    // its slow pulse; yellow-hot at the crust's edges and in its cracks.
    let pulse = 1.0 + 0.06 * sin(tm * 1.7 + lnoise(xz * 0.01, LAVA_NOWRAP, 15u).x * 4.0);
    let heat = clamp(roll * pulse + max(wh, 0.0) * 0.1 + stir * 0.4 + steep * 0.1, 0.6, 1.7);
    let melt = min(tex * vec3<f32>(1.2, 1.05, 0.85) * heat, vec3<f32>(1.0));
    // Incandescent: orange where it is faint, yellow at its hottest (never white).
    let core = vec3<f32>(1.0, 0.8, 0.22);
    let glow = clamp(crack * 0.8 + stir * 0.45, 0.0, 0.9);
    var c = mix(melt, mix(vec3<f32>(1.0, 0.5, 0.06), core, glow), glow);
    // A glossy skin: the light from above (in a cave), else the sun, across the bumps.
    let key = select(normalize(vec3<f32>(0.25, 1.0, 0.3)), lt.sun_dir.xyz, lt.sun_dir.w > 0.5);
    let gloss = pow(max(dot(n, normalize(key + v)), 0.0), 60.0) * 0.3;
    let fres = pow(1.0 - max(dot(n, v), 0.0), 4.0);
    c = c * clamp(1.0 + (dot(n, key) - dot(n0, key)) * 1.5, 0.75, 1.3) + core * gloss + melt * fres * 0.25;
    // The crust: dark cooled rock on top, lit by the lights about it (the lava's own among
    // them); its bevel catches the melt's glow from below, warm, not pale.
    let thin = 1.0 - smoothstep(0.0, 0.8, h);
    var rock = mix(vec3<f32>(0.2, 0.065, 0.05), tex * vec3<f32>(0.7, 0.26, 0.1), thin * 0.6) * (0.85 + 0.3 * shade);
    rock = mix(rock, tex * 0.85, pc);
    var j = i;
    j.normal = n;
    let under_glow = vec3<f32>(0.9, 0.32, 0.05) * (bevel * 0.8 + crack * 0.3) * (1.0 - pc);
    let rc = light(j, rock, face) * 0.7 + rock * 0.3 + under_glow;
    return mix(c, rc, crust);
}
const LAVA_BUMP: f32 = 0.5;
// How long each carried copy of the crust is carried before it is put back (s).
const LAVA_PERIOD: f32 = 4.0;

// Waterfalls and the streams feeding them: like a flowing liquid's, their surface is made in
// the texture's coordinates (+v downstream), which slide with the original animation, every
// pattern repeating with the texture.
struct FallSurf {
    // The height's gradient in uv (world units per uv) and the height (about -1..1).
    g: vec2<f32>,
    h: f32,
    // White water (0..1).
    foam: f32,
};

// Pattern offset along v (`per` repeats per v) moving a layer at `k` times the texture's speed
// `sp` relative to it (k > 0: faster; `slide_object` takes `sp` off v every frame).
fn fall_shift(t: f32, sp: f32, k: f32, per: f32) -> f32 {
    return -t * sp * k * per;
}
// How much of a pattern repeating `f` times per uv shows at footprint `fp` (finer than a few
// pixels: none).
fn fall_fade(fp: f32, f: f32) -> f32 {
    return 1.0 - smoothstep(0.08, 0.35, fp * f);
}

// The water at uv, time t (s): `steep` 0..1 (the fall), `lip` 0..1 (going over the edge:
// glassy), `calm` 0..1 (no white water yet), `low` 0..1 (near the pool: aerated), `fp` a
// pixel's footprint in uv, `sp` the texture's speed (v per second).
fn fall_surface(uv: vec2<f32>, t: f32, steep: f32, lip: f32, calm: f32, low: f32, fp: f32, sp: f32) -> FallSurf {
    var o: FallSurf;
    // The sheets sway sideways a little as they fall.
    let w = lnoise(vec2<f32>(uv.x * 3.0, uv.y * 2.0 + fall_shift(t, sp, -0.3, 2.0)), vec2<i32>(3, 2), 11u);
    let q = vec2<f32>(uv.x + w.x * 0.03 * steep, uv.y);
    // The fall: sheets, long down it and narrow across, ridged; the inner ones slower than
    // the outer (they slip past each other).
    let s1 = lnoise(vec2<f32>(q.x * 16.0, q.y * 2.0 + fall_shift(t, sp, -0.25, 2.0)), vec2<i32>(16, 2), 12u);
    let s2 = lnoise(vec2<f32>(q.x * 29.0, q.y * 3.0 + fall_shift(t, sp, 0.35, 3.0)), vec2<i32>(29, 3), 13u);
    let e1 = sqrt(s1.x * s1.x + 0.01);
    let e2 = sqrt(s2.x * s2.x + 0.01);
    let fd1 = fall_fade(fp, 16.0);
    let fd2 = fall_fade(fp, 29.0);
    let hs = (1.0 - e1) * 0.7 * fd1 + (1.0 - e2) * 0.45 * fd2;
    let gs = -(s1.x / e1) * s1.yz * vec2<f32>(16.0, 2.0) * 0.7 * fd1 - (s2.x / e2) * s2.yz * vec2<f32>(29.0, 3.0) * 0.45 * fd2;
    // The stream: streaks along the flow and small ripples riding it.
    let r1 = lnoise(vec2<f32>(q.x * 9.0, q.y * 4.0 + fall_shift(t, sp, -0.15, 4.0)), vec2<i32>(9, 4), 14u);
    let r2 = lnoise(uv * 14.0 + vec2<f32>(t * 0.4, fall_shift(t, sp, 0.1, 14.0)), vec2<i32>(14), 15u);
    let r3 = lnoise(uv * 27.0 + vec2<f32>(-t * 0.6, fall_shift(t, sp, -0.1, 27.0) + t * 0.3), vec2<i32>(27), 16u);
    let fr1 = fall_fade(fp, 9.0);
    let fr2 = fall_fade(fp, 14.0);
    let fr3 = fall_fade(fp, 27.0);
    let hf = r1.x * 0.5 * fr1 + r2.x * 0.3 * fr2 + r3.x * 0.15 * fr3;
    let gf = r1.yz * vec2<f32>(9.0, 4.0) * 0.5 * fr1 + r2.yz * 14.0 * 0.3 * fr2 + r3.yz * 27.0 * 0.15 * fr3;
    // Glassy where it goes over the edge.
    let smooth_ = 1.0 - lip * 0.85;
    o.h = mix(hf, hs, steep) * smooth_;
    o.g = mix(gf, gs, steep) * smooth_;
    // White water: streaks torn along the flow, faster than the water on the fall, coming
    // and going in patches (broken edges); flecks on the stream, a lot near the pool, none on
    // the lip.
    // (Speeds never depend on the slope: the faces differ a little, and over time any
    // difference would tear the patterns apart at their edges.)
    let fs1 = lnoise(vec2<f32>(q.x * 23.0, q.y * 4.0 + fall_shift(t, sp, 0.6, 4.0)), vec2<i32>(23, 4), 17u);
    let fs2 = lnoise(vec2<f32>(q.x * 41.0, q.y * 6.0 + fall_shift(t, sp, 0.9, 6.0)), vec2<i32>(41, 6), 20u);
    let fl = lnoise(vec2<f32>(q.x * 17.0, q.y * 7.0), vec2<i32>(17, 7), 23u);
    let patches = lnoise(vec2<f32>(uv.x * 5.0, uv.y * 3.0 + fall_shift(t, sp, 0.45, 3.0)), vec2<i32>(5, 3), 18u).x;
    let streak = mix(fl.x * fall_fade(fp, 17.0), fs1.x * 0.6 * fall_fade(fp, 23.0) + fs2.x * 0.4 * fall_fade(fp, 41.0), steep);
    // The pool's foot boils: blobs welling up against the fall.
    let b1 = lnoise(vec2<f32>(uv.x * 19.0 + sin(t * 0.7) * 0.3, uv.y * 9.0 + t * 0.8), vec2<i32>(19, 9), 21u).x;
    let th = mix(0.55, 0.3, steep) - patches * 0.35 * steep - low * 0.55 + calm * 0.9;
    o.foam = smoothstep(th, th + 0.35, streak + b1 * 0.35 * low) * fall_fade(fp, 10.0);
    // The outer sheets' crests catch the air.
    o.foam = max(o.foam, smoothstep(0.82, 0.97, 1.0 - e2) * steep * (1.0 - calm) * 0.5 * fd2);
    return o;
}

// The pool's surface below a fall at p (seen in front of it: along `face`; water, not the
// stream at its top), none: -1e9.
fn pool_y(p: vec3<f32>, face: vec3<f32>) -> f32 {
    var y = -1e9;
    for (var k = 0; k < 3; k++) {
        let q = p.xz + face.xz * (f32(k) * 6.0);
        let s = surface_y(q);
        if (s < p.y + 4.0 && material_at(q) < 0.5) { y = max(y, s); }
    }
    return y;
}

// Water drawn opaque whose texture slides ([`Liquid::Fall`]): a stream, and where it is steep
// its waterfall. The lip where it goes over the edge is glassy (its normal rolled round the
// edge: a highlight rolls over it), the fall in sheets slipping past each other
// with white water torn along them, aerated and misty near its pool; the stream rippled and
// streaked along its flow. Lit through the bumped normal like the liquids.
fn stream(i: VOut, tex: vec3<f32>, face: vec3<f32>, sx: vec3<f32>, sy: vec3<f32>, duvx: vec2<f32>, duvy: vec2<f32>) -> vec3<f32> {
    let gu = plane_grad(sx, sy, duvx.x, duvy.x);
    let gv = plane_grad(sx, sy, duvx.y, duvy.y);
    let steep = smoothstep(0.25, 0.7, 1.0 - abs(face.y));
    // A pixel's footprint in uv from its distance (its own derivatives change from face to
    // face, and the detail they fade would show the faces).
    let fp = max(i.vz - 10.0, 1.0) * 2.0 / (cam.proj.x * cam.size.x) * max(length(gu), length(gv));
    // The edge it goes over (`VOut::top`): the top of the fall and the stream just before it.
    // The art's corner there is sharp; the water's normal rounds it, rolling from the
    // stream's up to the fall's out (downstream: along v's gradient).
    let over = clamp((i.world.y - i.top) / 10.0, -2.5, 1.0);
    let lip = (1.0 - smoothstep(0.0, 1.0, abs(over))) * step(-1e8, i.top);
    var out = gv - face * dot(gv, face);
    out = select(face, normalize(out), dot(out, out) > 1e-12);
    // How far round the curve this is (0: the stream's, 1: the fall's).
    let roll = smoothstep(-1.0, 1.0, -over);
    let sky_n = normalize(vec3<f32>(face.x * 0.15, 1.0, face.z * 0.15));
    let fall_n = select(face, normalize(vec3<f32>(out.x, 0.0, out.z) + face * 0.5), steep < 0.5);
    // The art's smoothed normals lean up on the fall: mostly its faces'.
    var n0 = normalize(i.normal);
    if (dot(n0, face) < 0.0) { n0 = -n0; }
    let level = vec3<f32>(face.x, 0.0, face.z);
    n0 = normalize(mix(n0, select(face, normalize(level), dot(level, level) > 1e-6), 0.8 * steep));
    n0 = normalize(mix(n0, normalize(mix(sky_n, fall_n, roll)), lip));
    let pool = pool_y(i.world, face);
    let above = select(1e4, i.world.y - pool, pool > -1e8);
    let low = (1.0 - smoothstep(2.0, 45.0, above)) * steep;
    let t = lt.water2.x;
    // The water stays whole a little way down the fall, breaking up at different heights.
    let breakup = lnoise(vec2<f32>(i.uv.x * 7.0, t * 0.3), vec2<i32>(7, 65536), 22u).x;
    let calm = max(lip, (1.0 - smoothstep(0.0, 1.0, (i.top - i.world.y) / 28.0 + breakup * 0.35)) * step(-1e8, i.top) * steep);
    let sf = fall_surface(i.uv, t, steep, lip, calm, low, fp, i.slide);
    let hw = (sf.g.x * gu + sf.g.y * gv) * FALL_BUMP;
    let n = normalize(n0 - (hw - n0 * dot(hw, n0)));
    let v = normalize(lt.eye.xyz - i.world);
    // The body: the original's colours, deeper between the sheets, clear and deep on the lip,
    // paler where it is aerated.
    let up = smoothstep(-0.6, 1.0, sf.h);
    var body = tex * mix(vec3<f32>(0.62, 0.78, 0.92), vec3<f32>(1.05, 1.08, 1.1), up);
    body = mix(body, tex * vec3<f32>(0.55, 0.8, 1.0), lip * 0.5);
    let pale = vec3<f32>(0.86, 0.95, 1.0);
    body = mix(body, mix(tex, pale, 0.6), low * 0.45);
    var j = i;
    j.normal = n;
    var c = light(j, body, face);
    // A little light scattered through the falling water (it is never dark).
    c += body * 0.1 * steep;
    // The light about it across the sheets and ripples.
    let key = select(normalize(vec3<f32>(0.25, 1.0, 0.3)), lt.sun_dir.xyz, lt.sun_dir.w > 0.5);
    c *= clamp(1.0 + (dot(n, key) - dot(n0, key)) * 2.0, 0.6, 1.5);
    // Reflections: the sky, more on the glassy lip; the sun's highlight, broad and bright on
    // the lip (rolling over its curve), sparkling on the ripples.
    let fres = 0.04 + 0.96 * pow(1.0 - max(dot(n, v), 0.0), 5.0);
    var spec = sky_along(reflect(-v, n)) * fres * (0.5 + lip * 0.6);
    let sh = select(1.0, shadow(i.world + face * 2.0), lt.sun_dir.w > 0.5);
    let kc = select(vec3<f32>(0.6, 0.75, 0.9) * 0.5, lt.sun_color.rgb * 1.6, lt.sun_dir.w > 0.5);
    let nh = max(dot(n, normalize(key + v)), 0.0);
    spec += kc * (pow(nh, 160.0) * (1.0 + lip) + pow(nh, 24.0) * lip * 0.35) * sh;
    // A highlight along the lip from the sky above it.
    spec += lt.sky.rgb * pow(max(n0.y, 0.0), 3.0) * lip * 0.35;
    // White water, lit (never quite white: it keeps the water's hue in the shade).
    var amb = mix(lt.ground.rgb, lt.sky.rgb, n.y * 0.5 + 0.5);
    if (lt.sun_dir.w > 0.5) { amb += lt.sun_color.rgb * (max(dot(n0, key), 0.0) * 0.6 + 0.25) * sh; }
    let froth = pale * min(amb * 1.05, vec3<f32>(1.25));
    let fa = clamp(sf.foam * mix(0.65, 0.9, steep) + low * 0.25, 0.0, 1.0);
    c = mix(c + spec, froth, fa);
    // Mist about the pool, drifting up the fall's foot.
    let mist = lnoise(vec2<f32>(i.uv.x * 6.0 + t * 0.15, i.uv.y * 3.0 + fall_shift(t, i.slide, -1.3, 3.0)), vec2<i32>(6, 3), 19u).x;
    c = mix(c, froth, low * low * (0.35 + 0.2 * mist));
    return c;
}
const FALL_BUMP: f32 = 0.35;

// A waterfall the water pass draws (its steep faces, translucent: Garden's) with the falls'
// look ([`fall_surface`]): its texture in sheets, white water torn along them, aerated near
// its pool. Elsewhere `base` as it is.
fn fall_sheet(i: VOut, base: vec4<f32>) -> vec4<f32> {
    let sx = dpdx(i.world);
    let sy = dpdy(i.world);
    let gu = plane_grad(sx, sy, dpdx(i.uv.x), dpdy(i.uv.x));
    let gv = plane_grad(sx, sy, dpdx(i.uv.y), dpdy(i.uv.y));
    var face = cross(sx, sy);
    if (dot(face, face) < 1e-12) { return base; }
    face = normalize(face);
    if (dot(face, lt.eye.xyz - i.world) < 0.0) { face = -face; }
    let steep = smoothstep(0.25, 0.7, 1.0 - abs(face.y));
    if (steep <= 0.0 || i.slide == 0.0) { return base; }
    let fp = max(i.vz - 10.0, 1.0) * 2.0 / (cam.proj.x * cam.size.x) * max(length(gu), length(gv));
    let pool = pool_y(i.world, face);
    let above = select(1e4, i.world.y - pool, pool > -1e8);
    let low = (1.0 - smoothstep(2.0, 45.0, above)) * steep;
    let sf = fall_surface(i.uv, lt.water2.x, steep, 0.0, 0.0, low, fp, i.slide);
    let up = smoothstep(-0.6, 1.0, sf.h);
    let lit = mix(lt.ground.rgb, lt.sky.rgb, 0.7) + lt.sun_color.rgb * 0.5 * lt.sun_dir.w;
    let froth = vec3<f32>(0.86, 0.95, 1.0) * min(lit * 1.1, vec3<f32>(1.2));
    let fa = clamp(sf.foam * 0.85 + low * 0.3, 0.0, 1.0) * steep;
    let rgb = mix(base.rgb * mix(1.0, mix(0.7, 1.25, up), steep), froth, fa);
    return vec4<f32>(rgb, mix(base.a, max(base.a * mix(0.8, 1.3, up), 0.75), fa));
}

fn shade(i: VOut) -> vec4<f32> {
    // Derivatives first (uniform control flow).
    let dd = mat4x2<f32>(dpdx(i.world.xz), dpdy(i.world.xz), dpdx(i.uv), dpdy(i.uv));
    let sx = dpdx(i.world);
    let sy = dpdy(i.world);
    var face = cross(sx, sy);
    face = select(normalize(i.normal), normalize(face), dot(face, face) > 1e-12);
    if (dot(face, lt.eye.xyz - i.world) < 0.0) { face = -face; }
    // The far plane (`PCPrepareObject` drops faces past it; here per pixel, no popping).
    if ((i.flags & 1u) == 0u && i.vz - 10.0 >= cam.fog.w) { discard; }
    var c = textureSampleGrad(t, s, i.uv + refraction_uv(i, dd), dd[2], dd[3]) * i.color;
    if (floor(c.a * 255.0 + 0.5) <= cam.depth.y) { discard; }
    if (lt.sky.w > 0.5) {
        if ((i.flags & 64u) != 0u) {
            let m = u32(round(i.mat));
            if (m == 2u) {
                c = vec4<f32>(lava(i, c.rgb, face, sx, sy, dd[2], dd[3]), c.a);
            } else if (m == 3u) {
                c = vec4<f32>(stream(i, c.rgb, face, sx, sy, dd[2], dd[3]), c.a);
            } else {
                c = vec4<f32>(liquid(i, c.rgb, face, sx, sy, dd[2], dd[3]), c.a);
            }
        } else if (EMISSIVE) {
            c = vec4<f32>(min(c.rgb * 1.15, vec3<f32>(1.0)), c.a);
        } else if ((i.flags & 1u) == 0u) {
            c = vec4<f32>(light(i, c.rgb, face), c.a);
        }
    }
    if (cam.fog.x > 0.5) { c = vec4<f32>(mix(cam.fog_color.rgb, c.rgb, i.fog), c.a); }
    return c;
}

@fragment fn fs(i: VOut) -> @location(0) vec4<f32> {
    return shade(i);
}

struct FOut { @location(0) c: vec4<f32>, @location(1) mv: vec2<f32>, @location(2) d: f32 };
@fragment fn fs_mrt(i: VOut) -> FOut {
    var o: FOut;
    o.c = shade(i);
    let half = vec2<f32>(cam.size.x * 0.5, -cam.size.y * 0.5);
    o.mv = (i.prev.xy / i.prev.w - i.cur.xy / i.cur.w) * half;
    o.d = clamp(cam.depth.x / (i.vz - 10.0), 0.0, 1.0);
    return o;
}
"#;

/// Waterfall landing points passed to the generated surface and the mist (`Falls`).
const MAX_FALL_FEET: usize = 32;
const FALLS_SIZE: u64 = 16 + 32 * MAX_FALL_FEET as u64;
/// Mist puffs and spray drops over each landing point (`vs_mist`).
const MIST_PER_FOOT: u32 = 18;

/// Size of `Cam` and `Lights`.
const CAM_SIZE: u64 = 64 * 2 + 16 * 6;
const LIGHTS_SIZE: u64 = 16 * 14;
/// Point lights drawn at once (the nearest to the camera's target).
pub const MAX_POINT_LIGHTS: usize = 32;
/// Shadow map size.
const SHADOW_SIZE: u32 = 2048;

/// A water draw: record, mesh, vertex range, texture, wrap, additive, steep (a waterfall).
type WaterDraw = (u32, Key, core::ops::Range<u32>, TexHandle, bool, bool, bool);

/// The GPU side: uploaded meshes, per-frame buffers, pipelines.
pub(crate) struct Gpu {
    meshes: HashMap<Key, (wgpu::Buffer, Vec<Group>)>,
    shader: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    store_layout: wgpu::BindGroupLayout,
    cam: wgpu::Buffer,
    cam_bg: wgpu::BindGroup,
    lights: wgpu::Buffer,
    points: wgpu::Buffer,
    lights_bg: wgpu::BindGroup,
    shadow_ubo_bg: wgpu::BindGroup,
    shadow_view: wgpu::TextureView,
    shadow_pipeline: wgpu::RenderPipeline,
    /// The shadow pass for cut-out textures (their see-through pixels cast no shadow).
    shadow_cut_pipeline: wgpu::RenderPipeline,
    /// recs, mats, prev, and the bind group over them.
    store: Option<(wgpu::Buffer, wgpu::Buffer, wgpu::Buffer, wgpu::BindGroup)>,
    pipelines: HashMap<(wgpu::TextureFormat, u32, bool, bool, bool), wgpu::RenderPipeline>,
    /// This frame's draw list: (record, mesh key, vertex range, texture, wrap, emissive,
    /// cut-out: the texture has see-through pixels).
    list: Vec<(u32, Key, core::ops::Range<u32>, TexHandle, bool, bool, bool)>,
    /// Emissive and mean colour per texture CRC.
    tex_info: HashMap<u32, TexInfo>,
    /// The shadow map needs drawing this frame.
    shadows: bool,
    /// Subdivided water meshes, this frame's water draws (record, mesh, vertex range,
    /// texture, wrap, additive), their pipelines and the surface map's.
    water_meshes: HashMap<Key, (wgpu::Buffer, Vec<Group>)>,
    water_list: Vec<WaterDraw>,
    /// This frame's opaque liquid draws ([`Class::Liquid`]; in [`Self::list`] too), for the
    /// surface map: the waves run on them.
    liquid_list: Vec<(u32, Key, core::ops::Range<u32>)>,
    water_pipelines: HashMap<(wgpu::TextureFormat, u32, bool, bool), wgpu::RenderPipeline>,
    surface_pipeline: wgpu::RenderPipeline,
    bed_pipeline: wgpu::RenderPipeline,
    /// The obstacle map's ([`crate::water::WaterSim::obstacle_view`]).
    obstacle_pipeline: wgpu::RenderPipeline,
    bed_bg: wgpu::BindGroup,
    /// The generated water surface this frame: its texture (wrapping) and tint.
    grid: Option<(TexHandle, [f32; 3])>,
    grid_pipelines: HashMap<(wgpu::TextureFormat, u32, bool), wgpu::RenderPipeline>,
    /// The generated surface's group 0: the camera plus a copy of the frame drawn so far
    /// (colour and depth: reflections and what the water lets through) and whether there is
    /// one this frame.
    grid_layout: wgpu::PipelineLayout,
    grid_cam_layout: wgpu::BindGroupLayout,
    grid_cam_bg: wgpu::BindGroup,
    scene_flag: wgpu::Buffer,
    scene_copy: (wgpu::Texture, wgpu::Texture),
    linear_s: wgpu::Sampler,
    obstacle_s: wgpu::Sampler,
    /// Where the waterfalls land this frame (`Falls` in the shader: the generated surface's
    /// foam under them, the mist and spray over it) and how many.
    falls: wgpu::Buffer,
    fall_count: u32,
    mist_pipelines: HashMap<(wgpu::TextureFormat, u32, bool), wgpu::RenderPipeline>,
    sim: crate::water::WaterSim,
    /// Path-Traced (devices with ray queries), and whether this frame uses it.
    rtx: Option<RtParts>,
    rt_on: bool,
    /// Frames prepared (the ray-traced noise changes every frame when `temporal`: a jittered
    /// temporal upscaler averages it).
    frame: u32,
    temporal: bool,
    shadow_s: wgpu::Sampler,
    /// Mesh vertex buffer usage (`BLAS_INPUT` with ray queries).
    vb_usage: wgpu::BufferUsages,
}

/// The ray-traced variant's state ([`crate::rt`]).
struct RtParts {
    rt: crate::rt::Rt,
    shader: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    lights_layout: wgpu::BindGroupLayout,
    /// Group 3 with the scene's acceleration structure (remade when it is replaced).
    bg: Option<wgpu::BindGroup>,
}

/// The text of `src` between the lines `begin` and `end`.
fn block<'a>(src: &'a str, begin: &str, end: &str) -> &'a str {
    let a = src.find(begin).expect(begin) + begin.len();
    let b = src[a..].find(end).expect(end) + a;
    &src[a..b]
}

/// `src` with the text from `begin` to `end` (markers included) replaced by `with`.
fn replace_block(src: &str, begin: &str, end: &str, with: &str) -> String {
    let a = src.find(begin).expect(begin);
    let b = src[a..].find(end).expect(end) + a + end.len();
    format!("{}{}{}", &src[..a], with, &src[b..])
}

/// The lights of glowing group `g` drawn with matrix `m`: at its spots, the texture's colour
/// pushed warm, reaching further for larger surfaces.
fn point_lights(m: &[[f32; 4]; 4], g: &Group, color: [f32; 3], out: &mut Vec<PointLight>) {
    for &(c, ext) in &g.spots {
        // Above the surface (lava lakes light the rocks standing in them).
        let p = crate::math::mtx_xfm(m, c);
        let pos = [p[0], p[1] + 30.0 + ext * 0.15, p[2]];
        out.push(PointLight { pos, color: [color[0] * 1.7, color[1] * 1.4, color[2] * 1.1], radius: 140.0 + ext * 1.5 });
    }
}

/// How bright a flowing liquid's lights are against a glowing texture's ([`point_lights`]).
const LIQUID_LIGHT: f32 = 0.3;

/// What the lights need to know of a texture.
#[derive(Clone, Copy, Debug)]
struct TexInfo {
    /// Glows ([`is_emissive`]).
    emissive: bool,
    /// Mean colour.
    color: [f32; 3],
    /// No cut-out pixels (ray-traced shadows treat it as solid).
    opaque: bool,
}

/// [`TexInfo`] of texture `crc` (`None`: not loaded), cached in `info`.
fn tex_info(info: &mut HashMap<u32, TexInfo>, textures: &TextureRegistry, crc: u32) -> Option<TexInfo> {
    if let Some(i) = info.get(&crc) {
        return Some(*i);
    }
    let e = textures.entry(crc)?;
    let (color, opaque) = textures.images.get(e.image.0 as usize).map_or(([1.0; 3], true), |img| mean_color(&img.rgba));
    let i = TexInfo { emissive: is_emissive(&e.name), color, opaque };
    info.insert(crc, i);
    Some(i)
}

/// Mean colour of an image (`rgba`, weighted by alpha), and whether almost all of it is
/// opaque.
fn mean_color(rgba: &[u8]) -> ([f32; 3], bool) {
    let mut acc = [0.0f64; 4];
    let mut holes = 0usize;
    let px = rgba.as_chunks::<4>().0;
    for p in px {
        let a = p[3] as f64 / 255.0;
        for c in 0..3 {
            acc[c] += p[c] as f64 / 255.0 * a;
        }
        acc[3] += a;
        holes += (p[3] < 128) as usize;
    }
    let opaque = holes * 50 <= px.len();
    if acc[3] <= 0.0 {
        return ([1.0; 3], opaque);
    }
    ([(acc[0] / acc[3]) as f32, (acc[1] / acc[3]) as f32, (acc[2] / acc[3]) as f32], opaque)
}

/// The copies of the frame drawn so far that the generated water reads (colour in `format`,
/// depth).
fn scene_copy_textures(device: &wgpu::Device, w: u32, h: u32, format: wgpu::TextureFormat) -> (wgpu::Texture, wgpu::Texture) {
    let tex = |label, format| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    };
    (tex("scene copy", format), tex("scene depth copy", wgpu::TextureFormat::Depth32Float))
}

fn grid_cam_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    cam: &wgpu::Buffer,
    copy: &(wgpu::Texture, wgpu::Texture),
    sampler: &wgpu::Sampler,
    flag: &wgpu::Buffer,
    falls: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("generated water camera"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: cam.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&copy.0.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&copy.1.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(sampler) },
            wgpu::BindGroupEntry { binding: 4, resource: flag.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 5, resource: falls.as_entire_binding() },
        ],
    })
}

impl Gpu {
    pub fn new(device: &wgpu::Device, tex_layout: &wgpu::BindGroupLayout) -> Gpu {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("gpu meshes"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let ubo = |binding, vis| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: vis,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let storage = |binding, vis| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: vis,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let vf = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let cam_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("gpu mesh camera"), entries: &[ubo(0, vf)] });
        let v = wgpu::ShaderStages::VERTEX;
        let store_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gpu mesh draws"),
            entries: &[storage(0, v), storage(1, v), storage(2, v)],
        });
        let light_entries = [
            ubo(0, vf),
            storage(1, wgpu::ShaderStages::FRAGMENT),
            storage(4, vf),
            wgpu::BindGroupLayoutEntry {
                binding: 5,
                visibility: vf,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 9,
                visibility: vf,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 10,
                visibility: vf,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 11,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 12,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ];
        let lights_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("gpu mesh lights"), entries: &light_entries });
        let fr = wgpu::ShaderStages::FRAGMENT;
        let grid_cam_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("generated water camera"),
            entries: &[
                ubo(0, vf),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: fr,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: fr,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: fr, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                ubo(4, fr),
                ubo(5, vf),
            ],
        });
        // The shadow pass reads only the lights' uniform block (the map is its target).
        let shadow_ubo_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("gpu mesh shadow"), entries: &[ubo(0, vf)] });
        // The bed map pass reads the lights' block and the surface map.
        let bed_ubo_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water bed map"),
            entries: &[
                ubo(0, vf),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: vf,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gpu meshes"),
            bind_group_layouts: &[Some(&cam_layout), Some(tex_layout), Some(&store_layout), Some(&lights_layout)],
            immediate_size: 0,
        });
        let grid_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("generated water"),
            bind_group_layouts: &[Some(&grid_cam_layout), Some(tex_layout), Some(&store_layout), Some(&lights_layout)],
            immediate_size: 0,
        });
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gpu mesh shadow"),
            bind_group_layouts: &[None, None, Some(&store_layout), Some(&shadow_ubo_layout)],
            immediate_size: 0,
        });
        // Path-Traced: the same shader with the ray-traced hooks, and their bindings.
        let rtx = device.features().contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY).then(|| {
            let f = wgpu::ShaderStages::FRAGMENT;
            let mut entries = light_entries.to_vec();
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 6,
                visibility: f,
                ty: wgpu::BindingType::AccelerationStructure { vertex_return: false },
                count: None,
            });
            entries.push(storage(7, f));
            entries.push(storage(8, f));
            let lights_layout =
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("gpu mesh lights (ray traced)"), entries: &entries });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("gpu meshes (ray traced)"),
                bind_group_layouts: &[Some(&cam_layout), Some(tex_layout), Some(&store_layout), Some(&lights_layout)],
                immediate_size: 0,
            });
            let src = format!("enable wgpu_ray_query;\n{}", replace_block(SHADER, "// RT-BEGIN", "// RT-END", crate::rt::RT_FNS));
            let shader = device
                .create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("gpu meshes (ray traced)"), source: wgpu::ShaderSource::Wgsl(src.into()) });
            let rt = crate::rt::Rt::new(device, block(SHADER, "// LIGHTS-BEGIN", "// LIGHTS-END"), block(SHADER, "// WATER-BEGIN", "// WATER-END"));
            RtParts { rt, shader, layout, lights_layout, bg: None }
        });
        let buf = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage: usage | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
        };
        let cam = buf("gpu mesh camera", CAM_SIZE, wgpu::BufferUsages::UNIFORM);
        let lights = buf("gpu mesh lights", LIGHTS_SIZE, wgpu::BufferUsages::UNIFORM);
        let points = buf("gpu mesh point lights", (MAX_POINT_LIGHTS * 32) as u64, wgpu::BufferUsages::STORAGE);
        let cam_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gpu mesh camera"),
            layout: &cam_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: cam.as_entire_binding() }],
        });
        let shadow_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sun shadow map"),
            size: wgpu::Extent3d { width: SHADOW_SIZE, height: SHADOW_SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shadow_view = shadow_tex.create_view(&Default::default());
        let shadow_s = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let sim = crate::water::WaterSim::new(device);
        let linear_s = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("scene copy"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let obstacle_s = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("water obstacle map"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let scene_flag = buf("scene copy flag", 16, wgpu::BufferUsages::UNIFORM);
        let scene_copy = scene_copy_textures(device, 1, 1, wgpu::TextureFormat::Rgba8Unorm);
        let falls = buf("waterfall feet", FALLS_SIZE, wgpu::BufferUsages::UNIFORM);
        let grid_cam_bg = grid_cam_bind_group(device, &grid_cam_layout, &cam, &scene_copy, &linear_s, &scene_flag, &falls);
        let bed_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water bed map"),
            bind_group_layouts: &[None, None, Some(&store_layout), Some(&bed_ubo_layout)],
            immediate_size: 0,
        });
        let lights_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gpu mesh lights"),
            layout: &lights_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: lights.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: points.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&shadow_view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&shadow_s) },
                wgpu::BindGroupEntry { binding: 4, resource: sim.heights().as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&sim.surface_view) },
                wgpu::BindGroupEntry { binding: 9, resource: wgpu::BindingResource::TextureView(&sim.bed_view) },
                wgpu::BindGroupEntry { binding: 10, resource: wgpu::BindingResource::TextureView(&sim.flow_view) },
                wgpu::BindGroupEntry { binding: 11, resource: wgpu::BindingResource::TextureView(&sim.obstacle_view) },
                wgpu::BindGroupEntry { binding: 12, resource: wgpu::BindingResource::Sampler(&obstacle_s) },
            ],
        });
        let shadow_ubo_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gpu mesh shadow"),
            layout: &shadow_ubo_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: lights.as_entire_binding() }],
        });
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sun shadow"),
            layout: Some(&shadow_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_shadow"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_layout())],
            },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: wgpu::DepthBiasState { constant: 2, slope_scale: 2.0, clamp: 0.0 },
            }),
            multisample: Default::default(),
            fragment: None,
            multiview_mask: None,
            cache: None,
        });
        let shadow_cut_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gpu mesh shadow (cut-out)"),
            bind_group_layouts: &[None, Some(tex_layout), Some(&store_layout), Some(&shadow_ubo_layout)],
            immediate_size: 0,
        });
        let shadow_cut_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sun shadow (cut-out)"),
            layout: Some(&shadow_cut_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_shadow_cut"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_layout())],
            },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: wgpu::DepthBiasState { constant: 2, slope_scale: 2.0, clamp: 0.0 },
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs_shadow_cut"), compilation_options: Default::default(), targets: &[] }),
            multiview_mask: None,
            cache: None,
        });
        let bed_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water bed map"),
            layout: &bed_ubo_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: lights.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&sim.surface_view) },
            ],
        });
        let bed_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("water bed map"),
            layout: Some(&bed_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_bed"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_layout())],
            },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs_bed"), compilation_options: Default::default(), targets: &[] }),
            multiview_mask: None,
            cache: None,
        });
        // What stands in the water, from above: the most solid wins.
        let max = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Max };
        let obstacle_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("water obstacle map"),
            layout: Some(&bed_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_obstacle"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_layout())],
            },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_obstacle"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: crate::water::OBSTACLE_FORMAT,
                    blend: Some(wgpu::BlendState { color: max, alpha: max }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        // Top-down depth of the water: the highest surface wins.
        let surface_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("water surface map"),
            layout: Some(&shadow_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_surface"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_layout())],
            },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            // ... and how fast it flows (the flow map).
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_surface"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: crate::water::FLOW_FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Gpu {
            meshes: HashMap::new(),
            shader,
            layout,
            store_layout,
            cam,
            cam_bg,
            lights,
            points,
            lights_bg,
            shadow_ubo_bg,
            shadow_view,
            shadow_pipeline,
            shadow_cut_pipeline,
            store: None,
            pipelines: HashMap::new(),
            list: Vec::new(),
            tex_info: HashMap::new(),
            shadows: false,
            water_meshes: HashMap::new(),
            water_list: Vec::new(),
            liquid_list: Vec::new(),
            water_pipelines: HashMap::new(),
            surface_pipeline,
            bed_pipeline,
            obstacle_pipeline,
            bed_bg,
            grid: None,
            grid_pipelines: HashMap::new(),
            grid_layout,
            grid_cam_layout,
            grid_cam_bg,
            scene_flag,
            scene_copy,
            linear_s,
            obstacle_s,
            falls,
            fall_count: 0,
            mist_pipelines: HashMap::new(),
            sim,
            vb_usage: if rtx.is_some() {
                wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::BLAS_INPUT
            } else {
                wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST
            },
            rtx,
            rt_on: false,
            frame: 0,
            temporal: false,
            shadow_s,
        }
    }

    /// Uploads new meshes and this frame's records, matrices, camera and lights; resolves the
    /// textures of every draw. Returns the (texture, wrap) pairs whose bind groups the
    /// renderer must have before [`draw`](Self::draw). `extra_lights`: point lights the
    /// renderer found elsewhere (emissive sprites).
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sink: &mut Sink,
        textures: &TextureRegistry,
        white: TexHandle,
        view: &View,
        prev: Option<&View>,
        jitter: [f32; 2],
        fog: &crate::scene::Fog,
        lighting: Option<&Lighting>,
        extra_lights: &[PointLight],
        water_clock: f64,
        splashes: &mut Vec<crate::water::Splash>,
        ray_tracing: bool,
    ) -> Vec<(TexHandle, bool)> {
        self.frame = self.frame.wrapping_add(1);
        self.temporal = jitter != [0.0; 2];
        use wgpu::util::DeviceExt;
        for (key, verts, groups) in sink.water_pending.drain(..) {
            if verts.is_empty() {
                continue;
            }
            let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("water mesh"),
                contents: bytemuck::cast_slice(&verts),
                usage: wgpu::BufferUsages::VERTEX,
            });
            self.water_meshes.insert(key, (buf, groups));
        }
        self.water_list.clear();
        self.liquid_list.clear();
        self.sim.frame.steps = 0;
        for (key, verts) in sink.slid.drain(..) {
            if let Some((buf, _)) = self.meshes.get(&key).filter(|(b, _)| b.size() == std::mem::size_of_val(verts.as_slice()) as u64) {
                queue.write_buffer(buf, 0, bytemuck::cast_slice(&verts));
            }
        }
        for (key, verts, groups) in sink.pending.drain(..) {
            if verts.is_empty() {
                self.meshes.insert(
                    key,
                    (
                        device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 64, usage: wgpu::BufferUsages::VERTEX, mapped_at_creation: false }),
                        Vec::new(),
                    ),
                );
                continue;
            }
            let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("gpu mesh"),
                contents: bytemuck::cast_slice(&verts),
                usage: self.vb_usage,
            });
            self.meshes.insert(key, (buf, groups));
        }
        self.list.clear();
        self.shadows = false;
        if sink.draws.is_empty() && sink.water.is_empty() {
            return Vec::new();
        }
        // Records and matrices: storage buffers, grown as needed.
        let need = |n: usize, each: usize| ((n.max(1) * each) as u64).next_power_of_two().max(256);
        let (rn, mn) = (need(sink.recs.len(), 64), need(sink.mats.len(), 64));
        if self.store.as_ref().is_none_or(|(r, m, _, _)| r.size() < rn || m.size() < mn) {
            let mk = |label, size| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            };
            let (r, m, p) = (mk("gpu mesh records", rn), mk("gpu mesh matrices", mn), mk("gpu mesh previous matrices", mn));
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("gpu mesh draws"),
                layout: &self.store_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: r.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: m.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: p.as_entire_binding() },
                ],
            });
            self.store = Some((r, m, p, bg));
        }
        let (r, m, p, _) = self.store.as_ref().unwrap();
        queue.write_buffer(r, 0, bytemuck::cast_slice(&sink.recs));
        queue.write_buffer(m, 0, bytemuck::cast_slice(&sink.mats));
        queue.write_buffer(p, 0, bytemuck::cast_slice(&sink.prev));
        // Camera.
        let pv = prev.unwrap_or(view);
        let pr = |v: &View| {
            let p = crate::upscale::projection(v);
            [p[0][0], p[1][1], p[2][0], p[2][1]]
        };
        let mut cam: Vec<f32> = Vec::with_capacity((CAM_SIZE / 4) as usize);
        cam.extend(view.v_matrix.iter().flatten());
        cam.extend(pv.v_matrix.iter().flatten());
        cam.extend(pr(view));
        cam.extend(pr(pv));
        cam.extend([view.width, view.height, jitter[0], jitter[1]]);
        cam.extend([if fog.enabled { 1.0 } else { 0.0 }, fog.start, fog.end, view.far_clip]);
        cam.extend([fog.color[0], fog.color[1], fog.color[2], 1.0]);
        // Normal / Wrap passes: `tightAlphaCmpRS` (> 0x64).
        cam.extend([crate::upscale::DEPTH_NEAR, 100.0, 0.0, 0.0]);
        queue.write_buffer(&self.cam, 0, bytemuck::cast_slice(&cam));
        // Draw list with resolved textures (`PCRenderObject`: objects skip faces whose
        // texture is not loaded; the landscape draws them untextured), and the point lights
        // of emissive groups.
        let lit = lighting.is_some();
        let auto_lights = lighting.is_some_and(|l| l.emissive_lights);
        let mut found: Vec<PointLight> = Vec::new();
        let mut need_bg = Vec::new();
        for d in &sink.draws {
            let Some((_, groups)) = self.meshes.get(&d.key) else { continue };
            for g in groups {
                let tex = match d.override_tex.or_else(|| textures.current(g.crc)) {
                    Some(t) => t,
                    None if d.landscape => white,
                    None => continue,
                };
                let info = tex_info(&mut self.tex_info, textures, g.crc);
                let (emissive, color) = info.map_or((false, [1.0; 3]), |i| (i.emissive, i.color));
                let emissive = lit && emissive && d.override_tex.is_none() && !d.sky;
                let rec = &sink.recs[d.rec as usize];
                if emissive && auto_lights && d.key.1 != MESH_SKIN {
                    point_lights(&sink.mats[rec.mat as usize], g, color, &mut found);
                } else if lit && auto_lights && rec.flags & 64 != 0 && d.override_tex.is_none() {
                    // A flowing liquid's glow lights what stands in it and its banks, dimly.
                    point_lights(&sink.mats[rec.mat as usize], g, color.map(|c| c * LIQUID_LIGHT), &mut found);
                }
                let cut = d.override_tex.is_none() && info.is_some_and(|i| !i.opaque);
                if rec.flags & 64 != 0 {
                    self.liquid_list.push((d.rec, d.key, g.first..g.first + g.count));
                }
                self.list.push((d.rec, d.key, g.first..g.first + g.count, tex, d.wrap, emissive, cut));
                need_bg.push((tex, d.wrap));
            }
        }
        // Water; the flat faces' biggest texture tints the generated surface.
        let mut biggest: Option<(u32, u32, TexHandle)> = None;
        // Waterfalls stir the water where they land (a steady source, turbulent: the points
        // move about a little from step to step).
        let mut falls = Vec::new();
        let mut stir = |k: usize, p: [f32; 3]| {
            let h = |salt: u32| {
                let x = ((water_clock * 30.0) as u32)
                    .wrapping_mul(747796405)
                    .wrapping_add((k as u32).wrapping_mul(2891336453).wrapping_add(salt.wrapping_mul(1013904223)));
                let x = (x ^ (x >> 16)).wrapping_mul(2246822519);
                (x ^ (x >> 13)) as f32 / u32::MAX as f32 - 0.5
            };
            let pos = [p[0] + h(1) * FOOT_CELL, p[1], p[2] + h(2) * FOOT_CELL];
            // The fall's mesh may go on far under its pool's surface.
            falls.push(crate::water::Splash { pos, radius: 12.0, strength: 0.18 + 0.12 * h(3), above: 400.0, ..Default::default() });
            // A little below the foot: a fall may end over its pool's surface (Garden's), and its
            // mesh may go on far under it.
            let pos = [p[0] + h(1) * FOOT_CELL, p[1] - 20.0, p[2] + h(2) * FOOT_CELL];
            falls.push(crate::water::Splash { pos, radius: 12.0, strength: 0.12 + 0.08 * h(3), above: 400.0, ..Default::default() });
        };
        // ... and their foam, mist and spray (where each lands and the way it faces there).
        let mut feet_seen: Vec<([f32; 3], [f32; 3])> = Vec::new();
        // A group's feet are its faces' lowest points: those at its bottom are where it lands.
        let landing = |feet: &[[f32; 3]]| {
            let low = feet.iter().map(|f| f[1]).fold(f32::MAX, f32::min);
            move |f: &[f32; 3]| f[1] < low + FOOT_CELL
        };
        for d in &sink.draws {
            if let Some(feet) = sink.fall_feet.get(&d.key) {
                let m = &sink.mats[sink.recs[d.rec as usize].mat as usize];
                let outs = sink.fall_out.get(&d.key);
                let lands = landing(feet);
                for (k, &f) in feet.iter().enumerate() {
                    let p = crate::math::mtx_xfm(m, f);
                    stir(k + 64, p);
                    if lands(&f) {
                        let o = outs.and_then(|o| o.get(k)).map_or([0.0; 3], |&o| sub(crate::math::mtx_xfm(m, [f[0] + o[0], f[1] + o[1], f[2] + o[2]]), p));
                        feet_seen.push((p, o));
                    }
                }
            }
        }
        for &(key, rec, additive) in &sink.water {
            let Some((_, groups)) = self.water_meshes.get(&key) else { continue };
            for g in groups {
                let m = &sink.mats[sink.recs[rec as usize].mat as usize];
                let lands = landing(&g.feet);
                for (k, &f) in g.feet.iter().enumerate() {
                    let p = crate::math::mtx_xfm(m, f);
                    stir(k, p);
                    if lands(&f) {
                        feet_seen.push((p, [0.0; 3]));
                    }
                }
                let Some(tex) = textures.current(g.crc) else { continue };
                self.water_list.push((rec, key, g.first..g.first + g.count, tex, true, additive, g.steep));
                need_bg.push((tex, true));
                if !g.steep && biggest.is_none_or(|b| g.count > b.0) {
                    biggest = Some((g.count, g.crc, tex));
                }
            }
        }
        self.write_falls(queue, view, if lighting.is_some_and(|l| l.water) { feet_seen } else { Vec::new() });
        self.grid = biggest.filter(|_| lighting.is_some_and(|l| l.water)).map(|(_, crc, tex)| {
            let c = tex_info(&mut self.tex_info, textures, crc).map_or([0.3, 0.5, 0.7], |i| i.color);
            (tex, c)
        });
        // The wave simulation, around the camera's target.
        if lighting.is_some_and(|l| l.water) && (!self.water_list.is_empty() || !self.list.is_empty()) {
            let centre = view.camera.map_or([0.0; 3], |c| c.target);
            if self.sim.plan(queue, centre, water_clock, splashes, &falls) {
                splashes.clear();
            }
        }
        if auto_lights {
            for (key, m) in &sink.light_only {
                let Some((_, groups)) = self.meshes.get(key) else { continue };
                for g in groups {
                    if let Some(TexInfo { emissive: true, color, .. }) = tex_info(&mut self.tex_info, textures, g.crc) {
                        point_lights(m, g, color, &mut found);
                    }
                }
            }
        }
        self.rt_on = ray_tracing && lighting.is_some() && self.rtx.is_some();
        if self.rt_on {
            self.prepare_rt(device, queue, sink, textures);
        }
        self.write_lights(queue, view, lighting, found, extra_lights);
        need_bg
    }

    /// Path-Traced: acceleration structures for new meshes, this frame's instances, and the
    /// bind groups over them.
    fn prepare_rt(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, sink: &Sink, textures: &TextureRegistry) {
        let Gpu { rtx, meshes, tex_info: info, .. } = self;
        let Some(rx) = rtx.as_mut() else { return };
        let mut inst = Vec::with_capacity(sink.draws.len());
        for d in &sink.draws {
            if d.key.1 == MESH_SKIN || d.sky {
                continue;
            }
            let Some((_, groups)) = meshes.get(&d.key) else { continue };
            if !rx.rt.has_mesh(&d.key) {
                // Groups with solid textures (the landscape's untextured faces are solid too).
                let opaque: Vec<(u32, u32)> =
                    groups.iter().filter(|g| tex_info(info, textures, g.crc).map_or(d.landscape, |i| i.opaque)).map(|g| (g.first, g.count)).collect();
                rx.rt.add_mesh(device, d.key, &opaque);
            }
            let (mut sum, mut n) = ([0.0f32; 3], 0.0f32);
            for g in groups {
                let c = d.override_tex.map_or_else(|| tex_info(info, textures, g.crc).map_or([0.8; 3], |i| i.color), |_| [0.8; 3]);
                let w = g.count as f32;
                sum = [sum[0] + c[0] * w, sum[1] + c[1] * w, sum[2] + c[2] * w];
                n += w;
            }
            let albedo = if n > 0.0 { [sum[0] / n, sum[1] / n, sum[2] / n] } else { [0.8; 3] };
            inst.push((d.key, sink.mats[sink.recs[d.rec as usize].mat as usize], albedo));
        }
        rx.rt.set_instances(device, queue, &inst);
        if rx.rt.rebind || rx.bg.is_none() {
            rx.rt.rebind = false;
            rx.bg = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("gpu mesh lights (ray traced)"),
                layout: &rx.lights_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.lights.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: self.points.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&self.shadow_view) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.shadow_s) },
                    wgpu::BindGroupEntry { binding: 4, resource: self.sim.heights().as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&self.sim.surface_view) },
                    wgpu::BindGroupEntry { binding: 9, resource: wgpu::BindingResource::TextureView(&self.sim.bed_view) },
                    wgpu::BindGroupEntry { binding: 10, resource: wgpu::BindingResource::TextureView(&self.sim.flow_view) },
                    wgpu::BindGroupEntry { binding: 11, resource: wgpu::BindingResource::TextureView(&self.sim.obstacle_view) },
                    wgpu::BindGroupEntry { binding: 12, resource: wgpu::BindingResource::Sampler(&self.obstacle_s) },
                    wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::AccelerationStructure(&rx.rt.tlas) },
                    wgpu::BindGroupEntry { binding: 7, resource: rx.rt.albedo.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 8, resource: rx.rt.photons.as_entire_binding() },
                ],
            }));
            rx.rt.photon_bg = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("photons"),
                layout: &rx.rt.photon_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.lights.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: self.sim.heights().as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&self.sim.surface_view) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::AccelerationStructure(&rx.rt.tlas) },
                    wgpu::BindGroupEntry { binding: 4, resource: rx.rt.photons.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&self.sim.flow_view) },
                ],
            }));
        }
    }

    /// The lights' uniform block and point light list (the nearest to the camera's target).
    /// The waterfalls' landing points (`Falls`): the nearest to the camera's target first.
    fn write_falls(&mut self, queue: &wgpu::Queue, view: &View, mut feet: Vec<([f32; 3], [f32; 3])>) {
        let focus = view.camera.map_or([0.0; 3], |c| c.target);
        feet.sort_by(|a, b| dot(sub(a.0, focus), sub(a.0, focus)).total_cmp(&dot(sub(b.0, focus), sub(b.0, focus))));
        feet.truncate(MAX_FALL_FEET);
        let mut u = vec![0.0f32; (FALLS_SIZE / 4) as usize];
        u[0] = feet.len() as f32;
        for (k, (p, o)) in feet.iter().enumerate() {
            // Horizontal: the mist drifts out over the pool.
            let h = (o[0] * o[0] + o[2] * o[2]).sqrt();
            let o = if h > 1e-3 { [o[0] / h, o[2] / h] } else { [0.0; 2] };
            u[4 + k * 4..8 + k * 4].copy_from_slice(&[p[0], p[1], p[2], 0.0]);
            let j = 4 + MAX_FALL_FEET * 4 + k * 4;
            u[j..j + 4].copy_from_slice(&[o[0], 0.0, o[1], 0.0]);
        }
        self.fall_count = feet.len() as u32;
        queue.write_buffer(&self.falls, 0, bytemuck::cast_slice(&u));
    }

    fn write_lights(&mut self, queue: &wgpu::Queue, view: &View, lighting: Option<&Lighting>, mut found: Vec<PointLight>, extra: &[PointLight]) {
        let mut u = [0.0f32; (LIGHTS_SIZE / 4) as usize];
        let Some(l) = lighting else {
            queue.write_buffer(&self.lights, 0, bytemuck::cast_slice(&u));
            return;
        };
        let cam = view.camera.unwrap_or(crate::scene::Camera { source: [0.0, 400.0, -300.0], target: [0.0; 3], up: [0.0, 1.0, 0.0] });
        let focus = cam.target;
        let mut lights: Vec<PointLight> = l.point_lights.clone();
        if l.emissive_lights {
            found.extend_from_slice(extra);
            // Merge lights closer than 40 units (a lava pool's many groups, a torch's sprites).
            let mut merged: Vec<(PointLight, f32)> = Vec::new();
            for p in found {
                match merged.iter_mut().find(|(m, _)| dot(sub(m.pos, p.pos), sub(m.pos, p.pos)) < 40.0 * 40.0) {
                    Some((m, n)) => {
                        let k = 1.0 / (*n + 1.0);
                        m.pos = [m.pos[0] + (p.pos[0] - m.pos[0]) * k, m.pos[1] + (p.pos[1] - m.pos[1]) * k, m.pos[2] + (p.pos[2] - m.pos[2]) * k];
                        m.radius = m.radius.max(p.radius);
                        *n += 1.0;
                    }
                    None => merged.push((p, 1.0)),
                }
            }
            lights.extend(merged.into_iter().map(|(p, _)| p));
        }
        let d2 = |p: &PointLight| dot(sub(p.pos, focus), sub(p.pos, focus));
        lights.sort_by(|a, b| d2(a).total_cmp(&d2(b)));
        lights.truncate(MAX_POINT_LIGHTS);
        let mut pts = vec![0.0f32; MAX_POINT_LIGHTS * 8];
        for (k, p) in lights.iter().enumerate() {
            pts[k * 8..k * 8 + 8].copy_from_slice(&[p.pos[0], p.pos[1], p.pos[2], p.radius, p.color[0], p.color[1], p.color[2], 1.0]);
        }
        queue.write_buffer(&self.points, 0, bytemuck::cast_slice(&pts));
        // Shadow map frame: centred on the camera's target, sized by the camera distance,
        // snapped to whole texels so it does not shimmer as the camera moves.
        let sun = l.sun;
        let to_light = sun.map_or([0.0, 1.0, 0.0], |s| crate::lighting::normalize(s.to_light));
        let up_hint = if to_light[1].abs() > 0.95 { [0.0, 0.0, 1.0] } else { [0.0, 1.0, 0.0] };
        let right = crate::lighting::normalize(cross(up_hint, to_light));
        let up = cross(to_light, right);
        let dist = dot(sub(cam.source, cam.target), sub(cam.source, cam.target)).sqrt();
        let half = (dist * 1.6).clamp(500.0, 3000.0);
        let texel = 2.0 * half / SHADOW_SIZE as f32;
        let snap = |v: f32| (v / texel).round() * texel;
        let (cx, cy, cz) = (dot(focus, right), dot(focus, up), dot(focus, to_light));
        let (cx, cy) = (snap(cx), snap(cy));
        let centre =
            [right[0] * cx + up[0] * cy + to_light[0] * cz, right[1] * cx + up[1] * cy + to_light[1] * cz, right[2] * cx + up[2] * cy + to_light[2] * cz];
        let depth_range = 8000.0;
        self.shadows = sun.is_some_and(|s| s.shadows);
        let (sd, sc) = sun.map_or(([0.0; 3], [0.0; 3]), |s| (to_light, s.color));
        u[..36].copy_from_slice(&[
            l.sky[0],
            l.sky[1],
            l.sky[2],
            1.0,
            l.ground[0],
            l.ground[1],
            l.ground[2],
            lights.len() as f32,
            sd[0],
            sd[1],
            sd[2],
            sun.is_some() as u32 as f32,
            sc[0],
            sc[1],
            sc[2],
            l.specular,
            centre[0],
            centre[1],
            centre[2],
            half,
            right[0],
            right[1],
            right[2],
            depth_range,
            up[0],
            up[1],
            up[2],
            self.shadows as u32 as f32,
            to_light[0],
            to_light[1],
            to_light[2],
            1.0 / SHADOW_SIZE as f32,
            cam.source[0],
            cam.source[1],
            cam.source[2],
            if self.temporal { (self.frame % 4096) as f32 } else { -1.0 },
        ]);
        let wf = self.sim.frame;
        let on = l.water && self.sim.started();
        u[44] = l.baked.clamp(0.0, 1.0);
        if let Some((_, c)) = self.grid {
            u[48..52].copy_from_slice(&[c[0], c[1], c[2], 1.0]);
        }
        u[52..56].copy_from_slice(&[l.water_flow[0], l.water_flow[1], l.water_swell, 0.0]);
        u[36..44].copy_from_slice(&[
            wf.origin[0],
            wf.origin[1],
            crate::water::TEXEL,
            crate::water::N as f32,
            wf.time,
            on as u32 as f32,
            wf.ref_y,
            crate::water::RANGE,
        ]);
        queue.write_buffer(&self.lights, 0, bytemuck::cast_slice(&u));
    }

    /// The passes before the frame: the sun's shadow map (every GPU mesh draw, on screen or
    /// not), and on simulation steps the water's surface map and the steps.
    /// The sun's shadow map (before [`pre_passes`](Self::pre_passes)); `bind_group` as in
    /// [`draw`](Self::draw), for the cut-out textures.
    pub fn shadow_pass<'b>(&self, encoder: &mut wgpu::CommandEncoder, bind_group: impl Fn(TexHandle, bool) -> &'b wgpu::BindGroup) {
        let Some((_, _, _, store_bg)) = &self.store else { return };
        if !self.shadows || self.list.is_empty() {
            return;
        }
        let mut pass = depth_pass(encoder, "sun shadow", &self.shadow_view);
        pass.set_pipeline(&self.shadow_pipeline);
        pass.set_bind_group(2, store_bg, &[]);
        pass.set_bind_group(3, &self.shadow_ubo_bg, &[]);
        draw_list(&mut pass, &self.meshes, self.list.iter().filter(|d| !d.6).map(|d| (d.0, d.1, d.2.clone())));
        pass.set_pipeline(&self.shadow_cut_pipeline);
        let mut cur: Option<Key> = None;
        for (rec, key, range, tex, wrap, _, cut) in &self.list {
            if !*cut {
                continue;
            }
            if cur != Some(*key) {
                let Some((buf, _)) = self.meshes.get(key) else { continue };
                pass.set_vertex_buffer(0, buf.slice(..));
                cur = Some(*key);
            }
            pass.set_bind_group(1, bind_group(*tex, *wrap), &[]);
            pass.draw(range.clone(), *rec..*rec + 1);
        }
    }

    pub fn pre_passes(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some((_, _, _, store_bg)) = &self.store else { return };
        if self.sim.frame.steps > 0 {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("water surface map"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.sim.flow_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.sim.surface_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.surface_pipeline);
            pass.set_bind_group(2, store_bg, &[]);
            pass.set_bind_group(3, &self.shadow_ubo_bg, &[]);
            // Level water only: a waterfall would lift the pool it lands in to its top.
            draw_list(&mut pass, &self.water_meshes, self.water_list.iter().filter(|d| !d.6).map(|d| (d.0, d.1, d.2.clone())));
            // ... and the opaque liquids (their steep faces left out per pixel).
            draw_list(&mut pass, &self.meshes, self.liquid_list.iter().cloned());
            drop(pass);
            let mut pass = depth_pass(encoder, "water bed map", &self.sim.bed_view);
            pass.set_pipeline(&self.bed_pipeline);
            pass.set_bind_group(2, store_bg, &[]);
            pass.set_bind_group(3, &self.bed_bg, &[]);
            draw_list(&mut pass, &self.meshes, self.list.iter().map(|d| (d.0, d.1, d.2.clone())));
            drop(pass);
            // What stands in it: the level and rigid models (not the characters walking about).
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("water obstacle map"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.sim.obstacle_target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.obstacle_pipeline);
            pass.set_bind_group(2, store_bg, &[]);
            pass.set_bind_group(3, &self.bed_bg, &[]);
            draw_list(&mut pass, &self.meshes, self.list.iter().filter(|(_, key, ..)| key.1 != MESH_SKIN).map(|d| (d.0, d.1, d.2.clone())));
            drop(pass);
            self.sim.obstacle_mips(encoder);
            self.sim.run(encoder);
        }
        if self.rt_on {
            let Gpu { rtx, meshes, sim, .. } = self;
            if let Some(rx) = rtx.as_mut() {
                rx.rt.build(encoder, |k| meshes.get(k).map(|m| m.0.clone()));
                rx.rt.trace_photons(encoder, sim.started());
            }
        }
    }

    /// Records this frame's water draws into `pass` (after the opaque geometry).
    pub fn draw_water<'b>(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        format: wgpu::TextureFormat,
        samples: u32,
        mrt: bool,
        bind_group: impl Fn(TexHandle, bool) -> &'b wgpu::BindGroup,
    ) {
        let Some((_, _, _, store_bg)) = &self.store else { return };
        if self.water_list.is_empty() {
            return;
        }
        pass.set_bind_group(0, &self.cam_bg, &[]);
        pass.set_bind_group(2, store_bg, &[]);
        pass.set_bind_group(3, &self.lights_bg, &[]);
        let mut cur: Option<Key> = None;
        let mut cur_add = None;
        for (rec, key, range, tex, wrap, additive, steep) in &self.water_list {
            // The flat faces are the generated surface's.
            if self.grid.is_some() && !*steep {
                continue;
            }
            if cur_add != Some(*additive) {
                let Some(p) = self.water_pipelines.get(&(format, samples, mrt, *additive)) else { return };
                pass.set_pipeline(p);
                cur_add = Some(*additive);
            }
            if cur != Some(*key) {
                let Some((buf, _)) = self.water_meshes.get(key) else { continue };
                pass.set_vertex_buffer(0, buf.slice(..));
                cur = Some(*key);
            }
            pass.set_bind_group(1, bind_group(*tex, *wrap), &[]);
            pass.draw(range.clone(), *rec..*rec + 1);
        }
        if let (Some((tex, _)), Some(p)) = (self.grid, self.grid_pipelines.get(&(format, samples, mrt))) {
            pass.set_pipeline(p);
            pass.set_bind_group(0, &self.grid_cam_bg, &[]);
            pass.set_bind_group(1, bind_group(tex, true), &[]);
            pass.draw(0..GRID * GRID * 6, 0..1);
            if let (true, Some(p)) = (self.fall_count > 0, self.mist_pipelines.get(&(format, samples, mrt))) {
                pass.set_pipeline(p);
                pass.draw(0..self.fall_count * MIST_PER_FOOT * 6, 0..1);
            }
        }
    }

    pub fn has_water(&self) -> bool {
        !self.water_list.is_empty()
    }

    /// Whether this frame's water reads a copy of the frame drawn before it (colour `color`,
    /// depth `depth`, single-sampled, the size of the 3D frame). With `Some`, the caller
    /// copies them into [`scene_copy`](Self::scene_copy) right before
    /// [`draw_water`](Self::draw_water).
    pub fn use_scene_copy(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: Option<(&wgpu::Texture, &wgpu::Texture)>) {
        let on = frame.is_some_and(|(c, d)| c.size() == d.size() && c.sample_count() == 1 && d.format() == wgpu::TextureFormat::Depth32Float);
        if let (true, Some((c, _))) = (on, frame) {
            let (w, h, f) = (c.width(), c.height(), c.format());
            let cur = &self.scene_copy.0;
            if (cur.width(), cur.height(), cur.format()) != (w, h, f) {
                self.scene_copy = scene_copy_textures(device, w, h, f);
                self.grid_cam_bg =
                    grid_cam_bind_group(device, &self.grid_cam_layout, &self.cam, &self.scene_copy, &self.linear_s, &self.scene_flag, &self.falls);
            }
        }
        queue.write_buffer(&self.scene_flag, 0, bytemuck::cast_slice(&[if on { 1.0f32 } else { 0.0 }, 0.0, 0.0, 0.0]));
    }

    /// Copies the frame drawn so far (`color`, `depth`) for the water to read.
    pub fn scene_copy(&self, encoder: &mut wgpu::CommandEncoder, color: &wgpu::Texture, depth: &wgpu::Texture) {
        if color.size() != self.scene_copy.0.size() || color.format() != self.scene_copy.0.format() {
            return;
        }
        encoder.copy_texture_to_texture(color.as_image_copy(), self.scene_copy.0.as_image_copy(), color.size());
        encoder.copy_texture_to_texture(depth.as_image_copy(), self.scene_copy.1.as_image_copy(), depth.size());
    }

    fn water_pipeline(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat, samples: u32, mrt: bool, additive: bool) {
        if self.water_pipelines.contains_key(&(format, samples, mrt, additive)) {
            return;
        }
        let comp = |src, dst| wgpu::BlendComponent { src_factor: src, dst_factor: dst, operation: wgpu::BlendOperation::Add };
        use wgpu::BlendFactor as F;
        // The original's `Semi` / `Additive` blends.
        let blend = if additive {
            wgpu::BlendState { color: comp(F::SrcAlpha, F::One), alpha: comp(F::Zero, F::One) }
        } else {
            wgpu::BlendState { color: comp(F::SrcAlpha, F::OneMinusSrcAlpha), alpha: comp(F::One, F::OneMinusSrcAlpha) }
        };
        let targets = [
            Some(wgpu::ColorTargetState { format, blend: Some(blend), write_mask: wgpu::ColorWrites::ALL }),
            Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rg16Float, blend: None, write_mask: wgpu::ColorWrites::empty() }),
            Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::R32Float, blend: None, write_mask: wgpu::ColorWrites::empty() }),
            // The upscaler's reactive mask (CPU-drawn flickering textures only).
            Some(wgpu::ColorTargetState { format: crate::renderer::REACTIVE_FORMAT, blend: None, write_mask: wgpu::ColorWrites::empty() }),
        ];
        if !additive && !self.grid_pipelines.contains_key(&(format, samples, mrt)) {
            // Premultiplied: the reflection is added whatever the water lets through.
            let mut targets = targets.clone();
            targets[0] = Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState { color: comp(F::One, F::OneMinusSrcAlpha), alpha: comp(F::One, F::OneMinusSrcAlpha) }),
                write_mask: wgpu::ColorWrites::ALL,
            });
            let p = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("generated water"),
                layout: Some(&self.grid_layout),
                vertex: wgpu::VertexState { module: &self.shader, entry_point: Some("vs_grid"), compilation_options: Default::default(), buffers: &[] },
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
                fragment: Some(wgpu::FragmentState {
                    module: &self.shader,
                    entry_point: Some(if mrt { "fs_grid_mrt" } else { "fs_grid" }),
                    compilation_options: Default::default(),
                    targets: &targets[..if mrt { 4 } else { 1 }],
                }),
                multiview_mask: None,
                cache: None,
            });
            self.grid_pipelines.insert((format, samples, mrt), p);
            // The mist and spray over the waterfalls' pools: premultiplied like the surface,
            // after it.
            let p = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("waterfall mist"),
                layout: Some(&self.grid_layout),
                vertex: wgpu::VertexState { module: &self.shader, entry_point: Some("vs_mist"), compilation_options: Default::default(), buffers: &[] },
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
                fragment: Some(wgpu::FragmentState {
                    module: &self.shader,
                    entry_point: Some(if mrt { "fs_mist_mrt" } else { "fs_mist" }),
                    compilation_options: Default::default(),
                    targets: &targets[..if mrt { 4 } else { 1 }],
                }),
                multiview_mask: None,
                cache: None,
            });
            self.mist_pipelines.insert((format, samples, mrt), p);
        }
        let constants = [("ADDITIVE", if additive { 1.0 } else { 0.0 })];
        let p = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("water"),
            layout: Some(&self.layout),
            vertex: wgpu::VertexState {
                module: &self.shader,
                entry_point: Some("vs_water"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_layout())],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: &self.shader,
                entry_point: Some(if mrt { "fs_water_mrt" } else { "fs_water" }),
                compilation_options: wgpu::PipelineCompilationOptions { constants: &constants, ..Default::default() },
                targets: &targets[..if mrt { 4 } else { 1 }],
            }),
            multiview_mask: None,
            cache: None,
        });
        self.water_pipelines.insert((format, samples, mrt, additive), p);
    }

    fn pipeline(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat, samples: u32, mrt: bool, emissive: bool, rt: bool) {
        if self.pipelines.contains_key(&(format, samples, mrt, emissive, rt)) {
            return;
        }
        let (shader, layout) = match (&self.rtx, rt) {
            (Some(r), true) => (&r.shader, &r.layout),
            _ => (&self.shader, &self.layout),
        };
        let comp = |src, dst| wgpu::BlendComponent { src_factor: src, dst_factor: dst, operation: wgpu::BlendOperation::Add };
        use wgpu::BlendFactor as F;
        let blend = wgpu::BlendState { color: comp(F::SrcAlpha, F::OneMinusSrcAlpha), alpha: comp(F::One, F::OneMinusSrcAlpha) };
        let targets = [
            Some(wgpu::ColorTargetState { format, blend: Some(blend), write_mask: wgpu::ColorWrites::ALL }),
            Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rg16Float, blend: None, write_mask: wgpu::ColorWrites::ALL }),
            Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::R32Float, blend: None, write_mask: wgpu::ColorWrites::ALL }),
            // The upscaler's reactive mask (CPU-drawn flickering textures only).
            Some(wgpu::ColorTargetState { format: crate::renderer::REACTIVE_FORMAT, blend: None, write_mask: wgpu::ColorWrites::empty() }),
        ];
        let constants = [("EMISSIVE", if emissive { 1.0 } else { 0.0 })];
        let p = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gpu meshes"),
            layout: Some(layout),
            vertex: wgpu::VertexState { module: shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[Some(vertex_layout())] },
            // `D3DCULL_CW` on a y-down screen: the faces drawn are counter-clockwise on screen.
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some(if mrt { "fs_mrt" } else { "fs" }),
                compilation_options: wgpu::PipelineCompilationOptions { constants: &constants, ..Default::default() },
                targets: &targets[..if mrt { 4 } else { 1 }],
            }),
            multiview_mask: None,
            cache: None,
        });
        self.pipelines.insert((format, samples, mrt, emissive, rt), p);
    }

    /// Creates the pipelines [`draw`](Self::draw) will use.
    pub fn ensure_pipeline(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat, samples: u32, mrt: bool) {
        for rt in [false, self.rt_on] {
            self.pipeline(device, format, samples, mrt, false, rt);
            self.pipeline(device, format, samples, mrt, true, rt);
        }
        self.water_pipeline(device, format, samples, mrt, false);
        self.water_pipeline(device, format, samples, mrt, true);
    }

    /// Records this frame's mesh draws into `pass` (`bind_group(tex, wrap)` = the texture's
    /// bind group, prepared by the caller).
    pub fn draw<'b>(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        format: wgpu::TextureFormat,
        samples: u32,
        mrt: bool,
        bind_group: impl Fn(TexHandle, bool) -> &'b wgpu::BindGroup,
    ) {
        let Some((_, _, _, store_bg)) = &self.store else { return };
        let rt = self.rt_on;
        let (Some(lit), Some(emi)) = (self.pipelines.get(&(format, samples, mrt, false, rt)), self.pipelines.get(&(format, samples, mrt, true, rt))) else {
            return;
        };
        let lights_bg = match (&self.rtx, rt) {
            (Some(RtParts { bg: Some(bg), .. }), true) => bg,
            _ => &self.lights_bg,
        };
        if self.list.is_empty() {
            return;
        }
        pass.set_bind_group(0, &self.cam_bg, &[]);
        pass.set_bind_group(2, store_bg, &[]);
        pass.set_bind_group(3, lights_bg, &[]);
        let mut cur: Option<Key> = None;
        let mut cur_emissive = None;
        for (rec, key, range, tex, wrap, emissive, _) in &self.list {
            if cur_emissive != Some(*emissive) {
                pass.set_pipeline(if *emissive { emi } else { lit });
                cur_emissive = Some(*emissive);
            }
            if cur != Some(*key) {
                let Some((buf, _)) = self.meshes.get(key) else { continue };
                pass.set_vertex_buffer(0, buf.slice(..));
                cur = Some(*key);
            }
            pass.set_bind_group(1, bind_group(*tex, *wrap), &[]);
            pass.draw(range.clone(), *rec..*rec + 1);
        }
    }

    pub fn has_draws(&self) -> bool {
        !self.list.is_empty()
    }
}

/// A depth-only pass clearing `view` to 1.
fn depth_pass<'e>(encoder: &'e mut wgpu::CommandEncoder, label: &str, view: &wgpu::TextureView) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view,
            depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// Draws `(record, mesh, vertex range)`s of `meshes`.
fn draw_list(
    pass: &mut wgpu::RenderPass<'_>,
    meshes: &HashMap<Key, (wgpu::Buffer, Vec<Group>)>,
    list: impl Iterator<Item = (u32, Key, core::ops::Range<u32>)>,
) {
    let mut cur: Option<Key> = None;
    for (rec, key, range) in list {
        if cur != Some(key) {
            let Some((buf, _)) = meshes.get(&key) else { continue };
            pass.set_vertex_buffer(0, buf.slice(..));
            cur = Some(key);
        }
        pass.draw(range, rec..rec + 1);
    }
}

/// `Vert` as vertex attributes (uv at 16, normal at 32, colour at 48).
fn vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRS: [wgpu::VertexAttribute; 5] = [
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Uint32, offset: 12, shader_location: 1 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 16, shader_location: 2 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 32, shader_location: 3 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 48, shader_location: 4 },
    ];
    wgpu::VertexBufferLayout { array_stride: 64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &ATTRS }
}
