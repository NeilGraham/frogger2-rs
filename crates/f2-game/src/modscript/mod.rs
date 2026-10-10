//! P7b: sandboxed behaviour scripts for mods (`docs/modding.md` §4).
//!
//! Not a port of anything. Mods ship F2S scripts (`scripts/*.f2s`, or `scripts = [...]` in a
//! `level.toml`); the host compiles them into [`ScriptSource`]s and installs them with
//! [`Game::install_mod_scripts`] (`SessionConfig::scripts`). Scripts run *inside* the
//! deterministic tick:
//!
//! * integer-only language ([`lang`]) on a bounded stack VM ([`vm`]): per-tick instruction
//!   budget, stack/depth/string limits; any error disables the script (error log effect +
//!   [`ScriptUnit::disabled`]);
//! * the only randomness is the game's own `Random()`; no clock, no I/O, no floats;
//! * all mutable script state (globals, claims, queued events, budgets) lives in
//!   [`ModScriptState`] inside [`Game`], so snapshots, restore and the netplay checksum cover
//!   it (`netsync.rs` sub-state `modscript`); compiled programs are immutable `Arc`s.
//!
//! With no scripts installed every hook returns immediately: the authentic simulation is
//! unchanged. Hooks are not run during attract demos (`DEMO_MODE`).
//!
//! Call sites in the simulation (all marked `// P7b`):
//! * `load_level` (layout.rs) → [`Game::modscript_level_start`] (`on_level_start`);
//! * `RunGameLoop` (game.rs) → [`Game::modscript_tick`] (queued events, deaths, `on_tick`);
//! * `CheckForFroggerLanding` (frogmove.rs) → [`Game::modscript_frog_landed`];
//! * `PickupCollectable` (collect.rs) → [`Game::modscript_pickup`];
//! * `UpdateEnemies` (enemies.rs) → [`Game::modscript_update_enemy`] for claimed enemies.

pub mod api;
pub mod lang;
pub mod vm;

use std::sync::Arc;

use serde::Serialize;

use crate::define::mode::DEMO_MODE;
use crate::frogmove::FROGSTATUS_ISDEAD;
use crate::ids::{EnemyId, TileId};
use crate::netsync::StateHasher;
use crate::Game;
pub use lang::{compile, CompileError, Hook, Program, HOOKS};
pub use vm::{Value, VmError, STEPS_PER_TICK};

/// A script as the host hands it over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptSource {
    /// Display name, e.g. `hello-mod/scripts/hazard.f2s`.
    pub name: String,
    pub source: String,
    /// `Some((world, level))`: hooks only run on that level (from a `level.toml`).
    pub scope: Option<(u32, u32)>,
}

impl ScriptSource {
    /// Fixed 64-bit hash of name, scope and source (netplay fingerprint).
    pub fn hash(&self) -> u64 {
        let mut h = StateHasher::new();
        h.write_bytes(self.name.as_bytes());
        match self.scope {
            Some((w, l)) => {
                h.write_u64(1);
                h.write_u64(w as u64);
                h.write_u64(l as u64);
            }
            None => h.write_u64(0),
        }
        h.write_bytes(self.source.as_bytes());
        h.finish()
    }
}

/// Hash of a script set (0 for none): what peers must agree on.
pub fn scripts_hash(srcs: &[ScriptSource]) -> u64 {
    if srcs.is_empty() {
        return 0;
    }
    let mut h = StateHasher::new();
    for s in srcs {
        h.write_u64(s.hash());
    }
    h.finish()
}

/// One installed script.
#[derive(Clone, Debug, Serialize)]
pub struct ScriptUnit {
    pub name: String,
    pub hash: u64,
    pub scope: Option<(u32, u32)>,
    /// Compiled code (immutable, shared by snapshots; covered by `hash`).
    #[serde(skip)]
    pub program: Arc<Program>,
    /// Global variables (the script's persistent state).
    pub globals: Vec<Value>,
    /// Why the script stopped running (`None` = running).
    pub disabled: Option<String>,
    /// Budget bookkeeping: `actFrameCount` of the budget in `steps_left`.
    pub budget_frame: u32,
    pub steps_left: u32,
}

/// A hook event queued by the simulation, dispatched by [`Game::modscript_tick`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum ScriptEvent {
    Land { player: u8, tile: i32, state: i32 },
    Death { player: u8, cause: i32 },
    Pickup { player: u8, kind: i32 },
}

/// A text overlay shown by a script.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ScriptText {
    pub unit: u32,
    pub overlay: u32,
    /// `actFrameCount` at which it is removed (when `live`).
    pub expire: u32,
    pub live: bool,
}

/// All mutable script state (part of [`Game`], snapshots and checksum).
#[derive(Clone, Debug, Default, Serialize)]
pub struct ModScriptState {
    pub units: Vec<ScriptUnit>,
    /// Events since the last dispatch.
    pub queue: Vec<ScriptEvent>,
    /// `(enemy index, unit)`: enemies whose update a script took over.
    pub claims: Vec<(u32, u32)>,
    pub texts: Vec<ScriptText>,
    /// Per player: dead at the last dispatch (death edge detection).
    pub prev_dead: [bool; 4],
    /// Hash of the installed script set ([`scripts_hash`]).
    pub set_hash: u64,
}

impl Game {
    /// Compiles and installs mod scripts (replacing any installed ones) and runs their global
    /// initialisers. Returns one message per script that failed to compile (it is skipped) or
    /// whose initialiser failed (it is installed disabled).
    pub fn install_mod_scripts(&mut self, srcs: &[ScriptSource]) -> Vec<String> {
        let mut errors = Vec::new();
        self.modscript = ModScriptState { set_hash: scripts_hash(srcs), ..Default::default() };
        for s in srcs {
            let prog = match compile(&s.source) {
                Ok(p) => Arc::new(p),
                Err(e) => {
                    errors.push(format!("{}: {e}", s.name));
                    continue;
                }
            };
            let n = prog.globals.len();
            self.modscript.units.push(ScriptUnit {
                name: s.name.clone(),
                hash: s.hash(),
                scope: s.scope,
                program: prog,
                globals: vec![Value::Int(0); n],
                disabled: None,
                budget_frame: self.act_frame_count,
                steps_left: STEPS_PER_TICK,
            });
            let u = self.modscript.units.len() - 1;
            if self.modscript_run(u, 0, &[], "global initialiser").is_none() {
                errors.push(format!("{}: {}", s.name, self.modscript.units[u].disabled.clone().unwrap_or_default()));
            }
        }
        errors
    }

    /// Whether any script is installed.
    pub fn modscript_active(&self) -> bool {
        !self.modscript.units.is_empty()
    }

    /// `(script name, reason)` of every disabled script.
    pub fn modscript_errors(&self) -> Vec<(String, String)> {
        self.modscript.units.iter().filter_map(|u| u.disabled.clone().map(|d| (u.name.clone(), d))).collect()
    }

    /// Hash of the installed script set (0 = none); part of the netplay start fingerprint.
    pub fn modscript_hash(&self) -> u64 {
        self.modscript.set_hash
    }

    /// Hooks of unit `u` may run now.
    fn modscript_live(&self, u: usize) -> bool {
        let unit = &self.modscript.units[u];
        unit.disabled.is_none()
            && self.game_state.mode != DEMO_MODE
            && unit.scope.is_none_or(|(w, l)| self.frogger.player[0].world_num as u32 == w && self.frogger.player[0].level_num as u32 == l)
    }

    fn modscript_fail(&mut self, u: usize, why: String) {
        let unit = &mut self.modscript.units[u];
        let text = format!("modscript {}: disabled: {why}", unit.name);
        unit.disabled = Some(why);
        self.modscript.claims.retain(|c| c.1 != u as u32);
        self.effects.push(crate::Effect::Log { text });
    }

    /// Runs function `func` of unit `u`. `None` if it failed (the unit is then disabled).
    fn modscript_run(&mut self, u: usize, func: u16, args: &[Value], what: &str) -> Option<Value> {
        let frame = self.act_frame_count;
        let mut st = std::mem::take(&mut self.modscript);
        let unit = &mut st.units[u];
        if unit.budget_frame != frame {
            unit.budget_frame = frame;
            unit.steps_left = STEPS_PER_TICK;
        }
        let prog = unit.program.clone();
        let mut globals = std::mem::take(&mut unit.globals);
        let mut steps = unit.steps_left;
        let r = {
            let mut host = api::ApiHost { g: self, st: &mut st, unit: u };
            vm::run(&prog, &mut globals, func, args, &mut steps, &mut host)
        };
        st.units[u].globals = globals;
        st.units[u].steps_left = steps;
        // Anything the simulation queued while the state was out (no hooks fire then).
        let tmp = std::mem::replace(&mut self.modscript, st);
        self.modscript.queue.extend(tmp.queue);
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                self.modscript_fail(u, format!("{what}: {e}"));
                None
            }
        }
    }

    /// Calls `hook` on every live unit that defines it.
    fn modscript_hook(&mut self, hook: Hook, args: &[i32]) {
        let args: Vec<Value> = args.iter().map(|&v| Value::Int(v)).collect();
        for u in 0..self.modscript.units.len() {
            if !self.modscript_live(u) {
                continue;
            }
            if let Some(f) = self.modscript.units[u].program.hook(hook) {
                self.modscript_run(u, f, &args, HOOKS[hook as usize].0);
            }
        }
    }

    fn modscript_frog_dead(&self, p: usize) -> bool {
        self.frogger.player[p].frog_state & FROGSTATUS_ISDEAD != 0
    }

    /// P7b hook (end of `load_level`): resets per-level script state and calls
    /// `on_level_start(world, level)`.
    pub fn modscript_level_start(&mut self, world: u32, level: u32) {
        if !self.modscript_active() {
            return;
        }
        let st = &mut self.modscript;
        st.queue.clear();
        st.claims.clear();
        st.texts.clear(); // FreeAllLists dropped the overlays
        for u in &mut st.units {
            u.budget_frame = self.act_frame_count;
            u.steps_left = STEPS_PER_TICK;
        }
        for p in 0..4 {
            self.modscript.prev_dead[p] = self.modscript_frog_dead(p);
        }
        self.modscript_hook(Hook::LevelStart, &[world as i32, level as i32]);
    }

    /// P7b hook (end of `RunGameLoop`): death detection, queued events
    /// (`on_frog_land`, `on_frog_death`, `on_pickup`), `on_tick`, text expiry.
    pub fn modscript_tick(&mut self) {
        if !self.modscript_active() {
            return;
        }
        let n = self.frogger.num_frogs.clamp(0, 4) as usize;
        for p in 0..n {
            let dead = self.modscript_frog_dead(p);
            if dead && !self.modscript.prev_dead[p] {
                let cause = self.frogger.player[p].death_by as i32;
                self.modscript.queue.push(ScriptEvent::Death { player: p as u8, cause });
            }
            self.modscript.prev_dead[p] = dead;
        }
        let q = std::mem::take(&mut self.modscript.queue);
        for ev in q {
            match ev {
                ScriptEvent::Land { player, tile, state } => self.modscript_hook(Hook::FrogLand, &[player as i32, tile, state]),
                ScriptEvent::Death { player, cause } => self.modscript_hook(Hook::FrogDeath, &[player as i32, cause]),
                ScriptEvent::Pickup { player, kind } => self.modscript_hook(Hook::Pickup, &[player as i32, kind]),
            }
        }
        self.modscript_hook(Hook::Tick, &[]);
        let now = self.act_frame_count;
        let mut k = 0;
        while k < self.modscript.texts.len() {
            let t = &self.modscript.texts[k];
            if t.live && now.wrapping_sub(t.expire) as i32 >= 0 {
                let id = crate::ids::TextOverlayId(t.overlay);
                self.modscript.texts.remove(k);
                self.textover.sub_text_overlay(Some(id));
            } else {
                k += 1;
            }
        }
    }

    /// P7b hook (`CheckForFroggerLanding`): queues `on_frog_land(player, tile, state)`.
    pub fn modscript_frog_landed(&mut self, pl: i32, tile: Option<TileId>) {
        if !self.modscript_active() {
            return;
        }
        let (t, s) = tile.and_then(|t| self.map.tiles.get(t.idx()).map(|g| (t.idx() as i32, g.state as i32))).unwrap_or((-1, -1));
        self.modscript.queue.push(ScriptEvent::Land { player: pl as u8, tile: t, state: s });
    }

    /// P7b hook (`PickupCollectable`): queues `on_pickup(player, kind)`.
    pub fn modscript_pickup(&mut self, pl: i32, kind: i32) {
        if !self.modscript_active() {
            return;
        }
        self.modscript.queue.push(ScriptEvent::Pickup { player: pl as u8, kind });
    }

    /// P7b hook (`UpdateEnemies`): `true` if a live script claimed enemy `id` and its
    /// `on_enemy_update(e)` ran instead of the built-in movement.
    pub fn modscript_update_enemy(&mut self, id: EnemyId) -> bool {
        if !self.modscript_active() {
            return false;
        }
        let Some(&(_, u)) = self.modscript.claims.iter().find(|c| c.0 == id.0) else { return false };
        let u = u as usize;
        if !self.modscript_live(u) {
            return false;
        }
        let Some(f) = self.modscript.units[u].program.hook(Hook::EnemyUpdate) else { return false };
        self.modscript_run(u, f, &[Value::Int(id.0 as i32)], "on_enemy_update");
        true
    }
}

#[cfg(test)]
mod tests;
