//! Temporal upscaling and frame generation inputs (DLSS, FSR; enhanced, not in the original).
//!
//! With an upscaler the renderer draws the 3D scene (backdrops and the seven 3D passes) at a
//! lower *render* size with a sub-pixel camera jitter, and writes two more targets beside the
//! colour: per-pixel **motion vectors** (`Rg16Float`, render pixels, previous position minus
//! current, without the jitter) and a **depth** buffer (`R32Float`, reversed infinite:
//! `near / z`, 0 = sky). The opaque passes write them, from each vertex's previous-frame
//! screen position ([`crate::scene::RenderOptions::motion_vectors`]: the actor's last pose
//! through the last frame's camera). An [`Upscaler`] (the host's: native D3D12 libraries)
//! turns them into an output-sized image, and the 2D overlays (HUD, menus, text) are drawn
//! over that at the output size, so they stay sharp and out of the upscaler's history.
//!
//! [`UpscaleCamera`] describes the projection the way the libraries want it. The game's own
//! projection (`PCPrepareObject`) is `screen = centre + view.xy * -FOV * scale / (z - 10)`;
//! here it is a standard perspective matrix on a camera space `X = -x`, `Y = y`, `Z = z - 10`
//! (x right, y up, z forward), reversed-Z with an infinite far plane and [`DEPTH_NEAR`].

#[cfg(not(feature = "std"))]
use crate::fmath::FloatExt;
use crate::math::Mat4;
use crate::scene::View;

/// The near plane of the upscaler depth (`near / z`): the original's near clip (`z - 10 > 10`).
pub const DEPTH_NEAR: f32 = 10.0;

/// Upscaling quality modes: the per-axis ratio of output to render size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum UpscaleMode {
    /// Render at the output size, anti-aliasing only (DLAA, FSR "Native AA").
    Native,
    /// 1.5x.
    #[default]
    Quality,
    /// 1.7x (DLSS: 1.724x).
    Balanced,
    /// 2x.
    Performance,
    /// 3x.
    UltraPerformance,
}

impl UpscaleMode {
    pub const ALL: [UpscaleMode; 5] =
        [UpscaleMode::Native, UpscaleMode::Quality, UpscaleMode::Balanced, UpscaleMode::Performance, UpscaleMode::UltraPerformance];

    /// Output size / render size, per axis (the FSR and DLSS defaults).
    pub fn ratio(self) -> f32 {
        match self {
            UpscaleMode::Native => 1.0,
            UpscaleMode::Quality => 1.5,
            UpscaleMode::Balanced => 1.7,
            UpscaleMode::Performance => 2.0,
            UpscaleMode::UltraPerformance => 3.0,
        }
    }

    /// The render size for an `output` size.
    pub fn render_size(self, output: (u32, u32)) -> (u32, u32) {
        let r = self.ratio();
        let f = |v: u32| ((v as f32 / r).round() as u32).max(1);
        (f(output.0), f(output.1))
    }
}

/// Number of jitter phases for a render / output width (FSR's `ceil(8 * ratio^2)`; DLSS
/// recommends at least 8 per unit of area ratio as well).
pub fn jitter_phase_count(render_width: u32, output_width: u32) -> u32 {
    let r = output_width.max(1) as f32 / render_width.max(1) as f32;
    ((8.0 * r * r).ceil() as u32).max(8)
}

/// Radical inverse of `index` in `base` (the Halton sequence).
pub fn halton(mut index: u32, base: u32) -> f32 {
    let (mut f, mut r) = (1.0f32, 0.0f32);
    while index > 0 {
        f /= base as f32;
        r += f * (index % base) as f32;
        index /= base;
    }
    r
}

/// The sub-pixel jitter of frame `frame`: Halton(2, 3), each axis in -0.5..0.5 render pixels
/// (x right, y down; the image is drawn shifted by this much).
pub fn jitter(frame: u64, phases: u32) -> [f32; 2] {
    let i = (frame % phases.max(1) as u64) as u32 + 1;
    [halton(i, 2) - 0.5, halton(i, 3) - 0.5]
}

/// 4x4 matrix in the row-vector convention (`v' = v * M`, row-major), as Streamline and FSR
/// take them.
pub type Mat4x4 = [[f32; 4]; 4];

pub const IDENTITY4: Mat4x4 = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];

pub fn mul4(a: &Mat4x4, b: &Mat4x4) -> Mat4x4 {
    let mut o = [[0.0; 4]; 4];
    for (r, row) in o.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            *v = (0..4).map(|k| a[r][k] * b[k][c]).sum();
        }
    }
    o
}

/// General inverse (`None` when singular).
pub fn inverse4(m: &Mat4x4) -> Option<Mat4x4> {
    // Gauss-Jordan with partial pivoting, in f64.
    let mut a = [[0.0f64; 8]; 4];
    for r in 0..4 {
        for c in 0..4 {
            a[r][c] = m[r][c] as f64;
        }
        a[r][4 + r] = 1.0;
    }
    for c in 0..4 {
        let p = (c..4).max_by(|&x, &y| a[x][c].abs().total_cmp(&a[y][c].abs()))?;
        if a[p][c].abs() < 1e-30 {
            return None;
        }
        a.swap(c, p);
        let d = a[c][c];
        for v in &mut a[c] {
            *v /= d;
        }
        for r in 0..4 {
            if r != c {
                let k = a[r][c];
                if k != 0.0 {
                    let row = a[c];
                    for (v, x) in a[r].iter_mut().zip(row) {
                        *v -= k * x;
                    }
                }
            }
        }
    }
    let mut o = [[0.0; 4]; 4];
    for r in 0..4 {
        for c in 0..4 {
            o[r][c] = a[r][4 + c] as f32;
        }
    }
    Some(o)
}

/// The camera as the upscaling / frame generation libraries are told about it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UpscaleCamera {
    pub view_to_clip: Mat4x4,
    pub clip_to_view: Mat4x4,
    /// Current clip space to last frame's (for points that did not move).
    pub clip_to_prev_clip: Mat4x4,
    pub prev_clip_to_clip: Mat4x4,
    /// World space.
    pub position: [f32; 3],
    pub up: [f32; 3],
    pub right: [f32; 3],
    pub forward: [f32; 3],
    /// Near plane (reversed infinite depth, [`DEPTH_NEAR`]); `far` is a large stand-in for
    /// libraries that want a value.
    pub near: f32,
    pub far: f32,
    /// Vertical field of view (radians) and width / height.
    pub fov_y: f32,
    pub aspect: f32,
}

/// `view_to_clip` of a view: see the module docs.
pub fn projection(view: &View) -> Mat4x4 {
    let (w, h) = (view.width.max(1.0), view.height.max(1.0));
    let [vx, vy, vw, vh] = view.vp();
    let k = view.fov * view.proj_scale();
    let (a, b) = (2.0 * k / w, 2.0 * k / h);
    // The viewport centre (with D3D7's half-pixel offset, see the vertex shader) in NDC.
    let (cx, cy) = (vx + vw / 2.0, vy + vh / 2.0);
    let (ox, oy) = ((cx + 0.5) * 2.0 / w - 1.0, 1.0 - (cy + 0.5) * 2.0 / h);
    [[a, 0.0, 0.0, 0.0], [0.0, b, 0.0, 0.0], [ox, oy, 0.0, 1.0], [0.0, 0.0, DEPTH_NEAR, 0.0]]
}

/// World to camera space (`X = -x`, `Y = y`, `Z = z - 10` of the game's view space).
pub fn world_to_camera(v: &Mat4) -> Mat4x4 {
    let mut m = [[0.0; 4]; 4];
    for r in 0..4 {
        m[r] = [-v[r][0], v[r][1], v[r][2], if r == 3 { 1.0 } else { 0.0 }];
    }
    m[3][2] -= 10.0;
    m
}

impl UpscaleCamera {
    /// The camera of `view`; `prev` = last frame's view (`None`: no motion, e.g. after a cut).
    pub fn new(view: &View, prev: Option<&View>) -> UpscaleCamera {
        let p = projection(view);
        let pinv = inverse4(&p).unwrap_or(IDENTITY4);
        let w2c = world_to_camera(&view.v_matrix);
        let c2w = inverse4(&w2c).unwrap_or(IDENTITY4);
        let (c2p, p2c) = match prev {
            Some(pv) => {
                let pp = projection(pv);
                let to_prev = mul4(&mul4(&mul4(&pinv, &c2w), &world_to_camera(&pv.v_matrix)), &pp);
                let back = inverse4(&to_prev).unwrap_or(IDENTITY4);
                (to_prev, back)
            }
            None => (IDENTITY4, IDENTITY4),
        };
        let row = |r: usize| [c2w[r][0], c2w[r][1], c2w[r][2]];
        let norm = |v: [f32; 3]| {
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            if l > 1e-12 {
                [v[0] / l, v[1] / l, v[2] / l]
            } else {
                v
            }
        };
        UpscaleCamera {
            view_to_clip: p,
            clip_to_view: pinv,
            clip_to_prev_clip: c2p,
            prev_clip_to_clip: p2c,
            position: row(3),
            up: norm(row(1)),
            right: norm(row(0)),
            forward: norm(row(2)),
            near: DEPTH_NEAR,
            far: 1.0e7,
            fov_y: 2.0 * (1.0 / p[1][1]).atan(),
            aspect: p[1][1] / p[0][0],
        }
    }
}

/// One frame's upscaler inputs ([`crate::Renderer::render_upscaled`]). The textures are in
/// shader-read state (`TextureUses::RESOURCE`; `output`: `STORAGE_READ_WRITE`) and every
/// command that wrote them has been submitted; native code must leave them in those states.
#[cfg(feature = "gpu")]
pub struct UpscaleInputs<'a> {
    /// The scene at the render size (`Rgba8Unorm`, gamma-space LDR, no HUD).
    pub color: &'a wgpu::Texture,
    /// Reversed infinite depth (`R32Float`, `DEPTH_NEAR / z`, 0 = nothing drawn).
    pub depth: &'a wgpu::Texture,
    /// Motion vectors (`Rg16Float`, render pixels, previous position minus current).
    pub motion: &'a wgpu::Texture,
    /// Where the upscaled image goes (`Rgba8Unorm`, the output size, storage).
    pub output: &'a wgpu::Texture,
    pub render_size: (u32, u32),
    pub output_size: (u32, u32),
    /// This frame's jitter (render pixels, see [`jitter`]).
    pub jitter: [f32; 2],
    /// The history is invalid (first frame, a camera cut, a size change).
    pub reset: bool,
    pub camera: UpscaleCamera,
    /// Renderer frame number.
    pub frame: u64,
}

/// A temporal upscaler (DLSS, FSR, ...): implemented by the host, which owns the native side.
#[cfg(feature = "gpu")]
pub trait Upscaler {
    /// Writes the upscaled image into `inputs.output`; false = nothing was written (the
    /// renderer then stretches the render-size colour instead).
    fn upscale(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, inputs: &UpscaleInputs) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Camera;

    #[test]
    fn modes_and_jitter() {
        assert_eq!(UpscaleMode::Native.render_size((1920, 1080)), (1920, 1080));
        assert_eq!(UpscaleMode::Quality.render_size((3840, 2160)), (2560, 1440));
        assert_eq!(UpscaleMode::Performance.render_size((3840, 2160)), (1920, 1080));
        assert_eq!(UpscaleMode::UltraPerformance.render_size((3840, 2160)), (1280, 720));
        assert_eq!(jitter_phase_count(1920, 3840), 32);
        assert_eq!(jitter_phase_count(1920, 1920), 8);
        assert_eq!(halton(1, 2), 0.5);
        assert_eq!(halton(3, 3), 1.0 / 9.0);
        for f in 0..64 {
            let j = jitter(f, 16);
            assert!(j.iter().all(|v| (-0.5..0.5).contains(v)));
        }
        assert_eq!(jitter(0, 8), jitter(8, 8));
    }

    #[test]
    fn inverse_round_trip() {
        let m = [[2.0, 0.0, 0.0, 0.0], [0.0, 3.0, 0.0, 0.0], [0.5, -0.25, 0.0, 1.0], [0.0, 0.0, 10.0, 0.0]];
        let i = inverse4(&m).unwrap();
        let id = mul4(&m, &i);
        for r in 0..4 {
            for c in 0..4 {
                assert!((id[r][c] - IDENTITY4[r][c]).abs() < 1e-5);
            }
        }
    }

    /// The camera matrices put a point where `View::project` draws it.
    #[test]
    fn projection_matches_the_game() {
        let mut v = View::new(1280, 720);
        v.hor_plus = true;
        v.set_camera(&Camera { source: [100.0, 50.0, -30.0], target: [0.0, 40.0, 300.0], up: [0.0, 1.0, 0.0] });
        let cam = UpscaleCamera::new(&v, None);
        let w2c = world_to_camera(&v.v_matrix);
        let world = [37.0f32, 61.0, 250.0];
        let vp = crate::math::mtx_xfm(&v.v_matrix, world);
        let [sx, sy, _] = v.project(vp).unwrap();
        let c = mul4(&[[world[0], world[1], world[2], 1.0], [0.0; 4], [0.0; 4], [0.0; 4]], &mul4(&w2c, &cam.view_to_clip))[0];
        let (nx, ny, nz) = (c[0] / c[3], c[1] / c[3], c[2] / c[3]);
        let px = (nx + 1.0) / 2.0 * 1280.0 - 0.5;
        let py = (1.0 - ny) / 2.0 * 720.0 - 0.5;
        // `oozd` truncates the depth to an integer: allow for that.
        assert!((px - sx).abs() < 1.5 && (py - sy).abs() < 1.5, "{px},{py} vs {sx},{sy}");
        assert!((nz - DEPTH_NEAR / (vp[2] - 10.0)).abs() < 1e-5);
        // The forward axis looks at the target... from the source: the game swaps them.
        let fwd = cam.forward;
        assert!(fwd[2] > 0.5, "{fwd:?}");
    }
}
