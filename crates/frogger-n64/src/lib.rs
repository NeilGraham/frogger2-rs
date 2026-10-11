//! Frogger 2 — Nintendo 64 front end (phase 1B bring-up).
//!
//! Built as a `staticlib` for the custom MIPS III n32 target in `tools/n64/` and linked into
//! a libdragon ROM whose C `main()` (csrc/main.c) calls [`f2n64_init`] once and
//! [`f2n64_frame`] every frame. Rust calls back the narrow `n64_*` C interface (`ffi`).
//!
//! Current content: the RSP 3D renderer (`r3d`, phase 2R) and its test app (`r3dtest`:
//! cull check, benchmarks, PC-comparison scenes). With the `phase1` feature the ROM instead
//! runs the phase-1B bring-up demo (rdpq triangles, joypad, mixer beep, DFS) and the
//! CPU/RDP/DFS probes. Measurements: research/n64-performance.md.
#![cfg_attr(target_vendor = "nintendo64", no_std)]

extern crate alloc;

pub mod ffi;
pub mod kernels;
pub mod r3d;

#[cfg(target_vendor = "nintendo64")]
pub mod rt;

#[cfg(all(target_vendor = "nintendo64", feature = "phase1"))]
mod demo;
#[cfg(all(target_vendor = "nintendo64", feature = "phase1"))]
mod probes;
#[cfg(all(target_vendor = "nintendo64", not(feature = "phase1")))]
mod r3dtest;

#[cfg(all(target_vendor = "nintendo64", feature = "phase1"))]
static mut APP: Option<demo::Demo> = None;
#[cfg(all(target_vendor = "nintendo64", not(feature = "phase1")))]
static mut APP: Option<r3dtest::R3dTest> = None;

/// Called once by csrc/main.c (through `f2n64_call`) after libdragon is initialised.
#[cfg(target_vendor = "nintendo64")]
#[no_mangle]
pub extern "C" fn f2n64_init() {
    log!("f2n64_init: Rust core up (n32 on VR4300), RDRAM {} KiB", rt::memory_size() / 1024);
    let (a, b, c, v, r, sum) = kernels::f2_math_selftest();
    log!("f2-math: fmul(3,5)={} fdiv(10,4)={} fsqrt(2)={} unit=({},{},{}) Random(1000)={} checksum={:08x}", a, b, c, v.vx, v.vy, v.vz, r, sum);
    match rt::file_read("rom:/hello.txt") {
        Some(d) => log!("DFS: rom:/hello.txt {} bytes: {:?}", d.len(), core::str::from_utf8(&d).unwrap_or("<bin>").trim_end()),
        None => log!("DFS: rom:/hello.txt missing"),
    }
    #[cfg(feature = "phase1")]
    {
        probes::cpu_probes();
        probes::dfs_probes();
        // SAFETY: single-threaded; f2n64_init/f2n64_frame are only called from main().
        unsafe { APP = Some(demo::Demo::new()) };
    }
    #[cfg(not(feature = "phase1"))]
    // SAFETY: as above.
    unsafe {
        APP = Some(r3dtest::R3dTest::new())
    };
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
