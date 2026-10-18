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

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

/// `alloc` items the std prelude would provide (the crate also builds `no_std`).
#[allow(unused_imports)]
pub(crate) mod prelude {
    pub use alloc::borrow::ToOwned;
    pub use alloc::boxed::Box;
    pub use alloc::string::{String, ToString};
    pub use alloc::vec::Vec;
    pub use alloc::{format, vec};
}

#[cfg(not(feature = "std"))]
mod fmath;

use prelude::*;

pub mod anim;
pub mod font;
#[cfg(feature = "gles2")]
pub mod gles;
#[cfg(feature = "gpu")]
mod gpu_mesh;
pub mod lighting;
#[cfg(feature = "std")]
pub mod load;
pub mod math;
pub mod renderer;
#[cfg(feature = "gpu")]
mod rt;
pub mod scene;
pub mod soft;
pub mod textures;
pub mod upscale;
#[cfg(feature = "ps5-vulkan")]
pub mod vkraw;
#[cfg(feature = "gpu")]
pub mod water;

pub use anim::{actor_bounds, xform_actor, Actor};
pub use font::{Font, HiRes};
pub use lighting::{DirLight, Lighting, PointLight};
pub use renderer::{Blend, DepthMode, DrawCmd, EngineError, FontId, FrameView, PassState, Quad2D, Renderer, RgbaImage, UpscaleDebug, OVERLAY_SPACE};
#[cfg(feature = "gpu")]
pub use renderer::{GpuContext, WindowSurface};
pub use scene::{Camera, Fog, Frame, MeshBackend, MeshDraw, MeshSink, ObjDraw, ObjSphere, RenderOptions, TlVertex, Tri, View, XluSort, MESH_SKIN};
pub use soft::SoftTarget;
pub use textures::{TexHandle, TextureRegistry};
pub use upscale::{UpscaleCamera, UpscaleMode};
#[cfg(feature = "gpu")]
pub use upscale::{UpscaleInputs, Upscaler};

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
