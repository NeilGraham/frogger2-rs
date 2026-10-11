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
/// Triangles: `s`, `t` are normalised 0..1 (the host scales them to the TMEM-sized surface).
pub const UVNORM: u32 = 1 << 5;
/// `TexDesc::fmt` flag on a full image that does not fit TMEM: triangles sample it in TMEM
/// tiles (`N64_TEX_TILE3D`).
pub const TEX_TILE3D: u32 = 0x100;

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

/// A screen-aligned rectangle (`n64_rect_t`): pixels, normalised UVs, 0xRRGGBBAA tint.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub s0: f32,
    pub t0: f32,
    pub s1: f32,
    pub t1: f32,
    pub rgba: u32,
}

/// A sound-effect voice (`n64_voice_t`): linear left/right volume, frequency (Hz, 0 = keep).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Voice {
    pub lvol: f32,
    pub rvol: f32,
    pub freq: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeapStats {
    pub total: u32,
    pub used: u32,
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
    pub const C_UP: u32 = 1 << 3;
    pub const C_DOWN: u32 = 1 << 2;
    pub const C_LEFT: u32 = 1 << 1;
    pub const C_RIGHT: u32 = 1 << 0;
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
    pub fn n64_tex_create2(full: *const TexDesc, small: *const TexDesc) -> u32;
    pub fn n64_tex_free(handle: u32);
    pub fn n64_draw_rects(rects: *const Rect, n: u32, state: *const State);
    pub fn n64_heap_stats(out: *mut HeapStats);
    pub fn n64_sfx_open(path: *const u8, looped: u32) -> u32;
    pub fn n64_sfx_close(handle: u32);
    pub fn n64_sfx_channels() -> u32;
    pub fn n64_sfx_play(handle: u32, ch: u32, v: *const Voice);
    pub fn n64_ch_set(ch: u32, v: *const Voice);
    pub fn n64_ch_stop(ch: u32);
    pub fn n64_ch_playing(ch: u32) -> u32;
    pub fn n64_music_play(path: *const u8, looped: u32) -> u32;
    pub fn n64_music_vol(vol_permille: u32);
    pub fn n64_audio_poll();
    pub fn n64_prof(cmd: u32);
}
