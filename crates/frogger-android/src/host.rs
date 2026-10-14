//! The Android activity: paths, the `android-activity` main loop, GPU surface lifecycle (the
//! native window goes away whenever the app is backgrounded), input and the frame loop
//! around `frogger::app::App`.
//!
//! The loop runs on `android-activity` directly (not winit): winit's Android backend turns
//! every motion event into touch positions, so gamepad sticks and d-pads that report HAT
//! axes never reached the game.

use crate::import;
use crate::menu::{self, Menu};
use crate::pad::{self, Axes, Gamepad};
use crate::touch::{Phase, TouchPad};
use android_activity::input::{Axis, InputEvent, KeyAction, MotionAction, MotionEvent};
use android_activity::{AndroidApp, InputStatus, MainEvent, PollEvent};
use f2_audio::AudioEngine;
use f2_engine::{FontId, GpuContext, Renderer, WindowSurface};
use f2_platform::config::VideoMode;
use f2_platform::{Config, Controller, ControllerFamily, KeyboardState, PadButton, Profile, RawPad, WindowMode};
use frogger::app::{App, Step, WinInfo};
use frogger::{data, gameplay, present};
use ndk::native_window::NativeWindow;
use raw_window_handle::{DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle};
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub fn main(android: AndroidApp) {
    // Everything user-visible lives in the external files dir (reachable with adb / USB):
    // `data/` (imported game), `config.toml`, `android.toml`, `saves/`, `mods/`.
    let internal = android.internal_data_path().unwrap_or_else(|| PathBuf::from("/data/local/tmp/frogger2"));
    let files = android.external_data_path().unwrap_or_else(|| internal.clone());
    // Single-threaded here: before the logcat thread.
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
    let prefs = Prefs::load(&files);
    let mut host = Host {
        save_path: f2_platform::paths::save_dir().join("savegame.dat"),
        cfg,
        prefs,
        files,
        data,
        import: None,
        gfx: None,
        app: None,
        touch: None,
        menu: None,
        dp: android.config().density().map_or(2.0, |d| d as f32 / 160.0),
        kb: KeyboardState::default(),
        gamepad: Gamepad::default(),
        prev_pad: RawPad::default(),
        focused: true,
        // JavaVM / FroggerActivity global refs (valid for the process; the file picker).
        jni_ptrs: (android.vm_as_ptr() as usize, android.activity_as_ptr() as usize),
        last: Instant::now(),
        error: None,
    };

    let mut exit = false;
    while !exit {
        // Without a window nothing is drawn: sleep until the next lifecycle event.
        let timeout = if host.has_surface() { Some(Duration::ZERO) } else { None };
        android.poll_events(timeout, |ev| {
            if let PollEvent::Main(m) = ev {
                match m {
                    MainEvent::InitWindow { .. } => host.window_created(android.native_window()),
                    MainEvent::TerminateWindow { .. } => host.window_destroyed(),
                    MainEvent::GainedFocus => host.focused = true,
                    MainEvent::LostFocus => {
                        host.focused = false;
                        host.release_inputs();
                    }
                    // Android may kill a paused app without notice.
                    MainEvent::Pause => host.save_config(),
                    MainEvent::Destroy => exit = true,
                    _ => {}
                }
            }
        });
        if let Ok(mut it) = android.input_events_iter() {
            while it.next(|e| host.input(e)) {}
        }
        if !exit && host.frame() {
            exit = true;
        }
    }
    host.save_config();
    if let Some(e) = host.error {
        eprintln!("frogger: {e}");
    }
    eprintln!("frogger: exit");
}

/// The native window as a wgpu surface target.
struct AndroidWindow(NativeWindow);

impl HasWindowHandle for AndroidWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        self.0.window_handle()
    }
}

impl HasDisplayHandle for AndroidWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(DisplayHandle::android())
    }
}

/// Android-only settings (`android.toml` next to `config.toml`).
struct Prefs {
    touch_controls: bool,
}

impl Prefs {
    fn path(files: &std::path::Path) -> PathBuf {
        files.join("android.toml")
    }

    fn load(files: &std::path::Path) -> Prefs {
        let text = std::fs::read_to_string(Self::path(files)).unwrap_or_default();
        let get = |k: &str| text.lines().filter_map(|l| l.split_once('=')).find(|(a, _)| a.trim() == k).map(|(_, v)| v.trim().to_string());
        Prefs { touch_controls: get("touch_controls").is_none_or(|v| v != "false") }
    }

    fn save(&self, files: &std::path::Path) {
        let text = format!("# Android settings (also in the swipe-from-the-left menu).\ntouch_controls = {}\n", self.touch_controls);
        if let Err(e) = std::fs::write(Self::path(files), text) {
            eprintln!("frogger: saving android.toml: {e}");
        }
    }
}

struct Gfx {
    ctx: GpuContext,
    /// `None` while the app is in the background.
    surface: Option<WindowSurface>,
    window: Option<NativeWindow>,
    /// Touch controls and the overlay menu, drawn over the presented frame.
    ui: Renderer,
    ui_font: FontId,
    /// Render-resolution pass (fixed resolutions, brightness), created on demand.
    present: Option<present::Present>,
}

struct Host {
    cfg: Config,
    prefs: Prefs,
    files: PathBuf,
    save_path: PathBuf,
    /// Located game data waiting for the GPU (the app is created on the first window).
    data: Option<data::Data>,
    import: Option<import::Screen>,
    gfx: Option<Gfx>,
    app: Option<App>,
    touch: Option<TouchPad>,
    menu: Option<Menu>,
    /// Pixels per dp.
    dp: f32,
    /// Hardware / Bluetooth keyboard (and Back = Escape).
    kb: KeyboardState,
    gamepad: Gamepad,
    /// Player 1's pad last frame (press edges become `App::pad_event`s).
    prev_pad: RawPad,
    focused: bool,
    /// `JavaVM*` and the activity's `jobject`.
    jni_ptrs: (usize, usize),
    last: Instant,
    error: Option<String>,
}

impl Host {
    fn has_surface(&self) -> bool {
        self.gfx.as_ref().is_some_and(|g| g.surface.is_some())
    }

    fn window_created(&mut self, window: Option<NativeWindow>) {
        let Some(window) = window else { return };
        let (w, h) = (window.width().max(1) as u32, window.height().max(1) as u32);
        if let Err(e) = self.make_surface(window, w, h) {
            self.error = Some(e);
            return;
        }
        match &mut self.touch {
            Some(t) => t.resize(w as f32, h as f32, self.dp),
            None => self.touch = Some(TouchPad::new(w as f32, h as f32, self.dp)),
        }
        match &mut self.menu {
            Some(m) => m.resize(w as f32, h as f32, self.dp),
            None => self.menu = Some(Menu::new(w as f32, h as f32, self.dp)),
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

    fn window_destroyed(&mut self) {
        if let Some(g) = &mut self.gfx {
            g.surface = None;
            g.window = None;
        }
        self.release_inputs();
        if let Some(app) = &mut self.app {
            app.audio.set_master_volume(0.0);
        }
        self.save_config();
    }

    fn release_inputs(&mut self) {
        if let Some(t) = &mut self.touch {
            t.release_all();
        }
        self.gamepad.clear();
        self.kb.clear();
    }

    fn make_surface(&mut self, window: NativeWindow, w: u32, h: u32) -> Result<(), String> {
        let vsync = self.cfg.video.vsync;
        let make = |ctx: &GpuContext, surface: wgpu::Surface<'static>| -> Result<WindowSurface, String> {
            let mut s = WindowSurface::new(ctx, surface, w, h).map_err(|e| e.to_string())?;
            s.config.present_mode = if vsync { wgpu::PresentMode::AutoVsync } else { wgpu::PresentMode::AutoNoVsync };
            s.surface.configure(&ctx.device, &s.config);
            Ok(s)
        };
        let target = AndroidWindow(window.clone());
        match &mut self.gfx {
            Some(g) => {
                let surface = g.ctx.instance.create_surface(target).map_err(|e| e.to_string())?;
                g.surface = Some(make(&g.ctx, surface)?);
                g.window = Some(window);
            }
            None => {
                let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
                let surface = instance.create_surface(target).map_err(|e| e.to_string())?;
                let ctx = GpuContext::with_instance(instance, Some(&surface)).map_err(|e| e.to_string())?;
                let s = make(&ctx, surface)?;
                let info = ctx.adapter.get_info();
                eprintln!("frogger: {} {:?} ({:?}) {w}x{h}, {:.2} px/dp", info.name, info.backend, s.config.format, self.dp);
                let mut ui = Renderer::new(&ctx.device, &ctx.queue, s.config.format);
                ui.overlay = true;
                let ui_font = ui.add_font(frogger::tinyfont::builtin_font());
                self.gfx = Some(Gfx { ctx, surface: Some(s), window: Some(window), ui, ui_font, present: None });
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
        // Player 1 = joypad 0 (touch controls + the Android gamepad); a keyboard works
        // alongside.
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

    fn input(&mut self, ev: &InputEvent) -> InputStatus {
        match ev {
            InputEvent::KeyEvent(k) => {
                let code: u32 = k.key_code().into();
                if pad::system_key(code) {
                    return InputStatus::Unhandled;
                }
                let down = match k.action() {
                    KeyAction::Down => true,
                    KeyAction::Up => false,
                    _ => return InputStatus::Handled,
                };
                if code == pad::key::BACK {
                    if let Some(m) = self.menu.as_mut().filter(|m| m.open) {
                        if down {
                            m.close();
                        }
                        return InputStatus::Handled;
                    }
                }
                if self.gamepad.key(code, down) {
                    self.hide_touch();
                    return InputStatus::Handled;
                }
                match pad::dik(code) {
                    Some(d) => {
                        if down && k.repeat_count() == 0 {
                            self.kb.key_down(d);
                        } else if !down {
                            self.kb.key_up(d);
                        }
                        if code != pad::key::BACK {
                            self.hide_touch();
                        }
                        InputStatus::Handled
                    }
                    None => InputStatus::Unhandled,
                }
            }
            InputEvent::MotionEvent(m) => {
                let source = m.source();
                if source.is_joystick_class() {
                    self.joystick(m);
                    InputStatus::Handled
                } else if source.is_pointer_class() {
                    self.pointer(m);
                    InputStatus::Handled
                } else {
                    InputStatus::Unhandled
                }
            }
            _ => InputStatus::Unhandled,
        }
    }

    fn joystick(&mut self, m: &MotionEvent) {
        let p = m.pointer_at_index(0);
        let ax = |a: Axis| p.axis_value(a);
        self.gamepad.motion(Axes {
            x: ax(Axis::X),
            y: ax(Axis::Y),
            z: ax(Axis::Z),
            rz: ax(Axis::Rz),
            hat_x: ax(Axis::HatX),
            hat_y: ax(Axis::HatY),
            lt: ax(Axis::Ltrigger).max(ax(Axis::Brake)),
            rt: ax(Axis::Rtrigger).max(ax(Axis::Gas)),
        });
        if self.gamepad.active() {
            self.hide_touch();
        }
    }

    fn pointer(&mut self, m: &MotionEvent) {
        let one = |phase: Phase| {
            let p = m.pointer_at_index(m.pointer_index());
            vec![(p.pointer_id() as u64, phase, p.x(), p.y())]
        };
        let all = |phase: Phase| m.pointers().map(|p| (p.pointer_id() as u64, phase, p.x(), p.y())).collect::<Vec<_>>();
        let touches = match m.action() {
            MotionAction::Down | MotionAction::PointerDown => one(Phase::Started),
            MotionAction::Up | MotionAction::PointerUp => one(Phase::Ended),
            MotionAction::Move => all(Phase::Moved),
            MotionAction::Cancel => all(Phase::Ended),
            _ => Vec::new(),
        };
        for (id, phase, x, y) in touches {
            self.touch_point(id, phase, x, y);
        }
    }

    fn touch_point(&mut self, id: u64, phase: Phase, x: f32, y: f32) {
        if let Some(s) = &mut self.import {
            if phase == Phase::Started {
                s.tap(x, y);
            }
            return;
        }
        let (taken, action) = match &mut self.menu {
            Some(m) => m.touch(id, phase, x, y),
            None => (false, menu::Action::None),
        };
        match action {
            menu::Action::Opened => {
                if let Some(t) = &mut self.touch {
                    t.release_all();
                }
            }
            menu::Action::ToggleTouch => {
                self.prefs.touch_controls = !self.prefs.touch_controls;
                self.prefs.save(&self.files);
                eprintln!("frogger: touch controls {}", if self.prefs.touch_controls { "on" } else { "off" });
            }
            menu::Action::Closed => self.last = Instant::now(),
            menu::Action::None => {}
        }
        if taken || !self.prefs.touch_controls {
            return;
        }
        if let Some(t) = &mut self.touch {
            t.touch(id, phase, x, y);
        }
    }

    fn hide_touch(&mut self) {
        if let Some(t) = &mut self.touch {
            t.visible = false;
        }
    }

    /// One frame; true = quit.
    fn frame(&mut self) -> bool {
        if let Some(e) = &self.error {
            eprintln!("frogger: {e}");
            return true;
        }
        let Some(g) = &mut self.gfx else { return false };
        let (Some(s), Some(win)) = (&mut g.surface, &g.window) else { return false };
        // Follow the window size (rotation, split screen, cutout changes).
        let (ww, wh) = (win.width().max(1) as u32, win.height().max(1) as u32);
        if (ww, wh) != (s.config.width, s.config.height) {
            s.resize(&g.ctx.device, ww, wh);
            if let Some(t) = &mut self.touch {
                t.resize(ww as f32, wh as f32, self.dp);
            }
            if let Some(m) = &mut self.menu {
                m.resize(ww as f32, wh as f32, self.dp);
            }
        }
        let (w, h) = (s.config.width, s.config.height);

        // Import screen until data exists.
        if let Some(screen) = &mut self.import {
            let done = screen.update();
            let Some(tex) = acquire(&g.ctx, s) else { return false };
            let view = tex.texture.create_view(&Default::default());
            screen.render(&view, w, h);
            g.ctx.queue.present(tex);
            if done {
                self.import = None;
                match data::Data::locate(&self.cfg, None) {
                    Ok(d) => self.start_game(d),
                    Err(e) => self.error = Some(e),
                }
            }
            return false;
        }
        let (Some(app), Some(touch), Some(menu)) = (&mut self.app, &mut self.touch, &self.menu) else { return false };
        let now = Instant::now();
        let dt = now - self.last;
        self.last = now;

        // The game sees the physical pad (and the touch controls when they are on); nothing
        // runs while the overlay menu is open.
        let mut pad = self.gamepad.pad();
        if self.prefs.touch_controls {
            pad = pad::merge(pad, touch.pad());
        }
        if !menu.open {
            // Newly pressed buttons as raw events (FMV skipping, the controls settings
            // page), like the desktop's gamepad events.
            let prev = std::mem::replace(&mut self.prev_pad, pad);
            for b in PadButton::ALL {
                if pad.button(b) && !prev.button(b) {
                    app.pad_event(f2_platform::RawEvent::PadButton(b));
                }
            }
            app.update(dt, &self.kb, &[Some(pad), None, None, None], self.focused);
        }
        self.kb.end_frame();
        app.rumble.clear();
        let rq = &mut app.requests;
        let save = std::mem::take(&mut rq.save_bindings)
            | std::mem::take(&mut rq.save_gameplay)
            | rq.save_volumes.take().is_some()
            | std::mem::take(&mut rq.save_config);
        if std::mem::take(&mut rq.apply_vsync) {
            s.config.present_mode = if app.cfg.video.vsync { wgpu::PresentMode::AutoVsync } else { wgpu::PresentMode::AutoNoVsync };
            s.surface.configure(&g.ctx.device, &s.config);
        }
        // One fullscreen mode; the resolution is read every frame below.
        rq.toggle_fullscreen = false;
        rq.apply_window = false;
        rq.apply_input = false;
        if app.requests.quit || app.idle() {
            self.save_config();
            return true;
        }
        // The Display page: Auto = the screen; explicit sizes render at that size and are
        // scaled to fit with black bars.
        let screen = VideoMode { width: w, height: h, refresh_mhz: 60_000 };
        app.win = WinInfo { mode: WindowMode::Borderless, size: (w, h), modes: Vec::new(), desktop: Some(screen), console: false, fixed_mode: true };

        let v = &app.cfg.video;
        let (rs, via_present) = present::plan((w, h), true, v.resolution, v.brightness);
        let brightness = v.brightness;
        app.draw(rs.0, rs.1);
        let Some(tex) = acquire(&g.ctx, s) else { return false };
        let view = tex.texture.create_view(&Default::default());
        let clear = app.clear_color();
        if via_present {
            let p = g.present.get_or_insert_with(|| present::Present::new(&g.ctx.device, s.config.format));
            {
                let off = p.target(&g.ctx.device, rs.0, rs.1);
                app.r.render(off, rs.0, rs.1, clear);
            }
            p.blit(&g.ctx.device, &g.ctx.queue, &view, w, h, brightness);
        } else {
            app.r.render(&view, w, h, clear);
        }
        // Touch controls and the menu over the final image (in the black bars too).
        g.ui.begin_frame(w, h);
        if self.prefs.touch_controls && !menu.open {
            touch.draw(&mut g.ui);
        }
        menu.draw(&mut g.ui, g.ui_font, self.prefs.touch_controls);
        g.ui.render(&view, w, h, [0.0; 4]);
        g.ctx.queue.present(tex);
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
        false
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
fn acquire(ctx: &GpuContext, s: &mut WindowSurface) -> Option<wgpu::SurfaceTexture> {
    match s.surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => Some(t),
        wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
            let (w, h) = (s.config.width, s.config.height);
            s.resize(&ctx.device, w, h);
            None
        }
        _ => {
            std::thread::sleep(Duration::from_millis(5));
            None
        }
    }
}
