//! `f2fix.dll`: the retail-oracle shim for Frogger 2 (see `docs/ORACLE.md`).
//!
//! A 32-bit (i686-pc-windows-msvc) cdylib deployed next to the user's own
//! `frogger2.exe` as `dinput.dll`. The exe imports `DINPUT.dll`, so Windows
//! loads us before `WinMain`; we forward the DirectInput exports to the real
//! system DLL and patch the exe's import table so that a run is headless
//! (window off-screen, never activated, no display-mode change), muted,
//! needs no registry install or CD, and dumps the DirectDraw framebuffer to
//! PNG. The exe is never patched on disk.
//!
//! The design and several pieces are adapted from the sibling project
//! theme-park-world-recomp (`crates/tpwfix`, MIT, same owner): the proxy,
//! IAT patcher, log, audio-session mute and framebuffer conversion.
//!
//! Everything Windows-specific is behind `cfg(windows)`; on the Linux host
//! this crate builds the portable helpers only, which are unit-tested there.

pub mod addrs;
pub mod cfg;
pub mod fbconv;
pub mod keys;
pub mod trace;

#[cfg(windows)]
pub mod win;
