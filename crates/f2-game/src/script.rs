//! Port of `Common/script.c` / `script.h`: the level script interpreter.
//!
//! The original interprets the compiled `.FEV` bytes in place (`Interpret` /
//! `ExecuteCommand` / `LoadTrigger` walk a `UBYTE*`). The port runs the same
//! commands over the tree decoded by [`f2_formats::fev`], in the same order and with
//! the same success/failure semantics: a failing command stops its block
//! (`Interpret` returns 0), `EV_ON` attaches `InterpretEvent` with its sub-block
//! (a [`ScriptBlock`] path), `EV_IF` evaluates and frees its trigger immediately, and
//! `EV_RESETSCRIPT` kills all triggers and re-runs the outer block.
//!
//! Byte-level differences that cannot arise with retail data: a file the decoder
//! rejects (unknown opcode) runs nothing, where the original would run the commands
//! before the bad one and then fail (`InitLevelScript` then kills all triggers anyway).

use std::sync::Arc;

use f2_formats::fev::{Command, EffectParams, Script, Trigger as FevTrigger};
use f2_math::{add_vector_sss, scale_vector_s, FVector, Fixed};

use crate::define::mode::LEVELCOMPLETE_MODE;
use crate::define::SINGLEPLAYER;
use crate::effects::Effect;
use crate::event::{EventFunc, ScriptBlock, TriggerFunc, TRIGGER_LOGIC, TRIGGER_LOGIC2, TRIGGER_ONCE};
use crate::frogger::{FROG_ANIM_BREATHE, FROG_BREATHE_SPEED};
use crate::frogmove::{DEATHBY_NORMAL, FROGSTATUS_ALLHOPFLAGS, FROGSTATUS_ISDEAD, FROGSTATUS_ISTELEPORTING, NUM_DEATHTYPES};
use crate::ids::{EnemyId, PlatformId, TileId, TriggerId};
use crate::path::{get_position_for_path_node, scale_path_speed};
use crate::platform::PLATFORM_NEW_CARRYINGFROG;
use crate::specfx::FXTYPE_FLASH;
use crate::Game;

/// `SCRIPT_FEV_VERSION`.
pub const SCRIPT_FEV_VERSION: u8 = 3;

// `SetEnemy`/`SetPlatform` values (`script.c:73`).
pub const FS_SET_MOVE: i32 = 1;
pub const FS_SET_STOP: i32 = 2;
pub const FS_SET_TOGGLEMOVE: i32 = 3;
pub const FS_SET_INVIS: i32 = 4;
pub const FS_SET_VIS: i32 = 5;
pub const FS_SET_TOGGLEVIS: i32 = 6;
pub const FS_SET_DRAW: i32 = 7;
pub const FS_SET_NODRAW: i32 = 8;
pub const FS_SET_TOGGLEDRAW: i32 = 9;

pub const SCRIPT_MAX_FLAGS: i32 = 64;
pub const SCRIPT_FLAGS_BYTES: usize = (SCRIPT_MAX_FLAGS / 8) as usize;
pub const SCRIPT_MAX_COUNTERS: i32 = 16;

/// `STR_TRAINING1` (`lang.h`): first training hint string.
pub const STR_TRAINING1: i32 = 0;

/// Globals of `script.c` (+ `showEndLevelScreen`, defined in `game.c:88`).
#[derive(Clone, Debug)]
pub struct ScriptState {
    /// `scriptFlags[8]`. `InitScripting` (the only reset) is never called, so the flags
    /// persist across levels.
    pub script_flags: [u8; SCRIPT_FLAGS_BYTES],
    /// `scriptCounters[16]`.
    pub script_counters: [i16; SCRIPT_MAX_COUNTERS as usize],
    /// `lineNumber` (set by `C_DEBUG` markers).
    pub line_number: i32,
    /// `volTest` (`ChangeVolume`).
    pub vol_test: Fixed,
    /// `scriptBuffer` (None = NULL).
    pub script_buffer: Option<Arc<Script>>,
    /// `showEndLevelScreen` (`game.c:88`, initially 1).
    pub show_end_level_screen: i16,
}

impl Default for ScriptState {
    fn default() -> Self {
        ScriptState {
            script_flags: [0; SCRIPT_FLAGS_BYTES],
            script_counters: [0; SCRIPT_MAX_COUNTERS as usize],
            line_number: 0,
            vol_test: 2048,
            script_buffer: None,
            show_end_level_screen: 1,
        }
    }
}

/// Resolves the block at `path` (command indices of nested `EV_ON`/`EV_IF`).
pub fn block_at<'a>(mut block: &'a [Command], path: &[u32]) -> Option<&'a [Command]> {
    for &i in path {
        block = match block.get(i as usize)? {
            Command::On { block, .. } | Command::If { block, .. } => block,
            _ => return None,
        };
    }
    Some(block)
}

/// `PlaySound`/`StopSound` name handling: cut at the first `.`, `strlwr` (PC).
fn sample_name(name: &str) -> String {
    name.split('.').next().unwrap_or("").to_ascii_lowercase()
}

impl Game {
    /// Port of `PrintScriptDebugMessage` (Common/script.c:153).
    pub fn print_script_debug_message(&mut self, s: &str) {
        let text = if self.script.line_number != 0 {
            format!("[Interpreter Debug] {s} (line {})", self.script.line_number)
        } else {
            format!("[Interpreter Debug] {s}")
        };
        self.effects.push(Effect::Log { text });
    }

    /// Port of `GetTileFromNumber` (Common/script.c:182) (`&firstTile[number]`). The
    /// original only warns about an invalid number and returns a wild pointer; the port
    /// returns None for numbers outside the tile array.
    pub fn get_tile_from_number(&mut self, number: i32) -> Option<TileId> {
        if number > self.map.tiles.len() as i32 {
            self.print_script_debug_message(&format!("Invalid tile number: {number}"));
        }
        (number >= 0 && (number as usize) < self.map.tiles.len()).then_some(TileId(number as u32))
    }

    /// Port of `ChangeVolume` (Common/script.c:139).
    pub fn change_volume(&mut self, _a: i32, delta: Fixed) -> Fixed {
        self.script.vol_test = self.script.vol_test.wrapping_add(f2_math::fmul(delta, self.game_speed));
        self.script.vol_test
    }

    /// `worldTile` of node 0 of an enemy's path (`e->path->nodes->worldTile`).
    fn enemy_node0_tile(&self, e: EnemyId) -> Option<TileId> {
        let path = self.enemies.enemy_list.array[e.idx()].path;
        self.path.nodes.get(path.node_index(0)).and_then(|n| n.world_tile)
    }

    /// Port of `EnumPlaceholderTiles` (Common/script.c:211). The original returns an
    /// uninitialised count (unused by its caller).
    pub fn enum_placeholder_tiles(&mut self, id: i32, state: u32) {
        let mut cur = self.enemies.enemy_list.head.first;
        while let Some(e) = cur {
            let n = &self.enemies.enemy_list.array[e.idx()];
            cur = n.links.next;
            if id == 0 || n.uid == id {
                let path = n.path;
                for k in 0..path.num_nodes.max(0) as i32 {
                    // SetTileState (always returns 1)
                    if let Some(t) = self.path.node(&path, k).world_tile {
                        self.map.tile_mut(t).state = state;
                    }
                }
            }
        }
    }

    /// Port of `FrogOnPath` (Common/script.c:231).
    pub fn frog_on_path(&self, pl: i32, nme: EnemyId) -> bool {
        let p = pl as usize;
        // if true, we're either going to land on the tile shortly, or we're
        // double-jumping over it - skip the test
        if self.frogger.player[p].is_super_hopping != 0 {
            return false;
        }
        let path = self.enemies.enemy_list.array[nme.idx()].path;
        let ct = self.frogmove.curr_tile[p];
        self.path.nodes(&path).iter().any(|n| n.world_tile == ct)
    }

    /// Port of `InterpretEvent` (Common/script.c:262).
    pub fn interpret_event(&mut self, b: &ScriptBlock) {
        if !self.interpret(&b.script, &b.path) {
            self.effects.push(Effect::Log { text: "Error running script within trigger!".into() });
        }
    }

    /// Port of `OnFlagSet` (Common/script.c:288).
    pub fn on_flag_set(&self, f: i32) -> bool {
        self.script.script_flags.get((f >> 3) as usize).is_some_and(|b| b & (1 << (f & 7)) != 0)
    }

    /// Port of `OnCounterEquals` (Common/script.c:294).
    pub fn on_counter_equals(&self, c: i32, v: i32) -> bool {
        self.script.script_counters.get(c as usize).is_some_and(|&x| x as i32 == v)
    }

    /// Port of `SetEnemy` (Common/script.c:305).
    pub fn script_set_enemy(&mut self, nme: EnemyId, v: i32) -> i32 {
        let e = &self.enemies.enemy_list.array[nme.idx()];
        let (is_waiting, active, act) = (e.is_waiting, e.active, e.nme_actor);
        match v {
            FS_SET_MOVE => self.set_enemy_moving(nme, true),
            FS_SET_STOP => self.set_enemy_moving(nme, false),
            FS_SET_TOGGLEMOVE => self.set_enemy_moving(nme, is_waiting == 0),
            FS_SET_INVIS => self.set_enemy_visible(nme, false),
            FS_SET_VIS => self.set_enemy_visible(nme, true),
            FS_SET_TOGGLEVIS => self.set_enemy_visible(nme, active == 0),
            FS_SET_DRAW | FS_SET_NODRAW | FS_SET_TOGGLEDRAW => {
                if let Some(a) = act {
                    let d = &mut self.actor2.actors[a].draw;
                    *d = match v {
                        FS_SET_DRAW => 1,
                        FS_SET_NODRAW => 0,
                        _ => (*d == 0) as _,
                    };
                }
            }
            _ => {}
        }
        1
    }

    /// Port of `SetPlatform` (Common/script.c:354).
    pub fn script_set_platform(&mut self, plt: PlatformId, v: i32) -> i32 {
        let p = &self.platform.platform_list.array[plt.idx()];
        let (is_waiting, active, act) = (p.is_waiting, p.active, p.plt_actor);
        match v {
            FS_SET_MOVE => self.set_platform_moving(plt, true),
            FS_SET_STOP => self.set_platform_moving(plt, false),
            FS_SET_TOGGLEMOVE => self.set_platform_moving(plt, is_waiting == -1),
            FS_SET_INVIS => self.set_platform_visible(plt, false),
            FS_SET_VIS => self.set_platform_visible(plt, true),
            FS_SET_TOGGLEVIS => self.set_platform_visible(plt, active == 0),
            FS_SET_DRAW => {
                if let Some(a) = act {
                    self.actor2.actors[a].draw = 1;
                }
                self.move_platform_to_start_of_list(plt);
            }
            FS_SET_NODRAW => {
                if let Some(a) = act {
                    self.actor2.actors[a].draw = 0;
                }
                self.move_platform_to_end_of_list(plt);
            }
            FS_SET_TOGGLEDRAW => {
                if let Some(a) = act {
                    let d = &mut self.actor2.actors[a].draw;
                    *d = (*d == 0) as _;
                }
            }
            _ => {}
        }
        1
    }

    /// Port of `PathEffect` (Common/script.c:403).
    fn path_effect(&mut self, nme: EnemyId, p: &EffectParams, clip: bool) -> i32 {
        let path = self.enemies.enemy_list.array[nme.idx()].path;
        for k in 0..path.num_nodes.max(0) as i32 {
            let node = self.path.node(&path, k).clone();
            let Some(t) = node.world_tile else { continue };
            let v = get_position_for_path_node(&self.map, &node);
            let n = self.map.tile(t).normal;
            let up = FVector { vx: n.vx as i32, vy: n.vy as i32, vz: n.vz as i32 };
            let ty = p.kind as i16;
            let fx = if clip {
                self.create_special_effect(ty, &v, &up, p.size, p.speed, 0, p.lifetime)
            } else {
                self.create_special_effect_direct(ty, &v, &up, p.size, p.speed, 0, p.lifetime)
            };
            if fx.is_some() {
                // TODO(port specfx): fx->gravity = p->accn.
            }
        }
        1
    }

    /// Port of `EnemyEffect` (Common/script.c:431).
    fn enemy_effect(&mut self, nme: EnemyId, p: &EffectParams) -> i32 {
        let e = &self.enemies.enemy_list.array[nme.idx()];
        let (Some(t), Some(a)) = (e.in_tile, e.nme_actor) else {
            self.effects.push(Effect::Log { text: "EnemyEffect: enemy without tile/actor".into() });
            return 1;
        };
        let n = self.map.tile(t).normal;
        let up = FVector { vx: n.vx as i32, vy: n.vy as i32, vz: n.vz as i32 };
        let pos = self.actor2.actors[a].actor.position;
        // params.clip is always 1 for EV_SFX_ENEMY
        let fx = self.create_special_effect(p.kind as i16, &pos, &up, p.size, p.speed, 0, p.lifetime);
        if fx.is_some() {
            // TODO(port specfx): fx->gravity = p->accn.
        }
        1
    }

    /// Port of `SetEnemyFlag` / `ResetEnemyFlag` (Common/script.c:455/464): change the
    /// flags, then run the enemy's update functions once.
    pub fn script_change_enemy_flag(&mut self, e: EnemyId, flag: u32, set: bool) -> i32 {
        let n = &mut self.enemies.enemy_list.array[e.idx()];
        if set {
            n.flags |= flag;
        } else {
            n.flags &= !flag;
        }
        for k in 0..crate::enemies::ENEMY_UPDATEFUNCS {
            let Some(u) = self.enemies.enemy_list.array[e.idx()].update[k] else { break };
            self.call_enemy_update(u, e);
        }
        1
    }

    /// Port of `SetPlatformFlag` / `ResetPlatformFlag` (Common/script.c:473/474).
    pub fn script_change_platform_flag(&mut self, p: PlatformId, flag: u32, set: bool) -> i32 {
        let n = &mut self.platform.platform_list.array[p.idx()];
        if set {
            n.flags |= flag;
        } else {
            n.flags &= !flag;
        }
        // p->Update(p): a NULL Update would crash the original.
        self.call_platform_update(p);
        1
    }

    /// Port of `CollapsePlatform` (Common/script.c:476).
    pub fn collapse_platform(&mut self, p: PlatformId, time: i32) -> i32 {
        let r = self.rng.random((time / 4) as u32);
        self.platform.platform_list.array[p.idx()].countdown = time.wrapping_add(r);
        1
    }

    /// Port of `LoadTrigger` (Common/script.c:502). Returns None (`NULL`) when an
    /// enemy is missing; operands already made stay allocated (leaked) like the C.
    pub fn load_trigger(&mut self, t: &FevTrigger) -> Option<TriggerId> {
        let func = match *t {
            FevTrigger::EnemyOnTile { enemy, tile } => {
                let nme = self.get_enemy_from_uid(enemy as i32)?;
                let tile = self.get_tile_from_number(tile as i32).map_or(i32::MIN, |t| t.0 as i32);
                TriggerFunc::EnemyOnTile { nme, tile }
            }
            FevTrigger::FrogOnTile { frog, tile } => {
                let tl = self.get_tile_from_number(tile as i32).map_or(i32::MIN, |t| t.0 as i32);
                TriggerFunc::FrogOnTile { frog: frog as i32, tile: tl }
            }
            FevTrigger::FrogOnPlatform { frog, platform } => TriggerFunc::FrogOnPlatform { pl: frog as i32, id: platform as i32 },
            FevTrigger::EnemyAtFlag { enemy, flag } => TriggerFunc::EnemyAtFlag { id: enemy as i32, flag: flag as i32 },
            FevTrigger::PlatAtFlag { platform, flag } => TriggerFunc::PlatformAtFlag { id: platform as i32, flag: flag as i32 },
            FevTrigger::Or(ref a, ref b) | FevTrigger::And(ref a, ref b) => {
                let a = self.load_trigger(a)?;
                let b = self.load_trigger(b)?;
                let f = if matches!(t, FevTrigger::Or(..)) { TriggerFunc::Or(a, b) } else { TriggerFunc::And(a, b) };
                let tr = self.make_trigger(f);
                self.event.trigger_list.get_mut(tr).flags |= TRIGGER_LOGIC | TRIGGER_LOGIC2;
                return Some(tr);
            }
            FevTrigger::Not(ref a) => {
                let a = self.load_trigger(a)?;
                let tr = self.make_trigger(TriggerFunc::Not(a));
                self.event.trigger_list.get_mut(tr).flags |= TRIGGER_LOGIC;
                return Some(tr);
            }
            FevTrigger::Wait { time } => {
                let frames = (time.wrapping_mul(60) >> 12) as u32;
                TriggerFunc::OnTimeout { time: self.act_frame_count.wrapping_add(frames), interval: frames }
            }
            FevTrigger::RandomWait { start, end } => {
                let start = (start.wrapping_mul(60) >> 12) as u32;
                let end = (end.wrapping_mul(60) >> 12) as u32;
                let r = self.rng.random(end.wrapping_sub(start)) as u32;
                TriggerFunc::OnRandomTimeout { time: self.act_frame_count.wrapping_add(start).wrapping_add(r), start, end }
            }
            FevTrigger::FlagSet { flag } => TriggerFunc::OnFlagSet(flag as i32),
            FevTrigger::CounterEquals { counter, value } => TriggerFunc::OnCounterEquals { counter: counter as i32, value: value as i32 },
            FevTrigger::FrogIsDead { frog } => TriggerFunc::FrogIsDead(frog as i32),
            FevTrigger::LevelIsOpen { world, level } => TriggerFunc::LevelIsOpen { world: world as i32, level: level as i32 },
            FevTrigger::OnPath { frog, enemy } => {
                let nme = self.get_enemy_from_uid(enemy as i32)?;
                TriggerFunc::FrogOnPath { pl: frog as i32, nme }
            }
        };
        Some(self.make_trigger(func))
    }

    /// Port of `ExecuteCommand` (Common/script.c:640) for command `index` of the block
    /// at `path`. Returns false (`0`) on failure.
    pub fn execute_command(&mut self, script: &Arc<Script>, path: &[u32], index: usize, cmd: &Command) -> bool {
        match cmd {
            Command::Debug { text } => self.print_script_debug_message(text),

            Command::SetEnemy { id, value } => {
                let v = *value as i32;
                if self.enum_enemies(*id as i32, |g, e, v| g.script_set_enemy(e, v), v) == 0 {
                    return false;
                }
            }
            Command::SetPlatform { id, value } => {
                let v = *value as i32;
                if self.enum_platforms(*id as i32, |g, p, v| g.script_set_platform(p, v), v) == 0 {
                    return false;
                }
            }
            Command::SetEnemyFlag { id, flags } | Command::ResetEnemyFlag { id, flags } => {
                let set = matches!(cmd, Command::SetEnemyFlag { .. });
                if self.enum_enemies(*id as i32, |g, e, f| g.script_change_enemy_flag(e, f as u32, set), *flags as i32) == 0 {
                    return false;
                }
            }
            Command::SetPlatFlag { id, flags } | Command::ResetPlatFlag { id, flags } => {
                let set = matches!(cmd, Command::SetPlatFlag { .. });
                if self.enum_platforms(*id as i32, |g, p, f| g.script_change_platform_flag(p, f as u32, set), *flags as i32) == 0 {
                    return false;
                }
            }
            Command::AnimateActor { id, anim, flags, speed } => {
                // RUNANIMINFO { anim, loop = flags & 1, queue = flags & 2, speed }
                let (anim, lp, queue, speed) = (*anim as i32, flags & 1 != 0, flags & 2 != 0, *speed);
                self.enum_enemies(
                    *id as i32,
                    |g, e, _| {
                        if let Some(a) = g.enemies.enemy_list.array[e.idx()].nme_actor {
                            g.actor_animate(a, anim, lp, queue, speed, false);
                        }
                        1
                    },
                    0,
                );
                self.enum_platforms(
                    *id as i32,
                    |g, p, _| {
                        if let Some(a) = g.platform.platform_list.array[p.idx()].plt_actor {
                            g.actor_animate(a, anim, lp, queue, speed, false);
                        }
                        1
                    },
                    0,
                );
            }

            Command::On { trigger, flags, .. } => {
                let Some(t) = self.load_trigger(trigger) else { return false };
                let mut sub: Vec<u32> = path.to_vec();
                sub.push(index as u32);
                let block = ScriptBlock { script: script.clone(), path: Arc::from(sub) };
                self.attach_event(t, *flags as u16, 0, EventFunc::InterpretEvent(block));
            }

            Command::If { trigger, .. } => {
                let Some(t) = self.load_trigger(trigger) else { return false };
                if self.call_trigger(t) {
                    let mut sub: Vec<u32> = path.to_vec();
                    sub.push(index as u32);
                    // Interpret's result is ignored here.
                    self.interpret(script, &sub);
                }
                self.deallocate_trigger(Some(t));
            }

            Command::MoveEnemy { id, node } => {
                self.enum_enemies(*id as i32, |g, e, n| g.move_enemy_to_node(e, n), *node as i32);
            }
            Command::MovePlat { id, node } => {
                self.enum_platforms(*id as i32, |g, p, n| g.move_platform_to_node(p, n), *node as i32);
            }

            Command::ChangeVolume { n, delta, platform } => {
                let amount = self.change_volume(*n as i32, *delta);
                let Some(plt) = self.get_platform_from_uid(*platform as i32) else { return false };
                let path = self.platform.platform_list.array[plt.idx()].path;
                if path.num_nodes != 2 {
                    return false;
                }
                let mut u = get_position_for_path_node(&self.map, self.path.node(&path, 1));
                let mut v = get_position_for_path_node(&self.map, self.path.node(&path, 0));
                scale_vector_s(&mut u, amount >> 12);
                scale_vector_s(&mut v, (4096 - amount) >> 12);
                if let Some(a) = self.platform.platform_list.array[plt.idx()].plt_actor {
                    add_vector_sss(&mut self.actor2.actors[a].actor.position, &u, &v);
                }
                self.platform.platform_list.array[plt.idx()].is_waiting = -1;
            }

            Command::ChangeLevel { world, level } => {
                self.frogger.player[0].world_num = *world;
                self.frogger.player[0].level_num = *level;
                self.game_state.mode = LEVELCOMPLETE_MODE;
                self.game_state.multi = SINGLEPLAYER;
                self.game.mode_timer.init(1);
                self.script.show_end_level_screen = 0;
            }

            Command::Fog { r, g, b, min, max } => {
                let fog = &mut self.frogger.fog;
                fog.r = *r;
                fog.g = *g;
                fog.b = *b;
                fog.min = (*min as i32).wrapping_mul(100) as i16;
                fog.max = (*max as i32).wrapping_mul(100) as i16;
            }
            Command::NoFog => {
                let (w, l) = (self.frogger.player[0].world_num as usize, self.frogger.player[0].level_num as usize);
                self.frogger.fog.mode = 0;
                let far = self.layout.world_visual_data.get(w).and_then(|wv| wv.level_visual_data.get(l)).map_or(0, |lv| lv.far_clip);
                self.frogger.fog.max = far as i16;
            }

            Command::CameoModeOn => self.frogmove.cameo_mode = true,
            Command::CameoModeOff => {
                for pl in 0..4 {
                    self.frogger.player[pl].can_jump = 1;
                }
                self.frogmove.cameo_mode = false;
            }

            Command::Hop { frog, tile } => {
                let pl = *frog as i32;
                match self.get_tile_from_number(*tile as i32) {
                    Some(t) => self.hop_frog_to_tile(t, pl),
                    None => self.effects.push(Effect::Log { text: format!("EV_HOP: invalid tile {tile}") }),
                }
            }

            Command::SetFlag { flag } | Command::ResetFlag { flag } => {
                let f = *flag as usize;
                // BUG(original): flags >= 64 write past scriptFlags[8]; ignored here.
                if let Some(b) = self.script.script_flags.get_mut(f >> 3) {
                    if matches!(cmd, Command::SetFlag { .. }) {
                        *b |= 1 << (f & 7);
                    } else {
                        *b &= !(1 << (f & 7));
                    }
                } else {
                    self.effects.push(Effect::Log { text: format!("script flag {f} out of range") });
                }
            }

            Command::SfxEnemy(p) => {
                self.enum_enemies(p.id as i32, |g, e, _| g.enemy_effect(e, p), 0);
            }
            Command::SfxPlaceholder(p) => {
                self.enum_enemies(p.id as i32, |g, e, _| g.path_effect(e, p, true), 0);
            }
            Command::SfxPlaceholderNoClip(p) => {
                self.enum_enemies(p.id as i32, |g, e, _| g.path_effect(e, p, false), 0);
            }

            Command::ScaleEnemySpeed { id, speed } => {
                let Some(nme) = self.get_enemy_from_uid(*id as i32) else { return false };
                let path = self.enemies.enemy_list.array[nme.idx()].path;
                scale_path_speed(self.path.nodes_mut(&path), *speed);
            }
            Command::ScalePlatSpeed { id, speed } => {
                let Some(plat) = self.get_platform_from_uid(*id as i32) else { return false };
                let path = self.platform.platform_list.array[plat.idx()].path;
                scale_path_speed(self.path.nodes_mut(&path), *speed);
            }

            Command::IncCounter { counter } | Command::DecCounter { counter } | Command::SetCounter { counter, .. } => {
                let c = *counter as i32;
                // SCRIPT_VALIDATE_COUNTER
                if !(0..=SCRIPT_MAX_COUNTERS).contains(&c) {
                    self.print_script_debug_message("Invalid counter number");
                    return false;
                }
                // BUG(original): the check lets counter 16 through, writing past the array.
                let Some(x) = self.script.script_counters.get_mut(c as usize) else {
                    self.effects.push(Effect::Log { text: "scriptCounters[16] written (out of bounds)".into() });
                    return true;
                };
                match cmd {
                    Command::IncCounter { .. } => *x = x.wrapping_add(1),
                    Command::DecCounter { .. } => *x = x.wrapping_sub(1),
                    Command::SetCounter { value, .. } => *x = *value,
                    _ => unreachable!(),
                }
            }

            Command::CollapsePlat { id, time } => {
                let time = time.wrapping_mul(60);
                self.enum_platforms(*id as i32, |g, p, t| g.collapse_platform(p, t), time);
            }

            Command::ChangeFrog { frog, model } => {
                let pl = *frog as usize;
                if model.len() > 19 {
                    self.print_script_debug_message("Model names must be < 20 chars");
                    return false;
                }
                if let Some(f) = self.frogger.frog[pl] {
                    self.change_model(f, model);
                    self.actor_animate(f, FROG_ANIM_BREATHE, true, false, FROG_BREATHE_SPEED, false);
                }
            }

            Command::ShakeCamera { shakiness, falloff } => {
                self.cam.cam_shakiness = shakiness.wrapping_mul(10);
                self.cam.cam_shake_falloff = falloff.wrapping_mul(10);
            }

            Command::SetTileP { id, state } => self.enum_placeholder_tiles(*id as i32, *state as u32),

            Command::SetStartTileP { id } => {
                let Some(e) = self.get_enemy_from_uid(*id as i32) else { return false };
                self.map.g_t_start[0] = self.enemy_node0_tile(e);
                self.cam.start_cam_facing = self.cam.cam_facing[0] as i16;
                self.cam.start_frog_facing = self.frogmove.frog_facing[0] as i16;
            }

            Command::TeleportP { frog, id, time } => {
                let f_num = *frog as usize;
                let tile = self.get_enemy_from_uid(*id as i32).and_then(|e| self.enemy_node0_tile(e));
                let time = ((time.wrapping_mul(60) >> 12) as u32).wrapping_add(self.act_frame_count);

                if self.frogger.player[f_num].frog_state & FROGSTATUS_ISDEAD != 0 {
                    return true;
                }
                let Some(tile) = tile else {
                    // The original dereferences a NULL enemy / tile here.
                    self.effects.push(Effect::Log { text: format!("EV_TELEPORT_P: no placeholder {id}") });
                    return true;
                };

                let pp = &mut self.frogger.player[f_num];
                pp.frog_state &= !FROGSTATUS_ALLHOPFLAGS;
                pp.frog_state |= FROGSTATUS_ISTELEPORTING; // clear ALL other flags
                pp.can_jump = 0;
                self.frog_leave_platform(f_num as i32); // bah

                // PC: dissolveTex override texture (renderer state) dropped.

                if let (Some(ct), Some(f0)) = (self.frogmove.curr_tile[0], self.frogger.frog[0]) {
                    let n = self.map.tile(ct).normal;
                    let up = FVector { vx: n.vx as i32, vy: n.vy as i32, vz: n.vz as i32 };
                    let pos = self.actor2.actors[f0].actor.position;
                    self.create_special_effect(FXTYPE_FLASH, &pos, &up, 0, 12384, 512, 2048);
                }
                self.play_sample(f2_formats::crc::crc("teleport"), None, 0, crate::anim::SAMPLE_VOLUME, -1);

                let t = self.make_trigger(TriggerFunc::OnTimeout { time, interval: 0 });
                self.attach_event(t, TRIGGER_ONCE, 0, EventFunc::TeleportFrog { frog: f_num as i32, tile });
            }

            Command::SpringP { frog, id, height, time } => {
                let tile = self.get_enemy_from_uid(*id as i32).and_then(|e| self.enemy_node0_tile(e));
                match tile {
                    Some(t) => self.spring_frog_to_tile(t, height.wrapping_mul(10), *time, *frog as i32),
                    None => self.effects.push(Effect::Log { text: format!("EV_SPRING_P: no placeholder {id}") }),
                }
            }

            Command::PlaySound { name, uid } => {
                if name.len() > 31 {
                    self.effects.push(Effect::Log { text: "Sample name too long! Max 32 characters".into() });
                    return false;
                }
                let sample = f2_formats::crc::crc(&sample_name(name));
                // FindSample failing breaks silently; the audio layer resolves the uid.
                let uid = *uid as i32;
                if uid == 0 {
                    self.play_sample(sample, None, 0, crate::anim::SAMPLE_VOLUME, -1);
                } else if let Some(nme) = self.get_enemy_from_uid(uid) {
                    let pos = match self.enemies.enemy_list.array[nme.idx()].nme_actor {
                        Some(a) => Some(self.actor2.actors[a].actor.position),
                        None => self.enemy_node0_tile(nme).map(|t| self.map.tile(t).centre),
                    };
                    self.play_sample(sample, pos, 0, crate::anim::SAMPLE_VOLUME, -1);
                } else if let Some(plat) = self.get_platform_from_uid(uid) {
                    let p = &self.platform.platform_list.array[plat.idx()];
                    let pos = match p.plt_actor {
                        Some(a) => Some(self.actor2.actors[a].actor.position),
                        None => {
                            let path = p.path;
                            self.path.nodes.get(path.node_index(0)).and_then(|n| n.world_tile).map(|t| self.map.tile(t).centre)
                        }
                    };
                    self.play_sample(sample, pos, 0, crate::anim::SAMPLE_VOLUME, -1);
                } else {
                    self.print_script_debug_message("PlaySound(): Couldn't find actor");
                }
            }

            Command::AnimateFrog { frog, anim, flags, speed } => {
                if let Some(f) = self.frogger.frog[*frog as usize] {
                    self.actor_animate(f, *anim as i32, flags & 1 != 0, flags & 2 != 0, *speed, false);
                }
            }

            Command::KillFrog { frog, death } => {
                let pl = *frog as usize;
                let num = *death as i32;
                if let Some(cp) = self.platform.curr_platform[pl] {
                    let plat = &mut self.platform.platform_list.array[cp.idx()];
                    plat.flags &= !PLATFORM_NEW_CARRYINGFROG;
                    plat.carrying = None;
                    self.platform.curr_platform[pl] = None;
                }
                self.frogger.player[pl].frog_state |= FROGSTATUS_ISDEAD;
                if (num as usize) < NUM_DEATHTYPES {
                    self.call_death_anim(num, pl as i32);
                } else {
                    self.call_death_anim(DEATHBY_NORMAL, pl as i32);
                }
            }

            Command::StopSound { name } => {
                if name.len() > 31 {
                    self.effects.push(Effect::Log { text: "Sample name too long! Max 32 characters".into() });
                    return false;
                }
                let uid = f2_formats::crc::crc(&sample_name(name));
                self.effects.push(Effect::StopSample { uid });
            }

            Command::EndLevel => {
                self.game_state.mode = LEVELCOMPLETE_MODE;
                self.game_state.multi = SINGLEPLAYER;
                self.game.mode_timer.init(8);
                // TODO(port): SetTimeForLevel (story/records wave).
                // PrepareSong(AUDIOTRK_LEVELCOMPLETE, NO)
                self.effects.push(Effect::Log { text: "TODO(port) PrepareSong(AUDIOTRK_LEVELCOMPLETE)".into() });
                // TODO(port): StartLevelComplete (level-complete wave).
            }

            Command::ResetScript => {
                self.kill_all_triggers();
                match self.script.script_buffer.clone() {
                    Some(root) => {
                        self.interpret(&root, &[]);
                    }
                    None => self.effects.push(Effect::Log { text: "EV_RESETSCRIPT without scriptBuffer".into() }),
                }
                self.print_script_debug_message("Reset script");
                return false; // don't try to run any more commands!
            }

            Command::PokeFrogs => {
                for pl in 0..self.frogger.num_frogs.max(0) {
                    let ct = self.frogmove.curr_tile[pl as usize];
                    self.check_tile_state(ct, pl);
                }
            }

            Command::Hint { string } => {
                // TODO(port): TrainingHint(GAMESTRING(str+STR_TRAINING1)) (training wave).
                self.effects.push(Effect::Log { text: format!("TODO(port) TrainingHint(string {})", *string as i32 + STR_TRAINING1) });
            }

            Command::LineNumber { line } => self.script.line_number = *line as i32,

            Command::HurtFrog => {
                // EV_HURTFROG has no case in ExecuteCommand.
                self.print_script_debug_message("Unrecognised command");
                return false;
            }
        }
        true
    }

    /// Port of `Interpret` (Common/script.c:1300): runs the block at `path` of `script`.
    pub fn interpret(&mut self, script: &Arc<Script>, path: &[u32]) -> bool {
        let Some(block) = block_at(&script.commands, path) else {
            self.effects.push(Effect::Log { text: format!("Interpret: bad block path {path:?}") });
            return false;
        };
        for (i, cmd) in block.iter().enumerate() {
            if !self.execute_command(script, path, i, cmd) {
                if self.script.line_number != 0 {
                    self.effects.push(Effect::Log { text: format!("Possibly an error on line {}?", self.script.line_number) });
                }
                return false;
            }
        }
        true
    }

    /// Port of `InitLevelScript` (Common/script.c:1240) on a decoded script (the
    /// version check happened when decoding). Returns false on failure, after which
    /// the script is freed and all triggers killed.
    pub fn init_level_script(&mut self, script: Script) -> bool {
        self.kill_all_triggers();
        self.free_level_script();

        let root = Arc::new(script);
        self.script.script_buffer = Some(root.clone());

        if !self.interpret(&root, &[]) {
            self.script.script_buffer = None;
            self.kill_all_triggers();
            self.effects.push(Effect::Log { text: "InitLevelScript() failed".into() });
            return false;
        }
        true
    }

    /// Port of `FreeLevelScript` (Common/script.c:1279).
    pub fn free_level_script(&mut self) -> bool {
        self.script.script_buffer = None;
        true
    }

    /// The fog the renderer applies this frame (`fog`, changed by `EV_FOG`/`EV_NO_FOG`)
    /// and its far clip plane, `farClip = (float)fog.max*0.1` (PC/drawloop.cpp:198).
    pub fn render_fog(&self) -> (crate::frogger::Fog, f32) {
        let fog = self.frogger.fog;
        (fog, fog.max as f32 * 0.1)
    }

    /// Port of `InitScripting` (Common/script.c:1420) (never called by the game).
    pub fn init_scripting(&mut self) -> bool {
        self.script.script_flags = [0; SCRIPT_FLAGS_BYTES];
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{TRIGGER_ALWAYS, TRIGGER_FREE};
    use f2_formats::fev::Command as C;
    use f2_formats::fev::Trigger as T;

    fn game() -> Game {
        let mut g = Game::new();
        g.init_trigger_list();
        g
    }

    #[test]
    fn top_level_runs_in_order_and_on_attaches_blocks() {
        let mut g = game();
        let s = Script {
            commands: vec![
                C::SetCounter { counter: 0, value: 5 },
                C::On { trigger: T::FlagSet { flag: 1 }, flags: TRIGGER_ONCE as u8, block: vec![C::IncCounter { counter: 0 }] },
                C::On { trigger: T::CounterEquals { counter: 0, value: 6 }, flags: 0, block: vec![C::SetFlag { flag: 2 }, C::DecCounter { counter: 0 }] },
                C::SetFlag { flag: 1 },
            ],
        };
        assert!(g.init_level_script(s));
        assert_eq!(g.script.script_counters[0], 5);
        assert_eq!(g.event.trigger_list.count, 2);
        // the second EV_ON is first in the list
        g.update_events();
        // second ON (counter==6?) is checked first: false; then first ON fires: counter=6.
        assert_eq!(g.script.script_counters[0], 6);
        assert_eq!(g.event.trigger_list.count, 1);
        g.update_events();
        assert_eq!(g.script.script_counters[0], 5);
        assert!(g.on_flag_set(2));
    }

    #[test]
    fn if_evaluates_immediately_and_frees() {
        let mut g = game();
        let s = Script {
            commands: vec![
                C::If { trigger: T::FlagSet { flag: 0 }, block: vec![C::IncCounter { counter: 1 }] },
                C::SetFlag { flag: 0 },
                C::If { trigger: T::Not(Box::new(T::FlagSet { flag: 3 })), block: vec![C::IncCounter { counter: 1 }, C::IncCounter { counter: 1 }] },
            ],
        };
        assert!(g.init_level_script(s));
        assert_eq!(g.script.script_counters[1], 2);
        assert_eq!(g.event.trigger_list.count, 0, "EV_IF frees its trigger (and the NOT operand)");
    }

    #[test]
    fn failure_stops_block_and_init_kills_triggers() {
        let mut g = game();
        let s = Script {
            commands: vec![
                C::On { trigger: T::FlagSet { flag: 0 }, flags: 0, block: vec![] },
                C::IncCounter { counter: 2 },
                // no enemies: EnumEnemies returns 0 -> failure
                C::SetEnemy { id: 0, value: 1 },
                C::IncCounter { counter: 2 },
            ],
        };
        assert!(!g.init_level_script(s));
        assert_eq!(g.script.script_counters[2], 1);
        assert_eq!(g.event.trigger_list.count, 0);
        assert!(g.script.script_buffer.is_none());
    }

    #[test]
    fn wait_uses_act_frame_count() {
        let mut g = game();
        g.act_frame_count = 100;
        // 0.5 s = 2048 -> 30 frames
        let s = Script { commands: vec![C::On { trigger: T::Wait { time: 2048 }, flags: 0, block: vec![C::IncCounter { counter: 3 }] }] };
        assert!(g.init_level_script(s));
        let mut fired_at = vec![];
        for _ in 0..70 {
            g.act_frame_count += 1;
            let before = g.script.script_counters[3];
            g.update_events();
            if g.script.script_counters[3] != before {
                fired_at.push(g.act_frame_count);
            }
        }
        assert_eq!(fired_at, vec![130, 160]);
    }

    #[test]
    fn random_wait_draws_random_at_load_and_on_fire() {
        let mut g = game();
        let s = Script { commands: vec![C::On { trigger: T::RandomWait { start: 4096, end: 8192 }, flags: 0, block: vec![] }] };
        let mut rng = g.rng;
        assert!(g.init_level_script(s));
        let r = rng.random(60) as u32;
        assert_eq!(g.rng.rseed, rng.rseed);
        let t = g.event.trigger_list.head.first.unwrap();
        assert_eq!(g.event.trigger_list.get(t).func, TriggerFunc::OnRandomTimeout { time: 60 + r, start: 60, end: 120 });
    }

    #[test]
    fn reset_script_reinterprets_from_start() {
        let mut g = game();
        let s = Script {
            commands: vec![
                C::IncCounter { counter: 4 },
                C::On { trigger: T::CounterEquals { counter: 4, value: 2 }, flags: 0, block: vec![C::SetFlag { flag: 9 }] },
                C::On { trigger: T::FlagSet { flag: 8 }, flags: 0, block: vec![C::ResetFlag { flag: 8 }, C::ResetScript, C::SetFlag { flag: 10 }] },
            ],
        };
        assert!(g.init_level_script(s));
        assert_eq!(g.script.script_counters[4], 1);
        g.script.script_flags[1] |= 1; // flag 8
        g.update_events();
        // ResetScript re-ran the outer block (counter 2) and stopped its own block.
        assert_eq!(g.script.script_counters[4], 2);
        assert!(!g.on_flag_set(10));
        // UpdateEvents returned right after the reset: the new counter==2 trigger did
        // not run this frame...
        assert!(!g.on_flag_set(9));
        assert_eq!(g.event.trigger_list.count, 2);
        // ...but runs on the next.
        g.update_events();
        assert!(g.on_flag_set(9));
    }

    #[test]
    fn nested_on_inside_event() {
        let mut g = game();
        let s = Script {
            commands: vec![C::On {
                trigger: T::FlagSet { flag: 0 },
                flags: TRIGGER_ONCE as u8,
                block: vec![C::On { trigger: T::FlagSet { flag: 0 }, flags: TRIGGER_ALWAYS as u8, block: vec![C::IncCounter { counter: 5 }] }],
            }],
        };
        assert!(g.init_level_script(s));
        g.script.script_flags[0] = 1;
        g.update_events(); // outer fires: attaches inner at the head (not visited this frame)
        assert_eq!(g.script.script_counters[5], 0);
        g.update_events();
        g.update_events();
        assert_eq!(g.script.script_counters[5], 2);
        let ids = g.event.trigger_list.active();
        assert_eq!(ids.len(), 1);
        assert_eq!(g.event.trigger_list.get(ids[0]).flags & TRIGGER_FREE, 0);
    }

    #[test]
    fn fog_commands() {
        let mut g = game();
        g.frogger.player[0].world_num = 0;
        g.frogger.player[0].level_num = 0;
        let s = Script { commands: vec![C::Fog { r: 1, g: 2, b: 3, min: 10, max: 400 }] };
        assert!(g.init_level_script(s));
        assert_eq!((g.frogger.fog.r, g.frogger.fog.g, g.frogger.fog.b, g.frogger.fog.min, g.frogger.fog.max), (1, 2, 3, 1000, (40000i32) as i16));
        g.frogger.fog.mode = 1;
        let s = Script { commands: vec![C::NoFog] };
        assert!(g.init_level_script(s));
        assert_eq!(g.frogger.fog.mode, 0);
        assert_eq!(g.frogger.fog.max as i32, g.layout.world_visual_data[0].level_visual_data[0].far_clip as i16 as i32);
    }
}
