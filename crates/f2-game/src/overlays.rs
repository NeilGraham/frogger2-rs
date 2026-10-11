//! Port of `Common/overlays.c` (`SPRITEOVERLAY` list) and the state part of
//! `PrintSpriteOverlays` (PC/pcsprite.c:140), which slides overlays towards
//! `xPosTo`/`yPosTo` while drawing.
//!
//! Renderer API: [`SpriteOverlayState::block`]; draw entries with `used != 0 && draw != 0
//! && a != 0`, in passes `num == 0`, (text), `num == 1`, `num == 2`, each pass iterating
//! the block from the last entry to the first (`PrintSpriteOverlays`). Coordinates are in
//! 4096-wide/high overlay units (`OVERLAY_X/Y`), `angle` is 4096 per turn, `tex` is the
//! texture CRC (0 = `NULL`).

use crate::prelude::*;

use f2_math::{fabs, fmul, Fixed};
use serde::{Deserialize, Serialize};

use crate::ids::OverlayId;
use crate::Game;

/// `SPRITELIST_TEMP_IDONTGIVEASHIT_NUMBER`.
pub const SPRITE_OVERLAY_ENTRIES: usize = 120;

// sprite overlay animation flags (`overlays.h`).
pub const OVERLAY_IA: i16 = 1 << 0;
pub const ANIMATION_FORWARDS: i16 = 1 << 0;
pub const ANIMATION_BACKWARDS: i16 = 1 << 1;
pub const ANIMATION_CYCLE: i16 = 1 << 2;
pub const ANIMATION_PINGPONG: i16 = 1 << 3;
pub const ANIMATION_RANDOMFRAMES: i16 = 1 << 4;
pub const ANIMATION_PLAYONCE: i16 = 1 << 5;
pub const ANIMATION_RANDOMSTART: i16 = 1 << 6;

/// `SPRITEOVERLAY` (`types2d.h`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpriteOverlay {
    pub x_pos: i16,
    pub y_pos: i16,
    pub x_pos_to: i16,
    pub y_pos_to: i16,
    pub width: i16,
    pub height: i16,
    pub num: i16,
    pub flags: i16,
    pub speed: Fixed,
    pub angle: Fixed,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
    pub draw: u8,
    /// Texture CRC (`FindTexture(name)`); 0 = `NULL`.
    pub tex: u32,
    pub used: i16,
}

/// `SPRITEOVERLAYLIST spriteOverlayList` plus `drawOverlays`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpriteOverlayState {
    pub num_entries: i32,
    pub alloc: i32,
    pub block: Vec<SpriteOverlay>,
    /// `drawOverlays` (PC/main.cpp; on in retail).
    pub draw_overlays: bool,
}

impl Default for SpriteOverlayState {
    fn default() -> Self {
        SpriteOverlayState { num_entries: 0, alloc: 0, block: Vec::new(), draw_overlays: true }
    }
}

impl SpriteOverlayState {
    /// Port of `InitSpriteOverlayLinkedList` (Common/overlays.c:104).
    pub fn init_sprite_overlay_linked_list(&mut self) {
        self.block = vec![SpriteOverlay::default(); SPRITE_OVERLAY_ENTRIES];
        self.alloc = SPRITE_OVERLAY_ENTRIES as i32;
        self.num_entries = 0;
    }

    /// Port of `FreeSpriteOverlayLinkedList` (Common/overlays.c:120): frees and re-inits.
    pub fn free_sprite_overlay_linked_list(&mut self) {
        self.init_sprite_overlay_linked_list();
    }

    /// Port of `CreateAndAddSpriteOverlay` (Common/overlays.c:44). `txtr_name` None keeps
    /// whatever texture the reused slot had (the original does not clear it).
    #[allow(clippy::too_many_arguments)]
    pub fn create_and_add_sprite_overlay(
        &mut self,
        x: i16,
        y: i16,
        txtr_name: Option<&str>,
        width: i16,
        height: i16,
        alpha: u8,
        flags: i16,
    ) -> Option<OverlayId> {
        if self.num_entries == self.alloc {
            return None;
        }
        let j = self.block.iter().position(|o| o.used == 0)?;
        self.num_entries += 1;
        let o = &mut self.block[j];
        o.x_pos = x;
        o.y_pos = y;
        o.x_pos_to = x;
        o.y_pos_to = y;
        o.width = width;
        o.height = height;
        o.draw = 1;
        o.flags = flags;
        o.num = 0;
        o.r = 255;
        o.g = 255;
        o.b = 255;
        o.a = alpha;
        o.speed = 50;
        o.angle = 0;
        o.used = 1;
        if let Some(n) = txtr_name {
            o.tex = crate::specfx::find_texture(n);
        }
        Some(OverlayId(j as u32))
    }

    /// Port of `SubSpriteOverlay` (Common/overlays.c:85).
    pub fn sub_sprite_overlay(&mut self, spr: Option<OverlayId>) {
        if let Some(o) = spr.and_then(|s| self.block.get_mut(s.idx())) {
            if o.used != 0 {
                o.used = 0;
                o.draw = 0;
                self.num_entries -= 1;
            }
        }
    }

    /// Port of `EnableSpriteOverlay` (Common/overlays.c:136).
    pub fn enable(&mut self, s: Option<OverlayId>) {
        if let Some(o) = self.get_mut(s) {
            o.draw = 1;
        }
    }

    /// Port of `DisableSpriteOverlay` (Common/overlays.c:148).
    pub fn disable(&mut self, s: Option<OverlayId>) {
        if let Some(o) = self.get_mut(s) {
            o.draw = 0;
        }
    }

    pub fn get(&self, s: Option<OverlayId>) -> Option<&SpriteOverlay> {
        s.and_then(|s| self.block.get(s.idx()))
    }

    pub fn get_mut(&mut self, s: Option<OverlayId>) -> Option<&mut SpriteOverlay> {
        s.and_then(|s| self.block.get_mut(s.idx()))
    }
}

/// The slide step shared by `PrintSpriteOverlays` and `PrintTextOverlays`: move `xPos`
/// (else `yPos`) by `spd = FMul(gameSpeed, speed)>>12` towards the target; snap and
/// stop (`speed = 0`) when within one step.
pub(crate) fn slide_step(x: &mut i16, y: &mut i16, x_to: i16, y_to: i16, speed: &mut Fixed, game_speed: i32) {
    let spd = fmul(game_speed, *speed) >> 12;
    if fabs(x_to as i32 - *x as i32) != 0 {
        *x = (*x as i32).wrapping_add(if x_to > *x { spd } else { spd.wrapping_neg() }) as i16;
        if fabs(x_to as i32 - *x as i32) < fabs(spd) {
            *speed = 0;
            *x = x_to;
        }
    } else if fabs(y_to as i32 - *y as i32) != 0 {
        *y = (*y as i32).wrapping_add(if y_to > *y { spd } else { spd.wrapping_neg() }) as i16;
        if fabs(y_to as i32 - *y as i32) < fabs(spd) {
            *speed = 0;
            *y = y_to;
        }
    }
}

impl Game {
    /// The state part of `PrintSpriteOverlays(num)` (PC/pcsprite.c:140).
    pub fn print_sprite_overlays_state(&mut self, num: i16) {
        let gs = self.game_speed;
        let l = &mut self.overlays;
        if !l.draw_overlays || l.num_entries == 0 {
            return;
        }
        for o in l.block.iter_mut().rev() {
            if o.num != num {
                continue;
            }
            if o.used != 0 && o.draw != 0 && o.a != 0 {
                // Go to destination, if specified
                slide_step(&mut o.x_pos, &mut o.y_pos, o.x_pos_to, o.y_pos_to, &mut o.speed, gs);
            }
        }
    }

    /// Port of `SlideSpriteOverlayToPos` (Common/frontend.c:575).
    pub fn slide_sprite_overlay_to_pos(&mut self, s: Option<OverlayId>, x: i32, y: i32, from_x: i32, from_y: i32, num_frames: i32) {
        let gs = self.game_speed;
        let Some(o) = self.overlays.get_mut(s) else { return };
        let mut vect = f2_math::FVector::new(x.wrapping_sub(from_x), y.wrapping_sub(from_y), 0);
        let speed = fmul(f2_math::magnitude_f(&vect), gs);
        vect.vx = x.wrapping_sub(o.x_pos as i32);
        vect.vy = y.wrapping_sub(o.y_pos as i32);
        f2_math::make_unit(&mut vect);
        let step = if num_frames != 0 { speed / num_frames } else { 0 };
        if (o.x_pos as i32 - x).abs() <= step {
            o.x_pos = x as i16;
        } else {
            o.x_pos = (o.x_pos as i32).wrapping_add(fmul(vect.vx, step)) as i16;
        }
        if (o.y_pos as i32 - y).abs() <= step {
            o.y_pos = y as i16;
        } else {
            o.y_pos = (o.y_pos as i32).wrapping_add(fmul(vect.vy, step)) as i16;
        }
        o.x_pos_to = o.x_pos;
        o.y_pos_to = o.y_pos;
    }

    /// Port of `SlideTextOverlayToPos` (Common/frontend.c:606).
    pub fn slide_text_overlay_to_pos(&mut self, s: Option<crate::ids::TextOverlayId>, x: i32, y: i32, from_x: i32, from_y: i32, num_frames: i32) {
        let gs = self.game_speed;
        let Some(o) = self.textover.get_mut(s) else { return };
        let mut vect = f2_math::FVector::new(x.wrapping_sub(from_x), y.wrapping_sub(from_y), 0);
        let speed = fmul(f2_math::magnitude_f(&vect), gs);
        vect.vx = x.wrapping_sub(o.x_pos as i32);
        vect.vy = y.wrapping_sub(o.y_pos as i32);
        f2_math::make_unit(&mut vect);
        let step = if num_frames != 0 { speed / num_frames } else { 0 };
        if (o.x_pos as i32 - x).abs() <= step {
            o.x_pos = x as i16;
        } else {
            o.x_pos = (o.x_pos as i32).wrapping_add(fmul(vect.vx, step)) as i16;
        }
        if (o.y_pos as i32 - y).abs() <= step {
            o.y_pos = y as i16;
        } else {
            o.y_pos = (o.y_pos as i32).wrapping_add(fmul(vect.vy, step)) as i16;
        }
        o.x_pos_to = o.x_pos;
        o.y_pos_to = o.y_pos;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_reuse_and_slide() {
        let mut g = Game::new();
        g.overlays.init_sprite_overlay_linked_list();
        let a = g.overlays.create_and_add_sprite_overlay(0, 0, Some("FLASH"), 10, 10, 255, 0);
        let b = g.overlays.create_and_add_sprite_overlay(1, 1, None, 10, 10, 255, 0);
        assert_eq!((a, b), (Some(OverlayId(0)), Some(OverlayId(1))));
        g.overlays.sub_sprite_overlay(a);
        g.overlays.sub_sprite_overlay(a);
        assert_eq!(g.overlays.num_entries, 1);
        // reused slot keeps its texture when created without one
        let c = g.overlays.create_and_add_sprite_overlay(0, 0, None, 10, 10, 255, 0);
        assert_eq!(c, a);
        assert_eq!(g.overlays.get(c).unwrap().tex, crate::specfx::find_texture("FLASH"));

        let o = g.overlays.get_mut(c).unwrap();
        o.x_pos_to = 100;
        o.speed = 20 << 12;
        let mut n = 0;
        while g.overlays.get(c).unwrap().speed != 0 {
            g.print_sprite_overlays_state(0);
            n += 1;
        }
        assert_eq!(n, 5);
        assert_eq!(g.overlays.get(c).unwrap().x_pos, 100);
        for _ in 0..(SPRITE_OVERLAY_ENTRIES - 2) {
            assert!(g.overlays.create_and_add_sprite_overlay(0, 0, None, 1, 1, 1, 0).is_some());
        }
        assert!(g.overlays.create_and_add_sprite_overlay(0, 0, None, 1, 1, 1, 0).is_none());
    }
}
