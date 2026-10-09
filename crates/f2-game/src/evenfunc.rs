//! Port of `Common/evenfunc.c` / `evenfunc.h`: the trigger condition functions and
//! event action functions of the event system, plus the dispatch of
//! [`TriggerFunc`] / [`EventFunc`] (the C called them through function pointers).
//! The script-specific triggers (`ANDtrigger`, `OnFlagSet`, `FrogOnPath`, ...) and
//! `InterpretEvent` are defined in `script.c` and ported in [`crate::script`].

use f2_math::{distance_between_points_sv, scale_vector_ff, FVector};

use crate::actor2::orientate_ss;
use crate::define::mode::LEVELCOMPLETE_MODE;
use crate::define::tilestate;
use crate::effects::Effect;
use crate::event::{EventFunc, IntVar, PathOwner, SfxPoint, SfxSample, TriggerFunc, TRIGGER_FIRED};
use crate::frogger::FROG_BABYFROG;
use crate::frogmove::FROGSTATUS_ISDEAD;
use crate::frogmove::FROGSTATUS_ISTELEPORTING;
use crate::ids::{ActorId, TileId, TriggerId};
use crate::platform::PLATFORM_NEW_CARRYINGFROG;
use crate::Game;

/// `SAMPLE_VOLUME`.
use crate::anim::SAMPLE_VOLUME;

impl Game {
    /// Calls `trigger->func(trigger)`.
    pub fn call_trigger(&mut self, t: TriggerId) -> bool {
        let func = self.event.trigger_list.get(t).func.clone();
        match func {
            TriggerFunc::None => false,
            TriggerFunc::EnemyOnTile { nme, tile } => self.enemy_on_tile(nme, tile),
            TriggerFunc::FrogOnTile { frog, tile } => self.frog_on_tile(frog, tile),
            TriggerFunc::FrogOnPlatform { pl, id } => self.frog_on_platform(pl, id),
            TriggerFunc::ActorWithinRadius { actor, pos, radius } => distance_between_points_sv(&self.actor2.actors[actor].actor.position, &pos) < radius,
            TriggerFunc::OnTimeout { .. } => self.on_timeout(t),
            TriggerFunc::OnRandomTimeout { .. } => self.on_random_timeout(t),
            TriggerFunc::OnTrigger(o) => self.event.trigger_list.get(o).flags & TRIGGER_FIRED != 0,
            TriggerFunc::LogicalAnd(list) => {
                // Port of `LogicalAND` (Common/evenfunc.c:183).
                for o in list {
                    if !self.call_trigger(o) {
                        return false;
                    }
                }
                true
            }
            TriggerFunc::LogicalOr(list) => {
                // Port of `LogicalOR` (Common/evenfunc.c:207).
                for o in list {
                    if self.call_trigger(o) {
                        return true;
                    }
                }
                false
            }
            TriggerFunc::EnemyAtFlag { id, flag } => self.enemy_at_flag(id, flag),
            TriggerFunc::PlatformAtFlag { id, flag } => self.platform_at_flag(id, flag),
            TriggerFunc::PathAtFlag { path, flag } => {
                // Port of `PathAtFlag` (Common/evenfunc.c:258).
                let p = match path {
                    PathOwner::Enemy(e) => self.enemies.enemy_list.array[e.idx()].path,
                    PathOwner::Platform(p) => self.platform.platform_list.array[p.idx()].path,
                };
                p.from_node as i32 == flag
            }
            TriggerFunc::BitCheck { var, mask } => self.int_var(var) & mask != 0,
            TriggerFunc::FrogIsDead(pl) => self.frog_is_dead(pl),
            TriggerFunc::LevelIsOpen { world, level } => self.level_is_open(world, level),
            TriggerFunc::And(a, b) => self.call_trigger(a) && self.call_trigger(b),
            TriggerFunc::Or(a, b) => self.call_trigger(a) || self.call_trigger(b),
            TriggerFunc::Not(a) => !self.call_trigger(a),
            TriggerFunc::Always => true,
            TriggerFunc::OnFlagSet(f) => self.on_flag_set(f),
            TriggerFunc::OnCounterEquals { counter, value } => self.on_counter_equals(counter, value),
            TriggerFunc::FrogOnPath { pl, nme } => self.frog_on_path(pl, nme),
        }
    }

    /// Calls `event->func(event)` (a NULL func does nothing).
    pub fn call_event(&mut self, e: &EventFunc) {
        match e {
            EventFunc::None => {}
            EventFunc::InterpretEvent(b) => self.interpret_event(b),
            // Port of `ChangeActorScale` (Common/evenfunc.c:332): body commented out.
            EventFunc::ChangeActorScale => {}
            // Port of `AssignFloatToFloat` / `AssignIntToInt` (Common/evenfunc.c:350/367).
            EventFunc::AssignFloatToFloat { var, val } | EventFunc::AssignIntToInt { var, val } => self.set_int_var(*var, *val),
            EventFunc::TogglePlatformMove(p) => {
                // Port of `TogglePlatformMove` (Common/evenfunc.c:384).
                let plt = &mut self.platform.platform_list.array[p.idx()];
                plt.is_waiting = if plt.is_waiting != 0 { 0 } else { -1 };
            }
            EventFunc::ToggleEnemyMove(n) => {
                // Port of `ToggleEnemyMove` (Common/evenfunc.c:400).
                let nme = &mut self.enemies.enemy_list.array[n.idx()];
                nme.is_waiting = if nme.is_waiting != 0 { 0 } else { -1 };
            }
            EventFunc::ToggleTileLink(t) => {
                // Port of `ToggleTileLink` (Common/evenfunc.c:416).
                let tile = self.map.tile_mut(*t);
                tile.state = if tile.state == tilestate::BARRED { tilestate::NORMAL } else { tilestate::BARRED };
            }
            EventFunc::PlaySfx { sample, point } => self.play_sfx(*sample, *point),
            EventFunc::EvAnimateActor { actor, anim, flags, speed } => {
                // Port of `EvAnimateActor` (Common/evenfunc.c:446): anim/flags read as short.
                let (anim, flags) = (*anim as i16 as i32, *flags as i16);
                self.actor_animate(*actor, anim, flags & 1 != 0, flags & 2 != 0, *speed, false);
            }
            EventFunc::ChangeLevel { world, level } => self.change_level(*world, *level),
            EventFunc::TeleportFrog { frog, tile } => self.teleport_frog(*frog, *tile),
            EventFunc::DeathAnim { pl, func } => {
                // Port of `DeathAnim` (Common/evenfunc.c:497), PC branch: the death function
                // may ChangeModel the frog; the LOD controller is restored afterwards.
                let a = self.frogger.frog[*pl as usize];
                let o = a.map(|a| self.actor2.actors[a].actor.actual_actor.lod_object_controller);
                self.run_death_anim(*func, *pl);
                if let (Some(a), Some(o)) = (a, o) {
                    self.actor2.actors[a].actor.actual_actor.lod_object_controller = o;
                }
            }
        }
    }

    /// The `int*` behind an [`IntVar`].
    pub fn int_var(&self, v: IntVar) -> i32 {
        match v {
            IntVar::ScriptCounter(n) => self.script.script_counters.get(n as usize).map_or(0, |&c| c as i32),
            IntVar::CamShakiness => self.cam.cam_shakiness,
            IntVar::ScaleV => self.cam.scale_v,
        }
    }

    /// `*var = val`.
    pub fn set_int_var(&mut self, v: IntVar, val: i32) {
        match v {
            IntVar::ScriptCounter(n) => {
                if let Some(c) = self.script.script_counters.get_mut(n as usize) {
                    *c = val as i16;
                }
            }
            IntVar::CamShakiness => self.cam.cam_shakiness = val,
            IntVar::ScaleV => self.cam.scale_v = val,
        }
    }

    /// `&firstTile[n] == tile` for a tile kept as its raw number.
    fn tile_is(t: Option<TileId>, n: i32) -> bool {
        t.is_some_and(|t| t.0 as i64 == n as i64)
    }

    /// Port of `EnemyOnTile` (Common/evenfunc.c:50).
    pub fn enemy_on_tile(&self, nme: crate::ids::EnemyId, tile: i32) -> bool {
        Self::tile_is(self.enemies.enemy_list.array[nme.idx()].in_tile, tile)
    }

    /// Port of `FrogOnTile` (Common/evenfunc.c:70).
    pub fn frog_on_tile(&self, frog: i32, tile: i32) -> bool {
        Self::tile_is(self.frogmove.curr_tile.get(frog as usize).copied().flatten(), tile)
    }

    /// Port of `FrogOnPlatform` (Common/evenfunc.c:90).
    pub fn frog_on_platform(&self, pl: i32, id: i32) -> bool {
        match self.platform.curr_platform.get(pl as usize).copied().flatten() {
            Some(p) => self.platform.platform_list.array[p.idx()].uid == id,
            None => false,
        }
    }

    /// Port of `OnTimeout` (Common/evenfunc.c:131): fires once `actFrameCount` reaches
    /// `data[0]`, then advances `data[0]` by `data[1]`.
    pub fn on_timeout(&mut self, t: TriggerId) -> bool {
        let afc = self.act_frame_count;
        let tr = self.event.trigger_list.get_mut(t);
        let TriggerFunc::OnTimeout { time, interval } = &mut tr.func else { return false };
        if afc >= *time {
            *time = time.wrapping_add(*interval);
            true
        } else {
            false
        }
    }

    /// Port of `OnRandomTimeout` (Common/evenfunc.c:145).
    pub fn on_random_timeout(&mut self, t: TriggerId) -> bool {
        let TriggerFunc::OnRandomTimeout { time, start, end } = self.event.trigger_list.get(t).func else { return false };
        if self.act_frame_count >= time {
            let r = self.rng.random(end.wrapping_sub(start)) as u32;
            if let TriggerFunc::OnRandomTimeout { time: tm, .. } = &mut self.event.trigger_list.get_mut(t).func {
                *tm = time.wrapping_add(start).wrapping_add(r);
            }
            true
        } else {
            false
        }
    }

    /// Port of `EnemyAtFlag` (Common/evenfunc.c:224).
    pub fn enemy_at_flag(&self, id: i32, flag: i32) -> bool {
        let el = &self.enemies.enemy_list;
        let mut c = el.head.first;
        while let Some(e) = c {
            let n = &el.array[e.idx()];
            if (id == 0 || n.uid == id) && n.path.from_node as i32 == flag {
                return true;
            }
            c = n.links.next;
        }
        false
    }

    /// Port of `PlatformAtFlag` (Common/evenfunc.c:243).
    pub fn platform_at_flag(&self, id: i32, flag: i32) -> bool {
        let pl = &self.platform.platform_list;
        let mut c = pl.head.first;
        while let Some(p) = c {
            let n = &pl.array[p.idx()];
            if (id == 0 || n.uid == id) && n.path.from_node as i32 == flag {
                return true;
            }
            c = n.links.next;
        }
        false
    }

    /// Port of `LevelIsOpen` (Common/evenfunc.c:270). Out-of-range indices read past
    /// `worldVisualData` in the original; the port answers false.
    pub fn level_is_open(&self, world: i32, level: i32) -> bool {
        let Some(w) = usize::try_from(world).ok().and_then(|w| self.layout.world_visual_data.get(w)) else { return false };
        if level == 0xFF {
            w.world_open != 0
        } else {
            w.world_open != 0 && usize::try_from(level).ok().and_then(|l| w.level_visual_data.get(l)).is_some_and(|l| l.level_open != 0)
        }
    }

    /// Port of `FrogIsDead` (Common/evenfunc.c:300).
    pub fn frog_is_dead(&self, pl: i32) -> bool {
        self.frogger.player.get(pl as usize).is_some_and(|p| p.frog_state & FROGSTATUS_ISDEAD != 0)
    }

    /// Port of `PlaySFX` (Common/evenfunc.c:430).
    pub fn play_sfx(&mut self, sample: SfxSample, point: Option<SfxPoint>) {
        let pos = point.map(|p| match p {
            SfxPoint::ActorPosition(a) => self.actor2.actors[a].actor.position,
            SfxPoint::Point(v) => v,
        });
        match sample {
            SfxSample::Sample(uid) => self.play_sample(uid, pos, 0, SAMPLE_VOLUME, -1),
            SfxSample::Voice { player, uid } => {
                // TODO(port): FindVoice resolves per character in the audio layer; a
                // positioned voice has no Effect yet (tongue.c plays it at the frog).
                let _ = pos;
                self.effects.push(Effect::PlayVoice { player, uid });
            }
        }
    }

    /// Port of `ChangeLevel` (Common/evenfunc.c:466).
    pub fn change_level(&mut self, w_num: i32, l_num: i32) {
        self.frogger.player[0].world_num = w_num as u8;
        self.frogger.player[0].level_num = l_num as u8;
        self.game_state.mode = LEVELCOMPLETE_MODE;
        self.game.mode_timer.init(1);
        self.script.show_end_level_screen = 0;
    }

    /// Port of `TeleportFrog` (Common/evenfunc.c:470).
    pub fn teleport_frog(&mut self, f: i32, tile: TileId) {
        let fu = f as usize;
        let Some(act) = self.frogger.frog[fu] else {
            self.effects.push(Effect::Log { text: format!("TeleportFrog: frog[{f}] is NULL") });
            return;
        };
        self.teleport_actor_to_tile(act, tile, f);

        let s = self.frogger.global_frog_scale;
        self.actor2.actors[act].actor.size = FVector { vx: s, vy: s, vz: s };
        if self.frogger.player[fu].character == FROG_BABYFROG {
            scale_vector_ff(&mut self.actor2.actors[act].actor.size, 3072);
        }

        self.init_camera();
        // PC: dissolveTex override texture reset (renderer state) dropped.
    }

    /// Port of `TeleportActorToTile` (Common/actor2.c:1230): teleports the actor to the
    /// specified tile.
    pub fn teleport_actor_to_tile(&mut self, act: ActorId, tile: TileId, pl: i32) {
        let p = pl as usize;
        // make the teleport 'to' tile the current tile
        self.frogmove.curr_tile[p] = Some(tile);
        self.actor2.actors[act].actor.position = self.map.tile(tile).centre;
        let pp = &mut self.frogger.player[p];
        pp.frog_state &= !FROGSTATUS_ISTELEPORTING;
        pp.can_jump = 1;
        pp.frogon = -1;
        pp.frogunder = -1;

        if let Some(cp) = self.platform.curr_platform[p] {
            let plat = &mut self.platform.platform_list.array[cp.idx()];
            plat.carrying = None;
            plat.flags &= !PLATFORM_NEW_CARRYINGFROG;
        }

        self.frogmove.dest_tile[p] = None;
        self.platform.curr_platform[p] = None;
        self.platform.dest_platform[p] = None;

        self.frogger.player[p].extended_hop_dir = ((self.frogmove.frog_facing[p] - self.cam.cam_facing[p]) & 3) as u8;
        let t = self.map.tile(tile);
        let (dir, normal) = (t.dir_vector[self.frogmove.frog_facing[p] as usize & 3], t.normal);
        orientate_ss(&mut self.actor2.actors[act].actor.q_rot, &dir, &normal);
    }
}
