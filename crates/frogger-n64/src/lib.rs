//! Frogger 2 — Nintendo 64 front end.
//!
//! Built as a `staticlib` for the custom MIPS III n32 target in `tools/n64/` and linked into
//! a libdragon ROM whose C `main()` (csrc/main.c) calls [`f2n64_init`] once and
//! [`f2n64_frame`] every frame. Rust calls back the narrow `n64_*` C interface (`ffi`).
//!
//! Phase 2G: the game (`app`): `f2-game` at a fixed 60 Hz, `f2-draw`'s draw loop rendered by
//! `render_cpu`, audio on the libdragon mixer (`audio`), data from the DFS (`data`), pad
//! (`input`). With the `bringup` feature the ROM instead runs the phase-1B demo and
//! performance probes (research/n64-performance.md); with `r3d-test` the phase-2R RSP
//! renderer test app (`r3dtest`: cull check, benchmarks, PC-comparison scenes).
#![cfg_attr(target_vendor = "nintendo64", no_std)]

extern crate alloc;

pub mod audio;
pub mod data;
pub mod ffi;
pub mod input;
pub mod kernels;
pub mod ntx;
pub mod save;
pub mod r3d;
pub mod texconv;

#[cfg(target_vendor = "nintendo64")]
pub mod rt;

// Used by the N64-only front end (`app`); host builds only run the portable modules' tests.
#[cfg(not(target_vendor = "nintendo64"))]
use f2_draw as _;

#[cfg(target_vendor = "nintendo64")]
pub mod app;
#[cfg(target_vendor = "nintendo64")]
pub mod ntxload;
#[cfg(target_vendor = "nintendo64")]
pub mod render_cpu;
#[cfg(all(target_vendor = "nintendo64", not(feature = "render-cpu")))]
pub mod render_r3d;

#[cfg(all(target_vendor = "nintendo64", feature = "bringup"))]
mod demo;
#[cfg(all(target_vendor = "nintendo64", feature = "bringup"))]
mod f2z_probe;
#[cfg(all(target_vendor = "nintendo64", feature = "bringup"))]
mod probes;

#[cfg(all(target_vendor = "nintendo64", feature = "r3d-test", not(feature = "bringup")))]
mod r3dtest;

#[cfg(all(target_vendor = "nintendo64", feature = "bringup"))]
static mut APP: Option<demo::Demo> = None;
#[cfg(all(target_vendor = "nintendo64", feature = "r3d-test", not(feature = "bringup")))]
static mut APP: Option<r3dtest::R3dTest> = None;
#[cfg(all(target_vendor = "nintendo64", not(any(feature = "bringup", feature = "r3d-test"))))]
static mut APP: Option<app::App> = None;

/// Called once by csrc/main.c (through `f2n64_call`) after libdragon is initialised.
#[cfg(target_vendor = "nintendo64")]
#[no_mangle]
pub extern "C" fn f2n64_init() {
    log!("f2n64_init: Rust core up (n32 on VR4300), RDRAM {} KiB", rt::memory_size() / 1024);
    let (a, b, c, v, r, sum) = kernels::f2_math_selftest();
    log!("f2-math: fmul(3,5)={} fdiv(10,4)={} fsqrt(2)={} unit=({},{},{}) Random(1000)={} checksum={:08x}", a, b, c, v.vx, v.vy, v.vz, r, sum);
    #[cfg(feature = "bringup")]
    {
        match rt::file_read("rom:/hello.txt") {
            Some(d) => log!("DFS: rom:/hello.txt {} bytes: {:?}", d.len(), core::str::from_utf8(&d).unwrap_or("<bin>").trim_end()),
            None => log!("DFS: rom:/hello.txt missing"),
        }
        probes::cpu_probes();
        probes::dfs_probes();
        f2z_probe::f2z_probes();
        // SAFETY: single-threaded; f2n64_init/f2n64_frame are only called from main().
        unsafe { APP = Some(demo::Demo::new()) };
    }
    #[cfg(all(feature = "r3d-test", not(feature = "bringup")))]
    // SAFETY: single-threaded; f2n64_init/f2n64_frame are only called from main().
    unsafe {
        APP = Some(r3dtest::R3dTest::new())
    };
    #[cfg(not(any(feature = "bringup", feature = "r3d-test")))]
    {
        let boot = app::Boot::parse(option_env!("F2N64_BOOT"));
        log!("f2n64_init: boot {:?}", boot);
        // SAFETY: single-threaded; f2n64_init/f2n64_frame are only called from main().
        unsafe { APP = Some(app::App::new(boot)) };
    }
}

/// Called every frame by csrc/main.c.
#[cfg(target_vendor = "nintendo64")]
#[no_mangle]
pub extern "C" fn f2n64_frame() {
    // SAFETY: see f2n64_init.
    #[allow(static_mut_refs)]
    if let Some(app) = unsafe { APP.as_mut() } {
        app.frame();
    }
}
