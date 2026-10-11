//! GPU rendering through Piglet, the PS4 system's OpenGL ES 2.0 / EGL 1.4 library
//! (`libScePigletv2VSH.sprx`), with `f2-engine`'s GLES2 back end.
//!
//! Nothing here is linked at build time: the module is loaded with
//! `sceKernelLoadStartModule` and every EGL/GL entry point is resolved with
//! `sceKernelDlsym`, so a console (or emulator) without Piglet just fails
//! [`PigletGl::init`] and the host falls back to the CPU renderer.
//!
//! Shaders: retail firmware ships Piglet without its GLSL compiler (the compiler
//! module `libSceShaccVSH.sprx` exists only on development kits), so a program is
//! obtained, in order, from
//! 1. a saved program binary `f2_gles2.prog` (`glProgramBinary`; written by step 3),
//! 2. precompiled shader binaries `f2_vs.sb` + `f2_fs.sb` (`glShaderBinary`),
//! 3. the GLSL source, when `GL_SHADER_COMPILER` is true (a devkit Piglet and
//!    `libSceShaccVSH.sprx` placed in `/data/self/system/common/lib`, the RetroArch /
//!    ps4_gl_test convention); the linked program is then saved as `f2_gles2.prog`
//!    so later runs (and other consoles on the same firmware) no longer need the
//!    compiler,
//!
//! looking in `<save dir>/shaders` and `/app0/shaders`. Without any of them the
//! GPU path is unavailable and the CPU renderer is used.

use super::log;
use core::ffi::{c_char, c_int, c_void, CStr};
use f2_engine::gles::{self, GlFns, GlesBackend};
use std::ffi::CString;
use std::path::{Path, PathBuf};

extern "C" {
    fn sceKernelLoadStartModule(path: *const c_char, argc: usize, argv: *const c_void, flags: u32, opt: *mut c_void, res: *mut c_int) -> c_int;
    fn sceKernelDlsym(handle: c_int, symbol: *const c_char, addr: *mut *mut c_void) -> c_int;
    fn sceKernelGetFsSandboxRandomWord() -> *const c_char;
}

type EglDisplay = *mut c_void;
type EglSurface = *mut c_void;
type EglContext = *mut c_void;
type EglConfig = *mut c_void;
type EglInt = i32;

const EGL_NONE: EglInt = 0x3038;
const EGL_RED_SIZE: EglInt = 0x3024;
const EGL_GREEN_SIZE: EglInt = 0x3023;
const EGL_BLUE_SIZE: EglInt = 0x3022;
const EGL_ALPHA_SIZE: EglInt = 0x3021;
const EGL_DEPTH_SIZE: EglInt = 0x3025;
const EGL_STENCIL_SIZE: EglInt = 0x3026;
const EGL_SAMPLE_BUFFERS: EglInt = 0x3032;
const EGL_SAMPLES: EglInt = 0x3031;
const EGL_RENDERABLE_TYPE: EglInt = 0x3040;
const EGL_OPENGL_ES2_BIT: EglInt = 0x0004;
const EGL_SURFACE_TYPE: EglInt = 0x3033;
const EGL_WINDOW_BIT: EglInt = 0x0004;
const EGL_CONTEXT_CLIENT_VERSION: EglInt = 0x3098;
const EGL_RENDER_BUFFER: EglInt = 0x3086;
const EGL_BACK_BUFFER: EglInt = 0x3084;
const EGL_WIDTH: EglInt = 0x3057;
const EGL_HEIGHT: EglInt = 0x3056;
const EGL_OPENGL_ES_API: u32 = 0x30A0;

/// `OrbisPglWindow` (EGLNativeWindowType on Piglet): `id` in 0..8.
#[repr(C)]
struct PglWindow {
    id: u32,
    width: u32,
    height: u32,
    padding: u32,
}

/// `ScePglConfig` (0x88 bytes; layout from OpenOrbis `Pigletv2VSH.h` / flatz).
#[repr(C, align(8))]
struct PglConfig([u8; 0x88]);

impl PglConfig {
    fn put32(&mut self, off: usize, v: u32) {
        self.0[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn put64(&mut self, off: usize, v: u64) {
        self.0[off..off + 8].copy_from_slice(&v.to_le_bytes());
    }
}

/// Flags: `USE_COMPOSITE_EXT` (0x8) | `USE_FLEXIBLE_MEMORY` (0x80) | 0x60, as the
/// OpenOrbis sample and RetroArch.
const PGL_FLAGS: u32 = 0x8 | 0x80 | 0x60;

struct Egl {
    swap_buffers: unsafe extern "C" fn(EglDisplay, EglSurface) -> u32,
    make_current: unsafe extern "C" fn(EglDisplay, EglSurface, EglSurface, EglContext) -> u32,
    destroy_surface: unsafe extern "C" fn(EglDisplay, EglSurface) -> u32,
    destroy_context: unsafe extern "C" fn(EglDisplay, EglContext) -> u32,
    terminate: unsafe extern "C" fn(EglDisplay) -> u32,
    get_error: unsafe extern "C" fn() -> EglInt,
}

/// A Piglet window surface with a current GLES2 context and the GLES2 back end.
pub struct PigletGl {
    pub backend: GlesBackend,
    pub width: u32,
    pub height: u32,
    egl: Egl,
    display: EglDisplay,
    surface: EglSurface,
    context: EglContext,
}

/// Loads a module, logging the result. Returns the handle.
fn load_module(path: &str) -> Option<c_int> {
    let c = CString::new(path).ok()?;
    let mut res = 0;
    let h = unsafe { sceKernelLoadStartModule(c.as_ptr(), 0, core::ptr::null(), 0, core::ptr::null_mut(), &mut res) };
    log(&format!("piglet: load {path}: {h:#x} (start {res})"));
    (h >= 0).then_some(h)
}

fn dlsym(handle: c_int, name: &CStr) -> *const c_void {
    let mut p = core::ptr::null_mut();
    let r = unsafe { sceKernelDlsym(handle, name.as_ptr(), &mut p) };
    if r != 0 {
        core::ptr::null()
    } else {
        p
    }
}

/// Where the user may put development-kit modules (Piglet with its compiler hooks and
/// the shader compiler), as RetroArch and ps4_gl_test look for them.
const DEVKIT_LIB_DIR: &str = "/data/self/system/common/lib";

/// Loads Piglet (and the shader compiler when supplied). Returns the Piglet handle.
fn load_piglet() -> Result<c_int, String> {
    let devkit = |m: &str| format!("{DEVKIT_LIB_DIR}/{m}");
    let have_shacc = std::fs::metadata(devkit("libSceShaccVSH.sprx")).is_ok();
    let mut candidates = Vec::new();
    if have_shacc && std::fs::metadata(devkit("libScePigletv2VSH.sprx")).is_ok() {
        candidates.push(devkit("libScePigletv2VSH.sprx"));
    }
    let word = unsafe { sceKernelGetFsSandboxRandomWord() };
    if !word.is_null() {
        let w = unsafe { CStr::from_ptr(word) }.to_string_lossy().into_owned();
        if !w.is_empty() {
            candidates.push(format!("/{w}/common/lib/libScePigletv2VSH.sprx"));
        }
    }
    candidates.push("/system/common/lib/libScePigletv2VSH.sprx".into());
    let piglet = candidates.iter().find_map(|p| load_module(p)).ok_or_else(|| format!("Piglet module not loadable (tried {candidates:?})"))?;
    if have_shacc {
        // Piglet looks the compiler module up once it is loaded.
        let _ = load_module(&devkit("libSceShaccVSH.sprx"));
    }
    Ok(piglet)
}

macro_rules! sym {
    ($h:expr, $n:literal: $t:ty) => {{
        let p = dlsym($h, CStr::from_bytes_with_nul(concat!($n, "\0").as_bytes()).unwrap());
        if p.is_null() {
            return Err(format!("Piglet lacks {}", $n));
        }
        unsafe { core::mem::transmute::<*const c_void, $t>(p) }
    }};
}

impl PigletGl {
    /// Loads Piglet, creates a `width`×`height` window surface (the VideoOut output
    /// size) with a GLES2 context, and builds the back end. `shader_dirs`: where shader
    /// binaries are looked for; `cache_dir`: where a compiled program binary is saved.
    pub fn init(width: u32, height: u32, shader_dirs: &[PathBuf], cache_dir: Option<&Path>) -> Result<PigletGl, String> {
        let h = load_piglet()?;
        let set_config = sym!(h, "scePigletSetConfigurationVSH": unsafe extern "C" fn(*const PglConfig) -> bool);
        let get_display = sym!(h, "eglGetDisplay": unsafe extern "C" fn(c_int) -> EglDisplay);
        let initialize = sym!(h, "eglInitialize": unsafe extern "C" fn(EglDisplay, *mut EglInt, *mut EglInt) -> u32);
        let bind_api = sym!(h, "eglBindAPI": unsafe extern "C" fn(u32) -> u32);
        let swap_interval = sym!(h, "eglSwapInterval": unsafe extern "C" fn(EglDisplay, EglInt) -> u32);
        let choose = sym!(h, "eglChooseConfig": unsafe extern "C" fn(EglDisplay, *const EglInt, *mut EglConfig, EglInt, *mut EglInt) -> u32);
        let create_surface = sym!(h, "eglCreateWindowSurface": unsafe extern "C" fn(EglDisplay, EglConfig, *const PglWindow, *const EglInt) -> EglSurface);
        let create_context = sym!(h, "eglCreateContext": unsafe extern "C" fn(EglDisplay, EglConfig, EglContext, *const EglInt) -> EglContext);
        let query_surface = sym!(h, "eglQuerySurface": unsafe extern "C" fn(EglDisplay, EglSurface, EglInt, *mut EglInt) -> u32);
        let egl = Egl {
            swap_buffers: sym!(h, "eglSwapBuffers": unsafe extern "C" fn(EglDisplay, EglSurface) -> u32),
            make_current: sym!(h, "eglMakeCurrent": unsafe extern "C" fn(EglDisplay, EglSurface, EglSurface, EglContext) -> u32),
            destroy_surface: sym!(h, "eglDestroySurface": unsafe extern "C" fn(EglDisplay, EglSurface) -> u32),
            destroy_context: sym!(h, "eglDestroyContext": unsafe extern "C" fn(EglDisplay, EglContext) -> u32),
            terminate: sym!(h, "eglTerminate": unsafe extern "C" fn(EglDisplay) -> u32),
            get_error: sym!(h, "eglGetError": unsafe extern "C" fn() -> EglInt),
        };

        // Video memory: two or three 32-bit colour buffers plus depth at the output size,
        // plus textures and vertex streams.
        let fb = width as u64 * height as u64 * 4;
        let mut cfg = PglConfig([0; 0x88]);
        cfg.put32(0x00, 0x88);
        cfg.put32(0x04, PGL_FLAGS);
        cfg.0[0x08] = 1; // processOrder
        cfg.put64(0x18, 0x1000_0000); // systemSharedMemorySize: 256 MiB
        cfg.put64(0x28, (fb * 5 + 0x0800_0000).min(0x2000_0000)); // videoSharedMemorySize
        cfg.put64(0x30, 0xFFFF_FFFF); // maxMappedFlexibleMemory
        cfg.put32(0x40, width);
        cfg.put32(0x44, height);
        cfg.put32(0x54, 0x10_0000); // drawCommandBufferSize
        cfg.put32(0x58, 0x100_0000); // lcueResourceBufferSize
        cfg.put32(0x5C, 2);
        if !unsafe { set_config(&cfg) } {
            return Err("scePigletSetConfigurationVSH failed".into());
        }
        // Optional: Piglet's own shader cache (as RetroArch configures it).
        if let (Some(dir), p) = (cache_dir, dlsym(h, c"scePigletSetShaderCacheConfiguration")) {
            if !p.is_null() {
                let set_cache: unsafe extern "C" fn(*const u8) -> bool = unsafe { core::mem::transmute(p) };
                let mut c = [0u8; 0x10C];
                c[..4].copy_from_slice(&0x0001_0064u32.to_le_bytes());
                let d = format!("{}/", dir.display());
                if d.len() < 127 {
                    let _ = std::fs::create_dir_all(dir);
                    c[0x0C..0x0C + d.len()].copy_from_slice(d.as_bytes());
                    let ok = unsafe { set_cache(c.as_ptr()) };
                    log(&format!("piglet: shader cache {d}: {ok}"));
                }
            }
        }
        unsafe {
            let display = get_display(0);
            if display.is_null() {
                return Err("eglGetDisplay failed".into());
            }
            let (mut maj, mut min) = (0, 0);
            if initialize(display, &mut maj, &mut min) == 0 {
                return Err(format!("eglInitialize: {:#x}", (egl.get_error)()));
            }
            log(&format!("piglet: EGL {maj}.{min}"));
            bind_api(EGL_OPENGL_ES_API);
            // Depth 24 preferred (the CPU renderer's f32 depth is closest); 32 or 16 if not.
            let mut config = core::ptr::null_mut();
            let mut depth_bits = 0;
            for depth in [24, 32, 16] {
                let attrs = [
                    EGL_RED_SIZE,
                    8,
                    EGL_GREEN_SIZE,
                    8,
                    EGL_BLUE_SIZE,
                    8,
                    EGL_ALPHA_SIZE,
                    8,
                    EGL_DEPTH_SIZE,
                    depth,
                    EGL_STENCIL_SIZE,
                    0,
                    EGL_SAMPLE_BUFFERS,
                    0,
                    EGL_SAMPLES,
                    0,
                    EGL_RENDERABLE_TYPE,
                    EGL_OPENGL_ES2_BIT,
                    EGL_SURFACE_TYPE,
                    EGL_WINDOW_BIT,
                    EGL_NONE,
                ];
                let mut n = 0;
                if choose(display, attrs.as_ptr(), &mut config, 1, &mut n) != 0 && n >= 1 {
                    depth_bits = depth;
                    break;
                }
            }
            if depth_bits == 0 {
                let e = (egl.get_error)();
                (egl.terminate)(display);
                return Err(format!("eglChooseConfig: no RGBA8 + depth config ({e:#x})"));
            }
            log(&format!("piglet: config RGBA8, depth {depth_bits}"));
            let window = PglWindow { id: 0, width, height, padding: 0 };
            let wattrs = [EGL_RENDER_BUFFER, EGL_BACK_BUFFER, EGL_NONE];
            let surface = create_surface(display, config, &window, wattrs.as_ptr());
            if surface.is_null() {
                let e = (egl.get_error)();
                (egl.terminate)(display);
                return Err(format!("eglCreateWindowSurface {width}x{height}: {e:#x}"));
            }
            let cattrs = [EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE];
            let context = create_context(display, config, core::ptr::null_mut(), cattrs.as_ptr());
            let fail = |what: String| {
                if !context.is_null() {
                    (egl.destroy_context)(display, context);
                }
                (egl.destroy_surface)(display, surface);
                (egl.terminate)(display);
                what
            };
            if context.is_null() {
                return Err(fail(format!("eglCreateContext: {:#x}", (egl.get_error)())));
            }
            if (egl.make_current)(display, surface, surface, context) == 0 {
                return Err(fail(format!("eglMakeCurrent: {:#x}", (egl.get_error)())));
            }
            // Vsync: one frame per refresh (the game runs a fixed 60 Hz tick).
            swap_interval(display, 1);
            let (mut sw, mut sh) = (width as EglInt, height as EglInt);
            query_surface(display, surface, EGL_WIDTH, &mut sw);
            query_surface(display, surface, EGL_HEIGHT, &mut sh);
            let loader = |name: &CStr| dlsym(h, name);
            let gl = match GlFns::load(loader) {
                Ok(g) => g,
                Err(e) => {
                    (egl.make_current)(display, core::ptr::null_mut(), core::ptr::null_mut(), core::ptr::null_mut());
                    return Err(fail(e));
                }
            };
            log(&format!(
                "piglet: {} / {} / {}, surface {sw}x{sh}, shader compiler {}, binary formats {:?}, program binary formats {}",
                gl.string(gles::consts::VENDOR),
                gl.string(gles::consts::RENDERER),
                gl.string(gles::consts::VERSION),
                gl.shader_compiler(),
                gl.shader_binary_formats(),
                gl.program_binary_formats()
            ));
            let program = match program(&gl, shader_dirs, cache_dir) {
                Ok(p) => p,
                Err(e) => {
                    (egl.make_current)(display, core::ptr::null_mut(), core::ptr::null_mut(), core::ptr::null_mut());
                    return Err(fail(e));
                }
            };
            let backend = match GlesBackend::new(gl, program) {
                Ok(b) => b,
                Err(e) => {
                    (egl.make_current)(display, core::ptr::null_mut(), core::ptr::null_mut(), core::ptr::null_mut());
                    return Err(fail(e));
                }
            };
            log(&format!("piglet: GLES2 renderer ready: {}", backend.describe()));
            let (width, height) = (sw.max(1) as u32, sh.max(1) as u32);
            Ok(PigletGl { backend, width, height, egl, display, surface, context })
        }
    }

    /// Presents the frame (waits for vsync).
    pub fn swap(&mut self) -> bool {
        let ok = unsafe { (self.egl.swap_buffers)(self.display, self.surface) } != 0;
        if !ok {
            log(&format!("piglet: eglSwapBuffers: {:#x}", unsafe { (self.egl.get_error)() }));
        }
        ok
    }

    /// Fills the screen with one colour (status screens). `rgba`: R in the low byte.
    pub fn fill(&mut self, rgba: u32) {
        let gl = &self.backend.gl;
        let c = |s: u32| ((rgba >> s) & 0xff) as f32 / 255.0;
        unsafe {
            (gl.glBindFramebuffer)(gles::consts::FRAMEBUFFER, 0);
            (gl.glDisable)(gles::consts::SCISSOR_TEST);
            (gl.glViewport)(0, 0, self.width as i32, self.height as i32);
            (gl.glClearColor)(c(0), c(8), c(16), 1.0);
            (gl.glClear)(gles::consts::COLOR_BUFFER_BIT | gles::consts::DEPTH_BUFFER_BIT);
        }
        self.swap();
    }
}

impl Drop for PigletGl {
    fn drop(&mut self) {
        unsafe {
            (self.egl.make_current)(self.display, core::ptr::null_mut(), core::ptr::null_mut(), core::ptr::null_mut());
            (self.egl.destroy_context)(self.display, self.context);
            (self.egl.destroy_surface)(self.display, self.surface);
            (self.egl.terminate)(self.display);
        }
    }
}

/// Program binary saved after a successful source compile.
const PROGRAM_FILE: &str = "f2_gles2.prog";
const VS_FILE: &str = "f2_vs.sb";
const FS_FILE: &str = "f2_fs.sb";

/// Obtains the shader program (see the module docs for the order).
fn program(gl: &GlFns, dirs: &[PathBuf], cache_dir: Option<&Path>) -> Result<u32, String> {
    let mut why = Vec::new();
    for d in dirs {
        let p = d.join(PROGRAM_FILE);
        if let Ok(blob) = std::fs::read(&p) {
            match gles::program_from_binary(gl, &blob) {
                Ok(prog) => {
                    log(&format!("piglet: program binary {}", p.display()));
                    return Ok(prog);
                }
                Err(e) => why.push(format!("{}: {e}", p.display())),
            }
        }
    }
    let format = gl.shader_binary_formats().first().copied().unwrap_or(0);
    for d in dirs {
        if let (Ok(vs), Ok(fs)) = (std::fs::read(d.join(VS_FILE)), std::fs::read(d.join(FS_FILE))) {
            match gles::program_from_shader_binaries(gl, format, &vs, &fs) {
                Ok(prog) => {
                    log(&format!("piglet: shader binaries from {} (format {format:#x})", d.display()));
                    return Ok(prog);
                }
                Err(e) => why.push(format!("{}/{{{VS_FILE},{FS_FILE}}}: {e}", d.display())),
            }
        }
    }
    if gl.shader_compiler() {
        match gles::program_from_source(gl) {
            Ok(prog) => {
                log("piglet: compiled the GLSL shaders");
                if let (Some(dir), Some(bin)) = (cache_dir, gles::program_binary(gl, prog)) {
                    let _ = std::fs::create_dir_all(dir);
                    let p = dir.join(PROGRAM_FILE);
                    match std::fs::write(&p, &bin) {
                        Ok(()) => log(&format!("piglet: saved program binary {} ({} bytes)", p.display(), bin.len())),
                        Err(e) => log(&format!("piglet: saving {}: {e}", p.display())),
                    }
                }
                return Ok(prog);
            }
            Err(e) => why.push(format!("GLSL compile: {e}")),
        }
    } else {
        why.push("no shader compiler (retail Piglet)".into());
    }
    Err(format!("no usable shaders: {}", why.join("; ")))
}
