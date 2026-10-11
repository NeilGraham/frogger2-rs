//! 2D drawing of `DrawLoop` (PC/drawloop.cpp:315-330): screen fades (`PC/fadeout.cpp`),
//! sprite overlays (`PrintSpriteOverlays`, PC/pcsprite.c:140), text overlays
//! (`PrintTextOverlays` / `PrintText`, Common/textover.c:120/203), full-screen and tiled
//! backdrops (`PC/backdrop.c`, `DrawTiledBackdrop` pcsprite.c:432) and the per-frame
//! procedural texture upload (`PTSurfaceBlit`, PC/pcmisc.cpp).
//!
//! Overlays are in the 4096×4096 virtual space; the renderer maps it to the screen
//! (authentic: `OVERLAY_X = rXRes/4096` stretch) or re-anchors it (enhanced, see
//! `docs/enhancements.md`). The slide motion of overlays is simulation state (f2-game).

use f2_engine::{Blend, FontId, Quad2D, Renderer, TexHandle, OVERLAY_SPACE};
use f2_game::define::mode::PAUSE_MODE;
use f2_game::frontend::UiOverlay;
use f2_game::sprite::{SPRITE_ADDITIVE, SPRITE_SUBTRACTIVE};
use f2_game::textover::{Font as GameFont, TEXTOVERLAY_PAUSED, TEXTOVERLAY_SHADOW};
use f2_game::Game;

// Newer `core` has some of these methods inherently (then the trait goes unused).
#[cfg(not(feature = "std"))]
#[allow(unused_imports)]
use crate::fmath::FloatExt;
use crate::fx::FIXED_TO_RADS;
use crate::prelude::*;
use alloc::borrow::Cow;

/// The two game fonts (`pcFont` = `bigfont.bmp`, `pcFontSmall` = `smallfont.bmp`).
#[derive(Clone, Copy, Debug)]
pub struct GameFonts {
    pub normal: FontId,
    pub small: FontId,
}

impl GameFonts {
    fn get(&self, f: GameFont) -> FontId {
        match f {
            GameFont::Normal => self.normal,
            GameFont::Small => self.small,
        }
    }
}

fn tex_of(r: &Renderer, crc: u32) -> Option<TexHandle> {
    if crc == 0 {
        None
    } else {
        r.textures.current(crc)
    }
}

/// `SPRITECLIP*` (pcsprite.c:52): 10 pixels in from the 640×480 screen edges. Authentic
/// only (scaled with the resolution); the enhanced profile does not clip.
pub fn set_sprite_clip(r: &mut Renderer, authentic: bool) {
    r.sprite_clip = if authentic {
        let [x, y, w, h] = r.view.vp();
        let (sx, sy) = (w / 640.0, h / 480.0);
        Some([x + 10.0 * sx, y + 10.0 * sy, x + 630.0 * sx, y + 470.0 * sy])
    } else {
        None
    };
}

/// Port of the drawing part of `PrintSpriteOverlays(num)` (pcsprite.c:140): entries with
/// `num`, from the last block entry to the first.
pub fn print_sprite_overlays(r: &mut Renderer, game: &Game, num: i16) {
    let l = &game.overlays;
    if !l.draw_overlays || l.num_entries == 0 {
        return;
    }
    let n = (l.alloc.max(0) as usize).min(l.block.len());
    for cur in l.block[..n].iter().rev() {
        if cur.num != num || cur.used == 0 || cur.draw == 0 || cur.a == 0 {
            continue;
        }
        let blend = if cur.flags & SPRITE_ADDITIVE != 0 {
            Blend::Additive
        } else if cur.flags & SPRITE_SUBTRACTIVE != 0 {
            Blend::Subtractive
        } else {
            Blend::Semi
        };
        let angle = if cur.angle != 0 { cur.angle as f32 * -FIXED_TO_RADS } else { 0.0 };
        r.draw_sprite_overlay(
            cur.x_pos as f32,
            cur.y_pos as f32,
            cur.width as f32,
            cur.height as f32,
            [0.0, 0.0, 1.0, 1.0],
            tex_of(r, cur.tex),
            [cur.r, cur.g, cur.b, cur.a],
            blend,
            angle,
        );
    }
}

/// `PrintText` (textover.c:174, PC): `scale = cur->scale/4096 * rXRes/640` (in the
/// enhanced profile: the 4:3 area's width/640).
#[allow(clippy::too_many_arguments)]
fn print_text(r: &mut Renderer, font: FontId, x: i16, y: i16, text: &str, scale: i32, centred: bool, rgba: [u8; 4], dy_px: f32) {
    let (k, _) = r.overlay_scale(0.0, 0.0);
    let res = k * OVERLAY_SPACE / 640.0;
    let s = scale as f32 / 4096.0 * res;
    let text = expand_prompts(text);
    r.draw_text_overlay_ex(font, x as f32, y as f32, &text, rgba, s, centred, dy_px);
}

/// Button names for the `@X`/`@T`/`@S`/`@C` codes (set by the host each frame).
static PROMPTS: spin::Mutex<Vec<(char, String)>> = spin::Mutex::new(Vec::new());

/// Sets the names `@X` (cross), `@T` (triangle), `@S` (square), `@C` (circle) expand to.
pub fn set_button_prompts(p: Vec<(char, String)>) {
    *PROMPTS.lock() = p;
}

/// The key-name substitutions of the PC font code (`DrawFontStringAtLoc`, mdxFont.cpp,
/// source not preserved): `@X` etc. become the bound key/button name.
pub fn expand_prompts(text: &str) -> Cow<'_, str> {
    if !text.contains('@') {
        return text.into();
    }
    {
        let p = PROMPTS.lock();
        let mut out = String::with_capacity(text.len() + 8);
        let mut it = text.chars().peekable();
        while let Some(c) = it.next() {
            if c == '@' {
                if let Some(&n) = it.peek() {
                    if let Some((_, name)) = p.iter().find(|(k, _)| *k == n) {
                        out.push_str(name);
                        it.next();
                        continue;
                    }
                }
            }
            out.push(c);
        }
        out.into()
    }
}

/// Shadow offset of `PrintTextOverlays`: 6 pixels (`font`) / 4 (`fontSmall`), constant
/// pixels in the original; scaled with the 4:3 area in the enhanced profile.
fn shadow_dy(r: &Renderer, font: GameFont, authentic: bool) -> f32 {
    let d = if font == GameFont::Normal { 6.0 } else { 4.0 };
    if authentic {
        d
    } else {
        let (k, _) = r.overlay_scale(0.0, 0.0);
        d * (k * OVERLAY_SPACE / 640.0).max(1.0)
    }
}

/// Port of the drawing part of `PrintTextOverlays` (textover.c:203, PC).
pub fn print_text_overlays(r: &mut Renderer, game: &Game, fonts: &GameFonts, authentic: bool) {
    let l = &game.textover;
    if l.num_entries == 0 {
        return;
    }
    let paused = game.game_state.mode == PAUSE_MODE;
    let n = (l.alloc.max(0) as usize).min(l.block.len());
    for cur in &l.block[..n] {
        if cur.used == 0 || cur.draw == 0 || (paused && cur.flags & TEXTOVERLAY_PAUSED == 0) {
            continue;
        }
        let font = fonts.get(cur.font);
        let centred = cur.centred != 0;
        if cur.flags & TEXTOVERLAY_SHADOW != 0 {
            let a = (cur.a as f32 * 7.0 / 10.0) as u8;
            let dy = shadow_dy(r, cur.font, authentic);
            print_text(r, font, cur.x_pos, cur.y_pos, &cur.text, cur.scale, centred, [0, 0, 0, a], dy);
        }
        print_text(r, font, cur.x_pos, cur.y_pos, &cur.text, cur.scale, centred, [cur.r, cur.g, cur.b, cur.a], 0.0);
    }
}

/// The frontend's stand-in overlays (`f2_game::frontend::UiOverlay`: level complete,
/// game over, the end, multiplayer win screens, training) in `textover`/`overlays` order.
fn ui_overlays(game: &Game) -> Vec<&UiOverlay> {
    let f = &game.frontend;
    let mut v: Vec<&UiOverlay> = Vec::new();
    for o in [
        &f.t_text,
        &f.coin_text,
        &f.new_best_text,
        &f.extra_text,
        &f.n_text,
        &f.old_best_text,
        &f.lev_name,
        &f.extra_icon,
        &f.coin_icon,
        &f.b_icon,
        &f.game_over_text,
    ]
    .into_iter()
    .flatten()
    {
        v.push(o);
    }
    v.extend(f.o_text.iter().flatten());
    v.extend(f.sparkles.iter().flatten());
    v.extend(f.flying_coins.iter());
    v
}

/// Draws the frontend stand-in sprite overlays of pass `num` (`sprite == true`).
pub fn print_ui_sprite_overlays(r: &mut Renderer, game: &Game, num: i32) {
    for o in ui_overlays(game) {
        if !o.sprite || !o.draw || o.a <= 0 || o.num != num {
            continue;
        }
        let tex = if o.tex.is_empty() { None } else { r.textures.current(f2_game::specfx::find_texture(&o.tex)) };
        let blend = if o.flags & SPRITE_ADDITIVE as u32 != 0 { Blend::Additive } else { Blend::Semi };
        let a = o.a.clamp(0, 255) as u8;
        r.draw_sprite_overlay(o.x_pos as f32, o.y_pos as f32, o.width as f32, o.height as f32, [0.0, 0.0, 1.0, 1.0], tex, [o.r, o.g, o.b, a], blend, 0.0);
    }
}

/// Draws the frontend stand-in text overlays (with the text overlays pass).
pub fn print_ui_text_overlays(r: &mut Renderer, game: &Game, fonts: &GameFonts, authentic: bool) {
    let paused = game.game_state.mode == PAUSE_MODE;
    for o in ui_overlays(game) {
        if o.sprite || !o.draw || (paused && o.flags & TEXTOVERLAY_PAUSED as u32 == 0) {
            continue;
        }
        let a = o.a.clamp(0, 255) as u8;
        let (x, y) = (o.x_pos.clamp(i16::MIN as i32, i16::MAX as i32) as i16, o.y_pos.clamp(i16::MIN as i32, i16::MAX as i32) as i16);
        if o.flags & TEXTOVERLAY_SHADOW as u32 != 0 {
            let dy = shadow_dy(r, GameFont::Normal, authentic);
            print_text(r, fonts.normal, x, y, &o.text, o.scale, o.centred, [0, 0, 0, (a as f32 * 0.7) as u8], dy);
        }
        print_text(r, fonts.normal, x, y, &o.text, o.scale, o.centred, [o.r, o.g, o.b, a], 0.0);
    }
}

/// Port of the drawing of `DrawScreenFade` (fadeout.cpp:34): a full-screen flat rectangle
/// of grey `col`, subtracted (`xluSubRS`) or, for `flashScreen`, added.
pub fn draw_screen_transition(r: &mut Renderer, game: &Game) {
    let Some(col) = game.screen_fade_colour() else { return };
    let c = col.clamp(0, 255) as u8;
    let (w, h) = r.output_size();
    let blend = if game.fadeout.flash_screen { Blend::Additive } else { Blend::Subtractive };
    r.draw_quad(Quad2D::rect(0.0, 0.0, w as f32, h as f32, [0.0; 4], None, [c, c, c, 255], blend));
}

/// Scroll position of the tiled backdrop (`xScroll`/`yScroll` statics of
/// `DrawTiledBackdrop`), in 640×480 pixels.
#[derive(Clone, Copy, Debug, Default)]
pub struct TileScroll {
    pub x: f32,
    pub y: f32,
}

const X_TILE: i32 = 5;
const Y_TILE: i32 = 4;
const X_SCROLL_SPEED: f32 = 0.001;
const Y_SCROLL_SPEED: f32 = 0.0006;

impl TileScroll {
    /// The scroll step `DrawTiledBackdrop` makes per drawn frame (`gameSpeed` = 4096 per
    /// 60 Hz frame), at 640×480.
    pub fn step(&mut self, game_speed: i32) {
        let (xres, yres) = (640.0, 480.0);
        self.x += game_speed as f32 * X_SCROLL_SPEED;
        let xw = (xres * 4.0) / X_TILE as f32;
        if self.x > xw {
            self.x -= xw;
        } else if self.x < 0.0 {
            self.x += xw;
        }
        self.y += game_speed as f32 * Y_SCROLL_SPEED;
        let yh = (yres * 2.0) / Y_TILE as f32;
        if self.y > yh {
            self.y -= yh;
        } else if self.y < 0.0 {
            self.y += yh;
        }
    }
}

/// Port of `DrawTiledBackdrop` (pcsprite.c:432): 4 textures `<name>01..04` tiled 5×4 over
/// the screen, scrolling. Drawn as backdrops (before the scene).
pub fn draw_tiled_backdrop(r: &mut Renderer, name: &str, scroll: TileScroll) {
    let tex: Vec<Option<TexHandle>> = (1..=4).map(|i| r.textures.current(f2_game::specfx::find_texture(&format!("{name}0{i}")))).collect();
    if tex[0].is_none() {
        return;
    }
    let (w, h) = r.output_size();
    let (xres, yres) = (w as f32, h as f32);
    let (sx, sy) = (scroll.x * xres / 640.0, scroll.y * yres / 480.0);
    for x in -4..X_TILE + 4 {
        for y in -2..Y_TILE + 2 {
            // RECT members are longs: truncated.
            let left = ((x as f32 * xres) / X_TILE as f32 + sx) as i32 as f32;
            let right = (((x + 1) as f32 * xres) / X_TILE as f32 + sx) as i32 as f32;
            let top = ((y as f32 * yres) / Y_TILE as f32 + sy) as i32 as f32;
            let bottom = (((y + 1) as f32 * yres) / Y_TILE as f32 + sy) as i32 as f32;
            let i = (((x + 4) % 2) + ((y + 2) % 2) * 2 + ((x + 4) / 2) * 2) % 4;
            let t = tex[i as usize].or(tex[0]);
            r.draw_backdrop_quad(Quad2D::rect(left, top, right - left, bottom - top, [0.0, 0.0, 1.0, 1.0], t, [255; 4], Blend::Semi));
        }
    }
}

/// `ProcessProcTextures`' surface blit (`PTSurfaceBlit`, PC/pcmisc.cpp): uploads the last
/// blitted pixels of every procedural texture over the bank texture.
pub fn upload_proc_textures(r: &mut Renderer, game: &Game) {
    for pt in &game.ptexture.list {
        let Some(rgba) = pt.surface_rgba8() else { continue };
        let Some(e) = r.textures.entry(pt.crc) else { continue };
        let h = e.image;
        let side = ((rgba.len() / 4) as f32).sqrt() as u32;
        if side == 0 || (side * side * 4) as usize != rgba.len() {
            continue;
        }
        r.textures.replace_image(h, side, side, rgba);
    }
}
