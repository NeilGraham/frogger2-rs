//! Frogger 2 — Nintendo 64 front end (phase 1B bring-up).
//!
//! Built as a `staticlib` for the custom MIPS III n32 target in `tools/n64/` and linked into
//! a libdragon ROM whose C `main()` (csrc/main.c) calls [`f2n64_init`] once and
//! [`f2n64_frame`] every frame. Rust calls back the narrow `n64_*` C interface (`ffi`).
//!
//! Current content: platform bring-up demo (textured + shaded + z-buffered triangles via
//! rdpq, joypad, mixer beep, DFS file read, f2-math self-test) and the CPU/RDP/DFS
//! performance probes recorded in research/n64-performance.md.
#![cfg_attr(target_vendor = "nintendo64", no_std)]

extern crate alloc;

pub mod ffi;
pub mod kernels;

#[cfg(target_vendor = "nintendo64")]
pub mod rt;

#[cfg(target_vendor = "nintendo64")]
mod demo;
#[cfg(target_vendor = "nintendo64")]
mod f2z_probe;
#[cfg(target_vendor = "nintendo64")]
mod probes;

#[cfg(target_vendor = "nintendo64")]
static mut APP: Option<demo::Demo> = None;

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
    probes::cpu_probes();
    probes::dfs_probes();
    f2z_probe::f2z_probes();
    // SAFETY: single-threaded; f2n64_init/f2n64_frame are only called from main().
    unsafe { APP = Some(demo::Demo::new()) };
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
