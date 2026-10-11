//! Port of `Common/collect.c` / `collect.h`: garibs (coins) and power-up collectables.
//!
//! Rendering-side state the logic touches (the coin `SPRITE`, the arcade HUD
//! `collectText` overlay) is kept as plain data; drawing is not ported here.
//! Special effects go through the `specfx.rs` entry points (stubs until specfx is
//! ported) at their original call positions, since effect creation draws `Random()`.

use crate::prelude::*;

use f2_math::{distance_between_points_ss, rotate_vector_by_quaternion_ff, FVector, Fixed, SVector};
use serde::{Deserialize, Serialize};

use crate::anim::SAMPLE_VOLUME;
use crate::define::mode::{DEMO_MODE, LEVELCOMPLETE_MODE};
use crate::define::SINGLEPLAYER;
use crate::effects::Effect;
use crate::frogger::{SPAWN_SCOREUPTIMER, UP_VEC};
use crate::frogmove::FROGSTATUS_ISDEAD;
use crate::ids::{GaribId, SpecFxId, TileId};
use crate::list::{HasLinks, Head, Links};
use crate::timer::Timer;
use crate::Game;

/// `PICKUP_RADIUS` (`collect.h:27`).
pub const PICKUP_RADIUS: Fixed = 400 << 12;

/// Garib types (`collect.h:29`).
pub const SILVERCOIN_GARIB: i32 = 0;
pub const EXTRALIFE_GARIB: i32 = 1;
pub const AUTOHOP_GARIB: i32 = 2;
pub const LONGTONGUE_GARIB: i32 = 3;
pub const QUICKHOP_GARIB: i32 = 4;
pub const INVULNERABILITY_GARIB: i32 = 5;
pub const GOLDCOIN_GARIB: i32 = 6;
pub const SLOWHOP_GARIB: i32 = 7;
pub const NUM_GARIB_TYPES: i32 = 8;

// `FXTYPE_*` values used here (`specfx.h:70`).
// TODO(port): move to specfx.rs when its type list is completed.
const FXTYPE_SPARKLYTRAIL: i16 = 3;
const FXTYPE_HEALTHFLY: i16 = 21;
const FXTYPE_TWINKLE: i16 = 25;

/// `genSfx[GEN_COLLECT_COIN]` = `FindSample(UpdateCRC("pickupcoin"))` (PC/pcaudio.c:204).
fn gen_collect_coin() -> u32 {
    f2_formats::crc::crc("pickupcoin")
}

/// `genSfx[GEN_POWERUP]` = `FindSample(UpdateCRC("powerup"))` (PC/pcaudio.c:210).
fn gen_powerup() -> u32 {
    f2_formats::crc::crc("powerup")
}

/// The garib's `SPRITE` (`sprite.h:34`): the fields `CreateNewGarib`/`UpdateGaribs` set.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GaribSprite {
    /// `texture` (texture name; `FindTexture(name)`).
    pub texture: String,
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
}

/// `GARIB` (`collect.h:42`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Garib {
    /// `prev`/`next`.
    pub links: Links<GaribId>,
    /// `next != NULL`: the garib is in `garibList` (the C test for "in use").
    pub linked: bool,
    pub active: i8,
    /// `type` (`unsigned char`).
    pub garib_type: u8,
    /// `sprite` (coins only).
    pub sprite: Option<GaribSprite>,
    /// The `sprList` slot `AllocateSprites(1)` gave `garib->sprite`: the element the
    /// renderer draws (kept in sync with `sprite` where the C writes `garib->sprite`).
    pub pool_sprite: Option<crate::ids::SpriteId>,
    /// `fx` (power-ups: the `FXTYPE_HEALTHFLY` effect).
    pub fx: Option<SpecFxId>,
    /// `SetFXColour(garib->fx, r, g, b)`: the colour `UpdateGaribs` reads back as
    /// `fx->r/g/b` (kept here until specfx owns effect state).
    pub fx_rgb: [u8; 3],
    pub pos: SVector,
    pub drop_speed: i16,
}

impl HasLinks<GaribId> for Garib {
    fn links(&self) -> &Links<GaribId> {
        &self.links
    }
    fn links_mut(&mut self) -> &mut Links<GaribId> {
        &mut self.links
    }
}

/// `GARIBLIST` (`collect.h:58`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GaribList {
    /// `array` (`MALLOC0(sizeof(GARIB)*num)`); empty + `allocated == false` is `NULL`.
    pub array: Vec<Garib>,
    /// `array != NULL`.
    pub allocated: bool,
    /// `head` (sentinel). Iterate with `head.ids(&array)`.
    pub head: Head<GaribId>,
    pub blocks: i16,
    pub count: i16,
    pub total: i16,
    pub max_coins: i16,
}

/// Minimal state of `arcadeHud.collectText` (`TEXTOVERLAY`, hud.c:252) that
/// `PickupCollectable`/`UpdateGaribs` drive. TODO(port): move to the HUD/overlay port,
/// whose overlay update moves `x_pos` towards `x_pos_to` at `speed`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectTextOverlay {
    /// `GAMESTRING(STR_*)` id name of the text.
    pub text: String,
    pub x_pos: i16,
    pub x_pos_to: i16,
    pub speed: Fixed,
    pub draw: u8,
}

/// Globals of `collect.c`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectState {
    /// `garibList`.
    pub garib_list: GaribList,
    /// `pauseTimer`.
    pub pause_timer: Timer,
    /// `pauseFlag`.
    pub pause_flag: i32,
    /// `pickupObjName[16]`.
    pub pickup_obj_name: String,
    /// `arcadeHud.collectText` (see [`CollectTextOverlay`]).
    pub collect_text: CollectTextOverlay,
}

impl Game {
    #[inline]
    fn garib(&self, id: GaribId) -> &Garib {
        &self.collect.garib_list.array[id.idx()]
    }

    #[inline]
    fn garib_mut(&mut self, id: GaribId) -> &mut Garib {
        &mut self.collect.garib_list.array[id.idx()]
    }

    /// `gameState.multi != SINGLEPLAYER && multiplayerMode == MULTIMODE_BATTLE`.
    fn collect_battle_mode(&self) -> bool {
        self.game_state.multi != SINGLEPLAYER && self.multi.multiplayer_mode == crate::multi::MULTIMODE_BATTLE
    }

    /// Port of `TogglePowerup` (Common/collect.c:59): if in multiplayer then kill the
    /// effect, else set it to not draw.
    fn toggle_powerup(&mut self, id: GaribId, v: i8) {
        if self.garib(id).fx.is_none() {
            return;
        }
        let fx = self.garib(id).fx;
        self.set_fx_draw(fx, v);
        self.garib_mut(id).active = v;
    }

    /// Sets `arcadeHud.collectText` sliding in with a power-up name.
    fn collect_text_in(&mut self, text: &str) {
        let t = &mut self.collect.collect_text;
        t.text = text.to_string();
        t.x_pos_to = if t.x_pos < 0 { 2049 } else { 2047 };
        t.speed = 327680;
    }

    /// Port of `CheckTileForCollectable` (Common/collect.c:77): checks the frog's position
    /// for a collectable item (used when Frogger has jumped).
    pub fn check_tile_for_collectable(&mut self, _tile: Option<TileId>, pl: i32) {
        let p = pl as usize;
        if self.frogger.player[p].frog_state & FROGSTATUS_ISDEAD != 0 {
            return;
        }

        // check current tile for a garib
        let mut c = self.collect.garib_list.head.first;
        while let Some(id) = c {
            let next = self.garib(id).links.next;
            // Also don't pickup garibs that the frog is trying to tongue.
            if self.garib(id).active == 0 || self.tongue.tongue[p].thing == crate::tongue::TongueThing::Garib(id) {
                c = next;
                continue;
            }

            let g = self.garib(id);
            let check = match &g.sprite {
                Some(s) => s.pos,
                None => g.pos,
            };

            if distance_between_points_ss(&check, &self.frog_position(p)) < PICKUP_RADIUS {
                self.pickup_collectable(id, pl);
            }
            c = next;
        }
    }

    /// Port of `ProcessCollectables` (Common/collect.c:116).
    pub fn process_collectables(&mut self) {
        self.update_garibs();

        // update players (for spawn counters / score bonus)
        if self.frogger.player[0].spawn_timer != 0 && self.game_state.multi == SINGLEPLAYER {
            self.frogger.player[0].spawn_timer -= 1;
            if self.frogger.player[0].spawn_timer == 0 {
                self.frogger.player[0].spawn_score_level = 1;
            }
        }
    }

    /// Port of `PickupCollectable` (Common/collect.c:137).
    pub fn pickup_collectable(&mut self, id: GaribId, pl: i32) {
        let p = pl as usize;
        let garib_type = self.garib(id).garib_type as i32;
        let sprite_pos = self.garib(id).sprite.as_ref().map_or(self.garib(id).pos, |s| s.pos);
        self.modscript_pickup(pl, garib_type); // P7b (no-op without mod scripts)

        match garib_type {
            GOLDCOIN_GARIB | SILVERCOIN_GARIB => {
                if garib_type == GOLDCOIN_GARIB {
                    self.frogger.player[p].num_spawn = self.frogger.player[p].num_spawn.wrapping_add(5);
                }
                // (fall through to SILVERCOIN_GARIB)
                if self.game_state.multi == SINGLEPLAYER && self.game_state.mode != DEMO_MODE {
                    let (cz, co) = (self.hud.arcade_hud.coin_zoom, self.hud.arcade_hud.coins_over);
                    let (ox, oy) = self.overlays.get(co).map_or((0, 0), |o| (o.x_pos, o.y_pos));
                    let fpos = self.frogger.frog[p].map(|a| self.actor2.actors[a].actor.position).unwrap_or_default();
                    // TransformPoint (view.rs).
                    let (sx, sy) = self.transform_point(&fpos);
                    if let Some(o) = self.overlays.get_mut(cz) {
                        o.draw = 1;
                        o.x_pos_to = ox;
                        o.y_pos_to = oy;
                        o.speed = 4096 * 50;
                        o.x_pos = sx as _;
                        o.y_pos = sy as _;
                    }
                    self.hud.coin_zoom_x = sx;
                    self.hud.coin_zoom_y = sy;
                }

                self.frogger.player[p].num_spawn = self.frogger.player[p].num_spawn.wrapping_add(1);
                if self.game_state.multi == SINGLEPLAYER && self.frogger.player[p].num_spawn == 25 {
                    let co = self.hud.arcade_hud.coins_over;
                    if let Some(o) = self.overlays.get_mut(co) {
                        o.tex = crate::specfx::find_texture("COINMEDAL");
                    }
                    self.effects.push(Effect::PlayVoice { player: pl as u8, uid: f2_formats::crc::crc("frogokay") });
                }

                // we need to get the up vector for this collectable...
                let q_rot = self.frogger.frog[p].map(|a| self.actor2.actors[a].actor.q_rot).unwrap_or_default();
                let mut se_up = FVector::default();
                rotate_vector_by_quaternion_ff(&mut se_up, &UP_VEC, &q_rot);

                if self.collect_battle_mode() {
                    if garib_type == GOLDCOIN_GARIB {
                        self.multi.mpl[p].trail = self.multi.mpl[p].trail.wrapping_add(20);
                    } else {
                        self.multi.mpl[p].trail = self.multi.mpl[p].trail.wrapping_add(4);
                    }
                } else {
                    let pp = &mut self.frogger.player[p];
                    if pp.spawn_timer != 0 && pp.spawn_score_level < 5 {
                        pp.spawn_score_level += 1;
                    }

                    pp.spawn_timer = SPAWN_SCOREUPTIMER as i16;

                    if pp.spawn_score_level == 5 {
                        pp.spawn_score_level = 0;

                        self.effects.push(Effect::PlayVoice { player: pl as u8, uid: f2_formats::crc::crc("frogokay") });

                        let fx = self.create_special_effect(FXTYPE_SPARKLYTRAIL, &sprite_pos, &se_up, 204800, 16384, 0, 24576);
                        if fx.is_some() {
                            self.set_fx_colour(fx, 0, 255, 255);
                            let fpos = self.frogger.frog[p].map(|a| self.actor2.actors[a].actor.position).unwrap_or_default();
                            self.set_fx_rebound(fx, &fpos, Some(&se_up));
                            self.set_fx_gravity(fx, 6140);
                        }
                    }

                    // Check here for opening retro levels
                }

                let fx = self.create_special_effect(FXTYPE_SPARKLYTRAIL, &sprite_pos, &se_up, 81920, 8192, 0, 8192);
                if fx.is_some() {
                    self.set_fx_colour(fx, 255, 255, 255);
                    self.set_fx_rebound(fx, &sprite_pos, Some(&se_up));
                    self.set_fx_gravity(fx, 8190);
                }
                let ssl = self.frogger.player[p].spawn_score_level as i32;
                let fx = self.create_special_effect(FXTYPE_SPARKLYTRAIL, &sprite_pos, &se_up, ssl.wrapping_mul(10) << 12, ssl << 12, 0, 12288);
                if fx.is_some() {
                    self.set_fx_colour(fx, 255, 255, 0);
                    self.set_fx_rebound(fx, &sprite_pos, Some(&se_up));
                    self.set_fx_gravity(fx, 4100);
                }

                self.play_sample(gen_collect_coin(), None, 0, SAMPLE_VOLUME, -1);
            }

            EXTRALIFE_GARIB => {
                if self.frogger.player[p].lives < 999 && self.game_state.mode != DEMO_MODE {
                    self.frogger.player[p].lives += 1;
                }

                self.create_pickup_effect(pl, 0, 200, 0, 220, 220, 220);
                self.play_sample(gen_powerup(), None, 0, SAMPLE_VOLUME, -1);

                if self.garib(id).fx.is_some() {
                    let fx = self.garib(id).fx;
                    self.deallocate_fx(fx, 1);
                }

                if self.game_state.multi == SINGLEPLAYER {
                    self.collect_text_in("STR_EXTRA_LIFE");
                }
            }

            QUICKHOP_GARIB => {
                self.frogger.player[p].quickhop.init(11);

                self.create_pickup_effect(pl, 255, 255, 255, 130, 130, 255);
                self.play_sample(gen_powerup(), None, 0, SAMPLE_VOLUME, -1);

                self.toggle_powerup(id, 0);

                if self.game_state.multi == SINGLEPLAYER {
                    self.collect_text_in("STR_QUICK_HOP");
                }
            }

            SLOWHOP_GARIB => {
                // Slow down all OTHER players
                for i in 0..self.frogger.num_frogs {
                    if i != pl {
                        self.frogger.player[i as usize].slowhop.init(6);
                        self.create_pickup_effect(i, 0, 220, 0, 255, 0, 0);
                    }
                }

                self.create_pickup_effect(pl, 255, 255, 255, 0, 0, 255);

                self.play_sample(gen_powerup(), None, 0, SAMPLE_VOLUME, -1);

                self.toggle_powerup(id, 0);

                if self.game_state.multi == SINGLEPLAYER {
                    self.collect_text_in("STR_SLOW_HOP");
                }
            }

            AUTOHOP_GARIB => {
                self.frogger.player[p].autohop.init(11);

                self.create_pickup_effect(pl, 255, 255, 255, 255, 0, 0);
                self.play_sample(gen_powerup(), None, 0, SAMPLE_VOLUME, -1);

                self.toggle_powerup(id, 0);

                if self.game_state.multi == SINGLEPLAYER {
                    self.collect_text_in("STR_AUTO_HOP");
                }
            }

            INVULNERABILITY_GARIB => {
                self.frogger.player[p].safe.init(6);

                self.create_pickup_effect(pl, 255, 255, 255, 255, 0, 0);
                self.play_sample(gen_powerup(), None, 0, SAMPLE_VOLUME, -1);

                self.toggle_powerup(id, 0);

                if self.game_state.multi == SINGLEPLAYER {
                    self.collect_text_in("STR_INVULNERABILITY");
                }
            }

            _ => {}
        }

        if self.collect_battle_mode() {
            let fx = self.garib(id).fx;
            self.deallocate_fx(fx, 1);
            self.sub_garib(id);
            self.multi.num_multi_items = self.multi.num_multi_items.wrapping_sub(1);
        } else if !(garib_type == AUTOHOP_GARIB || garib_type == QUICKHOP_GARIB || garib_type == SLOWHOP_GARIB || garib_type == INVULNERABILITY_GARIB) {
            self.sub_garib(id);
        }
    }

    /// `CreateSpecialEffectDirect` (Common/specfx.c:181), see [`Game::create_special_effect_direct`].
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create_special_effect_direct_pending(
        &mut self,
        ty: i16,
        origin: &SVector,
        normal: &FVector,
        size: Fixed,
        speed: Fixed,
        accn: Fixed,
        lifetime: Fixed,
    ) -> Option<SpecFxId> {
        self.create_special_effect_direct(ty, origin, normal, size, speed, accn, lifetime)
    }

    // wave E: `CreatePickupEffect` moved to specfx.rs.

    /// Port of `InitGaribList` (Common/collect.c:351): initialises the garib linked list.
    pub fn init_garib_list(&mut self) {
        let gl = &mut self.collect.garib_list;
        gl.count = 0;
        gl.total = 0;
        gl.max_coins = 0;
        gl.blocks = 0;
        gl.array = Vec::new();
        gl.allocated = false;
        gl.head = Head::default();
    }

    /// Port of `AddGarib` (Common/collect.c:369): adds a garib to the head of the list.
    pub fn add_garib(&mut self, id: GaribId) {
        if !self.garib(id).linked {
            let ty = self.garib(id).garib_type as i32;
            let gl = &mut self.collect.garib_list;
            gl.count = gl.count.wrapping_add(1);
            gl.total = gl.total.wrapping_add(1);
            if ty == SILVERCOIN_GARIB {
                gl.max_coins = gl.max_coins.wrapping_add(1);
            } else if ty == GOLDCOIN_GARIB {
                gl.max_coins = gl.max_coins.wrapping_add(5);
            }

            gl.head.push_front(&mut gl.array, id);
            gl.array[id.idx()].linked = true;
        }
    }

    /// Port of `SubGarib` (Common/collect.c:395): removes a garib from the list and
    /// zeroes it.
    pub fn sub_garib(&mut self, id: GaribId) {
        if !self.garib(id).linked || self.collect.garib_list.count <= 0 {
            self.effects.push(Effect::Log { text: "GARIB ERROR: invalid SubGarib()\n".into() });
            return;
        }

        // DeallocateSprites(garib->sprite, 1): the sprite's state is owned by the garib
        // here; the pool slot is returned.
        if let Some(s) = self.garib(id).pool_sprite {
            self.sprite.deallocate_sprites(Some(s), 1);
        }
        let gl = &mut self.collect.garib_list;
        gl.head.remove(&mut gl.array, id);
        gl.count = gl.count.wrapping_sub(1);

        // memset(garib, 0, sizeof(GARIB))
        gl.array[id.idx()] = Garib::default();
    }

    /// Port of `FreeGaribList` (Common/collect.c:422).
    pub fn free_garib_list(&mut self) {
        // BUG(original): with count <= 0 the array is kept (and `blocks` too); the next
        // AllocGaribBlock then reuses it whatever its size (see alloc_garib_block).
        if self.collect.garib_list.count <= 0 {
            return;
        }

        while let Some(first) = self.collect.garib_list.head.first {
            let before = self.collect.garib_list.count;
            self.sub_garib(first);
            if self.collect.garib_list.count == before {
                // SubGarib refused (count reached 0 with garibs still linked): the C
                // loop would spin forever here.
                // BUG(original): infinite loop; the port stops.
                break;
            }
        }

        let gl = &mut self.collect.garib_list;
        gl.array = Vec::new();
        gl.allocated = false;
        gl.count = 0;
        gl.blocks = 0;
    }

    /// Port of `AllocGarib` (Common/collect.c:446): finds a free garib in the array
    /// (multiplayer).
    fn alloc_garib(&mut self) -> Option<GaribId> {
        let gl = &self.collect.garib_list;
        let mut i = gl.blocks as i32 - 1;
        // BUG(original): `for( ; i-1; i-- )` never checks index 0 and, if it gets
        // there with blocks <= 1, walks below the array; the port stops instead.
        while i - 1 != 0 {
            if i < 0 || i as usize >= gl.array.len() {
                return None;
            }
            if !gl.array[i as usize].linked {
                return Some(GaribId(i as u32));
            }
            i -= 1;
        }
        None
    }

    /// Port of `AllocGaribBlock` (Common/collect.c:464).
    pub fn alloc_garib_block(&mut self, num: i32) {
        let gl = &mut self.collect.garib_list;
        if !gl.allocated && num > 0 {
            gl.array = vec![Garib::default(); num as usize];
            gl.allocated = true;
        } else if gl.allocated && (num.max(0) as usize) > gl.array.len() {
            // BUG(original): an array kept by FreeGaribList is reused even when too small
            // (heap overrun). The port grows it with zeroed garibs.
            gl.array.resize(num as usize, Garib::default());
        }

        gl.count = 0;
        gl.blocks = num as i16;
    }

    /// Port of `CreateNewGarib` (Common/collect.c:481). `COINS_IN_ARCADE_MODE` is defined,
    /// so coins are created in arcade mode too.
    pub fn create_new_garib(&mut self, pos: SVector, garib_type: i32) -> Option<GaribId> {
        // If singleplayer then no dynamic garib stuff so get the next one in the array
        let id = if self.game_state.multi == SINGLEPLAYER {
            let gl = &self.collect.garib_list;
            if gl.count >= gl.blocks || gl.count < 0 || gl.count as usize >= gl.array.len() {
                return None;
            }
            GaribId(gl.count as u32)
        } else {
            // Else have to search for a free one. Meh.
            if self.collect.garib_list.count >= self.collect.garib_list.blocks {
                return None;
            }
            self.alloc_garib()?
        };

        {
            let g = self.garib_mut(id);
            g.pos = pos;
            g.garib_type = garib_type as u8;
            g.active = 1;
        }

        if garib_type == GOLDCOIN_GARIB || garib_type == SILVERCOIN_GARIB {
            // initialise garib sprite (AllocateSprites(1): sprite pool exhaustion, where
            // the C frees the array element and returns NULL, is not modelled).
            let ps = self.sprite.allocate_sprites(1);
            self.garib_mut(id).pool_sprite = ps;
            let texture = if self.cheat_mad_garibs() { "RGARIB01" } else { "SCOIN0001" };
            let (r, g, b) = if garib_type == GOLDCOIN_GARIB { (255, 255, 0) } else { (255, 255, 255) };
            let angle = self.rng.random(4096) as i16;
            let s = GaribSprite {
                texture: texture.to_string(),
                pos,
                scale_x: 0,
                scale_y: 0,
                flags: 0,
                r,
                g,
                b,
                // a = 255, then a = 200 at the end of the setup.
                a: 200,
                offset_x: -16,
                offset_y: -16,
                draw: 1,
                angle,
                angle_inc: 50,
            };
            // `garib->sprite` IS the `sprList` element: mirror it so PrintSprites draws it.
            if let Some(ps) = ps {
                let sp = self.sprite.get_mut(ps);
                sp.texture = crate::specfx::find_texture(&s.texture);
                sp.pos = s.pos;
                sp.scale_x = s.scale_x;
                sp.scale_y = s.scale_y;
                sp.flags = s.flags;
                (sp.r, sp.g, sp.b, sp.a) = (s.r, s.g, s.b, s.a);
                sp.offset_x = s.offset_x;
                sp.offset_y = s.offset_y;
                sp.draw = s.draw;
                sp.angle = s.angle;
                sp.angle_inc = s.angle_inc;
            }
            self.garib_mut(id).sprite = Some(s);
        } else {
            let (name, r, g, b) = match garib_type {
                EXTRALIFE_GARIB => ("bfly04.obe", 0, 200, 0),
                QUICKHOP_GARIB => ("bfly02.obe", 130, 130, 255),
                INVULNERABILITY_GARIB => ("bfly03.obe", 255, 0, 0),
                SLOWHOP_GARIB => ("bfly05.obe", 255, 255, 0),
                AUTOHOP_GARIB => ("bfly.obe", 255, 80, 255),
                _ => {
                    self.effects.push(Effect::Log { text: format!("Error creating garib - {} is not a known garib type!\n", garib_type) });
                    return None;
                }
            };
            self.collect.pickup_obj_name = name.to_string();

            let fx = self.create_special_effect_direct_pending(FXTYPE_HEALTHFLY, &pos, &UP_VEC, 4096, 4096, 246, 0);
            self.garib_mut(id).fx = fx;
            self.set_fx_colour(fx, r, g, b);
            self.garib_mut(id).fx_rgb = [r, g, b];
        }

        self.add_garib(id);
        Some(id)
    }

    /// Port of `UpdateGaribs` (Common/collect.c:585).
    pub fn update_garibs(&mut self) {
        if self.game_state.multi == SINGLEPLAYER && self.game_state.mode != LEVELCOMPLETE_MODE {
            let afc = self.act_frame_count;
            let c = &mut self.collect;
            if c.pause_flag == 2 && c.collect_text.x_pos == c.collect_text.x_pos_to {
                c.collect_text.draw = 0;
                c.pause_flag = 0;
            } else if c.pause_flag != 2 {
                c.collect_text.draw = 1;

                if c.pause_timer.time != 0 {
                    c.collect_text.speed = 0;
                    c.pause_timer.update(-1, afc);
                    if c.pause_timer.time == 0 {
                        c.collect_text.speed = 327680;
                        if c.collect_text.x_pos_to == 2049 {
                            c.collect_text.x_pos_to = 4096 + 2048;
                        } else {
                            c.collect_text.x_pos_to = -2048;
                        }
                        c.pause_flag = 2;
                    }
                } else if c.pause_flag == 0 && (c.collect_text.x_pos as i32 - 2048).abs() == 1 {
                    c.pause_timer.init(2);
                    c.collect_text.speed = 0;
                    c.pause_flag = 1;
                }
            }
        }

        // update garib scales and calculate distance from Frog
        let step = (self.game_speed >> 12).max(1) as i16;
        let mut c = self.collect.garib_list.head.first;
        while let Some(id) = c {
            let g = self.garib_mut(id);
            c = g.links.next;
            if g.active == 0 {
                continue;
            } else if g.active > 1 {
                g.active -= 1;
            }

            if let Some(s) = g.sprite.as_mut() {
                s.scale_x = 64;
                s.scale_y = 64;
                if let Some(ps) = g.pool_sprite {
                    let sp = self.sprite.get_mut(ps);
                    sp.scale_x = 64;
                    sp.scale_y = 64;
                }
            } else if let Some(gfx) = g.fx {
                g.drop_speed = g.drop_speed.wrapping_sub(step);

                if g.drop_speed <= 0 {
                    let gpos = g.pos;
                    let (pos, normal, rgb) = {
                        let e = self.specfx.fx(gfx);
                        let a0 = e.act.as_ref().and_then(|v| v.first().copied()).flatten();
                        let pos = a0.map_or(gpos, |a| self.actor2.actors[a].actor.position);
                        (pos, e.normal, [e.r, e.g, e.b])
                    };
                    let fx = self.create_special_effect(FXTYPE_TWINKLE, &pos, &normal, 123840, 0, 0, 16384);
                    if fx.is_some() {
                        self.set_fx_tilt(fx, 4096);
                        self.set_fx_colour(fx, rgb[0], rgb[1], rgb[2]);
                    }
                    self.garib_mut(id).drop_speed = 30;
                }
            }
        }
    }

    /// Port of `ReactivatePowerups` (Common/collect.c:671).
    pub fn reactivate_powerups(&mut self) {
        let mut c = self.collect.garib_list.head.first;
        while let Some(id) = c {
            c = self.garib(id).links.next;
            let t = self.garib(id).garib_type as i32;
            if t == AUTOHOP_GARIB || t == QUICKHOP_GARIB || t == SLOWHOP_GARIB || t == INVULNERABILITY_GARIB {
                self.toggle_powerup(id, 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game_with_frog() -> Game {
        let mut g = Game::new();
        g.game_state.multi = SINGLEPLAYER;
        g.game_state.mode = crate::define::mode::INGAME_MODE;
        g
    }

    #[test]
    fn create_order_and_random() {
        let mut g = game_with_frog();
        g.init_garib_list();
        g.alloc_garib_block(4);
        let seed = g.rng;
        let a = g.create_new_garib(SVector { vx: 1, vy: 2, vz: 3, pad: 0 }, SILVERCOIN_GARIB).unwrap();
        let b = g.create_new_garib(SVector { vx: 4, vy: 5, vz: 6, pad: 0 }, AUTOHOP_GARIB).unwrap();
        let c = g.create_new_garib(SVector { vx: 7, vy: 8, vz: 9, pad: 0 }, GOLDCOIN_GARIB).unwrap();
        // Two coins -> two Random(4096) draws.
        let mut r = seed;
        let a0 = r.random(4096);
        let a1 = r.random(4096);
        assert_eq!(g.rng, r);
        assert_eq!(g.garib(a).sprite.as_ref().unwrap().angle as i32, a0);
        assert_eq!(g.garib(c).sprite.as_ref().unwrap().angle as i32, a1);
        let gl = &g.collect.garib_list;
        assert_eq!(gl.head.ids(&gl.array), vec![c, b, a]);
        assert_eq!((gl.count, gl.total, gl.max_coins), (3, 3, 6));
        assert_eq!(g.collect.pickup_obj_name, "bfly.obe");
    }

    /// `garib->sprite` is the `sprList` element PrintSprites draws: the pool slot must
    /// carry the coin's texture/pos/colour and the scale UpdateGaribs sets (else the
    /// renderer skips it and coins are invisible).
    #[test]
    fn coin_sprite_is_in_sprite_list() {
        let mut g = game_with_frog();
        g.sprite.init_sprite_list();
        g.init_garib_list();
        g.alloc_garib_block(1);
        let pos = SVector { vx: 10, vy: 20, vz: 30, pad: 0 };
        let a = g.create_new_garib(pos, GOLDCOIN_GARIB).unwrap();
        g.update_garibs();
        let ps = g.garib(a).pool_sprite.unwrap();
        let (id, sp) = g.sprite.iter_list().next().unwrap();
        assert_eq!(id, ps);
        assert_eq!(sp.texture, crate::specfx::find_texture("SCOIN0001"));
        assert_eq!(sp.pos, pos);
        assert_eq!((sp.scale_x, sp.scale_y, sp.draw, sp.a), (64, 64, 1, 200));
        assert_eq!((sp.r, sp.g, sp.b, sp.offset_x, sp.offset_y), (255, 255, 0, -16, -16));
    }

    #[test]
    fn coin_pickup_counts_and_removes() {
        let mut g = game_with_frog();
        g.init_garib_list();
        g.alloc_garib_block(2);
        let a = g.create_new_garib(SVector::default(), GOLDCOIN_GARIB).unwrap();
        let far = g.create_new_garib(SVector { vx: 1000, vy: 0, vz: 0, pad: 0 }, SILVERCOIN_GARIB).unwrap();
        g.check_tile_for_collectable(None, 0);
        assert_eq!(g.frogger.player[0].num_spawn, 6);
        assert_eq!(g.frogger.player[0].spawn_timer, SPAWN_SCOREUPTIMER as i16);
        let gl = &g.collect.garib_list;
        assert_eq!(gl.head.ids(&gl.array), vec![far]);
        assert_eq!((gl.count, gl.total), (1, 2));
        assert_eq!(gl.array[a.idx()], Garib::default());
        // Spawn timer runs down in ProcessCollectables.
        for _ in 0..SPAWN_SCOREUPTIMER {
            g.process_collectables();
        }
        assert_eq!(g.frogger.player[0].spawn_timer, 0);
        assert_eq!(g.frogger.player[0].spawn_score_level, 1);
    }

    #[test]
    fn powerups_set_timers_and_stay_listed() {
        let mut g = game_with_frog();
        g.init_garib_list();
        g.alloc_garib_block(3);
        let q = g.create_new_garib(SVector::default(), QUICKHOP_GARIB).unwrap();
        g.create_new_garib(SVector::default(), INVULNERABILITY_GARIB).unwrap();
        g.create_new_garib(SVector::default(), EXTRALIFE_GARIB).unwrap();
        let lives = g.frogger.player[0].lives;
        g.check_tile_for_collectable(None, 0);
        let p = &g.frogger.player[0];
        assert_eq!(p.quickhop.time, 11);
        assert_eq!(p.safe.time, 6);
        assert_eq!(p.lives, lives + 1);
        // Power-ups stay in the list; the extra life is removed.
        assert_eq!(g.collect.garib_list.count, 2);
        // With no fx (specfx stub), TogglePowerup leaves them active, as in C.
        assert_eq!(g.garib(q).active, 1);
        g.reactivate_powerups();
        // Head-first order: extra life, invulnerability, then quick hop (created first).
        assert_eq!(g.collect.collect_text.text, "STR_QUICK_HOP");
    }

    #[test]
    fn dead_frog_collects_nothing() {
        let mut g = game_with_frog();
        g.init_garib_list();
        g.alloc_garib_block(1);
        g.create_new_garib(SVector::default(), SILVERCOIN_GARIB).unwrap();
        g.frogger.player[0].frog_state |= FROGSTATUS_ISDEAD;
        g.check_tile_for_collectable(None, 0);
        assert_eq!(g.collect.garib_list.count, 1);
        g.free_garib_list();
        assert_eq!(g.collect.garib_list.count, 0);
        assert!(g.collect.garib_list.head.first.is_none());
        g.update_garibs();
    }
}
