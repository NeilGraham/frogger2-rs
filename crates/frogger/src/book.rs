//! Render-to-texture of the PC frontend: the story book page (`DrawPageB` grabbed into
//! `page256a/b.bmp`) and the arcade machine's screen (`CopyTexture` onto `TEST5`).
//! The original drew into the back buffer and blitted it into the texture
//! (`GrabSurfaceToTexture`, drawloop.cpp:376); here the page is composed on the CPU and
//! uploaded with `replace_image`.

use f2_engine::textures::Image;
use f2_engine::{Font, Renderer};
use f2_game::language::{c_sprintf, FmtArg};
use f2_game::menus::strs::{STR_CHAPTER, STR_CHAPTER_1a};
use f2_game::menus::{BOOK_NUM_PAGES, CHAPTER_PIC};
use f2_game::Game;

/// Size of the grabbed area (`RECT r = {0, 0, 0xff, 0xff}`) and the page textures.
pub const PAGE_SIZE: u32 = 256;

/// An RGBA8 drawing surface (the back buffer region `DrawPageB` draws into).
pub struct Canvas {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl Canvas {
    pub fn new(w: u32, h: u32) -> Canvas {
        Canvas { w, h, rgba: vec![0; (w * h * 4) as usize] }
    }

    /// Source-over blend of `src` (straight alpha) times `col` into pixel `(x, y)`.
    fn blend(&mut self, x: i32, y: i32, src: [u8; 4], col: [f32; 4]) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return;
        }
        let i = ((y as u32 * self.w + x as u32) * 4) as usize;
        let a = src[3] as f32 / 255.0 * col[3];
        if a <= 0.0 {
            return;
        }
        for k in 0..3 {
            let s = src[k] as f32 * col[k];
            let d = self.rgba[i + k] as f32;
            self.rgba[i + k] = (s * a + d * (1.0 - a)).round().clamp(0.0, 255.0) as u8;
        }
        self.rgba[i + 3] = 255;
    }

    /// `DrawTexturedRect(r, colour, tex, 0, 0, 1, 1)`: `img` stretched over
    /// `[l, r) × [t, b)` (point sampled).
    pub fn draw_textured_rect(&mut self, (l, t, r, b): (i32, i32, i32, i32), col: [f32; 4], img: &Image) {
        let (rw, rh) = ((r - l).max(1), (b - t).max(1));
        if img.width == 0 || img.height == 0 || img.rgba.len() < (img.width * img.height * 4) as usize {
            return;
        }
        for y in t..b {
            let v = (((y - t) as f32 + 0.5) / rh as f32 * img.height as f32) as u32;
            for x in l..r {
                let u = (((x - l) as f32 + 0.5) / rw as f32 * img.width as f32) as u32;
                let si = ((v.min(img.height - 1) * img.width + u.min(img.width - 1)) * 4) as usize;
                let s = [img.rgba[si], img.rgba[si + 1], img.rgba[si + 2], img.rgba[si + 3]];
                self.blend(x, y, s, col);
            }
        }
    }

    /// `DrawFontStringAtLoc(x, y, text, colour, font, scale, centredX, 0)`: with a non-zero
    /// `centred_x` the string is centred on it. Latin-1 text.
    #[allow(clippy::too_many_arguments)] // the C signature
    pub fn draw_string(&mut self, font: &Font, x: i32, y: i32, text: &str, col: [f32; 4], scale: f32, centred_x: i32) {
        let bytes: Vec<u8> = text.chars().map(|c| if (c as u32) < 256 { c as u8 } else { 0 }).collect();
        let mut cx = if centred_x != 0 { centred_x - font.string_width(&bytes, scale) / 2 } else { x } as f32;
        let gh = (font.height as f32 * scale).trunc() as i32;
        for &b in &bytes {
            let Some(g) = font.glyphs[b as usize] else { continue };
            let gw = (g.width as f32 * scale).trunc() as i32;
            // The space has no image (`left` past the font): advance only.
            let rows = if g.left < font.width { gh } else { 0 };
            for py in 0..rows {
                let sy = ((py as f32 + 0.5) / scale) as u32;
                for px in 0..gw {
                    let sx = g.left.saturating_add(((px as f32 + 0.5) / scale) as u32);
                    if sx >= font.width || sy >= font.height {
                        continue;
                    }
                    let si = ((sy * font.width + sx) * 4) as usize;
                    let s = [font.rgba[si], font.rgba[si + 1], font.rgba[si + 2], font.rgba[si + 3]];
                    self.blend(cx as i32 + px, y + py, s, col);
                }
            }
            cx += gw as f32;
        }
    }
}

/// The current image of texture `name` (`GetTexEntryFromCRC(UpdateCRC(name))->surf`).
fn tex_image<'a>(r: &'a Renderer, name: &str) -> Option<&'a Image> {
    let h = r.textures.by_name(name)?;
    r.textures.images.get(h.0 as usize)
}

/// Port of `DrawPageB` (Common/menus.c:1862, PC): the story book page for
/// `options.pageNum` — `page32.bmp`, "Chapter N", the two chapter-name lines and
/// `CHAPTERn.bmp` — into a 256×256 canvas. `font` is `fontWhite`.
pub fn draw_page_b(r: &Renderer, game: &Game, font: Option<&Font>) -> Canvas {
    let mut c = Canvas::new(PAGE_SIZE, PAGE_SIZE);
    let white = [1.0; 4];
    if let Some(img) = tex_image(r, "page32.bmp") {
        c.draw_textured_rect((0, 0, 0xff, 0xff), white, img);
    }
    let pn = game.options.page_num;
    let ink = [0.0, 0.0, 0.0, 0.5];
    if let Some(f) = font {
        let title = c_sprintf(&game.gamestring(STR_CHAPTER), &[FmtArg::Int(pn + 1)]);
        c.draw_string(f, 10, 10, &title, ink, 1.0, 0);
        c.draw_string(f, 50, 200, &game.gamestring(pn * 2 + STR_CHAPTER_1a), ink, 0.5, 128);
        c.draw_string(f, 50, 220, &game.gamestring(1 + pn * 2 + STR_CHAPTER_1a), ink, 0.5, 128);
    }
    let pic = format!("{}.bmp", CHAPTER_PIC[(pn.max(0) as usize).min(BOOK_NUM_PAGES - 1)]).to_lowercase();
    if let Some(img) = tex_image(r, &pic) {
        c.draw_textured_rect((60, 70, 0xff - 60, 0xff - 100), white, img);
    }
    c
}

/// `GrabSurfaceToTexture(0, 0, tex, surface)`: copies the top-left of the canvas into the
/// texture's image (the texture keeps its size).
pub fn grab_to_texture(r: &mut Renderer, c: &Canvas, name: &str) -> bool {
    let Some(h) = r.textures.entry(f2_formats::crc(name)).map(|e| e.image) else { return false };
    let Some(img) = r.textures.images.get(h.0 as usize) else { return false };
    let (w, hgt) = (img.width, img.height);
    let mut rgba = img.rgba.clone();
    for y in 0..hgt.min(c.h) {
        for x in 0..w.min(c.w) {
            let d = ((y * w + x) * 4) as usize;
            let s = ((y * c.w + x) * 4) as usize;
            rgba[d..d + 4].copy_from_slice(&c.rgba[s..s + 4]);
        }
    }
    r.textures.replace_image(h, w, hgt, rgba);
    true
}

/// PC part of `DrawLoop` after the flip (drawloop.cpp:376): `grabToTexture` 1 draws the
/// page into `page256b.bmp` (→ 0), 3 into `page256a.bmp` (→ 2).
pub fn process_grab(r: &mut Renderer, game: &mut Game, font: Option<&Font>) {
    let target = match game.options.grab_to_texture {
        1 => "page256b.bmp",
        3 => "page256a.bmp",
        _ => return,
    };
    let c = draw_page_b(r, game, font);
    if let Some(dir) = std::env::var_os("F2_DUMP_PAGE") {
        let img = f2_engine::RgbaImage { width: c.w, height: c.h, pixels: c.rgba.clone() };
        let _ = img.write_png(std::path::Path::new(&dir).join(format!("page{}-{}.png", game.options.page_num, target)));
    }
    if !grab_to_texture(r, &c, target) {
        eprintln!("frogger: book page texture {target} not loaded");
    }
    game.options.grab_to_texture = if game.options.grab_to_texture == 1 { 0 } else { 2 };
}

/// `CopyTexture(arcadeScreenTex, FindTexture(name), 1)` (PC/pcmisc.cpp:403, hardware
/// `BltFast`): the source image replaces the destination's. Returns false when either
/// texture is missing.
pub fn copy_texture(r: &mut Renderer, dest: &str, src: &str) -> bool {
    let find = |n: &str| f2_game::specfx::find_texture(n);
    let Some(d) = r.textures.entry(find(dest)).map(|e| e.image) else { return false };
    let Some(s) = r.textures.current(find(src)) else { return false };
    if s == d {
        return true;
    }
    let img = r.textures.images[s.0 as usize].clone();
    r.textures.replace_image(d, img.width, img.height, img.rgba);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn textured_rect_stretches_and_blends() {
        let mut c = Canvas::new(4, 4);
        let img = Image { width: 1, height: 1, rgba: vec![200, 100, 50, 255] };
        c.draw_textured_rect((0, 0, 2, 2), [1.0; 4], &img);
        assert_eq!(&c.rgba[0..4], &[200, 100, 50, 255]);
        assert_eq!(&c.rgba[(2 * 4) as usize..(2 * 4 + 4) as usize], &[0, 0, 0, 0]);
        // Half-alpha black ink halves the colour.
        let ink = Image { width: 1, height: 1, rgba: vec![255, 255, 255, 255] };
        c.draw_textured_rect((0, 0, 1, 1), [0.0, 0.0, 0.0, 0.5], &ink);
        assert_eq!(&c.rgba[0..4], &[100, 50, 25, 255]);
    }
}
