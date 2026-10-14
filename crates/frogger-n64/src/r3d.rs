//! r3d: the RSP 3D renderer front end (phase 2R).
//!
//! The game loop computes each object's model matrix (the f32 `xform_actor` maths) and
//! queues "draw mesh M with matrix X and params P into frame list L"; per draw the CPU only
//! folds model x view x projection into one s15.16 matrix (≈ 100 flops) — vertices are
//! transformed, projected (MDX maths), rejected and set up on the RSP by
//! `csrc/rsp_f2mdx.S` via libdragon's magma pipeline (`csrc/r3d.c`). At the end of the frame
//! the lists are flushed in `DrawAllFrames` order with their render states.
//!
//! ```ignore
//! let mut r = R3d::new();                       // once (after the libdragon host is up)
//! let meshes = r.load_file(bytes)?;             // F2NM file (textures + meshes)
//! // every frame:
//! rt::begin_frame(clear);
//! r.begin_frame(&ViewParams { view, ..ViewParams::pc_default(320.0, 240.0) });
//! r.draw(List::Normal, mesh, &model_matrix, &DrawParams::default());
//! r.flush(|list| { /* CPU-side batches (sprites, effects) after `list` */ });
//! rt::present();
//! ```

use crate::ffi::{self, R3dDraw, R3dDrawUniform, R3dFrame, R3dList};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use f2_n64fmt::{File, MeshView};

/// A row-vector 4x4 matrix (`v' = v * M`, translation in row 3), as f2-engine `math.rs`.
pub type Mat4 = [[f32; 4]; 4];

pub const IDENTITY: Mat4 = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];

/// `DIST` (mdxRender.cpp:63).
pub const DIST: f32 = -10.0;
/// Far plane used for `noClip` draws (sky backdrop).
const NO_CLIP_FAR: f32 = 30000.0;

/// `mtx_cat` of f2-engine (affine row-vector product, `a` then `b`).
pub fn mtx_cat(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut r = [[0f32; 4]; 4];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    for j in 0..3 {
        r[3][j] = a[3][0] * b[0][j] + a[3][1] * b[1][j] + a[3][2] * b[2][j] + b[3][j];
    }
    r[3][3] = 1.0;
    r
}

/// Square root on the FPU-less fast path: bit-trick estimate + two Newton steps (relative
/// error < 1e-5; libm's `sqrtf` is a slow software loop on MIPS).
pub fn fast_sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut y = f32::from_bits((x.to_bits() >> 1) + 0x1FBD_1DF5);
    y = 0.5 * (y + x / y);
    0.5 * (y + x / y)
}

/// The game's view matrix (`Camera::view_matrix` of scene.rs = `gu_look_at(target, source,
/// up)`, mdxMath.cpp:136).
pub fn camera_view(source: [f32; 3], target: [f32; 3], up: [f32; 3]) -> Mat4 {
    let norm = |v: [f32; 3]| {
        let l = fast_sqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]);
        if l > 0.0 {
            [v[0] / l, v[1] / l, v[2] / l]
        } else {
            v
        }
    };
    let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let (eye, at) = (target, source);
    let view_dir = norm([eye[0] - at[0], eye[1] - at[1], eye[2] - at[2]]);
    let right = cross(up, view_dir);
    let up2 = norm(cross(view_dir, right));
    let right = norm(right);
    [
        [right[0], up2[0], view_dir[0], 0.0],
        [right[1], up2[1], view_dir[1], 0.0],
        [right[2], up2[2], view_dir[2], 0.0],
        [-dot(right, at), -dot(up2, at), -dot(view_dir, at), 1.0],
    ]
}

/// Frame lists in `DrawAllFrames` order (scene.rs `Frame`, minus `Glow`, which retail
/// never draws).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum List {
    Normal = 0,
    Wrap,
    Xlu,
    Additive,
    Late,
    LateAdditive,
    Overlay,
    /// The sky (`DrawBackground`, `noClip`) before everything else, without depth: on the PC
    /// it is the farthest geometry anyway, but its depth beyond the far plane saturates into
    /// the coarse far end of the RDP z-buffer, where it ties with (and hides) distant level
    /// geometry drawn after it.
    Background,
}

pub const NUM_LISTS: usize = 8;
pub const LIST_ORDER: [List; NUM_LISTS] =
    [List::Background, List::Normal, List::Wrap, List::Xlu, List::Additive, List::Late, List::LateAdditive, List::Overlay];
/// The PC frame lists (`DrawAllFrames` order; index = scene-file list number).
pub const FRAME_LISTS: [List; 7] = [List::Normal, List::Wrap, List::Xlu, List::Additive, List::Late, List::LateAdditive, List::Overlay];
/// Every list by its discriminant.
const BY_INDEX: [List; NUM_LISTS] =
    [List::Normal, List::Wrap, List::Xlu, List::Additive, List::Late, List::LateAdditive, List::Overlay, List::Background];

impl List {
    /// Render state of the list (renderer.rs `build()` passes): blend, depth, wrap, alpha
    /// reference (`tightAlphaCmpRS` > 100 → RDP threshold 101, `normalAlphaCmpRS` != 0 → 1).
    pub fn state(self, bilinear: bool, fog: bool) -> R3dList {
        let (blend, z, wrap, aref) = match self {
            List::Normal => (ffi::BLEND_SEMI, ffi::ZTEST | ffi::ZWRITE, false, 101),
            List::Wrap => (ffi::BLEND_SEMI, ffi::ZTEST | ffi::ZWRITE, true, 101),
            List::Xlu => (ffi::BLEND_SEMI, ffi::ZTEST, true, 1),
            List::Additive => (ffi::BLEND_ADD, ffi::ZTEST, true, 1),
            List::Late => (ffi::BLEND_SEMI, ffi::ZTEST, false, 1),
            List::LateAdditive => (ffi::BLEND_ADD, ffi::ZTEST, false, 1),
            List::Overlay => (ffi::BLEND_SEMI, 0, false, 1),
            List::Background => (ffi::BLEND_SEMI, 0, false, 101),
        };
        R3dList {
            blend,
            flags: z | if wrap { ffi::WRAP } else { 0 } | if bilinear { ffi::BILINEAR } else { 0 } | if fog && self != List::Overlay { ffi::FOG } else { 0 },
            alpha_ref: aref,
            cull: CULL_MODE,
        }
    }
}

/// magma culling mode reproducing `D3DCULL_CW` with MDX's screen-space winding
/// (verified in ares with the cull-test scene: see research/n64-performance.md).
pub const CULL_MODE: u32 = 3;

/// Camera and projection of a frame (`View` of scene.rs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewParams {
    /// World → view (`gu_look_at`, row-vector).
    pub view: Mat4,
    /// `FOV` (450 on PC).
    pub fov: f32,
    /// 3D viewport `[x, y, w, h]` in pixels.
    pub viewport: [f32; 4],
    /// `nearClip` (10).
    pub near: f32,
    /// `farClip`.
    pub far: f32,
    /// Bilinear filtering (`D3DTFN_LINEAR`, the PC default).
    pub bilinear: bool,
    /// Guard band in multiples of `FOV * scale` pixels from the centre (triangles beyond it
    /// are clipped on the RSP).
    pub guard: u32,
    /// Enhanced linear fog (`Fog` of scene.rs; retail: off).
    pub fog: Option<FogParams>,
}

/// Linear fog (`SetupFogParams`): view-space z range and colour (`0xRRGGBBAA`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FogParams {
    pub start: f32,
    pub end: f32,
    pub rgba: u32,
}

impl ViewParams {
    /// The PC defaults (`View::new`) for a `w` x `h` screen.
    pub fn pc_default(w: f32, h: f32) -> ViewParams {
        ViewParams { view: IDENTITY, fov: 450.0, viewport: [0.0, 0.0, w, h], near: 10.0, far: 1200.0, bilinear: true, guard: 6, fog: None }
    }
    /// `rXRes / 640` of `InitOneOverTable`.
    pub fn proj_scale(&self) -> f32 {
        self.viewport[2] / 640.0
    }
    /// Depth scale: screen z = vz * K (linear in view z like the PC's `sz = vz * 0.00025`),
    /// using the full 15-bit range up to the far plane.
    pub fn z_scale(&self) -> f32 {
        32767.0 / (self.far - DIST).max(1.0)
    }
    /// RDP depth (0..1, as `ffi::Vertex::z`) of a PC `TlVertex::sz` (`vz * 0.00025`), so CPU-side
    /// triangles (sprites, effects) share the RSP geometry's depth mapping.
    pub fn tl_z(&self, sz: f32) -> f32 {
        (sz * 4000.0 * self.z_scale() / 32767.0).clamp(0.0, 1.0)
    }
}

/// Per-draw parameters (`EmitCtx` of scene.rs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrawParams {
    /// Vertex alpha multiplier (`alpha_mul`: actor xlu fade).
    pub alpha_mul: f32,
    /// `SetObjectGouraudValues`: replaces the vertex colours' RGB.
    pub gouraud: Option<[f32; 3]>,
    /// `overrideTex`: every group uses this texture.
    pub override_tex: Option<TexId>,
    /// `noClip`: ignore the far plane (sky backdrop; never fogged).
    pub no_clip: bool,
    /// Fog per object (`PCRenderObject`'s `cFog`, the actor path) instead of per vertex
    /// (the landscape path). Approximated with the object origin's depth.
    pub object_fog: bool,
}

impl Default for DrawParams {
    fn default() -> Self {
        DrawParams { alpha_mul: 1.0, gouraud: None, override_tex: None, no_clip: false, object_fog: false }
    }
}

/// A registered texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TexId(pub u16);

/// A registered mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeshId(pub u32);

/// Resource counts of an [`R3d`] at some point ([`R3d::mark`]), to free everything loaded
/// after it ([`R3d::reset_to`], level unload).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct R3dMark {
    files: usize,
    meshes: usize,
    tex: u32,
    groups: u32,
}

/// Errors of [`R3d::load_file`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadError {
    Format(f2_n64fmt::Error),
    /// The C side ran out of texture/group slots.
    Full,
}

/// A C-side command block: one vertex-cache batch of a rigid mesh (culled by its bounding
/// sphere) or one texture group of a skinned mesh (never culled).
struct Part {
    handle: u16,
    /// The group texture's dense CRC id (`R3d::crc_tex`).
    cid: u16,
    tris: u16,
    /// Bounding sphere in stored position units (radius < 0: never culled).
    center: [f32; 3],
    radius: f32,
}

struct MeshRes {
    id: u32,
    parts: Vec<Part>,
    pos_scale: f32,
    skinned: bool,
    tris: u32,
    /// Sphere enclosing every part's sphere (parts without one are never culled; radius < 0:
    /// no part has one, e.g. skinned meshes): tested first, so wholly visible / invisible
    /// meshes skip the parts.
    center: [f32; 3],
    radius: f32,
    /// The mesh's vertices in its loaded file (16 bytes each, big-endian), read by the RSP's
    /// DMA: `MODGE`/`WAVE` meshes are rewritten in place each frame ([`R3d::animate_modge`]).
    verts: usize,
    n_verts: u32,
    modge: Option<alloc::boxed::Box<ModgeMesh>>,
    /// World-space positions (`mesh_flags::WORLD`): drawn with an identity model matrix.
    world: bool,
    /// Vertex colours split for the additive approximation ([`R3d::ensure_additive`]).
    add_split: bool,
}

/// Base data of a `MODGE`/`WAVE` mesh's vertices and the frame they were last animated for.
///
/// Every term of `PCCalcModgeValues` is `sin(z-phase + x-phase)` (one side holding a nested
/// sine), so the phases are resolved once per distinct x and z (a water grid shares them along
/// its rows and columns), and once per distinct (x, z) position (corners share positions) the
/// three sines are table lookups of the summed phases. All of it in integers (phases as `u32`
/// turns): the f32 version spent ~7 ms a frame in Garden 1 on float compares and saturating
/// float -> int conversions, an angle-addition version ~3 ms on products and table misses.
struct ModgeMesh {
    /// Per vertex: index into `pairs`; stored y, s, t, alpha.
    base: Vec<ModgeBase>,
    /// Distinct (index into `xph`, index into `zph`, stored alpha) of the vertices (corners
    /// share positions).
    pairs: Vec<(u16, u16, u8)>,
    /// Per frame scratch: per pair, the offsets of [`modge_pair`].
    pt: Vec<ModgePair>,
    /// Per distinct model-space x: the x parts of the phases of B, P, S; per z: of A, Q, R
    /// (see [`modge_x_terms`]).
    xph: Vec<[u32; 3]>,
    zph: Vec<[u32; 3]>,
    /// Per frame scratch: the summands of the three sines' phases per distinct x and z.
    xt: Vec<[u32; 3]>,
    zt: Vec<[u32; 3]>,
    /// Wave height in stored y units per Q10 `(sin + 1)`: `y += (S·ky) >> ky_sh`.
    ky: i32,
    ky_sh: u32,
    frame: u32,
    flags: u8,
    /// The per-vertex part on the RSP (`F2_Modge`; `None`: on the CPU).
    rsp: Option<ModgeRsp>,
}

/// `F2_Modge` descriptor bytes (the frame's header; in DMEM the x and z terms follow, 8
/// bytes each).
const MODGE_DESC_HEAD: usize = 56;
/// DMEM scratch of `F2_Modge` (magma's vertex cache + padding, rsp_f2mdx.S).
const MODGE_DMEM: usize = 1960;
/// The sine table as `F2_Modge` DMAs it (bytes; [`SinTab`] padded to 8).
const MODGE_TAB_BYTES: usize = 520;

/// RDRAM buffers of a mesh `F2_Modge` (rsp_f2modge.S) animates: the stored vertices, the
/// per-vertex position index, the position list (DMEM offsets of its x and z terms, stored
/// alpha << 7), the static x/z phases and a ring of three descriptors (the frame's header:
/// the RSP may still read the two previous frames' while the CPU writes the next).
struct ModgeRsp {
    buf: Vec<u64>,
    idx_off: usize,
    pos_off: usize,
    term_off: usize,
    desc_off: usize,
    /// The ring's stride.
    desc_stride: usize,
    /// DMEM offset of the position table (`MODGE_DMEM` - 8 x positions rounded up to 16).
    pt_off: u16,
    cur: usize,
    k: ModgeK16,
}

impl ModgeRsp {
    fn bytes(&mut self) -> &mut [u8] {
        // SAFETY: plain memory owned by `buf`.
        unsafe { core::slice::from_raw_parts_mut(self.buf.as_mut_ptr() as *mut u8, self.buf.len() * 8) }
    }
}

/// The sine table for `F2_Modge` in RDRAM (8-byte aligned for DMA, written back once).
fn modge_rsp_table() -> *const u8 {
    #[repr(C, align(16))]
    struct Tab([i16; MODGE_TAB_BYTES / 2]);
    static TABLE: spin::Once<Tab> = spin::Once::new();
    let t = TABLE.call_once(|| {
        let mut t = Tab([0; MODGE_TAB_BYTES / 2]);
        t.0[..257].copy_from_slice(sin_q14());
        R3d::c_writeback(unsafe { core::slice::from_raw_parts(t.0.as_ptr() as *const u8, MODGE_TAB_BYTES) });
        t
    });
    t.0.as_ptr() as *const u8
}

/// `Vec::shrink_to_fit` for [`R3d::reset_to`]'s list of vectors.
trait ShrinkVec {
    fn shrink(&mut self);
}
impl<T> ShrinkVec for Vec<T> {
    fn shrink(&mut self) {
        self.shrink_to_fit();
    }
}

/// The physical (RDRAM) address of a pointer into cached memory.
fn phys(p: *const u8) -> u32 {
    p as u32 & 0x1FFF_FFFF
}

/// Distinct values of `v` (by bit pattern) and each element's index into them.
fn distinct(v: &[f32]) -> (Vec<f32>, Vec<u16>) {
    let mut order: Vec<u32> = (0..v.len() as u32).collect();
    order.sort_unstable_by_key(|&i| v[i as usize].to_bits());
    let (mut vals, mut idx) = (Vec::new(), alloc::vec![0u16; v.len()]);
    for &i in &order {
        let x = v[i as usize];
        if vals.last().map(|l: &f32| l.to_bits()) != Some(x.to_bits()) {
            vals.push(x);
        }
        idx[i as usize] = (vals.len() - 1) as u16;
    }
    (vals, idx)
}

/// The sine table of [`isin`]: Q14 at 256 phases over one period (+1 for interpolation).
/// Small (512 bytes) so that it stays in the D-cache: with a 2 KiB table most lookups missed.
type SinTab = [i16; 257];

fn sin_q14() -> &'static SinTab {
    static TABLE: spin::Once<SinTab> = spin::Once::new();
    TABLE.call_once(|| core::array::from_fn(|i| libm::round(libm::sin(i as f64 * (core::f64::consts::TAU / 256.0)) * 16384.0) as i16))
}

/// `(sin + 1)` of a phase in Q10 (0..2048).
#[inline(always)]
fn sinp1(t: &SinTab, ph: u32) -> i32 {
    (isin(t, ph) + 16384 + 8) >> 4
}

/// An angle in radians as a phase (`2^32` = one turn).
fn phase(rad: f64) -> u32 {
    let turns = rad / core::f64::consts::TAU;
    ((turns - libm::floor(turns)) * 4_294_967_296.0) as u64 as u32
}

/// Q14 sine of a phase (table + linear interpolation; |error| < 2.5 / 16384).
#[inline(always)]
fn isin(t: &SinTab, ph: u32) -> i32 {
    let i = (ph >> 24) as usize;
    let f = ((ph >> 8) & 0xffff) as i32;
    let a = t[i] as i32;
    a + (((t[i + 1] as i32 - a) * f) >> 16)
}

#[inline(always)]
fn icos(t: &SinTab, ph: u32) -> i32 {
    isin(t, ph.wrapping_add(1 << 30))
}

/// The phase of a Q14 value taken as radians (`2^32 / 2π / 16384` ≈ 41721.6 per unit).
#[inline(always)]
fn q14_phase(v: i32) -> u32 {
    v.wrapping_mul(41722) as u32
}

/// Per frame `t` phases of `PCCalcModgeValues`' terms: B, P, S (x side), A, Q, R (z side):
///   A = z·1.5/35 + t·1.5/70   B = sin(x·1.3/49 + t·2/56)
///   P = x·1.5/49 + t·1.5/66   Q = cos(z·1.3/29 + t·2/66)
///   R = z·1.2/26 + t/26       S = cos(x·1.3/49 + t/26)
/// so that mx = sin(A + B), mz = cos(P + Q), my = sin(R + S).
fn modge_time_phases(fc: u32) -> [u32; 6] {
    // Integer: the per-frame phase steps in 2^-48 turns (f64 maths, `floor` and f64 -> u64
    // conversions cost ~20 us per call on the VR4300).
    const fn step(rad: f64) -> u64 {
        (rad / core::f64::consts::TAU * 281_474_976_710_656.0 + 0.5) as u64
    }
    const K: [u64; 6] = [step(2.0 / 56.0), step(1.5 / 66.0), step(1.0 / 26.0), step(1.5 / 70.0), step(2.0 / 66.0), step(1.0 / 26.0)];
    K.map(|k| ((fc as u64).wrapping_mul(k) >> 16) as u32)
}

/// The x-only phases (B, P, S) and z-only phases (A, Q, R) of a coordinate.
fn modge_x_phases(x: f32) -> [u32; 3] {
    let x = x as f64;
    [phase(x * (1.3 / 49.0)), phase(x * (1.5 / 49.0)), phase(x * (1.3 / 49.0))]
}
fn modge_z_phases(z: f32) -> [u32; 3] {
    let z = z as f64;
    [phase(z * (1.5 / 35.0)), phase(z * (1.3 / 29.0)), phase(z * (1.2 / 26.0))]
}

/// The x and z summands of the three sines' phases: per x [B, P, S], per z [A, Q + π/2, R]
/// (B, Q, S hold the nested sines), so that mx = sin(A + B), mz = cos(P + Q) =
/// sin(P + Q + π/2), my = sin(R + S). `tp` from [`modge_time_phases`]; S and R only with `wave`.
#[inline(always)]
fn modge_x_terms(t: &SinTab, ph: &[u32; 3], tp: &[u32; 6], wave: bool) -> [u32; 3] {
    let b = q14_phase(isin(t, ph[0].wrapping_add(tp[0])));
    let p = ph[1].wrapping_add(tp[1]);
    let s = if wave { q14_phase(icos(t, ph[2].wrapping_add(tp[2]))) } else { 0 };
    [b, p, s]
}
#[inline(always)]
fn modge_z_terms(t: &SinTab, ph: &[u32; 3], tp: &[u32; 6], wave: bool) -> [u32; 3] {
    let a = ph[0].wrapping_add(tp[3]);
    let q = q14_phase(icos(t, ph[1].wrapping_add(tp[4]))).wrapping_add(1 << 30);
    let r = if wave { ph[2].wrapping_add(tp[5]) } else { 0 };
    [a, q, r]
}

/// A `MODGE`/`WAVE` vertex's stored values: index into `ModgeMesh::pairs`, y, s, t.
#[derive(Clone, Copy)]
struct ModgeBase {
    pair: u16,
    y: i16,
    s: i16,
    t: i16,
}

/// The offsets of one (x, z, alpha) position at a frame: y, s, t to add (0 for the parts
/// `flags` leave alone) and the new alpha (written only when [`modge_writes_alpha`]).
#[derive(Clone, Copy, Default)]
struct ModgePair {
    dy: i16,
    ds: i16,
    dt: i16,
    a: u8,
}

/// Whether `flags` animate the alpha (MODGE and WAVE).
fn modge_writes_alpha(flags: u8) -> bool {
    flags & 3 == 3
}

/// The per-position part of [`modge_vertex`] from the x/z terms ([`modge_x_terms`]) and the
/// stored alpha `a0`. `ky`/`ky_sh` as in [`ModgeMesh`]; `flags` bit 0 MODGE (s, t), bit 1
/// WAVE (y).
#[inline(always)]
fn modge_pair(t: &SinTab, px: &[u32; 3], pz: &[u32; 3], a0: u8, ky: i32, ky_sh: u32, flags: u8) -> ModgePair {
    // tM? = (m + 1)·0.1 (·5 for y); S? = (m + 1) in Q10.
    let sx = sinp1(t, px[0].wrapping_add(pz[0]));
    let sz = sinp1(t, px[1].wrapping_add(pz[1]));
    let sy = sinp1(t, px[2].wrapping_add(pz[2]));
    // PC: alpha = 50 + (tMx+tMz)·400 (130 on average), scaled around the stored alpha:
    // a0·(50·1024 + (Sx+Sz)·40) / (130·1024), with 63/2^23 ≈ 1/133120.
    let num = (51200 + (sx + sz) * 40).max(0) as u32;
    let (modge, wave) = (flags & 1 != 0, flags & 2 != 0);
    ModgePair {
        dy: if wave { ((sy * ky + (1 << (ky_sh - 1))) >> ky_sh).clamp(-32768, 32767) as i16 } else { 0 },
        // s += 0.4·1024·tMx = S·40.96/1024 (41943 = 40.96·1024).
        ds: if modge { ((sx * 41943 + (1 << 19)) >> 20) as i16 } else { 0 },
        dt: if modge { ((sz * 41943 + (1 << 19)) >> 20) as i16 } else { 0 },
        a: ((a0 as u32).wrapping_mul(num.wrapping_mul(63)).wrapping_add(1 << 22) >> 23).min(255) as u8,
    }
}

/// A vertex's (y, s, t) from its stored values and its position's [`ModgePair`] (what
/// `F2_Modge` computes on the RSP).
#[inline(always)]
fn modge_apply(q: &ModgePair, y0: i16, s0: i16, t0: i16) -> (i16, i16, i16) {
    let sat16 = |v: i32| v.clamp(-32768, 32767) as i16;
    (sat16(y0 as i32 + q.dy as i32), sat16(s0 as i32 + q.ds as i32), sat16(t0 as i32 + q.dt as i32))
}

/// `F2_Modge`'s per-mesh constants (Q15 multipliers of `vmulf`): y per Q10 `(sin + 1)`
/// (WAVE; 0 without), s and t per Q10 `(sin + 1)` (MODGE; 0 without).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ModgeK16 {
    kyh: i16,
    kst: i16,
}

fn modge_k16(scale: f32, flags: u8) -> ModgeK16 {
    // dy = S·5/(1024·scale), ds = S·0.04 (0.4·1024·0.1/1024): vmulf(S, k) = S·k/32768.
    let ky = libm::round(5.0 / (1024.0 * scale as f64) * 32768.0).min(32767.0) as i16;
    ModgeK16 { kyh: if flags & 2 != 0 { ky } else { 0 }, kst: if flags & 1 != 0 { 1311 } else { 0 } }
}

/// RSP `VMULF` of one lane: `(a·b·2 + 0x8000) >> 16`, clamped to s16.
#[inline(always)]
fn rsp_vmulf(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64 * 2 + 0x8000) >> 16).clamp(-32768, 32767) as i32
}

/// RSP `VMUDM` of one lane: signed `a` × unsigned `b`, `>> 16`, clamped to s16.
#[inline(always)]
fn rsp_vmudm(a: i32, b: u32) -> i32 {
    ((a as i64 * b as i64) >> 16).clamp(-32768, 32767) as i32
}

/// `F2_Modge`'s `(sin + 1)` in Q10 of a 16-bit phase (the [`SinTab`] interpolated on 8 bits).
#[inline(always)]
fn rsp_sinp1(t: &SinTab, ph: u16) -> i32 {
    let i = (ph >> 8) as usize;
    let (a, d) = (t[i] as i32, t[i + 1] as i32 - t[i] as i32);
    let s = (a + rsp_vmudm(d, ((ph & 0xFF) as u32) << 8)).clamp(-32768, 32767);
    rsp_vmudm(s + 8, 4096) + 1024
}

/// The RSP's (`F2_Modge`) offsets of one position from its 16-bit x and z phase summands and
/// stored alpha (bit-exact model of rsp_f2mdx.S).
fn modge_pair16(t: &SinTab, x: &[u16; 3], z: &[u16; 3], a0: u8, k: ModgeK16) -> ModgePair {
    let (sx, sz, sy) = (rsp_sinp1(t, x[0].wrapping_add(z[0])), rsp_sinp1(t, x[1].wrapping_add(z[1])), rsp_sinp1(t, x[2].wrapping_add(z[2])));
    // alpha = a0·(1280 + Sx + Sz)/3328 (= a0·(50 + (tMx + tMz)·400)/130).
    let y = rsp_vmudm((a0 as i32) << 7, (sx + sz + 1280) as u32);
    ModgePair {
        dy: rsp_vmulf(sy, k.kyh as i32) as i16,
        ds: rsp_vmulf(sx, k.kst as i32) as i16,
        dt: rsp_vmulf(sz, k.kst as i32) as i16,
        a: rsp_vmulf(y, 5041).min(255) as u8,
    }
}

/// Q14 sine of a 16-bit phase as `F2_Modge` computes it.
#[inline(always)]
fn rsp_isin(t: &SinTab, ph: u16) -> i32 {
    let i = (ph >> 8) as usize;
    let (a, d) = (t[i] as i32, t[i + 1] as i32 - t[i] as i32);
    (a + rsp_vmudm(d, ((ph & 0xFF) as u32) << 8)).clamp(-32768, 32767)
}

/// A Q14 value taken as radians, as a 16-bit phase (`VMUDM` by 41722 = 2^32/2π/2^14/2^16·2^16).
#[inline(always)]
fn rsp_q14ph(v: i32) -> u16 {
    rsp_vmudm(v, 41722) as u16
}

/// `F2_Modge`'s x summands [B, P, S] from the static 16-bit x phases and the time phases
/// (`modge_x_terms` in 16 bits).
fn modge_x16(t: &SinTab, b: &[u16; 3], tp: &[u16; 6]) -> [u16; 3] {
    [rsp_q14ph(rsp_isin(t, b[0].wrapping_add(tp[0]))), b[1].wrapping_add(tp[1]), rsp_q14ph(rsp_isin(t, b[2].wrapping_add(tp[2]).wrapping_add(0x4000)))]
}

/// `F2_Modge`'s z summands [A, Q + π/2, R] (`modge_z_terms` in 16 bits).
fn modge_z16(t: &SinTab, b: &[u16; 3], tp: &[u16; 6]) -> [u16; 3] {
    let q = (rsp_vmudm(rsp_isin(t, b[1].wrapping_add(tp[4]).wrapping_add(0x4000)), 41722) + 16384) as u16;
    [b[0].wrapping_add(tp[3]), q, b[2].wrapping_add(tp[5])]
}

/// Static or time phases in the 16 bits `F2_Modge` uses.
fn ph16<const N: usize>(p: &[u32; N]) -> [u16; N] {
    p.map(|v| (v >> 16) as u16)
}

/// One vertex of `F2_Modge` (MODGE and WAVE) at coordinate (x, z) and frame `fc`.
#[cfg(test)]
fn modge_vertex16(x: f32, z: f32, fc: u32, y0: i16, s0: i16, t0: i16, a0: u8, k: ModgeK16) -> (i16, i16, i16, u8) {
    let (t, tp) = (sin_q14(), ph16(&modge_time_phases(fc)));
    let (px, pz) = (modge_x16(t, &ph16(&modge_x_phases(x)), &tp), modge_z16(t, &ph16(&modge_z_phases(z)), &tp));
    let q = modge_pair16(t, &px, &pz, a0, k);
    let (y, s, t) = modge_apply(&q, y0, s0, t0);
    (y, s, t, q.a)
}

/// One vertex of [`R3d::animate_modge`] (MODGE and WAVE): (y, s, t, alpha) from the stored
/// values and the x/z terms.
#[cfg(test)]
fn modge_vertex(px: &[u32; 3], pz: &[u32; 3], y0: i16, s0: i16, t0: i16, a0: u8, ky: i32, ky_sh: u32) -> (i16, i16, i16, u8) {
    let q = modge_pair(sin_q14(), px, pz, a0, ky, ky_sh, 3);
    let (y, s, t) = modge_apply(&q, y0, s0, t0);
    (y, s, t, q.a)
}

/// Draw slot `i`'s last draw (see [`R3d::draw_slot`]): reusable while nothing it depends
/// on changed. 32 bytes (two D-cache lines) in one dense array; the uniforms, written only when
/// they change, are in [`SlotUni`].
#[repr(C, align(16))]
#[derive(Clone, Copy, Default)]
struct Slot {
    /// `R3d::frame_no` of the draw (only the previous frame's are reused; whether the view
    /// changed since is `R3d::view_same`).
    frame: u32,
    /// The model matrix's stamp (`MeshDraw::stamp`).
    stamp: u32,
    /// Camera-independent culling data, valid for `stamp`/`mesh`: the radius scale bound and
    /// the world-space centre of the mesh's sphere. A frame with only a new view (the camera
    /// moved) re-culls from these.
    s: f32,
    wc: [f32; 3],
    mesh: u16,
    /// Visible parts in that frame's `vis` (`R3d::prev_vis` in the next frame).
    vis_first: u16,
    vis_count: u16,
    /// Interned parameters ([`R3d::intern_params`]; `PID_NONE`: not reusable).
    pid: u8,
    /// Current uniform of the ring ([`SlotUni::u`], bits 0-1) and `SLOT_*` bits.
    cf: u8,
}

/// `SlotUni::u[cur]` is the slot's uniform (its model, mesh and parameters) with some fog.
const SLOT_U: u8 = 8;
/// ... and those are the no-fog fields ([`fog_fields`] without fog).
const SLOT_U_NOFOG: u8 = 16;
const SLOT_CUR: u8 = 3;

/// A slot's uniforms: a ring of three (a change writes the next one, as the RSP may still read
/// the one of each of the two frames before: the CPU starts recording frame N before waiting
/// for a framebuffer, i.e. while N-2 and N-1 can still be in flight).
#[derive(Clone, Copy, Default)]
struct SlotUni {
    u: [R3dDrawUniform; 3],
}

/// Uniform rings per [`SlotUni`] chunk (chunks never move: the RSP reads the uniforms in place).
const SLOT_CHUNK: usize = 32;
/// A draw slot without a uniform ring (`R3d::slot_ui`).
const NO_UNI: u16 = u16::MAX;
/// A uniform ring unused for this many frames is no longer read by the RSP (frames N-1 and
/// N-2 can be in flight while N records) and can go to another slot.
const UNI_REUSE_FRAMES: u32 = 3;

/// No draw slot (not reusable).
const NO_SLOT: u16 = u16::MAX;
/// Parameter ids `>= PID_NONE` are not stored in slots (the table is reset between frames
/// when it grows past [`PID_RESET`]).
const PID_NONE: u16 = 255;
const PID_RESET: usize = 192;
/// Buckets of the parameter hash (power of two, > 2 x `PID_NONE`).
const PHASH: usize = 512;

/// A model matrix's three columns (the fourth is 0, 0, 0, 1).
type Model34 = [[f32; 3]; 4];
const IDENTITY34: Model34 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, 0.0]];

fn to34(m: &Mat4) -> Model34 {
    [[m[0][0], m[0][1], m[0][2]], [m[1][0], m[1][1], m[1][2]], [m[2][0], m[2][1], m[2][2]], [m[3][0], m[3][1], m[3][2]]]
}
fn to44(m: &Model34) -> Mat4 {
    [[m[0][0], m[0][1], m[0][2], 0.0], [m[1][0], m[1][1], m[1][2], 0.0], [m[2][0], m[2][1], m[2][2], 0.0], [m[3][0], m[3][1], m[3][2], 1.0]]
}

/// A draw recorded during the game's draw loop ([`R3d::queue`]) and queued to the RSP lists by
/// [`R3d::run_queue`] after it: the draw loop walks actors and models between two draws and
/// evicts the renderer's data (and code) from the caches, so the per-draw work runs in one
/// batch instead. 28 bytes: the model matrix goes to `R3d::slot_model` (only when its stamp
/// changed) or `R3d::qmodels`. Draws recorded with [`R3d::queue_raw`] carry the game's model
/// id and object instead of a mesh until [`R3d::resolve_draw`].
#[derive(Clone, Copy)]
struct QDraw {
    /// Matrix stamp (rigid; with `NO_SLOT`: index into `qmodels`) or first bone in `qbones`
    /// (skinned).
    stamp: u32,
    modge_frame: u32,
    alpha: f32,
    /// The game's model id (`queue_raw`).
    model: u32,
    /// Draw slot (rigid) or bone count (skinned).
    slot: u16,
    /// Mesh, `MESH_UNRESOLVED` (`queue_raw`) or `MESH_SKIP`.
    mesh: u16,
    /// Key of the slot's model matrix with the stamp (the object of `queue_raw`, the mesh of
    /// `queue`).
    key: u16,
    /// [`TexId`] + 1 (0: none).
    ovr: u16,
    /// Index + 1 into `qgouraud` (0: none).
    gour: u16,
    /// List (bits 0-2) and `Q_*` bits.
    lf: u8,
    modge: u8,
}

const Q_SKINNED: u8 = 8;
/// Testing (`F2N64_CULL_CHECK`): a draw of an actor the draw loop's actor test culled;
/// [`R3d::run_queue_with`] checks that it culls the draw wholly.
pub const Q_CHECK_CULLED: u8 = 64;
const Q_NO_CLIP: u8 = 16;
const Q_OBJECT_FOG: u8 = 32;
/// `QDraw::mesh` of a raw draw not resolved yet / of a draw not to draw.
const MESH_UNRESOLVED: u16 = 0xFFFE;
const MESH_SKIP: u16 = 0xFFFF;

/// A [`R3d::queue_raw`] draw whose mesh [`R3d::run_queue_with`] asks its resolver for (the
/// draw slot's memo had another model/object).
#[derive(Clone, Copy, Debug)]
pub struct RawDraw {
    pub model: u32,
    pub object: u16,
    pub list: List,
    pub alpha: f32,
    pub no_clip: bool,
}

/// Bit pattern of a draw's parameters (cache key).
fn params_key(p: &DrawParams) -> [u32; 6] {
    let g = p.gouraud.map_or([u32::MAX; 3], |g| g.map(f32::to_bits));
    [p.alpha_mul.to_bits(), g[0], g[1], g[2], p.override_tex.map_or(0, |t| t.0 as u32 + 1), p.no_clip as u32 | (p.object_fog as u32) << 1]
}

/// A part as the flush needs it (`R3d::gparts`).
#[derive(Clone, Copy)]
struct GPart {
    handle: u16,
    cid: u16,
    tris: u16,
}

/// A queued draw of a frame list (16 bytes on the N64).
struct DrawRec {
    /// The uniform at `uptr` (a slot's), or (null) the current frame's arena entry `u`.
    uptr: *const R3dDrawUniform,
    u: u16,
    /// The mesh's first part in `R3d::gparts` (`MeshHot::sph`).
    p0: u16,
    tex_override: u16,
    /// Visible parts: `R3d::vis[vis_first..vis_first + vis_count]` (indices into `parts`).
    vis_first: u16,
    vis_count: u16,
    /// Skinned: the bone uniforms are `u..u + n_bones` of the arena (0: rigid).
    n_bones: u8,
    /// Drawn with the sky's projection (`noClip`: far plane at `NO_CLIP_FAR`).
    no_clip: bool,
}

/// The per-draw data of a mesh in one 32-byte record (`R3d::mhot`, parallel to `meshes`).
#[repr(C, align(16))]
#[derive(Clone, Copy, Default)]
struct MeshHot {
    /// Sphere enclosing every part's sphere ([`MeshRes::center`]; radius < 0: none).
    center: [f32; 3],
    radius: f32,
    pos_scale: f32,
    tris: u32,
    /// The parts' spheres: `R3d::spheres[sph..sph + n]`.
    sph: u32,
    n: u16,
    /// `MH_*` bits.
    flags: u8,
    _pad: u8,
}

/// World-space positions (drawn with an identity model matrix).
const MH_WORLD: u8 = 1;
/// Vertex colours split for the additive approximation.
const MH_ADD_SPLIT: u8 = 2;
/// Some part has no bounding sphere (never culled).
const MH_OPEN: u8 = 4;

/// Per-frame constants of the bounding-sphere test (see [`R3d::draw`]): world-space planes
/// `p · (x, y, z, 1)` scaled so that a sphere of radius `r` is wholly outside a plane when
/// its centre's value is `>= r`. Order: left, right, top, bottom (the four screen-edge planes
/// through the eye, moved one pixel outwards), near, far.
#[derive(Clone, Copy, Debug, Default)]
struct Frustum {
    p: [[f32; 4]; 6],
}

impl Frustum {
    /// The planes of view x projection `vp` (columns X, Y, -, W), the screen-edge slopes `a`
    /// (`x <= a * w` inside, ...) and near/far in `W` units.
    fn new(vp: &Mat4, a: [f32; 4], near: f32, far: f32) -> Frustum {
        let col = |k: usize| [vp[0][k], vp[1][k], vp[2][k], vp[3][k]];
        let (x, y, w) = (col(0), col(1), col(3));
        let comb = |sx: f32, sy: f32, aw: f32| -> [f32; 4] {
            // (sx*X + sy*Y - aw*W) / sqrt(1 + aw^2) (with a little slack: conservative).
            let n = 1.0 / (fast_sqrt(1.0 + aw * aw) * 1.0001);
            core::array::from_fn(|i| (sx * x[i] + sy * y[i] - aw * w[i]) * n)
        };
        let mut nearp: [f32; 4] = core::array::from_fn(|i| -w[i]);
        nearp[3] += near;
        let mut farp = w;
        farp[3] -= far;
        Frustum { p: [comb(1.0, 0.0, a[0]), comb(-1.0, 0.0, a[1]), comb(0.0, 1.0, a[2]), comb(0.0, -1.0, a[3]), nearp, farp] }
    }

    /// Sphere test: `None` = wholly outside a plane (culled), else the mask of the planes the
    /// sphere straddles (0: wholly inside). `planes` = 5 without the far plane (`noClip`),
    /// else 6.
    #[inline(always)]
    fn classify(&self, c: &[f32; 3], r: f32, planes: usize) -> Option<u32> {
        let mut straddle = 0;
        for (k, p) in self.p[..planes].iter().enumerate() {
            let d = c[0] * p[0] + c[1] * p[1] + c[2] * p[2] + p[3];
            if d >= r {
                return None;
            }
            if d >= -r {
                straddle |= 1 << k;
            }
        }
        Some(straddle)
    }
}

/// A sphere enclosing the bounding spheres of the parts that have one (radius < 0 if none
/// has; parts without a sphere are never culled).
fn enclosing_sphere(parts: &[Part]) -> ([f32; 3], f32) {
    let parts: Vec<&Part> = parts.iter().filter(|p| p.radius >= 0.0).collect();
    if parts.is_empty() {
        return ([0.0; 3], -1.0);
    }
    if let [p] = parts[..] {
        return (p.center, p.radius);
    }
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in &parts {
        for k in 0..3 {
            lo[k] = lo[k].min(p.center[k] - p.radius);
            hi[k] = hi[k].max(p.center[k] + p.radius);
        }
    }
    let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5];
    let r = parts
        .iter()
        .map(|p| {
            let d = [p.center[0] - c[0], p.center[1] - c[1], p.center[2] - c[2]];
            fast_sqrt(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]) * 1.0001 + p.radius
        })
        .fold(0.0f32, f32::max);
    (c, r * 1.0001 + 1.0)
}

/// The renderer (one per program).
/// Measurement: bytes of padding before the struct (`F2N64_PADX`), to sample data layouts.
const PADX: usize = match option_env!("F2N64_PADX") { Some(v) => parse_usize(v), None => 0 };
const fn parse_usize(v: &str) -> usize {
    let (b, mut i, mut n) = (v.as_bytes(), 0, 0);
    while i < b.len() {
        n = n * 10 + (b[i] - b'0') as usize;
        i += 1;
    }
    n
}

#[repr(C)]
pub struct R3d {
    _pad: [u8; PADX],
    // Per-draw state first and contiguous (repr(C)): the D-cache is 8 KiB direct-mapped,
    // and with these fields spread over the struct, layout changes alone moved the batch
    // by 30 % (some of them sharing cache sets with each other or the stack).
    cull: Frustum,
    frame_no: u32,
    /// The view equals the previous frame's.
    view_same: bool,
    plast: u16,
    uni_frame: usize,
    /// Draws reused from the previous frame (statistics).
    pub cache_hits: u32,
    /// Draws that reused their model-side work with a new view (see `draw_slot`).
    pub view_hits: u32,
    /// Draw records queued so far (all frames; [`R3d::animate_modge`] runs only for queued
    /// draws).
    pub pushed: u32,
    /// Triangles of the meshes queued this frame.
    pub queued_tris: u32,
    /// Triangles sent to the RSP after bounding-sphere culling (the RSP still rejects
    /// faces exactly like MDX).
    pub sent_tris: u32,
    /// Bumped when the view changes between frames (invalidates `cache`).
    view_gen: u32,
    /// Per draw slot: the last draw queued in it.
    slots: Vec<Slot>,
    /// Per draw slot: its uniform ring in `rings` (`NO_UNI`: none). Slots get rings only when
    /// they draw: a level's slots count every draw of the frame, culled actors' included
    /// (640+ in heavy levels, ~70 drawn), and unused rings go back to the pool
    /// ([`R3d::slot_ring`]).
    slot_ui: Vec<u16>,
    /// Slot uniform rings by chunk, with per ring the owning slot (`NO_SLOT`: free) and the last
    /// frame that queued it.
    rings: Vec<alloc::boxed::Box<[SlotUni; SLOT_CHUNK]>>,
    uni_owner: Vec<u16>,
    uni_used: Vec<u32>,
    uni_free: Vec<u16>,
    /// Per draw slot: the (model, object) of the last raw draw in it and its mesh
    /// ([`R3d::run_queue_with`]).
    slot_key: Vec<(u32, u16, u16)>,
    /// Raw draws resolved this frame (statistics).
    pub drawn: u32,
    /// Resolve every raw draw (no memo).
    resolve_all: bool,
    /// Per draw slot: the model matrix of the last (stamp, key) recorded in it.
    /// Testing (`F2N64_CULL_CHECK`): OR-ed into the `lf` of [`queue_raw`](Self::queue_raw)
    /// draws ([`Q_CHECK_CULLED`]), and the counts of [`cull_check_draw`](Self::cull_check_draw).
    pub extra_lf: u8,
    cull_checked: u32,
    cull_wrong: u32,
    slot_model: Vec<(u32, u16, Model34)>,
    /// Per mesh: the per-draw data; the parts' bounding spheres of all meshes.
    mhot: Vec<MeshHot>,
    spheres: Vec<[f32; 4]>,
    vis: Vec<u16>,
    /// The previous frame's `vis` (reused draws copy their parts from it).
    prev_vis: Vec<u16>,
    lists: [Vec<DrawRec>; NUM_LISTS],
    /// Per-frame uniform arenas, cycled over 3 frames (the RSP reads a frame's uniforms by
    /// DMA while the CPU already queues the next one; triple-buffered display = at most 2
    /// frames in flight).
    uni: [Vec<R3dDrawUniform>; 3],
    /// Draws recorded since `begin_frame` ([`R3d::queue`]), bone matrices of the skinned ones,
    /// model matrices of the ones without a slot.
    q: Vec<QDraw>,
    qbones: Vec<Mat4>,
    qmodels: Vec<Model34>,
    qgouraud: Vec<[f32; 3]>,
    /// MODGE/WAVE meshes found visible by `run_queue` (animated after its loop).
    modge_todo: Vec<(MeshId, u32, u8)>,
    /// Interned draw parameters ([`R3d::intern_params`]) and their keys; the last id.
    ptab: Vec<DrawParams>,
    pkeys: Vec<[u32; 6]>,
    /// Hash of `pkeys` (id + 1, 0 = empty; ids < `PID_NONE` only).
    phash: alloc::boxed::Box<[u16; PHASH]>,
    /// View x projection (f32) of the frame, for culling and per-object fog.
    vp: Mat4,
    view: ViewParams,
    // Load-time / flush-time / measurement state.
    /// F2NM files (meshes and textures point into them).
    files: Vec<Vec<u8>>,
    tex_by_crc: BTreeMap<u32, u16>,
    /// Dense ids of the group texture CRCs and each one's current texture (0: none).
    crc_id: BTreeMap<u32, u16>,
    crc_tex: Vec<u16>,
    /// Per part of every mesh (index = `MeshHot::sph` + part): command block, texture, size.
    gparts: Vec<GPart>,
    meshes: Vec<MeshRes>,
    handle_scratch: Vec<u16>,
    tex_scratch: Vec<u16>,
    draw_scratch: Vec<R3dDraw>,
    frame: R3dFrame,
    /// `frame` with the sky's far plane (draws with `no_clip`).
    frame_noclip: R3dFrame,
    /// Measurement (polygon budget, 2026-10): draws and triangles per on-screen radius class
    /// (< 1, < 1.5, < 3 px; whole-mesh bounding sphere), summed with all draws and frames;
    /// logged every 300 frames.
    size_classes: [(u32, u32); 3],
    size_all: (u32, u32),
    size_frames: u32,
    /// `F2_Modge` commands of this frame (mesh, descriptor, size): queued at the flush, before
    /// the lists.
    modge_cmds: Vec<(u32, *const u8, u32)>,
    /// Measurement (`F2N64_R3D_PROF=1`): C0 ticks and counts per phase, logged every 300 frames.
    pub prof: R3dProf,

}

/// Measurement counters (`F2N64_R3D_PROF=1`).
#[derive(Clone, Copy, Debug, Default)]
pub struct R3dProf {
    pub frames: u32,
    pub cull_ticks: u32,
    pub cull_calls: u32,
    pub sphere_tests: u32,
    pub modge_ticks: u32,
    pub modge_runs: u32,
    pub full_ticks: u32,
    pub full_draws: u32,
    pub view_ticks: u32,
    pub view_draws: u32,
    pub skin_ticks: u32,
    pub skin_draws: u32,
    pub record_ticks: u32,
    pub records: u32,
    pub run_ticks: u32,
    pub flush_rust: u32,
    pub flush_state: u32,
    pub flush_draws: u32,
}

pub const R3D_PROF: bool = option_env!("F2N64_R3D_PROF").is_some();

#[inline(always)]
pub fn prof_ticks() -> u32 {
    #[cfg(target_vendor = "nintendo64")]
    if R3D_PROF {
        return crate::rt::ticks();
    }
    0
}

/// Testing (`F2N64_CVT_CHECK=1`): the shader's float -> s15.16 conversion (rsp_f2mdx.S
/// F2_Convert) against [`to_fixed`] on edge cases and random values of all magnitudes (logged).
#[cfg(target_vendor = "nintendo64")]
fn cvt_check() {
    let mut vals: Vec<f32> = alloc::vec![
        0.0, -0.0, 1.0, -1.0, 0.5, 1e-6, -1e-6, 1.0 / 65536.0, -1.0 / 65536.0, 1.5 / 65536.0, -1.5 / 65536.0, 32767.99, -32767.99, 32768.0,
        -32768.0, 1e9, -1e9, f32::INFINITY, f32::NEG_INFINITY, f32::NAN, 1e-40, -1e-40, 255.996, -0.999_999_9, 0.125, 1.0e-5, 7.629_394_5e-6f32,
        -(7.629_394_5e-6f32), 16384.0, -16384.0,
    ];
    let mut seed = 0x1234_5678u32;
    while vals.len() < 12 * 160 {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let mag = [1.0f32, 0.01, 30.0, 3000.0, 32000.0, 1e-4, 1e6][(seed >> 29) as usize % 7];
        vals.push(((seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 2.0 * mag);
    }
    let n = vals.len() / 12;
    let (mut oi, mut of) = (alloc::vec![0i16; 16 * n], alloc::vec![0u16; 16 * n]);
    unsafe { ffi::n64_r3d_cvt_check(vals.as_ptr(), n as u32, oi.as_mut_ptr(), of.as_mut_ptr()) };
    let (mut bad, mut first) = (0, None);
    for (k, &v) in vals.iter().enumerate().take(12 * n) {
        let (g, r, c) = (k / 12, k % 12 / 3, k % 3);
        let got = (oi[16 * g + 4 * r + c], of[16 * g + 4 * r + c]);
        if got != to_fixed(v) {
            bad += 1;
            first.get_or_insert((v, got, to_fixed(v)));
        }
    }
    crate::log!("r3d: RSP float -> s15.16 check: {} values, {} mismatches, first {:?}", 12 * n, bad, first);
}

/// f32 → s15.16 split into (integer, fraction) halves (saturating; truncated towards zero
/// like `trunc.w.s`, NaN → 0).
///
/// Integer-only (exponent / mantissa shifts): on the VR4300 the FPU path (multiply, two
/// range compares, truncate, move) costs several times more, and a draw converts 16 values.
#[cfg_attr(not(target_vendor = "nintendo64"), inline)]
#[cfg_attr(target_vendor = "nintendo64", inline(never), link_section = ".text.f2hot_q")]
pub fn to_fixed(v: f32) -> (i16, u16) {
    let b = v.to_bits();
    let e = ((b >> 23) & 0xFF) as i32;
    // |v| * 65536 = m * 2^(e - 127 - 23 + 16), m = 1.mantissa as a 24-bit integer.
    let sh = e - 134;
    let m = (b & 0x7F_FFFF) | 0x80_0000;
    let mag: u32 = if sh >= 8 {
        // |v| >= 32768 (or inf / NaN): saturate (NaN → 0).
        if e == 0xFF && b & 0x7F_FFFF != 0 {
            0
        } else if b >> 31 != 0 {
            return (i16::MIN, 0);
        } else {
            return (i16::MAX, 0xFFFF);
        }
    } else if sh >= 0 {
        m << sh
    } else if sh > -24 {
        m >> -sh
    } else {
        0
    };
    let x = if b >> 31 != 0 { (mag as i32).wrapping_neg() } else { mag as i32 };
    ((x >> 16) as i16, x as u16)
}

/// The folded clip-space matrix of a draw: `diag(s, s, s, 1) x model x view x P` with
/// `P`: X = vx, Y = vy, W = vz + DIST, Z = a*W + b (z planes = MDX near/far).
/// Reference for tests: the RSP folds [`model_scaled`] with [`view_proj`] itself.
pub fn clip_matrix(model: &Mat4, view: &ViewParams, pos_scale: f32, no_clip: bool) -> Mat4 {
    let mv = mtx_cat(model, &view.view);
    let (n, f) = (view.near, if no_clip { NO_CLIP_FAR } else { view.far });
    let a = (f + n) / (f - n);
    let b = -2.0 * f * n / (f - n);
    let mut m = [[0f32; 4]; 4];
    for r in 0..4 {
        let s = if r < 3 { pos_scale } else { 1.0 };
        let (dz, w) = if r == 3 { (DIST, 1.0) } else { (0.0, 0.0) };
        let vz = mv[r][2] + dz;
        m[r] = [mv[r][0] * s, mv[r][1] * s, (a * vz + b * w) * s, vz * s];
    }
    m
}

/// View x projection of a frame (row-vector, columns X, Y, Z, W): `clip_matrix(model) =
/// model_scaled(model) x view_proj` (the RSP computes the product per draw, phase 3P).
pub fn view_proj(view: &ViewParams, no_clip: bool) -> Mat4 {
    let v = &view.view;
    let (n, f) = (view.near, if no_clip { NO_CLIP_FAR } else { view.far });
    let a = (f + n) / (f - n);
    let b = -2.0 * f * n / (f - n);
    let mut m = [[0f32; 4]; 4];
    for (k, row) in m.iter_mut().enumerate() {
        let (dz, w) = if k == 3 { (DIST, 1.0) } else { (0.0, 0.0) };
        let vz = v[k][2] + dz;
        *row = [v[k][0], v[k][1], a * vz + b * w, vz];
    }
    m
}

/// The model matrix with the mesh's position scale on the direction rows (what the
/// `F2_DRAW` uniform carries; mesh positions are stored scaled integers).
#[inline]
pub fn model_scaled(model: &Mat4, pos_scale: f32) -> Mat4 {
    let s = pos_scale;
    [
        [model[0][0] * s, model[0][1] * s, model[0][2] * s, 0.0],
        [model[1][0] * s, model[1][1] * s, model[1][2] * s, 0.0],
        [model[2][0] * s, model[2][1] * s, model[2][2] * s, 0.0],
        [model[3][0], model[3][1], model[3][2], 1.0],
    ]
}

/// Bound of the radius scale of a scaled model matrix ([`model_scaled`]): the X, Y, W columns
/// of the view x projection are the orthonormal view rotation for the direction rows, so
/// transformed spheres grow by at most the longest scaled model row.
#[inline]
fn scale_bound(mp: &Mat4) -> f32 {
    let rn = |r: usize| mp[r][0] * mp[r][0] + mp[r][1] * mp[r][1] + mp[r][2] * mp[r][2];
    fast_sqrt(rn(0).max(rn(1)).max(rn(2))) * 1.0001
}

/// A point (stored position units) through a scaled model matrix.
#[inline(always)]
fn xf(mp: &Mat4, c: &[f32; 3]) -> [f32; 3] {
    core::array::from_fn(|k| c[0] * mp[0][k] + c[1] * mp[1][k] + c[2] * mp[2][k] + mp[3][k])
}

/// Packs a scaled model matrix ([`model_scaled`]; lanes x, y, z of each row) and the draw's
/// colour parameters into the `F2_DRAW` uniform.
pub fn draw_uniform(m: &Mat4, p: &DrawParams) -> R3dDrawUniform {
    let mut u = R3dDrawUniform::default();
    draw_uniform_into(m, p, &mut u);
    u
}

/// [`draw_uniform`] written in place (no fog).
#[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_q")]
pub fn draw_uniform_into(m: &Mat4, p: &DrawParams, out: &mut R3dDrawUniform) {
    // Floats: the RSP converts them (rsp_f2mdx.S F2_Convert = `to_fixed`).
    for r in 0..4 {
        for c in 0..3 {
            out.mtx[r * 3 + c] = m[r][c];
        }
    }
    out.cvt = [0, 0];
    // c = c_in * mul + add in the shader's <<7 units; +0x7F bias rounds 255 back to 255.
    let am = (p.alpha_mul.clamp(0.0, 1.0) * 32767.0) as i16;
    let (cmul, cadd) = match p.gouraud {
        None => ([0x7FFF, 0x7FFF, 0x7FFF, am], [0x7F; 4]),
        Some(g) => {
            let c = |v: f32| (((v.clamp(0.0, 1.0) * 255.0 + 0.5) as i16) << 7) | 0x7F;
            ([0, 0, 0, am], [c(g[0]), c(g[1]), c(g[2]), 0x7F])
        }
    };
    (out.cmul, out.cadd, out.fogc, out.fogm) = (cmul, cadd, [0; 4], [0xFF, 0, 0, 0]);
}

/// The RSP's fold (`F2FoldRow` in rsp_f2mdx.S) of a draw uniform's model rows with the
/// frame's view x projection: s15.16 x s15.16 partial products accumulated like
/// VMUDL/VMADM/VMADN/VMADH (the f x f term truncated), as s15.16 values.
pub fn rsp_fold(u: &R3dDrawUniform, fr: &R3dFrame) -> [[i64; 4]; 4] {
    let mut out = [[0i64; 4]; 4];
    for (r, row) in out.iter_mut().enumerate() {
        for (c, o) in row.iter_mut().enumerate() {
            let mut acc: i64 = 0;
            for k in 0..3 {
                let (ai, af) = (fr.vp_i[k * 4 + c] as i64, fr.vp_f[k * 4 + c] as i64);
                let (bi, bf) = to_fixed(u.mtx[r * 3 + k]);
                let (bi, bf) = (bi as i64, bf as i64);
                acc += ((af * bf) >> 16) + ai * bf + af * bi + ((ai * bi) << 16);
            }
            if r == 3 {
                acc += ((fr.vp_i[12 + c] as i64) << 16) + fr.vp_f[12 + c] as i64;
            }
            *o = acc.clamp(i32::MIN as i64, i32::MAX as i64);
        }
    }
    out
}

/// Sets a draw uniform's fog fields (per vertex, per object from the origin's depth `w0`
/// = `W` of the model origin, or none).
fn draw_fog(u: &mut R3dDrawUniform, view: &ViewParams, p: &DrawParams, w0: f32) {
    (u.fogc, u.fogm) = fog_fields(view, p, w0);
}

/// The fog fields (`fogc`, `fogm`) of a draw's uniform.
#[inline]
fn fog_fields(view: &ViewParams, p: &DrawParams, w0: f32) -> ([i16; 4], [u16; 4]) {
    match view.fog {
        Some(f) if !p.no_clip => {
            if p.object_fog {
                // Unfogged fraction (PC `FOG(x)`), RDPQ_FOG_STANDARD's shade alpha.
                let a = ((f.end - (w0 - DIST)) / (f.end - f.start).max(1.0)).clamp(0.0, 1.0);
                ([0, (a * 32767.0) as i16, 0, 0], [0x77, 0, 0, 0])
            } else {
                ([0; 4], [0xFF, 0x88, 0, 0])
            }
        }
        _ => ([0; 4], [0xFF, 0, 0, 0]),
    }
}

/// The `F2_FRAME` uniform of a view.
pub fn frame_uniform(v: &ViewParams) -> R3dFrame {
    let [x, y, w, h] = v.viewport;
    let q = |p: f32| (p * 4.0 + if p >= 0.0 { 0.5 } else { -0.5 }) as i16;
    let k = v.z_scale();
    let (ki, kf) = to_fixed(k);
    R3dFrame {
        vo: [q(x + w / 2.0), q(y + h / 2.0), (-DIST * k + 0.5) as i16, 0],
        vs: [(-8.0 * v.fov * v.proj_scale()) as i16, (-8.0 * v.fov * v.proj_scale()) as i16, ki, 0],
        vlo: [q(x), q(y), kf as i16, 0],
        vhi: [q(x + w - 1.0), q(y + h - 1.0), 0, 0],
        wone: [1, 0, 1, 0],
        fogf: match v.fog {
            // Shade alpha = unfogged fraction (RDPQ_FOG_STANDARD): (end - vz) * k = (W - E) * -k.
            Some(f) => [0, 0, -((32767.0 / (f.end - f.start).max(1.0)).max(1.0) as i16), (f.end + DIST) as i16],
            None => [0, 0, 0, 0],
        },
        vp_i: [0; 16],
        vp_f: [0; 16],
        clip_text: 0,
        clip_size: 0,
        modge_text: 0,
        modge_size: 0,
        guard: v.guard,
        fog_rgba: v.fog.map_or(0, |f| f.rgba | 0xFF),
    }
}

/// [`frame_uniform`] with the view x projection for normal (`no_clip` false) or sky draws.
///
/// Sky draws (far plane at `NO_CLIP_FAR`) carry clip-space X and Y divided by the guard
/// factor G, the screen scale multiplied by G, and guard factor 1: the same projection and
/// the same guard band on screen. With G applied by the RSP instead (magma's clip codes
/// compare X with W * G and Z * G with W * G, all s15.16), W * G saturated beyond
/// W = 32768 / G (5461 at G = 6; normal draws end at W = 1200): far sky vertices lost their
/// guard flags and their screen coordinates saturated, and X/Y themselves saturated beyond
/// ±32768 (the Space backgrounds: a scale-5 model with coordinates up to 6610) - spikes
/// towards the screen corners, varying with the camera. W (MDX's integer `vz + DIST`
/// divisor), Z and the near/far tests are unchanged.
pub fn frame_uniform_vp(v: &ViewParams, no_clip: bool) -> R3dFrame {
    let mut fr = frame_uniform(v);
    let mut vp = view_proj(v, no_clip);
    if no_clip {
        let g = if v.guard == 0 { 4 } else { v.guard };
        for row in vp.iter_mut() {
            row[0] /= g as f32;
            row[1] /= g as f32;
        }
        let s = (-8.0 * v.fov * v.proj_scale() * g as f32).clamp(-32768.0, 32767.0) as i16;
        fr.vs[0] = s;
        fr.vs[1] = s;
        fr.guard = 1;
    }
    for r in 0..4 {
        for c in 0..4 {
            let (i, f) = to_fixed(vp[r][c]);
            fr.vp_i[r * 4 + c] = i;
            fr.vp_f[r * 4 + c] = f;
        }
    }
    fr
}

impl Default for R3d {
    fn default() -> Self {
        Self::new()
    }
}

impl R3d {
    pub fn new() -> R3d {
        #[cfg(target_vendor = "nintendo64")]
        unsafe {
            ffi::n64_r3d_init()
        };
        #[cfg(target_vendor = "nintendo64")]
        if option_env!("F2N64_CVT_CHECK").is_some() {
            cvt_check();
        }
        R3d {
            _pad: [0; PADX],
            files: Vec::new(),
            tex_by_crc: BTreeMap::new(),
            crc_id: BTreeMap::new(),
            crc_tex: Vec::new(),
            gparts: Vec::new(),
            uni: [Vec::with_capacity(512), Vec::with_capacity(512), Vec::with_capacity(512)],
            uni_frame: 0,
            meshes: Vec::new(),
            mhot: Vec::new(),
            spheres: Vec::new(),
            lists: Default::default(),
            vis: Vec::with_capacity(2048),
            handle_scratch: Vec::with_capacity(2048),
            tex_scratch: Vec::with_capacity(2048),
            draw_scratch: Vec::with_capacity(256),
            view: ViewParams::pc_default(320.0, 240.0),
            frame: R3dFrame::default(),
            frame_noclip: R3dFrame::default(),
            vp: IDENTITY,
            cull: Frustum::default(),
            queued_tris: 0,
            sent_tris: 0,
            view_gen: 0,
            slots: Vec::new(),
            slot_ui: Vec::new(),
            rings: Vec::new(),
            uni_owner: Vec::new(),
            uni_used: Vec::new(),
            uni_free: Vec::new(),
            prev_vis: Vec::with_capacity(2048),
            frame_no: 1,
            cache_hits: 0,
            pushed: 0,
            view_hits: 0,
            size_classes: [(0, 0); 3],
            size_all: (0, 0),
            size_frames: 0,
            modge_cmds: Vec::new(),
            q: Vec::with_capacity(512),
            qbones: Vec::new(),
            qmodels: Vec::new(),
            qgouraud: Vec::new(),
            modge_todo: Vec::new(),
            slot_model: Vec::new(),
            extra_lf: 0,
            cull_checked: 0,
            cull_wrong: 0,
            slot_key: Vec::new(),
            drawn: 0,
            resolve_all: false,
            ptab: Vec::new(),
            pkeys: Vec::new(),
            plast: 0,
            phash: alloc::boxed::Box::new([0; PHASH]),
            view_same: false,
            prof: R3dProf::default(),
        }
    }

    /// Registers an F2NM file's textures (by CRC; later files override earlier ones) and
    /// meshes. The bytes must be 8-byte aligned (libdragon's allocator guarantees it) and
    /// are kept for the renderer's lifetime.
    pub fn load_file(&mut self, bytes: Vec<u8>) -> Result<Vec<MeshId>, LoadError> {
        let f = File::parse(&bytes).map_err(LoadError::Format)?;
        let mut ids = Vec::new();
        for i in 0..f.n_textures() {
            let t = f.texture(i).map_err(LoadError::Format)?;
            let texels = f.texels(&t);
            let tlut = f.tlut(&t);
            let desc = ffi::R3dTexDesc {
                fmt: t.fmt as u32,
                width: t.width as u32,
                height: t.height as u32,
                tlut_count: t.tlut_count as u32,
                texels: texels.as_ptr(),
                tlut: if tlut.is_empty() { core::ptr::null() } else { tlut.as_ptr() },
            };
            let h = Self::c_tex_create(&desc);
            if h == 0 {
                return Err(LoadError::Full);
            }
            self.set_crc(t.crc, h as u16);
        }
        for i in 0..f.n_meshes() {
            let m = f.mesh(i).map_err(LoadError::Format)?;
            let res = self.register_mesh(&m)?;
            ids.push(MeshId(self.meshes.len() as u32));
            self.mhot.push(MeshHot {
                center: res.center,
                radius: res.radius,
                pos_scale: res.pos_scale,
                tris: res.tris,
                sph: self.spheres.len() as u32,
                n: res.parts.len() as u16,
                flags: if res.world { MH_WORLD } else { 0 } | if res.parts.iter().any(|p| p.radius < 0.0) { MH_OPEN } else { 0 },
                _pad: 0,
            });
            self.spheres.extend(res.parts.iter().map(|p| [p.center[0], p.center[1], p.center[2], p.radius]));
            self.gparts.extend(res.parts.iter().map(|p| GPart { handle: p.handle, cid: p.cid, tris: p.tris }));
            self.meshes.push(res);
        }
        // The Vec's heap buffer does not move when the Vec itself is moved.
        self.files.push(bytes);
        Ok(ids)
    }

    fn register_mesh(&mut self, m: &MeshView) -> Result<MeshRes, LoadError> {
        let skinned = m.entry.is_skinned();
        let mut res = MeshRes {
            id: m.entry.id,
            parts: Vec::new(),
            pos_scale: m.entry.pos_scale(),
            skinned,
            tris: m.entry.n_tris,
            center: [0.0; 3],
            radius: -1.0,
            verts: m.vertices.as_ptr() as usize,
            n_verts: m.entry.n_verts as u32,
            modge: None,
            world: m.entry.is_world(),
            add_split: false,
        };
        let add = |cmds: &[u8], cid: u16, tris: u16, center: [f32; 3], radius: f32| -> Result<Part, LoadError> {
            let desc = ffi::R3dGroupDesc {
                verts: m.vertices.as_ptr(),
                cmds: cmds.as_ptr() as *const u32,
                n_verts: m.entry.n_verts as u32,
                n_cmds: (cmds.len() / 4) as u32,
                skinned: skinned as u32,
            };
            let h = Self::c_group_create(&desc);
            if h == 0 {
                return Err(LoadError::Full);
            }
            Ok(Part { handle: h as u16, cid, tris, center, radius })
        };
        let cids: Vec<u16> = m.groups().map(|g| self.crc_slot(g.tex_crc)).collect();
        for (gi, g) in m.groups().enumerate() {
            let cid = cids[gi];
            if skinned || g.batch_count == 0 {
                res.parts.push(add(m.group_cmd_bytes(&g), cid, g.n_tris, [0.0; 3], -1.0)?);
            } else {
                for b in m.group_batches(&g) {
                    let c = b.center.map(|v| v as f32);
                    res.parts.push(add(m.batch_cmd_bytes(&b), cid, b.n_tris, c, b.radius as f32)?);
                }
            }
        }
        (res.center, res.radius) = enclosing_sphere(&res.parts);
        Ok(res)
    }

    #[cfg(target_vendor = "nintendo64")]
    fn c_tex_create(d: &ffi::R3dTexDesc) -> u32 {
        unsafe { ffi::n64_r3d_tex_create(d) }
    }
    #[cfg(not(target_vendor = "nintendo64"))]
    fn c_tex_create(_d: &ffi::R3dTexDesc) -> u32 {
        1
    }
    #[cfg(target_vendor = "nintendo64")]
    fn c_group_create(d: &ffi::R3dGroupDesc) -> u32 {
        unsafe { ffi::n64_r3d_group_create(d) }
    }
    #[cfg(not(target_vendor = "nintendo64"))]
    fn c_group_create(_d: &ffi::R3dGroupDesc) -> u32 {
        1
    }

    /// The current resource counts (see [`R3dMark`]).
    pub fn mark(&self) -> R3dMark {
        #[cfg(target_vendor = "nintendo64")]
        let c = unsafe { ffi::n64_r3d_counts() };
        #[cfg(not(target_vendor = "nintendo64"))]
        let c = 0x0001_0001;
        R3dMark { files: self.files.len(), meshes: self.meshes.len(), tex: c >> 16, groups: c & 0xFFFF }
    }

    /// Frees every file, mesh and texture registered after `m` (waits for the RSP).
    pub fn reset_to(&mut self, m: R3dMark) {
        #[cfg(target_vendor = "nintendo64")]
        unsafe {
            ffi::n64_r3d_reset(m.tex, m.groups)
        };
        self.discard();
        // Per-level storage (the RSP finished above): freed, the next level grows its own.
        self.slots = Vec::new();
        self.slot_ui = Vec::new();
        self.rings = Vec::new();
        self.uni_owner = Vec::new();
        self.uni_used = Vec::new();
        self.uni_free = Vec::new();
        self.slot_model = Vec::new();
        self.slot_key = Vec::new();
        self.ptab = Vec::new();
        self.pkeys = Vec::new();
        self.phash.fill(0);
        self.plast = 0;
        self.q = Vec::new();
        self.qbones = Vec::new();
        self.qmodels = Vec::new();
        self.qgouraud = Vec::new();
        self.modge_todo = Vec::new();
        self.modge_cmds = Vec::new();
        for l in &mut self.lists {
            *l = Vec::new();
        }
        self.draw_scratch = Vec::new();
        if let Some(h) = self.mhot.get(m.meshes) {
            self.spheres.truncate(h.sph as usize);
            self.gparts.truncate(h.sph as usize);
        }
        self.mhot.truncate(m.meshes);
        self.meshes.truncate(m.meshes);
        for v in [&mut self.spheres as &mut dyn ShrinkVec, &mut self.gparts, &mut self.mhot, &mut self.meshes] {
            v.shrink();
        }
        if self.meshes.is_empty() {
            self.crc_id.clear();
            self.crc_tex = Vec::new();
        }
        self.files.truncate(m.files);
        if self.files.is_empty() {
            self.files = Vec::new(); // the next level's list is part of its load
        }
        self.tex_by_crc.retain(|_, h| (*h as u32) < m.tex);
        for (crc, &id) in &self.crc_id {
            self.crc_tex[id as usize] = self.tex_by_crc.get(crc).copied().unwrap_or(0);
        }
    }

    /// An r3d texture sampling main.c texture `tex`'s TMEM-sized surface (power-of-two
    /// RGBA16/RGBA32; `None` otherwise).
    pub fn alias_texture(&mut self, tex: u32) -> Option<TexId> {
        #[cfg(target_vendor = "nintendo64")]
        let h = unsafe { ffi::n64_r3d_tex_alias(tex) };
        #[cfg(not(target_vendor = "nintendo64"))]
        let h = tex;
        (h != 0).then_some(TexId(h as u16))
    }

    /// Points a CRC at a texture, or unmaps it (`None`: its groups are not drawn).
    pub fn map_texture(&mut self, crc: u32, tex: Option<TexId>) {
        // Called for every registry entry every frame: look before writing.
        if self.tex_by_crc.get(&crc).copied() == tex.map(|t| t.0) {
            return;
        }
        match tex {
            Some(t) => self.set_crc(crc, t.0),
            None => {
                self.tex_by_crc.remove(&crc);
                if let Some(&id) = self.crc_id.get(&crc) {
                    self.crc_tex[id as usize] = 0;
                }
            }
        }
    }

    /// The dense id of a texture CRC, for [`map_texture_id`](Self::map_texture_id).
    pub fn crc_id(&mut self, crc: u32) -> u16 {
        self.crc_slot(crc)
    }

    /// [`map_texture`](Self::map_texture) of a CRC with its dense id ([`crc_id`]
    /// (Self::crc_id)): an array compare when nothing changed.
    pub fn map_texture_id(&mut self, cid: u16, crc: u32, tex: Option<TexId>) {
        let h = tex.map_or(0, |t| t.0);
        // `crc_tex` mirrors `tex_by_crc` (0 = unmapped).
        if self.crc_tex[cid as usize] == h {
            return;
        }
        self.crc_tex[cid as usize] = h;
        match tex {
            Some(t) => {
                self.tex_by_crc.insert(crc, t.0);
            }
            None => {
                self.tex_by_crc.remove(&crc);
            }
        }
    }

    /// Points `crc` at texture handle `h` (registry and dense table).
    fn set_crc(&mut self, crc: u32, h: u16) {
        self.tex_by_crc.insert(crc, h);
        if let Some(&id) = self.crc_id.get(&crc) {
            self.crc_tex[id as usize] = h;
        }
    }

    /// The dense id of a group texture CRC (`crc_tex[id]` follows the registry).
    fn crc_slot(&mut self, crc: u32) -> u16 {
        if let Some(&id) = self.crc_id.get(&crc) {
            return id;
        }
        let id = self.crc_tex.len() as u16;
        self.crc_id.insert(crc, id);
        self.crc_tex.push(self.tex_by_crc.get(&crc).copied().unwrap_or(0));
        id
    }

    /// The packer id of a registered mesh.
    pub fn mesh_packer_id(&self, m: MeshId) -> u32 {
        self.meshes[m.0 as usize].id
    }

    /// Texture registered for a CRC.
    pub fn texture(&self, crc: u32) -> Option<TexId> {
        self.tex_by_crc.get(&crc).map(|&h| TexId(h))
    }
    /// Points a CRC at another texture (animated textures: `TextureRegistry::current`).
    pub fn set_texture(&mut self, crc: u32, tex: TexId) {
        self.set_crc(crc, tex.0);
    }
    /// Mesh with packer id `id` (first match).
    pub fn find_mesh(&self, id: u32) -> Option<MeshId> {
        self.meshes.iter().position(|m| m.id == id).map(|i| MeshId(i as u32))
    }
    pub fn mesh_skinned(&self, m: MeshId) -> bool {
        self.meshes[m.0 as usize].skinned
    }
    pub fn mesh_tris(&self, m: MeshId) -> u32 {
        self.meshes[m.0 as usize].tris
    }
    pub fn num_meshes(&self) -> usize {
        self.meshes.len()
    }

    /// Starts a frame (call after `rt::begin_frame`).
    pub fn begin_frame(&mut self, view: &ViewParams) {
        self.view_same = *view == self.view;
        if !self.view_same {
            self.view_gen = self.view_gen.wrapping_add(1);
        }
        // The parameter table between frames: reset when large (slots keep ids < PID_NONE).
        if self.pkeys.len() > PID_RESET {
            self.pkeys.clear();
            self.phash.fill(0);
            self.ptab.clear();
            self.plast = 0;
            for e in &mut self.slots {
                e.frame = 0;
            }
        }
        self.view = *view;
        self.frame_no = self.frame_no.wrapping_add(1);
        // `vis` holds the previous frame's parts (flush keeps them).
        core::mem::swap(&mut self.vis, &mut self.prev_vis);
        self.vis.clear();
        self.cache_hits = 0;
        self.view_hits = 0;
        self.drawn = 0;
        self.size_frames += 1;
        if option_env!("F2N64_MEASURE_SIZES").is_some() && self.size_frames == 300 {
            let f = self.size_frames;
            let [a, b, c] = self.size_classes.map(|(d, t)| (d / f, t / f));
            #[cfg(target_vendor = "nintendo64")]
            crate::log!(
                "r3d sizes per frame: {} draws {} tris; radius < 1 px: {} draws {} tris; < 1.5 px: {} / {}; < 3 px: {} / {}",
                self.size_all.0 / f, self.size_all.1 / f, a.0, a.1, b.0, b.1, c.0, c.1
            );
            let _ = (a, b, c);
            self.size_classes = [(0, 0); 3];
            self.size_all = (0, 0);
        }
        if self.size_frames >= 300 {
            self.size_frames = 0;
        }
        if R3D_PROF {
            self.prof.frames += 1;
            if self.prof.frames == 150 {
                let p = self.prof;
                let us = |t: u32| t / 47 / p.frames;
                #[cfg(target_vendor = "nintendo64")]
                crate::log!(
                    "r3dprof per frame: full {} draws {} us | view-only {} draws {} us | skinned {} {} us | cull {} calls {} tests {} us | modge {} runs {} us | record {} x {} us | run_queue {} us | flush: rust {} us, C states {} us, C draws {} us",
                    p.full_draws / p.frames, us(p.full_ticks), p.view_draws / p.frames, us(p.view_ticks), p.skin_draws / p.frames, us(p.skin_ticks),
                    p.cull_calls / p.frames, p.sphere_tests / p.frames, us(p.cull_ticks), p.modge_runs / p.frames, us(p.modge_ticks), p.records / p.frames, us(p.record_ticks), us(p.run_ticks), us(p.flush_rust), us(p.flush_state), us(p.flush_draws)
                );
                let _ = (p, us);
                self.prof = R3dProf::default();
            }
        }
        self.frame = frame_uniform_vp(view, false);
        self.frame_noclip = frame_uniform_vp(view, true);
        self.vp = view_proj(view, false);
        self.uni_frame = (self.uni_frame + 1) % 3;
        self.uni[self.uni_frame].clear();
        self.queued_tris = 0;
        self.sent_tris = 0;
        let [x, y, w, h] = view.viewport;
        let k = view.fov * view.proj_scale();
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        let a = [(cx - x - 1.0) / k, (x + w - 1.0 - cx - 1.0) / k, (cy - y - 1.0) / k, (y + h - 1.0 - cy - 1.0) / k];
        self.cull = Frustum::new(&self.vp, a, view.near, view.far);
    }

    /// Culls a rigid mesh's batches against the screen and near/far with their bounding
    /// spheres (the mesh's sphere centre `wc` in world space, radius scale bound `s` of the
    /// mesh's `model`), pushing the visible part indices to `vis`: first the whole mesh, then,
    /// only if its sphere straddles planes, each part against those planes (moved into model
    /// space: the parts lie inside the mesh's sphere, so the other planes cannot cull them).
    /// Conservative: a culled batch is one whose every face MDX would reject (all vertices
    /// off-screen past one edge, or beyond near/far), so the image is unchanged.
    #[inline(always)]
    fn cull_world(&mut self, mesh: usize, wc: &[f32; 3], s: f32, model: &Model34, no_clip: bool) -> u32 {
        let t0 = prof_ticks();
        let n = self.cull_world_inner(mesh, wc, s, model, no_clip);
        if R3D_PROF {
            self.prof.cull_ticks = self.prof.cull_ticks.wrapping_add(prof_ticks().wrapping_sub(t0));
            self.prof.cull_calls += 1;
        }
        n
    }

    #[inline(always)]
    fn cull_world_inner(&mut self, mesh: usize, wc: &[f32; 3], s: f32, model: &Model34, no_clip: bool) -> u32 {
        let res = &self.mhot[mesh];
        let fr = &self.cull;
        let planes = if no_clip { 5 } else { 6 };
        let n = res.n as usize;
        if R3D_PROF {
            self.prof.sphere_tests += 1;
        }
        let mask = if res.radius >= 0.0 {
            match fr.classify(wc, res.radius * s, planes) {
                None if res.flags & MH_OPEN == 0 => return 0,
                None => {
                    // Only the parts without a sphere.
                    let first = self.vis.len();
                    for (i, sp) in self.spheres[res.sph as usize..res.sph as usize + n].iter().enumerate() {
                        if sp[3] < 0.0 {
                            self.vis.push(i as u16);
                        }
                    }
                    return (self.vis.len() - first) as u32;
                }
                // Wholly inside, or a single part (its sphere is the mesh's): all visible.
                Some(m) if m == 0 || n == 1 => {
                    self.vis.extend(0..n as u16);
                    return n as u32;
                }
                Some(m) => m,
            }
        } else {
            (1 << planes) - 1
        };
        // The straddled planes in the mesh's stored position units: p · (c·M + t).
        let mp = model_scaled(&to44(model), res.pos_scale);
        let mut pm = [[0f32; 4]; 6];
        let mut np = 0;
        for (k, p) in fr.p[..planes].iter().enumerate() {
            if mask & 1 << k != 0 {
                let row = |r: usize| p[0] * mp[r][0] + p[1] * mp[r][1] + p[2] * mp[r][2];
                pm[np] = [row(0), row(1), row(2), row(3) + p[3]];
                np += 1;
            }
        }
        let pm = &pm[..np];
        let first = self.vis.len();
        let mut tests = 0;
        for (i, sp) in self.spheres[res.sph as usize..res.sph as usize + n].iter().enumerate() {
            if sp[3] >= 0.0 {
                tests += 1;
                let r = sp[3] * s;
                if pm.iter().any(|q| sp[0] * q[0] + sp[1] * q[1] + sp[2] * q[2] + q[3] >= r) {
                    continue;
                }
            }
            self.vis.push(i as u16);
        }
        if R3D_PROF {
            self.prof.sphere_tests += tests;
        }
        (self.vis.len() - first) as u32
    }

    /// `W` (view depth - DIST) of a model matrix's origin.
    #[inline]
    fn origin_w34(&self, model: &Model34) -> f32 {
        let vp = &self.vp;
        model[3][0] * vp[0][3] + model[3][1] * vp[1][3] + model[3][2] * vp[2][3] + vp[3][3]
    }

    /// `W` (view depth - DIST) of a model matrix's origin.
    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_q")]
    fn origin_w(&self, model: &Mat4) -> f32 {
        let vp = &self.vp;
        model[3][0] * vp[0][3] + model[3][1] * vp[1][3] + model[3][2] * vp[2][3] + vp[3][3]
    }

    pub fn view(&self) -> &ViewParams {
        &self.view
    }

    /// Queues a rigid mesh draw with its model → world matrix.
    pub fn draw(&mut self, list: List, mesh: MeshId, model: &Mat4, p: &DrawParams) {
        self.draw_slot(list, mesh, model, p, u32::MAX, 0);
    }

    /// Whether `mesh` is stored in world space (draw it with an identity model matrix).
    pub fn mesh_is_world(&self, mesh: MeshId) -> bool {
        self.mhot.get(mesh.0 as usize).is_some_and(|m| m.flags & MH_WORLD != 0)
    }

    /// A rigid mesh's bounding sphere in model units (centre, radius: the stored positions
    /// times `pos_scale`, plus one `pos_scale` step for the rounding of the stored and the
    /// MODGE/WAVE-animated positions), enclosing every vertex the RSP draws of it apart from
    /// a WAVE height; `None` for world meshes and meshes with a part without a sphere (those
    /// are never culled).
    pub fn mesh_sphere(&self, mesh: MeshId) -> Option<([f32; 3], f32)> {
        let m = self.mhot.get(mesh.0 as usize)?;
        if m.flags & (MH_WORLD | MH_OPEN) != 0 || m.radius < 0.0 {
            return None;
        }
        let s = m.pos_scale;
        Some((m.center.map(|c| c * s), (m.radius + 1.0) * s))
    }

    /// Whether a world-space sphere is wholly outside this frame's view (left / right / top /
    /// bottom, near, far: the planes [`cull_world`](Self::cull_world) culls normal draws with),
    /// i.e. every rigid draw inside it would be culled.
    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_d")]
    pub fn sphere_culled(&self, c: &[f32; 3], r: f32) -> bool {
        self.cull.classify(c, r, 6).is_none()
    }

    /// `PCCalcModgeValues` on a rigid mesh's vertices at frame `fc` (`flags` bit 0 MODGE:
    /// texture coordinates wobble by `0.4·(tMx, tMz)`; bit 1 WAVE: vertices rise by `tMy`;
    /// both: the water's alpha follows `50 + (tMx + tMz)·400` around its stored value), written
    /// into the mesh's vertex buffer once per frame (instances share the model-space result).
    /// The RSP may still be reading the previous frame's values: a wave one frame newer.
    #[inline(never)]
    pub fn animate_modge(&mut self, mesh: MeshId, fc: u32, flags: u8) {
        let Some(res) = self.meshes.get_mut(mesh.0 as usize) else { return };
        if res.skinned || res.n_verts == 0 || flags & 3 == 0 {
            return;
        }
        if res.modge.is_none() {
            res.modge = Some(Self::modge_init(res, flags));
        }
        let st = res.modge.as_deref_mut().unwrap();
        if st.frame == fc && st.flags == flags {
            return;
        }
        let t0 = prof_ticks();
        st.frame = fc;
        if st.flags != flags {
            if let Some(r) = &mut st.rsp {
                r.k = modge_k16(res.pos_scale, flags);
            }
        }
        st.flags = flags;
        let tb = sin_q14();
        let tp = modge_time_phases(fc);
        let (modge, wave) = (flags & 1 != 0, flags & 2 != 0);
        let alpha = modge_writes_alpha(flags);
        if st.rsp.is_none() {
            for (ph, o) in st.xph.iter().zip(st.xt.iter_mut()) {
                *o = modge_x_terms(tb, ph, &tp, wave);
            }
            for (ph, o) in st.zph.iter().zip(st.zt.iter_mut()) {
                *o = modge_z_terms(tb, ph, &tp, wave);
            }
        }
        let (ky, ky_sh) = (st.ky, st.ky_sh);
        let (xt, zt) = (&st.xt[..], &st.zt[..]);
        if st.rsp.is_none() {
            for (&(xi, zi, a0), q) in st.pairs.iter().zip(st.pt.iter_mut()) {
                if let (Some(px), Some(pz)) = (xt.get(xi as usize), zt.get(zi as usize)) {
                    *q = modge_pair(tb, px, pz, a0, ky, ky_sh, flags);
                }
            }
        }
        if let Some(r) = &mut st.rsp {
            // Everything on the RSP: this frame's descriptor (header + time phases).
            r.cur = (r.cur + 1) % 3;
            let off = r.desc_off + r.cur * r.desc_stride;
            let (idx_off, pos_off, term_off, pt_off, k) = (r.idx_off, r.pos_off, r.term_off, r.pt_off, r.k);
            let (n, np, nx, nz) = (st.base.len() as u16, st.pairs.len() as u16, st.xph.len() as u16, st.zph.len() as u16);
            let out = res.verts as *const u8;
            let b = r.bytes();
            let bp = b.as_ptr();
            let (base, idx, pos, terms) = (phys(bp), phys(bp.wrapping_add(idx_off)), phys(bp.wrapping_add(pos_off)), phys(bp.wrapping_add(term_off)));
            let d = &mut b[off..off + MODGE_DESC_HEAD];
            let mut w = |o: usize, v: &[u8]| d[o..o + v.len()].copy_from_slice(v);
            w(0, &base.to_be_bytes());
            w(4, &phys(out).to_be_bytes());
            w(8, &idx.to_be_bytes());
            w(12, &n.to_be_bytes());
            w(14, &np.to_be_bytes());
            w(16, &(if alpha { 4u16 } else { 0 }).to_be_bytes());
            w(18, &k.kyh.to_be_bytes());
            w(20, &k.kst.to_be_bytes());
            w(22, &pt_off.to_be_bytes());
            w(24, &pos.to_be_bytes());
            w(28, &phys(modge_rsp_table()).to_be_bytes());
            w(32, &terms.to_be_bytes());
            w(36, &nx.to_be_bytes());
            w(38, &nz.to_be_bytes());
            for (i, t) in ph16(&tp).iter().enumerate() {
                w(40 + 2 * i, &t.to_be_bytes());
            }
            Self::c_writeback(d);
            self.modge_cmds.push((mesh.0, d.as_ptr(), MODGE_DESC_HEAD as u32));
        } else {
            // SAFETY: `verts` points into a file kept in `self.files` for as long as the mesh.
            let v = unsafe { core::slice::from_raw_parts_mut(res.verts as *mut u8, res.n_verts as usize * 16) };
            let pt = &st.pt[..];
            for (c, b) in v.chunks_exact_mut(16).zip(st.base.iter()) {
                let Some(q) = pt.get(b.pair as usize) else { continue };
                let (y, s, tt) = modge_apply(q, b.y, b.s, b.t);
                let c: &mut [u8; 16] = c.try_into().unwrap();
                if wave {
                    [c[2], c[3]] = y.to_be_bytes();
                }
                if modge {
                    [c[12], c[13]] = s.to_be_bytes();
                    [c[14], c[15]] = tt.to_be_bytes();
                }
                if alpha {
                    c[11] = q.a;
                }
            }
            Self::c_writeback(v);
        }
        if R3D_PROF {
            self.prof.modge_ticks = self.prof.modge_ticks.wrapping_add(prof_ticks().wrapping_sub(t0));
            self.prof.modge_runs += 1;
        }
    }

    /// The per-vertex base data of a `MODGE`/`WAVE` mesh (first animation).
    #[cold]
    #[inline(never)]
    fn modge_init(res: &MeshRes, flags: u8) -> alloc::boxed::Box<ModgeMesh> {
        // SAFETY: as in `animate_modge`.
        let v = unsafe { core::slice::from_raw_parts(res.verts as *const u8, res.n_verts as usize * 16) };
        let scale = res.pos_scale;
        let rd = |o: usize| i16::from_be_bytes([v[o], v[o + 1]]);
        let n = res.n_verts as usize;
        let (xs, xi) = distinct(&(0..n).map(|i| rd(i * 16) as f32 * scale).collect::<Vec<_>>());
        let (zs, zi) = distinct(&(0..n).map(|i| rd(i * 16 + 4) as f32 * scale).collect::<Vec<_>>());
        let mut pair_of = BTreeMap::new();
        let mut pairs = Vec::new();
        let base: Vec<ModgeBase> = (0..n)
            .map(|i| {
                let o = i * 16;
                let key = (xi[i], zi[i], v[o + 11]);
                let pair = *pair_of.entry(key).or_insert_with(|| {
                    pairs.push(key);
                    pairs.len() as u16 - 1
                });
                ModgeBase { pair, y: rd(o + 2), s: rd(o + 12), t: rd(o + 14) }
            })
            .collect();
        #[cfg(target_vendor = "nintendo64")]
        crate::log!(
            "r3d: animating MODGE/WAVE mesh {:08x} ({} vertices, {}x{} distinct x/z, {} positions, flags {})",
            res.id, n, xs.len(), zs.len(), pairs.len(), flags
        );
        let _ = flags;
        // y += my / scale with my = 5·(sin + 1) = 5·S/1024: ky = 5 / scale / 1024 · 2^sh,
        // the largest sh (≤ 30) that keeps S·ky (S ≤ ~2048) in an i32.
        let k = 5.0 / scale as f64 / 1024.0;
        let mut ky_sh = 30;
        while ky_sh > 1 && k * (1u64 << ky_sh) as f64 * 2100.0 >= i32::MAX as f64 {
            ky_sh -= 1;
        }
        let ky = libm::round(k * (1u64 << ky_sh) as f64) as i32;
        let (xt, zt) = (alloc::vec![[0; 3]; xs.len()], alloc::vec![[0; 3]; zs.len()]);
        let pt = alloc::vec![ModgePair::default(); pairs.len()];
        // F2_Modge's DMEM: descriptor (header + terms), sine table, then the position table at
        // the end (8 bytes each, rounded up to 8 positions; the input chunk is in the clip cache).
        let desc_size = MODGE_DESC_HEAD + 8 * (xs.len() + zs.len());
        let pt_off = MODGE_DMEM - 8 * pairs.len().next_multiple_of(8);
        let (xph, zph): (Vec<[u32; 3]>, Vec<[u32; 3]>) = (xs.iter().map(|&x| modge_x_phases(x)).collect(), zs.iter().map(|&z| modge_z_phases(z)).collect());
        // (and a wave height multiplier within s16: stored-unit scales above 1/200)
        let fits = desc_size + MODGE_TAB_BYTES <= pt_off && 5.0 / (1024.0 * scale) * 32768.0 < 32767.0;
        let rsp = (cfg!(target_vendor = "nintendo64") && fits && option_env!("F2N64_MODGE_CPU").is_none()).then(|| {
            // Stored vertices (16 bytes each), position indices (+8 bytes: the RSP reads whole
            // 8-byte words), the position list, 3 descriptors (16-byte multiples).
            let idx_off = n * 16;
            let pos_off = (idx_off + n * 2 + 8 + 7) & !7;
            let term_off = pos_off + pairs.len() * 8;
            let desc_off = (term_off + (xph.len() + zph.len()) * 8 + 15) & !15;
            let desc_stride = (MODGE_DESC_HEAD + 15) & !15;
            let total = desc_off + 3 * desc_stride;
            let mut r = ModgeRsp {
                buf: alloc::vec![0u64; total.div_ceil(8) + 2],
                idx_off,
                pos_off,
                term_off,
                desc_off,
                desc_stride,
                pt_off: pt_off as u16,
                cur: 0,
                k: modge_k16(scale, flags),
            };
            let nx = xs.len();
            let b = r.bytes();
            b[..idx_off].copy_from_slice(v);
            for (i, m) in base.iter().enumerate() {
                b[idx_off + 2 * i..idx_off + 2 * i + 2].copy_from_slice(&m.pair.to_be_bytes());
            }
            for (i, &(xi, zi, a0)) in pairs.iter().enumerate() {
                let (xo, zo) = (MODGE_DESC_HEAD + 8 * xi as usize, MODGE_DESC_HEAD + 8 * (nx + zi as usize));
                let e = (xo as u64) << 48 | (zo as u64) << 32 | ((a0 as u64) << 7) << 16;
                b[pos_off + 8 * i..pos_off + 8 * i + 8].copy_from_slice(&e.to_be_bytes());
            }
            for (i, p) in xph.iter().chain(zph.iter()).enumerate() {
                let [a, c, d] = ph16(p);
                let e = (a as u64) << 48 | (c as u64) << 32 | (d as u64) << 16;
                b[term_off + 8 * i..term_off + 8 * i + 8].copy_from_slice(&e.to_be_bytes());
            }
            Self::c_writeback(&b[..desc_off]);
            r
        });
        #[cfg(target_vendor = "nintendo64")]
        if rsp.is_none() {
            crate::log!("r3d: MODGE/WAVE mesh {:08x} animated on the CPU ({} bytes of terms, {} positions)", res.id, desc_size, pairs.len());
        }
        alloc::boxed::Box::new(ModgeMesh { base, pairs, pt, xph, zph, xt, zt, ky, ky_sh, frame: u32::MAX, flags: 0, rsp })
    }

    /// Before a mesh's first additive draw: its vertex colours split like the packer's
    /// `additive_corner` (`rgb / max`, `a * max`; the additive variant textures are split the
    /// same way, so `colour * alpha` is the PC's additive term). The packer splits objects it
    /// knows are additive (bank flag, `ada`/`wat_`/`xxa_` names); others are made additive at
    /// run time (the frontend's `slu_flbm` light beams: dark corner colours drawn unsplit
    /// were dark veils). A split colour has max 255, so splitting again changes nothing.
    #[inline]
    pub fn ensure_additive(&mut self, mesh: MeshId) {
        if self.mhot.get(mesh.0 as usize).is_some_and(|r| r.flags & MH_ADD_SPLIT == 0) {
            self.split_additive(mesh);
        }
    }

    #[cold]
    #[inline(never)]
    fn split_additive(&mut self, mesh: MeshId) {
        let Some(res) = self.meshes.get_mut(mesh.0 as usize) else { return };
        if res.add_split || res.skinned || res.n_verts == 0 {
            return;
        }
        res.add_split = true;
        self.mhot[mesh.0 as usize].flags |= MH_ADD_SPLIT;
        // SAFETY: as in `animate_modge`.
        let v = unsafe { core::slice::from_raw_parts_mut(res.verts as *mut u8, res.n_verts as usize * 16) };
        for c in v.chunks_exact_mut(16) {
            let s = f2_n64fmt::bank::additive_corner([c[8], c[9], c[10], c[11]]);
            c[8..12].copy_from_slice(&s);
        }
        Self::c_writeback(v);
        // `F2_Modge` rewrites the vertex buffer from its stored copy: split that one too.
        if let Some(r) = res.modge.as_deref_mut().and_then(|m| m.rsp.as_mut()) {
            let n = v.len();
            let b = &mut r.bytes()[..n];
            for c in b.chunks_exact_mut(16) {
                let s = f2_n64fmt::bank::additive_corner([c[8], c[9], c[10], c[11]]);
                c[8..12].copy_from_slice(&s);
            }
            Self::c_writeback(b);
        }
    }

    #[cfg(target_vendor = "nintendo64")]
    fn c_writeback(v: &[u8]) {
        unsafe { ffi::n64_cache_writeback(v.as_ptr(), v.len() as u32) }
    }
    #[cfg(not(target_vendor = "nintendo64"))]
    fn c_writeback(_v: &[u8]) {}

    /// [`draw`](Self::draw) as draw number `slot` of the frame with matrix stamp `stamp`
    /// (`MeshDraw::stamp`): when the previous frame's draw `slot` had the same mesh, stamp,
    /// parameters and view, its uniform and visible parts are reused (no conversion, no
    /// culling, no copy). `slot` `u32::MAX` or `stamp` 0: no reuse.
    pub fn draw_slot(&mut self, list: List, mesh: MeshId, model: &Mat4, p: &DrawParams, slot: u32, stamp: u32) {
        // Queued like the game's draws; the work runs in `run_queue` (or at the flush).
        self.queue(list, mesh, &[*model], p, false, slot, stamp, 0, 0);
    }

    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn draw_slot_pid(&mut self, list: List, mesh: MeshId, model: &Model34, p: &DrawParams, pid: u16, slot: u16, stamp: u32) {
        if !R3D_PROF {
            return self.draw_slot_inner(list, mesh, model, p, pid, slot, stamp);
        }
        let (t0, h, v) = (prof_ticks(), self.cache_hits, self.view_hits);
        self.draw_slot_inner(list, mesh, model, p, pid, slot, stamp);
        let dt = prof_ticks().wrapping_sub(t0);
        if self.view_hits != v {
            self.prof.view_ticks = self.prof.view_ticks.wrapping_add(dt);
            self.prof.view_draws += 1;
        } else if self.cache_hits == h {
            self.prof.full_ticks = self.prof.full_ticks.wrapping_add(dt);
            self.prof.full_draws += 1;
        }
    }

    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn draw_slot_inner(&mut self, list: List, mesh: MeshId, model: &Model34, p: &DrawParams, pid: u16, slot: u16, stamp: u32) {
        let Some(res) = self.mhot.get(mesh.0 as usize) else { return };
        self.queued_tris += res.tris;
        let pos_scale = res.pos_scale;
        // Measurement (F2N64_MEASURE_SIZES=1): on-screen radius of the mesh's bounding
        // sphere (`p * M`, clip w).
        if option_env!("F2N64_MEASURE_SIZES").is_some() && res.radius >= 0.0 {
            let m = &model_scaled(&to44(model), pos_scale);
            let c = res.center;
            let wc = core::array::from_fn::<f32, 3, _>(|k| c[0] * m[0][k] + c[1] * m[1][k] + c[2] * m[2][k] + m[3][k]);
            let w = wc[0] * self.vp[0][3] + wc[1] * self.vp[1][3] + wc[2] * self.vp[2][3] + self.vp[3][3];
            let sc = fast_sqrt(m[0][0] * m[0][0] + m[0][1] * m[0][1] + m[0][2] * m[0][2]);
            let focal = self.view.fov * self.view.viewport[2] / 640.0;
            let px = if w > self.view.near { res.radius * sc * focal / w } else { f32::MAX };
            let (tris, t) = (res.tris, &mut self.size_classes);
            self.size_all.0 += 1;
            self.size_all.1 += tris;
            for (k, lim) in [1.0f32, 1.5, 3.0].into_iter().enumerate() {
                if px < lim {
                    t[k].0 += 1;
                    t[k].1 += tris;
                }
            }
        }
        let center = res.center;
        if slot == NO_SLOT {
            // Not reusable: everything into this frame's arena.
            let mp = model_scaled(&to44(model), pos_scale);
            let (s, wc) = (scale_bound(&mp), xf(&mp, &center));
            let vis_first = self.vis.len() as u32;
            let n = self.cull_world(mesh.0 as usize, &wc, s, model, p.no_clip);
            if n != 0 {
                let mut u = draw_uniform(&mp, p);
                let w0 = if p.object_fog && self.view.fog.is_some() { self.origin_w34(model) } else { 0.0 };
                draw_fog(&mut u, &self.view, p, w0);
                self.push_rec(list, mesh, p, u, vis_first, n);
            }
            return;
        }
        let i = slot as usize;
        if self.slots.len() <= i {
            self.grow_slots(i + 1);
        }
        let prev = self.frame_no.wrapping_sub(1);
        let e = &mut self.slots[i];
        let same = e.frame == prev && e.stamp == stamp && e.mesh as u32 == mesh.0 && e.pid as u16 == pid && pid < PID_NONE;
        if same && self.view_same {
            // Nothing changed: last frame's uniform and visible parts.
            self.cache_hits += 1;
            e.frame = self.frame_no;
            if e.vis_count == 0 {
                return;
            }
            // Queued last frame, so its ring is still its own (`UNI_REUSE_FRAMES`).
            let (a, n, cur) = (e.vis_first as usize, e.vis_count as usize, (e.cf & SLOT_CUR) as usize);
            let vis_first = self.vis.len() as u32;
            e.vis_first = vis_first as u16;
            self.vis.extend_from_slice(&self.prev_vis[a..a + n]);
            let r = self.slot_ui[i] as usize;
            self.uni_used[r] = self.frame_no;
            let uptr: *const R3dDrawUniform = &self.rings[r / SLOT_CHUNK][r % SLOT_CHUNK].u[cur];
            self.push_rec_ptr(list, mesh, p, uptr, vis_first, n as u32);
            return;
        }
        let (wc, s) = if same {
            // Only the view changed (the camera moved): re-cull from the cached sphere; no
            // matrix work unless parts are tested.
            self.view_hits += 1;
            (e.wc, e.s)
        } else {
            let mp = model_scaled(&to44(model), pos_scale);
            (xf(&mp, &center), scale_bound(&mp))
        };
        let vis_first = self.vis.len() as u32;
        let n = self.cull_world(mesh.0 as usize, &wc, s, model, p.no_clip);
        let frame_no = self.frame_no;
        let w0 = if n != 0 && p.object_fog && self.view.fog.is_some() { self.origin_w34(model) } else { 0.0 };
        let (fogc, fogm) = fog_fields(&self.view, p, w0);
        let nofog = self.view.fog.is_none() || p.no_clip;
        let e = &mut self.slots[i];
        if !same {
            e.mesh = mesh.0 as u16;
            e.stamp = stamp;
            e.pid = pid.min(PID_NONE) as u8;
            e.wc = wc;
            e.s = s;
            e.cf &= SLOT_CUR;
        }
        e.frame = frame_no;
        e.vis_first = vis_first as u16;
        e.vis_count = n as u16;
        if n == 0 {
            return;
        }
        let r = self.slot_ring(i);
        self.uni_used[r] = frame_no;
        let e = &mut self.slots[i];
        let su = &mut self.rings[r / SLOT_CHUNK][r % SLOT_CHUNK];
        // The ring's current uniform is this draw's with that frame's fog: with the same fog it
        // is this frame's too (no write, in flight or not). Else the next one, from the model.
        let mut cur = (e.cf & SLOT_CUR) as usize;
        let keep = e.cf & SLOT_U != 0 && if nofog { e.cf & SLOT_U_NOFOG != 0 } else { su.u[cur].fogc == fogc && su.u[cur].fogm == fogm };
        if !keep {
            cur = if cur >= 2 { 0 } else { cur + 1 };
            let u = &mut su.u[cur];
            draw_uniform_into(&model_scaled(&to44(model), pos_scale), p, u);
            (u.fogc, u.fogm) = (fogc, fogm);
            e.cf = cur as u8 | SLOT_U | if nofog { SLOT_U_NOFOG } else { 0 };
        }
        let uptr: *const R3dDrawUniform = &su.u[cur];
        self.push_rec_ptr(list, mesh, p, uptr, vis_first, n);
    }

    /// Makes room for `n` draw slots (their uniform rings come with their first draw).
    #[cold]
    #[inline(never)]
    fn grow_slots(&mut self, n: usize) {
        self.slots.resize(n, Slot::default());
        self.slot_ui.resize(n, NO_UNI);
    }

    /// Slot `i`'s uniform ring: its own, a free one, one unused for [`UNI_REUSE_FRAMES`]
    /// frames (taken from its slot, which writes a fresh uniform at its next draw), or a new
    /// chunk's (chunks are allocated whole and never move).
    fn slot_ring(&mut self, i: usize) -> usize {
        let r = self.slot_ui[i];
        if r != NO_UNI {
            return r as usize;
        }
        if self.uni_free.is_empty() {
            self.reclaim_rings();
        }
        let r = match self.uni_free.pop() {
            Some(r) => r,
            None => self.grow_rings(),
        };
        self.uni_owner[r as usize] = i as u16;
        self.slot_ui[i] = r;
        // The ring's entries are not this slot's: the draw writes a new one.
        self.slots[i].cf &= !(SLOT_U | SLOT_U_NOFOG);
        r as usize
    }

    /// Frees the rings of slots that did not queue them for [`UNI_REUSE_FRAMES`] frames.
    #[cold]
    #[inline(never)]
    fn reclaim_rings(&mut self) {
        for r in 0..self.uni_owner.len() {
            let owner = self.uni_owner[r];
            if owner != NO_SLOT && self.frame_no.wrapping_sub(self.uni_used[r]) >= UNI_REUSE_FRAMES {
                self.slot_ui[owner as usize] = NO_UNI;
                self.uni_owner[r] = NO_SLOT;
                self.uni_free.push(r as u16);
            }
        }
    }

    /// A new chunk of rings; returns one of them (the rest are free).
    #[cold]
    #[inline(never)]
    fn grow_rings(&mut self) -> u16 {
        let first = (self.rings.len() * SLOT_CHUNK) as u16;
        self.rings.push(alloc::boxed::Box::new([SlotUni::default(); SLOT_CHUNK]));
        self.uni_owner.resize(self.uni_owner.len() + SLOT_CHUNK, NO_SLOT);
        self.uni_used.resize(self.uni_used.len() + SLOT_CHUNK, 0);
        self.uni_free.extend((first + 1..first + SLOT_CHUNK as u16).rev());
        first
    }

    /// The id of a draw's parameters in `ptab` (equal parameters, equal ids, within a frame
    /// and, below `PID_NONE`, across frames until the table is reset).
    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_q")]
    fn intern_params(&mut self, p: &DrawParams) -> u16 {
        let key = params_key(p);
        // Compared word by word (an array `==` calls memcmp).
        let eq = |k: &[u32; 6]| k[0] == key[0] && k[1] == key[1] && k[2] == key[2] && k[3] == key[3] && k[4] == key[4] && k[5] == key[5];
        let last = self.plast as usize;
        if self.pkeys.get(last).is_some_and(eq) {
            return last as u16;
        }
        // Open addressing over ids < PID_NONE (fading actors give many distinct alphas: a
        // linear search over the table cost ~6 ms per frame in Lab 1). Shifts and xors
        // only: an integer multiply costs ~25 cycles here.
        let mut h = (key[0] ^ key[0] >> 9 ^ key[0] >> 17 ^ key[1] ^ key[1] >> 13 ^ (key[2] ^ key[3]) >> 7 ^ key[4] << 3 ^ key[5] << 1) as usize;
        let id = loop {
            h &= PHASH - 1;
            match self.phash[h] {
                0 => {
                    let id = self.pkeys.len();
                    self.pkeys.push(key);
                    self.ptab.push(*p);
                    if id < PID_NONE as usize {
                        self.phash[h] = id as u16 + 1;
                    }
                    break id;
                }
                e if eq(&self.pkeys[e as usize - 1]) => break e as usize - 1,
                _ => h += 1,
            }
        };
        self.plast = id as u16;
        id as u16
    }

    #[inline(always)]
    fn push_rec_ptr(&mut self, list: List, mesh: MeshId, p: &DrawParams, uptr: *const R3dDrawUniform, vis_first: u32, vis_count: u32) {
        self.pushed = self.pushed.wrapping_add(1);
        let rec =
            DrawRec { u: 0, uptr, p0: self.mhot[mesh.0 as usize].sph as u16, tex_override: p.override_tex.map_or(0, |t| t.0), n_bones: 0, no_clip: p.no_clip, vis_first: vis_first as u16, vis_count: vis_count as u16 };
        self.lists[list as usize].push(rec);
    }

    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_q")]
    fn push_rec(&mut self, list: List, mesh: MeshId, p: &DrawParams, u: R3dDrawUniform, vis_first: u32, vis_count: u32) {
        self.pushed = self.pushed.wrapping_add(1);
        let arena = &mut self.uni[self.uni_frame];
        arena.push(u);
        let rec = DrawRec {
            u: (arena.len() - 1) as u16,
            uptr: core::ptr::null(),
            p0: self.mhot[mesh.0 as usize].sph as u16,
            tex_override: p.override_tex.map_or(0, |t| t.0),
            n_bones: 0,
            no_clip: p.no_clip,
            vis_first: vis_first as u16,
            vis_count: vis_count as u16,
        };
        self.lists[list as usize].push(rec);
    }

    /// Records a draw for [`run_queue`](Self::run_queue) (same order as immediate draws):
    /// rigid (`mats[0]`, slot and stamp as in [`draw_slot`](Self::draw_slot); MODGE/WAVE
    /// animation `modge` at `modge_frame` if it turns out visible) or skinned (`mats` = bones).
    /// Runs inside the game's draw loop, between unrelated code: kept small (the I-cache is
    /// refilled for it at every call).
    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_d")]
    #[allow(clippy::too_many_arguments)]
    pub fn queue(&mut self, list: List, mesh: MeshId, mats: &[Mat4], p: &DrawParams, skinned: bool, slot: u32, stamp: u32, modge: u8, modge_frame: u32) {
        let lf = list as u8 | if p.no_clip { Q_NO_CLIP } else { 0 } | if p.object_fog { Q_OBJECT_FOG } else { 0 };
        let ovr = p.override_tex.map_or(0, |t| t.0 + 1);
        let m = mesh.0 as u16;
        self.queue_core(lf, 0, m, m, ovr, mats, p.alpha_mul, p.gouraud, skinned, slot, stamp, modge, modge_frame);
    }

    /// [`queue`](Self::queue) of the game's draw loop: the game's model id and object (the
    /// mesh is resolved in [`run_queue_with`](Self::run_queue_with)).
    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_d")]
    #[allow(clippy::too_many_arguments)]
    pub fn queue_raw(
        &mut self,
        list: List,
        model: u32,
        object: u16,
        mats: &[Mat4],
        alpha: f32,
        gouraud: Option<[f32; 3]>,
        override_tex: Option<TexId>,
        flags: (bool, bool, bool),
        slot: u32,
        stamp: u32,
        modge: u8,
        modge_frame: u32,
    ) {
        let (no_clip, object_fog, skinned) = flags;
        let lf = list as u8 | if no_clip { Q_NO_CLIP } else { 0 } | if object_fog { Q_OBJECT_FOG } else { 0 } | self.extra_lf;
        let ovr = override_tex.map_or(0, |t| t.0 + 1);
        self.queue_core(lf, model, MESH_UNRESOLVED, object, ovr, mats, alpha, gouraud, skinned, slot, stamp, modge, modge_frame);
    }

    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_d")]
    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    fn queue_core(
        &mut self,
        lf: u8,
        model: u32,
        mesh: u16,
        key: u16,
        ovr: u16,
        mats: &[Mat4],
        alpha: f32,
        gouraud: Option<[f32; 3]>,
        skinned: bool,
        slot: u32,
        stamp: u32,
        modge: u8,
        modge_frame: u32,
    ) {
        let Some(m0) = mats.first() else { return };
        if skinned || gouraud.is_some() || slot >= NO_SLOT as u32 || stamp == 0 {
            return self.queue_rare(lf, model, mesh, key, ovr, mats, alpha, gouraud, skinned, slot, stamp, modge, modge_frame);
        }
        // The slot's model matrix, written only when its (stamp, key) changed (a stamp versions
        // an actor's pose: its objects, and all landscape objects, share one).
        let i = slot as usize;
        match self.slot_model.get_mut(i) {
            Some(sm) if sm.0 == stamp && sm.1 == key => {}
            Some(sm) => *sm = (stamp, key, to34(m0)),
            None => self.grow_slot_model(i, stamp, key, m0),
        }
        self.q.push(QDraw { stamp, modge_frame, alpha, model, slot: slot as u16, mesh, key, ovr, gour: 0, lf, modge });
    }

    #[cold]
    #[inline(never)]
    fn grow_slot_model(&mut self, i: usize, stamp: u32, key: u16, m0: &Mat4) {
        self.slot_model.resize(i + 1, (0, u16::MAX, IDENTITY34));
        self.slot_model[i] = (stamp, key, to34(m0));
    }

    /// [`queue_core`](Self::queue_core) of skinned draws, draws with Gouraud colours and
    /// draws without a slot.
    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn queue_rare(
        &mut self,
        lf: u8,
        model: u32,
        mesh: u16,
        key: u16,
        ovr: u16,
        mats: &[Mat4],
        alpha: f32,
        gouraud: Option<[f32; 3]>,
        skinned: bool,
        slot: u32,
        stamp: u32,
        modge: u8,
        modge_frame: u32,
    ) {
        let gour = match gouraud {
            None => 0,
            Some(g) => {
                self.qgouraud.push(g);
                self.qgouraud.len() as u16
            }
        };
        let (stamp, slot) = if skinned {
            let first = self.qbones.len() as u32;
            self.qbones.extend_from_slice(mats);
            (first, mats.len().min(u16::MAX as usize) as u16)
        } else if slot < NO_SLOT as u32 && stamp != 0 {
            let i = slot as usize;
            if self.slot_model.len() <= i {
                self.slot_model.resize(i + 1, (0, u16::MAX, IDENTITY34));
            }
            let sm = &mut self.slot_model[i];
            if sm.0 != stamp || sm.1 != key {
                *sm = (stamp, key, to34(&mats[0]));
            }
            (stamp, slot as u16)
        } else {
            self.qmodels.push(to34(&mats[0]));
            (self.qmodels.len() as u32 - 1, NO_SLOT)
        };
        let lf = lf | if skinned { Q_SKINNED } else { 0 };
        self.q.push(QDraw { stamp, modge_frame, alpha, model, slot, mesh, key, ovr, gour, lf, modge });
    }

    /// Draws queued so far.
    pub fn queued(&self) -> usize {
        self.q.len()
    }

    /// Resolves every queued draw's mesh from now on with the resolver (no slot memo: logs
    /// of every draw, `F2N64_DUMP_FRAME`).
    pub fn resolve_all(&mut self, on: bool) {
        self.resolve_all = on;
    }

    /// Queues the draws recorded with [`queue`](Self::queue) (call before [`flush`](Self::flush)).
    pub fn run_queue(&mut self) {
        self.run_queue_with(&mut |_| None);
    }

    /// [`run_queue`](Self::run_queue); [`queue_raw`](Self::queue_raw) draws get their mesh from
    /// their slot's memo (same model and object as last time) or from `resolve` (`None`: not
    /// drawn).
    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_q")]
    pub fn run_queue_with(&mut self, resolve: &mut dyn FnMut(&RawDraw) -> Option<MeshId>) {
        let t_run = prof_ticks();
        let q = core::mem::take(&mut self.q);
        // No audio polls in here: the recording (draw loop) and the CPU batches after it poll.
        for d in q.iter() {
            let mut mesh = d.mesh;
            if mesh == MESH_UNRESOLVED {
                // The draw order rarely changes between frames: last frame's mesh of this slot.
                let memo = (d.lf & Q_SKINNED == 0 && d.slot != NO_SLOT && !self.resolve_all).then_some(d.slot as usize);
                mesh = match memo.and_then(|s| self.slot_key.get(s)) {
                    Some(&(m, o, id)) if m == d.model && o == d.key => id,
                    _ => self.resolve_raw(resolve, d, memo),
                };
            }
            if mesh >= MESH_UNRESOLVED {
                continue;
            }
            self.drawn += 1;
            let list = BY_INDEX[(d.lf & 7) as usize];
            let mesh = MeshId(mesh as u32);
            let p = DrawParams {
                alpha_mul: d.alpha,
                gouraud: if d.gour == 0 { None } else { Some(self.qgouraud[d.gour as usize - 1]) },
                override_tex: if d.ovr == 0 { None } else { Some(TexId(d.ovr - 1)) },
                no_clip: d.lf & Q_NO_CLIP != 0,
                object_fog: d.lf & Q_OBJECT_FOG != 0,
            };
            let pid = self.intern_params(&p);
            if matches!(list, List::Additive | List::LateAdditive) {
                self.ensure_additive(mesh);
            }
            if d.lf & Q_SKINNED != 0 {
                let bones = core::mem::take(&mut self.qbones);
                self.draw_skinned_q(list, mesh, &bones[d.stamp as usize..d.stamp as usize + d.slot as usize], &p);
                self.qbones = bones;
                continue;
            }
            // World-space landscape meshes (packer-baked key-0 transforms): identity.
            // By reference: most draws never read it (camera-only changes of whole meshes).
            let model: *const Model34 = if self.mesh_is_world(mesh) {
                &IDENTITY34
            } else if d.slot == NO_SLOT {
                &self.qmodels[d.stamp as usize]
            } else {
                &self.slot_model[d.slot as usize].2
            };
            let pushed = self.pushed;
            // SAFETY: `qmodels` and `slot_model` are only changed by `queue`, not while the
            // queue runs.
            self.draw_slot_pid(list, mesh, unsafe { &*model }, &p, pid, d.slot, if d.slot == NO_SLOT { 0 } else { d.stamp });
            if d.lf & Q_CHECK_CULLED != 0 {
                self.cull_check_draw(d.model, d.key, self.pushed != pushed);
            }
            // MODGE/WAVE vertices only for meshes on screen (the RSP reads them at the flush).
            if d.modge != 0 && self.pushed != pushed {
                self.modge_todo.push((mesh, d.modge_frame, d.modge));
            }
        }
        // After the loop: its code would evict the loop's from the I-cache.
        let todo = core::mem::take(&mut self.modge_todo);
        for &(mesh, fc, flags) in &todo {
            self.animate_modge(mesh, fc, flags);
        }
        self.modge_todo = todo;
        self.modge_todo.clear();
        self.q = q;
        self.q.clear();
        self.qbones.clear();
        self.qmodels.clear();
        self.qgouraud.clear();
        if R3D_PROF {
            self.prof.run_ticks = self.prof.run_ticks.wrapping_add(prof_ticks().wrapping_sub(t_run));
        }
    }

    /// Queues a skinned mesh draw: one model → world matrix per bone (pre-order object
    /// index, as `xform_actor` returns them).
    pub fn draw_skinned(&mut self, list: List, mesh: MeshId, bones: &[Mat4], p: &DrawParams) {
        self.queue(list, mesh, bones, p, true, u32::MAX, 0, 0, 0);
    }

    /// Testing (`F2N64_CULL_CHECK`): counts the draws of actors the draw loop culled, and
    /// those this batch did not cull wholly (logged: the actor test must never cull a draw
    /// that shows).
    #[cold]
    #[inline(never)]
    fn cull_check_draw(&mut self, model: u32, object: u16, shown: bool) {
        self.cull_checked += 1;
        if shown {
            self.cull_wrong += 1;
            if self.cull_wrong <= 20 {
                #[cfg(target_vendor = "nintendo64")]
                crate::log!("r3d: CULL CHECK FAILED: model {:08x} object {} is visible", model, object);
            }
        }
        if self.cull_checked % 2000 == 0 {
            #[cfg(target_vendor = "nintendo64")]
            crate::log!("r3d: cull check: {} draws of culled actors, {} visible", self.cull_checked, self.cull_wrong);
        }
        let _ = (model, object);
    }

    /// Asks the resolver for a raw draw's mesh (and remembers it for the slot).
    #[cold]
    #[inline(never)]
    fn resolve_raw(&mut self, resolve: &mut dyn FnMut(&RawDraw) -> Option<MeshId>, d: &QDraw, memo: Option<usize>) -> u16 {
        let raw = RawDraw { model: d.model, object: d.key, list: BY_INDEX[(d.lf & 7) as usize], alpha: d.alpha, no_clip: d.lf & Q_NO_CLIP != 0 };
        let mesh = resolve(&raw).map_or(MESH_SKIP, |m| m.0 as u16);
        // Only found meshes are remembered (a missing one is looked up again next time).
        if let (Some(s), true) = (memo, mesh != MESH_SKIP) {
            if self.slot_key.len() <= s {
                self.slot_key.resize(s + 1, (0, u16::MAX, MESH_SKIP));
            }
            self.slot_key[s] = (d.model, d.key, mesh);
        }
        mesh
    }

    /// The batch's skinned draw (`run_queue`).
    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_q")]
    fn draw_skinned_q(&mut self, list: List, mesh: MeshId, bones: &[Mat4], p: &DrawParams) {
        let t0 = prof_ticks();
        self.draw_skinned_inner(list, mesh, bones, p);
        if R3D_PROF {
            self.prof.skin_ticks = self.prof.skin_ticks.wrapping_add(prof_ticks().wrapping_sub(t0));
            self.prof.skin_draws += 1;
        }
    }

    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_q")]
    fn draw_skinned_inner(&mut self, list: List, mesh: MeshId, bones: &[Mat4], p: &DrawParams) {
        let Some(res) = self.meshes.get(mesh.0 as usize) else { return };
        if bones.is_empty() {
            return;
        }
        // Per-object fog depth: `W` of the root bone's origin (only needed with fog).
        let w0 = if p.object_fog && self.view.fog.is_some() { self.origin_w(&bones[0]) } else { 0.0 };
        let (fogc, fogm) = fog_fields(&self.view, p, w0);
        let pos_scale = res.pos_scale;
        let arena = &mut self.uni[self.uni_frame];
        let first = arena.len() as u32;
        // Converted in place (no 96-byte temporaries).
        arena.resize(first as usize + bones.len(), R3dDrawUniform::default());
        for (b, u) in bones.iter().zip(&mut arena[first as usize..]) {
            draw_uniform_into(&model_scaled(b, pos_scale), p, u);
            (u.fogc, u.fogm) = (fogc, fogm);
        }
        let res = &self.meshes[mesh.0 as usize];
        self.queued_tris += res.tris;
        let vis_first = self.vis.len() as u32;
        let n = res.parts.len();
        self.vis.extend(0..n as u16);
        let rec = DrawRec {
            u: first as u16,
            uptr: core::ptr::null(),
            p0: self.mhot[mesh.0 as usize].sph as u16,
            tex_override: p.override_tex.map_or(0, |t| t.0),
            // Bone uniforms only for skinned meshes (a rigid one draws with the first).
            n_bones: if res.skinned { bones.len().min(u8::MAX as usize) as u8 } else { 0 },
            no_clip: p.no_clip,
            vis_first: vis_first as u16,
            vis_count: n as u16,
        };
        self.lists[list as usize].push(rec);
    }

    /// Draws all queued lists in `DrawAllFrames` order; `after(list)` runs after each list
    /// (for CPU-side batches that belong between lists: sprites in `Late`, HUD in
    /// `Overlay`, ...). Clears the lists.
    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_f")]
    pub fn flush(&mut self, mut after: impl FnMut(List)) {
        if !self.q.is_empty() {
            self.run_queue();
        }
        #[cfg(target_vendor = "nintendo64")]
        unsafe {
            ffi::n64_r3d_begin(&self.frame);
            for &(_, d, size) in &self.modge_cmds {
                ffi::n64_r3d_modge(d, size);
            }
        };
        if option_env!("F2N64_MODGE_CHECK").is_some() && self.frame_no % 64 == 0 {
            self.modge_check();
        }
        self.modge_cmds.clear();
        for list in LIST_ORDER {
            self.flush_list(list);
            after(list);
        }
        if R3D_PROF {
            let c = self.stats();
            self.prof.flush_state = self.prof.flush_state.wrapping_add(c.ticks_state);
            self.prof.flush_draws = self.prof.flush_draws.wrapping_add(c.ticks_draws);
        }
        // `vis` stays until the next `begin_frame` (reused draws copy from it).
    }

    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_f")]
    fn flush_list(&mut self, list: List) {
        let recs = &self.lists[list as usize];
        if recs.is_empty() {
            return;
        }
        let t_fl = prof_ticks();
        // Resolve handles and textures first (the draw structs point into the scratch arrays).
        self.handle_scratch.clear();
        self.tex_scratch.clear();
        self.draw_scratch.clear();
        let (vis, gparts, crc_tex) = (&self.vis, &self.gparts, &self.crc_tex);
        let mut sent = 0;
        for r in recs {
            let off = self.handle_scratch.len();
            let parts = &gparts[r.p0 as usize..];
            for &pi in &vis[r.vis_first as usize..r.vis_first as usize + r.vis_count as usize] {
                let p = parts[pi as usize];
                sent += p.tris as u32;
                self.handle_scratch.push(p.handle);
                self.tex_scratch.push(if r.tex_override != 0 { r.tex_override } else { crc_tex[p.cid as usize] });
            }
            let u: *const R3dDrawUniform = if r.uptr.is_null() { &self.uni[self.uni_frame][r.u as usize] } else { r.uptr };
            let skinned = r.n_bones > 0;
            self.draw_scratch.push(R3dDraw {
                u,
                // Filled in below (the scratch arrays may still grow).
                groups: off as *const u16,
                texs: core::ptr::null(),
                n_groups: r.vis_count as u32,
                bones: if skinned { u } else { core::ptr::null() },
                n_bones: r.n_bones as u32,
                frame: if r.no_clip { &self.frame_noclip } else { core::ptr::null() },
            });
        }
        self.sent_tris += sent;
        for d in &mut self.draw_scratch {
            let off = d.groups as usize;
            d.groups = self.handle_scratch[off..].as_ptr();
            d.texs = self.tex_scratch[off..].as_ptr();
        }
        let state = list.state(self.view.bilinear, self.view.fog.is_some());
        if R3D_PROF {
            self.prof.flush_rust = self.prof.flush_rust.wrapping_add(prof_ticks().wrapping_sub(t_fl));
        }
        #[cfg(target_vendor = "nintendo64")]
        unsafe {
            ffi::n64_r3d_draw_list(&state, self.draw_scratch.as_ptr(), self.draw_scratch.len() as u32)
        };
        #[cfg(not(target_vendor = "nintendo64"))]
        let _ = state;
        self.lists[list as usize].clear();
    }

    /// Testing (`F2N64_MODGE_CHECK=1`): waits for the RSP and compares the vertex buffers
    /// `F2_Modge` wrote with the CPU's result (logged).
    #[cold]
    #[inline(never)]
    fn modge_check(&mut self) {
        #[cfg(target_vendor = "nintendo64")]
        unsafe {
            ffi::n64_rdp_wait()
        };
        for &(mesh, _, _) in &self.modge_cmds {
            let res = &self.meshes[mesh as usize];
            let Some(st) = res.modge.as_deref() else { continue };
            let Some(r) = &st.rsp else { continue };
            // The RSP wrote RDRAM behind the cache: read through the uncached segment.
            let v = (res.verts as usize & 0x1FFF_FFFF) | 0xA000_0000;
            let base = r.buf.as_ptr() as *const u8;
            let desc = r.desc_off + r.cur * r.desc_stride;
            // SAFETY: inside `buf` (the descriptor's time phases).
            let tp: [u16; 6] = core::array::from_fn(|i| unsafe { u16::from_be_bytes(*(base.add(desc + 40 + 2 * i) as *const [u8; 2])) });
            let t = sin_q14();
            let xt: Vec<[u16; 3]> = st.xph.iter().map(|p| modge_x16(t, &ph16(p), &tp)).collect();
            let zt: Vec<[u16; 3]> = st.zph.iter().map(|p| modge_z16(t, &ph16(p), &tp)).collect();
            let pt: Vec<ModgePair> = st.pairs.iter().map(|&(xi, zi, a0)| modge_pair16(t, &xt[xi as usize], &zt[zi as usize], a0, r.k)).collect();
            let (mut bad, mut first) = (0u32, None);
            for (i, b) in st.base.iter().enumerate() {
                // SAFETY: both point to `n_verts * 16` bytes.
                let (got, orig): ([u8; 16], [u8; 16]) =
                    unsafe { (core::ptr::read_volatile((v + i * 16) as *const [u8; 16]), *(base.add(i * 16) as *const [u8; 16])) };
                let q = &pt[b.pair as usize];
                let (y, s, t) = modge_apply(q, b.y, b.s, b.t);
                let mut want = orig;
                [want[2], want[3]] = y.to_be_bytes();
                [want[12], want[13]] = s.to_be_bytes();
                [want[14], want[15]] = t.to_be_bytes();
                if modge_writes_alpha(st.flags) {
                    want[11] = q.a;
                }
                if got != want {
                    bad += 1;
                    first.get_or_insert((i, got, want));
                }
            }
            #[cfg(target_vendor = "nintendo64")]
            crate::log!("modge check mesh {:08x}: {} vertices, {} mismatches, first {:?}", res.id, st.base.len(), bad, first);
            let _ = (bad, first);
        }
    }

    /// Drops the lists queued this frame (nothing to draw), keeping the reusable draws.
    pub fn discard_frame(&mut self) {
        for l in &mut self.lists {
            l.clear();
        }
        self.modge_cmds.clear();
        self.q.clear();
        self.qbones.clear();
        self.qmodels.clear();
        self.qgouraud.clear();
    }

    /// Drops everything queued this frame without drawing it.
    pub fn discard(&mut self) {
        for l in &mut self.lists {
            l.clear();
        }
        self.modge_cmds.clear();
        self.q.clear();
        self.qbones.clear();
        self.qmodels.clear();
        self.qgouraud.clear();
        self.vis.clear();
        // Mesh ids are reused after a reset: forget the reusable draws and the slot memo.
        self.slots.clear();
        self.slot_key.clear();
        self.prev_vis.clear();
    }

    /// C-side counters of the last flushed frame.
    pub fn stats(&self) -> ffi::R3dStats {
        #[allow(unused_mut)]
        let mut s = ffi::R3dStats::default();
        #[cfg(target_vendor = "nintendo64")]
        unsafe {
            ffi::n64_r3d_stats(&mut s)
        };
        s
    }
}

/// Bit-level model of the RSP vertex shader (`rsp_f2mdx.S`) for host tests: returns the
/// stored screen position (s13.2 x, y), z, clip byte and trivial-reject byte of a vertex.
pub fn emulate_vertex(u: &R3dDrawUniform, fr: &R3dFrame, guard: i32, pos: [i16; 3]) -> ([i32; 2], i32, u8, u8) {
    let mm = rsp_fold(u, fr);
    let m = |r: usize, c: usize| mm[r][c];
    let sat = |v: i64| v.clamp(i32::MIN as i64, i32::MAX as i64);
    // Clip-space s15.16 (the RSP clamps the accumulator high half to s16).
    let cs: [i64; 4] = core::array::from_fn(|c| sat(pos[0] as i64 * m(0, c) + pos[1] as i64 * m(1, c) + pos[2] as i64 * m(2, c) + m(3, c)));
    let (x, y, z, w) = (cs[0], cs[1], cs[2], cs[3]);
    // Clip codes: magma scales (X, Y, Z, W) by its clip factors (1, 1, G, G), saturating like
    // the clip-space values, and compares X, Y, Z * G with W * G.
    let gw = sat(w * guard as i64);
    let zg = sat(z * guard as i64);
    let mut clip = 0u8;
    if x <= -gw {
        clip |= 1;
    }
    if y <= -gw {
        clip |= 2;
    }
    if zg <= -gw {
        clip |= 4;
    }
    if x >= gw {
        clip |= 8;
    }
    if y >= gw {
        clip |= 16;
    }
    if zg >= gw {
        clip |= 32;
    }
    let q = (w >> 16).max(1);
    let s16 = |v: i64| v.clamp(-32768, 32767) as i32;
    let scr = |c: i64, o: i16| s16(o as i64 + (((c / (2 * q)) * fr.vs[0] as i64) >> 16));
    let sx = scr(x, fr.vo[0]);
    let sy = scr(y, fr.vo[1]);
    let k = ((fr.vs[2] as i64) << 16) | (fr.vlo[2] as u16 as i64);
    let zz = s16(fr.vo[2] as i64 + ((w * k) >> 32));
    let inside = |s: i32, lo: i16, hi: i16| s > lo as i32 && s < hi as i32;
    let tr = 0x3C | inside(sx, fr.vlo[0], fr.vhi[0]) as u8 | (inside(sy, fr.vlo[1], fr.vhi[1]) as u8) << 1;
    ([sx, sy], zz, clip, tr)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `animate_modge`'s integer per-x/per-z decomposition matches the direct
    /// `PCCalcModgeValues` terms (f2-engine `calc_modge_values`, f64 libm here).
    #[test]
    fn modge_matches_pc_terms() {
        let (xs, idx) = distinct(&[3.0, -1.0, 3.0, 0.0, -1.0]);
        assert_eq!((xs, idx), (alloc::vec![0.0, 3.0, -1.0], alloc::vec![1, 2, 1, 0, 2]));
        let tb = sin_q14();
        let mut seed = 77u32;
        let mut rnd = |m: u32| {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            (seed >> 8) % m
        };
        let (mut worst_st, mut worst_y, mut worst_a) = (0i32, 0f64, 0i32);
        for _ in 0..20000 {
            let (x, z) = ((rnd(8000) as f32 - 4000.0) * 0.25, (rnd(8000) as f32 - 4000.0) * 0.25);
            let fc = rnd(400_000);
            let scale = [0.25f32, 0.125, 1.0 / 16.0][rnd(3) as usize];
            let (y0, s0, t0, a0) = (rnd(2000) as i16 - 1000, rnd(4000) as i16 - 2000, rnd(4000) as i16 - 2000, rnd(256) as u8);
            let t = fc as f64;
            let (xd, zd) = (x as f64, z as f64);
            let mx = ((zd * (1.5 / 35.0) + (xd * (1.3 / 49.0) + t * (2.0 / 56.0)).sin() + t * (1.5 / 70.0)).sin() + 1.0) * 0.1;
            let mz = ((xd * (1.5 / 49.0) + (zd * (1.3 / 29.0) + t * (2.0 / 66.0)).cos() + t * (1.5 / 66.0)).cos() + 1.0) * 0.1;
            let my = ((zd * (1.2 / 26.0) + (xd * (1.3 / 49.0) + t * (1.0 / 26.0)).cos() + t * (1.0 / 26.0)).sin() + 1.0) * 5.0;
            let tp = modge_time_phases(fc);
            let px = modge_x_terms(tb, &modge_x_phases(x), &tp, true);
            let pz = modge_z_terms(tb, &modge_z_phases(z), &tp, true);
            let k = 5.0 / scale as f64 / 1024.0;
            let mut ky_sh = 30;
            while ky_sh > 1 && k * (1u64 << ky_sh) as f64 * 2100.0 >= i32::MAX as f64 {
                ky_sh -= 1;
            }
            let ky = (k * (1u64 << ky_sh) as f64).round() as i32;
            let (y, s, tt, a) = modge_vertex(&px, &pz, y0, s0, t0, a0, ky, ky_sh);
            let ey = y0 as f64 + my / scale as f64;
            let (es, et) = ((s0 as f64 + mx * 0.4 * 1024.0).round() as i32, (t0 as f64 + mz * 0.4 * 1024.0).round() as i32);
            let ea = (a0 as f64 * (50.0 + (mx + mz) * 400.0) / 130.0).round().min(255.0) as i32;
            worst_st = worst_st.max((s as i32 - es).abs()).max((tt as i32 - et).abs());
            worst_y = worst_y.max((y as f64 - ey).abs() * scale as f64);
            worst_a = worst_a.max((a as i32 - ea).abs());
        }
        // s/t in 1/1024 texels (the wobble spans ±41), y in world units (wave height 0..10).
        std::eprintln!("modge worst: s/t {worst_st}, y {worst_y}, alpha {worst_a}");
        assert!(worst_st <= 1, "s/t off by {worst_st}");
        assert!(worst_y <= 0.6, "y off by {worst_y} world units");
        assert!(worst_a <= 2, "alpha off by {worst_a}");
    }

    /// `F2_Modge`'s arithmetic (the bit-exact model `modge_pair16`) against the f64 PC terms.
    #[test]
    fn rsp_modge_matches_pc_terms() {
        let tb = sin_q14();
        let mut seed = 1234u32;
        let mut rnd = |m: u32| {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            (seed >> 8) % m
        };
        let (mut worst_st, mut worst_y, mut worst_a) = (0i32, 0f64, 0i32);
        for _ in 0..40000 {
            let (x, z) = ((rnd(8000) as f32 - 4000.0) * 0.25, (rnd(8000) as f32 - 4000.0) * 0.25);
            let fc = rnd(400_000);
            let scale = [0.25f32, 0.125, 1.0 / 16.0, 1.0 / 32.0][rnd(4) as usize];
            let (y0, s0, t0, a0) = (rnd(2000) as i16 - 1000, rnd(4000) as i16 - 2000, rnd(4000) as i16 - 2000, rnd(256) as u8);
            let t = fc as f64;
            let (xd, zd) = (x as f64, z as f64);
            let mx = ((zd * (1.5 / 35.0) + (xd * (1.3 / 49.0) + t * (2.0 / 56.0)).sin() + t * (1.5 / 70.0)).sin() + 1.0) * 0.1;
            let mz = ((xd * (1.5 / 49.0) + (zd * (1.3 / 29.0) + t * (2.0 / 66.0)).cos() + t * (1.5 / 66.0)).cos() + 1.0) * 0.1;
            let my = ((zd * (1.2 / 26.0) + (xd * (1.3 / 49.0) + t * (1.0 / 26.0)).cos() + t * (1.0 / 26.0)).sin() + 1.0) * 5.0;
            let tp = modge_time_phases(fc);
            let px = modge_x_terms(tb, &modge_x_phases(x), &tp, true);
            let pz = modge_z_terms(tb, &modge_z_phases(z), &tp, true);
            let _ = (&px, &pz);
            let (y, s, tt, a) = modge_vertex16(x, z, fc, y0, s0, t0, a0, modge_k16(scale, 3));
            let ey = y0 as f64 + my / scale as f64;
            let (es, et) = ((s0 as f64 + mx * 0.4 * 1024.0).round() as i32, (t0 as f64 + mz * 0.4 * 1024.0).round() as i32);
            let ea = (a0 as f64 * (50.0 + (mx + mz) * 400.0) / 130.0).round().min(255.0) as i32;
            worst_st = worst_st.max((s as i32 - es).abs()).max((tt as i32 - et).abs());
            worst_y = worst_y.max((y as f64 - ey).abs() * scale as f64);
            worst_a = worst_a.max((a as i32 - ea).abs());
        }
        std::eprintln!("rsp modge worst: s/t {worst_st}, y {worst_y}, alpha {worst_a}");
        assert!(worst_st <= 1, "s/t off by {worst_st}");
        assert!(worst_y <= 0.6, "y off by {worst_y} world units");
        assert!(worst_a <= 2, "alpha off by {worst_a}");
        // Without MODGE / WAVE the offsets vanish.
        let q = modge_pair16(tb, &[1, 2, 3], &[4, 5, 6], 100, modge_k16(0.125, 0));
        assert_eq!((q.dy, q.ds, q.dt), (0, 0, 0));
    }

    /// f2-engine `View::project` (scene.rs) for reference.
    fn mdx_project(v: &ViewParams, p: [f32; 3]) -> Option<[f32; 3]> {
        let z = p[2] + DIST;
        if !(z > v.near && z < v.far) {
            return None;
        }
        let i = ((p[2] as i64) + DIST as i64).clamp(1, 65534) as f32;
        let o = -v.fov * (v.proj_scale() / i);
        let [x, y, w, h] = v.viewport;
        Some([x + w / 2.0 + p[0] * o, y + h / 2.0 + p[1] * o, p[2]])
    }

    fn look_at(eye: [f32; 3], at: [f32; 3]) -> Mat4 {
        // Simple view matrix: camera at `eye` looking at `at` (+z forward), y up.
        let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        let norm = |a: [f32; 3]| {
            let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
            [a[0] / l, a[1] / l, a[2] / l]
        };
        let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
        let f = norm(sub(at, eye));
        let r = norm(cross([0.0, 1.0, 0.0], f));
        let u = cross(f, r);
        let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        [[r[0], u[0], f[0], 0.0], [r[1], u[1], f[1], 0.0], [r[2], u[2], f[2], 0.0], [-dot(r, eye), -dot(u, eye), -dot(f, eye), 1.0]]
    }

    /// Sky draws (`no_clip`, far at `NO_CLIP_FAR`): vertices far away (W * guard beyond s15.16)
    /// or with view-space x/y beyond ±32768 (the Space backgrounds: scale 5, coordinates up to
    /// 6610) get the clip codes and screen positions of MDX's projection, instead of a far
    /// flag (face dropped), a missing guard flag or a position saturated towards a corner.
    #[test]
    fn sky_vertices_project_like_mdx() {
        let mut view = ViewParams::pc_default(320.0, 240.0);
        view.view = look_at([30.0, 400.0, -500.0], [0.0, 0.0, 100.0]);
        let fr = frame_uniform_vp(&view, true);
        let mut far_view = view;
        far_view.far = NO_CLIP_FAR;
        let model: Mat4 = [[5.0, 0.0, 0.0, 0.0], [0.0, 5.0, 0.0, 0.0], [0.0, 0.0, 5.0, 0.0], [528.7, 285.7, 1170.0, 1.0]];
        let u = draw_uniform(&model_scaled(&model, 1.0), &DrawParams::default());
        let mv = mtx_cat(&model, &view.view);
        let g = view.guard as f32;
        let (mut drawn, mut big, mut deep, mut flagged) = (0, 0, 0, 0);
        let mut seed = 777u32;
        for _ in 0..4000 {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let p = [((seed >> 8) % 13220) as i16 - 6610, ((seed >> 4) % 13220) as i16 - 6610, ((seed >> 12) % 13220) as i16 - 6610];
            let mp = p.map(|c| c as f32);
            let v: [f32; 3] = core::array::from_fn(|c| mp[0] * mv[0][c] + mp[1] * mv[1][c] + mp[2] * mv[2][c] + mv[3][c]);
            let (scr, _, clip, _) = emulate_vertex(&u, &fr, fr.guard as i32, p);
            let w = v[2] + DIST;
            let Some(e) = mdx_project(&far_view, v) else { continue };
            // Boundary / integer-truncation rounding cases. Near the sky's far plane Z - W =
            // (a - 1) W + b changes by only 0.00067 per unit of W (near 10, far 30000), below
            // the s15.16 resolution of `a * W`: the far test is approximate within ~1000 units.
            if (w - view.near).abs() < 0.01 || (w - NO_CLIP_FAR).abs() < 1000.0 || (v[2] - v[2].round()).abs() < 0.01 {
                continue;
            }
            assert_eq!(clip & 0x24, 0, "{v:?}: near/far flag inside near/far");
            let band = v[0].abs().max(v[1].abs()) / w;
            if band > g * 1.001 {
                assert_ne!(clip & 0x1B, 0, "{v:?}: outside the guard band, not flagged");
                flagged += 1;
                continue;
            }
            if band > g * 0.999 {
                continue;
            }
            assert_eq!(clip, 0, "{v:?}: inside the guard band, flagged");
            let (sx, sy) = (scr[0] as f32 / 4.0, scr[1] as f32 / 4.0);
            // Half a pixel, plus the reciprocal's relative error off-screen (the guard band
            // reaches ~1350 px from the centre).
            let tol = |e: f32, c: f32| 0.5 + (e - c).abs() / 2048.0;
            assert!((sx - e[0]).abs() <= tol(e[0], 160.0) && (sy - e[1]).abs() <= tol(e[1], 120.0), "{v:?}: rsp ({sx}, {sy}) mdx {e:?}");
            drawn += 1;
            big += (v[0].abs() > 32768.0 || v[1].abs() > 32768.0) as u32;
            deep += (w > 32768.0 / g) as u32;
        }
        assert!(drawn > 500 && big > 20 && deep > 200 && flagged > 100, "drawn {drawn}, beyond ±32768 {big}, beyond W*G overflow {deep}, guard-flagged {flagged}");
    }

    #[test]
    fn shader_model_matches_mdx_projection() {
        let mut view = ViewParams::pc_default(320.0, 240.0);
        view.view = look_at([30.0, 400.0, -500.0], [0.0, 0.0, 100.0]);
        let fr = frame_uniform_vp(&view, false);
        // A mesh at some world position with a rotation, stored with pos_shift 3.
        let (s, c) = (0.3f32.sin(), 0.3f32.cos());
        let model: Mat4 = [[c, 0.0, -s, 0.0], [0.0, 1.0, 0.0, 0.0], [s, 0.0, c, 0.0], [120.0, -5.0, 60.0, 1.0]];
        let shift = 3;
        let u = draw_uniform(&model_scaled(&model, 1.0 / (1 << shift) as f32), &DrawParams::default());
        let mut checked = 0;
        let mut seed = 12345u32;
        for _ in 0..2000 {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let p = [((seed >> 8) % 4000) as i16 - 2000, ((seed >> 4) % 800) as i16 - 400, ((seed >> 12) % 4000) as i16 - 2000];
            let mp = [p[0] as f32 / 8.0, p[1] as f32 / 8.0, p[2] as f32 / 8.0];
            // Reference: f32 model x view, then MDX projection.
            let mv = mtx_cat(&model, &view.view);
            let v = [
                mp[0] * mv[0][0] + mp[1] * mv[1][0] + mp[2] * mv[2][0] + mv[3][0],
                mp[0] * mv[0][1] + mp[1] * mv[1][1] + mp[2] * mv[2][1] + mv[3][1],
                mp[0] * mv[0][2] + mp[1] * mv[1][2] + mp[2] * mv[2][2] + mv[3][2],
            ];
            let (scr, z, clip, tr) = emulate_vertex(&u, &fr, view.guard as i32, p);
            match mdx_project(&view, v) {
                None => {
                    // Off near/far: the shader flags a z plane (unless within rounding of it).
                    let w = v[2] + DIST;
                    if (w - view.near).abs() > 0.01 && (w - view.far).abs() > 0.01 {
                        assert!(clip & 0x24 != 0, "{v:?} not flagged ({clip:x})");
                    }
                }
                Some(e) => {
                    let w = v[2] + DIST;
                    if (w - view.near).abs() < 0.01 || (w - view.far).abs() < 0.01 || (v[2] - v[2].round()).abs() < 0.01 {
                        continue; // boundary / integer-truncation rounding cases
                    }
                    assert_eq!(clip & 0x24, 0, "{v:?}");
                    if clip != 0 {
                        continue; // guard band: clipped, recomputed by the clipper
                    }
                    let (sx, sy) = (scr[0] as f32 / 4.0, scr[1] as f32 / 4.0);
                    assert!((sx - e[0]).abs() <= 0.5 && (sy - e[1]).abs() <= 0.5, "{v:?}: rsp ({sx}, {sy}) mdx {e:?}");
                    let ez = v[2] * view.z_scale();
                    // The RSP folds two s15.16 matrices (model, view x projection): ~0.1 unit of
                    // depth error, up to 3 of the 32767 z steps (0.01 % of the depth range).
                    assert!((z as f32 - ez).abs() <= 3.0, "z {z} vs {ez}");
                    let inside_x = e[0] > 0.0 && e[0] < 319.0;
                    let inside_y = e[1] > 0.0 && e[1] < 239.0;
                    if (e[0] - 0.0).abs() > 0.5 && (e[0] - 319.0).abs() > 0.5 {
                        assert_eq!(tr & 1 != 0, inside_x, "{e:?}");
                    }
                    if (e[1] - 0.0).abs() > 0.5 && (e[1] - 239.0).abs() > 0.5 {
                        assert_eq!(tr & 2 != 0, inside_y, "{e:?}");
                    }
                    checked += 1;
                }
            }
        }
        assert!(checked > 300, "only {checked} vertices checked");
    }

    /// The integer conversion equals the FPU one (multiply, truncate, saturate).
    #[test]
    fn to_fixed_matches_fpu() {
        let fpu = |v: f32| -> (i16, u16) {
            let s = v * 65536.0;
            let x = if s.is_nan() {
                0
            } else if s >= 2_147_483_520.0 {
                i32::MAX
            } else if s > -2_147_483_520.0 {
                s as i32
            } else {
                i32::MIN
            };
            ((x >> 16) as i16, x as u16)
        };
        let mut seed = 0x1234_5678u32;
        let mut vals = vec![
            0.0f32,
            -0.0,
            1.0,
            -1.0,
            0.5,
            1e-6,
            -1e-6,
            1.0 / 65536.0,
            -1.0 / 65536.0,
            32767.99,
            -32767.99,
            32768.0,
            -32768.0,
            1e9,
            -1e9,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NAN,
            1e-40,
        ];
        for _ in 0..200_000 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let mag = [1.0f32, 0.01, 30.0, 3000.0, 32000.0][(seed >> 29) as usize % 5];
            vals.push(((seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 2.0 * mag);
        }
        for v in vals {
            assert_eq!(to_fixed(v), fpu(v), "{v:e}");
        }
    }

    /// A draw reused from the previous frame queues exactly what a fresh one would.
    #[test]
    fn reused_draws_match_fresh_ones() {
        let mut view = ViewParams::pc_default(320.0, 240.0);
        view.view = look_at([30.0, 400.0, -500.0], [0.0, 0.0, 100.0]);
        let model: Mat4 = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [10.0, 0.0, 20.0, 1.0]];
        let bytes = super::cull_tests::terrain();
        let snapshot = |r: &R3d| -> (Vec<u16>, Vec<R3dDrawUniform>, u32) {
            let us =
                r.lists[List::Normal as usize].iter().map(|d| if d.uptr.is_null() { r.uni[r.uni_frame][d.u as usize] } else { unsafe { *d.uptr } }).collect();
            (r.vis.clone(), us, r.sent_tris)
        };
        let mut fresh = R3d::new();
        let ids = fresh.load_file(bytes.clone()).unwrap();
        fresh.begin_frame(&view);
        fresh.draw(List::Normal, ids[0], &model, &DrawParams::default());
        fresh.run_queue();
        let want = snapshot(&fresh);
        let mut r = R3d::new();
        let ids = r.load_file(bytes).unwrap();
        for frame in 0..3 {
            r.begin_frame(&view);
            r.draw_slot(List::Normal, ids[0], &model, &DrawParams::default(), 0, 7);
            r.run_queue();
            assert_eq!(snapshot(&r), want, "frame {frame}");
            assert!(!r.lists[List::Normal as usize][0].uptr.is_null());
            assert_eq!(r.cache_hits, (frame > 0) as u32);
            r.flush(|_| {});
        }
        // A changed view invalidates the reuse.
        view.far = 900.0;
        r.begin_frame(&view);
        r.draw_slot(List::Normal, ids[0], &model, &DrawParams::default(), 0, 7);
        r.run_queue();
        assert_eq!(r.cache_hits, 0);
    }

    /// Draws recorded with `queue` and run after the draw loop give the immediate draws' lists.
    #[test]
    fn queued_draws_match_immediate_ones() {
        let bytes = super::cull_tests::terrain();
        let mut view = ViewParams::pc_default(320.0, 240.0);
        view.view = camera_view([0.0, 300.0, -700.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let m1: Mat4 = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [10.0, 0.0, 20.0, 1.0]];
        let m2: Mat4 = [[0.0, 0.0, -1.0, 0.0], [0.0, 1.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], [-40.0, 5.0, 60.0, 1.0]];
        let pg = DrawParams { gouraud: Some([0.5, 0.25, 1.0]), alpha_mul: 0.75, object_fog: true, ..Default::default() };
        let po = DrawParams { override_tex: Some(TexId(3)), no_clip: true, ..Default::default() };
        let draws: [(List, &Mat4, DrawParams, u32, u32); 4] =
            [(List::Normal, &m1, DrawParams::default(), 0, 5), (List::Xlu, &m2, pg, 1, 6), (List::Background, &m1, po, u32::MAX, 0), (List::Additive, &m2, pg, 2, 7)];
        let lists = |r: &R3d| -> Vec<Vec<(Vec<u16>, R3dDrawUniform, u16, u16, u8, bool)>> {
            r.lists
                .iter()
                .map(|l| {
                    l.iter()
                        .map(|d| {
                            let u = if d.uptr.is_null() { r.uni[r.uni_frame][d.u as usize] } else { unsafe { *d.uptr } };
                            (r.vis[d.vis_first as usize..(d.vis_first + d.vis_count) as usize].to_vec(), u, d.p0, d.tex_override, d.n_bones, d.no_clip)
                        })
                        .collect()
                })
                .collect()
        };
        let mut a = R3d::new();
        let ia = [a.load_file(bytes.clone()).unwrap()[0], a.load_file(bytes.clone()).unwrap()[0]];
        let mut b = R3d::new();
        let ib = [b.load_file(bytes.clone()).unwrap()[0], b.load_file(bytes).unwrap()[0]];
        for f in 0..4 {
            a.begin_frame(&view);
            b.begin_frame(&view);
            for (k, (list, m, p, slot, stamp)) in draws.iter().enumerate() {
                a.draw_slot(*list, ia[0], m, p, *slot, *stamp);
                b.queue(*list, ib[0], &[**m], p, false, *slot, *stamp, 0, 0);
                if k == 1 {
                    a.draw_skinned(List::Xlu, ia[0], &[m1, m2], &pg);
                    b.queue(List::Xlu, ib[0], &[m1, m2], &pg, true, 0, 0, 0, 0);
                }
            }
            // One pose (stamp) for two objects that swap slots between frames.
            let (x, y) = if f % 2 == 0 { (0, 1) } else { (1, 0) };
            for (k, slot) in [(x, 10), (y, 11)] {
                let m = [&m1, &m2][k];
                a.draw_slot(List::Normal, ia[k], m, &DrawParams::default(), slot, 99);
                b.queue(List::Normal, ib[k], &[*m], &DrawParams::default(), false, slot, 99, 0, 0);
            }
            a.run_queue();
            b.run_queue();
            assert_eq!(lists(&a), lists(&b));
            a.flush(|_| {});
            b.flush(|_| {});
        }
    }

    /// Equal parameters get equal ids (below `PID_NONE`), different ones different ids.
    #[test]
    fn interned_params() {
        let mut r = R3d::new();
        let ps: Vec<DrawParams> = (0..400)
            .map(|i| DrawParams { alpha_mul: i as f32 / 400.0, object_fog: i % 3 == 0, gouraud: (i % 7 == 0).then_some([0.5, i as f32, 1.0]), ..Default::default() })
            .collect();
        let ids: Vec<u16> = ps.iter().map(|p| r.intern_params(p)).collect();
        for (k, p) in ps.iter().enumerate().rev() {
            let id = r.intern_params(p);
            if ids[k] < PID_NONE {
                assert_eq!(id, ids[k]);
            }
            assert_eq!(params_key(&r.ptab[id as usize]), params_key(p));
        }
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 400);
    }

    /// The flush resolves each visible part's texture through the registry as it is at the
    /// flush (remapped and unmapped CRCs, overrides).
    #[test]
    fn flush_resolves_textures() {
        let bytes = super::cull_tests::terrain();
        let f = File::parse(&bytes).unwrap();
        let mv = f.mesh(0).unwrap();
        let part_crc: Vec<u32> = mv.groups().flat_map(|g| core::iter::repeat_n(g.tex_crc, g.batch_count as usize)).collect();
        let mut view = ViewParams::pc_default(320.0, 240.0);
        view.view = camera_view([0.0, 900.0, -10.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let mut r = R3d::new();
        let ids = r.load_file(bytes.clone()).unwrap();
        for (k, (remap, ovr)) in [(None, None), (Some((8, Some(TexId(5)))), None), (Some((9, None)), None), (None, Some(TexId(3)))].into_iter().enumerate() {
            if let Some((crc, t)) = remap {
                r.map_texture(crc, t);
            }
            r.begin_frame(&view);
            r.draw_slot(List::Normal, ids[0], &IDENTITY, &DrawParams { override_tex: ovr, ..Default::default() }, 0, 1);
            r.run_queue();
            let vis: Vec<u16> = r.vis.clone();
            r.flush(|_| {});
            let d = &r.draw_scratch;
            assert_eq!(d.len(), 1);
            let texs = unsafe { core::slice::from_raw_parts(d[0].texs, d[0].n_groups as usize) };
            let want: Vec<u16> = vis
                .iter()
                .map(|&pi| match ovr {
                    Some(t) => t.0,
                    None => r.tex_by_crc.get(&part_crc[pi as usize]).copied().unwrap_or(0),
                })
                .collect();
            assert!(vis.len() > 10, "{}", vis.len());
            assert_eq!(texs, &want[..], "step {k}");
            if k == 1 {
                assert!(texs.contains(&5));
            }
            if k == 2 {
                assert!(texs.contains(&0));
            }
        }
    }

    /// After a reset to an empty mark (level unload) the per-level storage is gone and the next
    /// level draws exactly like a fresh renderer.
    #[test]
    fn reset_frees_level_state() {
        let bytes = super::cull_tests::terrain();
        let mut view = ViewParams::pc_default(320.0, 240.0);
        view.view = camera_view([0.0, 300.0, -700.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let m: Mat4 = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [10.0, 0.0, 20.0, 1.0]];
        let snap = |r: &R3d| -> Vec<(Vec<u16>, R3dDrawUniform)> {
            r.lists[List::Normal as usize]
                .iter()
                .map(|d| (r.vis[d.vis_first as usize..(d.vis_first + d.vis_count) as usize].to_vec(), unsafe { *d.uptr }))
                .collect()
        };
        let mut r = R3d::new();
        let base = r.mark();
        let ids = r.load_file(bytes.clone()).unwrap();
        for f in 0..2 {
            r.begin_frame(&view);
            r.queue_raw(List::Normal, 5, 0, &[m], 1.0, None, None, (false, false, false), 40, 9, 0, 0);
            r.run_queue_with(&mut |_| Some(ids[0]));
            r.flush(|_| {});
            let _ = f;
        }
        r.reset_to(base);
        assert!(r.slots.is_empty() && r.rings.is_empty() && r.slot_model.is_empty() && r.mhot.is_empty() && r.spheres.is_empty());
        let ids = r.load_file(bytes.clone()).unwrap();
        let mut fresh = R3d::new();
        let fids = fresh.load_file(bytes).unwrap();
        for (x, id) in [(&mut r, ids[0]), (&mut fresh, fids[0])] {
            x.begin_frame(&view);
            x.queue_raw(List::Normal, 5, 0, &[m], 1.0, None, None, (false, false, false), 40, 9, 0, 0);
            x.run_queue_with(&mut |_| Some(id));
        }
        assert_eq!(snap(&r), snap(&fresh));
        assert!(!snap(&r).is_empty());
    }

    /// Raw draws (the game's model/object, resolved after the draw loop) give what draws
    /// queued with their meshes give; unresolved and skipped ones are not drawn.
    #[test]
    fn raw_draws_match_queued_ones() {
        let bytes = super::cull_tests::terrain();
        let mut view = ViewParams::pc_default(320.0, 240.0);
        view.view = camera_view([0.0, 300.0, -700.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let m1: Mat4 = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [10.0, 0.0, 20.0, 1.0]];
        let m2: Mat4 = [[0.0, 0.0, -1.0, 0.0], [0.0, 1.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], [-40.0, 5.0, 60.0, 1.0]];
        let snap = |r: &R3d| -> Vec<(Vec<u16>, R3dDrawUniform, u16)> {
            r.lists[List::Normal as usize]
                .iter()
                .chain(&r.lists[List::Xlu as usize])
                .map(|d| {
                    let u = if d.uptr.is_null() { r.uni[r.uni_frame][d.u as usize] } else { unsafe { *d.uptr } };
                    (r.vis[d.vis_first as usize..(d.vis_first + d.vis_count) as usize].to_vec(), u, d.tex_override)
                })
                .collect()
        };
        let mut a = R3d::new();
        let ia = a.load_file(bytes.clone()).unwrap();
        let mut b = R3d::new();
        let ib = b.load_file(bytes).unwrap();
        for f in 0..3u32 {
            a.begin_frame(&view);
            b.begin_frame(&view);
            let m = [&m1, &m2][f as usize % 2];
            let p = DrawParams { alpha_mul: 0.5, object_fog: true, override_tex: Some(TexId(4)), ..Default::default() };
            a.queue(List::Normal, ia[0], &[*m], &p, false, 0, 7 + f, 0, 0);
            a.queue(List::Xlu, ia[0], &[m1], &DrawParams::default(), false, 1, 3, 0, 0);
            b.queue_raw(List::Normal, 0x1234, 2, &[*m], 0.5, None, Some(TexId(4)), (false, true, false), 0, 7 + f, 0, 0);
            b.queue_raw(List::Xlu, 0x1234, 3, &[m1], 1.0, None, None, (false, false, false), 1, 3, 0, 0);
            b.queue_raw(List::Normal, 0x9999, 0, &[m1], 1.0, None, None, (false, false, false), 2, 3, 0, 0);
            a.run_queue();
            let mut asked = Vec::new();
            b.run_queue_with(&mut |d| {
                asked.push((d.model, d.object, d.list));
                (d.model == 0x1234).then_some(ib[0])
            });
            // The slot memo: asked in the first frame only (the skipped draw every time).
            let mut want = alloc::vec![(0x9999, 0, List::Normal)];
            if f == 0 {
                want = alloc::vec![(0x1234, 2, List::Normal), (0x1234, 3, List::Xlu), (0x9999, 0, List::Normal)];
            }
            assert_eq!(asked, want, "frame {f}");
            assert_eq!(b.drawn, 2);
            assert_eq!(snap(&a), snap(&b), "frame {f}");
            a.flush(|_| {});
            b.flush(|_| {});
        }
    }

    /// A slot draw whose view (or model) changed queues exactly what a fresh draw would.
    #[test]
    fn view_only_draws_match_fresh_ones() {
        let bytes = super::cull_tests::terrain();
        let snapshot = |r: &R3d| -> (Vec<u16>, Vec<R3dDrawUniform>) {
            let us =
                r.lists[List::Normal as usize].iter().map(|d| if d.uptr.is_null() { r.uni[r.uni_frame][d.u as usize] } else { unsafe { *d.uptr } }).collect();
            (r.vis.clone(), us)
        };
        let mut r = R3d::new();
        let ids = r.load_file(bytes.clone()).unwrap();
        let cams = [
            ([0.0, 300.0, -700.0], [0.0, 0.0, 0.0]),
            ([30.0, 290.0, -650.0], [10.0, 0.0, 20.0]),
            ([500.0, 120.0, 500.0], [-200.0, 0.0, 0.0]),
            ([-50.0, 211.0, 1181.0], [-50.0, 0.0, 1025.0]),
            ([0.0, 60.0, 0.0], [300.0, 0.0, 300.0]),
        ];
        let models: [Mat4; 2] = [
            [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [10.0, 0.0, 20.0, 1.0]],
            [[0.0, 0.0, -1.0, 0.0], [0.0, 1.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], [-40.0, 5.0, 60.0, 1.0]],
        ];
        let mut views = 0;
        for (k, (src, at)) in cams.iter().cycle().take(12).enumerate() {
            let mut view = ViewParams::pc_default(320.0, 240.0);
            view.view = camera_view(*src, *at, [0.0, 1.0, 0.0]);
            // Model (stamp) changes every fourth frame.
            let (model, stamp) = (&models[k / 4 % 2], 1 + (k / 4) as u32);
            let p = DrawParams { alpha_mul: 0.5, ..Default::default() };
            let mut fresh = R3d::new();
            let fids = fresh.load_file(bytes.clone()).unwrap();
            fresh.begin_frame(&view);
            fresh.draw(List::Normal, fids[0], model, &p);
            fresh.run_queue();
            r.begin_frame(&view);
            r.draw_slot(List::Normal, ids[0], model, &p, 3, stamp);
            r.run_queue();
            assert_eq!(snapshot(&r), snapshot(&fresh), "frame {k}");
            views += r.view_hits;
            r.flush(|_| {});
            fresh.flush(|_| {});
            assert_eq!(r.sent_tris, fresh.sent_tris);
        }
        assert!(views >= 6, "{views} view-only draws");
    }

    #[test]
    fn colour_uniforms() {
        let u = draw_uniform(&IDENTITY, &DrawParams::default());
        // Shader: out = (in<<7) * mul / 32768 + add, then >> 7.
        let shade = |c: u8, mul: i16, add: i16| (((((c as i32) << 7) * mul as i32 * 2) >> 16) + add as i32).clamp(0, 32767) >> 7;
        for c in [0u8, 1, 100, 254, 255] {
            assert_eq!(shade(c, u.cmul[0], u.cadd[0]), c as i32);
        }
        let g = draw_uniform(&IDENTITY, &DrawParams { gouraud: Some([0.5, 1.0, 0.0]), alpha_mul: 0.5, ..Default::default() });
        assert_eq!(shade(200, g.cmul[0], g.cadd[0]), 128);
        assert_eq!(shade(200, g.cmul[1], g.cadd[1]), 255);
        assert_eq!(shade(200, g.cmul[2], g.cadd[2]), 0);
        assert_eq!(shade(255, g.cmul[3], g.cadd[3]), 128); // PC: (0.5 * 255 + 0.5) as u8
    }

    #[test]
    fn list_states_match_renderer_build() {
        let n = List::Normal.state(true, false);
        assert_eq!((n.blend, n.alpha_ref), (ffi::BLEND_SEMI, 101));
        assert_eq!(n.flags & (ffi::ZTEST | ffi::ZWRITE | ffi::WRAP), ffi::ZTEST | ffi::ZWRITE);
        let x = List::Xlu.state(false, false);
        assert_eq!(x.flags, ffi::ZTEST | ffi::WRAP);
        assert_eq!(List::Overlay.state(false, true).flags, 0);
        assert_eq!(List::Additive.state(false, false).blend, ffi::BLEND_ADD);
        assert_eq!(List::Normal.state(false, true).flags & ffi::FOG, ffi::FOG);
    }
}

#[cfg(test)]
mod cull_tests {
    use super::*;
    use f2_n64fmt::encode::{encode_mesh, FaceIn, FileBuilder, MeshIn};
    use f2_n64fmt::Cmd;

    /// A bumpy 24x24 grid (1152 faces) spanning 1200 x 1200 units.
    pub(super) fn terrain() -> Vec<u8> {
        let n = 24;
        let mut pos = Vec::new();
        for y in 0..=n {
            for x in 0..=n {
                let h = ((x * 7 + y * 13) % 11) as f32 * 6.0;
                pos.push([x as f32 * 50.0 - 600.0, h, y as f32 * 50.0 - 600.0]);
            }
        }
        let id = |x: usize, y: usize| (y * (n + 1) + x) as u16;
        let mut faces = Vec::new();
        for y in 0..n {
            for x in 0..n {
                let c = [[255u8; 4]; 3];
                for t in [[id(x, y), id(x + 1, y + 1), id(x + 1, y)], [id(x, y), id(x, y + 1), id(x + 1, y + 1)]] {
                    faces.push(FaceIn { v: t, uv: [[0, 0], [1026, 0], [0, 1026]], rgba: c, tex_crc: 7 + ((x / 6) as u32) });
                }
            }
        }
        let mut fb = FileBuilder::new();
        fb.meshes.push(encode_mesh(&MeshIn { id: 1, positions: &pos, faces: &faces, bone_of: None, ordered: false, wrap: false, world_shift: None }));
        fb.finish()
    }

    /// Every face of every culled batch is one MDX rejects (checked with the shader model).
    #[test]
    fn culled_batches_are_fully_rejected() {
        let bytes = terrain();
        let f = File::parse(&bytes).unwrap();
        let mv = f.mesh(0).unwrap();
        let batches: Vec<_> = mv.groups().flat_map(|g| mv.group_batches(&g).collect::<Vec<_>>()).collect();
        let mut r = R3d::new();
        let ids = r.load_file(bytes.clone()).unwrap();
        assert_eq!(r.meshes[0].parts.len(), batches.len());
        let (mut culled, mut kept) = (0, 0);
        let cams = [
            ([0.0, 300.0, -700.0], [0.0, 0.0, 0.0], 1200.0),
            ([500.0, 120.0, 500.0], [-200.0, 0.0, 0.0], 700.0),
            ([-50.0, 211.0, 1181.0], [-50.0, 0.0, 1025.0], 700.0),
            ([0.0, 900.0, 0.0], [10.0, 0.0, 60.0], 600.0),
            ([0.0, 60.0, 0.0], [300.0, 0.0, 300.0], 400.0),
        ];
        for (k, (src, at, far)) in cams.into_iter().enumerate() {
            let mut view = ViewParams::pc_default(320.0, 240.0);
            view.view = camera_view(src, at, [0.0, 1.0, 0.0]);
            view.far = far;
            r.begin_frame(&view);
            let model = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [k as f32 * 20.0, 0.0, 0.0, 1.0]];
            r.draw(List::Normal, ids[0], &model, &DrawParams::default());
            r.run_queue();
            let vis: Vec<u16> = r.vis.clone();
            let u = draw_uniform(&model_scaled(&model, mv.entry.pos_scale()), &DrawParams::default());
            let fr = frame_uniform_vp(&view, false);
            for (i, b) in batches.iter().enumerate() {
                if vis.contains(&(i as u16)) {
                    kept += 1;
                    continue;
                }
                culled += 1;
                let mut slots = [None; 49];
                for w in mv.batch_cmd_bytes(b).chunks(4) {
                    match Cmd::decode(u32::from_be_bytes([w[0], w[1], w[2], w[3]])).unwrap() {
                        Cmd::Load { first, count, slot } => {
                            for j in 0..count as usize {
                                slots[slot as usize + j] = Some(emulate_vertex(&u, &fr, view.guard as i32, mv.vertex(first as usize + j).pos));
                            }
                        }
                        Cmd::Tri(t) => {
                            let v = t.map(|s| slots[s as usize].unwrap());
                            let depth = v.iter().any(|x| x.2 & 0x24 != 0);
                            let screen = v.iter().fold(0x3F, |a, x| a & !x.3) & 0x3F != 0;
                            assert!(depth || screen, "camera {k}: culled batch {i} has a visible face {v:?}");
                        }
                        Cmd::Bone(_) => {}
                    }
                }
            }
            r.flush(|_| {});
        }
        assert!(culled > 50 && kept > 20, "culled {culled}, kept {kept}");
    }
}
