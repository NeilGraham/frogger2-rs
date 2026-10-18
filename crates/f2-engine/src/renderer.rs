//! wgpu back end: rasterises the CPU-built TL triangle lists (`scene.rs`) with
//! the D3D7 render states MDX used, plus 2D quads, backdrops and text.

#[cfg(not(feature = "std"))]
use crate::fmath::FloatExt;
use crate::prelude::*;

use crate::anim::Actor;
use crate::font::Font;
use crate::scene::*;
use crate::textures::{TexHandle, TextureRegistry};
use f2_formats::objbank::Model;
use f2_formats::texture::Texture;
#[cfg(feature = "gpu")]
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct EngineError(pub String);
impl core::fmt::Display for EngineError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}
impl core::error::Error for EngineError {}
#[cfg(feature = "std")]
fn err(e: impl core::fmt::Display) -> EngineError {
    EngineError(e.to_string())
}

/// Instance, adapter, device and queue.
#[cfg(feature = "gpu")]
pub struct GpuContext {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

#[cfg(feature = "gpu")]
impl GpuContext {
    /// Creates a context with no surface (offscreen rendering). Honours `WGPU_BACKEND` etc.
    ///
    /// With feature `ps5-vulkan` and `F2_VK_RAW=1`, goes through the PS5 code path instead
    /// (`vkraw`: raw `vkGetInstanceProcAddr` from the system loader, instance handed to wgpu
    /// with `from_hal`), so host runs can check it renders identical frames.
    pub fn new_headless() -> Result<GpuContext, EngineError> {
        #[cfg(feature = "ps5-vulkan")]
        if std::env::var_os("F2_VK_RAW").is_some_and(|v| v == "1") {
            // SAFETY: loads the system Vulkan loader (desktop testing only).
            let entry = unsafe { ash::Entry::load() }.map_err(err)?;
            return Self::from_vulkan_entry(entry, &[]);
        }
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        Self::with_instance(instance, None)
    }

    /// Creates a context on `instance`, compatible with `surface` when given
    /// (the caller creates the surface from its window via `instance.create_surface`).
    pub fn with_instance(instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>) -> Result<GpuContext, EngineError> {
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            force_fallback_adapter: false,
            compatible_surface: surface,
            ..Default::default()
        }))
        .map_err(err)?;
        Self::with_adapter(instance, adapter)
    }

    /// A context on the adapter of `backends` whose name contains `name` (any case), else the
    /// high-performance one: no surface (a host that presents itself, e.g. the upscalers'
    /// Direct3D 12 swapchain).
    pub fn for_adapter_named(instance: wgpu::Instance, backends: wgpu::Backends, name: Option<&str>) -> Result<GpuContext, EngineError> {
        let adapters = pollster::block_on(instance.enumerate_adapters(backends));
        let want = name.map(|n| n.to_lowercase());
        let named = want.as_ref().and_then(|n| adapters.iter().position(|a| a.get_info().name.to_lowercase().contains(n.as_str())));
        let adapter = match named {
            Some(i) => adapters.into_iter().nth(i).unwrap(),
            None => pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
                ..Default::default()
            }))
            .map_err(err)?,
        };
        Self::with_adapter(instance, adapter)
    }

    fn with_adapter(instance: wgpu::Instance, adapter: wgpu::Adapter) -> Result<GpuContext, EngineError> {
        // Ray queries (the Path-Traced graphics, `crate::rt`) where the adapter has them;
        // `F2_NO_RAY_TRACING` leaves them off.
        let rt = adapter.features().contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY) && std::env::var_os("F2_NO_RAY_TRACING").is_none();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("f2-engine"),
            required_features: if rt { wgpu::Features::EXPERIMENTAL_RAY_QUERY } else { wgpu::Features::empty() },
            required_limits: {
                let l = wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits());
                if rt {
                    l.using_acceleration_structure_values(adapter.limits())
                } else {
                    l
                }
            },
            // SAFETY: ray queries are experimental in wgpu; the renderer only uses them on the
            // Path-Traced graphics setting.
            experimental_features: if rt { unsafe { wgpu::ExperimentalFeatures::enabled() } } else { wgpu::ExperimentalFeatures::disabled() },
            ..Default::default()
        }))
        .map_err(err)?;
        Ok(GpuContext { instance, adapter, device, queue })
    }
}

/// A configured window surface. Create the `wgpu::Surface` from any
/// raw-window-handle provider with `ctx.instance.create_surface(window)`.
#[cfg(feature = "gpu")]
pub struct WindowSurface {
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
}

#[cfg(feature = "gpu")]
impl WindowSurface {
    pub fn new(ctx: &GpuContext, surface: wgpu::Surface<'static>, width: u32, height: u32) -> Result<WindowSurface, EngineError> {
        let caps = surface.get_capabilities(&ctx.adapter);
        // The original renders in gamma space: prefer a non-sRGB format.
        let format =
            caps.formats.iter().copied().find(|f| !f.is_srgb()).or_else(|| caps.formats.first().copied()).ok_or_else(|| err("surface has no formats"))?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&ctx.device, &config);
        Ok(WindowSurface { surface, config })
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(device, &self.config);
    }
}

/// Blend modes used by MDX (`xluSemiRS`, `xluAddRS`, `xluSubRS`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Blend {
    /// SRCALPHA / INVSRCALPHA.
    Semi,
    /// SRCALPHA / ONE.
    Additive,
    /// ZERO / INVSRCCOLOR.
    Subtractive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
/// Depth state of a [`DrawCmd`] (MDX `normalZRS` / `xluZRS` / `noZRS`).
pub enum DepthMode {
    /// `normalZRS`: LESSEQUAL, write.
    TestWrite,
    /// `xluZRS`: LESSEQUAL, no write.
    Test,
    /// `noZRS`.
    Off,
}

/// A 2D textured quad in screen pixels (`DrawTexturedRect`/`DrawSpriteOverlay`).
#[derive(Clone, Copy, Debug)]
pub struct Quad2D {
    /// Corner positions (pixels, y down), in order TL, TR, BR, BL.
    pub pos: [[f32; 2]; 4],
    /// u1, v1, u2, v2.
    pub uv: [f32; 4],
    pub tex: Option<TexHandle>,
    pub color: [u8; 4],
    pub blend: Blend,
}

impl Quad2D {
    /// The two triangles `Renderer::build` makes of the quad (TL TR BR, BR BL TL).
    pub fn tl_vertices(&self) -> [TlVertex; 6] {
        let q = self;
        let col = q.color.map(|c| c as f32 / 255.0);
        let uvs = [[q.uv[0], q.uv[1]], [q.uv[2], q.uv[1]], [q.uv[2], q.uv[3]], [q.uv[0], q.uv[3]]];
        let v = |i: usize| TlVertex { sx: q.pos[i][0], sy: q.pos[i][1], sz: 0.0, rhw: 1.0, color: col, uv: uvs[i], fog: 1.0, alpha_ref: ALPHA_NONZERO };
        [v(0), v(1), v(2), v(2), v(3), v(0)]
    }

    #[allow(clippy::too_many_arguments)]
    pub fn rect(x: f32, y: f32, w: f32, h: f32, uv: [f32; 4], tex: Option<TexHandle>, color: [u8; 4], blend: Blend) -> Quad2D {
        Quad2D { pos: [[x, y], [x + w, y], [x + w, y + h], [x, y + h]], uv, tex, color, blend }
    }
}

/// Handle of a font registered with [`Renderer::add_font`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FontId(pub usize);

#[cfg(feature = "gpu")]
struct GpuImage {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

/// Size of the original's virtual overlay space (`OVERLAY_X = rXRes/4096`).
pub const OVERLAY_SPACE: f32 = 4096.0;

/// Alpha-test references: `tightAlphaCmpRS` (> 0x64) and `normalAlphaCmpRS` (!= 0).
const ALPHA_TIGHT: f32 = 100.0;
pub const ALPHA_NONZERO: f32 = 0.0;

/// The 3D passes of `DrawAllFrames` in order with their states: frame, blend, depth, wrap,
/// alpha reference, depth-sorted per [`RenderOptions::xlu_sort`].
const PASSES: [(Frame, Blend, DepthMode, bool, f32, bool); 7] = [
    (Frame::Normal, Blend::Semi, DepthMode::TestWrite, false, ALPHA_TIGHT, false),
    (Frame::Wrap, Blend::Semi, DepthMode::TestWrite, true, ALPHA_TIGHT, false),
    (Frame::Xlu, Blend::Semi, DepthMode::Test, true, ALPHA_NONZERO, true),
    (Frame::Additive, Blend::Additive, DepthMode::Test, true, ALPHA_NONZERO, true),
    // DrawLoop's post-DrawAllFrames batches (sprites, special FX): submission order.
    (Frame::Late, Blend::Semi, DepthMode::Test, false, ALPHA_NONZERO, false),
    (Frame::LateAdditive, Blend::Additive, DepthMode::Test, false, ALPHA_NONZERO, false),
    (Frame::Overlay, Blend::Semi, DepthMode::Off, false, ALPHA_NONZERO, false),
];

impl XluSort {
    /// Orders a translucent pass's triangles (`build_segments`).
    fn sort(self, tris: &mut [Tri]) {
        if self == XluSort::BackToFront {
            let key = |t: &Tri| t.v[0].sz + t.v[1].sz + t.v[2].sz;
            tris.sort_by(|a, b| key(b).total_cmp(&key(a)));
        }
    }
}

/// Render state of a 3D pass ([`Renderer::frame_view`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PassState {
    pub blend: Blend,
    pub depth: DepthMode,
    pub wrap: bool,
    pub alpha_ref: f32,
}

/// [`Renderer::frame_view`].
pub struct FrameView<'a> {
    pub viewport_clear: Option<Quad2D>,
    pub backdrops: &'a [Quad2D],
    pub passes: [(PassState, &'a [Tri]); 7],
    pub overlays: &'a [Quad2D],
    pub white: TexHandle,
}

/// One batch of the draw list built by [`Renderer::build`]: `count` vertices (whole
/// triangles) from `first`, drawn with one render state. Back ends (wgpu, `soft`, `gles`,
/// external ones such as the N64 RDP) consume these in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawCmd {
    pub blend: Blend,
    pub depth: DepthMode,
    /// Texture (`textures.images[tex.0]`); untextured geometry uses a 1x1 white image.
    pub tex: TexHandle,
    /// Texture coordinates wrap (else clamp).
    pub wrap: bool,
    /// 3D frame triangle (scissored to the viewport when it is clamped).
    pub scene: bool,
    pub first: u32,
    pub count: u32,
}

/// Sampler variant: (wrap, linear, anisotropy).
#[cfg(feature = "gpu")]
type SamplerKey = (bool, bool, u16);

/// Largest texture that gets a mip chain when anisotropic filtering is on (bank textures
/// are <= 256; screen-sized backdrops and video frames are drawn ~1:1 and skip it).
#[cfg(feature = "gpu")]
const MIP_MAX_SIZE: u32 = 512;

/// Box-filtered mip chain (alpha-weighted colour, so colour-keyed texels do not bleed
/// black into their neighbours). Returns levels 1.. (level 0 is the image itself).
#[cfg(feature = "gpu")]
fn build_mips(w: u32, h: u32, rgba: &[u8]) -> Vec<(u32, u32, Vec<u8>)> {
    let mut out = Vec::new();
    let (mut pw, mut ph) = (w, h);
    let mut prev = rgba.to_vec();
    while pw > 1 || ph > 1 {
        let (nw, nh) = ((pw / 2).max(1), (ph / 2).max(1));
        let mut next = vec![0u8; (nw * nh * 4) as usize];
        for y in 0..nh {
            for x in 0..nw {
                let mut acc = [0u32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let sx = (x * 2 + dx).min(pw - 1);
                    let sy = (y * 2 + dy).min(ph - 1);
                    let i = ((sy * pw + sx) * 4) as usize;
                    let a = prev[i + 3] as u32;
                    for c in 0..3 {
                        acc[c] += prev[i + c] as u32 * a;
                    }
                    acc[3] += a;
                }
                let o = ((y * nw + x) * 4) as usize;
                for c in 0..3 {
                    next[o + c] = (acc[c] + acc[3] / 2).checked_div(acc[3]).unwrap_or(0) as u8;
                }
                next[o + 3] = ((acc[3] + 2) / 4) as u8;
            }
        }
        out.push((nw, nh, next.clone()));
        prev = next;
        (pw, ph) = (nw, nh);
    }
    out
}

/// Per-byte atlas texture coordinates of a high-resolution font.
type GlyphUvs = Vec<Option<[f32; 4]>>;

/// An RGBA8 frame read back from the GPU.
#[derive(Clone, Debug)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl RgbaImage {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        self.pixels[i..i + 4].try_into().unwrap()
    }

    #[cfg(feature = "std")]
    pub fn write_png(&self, path: impl AsRef<std::path::Path>) -> Result<(), EngineError> {
        let f = std::fs::File::create(path).map_err(err)?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(f), self.width, self.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(err)?;
        w.write_image_data(&self.pixels).map_err(err)?;
        Ok(())
    }
}

#[cfg(feature = "gpu")]
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Size of the shader's uniform block `U`.
#[cfg(feature = "gpu")]
const UNIFORM_SIZE: u64 = 48;

/// The MDX replacement renderer.
///
/// Per frame: [`begin_frame`](Self::begin_frame), set the camera, queue
/// backdrops / actors / landscape / overlays / text, then
/// [`render`](Self::render) (to any texture view) or
/// [`render_to_image`](Self::render_to_image) (headless).
pub struct Renderer {
    #[cfg(feature = "gpu")]
    pub device: wgpu::Device,
    #[cfg(feature = "gpu")]
    pub queue: wgpu::Queue,
    #[cfg(feature = "gpu")]
    pub format: wgpu::TextureFormat,
    /// Draw over what the target already holds instead of clearing it (a host's UI pass on
    /// top of the presented frame). Only honoured without MSAA and render scaling.
    #[cfg(feature = "gpu")]
    pub overlay: bool,
    pub textures: TextureRegistry,
    pub view: View,
    pub options: RenderOptions,
    /// `SPRITECLIPLEFT/TOP/RIGHT/BOTTOM` (internal pixels) applied to unrotated sprite
    /// overlays (`DrawSpriteOverlay`); `None` = no clipping. Set per frame by the host.
    pub sprite_clip: Option<[f32; 4]>,
    /// When set, [`draw_actor`](Self::draw_actor) / [`draw_landscape`](Self::draw_landscape)
    /// record [`MeshDraw`]s here instead of transforming vertices (a back end with its own
    /// T&L, e.g. the N64 RSP renderer); everything else (sprites, effects, 2D) still goes
    /// through the frames. Cleared by [`begin_frame`](Self::begin_frame).
    pub mesh_sink: Option<MeshSink>,
    /// Models (`Model::id`) transformed on the CPU even with a mesh sink (e.g. meshes whose
    /// textures the external back end cannot sample).
    pub cpu_models: Vec<u32>,
    frames: Frames,
    backdrops: Vec<Quad2D>,
    overlays: Vec<Quad2D>,
    /// Fonts: metrics, image, and for a high-resolution font the per-byte atlas UVs.
    /// [`set_button_icons`](Self::set_button_icons): the icon image and its size.
    icons: Option<(TexHandle, u32)>,
    /// (metrics, image, atlas UVs of a high-resolution font, size multiplier)
    fonts: Vec<(Font, TexHandle, Option<GlyphUvs>, f32)>,
    white: TexHandle,
    #[cfg(feature = "gpu")]
    gpu_images: Vec<GpuImage>,
    #[cfg(feature = "gpu")]
    bind_groups: HashMap<(u32, SamplerKey), wgpu::BindGroup>,
    #[cfg(feature = "gpu")]
    tex_layout: wgpu::BindGroupLayout,
    #[cfg(feature = "gpu")]
    uniform_buf: wgpu::Buffer,
    #[cfg(feature = "gpu")]
    uniform_bg: wgpu::BindGroup,
    #[cfg(feature = "gpu")]
    pipeline_layout: wgpu::PipelineLayout,
    #[cfg(feature = "gpu")]
    shader: wgpu::ShaderModule,
    #[cfg(feature = "gpu")]
    pipelines: HashMap<PipeKey, wgpu::RenderPipeline>,
    #[cfg(feature = "gpu")]
    samplers: HashMap<SamplerKey, wgpu::Sampler>,
    /// Depth buffer (view, width, height, samples).
    #[cfg(feature = "gpu")]
    depth: Option<(wgpu::TextureView, u32, u32, u32)>,
    /// Internal colour target for supersampling (view, width, height).
    #[cfg(feature = "gpu")]
    internal: Option<(wgpu::TextureView, u32, u32)>,
    /// MSAA colour target (view, width, height, samples).
    #[cfg(feature = "gpu")]
    msaa_target: Option<(wgpu::TextureView, u32, u32, u32)>,
    #[cfg(feature = "gpu")]
    blit: Option<(wgpu::RenderPipeline, wgpu::Sampler)>,
    #[cfg(feature = "gpu")]
    vbuf: Option<wgpu::Buffer>,
    /// Size passed to [`begin_frame`](Self::begin_frame) (the target); `view` holds the
    /// internal (render-scaled) size.
    output: (u32, u32),
    /// Whether the GPU images were uploaded with mip chains.
    #[cfg(feature = "gpu")]
    gpu_mips: bool,
    /// Motion vectors ([`RenderOptions::motion_vectors`]): last frame's view, when this frame
    /// can be compared with it (set up by [`begin_frame`](Self::begin_frame)).
    motion: Option<Motion>,
    /// Frame counter ([`begin_frame`](Self::begin_frame)).
    frame_no: u32,
    /// This frame cannot use the upscaler's history (first frame, camera cut, size change).
    history_reset: bool,
    /// Upscaling: the 3D render size the host wants instead of `render_scale` (the
    /// upscaler's input size), set before [`begin_frame`](Self::begin_frame).
    pub upscale_render: Option<(u32, u32)>,
    /// Upscaling: this frame's sub-pixel jitter in render pixels ([`crate::upscale::jitter`]).
    pub jitter: [f32; 2],
    /// Upscaling debug view: shows an upscaler input instead of the picture.
    pub upscale_debug: UpscaleDebug,
    /// Upscaling: depth-writing draws (and their triangles) that wrote motion vectors and
    /// depth last frame (diagnostics).
    pub upscale_draws: (u32, u32),
    #[cfg(feature = "gpu")]
    up: Option<UpTargets>,
    /// [`RenderOptions::gpu_meshes`]: this frame's GPU mesh draws and the GPU side.
    #[cfg(feature = "gpu")]
    gpu_sink: crate::gpu_mesh::Sink,
    #[cfg(feature = "gpu")]
    gpu_mesh: Option<crate::gpu_mesh::Gpu>,
    /// Dynamic lights of the GPU meshes (`crate::lighting`; `None` = the baked vertex colours
    /// only). Used only with [`RenderOptions::gpu_meshes`].
    pub lighting: Option<crate::lighting::Lighting>,
    /// This frame's point lights from emissive sprites ([`light_sprite`](Self::light_sprite)).
    sprite_lights: Vec<crate::lighting::PointLight>,
    /// Emissive flag per texture CRC (sprites).
    emissive_crc: alloc::collections::BTreeMap<u32, bool>,
    /// [`Lighting::water`](crate::Lighting): the water simulation's clock (seconds; the host
    /// advances it with the game, e.g. ticks / 60 plus the interpolation fraction).
    pub water_clock: f64,
    /// Disturbances waiting for the next simulation step ([`water_splash`](Self::water_splash),
    /// moving actors).
    #[cfg(feature = "gpu")]
    splashes: Vec<crate::water::Splash>,
    /// The clock (×60, whole) at which moving actors last made waves.
    #[cfg(feature = "gpu")]
    wake_tick: i64,
    #[cfg(feature = "gpu")]
    wake_frame: bool,
    #[cfg(feature = "gpu")]
    mv_buf: Option<wgpu::Buffer>,
    /// Uniforms of the HUD pass of [`render_upscaled`](Self::render_upscaled) (no jitter).
    #[cfg(feature = "gpu")]
    ui_uniform: Option<(wgpu::Buffer, wgpu::BindGroup)>,
}

/// Pipeline variant: blend, depth, samples, colour format, with the upscaler targets.
#[cfg(feature = "gpu")]
type PipeKey = (Blend, DepthMode, u32, wgpu::TextureFormat, bool);

/// Formats of the upscaler inputs ([`crate::upscale`]).
#[cfg(feature = "gpu")]
pub const UPSCALE_COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
#[cfg(feature = "gpu")]
const MV_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg16Float;
#[cfg(feature = "gpu")]
const UP_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;

/// Textures of [`Renderer::render_upscaled`].
#[cfg(feature = "gpu")]
struct UpTargets {
    render: (u32, u32),
    output: (u32, u32),
    color: (wgpu::Texture, wgpu::TextureView),
    motion: (wgpu::Texture, wgpu::TextureView),
    depth: (wgpu::Texture, wgpu::TextureView),
    out: (wgpu::Texture, wgpu::TextureView),
    /// Depth-stencil of the HUD pass (its pipelines have a depth state).
    ui_depth: wgpu::TextureView,
}

/// [`Renderer::upscale_debug`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UpscaleDebug {
    #[default]
    Off,
    /// Motion vectors: grey = still, red/green = x/y motion (±20 render pixels saturate).
    Motion,
    /// The reversed depth (bright = near, black = nothing drawn).
    Depth,
}

/// [`UpscaleDebug`] views of the motion vector and depth targets.
#[cfg(feature = "gpu")]
const DEBUG_SHADER: &str = r#"
@group(0) @binding(0) var mvt: texture_2d<f32>;
@group(0) @binding(1) var dt: texture_2d<f32>;
struct VOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> VOut {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: VOut;
    o.pos = vec4<f32>(p * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    o.uv = p;
    return o;
}
fn texel(t: texture_2d<f32>, uv: vec2<f32>) -> vec4<f32> {
    let s = vec2<f32>(textureDimensions(t));
    return textureLoad(t, vec2<i32>(clamp(uv * s, vec2<f32>(0.0), s - 1.0)), 0);
}
@fragment fn fs_motion(i: VOut) -> @location(0) vec4<f32> {
    let m = texel(mvt, i.uv).xy / 20.0;
    return vec4<f32>(clamp(0.5 + m.x, 0.0, 1.0), clamp(0.5 + m.y, 0.0, 1.0), 0.5, 1.0);
}
@fragment fn fs_depth(i: VOut) -> @location(0) vec4<f32> {
    let d = sqrt(texel(dt, i.uv).r);
    return vec4<f32>(d, d, d, 1.0);
}
"#;

/// Camera moves bigger than this between two frames are cuts (motion vectors and the
/// upscaler history are dropped): render units, ~4 tiles.
const CUT_DISTANCE: f32 = 250.0;

#[cfg(feature = "gpu")]
const SHADER: &str = r#"
struct U { size: vec2<f32>, persp: f32, fog_on: f32, fog_color: vec4<f32>, jitter: vec2<f32>, depth_near: f32, pad: f32 };
@group(0) @binding(0) var<uniform> u: U;
@group(1) @binding(0) var t: texture_2d<f32>;
@group(1) @binding(1) var s: sampler;

struct VIn {
    @location(0) pos: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) fog: f32,
    @location(4) aref: f32,
};
struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) fog: f32,
    @location(3) @interpolate(flat) aref: f32,
    @location(4) mv: vec2<f32>,
    @location(5) @interpolate(linear) rhw: f32,
};

fn vert(i: VIn) -> VOut {
    var o: VOut;
    // D3D7 TL pixel centres are at integer coordinates. `jitter`: the upscalers' sub-pixel
    // camera jitter (render pixels; zero otherwise).
    let p = i.pos.xy + vec2<f32>(0.5, 0.5) + u.jitter;
    let ndc = vec2<f32>(p.x / u.size.x * 2.0 - 1.0, 1.0 - p.y / u.size.y * 2.0);
    var w = 1.0;
    if (u.persp > 0.5 && i.pos.w > 0.0) { w = 1.0 / i.pos.w; }
    o.pos = vec4<f32>(ndc * w, clamp(i.pos.z, 0.0, 1.0) * w, w);
    o.color = i.color;
    o.uv = i.uv;
    o.fog = i.fog;
    o.aref = i.aref;
    o.mv = vec2<f32>(0.0, 0.0);
    o.rhw = i.pos.w;
    return o;
}

@vertex fn vs(i: VIn) -> VOut {
    return vert(i);
}

// Upscaling: the vertex's screen motion (previous position minus current, render pixels).
@vertex fn vs_mrt(i: VIn, @location(5) mv: vec2<f32>) -> VOut {
    var o = vert(i);
    o.mv = mv;
    return o;
}

fn shade(i: VOut) -> vec4<f32> {
    var c = textureSample(t, s, i.uv) * i.color;
    if (floor(c.a * 255.0 + 0.5) <= i.aref) { discard; }
    if (u.fog_on > 0.5) { c = vec4<f32>(mix(u.fog_color.rgb, c.rgb, i.fog), c.a); }
    return c;
}

@fragment fn fs(i: VOut) -> @location(0) vec4<f32> {
    return shade(i);
}

// Upscaling: colour, motion vector and reversed infinite depth (`near / z`; `rhw = 1 / z`
// is linear in screen space, so it interpolates exactly).
struct FOut { @location(0) c: vec4<f32>, @location(1) mv: vec2<f32>, @location(2) d: f32 };
@fragment fn fs_mrt(i: VOut) -> FOut {
    var o: FOut;
    o.c = shade(i);
    o.mv = i.mv;
    o.d = clamp(u.depth_near * i.rhw, 0.0, 1.0);
    return o;
}
"#;

/// Supersampling resolve: draws the internal target over the output with bilinear
/// filtering (a 2x2 box filter at render scale 2).
#[cfg(feature = "gpu")]
const BLIT_SHADER: &str = r#"
@group(0) @binding(0) var t: texture_2d<f32>;
@group(0) @binding(1) var s: sampler;
struct VOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> VOut {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: VOut;
    o.pos = vec4<f32>(p * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    o.uv = p;
    return o;
}
@fragment fn fs(i: VOut) -> @location(0) vec4<f32> {
    return textureSampleLevel(t, s, i.uv, 0.0);
}
"#;

impl Renderer {
    #[cfg(feature = "gpu")]
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Renderer {
        let device = device.clone();
        let queue = queue.clone();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("mdx"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let ulayout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniforms"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uniforms"),
            layout: &ulayout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform_buf.as_entire_binding() }],
        });
        let ui_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hud uniforms"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let ui_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hud uniforms"),
            layout: &ulayout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: ui_buf.as_entire_binding() }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mdx"),
            bind_group_layouts: &[Some(&ulayout), Some(&tex_layout)],
            immediate_size: 0,
        });
        let mut r = Renderer {
            device,
            queue,
            format,
            overlay: false,
            textures: TextureRegistry::new(),
            view: View::new(640, 480),
            options: RenderOptions::default(),
            frames: Frames::default(),
            backdrops: Vec::new(),
            overlays: Vec::new(),
            fonts: Vec::new(),
            icons: None,
            white: TexHandle(0),
            gpu_images: Vec::new(),
            bind_groups: HashMap::new(),
            tex_layout,
            uniform_buf,
            uniform_bg,
            motion: None,
            frame_no: 0,
            history_reset: true,
            upscale_render: None,
            jitter: [0.0; 2],
            upscale_debug: UpscaleDebug::Off,
            upscale_draws: (0, 0),
            up: None,
            gpu_sink: Default::default(),
            gpu_mesh: None,
            lighting: None,
            sprite_lights: Vec::new(),
            emissive_crc: alloc::collections::BTreeMap::new(),
            water_clock: 0.0,
            splashes: Vec::new(),
            wake_tick: i64::MIN,
            wake_frame: false,
            mv_buf: None,
            ui_uniform: Some((ui_buf, ui_bg)),
            pipeline_layout,
            shader,
            pipelines: HashMap::new(),
            samplers: HashMap::new(),
            depth: None,
            internal: None,
            msaa_target: None,
            blit: None,
            vbuf: None,
            output: (640, 480),
            gpu_mips: false,
            sprite_clip: None,
            mesh_sink: None,
            cpu_models: Vec::new(),
        };
        r.white = r.textures.add_image(1, 1, vec![255; 4]);
        r
    }

    /// A renderer without a GPU (build without the `gpu` feature): frames are drawn
    /// on the CPU by [`render_soft`](Self::render_soft).
    #[cfg(not(feature = "gpu"))]
    pub fn new_soft() -> Renderer {
        let mut r = Renderer {
            textures: TextureRegistry::new(),
            view: View::new(640, 480),
            options: RenderOptions::default(),
            frames: Frames::default(),
            backdrops: Vec::new(),
            overlays: Vec::new(),
            fonts: Vec::new(),
            icons: None,
            white: TexHandle(0),
            output: (640, 480),
            sprite_clip: None,
            lighting: None,
            sprite_lights: Vec::new(),
            emissive_crc: alloc::collections::BTreeMap::new(),
            water_clock: 0.0,
            mesh_sink: None,
            cpu_models: Vec::new(),
            motion: None,
            frame_no: 0,
            history_reset: true,
            upscale_render: None,
            jitter: [0.0; 2],
            upscale_debug: UpscaleDebug::Off,
            upscale_draws: (0, 0),
        };
        r.white = r.textures.add_image(1, 1, vec![255; 4]);
        r
    }

    /// Renders the queued frame on the CPU (size = `begin_frame` size; the render scale
    /// is ignored) and returns it.
    #[cfg(not(feature = "gpu"))]
    pub fn render_to_image(&mut self, clear: [f64; 4]) -> Result<RgbaImage, EngineError> {
        let mut t = crate::soft::SoftTarget::default();
        self.render_soft(&mut t, clear, 1);
        let pixels = t.pixels.iter().flat_map(|p| p.to_le_bytes()).collect();
        Ok(RgbaImage { width: t.width, height: t.height, pixels })
    }

    // ---------------------------------------------------------------- textures

    /// Loads a texture bank (`LoadTexBankFile`).
    pub fn load_tex_bank(&mut self, bank: &f2_formats::texture::TexBank) {
        self.textures.load_bank(bank);
    }

    /// Registers a single image (e.g. a 640×480 backdrop) and returns its handle.
    pub fn add_texture(&mut self, t: &Texture) -> TexHandle {
        self.textures.add_texture(t)
    }

    /// Registers a font and uploads its glyph strip. The pixels move into the texture
    /// registry (the kept `Font` only needs its metrics).
    pub fn add_font(&mut self, mut font: Font) -> FontId {
        let rgba = core::mem::take(&mut font.rgba);
        let h = self.textures.add_image(font.width, font.height, rgba);
        self.fonts.push((font, h, None, 1.0));
        FontId(self.fonts.len() - 1)
    }

    /// Registers a font drawn from a high-resolution atlas ([`HiRes`], the modern fonts)
    /// with `font`'s metrics. The atlas is first resampled to `scale` atlas pixels per
    /// strip pixel (pick about the strip's on-screen magnification so it is sampled ~1:1).
    pub fn add_font_hires(&mut self, mut font: Font, hires: &crate::HiRes, scale: f32) -> FontId {
        font.rgba = Vec::new();
        let hr = hires.resampled(scale);
        let uvs = (0..=255u8).map(|c| hr.uv(c)).collect();
        let h = self.textures.add_image(hr.width, hr.height, hr.rgba);
        self.fonts.push((font, h, Some(uvs), 1.0));
        FontId(self.fonts.len() - 1)
    }

    /// [`add_font_hires`](Self::add_font_hires) for an atlas a back end already holds
    /// (`hires.rgba` is ignored; no resampling): a placeholder image of the atlas size.
    pub fn add_font_hires_placeholder(&mut self, mut font: Font, hires: &crate::HiRes) -> FontId {
        font.rgba = Vec::new();
        let uvs = (0..=255u8).map(|c| hires.uv(c)).collect();
        let h = self.textures.add_placeholder(hires.width, hires.height);
        self.fonts.push((font, h, Some(uvs), 1.0));
        FontId(self.fonts.len() - 1)
    }

    /// Makes the button-icon glyphs ([`font::ICONS`](crate::font::ICONS)) drawable, `px`
    /// pixels square (pick about the largest on-screen text height). Rebuilds the image only
    /// when the size changes.
    pub fn set_button_icons(&mut self, px: u32) {
        if self.icons.is_some_and(|(_, p)| p == px) {
            return;
        }
        let (w, h, rgba) = crate::font::button_icons(px);
        let t = self.textures.add_image(w, h, rgba);
        self.icons = Some((t, px));
    }

    /// Draws `font` `mul` times larger than the game asks (centred on the same spot; for
    /// low-resolution outputs, e.g. the N64's 240 lines). Layout metrics are unchanged.
    pub fn set_font_size_mul(&mut self, font: FontId, mul: f32) {
        self.fonts[font.0].3 = mul;
    }

    /// Adds a font whose image a back end already holds (no pixels: a placeholder image of
    /// the font's size, not queued for upload). `font.rgba` is ignored.
    pub fn add_font_placeholder(&mut self, mut font: Font) -> FontId {
        font.rgba = Vec::new();
        let h = self.textures.add_placeholder(font.width, font.height);
        self.fonts.push((font, h, None, 1.0));
        FontId(self.fonts.len() - 1)
    }

    pub fn font(&self, id: FontId) -> &Font {
        &self.fonts[id.0].0
    }

    #[cfg(feature = "gpu")]
    fn upload_dirty(&mut self) {
        let mips = self.options.linear_filter && self.options.anisotropy > 0;
        if mips != self.gpu_mips {
            // Re-upload everything with (or without) mip chains, in slot order.
            self.gpu_mips = mips;
            self.textures.dirty = (0..self.textures.images.len() as u32).collect();
        }
        for i in core::mem::take(&mut self.textures.dirty) {
            let img = &self.textures.images[i as usize];
            let size = wgpu::Extent3d { width: img.width, height: img.height, depth_or_array_layers: 1 };
            let chain = if mips && img.width.max(img.height) <= MIP_MAX_SIZE && img.rgba.len() as u32 == img.width * img.height * 4 {
                build_mips(img.width, img.height, &img.rgba)
            } else {
                Vec::new()
            };
            let idx = i as usize;
            // Same size and mip count (e.g. the next video frame): write the pixels into the
            // existing texture, keeping its view and bind groups.
            let reuse = self.gpu_images.get(idx).map(|g| g.texture.clone()).filter(|t| t.size() == size && t.mip_level_count() == 1 + chain.len() as u32);
            let tex = reuse.clone().unwrap_or_else(|| {
                self.device.create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size,
                    mip_level_count: 1 + chain.len() as u32,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                })
            });
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                &img.rgba,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(img.width * 4), rows_per_image: Some(img.height) },
                size,
            );
            for (level, (w, h, data)) in chain.iter().enumerate() {
                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: level as u32 + 1, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                    data,
                    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(*h) },
                    wgpu::Extent3d { width: *w, height: *h, depth_or_array_layers: 1 },
                );
            }
            if reuse.is_some() {
                continue;
            }
            let view = tex.create_view(&Default::default());
            let gi = GpuImage { texture: tex, view };
            if idx < self.gpu_images.len() {
                self.gpu_images[idx] = gi;
                self.bind_groups.retain(|k, _| k.0 != i);
            } else {
                // Images are appended in order, so this is the next slot.
                debug_assert_eq!(idx, self.gpu_images.len());
                self.gpu_images.push(gi);
            }
        }
    }

    #[cfg(feature = "gpu")]
    fn sampler_key(&self, wrap: bool) -> SamplerKey {
        let linear = self.options.linear_filter;
        (wrap, linear, if linear { self.options.anisotropy.min(16) } else { 0 })
    }

    #[cfg(feature = "gpu")]
    fn sampler(&mut self, key: SamplerKey) -> &wgpu::Sampler {
        let device = &self.device;
        self.samplers.entry(key).or_insert_with(|| {
            let (wrap, linear, aniso) = key;
            let am = if wrap { wgpu::AddressMode::Repeat } else { wgpu::AddressMode::ClampToEdge };
            let f = if linear { wgpu::FilterMode::Linear } else { wgpu::FilterMode::Nearest };
            let mut d = wgpu::SamplerDescriptor {
                label: None,
                address_mode_u: am,
                address_mode_v: am,
                address_mode_w: am,
                mag_filter: f,
                min_filter: f,
                ..Default::default()
            };
            if aniso > 0 {
                // Trilinear (+ anisotropic); retail has no mipmaps (`aniso == 0`).
                d.mipmap_filter = wgpu::MipmapFilterMode::Linear;
                d.anisotropy_clamp = aniso.max(1);
            }
            device.create_sampler(&d)
        })
    }

    #[cfg(feature = "gpu")]
    fn bind_group(&mut self, tex: TexHandle, wrap: bool) -> &wgpu::BindGroup {
        let skey = self.sampler_key(wrap);
        let key = (tex.0, skey);
        if !self.bind_groups.contains_key(&key) {
            self.sampler(skey);
            let s = &self.samplers[&skey];
            let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.tex_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&self.gpu_images[tex.0 as usize].view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(s) },
                ],
            });
            self.bind_groups.insert(key, bg);
        }
        &self.bind_groups[&key]
    }

    #[cfg(feature = "gpu")]
    fn pipeline(&mut self, blend: Blend, depth: DepthMode, samples: u32) -> &wgpu::RenderPipeline {
        self.pipeline_for((blend, depth, samples, self.format, false))
    }

    /// A pipeline for `key`; with `key.4` (upscaling) it also writes the motion vector and
    /// depth targets, in the depth-writing (opaque) passes only.
    #[cfg(feature = "gpu")]
    fn pipeline_for(&mut self, key: PipeKey) -> &wgpu::RenderPipeline {
        let (blend, depth, samples, format, mrt) = key;
        if !self.pipelines.contains_key(&key) {
            let comp = |src, dst| wgpu::BlendComponent { src_factor: src, dst_factor: dst, operation: wgpu::BlendOperation::Add };
            use wgpu::BlendFactor as F;
            let state = match blend {
                Blend::Semi => wgpu::BlendState { color: comp(F::SrcAlpha, F::OneMinusSrcAlpha), alpha: comp(F::One, F::OneMinusSrcAlpha) },
                Blend::Additive => wgpu::BlendState { color: comp(F::SrcAlpha, F::One), alpha: comp(F::Zero, F::One) },
                Blend::Subtractive => wgpu::BlendState { color: comp(F::Zero, F::OneMinusSrc), alpha: comp(F::Zero, F::One) },
            };
            let (write, cmp) = match depth {
                DepthMode::TestWrite => (true, wgpu::CompareFunction::LessEqual),
                DepthMode::Test => (false, wgpu::CompareFunction::LessEqual),
                DepthMode::Off => (false, wgpu::CompareFunction::Always),
            };
            let attrs = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x2, 3 => Float32, 4 => Float32];
            let mv_attrs = wgpu::vertex_attr_array![5 => Float32x2];
            let tl =
                wgpu::VertexBufferLayout { array_stride: core::mem::size_of::<TlVertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attrs };
            let mvl = wgpu::VertexBufferLayout { array_stride: 8, step_mode: wgpu::VertexStepMode::Vertex, attributes: &mv_attrs };
            let buffers = [Some(tl), Some(mvl)];
            let aux = if write { wgpu::ColorWrites::ALL } else { wgpu::ColorWrites::empty() };
            let targets = [
                Some(wgpu::ColorTargetState { format, blend: Some(state), write_mask: wgpu::ColorWrites::ALL }),
                Some(wgpu::ColorTargetState { format: MV_FORMAT, blend: None, write_mask: aux }),
                Some(wgpu::ColorTargetState { format: UP_DEPTH_FORMAT, blend: None, write_mask: aux }),
            ];
            let n = if mrt { 3 } else { 1 };
            let p = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("mdx"),
                layout: Some(&self.pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &self.shader,
                    entry_point: Some(if mrt { "vs_mrt" } else { "vs" }),
                    compilation_options: Default::default(),
                    buffers: &buffers[..if mrt { 2 } else { 1 }],
                },
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(write),
                    depth_compare: Some(cmp),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
                fragment: Some(wgpu::FragmentState {
                    module: &self.shader,
                    entry_point: Some(if mrt { "fs_mrt" } else { "fs" }),
                    compilation_options: Default::default(),
                    targets: &targets[..n],
                }),
                multiview_mask: None,
                cache: None,
            });
            self.pipelines.insert(key, p);
        }
        &self.pipelines[&key]
    }

    // ---------------------------------------------------------------- frame building

    /// Starts a frame of `width`×`height` pixels: clears all poly lists
    /// (`BlankAllFrames`) and sets the viewport (`InitOneOverTable`).
    ///
    /// Applies the frame-level [`RenderOptions`]: the internal size is
    /// `width`×`height` × `render_scale`, Hor+ and the ultrawide clamp set up
    /// [`View::hor_plus`] / [`View::viewport`]. 2D positions passed to
    /// [`draw_quad`](Self::draw_quad) / [`draw_text`](Self::draw_text) stay in
    /// `width`×`height` pixels.
    pub fn begin_frame(&mut self, width: u32, height: u32) {
        let (w, h) = (width.max(1), height.max(1));
        self.output = (w, h);
        self.frame_no = self.frame_no.wrapping_add(1);
        // Still the last frame's view (camera included): what motion vectors compare with.
        let last = self.options.motion_vectors.then(|| self.view.clone());
        let s = self.render_scale();
        let (iw, ih) = match self.upscale_render {
            Some((rw, rh)) => (rw.max(1), rh.max(1)),
            None if s == 1.0 => (w, h),
            None => (((w as f32 * s).round() as u32).max(1), ((h as f32 * s).round() as u32).max(1)),
        };
        self.view.width = iw as f32;
        self.view.height = ih as f32;
        self.view.hor_plus = self.options.hor_plus;
        self.view.viewport = None;
        let max_aspect = self.options.max_aspect;
        if max_aspect > 0.0 && iw as f32 > ih as f32 * max_aspect {
            let vw = (ih as f32 * max_aspect).round();
            let vx = ((iw as f32 - vw) / 2.0).floor();
            self.view.viewport = Some([vx, 0.0, vw, ih as f32]);
        }
        // Hor+ on a wide (tall) screen: `XfmPoint`'s camera-space box grows with the visible
        // area so sprites and effects past the old 4:3 edges are not rejected. [1, 1] (the
        // original constants) otherwise.
        let [kx, ky] = self.view.hor_plus_extent();
        self.view.horiz_clip = crate::scene::HORIZ_CLIP * kx;
        self.view.vert_clip = crate::scene::VERT_CLIP * ky;
        // GPU meshes need storage buffers in the vertex shader (not on GLES-class adapters).
        #[cfg(feature = "gpu")]
        if self.options.gpu_meshes && self.device.limits().max_storage_buffers_per_shader_stage < 4 {
            self.options.gpu_meshes = false;
        }
        self.view.exact_depth = self.options.gpu_meshes;
        #[cfg(feature = "gpu")]
        self.gpu_sink.clear();
        self.sprite_lights.clear();
        #[cfg(feature = "gpu")]
        {
            // Wakes once per 60 Hz tick of the water clock (whatever the frame rate).
            let tick = (self.water_clock * 60.0).floor() as i64;
            self.wake_frame = tick != self.wake_tick;
            self.wake_tick = tick;
        }
        let v = &self.view;
        self.motion = last.map(|l| {
            let same = l.camera.is_some() && (l.width, l.height, l.viewport, l.hor_plus) == (v.width, v.height, v.viewport, v.hor_plus);
            Motion { prev_view: same.then_some(l), frame: self.frame_no }
        });
        self.history_reset = self.motion.as_ref().is_none_or(|m| m.prev_view.is_none());
        self.frames.clear();
        self.backdrops.clear();
        self.overlays.clear();
        if let Some(s) = &mut self.mesh_sink {
            s.clear();
        }
    }

    /// Effective supersampling factor (clamped to 0.25..=4).
    fn render_scale(&self) -> f32 {
        let s = self.options.render_scale;
        if s.is_finite() && s > 0.0 {
            s.clamp(0.25, 4.0)
        } else {
            1.0
        }
    }

    /// Effective MSAA sample count: 1 or 4 (the counts every backend supports for the
    /// colour and depth formats without adapter-specific features).
    #[cfg(feature = "gpu")]
    fn samples(&self) -> u32 {
        if self.options.msaa > 1 {
            4
        } else {
            1
        }
    }

    /// Output-pixel to internal-pixel factor (render scale actually applied).
    pub fn ui_scale(&self) -> f32 {
        self.view.width / self.output.0 as f32
    }

    /// The size passed to [`begin_frame`](Self::begin_frame).
    pub fn output_size(&self) -> (u32, u32) {
        self.output
    }

    /// The centred 4:3 area of the 3D viewport `(x, width)` used for re-anchored overlays
    /// and pillarboxed backdrops (the whole viewport when it is not wider than 4:3).
    fn safe_area(&self) -> (f32, f32) {
        let [vx, _, vw, vh] = self.view.vp();
        let sw = vw.min(vh * 4.0 / 3.0);
        (vx + ((vw - sw) / 2.0).floor(), sw)
    }

    /// Maps an element spanning `x..x + w` of the 4096 overlay space horizontally:
    /// returns `(pixel x of overlay x = 0, pixels per overlay unit)`.
    ///
    /// Authentic: stretched over the whole screen like `OVERLAY_X = rXRes/4096`.
    /// With `hud_anchor`: scaled to the 4:3 safe area; elements centred in the left
    /// third stick to the left edge of the viewport, the right third to the right edge,
    /// everything else (centred text, full-screen backdrops) stays in the centred 4:3
    /// area, i.e. full-screen elements are pillarboxed.
    pub fn overlay_x_map(&self, x: f32, w: f32) -> (f32, f32) {
        let [vx, _, vw, _] = self.view.vp();
        if !self.options.hud_anchor {
            return (vx, vw / OVERLAY_SPACE);
        }
        let (sx0, sw) = self.safe_area();
        let k = sw / OVERLAY_SPACE;
        let c = x + w / 2.0;
        let origin = if w < OVERLAY_SPACE * 0.75 && c < OVERLAY_SPACE / 3.0 {
            vx
        } else if w < OVERLAY_SPACE * 0.75 && c > OVERLAY_SPACE * 2.0 / 3.0 {
            vx + vw - sw
        } else {
            sx0
        };
        (origin, k)
    }

    /// Vertical overlay mapping `(pixel y of overlay y = 0, pixels per unit)`.
    fn overlay_y_map(&self) -> (f32, f32) {
        let [_, vy, _, vh] = self.view.vp();
        (vy, vh / OVERLAY_SPACE)
    }

    pub fn set_camera(&mut self, cam: &Camera) {
        self.view.set_camera(cam);
        // A cut (a new level, a camera jump): nothing to compare this frame with.
        let d = |a: [f32; 3], b: [f32; 3]| ((a[0] - b[0]) * (a[0] - b[0]) + (a[1] - b[1]) * (a[1] - b[1]) + (a[2] - b[2]) * (a[2] - b[2])).sqrt();
        if let Some(p) = self.motion.as_ref().and_then(|m| m.prev_view.as_ref()?.camera) {
            if d(p.source, cam.source) > CUT_DISTANCE || d(p.target, cam.target) > 2.0 * CUT_DISTANCE {
                self.reset_motion();
            }
        }
    }

    /// Drops this frame's motion vectors and the upscaler's history (e.g. a scene change
    /// the camera test cannot see).
    pub fn reset_motion(&mut self) {
        if let Some(m) = &mut self.motion {
            m.prev_view = None;
        }
        self.history_reset = true;
    }

    /// Whether this frame cannot use the upscaler's history (see [`reset_motion`](Self::reset_motion)).
    pub fn history_reset(&self) -> bool {
        self.history_reset
    }

    /// The camera for the upscaling libraries ([`crate::upscale::UpscaleCamera`]).
    pub fn upscale_camera(&self) -> crate::upscale::UpscaleCamera {
        crate::upscale::UpscaleCamera::new(&self.view, self.motion.as_ref().and_then(|m| m.prev_view.as_ref()))
    }

    /// Renderer frame number (advanced by [`begin_frame`](Self::begin_frame)).
    pub fn frame_number(&self) -> u32 {
        self.frame_no
    }

    /// Advances animated textures to `frame_count` game frames.
    pub fn update_animating_textures(&mut self, frame_count: u32) {
        self.textures.update_animating_textures(frame_count);
    }

    fn emit_ctx(&self, alpha_mul: f32) -> EmitCtx<'_> {
        EmitCtx { view: &self.view, opts: &self.options, textures: &self.textures, alpha_mul, override_tex: None, gouraud: None, motion: self.motion.as_ref() }
    }

    /// Transforms and draws an actor (`XformActor` + `DrawActor`).
    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_d")]
    pub fn draw_actor(&mut self, model: &Model, actor: &mut Actor) {
        if let Some(sink) = self.mesh_sink.as_mut().filter(|_| !self.cpu_models.contains(&model.id)) {
            // Disjoint borrows of the renderer (no move of the sink out and back).
            let cx = EmitCtx {
                view: &self.view,
                opts: &self.options,
                textures: &self.textures,
                alpha_mul: actor.xlu.clamp(0.0, 1.0),
                override_tex: actor.override_tex,
                gouraud: actor.gouraud,
                motion: None,
            };
            record_actor(&cx, model, actor, sink);
            return;
        }
        self.draw_actor_cpu(model, actor);
    }

    /// [`anim::model_bound`](crate::anim::model_bound) of `model` with the mesh sink back
    /// end's mesh bounds: `None` without such a back end (or for a CPU-drawn model), i.e.
    /// never [`actor_culled`](Self::actor_culled).
    pub fn actor_bound(&self, model: &Model) -> Option<f32> {
        let b = self.mesh_sink.as_ref()?.backend.as_ref()?;
        if self.cpu_models.contains(&model.id) {
            return None;
        }
        crate::anim::model_bound(model, &|o| b.object_sphere(model.id, o))
    }

    /// Whether the mesh sink back end would cull every rigid draw inside the world-space
    /// sphere `(c, r)` (see [`actor_bound`](Self::actor_bound)); the caller then calls
    /// [`skip_actor`](Self::skip_actor) instead of [`draw_actor`](Self::draw_actor).
    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_d")]
    pub fn actor_culled(&self, c: &crate::math::Vec3, r: f32) -> bool {
        self.mesh_sink.as_ref().is_some_and(|s| s.sphere_culled(c, r))
    }

    /// An actor left out after [`actor_culled`](Self::actor_culled): the back end is told how
    /// many draws [`draw_actor`](Self::draw_actor) would have made.
    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_d")]
    pub fn skip_actor(&mut self, model: &Model, actor: &mut Actor) {
        if let Some(s) = self.mesh_sink.as_mut() {
            crate::scene::skip_actor(model, actor, s);
        }
    }

    /// Testing: [`MeshBackend::cull_check`] of the mesh sink's back end.
    pub fn cull_check(&self) -> bool {
        self.mesh_sink.as_ref().and_then(|s| s.backend.as_ref()).is_some_and(|b| b.cull_check())
    }

    /// Testing: [`MeshBackend::mark_culled`] of the mesh sink's back end.
    pub fn mark_culled(&mut self, on: bool) {
        if let Some(b) = self.mesh_sink.as_mut().and_then(|s| s.backend.as_mut()) {
            b.mark_culled(on);
        }
    }

    /// `n` draws left out after [`actor_culled`](Self::actor_culled) (the count
    /// [`draw_actor`](Self::draw_actor) made last time; [`skip_actor`](Self::skip_actor)
    /// works it out).
    #[cfg_attr(target_vendor = "nintendo64", link_section = ".text.f2hot_d")]
    pub fn skip_draws(&mut self, n: u32) {
        if let Some(b) = self.mesh_sink.as_mut().and_then(|s| s.backend.as_mut()) {
            b.skip(n);
        }
    }

    /// [`draw_actor`](Self::draw_actor) transformed on the CPU into the frames (out of line:
    /// keeps the mesh-sink path small for a console's instruction cache).
    #[inline(never)]
    fn draw_actor_cpu(&mut self, model: &Model, actor: &mut Actor) {
        #[cfg(feature = "gpu")]
        {
            // The host may set the lighting after `begin_frame`.
            self.gpu_sink.water_on = self.options.gpu_meshes && self.lighting.as_ref().is_some_and(|l| l.water);
            if self.gpu_sink.water_on {
                self.actor_wake(model, actor);
            }
        }
        let mut frames = core::mem::take(&mut self.frames);
        #[cfg(feature = "gpu")]
        let mut sink = core::mem::take(&mut self.gpu_sink);
        let cx = EmitCtx { override_tex: actor.override_tex, gouraud: actor.gouraud, ..self.emit_ctx(actor.xlu.clamp(0.0, 1.0)) };
        #[cfg(feature = "gpu")]
        {
            let no_clip = self.view.no_clip;
            let textures = &self.textures;
            let mut hook = |o: &crate::scene::GpuObj| sink.offer(o, no_clip, textures);
            draw_actor(&cx, model, actor, &mut frames, if self.options.gpu_meshes { Some(&mut hook) } else { None });
        }
        #[cfg(not(feature = "gpu"))]
        draw_actor(&cx, model, actor, &mut frames, None);
        self.frames = frames;
        #[cfg(feature = "gpu")]
        {
            self.gpu_sink = sink;
        }
    }

    /// Draws a static level model like `DrawLandscape` (key-0 pose).
    pub fn draw_landscape(&mut self, model: &Model) {
        if let Some(mut sink) = self.mesh_sink.take_if(|_| !self.cpu_models.contains(&model.id)) {
            record_landscape(&self.emit_ctx(1.0), model, &mut sink);
            self.mesh_sink = Some(sink);
            return;
        }
        let mut frames = core::mem::take(&mut self.frames);
        #[cfg(feature = "gpu")]
        {
            let mut sink = core::mem::take(&mut self.gpu_sink);
            let no_clip = self.view.no_clip;
            let textures = &self.textures;
            let mut hook = |o: &crate::scene::GpuObj| sink.offer(o, no_clip, textures);
            draw_landscape(&self.emit_ctx(1.0), model, &mut frames, if self.options.gpu_meshes { Some(&mut hook) } else { None });
            self.gpu_sink = sink;
        }
        #[cfg(not(feature = "gpu"))]
        draw_landscape(&self.emit_ctx(1.0), model, &mut frames, None);
        self.frames = frames;
    }

    /// Pushes a pre-transformed triangle into a frame (`PushPolys`).
    pub fn push_tri(&mut self, frame: Frame, tri: Tri) {
        self.frames.push(frame, tri);
    }

    /// Queues a `ProcessShadows` poly: drawn after the landscape and list-0 actors, before
    /// the list-1 actors (see [`Frames::shadows`]).
    pub fn push_shadow(&mut self, tri: Tri) {
        self.frames.push_shadow(tri);
    }

    /// `ActorListDraw(1)` starts (after `ProcessShadows`): see [`Frames::begin_late_actors`].
    pub fn begin_late_actors(&mut self) {
        self.frames.begin_late_actors();
    }

    /// Number of 3D triangles queued so far this frame.
    pub fn queued_tris(&self) -> usize {
        self.frames.total()
    }

    /// Triangles dropped so far this frame because a list could not grow (out of memory).
    pub fn dropped_tris(&self) -> u32 {
        self.frames.dropped
    }

    /// Full-screen backdrop (`mdxDrawBackdrop`), drawn before everything else.
    /// With `hud_anchor` (enhanced) it is pillarboxed to the centred 4:3 area.
    pub fn draw_backdrop(&mut self, tex: TexHandle) {
        let (x, w) = if self.options.hud_anchor { self.safe_area() } else { (0.0, self.view.width) };
        let h = self.view.height;
        self.backdrops.push(Quad2D::rect(x, 0.0, w, h, [0.0, 0.0, 1.0, 1.0], Some(tex), [255; 4], Blend::Semi));
    }

    /// A 2D quad in output pixels drawn with the backdrops, before the 3D frames
    /// (`DrawTiledBackdrop`'s rectangles).
    pub fn draw_backdrop_quad(&mut self, mut q: Quad2D) {
        let s = self.ui_scale();
        if s != 1.0 {
            for p in &mut q.pos {
                *p = [p[0] * s, p[1] * s];
            }
        }
        self.backdrops.push(q);
    }

    /// A 2D quad in output pixels (the `begin_frame` size), drawn after the 3D frames in
    /// submission order.
    pub fn draw_quad(&mut self, mut q: Quad2D) {
        let s = self.ui_scale();
        if s != 1.0 {
            for p in &mut q.pos {
                *p = [p[0] * s, p[1] * s];
            }
        }
        self.overlays.push(q);
    }

    /// Port of `DrawSpriteOverlay` / `DrawSpriteOverlayRotating`
    /// (pcsprite.c, mdxSprite.cpp:164) in the 4096×4096 virtual overlay space.
    /// `angle` in radians (the caller converts from game angle units).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_sprite_overlay(&mut self, x: f32, y: f32, w: f32, h: f32, uv: [f32; 4], tex: Option<TexHandle>, color: [u8; 4], blend: Blend, angle: f32) {
        let (ox, sx) = self.overlay_x_map(x, w);
        let (oy, sy) = self.overlay_y_map();
        let (mut x, mut y, mut w, mut h) = (ox + x * sx, oy + y * sy, w * sx, h * sy);
        let mut uv = uv;
        if let (Some([cl, ct, cr, cb]), true) = (self.sprite_clip, angle == 0.0) {
            // DrawSpriteOverlay (pcsprite.c:228): clip to SPRITECLIP* with u/v adjusted.
            // `xs`/`ys` are only used for the u/v maths (with the original's bookkeeping).
            let (mut x2, mut y2, mut xs, mut ys) = (x + w, y + h, w, h);
            if x < cl {
                if x2 < cl {
                    return;
                }
                uv[0] += (uv[2] - uv[0]) * (cl - x) / xs;
                xs += x - cl;
                x = cl;
            }
            if x2 > cr {
                if x > cr {
                    return;
                }
                uv[2] += (uv[2] - uv[0]) * (cr - x2) / xs;
                x2 = cr;
            }
            if y < ct {
                if y2 < ct {
                    return;
                }
                uv[1] += (uv[3] - uv[1]) * (ct - y) / ys;
                ys += y - ct;
                y = ct;
            }
            if y2 > cb {
                if y > cb {
                    return;
                }
                uv[3] += (uv[3] - uv[1]) * (cb - y2) / ys;
                y2 = cb;
            }
            w = x2 - x;
            h = y2 - y;
        }
        let mut q = Quad2D::rect(x, y, w, h, uv, tex, color, blend);
        if blend == Blend::Subtractive {
            // pcsprite.c: colour pre-multiplied by alpha for xluSubRS.
            let a = color[3] as u32;
            q.color = [((color[0] as u32 * a) >> 8) as u8, ((color[1] as u32 * a) >> 8) as u8, ((color[2] as u32 * a) >> 8) as u8, 1];
        }
        if angle != 0.0 {
            let (w2, h2) = (w * 0.5, h * 0.5);
            let (s, c) = angle.sin_cos();
            let corners = [[-w2, -h2], [w2, -h2], [w2, h2], [-w2, h2]];
            for (p, k) in q.pos.iter_mut().zip(corners) {
                *p = [k[0] * c + k[1] * s + x + w2, k[1] * c - k[0] * s + y + h2];
            }
        }
        self.overlays.push(q);
    }

    /// Whether the device can do the Path-Traced graphics ([`RenderOptions::ray_tracing`]).
    #[cfg(feature = "gpu")]
    pub fn ray_tracing_supported(&self) -> bool {
        self.device.features().contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY)
    }

    /// Enhanced water: something disturbs the water at `s.pos` (a landing, a splash). Applied
    /// at the next simulation step (where there is water near that height).
    #[cfg(feature = "gpu")]
    pub fn water_splash(&mut self, s: crate::water::Splash) {
        if self.gpu_sink.water_on && self.splashes.len() < crate::water::MAX_SPLATS {
            self.splashes.push(s);
        }
    }

    /// Enhanced water: an actor moving through the water makes waves (its movement since its
    /// last wake, once per 60 Hz tick of the water clock).
    #[cfg(feature = "gpu")]
    fn actor_wake(&mut self, model: &Model, actor: &mut Actor) {
        if !self.wake_frame {
            return;
        }
        let p = actor.pos;
        if let Some(q) = actor.water_prev.replace(p) {
            let d = ((p[0] - q[0]).powi(2) + (p[2] - q[2]).powi(2)).sqrt();
            let size = model.radius * actor.scale[0].abs().max(actor.scale[2].abs());
            if d > 0.05 && d < 60.0 && self.splashes.len() < crate::water::MAX_SPLATS {
                self.splashes.push(crate::water::Splash { pos: p, radius: (size * 0.5).clamp(6.0, 40.0), strength: (d * 0.25).min(2.0) });
            }
        }
    }

    /// [`lighting`](Self::lighting) with emissive lights, GPU meshes: a sprite drawn at world
    /// position `pos` with texture `crc` lights its surroundings if the texture glows (fire,
    /// flames: [`crate::lighting::is_emissive`]). `color` = its tint, `scale` = its size
    /// (`scale_x`, 64 = one sprite unit).
    pub fn light_sprite(&mut self, crc: u32, pos: [f32; 3], color: [f32; 3], scale: f32) {
        if !self.options.gpu_meshes || !self.lighting.as_ref().is_some_and(|l| l.emissive_lights) {
            return;
        }
        let textures = &self.textures;
        let glows = *self.emissive_crc.entry(crc).or_insert_with(|| textures.entry(crc).is_some_and(|e| crate::lighting::is_emissive(&e.name)));
        if glows && self.sprite_lights.len() < 256 {
            let k = 0.9;
            self.sprite_lights.push(crate::lighting::PointLight {
                pos,
                color: [color[0] * k * 1.3, color[1] * k * 1.05, color[2] * k * 0.8],
                radius: 120.0 + scale * 1.5,
            });
        }
    }

    /// Port of `DrawAlphaSprite` (mdxSprite.cpp:45): a screen-aligned sprite at
    /// screen position/size in pixels with screen depth `sz` (0..1), pushed into
    /// the late (post-`DrawAllFrames`) semi frame so it is depth-tested against the scene.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_alpha_sprite(&mut self, x: f32, y: f32, sz: f32, xs: f32, ys: f32, uv: [f32; 4], tex: Option<TexHandle>, color: [u8; 4]) {
        let col = color.map(|c| c as f32 / 255.0);
        let v = |sx, sy, u, vv| TlVertex { sx, sy, sz, rhw: 1.0, color: col, uv: [u, vv], fog: 1.0, alpha_ref: 0.0 };
        let (x2, y2) = (x + xs, y + ys);
        let q = [v(x, y, uv[0], uv[1]), v(x2, y, uv[2], uv[1]), v(x2, y2, uv[2], uv[3]), v(x, y2, uv[0], uv[3])];
        let tex = tex.or(Some(self.white));
        self.frames.push(Frame::Late, Tri { v: [q[0], q[1], q[2]], tex });
        self.frames.push(Frame::Late, Tri { v: [q[2], q[3], q[0]], tex });
    }

    /// Port of `DrawFontStringAtLoc` (mdxFont.cpp) at output-pixel position `(x, y)`.
    /// `text` is Latin-1 (chars above U+00FF draw nothing). With `centred_x`,
    /// the string is centred on that x. Returns the end x.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_text(&mut self, font: FontId, x: f32, y: f32, text: &str, color: [u8; 4], scale: f32, centred_x: Option<f32>) -> f32 {
        let s = self.ui_scale();
        if s == 1.0 {
            return self.draw_text_px(font, x, y, text, color, scale, centred_x);
        }
        self.draw_text_px(font, x * s, y * s, text, color, scale * s, centred_x.map(|c| c * s)) / s
    }

    fn text_bytes(text: &str) -> Vec<u8> {
        text.chars().map(|c| if (c as u32) < 256 { c as u8 } else { 0 }).collect()
    }

    /// [`draw_text`](Self::draw_text) in internal pixels.
    #[allow(clippy::too_many_arguments)]
    fn draw_text_px(&mut self, font: FontId, x: f32, y: f32, text: &str, color: [u8; 4], scale: f32, centred_x: Option<f32>) -> f32 {
        let bytes = Self::text_bytes(text);
        let (f, tex, hires, mul) = (&self.fonts[font.0].0, self.fonts[font.0].1, self.fonts[font.0].2.as_ref(), self.fonts[font.0].3);
        // Enlarged fonts grow around the string's centre (left-aligned text would run into
        // what sits to its right, e.g. the timer's clock).
        let (x, y, scale) = if mul != 1.0 {
            let grow_x = if centred_x.is_none() { (mul - 1.0) * f.string_width(&bytes, scale) as f32 / 2.0 } else { 0.0 };
            (x - grow_x, y - (mul - 1.0) * f.height as f32 * scale / 2.0, scale * mul)
        } else {
            (x, y, scale)
        };
        let mut cx = match centred_x {
            Some(c) => c - (f.string_width(&bytes, scale) / 2) as f32,
            None => x,
        };
        let (fw, fh) = (f.width as f32, f.height as f32);
        let mut quads = Vec::new();
        for &b in &bytes {
            let Some(g) = f.glyphs[b as usize] else { continue };
            let w = (g.width as f32 * scale).trunc();
            if let Some(i) = crate::font::ICONS.iter().position(|&c| c == b) {
                // Button icon, centred in its advance and the line; not tinted (the drop
                // shadow pass, drawn in black, stays black).
                if let Some((itex, _)) = self.icons {
                    let side = (w - 2.0 * scale).min(fh * scale * 0.8).max(1.0).trunc();
                    let col = if color[..3] == [0, 0, 0] { color } else { [255, 255, 255, color[3]] };
                    let (u1, u2) = (i as f32 / 4.0, (i + 1) as f32 / 4.0);
                    let (ix, iy) = ((cx + (w - side) / 2.0).trunc(), (y + (fh * scale - side) / 2.0).trunc());
                    quads.push(Quad2D::rect(ix, iy, side, side, [u1, 0.0, u2, 1.0], Some(itex), col, Blend::Semi));
                }
            } else if let Some(uvs) = hires {
                if let Some(uv) = uvs[b as usize] {
                    quads.push(Quad2D::rect(cx.trunc(), y.trunc(), w, (fh * scale).trunc(), uv, Some(tex), color, Blend::Semi));
                }
            } else if g.left < f.width {
                let u1 = g.left as f32 / fw;
                let u2 = (g.left + g.width).min(f.width) as f32 / fw;
                quads.push(Quad2D::rect(cx.trunc(), y.trunc(), w, (fh * scale).trunc(), [u1, 0.0, u2, 1.0], Some(tex), color, Blend::Semi));
            }
            cx += w;
        }
        self.overlays.extend(quads);
        cx
    }

    /// [`draw_text`](Self::draw_text) with coordinates in the 4096 overlay space.
    /// `scale` is the font scale in internal pixels, like the original's
    /// resolution-independent font scale.
    pub fn draw_text_overlay(&mut self, font: FontId, x: f32, y: f32, text: &str, color: [u8; 4], scale: f32) -> f32 {
        self.draw_text_overlay_ex(font, x, y, text, color, scale, false, 0.0)
    }

    /// [`draw_text_overlay`](Self::draw_text_overlay) with the options of
    /// `PrintText` (textover.c:174): `centred` centres the string on `x`
    /// (`DrawFontStringAtLoc(..., centre = xPos)`), `dy_px` is a vertical offset in
    /// internal pixels added after the overlay mapping (the drop shadow's `+6`/`+4`).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_text_overlay_ex(&mut self, font: FontId, x: f32, y: f32, text: &str, color: [u8; 4], scale: f32, centred: bool, dy_px: f32) -> f32 {
        let (oy, sy) = self.overlay_y_map();
        if !self.options.hud_anchor && self.view.viewport.is_none() {
            // Original mapping (bit-exact authentic path).
            let sx = self.view.width / OVERLAY_SPACE;
            let px = x * sx;
            return self.draw_text_px(font, px, y * sy + dy_px, text, color, scale, centred.then_some(px)) / sx;
        }
        // Anchor by the string's extent in overlay units.
        let (_, k) = self.overlay_x_map(0.0, 0.0);
        let px_w = self.fonts[font.0].0.string_width(&Self::text_bytes(text), scale) as f32;
        let left = if centred { x - px_w / k / 2.0 } else { x };
        let (ox, sx) = self.overlay_x_map(left, px_w / k);
        let px = ox + x * sx;
        let end = self.draw_text_px(font, px, oy + y * sy + dy_px, text, color, scale, centred.then_some(px));
        (end - ox) / sx
    }

    /// Pixels per overlay unit horizontally and vertically for an element at `x..x + w`
    /// (internal pixels): what `OVERLAY_X`/`OVERLAY_Y` are in the original, after the
    /// enhanced re-anchoring.
    pub fn overlay_scale(&self, x: f32, w: f32) -> (f32, f32) {
        (self.overlay_x_map(x, w).1, self.overlay_y_map().1)
    }

    // ---------------------------------------------------------------- submission

    /// Builds the frame's draw list in `DrawAllFrames` order: the viewport clear quad (when
    /// clamped), backdrops, the seven 3D frame passes (translucent passes depth-sorted per
    /// [`RenderOptions::xlu_sort`]), then 2D overlays. Screen-space vertices (pixels of the
    /// internal view, `sz` depth 0..1, `rhw`); each [`DrawCmd`] is a run of whole triangles.
    /// Does not clear the queued frame (call [`begin_frame`](Self::begin_frame) for that).
    /// Images to upload first: [`TextureRegistry::take_dirty`].
    pub fn build(&mut self, clear: [f64; 4]) -> (Vec<TlVertex>, Vec<DrawCmd>) {
        let (v, c, _) = self.build_segments(clear);
        (v, c)
    }

    /// [`build`](Self::build) plus the segment boundaries: `marks[k]` is the index of the
    /// first command of segment `k` (0 = viewport clear + backdrops, 1..=7 = the 3D passes
    /// in `DrawAllFrames` order -- Normal, Wrap, Xlu, Additive, Late, LateAdditive, Overlay --
    /// 8 = 2D overlays); `marks[9]` = the number of commands. Commands never span two
    /// segments, so a back end can interleave its own geometry between them.
    pub fn build_segments(&mut self, clear: [f64; 4]) -> (Vec<TlVertex>, Vec<DrawCmd>, [u32; 10]) {
        let (v, _, c, m) = self.build_all(clear, false);
        (v, c, m)
    }

    /// [`build_segments`](Self::build_segments); with `want_mv` also each vertex's screen
    /// motion ([`Frames::motion`], zero where none was recorded), parallel to the vertices.
    fn build_all(&mut self, clear: [f64; 4], want_mv: bool) -> (Vec<TlVertex>, Vec<[f32; 2]>, Vec<DrawCmd>, [u32; 10]) {
        let mut marks = [0u32; 10];
        let mut verts: Vec<TlVertex> = Vec::with_capacity(self.frames.total() * 3 + 6 * (self.backdrops.len() + self.overlays.len() + 1));
        let mut mvs: Vec<[f32; 2]> = Vec::with_capacity(if want_mv { verts.capacity() } else { 0 });
        let mut cmds: Vec<DrawCmd> = Vec::new();
        let white = self.white;
        // Commands before `barrier` belong to an earlier segment and are never extended.
        let barrier = core::cell::Cell::new(0usize);
        #[allow(clippy::too_many_arguments)]
        let push = |verts: &mut Vec<TlVertex>,
                    mvs: &mut Vec<[f32; 2]>,
                    cmds: &mut Vec<DrawCmd>,
                    blend,
                    depth,
                    wrap,
                    scene: bool,
                    tex: Option<TexHandle>,
                    tri: &[TlVertex],
                    mv: Option<&[[f32; 2]; 3]>| {
            let tex = tex.unwrap_or(white);
            let first = verts.len() as u32;
            verts.extend_from_slice(tri);
            if want_mv {
                match mv {
                    Some(m) if tri.len() == 3 => mvs.extend_from_slice(m),
                    _ => mvs.extend(core::iter::repeat_n([0.0; 2], tri.len())),
                }
            }
            let open = cmds.len() > barrier.get();
            if let Some(last) = cmds.last_mut().filter(|_| open) {
                if last.blend == blend && last.depth == depth && last.tex == tex && last.wrap == wrap && last.scene == scene && last.first + last.count == first
                {
                    last.count += tri.len() as u32;
                    return;
                }
            }
            cmds.push(DrawCmd { blend, depth, tex, wrap, scene, first, count: tri.len() as u32 });
        };
        let quad_tris = Quad2D::tl_vertices;
        if let Some([x, y, w, h]) = self.view.viewport {
            // Ultrawide clamp: the target is cleared black (pillars), the viewport gets the
            // scene's clear colour.
            let c = clear.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
            let q = Quad2D::rect(x, y, w, h, [0.0; 4], None, [c[0], c[1], c[2], 255], Blend::Semi);
            push(&mut verts, &mut mvs, &mut cmds, q.blend, DepthMode::Off, false, false, q.tex, &quad_tris(&q), None);
        }
        for q in &self.backdrops {
            push(&mut verts, &mut mvs, &mut cmds, q.blend, DepthMode::Off, false, false, q.tex, &quad_tris(q), None);
        }
        // Retail's two `DrawAllFrames` flushes around `ProcessShadows`: the opaque passes'
        // early triangles (landscape, list-0 actors), the shadows, then their late ones
        // (list-1 actors, which cover their own shadows). The early translucent triangles
        // are not split off: they are drawn with the late ones, after the shadows.
        let split = self.frames.late_split;
        let early = |frame: Frame, len: usize| match (frame, split) {
            (Frame::Normal, Some([n, _])) => n.min(len),
            (Frame::Wrap, Some([_, w])) => w.min(len),
            (Frame::Normal | Frame::Wrap, None) => len,
            _ => 0,
        };
        // `mot`: the motion of `tris` (aligned with it, possibly shorter: none for the rest).
        #[allow(clippy::too_many_arguments)]
        let emit =
            |verts: &mut Vec<TlVertex>, mvs: &mut Vec<[f32; 2]>, cmds: &mut Vec<DrawCmd>, tris: &[Tri], mot: &[[[f32; 2]; 3]], blend, depth, wrap, aref| {
                for (i, t) in tris.iter().enumerate() {
                    let mut v = t.v;
                    for x in &mut v {
                        x.alpha_ref = aref;
                    }
                    push(verts, mvs, cmds, blend, depth, wrap, true, t.tex, &v, mot.get(i));
                }
            };
        let (n_early, w_early) =
            (early(Frame::Normal, self.frames.tris[Frame::Normal as usize].len()), early(Frame::Wrap, self.frames.tris[Frame::Wrap as usize].len()));
        // Motion of a frame list's `range` (only the unsorted opaque lists keep theirs aligned).
        fn motion_of(frames: &Frames, f: Frame, range: core::ops::Range<usize>, want: bool) -> &[[[f32; 2]; 3]] {
            if !want || !matches!(f, Frame::Normal | Frame::Wrap) {
                return &[];
            }
            let m = &frames.motion[f as usize];
            &m[range.start.min(m.len())..range.end.min(m.len())]
        }
        for (k, (frame, blend, depth, wrap, aref, sorted)) in PASSES.into_iter().enumerate() {
            barrier.set(cmds.len());
            marks[k + 1] = cmds.len() as u32;
            if sorted {
                self.options.xlu_sort.sort(&mut self.frames.tris[frame as usize]);
            }
            let frames = &self.frames;
            let tris = &frames.tris[frame as usize];
            match frame {
                // Segment 1 holds the early opaque triangles of both opaque passes, the
                // shadows (`xluZRS`, `normalAlphaCmpRS`, clamped) and the late `Normal` ones.
                Frame::Normal => {
                    emit(&mut verts, &mut mvs, &mut cmds, &tris[..n_early], motion_of(frames, frame, 0..n_early, want_mv), blend, depth, wrap, aref);
                    let (_, wb, wd, ww, wa, _) = PASSES[1];
                    let wt = &frames.tris[Frame::Wrap as usize][..w_early];
                    emit(&mut verts, &mut mvs, &mut cmds, wt, motion_of(frames, Frame::Wrap, 0..w_early, want_mv), wb, wd, ww, wa);
                    emit(&mut verts, &mut mvs, &mut cmds, &frames.shadows, &[], Blend::Semi, DepthMode::Test, false, ALPHA_NONZERO);
                    emit(&mut verts, &mut mvs, &mut cmds, &tris[n_early..], motion_of(frames, frame, n_early..tris.len(), want_mv), blend, depth, wrap, aref);
                }
                Frame::Wrap => {
                    emit(&mut verts, &mut mvs, &mut cmds, &tris[w_early..], motion_of(frames, frame, w_early..tris.len(), want_mv), blend, depth, wrap, aref)
                }
                _ => emit(&mut verts, &mut mvs, &mut cmds, tris, &[], blend, depth, wrap, aref),
            }
        }
        barrier.set(cmds.len());
        marks[8] = cmds.len() as u32;
        for q in &self.overlays {
            push(&mut verts, &mut mvs, &mut cmds, q.blend, DepthMode::Off, false, false, q.tex, &quad_tris(q), None);
        }
        marks[9] = cmds.len() as u32;
        (verts, mvs, cmds, marks)
    }

    /// The frame's parts in [`build_segments`](Self::build_segments) order without building
    /// the vertex list (a back end that converts them itself, the N64): segment 0 = the
    /// viewport clear quad and the backdrops, 1..=7 = the 3D passes (translucent ones
    /// sorted as `build_segments` sorts them) with their states, 8 = the 2D overlays.
    /// Untextured geometry uses `white`. The vertices `build_segments` would make are the
    /// triangles' own with the pass's `alpha_ref`, and [`Quad2D::tl_vertices`] for quads.
    pub fn frame_view(&mut self, clear: [f64; 4]) -> FrameView<'_> {
        let viewport_clear = self.view.viewport.map(|[x, y, w, h]| {
            let c = clear.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
            Quad2D::rect(x, y, w, h, [0.0; 4], None, [c[0], c[1], c[2], 255], Blend::Semi)
        });
        // This view has no shadow slot: the shadows go back into `Xlu` where they used to be
        // queued (back ends that consume it draw them as before).
        if !self.frames.shadows.is_empty() {
            let at = self.frames.shadow_xlu_at.unwrap_or(0).min(self.frames.tris[Frame::Xlu as usize].len());
            let shadows = core::mem::take(&mut self.frames.shadows);
            self.frames.tris[Frame::Xlu as usize].splice(at..at, shadows);
        }
        let sort = self.options.xlu_sort;
        for (frame, _, _, _, _, sorted) in PASSES {
            if sorted {
                sort.sort(&mut self.frames.tris[frame as usize]);
            }
        }
        let tris = &self.frames.tris;
        FrameView {
            viewport_clear,
            backdrops: &self.backdrops,
            passes: PASSES.map(|(frame, blend, depth, wrap, alpha_ref, _)| (PassState { blend, depth, wrap, alpha_ref }, &tris[frame as usize][..])),
            overlays: &self.overlays,
            white: self.white,
        }
    }

    #[cfg(feature = "gpu")]
    fn colour_texture(&self, label: &str, w: u32, h: u32, samples: u32, usage: wgpu::TextureUsages) -> wgpu::TextureView {
        self.device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format: self.format,
                usage,
                view_formats: &[],
            })
            .create_view(&Default::default())
    }

    /// [`RenderOptions::gpu_meshes`]: uploads the frame's GPU mesh draws and prepares their
    /// pipeline and texture bind groups. Returns whether there is anything to draw.
    #[cfg(feature = "gpu")]
    fn prepare_gpu_meshes(&mut self, format: wgpu::TextureFormat, samples: u32, mrt: bool, jitter: [f32; 2]) -> bool {
        if !self.options.gpu_meshes {
            return false;
        }
        let device = &self.device;
        let gm = self.gpu_mesh.get_or_insert_with(|| crate::gpu_mesh::Gpu::new(device, &self.tex_layout));
        let prev = self.motion.as_ref().and_then(|m| m.prev_view.as_ref());
        let need = gm.prepare(
            &self.device,
            &self.queue,
            &mut self.gpu_sink,
            &self.textures,
            self.white,
            &self.view,
            prev,
            jitter,
            &self.options.fog,
            self.lighting.as_ref(),
            &self.sprite_lights,
            self.water_clock,
            &mut self.splashes,
            self.options.ray_tracing,
        );
        gm.ensure_pipeline(&self.device, format, samples, mrt);
        let any = gm.has_draws() || gm.has_water();
        for (t, w) in need {
            self.bind_group(t, w);
        }
        any
    }

    #[cfg(feature = "gpu")]
    fn ensure_targets(&mut self, w: u32, h: u32, samples: u32, scaled: bool) {
        if !matches!(self.depth, Some((_, dw, dh, ds)) if dw == w && dh == h && ds == samples) {
            let tex = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("depth"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            self.depth = Some((tex.create_view(&Default::default()), w, h, samples));
        }
        if samples > 1 && !matches!(self.msaa_target, Some((_, mw, mh, ms)) if mw == w && mh == h && ms == samples) {
            self.msaa_target = Some((self.colour_texture("msaa", w, h, samples, wgpu::TextureUsages::RENDER_ATTACHMENT), w, h, samples));
        }
        if scaled && !matches!(self.internal, Some((_, iw, ih)) if iw == w && ih == h) {
            let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
            self.internal = Some((self.colour_texture("internal", w, h, 1, usage), w, h));
        }
        if scaled {
            self.ensure_blit();
        }
    }

    /// The bilinear full-screen blit pipeline (render scale resolve, upscaler output).
    #[cfg(feature = "gpu")]
    fn ensure_blit(&mut self) {
        if self.blit.is_none() {
            let shader =
                self.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("blit"), source: wgpu::ShaderSource::Wgsl(BLIT_SHADER.into()) });
            let layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("blit"),
                bind_group_layouts: &[Some(&self.tex_layout)],
                immediate_size: 0,
            });
            let p = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("blit"),
                layout: Some(&layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: self.format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            });
            let s = self.device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("blit"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });
            self.blit = Some((p, s));
        }
    }

    /// Encodes the queued frame into `encoder`, targeting `target`
    /// (`width`×`height` = the `begin_frame` size, format `self.format`).
    /// `clear` = background colour.
    ///
    /// With the default (authentic) options this is a single pass straight into
    /// `target`. Enhanced options add an MSAA target (resolved into `target`) and/or an
    /// internal render-scaled target that is then filtered down to `target`.
    #[cfg(feature = "gpu")]
    pub fn encode(&mut self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, width: u32, height: u32, clear: [f64; 4]) {
        self.upload_dirty();
        let (iw, ih) = (self.view.width as u32, self.view.height as u32);
        let samples = self.samples();
        let scaled = (iw, ih) != (width, height);
        self.ensure_targets(iw, ih, samples, scaled);
        let fog = self.options.fog;
        let _ = fog;
        self.queue.write_buffer(&self.uniform_buf, 0, bytemuck::cast_slice(&self.uniforms([0.0; 2])));
        let (verts, _, cmds, marks) = self.build_all(clear, false);
        let fmt = self.format;
        let gpu = self.prepare_gpu_meshes(fmt, samples, false, [0.0; 2]);
        let clear = if self.view.viewport.is_some() { [0.0, 0.0, 0.0, 1.0] } else { clear };
        if !verts.is_empty() {
            let bytes: &[u8] = bytemuck::cast_slice(&verts);
            if self.vbuf.as_ref().is_none_or(|b| b.size() < bytes.len() as u64) {
                self.vbuf = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("tl vertices"),
                    size: (bytes.len() as u64).next_power_of_two().max(4096),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            }
            self.queue.write_buffer(self.vbuf.as_ref().unwrap(), 0, bytes);
        }
        // Create pipelines and bind groups before the pass borrows them.
        let mut keys = Vec::with_capacity(cmds.len());
        for c in &cmds {
            self.pipeline(c.blend, c.depth, samples);
            self.bind_group(c.tex, c.wrap);
            keys.push((c.tex.0, self.sampler_key(c.wrap)));
        }
        if gpu {
            self.gpu_mesh.as_mut().unwrap().pre_passes(encoder);
        }
        let depth_view = &self.depth.as_ref().unwrap().0;
        let main_target = if scaled { &self.internal.as_ref().unwrap().0 } else { target };
        let (view, resolve) = if samples > 1 { (&self.msaa_target.as_ref().unwrap().0, Some(main_target)) } else { (main_target, None) };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("mdx frame"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: resolve,
                ops: wgpu::Operations {
                    load: if self.overlay && samples == 1 && !scaled {
                        wgpu::LoadOp::Load
                    } else {
                        wgpu::LoadOp::Clear(wgpu::Color { r: clear[0], g: clear[1], b: clear[2], a: clear[3] })
                    },
                    store: if samples > 1 { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store },
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if let Some(vb) = &self.vbuf {
            pass.set_vertex_buffer(0, vb.slice(..));
        }
        pass.set_bind_group(0, &self.uniform_bg, &[]);
        // Ultrawide clamp: 3D frames are scissored to the viewport.
        let scissor = self.view.viewport.map(|[x, y, w, h]| {
            let (x, y) = ((x as u32).min(iw - 1), (y as u32).min(ih - 1));
            [x, y, (w as u32).min(iw - x).max(1), (h as u32).min(ih - y).max(1)]
        });
        let mut scissored = false;
        // GPU meshes: at the start of the 3D passes (opaque, depth-tested: their order against
        // the CPU's opaque triangles does not matter).
        // Enhanced water: at the start of the translucent passes.
        let mut gpu_left = gpu;
        let mut water_left = gpu && self.gpu_mesh.as_ref().is_some_and(|g| g.has_water());
        let draw_gpu = |pass: &mut wgpu::RenderPass<'_>, scissored: &mut bool, water: bool| {
            if let Some([x, y, w, h]) = scissor {
                pass.set_scissor_rect(x, y, w, h);
                *scissored = true;
            }
            let bgs = &self.bind_groups;
            let gm = self.gpu_mesh.as_ref().unwrap();
            if water {
                gm.draw_water(pass, fmt, samples, false, |t, w| &bgs[&(t.0, self.sampler_key(w))]);
            } else {
                gm.draw(pass, fmt, samples, false, |t, w| &bgs[&(t.0, self.sampler_key(w))]);
            }
            if let Some(vb) = &self.vbuf {
                pass.set_vertex_buffer(0, vb.slice(..));
            }
            pass.set_bind_group(0, &self.uniform_bg, &[]);
        };
        for (ci, (c, key)) in cmds.iter().zip(&keys).enumerate() {
            if gpu_left && ci >= marks[1] as usize {
                draw_gpu(&mut pass, &mut scissored, false);
                gpu_left = false;
            }
            if water_left && ci >= marks[3] as usize {
                draw_gpu(&mut pass, &mut scissored, true);
                water_left = false;
            }
            if let Some([x, y, w, h]) = scissor {
                if c.scene != scissored {
                    scissored = c.scene;
                    if scissored {
                        pass.set_scissor_rect(x, y, w, h);
                    } else {
                        pass.set_scissor_rect(0, 0, iw, ih);
                    }
                }
            }
            pass.set_pipeline(&self.pipelines[&(c.blend, c.depth, samples, self.format, false)]);
            pass.set_bind_group(1, &self.bind_groups[key], &[]);
            pass.draw(c.first..c.first + c.count, 0..1);
        }
        if gpu_left {
            draw_gpu(&mut pass, &mut scissored, false);
        }
        if water_left {
            draw_gpu(&mut pass, &mut scissored, true);
        }
        drop(pass);
        if scaled {
            let (pipeline, sampler) = self.blit.as_ref().unwrap();
            let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("blit"),
                layout: &self.tex_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&self.internal.as_ref().unwrap().0) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
                ],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("resolve scale"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// The shader's uniform block `U` with sub-pixel `jitter`.
    #[cfg(feature = "gpu")]
    fn uniforms(&self, jitter: [f32; 2]) -> [f32; 12] {
        let fog = self.options.fog;
        [
            self.view.width,
            self.view.height,
            if self.options.perspective_correct { 1.0 } else { 0.0 },
            if fog.enabled { 1.0 } else { 0.0 },
            fog.color[0],
            fog.color[1],
            fog.color[2],
            1.0,
            jitter[0],
            jitter[1],
            crate::upscale::DEPTH_NEAR,
            0.0,
        ]
    }

    #[cfg(feature = "gpu")]
    fn ensure_up_targets(&mut self, render: (u32, u32), output: (u32, u32)) {
        if self.up.as_ref().is_some_and(|u| u.render == render && u.output == output) {
            return;
        }
        let tex = |label, (w, h): (u32, u32), format, usage| {
            let t = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            });
            let v = t.create_view(&Default::default());
            (t, v)
        };
        use wgpu::TextureUsages as U;
        let read = U::RENDER_ATTACHMENT | U::TEXTURE_BINDING | U::COPY_SRC;
        self.up = Some(UpTargets {
            render,
            output,
            color: tex("upscale color", render, UPSCALE_COLOR_FORMAT, read),
            motion: tex("upscale motion", render, MV_FORMAT, read),
            depth: tex("upscale depth", render, UP_DEPTH_FORMAT, read),
            out: tex("upscale output", output, UPSCALE_COLOR_FORMAT, U::STORAGE_BINDING | U::TEXTURE_BINDING | U::COPY_SRC),
            ui_depth: tex("hud depth", output, DEPTH_FORMAT, U::RENDER_ATTACHMENT).1,
        });
        // Different sizes: the history is of no use.
        self.history_reset = true;
    }

    /// The upscaler's last output (the scene at the output size, before the HUD): what frame
    /// generation is given as the HUD-less image. `None` before the first upscaled frame.
    #[cfg(feature = "gpu")]
    pub fn upscaled_output(&self) -> Option<&wgpu::Texture> {
        self.up.as_ref().map(|u| &u.out.0)
    }

    /// [`render`](Self::render) through a temporal upscaler: the backdrops and the 3D passes
    /// at the render size ([`upscale_render`](Self::upscale_render), jittered by
    /// [`jitter`](Self::jitter)) with motion vectors and depth, `up` makes the output-sized
    /// image, then the 2D overlays are drawn over it at `width`×`height` into `target`
    /// (format `self.format`). Turn on [`RenderOptions::motion_vectors`] before drawing the
    /// frame; MSAA and the render scale do not apply.
    #[cfg(feature = "gpu")]
    pub fn render_upscaled(&mut self, target: &wgpu::TextureView, width: u32, height: u32, clear: [f64; 4], up: &mut dyn crate::upscale::Upscaler) {
        self.upload_dirty();
        let render = (self.view.width as u32, self.view.height as u32);
        let (iw, ih) = render;
        let output = (width.max(1), height.max(1));
        self.ensure_targets(iw, ih, 1, false);
        self.ensure_blit();
        self.ensure_up_targets(render, output);
        let jitter = self.jitter;
        self.queue.write_buffer(&self.uniform_buf, 0, bytemuck::cast_slice(&self.uniforms(jitter)));
        if let Some((b, _)) = &self.ui_uniform {
            self.queue.write_buffer(b, 0, bytemuck::cast_slice(&self.uniforms([0.0; 2])));
        }
        let (verts, mvs, cmds, marks) = self.build_all(clear, true);
        let clear = if self.view.viewport.is_some() { [0.0, 0.0, 0.0, 1.0] } else { clear };
        if !verts.is_empty() {
            let bytes: &[u8] = bytemuck::cast_slice(&verts);
            if self.vbuf.as_ref().is_none_or(|b| b.size() < bytes.len() as u64) {
                self.vbuf = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("tl vertices"),
                    size: (bytes.len() as u64).next_power_of_two().max(4096),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            }
            self.queue.write_buffer(self.vbuf.as_ref().unwrap(), 0, bytes);
            let mb: &[u8] = bytemuck::cast_slice(&mvs);
            if self.mv_buf.as_ref().is_none_or(|b| b.size() < mb.len() as u64) {
                self.mv_buf = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("motion"),
                    size: (mb.len() as u64).next_power_of_two().max(4096),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            }
            self.queue.write_buffer(self.mv_buf.as_ref().unwrap(), 0, mb);
        }
        let split = marks[8] as usize;
        self.upscale_draws = cmds[..split].iter().filter(|c| c.depth == DepthMode::TestWrite).fold((0, 0), |(n, t), c| (n + 1, t + c.count / 3));
        let key = |c: &DrawCmd, scene: bool, fmt| (c.blend, c.depth, 1, fmt, scene);
        let mut keys = Vec::with_capacity(cmds.len());
        for (i, c) in cmds.iter().enumerate() {
            let k = if i < split { key(c, true, UPSCALE_COLOR_FORMAT) } else { key(c, false, self.format) };
            self.pipeline_for(k);
            self.bind_group(c.tex, c.wrap);
            keys.push((k, (c.tex.0, self.sampler_key(c.wrap))));
        }
        let gpu = self.prepare_gpu_meshes(UPSCALE_COLOR_FORMAT, 1, true, jitter);
        let scissor = self.view.viewport.map(|[x, y, w, h]| {
            let (x, y) = ((x as u32).min(iw - 1), (y as u32).min(ih - 1));
            [x, y, (w as u32).min(iw - x).max(1), (h as u32).min(ih - y).max(1)]
        });
        let ut = self.up.as_ref().unwrap();
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("upscale scene") });
        if gpu {
            self.gpu_mesh.as_mut().unwrap().pre_passes(&mut enc);
        }
        {
            let att = |view, c: wgpu::Color| {
                Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(c), store: wgpu::StoreOp::Store },
                })
            };
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mdx scene (upscaler input)"),
                color_attachments: &[
                    att(&ut.color.1, wgpu::Color { r: clear[0], g: clear[1], b: clear[2], a: clear[3] }),
                    att(&ut.motion.1, wgpu::Color::TRANSPARENT),
                    att(&ut.depth.1, wgpu::Color::TRANSPARENT),
                ],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth.as_ref().unwrap().0,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let (Some(vb), Some(mb)) = (&self.vbuf, &self.mv_buf) {
                pass.set_vertex_buffer(0, vb.slice(..));
                pass.set_vertex_buffer(1, mb.slice(..));
            }
            pass.set_bind_group(0, &self.uniform_bg, &[]);
            let mut scissored = false;
            let mut gpu_left = gpu;
            let mut water_left = gpu && self.gpu_mesh.as_ref().is_some_and(|g| g.has_water());
            let draw_gpu = |pass: &mut wgpu::RenderPass<'_>, scissored: &mut bool, water: bool| {
                if let Some([x, y, w, h]) = scissor {
                    pass.set_scissor_rect(x, y, w, h);
                    *scissored = true;
                }
                let bgs = &self.bind_groups;
                let gm = self.gpu_mesh.as_ref().unwrap();
                if water {
                    gm.draw_water(pass, UPSCALE_COLOR_FORMAT, 1, true, |t, w| &bgs[&(t.0, self.sampler_key(w))]);
                } else {
                    gm.draw(pass, UPSCALE_COLOR_FORMAT, 1, true, |t, w| &bgs[&(t.0, self.sampler_key(w))]);
                }
                if let (Some(vb), Some(mb)) = (&self.vbuf, &self.mv_buf) {
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.set_vertex_buffer(1, mb.slice(..));
                }
                pass.set_bind_group(0, &self.uniform_bg, &[]);
            };
            for (ci, (c, (k, bg))) in cmds[..split].iter().zip(&keys).enumerate() {
                if gpu_left && ci >= marks[1] as usize {
                    draw_gpu(&mut pass, &mut scissored, false);
                    gpu_left = false;
                }
                if water_left && ci >= marks[3] as usize {
                    draw_gpu(&mut pass, &mut scissored, true);
                    water_left = false;
                }
                if let Some([x, y, w, h]) = scissor {
                    if c.scene != scissored {
                        scissored = c.scene;
                        if scissored {
                            pass.set_scissor_rect(x, y, w, h);
                        } else {
                            pass.set_scissor_rect(0, 0, iw, ih);
                        }
                    }
                }
                pass.set_pipeline(&self.pipelines[k]);
                pass.set_bind_group(1, &self.bind_groups[bg], &[]);
                pass.draw(c.first..c.first + c.count, 0..1);
            }
            if gpu_left {
                draw_gpu(&mut pass, &mut scissored, false);
            }
            if water_left {
                draw_gpu(&mut pass, &mut scissored, true);
            }
        }
        // Native code reads the inputs and writes the output in these states.
        use wgpu::TextureUses as Tu;
        let tr = |texture, state| wgpu::TextureTransition { texture, selector: None, state };
        enc.transition_resources(
            core::iter::empty(),
            [tr(&ut.color.0, Tu::RESOURCE), tr(&ut.motion.0, Tu::RESOURCE), tr(&ut.depth.0, Tu::RESOURCE), tr(&ut.out.0, Tu::STORAGE_READ_WRITE)].into_iter(),
        );
        self.queue.submit([enc.finish()]);
        let inputs = crate::upscale::UpscaleInputs {
            color: &ut.color.0,
            depth: &ut.depth.0,
            motion: &ut.motion.0,
            output: &ut.out.0,
            render_size: render,
            output_size: output,
            jitter,
            reset: self.history_reset,
            camera: crate::upscale::UpscaleCamera::new(&self.view, self.motion.as_ref().and_then(|m| m.prev_view.as_ref())),
            frame: self.frame_no as u64,
        };
        let ok = up.upscale(&self.device, &self.queue, &inputs);
        if self.upscale_debug != UpscaleDebug::Off {
            self.debug_view(target);
        }
        let ut = self.up.as_ref().unwrap();
        // The upscaled image (or, failing that, the render-size scene stretched), then the HUD.
        let src = if ok { &ut.out.1 } else { &ut.color.1 };
        let (pipeline, sampler) = self.blit.as_ref().unwrap();
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("upscale blit"),
            layout: &self.tex_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(src) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
            ],
        });
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("upscale hud") });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("upscaled image"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if self.upscale_debug == UpscaleDebug::Off { wgpu::LoadOp::Clear(wgpu::Color::BLACK) } else { wgpu::LoadOp::Load },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if self.upscale_debug == UpscaleDebug::Off {
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &bg, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        if cmds.len() > split && self.upscale_debug == UpscaleDebug::Off {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hud"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &ut.ui_depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let Some(vb) = &self.vbuf {
                pass.set_vertex_buffer(0, vb.slice(..));
            }
            pass.set_bind_group(0, &self.ui_uniform.as_ref().unwrap().1, &[]);
            for (c, (k, bg)) in cmds[split..].iter().zip(&keys[split..]) {
                pass.set_pipeline(&self.pipelines[k]);
                pass.set_bind_group(1, &self.bind_groups[bg], &[]);
                pass.draw(c.first..c.first + c.count, 0..1);
            }
        }
        self.queue.submit([enc.finish()]);
    }

    /// Draws the [`UpscaleDebug`] view of this frame's upscaler inputs over `target`.
    #[cfg(feature = "gpu")]
    fn debug_view(&mut self, target: &wgpu::TextureView) {
        let Some(ut) = &self.up else { return };
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("upscale debug"), source: wgpu::ShaderSource::Wgsl(DEBUG_SHADER.into()) });
        let tex = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let bgl = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("upscale debug"), entries: &[tex(0), tex(1)] });
        let layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("upscale debug"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let entry = if self.upscale_debug == UpscaleDebug::Motion { "fs_motion" } else { "fs_depth" };
        let p = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("upscale debug"),
            layout: Some(&layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: self.format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("upscale debug"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&ut.motion.1) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&ut.depth.1) },
            ],
        });
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("upscale debug") });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("upscale debug"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&p);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([enc.finish()]);
    }

    /// Renders the queued frame to `target` and submits.
    #[cfg(feature = "gpu")]
    pub fn render(&mut self, target: &wgpu::TextureView, width: u32, height: u32, clear: [f64; 4]) {
        let mut enc = self.device.create_command_encoder(&Default::default());
        self.encode(&mut enc, target, width, height, clear);
        self.queue.submit([enc.finish()]);
    }

    /// Renders the queued frame offscreen (size = `begin_frame` size) and reads it back.
    #[cfg(feature = "gpu")]
    pub fn render_to_image(&mut self, clear: [f64; 4]) -> Result<RgbaImage, EngineError> {
        let (w, h) = self.output;
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        let mut enc = self.device.create_command_encoder(&Default::default());
        self.encode(&mut enc, &view, w, h, clear);
        let bpr = (w * 4).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (bpr * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely()).map_err(err)?;
        rx.recv().map_err(err)?.map_err(err)?;
        let data = slice.get_mapped_range().map_err(err)?;
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for row in 0..h {
            let o = (row * bpr) as usize;
            pixels.extend_from_slice(&data[o..o + (w * 4) as usize]);
        }
        drop(data);
        buf.unmap();
        if matches!(self.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb) {
            for p in pixels.as_chunks_mut::<4>().0 {
                p.swap(0, 2);
            }
        }
        Ok(RgbaImage { width: w, height: h, pixels })
    }
}
