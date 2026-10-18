//! Light setups of the modern renderer per world (`Graphics::Enhanced`; `f2_engine::lighting`).
//!
//! The original bakes its lighting into the vertex colours; these add a sun or moon (with
//! shadows) and a sky / ground ambient that suit each world's art. Emissive surfaces and
//! sprites (lava, fire) add their own point lights everywhere.
//!
//! The game's cameras mostly look from +z down at the level, and the screen mirrors x (world
//! +x is screen left): suns come from +x (screen left) and +z (the camera's side), so the
//! faces the player sees are lit and shadows fall away to the right.

use f2_engine::lighting::normalize;
use f2_engine::{DirLight, Lighting};

fn setup(sky: [f32; 3], ground: [f32; 3], to_light: [f32; 3], sun: [f32; 3], specular: f32) -> Lighting {
    Lighting {
        sky,
        ground,
        sun: Some(DirLight { to_light: normalize(to_light), color: sun, shadows: true }),
        point_lights: Vec::new(),
        emissive_lights: true,
        specular,
        water: true,
        baked: 0.35,
    }
}

/// The lights of world `world`, level `level` (`player[0].world_num` / `level_num`).
pub fn for_level(world: u32, level: u32) -> Lighting {
    let _ = level;
    // Balanced so a surface facing up comes out a little brighter than the original (~1.2),
    // one facing the sun side-on about as bright, one facing away a little darker.
    match world {
        // Garden: warm late-afternoon sun.
        0 => setup([0.82, 0.84, 0.88], [0.60, 0.58, 0.52], [0.5, 0.75, 0.45], [0.66, 0.58, 0.46], 0.14),
        // Ancients: high, hot desert sun, warm bounce from the sand.
        1 => setup([0.78, 0.78, 0.80], [0.68, 0.61, 0.52], [0.3, 0.9, 0.3], [0.62, 0.55, 0.42], 0.16),
        // Space: hard white starlight, blue fill.
        2 => setup([0.68, 0.72, 0.86], [0.50, 0.50, 0.60], [0.6, 0.65, 0.45], [0.70, 0.70, 0.76], 0.28),
        // Subterranean: under rock (no sun to cast shadows); the lava does the lighting.
        4 => Lighting { sun: None, ..setup([0.96, 0.94, 0.96], [0.80, 0.74, 0.70], [0.0, 1.0, 0.0], [0.0; 3], 0.0) },
        // Laboratory: cool overhead strip lights.
        5 => setup([0.82, 0.86, 0.90], [0.64, 0.66, 0.68], [0.1, 1.0, 0.15], [0.42, 0.46, 0.50], 0.30),
        // Halloween: blue moonlight.
        6 => setup([0.74, 0.76, 0.88], [0.56, 0.54, 0.62], [0.45, 0.7, 0.4], [0.46, 0.52, 0.72], 0.22),
        // Retro, the hub, multiplayer: neutral daylight.
        _ => Lighting::daylight(),
    }
}
