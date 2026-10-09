//! Window, input (PSX pad abstraction), config and paths.
//!
//! - [`input`]: `padData` with PSX button bits, the original 56-entry keymap and
//!   `ProcessUserInput` mapping (always available, no windowing dependency).
//! - [`config`]: `config.toml` (serde) replacing the registry; [`paths`]: per-platform dirs.
//! - [`gamepad`] (feature `gamepad`): gilrs pads as `DIJOYSTATE`-like joypads.
//! - [`window`] (feature `window`): winit event-loop wrapper with DirectInput key codes.

pub mod config;
pub mod dik;
#[cfg(feature = "gamepad")]
pub mod gamepad;
pub mod input;
pub mod paths;
#[cfg(feature = "window")]
pub mod window;

pub use config::{Config, DisplaySettings, Profile, TextureFilter};
pub use input::{Controller, InputMapper, JoypadState, KeyboardState, Keymap, PadData, PadState};
