//! The narrow C interface of the libdragon host (`csrc/n64_ffi.h`, keep in sync).
//!
//! ABI rule (Rust is n32, C is o64; see tools/n64/README.md "ABI"): at most four
//! arguments per function, each a 32/64-bit integer or a pointer; no float arguments.

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vertex {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
    pub s: f32,
    pub t: f32,
    pub inv_w: f32,
}

pub const ZTEST: u32 = 1 << 0;
pub const ZWRITE: u32 = 1 << 1;
pub const BILINEAR: u32 = 1 << 2;
pub const WRAP: u32 = 1 << 3;
pub const PERSP: u32 = 1 << 4;

pub const BLEND_NONE: u32 = 0;
pub const BLEND_SEMI: u32 = 1;
pub const BLEND_ADD: u32 = 2;
pub const BLEND_SUB: u32 = 3;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub flags: u32,
    pub blend: u32,
    pub tex: u32,
    pub alpha_ref: u32,
}

#[repr(C)]
pub struct TexDesc {
    pub width: u32,
    pub height: u32,
    pub fmt: u32,
    pub pixels: *const u8,
}

/// Joypad buttons (`joypad_buttons_t.raw`, big-endian bitfield: A is bit 15).
pub mod pad {
    pub const A: u32 = 1 << 15;
    pub const B: u32 = 1 << 14;
    pub const Z: u32 = 1 << 13;
    pub const START: u32 = 1 << 12;
    pub const UP: u32 = 1 << 11;
    pub const DOWN: u32 = 1 << 10;
    pub const LEFT: u32 = 1 << 9;
    pub const RIGHT: u32 = 1 << 8;
    pub const L: u32 = 1 << 5;
    pub const R: u32 = 1 << 4;
}

#[cfg(target_vendor = "nintendo64")]
extern "C" {
    pub fn n64_log(msg: *const u8, len: u32);
    pub fn n64_panic(msg: *const u8, len: u32) -> !;
    pub fn n64_alloc(size: u32, align: u32) -> *mut u8;
    pub fn n64_free(ptr: *mut u8);
    pub fn n64_ticks() -> u32;
    pub fn n64_pad(port: u32) -> u32;
    pub fn n64_file_read(path: *const u8, len: *mut u32) -> *mut u8;
    pub fn n64_tex_create(desc: *const TexDesc) -> u32;
    pub fn n64_begin_frame(clear_rgba: u32);
    pub fn n64_draw_tris(verts: *const Vertex, ntris: u32, state: *const State);
    pub fn n64_present();
    pub fn n64_rdp_wait();
    pub fn n64_beep(freq_hz: u32, ms: u32);
    pub fn n64_audio_stats() -> u32;
    pub fn n64_memory_size() -> u32;
    pub fn n64_probe_c(kind: u32, n: u32) -> u32;
}
