//! Object banks for the N64 runtime with the RSP renderer (phase 3I), replacing the F2MB
//! `models/<bank>.mdl` of phase 1C in the default pack:
//!
//! | pack path | content |
//! |---|---|
//! | `objects/<bank>.dll.pcd` | the [`ObjectBank`], postcard, with mesh geometry reduced to the **vertices** (all the simulation reads: `FindActorCentre`); hierarchy, keys, flags, sprites, animations, bounding boxes exact |
//! | `objects/<bank>.dll.f2nm` | the bank's render meshes in F2NM (`f2_n64fmt::bank`: quantised positions, per-corner colours/UVs, RSP command lists) |
//!
//! The runtime decodes the `.pcd` twice (simulation model table; renderer hierarchy, which
//! drops the vertices too) and keeps the `.f2nm` resident while the level runs.

use std::path::Path;

use f2_formats::objbank::{Mesh, Object, ObjectBank};

use crate::{Class, Ctx, RawAsset, Result};

/// The bank with every mesh reduced to its vertices (faces, UVs, normals, colours, face
/// flags and texture CRCs emptied; `Some(Mesh)` kept).
pub fn sim_bank(bank: &ObjectBank) -> ObjectBank {
    fn strip(o: &mut Object) {
        if let Some(m) = o.mesh.as_mut() {
            *m = Mesh {
                vertices: std::mem::take(&mut m.vertices),
                faces: Vec::new(),
                face_uv: Vec::new(),
                normals: Vec::new(),
                colors: Vec::new(),
                face_flags: Vec::new(),
                textures: Vec::new(),
            };
        }
        for c in &mut o.children {
            strip(c);
        }
    }
    let mut b = bank.clone();
    for m in &mut b.models {
        strip(&mut m.root);
        for s in &mut m.root_siblings {
            strip(s);
        }
        m.loc_mesh = None;
    }
    b
}

pub fn convert(ctx: &Ctx) -> Result<(Vec<RawAsset>, Vec<String>)> {
    let dir = ctx.game().join("objects");
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll")))
        .collect();
    names.sort();
    let results = crate::tools::par_map(&names, ctx.jobs, |p: &std::path::PathBuf| convert_one(p));
    let mut out = Vec::new();
    let (mut src, mut pcd, mut f2nm, mut tris) = (0u64, 0u64, 0u64, 0u64);
    for r in results {
        let (a, b, t) = r?;
        src += a.source_bytes;
        pcd += a.data.len() as u64;
        f2nm += b.data.len() as u64;
        tris += t as u64;
        out.push(a);
        out.push(b);
    }
    let notes = vec![format!(
        "models: {} banks, {src} B of PE DLLs -> {pcd} B postcard (vertex-only geometry) + {f2nm} B F2NM render meshes ({tris} triangles) before compression",
        out.len() / 2
    )];
    Ok((out, notes))
}

fn convert_one(p: &Path) -> Result<(RawAsset, RawAsset, u32)> {
    let d = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let bank = ObjectBank::parse(&d).map_err(|e| format!("{}: {e}", p.display()))?;
    let pcd = postcard::to_allocvec(&sim_bank(&bank)).map_err(|e| e.to_string())?;
    // The landscape (`world.obe`) goes into world space with the key-0 matrices the game draws
    // it with (`Renderer::draw_landscape`), so its meshes share one position grid (no cracks).
    let world = f2_formats::crc("world.obe");
    let landscape_mats = bank
        .models
        .iter()
        .find(|m| m.id == world && !m.is_skinned)
        .map(|m| f2_engine::xform_actor(m, &mut f2_engine::Actor { animated: false, ..Default::default() }));
    let (f2nm, st) = f2_n64fmt::bank::encode_bank_with(&bank, landscape_mats.as_deref());
    let name = p.file_name().unwrap().to_string_lossy().to_lowercase();
    let mk = |suffix: &str, data: Vec<u8>, src: u64, note: String| RawAsset {
        path: format!("objects/{name}{suffix}"),
        class: Class::Models,
        data,
        source_bytes: src,
        notes: vec![note],
        segments: Vec::new(),
    };
    Ok((
        mk(".pcd", pcd, d.len() as u64, format!("{} models", bank.models.len())),
        mk(".f2nm", f2nm, 0, format!("{} meshes, {} triangles, {} vertices", st.meshes, st.tris, st.vertices)),
        st.tris,
    ))
}

/// Text size on the N64 relative to the PC layout: 240 lines make the original sizes hard to
/// read on a TV, so the modern fonts are drawn this much larger (centred on the same spot).
pub const N64_FONT_MUL: f32 = 1.25;

/// The `InitFont` strips the N64 front end draws text with (`textures/font/*.bmp`, the BMPs
/// unchanged: the runtime scans the glyphs like the PC and blits RGBA16), stored next to the
/// NTX1 textures. Only the three fonts the game uses: big/small (`GameFonts`) and mono (the
/// story book).
pub fn fonts(ctx: &Ctx) -> Result<(Vec<RawAsset>, Vec<String>)> {
    let mut out = Vec::new();
    for n in ["bigfont.bmp", "smallfont.bmp", "monofont.bmp"] {
        let p = ctx.game().join("textures/font").join(n);
        let data = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        out.push(RawAsset {
            path: format!("textures/font/{n}"),
            class: Class::Textures,
            source_bytes: data.len() as u64,
            data,
            notes: Vec::new(),
            segments: Vec::new(),
        });
    }
    // Modern fonts (`f2-fontgen`, built from the user's Gill Sans Ultra Bold when installed):
    // the 6x atlases resampled to the N64's on-screen size (320 px = half the PC's 640-wide
    // font scale) times `N64_FONT_MUL`, raw RGBA8 (the pack compresses it).
    let mut modern = String::from("modern fonts: none (typeface not found)");
    match f2_fontgen::ensure(&ctx.rendition) {
        Ok(Some(log)) => log.iter().for_each(|l| eprintln!("f2-n64pack: {l}")),
        Ok(None) => {}
        Err(e) => eprintln!("f2-n64pack: modern fonts: {e}"),
    }
    if f2_fontgen::present(&ctx.rendition) {
        let mut sizes = Vec::new();
        for (_, f2mf, _) in f2_fontgen::FONTS {
            let p = ctx.game().join("textures/font").join(f2mf);
            let src = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            let hr = f2_engine::HiRes::decode(&src).map_err(|e| format!("{}: {e}", p.display()))?.resampled(0.5 * N64_FONT_MUL);
            sizes.push(format!("{}x{}", hr.width, hr.height));
            let stem = f2mf.trim_end_matches(".f2mf");
            out.push(RawAsset {
                path: format!("textures/font/{stem}.f2mr"),
                class: Class::Textures,
                source_bytes: src.len() as u64,
                data: hr.encode_raw(),
                notes: Vec::new(),
                segments: Vec::new(),
            });
        }
        modern = format!("modern fonts {} at {N64_FONT_MUL}x size", sizes.join(" + "));
    }
    // The procedural-texture sources of each texture bank (`pt_texture_load` reads only the
    // `prc*` textures of the banks a level loads): a few KiB instead of the whole NTX bank.
    let dir = ctx.game().join("textures");
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("fla")))
        .collect();
    names.sort();
    let mut nprc = 0;
    for p in names {
        let raw = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let Ok(data) = f2_formats::fla::maybe_decompress(raw) else { continue };
        let Ok(bank) = f2_formats::texture::TexBank::parse(&data) else { continue };
        let textures: Vec<_> = bank.textures.into_iter().filter(|t| t.name.as_bytes().starts_with(b"prc")).collect();
        if textures.is_empty() {
            continue;
        }
        nprc += textures.len();
        let data = postcard::to_allocvec(&f2_formats::texture::TexBank { textures, anims: Vec::new() }).map_err(|e| e.to_string())?;
        let stem = p.file_stem().unwrap().to_string_lossy().to_lowercase();
        out.push(RawAsset { path: format!("textures/{stem}.prc"), class: Class::Textures, source_bytes: 0, data, notes: Vec::new(), segments: Vec::new() });
    }
    Ok((
        out,
        vec![format!(
            "fonts: bigfont/smallfont/monofont BMPs for the runtime's glyph scan; {modern}; {nprc} procedural-texture sources in textures/<bank>.prc"
        )],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The simulation's model table is identical from the vertex-only bank.
    #[test]
    fn sim_bank_keeps_what_the_simulation_reads() {
        let Some(r) = crate::ctx::find_rendition(Path::new(env!("CARGO_MANIFEST_DIR"))) else { return };
        let d = std::fs::read(r.join("game/objects/garden1.dll")).unwrap();
        let bank = ObjectBank::parse(&d).unwrap();
        let s: ObjectBank = postcard::from_bytes(&postcard::to_allocvec(&sim_bank(&bank)).unwrap()).unwrap();
        assert_eq!(s.models.len(), bank.models.len());
        for (a, b) in s.models.iter().zip(&bank.models) {
            assert_eq!((a.id, a.radius, a.is_skinned, a.animations.len()), (b.id, b.radius, b.is_skinned, b.animations.len()));
            assert_eq!(a.root.mesh.as_ref().map(|m| &m.vertices), b.root.mesh.as_ref().map(|m| &m.vertices));
            assert_eq!(a.root.rotate_keys, b.root.rotate_keys);
        }
    }
}
