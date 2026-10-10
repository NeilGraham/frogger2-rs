//! Render-to-texture of the PC frontend: the story book page (`DrawPageB` grabbed into
//! `page256a/b.bmp`) and the arcade machine's screen (`CopyTexture` onto `TEST5`).
//! The original drew into the back buffer and blitted it into the texture
//! (`GrabSurfaceToTexture`, drawloop.cpp:376); here the page is composed on the CPU and
//! uploaded with `replace_image`.

use f2_engine::textures::Image;
use f2_engine::{Font, Renderer};
use f2_game::language::{c_sprintf, FmtArg};
use f2_game::menus::strs::{STR_CHAPTER_1a, STR_CHAPTER};
use f2_game::menus::{BOOK_NUM_PAGES, CHAPTER_PIC};
use f2_game::Game;

/// Size of the grabbed area (`RECT r = {0, 0, 0xff, 0xff}`) and the page textures.
pub const PAGE_SIZE: u32 = 256;

/// Resolution multiplier of the composed page in the enhanced profile (the authentic
/// profile composes at the original 256×256).
pub const ENHANCED_SCALE: u32 = 4;

/// An RGBA8 drawing surface (the back buffer region `DrawPageB` draws into). Drawing
/// calls take the original's pixel coordinates; `scale` is the resolution multiplier
/// (1 = the original 256×256 page).
pub struct Canvas {
    pub w: u32,
    pub h: u32,
    pub scale: u32,
    pub rgba: Vec<u8>,
}

/// Premultiplied RGBA in 0..1.
type Px = [f32; 4];

/// Texel `(x, y)` of `rgba` (`w` wide, straight alpha) as premultiplied floats.
fn texel(rgba: &[u8], w: u32, x: u32, y: u32) -> Px {
    let i = ((y * w + x) * 4) as usize;
    let a = rgba[i + 3] as f32 / 255.0;
    [rgba[i] as f32 / 255.0 * a, rgba[i + 1] as f32 / 255.0 * a, rgba[i + 2] as f32 / 255.0 * a, a]
}

/// Bilinear filter (`D3DTFN_LINEAR`, the MDX default: mdxD3D.cpp `BILERP_DEFAULT`) at
/// texel-space `(u, v)` (texel centres at `n + 0.5`), premultiplied. `fetch` returns the
/// texel at integer coordinates (it does the addressing).
fn bilinear(u: f32, v: f32, fetch: impl Fn(i32, i32) -> Px) -> Px {
    let (fu, fv) = (u - 0.5, v - 0.5);
    let (x0, y0) = (fu.floor(), fv.floor());
    let (tx, ty) = (fu - x0, fv - y0);
    let (x0, y0) = (x0 as i32, y0 as i32);
    let (a, b, c, d) = (fetch(x0, y0), fetch(x0 + 1, y0), fetch(x0, y0 + 1), fetch(x0 + 1, y0 + 1));
    let mut o = [0.0; 4];
    for k in 0..4 {
        let top = a[k] + (b[k] - a[k]) * tx;
        let bot = c[k] + (d[k] - c[k]) * tx;
        o[k] = top + (bot - top) * ty;
    }
    o
}

impl Canvas {
    #[cfg(test)]
    pub fn new(w: u32, h: u32) -> Canvas {
        Canvas::scaled(w, h, 1)
    }

    /// A `w`×`h` (original pixels) canvas stored at `scale`× resolution.
    pub fn scaled(w: u32, h: u32, scale: u32) -> Canvas {
        let scale = scale.max(1);
        Canvas { w: w * scale, h: h * scale, scale, rgba: vec![0; (w * h * scale * scale * 4) as usize] }
    }

    /// Source-over blend of the premultiplied `src` modulated by `col`
    /// (`D3DTOP_MODULATE` on colour and alpha, then `SRCALPHA`/`INVSRCALPHA`) into
    /// canvas pixel `(x, y)`.
    fn blend(&mut self, x: i32, y: i32, src: Px, col: [f32; 4]) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return;
        }
        let i = ((y as u32 * self.w + x as u32) * 4) as usize;
        let a = src[3] * col[3];
        if a <= 0.0 {
            return;
        }
        for k in 0..3 {
            let s = src[k] * col[k] * col[3] * 255.0;
            let d = self.rgba[i + k] as f32;
            self.rgba[i + k] = (s + d * (1.0 - a)).round().clamp(0.0, 255.0) as u8;
        }
        self.rgba[i + 3] = 255;
    }

    /// `DrawTexturedRect(r, colour, tex, 0, 0, 1, 1)`: `img` stretched over
    /// `[l, r) × [t, b)` (original pixels), bilinear filtered with clamped addressing.
    pub fn draw_textured_rect(&mut self, (l, t, r, b): (i32, i32, i32, i32), col: [f32; 4], img: &Image) {
        if img.width == 0 || img.height == 0 || img.rgba.len() < (img.width * img.height * 4) as usize {
            return;
        }
        let s = self.scale as i32;
        let (l, t, r, b) = (l * s, t * s, r * s, b * s);
        let (rw, rh) = ((r - l).max(1) as f32, (b - t).max(1) as f32);
        let (iw, ih) = (img.width as i32, img.height as i32);
        let fetch = |x: i32, y: i32| texel(&img.rgba, img.width, x.clamp(0, iw - 1) as u32, y.clamp(0, ih - 1) as u32);
        for y in t..b {
            let v = ((y - t) as f32 + 0.5) / rh * ih as f32;
            for x in l..r {
                let u = ((x - l) as f32 + 0.5) / rw * iw as f32;
                let p = bilinear(u, v, fetch);
                self.blend(x, y, p, col);
            }
        }
    }

    /// `DrawFontStringAtLoc(x, y, text, colour, font, scale, centredX, 0)`: with a non-zero
    /// `centred_x` the string is centred on it. Latin-1 text. Each glyph is a colour-keyed
    /// rect (`DrawFontCharAtLoc`: `width*scale` × `height*scale`, truncated) modulated by
    /// `col`, sampled bilinearly inside its own cell (the font atlas keeps transparent
    /// texels around it). Above 1× canvas resolution the filtered coverage is
    /// re-sharpened to a one-pixel edge so magnified glyphs stay crisp.
    #[allow(clippy::too_many_arguments)] // the C signature
    pub fn draw_string(&mut self, font: &Font, x: i32, y: i32, text: &str, col: [f32; 4], scale: f32, centred_x: i32) {
        let bytes: Vec<u8> = text.chars().map(|c| if (c as u32) < 256 { c as u8 } else { 0 }).collect();
        let mut cx = if centred_x != 0 { centred_x - font.string_width(&bytes, scale) / 2 } else { x };
        let s = self.scale as i32;
        let gh = (font.height as f32 * scale) as i32;
        for &b in &bytes {
            let Some(g) = font.glyphs[b as usize] else { continue };
            let gw = (g.width as f32 * scale) as i32;
            // The space has no image (`left` past the font): advance only.
            if g.left < font.width && gw > 0 && gh > 0 {
                let (pw, ph) = (gw * s, gh * s);
                let (left, right) = (g.left as i32, (g.left + g.width).min(font.width) as i32);
                let fh = font.height as i32;
                let fetch = |fx: i32, fy: i32| {
                    if fx < left || fx >= right || fy < 0 || fy >= fh {
                        [0.0; 4]
                    } else {
                        texel(&font.rgba, font.width, fx as u32, fy as u32)
                    }
                };
                // Canvas pixels per font texel (> 1: sharpen the bilinear ramp).
                let mag = (pw as f32 / g.width as f32).min(ph as f32 / font.height as f32);
                let sharpen = self.scale > 1 && mag > 1.0;
                for py in 0..ph {
                    let v = (py as f32 + 0.5) / ph as f32 * font.height as f32;
                    for px in 0..pw {
                        let u = left as f32 + (px as f32 + 0.5) / pw as f32 * g.width as f32;
                        let mut p = bilinear(u, v, fetch);
                        if sharpen && p[3] > 0.0 {
                            let a = ((p[3] - 0.5) * mag + 0.5).clamp(0.0, 1.0);
                            let k = a / p[3];
                            p = [p[0] * k, p[1] * k, p[2] * k, a];
                        }
                        self.blend(cx * s + px, y * s + py, p, col);
                    }
                }
            }
            cx += gw;
        }
    }

    /// Fills the pixels past the drawn `[0, drawn)` square (the last original row and
    /// column, which `DrawPageB` leaves holding whatever was in the back buffer) from
    /// their neighbours, so the filtered page texture has no dark seam.
    fn extend_edges(&mut self, drawn: u32) {
        let lim = (drawn * self.scale).min(self.w).min(self.h);
        if lim == 0 {
            return;
        }
        for y in 0..self.h {
            for x in 0..self.w {
                if x < lim && y < lim {
                    continue;
                }
                let (sx, sy) = (x.min(lim - 1), y.min(lim - 1));
                let (d, s) = (((y * self.w + x) * 4) as usize, ((sy * self.w + sx) * 4) as usize);
                if self.rgba[d + 3] == 0 {
                    self.rgba.copy_within(s..s + 4, d);
                }
            }
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
/// `CHAPTERn.bmp` — into a 256×256 canvas (`scale`× resolution).
///
/// `font` is `fontWhite`, undefined in the preserved source; the retail exe's `InitFont`
/// sequence (0x43ce10) loads `bigfont`/`smallfont`/`monofont` and sets
/// `fontWhite = monofont.bmp`. The ink is `D3DRGBA(0,0,0,0.5)` (half-transparent black
/// over the white glyphs). Only the book's right page (`BookR` in start.dll, mapped to
/// `page256b.bmp` u 0.21..1, which crops the left of "Chapter N") and the back of the
/// turning page (`Pagehrt`, `page256a.bmp`) show this texture; the left page (`BookL`)
/// is plain `page32.bmp`.
pub fn draw_page_b(r: &Renderer, game: &Game, font: Option<&Font>, scale: u32) -> Canvas {
    let mut c = Canvas::scaled(PAGE_SIZE, PAGE_SIZE, scale);
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
    c.extend_edges(0xff);
    c
}

/// `GrabSurfaceToTexture(0, 0, tex, surface)`: the canvas (the 256×256 grabbed area,
/// the page textures' own size) replaces the texture's image; a canvas composed above
/// 1× gives the texture that higher resolution (enhanced profile).
pub fn grab_to_texture(r: &mut Renderer, c: &Canvas, name: &str) -> bool {
    let Some(h) = r.textures.entry(f2_formats::crc(name)).map(|e| e.image) else { return false };
    if r.textures.images.get(h.0 as usize).is_none() {
        return false;
    }
    r.textures.replace_image(h, c.w, c.h, c.rgba.clone());
    true
}

/// PC part of `DrawLoop` after the flip (drawloop.cpp:376): `grabToTexture` 1 draws the
/// page into `page256b.bmp` (→ 0), 3 into `page256a.bmp` (→ 2). `scale` is the page
/// resolution multiplier (1 authentic, [`ENHANCED_SCALE`] enhanced).
pub fn process_grab(r: &mut Renderer, game: &mut Game, font: Option<&Font>, scale: u32) {
    let target = match game.options.grab_to_texture {
        1 => "page256b.bmp",
        3 => "page256a.bmp",
        _ => return,
    };
    let c = draw_page_b(r, game, font, scale);
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

    /// A 2-glyph font: 'A' = a 2×2 white block at columns 1-2, 'B' at column 4.
    fn tiny_font() -> Font {
        let w = 6;
        let mut px = vec![0u16; w * 4];
        for y in 1..3 {
            for &x in &[1usize, 2, 4] {
                px[y * w + x] = 0x7fff;
            }
        }
        Font::from_pixels(w as u32, 4, &px)
    }

    fn paper(c: &mut Canvas) {
        for p in c.rgba.chunks_mut(4) {
            p.copy_from_slice(&[200, 160, 80, 255]);
        }
    }

    #[test]
    fn glyph_ink_keys_the_background() {
        let f = tiny_font();
        let mut c = Canvas::new(8, 4);
        paper(&mut c);
        c.draw_string(&f, 0, 0, "A", [0.0, 0.0, 0.0, 0.5], 1.0, 0);
        let px = |c: &Canvas, x: u32, y: u32| c.rgba[((y * c.w + x) * 4) as usize..][..4].to_vec();
        // Transparent texels leave the paper alone (no block behind the glyph) ...
        assert_eq!(px(&c, 0, 0), vec![200, 160, 80, 255]);
        assert_eq!(px(&c, 1, 0), vec![200, 160, 80, 255]);
        // ... ink texels darken it by half.
        assert_eq!(px(&c, 1, 1), vec![100, 80, 40, 255]);
    }

    #[test]
    fn scaled_canvas_keeps_layout_and_sharp_glyphs() {
        let f = tiny_font();
        let mut c = Canvas::scaled(8, 4, 4);
        assert_eq!((c.w, c.h), (32, 16));
        paper(&mut c);
        c.draw_string(&f, 0, 0, "A", [0.0, 0.0, 0.0, 1.0], 1.0, 0);
        let px = |x: u32, y: u32| c.rgba[((y * c.w + x) * 4) as usize];
        // The glyph is drawn at x 0..2, ink rows 1-2 of the 1× layout: 0..8 × 4..12 here.
        assert_eq!(px(1, 1), 200);
        assert_eq!(px(4, 8), 0);
        assert_eq!(px(12, 8), 200);
        // The edge is a short ramp, not a 4-pixel blur.
        let ramp = (0..16).filter(|&x| (1..200).contains(&px(x, 8))).count();
        assert!(ramp <= 2, "ramp {ramp}");
    }
}
