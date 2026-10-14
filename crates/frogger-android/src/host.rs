//! The Android activity: paths, event loop, GPU surface lifecycle (the window goes away
//! whenever the app is backgrounded), input and the frame loop around `frogger::app::App`.

use crate::import;
use crate::touch::{Phase, TouchPad};
use f2_audio::AudioEngine;
use f2_engine::{GpuContext, WindowSurface};
use f2_platform::window::{self, winit, Flow, WindowApp, WindowConfig};
use f2_platform::{Config, Controller, ControllerFamily, PadButton, Profile, RawPad};
use frogger::app::{App, Step};
use frogger::{data, gameplay};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::event::{ElementState, TouchPhase, WindowEvent};
use winit::event_loop::EventLoop;
use winit::keyboard::{NativeKeyCode, PhysicalKey};
use winit::platform::android::activity::AndroidApp;
use winit::platform::android::EventLoopBuilderExtAndroid;

pub fn main(android: AndroidApp) {
    // Everything user-visible lives in the external files dir (reachable with adb / USB):
    // `data/` (imported game), `config.toml`, `saves/`, `mods/`.
    let internal = android.internal_data_path().unwrap_or_else(|| PathBuf::from("/data/local/tmp/frogger2"));
    let files = android.external_data_path().unwrap_or_else(|| internal.clone());
    // Single-threaded here: before the logcat thread and the event loop.
    std::env::set_var("F2_DATA_ROOT", files.join("data"));
    std::env::set_var("HOME", &internal);
    f2_platform::paths::set_config_dir(files.clone());
    crate::logcat::redirect_stdio();
    std::panic::set_hook(Box::new(|info| {
        crate::logcat::write(&format!("frogger: panic: {info}"));
        crate::logcat::write(&format!("{}", std::backtrace::Backtrace::force_capture()));
    }));
    eprintln!("frogger: android, files {}", files.display());

    let cfg = Config::load_default().unwrap_or_else(|e| {
        eprintln!("frogger: config: {e} (using defaults)");
        Config::default()
    });
    let data = match data::Data::locate(&cfg, None) {
        Ok(mut d) => {
            eprintln!("frogger: data {}", d.rendition.display());
            if let Err(e) = d.enable_mods(&cfg.mods_dir(), &cfg.mods.enabled) {
                eprintln!("frogger: mods: {e}");
            }
            Some(d)
        }
        Err(e) => {
            eprintln!("frogger: {e}");
            None
        }
    };
    // JavaVM / FroggerActivity global refs (valid for the process; used by the file picker).
    let jni_ptrs = (android.vm_as_ptr() as usize, android.activity_as_ptr() as usize);
    let el = match EventLoop::builder().with_android_app(android).build() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("frogger: event loop: {e}");
            return;
        }
    };
    let host = Host {
        save_path: f2_platform::paths::save_dir().join("savegame.dat"),
        cfg,
        files,
        data,
        import: None,
        gfx: None,
        app: None,
        touch: None,
        gamepad: RawPad::default(),
        prev_pad: RawPad::default(),
        jni_ptrs,
        last: Instant::now(),
        error: None,
    };
    let wc = WindowConfig { title: "Frogger 2".into(), ..WindowConfig::default() };
    match window::run_with(el, wc, host) {
        Ok(h) => {
            if let Some(e) = h.error {
                eprintln!("frogger: {e}");
            }
        }
        Err(e) => eprintln!("frogger: {e}"),
    }
    eprintln!("frogger: exit");
}

struct Gfx {
    ctx: GpuContext,
    /// `None` while the app is in the background.
    surface: Option<WindowSurface>,
}

struct Host {
    cfg: Config,
    files: PathBuf,
    save_path: PathBuf,
    /// Located game data waiting for the GPU (the app is created on the first window).
    data: Option<data::Data>,
    import: Option<import::Screen>,
    gfx: Option<Gfx>,
    app: Option<App>,
    touch: Option<TouchPad>,
    /// Android gamepad buttons (key events) held now.
    gamepad: RawPad,
    /// Player 1's pad last frame (press edges become `App::pad_event`s).
    prev_pad: RawPad,
    /// `JavaVM*` and the activity's `jobject`.
    jni_ptrs: (usize, usize),
    last: Instant,
    error: Option<String>,
}

/// Android `KEYCODE_BUTTON_*` -> pad button.
fn gamepad_button(code: u32) -> Option<PadButton> {
    Some(match code {
        96 => PadButton::South,  // BUTTON_A
        97 => PadButton::East,   // BUTTON_B
        99 => PadButton::West,   // BUTTON_X
        100 => PadButton::North, // BUTTON_Y
        102 => PadButton::L1,
        103 => PadButton::R1,
        104 => PadButton::L2,
        105 => PadButton::R2,
        106 => PadButton::L3, // BUTTON_THUMBL
        107 => PadButton::R3,
        108 => PadButton::Start,
        109 => PadButton::Select,
        110 => PadButton::Mode,
        _ => return None,
    })
}

impl Host {
    fn make_surface(&mut self, window: &Arc<window::Window>) -> Result<(), String> {
        let size = window.inner_size();
        let vsync = self.cfg.video.vsync;
        let make = |ctx: &GpuContext, surface: wgpu::Surface<'static>| -> Result<WindowSurface, String> {
            let mut s = WindowSurface::new(ctx, surface, size.width.max(1), size.height.max(1)).map_err(|e| e.to_string())?;
            s.config.present_mode = if vsync { wgpu::PresentMode::AutoVsync } else { wgpu::PresentMode::AutoNoVsync };
            s.surface.configure(&ctx.device, &s.config);
            Ok(s)
        };
        match &mut self.gfx {
            Some(g) => {
                let surface = g.ctx.instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
                g.surface = Some(make(&g.ctx, surface)?);
            }
            None => {
                let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(window.clone())));
                let surface = instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
                let ctx = GpuContext::with_instance(instance, Some(&surface)).map_err(|e| e.to_string())?;
                let s = make(&ctx, surface)?;
                let info = ctx.adapter.get_info();
                eprintln!("frogger: {} {:?} ({:?}) {}x{}", info.name, info.backend, s.config.format, size.width, size.height);
                self.gfx = Some(Gfx { ctx, surface: Some(s) });
            }
        }
        Ok(())
    }

    fn start_game(&mut self, data: data::Data) {
        let Some(g) = &self.gfx else {
            self.data = Some(data);
            return;
        };
        let Some(s) = &g.surface else {
            self.data = Some(data);
            return;
        };
        let cfg = &self.cfg;
        let mut audio = if cfg.audio.muted { AudioEngine::null(44100) } else { AudioEngine::with_default_output() };
        audio.set_master_volume(cfg.audio.master as f32 / 100.0);
        audio.set_music_volume(cfg.audio.music as i32);
        audio.set_sfx_volume(cfg.audio.sfx as i32);
        eprintln!("frogger: audio {}", if audio.is_realtime() { "on" } else { "muted" });
        // Player 1 = joypad 0 (touch controls + Android gamepad buttons); a keyboard (or a
        // gamepad's d-pad, which Android reports as arrow keys) works alongside.
        let mut mapper = cfg.input_mapper();
        mapper.controllers[0] = Controller::Joypad(0);
        let mut app = App::new(data, &g.ctx.device, &g.ctx.queue, s.config.format, audio, mapper);
        app.set_config(cfg.clone());
        app.apply_master_volume();
        app.set_display(cfg.profile, cfg.display_settings(Profile::Enhanced));
        app.set_input(cfg.bindings(), cfg.input.deadzone, cfg.hop_buffer_frames(Profile::Enhanced));
        app.families[0] = ControllerFamily::PlayStation;
        app.save_path = Some(self.save_path.clone());
        app.set_gameplay(cfg.gameplay.clone(), Some(self.save_path.clone()));
        app.speedrun = gameplay::Speedrun::new(Some(f2_platform::paths::save_dir().join("splits.csv")));
        app.push_steps([Step::Boot]);
        self.app = Some(app);
        self.last = Instant::now();
    }

    fn save_config(&mut self) {
        let Some(app) = &mut self.app else { return };
        app.sync_cfg();
        self.cfg = app.cfg.clone();
        if let Err(e) = self.cfg.save_default() {
            eprintln!("frogger: saving config: {e}");
        }
    }
}

impl WindowApp for Host {
    fn init(&mut self, window: &Arc<window::Window>) {
        if let Err(e) = self.make_surface(window) {
            self.error = Some(e);
            return;
        }
        let size = window.inner_size();
        let dp = window.scale_factor() as f32;
        match &mut self.touch {
            Some(t) => t.resize(size.width as f32, size.height as f32, dp),
            None => self.touch = Some(TouchPad::new(size.width as f32, size.height as f32, dp)),
        }
        if let Some(app) = &mut self.app {
            // Back from the background.
            app.apply_master_volume();
        } else if let Some(d) = self.data.take() {
            self.start_game(d);
        } else if self.import.is_none() {
            let g = self.gfx.as_ref().unwrap();
            let fmt = g.surface.as_ref().unwrap().config.format;
            let ptrs = self.jni_ptrs;
            let picker: Box<dyn Fn()> = Box::new(move || pick_disc_image(ptrs));
            self.import =
                Some(import::Screen::new(&g.ctx.device, &g.ctx.queue, fmt, self.files.clone(), f2_platform::paths::default_data_root(), Some(picker)));
        }
        self.last = Instant::now();
    }

    fn suspended(&mut self) {
        if let Some(g) = &mut self.gfx {
            g.surface = None;
        }
        if let Some(t) = &mut self.touch {
            t.release_all();
        }
        self.gamepad = RawPad::default();
        if let Some(app) = &mut self.app {
            app.audio.set_master_volume(0.0);
        }
        // Android may kill a backgrounded app without notice.
        self.save_config();
    }

    fn event(&mut self, _w: &Arc<window::Window>, event: &WindowEvent) -> Flow {
        match event {
            WindowEvent::Touch(t) => {
                let phase = match t.phase {
                    TouchPhase::Started => Phase::Started,
                    TouchPhase::Moved => Phase::Moved,
                    TouchPhase::Ended | TouchPhase::Cancelled => Phase::Ended,
                };
                if let Some(tp) = &mut self.touch {
                    tp.touch(t.id, phase, t.location.x as f32, t.location.y as f32);
                }
                if phase == Phase::Started {
                    if let Some(s) = &mut self.import {
                        s.tap(t.location.x as f32, t.location.y as f32);
                    }
                }
            }
            WindowEvent::KeyboardInput { event: k, .. } => {
                if let PhysicalKey::Unidentified(NativeKeyCode::Android(code)) = k.physical_key {
                    if let Some(b) = gamepad_button(code) {
                        self.gamepad.set_button(b, k.state == ElementState::Pressed);
                    }
                }
                // A physical controller or keyboard in use: hide the touch controls until
                // the screen is touched again.
                if k.state == ElementState::Pressed {
                    if let Some(tp) = &mut self.touch {
                        tp.visible = false;
                    }
                }
            }
            WindowEvent::Resized(s) => {
                if let Some(Gfx { ctx, surface: Some(surf) }) = &mut self.gfx {
                    surf.resize(&ctx.device, s.width.max(1), s.height.max(1));
                }
            }
            _ => {}
        }
        Flow::Continue
    }

    fn resized(&mut self, w: &Arc<window::Window>, size: winit::dpi::PhysicalSize<u32>) {
        if let Some(t) = &mut self.touch {
            t.resize(size.width as f32, size.height as f32, w.scale_factor() as f32);
        }
    }

    fn frame(&mut self, frame: window::Frame<'_>) -> Flow {
        if let Some(e) = &self.error {
            eprintln!("frogger: {e}");
            return Flow::Exit;
        }
        // Import screen until data exists.
        if let Some(screen) = &mut self.import {
            let done = screen.update();
            let Some(Gfx { ctx, surface: Some(s) }) = &mut self.gfx else { return Flow::Continue };
            let Some(tex) = acquire(ctx, s, frame.window) else { return Flow::Continue };
            let view = tex.texture.create_view(&Default::default());
            let (w, h) = (s.config.width, s.config.height);
            screen.render(&view, w, h);
            frame.window.pre_present_notify();
            ctx.queue.present(tex);
            if done {
                self.import = None;
                match data::Data::locate(&self.cfg, None) {
                    Ok(d) => self.start_game(d),
                    Err(e) => self.error = Some(e),
                }
            }
            return Flow::Continue;
        }
        let (Some(app), Some(Gfx { ctx, surface: Some(s) }), Some(touch)) = (&mut self.app, &mut self.gfx, &mut self.touch) else {
            return Flow::Continue;
        };
        let now = Instant::now();
        let dt = now - self.last;
        self.last = now;

        let mut pad = touch.pad();
        pad.buttons |= self.gamepad.buttons;
        // Newly pressed buttons as raw events (FMV skipping, the controls settings page),
        // like the desktop's gamepad events.
        let prev = std::mem::replace(&mut self.prev_pad, pad);
        for b in PadButton::ALL {
            if pad.button(b) && !prev.button(b) {
                app.pad_event(f2_platform::RawEvent::PadButton(b));
            }
        }
        let raw = [Some(pad), None, None, None];
        app.update(dt, frame.keyboard, &raw, frame.focused);
        app.rumble.clear();
        let rq = &mut app.requests;
        let save = std::mem::take(&mut rq.save_bindings)
            | std::mem::take(&mut rq.save_gameplay)
            | rq.save_volumes.take().is_some()
            | std::mem::take(&mut rq.save_config);
        if std::mem::take(&mut rq.apply_vsync) {
            s.config.present_mode = if app.cfg.video.vsync { wgpu::PresentMode::AutoVsync } else { wgpu::PresentMode::AutoNoVsync };
            s.surface.configure(&ctx.device, &s.config);
        }
        // Window modes do not exist here.
        rq.toggle_fullscreen = false;
        rq.apply_window = false;
        rq.apply_input = false;
        if app.requests.quit || app.idle() {
            self.save_config();
            return Flow::Exit;
        }
        let inner = frame.window.inner_size();
        if inner.width > 0 && inner.height > 0 && (inner.width, inner.height) != (s.config.width, s.config.height) {
            s.resize(&ctx.device, inner.width, inner.height);
            touch.resize(inner.width as f32, inner.height as f32, frame.window.scale_factor() as f32);
        }
        let (w, h) = (s.config.width, s.config.height);
        app.draw(w, h);
        touch.draw(&mut app.r);
        let Some(tex) = acquire(ctx, s, frame.window) else { return Flow::Continue };
        let view = tex.texture.create_view(&Default::default());
        let clear = app.clear_color();
        app.r.render(&view, w, h, clear);
        frame.window.pre_present_notify();
        ctx.queue.present(tex);
        if save {
            self.save_config();
        }
        let cap = self.app.as_ref().map_or(0, |a| a.cfg.video.frame_cap);
        if cap > 0 {
            let period = Duration::from_secs_f64(1.0 / cap as f64);
            let spent = now.elapsed();
            if spent < period {
                std::thread::sleep(period - spent);
            }
        }
        Flow::Continue
    }
}

/// Opens the system file picker (`FroggerActivity.pickDiscImage`); the activity copies the
/// picked files into `files/import/`, where the import screen finds them.
fn pick_disc_image((vm, activity): (usize, usize)) {
    let res = (|| -> jni::errors::Result<()> {
        let vm = unsafe { jni::JavaVM::from_raw(vm as *mut jni::sys::JavaVM)? };
        let mut env = vm.attach_current_thread()?;
        let activity = unsafe { jni::objects::JObject::from_raw(activity as jni::sys::jobject) };
        let r = env.call_method(&activity, "pickDiscImage", "()V", &[]);
        if env.exception_check()? {
            env.exception_describe()?;
            env.exception_clear()?;
        }
        r.map(|_| ())
    })();
    if let Err(e) = res {
        eprintln!("frogger: file picker: {e}");
    }
}

/// The next swapchain image, re-configuring the surface when it went stale.
fn acquire(ctx: &GpuContext, s: &mut WindowSurface, window: &window::Window) -> Option<wgpu::SurfaceTexture> {
    match s.surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => Some(t),
        wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
            let sz = window.inner_size();
            s.resize(&ctx.device, sz.width.max(1), sz.height.max(1));
            None
        }
        _ => {
            std::thread::sleep(Duration::from_millis(5));
            None
        }
    }
}
