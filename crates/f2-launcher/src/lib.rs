//! The launcher behind `launcher` (desktop) and the Android launcher screen: one UI model
//! ([`ui`]) drawn by each host with its own renderer, plus the platform-free logic around it:
//! GitHub releases ([`releases`]), side-by-side versions ([`install`]) and mods ([`mods`]).
//!
//! Nothing here touches game data or a window: hosts feed [`ui::State`], draw through
//! [`ui::Painter`], and act on the returned [`ui::Action`]s.

pub mod install;
pub mod mods;
pub mod releases;
pub mod ui;
