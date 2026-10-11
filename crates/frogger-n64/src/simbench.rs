//! Phase-3S simulation benchmark (feature `simbench`): runs `f2_game::simbench::SCENARIOS`
//! back to back without rendering or audio, timing every `Game::tick` with C0 COUNT, and logs
//! per scenario the mean / worst tick and the end-of-run state checksum (must equal the
//! host's `cargo run --release -p f2-game --example sim_bench`). With `profile` the sampling
//! profiler runs inside the ticks only and dumps one `PROF` window per scenario
//! (`tools/n64/prof.py <log> <elf> <window>`).

use crate::data::{DfsStore, N64Data};
use crate::log;
use crate::rt::{self, ticks, ticks_to_us};
use alloc::string::String;
use alloc::sync::Arc;
use core::fmt::Write;
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};
use f2_game::simbench::{self, phase, SCENARIOS};

/// Ticks spent per tick phase (`Game::phase_hook`), and the time of the last mark.
static PHASES: [AtomicU32; phase::COUNT] = [const { AtomicU32::new(0) }; phase::COUNT];
static LAST: AtomicU32 = AtomicU32::new(0);

fn phase_hook(id: u32) {
    let t = ticks();
    PHASES[id as usize].fetch_add(t.wrapping_sub(LAST.swap(t, Relaxed)), Relaxed);
}

pub fn run_all() {
    let data = Arc::new(N64Data::new(DfsStore));
    #[cfg(feature = "profile")]
    unsafe {
        crate::ffi::n64_prof(0);
        crate::ffi::n64_prof(2);
    }
    log!("simbench: {} scenarios", SCENARIOS.len());
    probes(data.clone());
    for (i, s) in SCENARIOS.iter().enumerate() {
        let t0 = ticks();
        let mut g = simbench::setup(s, data.clone());
        g.phase_hook = Some(phase_hook);
        for p in &PHASES {
            p.store(0, Relaxed);
        }
        let load_ms = ticks_to_us(ticks().wrapping_sub(t0)) / 1000;
        let (mut total, mut worst, mut start) = (0u64, 0u32, 0u32);
        let out = simbench::run(s, &mut g, &mut |begin| {
            if begin {
                #[cfg(feature = "profile")]
                unsafe {
                    crate::ffi::n64_prof(3)
                };
                start = ticks();
                LAST.store(start, Relaxed);
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
        let mut line = String::new();
        for (k, p) in PHASES.iter().enumerate() {
            let us = (p.load(Relaxed) as u64 * 1_000_000 / rt::TICKS_PER_SEC as u64 / out.ticks.max(1) as u64) as u32;
            let _ = write!(line, " {} {}", phase::NAMES[k], us);
        }
        log!("simbench {} {} phases us/tick:{}", i, s.name, line);
        #[cfg(feature = "profile")]
        unsafe {
            crate::ffi::n64_prof(1);
            crate::ffi::n64_prof(2);
        }
    }
    log!("simbench: done");
}

/// Cost calibration on a loaded Garden 1 (C0 COUNT ticks -> CPU cycles = 2 x ticks).
fn probes(data: Arc<N64Data<DfsStore>>) {
    use core::hint::black_box;
    let mut g = simbench::setup(&SCENARIOS[1], data);
    for _ in 0..10 {
        g.tick(f2_game::TickInput::default());
    }
    let n_act = g.actor2.act_list.count.max(1) as u32;
    let cyc = |t: u32, n: u32| t * 2 / n;
    // walk links only
    let t0 = ticks();
    for _ in 0..10 {
        let mut cur = g.actor2.act_list.first;
        while let Some(id) = cur {
            cur = black_box(g.actor2.actors[id].links.next);
        }
    }
    let walk = cyc(ticks().wrapping_sub(t0), 10 * n_act);
    // walk + position
    let t0 = ticks();
    let mut acc = 0i32;
    for _ in 0..10 {
        let mut cur = g.actor2.act_list.first;
        while let Some(id) = cur {
            let a = &g.actor2.actors[id];
            acc = acc.wrapping_add(a.actor.position.vx as i32);
            cur = black_box(a.links.next);
        }
    }
    black_box(acc);
    let walk_pos = cyc(ticks().wrapping_sub(t0), 10 * n_act);
    // distance in cache
    let a = f2_math::SVector { vx: 100, vy: -2000, vz: 3000, pad: 0 };
    let b = f2_math::SVector { vx: -700, vy: 50, vz: 9, pad: 0 };
    let t0 = ticks();
    let mut d = 0;
    for _ in 0..1000 {
        d ^= f2_math::distance_between_points_ss(black_box(&a), black_box(&b));
    }
    black_box(d);
    let dist = cyc(ticks().wrapping_sub(t0), 1000);
    // f32 work in cache: quaternion_to_matrix-like
    let t0 = ticks();
    let mut x = black_box(0.5f32);
    for _ in 0..1000 {
        x = black_box(x * 1.0001 + 0.25 * x);
    }
    black_box(x);
    let fp = cyc(ticks().wrapping_sub(t0), 1000);
    // copy_actor_list
    let t0 = ticks();
    for _ in 0..10 {
        g.copy_actor_list();
    }
    let copy = cyc(ticks().wrapping_sub(t0), 10 * n_act);
    // read a 32 KiB buffer, 1 word per 16 bytes (cold misses), then again 1 word per 4 bytes
    let buf = alloc::vec![1u32; 65536];
    let t0 = ticks();
    let mut s = 0u32;
    for i in (0..buf.len()).step_by(4) {
        s = s.wrapping_add(buf[i]);
    }
    black_box(s);
    let line = cyc(ticks().wrapping_sub(t0), (buf.len() / 4) as u32);
    let t0 = ticks();
    let mut s = 0u32;
    for v in &buf[..2048] {
        s = s.wrapping_add(*v);
    }
    for v in &buf[..2048] {
        s = s.wrapping_add(*v);
    }
    black_box(s);
    let hot = cyc(ticks().wrapping_sub(t0), 4096);
    // sphere test, data hot: one actor's inputs, 1000 times
    let (mut sv, mut svn) = (0u32, 0u32);
    if let Some(view) = g.specfx.view {
        let ids = g.actor2.act_list.ids(&g.actor2.actors);
        for &id in ids.iter().take(8) {
            let a = &g.actor2.actors[id].actor.actual_actor;
            let Some(model) = a.object_controller.and_then(|m| g.models.get(m)) else { continue };
            let p = a.placement;
            let t0 = ticks();
            let mut n = 0u32;
            for _ in 0..1000 {
                n += f2_game::view::sphere_visible(black_box(model), black_box(&p), black_box(&view)) as u32;
            }
            sv += cyc(ticks().wrapping_sub(t0), 1000);
            svn += 1;
            black_box(n);
        }
    }
    let sphere = sv / svn.max(1);
    // quaternion_to_matrix alone
    let q = [0.1f32, 0.7, 0.2, 0.68];
    let t0 = ticks();
    let mut acc = 0.0f32;
    for _ in 0..1000 {
        acc += f2_game::view::quaternion_to_matrix(black_box(&q))[1][2];
    }
    black_box(acc);
    let q2m = cyc(ticks().wrapping_sub(t0), 1000);
    let t0 = ticks();
    let mut acc = 1.0f32;
    for i in 0..1000 {
        acc = black_box(acc / (i as f32 + 1.5));
    }
    black_box(acc);
    let fdivs = cyc(ticks().wrapping_sub(t0), 1000);
    log!("simbench probes (cycles): sphere_visible hot {} quaternion_to_matrix {} f32 div loop {}", sphere, q2m, fdivs);
    log!(
        "simbench probes (cycles): actors {} walk {}/actor walk+pos {}/actor copy_actor_list {}/actor distance_ss {} f32 mul-add {} cold line read {} hot word read {}",
        n_act, walk, walk_pos, copy, dist, fp, line, hot
    );
}
