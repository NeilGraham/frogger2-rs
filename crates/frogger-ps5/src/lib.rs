//! Native PlayStation 5 homebrew host for the Frogger 2 port.
//!
//! A staticlib for the stock `x86_64-unknown-freebsd` target (Rust std included), linked
//! against the PS5's own system libraries through the public ps5-payload-sdk's stubs and
//! converted to a PS5 native app (`eboot.bin` + `sce_sys/` + `sce_module/libc.prx`) by
//! `tools/ps5/build.sh`. The frame loop, pads, audio and std runtime shims are shared with
//! the PS4 host (`crates/console-host`); this crate adds the PS5 video out (tiled 4K /
//! 1440p / 1080p buffers, 120 Hz when the display takes it), notifications and `main`.
//! See `tools/ps5/README.md`.

#[cfg(target_os = "freebsd")]
mod compat;
#[cfg(all(target_os = "freebsd", feature = "gpu"))]
mod gpu;
#[cfg(target_os = "freebsd")]
pub mod prospero;
#[cfg(all(target_os = "freebsd", feature = "smoke"))]
mod smoke;
// The GPU build links the payload SDK's real libunwind (the driver's C++ needs it).
#[cfg(all(target_os = "freebsd", not(feature = "gpu")))]
mod unwind;
