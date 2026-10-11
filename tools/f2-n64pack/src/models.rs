//! Object banks (`objects/*.dll`) → packed model banks `models/<bank>.mdl` ("F2MB").
//!
//! The N64 runtime runs `f2-engine`'s CPU transform and `f2-game`'s view code on the parsed
//! [`f2_formats::objbank::ObjectBank`], so the packed format is a **lossless** serialisation
//! of exactly that structure: decoding yields a value whose every `f32` is bit-identical to
//! what `ObjectBank::parse` returns from the DLL (verified for every retail bank by the
//! tests). No field is quantised, because none needs to be: the bank tool wrote the C
//! source with `printf("%f")`, so every float is a 6-decimal number, vertices/bounding boxes
//! are integers, colours are exactly `round6(b / 255)` and normals `round6(i / 255)`. Each
//! float array is stored in the smallest exact integer form (checked value by value; any
//! array that does not fit falls back to raw IEEE bits), which also makes the gameplay
//! fields (vertices → `FindActorCentre` radius, keyframes, bounding boxes, sprite placement,
//! animation table) trivially exact.
//!
//! # Layout (big-endian)
//! ```text
//! "F2MB" u8 version(1) u8 0 u16 nstreams(= NSTREAMS)
//! u32 len[nstreams]
//! stream 0 .. stream nstreams-1, each zero-padded to a multiple of 4 bytes (so 2/4-byte
//! elements stay aligned for the codec's position contexts; `len` excludes the padding)
//! ```
//! The data is **columnar**: each stream holds one kind of value for the whole bank, in
//! traversal order, so the F2Z stage sees homogeneous arrays (it then applies byte-plane /
//! delta transforms per stream). Streams ([`S_META`] …):
//!
//! | id | stream | element |
//! |---|---|---|
//! | 0 | META | LEB128 varints: counts, flags, modes (signed values zigzagged) |
//! | 1 | NAMES | object names, NUL-terminated (Latin-1 bytes) |
//! | 2 | U32 | ids, phong CRC, texture palettes, sprite textures, raw scalar floats |
//! | 3 | VERT | i16: integer vertices / bounding-box corners |
//! | 4 | FACE | u16: zigzag delta of consecutive face vertex indices |
//! | 5 | UV | i16: face UVs (as stored) |
//! | 6 | NORMAL | i8: normals, value = round6(i / 255) |
//! | 7 | COLOR | u8: face colours, value = round6(b / 255) |
//! | 8 | FFLAG | u8: face flags |
//! | 9 | TEXIDX | u8: per-face index into the mesh's texture palette |
//! | 10 | DECV | i32: 3-vector keys (move/scale) as n / 10^k |
//! | 11 | DECQ | i32: rotation keys as n / 10^k |
//! | 12 | DECM | i32: other float arrays as n / 10^k |
//! | 13 | RAW | u32: IEEE bits of arrays with no exact integer form |
//! | 14 | TIME | i32: keyframe times, delta to the previous key of the array |
//! | 15 | IDX32 | i32: skin vertex indices, delta coded |
//! | 16 | SPRITE | i16: sprite pos[3], size[2], offset[2], flags |
//! | 17 | ANIM | i16: animation start, end |
//!
//! Float array modes (one META varint before each non-empty array):
//! `0` i8/255 (NORMAL), `1` u8/255 (COLOR), `2` integer i16 (VERT), `3 + k` (k = 0..6)
//! decimal `n / 10^k` as i32 (DECV/DECQ/DECM by field), `10` raw bits (RAW).
//! Reconstruction (must be done in `f64` then rounded once to `f32`):
//! `i/255 → ((round(i·1e6/255)) / 1e6) as f32`, `n/10^k → (n / 10^k) as f32`.
//!
//! Traversal order: `nmodels`, then per model: U32 id, U32 radius bits, META object_size
//! (zz), is_skinned, num_vtx (zz), nanims, has_loc_mesh, nsiblings; animations (ANIM
//! start, end; U32 speed bits, looped); the root object, the siblings, the LOC mesh. Then
//! `anim_past_end`: META count, per entry U32 id, META present, [ANIM start, end; U32 speed
//! bits, looped]. Object: U32 id, phong; NAMES name; META xlu, flags (zz), nsprites,
//! has_mesh, neff, nscale, nmove, nrot, has_bbox, matrix_identity, nchildren; sprites (U32
//! texture, SPRITE ×8); mesh; IDX32 skin indices; scale keys, move keys (float array of
//! 3·n values + TIME), rotate keys (4·n values + TIME); bbox (24 values); matrix (16 values
//! unless identity); children. Mesh: META nverts, nfaces, presence bits (1 vertices, 2
//! faces, 4 uvs, 8 normals, 16 colours, 32 face flags, 64 textures); arrays in that order;
//! textures as META palette size p, U32 × p, then TEXIDX per face (or U32 per face if
//! p > 256).

use std::path::Path;

use f2_formats::objbank::{Animation, Mat4, Mesh, Model, ObjSprite, Object, ObjectBank, QKey, VKey};
use f2_n64z::Segment;

use crate::{Class, Ctx, RawAsset, Result};

pub const MAGIC: [u8; 4] = *b"F2MB";
pub const VERSION: u8 = 1;

pub const S_META: usize = 0;
pub const S_NAMES: usize = 1;
pub const S_U32: usize = 2;
pub const S_VERT: usize = 3;
pub const S_FACE: usize = 4;
pub const S_UV: usize = 5;
pub const S_NORMAL: usize = 6;
pub const S_COLOR: usize = 7;
pub const S_FFLAG: usize = 8;
pub const S_TEXIDX: usize = 9;
pub const S_DECV: usize = 10;
pub const S_DECQ: usize = 11;
pub const S_DECM: usize = 12;
pub const S_RAW: usize = 13;
pub const S_TIME: usize = 14;
pub const S_IDX32: usize = 15;
pub const S_SPRITE: usize = 16;
pub const S_ANIM: usize = 17;
pub const NSTREAMS: usize = 18;

/// Element width of each stream (for the F2Z transforms); 0 = byte stream, no transform.
pub const STREAM_WIDTH: [u8; NSTREAMS] = [0, 0, 4, 2, 2, 2, 1, 1, 1, 1, 4, 4, 4, 4, 4, 4, 2, 2];

const MODE_I8: u32 = 0;
const MODE_U8: u32 = 1;
const MODE_I16: u32 = 2;
const MODE_DEC0: u32 = 3;
const MODE_RAW: u32 = 10;

const POW10: [f64; 7] = [1.0, 10.0, 100.0, 1e3, 1e4, 1e5, 1e6];

#[inline]
pub fn from_255(i: i32) -> f32 {
    ((f64::from(i) * 1e6 / 255.0).round() / 1e6) as f32
}
#[inline]
pub fn from_dec(n: i32, k: u32) -> f32 {
    (f64::from(n) / POW10[k as usize]) as f32
}

/// Which family of float field (selects the preferred mode and the DEC stream).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Field {
    Vert,
    Normal,
    Color,
    VKey,
    QKey,
    Misc,
}

fn dec_stream(f: Field) -> usize {
    match f {
        Field::VKey => S_DECV,
        Field::QKey => S_DECQ,
        _ => S_DECM,
    }
}

// ---- writer ----------------------------------------------------------------------

struct W {
    s: Vec<Vec<u8>>,
}

impl W {
    fn new() -> W {
        W { s: vec![Vec::new(); NSTREAMS] }
    }
    fn var(&mut self, mut v: u32) {
        let o = &mut self.s[S_META];
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                o.push(b);
                return;
            }
            o.push(b | 0x80);
        }
    }
    fn zz(&mut self, v: i32) {
        self.var(((v << 1) ^ (v >> 31)) as u32);
    }
    fn u32(&mut self, st: usize, v: u32) {
        self.s[st].extend_from_slice(&v.to_be_bytes());
    }
    fn i16(&mut self, st: usize, v: i16) {
        self.s[st].extend_from_slice(&v.to_be_bytes());
    }
    fn u8(&mut self, st: usize, v: u8) {
        self.s[st].push(v);
    }

    fn floats(&mut self, field: Field, v: &[f32]) {
        if v.is_empty() {
            return;
        }
        let exact = |f: &dyn Fn(f32) -> Option<i32>| -> Option<Vec<i32>> { v.iter().map(|&x| f(x)).collect() };
        let i255 = |x: f32, lo: i32, hi: i32| -> Option<i32> {
            let i = (f64::from(x) * 255.0).round();
            if !(f64::from(lo)..=f64::from(hi)).contains(&i) {
                return None;
            }
            (from_255(i as i32).to_bits() == x.to_bits()).then_some(i as i32)
        };
        match field {
            Field::Normal => {
                if let Some(q) = exact(&|x| i255(x, -128, 127)) {
                    self.var(MODE_I8);
                    for i in q {
                        self.u8(S_NORMAL, i as i8 as u8);
                    }
                    return;
                }
            }
            Field::Color => {
                if let Some(q) = exact(&|x| i255(x, 0, 255)) {
                    self.var(MODE_U8);
                    for i in q {
                        self.u8(S_COLOR, i as u8);
                    }
                    return;
                }
            }
            Field::Vert => {
                let int16 = |x: f32| -> Option<i32> {
                    let i = x as i32;
                    ((-32768..=32767).contains(&i) && (i as f32).to_bits() == x.to_bits()).then_some(i)
                };
                if let Some(q) = exact(&int16) {
                    self.var(MODE_I16);
                    for i in q {
                        self.i16(S_VERT, i as i16);
                    }
                    return;
                }
            }
            _ => {}
        }
        for k in 0..7u32 {
            let dec = |x: f32| -> Option<i32> {
                let n = (f64::from(x) * POW10[k as usize]).round();
                if !(f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&n) {
                    return None;
                }
                (from_dec(n as i32, k).to_bits() == x.to_bits()).then_some(n as i32)
            };
            if let Some(q) = exact(&dec) {
                self.var(MODE_DEC0 + k);
                let st = dec_stream(field);
                for n in q {
                    self.u32(st, n as u32);
                }
                return;
            }
        }
        self.var(MODE_RAW);
        for &x in v {
            self.u32(S_RAW, x.to_bits());
        }
    }

    fn times(&mut self, t: impl Iterator<Item = i32>) {
        let mut prev = 0i32;
        for x in t {
            self.u32(S_TIME, x.wrapping_sub(prev) as u32);
            prev = x;
        }
    }

    fn mesh(&mut self, m: &Mesh) {
        let nv = m.vertices.len().max(m.normals.len());
        let nf = [m.faces.len(), m.face_uv.len() / 3, m.colors.len() / 3, m.face_flags.len(), m.textures.len()].into_iter().max().unwrap();
        self.var(nv as u32);
        self.var(nf as u32);
        let pres = u32::from(!m.vertices.is_empty())
            | u32::from(!m.faces.is_empty()) << 1
            | u32::from(!m.face_uv.is_empty()) << 2
            | u32::from(!m.normals.is_empty()) << 3
            | u32::from(!m.colors.is_empty()) << 4
            | u32::from(!m.face_flags.is_empty()) << 5
            | u32::from(!m.textures.is_empty()) << 6;
        self.var(pres);
        let flat: Vec<f32> = m.vertices.iter().flatten().copied().collect();
        self.floats(Field::Vert, &flat);
        let mut prev = 0i32;
        for f in &m.faces {
            for &ix in f {
                let d = i32::from(ix) - prev;
                prev = i32::from(ix);
                self.s[S_FACE].extend_from_slice(&(((d << 1) ^ (d >> 31)) as u16).to_be_bytes());
            }
        }
        for uv in &m.face_uv {
            self.i16(S_UV, uv[0]);
            self.i16(S_UV, uv[1]);
        }
        let flat: Vec<f32> = m.normals.iter().flatten().copied().collect();
        self.floats(Field::Normal, &flat);
        let flat: Vec<f32> = m.colors.iter().flatten().copied().collect();
        self.floats(Field::Color, &flat);
        for &f in &m.face_flags {
            self.u8(S_FFLAG, f);
        }
        if !m.textures.is_empty() {
            let mut pal: Vec<u32> = Vec::new();
            for &t in &m.textures {
                if !pal.contains(&t) {
                    pal.push(t);
                }
            }
            self.var(pal.len() as u32);
            for &p in &pal {
                self.u32(S_U32, p);
            }
            for &t in &m.textures {
                let i = pal.iter().position(|&p| p == t).unwrap();
                if pal.len() <= 256 {
                    self.u8(S_TEXIDX, i as u8);
                } else {
                    self.u32(S_U32, t);
                }
            }
        }
    }

    fn object(&mut self, o: &Object) {
        self.u32(S_U32, o.id);
        self.u32(S_U32, o.phong);
        self.s[S_NAMES].extend(o.name.chars().map(|c| c as u32 as u8));
        self.s[S_NAMES].push(0);
        let ident = o.matrix == IDENTITY && o.matrix.iter().flatten().all(|x| x.to_bits() == 0 || x.to_bits() == 1f32.to_bits());
        self.var(u32::from(o.xlu));
        self.zz(i32::from(o.flags));
        self.var(o.sprites.len() as u32);
        self.var(u32::from(o.mesh.is_some()));
        self.var(o.effected_verts.len() as u32);
        self.var(o.scale_keys.len() as u32);
        self.var(o.move_keys.len() as u32);
        self.var(o.rotate_keys.len() as u32);
        self.var(u32::from(o.bbox.is_some()));
        self.var(u32::from(ident));
        self.var(o.children.len() as u32);
        for s in &o.sprites {
            self.u32(S_U32, s.texture);
            for v in [s.pos[0], s.pos[1], s.pos[2], s.size[0], s.size[1], s.offset[0], s.offset[1], s.flags] {
                self.i16(S_SPRITE, v);
            }
        }
        if let Some(m) = &o.mesh {
            self.mesh(m);
        }
        let mut prev = 0i32;
        for &e in &o.effected_verts {
            self.u32(S_IDX32, e.wrapping_sub(prev) as u32);
            prev = e;
        }
        for keys in [&o.scale_keys, &o.move_keys] {
            let flat: Vec<f32> = keys.iter().flat_map(|k| k.v).collect();
            self.floats(Field::VKey, &flat);
            self.times(keys.iter().map(|k| k.time));
        }
        let flat: Vec<f32> = o.rotate_keys.iter().flat_map(|k| k.q).collect();
        self.floats(Field::QKey, &flat);
        self.times(o.rotate_keys.iter().map(|k| k.time));
        if let Some(b) = &o.bbox {
            let flat: Vec<f32> = b.iter().flatten().copied().collect();
            self.floats(Field::Vert, &flat);
        }
        if !ident {
            let flat: Vec<f32> = o.matrix.iter().flatten().copied().collect();
            self.floats(Field::Misc, &flat);
        }
        for c in &o.children {
            self.object(c);
        }
    }

    fn anim(&mut self, a: &Animation) {
        self.i16(S_ANIM, a.start);
        self.i16(S_ANIM, a.end);
        self.u32(S_U32, a.speed.to_bits());
        self.u32(S_U32, a.looped as u32);
    }

    fn model(&mut self, m: &Model) {
        self.u32(S_U32, m.id);
        self.u32(S_U32, m.radius.to_bits());
        self.zz(i32::from(m.object_size));
        self.var(u32::from(m.is_skinned));
        self.zz(i32::from(m.num_vtx));
        self.var(m.animations.len() as u32);
        self.var(u32::from(m.loc_mesh.is_some()));
        self.var(m.root_siblings.len() as u32);
        for a in &m.animations {
            self.anim(a);
        }
        self.object(&m.root);
        for s in &m.root_siblings {
            self.object(s);
        }
        if let Some(l) = &m.loc_mesh {
            self.mesh(l);
        }
    }
}

const IDENTITY: Mat4 = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];

/// Serialises a bank. Returns the bytes and the F2Z transform segments (one per stream).
pub fn encode(bank: &ObjectBank) -> (Vec<u8>, Vec<Segment>) {
    let mut w = W::new();
    w.var(bank.models.len() as u32);
    for m in &bank.models {
        w.model(m);
    }
    w.var(bank.anim_past_end.len() as u32);
    for (id, a) in &bank.anim_past_end {
        w.u32(S_U32, *id);
        w.var(u32::from(a.is_some()));
        if let Some(a) = a {
            w.anim(a);
        }
    }
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    out.push(VERSION);
    out.push(0);
    out.extend_from_slice(&(NSTREAMS as u16).to_be_bytes());
    for s in &w.s {
        out.extend_from_slice(&(s.len() as u32).to_be_bytes());
    }
    let hdr = out.len();
    let mut segs = vec![Segment::raw(hdr as u32)];
    for (i, s) in w.s.iter().enumerate() {
        out.extend_from_slice(s);
        out.resize(out.len().next_multiple_of(4), 0);
        segs.push(stream_segment(i, s.len().next_multiple_of(4)));
    }
    (out, segs)
}

/// Default transform for stream `i`, measured per stream over all retail banks
/// (`examples/model_streams.rs`): byte planes help only the index/time streams; vertices,
/// UVs, keys and colours compress best untransformed (the codec's position-aligned literal
/// contexts and repeat offsets already exploit their structure; delta coding hurts because
/// vertex order is not spatially coherent).
fn stream_segment(i: usize, len: usize) -> Segment {
    let len = len as u32;
    match i {
        S_FACE | S_TIME | S_IDX32 => Segment::planes(u16::from(STREAM_WIDTH[i]), len),
        _ => Segment::raw(len),
    }
}

/// Candidate segmentations for the codec to try (per stream: raw / planes / delta).
pub fn candidate_segments(data: &[u8]) -> Vec<Vec<Segment>> {
    let Some(lens) = stream_lens(data) else { return Vec::new() };
    let hdr = 8 + 4 * NSTREAMS as u32;
    let mut planes = vec![Segment::raw(hdr)];
    let mut deltas = vec![Segment::raw(hdr)];
    let mut tuned = vec![Segment::raw(hdr)];
    for (i, &l) in lens.iter().enumerate() {
        let w = STREAM_WIDTH[i];
        let l4 = l.next_multiple_of(4);
        planes.push(if w > 1 { Segment::planes(u16::from(w), l4) } else { Segment::raw(l4) });
        deltas.push(if w > 1 { Segment::delta(u16::from(w), w, true, l4) } else { Segment::raw(l4) });
        let l = l.next_multiple_of(4);
        tuned.push(stream_segment(i, l as usize));
    }
    vec![tuned, planes, deltas]
}

fn stream_lens(data: &[u8]) -> Option<Vec<u32>> {
    if data.get(..4)? != MAGIC || *data.get(4)? != VERSION {
        return None;
    }
    let n = u16::from_be_bytes(data.get(6..8)?.try_into().ok()?) as usize;
    if n != NSTREAMS {
        return None;
    }
    (0..n).map(|i| data.get(8 + 4 * i..12 + 4 * i).map(|b| u32::from_be_bytes(b.try_into().unwrap()))).collect()
}

// ---- reader ----------------------------------------------------------------------

struct R<'a> {
    s: Vec<&'a [u8]>,
    p: [usize; NSTREAMS],
}

type RResult<T> = std::result::Result<T, String>;

impl<'a> R<'a> {
    fn take(&mut self, st: usize, n: usize) -> RResult<&'a [u8]> {
        let p = self.p[st];
        let b = self.s[st].get(p..p + n).ok_or_else(|| format!("stream {st} truncated"))?;
        self.p[st] = p + n;
        Ok(b)
    }
    fn var(&mut self) -> RResult<u32> {
        let mut v = 0u32;
        let mut sh = 0;
        loop {
            let b = self.take(S_META, 1)?[0];
            if sh > 28 {
                return Err("varint overflow".into());
            }
            v |= u32::from(b & 0x7f) << sh;
            if b & 0x80 == 0 {
                return Ok(v);
            }
            sh += 7;
        }
    }
    fn count(&mut self, max: usize) -> RResult<usize> {
        let v = self.var()? as usize;
        if v > max {
            return Err(format!("implausible count {v}"));
        }
        Ok(v)
    }
    fn zz(&mut self) -> RResult<i32> {
        let v = self.var()?;
        Ok(((v >> 1) as i32) ^ -((v & 1) as i32))
    }
    fn u32(&mut self, st: usize) -> RResult<u32> {
        Ok(u32::from_be_bytes(self.take(st, 4)?.try_into().unwrap()))
    }
    fn i16(&mut self, st: usize) -> RResult<i16> {
        Ok(i16::from_be_bytes(self.take(st, 2)?.try_into().unwrap()))
    }
    fn u8(&mut self, st: usize) -> RResult<u8> {
        Ok(self.take(st, 1)?[0])
    }

    fn floats(&mut self, field: Field, n: usize) -> RResult<Vec<f32>> {
        if n == 0 {
            return Ok(Vec::new());
        }
        let mode = self.var()?;
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(match mode {
                MODE_I8 => from_255(i32::from(self.u8(S_NORMAL)? as i8)),
                MODE_U8 => from_255(i32::from(self.u8(S_COLOR)?)),
                MODE_I16 => f32::from(self.i16(S_VERT)?),
                m if (MODE_DEC0..MODE_DEC0 + 7).contains(&m) => from_dec(self.u32(dec_stream(field))? as i32, m - MODE_DEC0),
                MODE_RAW => f32::from_bits(self.u32(S_RAW)?),
                m => return Err(format!("bad float mode {m}")),
            });
        }
        Ok(v)
    }

    fn times(&mut self, n: usize) -> RResult<Vec<i32>> {
        let mut prev = 0i32;
        (0..n)
            .map(|_| {
                prev = prev.wrapping_add(self.u32(S_TIME)? as i32);
                Ok(prev)
            })
            .collect()
    }

    fn mesh(&mut self) -> RResult<Mesh> {
        let nv = self.count(1 << 20)?;
        let nf = self.count(1 << 20)?;
        let pres = self.var()?;
        let has = |b: u32| pres & (1 << b) != 0;
        let vertices = if has(0) { to3(&self.floats(Field::Vert, 3 * nv)?) } else { vec![] };
        let mut faces = Vec::new();
        if has(1) {
            let mut prev = 0i32;
            for _ in 0..nf {
                let mut f = [0i16; 3];
                for x in f.iter_mut() {
                    let z = u16::from_be_bytes(self.take(S_FACE, 2)?.try_into().unwrap());
                    let d = i32::from(z >> 1) ^ -i32::from(z & 1);
                    prev += d;
                    *x = prev as i16;
                }
                faces.push(f);
            }
        }
        let mut face_uv = Vec::new();
        if has(2) {
            for _ in 0..3 * nf {
                face_uv.push([self.i16(S_UV)?, self.i16(S_UV)?]);
            }
        }
        let normals = if has(3) { to3(&self.floats(Field::Normal, 3 * nv)?) } else { vec![] };
        let colors = if has(4) { self.floats(Field::Color, 12 * nf)?.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect() } else { vec![] };
        let face_flags = if has(5) { self.take(S_FFLAG, nf)?.to_vec() } else { vec![] };
        let mut textures = Vec::new();
        if has(6) {
            let np = self.count(1 << 20)?;
            let pal: Vec<u32> = (0..np).map(|_| self.u32(S_U32)).collect::<RResult<_>>()?;
            for _ in 0..nf {
                textures.push(if np <= 256 { *pal.get(self.u8(S_TEXIDX)? as usize).ok_or("bad texture index")? } else { self.u32(S_U32)? });
            }
        }
        Ok(Mesh { vertices, faces, face_uv, normals, colors, face_flags, textures })
    }

    fn object(&mut self, depth: u32) -> RResult<Object> {
        if depth > 256 {
            return Err("hierarchy too deep".into());
        }
        let id = self.u32(S_U32)?;
        let phong = self.u32(S_U32)?;
        let names = self.s[S_NAMES];
        let p = self.p[S_NAMES];
        let end = names[p.min(names.len())..].iter().position(|&b| b == 0).ok_or("name truncated")? + p;
        let name: String = names[p..end].iter().map(|&b| b as char).collect();
        self.p[S_NAMES] = end + 1;
        let xlu = self.var()? as u8;
        let flags = self.zz()? as i16;
        let nspr = self.count(1 << 16)?;
        let has_mesh = self.var()? != 0;
        let neff = self.count(1 << 20)?;
        let (ns, nm, nr) = (self.count(1 << 20)?, self.count(1 << 20)?, self.count(1 << 20)?);
        let has_bbox = self.var()? != 0;
        let ident = self.var()? != 0;
        let nchildren = self.count(1 << 16)?;
        let mut sprites = Vec::with_capacity(nspr);
        for _ in 0..nspr {
            let texture = self.u32(S_U32)?;
            let mut v = [0i16; 8];
            for x in v.iter_mut() {
                *x = self.i16(S_SPRITE)?;
            }
            sprites.push(ObjSprite { texture, pos: [v[0], v[1], v[2]], size: [v[3], v[4]], offset: [v[5], v[6]], flags: v[7] });
        }
        let mesh = if has_mesh { Some(self.mesh()?) } else { None };
        let mut prev = 0i32;
        let effected_verts = (0..neff)
            .map(|_| {
                prev = prev.wrapping_add(self.u32(S_IDX32)? as i32);
                Ok(prev)
            })
            .collect::<RResult<_>>()?;
        let mut vkeys = |n: usize| -> RResult<Vec<VKey>> {
            let v = self.floats(Field::VKey, 3 * n)?;
            let t = self.times(n)?;
            Ok((0..n).map(|i| VKey { v: [v[3 * i], v[3 * i + 1], v[3 * i + 2]], time: t[i] }).collect())
        };
        let scale_keys = vkeys(ns)?;
        let move_keys = vkeys(nm)?;
        let q = self.floats(Field::QKey, 4 * nr)?;
        let t = self.times(nr)?;
        let rotate_keys = (0..nr).map(|i| QKey { q: [q[4 * i], q[4 * i + 1], q[4 * i + 2], q[4 * i + 3]], time: t[i] }).collect();
        let bbox = if has_bbox {
            let b = to3(&self.floats(Field::Vert, 24)?);
            Some(b.try_into().map_err(|_| "bbox")?)
        } else {
            None
        };
        let matrix = if ident {
            IDENTITY
        } else {
            let m = self.floats(Field::Misc, 16)?;
            core::array::from_fn(|i| core::array::from_fn(|j| m[i * 4 + j]))
        };
        let children = (0..nchildren).map(|_| self.object(depth + 1)).collect::<RResult<_>>()?;
        Ok(Object { id, name, xlu, sprites, mesh, effected_verts, scale_keys, move_keys, rotate_keys, bbox, phong, flags, matrix, children })
    }

    fn anim(&mut self) -> RResult<Animation> {
        Ok(Animation { start: self.i16(S_ANIM)?, end: self.i16(S_ANIM)?, speed: f32::from_bits(self.u32(S_U32)?), looped: self.u32(S_U32)? as i32 })
    }

    fn model(&mut self) -> RResult<Model> {
        let id = self.u32(S_U32)?;
        let radius = f32::from_bits(self.u32(S_U32)?);
        let object_size = self.zz()? as i16;
        let is_skinned = self.var()? != 0;
        let num_vtx = self.zz()? as i16;
        let nanim = self.count(1 << 16)?;
        let has_loc = self.var()? != 0;
        let nsib = self.count(1 << 16)?;
        let animations = (0..nanim).map(|_| self.anim()).collect::<RResult<_>>()?;
        let root = self.object(0)?;
        let root_siblings = (0..nsib).map(|_| self.object(1)).collect::<RResult<_>>()?;
        let loc_mesh = if has_loc { Some(self.mesh()?) } else { None };
        Ok(Model { id, radius, object_size, root, root_siblings, animations, loc_mesh, is_skinned, num_vtx })
    }
}

fn to3(v: &[f32]) -> Vec<[f32; 3]> {
    v.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect()
}

/// Parses a packed bank back into the exact `ObjectBank`.
pub fn decode(data: &[u8]) -> RResult<ObjectBank> {
    let lens = stream_lens(data).ok_or("not an F2MB v1 bank")?;
    let mut off = 8 + 4 * NSTREAMS;
    let mut s = Vec::with_capacity(NSTREAMS);
    for &l in &lens {
        let e = off + l as usize;
        let next = off + (l as usize).next_multiple_of(4);
        s.push(data.get(off..e).ok_or("stream beyond data")?);
        off = next;
    }
    if off != data.len() {
        return Err("trailing bytes".into());
    }
    let mut r = R { s, p: [0; NSTREAMS] };
    let n = r.count(1 << 16)?;
    let models = (0..n).map(|_| r.model()).collect::<RResult<_>>()?;
    let na = r.count(1 << 16)?;
    let mut anim_past_end = Vec::with_capacity(na);
    for _ in 0..na {
        let id = r.u32(S_U32)?;
        let a = if r.var()? != 0 { Some(r.anim()?) } else { None };
        anim_past_end.push((id, a));
    }
    for i in 0..NSTREAMS {
        if r.p[i] != r.s[i].len() {
            return Err(format!("stream {i} not fully consumed"));
        }
    }
    Ok(ObjectBank { models, anim_past_end })
}

/// Bitwise equality of two banks (every float compared by bits).
pub fn bits_eq(a: &ObjectBank, b: &ObjectBank) -> bool {
    // The serialisation is canonical and float-exact, and Debug prints floats round-trip
    // exactly (including -0.0), so equal Debug strings ⇔ equal banks.
    format!("{:?}{:?}", a.models, a.anim_past_end) == format!("{:?}{:?}", b.models, b.anim_past_end)
}

/// Converts every object bank of the rendition.
pub fn convert(ctx: &Ctx) -> Result<(Vec<RawAsset>, Vec<String>)> {
    let dir = ctx.game().join("objects");
    let mut names: Vec<_> = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?.filter_map(|e| e.ok().map(|e| e.path())).collect();
    names.sort();
    let results = crate::tools::par_map(&names, ctx.jobs, |p: &std::path::PathBuf| -> Result<RawAsset> { convert_one(p) });
    let mut out = Vec::new();
    let (mut src, mut packed) = (0u64, 0u64);
    for r in results {
        let a = r?;
        src += a.source_bytes;
        packed += a.data.len() as u64;
        out.push(a);
    }
    let notes = vec![format!("models: {} banks, {} B of PE DLLs -> {} B packed (lossless, columnar) before compression", out.len(), src, packed)];
    Ok((out, notes))
}

fn convert_one(p: &Path) -> Result<RawAsset> {
    let d = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let bank = ObjectBank::parse(&d).map_err(|e| format!("{}: {e}", p.display()))?;
    let (data, segs) = encode(&bank);
    let back = decode(&data).map_err(|e| format!("{}: packed bank does not decode: {e}", p.display()))?;
    if !bits_eq(&bank, &back) {
        return Err(format!("{}: packed bank does not round-trip", p.display()));
    }
    let stem = p.file_stem().unwrap().to_string_lossy().to_lowercase();
    let nm: usize = bank.models.len();
    Ok(RawAsset {
        path: format!("models/{stem}.mdl"),
        class: Class::Models,
        data,
        source_bytes: d.len() as u64,
        notes: vec![format!("{nm} models")],
        segments: segs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(name: &str, mesh: Option<Mesh>, children: Vec<Object>) -> Object {
        Object {
            id: 7,
            name: name.into(),
            xlu: 255,
            sprites: vec![ObjSprite { texture: 0xdead_beef, pos: [1, -2, 3], size: [4, 5], offset: [-6, 7], flags: -1 }],
            mesh,
            effected_verts: vec![5, 3, 9, 100_000],
            scale_keys: vec![VKey { v: [1.0, 1.0, 1.0], time: 0 }, VKey { v: [0.892857, 1.5, -0.25], time: 10 }],
            move_keys: vec![VKey { v: [-3045.0, 2575.5, 0.000001], time: 0 }, VKey { v: [1e20, f32::MIN_POSITIVE, -0.0], time: 5 }],
            rotate_keys: vec![QKey { q: [0.0, -0.707107, 0.0, 0.707107], time: 3 }],
            bbox: Some([[1.0, -2.0, 3.0]; 8]),
            phong: 0x1234,
            flags: -32768,
            matrix: if name == "m" { [[0.5, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [1.0, 2.0, 3.0, 1.0]] } else { IDENTITY },
            children,
        }
    }

    fn mesh() -> Mesh {
        Mesh {
            vertices: vec![[-25.0, 0.0, 25.0], [6608.0, -6610.0, 1.0], [0.5, 2.0, 3.0]],
            faces: vec![[0, 1, 2], [2, 1, 0]],
            face_uv: vec![[0, 1024]; 6],
            normals: vec![[0.0, 0.498039, 0.0], [-0.164706, 0.329412, -0.329412], [0.0, 0.0, 1.0]],
            colors: vec![[1.0, 0.509804, 0.0, 0.8]; 6],
            face_flags: vec![1, 2],
            textures: vec![0xaaaa, 0xbbbb],
        }
    }

    #[test]
    fn synthetic_round_trip() {
        let m = Model {
            id: 0x1111,
            radius: 12.5,
            object_size: 3,
            root: obj(
                "root",
                Some(mesh()),
                vec![
                    obj("m", None, vec![]),
                    obj(
                        "c",
                        Some(Mesh { vertices: vec![], faces: vec![], face_uv: vec![], normals: vec![], colors: vec![], face_flags: vec![], textures: vec![] }),
                        vec![],
                    ),
                ],
            ),
            root_siblings: vec![obj("sib", None, vec![])],
            animations: vec![Animation { start: 0, end: 10, speed: 1.0, looped: 1 }],
            loc_mesh: Some(mesh()),
            is_skinned: true,
            num_vtx: 3,
        };
        let bank = ObjectBank {
            models: vec![m.clone(), m],
            anim_past_end: vec![(1, None), (2, Some(Animation { start: -1, end: 3, speed: f32::from_bits(1), looped: -7 }))],
        };
        let (data, segs) = encode(&bank);
        assert_eq!(segs.iter().map(|s| s.len as usize).sum::<usize>(), data.len());
        let back = decode(&data).unwrap();
        assert!(bits_eq(&bank, &back));
        assert_eq!(encode(&back).0, data);
        // -0.0 must survive (raw fallback)
        assert_eq!(back.models[0].root.move_keys[1].v[2].to_bits(), (-0.0f32).to_bits());
        // truncated input is an error, not a panic
        for n in [0, 10, data.len() / 2, data.len() - 1] {
            assert!(decode(&data[..n]).is_err());
        }
    }

    #[test]
    fn real_banks_round_trip() {
        let Some(root) = crate::ctx::find_rendition(Path::new(env!("CARGO_MANIFEST_DIR"))) else {
            eprintln!("skipped: no game data");
            return;
        };
        let dir = root.join("game/objects");
        let mut n = 0;
        for e in std::fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
            // Every bank: `convert_one` asserts the bitwise round trip.
            let a = convert_one(&p).unwrap();
            assert!(a.data.len() < a.source_bytes as usize, "{}", p.display());
            n += 1;
        }
        assert!(n > 60);
    }
}
