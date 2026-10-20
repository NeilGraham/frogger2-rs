//! The port's own materials for some of the original textures (Enhanced / Path-Traced
//! graphics): normal, height (parallax occlusion) and occlusion / roughness maps made at load
//! time from the player's textures ([`f2_formats::pbrgen`]), by name. The recipes are all
//! that is here (no game data). A mod's own maps for a texture replace these
//! (`f2_modkit::tex_materials`, registered first).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use f2_formats::material::{MatImage, ModelMaterials, PbrMaterial, MODEL_ANY};
use f2_formats::pbrgen::{generate, Maps, Recipe};

use crate::textures::TextureRegistry;

/// Size of the generated maps (the originals are 32² to 64²: the maps are drawn smooth).
const SIZE: u32 = 256;

const BRICK: Recipe =
    Recipe { plateau: 0.75, depth: 0.035, rough: 0.75, strength: 6.0, ao: 0.7, detail: 0.3, bevel: 1.2, seam: 0.15, min_seam: 3.0, invert: false };
const PAVING: Recipe =
    Recipe { plateau: 0.8, depth: 0.018, rough: 0.5, strength: 4.0, ao: 0.5, detail: 0.5, bevel: 0.8, seam: 0.15, min_seam: 12.0, invert: false };

/// Texture (its name up to the first `.`, lowercase) → recipe.
const RECIPES: &[(&str, Recipe)] = &[
    // Ancients: golden bricks, turquoise paving.
    ("brick1", BRICK),
    ("brick2", BRICK),
    ("brick3", BRICK),
    ("pave", PAVING),
    ("pave7", PAVING),
    ("pavecrack", PAVING),
    ("nupav", PAVING),
    ("nupav2", PAVING),
    ("nupav5", PAVING),
    ("nupav6", PAVING),
    ("nupav7", PAVING),
];

/// The recipe for texture `name`; `F2_PBR_TUNE=<name>:<settings>;...` changes it (tuning:
/// `pave:depth=0.02,bevel=0.1`, see [`Recipe::with`]).
fn recipe(name: &str) -> Option<Recipe> {
    let stem = name.split('.').next().unwrap_or(name).to_ascii_lowercase();
    let r = RECIPES.iter().find(|(n, _)| *n == stem).map(|(_, r)| *r)?;
    let tune = std::env::var("F2_PBR_TUNE").unwrap_or_default();
    Some(tune.split(';').filter_map(|t| t.split_once(':')).filter(|(n, _)| n.trim() == stem).fold(r, |r, (_, spec)| r.with(spec)))
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
    let mut todo: Vec<(u32, Recipe, &crate::textures::Image, String, Option<Arc<Maps>>)> = Vec::new();
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
            .map(|t| (t.0, t.2.width, t.2.height, sc.spawn(move || Arc::new(generate(&t.2.rgba, t.2.width, t.2.height, SIZE, &t.1)))))
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
    for (crc, r, img, name, _) in &todo {
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
        });
    }
    (!m.materials.is_empty()).then_some(m)
}
