//! The Sony system layer shared by the PS4 and PS5 hosts: logging, pads, audio out and the
//! frame loop around [`frogger::app::App`]. Only built for `x86_64-unknown-freebsd`.
//!
//! A platform crate provides a [`Platform`] (notifications, its video out as a
//! [`Present`]) and calls [`main`] from its `main` symbol. Rendering and presenting are
//! one step ([`Present::frame`]) so a GPU renderer can replace the CPU one: [`SoftPresent`]
//! is the CPU renderer (`f2_engine::Renderer::render_soft`) feeding any [`Screen`] (a set
//! of scan-out buffers the RGBA image is scaled into).

pub mod alloc;
pub mod shim;

use crate::host;
use core::ffi::{c_char, c_int, c_void};
use f2_audio::{AudioEngine, AudioOutput, Mixer};
use f2_platform::{Config, KeyboardState, Profile, RawPad};
use frogger::app::{App, Step};
use std::ffi::{CStr, CString};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ------------------------------------------------------------------ system calls

/// `ScePadData` (only the leading fields are read; the buffer is oversized).
#[repr(C)]
struct PadData {
    buttons: u32,
    left: [u8; 2],
    right: [u8; 2],
    analog: [u8; 2],
    padding: u16,
    rest: [u8; 160],
}

#[repr(C)]
#[derive(Default)]
pub struct KEvent {
    pub ident: u64,
    pub filter: i16,
    pub flags: u16,
    pub fflags: u32,
    pub data: i64,
    pub udata: usize,
}

extern "C" {
    fn sceKernelDebugOutText(channel: c_int, text: *const c_char) -> c_int;
    pub fn sceKernelAllocateDirectMemory(start: i64, end: i64, len: usize, align: usize, mem_type: c_int, phys: *mut i64) -> c_int;
    pub fn sceKernelGetDirectMemorySize() -> usize;
    pub fn sceKernelMapDirectMemory(addr: *mut *mut c_void, len: usize, prot: c_int, flags: c_int, phys: i64, align: usize) -> c_int;
    pub fn sceKernelCreateEqueue(eq: *mut *mut c_void, name: *const c_char) -> c_int;
    pub fn sceKernelWaitEqueue(eq: *mut c_void, ev: *mut KEvent, num: c_int, out: *mut c_int, timeout: *mut u32) -> c_int;
    pub fn sceKernelSendNotificationRequest(device: c_int, req: *const c_void, size: usize, blocking: c_int) -> c_int;

    fn sceUserServiceInitialize(param: *const c_int) -> c_int;
    fn sceUserServiceGetInitialUser(user: *mut c_int) -> c_int;
    fn scePadInit() -> c_int;
    fn scePadOpen(user: c_int, ty: c_int, index: c_int, param: *const c_void) -> c_int;
    fn scePadReadState(h: c_int, data: *mut PadData) -> c_int;
    fn sceAudioOutInit() -> c_int;
    fn sceAudioOutOpen(user: c_int, ty: c_int, index: c_int, len: u32, freq: u32, param: u32) -> c_int;
    fn sceAudioOutOutput(h: c_int, buf: *const c_void) -> c_int;
    fn sceSystemServiceHideSplashScreen() -> c_int;
}

// ------------------------------------------------------------------ logging

static LOG: Mutex<Option<std::fs::File>> = Mutex::new(None);
static LOG_SINK: std::sync::OnceLock<fn(&CStr)> = std::sync::OnceLock::new();

/// Replaces where [`log`] lines go besides `frogger.log` (default:
/// `sceKernelDebugOutText`, the kernel log). Set once, before the first line.
pub fn set_log_sink(sink: fn(&CStr)) {
    let _ = LOG_SINK.set(sink);
}

/// Writes a line to the kernel log (visible with a klog viewer) and to `frogger.log` in
/// the save directory.
pub fn log(msg: &str) {
    if let Ok(c) = CString::new(format!("[frogger2] {msg}\n")) {
        match LOG_SINK.get() {
            Some(sink) => sink(&c),
            None => unsafe {
                sceKernelDebugOutText(0, c.as_ptr());
            },
        }
    }
    if let Ok(mut f) = LOG.lock() {
        if let Some(f) = f.as_mut() {
            let _ = writeln!(f, "{msg}");
            let _ = f.flush();
        }
    }
}

// ------------------------------------------------------------------ platform

/// What differs between the consoles.
pub struct Platform {
    /// "PS4" / "PS5", for the log.
    pub name: &'static str,
    /// Shows a system notification toast.
    pub notify: fn(&str),
    /// Opens the video out; called once, after the system services are up.
    pub open_present: fn() -> Result<Box<dyn Present>, String>,
}

/// Draws [`App`] frames and shows them.
pub trait Present {
    /// The size the app lays out and renders at.
    fn render_size(&self) -> (u32, u32);
    /// Renders the app's current frame (after [`App::draw`]) and shows it.
    fn frame(&mut self, app: &mut App);
    /// Shows one colour over the whole screen (status screens).
    fn fill(&mut self, rgba: u32);
    /// Creates the app for this presenter: the CPU renderer (`App::new_soft`) by default;
    /// with feature `gpu` (`frogger`'s wgpu `App`) the presenter builds it on its device.
    #[cfg(not(feature = "gpu"))]
    fn new_app(&self, data: frogger::data::Data, audio: AudioEngine, mapper: f2_platform::InputMapper) -> App {
        App::new_soft(data, audio, mapper)
    }
    #[cfg(feature = "gpu")]
    fn new_app(&self, data: frogger::data::Data, audio: AudioEngine, mapper: f2_platform::InputMapper) -> App;
    /// Timing summary since the last call, for the periodic log line.
    fn stats(&mut self) -> Option<String> {
        None
    }
}

/// A set of scan-out buffers that RGBA images are scaled into.
pub trait Screen {
    /// Display size in pixels.
    fn size(&self) -> (u32, u32);
    /// Scales `pixels` (`w`x`h`, RGBA bytes as u32) into the next buffer and flips to it.
    /// `exec` runs parallel jobs (the presenter's worker pool).
    fn show(&mut self, pixels: &[u32], w: u32, h: u32, exec: host::Exec);
    /// Fills the next buffer with one colour and flips to it.
    fn fill(&mut self, rgba: u32);
}

/// The CPU renderer on a [`Screen`] (not with feature `gpu`).
#[cfg(not(feature = "gpu"))]
pub struct SoftPresent<S: Screen> {
    pub screen: S,
    target: f2_engine::SoftTarget,
    threads: usize,
    pool: host::Pool,
    /// Time spent rendering / scaling + flipping since the last [`Present::stats`].
    times: (Duration, Duration, u32),
}

#[cfg(not(feature = "gpu"))]
impl<S: Screen> SoftPresent<S> {
    /// Renders `lines` lines at the screen's aspect ratio on `threads` threads.
    pub fn new(screen: S, lines: u32, threads: usize) -> Self {
        let (dw, dh) = screen.size();
        let (w, h) = host::internal_size_at(dw, dh, lines);
        let pool = host::Pool::new(threads.saturating_sub(1));
        SoftPresent { screen, target: f2_engine::SoftTarget::new(w, h), threads, pool, times: Default::default() }
    }
}

// `frogger`'s `gpu` feature replaces the CPU renderer in `App`.
#[cfg(not(feature = "gpu"))]
impl<S: Screen> Present for SoftPresent<S> {
    fn render_size(&self) -> (u32, u32) {
        (self.target.width, self.target.height)
    }

    fn frame(&mut self, app: &mut App) {
        let t = Instant::now();
        let exec = self.pool.exec();
        app.r.render_soft_with(&mut self.target, app.clear_color(), self.threads, &exec);
        let t1 = Instant::now();
        self.screen.show(&self.target.pixels, self.target.width, self.target.height, &exec);
        self.times.0 += t1 - t;
        self.times.1 += t1.elapsed();
        self.times.2 += 1;
    }

    fn fill(&mut self, rgba: u32) {
        self.screen.fill(rgba);
    }

    fn stats(&mut self) -> Option<String> {
        let (r, s, n) = std::mem::take(&mut self.times);
        let ms = |d: Duration| d.as_secs_f64() * 1000.0 / n.max(1) as f64;
        Some(format!("render {:.1} ms, scale+flip {:.1} ms", ms(r), ms(s)))
    }
}

/// Render threads for the CPU renderer: the hardware threads available (`fallback` when
/// the system does not say), clamped.
pub fn render_threads(max: usize, fallback: usize) -> usize {
    std::thread::available_parallelism().map_or(fallback, |n| n.get()).clamp(2, max)
}

// ------------------------------------------------------------------ audio

const AUDIO_RATE: u32 = 48_000;
const AUDIO_GRAIN: usize = 256;

struct SceAudio {
    user: c_int,
}

impl AudioOutput for SceAudio {
    fn sample_rate(&self) -> u32 {
        AUDIO_RATE
    }

    fn start(&mut self, mixer: Arc<Mutex<Mixer>>) -> Result<(), String> {
        // Port type 0 = main, format 1 = S16 stereo.
        let h = unsafe { sceAudioOutOpen(self.user, 0, 0, AUDIO_GRAIN as u32, AUDIO_RATE, 1) };
        if h < 0 {
            return Err(format!("sceAudioOutOpen: {h:#x}"));
        }
        std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || {
                let mut f = vec![0f32; AUDIO_GRAIN * 2];
                let mut s = vec![0i16; AUDIO_GRAIN * 2];
                loop {
                    match mixer.lock() {
                        Ok(mut m) => m.render(&mut f),
                        Err(_) => f.fill(0.0),
                    }
                    for (o, v) in s.iter_mut().zip(&f) {
                        *o = (v.clamp(-1.0, 1.0) * 32767.0) as i16;
                    }
                    // Blocks until the previous grain has been consumed (paces the thread).
                    unsafe { sceAudioOutOutput(h, s.as_ptr() as *const c_void) };
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn is_realtime(&self) -> bool {
        true
    }
}

/// `AudioEngine::new` falls back to muted when `start` fails; report that as an error.
fn start_audio(user: c_int) -> Result<AudioEngine, String> {
    let e = AudioEngine::new(Box::new(SceAudio { user }));
    if e.is_realtime() {
        Ok(e)
    } else {
        Err("audio output failed to start".into())
    }
}

// ------------------------------------------------------------------ main

struct Pad {
    handle: c_int,
}

impl Pad {
    fn read(&self) -> Option<RawPad> {
        if self.handle < 0 {
            return None;
        }
        let mut d: PadData = unsafe { core::mem::zeroed() };
        (unsafe { scePadReadState(self.handle, &mut d) } == 0).then(|| host::raw_pad(d.buttons, d.left, d.right))
    }
}

/// The program: call from the platform crate's `main` symbol.
pub fn main(platform: &Platform) -> c_int {
    let notify = platform.notify;
    std::panic::set_hook(Box::new(move |info| {
        let msg = format!("panic: {info}");
        log(&msg);
        notify(&format!("Frogger 2 crashed:\n{msg}"));
    }));
    match run(platform) {
        Ok(()) => 0,
        Err(e) => {
            log(&format!("fatal: {e}"));
            notify(&format!("Frogger 2: {e}"));
            std::thread::sleep(Duration::from_secs(5));
            1
        }
    }
}

fn run(platform: &Platform) -> Result<(), String> {
    let save_dir = host::writable_dir(host::SAVE_DIRS);
    if let Some(d) = &save_dir {
        f2_platform::paths::set_config_dir(d.clone());
        if let Ok(f) = std::fs::File::create(d.join("frogger.log")) {
            *LOG.lock().unwrap() = Some(f);
        }
    }
    log(&format!("Frogger 2 (Rust port) for {} homebrew, save dir {save_dir:?}", platform.name));

    let mut user = 0;
    unsafe {
        let prio: c_int = 700;
        sceUserServiceInitialize(&prio);
        sceUserServiceGetInitialUser(&mut user);
        scePadInit();
        sceAudioOutInit();
    }
    let pad = Pad { handle: unsafe { scePadOpen(user, 0, 0, core::ptr::null()) } };
    log(&format!("user {user:#x}, pad handle {:#x}", pad.handle));
    let mut present = (platform.open_present)()?;
    unsafe { sceSystemServiceHideSplashScreen() };

    let mut cfg = Config::load_default().unwrap_or_default();
    // Locate the data; until it is there, show a status screen and keep looking (it can
    // be copied over FTP while this runs).
    let mut warned = false;
    let data = loop {
        if let Some(d) = host::DATA_DIRS.iter().find_map(|d| frogger::data::Data::locate(&cfg, Some(PathBuf::from(d))).ok()) {
            break d;
        }
        if !warned {
            warned = true;
            let msg = "Frogger 2: game data not found. Import your CD on a PC with f2-import, then copy the data folder (containing game/) to /data/frogger2 or a USB drive at /frogger2.";
            log(msg);
            (platform.notify)(msg);
        }
        for _ in 0..120 {
            present.fill(0xff40_1010);
        }
    };
    log(&format!("data {}", data.rendition.display()));

    let audio = start_audio(user).unwrap_or_else(|e| {
        log(&format!("audio unavailable: {e}"));
        AudioEngine::null(AUDIO_RATE)
    });
    let mut mapper = cfg.input_mapper();
    mapper.reset_controllers(1);
    cfg.input.controllers = mapper.controllers;
    let mut app = present.new_app(data, audio, mapper);
    app.set_config(cfg.clone());
    app.apply_master_volume();
    app.set_display(cfg.profile, cfg.display_settings(Profile::Enhanced));
    app.set_input(cfg.bindings(), cfg.input.deadzone, cfg.hop_buffer_frames(Profile::Enhanced));
    // DualShock 4 / DualSense: button prompts say Cross/Circle/Square/Triangle.
    app.families[0] = f2_platform::ControllerFamily::PlayStation;
    let save_path = f2_platform::paths::save_dir().join("savegame.dat");
    let _ = std::fs::create_dir_all(f2_platform::paths::save_dir());
    app.save_path = save_dir.is_some().then(|| save_path.clone());
    app.set_gameplay(cfg.gameplay.clone(), save_dir.is_some().then_some(save_path));
    app.push_steps([Step::Boot]);

    let (iw, ih) = present.render_size();
    app.win = frogger::app::WinInfo { size: (iw, ih), ..Default::default() };
    log(&format!("rendering {iw}x{ih}"));
    let mut edges = host::RawEdges::default();
    let kb = KeyboardState::default();
    let mut last = Instant::now();
    let (mut frames, mut stat_t, mut busy) = (0u32, Instant::now(), Duration::ZERO);
    loop {
        let now = Instant::now();
        let dt = now - last;
        last = now;
        let raw = [pad.read(), None, None, None];
        if let Some(p) = &raw[0] {
            for ev in edges.events(p) {
                app.pad_event(ev);
            }
        }
        app.update(dt, &kb, &raw, true);
        app.rumble.clear();
        let rq = &mut app.requests;
        let save = std::mem::take(&mut rq.save_bindings)
            | std::mem::take(&mut rq.save_gameplay)
            | rq.save_volumes.take().is_some()
            | std::mem::take(&mut rq.save_config);
        if save {
            app.sync_cfg();
            if let Err(e) = app.cfg.save_default() {
                log(&format!("saving config: {e}"));
            }
        }
        if app.requests.quit || app.idle() {
            log("quit");
            return Ok(());
        }
        app.draw(iw, ih);
        present.frame(&mut app);
        busy += now.elapsed();
        frames += 1;
        if stat_t.elapsed() >= Duration::from_secs(10) {
            let s = stat_t.elapsed().as_secs_f64();
            let extra = present.stats().map(|s| format!(" ({s})")).unwrap_or_default();
            log(&format!("{:.1} fps, {:.1} ms/frame busy{extra}", frames as f64 / s, busy.as_secs_f64() * 1000.0 / frames.max(1) as f64));
            (frames, stat_t, busy) = (0, Instant::now(), Duration::ZERO);
        }
    }
}
