//! Phase 2R test app: RSP renderer (r3d) cull check, benchmarks and PC-comparison scenes.
//!
//! At boot: (1) the cull check (an MDX front face and back face, read back from the
//! framebuffer); (2) synthetic benchmark scenes (500..3000 textured z-buffered triangles in
//! 10..40 meshes): CPU submission time, RSP+RDP time (frame recorded into an rspq block,
//! then replayed until idle) and the achieved frame rate; (3) DFS scenes exported by
//! tools/n64/export (`rom:/scene*.f2nm` + `.f2ns`), shown in turn for screenshots.
//! Everything is logged with `R3D` prefixes (research/n64-performance.md).

use crate::log;
use crate::r3d::*;
use crate::rt::{self, ticks, ticks_to_us};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use f2_n64fmt::encode::{encode_mesh, encode_texture, EncodedMesh, FaceIn, FileBuilder, MeshIn, TexIn};
use f2_n64fmt::File;

const TEX_A: u32 = 0x00B0_0001;
const TEX_B: u32 = 0x00B0_0002;
/// Seconds each DFS scene is shown.
const SCENE_SECONDS: u32 = 6;

/// 32x32 RGBA8 checkerboard.
fn checker(c0: [u8; 4], c1: [u8; 4]) -> Vec<u8> {
    (0..32 * 32).flat_map(|i| if ((i % 32) / 8 + (i / 32) / 8) % 2 == 0 { c0 } else { c1 }).collect()
}

/// A `g` x `g`-quad grid (2g² triangles) of `size` units on the XZ plane (gentle y waves),
/// Gouraud corner colours, one texture period per mesh. `flip` reverses the winding.
fn grid(id: u32, g: usize, size: f32, tex: u32, flip: bool) -> EncodedMesh {
    let mut pos = Vec::new();
    for y in 0..=g {
        for x in 0..=g {
            let (fx, fy) = (x as f32 / g as f32, y as f32 / g as f32);
            let wave = libm::sinf(fx * 6.0) * libm::cosf(fy * 5.0) * size * 0.06;
            pos.push([(fx - 0.5) * size, wave, (fy - 0.5) * size]);
        }
    }
    let idx = |x: usize, y: usize| (y * (g + 1) + x) as u16;
    let uv = |x: usize, y: usize| [(x * 1026 / g) as i16, (y * 1026 / g) as i16];
    let col = |x: usize, y: usize| [255 - (x * 90 / g) as u8, 255, 255 - (y * 90 / g) as u8, 255];
    let mut faces = Vec::new();
    for y in 0..g {
        for x in 0..g {
            for t in [[(x, y), (x + 1, y), (x + 1, y + 1)], [(x, y), (x + 1, y + 1), (x, y + 1)]] {
                let t = if flip { [t[0], t[2], t[1]] } else { t };
                faces.push(FaceIn { v: t.map(|(a, b)| idx(a, b)), uv: t.map(|(a, b)| uv(a, b)), rgba: t.map(|(a, b)| col(a, b)), tex_crc: tex });
            }
        }
    }
    encode_mesh(&MeshIn { id, positions: &pos, faces: &faces, bone_of: None, ordered: false, wrap: false })
}

fn translate(x: f32, y: f32, z: f32) -> Mat4 {
    [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [x, y, z, 1.0]]
}

fn rot_y(a: f32, m: Mat4) -> Mat4 {
    let (s, c) = (libm::sinf(a), libm::cosf(a));
    mtx_cat(&[[c, 0.0, -s, 0.0], [0.0, 1.0, 0.0, 0.0], [s, 0.0, c, 0.0], [0.0, 0.0, 0.0, 1.0]], &m)
}

/// MDX-visible triangles of a draw, counted on the CPU with the shader model.
fn visible_tris(f: &File, mesh: usize, model: &Mat4, view: &ViewParams) -> u32 {
    let m = f.mesh(mesh).unwrap();
    let u = draw_uniform(&model_scaled(model, m.entry.pos_scale()), &DrawParams::default());
    let fr = frame_uniform_vp(view, false);
    let mut n = 0;
    for g in m.groups() {
        let mut slots = [None; 49];
        for c in m.group_cmds(&g) {
            match c {
                f2_n64fmt::Cmd::Load { first, count, slot } => {
                    for k in 0..count as usize {
                        slots[slot as usize + k] = Some(emulate_vertex(&u, &fr, view.guard as i32, m.vertex(first as usize + k).pos));
                    }
                }
                f2_n64fmt::Cmd::Tri(t) => {
                    let v = t.map(|s| slots[s as usize].unwrap());
                    if v.iter().any(|x| x.2 & 0x24 != 0) || v.iter().fold(0x3F, |a, x| a & !x.3) & 0x3F != 0 {
                        continue;
                    }
                    let (a, b, c) = (v[0].0, v[1].0, v[2].0);
                    let cross = (b[0] - a[0]) as i64 * (c[1] - a[1]) as i64 - (b[1] - a[1]) as i64 * (c[0] - a[0]) as i64;
                    if cross < 0 {
                        n += 1;
                    }
                }
                _ => {}
            }
        }
    }
    n
}

struct Bench {
    name: String,
    draws: Vec<(MeshId, Mat4)>,
    visible: u32,
}

struct Scene {
    name: String,
    view: ViewParams,
    clear: u32,
    /// Mesh, frame list, per-object fog, matrices (one per bone for skinned meshes).
    draws: Vec<(MeshId, List, bool, Vec<Mat4>)>,
    tris: u32,
}

impl Scene {
    fn queue(&self, r: &mut R3d) {
        for (m, l, object_fog, mats) in &self.draws {
            let p = DrawParams { object_fog: *object_fog, ..Default::default() };
            if r.mesh_skinned(*m) {
                r.draw_skinned(*l, *m, mats, &p);
            } else {
                r.draw(*l, *m, &mats[0], &p);
            }
        }
    }
}

pub struct R3dTest {
    r: R3d,
    bench_view: ViewParams,
    scenes: Vec<Scene>,
    frame: u32,
    last: u32,
    shown: usize,
    /// `rom:/r3d_show.txt` (a scene index): show only that scene (screenshots).
    fixed: Option<usize>,
}

fn be_f32(d: &[u8], o: usize) -> f32 {
    f32::from_bits(u32::from_be_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]))
}
fn be_u32(d: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

impl R3dTest {
    pub fn new() -> R3dTest {
        let mut r = R3d::new();
        let mut scenes = Vec::new();
        // ---- synthetic assets
        let mut view = ViewParams::pc_default(320.0, 240.0);
        view.view = camera_view([0.0, 330.0, -330.0], [0.0, 0.0, 380.0], [0.0, 1.0, 0.0]);
        view.far = 1600.0;
        let mut fb = FileBuilder::new();
        fb.textures.push(encode_texture(&TexIn { crc: TEX_A, width: 32, height: 32, rgba: &checker([40, 200, 60, 255], [250, 230, 80, 255]) }));
        fb.textures.push(encode_texture(&TexIn { crc: TEX_B, width: 32, height: 32, rgba: &checker([200, 80, 60, 255], [90, 140, 250, 255]) }));
        // Mesh winding: make MDX-front faces from the bench camera (checked with the model).
        let probe = grid(0, 1, 90.0, TEX_A, false);
        let front_flip = {
            let mut b = FileBuilder::new();
            b.meshes.push(probe);
            let bytes = b.finish();
            let f = File::parse(&bytes).unwrap();
            visible_tris(&f, 0, &translate(0.0, 0.0, 380.0), &view) == 0
        };
        let sizes = [(5usize, false), (5, true), (7, false), (10, false)];
        for (i, &(g, back)) in sizes.iter().enumerate() {
            for tex in [TEX_A, TEX_B] {
                fb.meshes.push(grid(100 + i as u32, g, 90.0, tex, front_flip ^ back));
            }
        }
        let bytes = fb.finish();
        let bench_file = bytes.clone();
        let ids = r.load_file(bytes).expect("bench assets");
        let f = File::parse(&bench_file).unwrap();
        // ids: [g5 A, g5 B, g5back A, g5back B, g7 A, g7 B, g10 A, g10 B]
        log!("R3D: bench meshes: {} (front winding flipped: {})", ids.len(), front_flip);

        // ---- cull check: front face left, back face right; read the framebuffer back.
        let cull_draws = [(ids[0], translate(-110.0, 0.0, 380.0)), (ids[2], translate(110.0, 0.0, 380.0))];
        rt::begin_frame(0x000000ff);
        r.begin_frame(&view);
        for (m, mat) in &cull_draws {
            r.draw(List::Normal, *m, mat, &DrawParams::default());
        }
        r.flush(|_| {});
        rt::present();
        let probe_px = |x: f32, z: f32| {
            let e = emulate_vertex(
                &draw_uniform(&model_scaled(&translate(x, 0.0, z), 1.0), &DrawParams::default()),
                &frame_uniform_vp(&view, false),
                6,
                [0, 0, 0],
            );
            let (px, py) = ((e.0[0] / 4) as u32, (e.0[1] / 4) as u32);
            rt::probe_c(5, px << 16 | py)
        };
        let (front, back) = (probe_px(-110.0, 380.0), probe_px(110.0, 380.0));
        log!(
            "R3D CULL: front face pixel {:04x} ({}), back face pixel {:04x} ({}) with CULL_MODE {} -> {}",
            front,
            if front & 0xFFFE != 0 { "drawn" } else { "missing" },
            back,
            if back & 0xFFFE != 0 { "drawn" } else { "missing" },
            CULL_MODE,
            if front & 0xFFFE != 0 && back & 0xFFFE == 0 { "PASS" } else { "FAIL" }
        );

        // ---- CPU micro-benchmarks of the per-draw work (1000 iterations each)
        {
            let model = rot_y(0.3, translate(10.0, 20.0, 300.0));
            let mut acc = 0f32;
            let t0 = ticks();
            for i in 0..1000 {
                let mut m = model;
                m[3][0] += i as f32 * 0.001;
                acc += clip_matrix(&m, &view, 0.125, false)[3][3];
            }
            let t1 = ticks();
            let cm = clip_matrix(&model, &view, 0.125, false);
            let mut acc2 = 0i32;
            for i in 0..1000 {
                let mut m = cm;
                m[3][0] += i as f32 * 0.001;
                acc2 += draw_uniform(&m, &DrawParams::default()).mtx_i[12] as i32;
            }
            let t2 = ticks();
            r.begin_frame(&view);
            let t3 = ticks();
            for i in 0..200 {
                let mut m = model;
                m[3][0] += i as f32 * 0.01;
                r.draw(List::Normal, ids[0], &m, &DrawParams::default());
            }
            let t4 = ticks();
            r.discard();
            log!(
                "R3D CPU: clip_matrix {} cycles, draw_uniform {} cycles, R3d::draw {} cycles ({} {})",
                (t1 - t0) as u64 * 2 / 1000,
                (t2 - t1) as u64 * 2 / 1000,
                (t4 - t3) as u64 * 2 / 200,
                acc,
                acc2
            );
        }

        // ---- benchmarks
        let place = |n: usize, mesh: &dyn Fn(usize) -> MeshId| -> Vec<(MeshId, Mat4)> {
            let cols = 8;
            (0..n)
                .map(|i| {
                    let (cx, rz) = ((i % cols) as f32, (i / cols) as f32);
                    let jitter = (i % 3) as f32 * 30.0;
                    (mesh(i), rot_y(0.2 * i as f32, translate((cx - 3.5) * 95.0 + jitter * 0.3, (i % 4) as f32 * 12.0, 120.0 + rz * 120.0 + jitter)))
                })
                .collect()
        };
        let g5 = |i: usize| ids[i % 2];
        let g7 = |i: usize| ids[4 + i % 2];
        let g10 = |i: usize| ids[6 + i % 2];
        let mut benches = vec![
            Bench { name: "500 tris / 10 meshes".into(), draws: place(10, &g5), visible: 0 },
            Bench { name: "1000 tris / 20 meshes".into(), draws: place(20, &g5), visible: 0 },
            Bench { name: "1000 tris / 20 meshes, 1 texture".into(), draws: place(20, &|_| ids[0]), visible: 0 },
            Bench { name: "1500 tris / 30 meshes".into(), draws: place(30, &g5), visible: 0 },
            Bench { name: "2000 tris / 40 meshes".into(), draws: place(40, &g5), visible: 0 },
            Bench { name: "2000 tris / 10 meshes".into(), draws: place(10, &g10), visible: 0 },
            Bench { name: "2940 tris / 30 meshes".into(), draws: place(30, &g7), visible: 0 },
        ];
        for b in &mut benches {
            b.visible = b.draws.iter().map(|(m, mat)| visible_tris(&f, m.0 as usize, mat, &view)).sum();
        }
        for b in &benches {
            let draws = &b.draws;
            run_bench(&mut r, &view, &b.name, b.visible, &|r: &mut R3d| {
                for (m, mat) in draws {
                    r.draw(List::Normal, *m, mat, &DrawParams::default());
                }
            });
        }

        // ---- DFS scenes
        for name in ["scene", "scene2", "scene3", "scene4"] {
            if let Some(s) = load_scene(&mut r, name) {
                log!("R3D SCENE {}: {} draws, {} tris queued, far {}", s.name, s.draws.len(), s.tris, s.view.far);
                scenes.push(s);
            }
        }
        for s in &scenes {
            let name = format!("scene {}", s.name);
            let view = s.view;
            run_bench(&mut r, &view, &name, 0, &|r: &mut R3d| s.queue(r));
        }
        log!("R3D BENCH DONE");
        let fixed = rt::file_read("rom:/r3d_show.txt").and_then(|d| core::str::from_utf8(&d).ok().and_then(|t| t.trim().parse().ok()));
        R3dTest { r, bench_view: view, scenes, frame: 0, last: ticks(), shown: usize::MAX, fixed }
    }

    pub fn frame(&mut self) {
        let now = ticks();
        let dt = now.wrapping_sub(self.last);
        self.last = now;
        self.frame += 1;
        let t0;
        if self.scenes.is_empty() {
            // No exported scene: animate the 1000-triangle benchmark scene.
            rt::begin_frame(0x183050ff);
            t0 = ticks();
            let view = self.bench_view;
            self.r.begin_frame(&view);
            for i in 0..20 {
                let m = MeshId((i % 2) as u32);
                let mat = rot_y(0.02 * self.frame as f32 + i as f32, translate(((i % 8) as f32 - 3.5) * 95.0, 0.0, 120.0 + (i / 8) as f32 * 120.0));
                self.r.draw(List::Normal, m, &mat, &DrawParams::default());
            }
        } else {
            let idx = self.fixed.unwrap_or((self.frame / (60 * SCENE_SECONDS)) as usize) % self.scenes.len();
            let s = &self.scenes[idx];
            if idx != self.shown {
                log!("R3D SHOW scene {} (frame {})", s.name, self.frame);
                self.shown = idx;
            }
            rt::begin_frame(s.clear);
            t0 = ticks();
            self.r.begin_frame(&s.view);
            s.queue(&mut self.r);
        }
        self.r.flush(|_| {});
        let cpu = ticks().wrapping_sub(t0);
        rt::present();
        if self.frame.is_multiple_of(120) {
            let st = self.r.stats();
            log!(
                "R3D frame {}: {} us/frame ({} fps), CPU submit {} us, {} tris queued, {} sent, {} draws, {} batches, {} texture loads",
                self.frame,
                ticks_to_us(dt),
                1_000_000 / ticks_to_us(dt).max(1),
                ticks_to_us(cpu),
                self.r.queued_tris,
                self.r.sent_tris,
                st.draws,
                st.groups,
                st.tex_loads
            );
        }
    }
}

fn load_scene(r: &mut R3d, name: &str) -> Option<Scene> {
    let model = rt::file_read(&format!("rom:/{name}.f2nm"))?;
    let sc = rt::file_read(&format!("rom:/{name}.f2ns"))?;
    if sc.len() < 96 || &sc[0..4] != b"F2NS" || be_u32(&sc, 4) != 3 {
        log!("R3D: {name}.f2ns: bad scene file");
        return None;
    }
    let ids = match r.load_file(model) {
        Ok(ids) => ids,
        Err(e) => {
            log!("R3D: {name}.f2nm: {:?}", e);
            return None;
        }
    };
    let mut view = ViewParams::pc_default(320.0, 240.0);
    for (i, row) in view.view.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = be_f32(&sc, 8 + (i * 4 + j) * 4);
        }
    }
    view.far = be_f32(&sc, 72);
    view.near = be_f32(&sc, 76);
    view.fov = be_f32(&sc, 80);
    let clear = be_u32(&sc, 84);
    if be_u32(&sc, 88) & 1 != 0 {
        view.fog = Some(FogParams { start: view.far * 0.5, end: view.far, rgba: clear });
    }
    let n = be_u32(&sc, 92) as usize;
    let mut draws = Vec::new();
    let mut tris = 0;
    let mut o = 96;
    for _ in 0..n {
        if o + 12 > sc.len() {
            break;
        }
        let mi = be_u32(&sc, o) as usize;
        let lw = be_u32(&sc, o + 4);
        let list = FRAME_LISTS.get((lw & 0xFF) as usize).copied().unwrap_or(List::Normal);
        let nm = be_u32(&sc, o + 8) as usize;
        o += 12;
        let mut mats = Vec::with_capacity(nm);
        for _ in 0..nm {
            let mut m = IDENTITY;
            for (i, row) in m.iter_mut().enumerate() {
                for (j, v) in row.iter_mut().enumerate() {
                    *v = be_f32(&sc, o + (i * 4 + j) * 4);
                }
            }
            o += 64;
            mats.push(m);
        }
        if let Some(&id) = ids.get(mi) {
            tris += r.mesh_tris(id);
            draws.push((id, list, lw & 0x100 != 0, mats));
        }
    }
    Some(Scene { name: String::from(name), view, clear, draws, tris })
}

/// Benchmarks one scene: sustained frame rate, CPU cost (frame recorded into an rspq block:
/// nothing executes), RSP+RDP cost (frame recorded in flat mode, replayed until idle).
fn run_bench(r: &mut R3d, view: &ViewParams, name: &str, visible: u32, queue: &dyn Fn(&mut R3d)) {
    // (1) Sustained frame rate over 60 frames (triple-buffered, vsync); also warms up the
    // recorded texture uploads.
    rt::rdp_wait();
    let t0 = ticks();
    let frames = 60;
    for _ in 0..frames {
        rt::begin_frame(0x202020ff);
        r.begin_frame(view);
        queue(r);
        r.flush(|_| {});
        rt::present();
    }
    rt::rdp_wait();
    let per_frame = ticks().wrapping_sub(t0) / frames;
    let st = r.stats();
    let (tris, sent) = (r.queued_tris, r.sent_tris);
    // (2) CPU only: record the frame into a block (nothing runs), best of 4.
    let (mut q_best, mut f_best) = (u32::MAX, u32::MAX);
    for _ in 0..4 {
        rt::begin_frame(0x202020ff);
        rt::rdp_wait();
        rt::probe_c(1, 0);
        let t0 = ticks();
        r.begin_frame(view);
        queue(r);
        let t1 = ticks();
        r.flush(|_| {});
        let t2 = ticks();
        rt::probe_c(2, 0);
        rt::probe_c(7, 0);
        rt::present();
        q_best = q_best.min(t1.wrapping_sub(t0));
        f_best = f_best.min(t2.wrapping_sub(t1));
    }
    // (3) RSP + RDP: flat-mode recording replayed until idle, best of 3.
    let mut rsp_best = u32::MAX;
    for _ in 0..3 {
        rt::begin_frame(0x202020ff);
        rt::rdp_wait();
        rt::probe_c(6, 1);
        rt::probe_c(1, 0);
        r.begin_frame(view);
        queue(r);
        r.flush(|_| {});
        rt::probe_c(2, 0);
        rt::probe_c(6, 0);
        rsp_best = rsp_best.min(rt::probe_c(3, 0));
        rt::present();
    }
    let cpu = q_best + f_best;
    let (cpu_us, rsp_us) = (ticks_to_us(cpu), ticks_to_us(rsp_best));
    // CPU cycles = 2 x ticks; RCP cycles = 4/3 x ticks.
    log!(
        "R3D BENCH {}: {} draws, {} tris ({} sent after culling, {} MDX-visible), {} batches, {} tex loads | CPU {} us = queue {} + flush {} ({} cycles/draw) | RSP+RDP {} us ({} RCP cycles/sent tri -> {} tris/frame @60, {} @30) | live frame {} us = {}.{} fps",
        name,
        st.draws,
        tris,
        sent,
        visible,
        st.groups,
        st.tex_loads,
        cpu_us,
        ticks_to_us(q_best),
        ticks_to_us(f_best),
        cpu as u64 * 2 / st.draws.max(1) as u64,
        rsp_us,
        rsp_best as u64 * 4 / 3 / sent.max(1) as u64,
        16_667u64 * sent as u64 / rsp_us.max(1) as u64,
        33_333u64 * sent as u64 / rsp_us.max(1) as u64,
        ticks_to_us(per_frame),
        1_000_000 / ticks_to_us(per_frame).max(1),
        (10_000_000 / ticks_to_us(per_frame).max(1)) % 10
    );
}
