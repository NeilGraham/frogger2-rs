//! `f2-viewer`: interactive asset viewer for imported Frogger 2 data.
//!
//! ```text
//! cargo run --release -p f2-viewer -- [rendition-or-data-root]
//! cargo run --release -p f2-viewer -- [dir] --screenshot model <bank> <model> [anim] [frame] out.png
//! cargo run --release -p f2-viewer -- [dir] --screenshot level <name> out.png
//! cargo run --release -p f2-viewer -- [dir] --screenshot texture <bank> <index|name> out.png
//! cargo run --release -p f2-viewer -- [dir] --screenshot video <name> <frame> out.png
//! ```
//! Options: `--size WxH` (default 800x600), `--mode <1-5|name>` (window start mode).

mod app;
mod data;

use app::{App, Input, Mode};
use f2_audio::AudioEngine;
use f2_engine::{GpuContext, WindowSurface};
use f2_platform::window::{self, winit, Flow, WindowApp, WindowConfig};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};

fn usage() -> ! {
    eprintln!(
        "usage: f2-viewer [rendition-dir|data-root] [--size WxH] [--mode N]\n\
         \x20      f2-viewer [dir] --screenshot model <bank> <model> [anim] [frame] out.png\n\
         \x20      f2-viewer [dir] --screenshot level <name> out.png\n\
         \x20      f2-viewer [dir] --screenshot texture <bank> <index|name> out.png\n\
         \x20      f2-viewer [dir] --screenshot video <name> <frame> out.png"
    );
    std::process::exit(2);
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut size = (800u32, 600u32);
    if let Some(i) = args.iter().position(|a| a == "--size") {
        let v = args.get(i + 1).cloned().unwrap_or_else(|| usage());
        let (w, h) = v.split_once('x').unwrap_or_else(|| usage());
        size = (w.parse().unwrap_or_else(|_| usage()), h.parse().unwrap_or_else(|_| usage()));
        args.drain(i..i + 2);
    }
    let mut start_mode = None;
    if let Some(i) = args.iter().position(|a| a == "--mode") {
        start_mode = Some(args.get(i + 1).and_then(|m| Mode::parse(m)).unwrap_or_else(|| usage()));
        args.drain(i..i + 2);
    }
    if args.iter().any(|a| a == "-h" || a == "--help") {
        usage();
    }
    let shot = args.iter().position(|a| a == "--screenshot").map(|i| args.split_off(i)[1..].to_vec());
    let dir = args.first().map(PathBuf::from);
    let data = match data::Data::open(dir) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("f2-viewer: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("f2-viewer: data {}", data.rendition.display());
    let res = match shot {
        Some(s) => screenshot(data, &s, size),
        None => run_window(data, size, start_mode),
    };
    if let Err(e) = res {
        eprintln!("f2-viewer: {e}");
        std::process::exit(1);
    }
}

fn screenshot(data: data::Data, a: &[String], size: (u32, u32)) -> Result<(), String> {
    if a.len() < 2 {
        usage();
    }
    let out = a.last().unwrap();
    let ctx = GpuContext::new_headless().map_err(|e| e.to_string())?;
    let mut app = App::new(data, &ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm, AudioEngine::null(44100));
    let mode = Mode::parse(&a[0]).unwrap_or_else(|| usage());
    let rest = &a[1..a.len() - 1];
    let num = |i: usize| rest.get(i).and_then(|s| s.parse::<f32>().ok());
    match mode {
        Mode::Models => {
            if rest.len() < 2 {
                usage();
            }
            app.select_model(&rest[0], &rest[1])?;
            app.set_anim(num(2).unwrap_or(0.0) as usize, num(3).unwrap_or(0.0));
        }
        Mode::Levels => app.select_level(rest.first().unwrap_or_else(|| usage()))?,
        Mode::Textures => app.select_texture(rest.first().unwrap_or_else(|| usage()), rest.get(1).map_or("0", |s| s.as_str()))?,
        Mode::Videos => app.select_video(rest.first().unwrap_or_else(|| usage()), num(1).unwrap_or(0.0) as u32)?,
        Mode::Sounds => return Err("sounds have no screenshot".into()),
    }
    // Two frames: the first applies the selection and frames the camera.
    let input = Input::default();
    app.frame(&input, 0.0, size.0, size.1);
    if mode == Mode::Models {
        // Re-apply the requested animation time (the first frame resets on model change).
        app.set_anim(num(2).unwrap_or(0.0) as usize, num(3).unwrap_or(0.0));
    }
    app.frame(&input, 0.0, size.0, size.1);
    let tris = app.r.queued_tris();
    let img = app.r.render_to_image(app.clear_color()).map_err(|e| e.to_string())?;
    img.write_png(out).map_err(|e| e.to_string())?;
    eprintln!("f2-viewer: wrote {out} ({}x{}, {tris} 3D triangles)", img.width, img.height);
    Ok(())
}

// ------------------------------------------------------------------ window

struct Gfx {
    ctx: GpuContext,
    surface: WindowSurface,
}

struct Viewer {
    data: Option<data::Data>,
    app: Option<App>,
    gfx: Option<Gfx>,
    start_mode: Option<Mode>,
    input: Input,
    mouse: Option<[f32; 2]>,
    left: bool,
    right: bool,
    last: Instant,
    error: Option<String>,
    /// `F2_VIEWER_KEYS="RETURN@1.5,SPACE@3"`: scripted key presses (DIK names, seconds) for smoke tests.
    script: Vec<(f32, u8)>,
    started: Instant,
}

fn parse_script(s: &str) -> Vec<(f32, u8)> {
    let mut v: Vec<(f32, u8)> = s
        .split(',')
        .filter_map(|e| {
            let (k, t) = e.trim().split_once('@')?;
            let code = f2_platform::dik::parse(k).or_else(|| f2_platform::dik::parse(&format!("KEY_{k}")))?;
            Some((t.parse().ok()?, code))
        })
        .collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v
}

impl WindowApp for Viewer {
    fn init(&mut self, window: &Arc<window::Window>) {
        if self.gfx.is_some() {
            return;
        }
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(window.clone())));
        let res = (|| -> Result<Gfx, String> {
            let surface = instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
            let ctx = GpuContext::with_instance(instance, Some(&surface)).map_err(|e| e.to_string())?;
            let surface = WindowSurface::new(&ctx, surface, size.width, size.height).map_err(|e| e.to_string())?;
            let mut surface = surface;
            if let Ok(m) = std::env::var("F2_PRESENT_MODE") {
                surface.config.present_mode = match m.as_str() {
                    "fifo" => wgpu::PresentMode::Fifo,
                    "immediate" => wgpu::PresentMode::Immediate,
                    "mailbox" => wgpu::PresentMode::Mailbox,
                    _ => wgpu::PresentMode::AutoVsync,
                };
                surface.surface.configure(&ctx.device, &surface.config);
            }
            Ok(Gfx { ctx, surface })
        })();
        match res {
            Ok(g) => {
                let audio = AudioEngine::with_default_output();
                eprintln!(
                    "f2-viewer: {} {:?} ({:?}), audio {}",
                    g.ctx.adapter.get_info().name,
                    g.ctx.adapter.get_info().backend,
                    g.surface.config.format,
                    if audio.is_realtime() { "on" } else { "muted" }
                );
                let mut app = App::new(self.data.take().expect("data"), &g.ctx.device, &g.ctx.queue, g.surface.config.format, audio);
                if let Some(m) = self.start_mode {
                    app.mode = m;
                }
                self.app = Some(app);
                self.gfx = Some(g);
            }
            Err(e) => self.error = Some(e),
        }
        self.last = Instant::now();
    }

    fn event(&mut self, _w: &Arc<window::Window>, event: &WindowEvent) -> Flow {
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                let p = [position.x as f32, position.y as f32];
                if let Some(m) = self.mouse {
                    let d = [p[0] - m[0], p[1] - m[1]];
                    if self.left {
                        self.input.drag[0] += d[0];
                        self.input.drag[1] += d[1];
                    }
                    if self.right {
                        self.input.rdrag[0] += d[0];
                        self.input.rdrag[1] += d[1];
                    }
                }
                self.mouse = Some(p);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let on = *state == ElementState::Pressed;
                match button {
                    MouseButton::Left => self.left = on,
                    MouseButton::Right => self.right = on,
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.input.wheel += match delta {
                    MouseScrollDelta::LineDelta(_, y) => *y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                };
            }
            _ => {}
        }
        Flow::Continue
    }

    fn resized(&mut self, _w: &Arc<window::Window>, size: winit::dpi::PhysicalSize<u32>) {
        if let Some(g) = &mut self.gfx {
            g.surface.resize(&g.ctx.device, size.width, size.height);
        }
    }

    fn frame(&mut self, frame: window::Frame<'_>) -> Flow {
        if self.error.is_some() {
            return Flow::Exit;
        }
        let (Some(app), Some(g)) = (&mut self.app, &mut self.gfx) else { return Flow::Continue };
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.1);
        self.last = now;
        self.input.pressed = frame.keyboard.pressed.clone();
        let t = self.started.elapsed().as_secs_f32();
        while self.script.first().is_some_and(|e| e.0 <= t) {
            let (_, k) = self.script.remove(0);
            eprintln!("f2-viewer: scripted key {} at {t:.2}s", f2_platform::dik::name(k));
            self.input.pressed.push(k);
        }
        self.input.down = frame.keyboard.down;
        let (w, h) = (g.surface.config.width, g.surface.config.height);
        app.frame(&self.input, dt, w, h);
        self.input.drag = [0.0; 2];
        self.input.rdrag = [0.0; 2];
        self.input.wheel = 0.0;
        if app.quit {
            return Flow::Exit;
        }
        let tex = match g.surface.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                let s = frame.window.inner_size();
                g.surface.resize(&g.ctx.device, s.width, s.height);
                return Flow::Continue;
            }
            _ => {
                std::thread::sleep(std::time::Duration::from_millis(5));
                return Flow::Continue;
            }
        };
        let view = tex.texture.create_view(&Default::default());
        app.r.render(&view, w, h, app.clear_color());
        frame.window.pre_present_notify();
        g.ctx.queue.present(tex);
        Flow::Continue
    }
}

/// WSLg's Weston 9 (rdprail shell) segfaults with winit's native Wayland client, taking every
/// GUI app down with it; under WSL prefer X11 (XWayland) unless `F2_VIEWER_WAYLAND=1`.
fn prefer_x11_on_wsl() {
    let wsl =
        std::env::var_os("WSL_DISTRO_NAME").is_some() || std::fs::read_to_string("/proc/version").is_ok_and(|v| v.to_ascii_lowercase().contains("microsoft"));
    if cfg!(target_os = "linux") && wsl && std::env::var_os("DISPLAY").is_some() && std::env::var_os("F2_VIEWER_WAYLAND").is_none() {
        // Single-threaded at this point (before the event loop, audio and decoder threads).
        std::env::remove_var("WAYLAND_DISPLAY");
    }
}

fn run_window(data: data::Data, size: (u32, u32), start_mode: Option<Mode>) -> Result<(), String> {
    prefer_x11_on_wsl();
    let cfg = WindowConfig { title: "Frogger 2 asset viewer".into(), width: size.0, height: size.1, fullscreen: false, resizable: true };
    let v = Viewer {
        data: Some(data),
        app: None,
        gfx: None,
        start_mode,
        input: Input::default(),
        mouse: None,
        left: false,
        right: false,
        last: Instant::now(),
        error: None,
        script: parse_script(&std::env::var("F2_VIEWER_KEYS").unwrap_or_default()),
        started: Instant::now(),
    };
    let v = window::run(cfg, v)?;
    match v.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
