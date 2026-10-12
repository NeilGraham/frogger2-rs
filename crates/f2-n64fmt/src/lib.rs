//! F2NM: the Nintendo 64 render-model format of the Frogger 2 port.
//!
//! One F2NM file holds RSP-ready meshes and TMEM-ready textures. Meshes are what the N64
//! renderer's RSP vertex shader (`crates/frogger-n64/csrc/rsp_f2mdx.S`) consumes directly:
//! 16-byte big-endian vertices DMA'd straight from RDRAM, and a pre-batched command list
//! (vertex-cache loads + triangles) per texture group, so the CPU never touches a vertex at
//! run time. The full byte layout is specified in `docs/n64-model-format.md`; this crate is
//! the single source of truth for both sides:
//!
//! * [`File`] — zero-copy, bounds-checked reader (any endianness host; used by the N64 core
//!   and by host tests);
//! * [`encode`] (feature `encode`, `alloc` only) — mesh batching / quantisation, a basic
//!   texture converter and the file writer, used by the asset packer (`tools/f2-n64pack`),
//!   the test-scene exporter and the on-console synthetic benchmarks.
//!
//! All multi-byte fields are **big-endian** (the N64's byte order: the RSP reads vertices
//! and the RDP reads texels and palettes without conversion).
#![no_std]
#![allow(clippy::needless_range_loop)]

#[cfg(feature = "encode")]
extern crate alloc;

#[cfg(feature = "encode")]
pub mod encode;

#[cfg(feature = "objbank")]
pub mod bank;

/// File magic.
pub const MAGIC: [u8; 4] = *b"F2NM";
/// Format version (bumped on any layout change).
pub const VERSION: u16 = 1;

pub const HEADER_SIZE: usize = 32;
pub const TEXTURE_ENTRY_SIZE: usize = 24;
pub const MESH_ENTRY_SIZE: usize = 40;
pub const GROUP_SIZE: usize = 16;
pub const BATCH_SIZE: usize = 16;
/// Bytes per vertex (the RSP shader's input stride).
pub const VERTEX_SIZE: usize = 16;
/// Vertices the RSP vertex cache holds (`MG_VERTEX_CACHE_COUNT`): a batch's loads address
/// cache slots `0..CACHE_SLOTS`.
pub const CACHE_SLOTS: usize = 48;
/// MDX texture-coordinate scale (`faceTC` x 0.000975, mdxObject.cpp).
pub const MDX_UV_SCALE: f32 = 0.000975;
/// Stored texture coordinates are in units of 1/`ST_ONE` of the texture (one wrap period).
pub const ST_ONE: i32 = 1024;
/// Stored |s|,|t| stay within this many periods (keeps the RSP's s10.5 texel coordinates in
/// range for textures up to 128 texels wide).
pub const ST_MAX_PERIODS: i32 = 4;

/// Mesh flags.
pub mod mesh_flags {
    /// Vertices are grouped by bone; the command list contains [`super::Cmd::Bone`].
    pub const SKINNED: u16 = 1 << 0;
    /// Faces kept in their original order (groups are runs of one texture, so a texture can
    /// appear in several groups). Set for meshes whose draw order matters (translucent).
    pub const ORDERED: u16 = 1 << 1;
    /// Positions are in world space (static landscape baked with its key-0 transforms, all
    /// meshes of the model on one position grid so shared edges are bit-identical): drawn
    /// with an identity model matrix.
    pub const WORLD: u16 = 1 << 2;
}

/// Texture flags.
pub mod tex_flags {
    /// Some texels are not fully opaque (1-bit or more alpha in use).
    pub const HAS_ALPHA: u8 = 1 << 0;
    /// Alpha has intermediate values (not only 0/255): needs a format with >1 bit alpha.
    pub const SMOOTH_ALPHA: u8 = 1 << 1;
}

/// RDP texel formats (values of [`TextureEntry::fmt`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TexFormat {
    /// 16-bit RGBA 5551.
    Rgba16 = 0,
    /// 8-bit palette index (256-entry RGBA16 TLUT).
    Ci8 = 1,
    /// 4-bit palette index (16-entry RGBA16 TLUT).
    Ci4 = 2,
    /// 4-bit intensity + 4-bit alpha.
    Ia8 = 3,
    /// 8-bit intensity + 8-bit alpha.
    Ia16 = 4,
    /// 8-bit intensity (alpha = intensity).
    I8 = 5,
    /// 4-bit intensity.
    I4 = 6,
    /// 32-bit RGBA 8888.
    Rgba32 = 7,
}

impl TexFormat {
    pub fn from_u8(v: u8) -> Option<TexFormat> {
        Some(match v {
            0 => TexFormat::Rgba16,
            1 => TexFormat::Ci8,
            2 => TexFormat::Ci4,
            3 => TexFormat::Ia8,
            4 => TexFormat::Ia16,
            5 => TexFormat::I8,
            6 => TexFormat::I4,
            7 => TexFormat::Rgba32,
            _ => return None,
        })
    }
    /// Bits per texel.
    pub fn bits(self) -> u32 {
        match self {
            TexFormat::Rgba32 => 32,
            TexFormat::Rgba16 | TexFormat::Ia16 => 16,
            TexFormat::Ci8 | TexFormat::Ia8 | TexFormat::I8 => 8,
            TexFormat::Ci4 | TexFormat::I4 => 4,
        }
    }
    /// Bytes of one row of `width` texels (the stored row stride).
    pub fn row_bytes(self, width: u32) -> u32 {
        (width * self.bits()).div_ceil(8)
    }
    /// Palette entries a texture of this format carries.
    pub fn tlut_entries(self) -> u32 {
        match self {
            TexFormat::Ci8 => 256,
            TexFormat::Ci4 => 16,
            _ => 0,
        }
    }
}

/// Reader errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    BadMagic,
    BadVersion(u16),
    /// A table or section lies outside the data.
    OutOfBounds,
    /// A section is not 8-byte aligned (the RSP/RDP DMA needs 8-byte alignment).
    Misaligned,
    BadFormat(u8),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::BadMagic => write!(f, "not an F2NM file"),
            Error::BadVersion(v) => write!(f, "unsupported F2NM version {v}"),
            Error::OutOfBounds => write!(f, "F2NM section out of bounds"),
            Error::Misaligned => write!(f, "F2NM section not 8-byte aligned"),
            Error::BadFormat(v) => write!(f, "unknown texture format {v}"),
        }
    }
}

#[inline]
fn be16(d: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([d[o], d[o + 1]])
}
#[inline]
fn be32(d: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// File header (32 bytes at offset 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub version: u16,
    pub flags: u16,
    pub n_textures: u16,
    pub n_meshes: u16,
    /// Offset of `TextureEntry[n_textures]`.
    pub tex_table: u32,
    /// Offset of `MeshEntry[n_meshes]`.
    pub mesh_table: u32,
    /// Total file size in bytes.
    pub size: u32,
}

impl Header {
    pub fn read(d: &[u8]) -> Result<Header, Error> {
        if d.len() < HEADER_SIZE {
            return Err(Error::OutOfBounds);
        }
        if d[0..4] != MAGIC {
            return Err(Error::BadMagic);
        }
        let h = Header {
            version: be16(d, 4),
            flags: be16(d, 6),
            n_textures: be16(d, 8),
            n_meshes: be16(d, 10),
            tex_table: be32(d, 12),
            mesh_table: be32(d, 16),
            size: be32(d, 20),
        };
        if h.version != VERSION {
            return Err(Error::BadVersion(h.version));
        }
        Ok(h)
    }
    pub fn write(&self, out: &mut [u8]) {
        out[0..4].copy_from_slice(&MAGIC);
        out[4..6].copy_from_slice(&self.version.to_be_bytes());
        out[6..8].copy_from_slice(&self.flags.to_be_bytes());
        out[8..10].copy_from_slice(&self.n_textures.to_be_bytes());
        out[10..12].copy_from_slice(&self.n_meshes.to_be_bytes());
        out[12..16].copy_from_slice(&self.tex_table.to_be_bytes());
        out[16..20].copy_from_slice(&self.mesh_table.to_be_bytes());
        out[20..24].copy_from_slice(&self.size.to_be_bytes());
        out[24..32].fill(0);
    }
}

/// A texture (24-byte table entry). Texels are stored row-major, rows of
/// `fmt.row_bytes(width)` bytes (TMEM-ready); the palette is `tlut_count` RGBA16 entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureEntry {
    /// The game's texture CRC (`TextureRegistry` key; faces reference textures by CRC).
    pub crc: u32,
    pub fmt: TexFormat,
    pub flags: u8,
    /// N64 size (powers of two, fits TMEM).
    pub width: u16,
    pub height: u16,
    /// Size of the PC source image (informational).
    pub src_width: u16,
    pub src_height: u16,
    pub tlut_count: u16,
    pub tlut_off: u32,
    pub texel_off: u32,
}

impl TextureEntry {
    pub fn read(d: &[u8], o: usize) -> Result<TextureEntry, Error> {
        let e = d.get(o..o + TEXTURE_ENTRY_SIZE).ok_or(Error::OutOfBounds)?;
        Ok(TextureEntry {
            crc: be32(e, 0),
            fmt: TexFormat::from_u8(e[4]).ok_or(Error::BadFormat(e[4]))?,
            flags: e[5],
            width: be16(e, 6),
            height: be16(e, 8),
            src_width: be16(e, 10),
            src_height: be16(e, 12),
            tlut_count: be16(e, 14),
            tlut_off: be32(e, 16),
            texel_off: be32(e, 20),
        })
    }
    pub fn write(&self, out: &mut [u8]) {
        out[0..4].copy_from_slice(&self.crc.to_be_bytes());
        out[4] = self.fmt as u8;
        out[5] = self.flags;
        out[6..8].copy_from_slice(&self.width.to_be_bytes());
        out[8..10].copy_from_slice(&self.height.to_be_bytes());
        out[10..12].copy_from_slice(&self.src_width.to_be_bytes());
        out[12..14].copy_from_slice(&self.src_height.to_be_bytes());
        out[14..16].copy_from_slice(&self.tlut_count.to_be_bytes());
        out[16..20].copy_from_slice(&self.tlut_off.to_be_bytes());
        out[20..24].copy_from_slice(&self.texel_off.to_be_bytes());
    }
    /// Texel bytes.
    pub fn texel_bytes(&self) -> u32 {
        self.fmt.row_bytes(self.width as u32) * self.height as u32
    }
}

/// A mesh (32-byte table entry).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeshEntry {
    /// Packer-defined id (e.g. the object's name CRC).
    pub id: u32,
    pub flags: u16,
    /// Positions are stored as `round(v * 2^pos_shift)`.
    pub pos_shift: i8,
    /// Bones referenced by [`Cmd::Bone`] (skinned meshes).
    pub n_bones: u8,
    pub n_verts: u16,
    pub n_groups: u16,
    /// Triangles drawn (sum over groups).
    pub n_tris: u32,
    /// Offset of `Vertex[n_verts]` (8-aligned).
    pub vert_off: u32,
    /// Offset of `Group[n_groups]`.
    pub group_off: u32,
    /// Offset of the `u32` command words of all groups.
    pub cmd_off: u32,
    pub n_cmds: u32,
    /// Offset of `Batch[n_batches]`.
    pub batch_off: u32,
    pub n_batches: u32,
}

impl MeshEntry {
    pub fn read(d: &[u8], o: usize) -> Result<MeshEntry, Error> {
        let e = d.get(o..o + MESH_ENTRY_SIZE).ok_or(Error::OutOfBounds)?;
        Ok(MeshEntry {
            id: be32(e, 0),
            flags: be16(e, 4),
            pos_shift: e[6] as i8,
            n_bones: e[7],
            n_verts: be16(e, 8),
            n_groups: be16(e, 10),
            n_tris: be32(e, 12),
            vert_off: be32(e, 16),
            group_off: be32(e, 20),
            cmd_off: be32(e, 24),
            n_cmds: be32(e, 28),
            batch_off: be32(e, 32),
            n_batches: be32(e, 36),
        })
    }
    pub fn write(&self, out: &mut [u8]) {
        out[0..4].copy_from_slice(&self.id.to_be_bytes());
        out[4..6].copy_from_slice(&self.flags.to_be_bytes());
        out[6] = self.pos_shift as u8;
        out[7] = self.n_bones;
        out[8..10].copy_from_slice(&self.n_verts.to_be_bytes());
        out[10..12].copy_from_slice(&self.n_groups.to_be_bytes());
        out[12..16].copy_from_slice(&self.n_tris.to_be_bytes());
        out[16..20].copy_from_slice(&self.vert_off.to_be_bytes());
        out[20..24].copy_from_slice(&self.group_off.to_be_bytes());
        out[24..28].copy_from_slice(&self.cmd_off.to_be_bytes());
        out[28..32].copy_from_slice(&self.n_cmds.to_be_bytes());
        out[32..36].copy_from_slice(&self.batch_off.to_be_bytes());
        out[36..40].copy_from_slice(&self.n_batches.to_be_bytes());
    }
    pub fn is_skinned(&self) -> bool {
        self.flags & mesh_flags::SKINNED != 0
    }

    pub fn is_world(&self) -> bool {
        self.flags & mesh_flags::WORLD != 0
    }
    /// `2^-pos_shift`: multiply stored positions by this to get model units.
    pub fn pos_scale(&self) -> f32 {
        let s = self.pos_shift as i32;
        if s >= 0 {
            1.0 / (1u32 << s) as f32
        } else {
            (1u32 << -s) as f32
        }
    }
}

/// A texture group of a mesh (16 bytes): its faces all use one texture. Its commands are
/// the concatenation of its batches' commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Group {
    /// Texture CRC (resolved at run time, so animated textures and `overrideTex` work).
    pub tex_crc: u32,
    /// First command word (index into the mesh's command array).
    pub cmd_first: u32,
    pub cmd_count: u16,
    pub n_tris: u16,
    /// The group's batches (indices into the mesh's batch table).
    pub batch_first: u16,
    pub batch_count: u16,
}

impl Group {
    pub fn read(d: &[u8], o: usize) -> Result<Group, Error> {
        let e = d.get(o..o + GROUP_SIZE).ok_or(Error::OutOfBounds)?;
        Ok(Group { tex_crc: be32(e, 0), cmd_first: be32(e, 4), cmd_count: be16(e, 8), n_tris: be16(e, 10), batch_first: be16(e, 12), batch_count: be16(e, 14) })
    }
    pub fn write(&self, out: &mut [u8]) {
        out[0..4].copy_from_slice(&self.tex_crc.to_be_bytes());
        out[4..8].copy_from_slice(&self.cmd_first.to_be_bytes());
        out[8..10].copy_from_slice(&self.cmd_count.to_be_bytes());
        out[10..12].copy_from_slice(&self.n_tris.to_be_bytes());
        out[12..14].copy_from_slice(&self.batch_first.to_be_bytes());
        out[14..16].copy_from_slice(&self.batch_count.to_be_bytes());
    }
}

/// A vertex-cache batch (16 bytes): one or more `Load`s then the triangles on those slots,
/// with a bounding sphere for culling (if the sphere is entirely off-screen or outside
/// near/far, every face of the batch would be rejected by MDX anyway).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Batch {
    /// First command word (index into the mesh's command array).
    pub cmd_first: u32,
    pub cmd_count: u16,
    pub n_tris: u16,
    /// Sphere centre in stored position units.
    pub center: [i16; 3],
    /// Radius in stored position units (rounded up).
    pub radius: u16,
}

impl Batch {
    pub fn read(d: &[u8], o: usize) -> Result<Batch, Error> {
        let e = d.get(o..o + BATCH_SIZE).ok_or(Error::OutOfBounds)?;
        Ok(Batch {
            cmd_first: be32(e, 0),
            cmd_count: be16(e, 4),
            n_tris: be16(e, 6),
            center: [be16(e, 8) as i16, be16(e, 10) as i16, be16(e, 12) as i16],
            radius: be16(e, 14),
        })
    }
    pub fn write(&self, out: &mut [u8]) {
        out[0..4].copy_from_slice(&self.cmd_first.to_be_bytes());
        out[4..6].copy_from_slice(&self.cmd_count.to_be_bytes());
        out[6..8].copy_from_slice(&self.n_tris.to_be_bytes());
        for k in 0..3 {
            out[8 + 2 * k..10 + 2 * k].copy_from_slice(&self.center[k].to_be_bytes());
        }
        out[14..16].copy_from_slice(&self.radius.to_be_bytes());
    }
}

/// One vertex (16 bytes, the RSP shader input):
/// `x y z: s16` (model units x 2^pos_shift), `bone: u8`, `pad: u8`, `r g b a: u8`,
/// `s t: s16` (texture coordinates in 1/[`ST_ONE`] of a wrap period).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Vertex {
    pub pos: [i16; 3],
    pub bone: u8,
    pub rgba: [u8; 4],
    pub st: [i16; 2],
}

impl Vertex {
    pub fn read(d: &[u8], o: usize) -> Result<Vertex, Error> {
        let e = d.get(o..o + VERTEX_SIZE).ok_or(Error::OutOfBounds)?;
        Ok(Vertex {
            pos: [be16(e, 0) as i16, be16(e, 2) as i16, be16(e, 4) as i16],
            bone: e[6],
            rgba: [e[8], e[9], e[10], e[11]],
            st: [be16(e, 12) as i16, be16(e, 14) as i16],
        })
    }
    pub fn write(&self, out: &mut [u8]) {
        for k in 0..3 {
            out[2 * k..2 * k + 2].copy_from_slice(&self.pos[k].to_be_bytes());
        }
        out[6] = self.bone;
        out[7] = 0;
        out[8..12].copy_from_slice(&self.rgba);
        out[12..14].copy_from_slice(&self.st[0].to_be_bytes());
        out[14..16].copy_from_slice(&self.st[1].to_be_bytes());
    }
}

/// A command word (big-endian `u32`, opcode in bits 31..28).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    /// `0x1 | first:16 | count:6 | slot:6`: transform vertices `first..first+count` of the
    /// mesh into cache slots `slot..slot+count` (count 1..=48). Loads of one batch use
    /// ascending slots (the RSP writes vertices in pairs, so an odd count also overwrites
    /// slot `slot+count`, which the next load of the batch then refills).
    Load { first: u16, count: u8, slot: u8 },
    /// `0x2 | 0:4 | a:8 | b:8 | c:8`: triangle on cache slots (original winding).
    Tri([u8; 3]),
    /// `0x3 | 0:20 | bone:8`: subsequent loads use bone `bone`'s matrix (skinned meshes).
    Bone(u8),
}

impl Cmd {
    pub fn encode(self) -> u32 {
        match self {
            Cmd::Load { first, count, slot } => 0x1000_0000 | (first as u32) << 12 | ((count as u32) & 63) << 6 | (slot as u32) & 63,
            Cmd::Tri([a, b, c]) => 0x2000_0000 | (a as u32) << 16 | (b as u32) << 8 | c as u32,
            Cmd::Bone(b) => 0x3000_0000 | b as u32,
        }
    }
    pub fn decode(w: u32) -> Option<Cmd> {
        Some(match w >> 28 {
            1 => Cmd::Load { first: (w >> 12) as u16, count: ((w >> 6) & 63) as u8, slot: (w & 63) as u8 },
            2 => Cmd::Tri([(w >> 16) as u8, (w >> 8) as u8, w as u8]),
            3 => Cmd::Bone(w as u8),
            _ => return None,
        })
    }
}

/// A parsed F2NM file (borrows the bytes; all accessors are bounds-checked).
#[derive(Clone, Copy, Debug)]
pub struct File<'a> {
    pub data: &'a [u8],
    pub header: Header,
}

/// Borrowed view of one mesh.
#[derive(Clone, Copy, Debug)]
pub struct MeshView<'a> {
    pub entry: MeshEntry,
    /// `n_verts * 16` bytes of big-endian vertices (DMA-able as is).
    pub vertices: &'a [u8],
    groups: &'a [u8],
    batches: &'a [u8],
    cmds: &'a [u8],
}

impl<'a> MeshView<'a> {
    pub fn group(&self, i: usize) -> Group {
        Group::read(self.groups, i * GROUP_SIZE).expect("validated")
    }
    pub fn groups(&self) -> impl Iterator<Item = Group> + '_ {
        (0..self.entry.n_groups as usize).map(|i| self.group(i))
    }
    pub fn batch(&self, i: usize) -> Batch {
        Batch::read(self.batches, i * BATCH_SIZE).expect("validated")
    }
    /// The batches of `g`.
    pub fn group_batches(&self, g: &Group) -> impl Iterator<Item = Batch> + '_ {
        let a = g.batch_first as usize;
        (a..a + g.batch_count as usize).map(|i| self.batch(i))
    }
    /// Raw big-endian command words of a batch.
    pub fn batch_cmd_bytes(&self, b: &Batch) -> &'a [u8] {
        let a = b.cmd_first as usize * 4;
        &self.cmds[a..a + b.cmd_count as usize * 4]
    }
    pub fn cmd_word(&self, i: usize) -> u32 {
        be32(self.cmds, i * 4)
    }
    /// The decoded commands of `g`.
    pub fn group_cmds(&self, g: &Group) -> impl Iterator<Item = Cmd> + '_ {
        let (a, n) = (g.cmd_first as usize, g.cmd_count as usize);
        (a..a + n).filter_map(|i| Cmd::decode(self.cmd_word(i)))
    }
    /// Raw big-endian command words of `g`.
    pub fn group_cmd_bytes(&self, g: &Group) -> &'a [u8] {
        let a = g.cmd_first as usize * 4;
        &self.cmds[a..a + g.cmd_count as usize * 4]
    }
    pub fn vertex(&self, i: usize) -> Vertex {
        Vertex::read(self.vertices, i * VERTEX_SIZE).expect("validated")
    }
}

impl<'a> File<'a> {
    /// Parses and validates the header and every table/section range.
    pub fn parse(data: &'a [u8]) -> Result<File<'a>, Error> {
        let header = Header::read(data)?;
        if header.size as usize > data.len() {
            return Err(Error::OutOfBounds);
        }
        let f = File { data: &data[..header.size as usize], header };
        for i in 0..header.n_textures as usize {
            let t = f.texture(i)?;
            let tb = t.texel_bytes() as usize;
            f.range(t.texel_off as usize, tb)?;
            f.range(t.tlut_off as usize, t.tlut_count as usize * 2)?;
        }
        for i in 0..header.n_meshes as usize {
            let m = f.mesh(i)?;
            for g in m.groups() {
                let end = g.cmd_first as usize + g.cmd_count as usize;
                if end > m.entry.n_cmds as usize || g.batch_first as usize + g.batch_count as usize > m.entry.n_batches as usize {
                    return Err(Error::OutOfBounds);
                }
            }
            for i in 0..m.entry.n_batches as usize {
                let b = m.batch(i);
                if b.cmd_first as usize + b.cmd_count as usize > m.entry.n_cmds as usize {
                    return Err(Error::OutOfBounds);
                }
            }
        }
        Ok(f)
    }

    fn range(&self, off: usize, len: usize) -> Result<&'a [u8], Error> {
        if len == 0 {
            return Ok(&[]);
        }
        if !off.is_multiple_of(8) {
            return Err(Error::Misaligned);
        }
        self.data.get(off..off.checked_add(len).ok_or(Error::OutOfBounds)?).ok_or(Error::OutOfBounds)
    }

    pub fn n_textures(&self) -> usize {
        self.header.n_textures as usize
    }
    pub fn n_meshes(&self) -> usize {
        self.header.n_meshes as usize
    }

    pub fn texture(&self, i: usize) -> Result<TextureEntry, Error> {
        if i >= self.n_textures() {
            return Err(Error::OutOfBounds);
        }
        TextureEntry::read(self.data, self.header.tex_table as usize + i * TEXTURE_ENTRY_SIZE)
    }
    /// Texels of texture `t` (row-major, TMEM-ready rows).
    pub fn texels(&self, t: &TextureEntry) -> &'a [u8] {
        self.range(t.texel_off as usize, t.texel_bytes() as usize).unwrap_or(&[])
    }
    /// Palette of texture `t` as big-endian RGBA16 bytes.
    pub fn tlut(&self, t: &TextureEntry) -> &'a [u8] {
        self.range(t.tlut_off as usize, t.tlut_count as usize * 2).unwrap_or(&[])
    }

    pub fn mesh(&self, i: usize) -> Result<MeshView<'a>, Error> {
        if i >= self.n_meshes() {
            return Err(Error::OutOfBounds);
        }
        let entry = MeshEntry::read(self.data, self.header.mesh_table as usize + i * MESH_ENTRY_SIZE)?;
        Ok(MeshView {
            entry,
            vertices: self.range(entry.vert_off as usize, entry.n_verts as usize * VERTEX_SIZE)?,
            groups: self.range(entry.group_off as usize, entry.n_groups as usize * GROUP_SIZE)?,
            batches: self.range(entry.batch_off as usize, entry.n_batches as usize * BATCH_SIZE)?,
            cmds: self.range(entry.cmd_off as usize, entry.n_cmds as usize * 4)?,
        })
    }
}

/// Converts an MDX face UV (`faceTC`, value x [`MDX_UV_SCALE`] = texture coordinate) to the
/// stored unit (1/[`ST_ONE`] of the texture), saturating to `i16`.
pub fn mdx_uv_to_st(raw: i16) -> i32 {
    let v = raw as f32 * MDX_UV_SCALE * ST_ONE as f32;
    let r = if v >= 0.0 { v + 0.5 } else { v - 0.5 };
    r as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd_roundtrip() {
        for c in [Cmd::Load { first: 65535, count: 48, slot: 47 }, Cmd::Load { first: 0, count: 1, slot: 0 }, Cmd::Tri([0, 47, 12]), Cmd::Bone(200)] {
            assert_eq!(Cmd::decode(c.encode()), Some(c));
        }
        assert_eq!(Cmd::decode(0), None);
    }

    #[test]
    fn vertex_layout_is_big_endian() {
        let v = Vertex { pos: [1, -2, 0x1234], bone: 7, rgba: [10, 20, 30, 40], st: [-1024, 2048] };
        let mut b = [0u8; 16];
        v.write(&mut b);
        assert_eq!(b, [0, 1, 0xff, 0xfe, 0x12, 0x34, 7, 0, 10, 20, 30, 40, 0xfc, 0x00, 0x08, 0x00]);
        assert_eq!(Vertex::read(&b, 0).unwrap(), v);
    }

    #[test]
    fn uv_units() {
        // 1/0.000975 raw units = one texture period = 1024 stored units.
        assert_eq!(mdx_uv_to_st(1026), 1024);
        assert_eq!(mdx_uv_to_st(-513), -512);
        assert_eq!(mdx_uv_to_st(0), 0);
    }
}
