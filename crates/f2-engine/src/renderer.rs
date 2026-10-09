//! wgpu back end: rasterises the CPU-built TL triangle lists (`scene.rs`) with
//! the D3D7 render states MDX used, plus 2D quads, backdrops and text.

use crate::anim::Actor;
use crate::font::Font;
use crate::scene::*;
use crate::textures::{TexHandle, TextureRegistry};
use f2_formats::objbank::Model;
use f2_formats::texture::Texture;
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
pub struct GpuContext {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

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
pub struct WindowSurface {
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
}

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
enum DepthMode {
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

struct GpuImage {
    view: wgpu::TextureView,
}

/// Size of the original's virtual overlay space (`OVERLAY_X = rXRes/4096`).
pub const OVERLAY_SPACE: f32 = 4096.0;

/// Alpha-test references: `tightAlphaCmpRS` (> 0x64) and `normalAlphaCmpRS` (!= 0).
const ALPHA_TIGHT: f32 = 100.0;
const ALPHA_NONZERO: f32 = 0.0;

struct DrawCmd {
    blend: Blend,
    depth: DepthMode,
    tex: TexHandle,
    wrap: bool,
    first: u32,
    count: u32,
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

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// The MDX replacement renderer.
///
/// Per frame: [`begin_frame`](Self::begin_frame), set the camera, queue
/// backdrops / actors / landscape / overlays / text, then
/// [`render`](Self::render) (to any texture view) or
/// [`render_to_image`](Self::render_to_image) (headless).
pub struct Renderer {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub format: wgpu::TextureFormat,
    pub textures: TextureRegistry,
    pub view: View,
    pub options: RenderOptions,
    frames: Frames,
    backdrops: Vec<Quad2D>,
    overlays: Vec<Quad2D>,
    fonts: Vec<(Font, TexHandle)>,
    white: TexHandle,
    gpu_images: Vec<GpuImage>,
    bind_groups: HashMap<(u32, bool, bool), wgpu::BindGroup>,
    tex_layout: wgpu::BindGroupLayout,
    uniform_buf: wgpu::Buffer,
    uniform_bg: wgpu::BindGroup,
    pipeline_layout: wgpu::PipelineLayout,
    shader: wgpu::ShaderModule,
    pipelines: HashMap<(Blend, DepthMode), wgpu::RenderPipeline>,
    samplers: [wgpu::Sampler; 4],
    depth: Option<(wgpu::TextureView, u32, u32)>,
    vbuf: Option<wgpu::Buffer>,
}

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

impl Renderer {
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
        let sampler = |wrap: bool, linear: bool| {
            let am = if wrap { wgpu::AddressMode::Repeat } else { wgpu::AddressMode::ClampToEdge };
            let f = if linear { wgpu::FilterMode::Linear } else { wgpu::FilterMode::Nearest };
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: None,
                address_mode_u: am,
                address_mode_v: am,
                address_mode_w: am,
                mag_filter: f,
                min_filter: f,
                ..Default::default()
            })
        };
        let samplers = [sampler(false, false), sampler(false, true), sampler(true, false), sampler(true, true)];
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
            samplers,
            depth: None,
            vbuf: None,
        };
        r.white = r.textures.add_image(1, 1, vec![255; 4]);
        r
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

    fn upload_dirty(&mut self) {
        for i in std::mem::take(&mut self.textures.dirty) {
            let img = &self.textures.images[i as usize];
            let size = wgpu::Extent3d { width: img.width, height: img.height, depth_or_array_layers: 1 };
            let tex = self.device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                &img.rgba,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(img.width * 4), rows_per_image: Some(img.height) },
                size,
            );
            let view = tex.create_view(&Default::default());
            let gi = GpuImage { view };
            let idx = i as usize;
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

    fn bind_group(&mut self, tex: TexHandle, wrap: bool) -> &wgpu::BindGroup {
        let linear = self.options.linear_filter;
        let key = (tex.0, wrap, linear);
        if !self.bind_groups.contains_key(&key) {
            let s = &self.samplers[(wrap as usize) * 2 + linear as usize];
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

    fn pipeline(&mut self, blend: Blend, depth: DepthMode) -> &wgpu::RenderPipeline {
        let key = (blend, depth);
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
                multisample: Default::default(),
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
    pub fn begin_frame(&mut self, width: u32, height: u32) {
        self.view.width = width.max(1) as f32;
        self.view.height = height.max(1) as f32;
        self.frames.clear();
        self.backdrops.clear();
        self.overlays.clear();
    }

    pub fn set_camera(&mut self, cam: &Camera) {
        self.view.set_camera(cam);
    }

    /// Advances animated textures to `frame_count` game frames.
    pub fn update_animating_textures(&mut self, frame_count: u32) {
        self.textures.update_animating_textures(frame_count);
    }

    fn emit_ctx(&self, alpha_mul: f32) -> EmitCtx<'_> {
        EmitCtx { view: &self.view, opts: &self.options, textures: &self.textures, alpha_mul }
    }

    /// Transforms and draws an actor (`XformActor` + `DrawActor`).
    pub fn draw_actor(&mut self, model: &Model, actor: &mut Actor) {
        let mut frames = std::mem::take(&mut self.frames);
        draw_actor(&self.emit_ctx(actor.xlu.clamp(0.0, 1.0)), model, actor, &mut frames);
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
    pub fn draw_backdrop(&mut self, tex: TexHandle) {
        let (w, h) = (self.view.width, self.view.height);
        self.backdrops.push(Quad2D::rect(0.0, 0.0, w, h, [0.0, 0.0, 1.0, 1.0], Some(tex), [255; 4], Blend::Semi));
    }

    /// A 2D quad in screen pixels, drawn after the 3D frames in submission order.
    pub fn draw_quad(&mut self, q: Quad2D) {
        self.overlays.push(q);
    }

    /// Port of `DrawSpriteOverlay` / `DrawSpriteOverlayRotating`
    /// (pcsprite.c, mdxSprite.cpp:164) in the 4096×4096 virtual overlay space.
    /// `angle` in radians (the caller converts from game angle units).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_sprite_overlay(&mut self, x: f32, y: f32, w: f32, h: f32, uv: [f32; 4], tex: Option<TexHandle>, color: [u8; 4], blend: Blend, angle: f32) {
        let (sx, sy) = (self.view.width / OVERLAY_SPACE, self.view.height / OVERLAY_SPACE);
        let (x, y, w, h) = (x * sx, y * sy, w * sx, h * sy);
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
    /// the XLU frame so it is depth-tested against the scene.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_alpha_sprite(&mut self, x: f32, y: f32, sz: f32, xs: f32, ys: f32, uv: [f32; 4], tex: Option<TexHandle>, color: [u8; 4]) {
        let col = color.map(|c| c as f32 / 255.0);
        let v = |sx, sy, u, vv| TlVertex { sx, sy, sz, rhw: 1.0, color: col, uv: [u, vv], fog: 1.0, alpha_ref: 0.0 };
        let (x2, y2) = (x + xs, y + ys);
        let q = [v(x, y, uv[0], uv[1]), v(x2, y, uv[2], uv[1]), v(x2, y2, uv[2], uv[3]), v(x, y2, uv[0], uv[3])];
        let tex = tex.or(Some(self.white));
        self.frames.push(Frame::Xlu, Tri { v: [q[0], q[1], q[2]], tex });
        self.frames.push(Frame::Xlu, Tri { v: [q[2], q[3], q[0]], tex });
    }

    /// Port of `DrawFontStringAtLoc` (mdxFont.cpp) at pixel position `(x, y)`.
    /// `text` is Latin-1 (chars above U+00FF draw nothing). With `centred_x`,
    /// the string is centred on that x. Returns the end x.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_text(&mut self, font: FontId, x: f32, y: f32, text: &str, color: [u8; 4], scale: f32, centred_x: Option<f32>) -> f32 {
        let bytes: Vec<u8> = text.chars().map(|c| if (c as u32) < 256 { c as u8 } else { 0 }).collect();
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
    pub fn draw_text_overlay(&mut self, font: FontId, x: f32, y: f32, text: &str, color: [u8; 4], scale: f32) -> f32 {
        let (sx, sy) = (self.view.width / OVERLAY_SPACE, self.view.height / OVERLAY_SPACE);
        self.draw_text(font, x * sx, y * sy, text, color, scale, None) / sx
    }

    // ---------------------------------------------------------------- submission

    fn build(&mut self) -> (Vec<TlVertex>, Vec<DrawCmd>) {
        let mut verts: Vec<TlVertex> = Vec::with_capacity(self.frames.total() * 3 + 6 * (self.backdrops.len() + self.overlays.len()));
        let mut cmds: Vec<DrawCmd> = Vec::new();
        let white = self.white;
        let push = |verts: &mut Vec<TlVertex>, cmds: &mut Vec<DrawCmd>, blend, depth, wrap, tex: Option<TexHandle>, tri: &[TlVertex]| {
            let tex = tex.unwrap_or(white);
            let first = verts.len() as u32;
            verts.extend_from_slice(tri);
            if let Some(last) = cmds.last_mut() {
                if last.blend == blend && last.depth == depth && last.tex == tex && last.wrap == wrap && last.first + last.count == first {
                    last.count += tri.len() as u32;
                    return;
                }
            }
            cmds.push(DrawCmd { blend, depth, tex, wrap, first, count: tri.len() as u32 });
        };
        let quad_tris = |q: &Quad2D| -> [TlVertex; 6] {
            let col = q.color.map(|c| c as f32 / 255.0);
            let uvs = [[q.uv[0], q.uv[1]], [q.uv[2], q.uv[1]], [q.uv[2], q.uv[3]], [q.uv[0], q.uv[3]]];
            let v = |i: usize| TlVertex { sx: q.pos[i][0], sy: q.pos[i][1], sz: 0.0, rhw: 1.0, color: col, uv: uvs[i], fog: 1.0, alpha_ref: ALPHA_NONZERO };
            [v(0), v(1), v(2), v(2), v(3), v(0)]
        };
        for q in &self.backdrops {
            push(&mut verts, &mut cmds, q.blend, DepthMode::Off, false, q.tex, &quad_tris(q));
        }
        let sort = self.options.xlu_sort;
        // DrawAllFrames order and states.
        let passes: [(Frame, Blend, DepthMode, bool, f32, bool); 5] = [
            (Frame::Normal, Blend::Semi, DepthMode::TestWrite, false, ALPHA_TIGHT, false),
            (Frame::Wrap, Blend::Semi, DepthMode::TestWrite, true, ALPHA_TIGHT, false),
            (Frame::Xlu, Blend::Semi, DepthMode::Test, true, ALPHA_NONZERO, true),
            (Frame::Additive, Blend::Additive, DepthMode::Test, true, ALPHA_NONZERO, true),
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
                push(&mut verts, &mut cmds, blend, depth, wrap, t.tex, &v);
            }
        }
        for q in &self.overlays {
            push(&mut verts, &mut cmds, q.blend, DepthMode::Off, false, q.tex, &quad_tris(q));
        }
        (verts, cmds)
    }

    fn ensure_depth(&mut self, w: u32, h: u32) {
        if matches!(self.depth, Some((_, dw, dh)) if dw == w && dh == h) {
            return;
        }
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        self.depth = Some((tex.create_view(&Default::default()), w, h));
    }

    /// Encodes the queued frame into `encoder`, targeting `target`
    /// (`width`×`height`, format `self.format`). `clear` = background colour.
    pub fn encode(&mut self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, width: u32, height: u32, clear: [f64; 4]) {
        self.upload_dirty();
        self.ensure_depth(width, height);
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
        let (verts, cmds) = self.build();
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
        for c in &cmds {
            self.pipeline(c.blend, c.depth);
            self.bind_group(c.tex, c.wrap);
        }
        let linear = self.options.linear_filter;
        let depth_view = &self.depth.as_ref().unwrap().0;
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("mdx frame"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: clear[0], g: clear[1], b: clear[2], a: clear[3] }),
                    store: wgpu::StoreOp::Store,
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
        for c in &cmds {
            pass.set_pipeline(&self.pipelines[&(c.blend, c.depth)]);
            pass.set_bind_group(1, &self.bind_groups[&(c.tex.0, c.wrap, linear)], &[]);
            pass.draw(c.first..c.first + c.count, 0..1);
        }
    }

    /// Renders the queued frame to `target` and submits.
    pub fn render(&mut self, target: &wgpu::TextureView, width: u32, height: u32, clear: [f64; 4]) {
        let mut enc = self.device.create_command_encoder(&Default::default());
        self.encode(&mut enc, target, width, height, clear);
        self.queue.submit([enc.finish()]);
    }

    /// Renders the queued frame offscreen (size = `begin_frame` size) and reads it back.
    pub fn render_to_image(&mut self, clear: [f64; 4]) -> Result<RgbaImage, EngineError> {
        let (w, h) = (self.view.width as u32, self.view.height as u32);
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
