//! The MDX transform-and-light poly pipeline on the CPU.
//!
//! Like the original (`mdxRender.cpp` + `mdxPoly.cpp`), every model vertex is
//! transformed and projected to screen space here, faces are rejected the same
//! way (near/far, off-screen, back-face), and the resulting screen-space
//! triangles are pushed into *frames* (`MA_FRAME_*`) that are drawn in
//! `DrawAllFrames` order. The GPU only rasterises pre-transformed vertices.

use crate::prelude::*;

use crate::anim::{for_each_object, xform_actor, xform_actor_cached, Actor};
#[cfg(not(feature = "std"))]
use crate::fmath::FloatExt;
use crate::math::*;
use crate::textures::{TexHandle, TextureRegistry};
use f2_formats::objbank::{Mesh, Model, Object};

pub const OBJECT_FLAGS_XLU: i16 = 1 << 1;
pub const OBJECT_FLAGS_SHEEN: i16 = 1 << 4;
pub const OBJECT_FLAGS_MODGE: i16 = 1 << 5;
pub const OBJECT_FLAGS_ADDITIVE: i16 = 1 << 6;
pub const OBJECT_FLAGS_WAVE: i16 = 1 << 7;
pub const OBJECT_FLAGS_PHONG: i16 = 1 << 8;
pub const OBJECT_FLAGS_FADEOUT: i16 = 1 << 9;
pub const OBJECT_FLAGS_CARTOON: i16 = 1 << 10;
pub const OBJECT_FLAGS_FLATSHADOW: i16 = 1 << 12;
pub const OBJECT_FLAGS_SUBDIVIDED: i16 = 1 << 13;
pub const OBJECT_FLAGS_GLOW: i16 = 1 << 14;

/// `DIST` (mdxRender.cpp:63).
pub const DIST: f32 = -10.0;
/// UV scale of `faceTC` (`RestoreObjectPointers`, mdxObject.cpp).
pub const UV_SCALE: f32 = 0.000975;
/// Screen z scale (`sz = vz * 0.00025`, `PCRenderObject`).
pub const SZ_SCALE: f32 = 0.00025;

/// `MA_FRAME_*` (mdxPoly.h:34). Frames are drawn in `DrawAllFrames` order:
/// Normal, Wrap, Xlu, Additive, then the late batches, then Overlay. `Glow` is
/// collected but — as on retail PC — never drawn (halos were disabled).
///
/// `Late` / `LateAdditive` are the `MA_FRAME_NORMAL` / `MA_FRAME_ADDITIVE` batches that
/// `DrawLoop` (drawloop.cpp:262-292) fills *after* `DrawAllFrames` with `PrintSprites`
/// and `DrawSpecialFX` and flushes with `DrawBatchedPolys` under `xluZRS` (z-test, no
/// z-write), `xluSemiRS` / `xluAddRS`, `normalAlphaCmpRS` (alpha != 0) and clamped
/// addressing. They are drawn in submission order (retail `sortMode = 0`), after every
/// world/actor frame, so a sprite or effect blends over all scene geometry it is not
/// depth-occluded by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Frame {
    Normal = 0,
    Glow,
    Xlu,
    Additive,
    Overlay,
    Wrap,
    Late,
    LateAdditive,
}
pub const NUM_FRAMES: usize = 8;

/// A `D3DTLVERTEX`: screen-space position in pixels, `sz` in 0..1, `rhw`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TlVertex {
    pub sx: f32,
    pub sy: f32,
    pub sz: f32,
    pub rhw: f32,
    /// Diffuse colour RGBA 0..1.
    pub color: [f32; 4],
    pub uv: [f32; 2],
    /// Fog factor (specular alpha): 1 = unfogged.
    pub fog: f32,
    /// Alpha-test reference (0..1): fragments with `a <= ref` are discarded.
    /// Filled in by the renderer per frame.
    pub alpha_ref: f32,
}

/// One screen-space triangle with its texture (`None` = untextured, white).
#[derive(Clone, Copy, Debug)]
pub struct Tri {
    pub v: [TlVertex; 3],
    pub tex: Option<TexHandle>,
}

/// Linear fog parameters (`SetupFogParams`, mdxD3D.cpp:183).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fog {
    pub enabled: bool,
    /// View-space z where fog starts (`fogStart`).
    pub start: f32,
    /// View-space z where fog is total (`farClip` at setup time).
    pub end: f32,
    pub color: [f32; 3],
}

impl Default for Fog {
    fn default() -> Self {
        // mdxRender.cpp:69-70 defaults; retail calls SetupFogParams(..., enable = 0).
        Fog { enabled: false, start: 400.0, end: 1200.0, color: [0.0; 3] }
    }
}

impl Fog {
    /// `FOG(x)` clamped to 0..1 (mdxRender.cpp:73).
    pub fn factor(&self, vz: f32) -> f32 {
        let range = 1.0 / (self.end - self.start);
        (1.0 - (vz - self.start) * range).clamp(0.0, 1.0)
    }
}

/// A camera as the game sets it (`CalcViewMatrix`, drawloop.cpp:65), in world units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// Camera position (`currCamSource`).
    pub source: Vec3,
    /// Look-at point (`currCamTarget`).
    pub target: Vec3,
    /// Up vector (`camVect`).
    pub up: Vec3,
}

impl Camera {
    pub fn view_matrix(&self) -> Mat4 {
        // The game passes target as "eye" and source as "at".
        gu_look_at(self.target, self.source, self.up)
    }
}

/// `horizClip` / `vertClip` (mdxRender.cpp): `XfmPoint`'s camera-space x/y rejection.
pub const HORIZ_CLIP: f32 = 3000.0;
pub const VERT_CLIP: f32 = 2500.0;

/// Projection state: `vMatrix`, `FOV`, clip planes, viewport (`mdxRender.cpp:46-120`).
#[derive(Clone, Debug)]
pub struct View {
    pub v_matrix: Mat4,
    /// `FOV` (450 on PC, `SetupRenderer`).
    pub fov: f32,
    pub width: f32,
    pub height: f32,
    /// `nearClip`.
    pub near_clip: f32,
    /// `farClip` (the game sets `fog.max * 0.1` each frame).
    pub far_clip: f32,
    pub horiz_clip: f32,
    pub vert_clip: f32,
    /// `noClipping`/`noClip`: ignore the far plane (used for the sky backdrop).
    pub no_clip: bool,
    /// Camera position/target kept for glow maths (`curEye`/`curAt`).
    pub camera: Option<Camera>,
    /// Enhanced: Hor+ projection. The original scales by `rXRes/640` only, so wider
    /// screens lose vertical view (Vert-); Hor+ keeps the 4:3 vertical FOV and widens
    /// the horizontal one (`min(w/640, h/480)`). Identical at 4:3.
    pub hor_plus: bool,
    /// 3D viewport `[x, y, w, h]` in pixels (`None` = the whole `width`×`height`); used for
    /// the ultrawide clamp (pillarboxed 3D view).
    pub viewport: Option<[f32; 4]>,
}

impl View {
    pub fn new(width: u32, height: u32) -> View {
        View {
            v_matrix: IDENTITY,
            fov: 450.0,
            width: width as f32,
            height: height as f32,
            near_clip: 10.0,
            far_clip: 1200.0,
            horiz_clip: HORIZ_CLIP,
            vert_clip: VERT_CLIP,
            no_clip: false,
            camera: None,
            hor_plus: false,
            viewport: None,
        }
    }

    pub fn set_camera(&mut self, cam: &Camera) {
        self.v_matrix = cam.view_matrix();
        self.camera = Some(*cam);
    }

    /// The 3D viewport `[x, y, w, h]`.
    pub fn vp(&self) -> [f32; 4] {
        self.viewport.unwrap_or([0.0, 0.0, self.width, self.height])
    }

    /// Enhanced Hor+: how far the visible 3D area reaches past the original 640×480
    /// frustum, as `[x, y]` multiples of its half extent (`1` = the 4:3 edge): above 1
    /// horizontally on wide screens, vertically on tall ones. Always `[1, 1]` without Hor+.
    ///
    /// The simulation culls against the 640×480 view (`ActorListDraw`, `CheckBoundingBox`,
    /// `IsPointVisible`); the host uses this to draw what that culling dropped but a wider
    /// screen shows.
    pub fn hor_plus_extent(&self) -> [f32; 2] {
        if !self.hor_plus {
            return [1.0, 1.0];
        }
        let [_, _, w, h] = self.vp();
        let s = self.proj_scale();
        if !s.is_finite() || s <= 0.0 {
            return [1.0, 1.0];
        }
        [(w / s / 640.0).max(1.0), (h / s / 480.0).max(1.0)]
    }

    /// Whether Hor+ shows more than the original 4:3 frustum (see [`Self::hor_plus_extent`]).
    pub fn widened(&self) -> bool {
        let [x, y] = self.hor_plus_extent();
        x > 1.0 || y > 1.0
    }

    /// `rXRes/640` of `InitOneOverTable`; with [`hor_plus`](Self::hor_plus), the
    /// smaller of the width and height ratios so the 4:3 FOV is kept on the narrow axis.
    pub fn proj_scale(&self) -> f32 {
        let [_, _, w, h] = self.vp();
        if self.hor_plus {
            (w / 640.0).min(h / 480.0)
        } else {
            w / 640.0
        }
    }

    /// `-FOV * oneOver[(long)vz + DIST]` with `oneOver[i] = (rXRes/640)/i`
    /// (`InitOneOverTable`, mdxRender.cpp:362). The depth is truncated to an integer.
    fn oozd(&self, vz: f32) -> f32 {
        let i = ((vz as i64) + DIST as i64).clamp(1, 65534) as f32;
        -self.fov * (self.proj_scale() / i)
    }

    /// Screen position of view-space `p` with depth reciprocal `o`.
    fn to_screen(&self, p: Vec3, o: f32) -> [f32; 3] {
        let [x, y, w, h] = self.vp();
        [x + w / 2.0 + p[0] * o, y + h / 2.0 + p[1] * o, p[2]]
    }

    /// Screen position of view-space `p` without the near/far rejection of
    /// [`project`](Self::project) (motion vectors: where a vertex was last frame).
    pub fn screen_unclipped(&self, p: Vec3) -> [f32; 2] {
        let s = self.to_screen(p, self.oozd(p[2]));
        [s[0], s[1]]
    }

    /// Projects a view-space point like `PCPrepareObject`: returns `(sx, sy, vz)`,
    /// or `None` where the original stores `vz = 0` (outside near/far).
    pub fn project(&self, p: Vec3) -> Option<[f32; 3]> {
        let z = p[2] + DIST;
        if z > self.near_clip && (z < self.far_clip || self.no_clip) {
            Some(self.to_screen(p, self.oozd(p[2])))
        } else {
            None
        }
    }

    /// Port of `XfmPoint` (mdxRender.cpp:331) for a world point.
    pub fn xfm_point(&self, p: Vec3) -> Option<[f32; 3]> {
        let v = mtx_xfm(&self.v_matrix, p);
        if v[0] > -self.horiz_clip && v[0] < self.horiz_clip && v[1] > -self.vert_clip && v[1] < self.vert_clip {
            self.project(v)
        } else {
            None
        }
    }
}

/// How translucent frames (Xlu, Additive) are ordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum XluSort {
    /// Back-to-front by triangle depth, like MDX's sorted path (`DrawSortedPolys`,
    /// `MA_SORTBACKFRONT`). Enhanced only: retail never sets `sortMode`.
    BackToFront,
    /// Submission order (the D3D hardware path with `sortMode = 0`, mdxPoly.cpp:73; nothing
    /// in the game ever changes it), the retail behaviour.
    #[default]
    Submission,
}

/// Rendering options; defaults are the authentic retail behaviour (the
/// enhanced options below are all off/1 by default, which keeps the original
/// single-pass render path bit-for-bit).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderOptions {
    /// Perspective-correct texturing. Retail submits `rhw = 1` with
    /// `TEXTUREPERSPECTIVE` off, i.e. affine mapping (false).
    pub perspective_correct: bool,
    /// Reject faces with no vertex strictly inside the screen on either axis
    /// (`BETWEEN` tests in `PCRenderObject`). Faithful but drops huge polys.
    pub screen_reject: bool,
    /// Bilinear filtering (`D3DTFN_LINEAR`, the default in mdxD3D.cpp:135).
    pub linear_filter: bool,
    pub xlu_sort: XluSort,
    pub fog: Fog,
    /// Enhanced: anisotropic filtering level (needs `linear_filter`); `0` = off (no
    /// mipmaps, as retail), `1` = trilinear mipmaps, `2..=16` = anisotropic.
    pub anisotropy: u16,
    /// Enhanced: Hor+ widescreen projection (see [`View::hor_plus`]).
    pub hor_plus: bool,
    /// Enhanced: widest 3D aspect ratio (e.g. `21.0 / 9.0`); wider screens are
    /// pillarboxed. `0` = no clamp.
    pub max_aspect: f32,
    /// Enhanced: re-anchor 4096×4096 overlays for wide screens (left/right thirds to the
    /// edges, centre and full-screen elements kept in a centred 4:3 area). Off = the
    /// original stretch over the whole screen.
    pub hud_anchor: bool,
    /// Enhanced: internal render resolution multiplier (supersampling > 1). `1` = native.
    pub render_scale: f32,
    /// Enhanced: MSAA sample count (1 or 4; other values are rounded to what every
    /// wgpu backend supports for the swapchain formats).
    pub msaa: u32,
    /// Enhanced: faces with a vertex outside the near / far range are clipped at the near
    /// plane and kept past the far one instead of being dropped whole (`PCPrepareObject`'s
    /// `vz = 0`), so big polygons near the camera no longer blink out at the screen edges.
    /// Off = the original (faces wholly past the far plane go either way).
    pub clip_faces: bool,
    /// Enhanced (temporal upscalers, frame generation): the opaque passes also record each
    /// vertex's screen position in the previous frame ([`Frames::motion`]). Off = no extra
    /// work and the retail path unchanged.
    pub motion_vectors: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions {
            perspective_correct: false,
            screen_reject: true,
            linear_filter: true,
            xlu_sort: XluSort::Submission,
            fog: Fog::default(),
            anisotropy: 0,
            hor_plus: false,
            max_aspect: 0.0,
            hud_anchor: false,
            render_scale: 1.0,
            msaa: 1,
            clip_faces: false,
            motion_vectors: false,
        }
    }
}

/// Per-frame triangle lists (`frameInfo[]`).
#[derive(Default)]
pub struct Frames {
    pub tris: [Vec<Tri>; NUM_FRAMES],
    /// `ProcessShadows`' polys. Retail draws them between its two `DrawAllFrames` flushes:
    /// after the landscape and list-0 actors, before list-1 actors (every actor with a
    /// shadow: frogs, babies, enemies), which therefore cover their own shadow.
    pub shadows: Vec<Tri>,
    /// Lengths of the `Normal` and `Wrap` lists when list-1 actors began
    /// ([`Frames::begin_late_actors`]): triangles after them are drawn after the shadows.
    pub late_split: Option<[usize; 2]>,
    /// Length of the `Xlu` list when the first shadow was queued (where the shadows sat
    /// in `Xlu` before they had their own list; [`crate::Renderer::frame_view`]).
    pub shadow_xlu_at: Option<usize>,
    /// Triangles dropped this frame because a list could not grow (out of memory: consoles).
    pub dropped: u32,
    /// Per frame list, the screen motion of each triangle's vertices (previous position minus
    /// current, internal pixels) for [`RenderOptions::motion_vectors`]: `motion[f][i]` belongs to
    /// `tris[f][i]`; triangles past the end (pushed without motion) have none (zero). Only the
    /// opaque lists use it (translucent ones are sorted and do not write motion vectors).
    pub motion: [Vec<[[f32; 2]; 3]>; NUM_FRAMES],
}

impl Frames {
    pub fn clear(&mut self) {
        for f in &mut self.tris {
            f.clear();
        }
        for m in &mut self.motion {
            m.clear();
        }
        self.shadows.clear();
        self.late_split = None;
        self.shadow_xlu_at = None;
        self.dropped = 0;
    }

    /// Queues a shadow poly (`ProcessShadows`, see [`Frames::shadows`]).
    pub fn push_shadow(&mut self, tri: Tri) {
        self.shadow_xlu_at.get_or_insert(self.tris[Frame::Xlu as usize].len());
        if self.shadows.len() == self.shadows.capacity() && self.shadows.try_reserve(1).is_err() {
            self.dropped += 1;
            return;
        }
        self.shadows.push(tri);
    }

    /// `ActorListDraw(1)` starts: opaque triangles from here on are drawn after the shadows.
    pub fn begin_late_actors(&mut self) {
        self.late_split = Some([self.tris[Frame::Normal as usize].len(), self.tris[Frame::Wrap as usize].len()]);
    }
    /// Queues a triangle. When the list cannot grow (out of memory, which only a console
    /// sees) it tries a small exact top-up, then drops the triangle instead of aborting.
    pub fn push(&mut self, frame: Frame, tri: Tri) {
        let v = &mut self.tris[frame as usize];
        if v.len() == v.capacity() && v.try_reserve(1).is_err() && v.try_reserve_exact(16).is_err() {
            self.dropped += 1;
            return;
        }
        v.push(tri);
    }

    /// [`push`](Self::push) with the vertices' screen motion ([`Frames::motion`]).
    pub fn push_moving(&mut self, frame: Frame, tri: Tri, mv: [[f32; 2]; 3]) {
        let n = self.tris[frame as usize].len();
        self.push(frame, tri);
        if self.tris[frame as usize].len() > n {
            let m = &mut self.motion[frame as usize];
            m.resize(n, [[0.0; 2]; 3]);
            m.push(mv);
        }
    }

    pub fn total(&self) -> usize {
        self.tris.iter().map(Vec::len).sum::<usize>() + self.shadows.len()
    }
}

/// Transformed vertex cache entry (`tV[]`): `(sx, sy, vz)` or `None` (`vz = 0`).
type Tv = Option<[f32; 3]>;

/// Port of `PCPrepareObject` (mdxRender.cpp:394).
fn prepare_object(view: &View, mesh: &Mesh, m: &Mat4, tv: &mut Vec<Tv>) {
    let f = mtx_cat(m, &view.v_matrix);
    tv.clear();
    tv.extend(mesh.vertices.iter().map(|&v| view.project(mtx_xfm(&f, v))));
}

/// Per-vertex `tMx`, `tMz`, `tMy` of [`calc_modge_values`].
pub(crate) type ModgeValues = Vec<[f32; 3]>;

/// Port of `PCCalcModgeValues` (mdxRender.cpp:430): per vertex the texture wobble `tMx`,
/// `tMz` (0..0.2) and the wave height `tMy` (0..10) at `timeInfo.frameCount` = `fc`. The
/// products are `double` (the `/` constants), each `sinf`/`cosf` takes a `float`.
fn calc_modge_values(mesh: &Mesh, fc: u32, out: &mut ModgeValues) {
    let t = fc as f64;
    // MSVC 6 inlines `sinf`/`cosf` as x87 `fsin`/`fcos` of the float argument.
    let sinf = |x: f64| ((x as f32) as f64).sin() as f32 as f64;
    let cosf = |x: f64| ((x as f32) as f64).cos() as f32 as f64;
    out.clear();
    out.extend(mesh.vertices.iter().map(|v| {
        let (x, z) = (v[0] as f64, v[2] as f64);
        let mx = (sinf(z * (1.5 / 35.0) + sinf(x * (1.3 / 49.0) + t * (2.0 / 56.0)) + t * (1.5 / 70.0)) + 1.0) * 0.1;
        let mz = (cosf(x * (1.5 / 49.0) + cosf(z * (1.3 / 29.0) + t * (2.0 / 66.0)) + t * (1.5 / 66.0)) + 1.0) * 0.1;
        let my = (sinf(z * (1.2 / 26.0) + cosf(x * (1.3 / 49.0) + t * (1.0 / 26.0)) + t * (1.0 / 26.0)) + 1.0) * 5.0;
        [mx as f32, mz as f32, my as f32]
    }));
}

/// Port of `PCPrepareModgyObject` (mdxRender.cpp:455): [`prepare_object`] with each vertex
/// raised by its wave height `tMy` (`OBJECT_FLAGS_WAVE`).
fn prepare_modgy_object(view: &View, mesh: &Mesh, m: &Mat4, mv: &ModgeValues, tv: &mut Vec<Tv>) {
    let f = mtx_cat(m, &view.v_matrix);
    tv.clear();
    tv.extend(mesh.vertices.iter().zip(mv).map(|(&v, w)| view.project(mtx_xfm(&f, [v[0], v[1] + w[2], v[2]]))));
}

/// The `PCRenderModgyObject` / `PCRenderModgyObject2` variant of [`render_mesh`]
/// (mdxRender.cpp:1573/1850, `OBJECT_FLAGS_MODGE`): texture coordinates wobble by
/// `0.4·tMx`, `0.4·tMz` and fog is per vertex; with `wave` (`PCRenderModgyObject`, water) the
/// vertex alpha is `50 + (tMx+tMz)·400` instead of the face colour's.
pub(crate) struct Modge<'a> {
    pub values: &'a ModgeValues,
    pub wave: bool,
}

/// Port of `PCPrepareSkinnedObject` (mdxRender.cpp:590): transform the root-mesh
/// vertices this bone moves.
fn prepare_skinned(view: &View, obj: &Object, mesh: &Mesh, m: &Mat4, tv: &mut [Tv]) {
    let f = mtx_cat(m, &view.v_matrix);
    for &j in &obj.effected_verts {
        let (Some(v), Some(slot)) = (mesh.vertices.get(j as usize), tv.get_mut(j as usize)) else { continue };
        let p = mtx_xfm(&f, *v);
        let z = p[2] + DIST;
        // The skinned path has no `noClip` override.
        *slot = if z > view.near_clip && z < view.far_clip { Some(view.to_screen(p, view.oozd(p[2]))) } else { None };
    }
}

/// Previous-frame screen positions of `mesh`'s vertices (motion vectors, not in the original):
/// model matrix `m` of the last frame (the current one for static geometry) through the
/// last frame's view, unclipped. `lift` = the wave heights `prepare_modgy_object` adds.
fn prepare_prev(prev: &View, mesh: &Mesh, m: &Mat4, lift: Option<&ModgeValues>, tvp: &mut Vec<[f32; 2]>) {
    let f = mtx_cat(m, &prev.v_matrix);
    tvp.clear();
    tvp.extend(mesh.vertices.iter().enumerate().map(|(i, &v)| {
        let dy = lift.and_then(|l| l.get(i)).map_or(0.0, |w| w[2]);
        prev.screen_unclipped(mtx_xfm(&f, [v[0], v[1] + dy, v[2]]))
    }));
}

/// [`prepare_prev`] for the root-mesh vertices one bone of a skin moves.
fn prepare_skinned_prev(prev: &View, obj: &Object, mesh: &Mesh, m: &Mat4, tvp: &mut [[f32; 2]]) {
    let f = mtx_cat(m, &prev.v_matrix);
    for &j in &obj.effected_verts {
        let (Some(v), Some(slot)) = (mesh.vertices.get(j as usize), tvp.get_mut(j as usize)) else { continue };
        *slot = prev.screen_unclipped(mtx_xfm(&f, *v));
    }
}

/// Last frame's state for motion vectors ([`RenderOptions::motion_vectors`]).
pub(crate) struct Motion {
    /// The view the previous frame was drawn with (same internal size); `None` = nothing to
    /// compare with this frame (first frame, cut): poses are still recorded for the next one.
    pub prev_view: Option<View>,
    /// This frame's renderer frame number ([`Actor::motion_prev`] of the last one is `frame - 1`).
    pub frame: u32,
}

impl Motion {
    /// The object matrices `actor` was drawn with last frame, if it was drawn then with as many
    /// objects; `None` = not drawn last frame (it has not moved, as far as motion vectors go).
    fn prev_mats<'a>(&self, actor: &'a Actor, n: usize) -> Option<&'a [Mat4]> {
        actor.motion_prev.as_ref().filter(|(f, m)| *f == self.frame.wrapping_sub(1) && m.len() == n).map(|(_, m)| &m[..])
    }
}

/// Whether every point of a polygon is past the same edge of the screen box `[x0, y0, x1, y1]`
/// (so it cannot show): the culling of [`RenderOptions::clip_faces`], which unlike the
/// original's screen test keeps polygons that cover the screen with no corner inside it.
fn off_screen(p: &[[f32; 3]], b: [f32; 4]) -> bool {
    p.iter().all(|v| v[0] < b[0]) || p.iter().all(|v| v[0] > b[2]) || p.iter().all(|v| v[1] < b[1]) || p.iter().all(|v| v[1] > b[3])
}

fn between(x: f32, a: f32, b: f32) -> bool {
    x > a && x < b
}

/// Everything needed to emit polys for one mesh.
pub(crate) struct EmitCtx<'a> {
    pub view: &'a View,
    pub opts: &'a RenderOptions,
    pub textures: &'a TextureRegistry,
    pub alpha_mul: f32,
    /// `overrideTex` of the actor being drawn.
    pub override_tex: Option<TexHandle>,
    /// Gouraud RGB set with `SetObjectGouraudValues`.
    pub gouraud: Option<[f32; 3]>,
    /// Motion vectors on: last frame's view (see [`Motion`]).
    pub motion: Option<&'a Motion>,
}

/// Port of `PCRenderObject` (mdxRender.cpp:2090) (`overrideTex` from the context):
/// pushes the visible faces of `mesh` using the transformed vertices `tv`.
/// `per_object_fog` reproduces the object path (`cFog` from the first face);
/// the landscape path (`false`) fogs per vertex and keeps faces without a loaded texture.
/// `ex`: per-vertex extras of the enhanced renderer (motion vectors, face clipping).
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_mesh(cx: &EmitCtx, mesh: &Mesh, tv: &[Tv], ex: Extra, frame: Frame, per_object_fog: bool, modge: Option<&Modge>, out: &mut Frames) {
    let view = cx.view;
    let [vx, vy, vw, vh] = view.vp();
    let (clx0, cly0, clx1, cly1) = (vx, vy, vx + vw - 1.0, vy + vh - 1.0);
    let fog = &cx.opts.fog;
    // Enhanced fog never applies to the unclipped sky backdrop.
    let fog_factor = |vz: f32| if view.no_clip && fog.enabled { 1.0 } else { fog.factor(vz) };
    let c_fog = mesh.faces.first().and_then(|f| tv.get(f[0] as usize).copied().flatten()).map_or(1.0, |v| fog_factor(v[2]));
    for (fi, face) in mesh.faces.iter().enumerate() {
        let crc = mesh.textures.get(fi).copied().unwrap_or(0);
        // A face whose texture is not loaded (`tex` NULL): objects skip it (`PCRenderObject`
        // draws only `if (... && (tex))`); the landscape draws it untextured
        // (`PCRenderLandscape` pushes every face, `PushPolys` binds a NULL surface and D3D's
        // MODULATE then gives the vertex colour), e.g. the frontend tree's thorns, whose
        // `janrock4` is only in Garden.fla.
        let tex = match cx.override_tex.or_else(|| cx.textures.current(crc)) {
            Some(t) => Some(t),
            None if !per_object_fog => None,
            None => continue,
        };
        let get = |k: usize| tv.get(face[k] as usize).copied().flatten();
        let (Some(a), Some(b), Some(c)) = (get(0), get(1), get(2)) else {
            // Enhanced: clip at the near plane and keep faces reaching past the far one
            // instead of dropping them.
            #[cfg(feature = "std")]
            if let Some(vv) = ex.view {
                clip_face(cx, mesh, fi, face, tex, vv, ex.prev, tv, frame, per_object_fog.then_some(c_fog), modge, out);
            }
            continue;
        };
        let p = [a, b, c];
        if cx.opts.screen_reject {
            let yon = p.iter().filter(|v| between(v[1], cly0, cly1)).count();
            let xon = p.iter().filter(|v| between(v[0], clx0, clx1)).count();
            if yon == 0 || xon == 0 {
                continue;
            }
        } else if cx.opts.clip_faces && off_screen(&p, [clx0, cly0, clx1, cly1]) {
            continue;
        }
        // D3DCULL_CW: cull clockwise (y-down screen) triangles.
        let cross = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        if cross >= 0.0 {
            continue;
        }
        let mut v = [TlVertex::default(); 3];
        for k in 0..3 {
            let (color, uv) = corner(cx, mesh, fi, face, k, modge);
            let vz = p[k][2];
            v[k] = TlVertex {
                sx: p[k][0],
                sy: p[k][1],
                sz: (vz * SZ_SCALE).clamp(0.0, 1.0),
                rhw: 1.0 / (vz + DIST),
                color,
                uv,
                fog: if per_object_fog && modge.is_none() { c_fog } else { fog_factor(vz) },
                alpha_ref: 0.0,
            };
        }
        match ex.prev {
            Some(tp) => {
                let mv = |k: usize| tp.get(face[k] as usize).map_or([0.0; 2], |q| [q[0] - p[k][0], q[1] - p[k][1]]);
                out.push_moving(frame, Tri { v, tex }, [mv(0), mv(1), mv(2)]);
            }
            None => out.push(frame, Tri { v, tex }),
        }
    }
}

/// Per-vertex extras of [`render_mesh`] (enhanced; `Default` = the original).
#[derive(Clone, Copy, Default)]
pub(crate) struct Extra<'a> {
    /// The vertices' previous-frame screen positions (motion vectors).
    pub prev: Option<&'a [[f32; 2]]>,
    /// The vertices' view-space positions ([`RenderOptions::clip_faces`]).
    #[cfg_attr(not(feature = "std"), allow(dead_code))]
    pub view: Option<&'a [Vec3]>,
}

/// Colour (clamped to bytes like `D3DRGBA`, alpha included) and texture coordinates of
/// corner `k` of face `fi`, as `PCRenderObject` and the modgy renderers set them.
fn corner(cx: &EmitCtx, mesh: &Mesh, fi: usize, face: &[i16; 3], k: usize, modge: Option<&Modge>) -> ([f32; 4], [f32; 2]) {
    let mut col = mesh.colors.get(fi * 3 + k).copied().unwrap_or([1.0; 4]);
    if let Some(g) = cx.gouraud {
        col = [g[0], g[1], g[2], col[3]];
    }
    let uv = mesh.face_uv.get(fi * 3 + k).copied().unwrap_or([0, 0]);
    let mut uv = [uv[0] as f32 * UV_SCALE, uv[1] as f32 * UV_SCALE];
    let mut alpha = col[3] * cx.alpha_mul;
    if let Some(m) = modge {
        let [mx, mz, _] = m.values.get(face[k] as usize).copied().unwrap_or_default();
        uv = [uv[0] + mx * 0.4, uv[1] + mz * 0.4];
        if m.wave {
            // `50+(unsigned long)((m1x+m1z)*400)<<24`
            alpha = (50 + ((mx + mz) * 400.0) as u32) as f32 / 255.0;
        }
    }
    ([col[0].clamp(0.0, 1.0), col[1].clamp(0.0, 1.0), col[2].clamp(0.0, 1.0), alpha.clamp(0.0, 1.0)], uv)
}

/// View-space positions of `mesh`'s vertices under model matrix `m` (the transform
/// `prepare_object` / `prepare_modgy_object` project), for [`RenderOptions::clip_faces`].
fn prepare_view(view: &View, mesh: &Mesh, m: &Mat4, lift: Option<&ModgeValues>, out: &mut Vec<Vec3>) {
    let f = mtx_cat(m, &view.v_matrix);
    out.clear();
    out.extend(mesh.vertices.iter().enumerate().map(|(i, &v)| {
        let dy = lift.and_then(|l| l.get(i)).map_or(0.0, |w| w[2]);
        mtx_xfm(&f, [v[0], v[1] + dy, v[2]])
    }));
}

/// [`prepare_view`] for the root-mesh vertices one bone of a skin moves.
fn prepare_skinned_view(view: &View, obj: &Object, mesh: &Mesh, m: &Mat4, out: &mut [Vec3]) {
    let f = mtx_cat(m, &view.v_matrix);
    for &j in &obj.effected_verts {
        if let (Some(v), Some(slot)) = (mesh.vertices.get(j as usize), out.get_mut(j as usize)) {
            *slot = mtx_xfm(&f, *v);
        }
    }
}

/// A face `render_mesh` would drop because a vertex is outside the near / far range
/// (`PCPrepareObject` stores `vz = 0`), drawn instead (enhanced, [`RenderOptions::clip_faces`]):
/// clipped at the near plane (Sutherland-Hodgman in view space, attributes interpolated) and
/// kept when it only reaches past the far plane. Faces wholly beyond the far plane still go.
/// `obj_fog`: the object path's per-object fog (`None` = per vertex).
#[cfg(feature = "std")]
#[cold]
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn clip_face(
    cx: &EmitCtx,
    mesh: &Mesh,
    fi: usize,
    face: &[i16; 3],
    tex: Option<TexHandle>,
    vv: &[Vec3],
    prev: Option<&[[f32; 2]]>,
    tv: &[Tv],
    frame: Frame,
    obj_fog: Option<f32>,
    modge: Option<&Modge>,
    out: &mut Frames,
) {
    let view = cx.view;
    // One clipped corner: view position, colour, uv, motion (previous screen position minus
    // this one, in pixels; interpolated across a cut).
    #[derive(Clone, Copy)]
    struct C {
        p: Vec3,
        col: [f32; 4],
        uv: [f32; 2],
        mv: [f32; 2],
    }
    let mut corners = [C { p: [0.0; 3], col: [0.0; 4], uv: [0.0; 2], mv: [0.0; 2] }; 3];
    for k in 0..3 {
        let i = face[k] as usize;
        let Some(&p) = vv.get(i).filter(|p| !p[0].is_nan()) else { return };
        let (col, uv) = corner(cx, mesh, fi, face, k, modge);
        // Motion of an unprojected vertex: from its unclipped screen position.
        let mv = match prev.and_then(|q| q.get(i)) {
            Some(q) => {
                let s = tv.get(i).copied().flatten().map_or_else(|| view.screen_unclipped(p), |s| [s[0], s[1]]);
                [q[0] - s[0], q[1] - s[1]]
            }
            None => [0.0; 2],
        };
        corners[k] = C { p, col, uv, mv };
    }
    let depth = |c: &C| c.p[2] + DIST;
    // Wholly beyond the far plane: dropped as before (the draw distance setting moves it).
    if !view.no_clip && corners.iter().all(|c| depth(c) >= view.far_clip) {
        return;
    }
    // Clip against the near plane (a hair inside it: `oozd` needs a positive depth).
    let near = view.near_clip + 0.01;
    let mut poly: [C; 4] = [corners[0]; 4];
    let mut n = 0;
    for k in 0..3 {
        let (a, b) = (corners[k], corners[(k + 1) % 3]);
        let (da, db) = (depth(&a) - near, depth(&b) - near);
        if da >= 0.0 {
            poly[n] = a;
            n += 1;
        }
        if (da >= 0.0) != (db >= 0.0) {
            let t = da / (da - db);
            let l = |x: f32, y: f32| x + (y - x) * t;
            poly[n] = C {
                p: [l(a.p[0], b.p[0]), l(a.p[1], b.p[1]), l(a.p[2], b.p[2])],
                col: [l(a.col[0], b.col[0]), l(a.col[1], b.col[1]), l(a.col[2], b.col[2]), l(a.col[3], b.col[3])],
                uv: [l(a.uv[0], b.uv[0]), l(a.uv[1], b.uv[1])],
                mv: [l(a.mv[0], b.mv[0]), l(a.mv[1], b.mv[1])],
            };
            n += 1;
        }
    }
    if n < 3 {
        return;
    }
    let s: [[f32; 3]; 4] = core::array::from_fn(|k| view.to_screen(poly[k].p, view.oozd(poly[k].p[2])));
    let [vx, vy, vw, vh] = view.vp();
    if off_screen(&s[..n], [vx, vy, vx + vw - 1.0, vy + vh - 1.0]) {
        return;
    }
    // D3DCULL_CW on the clipped polygon (clipping keeps the winding).
    let cross = (s[1][0] - s[0][0]) * (s[2][1] - s[0][1]) - (s[1][1] - s[0][1]) * (s[2][0] - s[0][0]);
    if cross >= 0.0 {
        return;
    }
    let fog = &cx.opts.fog;
    let fog_factor = |vz: f32| if view.no_clip && fog.enabled { 1.0 } else { fog.factor(vz) };
    let vert = |k: usize| {
        let vz = s[k][2];
        TlVertex {
            sx: s[k][0],
            sy: s[k][1],
            sz: (vz * SZ_SCALE).clamp(0.0, 1.0),
            rhw: 1.0 / (vz + DIST),
            color: poly[k].col,
            uv: poly[k].uv,
            fog: match obj_fog {
                Some(f) if modge.is_none() => f,
                _ => fog_factor(vz),
            },
            alpha_ref: 0.0,
        }
    };
    for (a, b, c) in [(0, 1, 2), (0, 2, 3)].into_iter().take(n - 2) {
        let tri = Tri { v: [vert(a), vert(b), vert(c)], tex };
        match prev {
            Some(_) => out.push_moving(frame, tri, [poly[a].mv, poly[b].mv, poly[c].mv]),
            None => out.push(frame, tri),
        }
    }
}

/// Frame selection of `DrawObject` (mdxRender.cpp:1065); `wrap` = `wrapCoords`.
fn object_frame(flags: i16, xlu_fade: bool, wrap: bool) -> Frame {
    let mut f = Frame::Normal;
    if flags & OBJECT_FLAGS_MODGE != 0 || wrap {
        f = Frame::Wrap;
    }
    if flags & OBJECT_FLAGS_XLU != 0 || xlu_fade {
        f = Frame::Xlu;
    }
    if flags & OBJECT_FLAGS_ADDITIVE != 0 {
        f = Frame::Additive;
    }
    if flags & OBJECT_FLAGS_GLOW != 0 {
        f = Frame::Glow;
    }
    f
}

/// Port of `XformActor` + `DrawActor` (mdxActor.cpp:225/547) for one model.
pub(crate) fn draw_actor(cx: &EmitCtx, model: &Model, actor: &mut Actor, out: &mut Frames) {
    let mats = xform_actor(model, actor);
    let fade = actor.xlu < 1.0;
    let extra = actor.extra_flags;
    let (wrap, fc) = (actor.wrap_tc, actor.modge_frame);
    // Motion vectors: last frame's matrices (this frame's when the actor was not drawn then).
    let motion = cx.motion.and_then(|m| Some((m.prev_view.as_ref()?, m.prev_mats(actor, mats.len()).unwrap_or(&mats))));
    let (mut tvp, mut tvv) = (Vec::new(), Vec::new());
    if model.is_skinned {
        // All bones transform their share of the root mesh, then the root mesh is drawn once.
        let Some(mesh) = model.root.mesh.as_ref() else { return };
        let mut tv: Vec<Tv> = vec![None; mesh.vertices.len()];
        for_each_object(model, |i, o| prepare_skinned(cx.view, o, mesh, &mats[i], &mut tv));
        if let Some((m, prev)) = motion {
            tvp = vec![[0.0; 2]; mesh.vertices.len()];
            for_each_object(model, |i, o| prepare_skinned_prev(m, o, mesh, &prev[i], &mut tvp));
        }
        if cx.opts.clip_faces {
            // NaN: a vertex no bone moves (dropped like the original's unset `tV`).
            tvv = vec![[f32::NAN; 3]; mesh.vertices.len()];
            for_each_object(model, |i, o| prepare_skinned_view(cx.view, o, mesh, &mats[i], &mut tvv));
        }
        // `PCRenderObject(root)` runs after `DrawObject` restored the frame, so the
        // skin always lands in the caller's frame (normal); only run-time flags apply.
        let frame = object_frame(extra, fade, wrap);
        let ex = Extra { prev: motion.map(|_| &tvp[..]), view: cx.opts.clip_faces.then_some(&tvv[..]) };
        render_mesh(cx, mesh, &tv, ex, frame, true, None, out);
    } else {
        let mut tv = Vec::new();
        let mut mv = ModgeValues::new();
        for_each_object(model, |i, o| {
            let Some(mesh) = o.mesh.as_ref() else { return };
            // DrawObject (mdxRender.cpp:1126): modge values for MODGE or WAVE, the wave
            // lifts the vertices, MODGE picks the modgy renderers.
            let flags = o.flags | extra;
            let (modge, wave) = (flags & OBJECT_FLAGS_MODGE != 0, flags & OBJECT_FLAGS_WAVE != 0);
            if modge || wave {
                calc_modge_values(mesh, fc, &mut mv);
            }
            if wave {
                prepare_modgy_object(cx.view, mesh, &mats[i], &mv, &mut tv);
            } else {
                prepare_object(cx.view, mesh, &mats[i], &mut tv);
            }
            if let Some((m, prev)) = motion {
                prepare_prev(m, mesh, &prev[i], wave.then_some(&mv), &mut tvp);
            }
            if cx.opts.clip_faces {
                prepare_view(cx.view, mesh, &mats[i], wave.then_some(&mv), &mut tvv);
            }
            let frame = object_frame(flags, fade, wrap);
            // PCRenderModgySheenObject (SHEEN as well) is drawn like PCRenderModgyObject.
            let m = modge.then_some(Modge { values: &mv, wave });
            let ex = Extra { prev: motion.map(|_| &tvp[..]), view: cx.opts.clip_faces.then_some(&tvv[..]) };
            render_mesh(cx, mesh, &tv, ex, frame, true, m.as_ref(), out);
        });
    }
    if let Some(m) = cx.motion {
        actor.motion_prev = Some((m.frame, mats));
    }
}

/// Pre-order object index of a skinned model's root mesh in a [`MeshDraw`] (the skin is one
/// mesh deformed by every bone).
pub const MESH_SKIN: u16 = u16::MAX;

/// One mesh draw recorded instead of transformed on the CPU, for a back end that does the
/// MDX transform itself (the N64 RSP renderer): which mesh, its model -> world matrices
/// (`xform_actor`, one per bone for skins), the frame list `DrawObject` would push its faces
/// to, and the `PCRenderObject` parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshDraw {
    /// `Model::id` (`crc("name.obe")`).
    pub model: u32,
    /// Pre-order object index (`for_each_object`), or [`MESH_SKIN`].
    pub object: u16,
    pub frame: Frame,
    /// `MeshSink::mats[mat_first..mat_first + mat_count]`.
    pub mat_first: u32,
    pub mat_count: u32,
    pub alpha_mul: f32,
    pub gouraud: Option<[f32; 3]>,
    pub override_tex: Option<TexHandle>,
    /// `View::no_clip` at record time (sky backdrop).
    pub no_clip: bool,
    /// Per-object fog (actor path) instead of per vertex (landscape).
    pub object_fog: bool,
    /// Version of the matrices (`PoseCache::stamp`): a draw of the same object with the
    /// same stamp has the same matrices. 0 = unknown.
    pub stamp: u32,
    /// The object's `OBJECT_FLAGS_MODGE` (bit 0) / `OBJECT_FLAGS_WAVE` (bit 1): the back end
    /// animates the mesh like `PCCalcModgeValues` at `modge_frame` (`timeInfo.frameCount`).
    pub modge: u8,
    pub modge_frame: u32,
}

/// Receives each recorded mesh draw as it is made (a [`MeshSink`] with a back end stores
/// nothing): the matrices are consumed while they are still in the data cache, which on a
/// console (the N64's 8 KiB D-cache) is much cheaper than storing and re-reading them.
pub trait MeshBackend {
    /// `d.mat_first`/`d.mat_count` are 0: `mats` are the draw's matrices (one per bone for
    /// [`MESH_SKIN`]). `view` is the renderer's view at record time.
    fn mesh(&mut self, view: &View, d: &MeshDraw, mats: &[Mat4]);

    /// The mesh draws of one rigid actor, in order: for each `o` of `objs`,
    /// `MeshDraw { object: o.object, frame: o.frame, modge: o.modge, ..*base }` with the matrix
    /// `mats[o.object]` (the same draws [`mesh`](Self::mesh) would get one by one; one call
    /// per actor instead of one per object).
    fn actor(&mut self, view: &View, base: &MeshDraw, objs: &[ObjDraw], mats: &[Mat4]) {
        for o in objs {
            let i = o.object as usize;
            self.mesh(view, &MeshDraw { object: o.object, frame: o.frame, modge: o.modge, ..*base }, &mats[i..i + 1]);
        }
    }

    /// Whether every rigid draw inside the world-space sphere `(c, r)` would be culled (wholly
    /// outside the view): the caller may then leave the draws out ([`skip`](Self::skip)).
    /// The default never culls.
    fn sphere_culled(&self, _c: &Vec3, _r: f32) -> bool {
        false
    }

    /// `n` draws left out because [`sphere_culled`](Self::sphere_culled) said so (a back end
    /// that numbers its draws keeps the numbers of the ones after them).
    fn skip(&mut self, _n: u32) {}

    /// The bounding sphere (model units) of what the back end draws for the mesh of object
    /// `object` of model `model`.
    fn object_sphere(&self, _model: u32, _object: u16) -> ObjSphere {
        ObjSphere::Unknown
    }

    /// Testing: whether culled actors are drawn anyway, their draws marked
    /// ([`mark_culled`](Self::mark_culled)) so the back end can check that it culls them all.
    fn cull_check(&self) -> bool {
        false
    }

    /// Testing ([`cull_check`](Self::cull_check)): the draws until the next call belong to
    /// an actor [`sphere_culled`](Self::sphere_culled) said is wholly outside the view.
    fn mark_culled(&mut self, _on: bool) {}
}

/// [`MeshBackend::object_sphere`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ObjSphere {
    /// No bound known: never cull.
    Unknown,
    /// The back end draws nothing for the object.
    NoMesh,
    /// Every vertex drawn lies within `radius` of `centre` (model units).
    Sphere(Vec3, f32),
}

/// One object draw of a rigid actor ([`MeshBackend::actor`]): pre-order object index, frame
/// list (never [`Frame::Glow`]) and the [`MeshDraw::modge`] bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjDraw {
    pub object: u16,
    pub frame: Frame,
    pub modge: u8,
}

/// Recorded mesh draws of a frame (see [`MeshDraw`]).
#[derive(Default)]
pub struct MeshSink {
    pub draws: Vec<MeshDraw>,
    pub mats: Vec<Mat4>,
    /// The landscape model's key-0 object matrices (static: computed once per model) and
    /// their stamp.
    landscape: Option<(u32, Vec<Mat4>, u32)>,
    /// When set, draws go straight to it instead of `draws`/`mats`.
    pub backend: Option<Box<dyn MeshBackend + Send>>,
}

impl core::fmt::Debug for MeshSink {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MeshSink").field("draws", &self.draws.len()).field("mats", &self.mats.len()).field("backend", &self.backend.is_some()).finish()
    }
}

impl MeshSink {
    pub fn clear(&mut self) {
        self.draws.clear();
        self.mats.clear();
    }

    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_d")]
    fn push(&mut self, view: &View, d: MeshDraw, mats: &[Mat4]) {
        if let Some(b) = &mut self.backend {
            b.mesh(view, &d, mats);
            return;
        }
        let mat_first = self.mats.len() as u32;
        self.mats.extend_from_slice(mats);
        self.draws.push(MeshDraw { mat_first, mat_count: mats.len() as u32, ..d });
    }

    /// [`MeshBackend::sphere_culled`] (false without a back end: recorded draws are kept).
    pub fn sphere_culled(&self, c: &Vec3, r: f32) -> bool {
        self.backend.as_ref().is_some_and(|b| b.sphere_culled(c, r))
    }

    /// [`push`](Self::push) of every draw of a rigid actor ([`MeshBackend::actor`]).
    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_d")]
    fn push_actor(&mut self, view: &View, base: &MeshDraw, objs: &[ObjDraw], mats: &[Mat4]) {
        if let Some(b) = &mut self.backend {
            b.actor(view, base, objs, mats);
            return;
        }
        for o in objs {
            let i = o.object as usize;
            self.push(view, MeshDraw { object: o.object, frame: o.frame, modge: o.modge, ..*base }, &mats[i..i + 1]);
        }
    }
}

/// [`Renderer::skip_actor`](crate::Renderer::skip_actor): tells the back end that the
/// actor's draws (as [`record_actor`] would make them now) are left out.
pub(crate) fn skip_actor(model: &Model, actor: &mut Actor, sink: &mut MeshSink) {
    let Some(b) = sink.backend.as_mut() else { return };
    let n = if model.is_skinned {
        1
    } else {
        let key = (model.id, actor.extra_flags, actor.xlu < 1.0, actor.wrap_tc);
        if actor.mesh_draws.as_ref().is_none_or(|(k, _)| *k != key) {
            new_draws(model, &mut actor.mesh_draws, key);
        }
        actor.mesh_draws.as_ref().map_or(0, |d| d.1.len() as u32)
    };
    b.skip(n);
}

#[cold]
#[inline(never)]
fn new_draws(model: &Model, slot: &mut Option<crate::anim::MeshDraws>, key: (u32, i16, bool, bool)) {
    *slot = Some((key, object_draws(model, key.1, key.2, key.3)));
}

/// The draws of a rigid model's objects that have a mesh, in pre-order: frame list
/// (`object_frame` of the object's flags with `extra` OR-ed in) and MODGE/WAVE bits; `Glow`
/// objects (never drawn on retail PC) left out.
fn object_draws(model: &Model, extra: i16, fade: bool, wrap: bool) -> Vec<ObjDraw> {
    let mut objs = Vec::new();
    for_each_object(model, |i, o| {
        if o.mesh.is_some() {
            let f = o.flags | extra;
            let frame = object_frame(f, fade, wrap);
            if frame != Frame::Glow {
                let modge = (f & OBJECT_FLAGS_MODGE != 0) as u8 | ((f & OBJECT_FLAGS_WAVE != 0) as u8) << 1;
                objs.push(ObjDraw { object: i as u16, frame, modge });
            }
        }
    });
    objs
}

/// [`draw_actor`] recorded into a [`MeshSink`]: same matrices, same frame selection, no
/// vertex work. Objects without a mesh are skipped; whether a mesh has drawable faces is
/// the back end's business. `Glow` (never drawn on retail PC) is dropped.
#[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_d")]
pub(crate) fn record_actor(cx: &EmitCtx, model: &Model, actor: &mut Actor, sink: &mut MeshSink) {
    let fade = actor.xlu < 1.0;
    let extra = actor.extra_flags;
    xform_actor_cached(model, actor);
    let stamp = actor.pose.stamp;
    let mats = &actor.pose.mats[..];
    let base = MeshDraw {
        model: model.id,
        object: 0,
        frame: Frame::Normal,
        mat_first: 0,
        mat_count: 0,
        alpha_mul: cx.alpha_mul,
        gouraud: cx.gouraud,
        override_tex: cx.override_tex,
        no_clip: cx.view.no_clip,
        object_fog: true,
        stamp,
        modge: 0,
        modge_frame: actor.modge_frame,
    };
    if model.is_skinned {
        record_skin(cx, model, extra, fade, actor.wrap_tc, &base, mats, sink);
    } else {
        // The draw list depends on the model and the flags only: kept while they hold.
        let key = (model.id, extra, fade, actor.wrap_tc);
        if actor.mesh_draws.as_ref().is_none_or(|(k, _)| *k != key) {
            new_draws(model, &mut actor.mesh_draws, key);
        }
        let objs = &actor.mesh_draws.as_ref().unwrap().1;
        sink.push_actor(cx.view, &base, objs, mats);
    }
}

/// The skinned part of [`record_actor`] (out of line: one model, the frog).
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn record_skin(cx: &EmitCtx, model: &Model, extra: i16, fade: bool, wrap: bool, base: &MeshDraw, mats: &[Mat4], sink: &mut MeshSink) {
    if model.root.mesh.is_none() {
        return;
    }
    let frame = object_frame(extra, fade, wrap);
    if frame != Frame::Glow {
        sink.push(cx.view, MeshDraw { object: MESH_SKIN, frame, ..*base }, mats);
    }
}

/// [`draw_landscape`] recorded into a [`MeshSink`].
pub(crate) fn record_landscape(cx: &EmitCtx, model: &Model, sink: &mut MeshSink) {
    let (_, mats, stamp) = match sink.landscape.take() {
        Some(l) if l.0 == model.id => l,
        _ => (model.id, xform_actor(model, &mut Actor { animated: false, ..Default::default() }), crate::anim::next_stamp()),
    };
    for_each_object(model, |i, o| {
        if o.mesh.is_none() {
            return;
        }
        let d = MeshDraw {
            model: model.id,
            object: i as u16,
            frame: Frame::Normal,
            mat_first: 0,
            mat_count: 0,
            alpha_mul: 1.0,
            gouraud: None,
            override_tex: None,
            no_clip: cx.view.no_clip,
            object_fog: false,
            stamp,
            modge: 0,
            modge_frame: 0,
        };
        sink.push(cx.view, d, &mats[i..i + 1]);
    });
    sink.landscape = Some((model.id, mats, stamp));
}

/// Port of `ConvertObjectToLandscape` + `DrawLandscape` (mdxLandscape.cpp:63,
/// mdxRender.cpp:1005): static level geometry baked with key-0 transforms,
/// drawn into the current (normal) frame with per-vertex fog.
pub(crate) fn draw_landscape(cx: &EmitCtx, model: &Model, out: &mut Frames) {
    let mut actor = Actor { animated: false, ..Default::default() };
    let mats = xform_actor(model, &mut actor);
    let (mut tv, mut tvp, mut tvv) = (Vec::new(), Vec::new(), Vec::new());
    for_each_object(model, |i, o| {
        let Some(mesh) = o.mesh.as_ref() else { return };
        prepare_object(cx.view, mesh, &mats[i], &mut tv);
        // Static: only the camera moves it.
        let prev = cx.motion.and_then(|m| m.prev_view.as_ref());
        if let Some(pv) = prev {
            prepare_prev(pv, mesh, &mats[i], None, &mut tvp);
        }
        if cx.opts.clip_faces {
            prepare_view(cx.view, mesh, &mats[i], None, &mut tvv);
        }
        let ex = Extra { prev: prev.map(|_| &tvp[..]), view: cx.opts.clip_faces.then_some(&tvv[..]) };
        render_mesh(cx, mesh, &tv, ex, Frame::Normal, false, None, out);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_matches_original_formula() {
        let v = View::new(640, 480);
        // vz = 110 → oneOver[100] = 1/100 → oozd = -4.5
        let p = v.project([10.0, 20.0, 110.0]).unwrap();
        assert_eq!(p, [320.0 - 45.0, 240.0 - 90.0, 110.0]);
        assert!(v.project([0.0, 0.0, 15.0]).is_none());
        assert!(v.project([0.0, 0.0, 5000.0]).is_none());
    }

    #[test]
    fn hor_plus_keeps_vertical_fov() {
        // At 4:3 Hor+ is the original formula.
        let mut a = View::new(1280, 960);
        let b = View { hor_plus: true, ..View::new(1280, 960) };
        assert_eq!(a.project([10.0, 20.0, 110.0]), b.project([10.0, 20.0, 110.0]));
        // 2560×1080: the original scales by width (Vert-), Hor+ by height.
        a = View::new(2560, 1080);
        let h = View { hor_plus: true, ..View::new(2560, 1080) };
        assert_eq!(a.proj_scale(), 4.0);
        assert_eq!(h.proj_scale(), 2.25);
        // A point at the 4:3 vertical edge (|y| = 240/4.5 at vz 110) lands on the screen edge (y flips).
        let p = h.project([0.0, -240.0 / 4.5, 110.0]).unwrap();
        assert!((p[1] - (540.0 + 240.0 * 2.25)).abs() < 1e-3);
        // Pillarboxed viewport: centred on the viewport.
        let v = View { hor_plus: true, viewport: Some([100.0, 0.0, 640.0, 480.0]), ..View::new(840, 480) };
        assert_eq!(v.project([0.0, 0.0, 110.0]).unwrap(), [420.0, 240.0, 110.0]);
    }

    #[test]
    fn hor_plus_extent_past_the_4_3_frustum() {
        // Without Hor+ (authentic, also on wide screens) the view is never widened.
        assert_eq!(View::new(2560, 1080).hor_plus_extent(), [1.0, 1.0]);
        assert!(!View::new(2560, 1080).widened());
        let hp = |w, h| View { hor_plus: true, ..View::new(w, h) };
        assert_eq!(hp(1280, 960).hor_plus_extent(), [1.0, 1.0]);
        assert!(!hp(640, 480).widened());
        // 32:9: 3840/(1080/480) = 1706.67 of 640 → 2.667 wide; tall 3:4 → 1.778 high.
        let [x, y] = hp(3840, 1080).hor_plus_extent();
        assert!((x - 8.0 / 3.0).abs() < 1e-4 && y == 1.0);
        let [x, y] = hp(1080, 1440).hor_plus_extent();
        assert!(x == 1.0 && (y - 16.0 / 9.0).abs() < 1e-4);
        // Ultrawide clamp: the pillarboxed viewport counts, not the window.
        let v = View { hor_plus: true, viewport: Some([100.0, 0.0, 640.0, 480.0]), ..View::new(840, 480) };
        assert!(!v.widened());
    }
}
