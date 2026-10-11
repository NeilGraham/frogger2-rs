//! Host test of the PS5 GPU path (`f2_engine::vkraw`, feature `ps5-vulkan`) with a real
//! swapchain: the context comes from a raw `vkGetInstanceProcAddr` (here the system
//! loader's, on the PS5 the statically linked RADV's) via `wgpu::Instance::from_hal`, and
//! frames are presented by `VkPresenter` on wgpu's own device/queue, exactly as on the
//! console. Only the surface differs: X11/Wayland here, `VK_KHR_display` there.
//!
//! ```sh
//! VK_INSTANCE_LAYERS=VK_LAYER_KHRONOS_validation \
//!   cargo run --release -p frogger --features ps5-vulkan --example vk_present -- game-data/usa 300
//! ```
//! Renders Garden 1 for N frames (default 300), then saves the last frame drawn with the
//! same renderer offscreen as `vk_present_last.png` (cwd) and exits.

use f2_audio::AudioEngine;
use f2_engine::vkraw::{ash, ash::vk, VkPresenter};
use f2_engine::GpuContext;
use f2_platform::window::raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use f2_platform::window::winit;
use f2_platform::{Config, Profile};
use frogger::app::{self, App, Step};
use frogger::data;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

struct Gfx {
    // Field order = drop order: presenter (swapchain) before the surface's instance.
    presenter: VkPresenter,
    app: App,
    ctx: GpuContext,
    surface: vk::SurfaceKHR,
    _window: Arc<Window>,
}

struct Host {
    data: Option<data::Data>,
    frames: u32,
    done: u32,
    start: Option<Instant>,
    gfx: Option<Gfx>,
    result: Result<(), String>,
}

unsafe fn create_surface(entry: &ash::Entry, instance: &ash::Instance, window: &Window) -> Result<vk::SurfaceKHR, String> {
    let d = window.display_handle().map_err(|e| e.to_string())?.as_raw();
    let w = window.window_handle().map_err(|e| e.to_string())?.as_raw();
    unsafe {
        match (d, w) {
            (RawDisplayHandle::Xlib(d), RawWindowHandle::Xlib(w)) => {
                let info = vk::XlibSurfaceCreateInfoKHR::default().dpy(d.display.map_or(std::ptr::null_mut(), |p| p.as_ptr()) as *mut _).window(w.window);
                ash::khr::xlib_surface::Instance::new(entry, instance).create_xlib_surface(&info, None)
            }
            (RawDisplayHandle::Xcb(d), RawWindowHandle::Xcb(w)) => {
                let info = vk::XcbSurfaceCreateInfoKHR::default().connection(d.connection.map_or(std::ptr::null_mut(), |p| p.as_ptr())).window(w.window.get());
                ash::khr::xcb_surface::Instance::new(entry, instance).create_xcb_surface(&info, None)
            }
            (RawDisplayHandle::Wayland(d), RawWindowHandle::Wayland(w)) => {
                let info = vk::WaylandSurfaceCreateInfoKHR::default().display(d.display.as_ptr()).surface(w.surface.as_ptr());
                ash::khr::wayland_surface::Instance::new(entry, instance).create_wayland_surface(&info, None)
            }
            _ => return Err("unsupported window system for this example".into()),
        }
        .map_err(|e| format!("create surface: {e}"))
    }
}

impl Host {
    fn init(&mut self, el: &ActiveEventLoop) -> Result<(), String> {
        let window = Arc::new(
            el.create_window(Window::default_attributes().with_title("vk_present").with_inner_size(winit::dpi::PhysicalSize::new(1280, 720)))
                .map_err(|e| e.to_string())?,
        );
        let size = window.inner_size();
        // SAFETY: desktop loader; the PS5 uses f2_engine::vkraw::ps5_entry() instead.
        let entry = unsafe { ash::Entry::load() }.map_err(|e| e.to_string())?;
        let ctx = GpuContext::from_vulkan_entry(entry, &[]).map_err(|e| e.0)?;
        let h = ctx.vulkan_handles().map_err(|e| e.0)?;
        let surface = unsafe { create_surface(&h.entry, &h.instance, &window)? };
        let presenter = unsafe { VkPresenter::from_vk_surface(&ctx, surface, size.width, size.height) }.map_err(|e| e.0)?;
        let (w, hgt) = presenter.size();
        eprintln!("vk_present: {} ({:?}), swapchain {w}x{hgt} {:?}", ctx.adapter.get_info().name, ctx.adapter.get_info().backend, presenter.format());
        let cfg = Config::default();
        let mut app = App::new(self.data.take().unwrap(), &ctx.device, &ctx.queue, presenter.format(), AudioEngine::null(44100), cfg.input_mapper());
        app.set_display(Profile::Enhanced, cfg.display_settings(Profile::Enhanced));
        app.win = app::WinInfo { size: (w, hgt), ..Default::default() };
        app.push_steps([Step::Level(0, 0)]);
        app.update(Duration::ZERO, &f2_platform::KeyboardState::default(), &[], true);
        if app.scene.is_none() {
            return Err("level failed to load".into());
        }
        self.gfx = Some(Gfx { presenter, app, ctx, surface, _window: window });
        self.start = Some(Instant::now());
        Ok(())
    }

    fn frame(&mut self) -> Result<bool, String> {
        let g = self.gfx.as_mut().unwrap();
        let (w, h) = g.presenter.size();
        g.app.tick(&[]);
        g.app.draw(w, h);
        let (frame, view) = g.presenter.acquire(&g.ctx).map_err(|e| e.0)?;
        let mut enc = g.ctx.device.create_command_encoder(&Default::default());
        let clear = g.app.clear_color();
        g.app.r.encode(&mut enc, view, w, h, clear);
        g.presenter.present(&g.ctx, frame, enc).map_err(|e| e.0)?;
        self.done += 1;
        if self.done < self.frames {
            return Ok(true);
        }
        let secs = self.start.unwrap().elapsed().as_secs_f64();
        eprintln!("vk_present: {} frames presented in {secs:.2} s ({:.1} fps)", self.done, self.done as f64 / secs);
        g.app.draw(w, h);
        let img = g.app.r.render_to_image(clear).map_err(|e| e.0)?;
        img.write_png("vk_present_last.png").map_err(|e| e.0)?;
        eprintln!("vk_present: wrote vk_present_last.png");
        Ok(false)
    }
}

impl ApplicationHandler for Host {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.gfx.is_none() && self.result.is_ok() {
            if let Err(e) = self.init(el) {
                self.result = Err(e);
                el.exit();
            }
        }
    }
    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, ev: WindowEvent) {
        if matches!(ev, WindowEvent::CloseRequested) {
            el.exit();
        }
    }
    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if self.gfx.is_none() {
            return;
        }
        match self.frame() {
            Ok(true) => {}
            Ok(false) => el.exit(),
            Err(e) => {
                self.result = Err(e);
                el.exit();
            }
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().unwrap_or_else(|| "game-data/usa".into()));
    let frames = args.next().and_then(|s| s.parse().ok()).unwrap_or(300);
    let data = data::Data::locate(&Config::default(), Some(root)).expect("game data");
    let el = EventLoop::new().expect("event loop");
    el.set_control_flow(winit::event_loop::ControlFlow::Poll);
    let mut host = Host { data: Some(data), frames, done: 0, start: None, gfx: None, result: Ok(()) };
    el.run_app(&mut host).expect("event loop");
    if let Some(g) = host.gfx.take() {
        let (ctx, surface) = (g.ctx.vulkan_handles().ok(), g.surface);
        drop(g.presenter);
        if let Some(h) = ctx {
            unsafe { ash::khr::surface::Instance::new(&h.entry, &h.instance).destroy_surface(surface, None) };
        }
    }
    if let Err(e) = host.result {
        eprintln!("vk_present: {e}");
        std::process::exit(1);
    }
}
