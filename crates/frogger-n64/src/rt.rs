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
            // Testing: who owns the play-time blocks (a full report without the trace build).
            if option_env!("F2N64_MEM_LIVE").is_some() {
                unsafe { ffi::n64_mem_report(b"fail".as_ptr(), 4, 8) };
            }
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

/// Allocation phases (`csrc/main.c` "the level heap"): while a level loads, every allocation
/// comes from the level heap, which the next level change empties again.
pub const MEM_PLAY: u32 = 0;
pub const MEM_LEVEL: u32 = 1;
/// Session data (lives for the whole run): main heap whatever the size.
pub const MEM_MAIN: u32 = 2;
/// A level load, from the level heap's top (the renderer's side; the simulation's InitLevel
/// fills it from the bottom).
pub const MEM_LEVEL_TOP: u32 = 3;

/// Sets the allocation phase; returns the previous one. MEM_PLAY -> MEM_LEVEL starts a new
/// load (epoch).
pub fn mem_phase(phase: u32) -> u32 {
    unsafe { ffi::n64_mem_phase(phase) }
}

/// Runs `f` with session allocations (main heap): for anything allocated during a level
/// load or play that outlives the level (fonts, caches, session state).
pub fn mem_play<T>(f: impl FnOnce() -> T) -> T {
    let old = mem_phase(MEM_MAIN);
    let r = f();
    mem_phase_resume(old);
    r
}

/// During a level load: `f`'s allocations from the level heap's other end (file buffers that
/// are parsed and dropped): they go into the free middle and leave no holes among the data
/// of the side that is loading (InitLevel from the bottom, the renderer from the top).
/// Outside a load, `f` as is.
pub fn mem_transient<T>(f: impl FnOnce() -> T) -> T {
    let old = mem_phase(MEM_LEVEL_TOP | 0x100);
    match old {
        MEM_LEVEL => {}
        MEM_LEVEL_TOP => {
            mem_phase_resume(MEM_LEVEL);
        }
        _ => {
            mem_phase_resume(old);
            return f();
        }
    }
    let r = f();
    mem_phase_resume(old);
    r
}

/// Back to a phase [`mem_phase`] returned, without starting a new load.
pub fn mem_phase_resume(old: u32) {
    unsafe { ffi::n64_mem_phase(old | 0x100) };
}

/// One `mem [tag]` log line (both heaps, level-heap blocks by lifetime); `check` also lists
/// level-heap blocks that earlier loads left behind.
pub fn mem_report(tag: &str, check: bool) {
    unsafe { ffi::n64_mem_report(tag.as_ptr(), tag.len() as u32, check as u32) }
}

/// A cheap `mem` line (counters only, no block walk): for the periodic perf log.
pub fn mem_report_quick(tag: &str) {
    unsafe { ffi::n64_mem_report(tag.as_ptr(), tag.len() as u32, 4) }
}

/// [`mem_report`], then the stack guard is repainted: later reports show the stack's depth
/// from here on.
pub fn mem_report_stack_restart(tag: &str) {
    unsafe { ffi::n64_mem_report(tag.as_ptr(), tag.len() as u32, 2) }
}

pub fn mem_reset_peaks() {
    unsafe { ffi::n64_mem_reset_peaks() }
}

/// Runs `f` on a temporary stack of `size` bytes allocated from the heap (in the current
/// [`mem_phase`]), for work whose frames do not fit libdragon's 64 KiB stack: a new
/// `Game` session mid-run (`Game::new` alone has a ~52 KiB frame; at boot a painted guard
/// below the stack covers it, csrc/main.c "Stack guard"). Returns `f`'s result and the stack
/// bytes it used (painted, measured afterwards).
pub fn on_big_stack<R, F: FnOnce() -> R>(size: usize, f: F) -> (R, usize) {
    const PAINT: u64 = 0x5741_434b_5741_434b;
    let mut stack: alloc::vec::Vec<u64> = alloc::vec![PAINT; size / 8];
    let mut st: (Option<F>, Option<R>) = (Some(f), None);
    extern "C" fn tramp<R, F: FnOnce() -> R>(p: *mut u8) {
        // SAFETY: `p` is `st` below, alive for the whole call.
        let st = unsafe { &mut *(p as *mut (Option<F>, Option<R>)) };
        st.1 = st.0.take().map(|f| f());
    }
    // SAFETY: the block outlives the call; its end is 16-byte aligned (n32), the trampoline
    // restores the stack pointer.
    unsafe {
        let top = (stack.as_mut_ptr().add(stack.len()) as usize & !15) as *mut u8;
        ffi::n64_call_on_stack(tramp::<R, F>, &mut st as *mut _ as *mut u8, top);
    }
    let used = size - 8 * stack.iter().take_while(|&&w| w == PAINT).count();
    (st.1.expect("on_big_stack: f did not run"), used)
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
/// `TICKS_READ()`: the COP0 count register (half the CPU clock), read inline (no call
/// into C: the r3d queue and the CPU batches read it per draw).
#[inline(always)]
pub fn ticks() -> u32 {
    let t: u32;
    // SAFETY: `mfc0 $9` (Count) only reads a register.
    unsafe { core::arch::asm!("mfc0 {0}, $9", out(reg) t, options(nomem, nostack, preserves_flags)) };
    t
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
/// In-place texel update written by the caller: `fill` gets the texture's tightly packed
/// texel bytes (`d.pixels` unused). `false` if the texture cannot be updated in place.
pub fn tex_update_with(h: u32, d: &TexDesc, fill: impl FnOnce(&mut [u8])) -> bool {
    let p = unsafe { ffi::n64_tex_update_begin(h, d) };
    if p.is_null() {
        return false;
    }
    let bpp = if d.fmt & 0xff == 1 { 4 } else { 2 };
    // SAFETY: `n64_tex_update_begin` returned the texture's buffer of exactly these bytes;
    // nothing else touches it until `n64_tex_update_end`.
    fill(unsafe { core::slice::from_raw_parts_mut(p, (d.width * d.height * bpp) as usize) });
    unsafe { ffi::n64_tex_update_end(h) };
    true
}

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
/// Display flips since the last call by how many vblanks the previous image was shown:
/// `[_, 1, 2, 3, 4, 5+]` (index 0 unused); resets.
pub fn flip_stats() -> [u32; 6] {
    let mut h = [0u32; 6];
    unsafe { ffi::n64_flip_stats(h.as_mut_ptr()) };
    h
}

pub fn rcp_load() -> (u32, u32) {
    let v = unsafe { ffi::n64_rcp_load() };
    (v & 0xFFFF, v >> 16)
}

/// Test boots: every video stops after its first frames (as if skipped at once).
pub fn set_fmv_autoskip(on: bool) {
    unsafe { ffi::n64_fmv_autoskip = on as i32 }
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
    // The C side's `n64_audio_poll_due` test, inline: called per draw in the hot loops.
    unsafe extern "C" {
        static audio_last_poll: u32;
    }
    // SAFETY: a plain u32 the C side updates on this (only) thread.
    let last = unsafe { core::ptr::read_volatile(&raw const audio_last_poll) };
    if ticks().wrapping_sub(last) > TICKS_PER_SEC / 1000 * 40 {
        audio_poll();
    }
}

/// C0 ticks spent in mixer polls since boot (wrapping; the C side's polls included).
#[inline]
pub fn audio_ticks_total() -> u32 {
    unsafe extern "C" {
        static audio_ticks_total: u32;
    }
    // SAFETY: a plain u32 the C side updates on this (only) thread.
    unsafe { core::ptr::read_volatile(&raw const audio_ticks_total) }
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
