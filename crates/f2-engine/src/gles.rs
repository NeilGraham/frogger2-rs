//! OpenGL ES 2.0 back end: draws the same TL draw list as the wgpu and CPU back ends
//! (`Renderer::build`) with the same render states, through a GLES2 context the host
//! owns (the PS4's Piglet library on the console, Mesa EGL in host tests).
//!
//! Platform-free: GL entry points come from a loader callback ([`GlFns::load`]), so
//! the host decides how they are found (`sceKernelDlsym` on the Piglet module,
//! `eglGetProcAddress` on a desktop). No windowing or context code lives here.
//!
//! Semantics follow `soft.rs` / the wgpu shader: D3D7 TL pixel centres at integer
//! coordinates, depth = `clamp(sz, 0, 1)`, colour/uv/fog affine unless
//! `perspective_correct` (then `w = 1/rhw`), `floor(a*255 + 0.5) <= aref` discards,
//! fog mix, Semi/Additive/Subtractive blends, wrap vs clamp, nearest/bilinear.
//! GLES2 specifics:
//! - no flat interpolation: the alpha reference (constant per triangle) is a varying
//!   rounded to an integer in the fragment shader, so interpolation noise cannot flip
//!   the test;
//! - no sampler objects: wrap/filter are texture parameters, re-set only on change;
//! - without `GL_OES_texture_npot`, a non-power-of-two texture must use
//!   `CLAMP_TO_EDGE` and no mipmaps (else it samples black): wrapped draws of such a
//!   texture then wrap and filter in the shader from NEAREST taps (the CPU renderer's
//!   texel maths);
//! - depth: NDC z = `2*sz - 1` (GL's [-1, 1] range) with the default depth range.

use crate::renderer::{Blend, DepthMode, Renderer};
use crate::scene::TlVertex;
use core::ffi::{c_char, c_void, CStr};

pub type GLenum = u32;
pub type GLint = i32;
pub type GLuint = u32;
pub type GLsizei = i32;
pub type GLboolean = u8;
pub type GLfloat = f32;
pub type GLbitfield = u32;
pub type GLsizeiptr = isize;
pub type GLintptr = isize;

/// GL enum values used here (GLES2 `gl2.h`).
pub mod consts {
    use super::GLenum;
    pub const NO_ERROR: GLenum = 0;
    pub const DEPTH_BUFFER_BIT: u32 = 0x0100;
    pub const COLOR_BUFFER_BIT: u32 = 0x4000;
    pub const TRIANGLES: GLenum = 0x0004;
    pub const ZERO: GLenum = 0;
    pub const ONE: GLenum = 1;
    pub const SRC_COLOR: GLenum = 0x0300;
    pub const ONE_MINUS_SRC_COLOR: GLenum = 0x0301;
    pub const SRC_ALPHA: GLenum = 0x0302;
    pub const ONE_MINUS_SRC_ALPHA: GLenum = 0x0303;
    pub const FUNC_ADD: GLenum = 0x8006;
    pub const LEQUAL: GLenum = 0x0203;
    pub const CULL_FACE: GLenum = 0x0B44;
    pub const DEPTH_TEST: GLenum = 0x0B71;
    pub const DITHER: GLenum = 0x0BD0;
    pub const BLEND: GLenum = 0x0BE2;
    pub const SCISSOR_TEST: GLenum = 0x0C11;
    pub const UNPACK_ALIGNMENT: GLenum = 0x0CF5;
    pub const PACK_ALIGNMENT: GLenum = 0x0D05;
    pub const MAX_TEXTURE_SIZE: GLenum = 0x0D33;
    pub const TEXTURE_2D: GLenum = 0x0DE1;
    pub const UNSIGNED_BYTE: GLenum = 0x1401;
    pub const FLOAT: GLenum = 0x1406;
    pub const RGBA: GLenum = 0x1908;
    pub const VENDOR: GLenum = 0x1F00;
    pub const RENDERER: GLenum = 0x1F01;
    pub const VERSION: GLenum = 0x1F02;
    pub const EXTENSIONS: GLenum = 0x1F03;
    pub const NEAREST: GLenum = 0x2600;
    pub const LINEAR: GLenum = 0x2601;
    pub const TEXTURE_MAG_FILTER: GLenum = 0x2800;
    pub const TEXTURE_MIN_FILTER: GLenum = 0x2801;
    pub const TEXTURE_WRAP_S: GLenum = 0x2802;
    pub const TEXTURE_WRAP_T: GLenum = 0x2803;
    pub const REPEAT: GLenum = 0x2901;
    pub const CLAMP_TO_EDGE: GLenum = 0x812F;
    pub const TEXTURE0: GLenum = 0x84C0;
    pub const ARRAY_BUFFER: GLenum = 0x8892;
    pub const STREAM_DRAW: GLenum = 0x88E0;
    pub const FRAGMENT_SHADER: GLenum = 0x8B30;
    pub const VERTEX_SHADER: GLenum = 0x8B31;
    pub const COMPILE_STATUS: GLenum = 0x8B81;
    pub const LINK_STATUS: GLenum = 0x8B82;
    pub const INFO_LOG_LENGTH: GLenum = 0x8B84;
    pub const SHADER_COMPILER: GLenum = 0x8DFA;
    pub const SHADER_BINARY_FORMATS: GLenum = 0x8DF8;
    pub const NUM_SHADER_BINARY_FORMATS: GLenum = 0x8DF9;
    pub const PROGRAM_BINARY_LENGTH: GLenum = 0x8741;
    pub const NUM_PROGRAM_BINARY_FORMATS: GLenum = 0x87FE;
    pub const FRAMEBUFFER: GLenum = 0x8D40;
}
use consts::*;

macro_rules! gl_fns {
    (
        required { $($name:ident: fn($($arg:ty),*) $(-> $ret:ty)?;)* }
        optional { $($oname:ident [$($alias:literal),+]: fn($($oarg:ty),*) $(-> $oret:ty)?;)* }
    ) => {
        /// GLES2 entry points (C ABI function pointers).
        #[allow(non_snake_case)]
        pub struct GlFns {
            $(pub $name: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
            $(pub $oname: Option<unsafe extern "C" fn($($oarg),*) $(-> $oret)?>,)*
        }

        impl GlFns {
            /// Resolves every entry point through `get` (symbol name -> address, null when
            /// missing). Fails with the first missing required function. Optional ones
            /// (program binaries) are tried under each listed name.
            ///
            /// # Safety
            /// Non-null addresses must be the GLES2 functions of that name, valid while
            /// the context exists.
            pub unsafe fn load(mut get: impl FnMut(&CStr) -> *const c_void) -> Result<GlFns, String> {
                Ok(GlFns {
                    $($name: {
                        let n = concat!(stringify!($name), "\0");
                        let p = get(CStr::from_bytes_with_nul(n.as_bytes()).unwrap());
                        if p.is_null() {
                            return Err(format!("GL function {} not found", stringify!($name)));
                        }
                        core::mem::transmute::<*const c_void, unsafe extern "C" fn($($arg),*) $(-> $ret)?>(p)
                    },)*
                    $($oname: {
                        let mut f = None;
                        for n in [$(concat!($alias, "\0")),+] {
                            let p = get(CStr::from_bytes_with_nul(n.as_bytes()).unwrap());
                            if !p.is_null() {
                                f = Some(core::mem::transmute::<*const c_void, unsafe extern "C" fn($($oarg),*) $(-> $oret)?>(p));
                                break;
                            }
                        }
                        f
                    },)*
                })
            }
        }
    };
}

gl_fns! {
    required {
        glGetError: fn() -> GLenum;
        glGetString: fn(GLenum) -> *const u8;
        glGetIntegerv: fn(GLenum, *mut GLint);
        glGetBooleanv: fn(GLenum, *mut GLboolean);
        glCreateShader: fn(GLenum) -> GLuint;
        glShaderSource: fn(GLuint, GLsizei, *const *const c_char, *const GLint);
        glCompileShader: fn(GLuint);
        glGetShaderiv: fn(GLuint, GLenum, *mut GLint);
        glGetShaderInfoLog: fn(GLuint, GLsizei, *mut GLsizei, *mut c_char);
        glShaderBinary: fn(GLsizei, *const GLuint, GLenum, *const c_void, GLsizei);
        glDeleteShader: fn(GLuint);
        glCreateProgram: fn() -> GLuint;
        glAttachShader: fn(GLuint, GLuint);
        glBindAttribLocation: fn(GLuint, GLuint, *const c_char);
        glLinkProgram: fn(GLuint);
        glGetProgramiv: fn(GLuint, GLenum, *mut GLint);
        glGetProgramInfoLog: fn(GLuint, GLsizei, *mut GLsizei, *mut c_char);
        glDeleteProgram: fn(GLuint);
        glUseProgram: fn(GLuint);
        glGetUniformLocation: fn(GLuint, *const c_char) -> GLint;
        glGetAttribLocation: fn(GLuint, *const c_char) -> GLint;
        glUniform1i: fn(GLint, GLint);
        glUniform4f: fn(GLint, GLfloat, GLfloat, GLfloat, GLfloat);
        glGenTextures: fn(GLsizei, *mut GLuint);
        glDeleteTextures: fn(GLsizei, *const GLuint);
        glBindTexture: fn(GLenum, GLuint);
        glTexImage2D: fn(GLenum, GLint, GLint, GLsizei, GLsizei, GLint, GLenum, GLenum, *const c_void);
        glTexSubImage2D: fn(GLenum, GLint, GLint, GLint, GLsizei, GLsizei, GLenum, GLenum, *const c_void);
        glTexParameteri: fn(GLenum, GLenum, GLint);
        glPixelStorei: fn(GLenum, GLint);
        glActiveTexture: fn(GLenum);
        glGenBuffers: fn(GLsizei, *mut GLuint);
        glDeleteBuffers: fn(GLsizei, *const GLuint);
        glBindBuffer: fn(GLenum, GLuint);
        glBufferData: fn(GLenum, GLsizeiptr, *const c_void, GLenum);
        glVertexAttribPointer: fn(GLuint, GLint, GLenum, GLboolean, GLsizei, *const c_void);
        glEnableVertexAttribArray: fn(GLuint);
        glEnable: fn(GLenum);
        glDisable: fn(GLenum);
        glBlendFuncSeparate: fn(GLenum, GLenum, GLenum, GLenum);
        glBlendEquation: fn(GLenum);
        glDepthFunc: fn(GLenum);
        glDepthMask: fn(GLboolean);
        glColorMask: fn(GLboolean, GLboolean, GLboolean, GLboolean);
        glScissor: fn(GLint, GLint, GLsizei, GLsizei);
        glViewport: fn(GLint, GLint, GLsizei, GLsizei);
        glClearColor: fn(GLfloat, GLfloat, GLfloat, GLfloat);
        glClearDepthf: fn(GLfloat);
        glClear: fn(GLbitfield);
        glDrawArrays: fn(GLenum, GLint, GLsizei);
        glReadPixels: fn(GLint, GLint, GLsizei, GLsizei, GLenum, GLenum, *mut c_void);
        glFinish: fn();
        glBindFramebuffer: fn(GLenum, GLuint);
    }
    optional {
        // OES_get_program_binary (Piglet exports the unsuffixed names).
        glGetProgramBinary ["glGetProgramBinary", "glGetProgramBinaryOES"]: fn(GLuint, GLsizei, *mut GLsizei, *mut GLenum, *mut c_void);
        glProgramBinary ["glProgramBinary", "glProgramBinaryOES"]: fn(GLuint, GLenum, *const c_void, GLint);
    }
}

impl GlFns {
    /// `glGetString`, or "" when null.
    pub fn string(&self, name: GLenum) -> String {
        let p = unsafe { (self.glGetString)(name) };
        if p.is_null() {
            return String::new();
        }
        unsafe { CStr::from_ptr(p as *const c_char) }.to_string_lossy().into_owned()
    }

    fn has_extension(&self, ext: &str) -> bool {
        self.string(EXTENSIONS).split_whitespace().any(|e| e == ext)
    }

    /// Whether the implementation can compile GLSL source (`GL_SHADER_COMPILER`;
    /// false on a retail PS4 without the shader compiler module).
    pub fn shader_compiler(&self) -> bool {
        let mut b: GLboolean = 0;
        unsafe { (self.glGetBooleanv)(SHADER_COMPILER, &mut b) };
        b != 0
    }

    /// The supported `glShaderBinary` formats.
    pub fn shader_binary_formats(&self) -> Vec<GLenum> {
        let mut n = 0;
        unsafe { (self.glGetIntegerv)(NUM_SHADER_BINARY_FORMATS, &mut n) };
        let mut v = vec![0i32; n.clamp(0, 64) as usize];
        if !v.is_empty() {
            unsafe { (self.glGetIntegerv)(SHADER_BINARY_FORMATS, v.as_mut_ptr()) };
        }
        v.into_iter().map(|f| f as GLenum).collect()
    }

    /// Number of supported program binary formats (0 = `glProgramBinary` unusable).
    pub fn program_binary_formats(&self) -> i32 {
        let mut n = 0;
        unsafe { (self.glGetIntegerv)(NUM_PROGRAM_BINARY_FORMATS, &mut n) };
        // A GLES2 context without the extension reports INVALID_ENUM: treat as none.
        while unsafe { (self.glGetError)() } != NO_ERROR {}
        n
    }
}

/// The vertex shader (GLSL ES 1.00).
pub const VERTEX_SHADER_SRC: &str = "\
attribute vec4 a_pos;
attribute vec4 a_color;
attribute vec2 a_uv;
attribute float a_fog;
attribute float a_aref;
uniform vec4 u_size;
varying vec4 v_color;
varying vec2 v_uv;
varying float v_fog;
varying float v_aref;
void main() {
    vec2 ndc = vec2((a_pos.x + 0.5) / u_size.x * 2.0 - 1.0, 1.0 - (a_pos.y + 0.5) / u_size.y * 2.0);
    float w = 1.0;
    if (u_size.z > 0.5 && a_pos.w > 0.0) { w = 1.0 / a_pos.w; }
    gl_Position = vec4(ndc * w, (clamp(a_pos.z, 0.0, 1.0) * 2.0 - 1.0) * w, w);
    v_color = a_color;
    v_uv = a_uv;
    v_fog = a_fog;
    v_aref = a_aref;
}
";

/// The fragment shader (GLSL ES 1.00). `u_wrap.x > 0.5`: wrap in the shader (a
/// non-power-of-two texture without `OES_texture_npot`, set to NEAREST / CLAMP_TO_EDGE),
/// `u_wrap.y > 0.5`: with bilinear filtering done in the shader, `u_wrap.zw` = texture
/// size. Same texel maths as `soft.rs` (`floor(u*w) mod w`, taps at `u*w - 0.5`).
pub const FRAGMENT_SHADER_SRC: &str = "\
#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
uniform sampler2D u_tex;
uniform vec4 u_fog;
uniform vec4 u_wrap;
varying vec4 v_color;
varying vec2 v_uv;
varying float v_fog;
varying float v_aref;
vec4 texel(vec2 t) {
    return texture2D(u_tex, (mod(t, u_wrap.zw) + 0.5) / u_wrap.zw);
}
vec4 sample(vec2 uv) {
    if (u_wrap.x < 0.5) { return texture2D(u_tex, uv); }
    if (u_wrap.y < 0.5) { return texel(floor(uv * u_wrap.zw)); }
    vec2 x = uv * u_wrap.zw - 0.5;
    vec2 b = floor(x);
    vec2 f = x - b;
    vec4 top = mix(texel(b), texel(b + vec2(1.0, 0.0)), f.x);
    vec4 bot = mix(texel(b + vec2(0.0, 1.0)), texel(b + vec2(1.0, 1.0)), f.x);
    return mix(top, bot, f.y);
}
void main() {
    vec4 c = sample(v_uv) * v_color;
    if (floor(c.a * 255.0 + 0.5) <= floor(v_aref + 0.5)) { discard; }
    if (u_fog.a > 0.5) { c.rgb = mix(u_fog.rgb, c.rgb, v_fog); }
    gl_FragColor = c;
}
";

/// Attribute slots (bound before linking source shaders; queried for binaries).
const ATTRS: [&CStr; 5] = [c"a_pos", c"a_color", c"a_uv", c"a_fog", c"a_aref"];

unsafe fn shader_log(gl: &GlFns, s: GLuint) -> String {
    let mut n = 0;
    (gl.glGetShaderiv)(s, INFO_LOG_LENGTH, &mut n);
    let mut buf = vec![0u8; n.clamp(1, 16384) as usize];
    let mut len = 0;
    (gl.glGetShaderInfoLog)(s, buf.len() as GLsizei, &mut len, buf.as_mut_ptr() as *mut c_char);
    String::from_utf8_lossy(&buf[..len.clamp(0, buf.len() as i32) as usize]).into_owned()
}

unsafe fn program_log(gl: &GlFns, p: GLuint) -> String {
    let mut n = 0;
    (gl.glGetProgramiv)(p, INFO_LOG_LENGTH, &mut n);
    let mut buf = vec![0u8; n.clamp(1, 16384) as usize];
    let mut len = 0;
    (gl.glGetProgramInfoLog)(p, buf.len() as GLsizei, &mut len, buf.as_mut_ptr() as *mut c_char);
    String::from_utf8_lossy(&buf[..len.clamp(0, buf.len() as i32) as usize]).into_owned()
}

unsafe fn link(gl: &GlFns, vs: GLuint, fs: GLuint) -> Result<GLuint, String> {
    let p = (gl.glCreateProgram)();
    if p == 0 {
        return Err("glCreateProgram failed".into());
    }
    (gl.glAttachShader)(p, vs);
    (gl.glAttachShader)(p, fs);
    for (i, a) in ATTRS.iter().enumerate() {
        (gl.glBindAttribLocation)(p, i as GLuint, a.as_ptr());
    }
    (gl.glLinkProgram)(p);
    (gl.glDeleteShader)(vs);
    (gl.glDeleteShader)(fs);
    let mut ok = 0;
    (gl.glGetProgramiv)(p, LINK_STATUS, &mut ok);
    if ok == 0 {
        let log = program_log(gl, p);
        (gl.glDeleteProgram)(p);
        return Err(format!("link failed: {log}"));
    }
    Ok(p)
}

/// Compiles and links the shaders from GLSL source (needs a shader compiler).
pub fn program_from_source(gl: &GlFns) -> Result<GLuint, String> {
    unsafe {
        let compile = |ty: GLenum, src: &str| -> Result<GLuint, String> {
            let s = (gl.glCreateShader)(ty);
            if s == 0 {
                return Err("glCreateShader failed".into());
            }
            let ptr = src.as_ptr() as *const c_char;
            let len = src.len() as GLint;
            (gl.glShaderSource)(s, 1, &ptr, &len);
            (gl.glCompileShader)(s);
            let mut ok = 0;
            (gl.glGetShaderiv)(s, COMPILE_STATUS, &mut ok);
            if ok == 0 {
                let log = shader_log(gl, s);
                (gl.glDeleteShader)(s);
                return Err(format!("{} shader: {log}", if ty == VERTEX_SHADER { "vertex" } else { "fragment" }));
            }
            Ok(s)
        };
        let vs = compile(VERTEX_SHADER, VERTEX_SHADER_SRC)?;
        let fs = match compile(FRAGMENT_SHADER, FRAGMENT_SHADER_SRC) {
            Ok(f) => f,
            Err(e) => {
                (gl.glDeleteShader)(vs);
                return Err(e);
            }
        };
        link(gl, vs, fs)
    }
}

/// Links a program from precompiled shader binaries (`glShaderBinary`; on the PS4,
/// format 0 = Piglet's PSSL shader binary).
pub fn program_from_shader_binaries(gl: &GlFns, format: GLenum, vs_bin: &[u8], fs_bin: &[u8]) -> Result<GLuint, String> {
    unsafe {
        while (gl.glGetError)() != NO_ERROR {}
        let vs = (gl.glCreateShader)(VERTEX_SHADER);
        let fs = (gl.glCreateShader)(FRAGMENT_SHADER);
        (gl.glShaderBinary)(1, &vs, format, vs_bin.as_ptr() as *const c_void, vs_bin.len() as GLsizei);
        (gl.glShaderBinary)(1, &fs, format, fs_bin.as_ptr() as *const c_void, fs_bin.len() as GLsizei);
        let e = (gl.glGetError)();
        if e != NO_ERROR {
            (gl.glDeleteShader)(vs);
            (gl.glDeleteShader)(fs);
            return Err(format!("glShaderBinary: error {e:#x}"));
        }
        link(gl, vs, fs)
    }
}

/// A linked program saved with [`program_binary`]: `format` (u32 LE) + the GL blob.
pub fn program_from_binary(gl: &GlFns, blob: &[u8]) -> Result<GLuint, String> {
    let load = gl.glProgramBinary.ok_or("glProgramBinary unavailable")?;
    if blob.len() < 5 {
        return Err("program binary too short".into());
    }
    let format = u32::from_le_bytes(blob[..4].try_into().unwrap());
    unsafe {
        while (gl.glGetError)() != NO_ERROR {}
        let p = (gl.glCreateProgram)();
        load(p, format, blob[4..].as_ptr() as *const c_void, (blob.len() - 4) as GLint);
        let mut ok = 0;
        (gl.glGetProgramiv)(p, LINK_STATUS, &mut ok);
        let e = (gl.glGetError)();
        if ok == 0 || e != NO_ERROR {
            (gl.glDeleteProgram)(p);
            return Err(format!("glProgramBinary rejected the blob (error {e:#x})"));
        }
        Ok(p)
    }
}

/// Serialises a linked program for [`program_from_binary`] (`None` when the
/// implementation cannot).
pub fn program_binary(gl: &GlFns, program: GLuint) -> Option<Vec<u8>> {
    let get = gl.glGetProgramBinary?;
    unsafe {
        let mut len = 0;
        (gl.glGetProgramiv)(program, PROGRAM_BINARY_LENGTH, &mut len);
        if (gl.glGetError)() != NO_ERROR || len <= 0 {
            return None;
        }
        let mut out = vec![0u8; 4 + len as usize];
        let (mut n, mut format) = (0, 0);
        get(program, len, &mut n, &mut format, out[4..].as_mut_ptr() as *mut c_void);
        if (gl.glGetError)() != NO_ERROR || n <= 0 {
            return None;
        }
        out.truncate(4 + n as usize);
        out[..4].copy_from_slice(&format.to_le_bytes());
        Some(out)
    }
}

/// A GL texture mirroring one `TextureRegistry` image.
#[derive(Clone, Copy, Default)]
struct GlTex {
    id: GLuint,
    width: u32,
    height: u32,
    /// Current (wrap, linear) parameters; `None` before the first draw.
    params: Option<(bool, bool)>,
}

/// Cached GL state, to skip redundant calls.
#[derive(Clone, Copy, PartialEq)]
struct GlState {
    blend: Blend,
    depth: DepthMode,
    scissor: bool,
    tex: GLuint,
    /// Shader wrap mode (0 = off, 1 = nearest, 2 = bilinear).
    wrapfix: u8,
}

/// The GLES2 renderer: GL objects plus the function table. Create it with the context
/// current; [`Renderer::render_gles`] draws a frame.
pub struct GlesBackend {
    pub gl: GlFns,
    program: GLuint,
    u_size: GLint,
    u_fog: GLint,
    u_wrap: GLint,
    attrs: [GLuint; 5],
    vbo: GLuint,
    textures: Vec<GlTex>,
    /// `OES_texture_npot` (or a desktop-class implementation): REPEAT works on any size.
    npot: bool,
    /// Force the shader wrap path for every wrapped draw of a non-power-of-two texture
    /// (tests the no-`OES_texture_npot` path on implementations that have it).
    pub force_npot_emulation: bool,
    max_texture: u32,
    /// Framebuffer object to draw into (0 = the default framebuffer).
    pub framebuffer: GLuint,
}

impl GlesBackend {
    /// Sets up the back end around a linked `program` (from [`program_from_source`],
    /// [`program_from_shader_binaries`] or [`program_from_binary`]).
    pub fn new(gl: GlFns, program: GLuint) -> Result<GlesBackend, String> {
        unsafe {
            let u = |n: &CStr| (gl.glGetUniformLocation)(program, n.as_ptr());
            let (u_size, u_fog, u_wrap, u_tex) = (u(c"u_size"), u(c"u_fog"), u(c"u_wrap"), u(c"u_tex"));
            if u_size < 0 || u_fog < 0 || u_wrap < 0 {
                return Err(format!("program lacks uniforms (u_size {u_size}, u_fog {u_fog}, u_wrap {u_wrap})"));
            }
            let mut attrs = [0; 5];
            for (i, a) in ATTRS.iter().enumerate() {
                let l = (gl.glGetAttribLocation)(program, a.as_ptr());
                if l < 0 {
                    return Err(format!("program lacks attribute {}", a.to_string_lossy()));
                }
                attrs[i] = l as GLuint;
            }
            (gl.glUseProgram)(program);
            if u_tex >= 0 {
                (gl.glUniform1i)(u_tex, 0);
            }
            let mut vbo = 0;
            (gl.glGenBuffers)(1, &mut vbo);
            let mut max = 0;
            (gl.glGetIntegerv)(MAX_TEXTURE_SIZE, &mut max);
            let version = gl.string(VERSION);
            let npot = gl.has_extension("GL_OES_texture_npot") || gl.has_extension("GL_ARB_texture_non_power_of_two");
            let e = (gl.glGetError)();
            if e != NO_ERROR {
                return Err(format!("GL setup error {e:#x} ({version})"));
            }
            Ok(GlesBackend {
                gl,
                program,
                u_size,
                u_fog,
                u_wrap,
                attrs,
                vbo,
                textures: Vec::new(),
                npot,
                force_npot_emulation: false,
                max_texture: max.max(64) as u32,
                framebuffer: 0,
            })
        }
    }

    /// One-line description of the GL implementation (for logs).
    pub fn describe(&self) -> String {
        format!(
            "{} / {} / {} (npot {}, max texture {})",
            self.gl.string(VENDOR),
            self.gl.string(RENDERER),
            self.gl.string(VERSION),
            self.npot,
            self.max_texture
        )
    }

    /// Reads back the `w`×`h` framebuffer as RGBA8 rows, top row first.
    pub fn read_pixels(&self, w: u32, h: u32) -> Vec<u8> {
        let gl = &self.gl;
        let mut px = vec![0u8; (w * h * 4) as usize];
        unsafe {
            (gl.glBindFramebuffer)(FRAMEBUFFER, self.framebuffer);
            (gl.glPixelStorei)(PACK_ALIGNMENT, 1);
            (gl.glReadPixels)(0, 0, w as GLsizei, h as GLsizei, RGBA, UNSIGNED_BYTE, px.as_mut_ptr() as *mut c_void);
        }
        let row = (w * 4) as usize;
        let mut out = Vec::with_capacity(px.len());
        for y in (0..h as usize).rev() {
            out.extend_from_slice(&px[y * row..(y + 1) * row]);
        }
        out
    }

    fn upload(&mut self, r: &mut Renderer) {
        let gl = &self.gl;
        for i in std::mem::take(&mut r.textures.dirty) {
            let img = &r.textures.images[i as usize];
            let i = i as usize;
            if self.textures.len() <= i {
                self.textures.resize(i + 1, GlTex::default());
            }
            let valid = img.width > 0 && img.height > 0 && img.rgba.len() as u32 == img.width * img.height * 4;
            let t = &mut self.textures[i];
            unsafe {
                if t.id == 0 {
                    (gl.glGenTextures)(1, &mut t.id);
                }
                (gl.glBindTexture)(TEXTURE_2D, t.id);
                if !valid {
                    t.width = 0;
                    continue;
                }
                let data = img.rgba.as_ptr() as *const c_void;
                if (t.width, t.height) == (img.width, img.height) {
                    // Same size (e.g. the next video frame): replace the pixels.
                    (gl.glTexSubImage2D)(TEXTURE_2D, 0, 0, 0, img.width as GLsizei, img.height as GLsizei, RGBA, UNSIGNED_BYTE, data);
                } else {
                    (gl.glTexImage2D)(TEXTURE_2D, 0, RGBA as GLint, img.width as GLsizei, img.height as GLsizei, 0, RGBA, UNSIGNED_BYTE, data);
                    t.width = img.width;
                    t.height = img.height;
                    t.params = None;
                }
            }
        }
    }

    unsafe fn set_blend(gl: &GlFns, b: Blend) {
        match b {
            Blend::Semi => (gl.glBlendFuncSeparate)(SRC_ALPHA, ONE_MINUS_SRC_ALPHA, ONE, ONE_MINUS_SRC_ALPHA),
            Blend::Additive => (gl.glBlendFuncSeparate)(SRC_ALPHA, ONE, ZERO, ONE),
            Blend::Subtractive => (gl.glBlendFuncSeparate)(ZERO, ONE_MINUS_SRC_COLOR, ZERO, ONE),
        }
    }

    unsafe fn set_depth(gl: &GlFns, d: DepthMode) {
        match d {
            DepthMode::TestWrite => {
                (gl.glEnable)(DEPTH_TEST);
                (gl.glDepthMask)(1);
            }
            DepthMode::Test => {
                (gl.glEnable)(DEPTH_TEST);
                (gl.glDepthMask)(0);
            }
            // A disabled depth test also writes no depth (noZRS).
            DepthMode::Off => (gl.glDisable)(DEPTH_TEST),
        }
    }
}

impl Drop for GlesBackend {
    fn drop(&mut self) {
        let gl = &self.gl;
        unsafe {
            for t in &self.textures {
                if t.id != 0 {
                    (gl.glDeleteTextures)(1, &t.id);
                }
            }
            (gl.glDeleteBuffers)(1, &self.vbo);
            (gl.glDeleteProgram)(self.program);
        }
    }
}

fn pot(v: u32) -> bool {
    v.is_power_of_two()
}

impl Renderer {
    /// Draws the queued frame with the GLES2 back end into its framebuffer (`fb_w`×`fb_h`;
    /// the internal view size from `begin_frame` is centred in it, normally equal).
    /// The caller presents (e.g. `eglSwapBuffers`).
    pub fn render_gles(&mut self, be: &mut GlesBackend, fb_w: u32, fb_h: u32, clear: [f64; 4]) {
        be.upload(self);
        let (w, h) = (self.view.width as u32, self.view.height as u32);
        let (ox, oy) = ((fb_w.saturating_sub(w) / 2) as i32, (fb_h.saturating_sub(h) / 2) as i32);
        let (verts, cmds) = self.build(clear);
        let clear = if self.view.viewport.is_some() { [0.0, 0.0, 0.0, 1.0] } else { clear };
        let fog = self.options.fog;
        let linear = self.options.linear_filter;
        let persp = self.options.perspective_correct;
        let gl = &be.gl;
        // `u_wrap.zw` must never be 0 (mod by zero) even when unused.
        unsafe {
            (gl.glBindFramebuffer)(FRAMEBUFFER, be.framebuffer);
            (gl.glUseProgram)(be.program);
            (gl.glDisable)(SCISSOR_TEST);
            (gl.glDisable)(CULL_FACE);
            (gl.glDisable)(DITHER);
            (gl.glColorMask)(1, 1, 1, 1);
            (gl.glDepthMask)(1);
            (gl.glViewport)(0, 0, fb_w as GLsizei, fb_h as GLsizei);
            if (w, h) != (fb_w, fb_h) {
                (gl.glClearColor)(0.0, 0.0, 0.0, 1.0);
                (gl.glClearDepthf)(1.0);
                (gl.glClear)(COLOR_BUFFER_BIT | DEPTH_BUFFER_BIT);
            }
            // GL's origin is bottom-left.
            let gy = |y: i32, hh: i32| fb_h as i32 - oy - y - hh;
            (gl.glViewport)(ox, gy(0, h as i32), w as GLsizei, h as GLsizei);
            (gl.glEnable)(SCISSOR_TEST);
            (gl.glScissor)(ox, gy(0, h as i32), w as GLsizei, h as GLsizei);
            (gl.glClearColor)(clear[0] as f32, clear[1] as f32, clear[2] as f32, clear[3] as f32);
            (gl.glClearDepthf)(1.0);
            (gl.glClear)(COLOR_BUFFER_BIT | DEPTH_BUFFER_BIT);
            (gl.glDepthFunc)(LEQUAL);
            (gl.glEnable)(BLEND);
            (gl.glBlendEquation)(FUNC_ADD);
            (gl.glActiveTexture)(TEXTURE0);
            (gl.glUniform4f)(be.u_size, w as f32, h as f32, if persp { 1.0 } else { 0.0 }, 0.0);
            (gl.glUniform4f)(be.u_fog, fog.color[0], fog.color[1], fog.color[2], if fog.enabled { 1.0 } else { 0.0 });
            (gl.glUniform4f)(be.u_wrap, 0.0, 0.0, 1.0, 1.0);
            if verts.is_empty() {
                (gl.glDisable)(SCISSOR_TEST);
                return;
            }
            upload_vertices(be, &verts);
        }
        let gl = &be.gl;
        let scissor = self.view.viewport.map(|[x, y, vw, vh]| {
            let (x, y) = ((x as i32).clamp(0, w as i32 - 1), (y as i32).clamp(0, h as i32 - 1));
            let (vw, vh) = ((vw as i32).min(w as i32 - x).max(1), (vh as i32).min(h as i32 - y).max(1));
            [ox + x, fb_h as i32 - oy - y - vh, vw, vh]
        });
        let full = [ox, fb_h as i32 - oy - h as i32, w as i32, h as i32];
        let mut st: Option<GlState> = None;
        for c in &cmds {
            let Some(t) = be.textures.get(c.tex.0 as usize).copied() else { continue };
            if t.id == 0 || t.width == 0 {
                continue;
            }
            let npot_tex = !pot(t.width) || !pot(t.height);
            let emulate = c.wrap && npot_tex && (!be.npot || be.force_npot_emulation);
            let wrapfix = if !emulate {
                0
            } else if linear {
                2
            } else {
                1
            };
            let want = GlState { blend: c.blend, depth: c.depth, scissor: c.scene && scissor.is_some(), tex: t.id, wrapfix };
            unsafe {
                if st.is_none_or(|s| s.blend != want.blend) {
                    GlesBackend::set_blend(gl, want.blend);
                }
                if st.is_none_or(|s| s.depth != want.depth) {
                    GlesBackend::set_depth(gl, want.depth);
                }
                if st.is_none_or(|s| s.scissor != want.scissor) {
                    let [x, y, sw, sh] = if want.scissor { scissor.unwrap() } else { full };
                    (gl.glScissor)(x, y, sw, sh);
                }
                if st.is_none_or(|s| s.tex != want.tex) {
                    (gl.glBindTexture)(TEXTURE_2D, t.id);
                }
                if st.is_none_or(|s| s.wrapfix != want.wrapfix || (wrapfix != 0 && s.tex != want.tex)) {
                    let f = |b: bool| if b { 1.0 } else { 0.0 };
                    (gl.glUniform4f)(be.u_wrap, f(wrapfix != 0), f(wrapfix == 2), t.width as f32, t.height as f32);
                }
                // Texture parameters (GLES2 has no sampler objects). The shader wrap
                // path filters itself from NEAREST taps.
                let gl_wrap = c.wrap && !emulate;
                let params = (gl_wrap, linear && !emulate);
                let tex = &mut be.textures[c.tex.0 as usize];
                if tex.params != Some(params) {
                    let mode = if gl_wrap { REPEAT } else { CLAMP_TO_EDGE } as GLint;
                    let filter = if linear { LINEAR } else { NEAREST } as GLint;
                    (gl.glTexParameteri)(TEXTURE_2D, TEXTURE_WRAP_S, mode);
                    (gl.glTexParameteri)(TEXTURE_2D, TEXTURE_WRAP_T, mode);
                    (gl.glTexParameteri)(TEXTURE_2D, TEXTURE_MIN_FILTER, filter);
                    (gl.glTexParameteri)(TEXTURE_2D, TEXTURE_MAG_FILTER, filter);
                    tex.params = Some(params);
                }
                (gl.glDrawArrays)(TRIANGLES, c.first as GLint, c.count as GLsizei);
            }
            st = Some(want);
        }
        unsafe {
            (gl.glDisable)(SCISSOR_TEST);
            (gl.glDepthMask)(1);
        }
    }
}

/// Uploads the frame's vertices into the stream buffer (re-specified each frame, which
/// lets the driver hand out fresh storage instead of waiting for the previous frame)
/// and points the attributes at it.
unsafe fn upload_vertices(be: &GlesBackend, verts: &[TlVertex]) {
    let gl = &be.gl;
    let bytes = core::mem::size_of_val(verts);
    (gl.glBindBuffer)(ARRAY_BUFFER, be.vbo);
    (gl.glBufferData)(ARRAY_BUFFER, bytes as GLsizeiptr, verts.as_ptr() as *const c_void, STREAM_DRAW);
    let stride = core::mem::size_of::<TlVertex>() as GLsizei;
    // TlVertex: sx sy sz rhw | r g b a | u v | fog | alpha_ref (f32 each).
    let layout: [(GLint, usize); 5] = [(4, 0), (4, 16), (2, 32), (1, 40), (1, 44)];
    for (slot, (n, off)) in layout.into_iter().enumerate() {
        (gl.glEnableVertexAttribArray)(be.attrs[slot]);
        (gl.glVertexAttribPointer)(be.attrs[slot], n, FLOAT, 0, stride, off as *const c_void);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_layout_matches_attributes() {
        assert_eq!(core::mem::size_of::<TlVertex>(), 48);
        let v = TlVertex::default();
        let base = &v as *const TlVertex as usize;
        assert_eq!(&v.color as *const _ as usize - base, 16);
        assert_eq!(&v.uv as *const _ as usize - base, 32);
        assert_eq!(&v.fog as *const _ as usize - base, 40);
        assert_eq!(&v.alpha_ref as *const _ as usize - base, 44);
    }

    #[test]
    fn loader_reports_missing_function() {
        let r = unsafe { GlFns::load(|_| core::ptr::null()) };
        assert!(r.err().unwrap().contains("glGetError"));
    }
}
