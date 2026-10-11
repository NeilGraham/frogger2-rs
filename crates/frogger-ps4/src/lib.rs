//! PlayStation 4/5 homebrew host for the Frogger 2 port.
//!
//! Drives [`frogger::app::App`] (the same front end as the desktop game) with the CPU
//! renderer, the console's video out, pads and audio. The console build is a staticlib
//! for `x86_64-unknown-freebsd` linked against the system libraries with the OpenOrbis
//! toolchain into an fPKG: see `tools/ps4/README.md`. [`host`] holds the portable parts.

pub mod host;
#[cfg(target_os = "freebsd")]
pub mod orbis;
