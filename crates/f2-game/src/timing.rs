//! Phase 3T: the retail PC game's **variable timestep**, as an opt-in alternative to the
//! fixed 60 Hz [`Game::tick`].
//!
//! The original main loop (`RunWindowsLoop`, `MDX Library/mdxWindows.cpp:216`) runs
//! `LoopFunc` (`PC/Main.cpp:686`) and then `UpdateTiming` (`MDX Library/mdxTiming.cpp:50`)
//! once per rendered frame. `UpdateTiming` measures the wall-clock time since the previous
//! frame (capped at `maxTickIncrement` = 500 ms) as `timeInfo.speed` (elapsed 60 Hz frames, a
//! float) and `timeInfo.frameCount` (60 Hz frames since `InitTiming`); `LoopFunc` turns them
//! into `gameSpeed` (20.12, 4096 = one frame) and `actFrameCount`, then runs one `GameLoop`.
//! On a slow PC every movement in the game is therefore one step of `gameSpeed`, frame-gated
//! logic (`actFrameCount` deltas, `frameCount & 1`) sees fewer, larger steps, and so on
//! (`research/known-bugs.md` #10, `research/n64-performance.md` §3T).
//!
//! This module ports exactly that: [`TimeInfo`] = `MDX_TIMING_VAL timeInfo` with `InitTiming`
//! / `UpdateTiming` (x87 PC24 arithmetic as in the retail exe, `FUN_0044b430`), and
//! [`Game::tick_timed`] = `LoopFunc` (retail `0x43cc90`) including the pause branch
//! (`gameSpeed = 0`, `pFrameModifier`, `beenPaused`, `pauseGameSpeed`). Clocks:
//! * [`Game::update_timing`]: the retail `UpdateTiming` from a host millisecond clock
//!   (`timeGetTime`); with [`Game::timing_clock`] set, `InitTiming` (inside `InitLevel`) and
//!   `InitBackdrop`'s `UpdateTiming` read it too, as the retail game does;
//! * [`Game::advance_timing_frames`]: a clock that advances in whole 60 Hz frames — the
//!   oracle's `h_tick_timing` hook (`docs/ORACLE.md`: `frameCount += 1`, `tickCount =
//!   frameCount*1000/60`, `speed = 1.0`) generalised to `n` frames, i.e. a PC whose frame
//!   took exactly `n/60` s. `tick_frames(input, 1)` is the oracle's (and [`Game::tick`]'s)
//!   timing.
//!
//! [`Game::tick`] is unchanged (it never touches this state and keeps `gameSpeed` at 4096):
//! it stays the parity reference. Outside the pause menu `tick_frames(input, 1)` produces the
//! same trace as `tick(input)`. In `PAUSE_MODE` the two differ: `tick` keeps `gameSpeed`
//! 4096 and advances `actFrameCount`, while the retail `LoopFunc` (and `tick_timed`) freezes
//! both (a port gap of `tick`; the parity corpus never pauses).

use serde::{Deserialize, Serialize};

use crate::{Game, TickInput};

/// `maxTickIncrement` default (`InitTiming`, `mdxTiming.cpp:39`): one frame measures at most
/// 500 ms, i.e. 30 frames of 60 Hz.
pub const MAX_TICK_INCREMENT_MS: i32 = 500;
/// The most whole 60 Hz frames one main-loop iteration can advance (500 ms).
pub const MAX_FRAMES_PER_TICK: u32 = 30;
/// `InitTiming(60)` (`Common/layout.c:742`, `:942`; `PC/Main.cpp:827`).
pub const FRAME_SPEED: f32 = 60.0;

/// `MDX_TIMING_VAL timeInfo` (`MDX Library/mdxTiming.h`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimeInfo {
    /// `firstTicks` (`long`): `timeGetTime()` at `InitTiming`, moved forward by clamped time.
    pub first_ticks: i32,
    /// `tickCount`: milliseconds since `InitTiming` at the last `UpdateTiming`.
    pub tick_count: u32,
    /// `frameCount` (`CFRAME`): 60 Hz frames since `InitTiming`.
    pub frame_count: u32,
    /// `tickModifier`.
    pub tick_modifier: i32,
    /// `maxTickIncrement` (ms).
    pub max_tick_increment: i32,
    /// `frameSpeed` (60.0).
    pub frame_speed: f32,
    /// `speed`: 60 Hz frames elapsed in the last frame (1.0 at 60 fps).
    pub speed: f32,
}

impl Default for TimeInfo {
    fn default() -> Self {
        // InitTiming(60) at time 0; `speed` is not set by InitTiming (zero-initialised global).
        TimeInfo {
            first_ticks: 0,
            tick_count: 0,
            frame_count: 0,
            tick_modifier: 0,
            max_tick_increment: MAX_TICK_INCREMENT_MS,
            frame_speed: FRAME_SPEED,
            speed: 0.0,
        }
    }
}

/// `ftol` (MSVC `_ftol`, retail `0x462c64`): `fistp qword` with the rounding control set to
/// chop, the low 32 bits returned in `eax`.
#[inline]
fn ftol(x: f32) -> u32 {
    (x as f64) as i64 as u32
}

impl TimeInfo {
    /// Port of `InitTiming(frameSpeed)` (`mdxTiming.cpp:32`); `now_ms` = `timeGetTime()`.
    pub fn init(&mut self, now_ms: u32, frame_speed: f32) {
        self.first_ticks = now_ms as i32;
        self.tick_count = 0;
        self.frame_count = 0;
        self.frame_speed = frame_speed;
        self.tick_modifier = 0;
        self.max_tick_increment = MAX_TICK_INCREMENT_MS;
    }

    /// `mult = frameSpeed/1000.0F`: the retail exe multiplies by the `float` constant 0.001
    /// (`0x3a83126f`) and stores the PC24 product as a `float` (0.060000002 for 60).
    #[inline]
    fn mult(&self) -> f32 {
        f2_math::mul_pc24(self.frame_speed as f64, 0.001f32 as f64)
    }

    /// Port of `UpdateTiming` (`mdxTiming.cpp:50`, retail `0x44b430`); `now_ms` =
    /// `timeGetTime()`. Integer maths is `unsigned long` (wrapping); the two products are x87
    /// PC24 (`fild` of the zero-extended count, one rounding to 24 bits), `frameCount` is
    /// `ftol` of the unstored product.
    pub fn update(&mut self, now_ms: u32) {
        let mult = self.mult();
        let mut current = now_ms.wrapping_add(self.tick_modifier as u32).wrapping_sub(self.first_ticks as u32);
        let mut interval = current.wrapping_sub(self.tick_count);
        // limit maximum clock increment (unsigned compare: `jbe`)
        let max = self.max_tick_increment as u32;
        if interval > max {
            interval = interval.wrapping_sub(max);
            self.first_ticks = self.first_ticks.wrapping_add(interval as i32);
            current = current.wrapping_sub(interval);
            interval = max;
        }
        self.speed = f2_math::mul_pc24(interval as f64, mult as f64);
        self.frame_count = ftol(f2_math::mul_pc24(current as f64, mult as f64));
        self.tick_count = current;
    }

    /// A clock that advances `n` whole 60 Hz frames (capped at the 500 ms of
    /// `maxTickIncrement`, [`MAX_FRAMES_PER_TICK`]): the oracle's `h_tick_timing`
    /// (`crates/f2fix/src/win/oracle.rs`) for `n` = 1, i.e. `UpdateTiming` on a PC whose
    /// frames took exactly `n/60` s.
    pub fn step_frames(&mut self, n: u32) {
        let n = n.min(MAX_FRAMES_PER_TICK);
        self.frame_count = self.frame_count.wrapping_add(n);
        self.tick_count = ((self.frame_count as u64 * 1000) / 60) as u32;
        self.speed = n as f32;
    }
}

/// The variable-timestep state: `timeInfo` plus `LoopFunc`'s globals (`PC/Main.cpp`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimingState {
    /// `timeInfo`.
    pub time_info: TimeInfo,
    /// `pFrameModifier` (`long`): frames spent paused, subtracted from `frameCount`.
    pub p_frame_modifier: u32,
    /// `beenPaused`.
    pub been_paused: i32,
    /// `lastFrames`, `lastTicks`: `timeInfo` at the last unpaused frame.
    pub last_frames: u32,
    pub last_ticks: u32,
    /// `turbo` (`long turbo = 4096`; `LoopFunc` uses `turbo/4096` in integer maths).
    pub turbo: i32,
    /// The host clock last passed to [`Game::update_timing`] (`InitTiming`'s
    /// `timeGetTime()` when no [`Game::timing_clock`] is set).
    pub now_ms: u32,
    /// Set once the variable-timestep API has been used (the state then joins the netsync
    /// checksum; under [`Game::tick`] it stays untouched and out of it).
    pub active: bool,
}

impl Default for TimingState {
    fn default() -> Self {
        TimingState {
            time_info: TimeInfo::default(),
            p_frame_modifier: 0,
            been_paused: 0,
            last_frames: 0,
            last_ticks: 0,
            turbo: 4096,
            now_ms: 0,
            active: false,
        }
    }
}

impl Game {
    /// `timeGetTime()` for `InitTiming`/`InitBackdrop` (the host clock if one is set).
    fn timing_now(&self) -> u32 {
        match self.timing_clock {
            Some(f) => f(),
            None => self.timing.now_ms,
        }
    }

    /// `InitTiming(60)` (`Common/layout.c:742`, `:942`, inside `InitLevel`). The fixed-step
    /// [`Game::tick`] does not read `timeInfo`.
    pub(crate) fn init_timing(&mut self) {
        let now = self.timing_now();
        self.timing.time_info.init(now, FRAME_SPEED);
    }

    /// `InitBackdrop`'s / `InitCDBackdrop`'s trailing `UpdateTiming()` (`PC/backdrop.c:55`):
    /// with a host clock ([`Game::timing_clock`]) the load time is counted as the retail game
    /// does; without one the clock is held (the oracle's `h_hold_timing`).
    pub(crate) fn backdrop_update_timing(&mut self) {
        if let Some(f) = self.timing_clock {
            if self.timing.active {
                let now = f();
                self.timing.now_ms = now;
                self.timing.time_info.update(now);
            }
        }
    }

    /// The retail `UpdateTiming()` from a host millisecond clock (`timeGetTime`); call it
    /// once per main-loop iteration before [`Game::tick_timed`].
    pub fn update_timing(&mut self, now_ms: u32) {
        self.timing.active = true;
        self.timing.now_ms = now_ms;
        self.timing.time_info.update(now_ms);
    }

    /// Advances the timing clock by `n` whole 60 Hz frames (at most
    /// [`MAX_FRAMES_PER_TICK`]); see [`TimeInfo::step_frames`].
    pub fn advance_timing_frames(&mut self, n: u32) {
        self.timing.active = true;
        self.timing.time_info.step_frames(n);
    }

    /// One iteration of the retail main loop with the current `timeInfo`: `LoopFunc`'s
    /// timing (`gameSpeed`, `actFrameCount`, pause handling), `ProcessUserInput`, `GameLoop`,
    /// `CopyActorList` and `DrawLoop`'s state changes.
    pub fn tick_timed(&mut self, input: TickInput) {
        self.tick_timed_game_loop(input);
        self.after_game_loop();
    }

    /// The first half of [`Game::tick_timed`] (as [`Game::tick_game_loop`] is of
    /// [`Game::tick`]): `LoopFunc`'s timing, `ProcessUserInput`, `GameLoop`; the trace point.
    pub fn tick_timed_game_loop(&mut self, input: TickInput) {
        self.timing.active = true;
        self.loop_func_timing();
        self.process_user_input(input.pads);
        self.phase(crate::simbench::phase::INPUT);
        self.game_loop();
        self.phase(crate::simbench::phase::GAME_LOOP_REST);
    }

    /// One main-loop iteration that took `n` 60 Hz frames (`n` ≥ 1, capped at
    /// [`MAX_FRAMES_PER_TICK`]): [`Game::advance_timing_frames`] + [`Game::tick_timed`].
    /// `gameSpeed` = 4096·n and `actFrameCount` advances by `n` (outside the pause menu).
    pub fn tick_frames(&mut self, input: TickInput, n: u32) {
        self.advance_timing_frames(n);
        self.tick_timed(input);
    }

    /// The timing part of `LoopFunc` (`PC/Main.cpp:688-713`, retail `0x43cc90`).
    fn loop_func_timing(&mut self) {
        use crate::define::mode::PAUSE_MODE;
        let t = &mut self.timing;
        let paused = self.game_state.mode == PAUSE_MODE && !(self.menus.quitting_level != 0 && self.menus.pause_confirm_mode == 0);
        if !paused {
            self.last_act_frame_count = self.act_frame_count;
            // (float)(turbo/4096): integer division (cdq / and 0xfff / sar 12), then fild.
            let turbo = (t.turbo / 4096) as f64;
            // fild qword of the zero-extended difference, fmul turbo (PC24), ftol.
            let frames = t.time_info.frame_count.wrapping_sub(t.p_frame_modifier);
            self.act_frame_count = ftol(f2_math::mul_pc24(frames as f64, turbo));
            // gameSpeed = ftol(turbo * timeInfo.speed * 4096.0f), each product PC24.
            let s = f2_math::mul_pc24(turbo, t.time_info.speed as f64);
            self.game_speed = ftol(f2_math::mul_pc24(s as f64, 4096.0)) as i32;
            t.last_frames = t.time_info.frame_count;
            t.last_ticks = t.time_info.tick_count;
            if t.been_paused != 0 {
                t.p_frame_modifier = t.p_frame_modifier.wrapping_add(self.act_frame_count.wrapping_sub(self.last_act_frame_count));
                self.act_frame_count = self.last_act_frame_count;
                self.game_speed = self.menus.pause_game_speed;
                t.been_paused = 0;
            }
            self.menus.pause_game_speed = self.game_speed;
        } else {
            t.been_paused = 1;
            self.game_speed = 0;
            t.time_info.speed = 0.0;
            t.time_info.frame_count = t.last_frames;
            t.time_info.tick_count = t.last_ticks;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mult_is_the_retail_constant() {
        assert_eq!(0.001f32.to_bits(), 0x3a83_126f);
        let t = TimeInfo::default();
        // 60 * 0.001f rounded once to float.
        assert_eq!(t.mult(), (60.0f64 * 0.001f32 as f64) as f32);
    }

    #[test]
    fn update_timing_counts_and_clamps() {
        let mut t = TimeInfo::default();
        t.init(1000, FRAME_SPEED);
        t.update(1017);
        assert_eq!(t.tick_count, 17);
        assert_eq!(t.frame_count, 1);
        assert_eq!((t.speed * 4096.0) as i32, 4177); // 17 ms at 60 fps
        t.update(1033);
        assert_eq!(t.frame_count, 1); // 33 ms = 1.98 frames
        t.update(1050);
        assert_eq!(t.frame_count, 3); // 50 ms = 3.0000002 frames
        assert_eq!((t.speed * 4096.0) as i32, 4177);
        // A 2 s stall counts as 500 ms; firstTicks moves forward by the rest.
        t.update(3050);
        assert_eq!(t.tick_count, 550);
        assert_eq!(t.first_ticks, 1000 + 1500);
        assert_eq!(t.speed, f2_math::mul_pc24(500.0, t.mult() as f64));
        assert_eq!(t.frame_count, 33);
    }

    #[test]
    fn step_frames_matches_the_oracle_hook() {
        let mut t = TimeInfo::default();
        for i in 1..=10u32 {
            t.step_frames(1);
            assert_eq!(t.frame_count, i);
            assert_eq!(t.tick_count, i * 1000 / 60);
            assert_eq!(t.speed, 1.0);
        }
        t.step_frames(100);
        assert_eq!(t.frame_count, 10 + MAX_FRAMES_PER_TICK);
        assert_eq!(t.speed, MAX_FRAMES_PER_TICK as f32);
    }

    #[test]
    fn loop_func_speed_and_pause() {
        use crate::define::mode::{INGAME_MODE, PAUSE_MODE};
        let mut g = Game::new();
        g.game_state.mode = INGAME_MODE;
        g.advance_timing_frames(3);
        g.loop_func_timing();
        assert_eq!((g.game_speed, g.act_frame_count, g.last_act_frame_count), (3 * 4096, 3, 0));
        assert_eq!(g.menus.pause_game_speed, 3 * 4096);
        // Paused: gameSpeed 0, actFrameCount frozen, the clock held at the last frame.
        g.game_state.mode = PAUSE_MODE;
        for _ in 0..5 {
            g.advance_timing_frames(2);
            g.loop_func_timing();
            assert_eq!((g.game_speed, g.act_frame_count), (0, 3));
            assert_eq!(g.timing.time_info.speed, 0.0);
        }
        // Resumed: the first frame keeps actFrameCount and uses pauseGameSpeed; the frames
        // spent paused go into pFrameModifier.
        g.game_state.mode = INGAME_MODE;
        g.advance_timing_frames(1);
        g.loop_func_timing();
        assert_eq!((g.game_speed, g.act_frame_count, g.timing.p_frame_modifier), (3 * 4096, 3, 1));
        g.advance_timing_frames(1);
        g.loop_func_timing();
        assert_eq!((g.game_speed, g.act_frame_count, g.last_act_frame_count), (4096, 4, 3));
    }
}
