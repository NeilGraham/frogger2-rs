//! Shared PlayStation 4/5 homebrew host for the Frogger 2 port.
//!
//! - [`host`]: the portable parts (pad conversion, framebuffer scalers for linear and
//!   tiled scan-out buffers, where data and saves live), unit-tested on the build machine;
//! - `sce` (console target only): the runtime that lets Rust std run on the consoles'
//!   own system libraries (`libkernel`, `libSceLibcInternal`), and the frame loop that
//!   drives [`frogger::app::App`] with pads, audio and a platform's [`sce::Present`].
//!
//! The platform crates (`crates/frogger-ps4`, `crates/frogger-ps5`) add video out,
//! notifications and the `main` symbol; see `tools/ps4/README.md` and `tools/ps5/README.md`.

pub mod host;
#[cfg(target_os = "freebsd")]
pub mod sce;
