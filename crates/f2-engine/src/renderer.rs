//! wgpu back end: rasterises the CPU-built TL triangle lists (`scene.rs`) with
//! the D3D7 render states MDX used, plus 2D quads, backdrops and text.

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
impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for EngineError {}
fn err(e: impl std::fmt::Display) -> EngineError {
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
    pub fn new_headless() -> Result<GpuContext, EngineError> {
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
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("f2-engine"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
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
pub(crate) enum DepthMode {
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
const ALPHA_NONZERO: f32 = 0.0;

pub(crate) struct DrawCmd {
    pub(crate) blend: Blend,
    pub(crate) depth: DepthMode,
    pub(crate) tex: TexHandle,
    pub(crate) wrap: bool,
    /// 3D frame triangle (scissored to the viewport when it is clamped).
    pub(crate) scene: bool,
    pub(crate) first: u32,
    pub(crate) count: u32,
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
    pub textures: TextureRegistry,
    pub view: View,
    pub options: RenderOptions,
    /// `SPRITECLIPLEFT/TOP/RIGHT/BOTTOM` (internal pixels) applied to unrotated sprite
    /// overlays (`DrawSpriteOverlay`); `None` = no clipping. Set per frame by the host.
    pub sprite_clip: Option<[f32; 4]>,
    frames: Frames,
    backdrops: Vec<Quad2D>,
    overlays: Vec<Quad2D>,
    fonts: Vec<(Font, TexHandle)>,
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
    pipelines: HashMap<(Blend, DepthMode, u32), wgpu::RenderPipeline>,
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
}

#[cfg(feature = "gpu")]
const SHADER: &str = r#"
struct U { size: vec2<f32>, persp: f32, fog_on: f32, fog_color: vec4<f32> };
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
};

@vertex fn vs(i: VIn) -> VOut {
    var o: VOut;
    // D3D7 TL pixel centres are at integer coordinates.
    let ndc = vec2<f32>((i.pos.x + 0.5) / u.size.x * 2.0 - 1.0, 1.0 - (i.pos.y + 0.5) / u.size.y * 2.0);
    var w = 1.0;
    if (u.persp > 0.5 && i.pos.w > 0.0) { w = 1.0 / i.pos.w; }
    o.pos = vec4<f32>(ndc * w, clamp(i.pos.z, 0.0, 1.0) * w, w);
    o.color = i.color;
    o.uv = i.uv;
    o.fog = i.fog;
    o.aref = i.aref;
    return o;
}

@fragment fn fs(i: VOut) -> @location(0) vec4<f32> {
    var c = textureSample(t, s, i.uv) * i.color;
    if (floor(c.a * 255.0 + 0.5) <= i.aref) { discard; }
    if (u.fog_on > 0.5) { c = vec4<f32>(mix(u.fog_color.rgb, c.rgb, i.fog), c.a); }
    return c;
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
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uniforms"),
            layout: &ulayout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform_buf.as_entire_binding() }],
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
            textures: TextureRegistry::new(),
            view: View::new(640, 480),
            options: RenderOptions::default(),
            frames: Frames::default(),
            backdrops: Vec::new(),
            overlays: Vec::new(),
            fonts: Vec::new(),
            white: TexHandle(0),
            gpu_images: Vec::new(),
            bind_groups: HashMap::new(),
            tex_layout,
            uniform_buf,
            uniform_bg,
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
            white: TexHandle(0),
            output: (640, 480),
            sprite_clip: None,
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

    /// Registers a font and uploads its glyph strip.
    pub fn add_font(&mut self, font: Font) -> FontId {
        let h = self.textures.add_image(font.width, font.height, font.rgba.clone());
        self.fonts.push((font, h));
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
        for i in std::mem::take(&mut self.textures.dirty) {
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
        let key = (blend, depth, samples);
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
            let p = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("mdx"),
                layout: Some(&self.pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &self.shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<TlVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attrs,
                    })],
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
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: self.format, blend: Some(state), write_mask: wgpu::ColorWrites::ALL })],
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
        let s = self.render_scale();
        let (iw, ih) = if s == 1.0 { (w, h) } else { (((w as f32 * s).round() as u32).max(1), ((h as f32 * s).round() as u32).max(1)) };
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
        self.frames.clear();
        self.backdrops.clear();
        self.overlays.clear();
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
    }

    /// Advances animated textures to `frame_count` game frames.
    pub fn update_animating_textures(&mut self, frame_count: u32) {
        self.textures.update_animating_textures(frame_count);
    }

    fn emit_ctx(&self, alpha_mul: f32) -> EmitCtx<'_> {
        EmitCtx { view: &self.view, opts: &self.options, textures: &self.textures, alpha_mul, override_tex: None, gouraud: None }
    }

    /// Transforms and draws an actor (`XformActor` + `DrawActor`).
    pub fn draw_actor(&mut self, model: &Model, actor: &mut Actor) {
        let mut frames = std::mem::take(&mut self.frames);
        let cx = EmitCtx { override_tex: actor.override_tex, gouraud: actor.gouraud, ..self.emit_ctx(actor.xlu.clamp(0.0, 1.0)) };
        draw_actor(&cx, model, actor, &mut frames);
        self.frames = frames;
    }

    /// Draws a static level model like `DrawLandscape` (key-0 pose).
    pub fn draw_landscape(&mut self, model: &Model) {
        let mut frames = std::mem::take(&mut self.frames);
        draw_landscape(&self.emit_ctx(1.0), model, &mut frames);
        self.frames = frames;
    }

    /// Pushes a pre-transformed triangle into a frame (`PushPolys`).
    pub fn push_tri(&mut self, frame: Frame, tri: Tri) {
        self.frames.push(frame, tri);
    }

    /// Number of 3D triangles queued so far this frame.
    pub fn queued_tris(&self) -> usize {
        self.frames.total()
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
        let (f, tex) = (&self.fonts[font.0].0, self.fonts[font.0].1);
        let mut cx = match centred_x {
            Some(c) => c - (f.string_width(&bytes, scale) / 2) as f32,
            None => x,
        };
        let (fw, fh) = (f.width as f32, f.height as f32);
        let mut quads = Vec::new();
        for &b in &bytes {
            let Some(g) = f.glyphs[b as usize] else { continue };
            let w = (g.width as f32 * scale).trunc();
            if g.left < f.width {
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

    pub(crate) fn build(&mut self, clear: [f64; 4]) -> (Vec<TlVertex>, Vec<DrawCmd>) {
        let mut verts: Vec<TlVertex> = Vec::with_capacity(self.frames.total() * 3 + 6 * (self.backdrops.len() + self.overlays.len() + 1));
        let mut cmds: Vec<DrawCmd> = Vec::new();
        let white = self.white;
        #[allow(clippy::too_many_arguments)]
        let push = |verts: &mut Vec<TlVertex>, cmds: &mut Vec<DrawCmd>, blend, depth, wrap, scene: bool, tex: Option<TexHandle>, tri: &[TlVertex]| {
            let tex = tex.unwrap_or(white);
            let first = verts.len() as u32;
            verts.extend_from_slice(tri);
            if let Some(last) = cmds.last_mut() {
                if last.blend == blend && last.depth == depth && last.tex == tex && last.wrap == wrap && last.scene == scene && last.first + last.count == first
                {
                    last.count += tri.len() as u32;
                    return;
                }
            }
            cmds.push(DrawCmd { blend, depth, tex, wrap, scene, first, count: tri.len() as u32 });
        };
        let quad_tris = |q: &Quad2D| -> [TlVertex; 6] {
            let col = q.color.map(|c| c as f32 / 255.0);
            let uvs = [[q.uv[0], q.uv[1]], [q.uv[2], q.uv[1]], [q.uv[2], q.uv[3]], [q.uv[0], q.uv[3]]];
            let v = |i: usize| TlVertex { sx: q.pos[i][0], sy: q.pos[i][1], sz: 0.0, rhw: 1.0, color: col, uv: uvs[i], fog: 1.0, alpha_ref: ALPHA_NONZERO };
            [v(0), v(1), v(2), v(2), v(3), v(0)]
        };
        if let Some([x, y, w, h]) = self.view.viewport {
            // Ultrawide clamp: the target is cleared black (pillars), the viewport gets the
            // scene's clear colour.
            let c = clear.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
            let q = Quad2D::rect(x, y, w, h, [0.0; 4], None, [c[0], c[1], c[2], 255], Blend::Semi);
            push(&mut verts, &mut cmds, q.blend, DepthMode::Off, false, false, q.tex, &quad_tris(&q));
        }
        for q in &self.backdrops {
            push(&mut verts, &mut cmds, q.blend, DepthMode::Off, false, false, q.tex, &quad_tris(q));
        }
        let sort = self.options.xlu_sort;
        // DrawAllFrames order and states.
        let passes: [(Frame, Blend, DepthMode, bool, f32, bool); 7] = [
            (Frame::Normal, Blend::Semi, DepthMode::TestWrite, false, ALPHA_TIGHT, false),
            (Frame::Wrap, Blend::Semi, DepthMode::TestWrite, true, ALPHA_TIGHT, false),
            (Frame::Xlu, Blend::Semi, DepthMode::Test, true, ALPHA_NONZERO, true),
            (Frame::Additive, Blend::Additive, DepthMode::Test, true, ALPHA_NONZERO, true),
            // DrawLoop's post-DrawAllFrames batches (sprites, special FX): submission order.
            (Frame::Late, Blend::Semi, DepthMode::Test, false, ALPHA_NONZERO, false),
            (Frame::LateAdditive, Blend::Additive, DepthMode::Test, false, ALPHA_NONZERO, false),
            (Frame::Overlay, Blend::Semi, DepthMode::Off, false, ALPHA_NONZERO, false),
        ];
        for (frame, blend, depth, wrap, aref, sorted) in passes {
            let tris = &mut self.frames.tris[frame as usize];
            if sorted && sort == XluSort::BackToFront {
                let key = |t: &Tri| t.v[0].sz + t.v[1].sz + t.v[2].sz;
                tris.sort_by(|a, b| key(b).total_cmp(&key(a)));
            }
            for t in tris.iter() {
                let mut v = t.v;
                for x in &mut v {
                    x.alpha_ref = aref;
                }
                push(&mut verts, &mut cmds, blend, depth, wrap, true, t.tex, &v);
            }
        }
        for q in &self.overlays {
            push(&mut verts, &mut cmds, q.blend, DepthMode::Off, false, false, q.tex, &quad_tris(q));
        }
        (verts, cmds)
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
        if scaled && self.blit.is_none() {
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
        let uni: [f32; 8] = [
            self.view.width,
            self.view.height,
            if self.options.perspective_correct { 1.0 } else { 0.0 },
            if fog.enabled { 1.0 } else { 0.0 },
            fog.color[0],
            fog.color[1],
            fog.color[2],
            1.0,
        ];
        self.queue.write_buffer(&self.uniform_buf, 0, bytemuck::cast_slice(&uni));
        let (verts, cmds) = self.build(clear);
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
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: clear[0], g: clear[1], b: clear[2], a: clear[3] }),
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
        for (c, key) in cmds.iter().zip(&keys) {
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
            pass.set_pipeline(&self.pipelines[&(c.blend, c.depth, samples)]);
            pass.set_bind_group(1, &self.bind_groups[key], &[]);
            pass.draw(c.first..c.first + c.count, 0..1);
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
