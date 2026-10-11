//! N64 runtime glue: global allocator over libdragon's heap, panic handler, logging and
//! thin safe wrappers over the C interface.

use crate::ffi::{self, State, TexDesc, Vertex};
use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};
use core::fmt::{self, Write};

struct LibdragonHeap;

unsafe impl GlobalAlloc for LibdragonHeap {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        unsafe { ffi::n64_alloc(l.size() as u32, l.align() as u32) }
    }
    unsafe fn dealloc(&self, p: *mut u8, _l: Layout) {
        unsafe { ffi::n64_free(p) }
    }
}

#[global_allocator]
static HEAP: LibdragonHeap = LibdragonHeap;

/// Fixed-size formatting buffer (usable without the heap, e.g. in the panic handler).
pub struct LineBuf {
    buf: [u8; 512],
    len: usize,
}

impl LineBuf {
    pub const fn new() -> Self {
        LineBuf { buf: [0; 512], len: 0 }
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
    let mut z = Vec::with_capacity(path.len() + 1);
    z.extend_from_slice(path.as_bytes());
    z.push(0);
    let mut len = 0u32;
    let p = unsafe { ffi::n64_file_read(z.as_ptr(), &mut len) };
    if p.is_null() {
        return None;
    }
    // asset_load allocates with malloc; our global allocator frees with free() whatever the
    // layout, so the buffer can be adopted by a Vec.
    Some(unsafe { Vec::from_raw_parts(p, len as usize, len as usize) })
}

pub fn tex_create_rgba16(width: u32, height: u32, pixels: &[u16]) -> u32 {
    let d = TexDesc { width, height, fmt: 0, pixels: pixels.as_ptr() as *const u8 };
    unsafe { ffi::n64_tex_create(&d) }
}

pub fn begin_frame(clear_rgba: u32) {
    unsafe { ffi::n64_begin_frame(clear_rgba) }
}

pub fn draw_tris(verts: &[Vertex], state: &State) {
    unsafe { ffi::n64_draw_tris(verts.as_ptr(), (verts.len() / 3) as u32, state) }
}

pub fn present() {
    unsafe { ffi::n64_present() }
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
