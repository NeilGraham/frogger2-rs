//! Port of the state of `PC/backdrop.c` (full-screen bitmap backdrops and the loading
//! screen) and of the tiled backdrop of `PC/pcsprite.c` (`InitTiledBackdrop`), plus the
//! `drawGame` / `drawLandscape` switches the frontend flips. The renderer reads these.

use serde::{Deserialize, Serialize};

use crate::Game;

/// What the renderer should show behind / instead of the game.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackdropState {
    /// `mdxLoadBackdrop(path)`: the full-screen backdrop file (`textures\<name>.fla`), if any.
    pub backdrop: Option<String>,
    /// `tileTexture[0..4]` set by `InitTiledBackdrop(name)`: textures `<name>01`..`<name>04`
    /// scrolled across the screen (`DrawTiledBackdrop`); `None` after `FreeTiledBackdrop`.
    pub tiled_backdrop: Option<String>,
    /// `loadProgress` of the loading screen (0-100).
    pub load_progress: i16,
    /// `drawGame` (`PC/drawloop.cpp`): draw the 3D scene at all.
    pub draw_game: bool,
    /// `drawLandscape`: draw the level landscape.
    pub draw_landscape: bool,
}

impl Default for BackdropState {
    fn default() -> Self {
        BackdropState { backdrop: None, tiled_backdrop: None, load_progress: 0, draw_game: true, draw_landscape: true }
    }
}

impl Game {
    /// Port of `InitBackdrop(filename)` (PC/backdrop.c:27): `baseDirectory\textures\<name>.fla`.
    pub fn init_backdrop(&mut self, filename: &str) -> i32 {
        self.backdrop.backdrop = Some(format!("textures\\{filename}.fla"));
        1
    }

    /// Port of `FreeBackdrop` (PC/backdrop.c:84).
    pub fn free_backdrop(&mut self) {
        self.backdrop.backdrop = None;
    }

    /// Port of `InitLoadingScreen(filename)` (PC/backdrop.c:111, non-demo build).
    pub fn init_loading_screen(&mut self, filename: &str) {
        self.init_backdrop(filename);
        self.backdrop.load_progress = 0;
    }

    /// The state part of `UpdateLoadingScreen(addprog)` (PC/backdrop.c:132).
    pub fn update_loading_screen(&mut self, addprog: i16) {
        self.backdrop.load_progress = self.backdrop.load_progress.wrapping_add(addprog);
        if self.backdrop.load_progress > 100 {
            self.backdrop.load_progress = 100;
        }
    }

    /// Port of `FreeLoadingScreen` (PC/backdrop.c:174).
    pub fn free_loading_screen(&mut self) {
        self.free_backdrop();
    }

    /// Port of `InitTiledBackdrop(filename)` (PC/pcsprite.c:414).
    pub fn init_tiled_backdrop(&mut self, filename: &str) {
        self.backdrop.tiled_backdrop = Some(filename.to_string());
    }

    /// Port of `FreeTiledBackdrop` (PC/pcsprite.c).
    pub fn free_tiled_backdrop(&mut self) {
        self.backdrop.tiled_backdrop = None;
    }
}
