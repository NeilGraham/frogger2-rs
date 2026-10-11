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
    group: u16,
    tris: u16,
    /// Bounding sphere in stored position units (radius < 0: never culled).
    center: [f32; 3],
    radius: f32,
}

struct MeshRes {
    id: u32,
    parts: Vec<Part>,
    group_crc: Vec<u32>,
    /// `group_crc` resolved through the texture registry at `tex_gen`.
    group_tex: Vec<u16>,
    tex_gen: u32,
    pos_scale: f32,
    skinned: bool,
    tris: u32,
}

struct DrawRec {
    /// Index of the draw's uniform in the current frame's arena.
    u: u32,
    mesh: u32,
    tex_override: u16,
    /// Index of the first bone uniform in `R3d::bones` (skinned), else `u32::MAX`.
    bones: u32,
    /// Visible parts: `R3d::vis[vis_first..vis_first + vis_count]` (indices into `parts`).
    vis_first: u32,
    vis_count: u32,
}

/// Per-frame constants of the bounding-sphere test (see [`R3d::draw`]).
#[derive(Clone, Copy, Debug, Default)]
struct CullPlanes {
    /// Slopes of the four screen-edge planes through the eye (left, right, top, bottom),
    /// moved one pixel outwards, and `sqrt(1 + a^2)`.
    a: [f32; 4],
    n: [f32; 4],
}

/// The renderer (one per program).
pub struct R3d {
    /// F2NM files (meshes and textures point into them).
    files: Vec<Vec<u8>>,
    tex_by_crc: BTreeMap<u32, u16>,
    /// Bumped whenever `tex_by_crc` changes (meshes re-resolve their group textures).
    tex_gen: u32,
    /// Per-frame uniform arenas, cycled over 3 frames (the RSP reads a frame's uniforms by
    /// DMA while the CPU already queues the next one; triple-buffered display = at most 2
    /// frames in flight).
    uni: [Vec<R3dDrawUniform>; 3],
    uni_frame: usize,
    meshes: Vec<MeshRes>,
    lists: [Vec<DrawRec>; NUM_LISTS],
    bones: Vec<R3dDrawUniform>,
    vis: Vec<u16>,
    handle_scratch: Vec<u16>,
    tex_scratch: Vec<u16>,
    draw_scratch: Vec<R3dDraw>,
    view: ViewParams,
    frame: R3dFrame,
    cull: CullPlanes,
    /// Triangles of the meshes queued this frame.
    pub queued_tris: u32,
    /// Triangles sent to the RSP after bounding-sphere culling (the RSP still rejects
    /// faces exactly like MDX).
    pub sent_tris: u32,
}

/// f32 → s15.16 split into (integer, fraction) halves (saturating).
pub fn to_fixed(v: f32) -> (i16, u16) {
    let s = v * 65536.0;
    // Clamp then convert without Rust's saturating-cast checks (cheaper on the VR4300).
    let x = if s >= 2_147_483_520.0 {
        i32::MAX
    } else if s > -2_147_483_520.0 {
        // SAFETY: finite and within i32 range here.
        unsafe { s.to_int_unchecked::<i32>() }
    } else {
        i32::MIN
    };
    ((x >> 16) as i16, x as u16)
}

/// The folded clip-space matrix of a draw: `diag(s, s, s, 1) x model x view x P` with
/// `P`: X = vx, Y = vy, W = vz + DIST, Z = a*W + b (z planes = MDX near/far).
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

/// Packs a clip matrix and draw colour parameters into the `F2_DRAW` uniform.
pub fn draw_uniform(m: &Mat4, p: &DrawParams) -> R3dDrawUniform {
    let mut u = R3dDrawUniform::default();
    for r in 0..4 {
        for c in 0..4 {
            let (i, f) = to_fixed(m[r][c]);
            u.mtx_i[r * 4 + c] = i;
            u.mtx_f[r * 4 + c] = f;
        }
    }
    // c = c_in * mul + add in the shader's <<7 units; +0x7F bias rounds 255 back to 255.
    let am = (p.alpha_mul.clamp(0.0, 1.0) * 32767.0) as i16;
    match p.gouraud {
        None => {
            u.cmul = [0x7FFF, 0x7FFF, 0x7FFF, am];
            u.cadd = [0x7F; 4];
        }
        Some(g) => {
            let c = |v: f32| (((v.clamp(0.0, 1.0) * 255.0 + 0.5) as i16) << 7) | 0x7F;
            u.cmul = [0, 0, 0, am];
            u.cadd = [c(g[0]), c(g[1]), c(g[2]), 0x7F];
        }
    }
    u.fogm = [0xFF, 0, 0, 0];
    u
}

/// Sets a draw uniform's fog fields (per vertex, per object from the origin's depth `w0`
/// = `W` of the model origin, or none).
fn draw_fog(u: &mut R3dDrawUniform, view: &ViewParams, p: &DrawParams, w0: f32) {
    match view.fog {
        Some(f) if !p.no_clip => {
            if p.object_fog {
                // Unfogged fraction (PC `FOG(x)`), RDPQ_FOG_STANDARD's shade alpha.
                let a = ((f.end - (w0 - DIST)) / (f.end - f.start).max(1.0)).clamp(0.0, 1.0);
                u.fogc = [0, (a * 32767.0) as i16, 0, 0];
                u.fogm = [0x77, 0, 0, 0];
            } else {
                u.fogm = [0xFF, 0x88, 0, 0];
            }
        }
        _ => u.fogm = [0xFF, 0, 0, 0],
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
        clip_text: 0,
        clip_size: 0,
        guard: v.guard,
        fog_rgba: v.fog.map_or(0, |f| f.rgba | 0xFF),
    }
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
        R3d {
            files: Vec::new(),
            tex_by_crc: BTreeMap::new(),
            tex_gen: 1,
            uni: [Vec::with_capacity(512), Vec::with_capacity(512), Vec::with_capacity(512)],
            uni_frame: 0,
            meshes: Vec::new(),
            lists: Default::default(),
            bones: Vec::with_capacity(256),
            vis: Vec::with_capacity(2048),
            handle_scratch: Vec::with_capacity(2048),
            tex_scratch: Vec::with_capacity(2048),
            draw_scratch: Vec::with_capacity(256),
            view: ViewParams::pc_default(320.0, 240.0),
            frame: R3dFrame::default(),
            cull: CullPlanes::default(),
            queued_tris: 0,
            sent_tris: 0,
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
            self.tex_by_crc.insert(t.crc, h as u16);
        }
        self.tex_gen += 1;
        for i in 0..f.n_meshes() {
            let m = f.mesh(i).map_err(LoadError::Format)?;
            let res = self.register_mesh(&m)?;
            ids.push(MeshId(self.meshes.len() as u32));
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
            group_crc: Vec::new(),
            group_tex: Vec::new(),
            tex_gen: 0,
            pos_scale: m.entry.pos_scale(),
            skinned,
            tris: m.entry.n_tris,
        };
        let add = |cmds: &[u8], group: usize, tris: u16, center: [f32; 3], radius: f32| -> Result<Part, LoadError> {
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
            Ok(Part { handle: h as u16, group: group as u16, tris, center, radius })
        };
        for (gi, g) in m.groups().enumerate() {
            res.group_crc.push(g.tex_crc);
            if skinned || g.batch_count == 0 {
                res.parts.push(add(m.group_cmd_bytes(&g), gi, g.n_tris, [0.0; 3], -1.0)?);
            } else {
                for b in m.group_batches(&g) {
                    let c = b.center.map(|v| v as f32);
                    res.parts.push(add(m.batch_cmd_bytes(&b), gi, b.n_tris, c, b.radius as f32)?);
                }
            }
        }
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
        self.meshes.truncate(m.meshes);
        self.files.truncate(m.files);
        self.tex_by_crc.retain(|_, h| (*h as u32) < m.tex);
        self.tex_gen += 1;
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
        let changed = match tex {
            Some(t) => self.tex_by_crc.insert(crc, t.0) != Some(t.0),
            None => self.tex_by_crc.remove(&crc).is_some(),
        };
        if changed {
            self.tex_gen += 1;
        }
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
        self.tex_by_crc.insert(crc, tex.0);
        self.tex_gen += 1;
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
        self.view = *view;
        self.frame = frame_uniform(view);
        self.uni_frame = (self.uni_frame + 1) % 3;
        self.uni[self.uni_frame].clear();
        self.queued_tris = 0;
        self.sent_tris = 0;
        let [x, y, w, h] = view.viewport;
        let k = view.fov * view.proj_scale();
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        let a = [(cx - x - 1.0) / k, (x + w - 1.0 - cx - 1.0) / k, (cy - y - 1.0) / k, (y + h - 1.0 - cy - 1.0) / k];
        self.cull = CullPlanes { a, n: a.map(|a| fast_sqrt(1.0 + a * a) * 1.0001) };
    }

    /// Culls a rigid mesh's batches against the screen and near/far with their bounding
    /// spheres, pushing the visible part indices to `vis`. Conservative: a culled batch is
    /// one whose every face MDX would reject (all vertices off-screen past one edge, or
    /// beyond near/far), so the image is unchanged.
    fn cull_parts(&mut self, mesh: usize, m: &Mat4, no_clip: bool) -> u32 {
        let res = &self.meshes[mesh];
        let first = self.vis.len();
        // Scale bound of the folded matrix (X, Y, W columns: view space, rotation x scale).
        let rn = |r: usize| m[r][0] * m[r][0] + m[r][1] * m[r][1] + m[r][3] * m[r][3];
        let s = fast_sqrt(rn(0).max(rn(1)).max(rn(2))) * 1.0001;
        let (near, far) = (self.view.near, self.view.far);
        let CullPlanes { a, n } = self.cull;
        let mut sent = 0;
        for (i, p) in res.parts.iter().enumerate() {
            if p.radius >= 0.0 {
                let c = p.center;
                let x = c[0] * m[0][0] + c[1] * m[1][0] + c[2] * m[2][0] + m[3][0];
                let y = c[0] * m[0][1] + c[1] * m[1][1] + c[2] * m[2][1] + m[3][1];
                let w = c[0] * m[0][3] + c[1] * m[1][3] + c[2] * m[2][3] + m[3][3];
                let r = p.radius * s;
                if w + r <= near
                    || (!no_clip && w - r >= far)
                    || x - a[0] * w >= r * n[0]
                    || -x - a[1] * w >= r * n[1]
                    || y - a[2] * w >= r * n[2]
                    || -y - a[3] * w >= r * n[3]
                {
                    continue;
                }
            }
            self.vis.push(i as u16);
            sent += p.tris as u32;
        }
        self.sent_tris += sent;
        (self.vis.len() - first) as u32
    }

    pub fn view(&self) -> &ViewParams {
        &self.view
    }

    /// Queues a rigid mesh draw with its model → world matrix.
    pub fn draw(&mut self, list: List, mesh: MeshId, model: &Mat4, p: &DrawParams) {
        let Some(res) = self.meshes.get(mesh.0 as usize) else { return };
        let m = clip_matrix(model, &self.view, res.pos_scale, p.no_clip);
        self.queued_tris += res.tris;
        let vis_first = self.vis.len() as u32;
        let vis_count = self.cull_parts(mesh.0 as usize, &m, p.no_clip);
        if vis_count == 0 {
            return;
        }
        let mut u = draw_uniform(&m, p);
        draw_fog(&mut u, &self.view, p, m[3][3]);
        let arena = &mut self.uni[self.uni_frame];
        arena.push(u);
        let rec = DrawRec { u: arena.len() as u32 - 1, mesh: mesh.0, tex_override: p.override_tex.map_or(0, |t| t.0), bones: u32::MAX, vis_first, vis_count };
        self.lists[list as usize].push(rec);
    }

    /// Queues a skinned mesh draw: one model → world matrix per bone (pre-order object
    /// index, as `xform_actor` returns them).
    pub fn draw_skinned(&mut self, list: List, mesh: MeshId, bones: &[Mat4], p: &DrawParams) {
        let Some(res) = self.meshes.get(mesh.0 as usize) else { return };
        let first = self.bones.len() as u32;
        for b in bones {
            let m = clip_matrix(b, &self.view, res.pos_scale, p.no_clip);
            let mut u = draw_uniform(&m, p);
            let w0 = bones.first().map_or(0.0, |b0| clip_matrix(b0, &self.view, 1.0, p.no_clip)[3][3]);
            draw_fog(&mut u, &self.view, p, w0);
            self.bones.push(u);
        }
        self.queued_tris += res.tris;
        self.sent_tris += res.tris;
        let vis_first = self.vis.len() as u32;
        let n = res.parts.len();
        self.vis.extend(0..n as u16);
        let arena = &mut self.uni[self.uni_frame];
        arena.push(self.bones.get(first as usize).copied().unwrap_or_default());
        let rec =
            DrawRec { u: arena.len() as u32 - 1, mesh: mesh.0, tex_override: p.override_tex.map_or(0, |t| t.0), bones: first, vis_first, vis_count: n as u32 };
        self.lists[list as usize].push(rec);
    }

    /// Draws all queued lists in `DrawAllFrames` order; `after(list)` runs after each list
    /// (for CPU-side batches that belong between lists: sprites in `Late`, HUD in
    /// `Overlay`, ...). Clears the lists.
    pub fn flush(&mut self, mut after: impl FnMut(List)) {
        #[cfg(target_vendor = "nintendo64")]
        unsafe {
            ffi::n64_r3d_begin(&self.frame)
        };
        for list in LIST_ORDER {
            self.flush_list(list);
            after(list);
        }
        self.bones.clear();
        self.vis.clear();
    }

    fn flush_list(&mut self, list: List) {
        let recs = &self.lists[list as usize];
        if recs.is_empty() {
            return;
        }
        // Group textures through the registry (re-resolved only after registry changes).
        for r in recs.iter() {
            let res = &mut self.meshes[r.mesh as usize];
            if res.tex_gen != self.tex_gen {
                res.group_tex = res.group_crc.iter().map(|c| self.tex_by_crc.get(c).copied().unwrap_or(0)).collect();
                res.tex_gen = self.tex_gen;
            }
        }
        // Resolve handles and textures first (the draw structs point into the scratch arrays).
        self.handle_scratch.clear();
        self.tex_scratch.clear();
        for r in recs {
            let res = &self.meshes[r.mesh as usize];
            for &pi in &self.vis[r.vis_first as usize..(r.vis_first + r.vis_count) as usize] {
                let p = &res.parts[pi as usize];
                self.handle_scratch.push(p.handle);
                self.tex_scratch.push(if r.tex_override != 0 { r.tex_override } else { res.group_tex[p.group as usize] });
            }
        }
        self.draw_scratch.clear();
        let mut off = 0;
        for r in recs {
            let res = &self.meshes[r.mesh as usize];
            let n = r.vis_count as usize;
            self.draw_scratch.push(R3dDraw {
                u: &self.uni[self.uni_frame][r.u as usize],
                groups: self.handle_scratch[off..].as_ptr(),
                texs: self.tex_scratch[off..].as_ptr(),
                n_groups: n as u32,
                bones: if r.bones == u32::MAX || !res.skinned { core::ptr::null() } else { self.bones[r.bones as usize..].as_ptr() },
            });
            off += n;
        }
        let state = list.state(self.view.bilinear, self.view.fog.is_some());
        #[cfg(target_vendor = "nintendo64")]
        unsafe {
            ffi::n64_r3d_draw_list(&state, self.draw_scratch.as_ptr(), self.draw_scratch.len() as u32)
        };
        #[cfg(not(target_vendor = "nintendo64"))]
        let _ = state;
        self.lists[list as usize].clear();
    }

    /// Drops everything queued this frame without drawing it.
    pub fn discard(&mut self) {
        for l in &mut self.lists {
            l.clear();
        }
        self.bones.clear();
        self.vis.clear();
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
    let m = |r: usize, c: usize| ((u.mtx_i[r * 4 + c] as i64) << 16) | u.mtx_f[r * 4 + c] as i64;
    let sat = |v: i64| v.clamp(i32::MIN as i64, i32::MAX as i64);
    // Clip-space s15.16 (the RSP clamps the accumulator high half to s16).
    let cs: [i64; 4] = core::array::from_fn(|c| sat(pos[0] as i64 * m(0, c) + pos[1] as i64 * m(1, c) + pos[2] as i64 * m(2, c) + m(3, c)));
    let (x, y, z, w) = (cs[0], cs[1], cs[2], cs[3]);
    let gw = w * guard as i64;
    let mut clip = 0u8;
    if x <= -gw {
        clip |= 1;
    }
    if y <= -gw {
        clip |= 2;
    }
    if z <= -w {
        clip |= 4;
    }
    if x >= gw {
        clip |= 8;
    }
    if y >= gw {
        clip |= 16;
    }
    if z >= w {
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

    #[test]
    fn shader_model_matches_mdx_projection() {
        let mut view = ViewParams::pc_default(320.0, 240.0);
        view.view = look_at([30.0, 400.0, -500.0], [0.0, 0.0, 100.0]);
        let fr = frame_uniform(&view);
        // A mesh at some world position with a rotation, stored with pos_shift 3.
        let (s, c) = (0.3f32.sin(), 0.3f32.cos());
        let model: Mat4 = [[c, 0.0, -s, 0.0], [0.0, 1.0, 0.0, 0.0], [s, 0.0, c, 0.0], [120.0, -5.0, 60.0, 1.0]];
        let shift = 3;
        let m = clip_matrix(&model, &view, 1.0 / (1 << shift) as f32, false);
        let u = draw_uniform(&m, &DrawParams::default());
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
                    assert!((z as f32 - ez).abs() <= 2.0, "z {z} vs {ez}");
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
    fn terrain() -> Vec<u8> {
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
        fb.meshes.push(encode_mesh(&MeshIn { id: 1, positions: &pos, faces: &faces, bone_of: None, ordered: false, wrap: false }));
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
            let vis: Vec<u16> = r.vis.clone();
            let u = draw_uniform(&clip_matrix(&model, &view, mv.entry.pos_scale(), false), &DrawParams::default());
            let fr = frame_uniform(&view);
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
