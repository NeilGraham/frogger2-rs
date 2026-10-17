//! The game front end shared by the desktop executable (`main.rs`, feature `desktop`:
//! winit window, wgpu, gilrs, cpal) and platform hosts without them (`crates/frogger-ps4`),
//! which drive [`app::App`] with their own video, audio and pad layers and the CPU
//! renderer.

pub mod app;
pub mod book;
pub mod controls;
pub mod data;
pub mod editor;
#[cfg(feature = "export")]
pub mod export;
pub use f2_draw::fx;
pub mod gameplay;
pub mod icon;
pub mod mods;
pub mod netplay;
pub use f2_draw::overlay;
pub mod padscript;
#[cfg(feature = "gpu")]
pub mod present;
pub mod record;
pub mod render;
#[cfg(feature = "desktop")]
pub mod setup;
pub mod tinyfont;
pub mod video;
