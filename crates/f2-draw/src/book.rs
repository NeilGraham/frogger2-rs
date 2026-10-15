//! Render-to-texture of the PC frontend: the story book page (`DrawPageB` grabbed into
//! `page256a/b.bmp`) and the arcade machine's screen (`CopyTexture` onto `TEST5`).
//! The original drew into the back buffer and blitted it into the texture
//! (`GrabSurfaceToTexture`, drawloop.cpp:376); here the page is composed on the CPU and
//! uploaded with `replace_image`.

// Newer `core` has some of these methods inherently (then the trait goes unused).
#[cfg(not(feature = "std"))]
#[allow(unused_imports)]
use crate::fmath::FloatExt;
use crate::prelude::*;
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
    /// The stored rows `y0..y0 + rows` of the `w`×`h` page (all of it, except for a strip
    /// canvas: a console composes the page a strip at a time, see [`Canvas::strip`]); drawing
    /// outside them is clipped.
    pub y0: u32,
    pub rows: u32,
    pub rgba: Vec<u8>,
}

/// Premultiplied RGBA in 0..1.
type Px = [f32; 4];

/// Texel `(x, y)` of `rgba` (`w` wide, straight alpha) as premultiplied floats.
/// `v as f32 / 255.0` for every byte (a table: float division is slow on console CPUs; the
/// values are the same).
/// Computed at compile time (IEEE arithmetic, so the same values; a lazily initialised
/// table cost an atomic check per lookup).
static UNIT: [f32; 256] = {
    let mut t = [0.0f32; 256];
    let mut i = 0;
    while i < 256 {
        t[i] = i as f32 / 255.0;
        i += 1;
    }
    t
};

fn texel(rgba: &[u8], w: u32, x: u32, y: u32) -> Px {
    let i = ((y * w + x) * 4) as usize;
    let u = &UNIT;
    let a = u[rgba[i + 3] as usize];
    [u[rgba[i] as usize] * a, u[rgba[i + 1] as usize] * a, u[rgba[i + 2] as usize] * a, a]
}

/// `v.floor()` for |v| < 2^31 (exact; without `std` `f32::floor` is a software routine).
#[inline]
fn floor_f32(v: f32) -> f32 {
    let t = v as i32;
    (if t as f32 > v { t - 1 } else { t }) as f32
}

/// `v.round()` (half away from zero) for 0 <= v < 2^23 (exact, as [`floor_f32`]).
#[inline]
fn round_nonneg(v: f32) -> f32 {
    let t = v as i32;
    (if v - t as f32 >= 0.5 { t + 1 } else { t }) as f32
}

/// Bilinear filter (`D3DTFN_LINEAR`, the MDX default: mdxD3D.cpp `BILERP_DEFAULT`) at
/// texel-space `(u, v)` (texel centres at `n + 0.5`), premultiplied. `fetch` returns the
/// texel at integer coordinates (it does the addressing).
fn bilinear(u: f32, v: f32, fetch: impl Fn(i32, i32) -> Px) -> Px {
    let (fu, fv) = (u - 0.5, v - 0.5);
    let (x0, y0) = (floor_f32(fu), floor_f32(fv));
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

/// A source image region for [`Canvas::blit_fixed`]: `clamp` addressing (images) or
/// transparent outside columns `u0..u0 + uw` (a glyph's cell in the font atlas).
struct Src<'a> {
    rgba: &'a [u8],
    w: u32,
    h: u32,
    u0: u32,
    uw: u32,
    clamp: bool,
}

impl Canvas {
    #[cfg(test)]
    pub fn new(w: u32, h: u32) -> Canvas {
        Canvas::scaled(w, h, 1)
    }

    /// A `w`×`h` (original pixels) canvas stored at `scale`× resolution.
    pub fn scaled(w: u32, h: u32, scale: u32) -> Canvas {
        let scale = scale.max(1);
        Canvas { w: w * scale, h: h * scale, scale, y0: 0, rows: h * scale, rgba: vec![0; (w * h * scale * scale * 4) as usize] }
    }

    /// A `w`×`h` page canvas (1×) storing `rows` rows at a time; [`Canvas::window`] moves it.
    pub fn strip(w: u32, h: u32, rows: u32) -> Canvas {
        let rows = rows.min(h);
        Canvas { w, h, scale: 1, y0: 0, rows, rgba: vec![0; (w * rows * 4) as usize] }
    }

    /// Stores rows `y0..y0 + rows` from now on (the contents are not kept).
    pub fn window(&mut self, y0: u32) {
        self.y0 = y0.min(self.h - self.rows);
    }

    /// Whether canvas row `y` is stored.
    #[inline]
    fn has_row(&self, y: i32) -> bool {
        y >= self.y0 as i32 && y < (self.y0 + self.rows) as i32
    }

    /// Source-over blend of the premultiplied `src` modulated by `col`
    /// (`D3DTOP_MODULATE` on colour and alpha, then `SRCALPHA`/`INVSRCALPHA`) into
    /// canvas pixel `(x, y)`.
    fn blend(&mut self, x: i32, y: i32, src: Px, col: [f32; 4]) {
        if x < 0 || x >= self.w as i32 || !self.has_row(y) {
            return;
        }
        let i = (((y as u32 - self.y0) * self.w + x as u32) * 4) as usize;
        let a = src[3] * col[3];
        if a <= 0.0 {
            return;
        }
        for k in 0..3 {
            let s = src[k] * col[k] * col[3] * 255.0;
            let d = self.rgba[i + k] as f32;
            // s, d >= 0 and a <= 1: never negative (round_nonneg's range).
            self.rgba[i + k] = round_nonneg(s + d * (1.0 - a)).clamp(0.0, 255.0) as u8;
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
        if cfg!(feature = "lean") {
            self.textured_rect_fixed((l, t, r, b), col, img);
            return;
        }
        let fetch = |x: i32, y: i32| texel(&img.rgba, img.width, x.clamp(0, iw - 1) as u32, y.clamp(0, ih - 1) as u32);
        // `bilinear(u, v)` with its per-column / per-row parts computed once (the same
        // operations, so the same pixels; the per-pixel divisions and floors were most of a
        // page's cost on the N64).
        let split = |p: f32| {
            let f = p - 0.5;
            let i = floor_f32(f);
            (i as i32, f - i)
        };
        let cols: Vec<(i32, f32)> = (l..r).map(|x| split(((x - l) as f32 + 0.5) / rw * iw as f32)).collect();
        for y in t..b {
            if !self.has_row(y) {
                continue;
            }
            let (y0, ty) = split(((y - t) as f32 + 0.5) / rh * ih as f32);
            for (x, &(x0, tx)) in (l..r).zip(&cols) {
                let (a, b2, c, d) = (fetch(x0, y0), fetch(x0 + 1, y0), fetch(x0, y0 + 1), fetch(x0 + 1, y0 + 1));
                let mut p = [0.0; 4];
                for k in 0..4 {
                    let top = a[k] + (b2[k] - a[k]) * tx;
                    let bot = c[k] + (d[k] - c[k]) * tx;
                    p[k] = top + (bot - top) * ty;
                }
                self.blend(x, y, p, col);
            }
        }
    }

    /// [`draw_textured_rect`](Self::draw_textured_rect) in integer maths (`lean`: the N64,
    /// where the float version took ~0.7 s for a page background): 8-bit filter weights,
    /// premultiplied 8-bit texels; within 2 of the float result per channel. `(l, t, r, b)`
    /// in canvas pixels.
    fn textured_rect_fixed(&mut self, (l, t, r, b): (i32, i32, i32, i32), col: [f32; 4], img: &Image) {
        let src = Src { rgba: &img.rgba, w: img.width, h: img.height, u0: 0, uw: img.width, clamp: true };
        self.blit_fixed((l, t, r - l, b - t), &src, col);
    }

    /// Bilinear blit of `src` (texels `u0..u0 + uw` across, the whole height down) onto the
    /// canvas pixels `(x, y, w, h)`, blended with `col` (the integer version of the float
    /// `bilinear` + `blend` loops; see [`textured_rect_fixed`](Self::textured_rect_fixed)).
    fn blit_fixed(&mut self, (l, t, w, h): (i32, i32, i32, i32), src: &Src, col: [f32; 4]) {
        if w <= 0 || h <= 0 || src.w == 0 || src.h == 0 {
            return;
        }
        let (rw, rh) = (w as f32, h as f32);
        let (lo, hi) = if src.clamp { (0, src.w as i32 - 1) } else { (src.u0 as i32, (src.u0 + src.uw).min(src.w) as i32 - 1) };
        // Texel index (None: transparent, outside a glyph cell) and 8-bit weight of the next.
        let split = |p: f32| {
            let f = p - 0.5;
            let i = floor_f32(f);
            (i as i32, ((f - i) * 256.0) as u32)
        };
        let idx = |i: i32, lo: i32, hi: i32| -> Option<u32> {
            if src.clamp {
                Some(i.clamp(lo, hi) as u32)
            } else {
                (i >= lo && i <= hi).then_some(i as u32)
            }
        };
        let cols: Vec<(Option<u32>, Option<u32>, u32)> = (0..w)
            .map(|x| {
                let (x0, wx) = split(src.u0 as f32 + (x as f32 + 0.5) / rw * src.uw as f32);
                (idx(x0, lo, hi), idx(x0 + 1, lo, hi), wx)
            })
            .collect();
        let k = |v: f32| (v.clamp(0.0, 1.0) * 256.0 + 0.5) as u32;
        let (cm, ca) = ([k(col[0] * col[3]), k(col[1] * col[3]), k(col[2] * col[3])], k(col[3]));
        // Premultiplied texels (c * a / 255 rounded), once per blit: integer multiplies cost
        // ~5 cycles each on the VR4300.
        // Only the columns read (a glyph's cell of the font atlas, or the whole image).
        let pw = (hi - lo + 1).max(0) as u32;
        let pm: Vec<[u8; 4]> = (0..src.h)
            .flat_map(|row| (lo as u32..lo as u32 + pw).map(move |c| ((row * src.w + c) * 4) as usize))
            .map(|i| {
                let p = &src.rgba[i..i + 4];
                let a = p[3] as u32;
                let m = |c: u8| {
                    let v = c as u32 * a + 128;
                    ((v + (v >> 8)) >> 8) as u8
                };
                [m(p[0]), m(p[1]), m(p[2]), p[3]]
            })
            .collect();
        let tex = |row: Option<u32>, c: Option<u32>| -> [u32; 4] {
            match (row, c) {
                (Some(row), Some(c)) => pm[(row * pw + c - lo as u32) as usize].map(|v| v as u32),
                _ => [0; 4],
            }
        };
        // Horizontal lerps of the current texel row pair (0..255 << 8), recomputed only when
        // the pair changes (every ~8 rows when magnifying 8x).
        let mut line: Vec<([u32; 4], [u32; 4])> = alloc::vec![([0; 4], [0; 4]); w as usize];
        let mut line_rows = None;
        let (vlo, vhi) = (0, src.h as i32 - 1);
        for y in 0..h {
            let cy = t + y;
            if !self.has_row(cy) {
                continue;
            }
            let (y0, wy) = split((y as f32 + 0.5) / rh * src.h as f32);
            let rows = (idx(y0, vlo, vhi), idx(y0 + 1, vlo, vhi));
            if line_rows != Some(rows) {
                line_rows = Some(rows);
                for (o, &(xa, xb, wx)) in line.iter_mut().zip(&cols) {
                    let (pa, pb, pc, pd) = (tex(rows.0, xa), tex(rows.0, xb), tex(rows.1, xa), tex(rows.1, xb));
                    for k in 0..4 {
                        o.0[k] = pa[k] * (256 - wx) + pb[k] * wx;
                        o.1[k] = pc[k] * (256 - wx) + pd[k] * wx;
                    }
                }
            }
            let row = ((cy as u32 - self.y0) * self.w) as usize;
            for (x, (top, bot)) in line.iter().enumerate() {
                let cx = l + x as i32;
                if cx < 0 || cx >= self.w as i32 {
                    continue;
                }
                // 0..255 << 16.
                let lerp = |k: usize| top[k] * (256 - wy) + bot[k] * wy;
                let a = (lerp(3) * ca) >> 16; // 0..255 << 8
                if a == 0 {
                    continue;
                }
                let i = (row + cx as usize) * 4;
                let inv = (255 << 8) - a.min(255 << 8);
                #[allow(clippy::needless_range_loop)] // c also indexes lerp() and the pixel
                for c in 0..3 {
                    let s = (lerp(c) >> 8) * cm[c]; // 0..255 << 16
                                                    // dst * (1 - alpha) << 16 = dst * inv * 256 / 255 (inv is (1 - alpha) * 255 << 8);
                                                    // * 256 / 255 as d + d/256 + d/65536 (no division).
                    let d = if inv == 0 { 0 } else { self.rgba[i + c] as u32 * inv };
                    let d = d + (d >> 8) + (d >> 16);
                    let v = (s + d + (1 << 15)) >> 16;
                    self.rgba[i + c] = v.min(255) as u8;
                }
                self.rgba[i + 3] = 255;
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
                if cfg!(feature = "lean") && self.scale == 1 {
                    let src = Src { rgba: &font.rgba, w: font.width, h: font.height, u0: left as u32, uw: g.width, clamp: false };
                    self.blit_fixed((cx * s, y * s, pw, ph), &src, col);
                    cx += gw;
                    continue;
                }
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
        for y in self.y0..self.y0 + self.rows {
            for x in 0..self.w {
                if x < lim && y < lim {
                    continue;
                }
                let (sx, sy) = (x.min(lim - 1), y.min(lim - 1));
                if !self.has_row(sy as i32) {
                    continue; // the source row is in another strip (never for DrawPageB's)
                }
                let (y, sy) = (y - self.y0, sy - self.y0);
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
    draw_page_b_into(r, game, font, &mut c);
    c
}

/// [`draw_page_b`] into an existing canvas (cleared first; its size and scale are kept),
/// for hosts that keep one page buffer instead of allocating 256 KiB per grab (the N64).
pub fn draw_page_b_into(r: &Renderer, game: &Game, font: Option<&Font>, c: &mut Canvas) {
    c.rgba.fill(0);
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
}

/// An empty page canvas (`PAGE_SIZE` square at `scale`), for [`draw_page_b_into`].
pub fn page_canvas(scale: u32) -> Canvas {
    Canvas::scaled(PAGE_SIZE, PAGE_SIZE, scale)
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

/// The page texture `grabToTexture` asks for this frame (`1` → `page256b.bmp`, `3` →
/// `page256a.bmp`; drawloop.cpp:376), if any.
pub fn grab_target(game: &Game) -> Option<&'static str> {
    match game.options.grab_to_texture {
        1 => Some("page256b.bmp"),
        3 => Some("page256a.bmp"),
        _ => None,
    }
}

/// PC part of `DrawLoop` after the flip (drawloop.cpp:376): `grabToTexture` 1 draws the
/// page into `page256b.bmp` (→ 0), 3 into `page256a.bmp` (→ 2). `scale` is the page
/// resolution multiplier (1 authentic, [`ENHANCED_SCALE`] enhanced). Returns the composed
/// page and whether the target texture existed (`None`: nothing to grab).
pub fn process_grab(r: &mut Renderer, game: &mut Game, font: Option<&Font>, scale: u32) -> Option<(Canvas, &'static str, bool)> {
    let target = grab_target(game)?;
    let c = draw_page_b(r, game, font, scale);
    let ok = grab_to_texture(r, &c, target);
    game.options.grab_to_texture = if game.options.grab_to_texture == 1 { 0 } else { 2 };
    Some((c, target, ok))
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

    /// `draw_textured_rect`'s hoisted loop draws the same bytes as per-pixel `bilinear`.
    #[test]
    #[cfg(not(feature = "lean"))]
    fn textured_rect_matches_per_pixel_bilinear() {
        let (iw, ih) = (32u32, 32u32);
        let rgba: Vec<u8> = (0..iw * ih).flat_map(|i| [(i * 37) as u8, (i * 91 + 3) as u8, (i * 13 + 200) as u8, (i * 7 + 100) as u8]).collect();
        let img = Image { width: iw, height: ih, rgba: rgba.clone() };
        for (rect, col) in [((0, 0, 0xff, 0xff), [1.0; 4]), ((60, 70, 0xff - 60, 0xff - 100), [0.8, 0.6, 1.0, 0.5])] {
            let mut fast = Canvas::new(256, 256);
            fast.rgba.iter_mut().enumerate().for_each(|(i, b)| *b = (i * 31) as u8);
            let mut slow = Canvas::new(256, 256);
            slow.rgba.copy_from_slice(&fast.rgba);
            fast.draw_textured_rect(rect, col, &img);
            let (l, t, r, b) = rect;
            let (rw, rh) = ((r - l) as f32, (b - t) as f32);
            let fetch = |x: i32, y: i32| texel(&rgba, iw, x.clamp(0, iw as i32 - 1) as u32, y.clamp(0, ih as i32 - 1) as u32);
            for y in t..b {
                let v = ((y - t) as f32 + 0.5) / rh * ih as f32;
                for x in l..r {
                    let u = ((x - l) as f32 + 0.5) / rw * iw as f32;
                    slow.blend(x, y, bilinear(u, v, fetch), col);
                }
            }
            assert!(fast.rgba == slow.rgba, "{rect:?}");
        }
    }

    /// The `lean` integer path stays within 2 of the float path per channel.
    #[test]
    fn textured_rect_fixed_close_to_float() {
        let (iw, ih) = (32u32, 32u32);
        let rgba: Vec<u8> = (0..iw * ih).flat_map(|i| [(i * 37) as u8, (i * 91 + 3) as u8, (i * 13 + 200) as u8, (i * 7 + 100) as u8]).collect();
        let img = Image { width: iw, height: ih, rgba };
        for (rect, col) in [((0, 0, 0xff, 0xff), [1.0; 4]), ((60, 70, 0xff - 60, 0xff - 100), [0.8, 0.6, 1.0, 0.5])] {
            let mut fixed = Canvas::new(256, 256);
            fixed.rgba.iter_mut().enumerate().for_each(|(i, b)| *b = (i * 31) as u8);
            let mut float = Canvas::new(256, 256);
            float.rgba.copy_from_slice(&fixed.rgba);
            fixed.textured_rect_fixed(rect, col, &img);
            if !cfg!(feature = "lean") {
                float.draw_textured_rect(rect, col, &img);
            }
            let worst = fixed.rgba.iter().zip(&float.rgba).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
            if !cfg!(feature = "lean") {
                assert!(worst <= 2, "{rect:?}: max difference {worst}");
            }
        }
    }

    /// Composing in strips gives the whole canvas's bytes, strip by strip.
    #[test]
    fn strips_match_whole_canvas() {
        let (iw, ih) = (32u32, 32u32);
        let rgba: Vec<u8> = (0..iw * ih).flat_map(|i| [(i * 37) as u8, (i * 91 + 3) as u8, (i * 13 + 200) as u8, (i * 7 + 100) as u8]).collect();
        let img = Image { width: iw, height: ih, rgba };
        let draw = |c: &mut Canvas| {
            c.rgba.fill(0);
            c.draw_textured_rect((0, 0, 0xff, 0xff), [1.0; 4], &img);
            c.draw_textured_rect((60, 70, 0xff - 60, 0xff - 100), [0.8, 0.6, 1.0, 0.5], &img);
            c.extend_edges(0xff);
        };
        let mut whole = Canvas::new(256, 256);
        draw(&mut whole);
        let mut strip = Canvas::strip(256, 256, 64);
        for k in 0..4 {
            strip.window(k * 64);
            draw(&mut strip);
            let n = (256 * 64 * 4) as usize;
            assert!(strip.rgba == whole.rgba[k as usize * n..(k as usize + 1) * n], "strip {k}");
        }
    }

    /// The fast helpers equal the `std` operations over their ranges (the PC page bytes
    /// must not change).
    #[test]
    fn exact_float_helpers() {
        // -0.0 floors to +0.0 here (same value; bilinear only subtracts and truncates it).
        for i in -600_000..600_000 {
            let x = i as f32 * 1e-3;
            assert_eq!(floor_f32(x), x.floor(), "floor {x}");
            if x >= 0.0 {
                assert_eq!(round_nonneg(x), x.round(), "round {x}");
            }
        }
        for v in [0.49999997f32, 0.5, 1.5, 2.5, 254.5, 254.49998, 0.0] {
            assert_eq!(round_nonneg(v), v.round(), "{v}");
        }
        for b in 0..=255u8 {
            assert_eq!(UNIT[b as usize].to_bits(), (b as f32 / 255.0).to_bits());
        }
    }

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
