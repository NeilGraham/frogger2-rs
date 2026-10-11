//! Performance probes (phase 1B), logged via debugf and recorded in
//! research/n64-performance.md. Ticks are C0 COUNT (46.875 MHz); CPU cycles = 2 x ticks.

use crate::ffi::{self, State, Vertex};
use crate::kernels::{self, TlVertex};
use crate::log;
use crate::rt::{self, ticks, ticks_to_us};
use alloc::vec::Vec;
use f2_math::Rng;

fn best_of<F: FnMut() -> u32>(runs: u32, mut f: F) -> u32 {
    (0..runs).map(|_| f()).min().unwrap_or(0)
}

/// TlVertex (scene.rs) -> rdpq vertex, as the phase-2 renderer will do.
pub fn to_n64(tris: &[[TlVertex; 3]], tex_w: f32, tex_h: f32, out: &mut Vec<Vertex>) {
    out.clear();
    for t in tris {
        for v in t {
            out.push(Vertex {
                x: v.sx,
                y: v.sy,
                z: v.sz,
                r: v.color[0],
                g: v.color[1],
                b: v.color[2],
                a: v.color[3],
                s: v.uv[0] * tex_w,
                t: v.uv[1] * tex_h,
                inv_w: 1.0,
            });
        }
    }
}

/// Probes (a) f32 transform and (c) fixed-point tick work.
pub fn cpu_probes() {
    // (a) scene.rs prepare_object + render_mesh on a 32x32 grid (1024 vertices, 1922 faces).
    let view = kernels::View::n64();
    let mesh = kernels::grid_mesh(32, 24.0);
    let (m, vm) = (kernels::camera_matrix(0.3), kernels::view_matrix());
    let mut tv = Vec::with_capacity(mesh.vertices.len());
    let mut tris = Vec::with_capacity(mesh.faces.len());
    let mut verts = Vec::with_capacity(mesh.faces.len() * 3);
    let t_prep = best_of(5, || {
        let t0 = ticks();
        kernels::prepare_object(&view, &mesh, &m, &vm, &mut tv);
        ticks() - t0
    });
    let t_emit = best_of(5, || {
        tris.clear();
        let t0 = ticks();
        kernels::render_mesh(&view, &mesh, &tv, &mut tris);
        ticks() - t0
    });
    let t_conv = best_of(5, || {
        let t0 = ticks();
        to_n64(&tris, 32.0, 32.0, &mut verts);
        ticks() - t0
    });
    let nv = mesh.vertices.len() as u32;
    let nt = tris.len().max(1) as u32;
    log!(
        "PROBE a transform: {} verts prepare {} us ({} cyc/vert); {} faces -> {} tris render_mesh {} us ({} cyc/face, {} cyc/tri out); to_rdpq {} us ({} cyc/tri); checksum {:08x}",
        nv,
        ticks_to_us(t_prep),
        t_prep * 2 / nv,
        mesh.faces.len(),
        nt,
        ticks_to_us(t_emit),
        t_emit * 2 / mesh.faces.len() as u32,
        t_emit * 2 / nt,
        ticks_to_us(t_conv),
        t_conv * 2 / nt,
        kernels::tl_checksum(&tris)
    );

    // Raw f32 matrix-vector throughput (mtx_xfm only, 4096 vertices).
    let pts: Vec<[f32; 3]> = (0..4096).map(|i| [i as f32, (i * 3 % 101) as f32, (i % 37) as f32]).collect();
    let f = kernels::mtx_cat(&m, &vm);
    let mut acc = 0.0f32;
    let t_x = best_of(3, || {
        let t0 = ticks();
        for p in &pts {
            let r = kernels::mtx_xfm(&f, *p);
            acc += r[0] + r[1] + r[2];
        }
        ticks() - t0
    });
    log!("PROBE a mtx_xfm: 4096 verts {} us ({} cyc/vert) [acc {}]", ticks_to_us(t_x), t_x * 2 / 4096, acc as i32);

    // (c) fixed-point "tick": 64 and 256 actors.
    for n in [64usize, 256] {
        let mut actors = kernels::make_actors(n);
        let mut rng = Rng::default();
        let mut sum = 0u32;
        let t = best_of(5, || {
            let t0 = ticks();
            sum = sum.wrapping_add(kernels::fixed_tick(&mut actors, &mut rng));
            ticks() - t0
        });
        log!(
            "PROBE c fixed tick: {} actors {} us/tick ({} cyc/actor), {}% of a 60 Hz frame [sum {:08x}]",
            n,
            ticks_to_us(t),
            t * 2 / n as u32,
            ticks_to_us(t) / 167,
            sum
        );
    }
    let t_self = best_of(3, || {
        let t0 = ticks();
        core::hint::black_box(kernels::f2_math_selftest());
        ticks() - t0
    });
    log!("PROBE c f2-math selftest (2000 iters fmul/fdiv/rsin/fsqrt/Random): {} us", ticks_to_us(t_self));

    // Individual f2-math operations (cycles per call, 1000 calls each).
    {
        use core::hint::black_box as bb;
        use f2_math::{fdiv, fmul, fsqrt, iquat_slerp, make_unit, rsin, FVector, IQuaternion};
        let n = 1000u32;
        let mut r = Rng::default();
        let cyc = |t: u32| t * 2 / n;
        let t_fmul = best_of(3, || {
            let t0 = ticks();
            for i in 0..n as i32 {
                bb(fmul(bb(i * 4099), bb(77 << 12)));
            }
            ticks() - t0
        });
        let t_fdiv = best_of(3, || {
            let t0 = ticks();
            for i in 1..=n as i32 {
                bb(fdiv(bb(i << 12), bb(3 << 12)));
            }
            ticks() - t0
        });
        let t_fsqrt = best_of(3, || {
            let t0 = ticks();
            for i in 0..n as i32 {
                bb(fsqrt(bb(i << 12)));
            }
            ticks() - t0
        });
        let t_rsin = best_of(3, || {
            let t0 = ticks();
            for i in 0..n as i32 {
                bb(rsin(bb(i * 5)));
            }
            ticks() - t0
        });
        let t_unit = best_of(3, || {
            let t0 = ticks();
            for i in 0..n as i32 {
                let mut v = FVector { vx: i << 12, vy: 300 << 12, vz: -(i << 11) };
                make_unit(bb(&mut v));
            }
            ticks() - t0
        });
        let t_slerp = best_of(3, || {
            let a = IQuaternion { x: 0, y: 0, z: 0, w: 4096 };
            let b = IQuaternion { x: 0, y: 2896, z: 0, w: 2896 };
            let t0 = ticks();
            for i in 0..n as i32 {
                let mut q = IQuaternion::default();
                iquat_slerp(bb(&a), bb(&b), bb(i & 4095), &mut q);
            }
            ticks() - t0
        });
        let t_rand = best_of(3, || {
            let t0 = ticks();
            for _ in 0..n {
                bb(r.random(bb(1000)));
            }
            ticks() - t0
        });
        log!(
            "PROBE c f2-math cyc/call: fmul {} fdiv {} fsqrt {} rsin {} make_unit {} iquat_slerp {} Random {}",
            cyc(t_fmul),
            cyc(t_fdiv),
            cyc(t_fsqrt),
            cyc(t_rsin),
            cyc(t_unit),
            cyc(t_slerp),
            cyc(t_rand)
        );
    }

    // Host-comparable checksum of the fixed tick (64 actors, 10 ticks, like the host test).
    let mut actors = kernels::make_actors(64);
    let mut rng = Rng::default();
    let mut s = 0u32;
    for _ in 0..10 {
        s = s.wrapping_add(kernels::fixed_tick(&mut actors, &mut rng));
    }
    log!("PROBE c fixed tick checksum {:08x}", s);
}

/// Probe (d): DFS read + decompression of the same 1 MiB file at mkasset levels 0..3.
pub fn dfs_probes() {
    for lvl in 0..4 {
        let path = alloc::format!("rom:/probe_c{lvl}.bin");
        let t0 = ticks();
        let d = rt::file_read(&path);
        let t = ticks() - t0;
        match d {
            Some(d) => {
                let us = ticks_to_us(t).max(1);
                let sum = d.iter().fold(0u32, |a, &b| a.wrapping_mul(33).wrapping_add(b as u32));
                log!(
                    "PROBE d dfs level {}: {} bytes in {} us = {} KiB/s (decompressed output) [sum {:08x}]",
                    lvl,
                    d.len(),
                    us,
                    (d.len() as u64 * 1_000_000 / 1024 / us as u64),
                    sum
                );
            }
            None => log!("PROBE d dfs level {}: {} missing", lvl, path),
        }
    }
}

/// A batch of `n` triangles of roughly `size` px legs, spread over the screen.
fn tri_batch(n: usize, size: f32, textured: bool) -> Vec<Vertex> {
    let mut v = Vec::with_capacity(n * 3);
    let mut rng = Rng::default();
    for i in 0..n {
        let x = rng.random((320.0 - size) as u32) as f32;
        let y = rng.random((240.0 - size) as u32) as f32;
        let z = (i % 100) as f32 / 100.0;
        let c = (i % 3) as f32 / 2.0;
        let mk = |dx: f32, dy: f32, s: f32, t: f32| Vertex {
            x: x + dx,
            y: y + dy,
            z,
            r: c,
            g: 1.0 - c,
            b: 0.5,
            a: 1.0,
            s: if textured { s } else { 0.0 },
            t: if textured { t } else { 0.0 },
            inv_w: 1.0,
        };
        v.push(mk(0.0, 0.0, 0.0, 0.0));
        v.push(mk(size, size * 0.2, 32.0, 0.0));
        v.push(mk(size * 0.3, size, 0.0, 32.0));
    }
    v
}

pub struct RdpProbe {
    step: u32,
    tex: u32,
}

impl RdpProbe {
    pub fn new(tex: u32) -> Self {
        RdpProbe { step: 0, tex }
    }

    /// Runs one probe per call (each owns a whole frame); returns false when done.
    pub fn frame(&mut self) -> bool {
        let zs = State { flags: ffi::ZTEST | ffi::ZWRITE, blend: ffi::BLEND_NONE, tex: 0, alpha_ref: 0 };
        let zt = State { flags: ffi::ZTEST | ffi::ZWRITE | ffi::BILINEAR | ffi::WRAP, tex: self.tex, ..zs };
        let cases: [(&str, usize, f32, State); 6] = [
            ("shade+z 8px", 2000, 8.0, zs),
            ("shade+z 24px", 1000, 24.0, zs),
            ("tex+shade+z 8px", 2000, 8.0, zt),
            ("tex+shade+z 24px", 1000, 24.0, zt),
            ("tex+shade+z 64px", 300, 64.0, zt),
            ("tex+shade+z 160px", 50, 160.0, zt),
        ];
        let i = self.step as usize;
        self.step += 1;
        if i < cases.len() {
            let (name, n, size, st) = cases[i];
            let batch = tri_batch(n, size, st.tex != 0);
            rt::begin_frame(0x000000ff);
            rt::rdp_wait();
            let t0 = ticks();
            rt::draw_tris(&batch, &st);
            let t1 = ticks();
            rt::rdp_wait();
            let t2 = ticks();
            // Same batch recorded into an rspq block (CPU-only cost), then replayed
            // (RSP triangle setup + RDP only).
            rt::probe_c(1, 0);
            let t3 = ticks();
            rt::draw_tris(&batch, &st);
            let t4 = ticks();
            rt::probe_c(2, 0);
            let rsp_rdp = rt::probe_c(3, 0);
            rt::present();
            let (cpu, all) = (ticks_to_us(t1 - t0).max(1), ticks_to_us(t2 - t0).max(1));
            let (rec, rr) = (ticks_to_us(t4 - t3).max(1), ticks_to_us(rsp_rdp).max(1));
            log!(
                "PROBE b rdpq {}: {} tris (~{} px each) live: submit {} us, RDP idle at {} us | CPU-only {} us ({} cyc/tri) | RSP+RDP-only {} us ({} RCP cyc/tri) -> CPU-bound {} tris/frame @60, RCP-bound {} @60 / {} @30",
                name,
                n,
                (size * size * 0.43) as u32,
                cpu,
                all,
                rec,
                (t4 - t3) * 2 / n as u32,
                rr,
                rsp_rdp as u64 * 4 / 3 / n as u64,
                n as u64 * 16_667 / rec as u64,
                n as u64 * 16_667 / rr as u64,
                n as u64 * 33_333 / rr as u64
            );
            return true;
        }
        if i == cases.len() {
            // C-side rdpq_triangle loops (no FFI, no per-call state check), recorded into a
            // block so only the CPU cost is measured.
            rt::begin_frame(0x000000ff);
            rt::rdp_wait();
            for kind in [0u32, 4] {
                rt::probe_c(1, 0);
                let t = rt::probe_c(kind, 2000);
                rt::probe_c(2, 0);
                let rr = rt::probe_c(3, 0);
                log!(
                    "PROBE b rdpq C loop {}: 2000 tris CPU-only {} us ({} cyc/tri), RSP+RDP {} us",
                    if kind == 0 { "shade+z" } else { "tex+shade+z" },
                    ticks_to_us(t),
                    t * 2 / 2000,
                    ticks_to_us(rr)
                );
            }
            rt::present();
            return true;
        }
        if i == cases.len() + 1 || i == cases.len() + 2 {
            // Full pipeline on the probe-(a) grid (run twice: cold, then warm caches/heap):
            // transform + emit + convert + submit, then RSP/RDP drain.
            let view = kernels::View::n64();
            let mesh = kernels::grid_mesh(32, 24.0);
            let (m, vm) = (kernels::camera_matrix(0.3), kernels::view_matrix());
            let mut tv = Vec::with_capacity(mesh.vertices.len());
            let mut tris = Vec::with_capacity(mesh.faces.len());
            let mut verts = Vec::with_capacity(mesh.faces.len() * 3);
            rt::begin_frame(0x204060ff);
            rt::rdp_wait();
            let t0 = ticks();
            kernels::prepare_object(&view, &mesh, &m, &vm, &mut tv);
            let t1 = ticks();
            kernels::render_mesh(&view, &mesh, &tv, &mut tris);
            let t2 = ticks();
            to_n64(&tris, 32.0, 32.0, &mut verts);
            let t3 = ticks();
            rt::draw_tris(&verts, &zt);
            let t4 = ticks();
            rt::rdp_wait();
            let t5 = ticks();
            rt::present();
            log!(
                "PROBE b full pipeline ({}): {} verts -> {} tris: prepare {} us, render_mesh {} us, to_rdpq {} us, submit {} us; CPU total {} us, RDP idle +{} us => {} cyc/tri all-in, {} fps-equivalent",
                if i == cases.len() + 1 { "cold" } else { "warm" },
                mesh.vertices.len(),
                tris.len(),
                ticks_to_us(t1 - t0),
                ticks_to_us(t2 - t1),
                ticks_to_us(t3 - t2),
                ticks_to_us(t4 - t3),
                ticks_to_us(t4 - t0),
                ticks_to_us(t5 - t4),
                (t4 - t0) as u64 * 2 / tris.len().max(1) as u64,
                1_000_000 / ticks_to_us(t5 - t0).max(1)
            );
            return true;
        }
        false
    }
}
