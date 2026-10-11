//! Portable compute kernels used by the N64 self-test and performance probes. They run on
//! the host too (`cargo test`), so N64 results can be compared bit-for-bit with x86.

use alloc::vec::Vec;
use f2_math::{fdiv, fmul, fsqrt, iquat_slerp, make_unit, rcos, rsin, FVector, Fixed, IQuaternion, Rng};

// ---------------------------------------------------------------------------------------
// f2-math self-test.

/// Exercises the game's fixed-point maths and RNG; returns (fmul, fdiv, fsqrt, unit, rng,
/// checksum). The checksum over many calls must match the host's.
pub fn f2_math_selftest() -> (Fixed, Fixed, Fixed, FVector, i32, u32) {
    let a = fmul(3 << 12, 5 << 12);
    let b = fdiv(10 << 12, 4 << 12);
    let c = fsqrt(2 << 12);
    let mut v = FVector { vx: 300 << 12, vy: -400 << 12, vz: 1200 << 12 };
    make_unit(&mut v);
    let mut rng = Rng::default();
    let r = rng.random(1000);
    let mut sum = 0u32;
    for i in 0..2000i32 {
        let x = fmul(i * 977, rsin(i * 37)) ^ fdiv((i + 1) << 12, (i % 7 + 1) << 10);
        sum = sum.wrapping_mul(31).wrapping_add(x as u32).wrapping_add(rng.random(65536) as u32);
        sum ^= fsqrt(i << 8) as u32;
    }
    (a, b, c, v, r, sum)
}

// ---------------------------------------------------------------------------------------
// Probe (a): f2-engine's CPU transform (scene.rs `prepare_object` + `render_mesh`).

pub type Mat4 = [[f32; 4]; 4];
pub type Vec3 = [f32; 3];

/// scene.rs constants (`DIST`, `SZ_SCALE`, `UV_SCALE`).
const DIST: f32 = -10.0;
const SZ_SCALE: f32 = 0.00025;
const UV_SCALE: f32 = 0.000975;

/// f2-engine math.rs `mtx_cat`.
pub fn mtx_cat(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut r = [[0f32; 4]; 4];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    for j in 0..3 {
        r[3][j] = a[3][0] * b[0][j] + a[3][1] * b[1][j] + a[3][2] * b[2][j] + b[3][j];
    }
    r[3][3] = 1.0;
    r
}

/// f2-engine math.rs `mtx_xfm`.
#[inline]
pub fn mtx_xfm(m: &Mat4, v: Vec3) -> Vec3 {
    [
        m[0][0] * v[0] + m[1][0] * v[1] + m[2][0] * v[2] + m[3][0],
        m[0][1] * v[0] + m[1][1] * v[1] + m[2][1] * v[2] + m[3][1],
        m[0][2] * v[0] + m[1][2] * v[1] + m[2][2] * v[2] + m[3][2],
    ]
}

/// The parts of scene.rs `View` used by the transform.
pub struct View {
    pub vp: [f32; 4],
    pub fov: f32,
    pub proj_scale: f32,
    pub near_clip: f32,
    pub far_clip: f32,
}

impl View {
    pub fn n64() -> View {
        View { vp: [0.0, 0.0, 320.0, 240.0], fov: 450.0, proj_scale: 0.5, near_clip: 10.0, far_clip: 1200.0 }
    }
    #[inline]
    fn oozd(&self, vz: f32) -> f32 {
        let i = ((vz as i64) + DIST as i64).clamp(1, 65534) as f32;
        -self.fov * (self.proj_scale / i)
    }
    #[inline]
    pub fn project(&self, p: Vec3) -> Option<[f32; 3]> {
        let z = p[2] + DIST;
        if z > self.near_clip && z < self.far_clip {
            let o = self.oozd(p[2]);
            let [x, y, w, h] = self.vp;
            Some([x + w / 2.0 + p[0] * o, y + h / 2.0 + p[1] * o, p[2]])
        } else {
            None
        }
    }
}

/// A synthetic mesh shaped like a landscape tile set: a `n`x`n` vertex grid, two faces per cell.
pub struct Mesh {
    pub vertices: Vec<Vec3>,
    pub faces: Vec<[u16; 3]>,
    pub colors: Vec<[f32; 4]>,
    pub face_uv: Vec<[i16; 2]>,
}

pub fn grid_mesh(n: usize, spacing: f32) -> Mesh {
    let mut m = Mesh { vertices: Vec::new(), faces: Vec::new(), colors: Vec::new(), face_uv: Vec::new() };
    let half = (n as f32 - 1.0) * spacing / 2.0;
    for z in 0..n {
        for x in 0..n {
            let h = libm::sinf(x as f32 * 0.7) * libm::cosf(z as f32 * 0.5) * 20.0;
            m.vertices.push([x as f32 * spacing - half, h, z as f32 * spacing - half]);
        }
    }
    for z in 0..n - 1 {
        for x in 0..n - 1 {
            let i = (z * n + x) as u16;
            let n16 = n as u16;
            for f in [[i, i + n16, i + 1], [i + 1, i + n16, i + n16 + 1]] {
                m.faces.push(f);
                for (k, &fk) in f.iter().enumerate() {
                    let c = (fk % 7) as f32 / 7.0;
                    m.colors.push([0.5 + c * 0.5, 0.8, 1.0 - c * 0.5, 1.0]);
                    m.face_uv.push([(k as i16) * 1024, ((k as i16) >> 1) * 1024]);
                }
            }
        }
    }
    m
}

/// scene.rs `TlVertex`.
#[derive(Clone, Copy, Default, Debug)]
pub struct TlVertex {
    pub sx: f32,
    pub sy: f32,
    pub sz: f32,
    pub rhw: f32,
    pub color: [f32; 4],
    pub uv: [f32; 2],
    pub fog: f32,
    pub alpha_ref: f32,
}

pub type Tv = Option<[f32; 3]>;

/// scene.rs `prepare_object`.
pub fn prepare_object(view: &View, mesh: &Mesh, m: &Mat4, v_matrix: &Mat4, tv: &mut Vec<Tv>) {
    let f = mtx_cat(m, v_matrix);
    tv.clear();
    tv.extend(mesh.vertices.iter().map(|&v| view.project(mtx_xfm(&f, v))));
}

fn between(x: f32, a: f32, b: f32) -> bool {
    x > a && x < b
}

/// scene.rs `render_mesh` (textures/gouraud/fog options fixed: retail defaults, fog off).
pub fn render_mesh(view: &View, mesh: &Mesh, tv: &[Tv], out: &mut Vec<[TlVertex; 3]>) {
    let [vx, vy, vw, vh] = view.vp;
    let (clx0, cly0, clx1, cly1) = (vx, vy, vx + vw - 1.0, vy + vh - 1.0);
    for (fi, face) in mesh.faces.iter().enumerate() {
        let get = |k: usize| tv.get(face[k] as usize).copied().flatten();
        let (Some(a), Some(b), Some(c)) = (get(0), get(1), get(2)) else { continue };
        let p = [a, b, c];
        let yon = p.iter().filter(|v| between(v[1], cly0, cly1)).count();
        let xon = p.iter().filter(|v| between(v[0], clx0, clx1)).count();
        if yon == 0 || xon == 0 {
            continue;
        }
        let cross = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        if cross >= 0.0 {
            continue;
        }
        let mut v = [TlVertex::default(); 3];
        for k in 0..3 {
            let col = mesh.colors.get(fi * 3 + k).copied().unwrap_or([1.0; 4]);
            let uv = mesh.face_uv.get(fi * 3 + k).copied().unwrap_or([0, 0]);
            let vz = p[k][2];
            v[k] = TlVertex {
                sx: p[k][0],
                sy: p[k][1],
                sz: (vz * SZ_SCALE).clamp(0.0, 1.0),
                rhw: 1.0 / (vz + DIST),
                color: [col[0].clamp(0.0, 1.0), col[1].clamp(0.0, 1.0), col[2].clamp(0.0, 1.0), col[3].clamp(0.0, 1.0)],
                uv: [uv[0] as f32 * UV_SCALE, uv[1] as f32 * UV_SCALE],
                fog: 1.0,
                alpha_ref: 0.0,
            };
        }
        out.push(v);
    }
}

/// A camera matrix looking down at the grid from (0, 300, -400) (rotation about X).
pub fn camera_matrix(angle: f32) -> Mat4 {
    let (s, c) = (libm::sinf(angle), libm::cosf(angle));
    [[c, 0.0, -s, 0.0], [0.0, 1.0, 0.0, 0.0], [s, 0.0, c, 0.0], [0.0, 0.0, 0.0, 1.0]]
}

pub fn view_matrix() -> Mat4 {
    let (s, c) = (libm::sinf(-0.6), libm::cosf(-0.6));
    [[1.0, 0.0, 0.0, 0.0], [0.0, c, s, 0.0], [0.0, -s, c, 0.0], [0.0, -60.0, 700.0, 1.0]]
}

/// Bit-level checksum of transformed output (host/N64 float determinism check).
pub fn tl_checksum(tris: &[[TlVertex; 3]]) -> u32 {
    let mut h = 0x811c_9dc5u32;
    for t in tris {
        for v in t {
            for x in [v.sx, v.sy, v.sz, v.rhw, v.color[0], v.uv[0], v.uv[1]] {
                h = (h ^ x.to_bits()).wrapping_mul(0x0100_0193);
            }
        }
    }
    h
}

// ---------------------------------------------------------------------------------------
// Probe (c): integer fixed-point work shaped like an f2-game tick.

#[derive(Clone, Copy, Default)]
pub struct Actor {
    pub pos: FVector,
    pub vel: FVector,
    pub target: FVector,
    pub q: IQuaternion,
    pub q_aim: IQuaternion,
    pub angle: i32,
}

pub fn make_actors(n: usize) -> Vec<Actor> {
    let mut rng = Rng::default();
    (0..n)
        .map(|_| {
            let r = |rng: &mut Rng| (rng.random(2000) - 1000) << 12;
            Actor {
                pos: FVector { vx: r(&mut rng), vy: r(&mut rng), vz: r(&mut rng) },
                target: FVector { vx: r(&mut rng), vy: 0, vz: r(&mut rng) },
                q: IQuaternion { x: 0, y: 0, z: 0, w: 4096 },
                q_aim: IQuaternion { x: 0, y: 2896, z: 0, w: 2896 },
                ..Default::default()
            }
        })
        .collect()
}

/// One "tick" over all actors: steer towards a target (normalise, scale), integrate,
/// distance test (sqrt), slerp orientation, sine-wave bob, RNG — the operation mix of
/// enemy/platform/frog updates. Returns a checksum.
pub fn fixed_tick(actors: &mut [Actor], rng: &mut Rng) -> u32 {
    let mut sum = 0u32;
    for a in actors.iter_mut() {
        let mut d = FVector { vx: a.target.vx - a.pos.vx, vy: a.target.vy - a.pos.vy, vz: a.target.vz - a.pos.vz };
        let dist = fsqrt(fmul(d.vx >> 6, d.vx >> 6).wrapping_add(fmul(d.vz >> 6, d.vz >> 6)));
        make_unit(&mut d);
        let speed = fdiv(5 << 12, (dist >> 4) + 4096);
        a.vel = FVector { vx: fmul(d.vx, speed), vy: fmul(d.vy, speed), vz: fmul(d.vz, speed) };
        a.pos.vx = a.pos.vx.wrapping_add(a.vel.vx);
        a.pos.vy = a.pos.vy.wrapping_add(fmul(rsin(a.angle), 2 << 12));
        a.pos.vz = a.pos.vz.wrapping_add(a.vel.vz);
        a.angle = (a.angle + 64) & 4095;
        let mut q = IQuaternion::default();
        iquat_slerp(&a.q, &a.q_aim, 410, &mut q);
        a.q = q;
        if dist < (20 << 12) {
            a.target.vx = (rng.random(2000) - 1000) << 12;
            a.target.vz = (rng.random(2000) - 1000) << 12;
        }
        sum = sum.wrapping_add(a.pos.vx as u32 ^ a.q.w as u32 ^ rcos(a.angle) as u32);
    }
    sum
}

/// FNV-1a 32 (decode checks against host-computed values, tools/n64/stage-f2z-probe.py).
pub fn fnv1a(data: &[u8]) -> u32 {
    data.iter().fold(0x811c_9dc5u32, |h, &b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_reference() {
        assert_eq!(fnv1a(b""), 0x811c_9dc5);
        assert_eq!(fnv1a(b"a"), 0xe40c_292c);
        assert_eq!(fnv1a(b"foobar"), 0xbf9c_f968);
    }

    #[test]
    fn host_reference_values() {
        // Printed so the N64 log can be compared against the host (x86) results.
        let st = f2_math_selftest();
        std::println!("selftest {st:?}");
        let view = View::n64();
        let mesh = grid_mesh(32, 24.0);
        let mut tv = Vec::new();
        let mut tris = Vec::new();
        prepare_object(&view, &mesh, &camera_matrix(0.3), &view_matrix(), &mut tv);
        render_mesh(&view, &mesh, &tv, &mut tris);
        std::println!("transform tris {} checksum {:08x}", tris.len(), tl_checksum(&tris));
        let mut actors = make_actors(64);
        let mut rng = Rng::default();
        let mut s = 0u32;
        for _ in 0..10 {
            s = s.wrapping_add(fixed_tick(&mut actors, &mut rng));
        }
        std::println!("fixed tick checksum {s:08x}");
        // Identical values were logged by the ROM in ares (research/n64-performance.md):
        // fixed-point, RNG and the f32 transform are bit-exact between x86-64 and the VR4300.
        assert_eq!(st.5, 0x088a_1a37);
        assert_eq!((tris.len(), tl_checksum(&tris)), (1759, 0xb689_c4ca));
        assert_eq!(s, 0xf786_37a0);
    }
}
