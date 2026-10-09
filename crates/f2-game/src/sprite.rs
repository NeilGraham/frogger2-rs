//! Port of `Common/sprite.c` (the `SPRITE` pool and list) plus the state part of
//! `PC/pcsprite.c` (`PrintSprite` advances the angle of rotating sprites while drawing).
//!
//! Sprites live in a fixed pool (`sprList.array`, `maxSprites = 512` on PC) with a free
//! stack and an intrusive circular list headed by `sprList.head`. Allocation pushes each
//! sprite at the list head; a block of `n` sprites therefore forms a contiguous run in the
//! list starting at the returned (last allocated) sprite. Deallocation does **not** clear
//! the sprite (only `next`/`prev`/`draw`), so stale fields survive into the next use,
//! exactly like the original.
//!
//! Renderer API: [`SpriteState::iter_list`] yields the live sprites in list order
//! (`sprList.head.next` → ...), [`SpriteState::get`] reads one sprite.

use f2_math::SVector;
use serde::{Deserialize, Serialize};

use crate::ids::SpriteId;
use crate::Game;

/// `maxSprites` (PC).
pub const MAX_SPRITES: usize = 512;

// `SPRITE_*` flags (`sprite.h:21`).
pub const SPRITE_TRANSLUCENT: i16 = 1 << 0;
pub const SPRITE_DONE: i16 = 1 << 1;
pub const SPRITE_FLAGS_COLOUR_BLEND: i16 = 1 << 2;
pub const SPRITE_FLAGS_COLOUR_BLEND_AFTERLIGHT: i16 = 1 << 3;
pub const SPRITE_FLAGS_ROTATE: i16 = 1 << 4;
pub const SPRITE_ADDITIVE: i16 = 1 << 5;
pub const SPRITE_SUBTRACTIVE: i16 = 1 << 6;
pub const SPRITE_LOADING: i16 = 1 << 7;
pub const SPRITE_CLIP1: i16 = 1 << 8;
pub const SPRITE_CLIP2: i16 = 1 << 9;

/// Null link.
const NIL: u32 = u32::MAX;

/// `SPRITE` (`sprite.h:36`). `texture` is the texture CRC (`FindTexture(name)` =
/// `UpdateCRC("name.bmp")`, lower case); 0 = `NULL`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sprite {
    pub texture: u32,
    pub pos: SVector,
    pub scale_x: i16,
    pub scale_y: i16,
    pub flags: i16,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
    pub offset_x: i8,
    pub offset_y: i8,
    pub draw: i8,
    pub angle: i16,
    pub angle_inc: i16,
    pub array_index: i16,
}

/// `SPRITELIST sprList` (`sprite.c`). Index `max` in `next`/`prev` is the static head.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpriteState {
    /// `sprList.array` (empty = not allocated).
    pub array: Vec<Sprite>,
    next: Vec<u32>,
    prev: Vec<u32>,
    stack: Vec<u32>,
    /// `sprList.stackPtr`.
    pub stack_ptr: i32,
    /// `sprList.count`.
    pub count: i32,
    /// `maxSprites`.
    pub max_sprites: i32,
}

impl SpriteState {
    #[inline]
    fn head(&self) -> u32 {
        self.max_sprites as u32
    }

    /// Port of `InitSpriteList` (Common/sprite.c:147).
    pub fn init_sprite_list(&mut self) {
        let n = MAX_SPRITES;
        self.max_sprites = n as i32;
        self.array = vec![Sprite::default(); n];
        self.next = vec![NIL; n + 1];
        self.prev = vec![NIL; n + 1];
        self.next[n] = n as u32;
        self.prev[n] = n as u32;
        self.stack = (0..n as u32).collect();
        self.count = 0;
        self.stack_ptr = n as i32 - 1;
    }

    /// Port of `FreeSpriteList` (Common/sprite.c:108).
    pub fn free_sprite_list(&mut self) {
        self.array.clear();
        self.next.clear();
        self.prev.clear();
        self.stack.clear();
        self.count = 0;
        self.stack_ptr = 0;
    }

    /// Port of `AllocateSprites` (Common/sprite.c:45). Returns the first sprite of the
    /// block (the last one allocated); walk the block with [`SpriteState::next_of`].
    pub fn allocate_sprites(&mut self, number: i32) -> Option<SpriteId> {
        if self.array.is_empty() || number <= 0 || self.stack_ptr - number < 0 || number >= self.max_sprites - self.count {
            return None;
        }
        let head = self.head();
        let mut n = number;
        while n > 0 {
            n -= 1;
            let s = self.stack[self.stack_ptr as usize];
            self.stack_ptr -= 1;
            let first = self.next[head as usize];
            self.prev[s as usize] = head;
            self.next[s as usize] = first;
            self.prev[first as usize] = s;
            self.next[head as usize] = s;
            self.array[s as usize].draw = 1;
            self.count += 1;
        }
        Some(SpriteId(self.stack[(self.stack_ptr + 1) as usize]))
    }

    /// Port of `DeallocateSprites` (Common/sprite.c:79).
    pub fn deallocate_sprites(&mut self, head: Option<SpriteId>, number: i32) {
        let Some(h) = head else { return };
        let mut s = h.0;
        if self.array.is_empty() || self.next[s as usize] == NIL || number <= 0 || self.stack_ptr + number >= self.max_sprites {
            return;
        }
        let mut n = number;
        while n > 0 {
            n -= 1;
            let t = self.next[s as usize];
            if s == self.head() || s == NIL {
                // C would unlink the static head / dereference NULL; never happens for
                // well-formed blocks.
                return;
            }
            let (p, nx) = (self.prev[s as usize], self.next[s as usize]);
            self.next[p as usize] = nx;
            self.prev[nx as usize] = p;
            self.next[s as usize] = NIL;
            self.prev[s as usize] = NIL;
            self.array[s as usize].draw = 0;
            self.count -= 1;
            self.stack_ptr += 1;
            self.stack[self.stack_ptr as usize] = s;
            s = t;
        }
    }

    /// `s->next` (None at the list head or for an unlinked sprite).
    pub fn next_of(&self, s: SpriteId) -> Option<SpriteId> {
        let n = *self.next.get(s.idx())?;
        (n != NIL && n != self.head()).then_some(SpriteId(n))
    }

    /// The `n` sprites starting at `first`, following `next` (an effect's sprite block).
    pub fn chain(&self, first: Option<SpriteId>, n: i32) -> Vec<SpriteId> {
        let mut v = Vec::new();
        let mut c = first;
        for _ in 0..n.max(0) {
            let Some(s) = c else { break };
            v.push(s);
            c = self.next_of(s);
        }
        v
    }

    pub fn get(&self, s: SpriteId) -> &Sprite {
        &self.array[s.idx()]
    }

    pub fn get_mut(&mut self, s: SpriteId) -> &mut Sprite {
        &mut self.array[s.idx()]
    }

    /// Whether `s` is currently linked into the list.
    pub fn is_linked(&self, s: SpriteId) -> bool {
        self.next.get(s.idx()).is_some_and(|&n| n != NIL)
    }

    /// Live sprites in list order (`for(cur = sprList.head.next; cur != &sprList.head; ...)`).
    pub fn iter_list(&self) -> impl Iterator<Item = (SpriteId, &Sprite)> + '_ {
        let head = self.head();
        let mut c = if self.array.is_empty() { head } else { self.next[head as usize] };
        std::iter::from_fn(move || {
            if self.array.is_empty() || c == head || c == NIL {
                return None;
            }
            let id = c;
            c = self.next[c as usize];
            Some((SpriteId(id), &self.array[id as usize]))
        })
    }
}

impl Game {
    /// The state part of `PrintSprites`/`PrintSprite` (PC/pcsprite.c:107/290): every drawn
    /// rotating sprite advances `angle += (angleInc*gameSpeed)>>12`, wrapped to 0..4096.
    ///
    /// DEVIATION: the original only rotates sprites that survive the renderer's z-sort
    /// (`sc.vz - frog.vz < farClip`, `sc.vz != 0`, at most `MAX_ARRAY_SPRITES`); the
    /// headless port rotates every drawable sprite. Sprite angles are visual only.
    pub fn print_sprites_state(&mut self) {
        if self.sprite.count == 0 {
            return;
        }
        let gs = self.game_speed;
        let ids: Vec<SpriteId> = self.sprite.iter_list().map(|(i, _)| i).collect();
        for id in ids {
            let s = self.sprite.get_mut(id);
            if s.draw == 0 || s.texture == 0 || s.scale_x == 0 || s.scale_y == 0 {
                continue;
            }
            if s.flags & SPRITE_FLAGS_ROTATE != 0 {
                let mut a = (s.angle as i32).wrapping_add((s.angle_inc as i32).wrapping_mul(gs) >> 12) as i16;
                if a >= 4096 {
                    a -= 4096;
                } else if a < 0 {
                    a += 4096;
                }
                s.angle = a;
            }
        }
    }
}

impl Game {
    /// Whether `DrawLoop` (PC/drawloop.cpp:142) draws the 3D scene this mode.
    fn draw_loop_draws_scene(&self) -> bool {
        use crate::define::mode::*;
        !matches!(self.game_state.mode, ARTVIEWER_MODE | STARTUP_MODE | TEASERSCREEN_MODE)
    }

    /// The state changes `DrawLoop` makes while drawing the scene (after
    /// `UpdateFrogTongue`/`UpdateFrogCroak`): `PrintSprites` rotates sprites and
    /// `DrawSpecialFX` calls the effects' `Draw` functions (health flies draw `Random()`).
    pub fn draw_loop_3d_state(&mut self) {
        use crate::define::mode::*;
        if !self.draw_loop_draws_scene() {
            return;
        }
        self.print_sprites_state();
        if !matches!(self.game_state.mode, MULTI_WINRACE_MODE | MULTI_WINBATTLE_MODE | MULTI_WINCOLLECT_MODE | LEVELCOMPLETE_MODE) {
            self.draw_special_fx_state();
        }
    }

    /// The state changes of the rest of `DrawLoop`: sprite/text overlay slides and
    /// `ProcessProcTextures` (unless paused / game over).
    pub fn draw_loop_2d_state(&mut self) {
        if !self.draw_loop_draws_scene() {
            return;
        }
        self.print_overlays_state();
        self.process_proc_textures_drawloop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocate_blocks_are_contiguous_and_reverse_ordered() {
        let mut s = SpriteState::default();
        s.init_sprite_list();
        let a = s.allocate_sprites(3).unwrap();
        // popped 511, 510, 509; the block starts at the last one popped
        assert_eq!(a, SpriteId(509));
        assert_eq!(s.chain(Some(a), 3), vec![SpriteId(509), SpriteId(510), SpriteId(511)]);
        let b = s.allocate_sprites(2).unwrap();
        assert_eq!(s.chain(Some(b), 5), vec![SpriteId(507), SpriteId(508), SpriteId(509), SpriteId(510), SpriteId(511)]);
        assert_eq!(s.count, 5);
        s.get_mut(a).angle = 77;
        s.deallocate_sprites(Some(a), 3);
        assert_eq!(s.count, 2);
        assert_eq!(s.iter_list().count(), 2);
        // stale data survives; the freed block is reused LIFO
        let c = s.allocate_sprites(1).unwrap();
        assert_eq!(c, SpriteId(511));
        let c = s.allocate_sprites(1).unwrap();
        assert_eq!(c, SpriteId(510));
        let c = s.allocate_sprites(1).unwrap();
        assert_eq!(c, SpriteId(509));
        assert_eq!(s.get(c).angle, 77);
        assert_eq!(s.get(c).draw, 1);
    }

    #[test]
    fn allocation_limits() {
        let mut s = SpriteState::default();
        assert!(s.allocate_sprites(1).is_none());
        s.init_sprite_list();
        assert!(s.allocate_sprites(0).is_none());
        assert!(s.allocate_sprites(512).is_none());
        assert!(s.allocate_sprites(511).is_some());
        assert!(s.allocate_sprites(1).is_none());
        // double free is ignored (next == NULL)
        let mut s = SpriteState::default();
        s.init_sprite_list();
        let a = s.allocate_sprites(1);
        s.deallocate_sprites(a, 1);
        s.deallocate_sprites(a, 1);
        assert_eq!(s.count, 0);
    }
}
