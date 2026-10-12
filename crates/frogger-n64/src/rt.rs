//! N64 runtime glue: global allocator over libdragon's heap, panic handler, logging and
//! thin safe wrappers over the C interface.

use crate::ffi::{self, HeapStats, Rect, State, TexDesc, Vertex, Voice};
use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

/// libdragon's heap with byte counters of the Rust allocations (live, high-water mark).
struct LibdragonHeap;

static LIVE: AtomicU32 = AtomicU32::new(0);
static PEAK: AtomicU32 = AtomicU32::new(0);

unsafe impl GlobalAlloc for LibdragonHeap {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { ffi::n64_alloc(l.size() as u32, l.align() as u32) };
        if !p.is_null() {
            let live = LIVE.load(Relaxed) + l.size() as u32;
            LIVE.store(live, Relaxed);
            if live > PEAK.load(Relaxed) {
                PEAK.store(live, Relaxed);
            }
        } else {
            // Out of memory (or too fragmented): say how much was in use (no allocation here).
            let h = heap_stats();
            crate::log!(
                "ALLOC FAILED: {} bytes (align {}); rust live {} KiB; libdragon used {} / {} KiB",
                l.size(),
                l.align(),
                LIVE.load(Relaxed) / 1024,
                h.used / 1024,
                h.total / 1024
            );
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.store(LIVE.load(Relaxed).wrapping_sub(l.size() as u32), Relaxed);
        unsafe { ffi::n64_free(p) }
    }
}

/// Bytes the Rust side has allocated now / at most since the last [`reset_peak`].
pub fn rust_heap() -> (u32, u32) {
    (LIVE.load(Relaxed), PEAK.load(Relaxed))
}

pub fn reset_peak() {
    PEAK.store(LIVE.load(Relaxed), Relaxed);
}

/// libdragon heap (C and Rust allocations): (total, used) bytes.
pub fn heap_stats() -> HeapStats {
    let mut s = HeapStats::default();
    unsafe { ffi::n64_heap_stats(&mut s) };
    s
}

/// NUL-terminated copy of a path for the C side.
fn cpath(path: &str) -> Vec<u8> {
    let mut z = Vec::with_capacity(path.len() + 1);
    z.extend_from_slice(path.as_bytes());
    z.push(0);
    z
}

#[global_allocator]
static HEAP: LibdragonHeap = LibdragonHeap;

/// Fixed-size formatting buffer (usable without the heap, e.g. in the panic handler).
pub struct LineBuf {
    buf: [u8; 1024],
    len: usize,
}

impl LineBuf {
    pub const fn new() -> Self {
        LineBuf { buf: [0; 1024], len: 0 }
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

impl Default for LineBuf {
    fn default() -> Self {
        Self::new()
    }
}

impl Write for LineBuf {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

pub fn log_fmt(args: fmt::Arguments) {
    let mut b = LineBuf::new();
    let _ = b.write_fmt(args);
    let _ = b.write_str("\n");
    unsafe { ffi::n64_log(b.as_bytes().as_ptr(), b.len as u32) };
}

#[macro_export]
macro_rules! log {
    ($($t:tt)*) => { $crate::rt::log_fmt(format_args!($($t)*)) };
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let mut b = LineBuf::new();
    let _ = write!(b, "{info}");
    unsafe { ffi::n64_panic(b.as_bytes().as_ptr(), b.len as u32) }
}

/// C0 COUNT ticks (46.875 MHz = CPU clock / 2).
#[inline]
pub fn ticks() -> u32 {
    unsafe { ffi::n64_ticks() }
}

pub const TICKS_PER_SEC: u32 = 46_875_000;

pub fn ticks_to_us(t: u32) -> u32 {
    ((t as u64 * 1_000_000) / TICKS_PER_SEC as u64) as u32
}

pub fn pad(port: u32) -> u32 {
    unsafe { ffi::n64_pad(port) }
}

/// Reads a DFS file (`rom:/...`), transparently decompressing mkasset files.
pub fn file_read(path: &str) -> Option<Vec<u8>> {
    let z = cpath(path);
    let mut len = 0u32;
    let p = unsafe { ffi::n64_file_read(z.as_ptr(), &mut len) };
    if p.is_null() {
        return None;
    }
    // asset_load allocates with malloc; our global allocator frees with free() whatever the
    // layout, so the buffer can be adopted by a Vec (counted from now on).
    let live = LIVE.load(Relaxed) + len;
    LIVE.store(live, Relaxed);
    if live > PEAK.load(Relaxed) {
        PEAK.store(live, Relaxed);
    }
    Some(unsafe { Vec::from_raw_parts(p, len as usize, len as usize) })
}

pub fn tex_create_rgba16(width: u32, height: u32, pixels: &[u16]) -> u32 {
    let d = TexDesc { width, height, fmt: 0, pixels: pixels.as_ptr() as *const u8 };
    unsafe { ffi::n64_tex_create(&d) }
}

/// Creates a texture from its full image and an optional TMEM-sized version for triangles.
/// Replaces texture `h`'s texels in place (same size/format); false if not possible.
pub fn tex_update(h: u32, d: &TexDesc) -> bool {
    unsafe { ffi::n64_tex_update(h, d) != 0 }
}

pub fn tex_create2(full: &TexDesc, small: Option<&TexDesc>) -> u32 {
    unsafe { ffi::n64_tex_create2(full, small.map_or(core::ptr::null(), |s| s as *const TexDesc)) }
}

pub fn tex_free(h: u32) {
    unsafe { ffi::n64_tex_free(h) }
}

pub fn draw_rects(rects: &[Rect], state: &State) {
    unsafe { ffi::n64_draw_rects(rects.as_ptr(), rects.len() as u32, state) }
}

pub fn sfx_open(path: &str, looped: bool) -> u32 {
    let z = cpath(path);
    unsafe { ffi::n64_sfx_open(z.as_ptr(), looped as u32) }
}

/// RSP and RDP busy fractions (permille) since the last call.
pub fn rcp_load() -> (u32, u32) {
    let v = unsafe { ffi::n64_rcp_load() };
    (v & 0xFFFF, v >> 16)
}

/// Plays a video (blocking); false if the file is missing.
pub fn fmv_play(path: &str) -> bool {
    let z = cpath(path);
    unsafe { ffi::n64_fmv_play(z.as_ptr()) != 0 }
}

/// Reads `len` bytes of cartridge SRAM at `off`.
pub fn sram_read(off: usize, len: usize) -> Option<Vec<u8>> {
    let mut b = alloc::vec![0u8; len];
    (unsafe { ffi::n64_save_read(b.as_mut_ptr(), off as u32, len as u32) } as usize == len).then_some(b)
}

/// Writes cartridge SRAM at `off`.
pub fn sram_write(off: usize, data: &[u8]) -> bool {
    unsafe { ffi::n64_save_write(data.as_ptr(), off as u32, data.len() as u32) as usize == data.len() }
}

/// Native playback rate of an opened sample (Hz), 0 if unknown.
pub fn sfx_freq(h: u32) -> u32 {
    unsafe { ffi::n64_sfx_freq(h) }
}

pub fn sfx_close(h: u32) {
    unsafe { ffi::n64_sfx_close(h) }
}

/// (first sound-effect mixer channel, number of channels).
pub fn sfx_channels() -> (u32, u32) {
    let v = unsafe { ffi::n64_sfx_channels() };
    (v >> 8, v & 0xff)
}

pub fn sfx_play(h: u32, ch: u32, v: &Voice) {
    unsafe { ffi::n64_sfx_play(h, ch, v) }
}

pub fn ch_set(ch: u32, v: &Voice) {
    unsafe { ffi::n64_ch_set(ch, v) }
}

pub fn ch_stop(ch: u32) {
    unsafe { ffi::n64_ch_stop(ch) }
}

pub fn ch_playing(ch: u32) -> bool {
    unsafe { ffi::n64_ch_playing(ch) != 0 }
}

/// Starts a wav64 music stream (`None` stops); false if the file is missing.
pub fn music_play(path: Option<&str>, looped: bool) -> bool {
    match path {
        Some(p) => {
            let z = cpath(p);
            unsafe { ffi::n64_music_play(z.as_ptr(), looped as u32) != 0 }
        }
        None => unsafe { ffi::n64_music_play(core::ptr::null(), 0) != 0 },
    }
}

pub fn music_vol(vol: f32) {
    unsafe { ffi::n64_music_vol((vol.clamp(0.0, 1.0) * 1000.0) as u32) }
}

/// Feeds the audio mixer if the AI needs data (call often during long work).
pub fn audio_poll() {
    unsafe { ffi::n64_audio_poll() }
}

/// [`audio_poll`] if 40 ms have passed since the last poll (for loops that can run long).
#[inline]
pub fn audio_poll_due() {
    unsafe { ffi::n64_audio_poll_due() }
}

/// The longest time between two audio polls (any, the C side's included) since the last
/// call, in ms (the music buffer holds 300: longer gaps are audible dropouts).
pub fn audio_gap_take_ms() -> u32 {
    ticks_to_us(unsafe { ffi::n64_audio_gap_take() }) / 1000
}

/// C0 ticks the last [`begin_frame`] waited for a free framebuffer (the RSP/RDP and VI
/// still busy with earlier frames).
pub static FB_WAIT: AtomicU32 = AtomicU32::new(0);

pub fn begin_frame(clear_rgba: u32) {
    let t0 = ticks();
    begin_frame_inner(clear_rgba);
    FB_WAIT.store(ticks().wrapping_sub(t0), Relaxed);
}

fn begin_frame_inner(clear_rgba: u32) {
    unsafe { ffi::n64_begin_frame(clear_rgba) }
}

/// Scene triangles of the renderer's frame lists, converted on the C side
/// (`n64_draw_tl_tris`).
pub fn draw_tl_tris(tris: &[f2_engine::Tri], zk: f32, state: &State) {
    let Some(first) = tris.first() else { return };
    let b = ffi::TlBatch { tris: first.v.as_ptr() as *const u8, stride: core::mem::size_of::<f2_engine::Tri>() as u32, ntris: tris.len() as u32, zk };
    unsafe { ffi::n64_draw_tl_tris(&b, state) }
}

pub fn draw_tris(verts: &[Vertex], state: &State) {
    unsafe { ffi::n64_draw_tris(verts.as_ptr(), (verts.len() / 3) as u32, state) }
}

pub fn present() {
    unsafe { ffi::n64_present() }
}

/// Shows frames at most `fps` times a second (`display_set_fps_limit`; 0 = no limit).
pub fn set_fps_limit(fps: u32) {
    unsafe { ffi::n64_set_fps_limit(fps) }
}

pub fn rdp_wait() {
    unsafe { ffi::n64_rdp_wait() }
}

pub fn beep(freq_hz: u32, ms: u32) {
    unsafe { ffi::n64_beep(freq_hz, ms) }
}

pub fn audio_samples() -> u32 {
    unsafe { ffi::n64_audio_stats() }
}

pub fn memory_size() -> u32 {
    unsafe { ffi::n64_memory_size() }
}

pub fn probe_c(kind: u32, n: u32) -> u32 {
    unsafe { ffi::n64_probe_c(kind, n) }
}
