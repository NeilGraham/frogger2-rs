//! DirectDraw 7 hooks: present counting and framebuffer dumps.
//!
//! The game (mdxDDraw.cpp) creates `IDirectDraw7` with `DirectDrawCreateEx`
//! and, windowed, a primary surface plus a 640x480 off-screen render surface
//! (`surface[RENDER_SRF]`, in the desktop's pixel format). Every frame it
//! presents with `primary->Blt(&windowRect, render, &clientRect, ...)`
//! (`DDrawFlip`); FMVs (video.cpp, windowed) blit a temporary Bink surface
//! to the primary the same way. As TPW's notes advise (shims may wrap
//! ddraw), the hooks go on the vtable of **the game's own** objects: the
//! `IDirectDraw7` it gets back from `DirectDrawCreateEx` and the primary
//! surface it gets back from `CreateSurface`.
//!
//! A blit to the primary with a source surface is a *present*. When a dump
//! is due, the source is locked read-only, converted (any RGB mask format:
//! 565, 555, 24/32-bit; `fbconv`) and written as `f2-shot-<name>.png` in the
//! run directory before the blit goes on. The window is off-screen, so the
//! blit itself is clipped away; the source surface is the frame.

use super::cfg;
use super::iat::{self, orig};
use super::log::flog;
use crate::cfg::{want_dump, WindowMode};
use crate::fbconv::{self, Format};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Mutex;

static O_CREATE_SURFACE: AtomicUsize = AtomicUsize::new(0);
static O_SET_COOP: AtomicUsize = AtomicUsize::new(0);
static O_SET_MODE: AtomicUsize = AtomicUsize::new(0);
static O_BLT: AtomicUsize = AtomicUsize::new(0);
static O_FLIP: AtomicUsize = AtomicUsize::new(0);

static PRIMARY: AtomicUsize = AtomicUsize::new(0);
static FRAMES: AtomicU32 = AtomicU32::new(0);
static DUMPS: AtomicU32 = AtomicU32::new(0);
static LAST_DUMP_MS: AtomicU32 = AtomicU32::new(0);
static FINAL: AtomicBool = AtomicBool::new(false);
static DUMP_LOCK: Mutex<()> = Mutex::new(());

const DDSCAPS_PRIMARYSURFACE: u32 = 0x200;
const DDSCL_FULLSCREEN: u32 = 0x1;
const DDSCL_NORMAL: u32 = 0x8;
const DDSCL_EXCLUSIVE: u32 = 0x10;
const DDLOCK_WAIT: u32 = 0x1;
const DDLOCK_READONLY: u32 = 0x10;

/// exit.seconds: dump the next present as `final`, then terminate.
pub fn request_final() {
    FINAL.store(true, Ordering::Release);
}

fn headless() -> bool {
    cfg().window != WindowMode::Off
}

/// Hook a fresh `IDirectDraw7` (vtable: 6 CreateSurface, 20
/// SetCooperativeLevel, 21 SetDisplayMode).
pub fn on_ddraw7(dd: usize) {
    // SAFETY: a live IDirectDraw7 just returned by DirectDrawCreateEx.
    unsafe {
        iat::patch_vtbl(dd, 6, h_create_surface as *const () as usize, &O_CREATE_SURFACE);
        iat::patch_vtbl(dd, 20, h_set_coop as *const () as usize, &O_SET_COOP);
        iat::patch_vtbl(dd, 21, h_set_mode as *const () as usize, &O_SET_MODE);
    }
}

type CreateSurfaceFn = unsafe extern "system" fn(usize, *mut u8, *mut usize, usize) -> i32;
unsafe extern "system" fn h_create_surface(this: usize, ddsd: *mut u8, out: *mut usize, outer: usize) -> i32 {
    static N: AtomicU32 = AtomicU32::new(0);
    let hr = orig!(O_CREATE_SURFACE, CreateSurfaceFn)(this, ddsd, out, outer);
    if ddsd.is_null() {
        return hr;
    }
    let rd = |o: usize| (ddsd.add(o) as *const u32).read_unaligned();
    let caps = rd(104);
    let s = if hr >= 0 && !out.is_null() { *out } else { 0 };
    if N.fetch_add(1, Ordering::Relaxed) < 48 {
        flog!("dd: CreateSurface(flags {:#x} caps {:#x} {}x{}) = {:#010x} -> {:#x}", rd(4), caps, rd(12), rd(8), hr, s);
    }
    if s != 0 && caps & DDSCAPS_PRIMARYSURFACE != 0 {
        PRIMARY.store(s, Ordering::Release);
        // IDirectDrawSurface7: 5 Blt, 11 Flip.
        let a = iat::patch_vtbl(s, 5, h_blt as *const () as usize, &O_BLT);
        let b = iat::patch_vtbl(s, 11, h_flip as *const () as usize, &O_FLIP);
        flog!("dd: primary surface {:#x}; Blt/Flip hooks {} {}", s, a, b);
    }
    hr
}

type SetCoopFn = unsafe extern "system" fn(usize, usize, u32) -> i32;
unsafe extern "system" fn h_set_coop(this: usize, hwnd: usize, flags: u32) -> i32 {
    let mut f = flags;
    if headless() && flags & (DDSCL_FULLSCREEN | DDSCL_EXCLUSIVE) != 0 {
        f = DDSCL_NORMAL;
        flog!("dd: SetCooperativeLevel({:#x}) forced to DDSCL_NORMAL (headless never goes fullscreen)", flags);
    }
    let hr = orig!(O_SET_COOP, SetCoopFn)(this, hwnd, f);
    flog!("dd: SetCooperativeLevel(hwnd {:#x}, {:#x}) = {:#010x}", hwnd, f, hr);
    hr
}

type SetModeFn = unsafe extern "system" fn(usize, u32, u32, u32, u32, u32) -> i32;
unsafe extern "system" fn h_set_mode(this: usize, w: u32, h: u32, bpp: u32, hz: u32, fl: u32) -> i32 {
    if headless() {
        flog!("dd: SetDisplayMode({}x{}x{}) refused (headless: the display mode is never changed)", w, h, bpp);
        return 0x8876_0091u32 as i32; // DDERR_NOEXCLUSIVEMODE
    }
    orig!(O_SET_MODE, SetModeFn)(this, w, h, bpp, hz, fl)
}

type BltFn = unsafe extern "system" fn(usize, *const i32, usize, *const i32, u32, usize) -> i32;
unsafe extern "system" fn h_blt(this: usize, dst: *const i32, src: usize, srect: *const i32, fl: u32, fx: usize) -> i32 {
    if src != 0 && this == PRIMARY.load(Ordering::Acquire) {
        present(src, srect);
    }
    orig!(O_BLT, BltFn)(this, dst, src, srect, fl, fx)
}

type FlipFn = unsafe extern "system" fn(usize, usize, u32) -> i32;
unsafe extern "system" fn h_flip(this: usize, target: usize, fl: u32) -> i32 {
    static N: AtomicU32 = AtomicU32::new(0);
    if this == PRIMARY.load(Ordering::Acquire) && N.fetch_add(1, Ordering::Relaxed) == 0 {
        flog!("dd: Flip on the primary (fullscreen path): not dumped");
    }
    orig!(O_FLIP, FlipFn)(this, target, fl)
}

unsafe fn present(src: usize, srect: *const i32) {
    let c = cfg();
    let frame = FRAMES.fetch_add(1, Ordering::AcqRel) + 1;
    if c.log_presents > 0 && frame.is_multiple_of(c.log_presents) {
        flog!("dd: present {} from {:#x}", frame, src);
    }
    let fin = FINAL.load(Ordering::Acquire);
    let exit_now = c.exit_frames > 0 && frame >= c.exit_frames;
    if fin || exit_now {
        dump(src, srect, frame, "final");
        super::terminate(if fin { "exit.seconds" } else { "exit.frames" });
    }
    let since = super::log::ms().wrapping_sub(LAST_DUMP_MS.load(Ordering::Acquire));
    if want_dump(c, frame, DUMPS.load(Ordering::Acquire), since) {
        dump(src, srect, frame, &format!("{:06}", frame));
    }
}

/// Lock `src` read-only, convert `srect` (or the whole surface) and write
/// `f2-shot-<name>.png`.
unsafe fn dump(src: usize, srect: *const i32, frame: u32, name: &str) {
    type LockFn = unsafe extern "system" fn(usize, *const i32, *mut u8, u32, usize) -> i32;
    type UnlockFn = unsafe extern "system" fn(usize, *const i32) -> i32;
    let _g = DUMP_LOCK.lock();
    let mut d = [0u8; 124];
    d[0..4].copy_from_slice(&124u32.to_le_bytes());
    let lock = core::mem::transmute::<usize, LockFn>(iat::vslot(src, 25));
    let unlock = core::mem::transmute::<usize, UnlockFn>(iat::vslot(src, 32));
    let hr = lock(src, core::ptr::null(), d.as_mut_ptr(), DDLOCK_WAIT | DDLOCK_READONLY, 0);
    if hr < 0 {
        flog!("fbdump: Lock({:#x}) = {:#010x}; frame {} not dumped", src, hr, frame);
        return;
    }
    let rd = |o: usize| u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
    let (sh, sw, pitch, bits) = (rd(8) as i32, rd(12) as i32, rd(16) as usize, rd(36) as usize);
    let f = Format { bpp: rd(84), rmask: rd(88), gmask: rd(92), bmask: rd(96) };
    let (mut l, mut t, mut r, mut b) = (0, 0, sw, sh);
    if !srect.is_null() {
        l = (*srect).clamp(0, sw);
        t = (*srect.add(1)).clamp(0, sh);
        r = (*srect.add(2)).clamp(l, sw);
        b = (*srect.add(3)).clamp(t, sh);
    }
    let (w, h) = ((r - l) as usize, (b - t) as usize);
    let rgb = match f.bytes_per_pixel() {
        Some(bpp) if w > 0 && h > 0 && bits != 0 => {
            let start = bits + t as usize * pitch + l as usize * bpp;
            let len = (h - 1) * pitch + w * bpp;
            fbconv::to_rgb(core::slice::from_raw_parts(start as *const u8, len), w, h, pitch, f)
        }
        _ => None,
    };
    unlock(src, core::ptr::null());
    let Some(rgb) = rgb else {
        flog!("fbdump: unsupported surface {}x{} pitch {} format {:?}; frame {} not dumped", sw, sh, pitch, f, frame);
        return;
    };
    if DUMPS.load(Ordering::Acquire) == 0 {
        flog!("fbdump: first source surface {}x{} pitch {} bpp {} masks {:#x}/{:#x}/{:#x}", sw, sh, pitch, f.bpp, f.rmask, f.gmask, f.bmask);
    }
    let n = (w * h) as u64;
    let sum: u64 = rgb.iter().map(|&v| v as u64).sum();
    let lit = rgb.as_chunks::<3>().0.iter().filter(|p| p.iter().any(|&v| v > 16)).count() as u64;
    let Some(png) = fbconv::png_rgb(w as u32, h as u32, &rgb) else { return };
    let dir = super::run_dir();
    let path = dir.join(format!("f2-shot-{}.png", name));
    let part = dir.join(format!("f2-shot-{}.png.part", name));
    let ok = std::fs::write(&part, &png).is_ok() && std::fs::rename(&part, &path).is_ok();
    DUMPS.fetch_add(1, Ordering::AcqRel);
    LAST_DUMP_MS.store(super::log::ms(), Ordering::Release);
    flog!(
        "fbdump: {} {} ({}x{}, frame {}, t {} ms, digest {:08x}, mean {:.1}, non-black {:.1}%)",
        if ok { "wrote" } else { "FAILED to write" },
        path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        w,
        h,
        frame,
        super::log::ms(),
        fbconv::digest(&rgb),
        sum as f64 / (n * 3) as f64,
        lit as f64 * 100.0 / n as f64
    );
}
