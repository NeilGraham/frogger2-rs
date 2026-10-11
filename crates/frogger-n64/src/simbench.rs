//! Phase-3S simulation benchmark (feature `simbench`): runs `f2_game::simbench::SCENARIOS`
//! back to back without rendering or audio, timing every `Game::tick` with C0 COUNT, and logs
//! per scenario the mean / worst tick and the end-of-run state checksum (must equal the
//! host's `cargo run --release -p f2-game --example sim_bench`). With `profile` the sampling
//! profiler runs inside the ticks only and dumps one `PROF` window per scenario
//! (`tools/n64/prof.py <log> <elf> <window>`).

use crate::data::{DfsStore, N64Data};
use crate::log;
use crate::rt::{self, ticks, ticks_to_us};
use alloc::sync::Arc;
use f2_game::simbench::{self, SCENARIOS};

pub fn run_all() {
    let data = Arc::new(N64Data::new(DfsStore));
    #[cfg(feature = "profile")]
    unsafe {
        crate::ffi::n64_prof(0);
        crate::ffi::n64_prof(2);
    }
    log!("simbench: {} scenarios", SCENARIOS.len());
    for (i, s) in SCENARIOS.iter().enumerate() {
        let t0 = ticks();
        let mut g = simbench::setup(s, data.clone());
        let load_ms = ticks_to_us(ticks().wrapping_sub(t0)) / 1000;
        let (mut total, mut worst, mut start) = (0u64, 0u32, 0u32);
        let out = simbench::run(s, &mut g, &mut |begin| {
            if begin {
                #[cfg(feature = "profile")]
                unsafe {
                    crate::ffi::n64_prof(3)
                };
                start = ticks();
            } else {
                let d = ticks().wrapping_sub(start);
                #[cfg(feature = "profile")]
                unsafe {
                    crate::ffi::n64_prof(2)
                };
                total += d as u64;
                worst = worst.max(d);
            }
        });
        drop(g);
        let mean_us = (total * 1_000_000 / rt::TICKS_PER_SEC as u64 / out.ticks.max(1) as u64) as u32;
        log!(
            "simbench {} {}: ticks {} actors {} checksum {:016x} mean {} us/tick worst {} us (load {} ms)",
            i,
            s.name,
            out.ticks,
            out.actors,
            out.checksum,
            mean_us,
            ticks_to_us(worst),
            load_ms
        );
        #[cfg(feature = "profile")]
        unsafe {
            crate::ffi::n64_prof(1);
            crate::ffi::n64_prof(2);
        }
    }
    log!("simbench: done");
}
