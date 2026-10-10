//! Port of `Common/multi.c` (+ `multi.h`): local multiplayer (2-4 players) race,
//! battle (light-cycle trails) and collect modes: per-player state (`mpl[]`), round and
//! match logic, win detection, respawns, the battle trail node pool and the
//! multiplayer camera targets.
//!
//! Not here (other modules/agents): the win screens (`RunMultiWinRace`/`Collect`,
//! frontend.c — this module only sets `gameState.mode`), the HUD (`UpDateMultiplayerInfo`,
//! text overlay sliding/projection), the camera itself (`CalcMPCamera` is ported but called
//! by cam.c), and special effects (`CreateRespawnEffect` is ported here on top of the
//! specfx stubs).

use f2_math::{
    add_to_vector_ff, add_to_vector_fs, add_vector_fff, distance_between_points_ss, dot_product_ff, make_unit, scale_vector, scale_vector_ff, set_vector_fs,
    set_vector_sf, sub_vector_fss, FVector, Fixed, SVector,
};
use serde::{Deserialize, Serialize};

use crate::actor2::{orientate, orientate_ss};
use crate::anim::SAMPLE_VOLUME;
use crate::babyfrog::{BABY_ANIM_SNOOZE, NUM_BABIES};
use crate::define::{tilestate, DIFFICULTY_HARD};
use crate::enemies::ENEMY_NEW_BATTLEMODE;
use crate::frogger::{
    FROG_ANIM_BREATHE, FROG_ANIM_HANDSPRING, FROG_ANIM_HOP, FROG_ANIM_HOPLEFT, FROG_ANIM_HOPRIGHT, FROG_ANIM_LOOKAROUND, FROG_ANIM_SUPERHOP,
    FROG_BREATHE_SPEED, MAX_FROGS, OUT_VEC, UP_VEC,
};
use crate::frogmove::*;
use crate::general::{find_joined_tile_by_direction, find_joined_tile_by_direction_and_type};
use crate::ids::{ActorId, EnemyId, TextOverlayId, TileId};
use crate::layout::worldid;
use crate::levplay::{PAD_DOWN, PAD_LEFT, PAD_RIGHT, PAD_START, PAD_UP};
use crate::map::SCALE;
use crate::textover::{Font, TEXTOVERLAY_SHADOW};
use crate::timer::Timer;
use crate::Game;

/// `MULTIMODE_*` (`multi.h:28`).
pub const MULTIMODE_RACE: i32 = 1;
pub const MULTIMODE_BATTLE: i32 = 2;
pub const MULTIMODE_COLLECT: i32 = 3;

/// `MULTI_BATTLE_MAXITEMS` (`multi.h:33`).
pub const MULTI_BATTLE_MAXITEMS: u32 = 15;
/// `MULTI_BATTLE_TRAILLENGTH` (`multi.h:34`).
pub const MULTI_BATTLE_TRAILLENGTH: i16 = 8;
/// `MULTI_NUM_CHARS` (`multi.h:36`).
pub const MULTI_NUM_CHARS: i32 = 9;
/// `MULTI_RACE_TIMEPENALTY` (`multi.h:38`).
pub const MULTI_RACE_TIMEPENALTY: u32 = 60;
/// `MULTI_ROUND_DRAW` (`multi.h:40`).
pub const MULTI_ROUND_DRAW: i8 = 0x7F;
/// `MAX_BATTLENODES` (`multi.c:113`).
pub const MAX_BATTLENODES: usize = 256;

/// `multiLevelIndex[MAX_WORLDS]` (`multi.c:66`).
pub const MULTI_LEVEL_INDEX: [i8; 9] = [1, 3, 3, 3, 3, 3, 3, 5, 10];

/// `multiGameTypes[MAX_WORLDS]` (`multi.c:72`): the mode `GoMulti` (options.c:966) picks
/// for `player[0].worldNum`.
pub const MULTI_GAME_TYPES: [i32; 9] = [0, MULTIMODE_RACE, 0, MULTIMODE_RACE, 0, MULTIMODE_BATTLE, 0, MULTIMODE_COLLECT, 0];

/// `mpltextx`, `mpltexty` (PC branch, `multi.c:690`).
pub const MPLTEXTX: i32 = 0;
pub const MPLTEXTY: i32 = -200;

/// `camDiv[4]`, `camPlus[4]` (`multi.c:1609`; the 4th entries are zero).
pub const CAM_DIV: [i32; 4] = [5000, 6500, 5000, 0];
pub const CAM_PLUS: [i32; 4] = [3000, 3000, 3000, 0];
/// `battleCamFac`, `battleCamPlus` (`multi.c:1680`).
pub const BATTLE_CAM_FAC: i32 = 3000;
pub const BATTLE_CAM_PLUS: i32 = 5000;

/// `scaleV` initial value (`cam.c:73`).
pub const SCALE_V_INIT: Fixed = 4506;

/// `MPINFO` (`multi.h:63`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MpInfo {
    /// Race and battle.
    pub wins: i16,
    pub lap: i16,
    /// Race.
    pub timer: u32,
    /// Race.
    pub penalty: u32,
    /// Race.
    pub check: i16,
    /// Battle.
    pub score: i16,
    /// Battle.
    pub trail: i16,
    /// Both.
    pub ready: i8,
    /// Battle.
    pub lasttile: i16,
    /// Battle: head of the trail (`BATTLENODE *path`), an index into `nodePool.array`.
    pub path: Option<u16>,
    pub scr_x: i16,
    pub scr_y: i16,
    /// `penalText` (`NULL` outside race mode).
    pub penal_text: Option<TextOverlayId>,
    /// `numText`.
    pub num_text: Option<TextOverlayId>,
}

/// `BATTLENODE` (`multi.h:43`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BattleNode {
    pub next: Option<u16>,
    pub prev: Option<u16>,
    /// Node is on this tile.
    pub tile: Option<TileId>,
    /// Used in battle multiplayer mode to make tiles look dangerous (`cube.obe`).
    pub fx: Option<ActorId>,
    /// Port-only: `fx->actor->size.vy` when the cube model is not loaded (the original
    /// always has the actor; the fade value doubles as the trail's lifetime).
    pub fx_size_vy: Fixed,
}

/// `BATTLENODELIST` (`multi.h:54`): `array == NULL` is an empty `array`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BattleNodeList {
    pub array: Vec<BattleNode>,
    pub count: i16,
    pub stack: Vec<u16>,
    pub stack_ptr: i16,
}

/// Globals of `multi.c`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultiState {
    /// `multiplayerMode`.
    pub multiplayer_mode: i32,
    /// `started`: 0 = waiting, 1 = countdown, 2 = running.
    pub started: i32,
    /// `numMultiItems`.
    pub num_multi_items: u32,
    /// `gameWinner` (-1, a player, or `MULTI_ROUND_DRAW`).
    pub game_winner: i8,
    /// `matchWinner`.
    pub match_winner: i8,
    /// `mpl[4]`.
    pub mpl: [MpInfo; MAX_FROGS],
    /// `powerupTimer`, `multiTimer`, `endTimer`.
    pub powerup_timer: Timer,
    pub multi_timer: Timer,
    pub end_timer: Timer,
    /// `nodePool`.
    pub node_pool: BattleNodeList,
    /// `initialCamHeight`.
    pub initial_cam_height: Fixed,
}

impl Default for MultiState {
    fn default() -> Self {
        MultiState {
            multiplayer_mode: 0,
            started: 0,
            num_multi_items: 0,
            game_winner: -1,
            match_winner: -1,
            mpl: Default::default(),
            powerup_timer: Timer::default(),
            multi_timer: Timer::default(),
            end_timer: Timer::default(),
            node_pool: BattleNodeList::default(),
            initial_cam_height: 0,
        }
    }
}

/// `SetVectorFF(&f, &svector)`: the macro copies the fields without shifting.
#[inline]
fn ff_from_s(v: &SVector) -> FVector {
    FVector { vx: v.vx as i32, vy: v.vy as i32, vz: v.vz as i32 }
}

#[inline]
fn is_frogger_area(state: u32) -> bool {
    (tilestate::FROGGER1AREA..=tilestate::FROGGER4AREA).contains(&state)
}

impl Game {
    #[inline]
    fn num_frogs_mp(&self) -> usize {
        self.frogger.num_frogs.clamp(0, MAX_FROGS as i32) as usize
    }

    #[inline]
    fn tile_state(&self, t: Option<TileId>) -> u32 {
        t.map_or(tilestate::NORMAL, |t| self.map.tile(t).state)
    }

    #[inline]
    fn is_dead(&self, i: usize) -> bool {
        self.frogger.player[i].frog_state & FROGSTATUS_ISDEAD != 0
    }

    #[inline]
    fn set_frog_draw(&mut self, pl: usize, v: i8) {
        if let Some(f) = self.frogger.frog[pl] {
            if let Some(a) = self.actor2.actors.get_mut(f) {
                a.draw = v;
            }
        }
    }

    /// `PlaySample(FindSample(utilStr2CRC(name)), NULL, 0, SAMPLE_VOLUME, -1)`.
    #[inline]
    fn mp_sample(&mut self, name: &str) {
        self.play_sample(f2_formats::crc::crc(name), None, 0, SAMPLE_VOLUME, -1);
    }

    /// `node->fx->actor->size.vy` (see [`BattleNode::fx_size_vy`]).
    fn node_size_vy(&self, n: u16) -> Fixed {
        let node = &self.multi.node_pool.array[n as usize];
        match node.fx.and_then(|f| self.actor2.actors.get(f)) {
            Some(a) => a.actor.size.vy,
            None => node.fx_size_vy,
        }
    }

    fn set_node_size_vy(&mut self, n: u16, v: Fixed) {
        let node = &mut self.multi.node_pool.array[n as usize];
        node.fx_size_vy = v;
        if let Some(a) = node.fx.and_then(|f| self.actor2.actors.get_mut(f)) {
            a.actor.size.vy = v;
        }
    }

    fn set_node_fx_draw(&mut self, n: u16, v: i8) {
        if let Some(a) = self.multi.node_pool.array[n as usize].fx.and_then(|f| self.actor2.actors.get_mut(f)) {
            a.draw = v;
        }
    }

    /// Countdown shared by the three modes: returns after updating `multiTimer`.
    /// `go_enemies` releases the battle-mode enemies on "go".
    fn mp_countdown(&mut self, go_enemies: bool) {
        let old_time = self.multi.multi_timer.time;
        let afc = self.act_frame_count;
        self.multi.multi_timer.update(-1, afc);
        if self.multi.multi_timer.time == 1 {
            if self.multi.started != 2 {
                self.mp_sample("racehorngo");
                self.multi.started = 2;
                for i in 0..self.num_frogs_mp() {
                    self.frogger.player[i].can_jump = 1;
                }
                if go_enemies {
                    for nme in self.enemies.enemy_list.head.ids(&self.enemies.enemy_list.array) {
                        let e = &mut self.enemies.enemy_list.array[nme.idx()];
                        if e.flags & ENEMY_NEW_BATTLEMODE != 0 {
                            e.is_waiting = 0;
                        }
                    }
                }
            }
        } else if self.multi.multi_timer.time == 0 {
            let r = self.rng.random(self.frogger.num_frogs as u32);
            self.play_voice(r, "frogletsgo");
        } else if self.multi.multi_timer.time != old_time && self.multi.multi_timer.time < 5 {
            self.mp_sample("racehorn");
        }
    }

    /// Port of `UpdateRace` (Common/multi.c:127): game mechanics for multiplayer race mode.
    pub fn update_race(&mut self) {
        let n = self.num_frogs_mp();

        // Wait for all players to be on the start/finish line
        if self.multi.started == 0 {
            // Hack to make camera move up only
            self.multi.initial_cam_height = self.cam.cam_source.vy;

            let mut j = 0;
            for i in 0..n {
                let Some(ct) = self.frogmove.curr_tile[i] else { continue };
                if self.map.tile(ct).state == tilestate::FROGGER1AREA {
                    j += 1;
                    self.frogger.player[i].can_jump = 0;
                } else {
                    let fwd = ff_from_s(&self.map.tile(ct).dir_vector[((self.frogmove.frog_facing[i] + self.cam.cam_facing[i]) & 3) as usize]);
                    // Hop forward to starting line
                    if self.frogger.player[i].can_jump != 0 {
                        // The original passes a NULL tile on when there is no joined tile.
                        if let Some(t) = find_joined_tile_by_direction(&self.map, ct, &fwd) {
                            self.hop_frog_to_tile(t, i as i32);
                        }
                    }
                }
            }

            self.multi.multi_timer.init(0);
            if j == n {
                self.multi.end_timer.init(0);
                self.multi.multi_timer.init(6);
                self.multi.started = 1;
            }
        }

        // When all players are ready, start a countdown
        if self.multi.multi_timer.time != 0 {
            self.mp_countdown(false);
        } else if self.multi.end_timer.time != 0 {
            // If finished the race then wait before replaying
            let afc = self.act_frame_count;
            self.multi.end_timer.update(-1, afc);
            if self.multi.end_timer.time == 0 {
                self.start_multi_win_game();
            }
            return;
        }

        // Mini-loop for respawning before race start
        if self.multi.started != 2 {
            for i in 0..n {
                // If waiting to respawn, update timer and check for respawn timeout
                if self.frogger.player[i].dead.time != 0 {
                    let afc = self.act_frame_count;
                    self.frogger.player[i].dead.update(-1, afc);
                    if self.frogger.player[i].dead.time == 0 {
                        self.race_respawn(i as i32);
                        self.set_frog_draw(i, 1);
                        self.frogger.player[i].safe.init(3);
                    }
                }
            }
            return;
        }

        // Proper game loop thing
        for i in 0..n {
            let pt = self.multi.mpl[i].penal_text;
            if self.textover.get(pt).is_some_and(|t| t.draw != 0) {
                let (bx, by) = (crate::frontend::BACK_TEXT_X[i], crate::frontend::BACK_TEXT_Y[i]);
                let (sx, sy) = (self.multi.mpl[i].scr_x as i32, self.multi.mpl[i].scr_y as i32);
                self.slide_text_overlay_to_pos(pt, bx, by, sx, sy, 40);
                if let Some(t) = self.textover.get_mut(pt) {
                    if t.x_pos as i32 == bx && t.y_pos as i32 == by {
                        t.draw = 0;
                    }
                }
            }

            if self.multi.mpl[i].ready == 0 {
                // Increase timer if not finished
                self.multi.mpl[i].timer = self.multi.mpl[i].timer.wrapping_add(self.act_frame_count.wrapping_sub(self.last_act_frame_count));

                // If waiting to respawn, update timer and check for respawn timeout
                if self.frogger.player[i].dead.time != 0 {
                    let afc = self.act_frame_count;
                    self.frogger.player[i].dead.update(-1, afc);
                    if self.frogger.player[i].dead.time == 0 {
                        self.race_respawn(i as i32);
                        self.set_frog_draw(i, 1);
                        self.frogger.player[i].safe.init(3);
                    } else {
                        continue;
                    }
                }

                // Initialise quickhop for everyone but the lead player
                if i as i32 != self.cam.player_focus as i32 {
                    self.frogger.player[i].quickhop.init(3);
                }

                let state = self.tile_state(self.frogmove.curr_tile[i]);
                // Kill frogs that have fallen off screen
                if self.frame_count > 50 && !self.is_point_visible(&self.frog_position(i)) {
                    self.kill_mp_frog(i as i32);
                } else if is_frogger_area(state) || state == tilestate::FINISHLINE {
                    let mp = &mut self.multi.mpl[i];
                    // If last tile state was type 4 and this is type 1 then we've got around another lap
                    if state == tilestate::FINISHLINE && mp.lasttile as i32 != tilestate::FINISHLINE as i32 {
                        mp.lap = mp.lap.wrapping_add(1);
                        mp.lasttile = state as i16;
                        mp.check = 0;

                        // Start of a new lap - if more then the defined number of maps for the race then this player is the winner
                        if mp.lap >= 1 {
                            mp.ready = 1;
                            self.frogger.player[i].can_jump = 0;
                            self.player_race_win(i as i32);
                        }
                    }
                    // Else if we've just got to another checkpoint (unlimited repetitions of 2,3,4. Tilestate 1 is used to signal lap changes)
                    else if state as i32 == mp.lasttile as i32 + 1
                        || (mp.lasttile as i32 == tilestate::FROGGER4AREA as i32 && state == tilestate::FROGGER2AREA)
                    {
                        mp.check = mp.check.wrapping_add(1);
                        mp.lasttile = state as i16;
                    }
                }
            }
        }

        // Check players for finish; if all players finished, check win conditions
        if (0..n).all(|i| self.multi.mpl[i].ready != 0) {
            // Find best time. `winner` is uninitialised in the original when no time beats
            // 999999999; zero (BSS-like) here.
            let mut best: u32 = 999_999_999;
            let mut winner = 0usize;
            for j in 0..n {
                let time = self.multi.mpl[j].timer.wrapping_add(self.multi.mpl[j].penalty);
                if time < best {
                    best = time;
                    winner = j;
                }
            }
            // If more then one on best time, draw
            let draw = (0..n).any(|j| j != winner && self.multi.mpl[j].timer.wrapping_add(self.multi.mpl[j].penalty) == best);

            if draw {
                self.multi.game_winner = MULTI_ROUND_DRAW;
            } else {
                self.play_voice(winner as i32, "frogokay");
                self.multi.game_winner = winner as i8;
                self.multi.mpl[winner].wins = self.multi.mpl[winner].wins.wrapping_add(1);
            }

            self.multi.end_timer.init(2);
            self.cam.control_camera = 1;
        }
    }

    /// Port of `UpdateBattle` (Common/multi.c:337): game mechanics for lightcycles mode.
    pub fn update_battle(&mut self) {
        let n = self.num_frogs_mp();
        let mut dead = 0usize;

        if self.multi.started == 0 {
            let mut count = 1;
            for nme in self.enemies.enemy_list.head.ids(&self.enemies.enemy_list.array) {
                if self.enemies.enemy_list.array[nme.idx()].flags & ENEMY_NEW_BATTLEMODE != 0 {
                    self.move_enemy_to_node(nme, count);
                    let e = &mut self.enemies.enemy_list.array[nme.idx()];
                    e.is_waiting = -1;
                    e.in_tile = None;
                    count += 1;
                }
            }

            for i in 0..n {
                self.frogger.player[i].idle_enable = 0;
                // Face all frogs towards the centre of the map
                self.face_frog_to_centre(i as i32, 1);
            }

            self.multi.end_timer.init(0);
            let r = self.rng.random(4);
            self.multi.powerup_timer.init(r + 3);
            self.multi.multi_timer.init(6);
            self.multi.started = 1;
        }

        // When all players are ready, start a countdown
        if self.multi.multi_timer.time != 0 {
            self.mp_countdown(true);
        }
        if self.multi.multi_timer.time <= 1 && self.multi.end_timer.time != 0 {
            let afc = self.act_frame_count;
            self.multi.end_timer.update(-1, afc);
            if self.multi.end_timer.time == 0 {
                self.start_multi_win_game();
            }
            return;
        }

        if self.multi.started != 0 {
            let afc = self.act_frame_count;
            self.multi.powerup_timer.update(-1, afc);
            if self.multi.powerup_timer.time == 0 {
                if self.multi.num_multi_items < MULTI_BATTLE_MAXITEMS && !self.map.tiles.is_empty() {
                    let tile_count = self.map.tiles.len() as u32;
                    let mut r;
                    loop {
                        r = self.rng.random(tile_count) as usize;
                        let s = self.map.tiles[r].state;
                        if !(s == tilestate::BARRED || is_frogger_area(s)) {
                            break;
                        }
                    }

                    let (normal, centre) = (self.map.tiles[r].normal, self.map.tiles[r].centre);
                    let mut pos = ff_from_s(&normal);
                    let mut tp = SVector::default();
                    if self.rng.random(5) == 0 {
                        scale_vector(&mut pos, 150);
                        add_to_vector_fs(&mut pos, &centre);
                        set_vector_sf(&mut tp, &pos);

                        match self.rng.random(3) {
                            0 => self.create_new_garib(tp, crate::collect::GOLDCOIN_GARIB),
                            1 => self.create_new_garib(tp, crate::collect::QUICKHOP_GARIB),
                            _ => self.create_new_garib(tp, crate::collect::SLOWHOP_GARIB),
                        };
                    } else {
                        scale_vector(&mut pos, 200);
                        add_to_vector_fs(&mut pos, &centre);
                        set_vector_sf(&mut tp, &pos);

                        self.create_new_garib(tp, crate::collect::SILVERCOIN_GARIB);
                    }

                    self.multi.num_multi_items = self.multi.num_multi_items.wrapping_add(1);
                }

                let r = self.rng.random(4);
                self.multi.powerup_timer.init(r + 3);
            }
        }

        for i in 0..n {
            // Change desired frog facing
            self.battle_process_controller(i as i32);

            if self.multi.started != 2 {
                continue;
            }

            // If the frog can move then continue in current direction
            if self.frogger.player[i].can_jump != 0 && !self.is_dead(i) {
                let mut tf = true;

                let res = self.move_to_requested_destination((self.frogmove.frog_facing[i] - self.cam.cam_facing[i]) & 3, i as i32);
                self.frogger.player[i].can_jump = 0;

                // Add a new node to the trail
                if res {
                    tf = self.add_battle_trail_node(i as i32);
                    // frogAnimSpeed /= 2; frogAnimSpeed2 /= 2; AnimateFrogHop; restore.
                    self.animate_frog_hop_half_speed(self.frogmove.frog_facing[i], i as i32);
                }

                if !tf || !res {
                    for j in 0..n {
                        if i != j && !self.is_dead(j) && !self.is_dead(i) && self.frogs_collide(i, j) {
                            self.kill_mp_frog(j as i32);
                        }
                    }
                    self.kill_mp_frog(i as i32);
                }
            }

            if let Some(path) = self.multi.mpl[i].path {
                let mut fo: Fixed = (256 * self.game_speed) >> 12;

                if !self.is_dead(i) {
                    self.multi.mpl[i].score = self.multi.mpl[i].score.wrapping_add(1);
                    let mut node = self.multi.node_pool.array[path as usize].next;

                    // Update trail. For 0->length, increase alpha to limit.
                    let mut j = 0;
                    while let Some(nd) = node {
                        if j >= self.multi.mpl[i].trail as i32 {
                            break;
                        }
                        // `if (node->fx && ...)`: the fx always exists in the original.
                        let s = self.node_size_vy(nd);
                        if s < 4096 - fo {
                            self.set_node_size_vy(nd, s + fo);
                        }
                        j += 1;
                        node = self.multi.node_pool.array[nd as usize].next;
                    }

                    fo = (128 * self.game_speed) >> 12;
                    // For length->endoflist, fade out and make not deadly
                    while let Some(nd) = node {
                        let temp = self.multi.node_pool.array[nd as usize].next;
                        let s = self.node_size_vy(nd);
                        if s <= fo {
                            self.sub_battle_trail_node(nd);
                        } else {
                            self.set_node_size_vy(nd, s - fo);
                        }
                        node = temp;
                    }
                } else {
                    let mut node = path;
                    if self.multi.node_pool.array[node as usize].next.is_some() {
                        let s = self.node_size_vy(node);
                        if s < 4096 - fo {
                            self.set_node_size_vy(node, s + fo);
                        }
                    }

                    fo = (128 * self.game_speed) >> 12;

                    while let Some(nx) = self.multi.node_pool.array[node as usize].next {
                        node = nx;
                    }

                    let s = self.node_size_vy(node);
                    if s <= fo {
                        self.sub_battle_trail_node(node);
                        if Some(node) == self.multi.mpl[i].path {
                            self.multi.mpl[i].path = None;
                        }
                    } else {
                        self.set_node_size_vy(node, s - fo);
                    }
                }
            }

            for j in 0..n {
                if i != j && !self.is_dead(j) && !self.is_dead(i) && self.frogs_collide(i, j) {
                    self.kill_mp_frog(i as i32);
                    self.kill_mp_frog(j as i32);
                }
            }

            if self.is_dead(i) {
                dead += 1;
            }
        }

        // The last player alive wins!
        if n > 1 && dead == n - 1 {
            let mut top_score: i16 = 0;
            let mut num_top = 0;
            for i in 0..n {
                if !self.is_dead(i) {
                    self.multi.mpl[i].score = self.multi.mpl[i].score.wrapping_add(1);
                }
            }
            for i in 0..n {
                if self.multi.mpl[i].score > top_score {
                    top_score = self.multi.mpl[i].score;
                }
            }
            for i in 0..n {
                if (self.multi.mpl[i].score as i32 - top_score as i32).abs() < 2 {
                    self.multi.mpl[i].score = top_score;
                    num_top += 1;
                    self.multi.game_winner = i as i8;
                }
            }

            if num_top > 1 {
                self.cam.control_camera = 1;
                self.multi.game_winner = MULTI_ROUND_DRAW;
            } else {
                let w = self.multi.game_winner;
                self.play_voice(w as i32, "frogokay");
                let m = &mut self.multi.mpl[w as usize & 3];
                m.wins = m.wins.wrapping_add(1);
            }

            self.multi.end_timer.init(2);
        } else if dead == n {
            self.multi.game_winner = MULTI_ROUND_DRAW;
            self.multi.end_timer.init(2);
        }
    }

    /// `currTile[j] == currTile[i] || (destTile[j] && destTile[i] && destTile[j] == destTile[i])`.
    fn frogs_collide(&self, i: usize, j: usize) -> bool {
        let fm = &self.frogmove;
        fm.curr_tile[j] == fm.curr_tile[i] || (fm.dest_tile[j].is_some() && fm.dest_tile[i].is_some() && fm.dest_tile[j] == fm.dest_tile[i])
    }

    /// `AnimateFrogHop` (Common/frogmove.c:971) with `frogAnimSpeed`/`frogAnimSpeed2`
    /// halved, as `UpdateBattle` does around its call.
    fn animate_frog_hop_half_speed(&mut self, direction: i32, pl: i32) {
        let p = pl as usize;
        let Some(f) = self.frogger.frog[p] else { return };
        if self.frogger.player[p].is_super_hopping != 0 {
            let anim_speed = self.frogger.player[p].jump_speed.wrapping_mul(15) >> 4;
            self.actor_animate(f, FROG_ANIM_SUPERHOP, false, false, anim_speed, false);
            return;
        }
        match (direction - self.frogmove.frog_facing[p]) & 3 {
            1 => self.actor_animate(f, FROG_ANIM_HOPLEFT, false, false, FROG_ANIM_SPEED2 / 2, false),
            3 => self.actor_animate(f, FROG_ANIM_HOPRIGHT, false, false, FROG_ANIM_SPEED2 / 2, false),
            _ => self.actor_animate(f, FROG_ANIM_HOP, false, false, FROG_ANIM_SPEED / 2, false),
        }
    }

    /// Port of `UpdateCollect` (Common/multi.c:579): game mechanics for Frogger1 collection mode.
    pub fn update_collect(&mut self) {
        let n = self.num_frogs_mp();
        let mut total: i32 = 0;

        if self.multi.started == 0 {
            for i in 0..n {
                self.frogger.player[i].can_jump = 0;
                if let Some(f) = self.frogger.frog[i] {
                    orientate(&mut self.actor2.actors[f].actor.q_rot, &OUT_VEC, &UP_VEC);
                }
            }

            self.multi.end_timer.init(0);
            let r = self.rng.random(4);
            self.multi.powerup_timer.init(r + 3);
            self.multi.multi_timer.init(6);
            self.multi.started = 1;
        }

        // When all players are ready, start a countdown
        if self.multi.multi_timer.time != 0 {
            self.mp_countdown(false);
        } else if self.multi.end_timer.time != 0 {
            let afc = self.act_frame_count;
            self.multi.end_timer.update(-1, afc);
            if self.multi.end_timer.time == 0 {
                self.start_multi_win_game();
            }
            return;
        }

        if self.multi.started == 2 {
            for i in 0..n {
                // Check for collection of objects and update total
                total = total.wrapping_add(self.multi.mpl[i].score as i32);

                // If waiting to respawn, update timer and check for respawn timeout
                if self.frogger.player[i].dead.time != 0 {
                    let afc = self.act_frame_count;
                    self.frogger.player[i].dead.update(-1, afc);
                    if self.frogger.player[i].dead.time == 0 {
                        self.frogger.player[i].frog_state &= !FROGSTATUS_ISDEAD;
                        self.collect_respawn(i as i32);
                        self.set_frog_draw(i, 1);
                    } else {
                        continue;
                    }
                }

                // Respawn on start tile when dead, after countdown
                if self.is_dead(i) {
                    self.frogger.player[i].dead.init(4);
                }
            }

            // Find the highest scoring frog
            let mut best: i16 = 0;
            let mut winner: i32 = -1;
            for i in 0..n {
                if self.multi.mpl[i].score >= best {
                    best = self.multi.mpl[i].score;
                    winner = i as i32;
                }
            }

            // Check if any other player can possibly win (if score difference is less than number of remaining items)
            if winner >= 0 {
                let ws = self.multi.mpl[winner as usize].score as i32;
                for i in 0..n {
                    if i as i32 != winner && ws - self.multi.mpl[i].score as i32 <= self.map.num_babies.wrapping_sub(total) {
                        winner = -1;
                        break;
                    }
                }
            }

            // If the leading frog has an unassailable lead then he wins
            if winner != -1 {
                self.play_voice(winner, "frogokay");
                self.multi.game_winner = winner as i8;
                let m = &mut self.multi.mpl[winner as usize];
                m.wins = m.wins.wrapping_add(1);
                self.multi.end_timer.init(2);
            } else if total == self.map.num_babies {
                // If all objects collected then declare draw
                self.multi.game_winner = MULTI_ROUND_DRAW;
                self.multi.end_timer.init(2);
            }
        }
    }

    /// Port of `RunMultiplayer` (Common/multi.c:695): call update function for each game type.
    pub fn run_multiplayer(&mut self) {
        self.up_date_multiplayer_info();

        match self.multi.multiplayer_mode {
            MULTIMODE_RACE => self.update_race(),
            MULTIMODE_BATTLE => self.update_battle(),
            MULTIMODE_COLLECT => self.update_collect(),
            _ => {}
        }

        for i in 0..self.num_frogs_mp() {
            let show = self.multi.multi_timer.time > 1;
            let pos = self.frog_position(i);
            let (x, y) = self.transform_point(&pos);
            if let Some(t) = self.textover.get_mut(self.multi.mpl[i].num_text) {
                t.draw = show as u8;
                t.x_pos = (x as i16).wrapping_add(MPLTEXTX as i16);
                t.y_pos = (y as i16).wrapping_add(MPLTEXTY as i16);
            }
            if self.multi.mpl[i].wins == 3 {
                if self.multi.match_winner == -1 {
                    self.multi.end_timer.init(2);
                    self.multi.match_winner = i as i8;
                }
                break;
            }
        }
    }

    /// Port of `RaceRespawn` (Common/multi.c:768): teleport frog to a non-dead player.
    /// Respawn near the lead frog if available, or search for another if not.
    pub fn race_respawn(&mut self, pl: i32) {
        let p = pl as usize;
        let n = self.num_frogs_mp();
        let mut respawn = 0usize;

        self.frogger.player[p].death_by = 0;
        self.frogger.player[p].frog_state &= !FROGSTATUS_ISDEAD;
        if let Some(f) = self.frogger.frog[p] {
            self.actor_animate(f, FROG_ANIM_BREATHE, true, false, 64, false);
        }

        let pf = self.cam.player_focus as usize & 3;
        if pf != p && !self.is_dead(pf) && self.is_point_visible(&self.frog_position(pf)) {
            respawn = pf;
        } else {
            for j in 0..n {
                if j != p && j != pf && !self.is_dead(j) && self.is_point_visible(&self.frog_position(j)) {
                    respawn = j;
                    break;
                }
            }
        }

        let mut diff = FVector::default();
        sub_vector_fss(&mut diff, &self.frog_position(p), &self.frog_position(respawn));
        make_unit(&mut diff);

        // Find a tile to teleport to - first try a tile next to the target, then on the nearest "safe" tile, then on the target frog if that fails
        let mut tile = self.frogmove.curr_tile[respawn].and_then(|ct| find_joined_tile_by_direction_and_type(&self.map, ct, &diff, tilestate::NORMAL));
        if let Some(t) = tile {
            if (0..n).any(|j| j != p && self.frogmove.curr_tile[j] == Some(t)) {
                tile = None;
            }
        }
        if tile.is_none() {
            let mut target = None;
            let mut best: i32 = 99_999_999;
            for (ti, t) in self.map.tiles.iter().enumerate() {
                let tid = TileId(ti as u32);
                if t.state == tilestate::SAFE || t.state == tilestate::NORMAL || is_frogger_area(t.state) {
                    if (0..n).any(|j| j != p && self.frogmove.curr_tile[j] == Some(tid)) {
                        continue;
                    }
                    let dist = distance_between_points_ss(&self.frog_position(respawn), &t.centre)
                        .wrapping_add(distance_between_points_ss(&self.frog_position(p), &t.centre));
                    if dist < best && self.is_point_visible(&t.centre) {
                        target = Some(tid);
                        best = dist;
                    }
                }
            }
            tile = target.or(self.frogmove.curr_tile[respawn]);
        }

        self.cam.cam_facing[p] = self.cam.cam_facing[respawn];
        self.frogmove.frog_facing[p] = self.frogmove.frog_facing[respawn];
        self.cam.prev_cam_facing[p] = self.cam.prev_cam_facing[respawn];

        let Some(tile) = tile else { return }; // the original would dereference NULL
        if let Some(f) = self.frogger.frog[p] {
            self.teleport_actor_to_tile(f, tile, pl);
        }

        self.create_respawn_effect(tile, pl);

        // Update progress info
        self.multi.mpl[p].check = self.multi.mpl[respawn].check;
        self.multi.mpl[p].lap = self.multi.mpl[respawn].lap;

        // If other frog has finished then we have to be 1 checkpoint behind
        if self.multi.mpl[respawn].ready != 0 {
            self.multi.mpl[p].lasttile = tilestate::FROGGER4AREA as i16;
        } else {
            self.multi.mpl[p].lasttile = self.multi.mpl[respawn].lasttile;
        }

        self.cam.last_tile[p] = Some(tile);
        self.frogmove.dest_tile[p] = Some(tile);

        self.check_for_frog_on(pl, Some(tile));
    }

    /// Port of `CollectRespawn` (Common/multi.c:902): just respawn on the proper respawn tile.
    pub fn collect_respawn(&mut self, pl: i32) {
        let p = pl as usize;
        let Some(start) = self.map.g_t_start[p] else { return }; // NULL in the original
        if let Some(f) = self.frogger.frog[p] {
            self.teleport_actor_to_tile(f, start, pl);
            self.actor_animate(f, FROG_ANIM_BREATHE, true, false, FROG_BREATHE_SPEED, false);
        }

        self.cam.last_tile[p] = Some(start);
        self.frogmove.dest_tile[p] = Some(start);

        self.create_respawn_effect(start, pl);
        self.check_for_frog_on(pl, self.frogmove.curr_tile[p]);
    }

    /// Port of `BattleProcessController` (Common/multi.c:922): just change direction,
    /// and maybe use tongue.
    pub fn battle_process_controller(&mut self, pl: i32) {
        let p = pl as usize;
        let button = self.levplay.pad_data.debounce[p];

        // Change frog facing on direction keys
        self.frogger.player[p].frog_state &= !FROGSTATUS_ALLHOPFLAGS;

        let cf = self.cam.cam_facing[p];
        if button & PAD_UP != 0 {
            self.frogmove.frog_facing[p] = (cf + MOVE_UP) & 3;
        } else if button & PAD_DOWN != 0 {
            self.frogmove.frog_facing[p] = (cf + MOVE_DOWN) & 3;
        } else if button & PAD_LEFT != 0 {
            self.frogmove.frog_facing[p] = (cf + MOVE_LEFT) & 3;
        } else if button & PAD_RIGHT != 0 {
            self.frogmove.frog_facing[p] = (cf + MOVE_RIGHT) & 3;
        }

        self.frogmove.next_frog_facing[p] = self.frogmove.frog_facing[p];

        if button & PAD_START != 0 && !self.is_dead(p) {
            self.frogger.pause_controller = pl;
            self.start_pause_menu();
        }
    }

    /// Port of `AddBattleTrailNode` (Common/multi.c:956): add a node to the trail and put
    /// an effect on it. Returns FALSE when the frog hopped onto a trail.
    pub fn add_battle_trail_node(&mut self, i: i32) -> bool {
        let p = i as usize;
        let Some(dt) = self.frogmove.dest_tile[p] else { return true };
        if is_frogger_area(self.map.tile(dt).state) {
            return false;
        }

        // Return if allocation is impossible for any reason
        if self.multi.node_pool.stack_ptr < 1 || self.multi.node_pool.array.is_empty() {
            return true;
        }

        let pool = &mut self.multi.node_pool;
        let node = pool.stack[pool.stack_ptr as usize];
        pool.stack_ptr -= 1;

        let head = self.multi.mpl[p].path;
        {
            let nd = &mut self.multi.node_pool.array[node as usize];
            nd.prev = None;
            nd.next = head;
        }
        if let Some(h) = head {
            self.multi.node_pool.array[h as usize].prev = Some(node);
        }
        self.multi.mpl[p].path = Some(node);

        self.multi.node_pool.count = self.multi.node_pool.count.wrapping_add(1);

        // Found a pathnode, continue adding it
        self.multi.node_pool.array[node as usize].tile = Some(dt);
        self.map.tiles[dt.idx()].state = tilestate::FROGGER1AREA + i as u32;

        self.set_node_fx_draw(node, 1);
        let centre = self.map.tile(dt).centre;
        if let Some(a) = self.multi.node_pool.array[node as usize].fx.and_then(|f| self.actor2.actors.get_mut(f)) {
            a.actor.position = centre;
        }
        self.set_node_size_vy(node, 10);

        // PC: overrideTex = GetTexEntryFromCRC(UpdateCRC("mplt%i.bmp", player[i].character))
        // (renderer state: the trail cube is tinted with the player's texture).
        true
    }

    /// Port of `SubBattleTrailNode` (Common/multi.c:1001).
    pub fn sub_battle_trail_node(&mut self, n: u16) {
        let (prev, next) = {
            let nd = &self.multi.node_pool.array[n as usize];
            (nd.prev, nd.next)
        };
        if let Some(pv) = prev {
            self.multi.node_pool.array[pv as usize].next = next;
        }
        if let Some(nx) = next {
            self.multi.node_pool.array[nx as usize].prev = prev;
        }
        {
            let nd = &mut self.multi.node_pool.array[n as usize];
            nd.next = None;
            nd.prev = None;
        }

        self.set_node_fx_draw(n, 0);
        if let Some(t) = self.multi.node_pool.array[n as usize].tile {
            self.map.tiles[t.idx()].state = tilestate::NORMAL;
        }
        self.multi.node_pool.array[n as usize].tile = None;

        let pool = &mut self.multi.node_pool;
        pool.count = pool.count.wrapping_sub(1);
        pool.stack_ptr += 1;
        pool.stack[pool.stack_ptr as usize] = n;
    }

    /// Port of `FaceFrogToCentre` (Common/multi.c:1027): make a player face the middle of
    /// the battle arena; `toggle` switches between face in/out.
    pub fn face_frog_to_centre(&mut self, pl: i32, toggle: i32) {
        let p = pl as usize;
        let Some(ct) = self.frogmove.curr_tile[p] else { return };
        let mut best: Fixed = -1;
        let mut facing = 0;

        // Find direction to centre
        let mut dir = FVector::default();
        set_vector_fs(&mut dir, &self.frog_position(p));
        make_unit(&mut dir);
        scale_vector(&mut dir, toggle);

        let tile = self.map.tile(ct).clone();
        for i in 0..4 {
            let dp = dot_product_ff(&dir, &ff_from_s(&tile.dir_vector[i]));
            if dp > best {
                facing = i as i32;
                best = dp;
            }
        }

        self.frogmove.frog_facing[p] = facing;
        if let Some(f) = self.frogger.frog[p] {
            orientate_ss(&mut self.actor2.actors[f].actor.q_rot, &tile.dir_vector[facing as usize], &tile.normal);
        }
    }

    /// Port of `PickupBabyFrogMulti` (Common/multi.c:1064): player has collected baby frog
    /// in a multiplayer game. Probably not an actual baby frog, more likely a flag or
    /// other level collectable.
    pub fn pickup_baby_frog_multi(&mut self, baby: EnemyId, pl: i32) {
        let p = pl as usize;
        let nb = (self.map.num_babies.max(0) as usize).min(NUM_BABIES);
        let (nme_actor, in_tile, path) = {
            let e = &self.enemies.enemy_list.array[baby.idx()];
            (e.nme_actor, e.in_tile, e.path)
        };

        let Some(i) = (0..nb).find(|&i| self.babyfrog.baby_list[i].baby == nme_actor) else { return };
        if self.babyfrog.baby_list[i].is_saved != 0 {
            return;
        }

        let off2 = if path.num_nodes > 0 { self.path.node(&path, 0).offset2 } else { 0 };
        let mut n = if off2 != 0 { ((off2 >> 12) / SCALE) - 1 } else { i as i32 };
        if n > self.map.num_babies {
            n = 0;
        }

        self.babyfrog.last_baby_saved = i as i32;
        self.babyfrog.baby_list[i].is_saved = 1;
        self.babyfrog.baby_list[i].collect = 40;
        // BUG(original)?: babiesSaved is not incremented in multiplayer, so this only hides
        // the baby on single-baby levels.
        if self.babyfrog.babies_saved as i32 == self.map.num_babies - 1 {
            if let Some(a) = nme_actor.and_then(|a| self.actor2.actors.get_mut(a)) {
                a.draw = 0;
            }
        }

        if self.multi.multiplayer_mode == MULTIMODE_COLLECT {
            self.multi.mpl[p].score = self.multi.mpl[p].score.wrapping_add(1);
            self.frogger.player[p].dead.init(2);

            self.mp_sample("getbabyfrog"); // genSfx[GEN_COLLECT_BABY]
            self.play_voice(pl, "frogokay");

            self.baby_collect_effect(nme_actor, in_tile, n);
        }
    }

    /// Port of `KillMPFrog` (Common/multi.c:1112): subtract healthpoint or kill, or
    /// respawn if appropriate.
    pub fn kill_mp_frog(&mut self, pl: i32) {
        let p = pl as usize;
        self.frogger.player[p].frog_state |= FROGSTATUS_ISDEAD;

        // A standard multiplayer death
        if let Some(t) = self.frogmove.curr_tile[p] {
            self.create_respawn_effect(t, pl);
        }

        self.mp_sample("frogexplode"); // genSfx[GEN_DEATHEXPLODE]

        match self.multi.multiplayer_mode {
            MULTIMODE_RACE => {
                let m = &mut self.multi.mpl[p];
                m.penalty = m.penalty.wrapping_add(MULTI_RACE_TIMEPENALTY);
                let (x, y) = self.transform_point(&self.frog_position(p));
                let m = &mut self.multi.mpl[p];
                m.scr_x = x as i16;
                m.scr_y = y as i16;
                let (pt, sx, sy) = (m.penal_text, m.scr_x, m.scr_y);
                if let Some(t) = self.textover.get_mut(pt) {
                    t.draw = 1;
                    t.x_pos = sx;
                    t.y_pos = sy;
                }
                self.frogger.player[p].dead.init(2);
                self.set_frog_draw(p, 0);
            }
            MULTIMODE_BATTLE | MULTIMODE_COLLECT => {
                self.frogger.player[p].dead.init(2);
                self.set_frog_draw(p, 0);
            }
            _ => {
                let pp = &mut self.frogger.player[p];
                pp.stun.init(2);
                pp.safe.init(5);
                pp.health_points = 0;
                pp.death_by = DEATHBY_NORMAL as u8;
                self.set_frog_draw(p, 0);
            }
        }
    }

    /// Port of `FreeMultiplayer` (Common/multi.c:1162): deallocate any data that needs it.
    pub fn free_multiplayer(&mut self) {
        if !self.multi.node_pool.array.is_empty() {
            // Remove all particles in array from particle list so they don't get removed after deallocation
            for i in 0..self.multi.node_pool.array.len() {
                if let Some(fx) = self.multi.node_pool.array[i].fx {
                    self.actor2_free(fx);
                }
                self.multi.node_pool.array[i].next = None;
            }
            self.multi.node_pool.array = Vec::new();
        }
        self.multi.node_pool.stack = Vec::new();
        self.multi.node_pool.count = 0;
        self.multi.node_pool.stack_ptr = 0;
    }

    /// Port of `ReinitialiseMultiplayer` (Common/multi.c:1200): set mp data back to very
    /// initial values.
    pub fn reinitialise_multiplayer(&mut self) {
        self.multi.match_winner = -1;
        self.multi.mpl = Default::default();

        if self.multi.multiplayer_mode == MULTIMODE_BATTLE {
            self.multi.node_pool.array = vec![BattleNode::default(); MAX_BATTLENODES];
            self.multi.node_pool.stack = vec![0; MAX_BATTLENODES];
            for i in 0..MAX_BATTLENODES {
                self.multi.node_pool.stack[i] = i as u16;
                let fx = self.create_and_add_actor("cube.obe", 0, 0, 0, 0);
                self.multi.node_pool.array[i].fx = fx;
                self.set_node_fx_draw(i as u16, 0);
            }
            self.multi.node_pool.count = 0;
            self.multi.node_pool.stack_ptr = MAX_BATTLENODES as i16 - 1;
        }

        for i in 0..self.num_frogs_mp() {
            // PC MULTIREMOTE nameText: network play is not ported.
            // numberText[i] (options.c:496)
            let num = ["1", "2", "3", "4"][i];
            self.multi.mpl[i].num_text = self.textover.create_and_add_text_overlay(-1000, 0, num, true, 255, Font::Normal, TEXTOVERLAY_SHADOW);
            self.multi.mpl[i].trail = if self.game_state.difficulty == DIFFICULTY_HARD { MULTI_BATTLE_TRAILLENGTH * 2 } else { MULTI_BATTLE_TRAILLENGTH };
            self.multi.mpl[i].lasttile = tilestate::FROGGER1AREA as i16;
            if self.multi.multiplayer_mode == MULTIMODE_RACE {
                // penalStr (multi.c:63)
                let t = self.textover.create_and_add_text_overlay(0, 0, "+1", false, 255, Font::Normal, TEXTOVERLAY_SHADOW);
                if let Some(o) = self.textover.get_mut(t) {
                    o.g = 0;
                    o.b = 0;
                    o.draw = 0;
                }
                self.multi.mpl[i].penal_text = t;
            }
        }
    }

    /// Port of `ResetMultiplayer` (Common/multi.c:1296): set multiplayer info to initial
    /// values EXCEPT the stuff that needs to be preserved between levels.
    pub fn reset_multiplayer(&mut self) {
        let n = self.num_frogs_mp();
        self.multi.started = 0;
        self.cam.player_focus = 0;
        self.multi.game_winner = -1;
        self.multi.match_winner = -1;
        self.multi.powerup_timer.init(0);

        for i in 0..n {
            self.multi.mpl[i].ready = 0;
            self.frogger.player[i].idle_enable = 1;
            self.frogger.player[i].can_jump = 1;
        }

        match self.multi.multiplayer_mode {
            MULTIMODE_COLLECT => {
                for i in 0..n {
                    self.multi.mpl[i].ready = 0;
                    self.multi.mpl[i].score = 0;
                    if let Some(t) = self.map.g_t_start[i] {
                        self.set_frogger_start_pos(t, i as i32);
                    }
                }
                let nb = (self.map.num_babies.max(0) as usize).min(NUM_BABIES);
                for i in 0..nb {
                    let b = &mut self.babyfrog.baby_list[i];
                    b.is_saved = 0;
                    b.idle = 0;
                    b.collect = 0;
                    let (baby, enemy) = (b.baby, b.enemy);
                    if let Some(a) = baby.and_then(|a| self.actor2.actors.get_mut(a)) {
                        a.draw = 1;
                    }
                    self.babyfrog.babies_saved = 0;
                    self.babyfrog.last_baby_saved = -1;
                    if let Some(e) = enemy {
                        self.move_enemy_to_node(e, 0);
                    }
                    if let Some(a) = baby {
                        self.actor_animate(a, BABY_ANIM_SNOOZE, true, false, 100, false);
                    }
                }
            }
            MULTIMODE_BATTLE => {
                for i in 0..n {
                    self.multi.mpl[i].trail =
                        if self.game_state.difficulty == DIFFICULTY_HARD { MULTI_BATTLE_TRAILLENGTH * 2 } else { MULTI_BATTLE_TRAILLENGTH };
                    self.multi.mpl[i].ready = 0;
                    self.multi.mpl[i].score = 0;
                    let mut node = self.multi.mpl[i].path;
                    while let Some(nd) = node {
                        if self.multi.node_pool.array.is_empty() {
                            break;
                        }
                        let temp = self.multi.node_pool.array[nd as usize].next;
                        self.sub_battle_trail_node(nd);
                        node = temp;
                    }
                    self.multi.mpl[i].path = None;
                    if let Some(t) = self.map.g_t_start[i] {
                        self.set_frogger_start_pos(t, i as i32);
                    }
                }

                self.free_spec_fx_list();
                self.init_spec_fx_list();
                self.free_garib_list();
                self.init_garib_list();
                self.alloc_garib_block(MULTI_BATTLE_MAXITEMS as i32);
                // PC: InitSpriteSortArray() (renderer).
                self.multi.num_multi_items = 0;
                self.cam.scale_v = SCALE_V_INIT;
            }
            MULTIMODE_RACE => {
                for i in 0..n {
                    let m = &mut self.multi.mpl[i];
                    m.check = 0;
                    m.lap = 0;
                    m.lasttile = tilestate::FROGGER1AREA as i16;
                    m.timer = 0;
                    m.penalty = 0;
                    if let Some(t) = self.map.g_t_start[i] {
                        self.set_frogger_start_pos(t, i as i32);
                    }
                }
            }
            _ => {}
        }

        self.init_camera();
    }

    /// Port of `CalcMPCamera` (Common/multi.c:1384): different camera targets for
    /// different game types. Called by the camera (cam.c `CalcSPCamera` counterpart).
    pub fn calc_mp_camera(&mut self, target: &mut FVector) {
        match self.multi.multiplayer_mode {
            MULTIMODE_RACE => {
                if self.frogger.player[0].world_num as u32 == worldid::SUBTERRANEAN {
                    self.calc_race_circuit_camera(target);
                } else {
                    self.calc_race_straight_camera(target);
                }
            }
            MULTIMODE_BATTLE => self.calc_battle_camera(target),
            MULTIMODE_COLLECT => self.calc_collect_camera(target),
            _ => {}
        }
    }

    /// The linear position of a jumping frog, else its actor position (as an `FVECTOR`).
    fn mp_frog_cam_pos(&self, i: usize) -> FVector {
        let pp = &self.frogger.player[i];
        let mut v = FVector::default();
        if pp.jump_time > 0 {
            // jumping; calculate linear position
            v = pp.jump_fwd_vector;
            scale_vector_ff(&mut v, pp.jump_time);
            add_to_vector_fs(&mut v, &pp.jump_origin);
        } else {
            set_vector_fs(&mut v, &self.frog_position(i));
        }
        v
    }

    /// Port of `CalcRaceCircuitCamera` (Common/multi.c:1414): focus on the average point
    /// of all the lead players.
    pub fn calc_race_circuit_camera(&mut self, target: &mut FVector) {
        let n = self.num_frogs_mp();
        let (mut best_lap, mut best_check) = (0i16, 0i16);
        *target = FVector::default();

        let gw = self.multi.game_winner;
        if gw != -1 && gw != MULTI_ROUND_DRAW {
            self.cam.player_focus = gw;
            set_vector_fs(target, &self.frog_position(gw as usize & 3));
            return;
        }

        for i in 0..n {
            let m = &self.multi.mpl[i];
            if m.lap >= best_lap && m.check >= best_check {
                best_lap = m.lap;
                best_check = m.check;
                self.cam.player_focus = i as i8;
            }
        }

        let mut num = 0;
        for i in 0..n {
            if self.multi.mpl[i].lap == best_lap && self.multi.mpl[i].check == best_check {
                num += 1;
                let v = self.mp_frog_cam_pos(i);
                add_to_vector_ff(target, &v);
            }
        }

        scale_vector_ff(target, 4096 / num.max(1));
    }

    /// Port of `CalcRaceStraightCamera` (Common/multi.c:1466): focus on the average point
    /// of all players, double weight for the lead player.
    pub fn calc_race_straight_camera(&mut self, target: &mut FVector) {
        let n = self.num_frogs_mp();
        let (mut best_lap, mut best_check, mut best_z) = (0i16, 0i16, 999_999_999i32);
        *target = FVector::default();

        let gw = self.multi.game_winner;
        if gw != -1 && gw != MULTI_ROUND_DRAW {
            self.cam.player_focus = gw;
            set_vector_fs(target, &self.frog_position(gw as usize & 3));
            return;
        }

        for i in 0..n {
            let m = self.multi.mpl[i];
            let z = self.frog_position(i).vz as i32;
            if m.lap >= best_lap && m.check >= best_check && z < best_z {
                best_lap = m.lap;
                best_check = m.check;
                best_z = z;
                self.cam.player_focus = i as i8;
            }
        }

        for i in 0..n {
            let diff = self.mp_frog_cam_pos(i);
            add_to_vector_ff(target, &diff);
            // Double weight for lead player
            if i as i32 == self.cam.player_focus as i32 {
                add_to_vector_ff(target, &diff);
            }
        }

        let scale = 4096 / (self.frogger.num_frogs + 1);
        scale_vector_ff(target, scale);

        if target.vy < self.multi.initial_cam_height {
            target.vy = self.multi.initial_cam_height;
        }
    }

    /// Bounding box of the live frogs' (linear) positions: `(count, min, max)`.
    fn mp_live_frog_bounds(&self) -> (i32, FVector, FVector) {
        let (mut min_v, mut max_v) = (FVector::default(), FVector::default());
        let mut l = 0;
        for i in 0..self.num_frogs_mp() {
            if self.is_dead(i) {
                continue;
            }
            let v = self.mp_frog_cam_pos(i);
            if l == 0 {
                min_v = v;
                max_v = v;
            } else {
                min_v.vx = min_v.vx.min(v.vx);
                min_v.vy = min_v.vy.min(v.vy);
                min_v.vz = min_v.vz.min(v.vz);
                max_v.vx = max_v.vx.max(v.vx);
                max_v.vy = max_v.vy.max(v.vy);
                max_v.vz = max_v.vz.max(v.vz);
            }
            l += 1;
        }
        (l, min_v, max_v)
    }

    /// Port of `CalcCollectCamera` (Common/multi.c:1612): focus on average position of
    /// frogs and zoom depending on separation.
    pub fn calc_collect_camera(&mut self, target: &mut FVector) {
        let (l, min_v, max_v) = self.mp_live_frog_bounds();

        // Zoom in/out to keep multiplayer frogs in view
        let sc = self.find_max_inter_frog_distance();
        if sc != -4096 {
            let w = self.frogger.player[0].world_num as usize;
            let nsp = self.layout.world_visual_data.get(w).map_or(0, |v| v.num_single_player_levels as i32);
            let idx = self.frogger.player[0].level_num as i32 - nsp;
            // BUG(original): camDiv has 3 initialised entries; other indices read past the
            // array or divide by zero. Leave scaleV alone there.
            if (0..4).contains(&idx) && CAM_DIV[idx as usize] != 0 {
                self.cam.scale_v = sc / CAM_DIV[idx as usize] + CAM_PLUS[idx as usize];
            }
        }

        if l > 0 {
            add_vector_fff(target, &min_v, &max_v);
            scale_vector_ff(target, 2048);
        }
    }

    /// Port of `CalcBattleCamera` (Common/multi.c:1683): focus on average position of
    /// frogs and zoom depending on separation.
    pub fn calc_battle_camera(&mut self, target: &mut FVector) {
        let (l, min_v, max_v) = self.mp_live_frog_bounds();

        // Zoom in/out to keep multiplayer frogs in view
        let sc = self.find_max_inter_frog_distance();
        if sc > 0 {
            self.cam.scale_v = sc / BATTLE_CAM_FAC + BATTLE_CAM_PLUS;
        }

        if l > 0 {
            add_vector_fff(target, &min_v, &max_v);
            scale_vector_ff(target, 2048);
        }
    }

    /// Port of `RestartMulti` (Common/multi.c:1799): start a new match.
    pub fn restart_multi(&mut self) {
        self.multi.match_winner = -1;
        for i in 0..self.num_frogs_mp() {
            let m = &mut self.multi.mpl[i];
            m.check = 0;
            m.lap = 0;
            m.penalty = 0;
            m.score = 0;
            m.wins = 0;
            for j in 0..3 {
                let t = self.hud.multi_hud.trophies[i][j];
                self.overlays.disable(t);
            }
        }
        self.reset_multiplayer();
        let ct = self.hud.multi_hud.centre_text;
        self.textover.enable(ct);
        self.enable_hud();
    }

    /// Port of `PlayerRaceWin` (Common/multi.c:1813).
    pub fn player_race_win(&mut self, pl: i32) {
        let p = pl as usize;
        self.multi.mpl[p].ready = 1;
        self.frogger.player[p].can_jump = 0;
        if let Some(f) = self.frogger.frog[p] {
            self.actor_animate(f, FROG_ANIM_LOOKAROUND, false, false, 80, false);
            self.actor_animate(f, FROG_ANIM_HANDSPRING, false, true, 80, false);
            self.actor_animate(f, FROG_ANIM_BREATHE, true, true, 50, false);
        }
        let c = self.frogger.frog_pool(self.frogger.player[p].character as usize % crate::frogger::FROG_NUMFROGS);
        self.create_pickup_effect(pl, c.r, c.g, c.b, 255, 255, 255);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::define::mode::INGAME_MODE;
    use crate::define::MULTILOCAL;
    use crate::map::GameTile;

    /// A game with `n` frogs (no actors) on a row of `tiles` plain tiles; frog i on tile i.
    fn mp_game(n: i32, mode: i32, tiles: usize) -> Game {
        let mut g = Game::new();
        g.game_state.multi = MULTILOCAL;
        g.game_state.mode = INGAME_MODE;
        g.frogger.num_frogs = n;
        g.multi.multiplayer_mode = mode;
        g.textover.init_text_overlay_linked_list(40);
        g.overlays.init_sprite_overlay_linked_list();
        g.map.tiles = vec![GameTile::default(); tiles];
        for i in 0..n as usize {
            g.frogmove.curr_tile[i] = Some(TileId(i as u32));
            g.map.g_t_start[i] = Some(TileId(i as u32));
        }
        g
    }

    /// Steps the frame counter by `frames` and runs `f` each frame.
    fn run(g: &mut Game, frames: u32, mut f: impl FnMut(&mut Game)) {
        for _ in 0..frames {
            if g.game_state.mode != INGAME_MODE {
                break; // the win screen (frontend) takes over
            }
            g.last_act_frame_count = g.act_frame_count;
            g.act_frame_count += 1;
            f(g);
        }
    }

    #[test]
    fn race_best_time_wins_and_penalties_count() {
        let mut g = mp_game(3, MULTIMODE_RACE, 4);
        g.multi.started = 2;
        g.multi.mpl[0] = MpInfo { ready: 1, timer: 1000, penalty: 120, ..Default::default() };
        g.multi.mpl[1] = MpInfo { ready: 1, timer: 1100, ..Default::default() };
        g.multi.mpl[2] = MpInfo { ready: 1, timer: 1050, penalty: 60, ..Default::default() };
        g.act_frame_count = 10;
        g.last_act_frame_count = 9;
        g.update_race();
        assert_eq!(g.multi.game_winner, 1);
        assert_eq!(g.multi.mpl[1].wins, 1);
        assert_eq!(g.multi.end_timer.time, 2);
        assert_eq!(g.cam.control_camera, 1);

        // the end timer then counts down two seconds and opens the win screen
        run(&mut g, 200, |g| g.update_race());
        assert_eq!(g.game_state.mode, crate::define::mode::MULTI_WINRACE_MODE);
        assert_eq!(g.multi.mpl[1].wins, 1, "round scored once");
    }

    #[test]
    fn race_tie_is_a_draw() {
        let mut g = mp_game(2, MULTIMODE_RACE, 4);
        g.multi.started = 2;
        g.multi.mpl[0] = MpInfo { ready: 1, timer: 1000, penalty: 60, ..Default::default() };
        g.multi.mpl[1] = MpInfo { ready: 1, timer: 1060, ..Default::default() };
        g.update_race();
        assert_eq!(g.multi.game_winner, MULTI_ROUND_DRAW);
        assert_eq!(g.multi.mpl[0].wins + g.multi.mpl[1].wins, 0);
    }

    #[test]
    fn race_laps_and_checkpoints() {
        let mut g = mp_game(2, MULTIMODE_RACE, 8);
        g.multi.started = 2;
        g.frame_count = 0;
        for i in 0..2 {
            g.multi.mpl[i].lasttile = tilestate::FROGGER1AREA as i16;
        }
        g.multi.mpl[1].ready = 1; // only frog 0 races
        let states = [tilestate::FROGGER2AREA, tilestate::FROGGER3AREA, tilestate::FROGGER4AREA, tilestate::FROGGER2AREA];
        for (k, s) in states.iter().enumerate() {
            g.map.tiles[4 + k].state = *s;
            g.frogmove.curr_tile[0] = Some(TileId(4 + k as u32));
            g.update_race();
        }
        assert_eq!(g.multi.mpl[0].check, 4);
        assert_eq!(g.multi.mpl[0].lap, 0);
        g.map.tiles[3].state = tilestate::FINISHLINE;
        g.frogmove.curr_tile[0] = Some(TileId(3));
        g.update_race();
        assert_eq!((g.multi.mpl[0].lap, g.multi.mpl[0].check, g.multi.mpl[0].ready), (1, 0, 1));
        // both ready: frog 0 had the lower time (frog 1 never ran)
        assert_eq!(g.multi.end_timer.time, 2);
    }

    #[test]
    fn kill_mp_frog_race_penalty_and_respawn_timer() {
        let mut g = mp_game(2, MULTIMODE_RACE, 4);
        g.reinitialise_multiplayer();
        g.kill_mp_frog(1);
        assert_eq!(g.multi.mpl[1].penalty, MULTI_RACE_TIMEPENALTY);
        assert_eq!(g.frogger.player[1].dead.time, 2);
        assert!(g.frogger.player[1].frog_state & FROGSTATUS_ISDEAD != 0);
        assert_eq!(g.textover.get(g.multi.mpl[1].penal_text).unwrap().draw, 1);
    }

    #[test]
    fn battle_last_frog_standing_and_close_scores_draw() {
        let mut g = mp_game(3, MULTIMODE_BATTLE, 4);
        g.multi.started = 2;
        g.multi.multi_timer.init(0);
        g.multi.powerup_timer.init(5);
        for i in 0..3 {
            g.frogger.player[i].can_jump = 0;
        }
        g.frogger.player[1].frog_state |= FROGSTATUS_ISDEAD;
        g.frogger.player[2].frog_state |= FROGSTATUS_ISDEAD;
        g.multi.mpl[0].score = 10;
        g.multi.mpl[1].score = 40;
        g.multi.mpl[2].score = 3;
        g.update_battle();
        // survivor gets +1 but frog 1 still has the top score
        assert_eq!(g.multi.mpl[0].score, 11);
        assert_eq!(g.multi.game_winner, 1);
        assert_eq!(g.multi.mpl[1].wins, 1);

        let mut g = mp_game(2, MULTIMODE_BATTLE, 4);
        g.multi.started = 2;
        g.multi.powerup_timer.init(5);
        g.frogger.player[0].can_jump = 0;
        g.frogger.player[1].can_jump = 0;
        g.frogger.player[1].frog_state |= FROGSTATUS_ISDEAD;
        g.multi.mpl[0].score = 20;
        g.multi.mpl[1].score = 22;
        g.update_battle();
        // 21 vs 22: within one -> both level and a draw
        assert_eq!(g.multi.game_winner, MULTI_ROUND_DRAW);
        assert_eq!(g.multi.mpl[0].score, 22);
        assert_eq!(g.multi.mpl[0].wins + g.multi.mpl[1].wins, 0);
    }

    #[test]
    fn battle_head_on_collision_kills_both() {
        let mut g = mp_game(2, MULTIMODE_BATTLE, 4);
        g.multi.started = 2;
        g.multi.powerup_timer.init(5);
        g.frogger.player[0].can_jump = 0;
        g.frogger.player[1].can_jump = 0;
        g.frogmove.curr_tile[1] = Some(TileId(0));
        g.update_battle();
        assert!(g.is_dead(0) && g.is_dead(1));
        assert_eq!(g.multi.game_winner, MULTI_ROUND_DRAW);
    }

    #[test]
    fn battle_trail_pool_add_fade_and_free() {
        let mut g = mp_game(2, MULTIMODE_BATTLE, 6);
        g.reinitialise_multiplayer();
        assert_eq!(g.multi.node_pool.stack_ptr, 255);
        for t in 2..5u32 {
            g.frogmove.dest_tile[0] = Some(TileId(t));
            assert!(g.add_battle_trail_node(0));
        }
        assert_eq!(g.multi.node_pool.count, 3);
        assert_eq!(g.map.tiles[4].state, tilestate::FROGGER1AREA);
        assert_eq!(g.multi.mpl[0].path, Some(253));
        // hopping onto a trail tile fails
        g.frogmove.dest_tile[1] = Some(TileId(3));
        assert!(!g.add_battle_trail_node(1));

        // a trail of length 1: older nodes fade out at 128/frame and free their tiles
        g.multi.mpl[0].trail = 1;
        g.multi.started = 2;
        g.multi.powerup_timer.init(50);
        g.frogger.player[0].can_jump = 0;
        g.frogger.player[1].can_jump = 0;
        g.frogmove.curr_tile[1] = Some(TileId(5));
        g.update_battle();
        assert_eq!(g.node_size_vy(254), 10 + 256, "inside the trail: fades in");
        assert_eq!(g.multi.node_pool.count, 2, "the oldest (alpha 10) node is freed");
        assert_eq!(g.map.tiles[2].state, tilestate::NORMAL);
        assert_eq!(g.multi.mpl[0].score, 1);

        g.reset_multiplayer();
        assert_eq!(g.multi.node_pool.count, 0);
        assert_eq!(g.multi.node_pool.stack_ptr, 255);
        assert_eq!(g.multi.mpl[0].path, None);
        assert!(g.map.tiles.iter().all(|t| t.state == tilestate::NORMAL));
    }

    #[test]
    fn collect_unassailable_lead_and_draw() {
        let mut g = mp_game(2, MULTIMODE_COLLECT, 4);
        g.map.num_babies = 5;
        g.multi.started = 2;
        g.multi.mpl[0].score = 3;
        g.multi.mpl[1].score = 1;
        // 3 - 1 = 2 > 5 - 4 remaining: player 0 wins
        g.update_collect();
        assert_eq!(g.multi.game_winner, 0);
        assert_eq!(g.multi.mpl[0].wins, 1);

        let mut g = mp_game(2, MULTIMODE_COLLECT, 4);
        g.map.num_babies = 5;
        g.multi.started = 2;
        g.multi.mpl[0].score = 2;
        g.multi.mpl[1].score = 1;
        g.update_collect();
        assert_eq!(g.multi.game_winner, -1, "player 1 can still catch up");

        let mut g = mp_game(2, MULTIMODE_COLLECT, 4);
        g.map.num_babies = 4;
        g.multi.started = 2;
        g.multi.mpl[0].score = 2;
        g.multi.mpl[1].score = 2;
        g.update_collect();
        assert_eq!(g.multi.game_winner, MULTI_ROUND_DRAW);
        run(&mut g, 200, |g| g.update_collect());
        assert_eq!(g.game_state.mode, crate::define::mode::MULTI_WINCOLLECT_MODE);
    }

    #[test]
    fn third_win_takes_the_match() {
        let mut g = mp_game(2, 0, 4);
        g.multi.mpl[1].wins = 3;
        g.run_multiplayer();
        assert_eq!(g.multi.match_winner, 1);
        assert_eq!(g.multi.end_timer.time, 2);
        g.restart_multi();
        assert_eq!(g.multi.match_winner, -1);
        assert_eq!(g.multi.mpl[1].wins, 0);
    }

    #[test]
    fn countdown_draws_random_once_at_go() {
        let mut g = mp_game(2, MULTIMODE_COLLECT, 4);
        g.update_collect();
        assert_eq!(g.multi.started, 1);
        assert_eq!(g.multi.multi_timer.time, 6);
        let seed = g.rng.rseed;
        let mut gos = 0;
        run(&mut g, 60 * 7, |g| {
            let before = g.multi.multi_timer.time;
            g.update_collect();
            if before == 1 && g.multi.multi_timer.time == 0 {
                gos += 1;
            }
        });
        assert_eq!(g.multi.started, 2);
        assert_eq!(gos, 1);
        let mut r = f2_math::Rng { rseed: seed };
        r.random(2);
        assert_eq!(g.rng.rseed, r.rseed, "exactly one Random() (the voice) after the start");
        assert!(g.frogger.player.iter().take(2).all(|p| p.can_jump == 1));
    }

    #[test]
    fn multi_hud_countdown_race_clock_and_trophies() {
        let mut g = mp_game(2, MULTIMODE_RACE, 4);
        g.init_multi_hud();
        let m = g.hud.multi_hud.clone();
        assert!(m.time_text_min.is_some() && m.penal_over[1].is_some() && m.back_chars[1].is_some());
        assert!(g.frontend.menu_text[0].is_some() && g.frontend.menu_text[1].is_some());

        g.multi.multi_timer.init(4);
        g.up_date_multiplayer_info();
        let ct = g.textover.get(m.centre_text).unwrap();
        assert_eq!((ct.text.as_str(), ct.r, ct.g, ct.b, ct.scale, ct.draw), ("3", 255, 0, 0, 6144, 1));
        assert_eq!(g.hud.countdown_string, "3");

        g.multi.multi_timer.init(0);
        g.multi.mpl[0].timer = 3 * 3600 + 61;
        g.multi.mpl[1].penalty = 120;
        g.up_date_multiplayer_info();
        let t = |id| g.textover.get(id).unwrap().text.clone();
        assert_eq!(g.textover.get(m.centre_text).unwrap().draw, 0);
        assert_eq!((t(m.penalise_text[0]), t(m.penalise_text[1])), ("00".to_string(), "02".to_string()));
        assert_eq!((t(m.time_text_min), t(m.time_text_sec), t(m.time_text_hsec)), (" 3:".to_string(), "01:".to_string(), "01".to_string()));

        g.multi.mpl[0].wins = 2;
        g.enable_multi_hud();
        let d = |g: &Game, id| g.overlays.get(id).unwrap().draw;
        assert_eq!((d(&g, m.trophies[0][0]), d(&g, m.trophies[0][1]), d(&g, m.trophies[0][2]), d(&g, m.trophies[1][0])), (1, 1, 0, 0));
        g.disable_multi_hud();
        assert_eq!(d(&g, m.back_chars[0]), 0);
    }

    #[test]
    fn transform_point_without_view_is_origin() {
        let g = mp_game(2, MULTIMODE_RACE, 1);
        assert_eq!(g.transform_point(&SVector::default()), (0, 0));
    }
}
