//! The game front end shared by the desktop executable (`main.rs`, feature `desktop`:
//! winit window, wgpu, gilrs, cpal) and platform hosts without them (`crates/frogger-ps4`),
//! which drive [`app::App`] with their own video, audio and pad layers and the CPU
//! renderer.

pub mod app;
pub mod book;
pub mod controls;
pub mod data;
pub mod editor;
pub use f2_draw::fx;
pub mod gameplay;
pub mod mods;
pub mod netplay;
pub use f2_draw::overlay;
pub mod padscript;
pub mod record;
pub mod render;
pub mod video;
