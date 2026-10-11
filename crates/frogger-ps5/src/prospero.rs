//! The PS5 system layer: `main`, video out and notifications. Only built for
//! `x86_64-unknown-freebsd` and linked by `tools/ps5/build.sh`.
//!
//! Video out: VideoOut buffers on the PS5 are tiled (tiling mode 0; the console scans
//! them out and scales them to the TV's mode itself), so the CPU renderer's image is
//! scaled into them with [`TiledScaler`]. The buffer size follows the output resolution
//! when `sceVideoOutGetResolutionStatus` reports it (3840x2160, else 2560x1440, else
//! 1920x1080), and falls back down that list if registering a size fails. 120 Hz: the
//! title declares high-frame-rate support in `param.json` (`attribute3` 0x80040); output
//! mode 15 is then requested with `sceVideoOutConfigureOutput`, kept only if the measured
//! vblank period shows the display really refreshes at ~120 Hz, else mode 1 (59.94 Hz) is
//! restored. This is the sequence the PS5 Vulkan project measured on hardware
//! (`driver/ps5vk_wsi.c`, jobs r51/r92 there). The game keeps its fixed 60 Hz tick; at
//! 120 Hz the extra frames come from the front end's interpolated rendering.

use console_host::host::{self, TiledScaler};
#[cfg(not(feature = "smoke"))]
use console_host::sce::Platform;
#[cfg(not(feature = "gpu"))]
use console_host::sce::SoftPresent;
use console_host::sce::{self, KEvent, Present, Screen};
use core::ffi::{c_char, c_int, c_void};
use std::time::{Duration, Instant};

// ------------------------------------------------------------------ system calls

/// `sceVideoOutRegisterBuffers2` entry.
#[repr(C)]
struct VideoBuffer {
    data: *mut c_void,
    metadata: *mut c_void,
    reserved: [*mut c_void; 2],
}

/// The 80-byte opaque attribute `sceVideoOutSetBufferAttribute2` fills.
#[repr(C, align(16))]
struct BufferAttribute([u8; 80]);

extern "C" {
    fn sceVideoOutOpen(user: c_int, bus: c_int, index: c_int, param: *const c_void) -> c_int;
    fn sceVideoOutSetFlipRate(h: c_int, rate: c_int) -> c_int;
    fn sceVideoOutSetBufferAttribute2(attr: *mut BufferAttribute, format: u64, tiling: u32, width: u32, height: u32, option: u64, dcc: u32, dcc_clear: u64);
    fn sceVideoOutRegisterBuffers2(
        h: c_int,
        set: c_int,
        start: c_int,
        buffers: *const VideoBuffer,
        n: c_int,
        attr: *const BufferAttribute,
        category: c_int,
        option: *const c_void,
    ) -> c_int;
    fn sceVideoOutAddFlipEvent(eq: *mut c_void, h: c_int, udata: *mut c_void) -> c_int;
    fn sceVideoOutSubmitFlip(h: c_int, index: c_int, mode: u32, arg: i64) -> c_int;
    /// Fills 16 64-bit words; word 3 is the argument of the latest flip shown.
    fn sceVideoOutGetFlipStatus(h: c_int, status: *mut [u64; 16]) -> c_int;
    fn sceVideoOutWaitVblank(h: c_int) -> c_int;
    fn sceVideoOutIsOutputSupported(h: c_int, mode: u32, a: *const c_void, b: *const c_void, c: *const c_void) -> c_int;
    fn sceVideoOutConfigureOutput(h: c_int, mode: u32, a: *const c_void, b: *const c_void, c: *const c_void) -> c_int;
    /// PS4 layout: full width, full height, pane width, pane height (u32), refresh rate
    /// (u64), ...; read defensively.
    fn sceVideoOutGetResolutionStatus(h: c_int, status: *mut [u32; 16]) -> c_int;
}

/// A system notification toast (the payload SDK's `notify_request_t` layout).
pub fn notify(msg: &str) {
    #[repr(C)]
    struct Request {
        reserved: [u8; 45],
        message: [u8; 3075],
    }
    let mut req: Box<Request> = unsafe { Box::new(core::mem::zeroed()) };
    let n = msg.len().min(3074);
    req.message[..n].copy_from_slice(&msg.as_bytes()[..n]);
    unsafe { sce::sceKernelSendNotificationRequest(0, &*req as *const Request as *const c_void, core::mem::size_of::<Request>(), 0) };
}

// ------------------------------------------------------------------ video

/// `SCE_VIDEO_OUT_PIXEL_FORMAT_A8B8G8R8_SRGB` (64-bit form): bytes R, G, B, A, the CPU
/// renderer's order.
const PIXEL_FORMAT: u64 = 0x8000_0000_2200_0000;
const BUFFERS: usize = 2;
const ALIGN: usize = 0x20_0000;
/// Output modes for `sceVideoOutConfigureOutput`: 15 = high frame rate (119.88 Hz),
/// 1 = the default (59.94 Hz).
const MODE_HIGH_FRAME_RATE: u32 = 15;
const MODE_DEFAULT: u32 = 1;
/// Buffer sizes tried, largest first.
const SIZES: [(u32, u32); 3] = [(3840, 2160), (2560, 1440), (1920, 1080)];
/// Creating this file keeps the output at 60 Hz.
const NO_HFR_FLAG: &str = "/data/frogger2/60hz";

pub struct Video {
    handle: c_int,
    queue: *mut c_void,
    buffers: [*mut u8; BUFFERS],
    width: u32,
    height: u32,
    frame: i64,
    scaler: TiledScaler,
    threads: usize,
    /// Display refresh in Hz (measured).
    pub refresh: f64,
}

/// Average vblank period over `n` vblanks (after a few to settle).
fn vblank_period(h: c_int, n: u32) -> Duration {
    for _ in 0..4 {
        unsafe { sceVideoOutWaitVblank(h) };
    }
    let t = Instant::now();
    for _ in 0..n {
        unsafe { sceVideoOutWaitVblank(h) };
    }
    t.elapsed() / n
}

impl Video {
    pub fn open(threads: usize) -> Result<Video, String> {
        let handle = unsafe { sceVideoOutOpen(0xff, 0, 0, core::ptr::null()) };
        if handle < 0 {
            return Err(format!("sceVideoOutOpen: {handle:#x}"));
        }

        // The TV's mode, to skip buffer sizes the output would only scale down.
        let mut st = [0u32; 16];
        let r = unsafe { sceVideoOutGetResolutionStatus(handle, &mut st) };
        let output = (r == 0 && (640..=7680).contains(&st[0]) && (480..=4320).contains(&st[1])).then_some((st[0], st[1]));
        sce::log(&format!("resolution status {r:#x}: {:?} -> output {output:?}", &st[..6]));

        // 120 Hz on a fresh handle, before the flip rate and buffers.
        let mut refresh = 1.0 / vblank_period(handle, 20).as_secs_f64();
        if std::path::Path::new(NO_HFR_FLAG).exists() {
            sce::log(&format!("{NO_HFR_FLAG} exists: staying at {refresh:.2} Hz"));
        } else {
            let null = core::ptr::null();
            let supported = unsafe { sceVideoOutIsOutputSupported(handle, MODE_HIGH_FRAME_RATE, null, null, null) };
            let configured = if supported > 0 { unsafe { sceVideoOutConfigureOutput(handle, MODE_HIGH_FRAME_RATE, null, null, null) } } else { supported };
            if configured == 0 {
                let hz = 1.0 / vblank_period(handle, 30).as_secs_f64();
                if hz > 100.0 {
                    sce::log(&format!("output: 119.88 Hz selected, measured {hz:.2} Hz"));
                    refresh = hz;
                } else {
                    let back = unsafe { sceVideoOutConfigureOutput(handle, MODE_DEFAULT, null, null, null) };
                    sce::log(&format!("output: 119.88 Hz accepted but measured {hz:.2} Hz; 59.94 Hz restored ({back:#x})"));
                }
            } else {
                sce::log(&format!("output: 119.88 Hz not available (supported {supported:#x}, configure {configured:#x}); {refresh:.2} Hz"));
            }
        }

        let mut queue = core::ptr::null_mut();
        let r = unsafe { sce::sceKernelCreateEqueue(&mut queue, c"frogger flip".as_ptr()) };
        if r < 0 {
            return Err(format!("sceKernelCreateEqueue: {r:#x}"));
        }
        unsafe { sceVideoOutAddFlipEvent(queue, handle, core::ptr::null_mut()) };

        let sizes: Vec<(u32, u32)> = match output {
            Some((_, oh)) => SIZES.iter().copied().filter(|&(_, h)| h <= oh.max(1080)).collect(),
            None => SIZES.to_vec(),
        };
        let fb = host::tiled_size(sizes[0].0, sizes[0].1).div_ceil(ALIGN) * ALIGN;
        let total = fb * BUFFERS;
        let mut phys = 0i64;
        // Memory type 3 = write-combined (WB_ONION), CPU and GPU read/write.
        let r = unsafe { sce::sceKernelAllocateDirectMemory(0, sce::sceKernelGetDirectMemorySize() as i64, total, ALIGN, 3, &mut phys) };
        if r < 0 {
            return Err(format!("sceKernelAllocateDirectMemory({total:#x}): {r:#x}"));
        }
        let mut mem = core::ptr::null_mut();
        let r = unsafe { sce::sceKernelMapDirectMemory(&mut mem, total, 0x33, 0, phys, ALIGN) };
        if r < 0 {
            return Err(format!("sceKernelMapDirectMemory: {r:#x}"));
        }
        let buffers: [*mut u8; BUFFERS] = core::array::from_fn(|i| unsafe { (mem as *mut u8).add(i * fb) });
        unsafe { core::ptr::write_bytes(mem as *mut u8, 0, total) };
        unsafe { sceVideoOutSetFlipRate(handle, 0) };

        let entries: [VideoBuffer; BUFFERS] =
            buffers.map(|b| VideoBuffer { data: b as *mut c_void, metadata: core::ptr::null_mut(), reserved: [core::ptr::null_mut(); 2] });
        for &(width, height) in &sizes {
            let mut attr = BufferAttribute([0; 80]);
            unsafe {
                // Tiling mode 0 (the scan-out tiled layout), no DCC.
                sceVideoOutSetBufferAttribute2(&mut attr, PIXEL_FORMAT, 0, width, height, 0, 0, 0);
            }
            let r = unsafe { sceVideoOutRegisterBuffers2(handle, 0, 0, entries.as_ptr(), BUFFERS as c_int, &attr, 0, core::ptr::null()) };
            if r == 0 {
                sce::log(&format!("video out {width}x{height} tiled at {refresh:.2} Hz, {BUFFERS} buffers"));
                return Ok(Video { handle, queue, buffers, width, height, frame: 0, scaler: TiledScaler::default(), threads, refresh });
            }
            sce::log(&format!("sceVideoOutRegisterBuffers2 {width}x{height}: {r:#x}"));
        }
        Err("no VideoOut buffer size was accepted".into())
    }

    /// Waits until flip `arg` has been displayed.
    fn wait_flip(&self, arg: i64) {
        if arg <= 0 {
            return;
        }
        let mut st = [0u64; 16];
        for _ in 0..240 {
            unsafe { sceVideoOutGetFlipStatus(self.handle, &mut st) };
            if st[3] as i64 >= arg {
                return;
            }
            let (mut ev, mut n) = (KEvent::default(), 0);
            if unsafe { sce::sceKernelWaitEqueue(self.queue, &mut ev, 1, &mut n, core::ptr::null_mut()) } != 0 {
                return;
            }
        }
    }

    /// The buffer for the next frame (once the frame that last used it is off screen).
    fn back_buffer(&self) -> *mut u8 {
        self.wait_flip(self.frame - 1);
        self.buffers[(self.frame as usize + 1) % BUFFERS]
    }

    fn flip(&mut self) {
        self.frame += 1;
        // Flip mode 1: at vblank.
        unsafe { sceVideoOutSubmitFlip(self.handle, (self.frame as usize % BUFFERS) as c_int, 1, self.frame) };
    }
}

impl Screen for Video {
    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn show(&mut self, pixels: &[u32], w: u32, h: u32, exec: host::Exec) {
        let fb = self.back_buffer();
        unsafe { self.scaler.blit(pixels, w, h, fb, self.width, self.height, self.threads, exec) };
        self.flip();
    }

    fn fill(&mut self, rgba: u32) {
        let fb = self.back_buffer() as *mut u32;
        // Every pixel of the (tiled) buffer has the same value: layout does not matter.
        for i in 0..host::tiled_size(self.width, self.height) / 4 {
            unsafe { fb.add(i).write(rgba) };
        }
        self.flip();
    }
}

/// The CPU build packaged beside the GPU one (`tools/ps5/build.sh --gpu`).
#[cfg(feature = "gpu")]
const CPU_EBOOT: &core::ffi::CStr = c"/app0/eboot_cpu.bin";

/// GPU build: wgpu on the PS5 Vulkan driver. `frogger`'s `gpu` feature compiles the CPU
/// renderer out of `App`, so the fallback when Vulkan does not start is the CPU build in
/// the same folder, started in place of this process with `sceSystemServiceLoadExec`.
#[cfg(feature = "gpu")]
#[cfg_attr(feature = "smoke", allow(dead_code))]
fn open_present() -> Result<Box<dyn Present>, String> {
    extern "C" {
        fn sceSystemServiceLoadExec(path: *const c_char, argv: *const *const c_char) -> c_int;
    }
    match crate::gpu::open() {
        Ok(p) => Ok(Box::new(p)),
        Err(e) => {
            sce::log(&format!("GPU renderer unavailable ({e}); starting the CPU build {CPU_EBOOT:?}"));
            let r = unsafe { sceSystemServiceLoadExec(CPU_EBOOT.as_ptr(), core::ptr::null()) };
            Err(format!("{e}; sceSystemServiceLoadExec({CPU_EBOOT:?}): {r:#x}"))
        }
    }
}

#[cfg(not(feature = "gpu"))]
#[cfg_attr(feature = "smoke", allow(dead_code))]
fn open_present() -> Result<Box<dyn Present>, String> {
    // PS5 titles get 13 of the 16 hardware threads; leave room for audio and the system
    // (8 when the system does not report a count, as in the KytyPS5 emulator).
    let threads = sce::render_threads(10, 8);
    let video = Video::open(threads)?;
    let lines = host::render_lines(video.height);
    sce::log(&format!("CPU renderer: {lines} lines, {threads} threads"));
    Ok(Box::new(SoftPresent::new(video, lines, threads)))
}

#[cfg(not(feature = "smoke"))]
#[no_mangle]
pub extern "C" fn main(_argc: c_int, _argv: *const *const c_char) -> c_int {
    sce::set_log_sink(crate::compat::log_sink);
    sce::main(&Platform { name: "PS5", notify, open_present })
}

#[cfg(feature = "smoke")]
#[no_mangle]
pub extern "C" fn main(_argc: c_int, _argv: *const *const c_char) -> c_int {
    sce::set_log_sink(crate::compat::log_sink);
    crate::smoke::run()
}
