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

use crate::host::{self, VideoMode};
use core::ffi::{c_char, c_int, c_void};
use f2_audio::{AudioEngine, AudioOutput, Mixer};
pub use f2_platform::Config;
use f2_platform::{KeyboardState, Profile, RawPad};
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

/// A line straight to the kernel log (`sceKernelDebugOutText`), without allocating: usable
/// as the very first thing in `main`, before the heap or thread locals are touched.
pub fn klog(line: &CStr) {
    unsafe { sceKernelDebugOutText(0, line.as_ptr()) };
}

/// Adds a second place [`log`] lines go to besides the kernel log and `frogger.log` (the
/// PS5 build also prints them to stdout, which KytyPS5 shows). Set once, before the first line.
pub fn set_log_sink(sink: fn(&CStr)) {
    let _ = LOG_SINK.set(sink);
}

/// Writes a line to the kernel log (klog: `[frogger2] ...`), the extra sink if any, and
/// `frogger.log` once [`open_log_file`] has opened it.
pub fn log(msg: &str) {
    if let Ok(c) = CString::new(format!("[frogger2] {msg}\n")) {
        klog(&c);
        if let Some(sink) = LOG_SINK.get() {
            sink(&c);
        }
    }
    if let Ok(mut f) = LOG.lock() {
        if let Some(f) = f.as_mut() {
            let _ = writeln!(f, "{msg}");
            let _ = f.flush();
        }
    }
}

/// Opens `frogger.log` in the first writable [`host::LOG_DIRS`] entry (internal storage,
/// then USB, the app-private places last) and says in the kernel log which one it is (or
/// that none was writable). Returns the file's path.
pub fn open_log_file() -> Option<PathBuf> {
    let path = host::writable_dir(host::LOG_DIRS).map(|d| d.join("frogger.log"));
    match path.as_ref().map(|p| (p, std::fs::File::create(p))) {
        Some((p, Ok(f))) => {
            *LOG.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
            log(&format!("log file {}", p.display()));
            path
        }
        Some((p, Err(e))) => {
            log(&format!("log file {}: {e} (kernel log only)", p.display()));
            None
        }
        None => {
            log(&format!("no writable log directory among {:?} (kernel log only)", host::LOG_DIRS));
            None
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
    /// Opens the video out in the output mode `cfg`'s Display settings ask for; called
    /// once, after the system services are up.
    pub open_present: fn(cfg: &Config) -> Result<Box<dyn Present>, String>,
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
    /// Output modes [`Present::set_mode`] can switch to (empty: fixed output).
    fn modes(&self) -> Vec<VideoMode> {
        Vec::new()
    }
    /// The output mode in use, if known.
    fn mode(&self) -> Option<VideoMode> {
        None
    }
    /// Switches the output mode (one of [`Present::modes`]); [`Present::render_size`] may
    /// change.
    fn set_mode(&mut self, _mode: VideoMode) -> Result<(), String> {
        Err("this output has one mode".into())
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
    /// As [`Present::modes`], [`Present::mode`] and [`Present::set_mode`].
    fn modes(&self) -> Vec<VideoMode> {
        Vec::new()
    }
    fn mode(&self) -> Option<VideoMode> {
        None
    }
    fn set_mode(&mut self, _mode: VideoMode) -> Result<(), String> {
        Err("this output has one mode".into())
    }
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

    fn modes(&self) -> Vec<VideoMode> {
        self.screen.modes()
    }

    fn mode(&self) -> Option<VideoMode> {
        self.screen.mode()
    }

    /// Switches the screen's mode and renders at the new size's [`host::render_lines`].
    fn set_mode(&mut self, mode: VideoMode) -> Result<(), String> {
        let r = self.screen.set_mode(mode);
        let (dw, dh) = self.screen.size();
        let (w, h) = host::internal_size_at(dw, dh, host::render_lines(dh));
        if (w, h) != (self.target.width, self.target.height) {
            self.target = f2_engine::SoftTarget::new(w, h);
        }
        r
    }

    fn stats(&mut self) -> Option<String> {
        let (r, s, n) = std::mem::take(&mut self.times);
        let ms = |d: Duration| d.as_secs_f64() * 1000.0 / n.max(1) as f64;
        Some(format!("render {:.1} ms, scale+flip {:.1} ms", ms(r), ms(s)))
    }
}

/// The Display settings' view of a console output: always fullscreen, its modes, and the
/// one in use (shown as Auto rather than the render size).
fn win_info(present: &dyn Present) -> frogger::app::WinInfo {
    let size = present.render_size();
    let mode = present.mode().unwrap_or(VideoMode { width: size.0, height: size.1, refresh_mhz: host::REFRESH_60_MHZ });
    let mut modes = present.modes();
    if modes.is_empty() {
        modes.push(mode);
    }
    frogger::app::WinInfo { size, mode: f2_platform::WindowMode::Fullscreen, modes, desktop: Some(mode), console: true, fixed_mode: false }
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
        // Port type 0 = main, format 1 = S16 stereo. The main port is opened as the SYSTEM user
        // (0xff), as hardware-proven PS5 titles do (UnleashedRecomp's sceAudioOut driver); the
        // initial user is only a fallback (a user-bound main port was silent on a real PS5).
        let mut h = unsafe { sceAudioOutOpen(0xff, 0, 0, AUDIO_GRAIN as u32, AUDIO_RATE, 1) };
        log(&format!("audio: sceAudioOutOpen(system user) = {h:#x}"));
        if h < 0 {
            h = unsafe { sceAudioOutOpen(self.user, 0, 0, AUDIO_GRAIN as u32, AUDIO_RATE, 1) };
            log(&format!("audio: sceAudioOutOpen(user {:#x}) = {h:#x}", self.user));
        }
        if h < 0 {
            return Err(format!("sceAudioOutOpen: {h:#x}"));
        }
        std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || {
                let mut f = vec![0f32; AUDIO_GRAIN * 2];
                let mut s = vec![0i16; AUDIO_GRAIN * 2];
                let mut reported = false;
                loop {
                    match mixer.lock() {
                        Ok(mut m) => m.render(&mut f),
                        Err(_) => f.fill(0.0),
                    }
                    for (o, v) in s.iter_mut().zip(&f) {
                        *o = (v.clamp(-1.0, 1.0) * 32767.0) as i16;
                    }
                    // Blocks until the previous grain has been consumed (paces the thread).
                    let r = unsafe { sceAudioOutOutput(h, s.as_ptr() as *const c_void) };
                    if r < 0 && !reported {
                        reported = true;
                        log(&format!("audio: sceAudioOutOutput = {r:#x}"));
                    }
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
    klog(c"[frogger2] sce::main: std runtime up\n");
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
    log(&format!("Frogger 2 (Rust port) for {} homebrew starting", platform.name));
    open_log_file();
    let save_dir = host::writable_dir(host::SAVE_DIRS);
    if let Some(d) = &save_dir {
        f2_platform::paths::set_config_dir(d.clone());
    }
    log(&format!("save dir {save_dir:?}"));

    let mut user = 0;
    unsafe {
        let prio: c_int = 700;
        sceUserServiceInitialize(&prio);
        sceUserServiceGetInitialUser(&mut user);
        scePadInit();
        // 0x8026000e = already initialised (fine).
        let r = sceAudioOutInit();
        log(&format!("audio: sceAudioOutInit = {r:#x}"));
    }
    let pad = Pad { handle: unsafe { scePadOpen(user, 0, 0, core::ptr::null()) } };
    log(&format!("user {user:#x}, pad handle {:#x}", pad.handle));
    let mut cfg = Config::load_default().unwrap_or_default();
    log("opening video out");
    let mut present = (platform.open_present)(&cfg)?;
    unsafe { sceSystemServiceHideSplashScreen() };
    log("video out open, splash screen hidden");

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

    log("starting audio");
    let audio = start_audio(user).unwrap_or_else(|e| {
        log(&format!("audio unavailable: {e}"));
        AudioEngine::null(AUDIO_RATE)
    });
    let mut mapper = cfg.input_mapper();
    mapper.reset_controllers(1);
    cfg.input.controllers = mapper.controllers;
    log("creating the app (loading data)");
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

    let (mut iw, mut ih) = present.render_size();
    app.win = win_info(present.as_ref());
    log(&format!("rendering {iw}x{ih}"));
    let mut edges = host::RawEdges::default();
    let kb = KeyboardState::default();
    let mut last = Instant::now();
    let (mut frames, mut stat_t, mut busy) = (0u32, Instant::now(), Duration::ZERO);
    // Testing aid (emulators, no pad scripting): `mode-cycle.txt` in the save directory
    // switches to the next output mode every 5 seconds.
    let cycle_modes = save_dir.as_ref().is_some_and(|d| d.join("mode-cycle.txt").exists());
    let mut cycle_t = Instant::now();
    if cycle_modes {
        log("mode-cycle.txt: cycling the output modes every 5 s");
    }
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
        // Display settings changed: switch the output mode if the choice now picks another.
        let mut want = None;
        if std::mem::take(&mut app.requests.apply_window) | std::mem::take(&mut app.requests.apply_vsync) {
            let v = &app.cfg.video;
            want = host::pick_output_mode(&present.modes(), v.resolution, v.refresh_mhz);
            log(&format!("display settings {:?} / {:?} pick {want:?}", v.resolution, v.refresh_mhz));
        }
        if cycle_modes && cycle_t.elapsed() >= Duration::from_secs(5) {
            cycle_t = Instant::now();
            let modes = present.modes();
            let i = modes.iter().position(|&m| Some(m) == present.mode()).map_or(0, |i| i + 1);
            want = modes.get(i % modes.len().max(1)).copied();
        }
        if let Some(want) = want.filter(|&m| Some(m) != present.mode()) {
            log(&format!("output mode: {:?} -> {want:?}", present.mode()));
            if let Err(e) = present.set_mode(want) {
                log(&format!("output mode switch failed: {e}"));
            }
            (iw, ih) = present.render_size();
            app.win = win_info(present.as_ref());
            log(&format!("output mode now {:?}, rendering {iw}x{ih}", present.mode()));
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
