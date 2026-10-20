//! The port's own materials for some of the original textures (Enhanced / Path-Traced
//! graphics): normal, height (parallax occlusion) and occlusion / roughness maps made at load
//! time from the player's textures ([`f2_formats::pbrgen`]), by name, and how each is tiled
//! (smart tiling, [`Tiling`]: every slab of the walkways its own; drawn by `crate::pbr`). The
//! recipes are all that is here (no game data). A mod's own maps for a texture replace these
//! (`f2_modkit::tex_materials`, registered first).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use f2_formats::material::{MatImage, ModelMaterials, PbrMaterial, Tiling, MODEL_ANY};
use f2_formats::pbrgen::{generate, Maps, Recipe};

use crate::textures::TextureRegistry;

/// Size of the generated maps (the originals are 32² to 64²: the maps are drawn smooth).
const SIZE: u32 = 256;

const BRICK: Recipe =
    Recipe { plateau: 0.75, depth: 0.035, rough: 0.75, strength: 6.0, ao: 0.7, detail: 0.3, bevel: 1.2, seam: 0.15, min_seam: 3.0, invert: false, tile: 0.0 };
const PAVING: Recipe =
    Recipe { plateau: 0.8, depth: 0.018, rough: 0.5, strength: 4.0, ao: 0.5, detail: 0.5, bevel: 0.8, seam: 0.15, min_seam: 12.0, invert: false, tile: 0.0 };
/// One slab of paving per texture (a dark rim all round): a straight bevel all round, the
/// art's own mottling on the face.
const SLAB: Recipe =
    Recipe { plateau: 0.82, depth: 0.04, rough: 0.62, strength: 4.0, ao: 0.6, detail: 0.6, bevel: 1.4, seam: 0.0, min_seam: 3.0, invert: false, tile: 0.9 };
/// A slab with a pattern cut into it (the Ancients' meander tiles): the slab's bevel, the
/// pattern's relief from the art's light and dark (its grooves are too broken up for seams).
const CARVED: Recipe =
    Recipe { plateau: 0.45, depth: 0.03, rough: 0.62, strength: 6.0, ao: 0.6, detail: 0.4, bevel: 1.0, seam: 0.0, min_seam: 3.0, invert: false, tile: 0.9 };

/// Paving laid by hand and walked on for centuries: every slab turned at random, a little
/// tilted and off-colour, some chipped or cracked, the walkway's edges worn.
const LAID: Tiling = Tiling { variants: 2, turn_color: true, wear: 0.6, tint: 0.5, uneven: 0.6, edges: 0.7, grime: 0.6 };
/// The same with the pattern kept the way the art has it.
const LAID_CARVED: Tiling = Tiling { variants: 0, turn_color: false, wear: 0.4, tint: 0.3, uneven: 0.4, edges: 0.6, grime: 0.5 };
/// Every repeat the same.
const NONE: Tiling = Tiling { variants: 0, turn_color: false, wear: 0.0, tint: 0.0, uneven: 0.0, edges: 0.0, grime: 0.0 };

/// Texture (its name up to the first `.`, lowercase) → recipe and smart tiling.
const RECIPES: &[(&str, Recipe, Tiling)] = &[
    // Ancients: golden bricks, turquoise paving (a repeat of these holds many bricks or
    // flags: tiling them by repeat would show the texture's grid).
    ("brick1", BRICK, NONE),
    ("brick2", BRICK, NONE),
    ("brick3", BRICK, NONE),
    ("pave", PAVING, NONE),
    ("pave7", PAVING, NONE),
    ("pavecrack", PAVING, NONE),
    // Ancients: the walkways' sandstone and turquoise slabs.
    ("nupav", SLAB, LAID),
    ("nupav2", SLAB, LAID),
    ("nupav3", CARVED, LAID_CARVED),
    ("nupav4", CARVED, LAID_CARVED),
    ("nupav5", SLAB, LAID),
    ("nupav6", SLAB, LAID),
    ("nupav7", SLAB, LAID),
];

/// The recipe and tiling for texture `name`; `F2_PBR_TUNE=<name>:<settings>;...` changes them
/// (tuning: `nupav2:depth=0.02,bevel=0.1,wear=0.8`, see [`Recipe::with`]; the tiling's
/// `variants`, `turn_color`, `wear`, `tint`, `uneven`, `edges`, `grime` too).
fn recipe(name: &str) -> Option<(Recipe, Tiling)> {
    let stem = name.split('.').next().unwrap_or(name).to_ascii_lowercase();
    let (_, r, t) = RECIPES.iter().find(|(n, _, _)| *n == stem).copied()?;
    let tune = std::env::var("F2_PBR_TUNE").unwrap_or_default();
    Some(
        tune.split(';')
            .filter_map(|t| t.split_once(':'))
            .filter(|(n, _)| n.trim() == stem)
            .fold((r, t), |(r, t), (_, spec)| (r.with(spec), tune_tiling(t, spec))),
    )
}

/// `t` with `key=value` settings from `spec` (as [`Recipe::with`]).
fn tune_tiling(mut t: Tiling, spec: &str) -> Tiling {
    for (k, v) in spec.split(',').filter_map(|kv| kv.split_once('=')) {
        let Ok(x) = v.trim().parse::<f32>() else { continue };
        match k.trim() {
            "variants" => t.variants = x as u8,
            "turn_color" => t.turn_color = x != 0.0,
            "wear" => t.wear = x,
            "tint" => t.tint = x,
            "uneven" => t.uneven = x,
            "edges" => t.edges = x,
            "grime" => t.grime = x,
            _ => {}
        }
    }
    t
}

/// Generated maps by texture CRC and image size (made once per run).
fn cache() -> &'static Mutex<HashMap<(u32, u32, u32), Arc<Maps>>> {
    static C: OnceLock<Mutex<HashMap<(u32, u32, u32), Arc<Maps>>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// The materials for the registry's textures that have a recipe and no material yet (a mod's
/// win). `F2_NO_BUILTIN_PBR=1` turns them off.
pub(crate) fn materials(reg: &TextureRegistry) -> Option<ModelMaterials> {
    if std::env::var_os("F2_NO_BUILTIN_PBR").is_some() {
        return None;
    }
    // The textures to do (with their maps from the cache, or to make: side by side).
    let mut todo: Vec<(u32, (Recipe, Tiling), &crate::textures::Image, String, Option<Arc<Maps>>)> = Vec::new();
    for e in &reg.entries {
        let Some(r) = recipe(&e.name) else { continue };
        if reg.pbr(MODEL_ANY, e.crc).is_some() {
            continue;
        }
        let Some(img) = reg.images.get(e.image.0 as usize) else { continue };
        if img.rgba.len() != (img.width * img.height * 4) as usize {
            continue;
        }
        let cached = if std::env::var_os("F2_PBR_TUNE").is_some() { None } else { cache().lock().unwrap().get(&(e.crc, img.width, img.height)).cloned() };
        todo.push((e.crc, r, img, e.name.clone(), cached));
    }
    std::thread::scope(|sc| {
        let made: Vec<_> = todo
            .iter()
            .filter(|t| t.4.is_none())
            .map(|t| (t.0, t.2.width, t.2.height, sc.spawn(move || Arc::new(generate(&t.2.rgba, t.2.width, t.2.height, SIZE, &t.1 .0)))))
            .collect();
        for (crc, w, h, j) in made {
            cache().lock().unwrap().insert((crc, w, h), j.join().expect("material maps"));
        }
        // Debug: `F2_PBR_DUMP=<dir>` writes the source and the maps as PNGs.
        if let Some(dir) = std::env::var_os("F2_PBR_DUMP").map(std::path::PathBuf::from) {
            for t in &todo {
                let Some(m) = cache().lock().unwrap().get(&(t.0, t.2.width, t.2.height)).cloned() else { continue };
                let stem = t.3.split('.').next().unwrap_or(&t.3).to_string();
                let img = |w, h, px: &Vec<u8>| crate::RgbaImage { width: w, height: h, pixels: px.clone() };
                let _ = img(t.2.width, t.2.height, &t.2.rgba).write_png(dir.join(format!("{stem}.png")));
                for (k, px) in [("height", &m.height), ("normal", &m.normal), ("orm", &m.orm)] {
                    let _ = img(SIZE, SIZE, px).write_png(dir.join(format!("{stem}_{k}.png")));
                }
            }
        }
    });
    let mut m = ModelMaterials { model_id: MODEL_ANY, ..Default::default() };
    for (crc, (r, tiling), img, name, _) in &todo {
        let Some(maps) = cache().lock().unwrap().get(&(*crc, img.width, img.height)).cloned() else { continue };
        let base = m.images.len() as u32;
        for (kind, px) in [("_normal", &maps.normal), ("_height", &maps.height), ("_orm", &maps.orm)] {
            m.images.push(MatImage { name: format!("{name}{kind}"), width: SIZE, height: SIZE, rgba: px.clone() });
        }
        m.materials.push(PbrMaterial {
            crc: *crc,
            base_color: [1.0; 4],
            metallic: 1.0,
            roughness: 1.0,
            normal_scale: 1.0,
            occlusion_strength: 1.0,
            emissive: [0.0; 3],
            parallax_scale: r.depth,
            normal: Some(base),
            metal_rough: Some(base + 2),
            occlusion: Some(base + 2),
            emissive_map: None,
            height: Some(base + 1),
            tiling: *tiling,
        });
    }
    (!m.materials.is_empty()).then_some(m)
}
