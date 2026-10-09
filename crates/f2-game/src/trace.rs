//! Per-tick state trace in the retail oracle's schema (`tools/f2-trace`,
//! `crates/f2fix/src/win/oracle.rs` `read_frame`).

use f2_trace::{EnemyState, GaribState, PlatformState, PlayerState, TraceFrame, SCHEMA_VERSION};

use crate::ids::{ActorId, TileId};
use crate::Game;

/// `(ptr - firstTile) / sizeof(GAMETILE)`, -1 for NULL.
fn tile_index(t: Option<TileId>) -> i32 {
    t.map_or(-1, |t| t.0 as i32)
}

impl Game {
    fn actor_pos_trace(&self, a: Option<ActorId>) -> Option<[i16; 3]> {
        let a = self.actor2.actors.get(a?)?;
        let p = a.actor.position;
        Some([p.vx, p.vy, p.vz])
    }

    /// The state after this tick's `GameLoop`, as the oracle records it. `seq` is set
    /// to `act_frame_count - 1` (the oracle's line index when tracing from the level
    /// start); callers writing a different window should overwrite it.
    pub fn trace_frame(&self) -> TraceFrame {
        let mut f = TraceFrame {
            v: SCHEMA_VERSION,
            seq: self.act_frame_count.wrapping_sub(1),
            act_frame_count: self.act_frame_count,
            frame_count: self.frame_count,
            game_speed: self.game_speed,
            mode: self.game_state.mode,
            multi: self.game_state.multi,
            single: self.game_state.single,
            world: self.frogger.player[0].world_num,
            level: self.frogger.player[0].level_num,
            rseed: self.rng.rseed,
            pads: self.levplay.pad_data.digital,
            babies_saved: self.babyfrog.babies_saved as i32,
            num_babies: self.map.num_babies,
            end_level_timer: self.game.end_level_timer.time,
            cam_source: [self.cam.curr_cam_source.vx, self.cam.curr_cam_source.vy, self.cam.curr_cam_source.vz],
            cam_target: [self.cam.curr_cam_target.vx, self.cam.curr_cam_target.vy, self.cam.curr_cam_target.vz],
            ..Default::default()
        };

        for i in 0..self.frogger.num_frogs.clamp(0, 4) as usize {
            let p = &self.frogger.player[i];
            f.players.push(PlayerState {
                pos: self.actor_pos_trace(self.frogger.frog[i]),
                frog_state: p.frog_state,
                curr_tile: tile_index(self.frogmove.curr_tile[i]),
                lives: p.lives,
                health: p.health_points,
                death_by: p.death_by,
                can_jump: p.can_jump,
                has_jumped: p.has_jumped,
                is_super_hopping: p.is_super_hopping,
                is_sinking: p.is_sinking,
                num_spawn: p.num_spawn,
                safe_time: p.safe.time,
                dead_time: p.dead.time,
                jump_time: p.jump_time,
                height_jumped: p.height_jumped,
            });
        }

        let el = &self.enemies.enemy_list;
        let mut c = el.head.first;
        while let Some(id) = c {
            let e = &el.array[id.idx()];
            f.enemies.push(EnemyState {
                uid: e.uid,
                active: e.active,
                pos: self.actor_pos_trace(e.nme_actor),
                flags: e.flags,
                is_waiting: e.is_waiting,
                speed: e.speed,
                in_tile: tile_index(e.in_tile),
            });
            c = e.links.next;
        }

        let pl = &self.platform.platform_list;
        let mut c = pl.head.first;
        while let Some(id) = c {
            let p = &pl.array[id.idx()];
            f.platforms.push(PlatformState {
                uid: p.uid,
                active: p.active,
                pos: self.actor_pos_trace(p.plt_actor),
                flags: p.flags,
                is_waiting: p.is_waiting,
                curr_speed: p.curr_speed,
                in_tile: [tile_index(p.in_tile[0]), tile_index(p.in_tile[1])],
            });
            c = p.links.next;
        }

        let gl = &self.collect.garib_list;
        f.garib_count = gl.count;
        f.garib_total = gl.total;
        for id in gl.head.ids(&gl.array) {
            let g = &gl.array[id.idx()];
            f.garibs.push(GaribState { kind: g.garib_type, active: g.active, pos: [g.pos.vx, g.pos.vy, g.pos.vz] });
        }
        f
    }
}
