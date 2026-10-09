//! Per-tick state trace in the retail oracle's schema (`tools/f2-trace`,
//! `crates/f2fix/src/win/oracle.rs` `read_frame`).

use f2_math::{FVector, SVector};
use f2_trace::{
    BabyState, CameraState, EnemyState, GaribExt, GaribState, HudState, PlatformState, PlayerExt, PlayerState, ScriptState, SpecFxSection, SpecFxState,
    SpritesState, TongueState, TraceFrame, TriggerState, TriggersState, SCHEMA_VERSION,
};

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
        self.trace_v2(&mut f);
        f
    }

    /// Schema v2 sections, read like `crates/f2fix/src/win/oracle.rs` `read_v2`.
    fn trace_v2(&self, f: &mut TraceFrame) {
        fn fv(v: &FVector) -> [i32; 3] {
            [v.vx, v.vy, v.vz]
        }
        fn sv(v: &SVector) -> [i16; 3] {
            [v.vx, v.vy, v.vz]
        }
        let n = f.players.len();
        let mut px = Vec::with_capacity(n);
        let mut tg = Vec::with_capacity(n);
        for i in 0..n {
            let p = &self.frogger.player[i];
            px.push(PlayerExt {
                frog_facing: self.frogmove.frog_facing[i],
                cam_facing: self.cam.cam_facing[i],
                frogon: p.frogon,
                frogunder: p.frogunder,
                old_lives: p.old_lives,
                input_pause: p.input_pause,
                spawn_timer: p.spawn_timer,
                spawn_score_level: p.spawn_score_level,
                idle_time: p.idle_time,
                idle_enable: p.idle_enable,
                has_double_jumped: p.has_double_jumped,
                extended_hop_dir: p.extended_hop_dir,
                stun_time: p.stun.time,
                croak_time: p.is_croaking.time,
                autohop_time: p.autohop.time,
                quickhop_time: p.quickhop.time,
                slowhop_time: p.slowhop.time,
                jump_start_frame: p.jump_start_frame,
                jump_speed: p.jump_speed,
                jump_multiplier: p.jump_multiplier,
                jump_fwd: fv(&p.jump_fwd_vector),
                jump_up: fv(&p.jump_up_vector),
                jump_origin: sv(&p.jump_origin),
            });
            let t = &self.tongue.tongue[i];
            tg.push(TongueState {
                flags: t.flags,
                radius: t.radius,
                progress: t.progress,
                kind: t.ty,
                can_tongue: t.can_tongue,
                source: sv(&t.source),
                target: sv(&t.target),
                pos: sv(&t.pos),
                has_thing: t.thing.is_some() as u8,
            });
        }
        f.players_ext = Some(px);

        let gl = &self.collect.garib_list;
        f.garib_ext = Some(GaribExt {
            blocks: gl.blocks,
            max_coins: gl.max_coins,
            pause_flag: self.collect.pause_flag,
            pause_timer: self.collect.pause_timer.time,
            drop_speed: gl.head.ids(&gl.array).into_iter().map(|id| gl.array[id.idx()].drop_speed).collect(),
        });

        let nb = self.map.num_babies.clamp(0, self.babyfrog.baby_list.len() as i32) as usize;
        f.babies = Some(
            self.babyfrog.baby_list[..nb]
                .iter()
                .map(|b| BabyState { is_saved: b.is_saved as i32, idle: b.idle, collect: b.collect, pos: self.actor_pos_trace(b.baby) })
                .collect(),
        );

        let c = &self.cam;
        let tc = &c.tc_list;
        let cur0 = self.frogmove.curr_tile[0];
        let ntc = (tc.count.max(0) as usize).min(tc.array.len());
        let trans = match cur0 {
            Some(t) => (0..ntc).rev().find(|&i| tc.array[i].tile == Some(t)).map_or(-1, |i| i as i32),
            None => -1,
        };
        let fog = &self.frogger.fog;
        f.camera = Some(CameraState {
            source: fv(&c.cam_source),
            target: fv(&c.cam_target),
            cam_vect: fv(&c.cam_vect),
            curr_cam_dist: fv(&c.curr_cam_dist),
            cam_dist: sv(&c.cam_dist),
            curr_cam_offset: fv(&c.curr_cam_offset),
            cam_look_ofs: c.cam_look_ofs,
            trans_cam_speed_mult: c.trans_cam_speed_mult,
            y_fov_new: c.y_fov_new,
            fixed: [c.fixed_dir as u8, c.fixed_pos as u8, c.fixed_up as u8],
            cam_shakiness: c.cam_shakiness,
            trans,
            trans_count: tc.count,
            fog: [fog.r as i32, fog.g as i32, fog.b as i32, fog.mode as i32, fog.min as i32, fog.max as i32],
        });

        f.script = Some(ScriptState { flags: self.script.script_flags.to_vec(), counters: self.script.script_counters.to_vec() });

        let tl = &self.event.trigger_list;
        f.triggers = Some(TriggersState {
            count: tl.count,
            list: tl
                .head
                .ids(&tl.triggers)
                .into_iter()
                .map(|id| {
                    let t = &tl.triggers[id.idx()];
                    TriggerState { flags: t.flags, count: t.count, delay: t.delay }
                })
                .collect(),
        });

        f.specfx = Some(SpecFxSection {
            count: self.specfx.count as i16,
            list: self
                .specfx
                .iter_list()
                .map(|(_, x)| SpecFxState { kind: x.ty, origin: sv(&x.origin), lifetime: x.lifetime, update_type: x.update_type as u8 })
                .collect(),
        });
        f.sprites = Some(SpritesState { count: self.sprite.count as i16, particles: self.particle.count as i16 });
        f.tongue = Some(tg);
        f.hud = Some(HudState {
            score_timer: self.game.score_timer.time,
            mode_timer: self.game.mode_timer.time,
            go_timer: self.hud.go_timer.time,
            demo_timeout: self.levplay.demo_timeout.time,
            hand_angle: self.hud.hand_angle,
            store_frame_count: self.hud.store_frame_count as u32,
        });
    }
}
