//! Host-side GLES2 context for checking the GLES2 back end without a console: Mesa's
//! EGL (`libEGL.so.1`, surfaceless platform, llvmpipe) with a pbuffer as the default
//! framebuffer, GL entry points through `eglGetProcAddress`. Linux only; loaded with
//! `dlopen` so nothing links against EGL at build time.

#![allow(dead_code)]

use core::ffi::{c_char, c_int, c_void, CStr};
use f2_engine::gles::GlFns;

extern "C" {
    fn dlopen(file: *const c_char, mode: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}

type EglInt = i32;
type Display = *mut c_void;

const EGL_NONE: EglInt = 0x3038;
const EGL_PLATFORM_SURFACELESS_MESA: u32 = 0x31DD;
const EGL_SURFACE_TYPE: EglInt = 0x3033;
const EGL_PBUFFER_BIT: EglInt = 0x0001;
const EGL_RENDERABLE_TYPE: EglInt = 0x3040;
const EGL_OPENGL_ES2_BIT: EglInt = 0x0004;
const EGL_RED_SIZE: EglInt = 0x3024;
const EGL_GREEN_SIZE: EglInt = 0x3023;
const EGL_BLUE_SIZE: EglInt = 0x3022;
const EGL_ALPHA_SIZE: EglInt = 0x3021;
const EGL_DEPTH_SIZE: EglInt = 0x3025;
const EGL_WIDTH: EglInt = 0x3057;
const EGL_HEIGHT: EglInt = 0x3056;
const EGL_CONTEXT_CLIENT_VERSION: EglInt = 0x3098;
const EGL_OPENGL_ES_API: u32 = 0x30A0;

pub struct HostGl {
    pub width: u32,
    pub height: u32,
    get_proc: unsafe extern "C" fn(*const c_char) -> *const c_void,
}

impl HostGl {
    /// A current GLES2 context with a `w`×`h` RGBA8 + depth 24 pbuffer.
    pub fn new(w: u32, h: u32) -> Result<HostGl, String> {
        unsafe {
            let lib = dlopen(c"libEGL.so.1".as_ptr(), 2);
            if lib.is_null() {
                return Err("libEGL.so.1 not found".into());
            }
            macro_rules! sym {
                ($n:literal: $t:ty) => {{
                    let p = dlsym(lib, concat!($n, "\0").as_ptr() as *const c_char);
                    if p.is_null() {
                        return Err(format!("{} missing", $n));
                    }
                    core::mem::transmute::<*mut c_void, $t>(p)
                }};
            }
            let get_proc = sym!("eglGetProcAddress": unsafe extern "C" fn(*const c_char) -> *const c_void);
            let get_platform_display = sym!("eglGetPlatformDisplay": unsafe extern "C" fn(u32, *mut c_void, *const isize) -> Display);
            let initialize = sym!("eglInitialize": unsafe extern "C" fn(Display, *mut EglInt, *mut EglInt) -> u32);
            let bind_api = sym!("eglBindAPI": unsafe extern "C" fn(u32) -> u32);
            let choose = sym!("eglChooseConfig": unsafe extern "C" fn(Display, *const EglInt, *mut *mut c_void, EglInt, *mut EglInt) -> u32);
            let pbuffer = sym!("eglCreatePbufferSurface": unsafe extern "C" fn(Display, *mut c_void, *const EglInt) -> *mut c_void);
            let context = sym!("eglCreateContext": unsafe extern "C" fn(Display, *mut c_void, *mut c_void, *const EglInt) -> *mut c_void);
            let make_current = sym!("eglMakeCurrent": unsafe extern "C" fn(Display, *mut c_void, *mut c_void, *mut c_void) -> u32);
            let get_error = sym!("eglGetError": unsafe extern "C" fn() -> EglInt);

            let dpy = get_platform_display(EGL_PLATFORM_SURFACELESS_MESA, core::ptr::null_mut(), core::ptr::null());
            if dpy.is_null() {
                return Err("no surfaceless EGL display".into());
            }
            let (mut maj, mut min) = (0, 0);
            if initialize(dpy, &mut maj, &mut min) == 0 {
                return Err(format!("eglInitialize: {:#x}", get_error()));
            }
            bind_api(EGL_OPENGL_ES_API);
            let attrs = [
                EGL_SURFACE_TYPE,
                EGL_PBUFFER_BIT,
                EGL_RENDERABLE_TYPE,
                EGL_OPENGL_ES2_BIT,
                EGL_RED_SIZE,
                8,
                EGL_GREEN_SIZE,
                8,
                EGL_BLUE_SIZE,
                8,
                EGL_ALPHA_SIZE,
                8,
                EGL_DEPTH_SIZE,
                24,
                EGL_NONE,
            ];
            let mut cfg = core::ptr::null_mut();
            let mut n = 0;
            if choose(dpy, attrs.as_ptr(), &mut cfg, 1, &mut n) == 0 || n < 1 {
                return Err(format!("eglChooseConfig: none ({:#x})", get_error()));
            }
            let sattrs = [EGL_WIDTH, w as EglInt, EGL_HEIGHT, h as EglInt, EGL_NONE];
            let surf = pbuffer(dpy, cfg, sattrs.as_ptr());
            if surf.is_null() {
                return Err(format!("eglCreatePbufferSurface: {:#x}", get_error()));
            }
            let cattrs = [EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE];
            let ctx = context(dpy, cfg, core::ptr::null_mut(), cattrs.as_ptr());
            if ctx.is_null() {
                return Err(format!("eglCreateContext: {:#x}", get_error()));
            }
            if make_current(dpy, surf, surf, ctx) == 0 {
                return Err(format!("eglMakeCurrent: {:#x}", get_error()));
            }
            GlFns::load(|name: &CStr| get_proc(name.as_ptr()))?;
            Ok(HostGl { width: w, height: h, get_proc })
        }
    }

    /// The GL function table of the context.
    pub fn fns(&self) -> GlFns {
        unsafe { GlFns::load(|name: &CStr| (self.get_proc)(name.as_ptr())).unwrap() }
    }
}

/// Per-channel difference statistics between two RGBA8 images of the same size:
/// (mean absolute difference over RGB, share of pixels with any RGB channel differing
/// by more than `tol`, maximum channel difference).
pub fn diff(a: &[u8], b: &[u8], tol: u8) -> (f64, f64, u8) {
    assert_eq!(a.len(), b.len());
    let (mut sum, mut bad, mut max) = (0u64, 0u64, 0u8);
    let n = a.len() / 4;
    for (pa, pb) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0) {
        let mut over = false;
        for c in 0..3 {
            let d = pa[c].abs_diff(pb[c]);
            sum += d as u64;
            max = max.max(d);
            over |= d > tol;
        }
        bad += over as u64;
    }
    (sum as f64 / (n * 3) as f64, bad as f64 / n as f64, max)
}
