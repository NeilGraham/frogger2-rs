//! wgpu renderer replacing the MDX library: models, textures, sprites, overlays, fonts.
//!
//! Architecture (faithful to MDX): the CPU transforms and projects model
//! vertices exactly like `mdxRender.cpp` (row-vector matrices, `FOV = 450`,
//! `DIST = -10`, integer-truncated depth reciprocal, `sz = vz * 0.00025`),
//! rejects faces the same way, and pushes screen-space triangles into the
//! `MA_FRAME_*` lists (`scene`). The GPU (`renderer`) draws those lists with
//! the D3D7 render states of `DrawAllFrames`, then 2D overlays and text.
//!
//! World conventions: right-handed, Y up, original float units of the banks.

pub mod anim;
pub mod font;
pub mod load;
pub mod math;
pub mod renderer;
pub mod scene;
pub mod textures;

pub use anim::{actor_bounds, xform_actor, Actor};
pub use font::Font;
pub use renderer::{Blend, EngineError, FontId, GpuContext, Quad2D, Renderer, RgbaImage, WindowSurface, OVERLAY_SPACE};
pub use scene::{Camera, Fog, Frame, RenderOptions, TlVertex, Tri, View, XluSort};
pub use textures::{TexHandle, TextureRegistry};

/// Finds a model in a bank by root object name (case-insensitive, with or
/// without `.obe`), by `CRC("<name>.obe")`, or by decimal index.
pub fn find_model<'a>(bank: &'a f2_formats::objbank::ObjectBank, key: &str) -> Option<&'a f2_formats::objbank::Model> {
    let base = key.trim_end_matches(".obe").trim_end_matches(".psi");
    let crc = f2_formats::crc(&format!("{}.obe", base.to_ascii_lowercase()));
    bank.models
        .iter()
        .find(|m| m.name().eq_ignore_ascii_case(base) || m.id == crc || m.name().trim_end_matches(".obe").eq_ignore_ascii_case(base))
        .or_else(|| key.parse::<usize>().ok().and_then(|i| bank.models.get(i)))
}
