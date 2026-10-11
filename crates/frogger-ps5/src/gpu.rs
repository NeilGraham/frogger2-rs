//! GPU presenter (feature `gpu`, `tools/ps5/build.sh --gpu`): the game's wgpu renderer on
//! the PS5 Vulkan driver (PS5_Vulkan's RADV, statically linked; `f2_engine::vkraw`, see
//! `tools/ps5/VULKAN.md`), presenting through a `VK_KHR_display` swapchain on VideoOut.
//! [`open`] fails cleanly when the driver cannot start; the caller then falls back to the
//! CPU renderer.

use console_host::sce::{self, Present};
use f2_engine::vkraw::VkPresenter;
use f2_engine::GpuContext;
use frogger::app::App;

pub struct GpuPresent {
    ctx: GpuContext,
    out: VkPresenter,
    failed: bool,
}

/// Starts the driver and the display swapchain.
pub fn open() -> Result<GpuPresent, String> {
    // Keeps Mesa's non-conformance banner off stderr (VULKAN.md §3).
    std::env::set_var("MESA_VK_IGNORE_CONFORMANCE_WARNING", "1");
    let ctx = GpuContext::new_ps5().map_err(|e| format!("Vulkan: {e}"))?;
    let out = VkPresenter::new_display(&ctx).map_err(|e| format!("VK_KHR_display swapchain: {e}"))?;
    let (w, h) = out.size();
    sce::log(&format!("GPU renderer: {} ({w}x{h}, {:?})", ctx.adapter.get_info().name, out.format()));
    Ok(GpuPresent { ctx, out, failed: false })
}

impl GpuPresent {
    fn submit(&mut self, draw: impl FnOnce(&mut wgpu::CommandEncoder, &wgpu::TextureView)) {
        if self.failed {
            return;
        }
        let r = match self.out.acquire(&self.ctx) {
            Ok((frame, view)) => {
                let mut enc = self.ctx.device.create_command_encoder(&Default::default());
                draw(&mut enc, view);
                self.out.present(&self.ctx, frame, enc)
            }
            Err(e) => Err(e),
        };
        if let Err(e) = r {
            // Logged once; later frames are dropped rather than spamming the log.
            sce::log(&format!("GPU present failed: {e}"));
            self.failed = true;
        }
    }
}

impl Present for GpuPresent {
    fn render_size(&self) -> (u32, u32) {
        self.out.size()
    }

    fn new_app(&self, data: frogger::data::Data, audio: f2_audio::AudioEngine, mapper: f2_platform::InputMapper) -> App {
        App::new(data, &self.ctx.device, &self.ctx.queue, self.out.format(), audio, mapper)
    }

    fn frame(&mut self, app: &mut App) {
        let (w, h) = self.out.size();
        let clear = app.clear_color();
        self.submit(|enc, view| app.r.encode(enc, view, w, h, clear));
    }

    fn fill(&mut self, rgba: u32) {
        let c = |s: u32| ((rgba >> s) & 0xff) as f64 / 255.0;
        let color = wgpu::Color { r: c(0), g: c(8), b: c(16), a: 1.0 };
        self.submit(|enc, view| {
            enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fill"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(color), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
        });
    }
}
