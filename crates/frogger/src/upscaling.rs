//! `[upscale]` on the desktop: DLSS / FSR super resolution and frame generation through
//! `f2-upscale` (Windows, Direct3D 12; docs/UPSCALING.md). The window then has no wgpu surface:
//! the native side owns the swapchain and [`f2_upscale::Native::present`] shows each frame.

use f2_engine::{GpuContext, UpscaleMode};
use f2_platform::window::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use f2_platform::window::Window;
use f2_platform::{Config, FrameGeneration, ReflexMode, Upscale, UpscaleQuality, UpscalerKind};
use f2_upscale::{FgBackend, FgMode, Options, Reflex, SrBackend};
use std::path::PathBuf;

pub use f2_upscale::{Backbuffer, Native, Status, PRESENT_FORMAT};

pub fn sr_backend(k: UpscalerKind) -> SrBackend {
    match k {
        UpscalerKind::Off => SrBackend::Off,
        UpscalerKind::Dlss => SrBackend::Dlss,
        UpscalerKind::Fsr3 => SrBackend::Fsr3,
        UpscalerKind::Fsr4 => SrBackend::Fsr4,
    }
}

pub fn fg_backend(f: FrameGeneration) -> FgBackend {
    match f {
        FrameGeneration::Off => FgBackend::Off,
        FrameGeneration::Fsr2x => FgBackend::Fsr,
        _ => FgBackend::Dlss,
    }
}

pub fn mode(q: UpscaleQuality) -> UpscaleMode {
    match q {
        UpscaleQuality::Native => UpscaleMode::Native,
        UpscaleQuality::Quality => UpscaleMode::Quality,
        UpscaleQuality::Balanced => UpscaleMode::Balanced,
        UpscaleQuality::Performance => UpscaleMode::Performance,
        UpscaleQuality::UltraPerformance => UpscaleMode::UltraPerformance,
    }
}

/// The live options of `cfg` (`[upscale]` and `[video] vsync`).
pub fn options(cfg: &Config) -> Options {
    let u = &cfg.upscale;
    Options {
        mode: mode(u.quality),
        dlss_preset: u.dlss_preset.streamline(),
        sharpness: u.sharpness.min(100) as f32 / 100.0,
        fg: match u.frame_generation {
            FrameGeneration::Off => FgMode::Off,
            FrameGeneration::Dlss2x | FrameGeneration::Fsr2x => FgMode::X2,
            FrameGeneration::Dlss3x => FgMode::X3,
            FrameGeneration::Dlss4x => FgMode::X4,
            FrameGeneration::DlssDynamic => FgMode::Dynamic,
        },
        fg_target_fps: u.target_fps as f32,
        reflex: match u.reflex {
            ReflexMode::Off => Reflex::Off,
            ReflexMode::On => Reflex::On,
            ReflexMode::Boost => Reflex::Boost,
        },
        vsync: cfg.video.vsync,
    }
}

/// Where the DLSS / FSR DLLs are looked for: `runtime_dir`, else `upscalers` beside the
/// executable (or its parent folder) if it exists, else the executable's folder.
pub fn runtime_dir(u: &Upscale) -> PathBuf {
    if let Some(d) = &u.runtime_dir {
        return d.clone();
    }
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_else(|| PathBuf::from("."));
    // Beside the exe, or beside the install root's game exe when this is `engine\frogger2.exe`.
    let candidates = [Some(exe_dir.join("upscalers")), exe_dir.parent().map(|p| p.join("upscalers"))];
    candidates.into_iter().flatten().find(|d| d.is_dir()).unwrap_or(exe_dir)
}

/// The GPU context and the native upscaler / swapchain for `window` (Direct3D 12 only).
pub fn create(window: &Window, cfg: &Config, display: Box<dyn wgpu::wgt::WgpuHasDisplayHandle>) -> Result<(GpuContext, Native), String> {
    if !cfg!(windows) {
        return Err("DLSS / FSR need Windows (Direct3D 12)".into());
    }
    let hwnd = match window.window_handle().map_err(|e| e.to_string())?.as_raw() {
        RawWindowHandle::Win32(h) => h.hwnd.get(),
        _ => return Err("not a Win32 window".into()),
    };
    let logs = f2_platform::paths::config_dir().join("logs");
    let _ = std::fs::create_dir_all(&logs);
    f2_upscale::open_log(&logs);
    f2_upscale::log(&format!("starting: {:?} upscaler, {:?} frame generation", cfg.upscale.upscaler, cfg.upscale.frame_generation));
    let mut desc = wgpu::InstanceDescriptor::new_with_display_handle_from_env(display);
    desc.backends = wgpu::Backends::DX12;
    let instance = wgpu::Instance::new(desc);
    // After the instance: wgpu creates an `IDXGIFactoryMedia` there, which ReShade's
    // `CreateDXGIFactory1` hook crashes on. Device and swapchain creation come after.
    load_reshade();
    let u = &cfg.upscale;
    let ctx = GpuContext::for_adapter_named(instance, wgpu::Backends::DX12, u.adapter.as_deref()).map_err(|e| e.to_string())?;
    f2_upscale::log(&format!("GPU: {} ({:?})", ctx.adapter.get_info().name, ctx.adapter.get_info().backend));
    let size = window.inner_size();
    let dir = runtime_dir(u);

    let init = f2_upscale::Init {
        hwnd,
        runtime_dir: &dir,
        log_dir: &logs,
        sr: sr_backend(u.upscaler),
        fg: fg_backend(u.frame_generation),
        size: (size.width, size.height),
        nr_helpers: u.dlss5_helpers,
    };
    let mut native = Native::new(&ctx.device, &ctx.queue, &init)?;
    native.set_options(&options(cfg));
    Ok((ctx, native))
}

/// One line for the log / FPS counter: what runs.
pub fn describe(s: &Status) -> String {
    let mut parts = Vec::new();
    if !s.sr_name.is_empty() {
        parts.push(s.sr_name.clone());
    }
    if s.fg_active && !s.fg_name.is_empty() {
        parts.push(s.fg_name.clone());
    }
    if parts.is_empty() {
        "no upscaler".into()
    } else {
        parts.join(" + ")
    }
}

/// Testing (`F2_UPSCALE_SHOT=FILE.png:FRAME`): writes `tex` ([`PRESENT_FORMAT`]) to a PNG.
pub fn save_png(device: &wgpu::Device, queue: &wgpu::Queue, tex: &wgpu::Texture, path: &std::path::Path) -> Result<(), String> {
    let (w, h) = (tex.width(), tex.height());
    let bpr = (w * 4).div_ceil(256) * 256;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("shot"),
        size: (bpr * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(h) } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit([enc.finish()]);
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| e.to_string())?;
    let data = slice.get_mapped_range().map_err(|e| e.to_string())?;
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for row in 0..h {
        let o = (row * bpr) as usize;
        px.extend_from_slice(&data[o..o + (w * 4) as usize]);
    }
    drop(data);
    buf.unmap();
    f2_engine::RgbaImage { width: w, height: h, pixels: px }.write_png(path).map_err(|e| e.to_string())
}

/// Testing knobs from the environment: `F2_UPSCALE_DEBUG=motion|depth` (the upscaler's
/// inputs instead of the picture), `F2_UPSCALE_SHOT=FILE.png:FRAME` (a screenshot of the
/// presented image at that frame), `F2_UPSCALE_EXIT=FRAME` (quit then).
#[derive(Clone, Debug, Default)]
pub struct Debug {
    pub view: f2_engine::UpscaleDebug,
    pub shot: Option<(PathBuf, u64)>,
    pub exit: Option<u64>,
}

impl Debug {
    pub fn from_env() -> Debug {
        let view = match std::env::var("F2_UPSCALE_DEBUG").as_deref() {
            Ok("motion") => f2_engine::UpscaleDebug::Motion,
            Ok("depth") => f2_engine::UpscaleDebug::Depth,
            _ => f2_engine::UpscaleDebug::Off,
        };
        let shot = std::env::var("F2_UPSCALE_SHOT").ok().and_then(|s| {
            let (p, f) = s.rsplit_once(':')?;
            Some((PathBuf::from(p), f.parse().ok()?))
        });
        let exit = std::env::var("F2_UPSCALE_EXIT").ok().and_then(|s| s.parse().ok());
        Debug { view, shot, exit }
    }
}

/// ReShade (DLSS 5 Neural Rendering through RenoDX's add-on, docs/UPSCALING.md): when
/// `ReShade64.dll` sits beside the executable, [`create`] loads it after wgpu's instance and
/// before the device and swapchain (as a `dxgi.dll` proxy ReShade sees the instance's
/// `IDXGIFactoryMedia` request and crashes inside DXGI).
pub fn load_reshade() {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn LoadLibraryW(name: *const u16) -> isize;
            fn GetSystemDirectoryW(buf: *mut u16, len: u32) -> u32;
        }
        use std::os::windows::ffi::OsStrExt;
        let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) else { return };
        let dll = dir.join("ReShade64.dll");
        if !dll.is_file() {
            return;
        }
        let wide = |p: &std::path::Path| p.as_os_str().encode_wide().chain([0]).collect::<Vec<u16>>();
        let mut sys = [0u16; 260];
        // SAFETY: the buffer is valid for its length; the names are NUL-terminated.
        unsafe {
            let n = GetSystemDirectoryW(sys.as_mut_ptr(), sys.len() as u32) as usize;
            let sys = std::path::PathBuf::from(String::from_utf16_lossy(&sys[..n.min(sys.len())]));
            for lib in ["dxgi.dll", "d3d12.dll"] {
                LoadLibraryW(wide(&sys.join(lib)).as_ptr());
            }
            let ok = LoadLibraryW(wide(&dll).as_ptr()) != 0;
            f2_upscale::log(&format!("ReShade {}: {}", if ok { "loaded" } else { "failed to load" }, dll.display()));
        }
    }
}
