//! winit window + event loop wrapper. The window is an `Arc<winit::window::Window>`, which
//! implements `HasWindowHandle`/`HasDisplayHandle` (raw-window-handle 0.6) for wgpu surfaces.

use crate::dik;
use crate::input::KeyboardState;
use std::sync::Arc;
pub use winit;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
pub use winit::raw_window_handle;
pub use winit::window::Window;
use winit::window::{Fullscreen, WindowId};

#[derive(Clone, Debug)]
pub struct WindowConfig {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
    pub resizable: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        WindowConfig { title: "Frogger 2".into(), width: 640, height: 480, fullscreen: false, resizable: true }
    }
}

impl WindowConfig {
    pub fn from_config(c: &crate::config::Config) -> WindowConfig {
        WindowConfig { width: c.video.width, height: c.video.height, fullscreen: c.video.fullscreen, ..Default::default() }
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

/// Switches between borderless fullscreen and windowed.
pub fn set_fullscreen(window: &Window, on: bool) {
    window.set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
}

struct Runner<A: WindowApp> {
    cfg: WindowConfig,
    app: A,
    window: Option<Arc<Window>>,
    keyboard: KeyboardState,
    focused: bool,
    error: Option<String>,
}

impl<A: WindowApp> ApplicationHandler for Runner<A> {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(self.cfg.title.clone())
            .with_inner_size(PhysicalSize::new(self.cfg.width, self.cfg.height))
            .with_resizable(self.cfg.resizable)
            .with_fullscreen(self.cfg.fullscreen.then_some(Fullscreen::Borderless(None)));
        match el.create_window(attrs) {
            Ok(w) => {
                let w = Arc::new(w);
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
            WindowEvent::KeyboardInput { event: k, .. } => {
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
    let mut r = Runner { cfg, app, window: None, keyboard: KeyboardState::default(), focused: true, error: None };
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
