//! Phase 3T: simulation pacing with the retail variable timestep (`f2_game::timing`).
//!
//! The PC game runs one `GameLoop` per rendered frame and scales every movement by the
//! measured frame time (`gameSpeed` = 4096 per 60 Hz frame, capped at 500 ms). The N64 does
//! the same with a whole-frame clock: when the main loop is on time it runs ordinary 60 Hz
//! ticks; when it falls behind by `k` frames it runs **one** tick of `k` frames
//! (`Game::tick_frames`, `gameSpeed` = 4096·k) instead of `k` ticks, `k` capped at
//! [`MAX_FRAMES_PER_TICK`] (time beyond the cap is dropped, like the PC's 500 ms clamp).
//! This is the original game's own slow-machine behaviour (`docs/DECISIONS.md`,
//! `research/n64-performance.md` §3T).
//!
//! Build-time switches: `F2N64_TIMESTEP=fixed` keeps the fixed 60 Hz catch-up loop
//! (`Game::tick`, up to `MAX_TICKS_PER_FRAME` ticks per frame; for debugging and parity);
//! `F2N64_MAXSTEP=n` sets the cap (default 4 frames = 15 fps).

use f2_game::{Game, TickInput};

/// Variable timestep (default) or the fixed 60 Hz loop (`F2N64_TIMESTEP=fixed`).
pub const VARIABLE: bool = match option_env!("F2N64_TIMESTEP") {
    Some(s) => !matches!(s.as_bytes(), b"fixed"),
    None => true,
};

const fn parse_cap(s: Option<&str>) -> u32 {
    let Some(s) = s else { return 4 };
    let b = s.as_bytes();
    let mut v = 0u32;
    let mut i = 0;
    while i < b.len() {
        if b[i] < b'0' || b[i] > b'9' {
            return 4;
        }
        v = v * 10 + (b[i] - b'0') as u32;
        i += 1;
    }
    if v == 0 {
        1
    } else if v > f2_game::timing::MAX_FRAMES_PER_TICK {
        f2_game::timing::MAX_FRAMES_PER_TICK
    } else {
        v
    }
}

/// Most 60 Hz frames one variable tick covers (`F2N64_MAXSTEP`, default 4).
pub const MAX_FRAMES_PER_TICK: u32 = parse_cap(option_env!("F2N64_MAXSTEP"));

/// What one main-loop iteration simulates: `ticks` ticks of `frames` 60 Hz frames each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    pub ticks: u32,
    pub frames: u32,
    /// 60 Hz frames of real time dropped (beyond the catch-up limit).
    pub dropped: u32,
}

/// Plans an iteration from `acc` (real time owed, in C0 COUNT ticks; `tick` per 60 Hz frame)
/// and removes the dropped time from `acc`. The caller subtracts `frames * tick` per tick run.
pub fn plan(acc: &mut u32, tick: u32, max_fixed_ticks: u32) -> Plan {
    let owed = *acc / tick;
    let (ticks, frames, dropped) = if VARIABLE {
        match owed {
            0 => (0, 1, 0),
            n => {
                let k = n.min(MAX_FRAMES_PER_TICK);
                (1, k, n - k)
            }
        }
    } else {
        let n = owed.min(max_fixed_ticks);
        (n, 1, owed - n)
    };
    *acc -= dropped * tick;
    Plan { ticks, frames, dropped }
}

/// Runs one simulation tick of `frames` 60 Hz frames.
#[inline]
pub fn tick(game: &mut Game, input: TickInput, frames: u32) {
    if VARIABLE {
        game.tick_frames(input, frames);
    } else {
        game.tick(input);
    }
}

/// Render interpolation between the last tick's start and end state: `acc` (time owed
/// since that tick) over the tick's length (`frames` 60 Hz frames).
#[inline]
pub fn alpha(acc: u32, tick: u32, frames: u32) -> f32 {
    (acc as f32 / (tick * frames.max(1)) as f32).clamp(0.0, 1.0)
}

/// [`crate::app::pick_fps`] for a given timestep mode: the first of `rates` that fits.
pub fn pick_fps_mode(rates: &[u32], tick_us: u32, draw_us: u32, budget_us: u32, variable: bool, max_frames_per_tick: u32) -> u32 {
    let (t, d) = (tick_us as u64, draw_us as u64);
    rates
        .iter()
        .copied()
        .find(|&f| {
            let f = f as u64;
            if variable {
                60 / f <= max_frames_per_tick as u64 && f * (t + d) <= budget_us as u64
            } else {
                60 * t + f * d <= budget_us as u64
            }
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_cap_counts_one_tick_per_frame() {
        const R: [u32; 4] = [60, 30, 20, 15];
        // City (ares): tick 7.0 ms, frame 23.7 ms: 20 fps (30 would need 921 ms/s), not the
        // 15 the 60-ticks-a-second model picked.
        assert_eq!(pick_fps_mode(&R, 6962, 23716, 900_000, true, 4), 20);
        // ... it could not climb back from 15: under the old model even 20 misses the 800 ms
        // "faster" threshold; counted per frame it fits.
        assert_eq!(pick_fps_mode(&R, 6962, 23716, 800_000, false, 4), 15);
        assert_eq!(pick_fps_mode(&R, 6962, 23716, 800_000, true, 4), 20);
        // 15 fps is the lowest the 4-frames-per-tick cap keeps in real time.
        assert_eq!(pick_fps_mode(&R, 11_000, 40_000, 900_000, true, 4), 15);
        assert_eq!(pick_fps_mode(&R, 11_000, 40_000, 900_000, true, 3), 0);
        assert_eq!(pick_fps_mode(&R, 4_000, 10_000, 900_000, true, 4), 60);
    }

    #[test]
    fn plans() {
        let t = 100;
        let mut acc = 50;
        assert_eq!(plan(&mut acc, t, 6), Plan { ticks: 0, frames: 1, dropped: 0 });
        if VARIABLE {
            let mut acc = 250;
            assert_eq!(plan(&mut acc, t, 6), Plan { ticks: 1, frames: 2, dropped: 0 });
            assert_eq!(acc, 250);
            let mut acc = 1050;
            let p = plan(&mut acc, t, 6);
            assert_eq!(p, Plan { ticks: 1, frames: MAX_FRAMES_PER_TICK, dropped: 10 - MAX_FRAMES_PER_TICK });
            assert_eq!(acc, 1050 - p.dropped * t);
        }
        assert_eq!(parse_cap(None), 4);
        assert_eq!(parse_cap(Some("2")), 2);
        assert_eq!(parse_cap(Some("99")), 30);
        assert_eq!(parse_cap(Some("x")), 4);
        assert!((alpha(150, 100, 2) - 0.75).abs() < 1e-6);
    }
}
