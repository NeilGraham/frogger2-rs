//! Encoders: MDX mesh → batched RSP mesh, RGBA8 image → TMEM texture, and the file writer.
//!
//! Used by the asset packer (host), the test-scene exporter and the on-console benchmarks
//! (the module needs `alloc` only).

use crate::*;
use alloc::vec;
use alloc::vec::Vec;

// ------------------------------------------------------------------------------ meshes

/// One input face (an MDX `faceIndex` triangle with its per-corner attributes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceIn {
    /// Indices into [`MeshIn::positions`].
    pub v: [u16; 3],
    /// MDX `faceTC` per corner (texture coordinate = value x 0.000975).
    pub uv: [[i16; 2]; 3],
    /// Per-corner RGBA (the PC's `D3DRGBA`-clamped vertex colour).
    pub rgba: [[u8; 4]; 3],
    /// Texture CRC (0 = none: such faces are never drawn by `PCRenderObject` and should be
    /// dropped by the caller).
    pub tex_crc: u32,
}

/// One input mesh.
#[derive(Clone, Copy, Debug)]
pub struct MeshIn<'a> {
    pub id: u32,
    pub positions: &'a [[f32; 3]],
    pub faces: &'a [FaceIn],
    /// Skinned meshes: the bone (pre-order object index) that moves each position.
    pub bone_of: Option<&'a [u8]>,
    /// Keep faces in their original order (translucent meshes).
    pub ordered: bool,
    /// The mesh is drawn with wrapped texture addressing (MDX `Wrap`/`Xlu`/`Additive` frame
    /// lists): out-of-range texture coordinates may be shifted by whole periods. Clamped
    /// meshes (`Normal` list: landscape, most objects, skins) keep their coordinates.
    pub wrap: bool,
}

/// An encoded mesh, ready for [`FileBuilder`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EncodedMesh {
    pub id: u32,
    pub flags: u16,
    pub pos_shift: i8,
    pub n_bones: u8,
    pub n_tris: u32,
    pub vertices: Vec<Vertex>,
    pub groups: Vec<Group>,
    pub batches: Vec<Batch>,
    pub cmds: Vec<u32>,
}

/// Largest stored |coordinate| the position quantiser aims for: balances the positions'
/// own rounding (`2^-pos_shift`) against the s15.16 matrix rounding (which grows with the
/// stored magnitude).
const POS_TARGET: f32 = 8192.0;

fn pos_shift_for(maxabs: f32) -> i8 {
    if maxabs <= 0.0 || maxabs.is_nan() {
        return 0;
    }
    let mut s: i32 = 0;
    while s < 12 && maxabs * pow2(s + 1) <= POS_TARGET {
        s += 1;
    }
    while s > -8 && maxabs * pow2(s) > 32767.0 {
        s -= 1;
    }
    s as i8
}

fn pow2(s: i32) -> f32 {
    if s >= 0 {
        (1u32 << s) as f32
    } else {
        1.0 / (1u32 << -s) as f32
    }
}

fn round_i32(v: f32) -> i32 {
    if v >= 0.0 {
        (v + 0.5) as i32
    } else {
        (v - 0.5) as i32
    }
}

/// Texture coordinates of a face in stored units, brought into range. Wrapped meshes: faces
/// whose coordinates exceed ±[`ST_MAX_PERIODS`] are shifted by whole periods (exact for
/// wrapping) and clamped to that range. Clamped meshes: coordinates are only saturated to
/// `i16` (a shift would change what clamping shows; the RSP saturates the texel coordinate).
fn face_st(f: &FaceIn, wrap: bool) -> [[i16; 2]; 3] {
    let lim = if wrap { ST_MAX_PERIODS * ST_ONE } else { i16::MAX as i32 };
    let mut st = [[0i32; 2]; 3];
    for k in 0..3 {
        for a in 0..2 {
            st[k][a] = mdx_uv_to_st(f.uv[k][a]);
        }
    }
    for a in 0..2 {
        let (lo, hi) = (st.iter().map(|c| c[a]).min().unwrap(), st.iter().map(|c| c[a]).max().unwrap());
        if wrap && (lo < -lim || hi > lim) {
            let shift = lo.div_euclid(ST_ONE) * ST_ONE;
            for c in &mut st {
                c[a] -= shift;
            }
        }
    }
    st.map(|c| c.map(|v| v.clamp(-lim, lim) as i16))
}

/// Bounding sphere (box centre, radius rounded up) of a batch's vertices, stored units.
fn bounding_sphere(verts: &[Vertex]) -> ([i16; 3], u16) {
    let mut lo = [i32::MAX; 3];
    let mut hi = [i32::MIN; 3];
    for v in verts {
        for k in 0..3 {
            lo[k] = lo[k].min(v.pos[k] as i32);
            hi[k] = hi[k].max(v.pos[k] as i32);
        }
    }
    let c: [i32; 3] = core::array::from_fn(|k| (lo[k] + hi[k]) >> 1);
    let mut r2 = 0i64;
    for v in verts {
        let d: i64 = (0..3).map(|k| ((v.pos[k] as i32 - c[k]) as i64).pow(2)).sum();
        r2 = r2.max(d);
    }
    let mut r = 0i64;
    while r * r < r2 {
        r += 1 + r / 2;
    }
    while r > 0 && (r - 1) * (r - 1) >= r2 {
        r -= 1;
    }
    (c.map(|v| v as i16), (r + 1).min(65535) as u16)
}

/// Encodes a mesh: quantises positions/UVs, splits faces into texture groups and each group
/// into vertex-cache batches of at most [`CACHE_SLOTS`] unique vertices (a vertex = unique
/// position + colour + UV, per batch contiguous in the vertex array so one `Load` per batch
/// (per bone for skinned meshes) fills the cache).
pub fn encode_mesh(m: &MeshIn) -> EncodedMesh {
    let used = |i: usize| m.faces.iter().any(|f| f.v.contains(&(i as u16)));
    let maxabs =
        m.positions.iter().enumerate().filter(|(i, _)| m.positions.len() < 4096 || used(*i)).flat_map(|(_, p)| p.iter()).fold(0f32, |a, &v| a.max(v.abs()));
    let pos_shift = pos_shift_for(maxabs);
    let scale = pow2(pos_shift as i32);
    let qpos: Vec<[i16; 3]> = m.positions.iter().map(|p| p.map(|v| round_i32(v * scale).clamp(-32768, 32767) as i16)).collect();
    let bone = |pi: u16| m.bone_of.and_then(|b| b.get(pi as usize).copied()).unwrap_or(0);

    // Texture groups (face index lists).
    let mut groups: Vec<(u32, Vec<usize>)> = Vec::new();
    for (fi, f) in m.faces.iter().enumerate() {
        if m.ordered {
            match groups.last_mut() {
                Some((crc, list)) if *crc == f.tex_crc => list.push(fi),
                _ => groups.push((f.tex_crc, vec![fi])),
            }
        } else if let Some((_, list)) = groups.iter_mut().find(|(crc, _)| *crc == f.tex_crc) {
            list.push(fi);
        } else {
            groups.push((f.tex_crc, vec![fi]));
        }
    }

    let mut out = EncodedMesh {
        id: m.id,
        flags: if m.bone_of.is_some() { mesh_flags::SKINNED } else { 0 } | if m.ordered { mesh_flags::ORDERED } else { 0 },
        pos_shift,
        n_bones: m.bone_of.map_or(0, |b| b.iter().copied().max().map_or(0, |x| x.saturating_add(1))),
        ..Default::default()
    };
    for (crc, faces) in &groups {
        let cmd_first = out.cmds.len() as u32;
        let batch_first = out.batches.len() as u16;
        let mut n_tris = 0u32;
        let mut i = 0;
        while i < faces.len() {
            // Grow the batch while its unique vertices fit the cache.
            let mut verts: Vec<Vertex> = Vec::new();
            let mut tris: Vec<[usize; 3]> = Vec::new();
            while i < faces.len() {
                let f = &m.faces[faces[i]];
                let st = face_st(f, m.wrap);
                let mut idx = [0usize; 3];
                let mut add: Vec<Vertex> = Vec::new();
                for k in 0..3 {
                    let v = Vertex { pos: qpos[f.v[k] as usize], bone: bone(f.v[k]), rgba: f.rgba[k], st: st[k] };
                    idx[k] = match verts.iter().chain(add.iter()).position(|x| *x == v) {
                        Some(p) => p,
                        None => {
                            add.push(v);
                            verts.len() + add.len() - 1
                        }
                    };
                }
                if verts.len() + add.len() > CACHE_SLOTS {
                    break;
                }
                verts.extend(add);
                tris.push(idx);
                i += 1;
            }
            // Slot order: grouped by bone (stable) so each bone's vertices are one Load.
            let mut order: Vec<usize> = (0..verts.len()).collect();
            if m.bone_of.is_some() {
                order.sort_by_key(|&k| verts[k].bone);
            }
            let mut slot_of = vec![0u8; verts.len()];
            for (slot, &k) in order.iter().enumerate() {
                slot_of[k] = slot as u8;
            }
            let batch_cmd_first = out.cmds.len() as u32;
            let base = out.vertices.len();
            assert!(base + verts.len() <= 65535, "mesh {:08x}: more than 65535 vertices", m.id);
            out.vertices.extend(order.iter().map(|&k| verts[k]));
            let mut s = 0;
            while s < order.len() {
                let b = verts[order[s]].bone;
                let mut e = s + 1;
                while e < order.len() && (m.bone_of.is_none() || verts[order[e]].bone == b) {
                    e += 1;
                }
                if m.bone_of.is_some() {
                    out.cmds.push(Cmd::Bone(b).encode());
                }
                out.cmds.push(Cmd::Load { first: (base + s) as u16, count: (e - s) as u8, slot: s as u8 }.encode());
                s = e;
            }
            for t in &tris {
                out.cmds.push(Cmd::Tri(t.map(|k| slot_of[k])).encode());
            }
            n_tris += tris.len() as u32;
            let (center, radius) = bounding_sphere(&verts);
            out.batches.push(Batch {
                cmd_first: batch_cmd_first,
                cmd_count: (out.cmds.len() as u32 - batch_cmd_first) as u16,
                n_tris: tris.len() as u16,
                center,
                radius,
            });
        }
        out.groups.push(Group {
            tex_crc: *crc,
            cmd_first,
            cmd_count: (out.cmds.len() as u32 - cmd_first) as u16,
            n_tris: n_tris as u16,
            batch_first,
            batch_count: out.batches.len() as u16 - batch_first,
        });
        out.n_tris += n_tris;
    }
    out
}

// ---------------------------------------------------------------------------- textures

/// An RGBA8 source image.
#[derive(Clone, Copy, Debug)]
pub struct TexIn<'a> {
    pub crc: u32,
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, R G B A.
    pub rgba: &'a [u8],
}

/// An encoded texture, ready for [`FileBuilder`].
#[derive(Clone, Debug, PartialEq)]
pub struct EncodedTexture {
    pub crc: u32,
    pub fmt: TexFormat,
    pub flags: u8,
    pub width: u16,
    pub height: u16,
    pub src_width: u16,
    pub src_height: u16,
    /// RGBA16 palette entries.
    pub tlut: Vec<u16>,
    pub texels: Vec<u8>,
}

fn prev_pow2(v: u32) -> u32 {
    if v <= 1 {
        1
    } else {
        1 << (31 - v.leading_zeros())
    }
}

/// Target size: powers of two not above the source, halved (larger side first) until at
/// most `max_texels`, at least `min_w` wide.
fn fit_size(w: u32, h: u32, max_texels: u32, min_w: u32) -> (u32, u32) {
    let (mut tw, mut th) = (prev_pow2(w).max(min_w), prev_pow2(h).max(1));
    while tw * th > max_texels {
        if (tw >= th && tw > min_w) || th == 1 {
            tw /= 2;
        } else {
            th /= 2;
        }
    }
    (tw.max(min_w), th)
}

/// Box-filtered resample (alpha-weighted colour). With `hard_alpha`, alpha is re-thresholded
/// at 128 (keeps colour-keyed edges crisp).
fn resample(src: &TexIn, tw: u32, th: u32, hard_alpha: bool) -> Vec<[u8; 4]> {
    let (sw, sh) = (src.width.max(1), src.height.max(1));
    let mut out = Vec::with_capacity((tw * th) as usize);
    for y in 0..th {
        let (y0, y1) = (y * sh / th, ((y + 1) * sh).div_ceil(th).max(y * sh / th + 1));
        for x in 0..tw {
            let (x0, x1) = (x * sw / tw, ((x + 1) * sw).div_ceil(tw).max(x * sw / tw + 1));
            let (mut c, mut a, mut n) = ([0u32; 3], 0u32, 0u32);
            for sy in y0..y1.min(sh) {
                for sx in x0..x1.min(sw) {
                    let i = ((sy * sw + sx) * 4) as usize;
                    let p = &src.rgba[i..i + 4];
                    for k in 0..3 {
                        c[k] += p[k] as u32 * p[3] as u32;
                    }
                    a += p[3] as u32;
                    n += 1;
                }
            }
            let n = n.max(1);
            let rgb = if a > 0 { c.map(|v| ((v + a / 2) / a) as u8) } else { [0; 3] };
            let mut al = ((a + n / 2) / n) as u8;
            if hard_alpha {
                al = if al >= 128 { 255 } else { 0 };
            }
            out.push([rgb[0], rgb[1], rgb[2], al]);
        }
    }
    out
}

/// RGBA8 → RGBA16 5551 (1-bit alpha = `a >= 128`).
pub fn rgba16(p: [u8; 4]) -> u16 {
    let q = |v: u8| ((v as u32 * 31 + 127) / 255) as u16;
    q(p[0]) << 11 | q(p[1]) << 6 | q(p[2]) << 1 | (p[3] >= 128) as u16
}

/// Converts an image to the best-fitting TMEM format (lossless palettes when the 5551
/// colour count allows; IA16 / RGBA32 for smooth alpha). Basic reference converter: the
/// asset packer may choose formats with its own error budget.
pub fn encode_texture(t: &TexIn) -> EncodedTexture {
    let n = (t.width * t.height) as usize;
    let px = |i: usize| [t.rgba[i * 4], t.rgba[i * 4 + 1], t.rgba[i * 4 + 2], t.rgba[i * 4 + 3]];
    let has_alpha = (0..n).any(|i| px(i)[3] < 255);
    let smooth_count = (0..n).filter(|&i| (1..255).contains(&px(i)[3])).count();
    let smooth = smooth_count * 100 > n.max(1);
    let grey = (0..n).all(|i| {
        let p = px(i);
        p[3] == 0 || (p[0].abs_diff(p[1]) <= 8 && p[1].abs_diff(p[2]) <= 8)
    });
    let flags = if has_alpha { tex_flags::HAS_ALPHA } else { 0 } | if smooth { tex_flags::SMOOTH_ALPHA } else { 0 };
    let mk = |fmt: TexFormat, w: u32, h: u32, tlut: Vec<u16>, texels: Vec<u8>| EncodedTexture {
        crc: t.crc,
        fmt,
        flags,
        width: w as u16,
        height: h as u16,
        src_width: t.width as u16,
        src_height: t.height as u16,
        tlut,
        texels,
    };
    if smooth {
        if grey {
            let (w, h) = fit_size(t.width, t.height, 2048, 4);
            let img = resample(t, w, h, false);
            let texels = img.iter().flat_map(|p| [((p[0] as u32 + p[1] as u32 + p[2] as u32) / 3) as u8, p[3]]).collect();
            return mk(TexFormat::Ia16, w, h, Vec::new(), texels);
        }
        let (w, h) = fit_size(t.width, t.height, 1024, 2);
        let img = resample(t, w, h, false);
        return mk(TexFormat::Rgba32, w, h, Vec::new(), img.iter().flat_map(|p| *p).collect());
    }
    // Palette candidates: CI4 at up to 4096 texels, else CI8 / RGBA16 at 2048.
    for (fmt, max_texels, max_colours, min_w) in [(TexFormat::Ci4, 4096, 16, 16), (TexFormat::Ci8, 2048, 256, 8)] {
        let (w, h) = fit_size(t.width, t.height, max_texels, min_w);
        let img: Vec<u16> = resample(t, w, h, true).into_iter().map(rgba16).collect();
        let mut pal: Vec<u16> = Vec::new();
        let mut ok = true;
        for &c in &img {
            if !pal.contains(&c) {
                if pal.len() == max_colours {
                    ok = false;
                    break;
                }
                pal.push(c);
            }
        }
        if !ok {
            continue;
        }
        let index = |c: u16| pal.iter().position(|&p| p == c).unwrap() as u8;
        let texels: Vec<u8> = if fmt == TexFormat::Ci4 {
            img.chunks(2).map(|p| index(p[0]) << 4 | p.get(1).map_or(0, |&c| index(c))).collect()
        } else {
            img.iter().map(|&c| index(c)).collect()
        };
        pal.resize(max_colours, 0);
        return mk(fmt, w, h, pal, texels);
    }
    let (w, h) = fit_size(t.width, t.height, 2048, 4);
    let img = resample(t, w, h, true);
    mk(TexFormat::Rgba16, w, h, Vec::new(), img.into_iter().flat_map(|p| rgba16(p).to_be_bytes()).collect())
}

/// Decodes an encoded texture back to RGBA8 (tests, previews).
pub fn decode_texture(fmt: TexFormat, w: u32, h: u32, tlut: &[u16], texels: &[u8]) -> Vec<[u8; 4]> {
    let c16 = |c: u16| {
        let e = |v: u16| ((v as u32 * 255 + 15) / 31) as u8;
        [e(c >> 11 & 31), e(c >> 6 & 31), e(c >> 1 & 31), if c & 1 != 0 { 255 } else { 0 }]
    };
    (0..(w * h) as usize)
        .map(|i| match fmt {
            TexFormat::Rgba16 => c16(u16::from_be_bytes([texels[2 * i], texels[2 * i + 1]])),
            TexFormat::Ci8 => c16(tlut[texels[i] as usize]),
            TexFormat::Ci4 => c16(tlut[(texels[i / 2] >> if i % 2 == 0 { 4 } else { 0 } & 15) as usize]),
            TexFormat::Ia16 => [texels[2 * i], texels[2 * i], texels[2 * i], texels[2 * i + 1]],
            TexFormat::Ia8 => {
                let b = texels[i];
                [(b >> 4) * 17, (b >> 4) * 17, (b >> 4) * 17, (b & 15) * 17]
            }
            TexFormat::I8 => [texels[i]; 4],
            TexFormat::I4 => {
                let v = (texels[i / 2] >> if i % 2 == 0 { 4 } else { 0 } & 15) * 17;
                [v; 4]
            }
            TexFormat::Rgba32 => [texels[4 * i], texels[4 * i + 1], texels[4 * i + 2], texels[4 * i + 3]],
        })
        .collect()
}

// -------------------------------------------------------------------------------- file

/// Collects textures and meshes and writes an F2NM file.
#[derive(Default)]
pub struct FileBuilder {
    pub textures: Vec<EncodedTexture>,
    pub meshes: Vec<EncodedMesh>,
}

fn align8(v: &mut Vec<u8>) {
    while !v.len().is_multiple_of(8) {
        v.push(0);
    }
}

impl FileBuilder {
    pub fn new() -> FileBuilder {
        FileBuilder::default()
    }

    pub fn finish(&self) -> Vec<u8> {
        let tex_table = HEADER_SIZE;
        let mesh_table = tex_table + self.textures.len() * TEXTURE_ENTRY_SIZE;
        let mut out = vec![0u8; mesh_table + self.meshes.len() * MESH_ENTRY_SIZE];
        align8(&mut out);
        for (i, m) in self.meshes.iter().enumerate() {
            align8(&mut out);
            let vert_off = out.len();
            for v in &m.vertices {
                let mut b = [0u8; VERTEX_SIZE];
                v.write(&mut b);
                out.extend_from_slice(&b);
            }
            align8(&mut out);
            let group_off = out.len();
            for g in &m.groups {
                let mut b = [0u8; GROUP_SIZE];
                g.write(&mut b);
                out.extend_from_slice(&b);
            }
            align8(&mut out);
            let batch_off = out.len();
            for b in &m.batches {
                let mut bb = [0u8; BATCH_SIZE];
                b.write(&mut bb);
                out.extend_from_slice(&bb);
            }
            align8(&mut out);
            let cmd_off = out.len();
            for c in &m.cmds {
                out.extend_from_slice(&c.to_be_bytes());
            }
            let e = MeshEntry {
                id: m.id,
                flags: m.flags,
                pos_shift: m.pos_shift,
                n_bones: m.n_bones,
                n_verts: m.vertices.len() as u16,
                n_groups: m.groups.len() as u16,
                n_tris: m.n_tris,
                vert_off: vert_off as u32,
                group_off: group_off as u32,
                cmd_off: cmd_off as u32,
                n_cmds: m.cmds.len() as u32,
                batch_off: batch_off as u32,
                n_batches: m.batches.len() as u32,
            };
            let o = mesh_table + i * MESH_ENTRY_SIZE;
            e.write(&mut out[o..o + MESH_ENTRY_SIZE]);
        }
        for (i, t) in self.textures.iter().enumerate() {
            align8(&mut out);
            let tlut_off = out.len();
            for c in &t.tlut {
                out.extend_from_slice(&c.to_be_bytes());
            }
            align8(&mut out);
            let texel_off = out.len();
            out.extend_from_slice(&t.texels);
            let e = TextureEntry {
                crc: t.crc,
                fmt: t.fmt,
                flags: t.flags,
                width: t.width,
                height: t.height,
                src_width: t.src_width,
                src_height: t.src_height,
                tlut_count: t.tlut.len() as u16,
                tlut_off: if t.tlut.is_empty() { 0 } else { tlut_off as u32 },
                texel_off: texel_off as u32,
            };
            let o = tex_table + i * TEXTURE_ENTRY_SIZE;
            e.write(&mut out[o..o + TEXTURE_ENTRY_SIZE]);
        }
        align8(&mut out);
        let h = Header {
            version: VERSION,
            flags: 0,
            n_textures: self.textures.len() as u16,
            n_meshes: self.meshes.len() as u16,
            tex_table: tex_table as u32,
            mesh_table: mesh_table as u32,
            size: out.len() as u32,
        };
        h.write(&mut out[..HEADER_SIZE]);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(n: usize, tex: impl Fn(usize) -> u32) -> (Vec<[f32; 3]>, Vec<FaceIn>) {
        let mut pos = Vec::new();
        for y in 0..=n {
            for x in 0..=n {
                pos.push([x as f32 * 10.0, 0.0, y as f32 * 10.0]);
            }
        }
        let mut faces = Vec::new();
        let id = |x: usize, y: usize| (y * (n + 1) + x) as u16;
        for y in 0..n {
            for x in 0..n {
                let uv = |x: usize, y: usize| [(x * 1026 / n) as i16, (y * 1026 / n) as i16];
                let c = [255, 255, 255, 255];
                faces.push(FaceIn {
                    v: [id(x, y), id(x + 1, y), id(x + 1, y + 1)],
                    uv: [uv(x, y), uv(x + 1, y), uv(x + 1, y + 1)],
                    rgba: [c; 3],
                    tex_crc: tex(faces.len()),
                });
                faces.push(FaceIn {
                    v: [id(x, y), id(x + 1, y + 1), id(x, y + 1)],
                    uv: [uv(x, y), uv(x + 1, y + 1), uv(x, y + 1)],
                    rgba: [c; 3],
                    tex_crc: tex(faces.len()),
                });
            }
        }
        (pos, faces)
    }

    /// Replays the command lists with a simulated vertex cache and checks every face comes
    /// out with its original corners.
    fn check(m: &MeshIn, e: &EncodedMesh) {
        let mut seen = Vec::new();
        for g in &e.groups {
            let mut cache = [None::<Vertex>; CACHE_SLOTS + 1];
            for i in g.cmd_first..g.cmd_first + g.cmd_count as u32 {
                match Cmd::decode(e.cmds[i as usize]).unwrap() {
                    Cmd::Load { first, count, slot } => {
                        assert!(count >= 1 && slot as usize + count as usize <= CACHE_SLOTS);
                        for k in 0..count as usize {
                            cache[slot as usize + k] = Some(e.vertices[first as usize + k]);
                        }
                        if count % 2 == 1 {
                            cache[slot as usize + count as usize] = None; // odd loads clobber one slot
                        }
                    }
                    Cmd::Tri(t) => seen.push((g.tex_crc, t.map(|s| cache[s as usize].expect("slot loaded").pos))),
                    Cmd::Bone(_) => {}
                }
            }
        }
        assert_eq!(seen.len(), m.faces.len());
        let s = pow2(e.pos_shift as i32);
        for f in m.faces {
            let want = f.v.map(|i| m.positions[i as usize].map(|v| round_i32(v * s) as i16));
            let k = seen.iter().position(|(c, p)| *c == f.tex_crc && *p == want).expect("face emitted");
            seen.swap_remove(k);
        }
    }

    #[test]
    fn mesh_batches_cover_all_faces() {
        let (pos, faces) = grid(16, |i| [11, 22, 33][i % 3]);
        let m = MeshIn { id: 1, positions: &pos, faces: &faces, bone_of: None, ordered: false, wrap: false };
        let e = encode_mesh(&m);
        assert_eq!(e.groups.len(), 3);
        assert_eq!(e.n_tris, 512);
        check(&m, &e);
        let ordered = MeshIn { ordered: true, ..m };
        let eo = encode_mesh(&ordered);
        assert_eq!(eo.groups.len(), 512);
        check(&ordered, &eo);
    }

    #[test]
    fn batch_spheres_contain_their_vertices() {
        let (pos, faces) = grid(16, |i| [11, 22][i % 2]);
        let e = encode_mesh(&MeshIn { id: 3, positions: &pos, faces: &faces, bone_of: None, ordered: false, wrap: false });
        for g in &e.groups {
            let bs = &e.batches[g.batch_first as usize..(g.batch_first + g.batch_count) as usize];
            assert_eq!(bs.iter().map(|b| b.n_tris as u32).sum::<u32>(), g.n_tris as u32);
            assert_eq!(bs[0].cmd_first, g.cmd_first);
            for b in bs {
                for i in b.cmd_first..b.cmd_first + b.cmd_count as u32 {
                    if let Some(Cmd::Load { first, count, .. }) = Cmd::decode(e.cmds[i as usize]) {
                        for v in &e.vertices[first as usize..(first + count as u16) as usize] {
                            let d2: i64 = (0..3).map(|k| ((v.pos[k] - b.center[k]) as i64).pow(2)).sum();
                            assert!(d2 <= (b.radius as i64).pow(2), "{v:?} outside {b:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn skinned_loads_per_bone() {
        let (pos, faces) = grid(8, |_| 5);
        let bones: Vec<u8> = (0..pos.len()).map(|i| (i % 3) as u8).collect();
        let m = MeshIn { id: 2, positions: &pos, faces: &faces, bone_of: Some(&bones), ordered: false, wrap: false };
        let e = encode_mesh(&m);
        assert_eq!(e.n_bones, 3);
        check(&m, &e);
        // Each Load is preceded by the Bone of its vertices.
        let mut cur = None;
        for &w in &e.cmds {
            match Cmd::decode(w).unwrap() {
                Cmd::Bone(b) => cur = Some(b),
                Cmd::Load { first, count, .. } => {
                    for k in 0..count as usize {
                        assert_eq!(Some(e.vertices[first as usize + k].bone), cur);
                    }
                }
                Cmd::Tri(_) => {}
            }
        }
    }

    #[test]
    fn st_out_of_range_shifted_by_periods() {
        let f = FaceIn { v: [0, 1, 2], uv: [[10260, 0], [11286, 0], [10260, 1026]], rgba: [[255; 4]; 3], tex_crc: 1 };
        let st = face_st(&f, true);
        assert!((0..ST_ONE as i16).contains(&st[0][0]), "{st:?}");
        assert_eq!(st[1][0] - st[0][0], (mdx_uv_to_st(11286) - mdx_uv_to_st(10260)) as i16);
        assert_eq!(st[2][1], mdx_uv_to_st(1026) as i16);
    }

    #[test]
    fn pos_shift_balances() {
        assert_eq!(pos_shift_for(3000.0), 1);
        assert_eq!(pos_shift_for(30.0), 8);
        assert_eq!(pos_shift_for(60000.0), -1);
    }

    #[test]
    fn file_roundtrip() {
        let (pos, faces) = grid(4, |_| 0xABCD);
        let mesh = encode_mesh(&MeshIn { id: 9, positions: &pos, faces: &faces, bone_of: None, ordered: false, wrap: false });
        let rgba: Vec<u8> = (0..32 * 32).flat_map(|i| if i % 2 == 0 { [255, 0, 0, 255] } else { [0, 0, 255, 0] }).collect();
        let tex = encode_texture(&TexIn { crc: 0xABCD, width: 32, height: 32, rgba: &rgba });
        assert_eq!(tex.fmt, TexFormat::Ci4);
        let dec = decode_texture(tex.fmt, tex.width as u32, tex.height as u32, &tex.tlut, &tex.texels);
        assert_eq!(dec[0], [255, 0, 0, 255]);
        assert_eq!(dec[1][3], 0);
        let mut b = FileBuilder::new();
        b.meshes.push(mesh.clone());
        b.textures.push(tex.clone());
        let bytes = b.finish();
        let f = File::parse(&bytes).unwrap();
        assert_eq!(f.n_meshes(), 1);
        let mv = f.mesh(0).unwrap();
        assert_eq!(mv.entry.n_tris, 32);
        assert_eq!(mv.vertex(3), mesh.vertices[3]);
        assert_eq!(mv.groups().count(), 1);
        let g = mv.group(0);
        assert_eq!(mv.group_cmds(&g).count(), g.cmd_count as usize);
        let te = f.texture(0).unwrap();
        assert_eq!(te.crc, 0xABCD);
        assert_eq!(f.texels(&te), &tex.texels[..]);
        assert_eq!(f.tlut(&te).len(), 32);
    }

    #[test]
    fn smooth_alpha_and_large_textures() {
        let h = |i: u32, k: u32| ((i ^ (k << 20)).wrapping_mul(2654435761).wrapping_mul(k * 2 + 1) >> 24) as u8;
        let rgba: Vec<u8> = (0..64 * 64u32).flat_map(|i| [h(i, 1), h(i, 2), h(i, 3), 255]).collect();
        let t = encode_texture(&TexIn { crc: 1, width: 64, height: 64, rgba: &rgba });
        assert_eq!(t.fmt, TexFormat::Rgba16);
        assert!(t.width as u32 * t.height as u32 <= 2048);
        let glow: Vec<u8> = (0..64 * 64).flat_map(|i| [255, 255, 255, (i % 256) as u8]).collect();
        let g = encode_texture(&TexIn { crc: 2, width: 64, height: 64, rgba: &glow });
        assert_eq!(g.fmt, TexFormat::Ia16);
    }
}
