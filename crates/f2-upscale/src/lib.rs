//! DLSS, FSR 3.1 and FSR 4 super resolution and frame generation for the wgpu renderer
//! (enhanced; Windows, Direct3D 12).
//!
//! [`f2_engine::Renderer::render_upscaled`] draws the scene at the render size with jitter,
//! motion vectors and depth and calls [`Native`] (an [`f2_engine::Upscaler`]) in the middle; the
//! native side (`native/f2_upscale.cpp`) runs NVIDIA Streamline (DLSS super resolution, DLSS
//! frame generation / multi frame generation, Reflex) or the AMD FidelityFX API (FSR 3.1 / FSR 4
//! upscaling, FSR frame generation) on wgpu's own `ID3D12Device` and queue, and owns the DXGI
//! swapchain, because frame generation has to sit in the present. The libraries are the
//! player's: loaded at run time from a folder (`docs/UPSCALING.md`), never linked or shipped.
//!
//! Elsewhere (Linux, macOS, consoles) [`Native::new`] reports that it needs Windows.

use f2_engine::upscale::{jitter, jitter_phase_count, UpscaleInputs, UpscaleMode, Upscaler};
use std::path::Path;

/// Super resolution back end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum SrBackend {
    /// No upscaler (frame generation only, at the output size).
    #[default]
    Off,
    /// NVIDIA DLSS (RTX GPUs) through Streamline.
    Dlss,
    /// AMD FSR 3.1: every GPU.
    Fsr3,
    /// AMD FSR 4: Radeon RX 7000 / 9000.
    Fsr4,
}

/// Frame generation back end (chosen at start: it owns the swapchain).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum FgBackend {
    #[default]
    Off,
    /// DLSS Frame Generation: 2x on RTX 40, 2x / 3x / 4x / dynamic (multi frame generation) on RTX 50.
    Dlss,
    /// FSR Frame Generation: 2x, every GPU (the ML generator on RX 9000 with FSR 4).
    Fsr,
}

/// Frame generation multiplier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum FgMode {
    #[default]
    Off,
    X2,
    /// DLSS multi frame generation (RTX 50).
    X3,
    X4,
    /// DLSS dynamic multi frame generation to [`Options::fg_target_fps`] (RTX 50).
    Dynamic,
}

/// NVIDIA Reflex low latency.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Reflex {
    #[default]
    Off,
    On,
    Boost,
}

/// Live options (applied with [`Native::set_options`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    pub mode: UpscaleMode,
    /// DLSS preset: 0 = the driver's default, else `sl::DLSSPreset` (10 J, 11 K, 12 L, 13 M).
    pub dlss_preset: u32,
    /// FSR sharpening 0..1 (0 = off).
    pub sharpness: f32,
    pub fg: FgMode,
    /// Dynamic frame generation target (0 = the display's refresh rate).
    pub fg_target_fps: f32,
    pub reflex: Reflex,
    pub vsync: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { mode: UpscaleMode::Quality, dlss_preset: 0, sharpness: 0.0, fg: FgMode::Off, fg_target_fps: 0.0, reflex: Reflex::Off, vsync: true }
    }
}

/// What the native side found and runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub streamline: bool,
    pub dlss: bool,
    pub dlss_fg: bool,
    /// Most frames DLSS frame generation can insert per rendered frame (1 = 2x, 3 = 4x).
    pub dlss_fg_max_frames: u32,
    pub dlss_fg_dynamic: bool,
    pub reflex: bool,
    pub fidelityfx: bool,
    pub fsr3: bool,
    pub fsr4: bool,
    pub fsr_fg: bool,
    /// The super resolution that ran last frame (`Off` when none could).
    pub active: SrBackend,
    pub fg_backend: FgBackend,
    pub fg_active: bool,
    pub sr_name: String,
    pub fg_name: String,
    pub error: String,
}

/// [`Native::new`] arguments.
pub struct Init<'a> {
    /// The window (`HWND`).
    pub hwnd: isize,
    /// Folder with the runtimes (`sl.interposer.dll`, `nvngx_dlss*.dll`, `amd_fidelityfx_*_dx12.dll`).
    pub runtime_dir: &'a Path,
    /// Streamline's log folder.
    pub log_dir: &'a Path,
    pub sr: SrBackend,
    pub fg: FgBackend,
    /// Swapchain size.
    pub size: (u32, u32),
    /// DLSS 5 (RenoDX add-on) helpers, see `native/f2_upscale.h`.
    pub nr_helpers: bool,
}

#[cfg(windows)]
mod ffi {
    use std::ffi::{c_char, c_int, c_void};

    #[repr(C)]
    pub struct Init {
        pub device: *mut c_void,
        pub queue: *mut c_void,
        pub hwnd: *mut c_void,
        pub runtime_dir: *const u16,
        pub log_dir: *const u16,
        pub sr: u32,
        pub fg: u32,
        pub width: u32,
        pub height: u32,
        pub log: Option<unsafe extern "C" fn(c_int, *const c_char)>,
        pub nr_helpers: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, PartialEq)]
    pub struct Options {
        pub sr_mode: u32,
        pub dlss_preset: u32,
        pub sharpness: f32,
        pub fg_mode: u32,
        pub fg_target_fps: f32,
        pub reflex: u32,
        pub vsync: u32,
    }

    #[repr(C)]
    pub struct Frame {
        pub color: *mut c_void,
        pub depth: *mut c_void,
        pub motion: *mut c_void,
        pub output: *mut c_void,
        pub render_w: u32,
        pub render_h: u32,
        pub output_w: u32,
        pub output_h: u32,
        pub jitter_x: f32,
        pub jitter_y: f32,
        pub reset: u32,
        pub frame_time_ms: f32,
        pub view_to_clip: [f32; 16],
        pub clip_to_view: [f32; 16],
        pub clip_to_prev_clip: [f32; 16],
        pub prev_clip_to_clip: [f32; 16],
        pub cam_pos: [f32; 3],
        pub cam_up: [f32; 3],
        pub cam_right: [f32; 3],
        pub cam_fwd: [f32; 3],
        pub near_z: f32,
        pub far_z: f32,
        pub fov_y: f32,
        pub aspect: f32,
        pub frame: u64,
    }

    #[repr(C)]
    pub struct Status {
        pub sl_loaded: u32,
        pub dlss_supported: u32,
        pub dlssg_supported: u32,
        pub dlssg_max_frames: u32,
        pub dlssg_dynamic: u32,
        pub reflex_supported: u32,
        pub ffx_loaded: u32,
        pub fsr3_available: u32,
        pub fsr4_available: u32,
        pub fsr_fg_available: u32,
        pub sr_active: u32,
        pub fg_active: u32,
        pub fg_backend: u32,
        pub presented: u32,
        pub sr_name: [c_char; 64],
        pub fg_name: [c_char; 64],
        pub error: [c_char; 256],
    }

    #[repr(C)]
    pub struct F2up {
        _p: [u8; 0],
    }

    extern "C" {
        pub fn f2up_create(init: *const Init, err: *mut c_char, errlen: u32) -> *mut F2up;
        pub fn f2up_destroy(u: *mut F2up);
        pub fn f2up_set_backend(u: *mut F2up, sr: u32);
        pub fn f2up_set_options(u: *mut F2up, o: *const Options);
        pub fn f2up_render_size(u: *mut F2up, ow: u32, oh: u32, rw: *mut u32, rh: *mut u32);
        pub fn f2up_begin_frame(u: *mut F2up, frame: u64);
        pub fn f2up_evaluate(u: *mut F2up, f: *const Frame) -> c_int;
        pub fn f2up_present(u: *mut F2up, src: *mut c_void) -> c_int;
        pub fn f2up_resize(u: *mut F2up, w: u32, h: u32) -> c_int;
        pub fn f2up_status(u: *mut F2up, s: *mut Status);
    }
}

#[cfg(windows)]
mod dx12 {
    use std::ffi::c_void;
    use wgpu::hal::api::Dx12;
    use windows_core::Interface;

    pub fn device(d: &wgpu::Device) -> Option<*mut c_void> {
        // SAFETY: only the raw pointer leaves the guard; the device outlives its use.
        unsafe { d.as_hal::<Dx12>().map(|h| h.raw_device().as_raw()) }
    }

    pub fn queue(q: &wgpu::Queue) -> Option<*mut c_void> {
        // SAFETY: as above.
        unsafe { q.as_hal::<Dx12>().map(|h| h.as_raw().as_raw()) }
    }

    pub fn texture(t: &wgpu::Texture) -> Option<*mut c_void> {
        // SAFETY: as above; the texture is alive for the whole native call.
        unsafe { t.as_hal::<Dx12>().map(|h| h.raw_resource().as_raw()) }
    }
}

/// The native upscaler and presenter (see the crate docs).
pub struct Native {
    #[cfg(windows)]
    raw: *mut ffi::F2up,
    sr: SrBackend,
    opts: Options,
    last: std::time::Instant,
}

/// The texture the finished frame (HUD included) is drawn into and [`Native::present`] copies
/// to the swapchain ([`PRESENT_FORMAT`], the swapchain's size).
pub struct Backbuffer {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
}

impl Backbuffer {
    /// `slot` holding a `w`×`h` back buffer (re-created when the size changes).
    pub fn ensure<'a>(slot: &'a mut Option<Backbuffer>, device: &wgpu::Device, w: u32, h: u32) -> &'a Backbuffer {
        let (w, h) = (w.max(1), h.max(1));
        if !slot.as_ref().is_some_and(|b| b.texture.width() == w && b.texture.height() == h) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("present"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: PRESENT_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            *slot = Some(Backbuffer { texture, view });
        }
        slot.as_ref().unwrap()
    }
}

static LOG_FILE: std::sync::Mutex<Option<std::fs::File>> = std::sync::Mutex::new(None);

/// Starts `upscale.log` in `dir` (what [`log`] writes to).
pub fn open_log(dir: &Path) {
    if let Ok(mut f) = LOG_FILE.lock() {
        *f = std::fs::File::create(dir.join("upscale.log")).ok();
    }
}

/// Logs `msg` to stderr and to `upscale.log` in the log folder given to [`Native::new`].
pub fn log(msg: &str) {
    eprintln!("frogger: upscale: {msg}");
    if let Ok(mut f) = LOG_FILE.lock() {
        if let Some(f) = f.as_mut() {
            use std::io::Write;
            let _ = writeln!(f, "{msg}");
        }
    }
}

#[cfg(windows)]
unsafe extern "C" fn log_cb(level: std::ffi::c_int, msg: *const std::ffi::c_char) {
    // SAFETY: the native side passes a NUL-terminated string.
    let m = unsafe { std::ffi::CStr::from_ptr(msg) }.to_string_lossy();
    let tag = match level {
        2 => "error: ",
        1 => "warning: ",
        _ => "",
    };
    log(&format!("{tag}{m}"));
}

#[cfg(windows)]
fn wide(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().chain([0]).collect()
}

#[cfg(windows)]
fn cstr(b: &[std::ffi::c_char]) -> String {
    let bytes: Vec<u8> = b.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(windows)]
fn sr_code(s: SrBackend) -> u32 {
    match s {
        SrBackend::Off => 0,
        SrBackend::Dlss => 1,
        SrBackend::Fsr3 => 2,
        SrBackend::Fsr4 => 3,
    }
}

/// Wanted size for the format the swapchain copy needs.
pub const PRESENT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl Native {
    /// Loads the runtimes `init` asks for and creates the swapchain on `init.hwnd`. The
    /// device must be wgpu's DX12 back end.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, init: &Init) -> Result<Native, String> {
        #[cfg(windows)]
        {
            let (Some(d), Some(q)) = (dx12::device(device), dx12::queue(queue)) else {
                return Err("upscalers need the Direct3D 12 back end".into());
            };
            log(&format!("runtimes from {}", init.runtime_dir.display()));
            let dir = wide(init.runtime_dir);
            let logs = wide(init.log_dir);
            let fi = ffi::Init {
                device: d,
                queue: q,
                hwnd: init.hwnd as *mut std::ffi::c_void,
                runtime_dir: dir.as_ptr(),
                log_dir: logs.as_ptr(),
                sr: sr_code(init.sr),
                fg: match init.fg {
                    FgBackend::Off => 0,
                    FgBackend::Dlss => 1,
                    FgBackend::Fsr => 2,
                },
                width: init.size.0.max(1),
                height: init.size.1.max(1),
                log: Some(log_cb),
                nr_helpers: init.nr_helpers as u32,
            };
            let mut err = [0 as std::ffi::c_char; 512];
            // SAFETY: valid pointers for the call; the strings outlive it.
            let raw = unsafe { ffi::f2up_create(&fi, err.as_mut_ptr(), err.len() as u32) };
            if raw.is_null() {
                return Err(cstr(&err));
            }
            let mut n = Native { raw, sr: init.sr, opts: Options::default(), last: std::time::Instant::now() };
            n.push_options();
            Ok(n)
        }
        #[cfg(not(windows))]
        {
            let _ = (device, queue, init);
            Err("DLSS / FSR need Windows (Direct3D 12)".into())
        }
    }

    pub fn backend(&self) -> SrBackend {
        self.sr
    }

    /// Switches the super resolution back end (live).
    pub fn set_backend(&mut self, sr: SrBackend) {
        self.sr = sr;
        #[cfg(windows)]
        // SAFETY: `raw` is alive.
        unsafe {
            ffi::f2up_set_backend(self.raw, sr_code(sr))
        };
    }

    pub fn options(&self) -> &Options {
        &self.opts
    }

    pub fn set_options(&mut self, o: &Options) {
        if *o != self.opts {
            self.opts = *o;
            self.push_options();
        }
    }

    fn push_options(&mut self) {
        #[cfg(windows)]
        {
            let o = &self.opts;
            let fo = ffi::Options {
                sr_mode: UpscaleMode::ALL.iter().position(|m| *m == o.mode).unwrap_or(1) as u32,
                dlss_preset: o.dlss_preset,
                sharpness: o.sharpness.clamp(0.0, 1.0),
                fg_mode: match o.fg {
                    FgMode::Off => 0,
                    FgMode::X2 => 1,
                    FgMode::X3 => 2,
                    FgMode::X4 => 3,
                    FgMode::Dynamic => 4,
                },
                fg_target_fps: o.fg_target_fps,
                reflex: o.reflex as u32,
                vsync: o.vsync as u32,
            };
            // SAFETY: `raw` is alive.
            unsafe { ffi::f2up_set_options(self.raw, &fo) };
        }
    }

    /// The 3D render size for `output` (the output size itself without an upscaler).
    pub fn render_size(&self, output: (u32, u32)) -> (u32, u32) {
        if self.sr == SrBackend::Off {
            return output;
        }
        #[cfg(windows)]
        if self.sr == SrBackend::Dlss {
            let (mut w, mut h) = (0, 0);
            // SAFETY: `raw` is alive.
            unsafe { ffi::f2up_render_size(self.raw, output.0, output.1, &mut w, &mut h) };
            if w > 0 && h > 0 {
                return (w.min(output.0), h.min(output.1));
            }
        }
        self.opts.mode.render_size(output)
    }

    /// This frame's sub-pixel jitter (none without an upscaler).
    pub fn jitter(&self, frame: u64, render: (u32, u32), output: (u32, u32)) -> [f32; 2] {
        if self.sr == SrBackend::Off {
            return [0.0; 2];
        }
        jitter(frame, jitter_phase_count(render.0, output.0))
    }

    /// Start of a frame (Reflex, latency markers): call before the simulation runs.
    pub fn begin_frame(&mut self, frame: u64) {
        #[cfg(windows)]
        // SAFETY: `raw` is alive.
        unsafe {
            ffi::f2up_begin_frame(self.raw, frame)
        };
        #[cfg(not(windows))]
        let _ = frame;
    }

    /// Presents `src` (a [`Backbuffer`], drawn and submitted).
    pub fn present(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, src: &wgpu::Texture) -> bool {
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("present") });
        enc.transition_resources(
            std::iter::empty(),
            [wgpu::TextureTransition { texture: src, selector: None, state: wgpu::TextureUses::COPY_SRC }].into_iter(),
        );
        queue.submit([enc.finish()]);
        #[cfg(windows)]
        let ok = match dx12::texture(src) {
            // SAFETY: `raw` is alive, the texture is in COPY_SOURCE and alive.
            Some(t) => unsafe { ffi::f2up_present(self.raw, t) != 0 },
            None => false,
        };
        #[cfg(not(windows))]
        let ok = false;
        // Our own present: let wgpu reclaim what finished.
        let _ = device.poll(wgpu::PollType::Poll);
        ok
    }

    /// Resizes the swapchain (waits for the GPU).
    pub fn resize(&mut self, w: u32, h: u32) {
        #[cfg(windows)]
        // SAFETY: `raw` is alive.
        unsafe {
            ffi::f2up_resize(self.raw, w.max(1), h.max(1))
        };
        #[cfg(not(windows))]
        let _ = (w, h);
    }

    pub fn status(&self) -> Status {
        #[cfg(windows)]
        {
            // SAFETY: plain C struct, filled by the call.
            let mut s: ffi::Status = unsafe { std::mem::zeroed() };
            unsafe { ffi::f2up_status(self.raw, &mut s) };
            Status {
                streamline: s.sl_loaded != 0,
                dlss: s.dlss_supported != 0,
                dlss_fg: s.dlssg_supported != 0,
                dlss_fg_max_frames: s.dlssg_max_frames,
                dlss_fg_dynamic: s.dlssg_dynamic != 0,
                reflex: s.reflex_supported != 0,
                fidelityfx: s.ffx_loaded != 0,
                fsr3: s.fsr3_available != 0,
                fsr4: s.fsr4_available != 0,
                fsr_fg: s.fsr_fg_available != 0,
                active: match s.sr_active {
                    1 => SrBackend::Dlss,
                    2 => SrBackend::Fsr3,
                    3 => SrBackend::Fsr4,
                    _ => SrBackend::Off,
                },
                fg_backend: match s.fg_backend {
                    1 => FgBackend::Dlss,
                    2 => FgBackend::Fsr,
                    _ => FgBackend::Off,
                },
                fg_active: s.fg_active != 0,
                sr_name: cstr(&s.sr_name),
                fg_name: cstr(&s.fg_name),
                error: cstr(&s.error),
            }
        }
        #[cfg(not(windows))]
        Status::default()
    }
}

impl Upscaler for Native {
    fn upscale(&mut self, _device: &wgpu::Device, _queue: &wgpu::Queue, i: &UpscaleInputs) -> bool {
        let now = std::time::Instant::now();
        let dt = (now - self.last).as_secs_f32() * 1000.0;
        self.last = now;
        #[cfg(windows)]
        {
            let (Some(color), Some(depth), Some(motion), Some(output)) =
                (dx12::texture(i.color), dx12::texture(i.depth), dx12::texture(i.motion), dx12::texture(i.output))
            else {
                return false;
            };
            let flat = |m: &[[f32; 4]; 4]| {
                let mut o = [0.0; 16];
                for r in 0..4 {
                    o[r * 4..r * 4 + 4].copy_from_slice(&m[r]);
                }
                o
            };
            let c = &i.camera;
            let f = ffi::Frame {
                color,
                depth,
                motion,
                output,
                render_w: i.render_size.0,
                render_h: i.render_size.1,
                output_w: i.output_size.0,
                output_h: i.output_size.1,
                jitter_x: i.jitter[0],
                jitter_y: i.jitter[1],
                reset: i.reset as u32,
                frame_time_ms: dt.clamp(0.1, 1000.0),
                view_to_clip: flat(&c.view_to_clip),
                clip_to_view: flat(&c.clip_to_view),
                clip_to_prev_clip: flat(&c.clip_to_prev_clip),
                prev_clip_to_clip: flat(&c.prev_clip_to_clip),
                cam_pos: c.position,
                cam_up: c.up,
                cam_right: c.right,
                cam_fwd: c.forward,
                near_z: c.near,
                far_z: c.far,
                fov_y: c.fov_y,
                aspect: c.aspect,
                frame: i.frame,
            };
            // SAFETY: `raw` is alive; the resources are in the states `UpscaleInputs` documents.
            unsafe { ffi::f2up_evaluate(self.raw, &f) != 0 }
        }
        #[cfg(not(windows))]
        {
            let _ = (i, dt);
            false
        }
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        #[cfg(windows)]
        // SAFETY: created by `f2up_create`, destroyed once.
        unsafe {
            ffi::f2up_destroy(self.raw)
        };
    }
}
