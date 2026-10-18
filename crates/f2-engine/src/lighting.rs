//! Dynamic lighting of the modern renderer (enhanced, GPU meshes; `docs/LIGHTING.md`).
//!
//! The original has no lights: its lighting is baked into the vertex colours. The modern
//! renderer keeps those colours as the base and adds, per pixel: a hemisphere ambient (sky /
//! ground), one directional light (sun or moon) with a shadow map, and point lights, both
//! authored ([`Lighting::point_lights`]) and found automatically on emissive surfaces (lava,
//! fire, flames, bulbs: [`is_emissive`]), which are themselves drawn unlit. The terms are
//! balanced around 1 so a level keeps its art direction: a surface facing the light gets a
//! little brighter than the original, one facing away a little darker.

#[cfg(not(feature = "std"))]
use crate::fmath::FloatExt;
use crate::prelude::*;

/// One directional light (sun or moon).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DirLight {
    /// Unit vector from a surface towards the light (world space, +Y up).
    pub to_light: [f32; 3],
    /// Colour × intensity added for a surface facing the light.
    pub color: [f32; 3],
    /// Casts a shadow map.
    pub shadows: bool,
}

/// A point light (world space).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointLight {
    pub pos: [f32; 3],
    pub color: [f32; 3],
    /// Distance at which it fades to nothing (world units).
    pub radius: f32,
}

/// The scene's lights ([`crate::Renderer::lighting`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Lighting {
    /// Ambient from above and from below (blended by the surface's facing).
    pub sky: [f32; 3],
    pub ground: [f32; 3],
    pub sun: Option<DirLight>,
    pub point_lights: Vec<PointLight>,
    /// Point lights at emissive surfaces and sprites ([`is_emissive`]).
    pub emissive_lights: bool,
    /// Strength of the sun's highlight (0 = none).
    pub specular: f32,
    /// The enhanced water (`crate::water`): water meshes drawn on the GPU with waves objects
    /// make, MODGE surfaces on the GPU, caustics under the water.
    pub water: bool,
    /// How much of the shading baked into the vertex colours to keep (0 = none: the dynamic
    /// lights alone shade; 1 = all, the original's look under the lights). Hue and near-black
    /// (pits, fade-outs) are always kept.
    pub baked: f32,
}

impl Lighting {
    /// A neutral daylight setup.
    pub fn daylight() -> Lighting {
        Lighting {
            sky: [0.82, 0.84, 0.86],
            ground: [0.60, 0.58, 0.56],
            sun: Some(DirLight { to_light: normalize([0.45, 0.8, 0.4]), color: [0.60, 0.56, 0.50], shadows: true }),
            point_lights: Vec::new(),
            emissive_lights: true,
            specular: 0.12,
            water: true,
            baked: 0.35,
        }
    }
}

pub fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-6 {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        [0.0, 1.0, 0.0]
    }
}

/// Whether a texture (by file name) glows: lava, fire, flames, bulbs, burning surfaces.
pub fn is_emissive(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    // Not: star fields and UI stars, grass "light".
    ["lava", "fire", "flame", "glow", "burn", "bulb", "lairfl", "ember", "candle", "neon", "lamp"].iter().any(|k| n.contains(k))
}

/// Whether a texture (by file name) is a water surface: rivers, ponds, swamp water, sludge.
pub fn is_water(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    let wat_n = n.strip_prefix("wat").is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit()));
    wat_n
        || ["ai_water", "ai_ripple", "00wate", "00nwat", "aniswamp", "aqua0", "aquatop", "ripple", "sludge", "swampi", "swampyi"]
            .iter()
            .any(|k| n.starts_with(k))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emissive_names() {
        for n in ["00lava04.bm", "fire0000.bm", "flame3.bmp", "glowtex.bmp", "labburn.bmp", "chipbulb1.b", "lairfl2.bmp", "prc_fire1.b", "lavafall.bm"] {
            assert!(is_emissive(n), "{n}");
        }
        for n in ["star1.bmp", "grasslight1", "wat0.bmp", "aqua001.bmp", "start2.bmp"] {
            assert!(!is_emissive(n), "{n}");
        }
    }

    #[test]
    fn water_names() {
        for n in ["00wate04.bm", "wat10.bmp", "ai_water.bm", "aniswamp03.", "aqua001.bmp", "sludge.bmp", "swampyi02.b", "ripple.bmp"] {
            assert!(is_water(n), "{n}");
        }
        for n in ["watdrop.bmp", "swampy.bmp", "aquatil001.", "riverbottom", "00lava04.bm"] {
            assert!(!is_water(n), "{n}");
        }
    }
}
