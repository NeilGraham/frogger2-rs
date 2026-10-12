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
/// r3d: blender fog (shade alpha = fog factor).
pub const FOG: u32 = 1 << 6;
/// r3d: the RDP's real (wrapping) additive blender instead of the approximation.
pub const TRUE_ADDITIVE: u32 = 1 << 7;

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

/// `n64_ntx_desc_t`: an NTX1 image as a texture (texels/TLUT stay in the caller's buffer).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NtxDesc {
    pub fmt: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub texels: *const u8,
    pub tlut: *const u8,
    pub tlut_count: u32,
    pub flags: u32,
    pub tile_w: u32,
    pub tile_h: u32,
}
pub const NTX_SINGLE: u32 = 1 << 0;
pub const NTX_PER_TILE: u32 = 1 << 1;

// ---- r3d (csrc/r3d.c): RSP 3D renderer ----

#[repr(C)]
pub struct R3dTexDesc {
    pub fmt: u32,
    pub width: u32,
    pub height: u32,
    pub tlut_count: u32,
    pub texels: *const u8,
    pub tlut: *const u8,
}

#[repr(C)]
pub struct R3dGroupDesc {
    pub verts: *const u8,
    pub cmds: *const u32,
    pub n_verts: u32,
    pub n_cmds: u32,
    pub skinned: u32,
}

/// `F2_DRAW` uniform (96 bytes): the model matrix (`r3d::model_scaled`: rows x, y, z,
/// translation; lanes x, y, z, unused) in s15.16, integer then fraction halves (the RSP folds
/// it with the frame's view x projection); colour multiplier and addend; fog.
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct R3dDrawUniform {
    pub mtx_i: [i16; 16],
    pub mtx_f: [u16; 16],
    pub cmul: [i16; 4],
    pub cadd: [i16; 4],
    /// `[0, constant unfogged fraction (0..0x7FFF), 0, 0]`.
    pub fogc: [i16; 4],
    /// VCC masks `[keep colour alpha (0xFF; 0x77 = constant fog), per-vertex fog (0x88), 0, 0]`.
    pub fogm: [u16; 4],
}

/// `F2_FRAME` uniform (first 120 bytes) + host fields.
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct R3dFrame {
    pub vo: [i16; 4],
    pub vs: [i16; 4],
    pub vlo: [i16; 4],
    pub vhi: [i16; 4],
    pub wone: [i16; 4],
    /// `[0, 0, -32767 / (end - start), end - 10]` (fog in W units).
    pub fogf: [i16; 4],
    /// View x projection (row-vector, lanes X, Y, Z, W) in s15.16: the RSP folds each
    /// draw's model matrix with it.
    pub vp_i: [i16; 16],
    pub vp_f: [u16; 16],
    pub clip_text: u32,
    pub clip_size: u32,
    pub guard: u32,
    pub fog_rgba: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct R3dDraw {
    /// The draw's uniform in RDRAM (loaded by RSP DMA; must stay valid until the RSP ran it).
    pub u: *const R3dDrawUniform,
    pub groups: *const u16,
    pub texs: *const u16,
    pub n_groups: u32,
    pub bones: *const R3dDrawUniform,
    /// Number of bone uniforms at `bones` (loaded by RSP DMA from the frame's arena).
    pub n_bones: u32,
    /// Frame uniform to draw with (`noClip`: the sky's projection), null = the frame's.
    pub frame: *const R3dFrame,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeapStats {
    pub total: u32,
    pub used: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct R3dList {
    pub blend: u32,
    pub flags: u32,
    pub alpha_ref: u32,
    pub cull: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct R3dStats {
    pub draws: u32,
    pub groups: u32,
    pub tex_loads: u32,
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
    pub fn n64_set_fps_limit(fps: u32);
    pub fn n64_beep(freq_hz: u32, ms: u32);
    pub fn n64_audio_stats() -> u32;
    pub fn n64_memory_size() -> u32;
    pub fn n64_probe_c(kind: u32, n: u32) -> u32;
    pub fn n64_tex_create2(full: *const TexDesc, small: *const TexDesc) -> u32;
    pub fn n64_tex_free(handle: u32);
    pub fn n64_draw_rects(rects: *const Rect, n: u32, state: *const State);
    /// Writes `len` bytes at `p` back from the data cache (data the RSP will DMA).
    pub fn n64_cache_writeback(p: *const u8, len: u32);
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
    pub fn n64_r3d_init();
    pub fn n64_r3d_tex_create(desc: *const R3dTexDesc) -> u32;
    pub fn n64_r3d_group_create(desc: *const R3dGroupDesc) -> u32;
    pub fn n64_r3d_begin(frame: *const R3dFrame);
    pub fn n64_r3d_draw_list(state: *const R3dList, draws: *const R3dDraw, n: u32);
    pub fn n64_r3d_stats(out: *mut R3dStats);
    pub fn n64_tex_update(handle: u32, desc: *const TexDesc) -> u32;
    pub fn n64_r3d_tex_alias(tex: u32) -> u32;
    pub fn n64_r3d_reset(tex_first: u32, group_first: u32);
    pub fn n64_r3d_counts() -> u32;
    pub fn n64_tex_create_ntx(desc: *const NtxDesc) -> u32;
    pub fn n64_sfx_freq(handle: u32) -> u32;
    pub fn n64_fmv_play(path: *const u8) -> u32;
    pub fn n64_rcp_load() -> u32;
    pub fn n64_save_read(dst: *mut u8, len: u32) -> u32;
    pub fn n64_save_write(src: *const u8, len: u32) -> u32;
}
