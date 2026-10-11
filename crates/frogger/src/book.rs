//! The story book page and arcade screen render-to-texture: shared with the N64 in
//! `f2-draw` (re-exported); this wrapper adds the desktop's `F2_DUMP_PAGE` debug dump.

pub use f2_draw::book::*;
use f2_engine::{Font, Renderer};
use f2_game::Game;

/// [`f2_draw::book::process_grab`] plus `F2_DUMP_PAGE=<dir>` (writes each grabbed page as
/// PNG) and a log line when the page texture is not loaded.
pub fn process_grab(r: &mut Renderer, game: &mut Game, font: Option<&Font>, scale: u32) {
    let page = game.options.page_num;
    let Some((c, target, ok)) = f2_draw::book::process_grab(r, game, font, scale) else { return };
    if let Some(dir) = std::env::var_os("F2_DUMP_PAGE") {
        let img = f2_engine::RgbaImage { width: c.w, height: c.h, pixels: c.rgba };
        let _ = img.write_png(std::path::Path::new(&dir).join(format!("page{page}-{target}.png")));
    }
    if !ok {
        eprintln!("frogger: book page texture {target} not loaded");
    }
}
