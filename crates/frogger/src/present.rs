//! Final present pass (host): used only when the frame cannot go straight to the window
//! surface, i.e. a brightness other than 100 % (`[video] brightness`, a final multiply) or
//! an explicit render resolution in borderless fullscreen (`[video] resolution`). The frame
//! is rendered into an offscreen target of the render size and drawn onto the surface,
//! scaled to fit with black bars, multiplied by the brightness. Without either option the
//! renderer targets the surface directly (unchanged path).

const SHADER: &str = r#"
@group(0) @binding(0) var t: texture_2d<f32>;
@group(0) @binding(1) var s: sampler;
@group(0) @binding(2) var<uniform> u: vec4<f32>;
struct VOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> VOut {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: VOut;
    o.pos = vec4<f32>(p * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    o.uv = p;
    return o;
}
@fragment fn fs(i: VOut) -> @location(0) vec4<f32> {
    let c = textureSampleLevel(t, s, i.uv, 0.0);
    return vec4<f32>(min(c.rgb * u.x, vec3<f32>(1.0)), 1.0);
}
"#;

pub struct Present {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    target: Option<(wgpu::Texture, wgpu::TextureView, wgpu::BindGroup, u32, u32)>,
    format: wgpu::TextureFormat,
}

/// The render size and whether the present pass is needed, for a `surface` of
/// `sw`×`sh`, the configured explicit `resolution` (applies in borderless fullscreen only)
/// and `brightness` (percent).
pub fn plan(surface: (u32, u32), borderless: bool, resolution: Option<[u32; 2]>, brightness: u32) -> ((u32, u32), bool) {
    let size = match (borderless, resolution) {
        (true, Some([w, h])) if w > 0 && h > 0 => (w, h),
        _ => surface,
    };
    (size, size != surface || brightness != 100)
}

/// Letterboxed destination rectangle of a `src` image on a `dst` surface.
pub fn fit(src: (u32, u32), dst: (u32, u32)) -> [f32; 4] {
    let (sw, sh) = (src.0.max(1) as f32, src.1.max(1) as f32);
    let (dw, dh) = (dst.0.max(1) as f32, dst.1.max(1) as f32);
    let k = (dw / sw).min(dh / sh);
    let (w, h) = ((sw * k).round().max(1.0), (sh * k).round().max(1.0));
    [((dw - w) / 2.0).floor(), ((dh - h) / 2.0).floor(), w, h]
}

impl Present {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Present {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("present"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty, count: None };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("present"),
            entries: &[
                entry(
                    0,
                    wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                ),
                entry(1, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
                entry(2, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }),
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("present"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("present"),
            layout: Some(&pl),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("present"), mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("present"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Present { pipeline, layout, sampler, uniform, target: None, format }
    }

    /// The offscreen target of `w`×`h` (created on demand).
    pub fn target(&mut self, device: &wgpu::Device, w: u32, h: u32) -> &wgpu::TextureView {
        if !matches!(&self.target, Some((_, _, _, tw, th)) if *tw == w && *th == h) {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("present source"),
                size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = tex.create_view(&Default::default());
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("present"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    wgpu::BindGroupEntry { binding: 2, resource: self.uniform.as_entire_binding() },
                ],
            });
            self.target = Some((tex, view, bg, w, h));
        }
        &self.target.as_ref().unwrap().1
    }

    /// Draws the offscreen target onto `surface` (`sw`×`sh`), letterboxed, times
    /// `brightness` percent.
    pub fn blit(&self, device: &wgpu::Device, queue: &wgpu::Queue, surface: &wgpu::TextureView, sw: u32, sh: u32, brightness: u32) {
        let Some((_, _, bg, w, h)) = &self.target else { return };
        let b = brightness as f32 / 100.0;
        let mut u = [0u8; 16];
        u[..4].copy_from_slice(&b.to_le_bytes());
        queue.write_buffer(&self.uniform, 0, &u);
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("present"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: surface,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let [x, y, vw, vh] = fit((*w, *h), (sw, sh));
            pass.set_viewport(x, y, vw, vh, 0.0, 1.0);
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, bg, &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit([enc.finish()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_and_fit() {
        assert_eq!(plan((1920, 1080), false, Some([640, 480]), 100), ((1920, 1080), false));
        assert_eq!(plan((1920, 1080), true, Some([640, 480]), 100), ((640, 480), true));
        assert_eq!(plan((1920, 1080), true, None, 120), ((1920, 1080), true));
        assert_eq!(fit((640, 480), (1920, 1080)), [240.0, 0.0, 1440.0, 1080.0]);
        assert_eq!(fit((1920, 1080), (1920, 1080)), [0.0, 0.0, 1920.0, 1080.0]);
    }
}
