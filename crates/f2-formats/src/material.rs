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
    /// The colour drawn instead of the face texture (the port's redrawn slabs; the face
    /// texture stays what Original graphics draw). None: the face texture.
    pub base_map: Option<u32>,
    /// Smart tiling: every repeat of the texture made a little different (none by default).
    pub tiling: Tiling,
}

/// Smart tiling of a material (Enhanced graphics; like connected / random textures in block
/// games): each repeat of the texture (a tile of paving, a cell of the mesh's texture grid) gets
/// its own turn, tint, setting and wear from a seed, and its edges look at what they meet
/// (another tile flush with it, a drop, a wall; found from the mesh when it is loaded). All 0:
/// every repeat the same (the material as drawn before).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tiling {
    /// Random turns of each repeat: 0 none, 1 mirrored, 2 also turned by quarters (square tiles).
    pub variants: u8,
    /// The turns move the colour texture too (else only the relief: normal, height, ORM).
    pub turn_color: bool,
    /// Chips at the corners and edges, cracks and pits (0..1, how many and how big).
    pub wear: f32,
    /// How far each repeat's colour strays (brightness and warmth, 0..1).
    pub tint: f32,
    /// Uneven setting: each tile tilted and sunk a little (0..1).
    pub uneven: f32,
    /// Extra wear on edges that drop away (a walkway's sides; 0..1).
    pub edges: f32,
    /// Dirt in the damage and along walls (0..1).
    pub grime: f32,
}

impl Tiling {
    /// Anything to do.
    pub fn is_on(&self) -> bool {
        self.variants > 0 || self.wear > 0.0 || self.tint > 0.0 || self.uneven > 0.0 || self.edges > 0.0 || self.grime > 0.0
    }
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
/// [`ModelMaterials::model_id`] of a texture's own material (every model drawing it).
pub const MODEL_ANY: u32 = u32::MAX;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelMaterials {
    /// The model these are for ([`MODEL_ANY`]: a bank texture's material, on every model and
    /// the landscape).
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
