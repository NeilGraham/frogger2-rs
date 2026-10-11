//! The PS4 system layer (OpenOrbis homebrew, also runs on a jailbroken PS5 as a PS4
//! fPKG): `main`, video out, pads, audio out, logging. Only built for
//! `x86_64-unknown-freebsd` and linked by `tools/ps4/build-pkg.sh`.

pub mod alloc;
pub mod shim;

use crate::host::{self, Scaler};
use core::ffi::{c_char, c_int, c_void};
use f2_audio::{AudioEngine, AudioOutput, Mixer};
use f2_platform::{Config, KeyboardState, Profile, RawPad};
use frogger::app::{App, Step};
use std::ffi::CString;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ------------------------------------------------------------------ system calls

#[repr(C)]
#[derive(Default)]
struct BufferAttribute {
    format: i32,
    tmode: i32,
    aspect: i32,
    width: u32,
    height: u32,
    pixel_pitch: u32,
    reserved: [u64; 2],
}

#[repr(C)]
#[derive(Default)]
struct FlipStatus {
    num: u64,
    ptime: u64,
    stime: u64,
    flip_arg: i64,
    reserved: [u64; 2],
    num_gpu_flip_pending: i32,
    num_flip_pending: i32,
    current_buffer: i32,
    reserved1: u32,
}

/// `SceVideoOutResolutionStatus`: the current output mode.
#[repr(C)]
#[derive(Default)]
struct ResolutionStatus {
    full_width: u32,
    full_height: u32,
    pane_width: u32,
    pane_height: u32,
    refresh_rate: u64,
    screen_size: f32,
    flags: u16,
    reserved0: u16,
    reserved1: [u32; 3],
}

#[repr(C)]
#[derive(Default)]
struct KEvent {
    ident: u64,
    filter: i16,
    flags: u16,
    fflags: u32,
    data: i64,
    udata: usize,
}

/// `OrbisPadData` (only the leading fields are read; the buffer is oversized).
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
struct NotificationRequest {
    ty: i32,
    req_id: i32,
    priority: i32,
    msg_id: i32,
    target_id: i32,
    user_id: i32,
    unk1: i32,
    unk2: i32,
    app_id: i32,
    error_num: i32,
    unk3: i32,
    use_icon_image_uri: u8,
    message: [u8; 1024],
    icon_uri: [u8; 1024],
    unk: [u8; 1024],
}

extern "C" {
    fn sceKernelDebugOutText(channel: c_int, text: *const c_char) -> c_int;
    fn sceKernelSendNotificationRequest(device: c_int, req: *const NotificationRequest, size: usize, blocking: c_int) -> c_int;
    fn sceKernelAllocateDirectMemory(start: i64, end: i64, len: usize, align: usize, mem_type: c_int, phys: *mut i64) -> c_int;
    fn sceKernelGetDirectMemorySize() -> usize;
    fn sceKernelMapDirectMemory(addr: *mut *mut c_void, len: usize, prot: c_int, flags: c_int, phys: i64, align: usize) -> c_int;
    fn sceKernelCreateEqueue(eq: *mut *mut c_void, name: *const c_char) -> c_int;
    fn sceKernelWaitEqueue(eq: *mut c_void, ev: *mut KEvent, num: c_int, out: *mut c_int, timeout: *mut u32) -> c_int;

    fn sceVideoOutOpen(user: c_int, bus: c_int, index: c_int, param: *const c_void) -> c_int;
    fn sceVideoOutSetBufferAttribute(attr: *mut BufferAttribute, format: u32, tmode: u32, aspect: u32, width: u32, height: u32, pitch: u32);
    fn sceVideoOutRegisterBuffers(h: c_int, start: c_int, addrs: *const *mut c_void, n: c_int, attr: *const BufferAttribute) -> c_int;
    fn sceVideoOutSetFlipRate(h: c_int, rate: c_int) -> c_int;
    fn sceVideoOutAddFlipEvent(eq: *mut c_void, h: c_int, udata: *mut c_void) -> c_int;
    fn sceVideoOutSubmitFlip(h: c_int, index: c_int, mode: c_int, arg: i64) -> c_int;
    fn sceVideoOutGetFlipStatus(h: c_int, status: *mut FlipStatus) -> c_int;
    fn sceVideoOutGetResolutionStatus(h: c_int, status: *mut ResolutionStatus) -> c_int;

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

/// Writes a line to the kernel log (visible with a klog/debug-log viewer) and to
/// `frogger.log` in the save directory.
pub fn log(msg: &str) {
    if let Ok(c) = CString::new(format!("[frogger2] {msg}\n")) {
        unsafe { sceKernelDebugOutText(0, c.as_ptr()) };
    }
    if let Ok(mut f) = LOG.lock() {
        if let Some(f) = f.as_mut() {
            let _ = writeln!(f, "{msg}");
            let _ = f.flush();
        }
    }
}

/// A system notification toast.
fn notify(msg: &str) {
    let mut req: Box<NotificationRequest> = unsafe { Box::new(core::mem::zeroed()) };
    req.target_id = -1;
    let n = msg.len().min(1023);
    req.message[..n].copy_from_slice(&msg.as_bytes()[..n]);
    unsafe { sceKernelSendNotificationRequest(0, &*req, core::mem::size_of::<NotificationRequest>(), 0) };
}

// ------------------------------------------------------------------ video

const BUFFERS: usize = 2;
/// `ORBIS_VIDEO_OUT_PIXEL_FORMAT_A8B8G8R8_SRGB`: bytes R, G, B, A, the CPU renderer's order.
const PIXEL_FORMAT: u32 = 0x8000_2200;

struct Video {
    /// Framebuffer size (`host::display_size` of the output mode).
    width: u32,
    height: u32,
    handle: c_int,
    queue: *mut c_void,
    buffers: [*mut u32; BUFFERS],
    frame: i64,
}

impl Video {
    fn open() -> Result<Video, String> {
        unsafe {
            let handle = sceVideoOutOpen(0xff, 0, 0, core::ptr::null());
            if handle < 0 {
                return Err(format!("sceVideoOutOpen: {handle:#x}"));
            }
            let mut queue = core::ptr::null_mut();
            let r = sceKernelCreateEqueue(&mut queue, c"frogger flip".as_ptr());
            if r < 0 {
                return Err(format!("sceKernelCreateEqueue: {r:#x}"));
            }
            sceVideoOutAddFlipEvent(queue, handle, core::ptr::null_mut());
            let mut res = ResolutionStatus::default();
            let r = sceVideoOutGetResolutionStatus(handle, &mut res);
            let (width, height) = host::display_size(if r == 0 { (res.full_width, res.full_height) } else { (0, 0) });
            log(&format!("video out: mode {}x{} ({r:#x}), framebuffers {width}x{height}", res.full_width, res.full_height));
            const ALIGN: usize = 0x20_0000;
            let fb = (width * height * 4) as usize;
            let fb = fb.div_ceil(ALIGN) * ALIGN;
            let size = fb * BUFFERS;
            let mut phys = 0i64;
            // Memory type 3 = write-combined (WB_ONION), as the OpenOrbis samples.
            let r = sceKernelAllocateDirectMemory(0, sceKernelGetDirectMemorySize() as i64, size, ALIGN, 3, &mut phys);
            if r < 0 {
                return Err(format!("sceKernelAllocateDirectMemory: {r:#x}"));
            }
            let mut mem = core::ptr::null_mut();
            // prot 0x33 = CPU read/write + GPU read/write.
            let r = sceKernelMapDirectMemory(&mut mem, size, 0x33, 0, phys, ALIGN);
            if r < 0 {
                return Err(format!("sceKernelMapDirectMemory: {r:#x}"));
            }
            let mut buffers = [core::ptr::null_mut(); BUFFERS];
            for (i, b) in buffers.iter_mut().enumerate() {
                *b = (mem as *mut u8).add(i * fb) as *mut u32;
                core::ptr::write_bytes(*b, 0, (width * height) as usize);
            }
            let mut attr = BufferAttribute::default();
            // Linear tiling, 16:9.
            sceVideoOutSetBufferAttribute(&mut attr, PIXEL_FORMAT, 1, 0, width, height, width);
            let ptrs: [*mut c_void; BUFFERS] = buffers.map(|b| b as *mut c_void);
            let r = sceVideoOutRegisterBuffers(handle, 0, ptrs.as_ptr(), BUFFERS as c_int, &attr);
            if r < 0 {
                return Err(format!("sceVideoOutRegisterBuffers: {r:#x}"));
            }
            sceVideoOutSetFlipRate(handle, 0);
            Ok(Video { width, height, handle, queue, buffers, frame: 0 })
        }
    }

    /// Waits until flip `arg` has been displayed.
    fn wait_flip(&self, arg: i64) {
        if arg <= 0 {
            return;
        }
        let mut st = FlipStatus::default();
        for _ in 0..120 {
            unsafe { sceVideoOutGetFlipStatus(self.handle, &mut st) };
            if st.flip_arg >= arg {
                return;
            }
            let (mut ev, mut n) = (KEvent::default(), 0);
            if unsafe { sceKernelWaitEqueue(self.queue, &mut ev, 1, &mut n, core::ptr::null_mut()) } != 0 {
                return;
            }
        }
    }

    /// The buffer for the next frame (once the frame that last used it is off screen).
    fn back_buffer(&self) -> *mut u32 {
        self.wait_flip(self.frame - 1);
        self.buffers[(self.frame as usize + 1) % BUFFERS]
    }

    fn present(&mut self) {
        self.frame += 1;
        unsafe { sceVideoOutSubmitFlip(self.handle, (self.frame as usize % BUFFERS) as c_int, 1, self.frame) };
    }

    /// Clears every framebuffer to black without presenting.
    fn clear_all(&self) {
        for &b in &self.buffers {
            unsafe { core::ptr::write_bytes(b, 0, (self.width * self.height) as usize) };
        }
    }

    /// Fills the whole screen with one colour (status screens).
    fn fill(&mut self, rgba: u32) {
        let b = self.back_buffer();
        for i in 0..(self.width * self.height) as usize {
            unsafe { *b.add(i) = rgba };
        }
        self.present();
    }
}

// ------------------------------------------------------------------ audio

const AUDIO_RATE: u32 = 48_000;
const AUDIO_GRAIN: usize = 256;

struct OrbisAudio {
    user: c_int,
}

impl AudioOutput for OrbisAudio {
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

#[no_mangle]
pub extern "C" fn main(_argc: c_int, _argv: *const *const c_char) -> c_int {
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("panic: {info}");
        log(&msg);
        notify(&format!("Frogger 2 crashed:\n{msg}"));
    }));
    match run() {
        Ok(()) => 0,
        Err(e) => {
            log(&format!("fatal: {e}"));
            notify(&format!("Frogger 2: {e}"));
            std::thread::sleep(Duration::from_secs(5));
            1
        }
    }
}

fn run() -> Result<(), String> {
    let save_dir = host::writable_dir(host::SAVE_DIRS);
    if let Some(d) = &save_dir {
        f2_platform::paths::set_config_dir(d.clone());
        if let Ok(f) = std::fs::File::create(d.join("frogger.log")) {
            *LOG.lock().unwrap() = Some(f);
        }
    }
    log(&format!("Frogger 2 (Rust port) for PS4/PS5 homebrew, save dir {save_dir:?}"));

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
    let mut video = Video::open()?;
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
            notify(msg);
        }
        for _ in 0..120 {
            video.fill(0xff40_1010);
        }
    };
    log(&format!("data {}", data.rendition.display()));

    let audio = match (OrbisAudio { user }).start_engine() {
        Ok(a) => a,
        Err(e) => {
            log(&format!("audio unavailable: {e}"));
            AudioEngine::null(AUDIO_RATE)
        }
    };
    let mut mapper = cfg.input_mapper();
    mapper.reset_controllers(1);
    cfg.input.controllers = mapper.controllers;
    let mut app = App::new_soft(data, audio, mapper);
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
    // Dev/testing: `autostart.txt` in the save dir picks the start-up (see host::autostart).
    let start = save_dir.as_ref().and_then(|d| std::fs::read_to_string(d.join("autostart.txt")).ok()).and_then(|t| host::autostart(&t));
    if let Some(s) = &start {
        log(&format!("autostart: {s:?}"));
    }
    app.push_steps([start.unwrap_or(Step::Boot)]);

    let (dw, dh) = (video.width, video.height);
    let mut dynres = host::DynRes::new(dh);
    let (mut iw, mut ih) = host::internal_size_at(dw, dh, dynres.height());
    app.win = frogger::app::WinInfo { size: (iw, ih), ..Default::default() };
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 7);
    log(&format!(
        "rendering {iw}x{ih} (dynamic, up to {}p) on {threads} threads, output {dw}x{dh}",
        host::DynRes::LEVELS[..].iter().rev().find(|&&h| h <= dh).unwrap_or(&480)
    ));
    let mut target = f2_engine::SoftTarget::new(iw, ih);
    let mut scaler = Scaler::default();
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
        let t0 = Instant::now();
        app.r.render_soft(&mut target, app.clear_color(), threads);
        let render_ms = t0.elapsed().as_secs_f32() * 1000.0;
        let fb = video.back_buffer();
        unsafe { scaler.blit_par(&target.pixels, target.width, target.height, fb, dw, dh, dw, threads) };
        if dynres.update(render_ms) {
            (iw, ih) = host::internal_size_at(dw, dh, dynres.height());
            app.win.size = (iw, ih);
            // The new size can letterbox differently: clear stale borders.
            video.clear_all();
            log(&format!("render size now {iw}x{ih} ({render_ms:.1} ms)"));
        }
        busy += now.elapsed();
        video.present();
        frames += 1;
        if stat_t.elapsed() >= Duration::from_secs(10) {
            let s = stat_t.elapsed().as_secs_f64();
            log(&format!("{:.1} fps, {:.1} ms/frame busy", frames as f64 / s, busy.as_secs_f64() * 1000.0 / frames.max(1) as f64));
            (frames, stat_t, busy) = (0, Instant::now(), Duration::ZERO);
        }
    }
}

trait StartEngine {
    fn start_engine(self) -> Result<AudioEngine, String>;
}

impl StartEngine for OrbisAudio {
    /// `AudioEngine::new` falls back to muted when `start` fails; report that as an error.
    fn start_engine(self) -> Result<AudioEngine, String> {
        let e = AudioEngine::new(Box::new(self));
        if e.is_realtime() {
            Ok(e)
        } else {
            Err("audio output failed to start".into())
        }
    }
}
