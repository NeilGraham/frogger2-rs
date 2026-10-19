//! Modded models' materials (not a retail format): what a glTF override brings besides the
//! geometry (`f2-modkit`'s importer): its own images, and physically based materials for the
//! Enhanced GPU mesh pass (`f2-engine` `gpu_mesh`). Retail banks have none.

use crate::prelude::*;

/// An RGBA8 image of a glTF document (full colour, not reduced to X1R5G5B5).
#[derive(Clone, PartialEq, Eq)]
pub struct MatImage {
    /// The image's name or file stem (`""`: none).
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl core::fmt::Debug for MatImage {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "MatImage({:?}, {}x{})", self.name, self.width, self.height)
    }
}

/// glTF 2.0 metallic-roughness material of the faces with texture CRC `crc` (the face key
/// within its model). Map fields index [`ModelMaterials::images`].
#[derive(Clone, Debug, PartialEq)]
pub struct PbrMaterial {
    pub crc: u32,
    /// `baseColorFactor` (× the face texture × the vertex colour).
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    /// `normalTexture.scale`.
    pub normal_scale: f32,
    /// `occlusionTexture.strength`.
    pub occlusion_strength: f32,
    /// `emissiveFactor` (× `emissiveTexture`; × `KHR_materials_emissive_strength`).
    pub emissive: [f32; 3],
    /// Parallax occlusion depth (`extras.parallaxScale`, texture coordinate units per full height).
    pub parallax_scale: f32,
    /// Tangent-space normal map (OpenGL convention: +Y = up in the image).
    pub normal: Option<u32>,
    /// G: roughness, B: metalness.
    pub metal_rough: Option<u32>,
    /// R: ambient occlusion.
    pub occlusion: Option<u32>,
    pub emissive_map: Option<u32>,
    /// R: height (white = high).
    pub height: Option<u32>,
}

/// A modded skin's smooth weights (linear blend skinning on the GPU mesh path; the CPU path
/// keeps each vertex on its heaviest joint, the object listing it in `effected_verts`). Per
/// vertex of the model's skin mesh (`root.mesh`), which holds each vertex in its heaviest
/// joint's space.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkinWeights {
    /// Pre-order object indices, heaviest first.
    pub joints: Vec<[u8; 4]>,
    /// In 255ths, summing to 255 (unused: 0).
    pub weights: Vec<[u8; 4]>,
}

/// What a modded model brings besides its geometry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelMaterials {
    pub model_id: u32,
    pub images: Vec<MatImage>,
    /// Face textures from the document's images: (texture CRC, image), registered like a
    /// bank texture (replacing one of that CRC).
    pub textures: Vec<(u32, u32)>,
    /// Face textures showing another texture's image (a CRC made unique for a second material
    /// on the same image): (new CRC, shown CRC).
    pub aliases: Vec<(u32, u32)>,
    pub materials: Vec<PbrMaterial>,
    /// Smooth skin weights (a mesh-only skin override with vertices on several joints).
    pub skin: Option<SkinWeights>,
}

impl ModelMaterials {
    pub fn is_empty(&self) -> bool {
        self.textures.is_empty() && self.aliases.is_empty() && self.materials.is_empty() && self.skin.is_none()
    }
}
