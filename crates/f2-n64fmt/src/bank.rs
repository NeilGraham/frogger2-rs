//! MDX object banks (`f2_formats::objbank`) → F2NM meshes, and the runtime side of that
//! mapping (feature `objbank`).
//!
//! One F2NM file per object bank holds a mesh for every drawable MDX mesh of the bank (no
//! textures: faces reference textures by CRC, resolved by the renderer's texture registry).
//! Mesh ids encode where the mesh came from, so the N64 front end can find the mesh of a
//! recorded draw (`f2_engine::MeshDraw { model, object }`) without names:
//!
//! `id = model_index << 16 | object` — `model_index` = position in `ObjectBank::models`,
//! `object` = pre-order object index (`for_each_object`: root subtree, then each root
//! sibling's subtree), or [`SKIN`] for a skinned model's root mesh (deformed by every bone).
//!
//! Face selection matches `PCRenderObject`: faces without a texture (CRC 0) are dropped
//! (a face whose texture is not loaded at run time is skipped by the renderer, which finds
//! no texture for its group), as are faces with out-of-range indices and, for skins, faces
//! using a vertex no bone moves. Texture addressing (`wrap`) and face order (`ordered`)
//! follow the frame list `DrawObject` picks from the object's flags; the landscape
//! (`world.obe`, always drawn in the normal list) is clamped and unordered.

use crate::encode::{encode_mesh, FaceIn, FileBuilder, MeshIn};
use alloc::vec;
use alloc::vec::Vec;
use f2_formats::objbank::{Mesh, Model, Object, ObjectBank};

/// `object` part of a mesh id for a skinned model's root mesh.
pub const SKIN: u16 = 0xFFFF;

/// Mesh id of model `model_index`, object `object` (or [`SKIN`]).
pub fn mesh_id(model_index: usize, object: u16) -> u32 {
    ((model_index as u32) << 16) | object as u32
}

/// `(model_index, object)` of a mesh id.
pub fn split_id(id: u32) -> (usize, u16) {
    ((id >> 16) as usize, id as u16)
}

/// Visits every object in pre-order (as `f2_engine::anim::for_each_object`).
pub fn for_each_object<'a>(model: &'a Model, mut f: impl FnMut(usize, &'a Object)) {
    fn walk<'a>(o: &'a Object, idx: &mut usize, f: &mut dyn FnMut(usize, &'a Object)) {
        f(*idx, o);
        *idx += 1;
        for c in &o.children {
            walk(c, idx, f);
        }
    }
    let mut i = 0;
    walk(&model.root, &mut i, &mut f);
    for s in &model.root_siblings {
        walk(s, &mut i, &mut f);
    }
}

// MDX object flags (f2_engine::scene).
const OBJECT_FLAGS_XLU: i16 = 1 << 1;
const OBJECT_FLAGS_MODGE: i16 = 1 << 5;
const OBJECT_FLAGS_ADDITIVE: i16 = 1 << 6;
const OBJECT_FLAGS_GLOW: i16 = 1 << 14;

/// `(wrap, ordered)` of an object drawn with `flags` (`object_frame`: MODGE → Wrap,
/// XLU → Xlu, ADDITIVE → Additive lists all wrap; the translucent ones keep face order).
fn addressing(flags: i16) -> (bool, bool) {
    let xlu = flags & (OBJECT_FLAGS_XLU | OBJECT_FLAGS_ADDITIVE) != 0;
    (xlu || flags & OBJECT_FLAGS_MODGE != 0, xlu)
}

/// Corner colour of a face drawn additively on the N64: the renderer blends `src * a` with
/// the alpha blender from the texture's additive variant (colour / max, alpha * max), so the
/// vertex colour is split the same way (`rgb / max`, `a * max`) and the product equals the
/// PC's `tex * diffuse * alpha` additive term.
pub fn additive_corner(c: [u8; 4]) -> [u8; 4] {
    let m = c[0].max(c[1]).max(c[2]) as u32;
    if m == 0 {
        return [0, 0, 0, 0];
    }
    let n = |v: u8| ((v as u32 * 255 + m / 2) / m) as u8;
    [n(c[0]), n(c[1]), n(c[2]), ((c[3] as u32 * m + 127) / 255) as u8]
}

fn faces_of(mesh: &Mesh, bone_of: Option<&[u8]>) -> Vec<FaceIn> {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let mut out = Vec::new();
    for (fi, f) in mesh.faces.iter().enumerate() {
        let crc = mesh.textures.get(fi).copied().unwrap_or(0);
        if crc == 0 || f.iter().any(|&i| i < 0 || i as usize >= mesh.vertices.len()) {
            continue;
        }
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

/// Models whose actors the game draws additively whatever their object flags:
/// `ada*` (`CreateAndAddActor`, actor2.rs), `wat_*` water and `xxa_*` scenics (pc_banks.rs).
pub fn additive_by_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("ada") || n.starts_with("wat_") || n.starts_with("xxa_")
}

/// Statistics of [`encode_bank`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BankStats {
    pub meshes: usize,
    pub tris: u32,
    pub vertices: usize,
    pub skinned: usize,
}

/// The F2NM file (meshes only) of an object bank.
pub fn encode_bank(bank: &ObjectBank) -> (Vec<u8>, BankStats) {
    encode_bank_with(bank, None)
}

/// [`encode_bank`]; with `landscape_mats` (the `world.obe` model's key-0 object matrices,
/// pre-order, row-vector `[x y z 1] * M`) the landscape is stored in world space on one
/// position grid ([`mesh_flags::WORLD`](crate::mesh_flags::WORLD)): shared edges between its
/// meshes quantise identically, so no cracks open between them.
pub fn encode_bank_with(bank: &ObjectBank, landscape_mats: Option<&[[[f32; 4]; 4]]>) -> (Vec<u8>, BankStats) {
    let world = f2_formats::crc("world.obe");
    let mut fb = FileBuilder::new();
    let mut st = BankStats::default();
    for (mi, model) in bank.models.iter().enumerate() {
        let landscape = model.id == world;
        let xf = |m: &[[f32; 4]; 4], v: [f32; 3]| -> [f32; 3] { core::array::from_fn(|k| v[0] * m[0][k] + v[1] * m[1][k] + v[2] * m[2][k] + m[3][k]) };
        // World-space landscape: every mesh's positions through its object's matrix, one grid.
        let world_pos: Option<(alloc::vec::Vec<alloc::vec::Vec<[f32; 3]>>, i8)> = match (landscape && !model.is_skinned, landscape_mats) {
            (true, Some(mats)) => {
                let mut per: alloc::vec::Vec<alloc::vec::Vec<[f32; 3]>> = alloc::vec::Vec::new();
                let mut maxabs = 0f32;
                for_each_object(model, |i, o| {
                    while per.len() <= i {
                        per.push(alloc::vec::Vec::new());
                    }
                    if let (Some(mesh), Some(m)) = (o.mesh.as_ref(), mats.get(i)) {
                        per[i] = mesh.vertices.iter().map(|&v| xf(m, v)).collect();
                        maxabs = per[i].iter().flat_map(|p| p.iter()).fold(maxabs, |a, &v| a.max(v.abs()));
                    }
                });
                Some((per, crate::encode::pos_shift_for(maxabs)))
            }
            _ => None,
        };
        if model.is_skinned {
            let Some(mesh) = model.root.mesh.as_ref() else { continue };
            let mut bone_of = vec![u8::MAX; mesh.vertices.len()];
            for_each_object(model, |i, o| {
                for &j in &o.effected_verts {
                    if let Some(b) = bone_of.get_mut(j as usize) {
                        *b = i.min(254) as u8;
                    }
                }
            });
            let faces = faces_of(mesh, Some(&bone_of));
            if faces.is_empty() {
                continue;
            }
            let em = encode_mesh(&MeshIn {
                id: mesh_id(mi, SKIN),
                positions: &mesh.vertices,
                faces: &faces,
                bone_of: Some(&bone_of),
                ordered: false,
                wrap: false,
                world_shift: None,
            });
            st.skinned += 1;
            st.tris += em.n_tris;
            st.vertices += em.vertices.len();
            fb.meshes.push(em);
        } else {
            for_each_object(model, |i, o| {
                let Some(mesh) = o.mesh.as_ref() else { return };
                let faces = faces_of(mesh, None);
                if faces.is_empty() || i >= SKIN as usize {
                    return;
                }
                let (wrap, ordered) = if landscape { (false, false) } else { addressing(o.flags) };
                // Additive objects: flagged in the bank, or whole models the game makes
                // additive by name when it creates their actors.
                let mut faces = faces;
                if !landscape && o.flags & OBJECT_FLAGS_GLOW == 0 && (o.flags & OBJECT_FLAGS_ADDITIVE != 0 || additive_by_name(model.name())) {
                    for f in &mut faces {
                        f.rgba = f.rgba.map(additive_corner);
                    }
                }
                let (positions, world_shift) = match &world_pos {
                    Some((per, shift)) if per.get(i).is_some_and(|p| p.len() == mesh.vertices.len()) => (&per[i][..], Some(*shift)),
                    _ => (&mesh.vertices[..], None),
                };
                let em = encode_mesh(&MeshIn { id: mesh_id(mi, i as u16), positions, faces: &faces, bone_of: None, ordered, wrap, world_shift });
                st.tris += em.n_tris;
                st.vertices += em.vertices.len();
                fb.meshes.push(em);
            });
        }
    }
    st.meshes = fb.meshes.len();
    (fb.finish(), st)
}

/// Drops the render geometry of a bank the N64 draws from F2NM meshes (vertices, faces,
/// UVs, normals, colours, textures; `Some(Mesh)` stays so "has a mesh" is unchanged). The
/// hierarchy, keys, flags and sprites (what `xform_actor` and the draw loop read) are kept.
pub fn strip_geometry(bank: &mut ObjectBank) {
    for m in &mut bank.models {
        strip_model(m);
    }
}

/// [`strip_geometry`] of one model.
pub fn strip_model(m: &mut Model) {
    fn strip(o: &mut Object) {
        if let Some(m) = o.mesh.as_mut() {
            *m = Mesh {
                vertices: Vec::new(),
                faces: Vec::new(),
                face_uv: Vec::new(),
                normals: Vec::new(),
                colors: Vec::new(),
                face_flags: Vec::new(),
                textures: Vec::new(),
            };
        }
        o.effected_verts = Vec::new();
        for c in &mut o.children {
            strip(c);
        }
    }
    strip(&mut m.root);
    for s in &mut m.root_siblings {
        strip(s);
    }
    m.loc_mesh = None;
}

/// Whether any face of the model uses one of the texture CRCs.
pub fn uses_texture(m: &Model, crcs: &[u32]) -> bool {
    let mut hit = false;
    for_each_object(m, |_, o| {
        if let Some(mesh) = &o.mesh {
            hit |= mesh.textures.iter().any(|t| crcs.contains(t));
        }
    });
    hit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        assert_eq!(split_id(mesh_id(3, 7)), (3, 7));
        assert_eq!(split_id(mesh_id(70, SKIN)), (70, SKIN));
    }

    #[test]
    fn additive_corner_split() {
        assert_eq!(additive_corner([255, 128, 0, 200]), [255, 128, 0, 200]);
        // Dark olive at full alpha: bright colour, alpha = brightness.
        let c = additive_corner([64, 60, 30, 255]);
        assert_eq!(c, [255, 239, 120, 64]);
        assert_eq!(additive_corner([0, 0, 0, 255]), [0; 4]);
    }

    #[test]
    fn addressing_follows_frames() {
        assert_eq!(addressing(0), (false, false));
        assert_eq!(addressing(OBJECT_FLAGS_MODGE), (true, false));
        assert_eq!(addressing(OBJECT_FLAGS_XLU), (true, true));
        assert_eq!(addressing(OBJECT_FLAGS_ADDITIVE), (true, true));
    }
}
