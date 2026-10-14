# F2NM — N64 render-model format

Render geometry and textures for the N64 renderer (`crates/frogger-n64` r3d, phase 2R).
Single source of truth: the `no_std` crate **`crates/f2-n64fmt`** (reader `File`, encoders in
`f2_n64fmt::encode`, feature `encode`, `alloc` only). The asset packer (`tools/f2-n64pack`)
should emit files with `encode::FileBuilder`; the N64 core loads them unchanged into RDRAM
(the RSP DMAs vertices and the RDP loads texels/palettes straight from the file buffer).

Scope: *what the RSP/RDP consume*. Model hierarchy, keyframes, collision etc. are the
packer's own formats; a mesh here is one MDX object mesh (or a skinned root mesh) and is
referenced by an id the packer chooses (e.g. the object name CRC).

## Conventions
- All multi-byte fields **big-endian**. Every section starts 8-byte aligned (RSP/RDP DMA);
  the file buffer itself must be 8-byte aligned in RDRAM (libdragon `malloc`/`asset_load`
  are).
- `VERSION = 1`. Readers reject other versions.

## Layout
```
Header (32)          "F2NM" u16 version u16 flags u16 n_textures u16 n_meshes
                     u32 tex_table u32 mesh_table u32 file_size u8[8] 0
TextureEntry[n] (24) at tex_table
MeshEntry[n]   (40)  at mesh_table
... per mesh: Vertex[] | Group[] | Batch[] | u32 cmds[]
... per texture: TLUT (RGBA16[]) | texels
```

### TextureEntry (24 bytes)
| off | type | field |
|---|---|---|
| 0 | u32 | `crc` — the game's texture CRC (`TextureRegistry` key; faces reference textures by CRC) |
| 4 | u8 | `fmt`: 0 RGBA16 (5551), 1 CI8, 2 CI4, 3 IA8, 4 IA16, 5 I8, 6 I4, 7 RGBA32 |
| 5 | u8 | flags: bit0 has alpha, bit1 smooth (non 0/255) alpha |
| 6 | u16×2 | N64 `width`, `height` — powers of two, fit TMEM (RGBA16/CI8/IA16 ≤ 2048 texels, CI4/IA8/I4 ≤ 4096, RGBA32 ≤ 1024; CI TLUT in the upper 2 KB) |
| 10 | u16×2 | PC source size (informational) |
| 14 | u16 | `tlut_count` (256 CI8, 16 CI4, else 0) |
| 16 | u32 | `tlut_off` (RGBA16 entries) |
| 20 | u32 | `texel_off`: rows of `row_bytes(width)` bytes, row-major (LOAD_BLOCK-ready) |

### MeshEntry (40 bytes)
| off | type | field |
|---|---|---|
| 0 | u32 | `id` (packer-defined) |
| 4 | u16 | flags: bit0 `SKINNED` (commands contain `Bone`), bit1 `ORDERED` (original face order kept) |
| 6 | i8 | `pos_shift`: stored position = round(v · 2^pos_shift) |
| 7 | u8 | `n_bones` |
| 8 | u16 | `n_verts` (≤ 65535) |
| 10 | u16 | `n_groups` |
| 12 | u32 | `n_tris` |
| 16 | u32 | `vert_off` |
| 20 | u32 | `group_off` |
| 24 | u32 | `cmd_off` |
| 28 | u32 | `n_cmds` |
| 32 | u32 | `batch_off` |
| 36 | u32 | `n_batches` |

### Vertex (16 bytes) — the RSP shader input
`s16 x, y, z` (model units × 2^pos_shift) · `u8 bone` · `u8 0` · `u8 r, g, b, a` (the PC's
clamped `D3DRGBA` corner colour) · `s16 s, t` in **1/1024 of a texture period**
(`round(faceTC × 0.000975 × 1024)`), texture-size independent: the renderer scales by the
current texture's size (so packers may resize textures and `overrideTex` works).
A vertex is a unique (position, colour, UV) per batch (MDX colours/UVs are per face corner).

### Group (16 bytes) — one texture
`u32 tex_crc` · `u32 cmd_first` · `u16 cmd_count` · `u16 n_tris` · `u16 batch_first` ·
`u16 batch_count`. A group's commands are the concatenation of its batches'. Unordered meshes
have one group per texture (faces stably grouped); `ORDERED` meshes (translucent lists) use
runs of consecutive same-texture faces.

### Batch (16 bytes) — one vertex-cache fill
`u32 cmd_first` · `u16 cmd_count` · `u16 n_tris` · `s16 center[3]` · `u16 radius` (stored
units, rounded up): bounding sphere of the batch's vertices, used for conservative culling
(a batch entirely past one screen edge or beyond near/far is one MDX would reject face by
face).

### Commands (u32, opcode in bits 31..28)
| op | encoding | meaning |
|---|---|---|
| 1 `Load` | `first:16 count:6 slot:6` | transform vertices `first..first+count` into cache slots `slot..` (count 1..48). The RSP writes pairs: an odd count also overwrites slot `slot+count`, so a batch's loads use ascending slots |
| 2 `Tri` | `0:4 a:8 b:8 c:8` | triangle on cache slots, original winding |
| 3 `Bone` | `0:20 bone:8` | following loads use bone `bone`'s matrix (skinned meshes; vertices grouped by bone within a batch) |

Each batch is: (`Bone`) `Load`… then `Tri`…, at most 48 unique vertices.

## Encoder rules (`encode::encode_mesh`, `MeshIn`)
- `pos_shift` keeps |stored| ≲ 8192: balances position rounding against s15.16 matrix
  rounding on the RSP.
- UVs: meshes drawn with **clamped** addressing (`Normal` list: landscape, most objects,
  skins: `wrap: false`) keep their coordinates (saturated to s16); `wrap: true` meshes
  (`Wrap`/`Xlu`/`Additive` lists) shift faces beyond ±4 periods by whole periods. Shifting a
  clamped face changes what clamping shows (seen on the frog's skin).
- Faces with no texture (CRC 0 / not loaded) must be dropped by the caller (PC skips them);
  skinned faces using a vertex no bone moves likewise.
- `encode::encode_texture` is a basic reference converter (lossless CI4/CI8 when the 5551
  colour count allows, IA16 for grey smooth alpha, RGBA32 for colour smooth alpha, else
  RGBA16, box-downscaled to fit TMEM). The packer may pick formats by error budget.

## Runtime use (r3d)
Textures register by CRC; each *batch* of a rigid mesh (each *group* of a skinned mesh) is
recorded once into an rspq block. Per draw the CPU culls batches by sphere, then runs the
visible blocks, loading a texture (a 144-byte RDP command descriptor processed inside the
magma overlay) when it changes. See `crates/frogger-n64/src/r3d.rs` and
`research/n64-performance.md` (phase 2R).

Texture sliding (`CopyActorList`/`SlideObjectTextures`: flowing water, sludge, lava, conveyor
belts, the `spl_`/`spm_`/`spf_` scenery such as the Subterranean ant columns): the PC edits the
model's `faceTC.v`; F2NM vertices are static, so the draw's `F2_DRAW` uniform carries an st
offset (`st_off`, 1/1024-period units like the vertices) that the shader adds before the
texture scale. The offset is the model's total slide (`render_r3d::R3dLevel::slide_textures`,
once per game tick) mod 8192 `faceTC` units (the PC's per-face +8192 wrap), less whole periods
(`r3d::slide_st`: -1023..=0; sliding actors are `ACTOR_WRAPTC`, drawn with wrapping).

## Object banks (phase 3I, `f2_n64fmt::bank`, feature `objbank`)
The pack stores one F2NM file per object bank (`objects/<bank>.dll.f2nm`, meshes only:
textures come from the NTX1 banks and are resolved by CRC). Mesh ids are
`model_index << 16 | object` (`object` = pre-order index, `0xFFFF` = a skinned model's root
mesh), so the front end maps a recorded draw (`f2_engine::MeshDraw { model, object }`) to its
mesh without names. Faces without a texture CRC, with out-of-range indices or (skins) with a
vertex no bone moves are dropped like `PCRenderObject` does; `wrap`/`ordered` follow the frame
list of the object's flags (the landscape `world.obe` is clamped and unordered). Meshes the game
always draws additively (object flag, or models named `ada*`, `wat_*`, `xxa_*`) store their
corner colours split as `rgb / max(rgb)`, `a * max(rgb)` (`additive_corner`): the renderer
draws additive lists with the alpha blender and each texture's additive variant (same split),
which reproduces the D3D `src * a` term exactly.
