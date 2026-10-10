//! winit window + event loop wrapper. The window is an `Arc<winit::window::Window>`, which
//! implements `HasWindowHandle`/`HasDisplayHandle` (raw-window-handle 0.6) for wgpu surfaces.

use crate::config::WindowMode;
use crate::dik;
use crate::input::KeyboardState;
use std::sync::Arc;
pub use winit;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
pub use winit::raw_window_handle;
pub use winit::window::Window;
use winit::window::{Fullscreen, WindowId};

#[derive(Clone, Debug)]
pub struct WindowConfig {
    pub title: String,
    /// Windowed inner size in LOGICAL points (see [`logical_to_physical`]).
    pub width: u32,
    pub height: u32,
    pub mode: WindowMode,
    /// Explicit resolution (`None` = auto), see [`crate::config::Video::resolution`].
    pub resolution: Option<[u32; 2]>,
    pub refresh_mhz: Option<u32>,
    /// Windowed outer position.
    pub position: Option<[i32; 2]>,
    pub resizable: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        WindowConfig { title: "Frogger 2".into(), width: 640, height: 480, mode: WindowMode::Windowed, resolution: None, refresh_mhz: None, position: None, resizable: true }
    }
}

impl WindowConfig {
    pub fn from_config(c: &crate::config::Config) -> WindowConfig {
        let v = &c.video;
        let [width, height] = match (v.window_mode, v.resolution) {
            (WindowMode::Windowed, Some(r)) => r,
            _ => [v.width, v.height],
        };
        WindowConfig { width, height, mode: v.window_mode, resolution: v.resolution, refresh_mhz: v.refresh_mhz, position: v.window_pos, ..Default::default() }
    }
}

/// A video mode of the window's monitor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoMode {
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
}

fn monitor(window: &Window) -> Option<winit::monitor::MonitorHandle> {
    window.current_monitor().or_else(|| window.primary_monitor()).or_else(|| window.available_monitors().next())
}

/// The video modes of the window's monitor (largest first, then fastest), without
/// duplicates (bit depths).
pub fn video_modes(window: &Window) -> Vec<VideoMode> {
    let Some(m) = monitor(window) else { return Vec::new() };
    let mut v: Vec<VideoMode> = m.video_modes().map(|h| VideoMode { width: h.size().width, height: h.size().height, refresh_mhz: h.refresh_rate_millihertz() }).collect();
    v.sort_by_key(|m| std::cmp::Reverse((m.width * m.height, m.width, m.refresh_mhz)));
    v.dedup();
    v
}

/// The monitor's current (desktop) size and refresh rate.
pub fn monitor_mode(window: &Window) -> Option<VideoMode> {
    let m = monitor(window)?;
    let s = m.size();
    Some(VideoMode { width: s.width, height: s.height, refresh_mhz: m.refresh_rate_millihertz().unwrap_or(0) })
}

/// The window's actual mode.
pub fn actual_mode(window: &Window) -> WindowMode {
    match window.fullscreen() {
        None => WindowMode::Windowed,
        Some(Fullscreen::Borderless(_)) => WindowMode::Borderless,
        Some(Fullscreen::Exclusive(_)) => WindowMode::Fullscreen,
    }
}

/// The exclusive video mode for `resolution` (`None`: the desktop size) and `refresh_mhz`
/// (`None` or unavailable: the highest rate of that size); the closest size when the exact
/// one does not exist.
pub fn pick_video_mode(modes: &[VideoMode], desktop: Option<VideoMode>, resolution: Option<[u32; 2]>, refresh_mhz: Option<u32>) -> Option<VideoMode> {
    let [w, h] = resolution.or(desktop.map(|d| [d.width, d.height]))?;
    let same: Vec<&VideoMode> = modes.iter().filter(|m| m.width == w && m.height == h).collect();
    let pool: Vec<&VideoMode> = if same.is_empty() {
        let best = modes.iter().min_by_key(|m| (m.width as i64 - w as i64).abs() + (m.height as i64 - h as i64).abs())?;
        modes.iter().filter(|m| m.width == best.width && m.height == best.height).collect()
    } else {
        same
    };
    pool.iter().find(|m| Some(m.refresh_mhz) == refresh_mhz).or_else(|| pool.iter().max_by_key(|m| m.refresh_mhz)).map(|m| **m)
}

/// Puts the window in `mode`: windowed at `windowed_size` (or the explicit resolution),
/// borderless on its monitor, or exclusive fullscreen with the video mode matching
/// `resolution` / `refresh_mhz` (falls back to borderless when the platform lists no video
/// modes). Returns a log line describing what was requested.
pub fn apply_mode(window: &Window, mode: WindowMode, resolution: Option<[u32; 2]>, refresh_mhz: Option<u32>, windowed_size: [u32; 2]) -> String {
    match mode {
        WindowMode::Windowed => {
            if window.fullscreen().is_some() {
                window.set_fullscreen(None);
                // Leaving fullscreen restores the saved placement, which can be maximized.
                window.set_maximized(false);
            }
            let [w, h] = resolution.unwrap_or(windowed_size);
            let _ = window.request_inner_size(LogicalSize::new(w.max(160), h.max(120)));
            format!("windowed {w}x{h}")
        }
        WindowMode::Borderless => {
            window.set_fullscreen(Some(Fullscreen::Borderless(monitor(window))));
            match resolution {
                Some([w, h]) => format!("borderless, render target {w}x{h}"),
                None => "borderless (desktop resolution)".into(),
            }
        }
        WindowMode::Fullscreen => {
            let Some(m) = monitor(window) else {
                window.set_fullscreen(Some(Fullscreen::Borderless(None)));
                return "exclusive fullscreen: no monitor, using borderless".into();
            };
            let modes = video_modes(window);
            let want = pick_video_mode(&modes, monitor_mode(window), resolution, refresh_mhz);
            let handle = want.and_then(|v| m.video_modes().find(|h| h.size().width == v.width && h.size().height == v.height && h.refresh_rate_millihertz() == v.refresh_mhz));
            match (want, handle) {
                (Some(v), Some(h)) => {
                    window.set_fullscreen(Some(Fullscreen::Exclusive(h)));
                    format!("exclusive fullscreen {}x{} @ {:.2} Hz", v.width, v.height, v.refresh_mhz as f32 / 1000.0)
                }
                _ => {
                    window.set_fullscreen(Some(Fullscreen::Borderless(Some(m))));
                    "exclusive fullscreen: no video modes, using borderless".into()
                }
            }
        }
    }
}

/// What the app wants after a callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Exit,
}

/// Per-frame context handed to [`WindowApp::frame`].
pub struct Frame<'a> {
    pub window: &'a Arc<Window>,
    /// Keyboard in DirectInput scan codes; its event buffer is cleared after the frame.
    pub keyboard: &'a KeyboardState,
    /// `windowActive`.
    pub focused: bool,
}

/// Application callbacks. `frame` is called continuously (poll mode); the app paces itself
/// (e.g. fixed 60 Hz ticks with an accumulator).
pub trait WindowApp {
    /// The window was created (or re-created after a suspend).
    fn init(&mut self, _window: &Arc<Window>) {}
    /// Raw window events after the wrapper handled keyboard/focus.
    fn event(&mut self, _window: &Arc<Window>, _event: &WindowEvent) -> Flow {
        Flow::Continue
    }
    fn resized(&mut self, _window: &Arc<Window>, _size: PhysicalSize<u32>) {}
    fn frame(&mut self, frame: Frame<'_>) -> Flow;
}

/// After a switch to windowed has settled: restores the wanted client size (logical
/// points) when the platform restored a different placement, and re-centres a window that
/// covers its whole monitor (a placement saved mid-transition), keeping the title bar
/// reachable.
pub fn fix_windowed(window: &Window, size: [u32; 2]) {
    if window.fullscreen().is_some() || window.is_maximized() {
        return;
    }
    let scale = window.scale_factor();
    let [w, h] = [size[0].max(160), size[1].max(120)];
    let inner = window.inner_size();
    if (inner.width, inner.height) == (logical_to_physical(w, scale), logical_to_physical(h, scale)) {
        return;
    }
    let _ = window.request_inner_size(LogicalSize::new(w, h));
    let (Some(m), Ok(pos)) = (monitor(window), window.outer_position()) else { return };
    let (mp, ms, outer) = (m.position(), m.size(), window.outer_size());
    if outer.width >= ms.width || outer.height >= ms.height || pos.y < mp.y {
        let (pw, ph) = (logical_to_physical(w, scale) as i32, logical_to_physical(h, scale) as i32);
        let x = mp.x + (ms.width as i32 - pw).max(0) / 2;
        let y = mp.y + (ms.height as i32 - ph).max(0) / 2;
        window.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
    }
}

/// Switches between borderless fullscreen and windowed.
pub fn set_fullscreen(window: &Window, on: bool) {
    window.set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
}

/// Remembers the windowed position (`WindowEvent::Moved` while windowed).
pub fn windowed_position(window: &Window) -> Option<[i32; 2]> {
    if window.fullscreen().is_some() {
        return None;
    }
    window.outer_position().ok().map(|p| [p.x, p.y])
}

/// Converts a physical pixel length to logical points for `scale` (the window's
/// `scale_factor()`); non-finite or non-positive scales count as 1.
pub fn physical_to_logical(px: u32, scale: f64) -> u32 {
    (px as f64 / sane_scale(scale)).round() as u32
}

/// Converts a logical length to physical pixels for `scale`.
pub fn logical_to_physical(pt: u32, scale: f64) -> u32 {
    (pt as f64 * sane_scale(scale)).round() as u32
}

fn sane_scale(s: f64) -> f64 {
    if s.is_finite() && s > 0.0 {
        s
    } else {
        1.0
    }
}

/// The requested window size. One rule on every platform: configured windowed sizes are
/// LOGICAL (a 640x480 window stays 640x480 points on a Retina / 150% display, rendered at
/// the physical size: the surface follows `inner_size()`/`Resized`, which are physical).
/// Exclusive-fullscreen resolutions and render targets stay physical.
fn initial_size(cfg: &WindowConfig) -> winit::dpi::Size {
    LogicalSize::new(cfg.width, cfg.height).into()
}

struct Runner<A: WindowApp> {
    cfg: WindowConfig,
    app: A,
    window: Option<Arc<Window>>,
    keyboard: KeyboardState,
    focused: bool,
    error: Option<String>,
    /// Cmd (super) held: macOS shortcut handling, see [`MAC_SHORTCUTS`].
    cmd_held: bool,
}

/// macOS conventions: Cmd+Q / Cmd+W quit, Cmd+F, Ctrl+Cmd+F and Cmd+Enter toggle fullscreen
/// (delivered to the app as F11, like Alt+Enter on Windows/Linux). Other Cmd+key presses are
/// hidden from the game so system shortcuts (Cmd+H, Cmd+M, Cmd+Tab) never register as input.
const MAC_SHORTCUTS: bool = cfg!(target_os = "macos");

impl<A: WindowApp> ApplicationHandler for Runner<A> {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let mut attrs = Window::default_attributes()
            .with_title(self.cfg.title.clone())
            .with_inner_size(initial_size(&self.cfg))
            .with_resizable(self.cfg.resizable);
        if let Some([x, y]) = self.cfg.position {
            attrs = attrs.with_position(PhysicalPosition::new(x, y));
        }
        match el.create_window(attrs) {
            Ok(w) => {
                let w = Arc::new(w);
                if self.cfg.mode != WindowMode::Windowed {
                    let c = &self.cfg;
                    let msg = apply_mode(&w, c.mode, c.resolution, c.refresh_mhz, [c.width, c.height]);
                    eprintln!("frogger: window: {msg}");
                }
                self.app.init(&w);
                self.window = Some(w);
            }
            Err(e) => {
                self.error = Some(e.to_string());
                el.exit();
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(w) = self.window.clone() else { return };
        match &event {
            WindowEvent::CloseRequested => {
                el.exit();
                return;
            }
            WindowEvent::Focused(f) => {
                self.focused = *f;
                if !f {
                    self.keyboard.clear();
                }
            }
            WindowEvent::Resized(s) => self.app.resized(&w, *s),
            WindowEvent::ModifiersChanged(m) => self.cmd_held = m.state().super_key(),
            WindowEvent::KeyboardInput { event: k, .. } if MAC_SHORTCUTS && self.cmd_held && k.state == ElementState::Pressed => match k.physical_key {
                PhysicalKey::Code(KeyCode::KeyQ | KeyCode::KeyW) => {
                    el.exit();
                    return;
                }
                PhysicalKey::Code(KeyCode::KeyF | KeyCode::Enter | KeyCode::NumpadEnter) if !k.repeat => self.keyboard.key_down(dik::F11),
                _ => {}
            },
            WindowEvent::KeyboardInput { event: k, .. } => {
                if MAC_SHORTCUTS && matches!(k.physical_key, PhysicalKey::Code(KeyCode::KeyF | KeyCode::Enter | KeyCode::NumpadEnter)) {
                    self.keyboard.key_up(dik::F11);
                }
                if let PhysicalKey::Code(code) = k.physical_key {
                    if let Some(d) = dik_from_keycode(code) {
                        match k.state {
                            ElementState::Pressed if !k.repeat => self.keyboard.key_down(d),
                            ElementState::Pressed => {}
                            ElementState::Released => self.keyboard.key_up(d),
                        }
                    }
                }
            }
            _ => {}
        }
        if self.app.event(&w, &event) == Flow::Exit {
            el.exit();
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        let Some(w) = self.window.clone() else { return };
        let flow = self.app.frame(Frame { window: &w, keyboard: &self.keyboard, focused: self.focused });
        self.keyboard.end_frame();
        if flow == Flow::Exit {
            el.exit();
        }
    }
}

/// Creates the window and runs the event loop until the app exits or the window is closed.
/// True when running under WSL (WSLg).
pub fn is_wsl() -> bool {
    std::fs::read_to_string("/proc/version").map(|v| v.to_lowercase().contains("microsoft")).unwrap_or(false)
}

/// Under WSLg a native Wayland winit window crashes the Weston compositor;
/// use X11 (XWayland) unless `F2_WAYLAND=1` is set.
fn prefer_x11_on_wsl() {
    if cfg!(target_os = "linux") && is_wsl() && std::env::var_os("F2_WAYLAND").is_none() && std::env::var_os("DISPLAY").is_some() {
        // Single-threaded at this point (before the event loop starts).
        #[allow(deprecated)]
        std::env::remove_var("WAYLAND_DISPLAY");
    }
}

pub fn run<A: WindowApp>(cfg: WindowConfig, app: A) -> Result<A, String> {
    prefer_x11_on_wsl();
    let el = EventLoop::new().map_err(|e| e.to_string())?;
    el.set_control_flow(ControlFlow::Poll);
    let mut r = Runner { cfg, app, window: None, keyboard: KeyboardState::default(), focused: true, error: None, cmd_held: false };
    el.run_app(&mut r).map_err(|e| e.to_string())?;
    match r.error {
        Some(e) => Err(e),
        None => Ok(r.app),
    }
}

/// Physical key -> DirectInput scan code (US layout positions, like DirectInput).
pub fn dik_from_keycode(k: KeyCode) -> Option<u8> {
    use KeyCode::*;
    Some(match k {
        Escape => dik::ESCAPE,
        Digit1 => dik::KEY_1,
        Digit2 => dik::KEY_2,
        Digit3 => dik::KEY_3,
        Digit4 => dik::KEY_4,
        Digit5 => dik::KEY_5,
        Digit6 => dik::KEY_6,
        Digit7 => dik::KEY_7,
        Digit8 => dik::KEY_8,
        Digit9 => dik::KEY_9,
        Digit0 => dik::KEY_0,
        Minus => dik::MINUS,
        Equal => dik::EQUALS,
        Backspace => dik::BACK,
        Tab => dik::TAB,
        KeyQ => dik::Q,
        KeyW => dik::W,
        KeyE => dik::E,
        KeyR => dik::R,
        KeyT => dik::T,
        KeyY => dik::Y,
        KeyU => dik::U,
        KeyI => dik::I,
        KeyO => dik::O,
        KeyP => dik::P,
        BracketLeft => dik::LBRACKET,
        BracketRight => dik::RBRACKET,
        Enter => dik::RETURN,
        ControlLeft => dik::LCONTROL,
        KeyA => dik::A,
        KeyS => dik::S,
        KeyD => dik::D,
        KeyF => dik::F,
        KeyG => dik::G,
        KeyH => dik::H,
        KeyJ => dik::J,
        KeyK => dik::K,
        KeyL => dik::L,
        Semicolon => dik::SEMICOLON,
        Quote => dik::APOSTROPHE,
        Backquote => dik::GRAVE,
        ShiftLeft => dik::LSHIFT,
        Backslash => dik::BACKSLASH,
        KeyZ => dik::Z,
        KeyX => dik::X,
        KeyC => dik::C,
        KeyV => dik::V,
        KeyB => dik::B,
        KeyN => dik::N,
        KeyM => dik::M,
        Comma => dik::COMMA,
        Period => dik::PERIOD,
        Slash => dik::SLASH,
        ShiftRight => dik::RSHIFT,
        NumpadMultiply => dik::MULTIPLY,
        AltLeft => dik::LMENU,
        Space => dik::SPACE,
        CapsLock => dik::CAPITAL,
        F1 => dik::F1,
        F2 => dik::F2,
        F3 => dik::F3,
        F4 => dik::F4,
        F5 => dik::F5,
        F6 => dik::F6,
        F7 => dik::F7,
        F8 => dik::F8,
        F9 => dik::F9,
        F10 => dik::F10,
        NumLock => dik::NUMLOCK,
        ScrollLock => dik::SCROLL,
        Numpad7 => dik::NUMPAD7,
        Numpad8 => dik::NUMPAD8,
        Numpad9 => dik::NUMPAD9,
        NumpadSubtract => dik::SUBTRACT,
        Numpad4 => dik::NUMPAD4,
        Numpad5 => dik::NUMPAD5,
        Numpad6 => dik::NUMPAD6,
        NumpadAdd => dik::ADD,
        Numpad1 => dik::NUMPAD1,
        Numpad2 => dik::NUMPAD2,
        Numpad3 => dik::NUMPAD3,
        Numpad0 => dik::NUMPAD0,
        NumpadDecimal => dik::DECIMAL,
        IntlBackslash => dik::OEM_102,
        F11 => dik::F11,
        F12 => dik::F12,
        F13 => dik::F13,
        F14 => dik::F14,
        F15 => dik::F15,
        NumpadEqual => dik::NUMPADEQUALS,
        NumpadEnter => dik::NUMPADENTER,
        ControlRight => dik::RCONTROL,
        NumpadComma => dik::NUMPADCOMMA,
        NumpadDivide => dik::DIVIDE,
        PrintScreen => dik::SYSRQ,
        AltRight => dik::RMENU,
        Pause => dik::PAUSE,
        Home => dik::HOME,
        ArrowUp => dik::UP,
        PageUp => dik::PRIOR,
        ArrowLeft => dik::LEFT,
        ArrowRight => dik::RIGHT,
        End => dik::END,
        ArrowDown => dik::DOWN,
        PageDown => dik::NEXT,
        Insert => dik::INSERT,
        Delete => dik::DELETE,
        SuperLeft => dik::LWIN,
        SuperRight => dik::RWIN,
        ContextMenu => dik::APPS,
        IntlYen => dik::YEN,
        KanaMode => dik::KANA,
        Convert => dik::CONVERT,
        NonConvert => dik::NOCONVERT,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_mode_choice() {
        let m = |width, height, hz: u32| VideoMode { width, height, refresh_mhz: hz * 1000 };
        let modes = [m(2560, 1440, 144), m(2560, 1440, 60), m(1920, 1080, 120), m(1920, 1080, 60), m(640, 480, 60)];
        let desk = Some(m(2560, 1440, 144));
        assert_eq!(pick_video_mode(&modes, desk, None, None), Some(m(2560, 1440, 144)));
        assert_eq!(pick_video_mode(&modes, desk, Some([1920, 1080]), None), Some(m(1920, 1080, 120)));
        assert_eq!(pick_video_mode(&modes, desk, Some([1920, 1080]), Some(60_000)), Some(m(1920, 1080, 60)));
        // Unknown size: the closest one.
        assert_eq!(pick_video_mode(&modes, desk, Some([1900, 1000]), Some(60_000)), Some(m(1920, 1080, 60)));
        assert_eq!(pick_video_mode(&[], desk, None, None), None);
    }
}

#[cfg(test)]
mod scale_tests {
    use super::*;

    #[test]
    fn logical_physical_round_trip() {
        for scale in [1.0, 1.25, 1.5, 2.0, 3.0] {
            for pt in [160u32, 480, 640, 1281] {
                assert_eq!(physical_to_logical(logical_to_physical(pt, scale), scale), pt, "{pt} @ {scale}");
            }
        }
        assert_eq!(logical_to_physical(640, 2.0), 1280);
        assert_eq!(physical_to_logical(1280, 2.0), 640);
    }

    #[test]
    fn bad_scale_is_identity() {
        for s in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(logical_to_physical(640, s), 640);
            assert_eq!(physical_to_logical(640, s), 640);
        }
    }
}
