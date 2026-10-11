//! Bring-up demo: two interpenetrating textured, Gouraud-shaded, z-buffered cubes plus a
//! translucent overlay triangle, moved with the joypad; A beeps through the mixer.

use crate::ffi::{self, pad, State, Vertex};
use crate::log;
use crate::probes::RdpProbe;
use crate::rt::{self, ticks, ticks_to_us};
use alloc::vec::Vec;
use f2_math::{fmul, rcos, rsin};

pub struct Demo {
    tex: u32,
    frame: u32,
    angle: i32,
    pos: [f32; 2],
    prev_pad: u32,
    last_ticks: u32,
    beeps: u32,
    probe: Option<RdpProbe>,
    verts: Vec<Vertex>,
}

/// 32x32 RGBA16 (5551) checkerboard with a frog-green / yellow palette.
fn checker() -> Vec<u16> {
    let rgba16 = |r: u16, g: u16, b: u16| (r >> 3) << 11 | (g >> 3) << 6 | (b >> 3) << 1 | 1;
    (0..32 * 32)
        .map(|i| {
            let (x, y) = (i % 32, i / 32);
            if ((x / 8) + (y / 8)) % 2 == 0 {
                rgba16(40, 200, 60)
            } else {
                rgba16(250, 230, 80)
            }
        })
        .collect()
}

const CUBE: [[f32; 3]; 8] = [[-1., -1., -1.], [1., -1., -1.], [1., 1., -1.], [-1., 1., -1.], [-1., -1., 1.], [1., -1., 1.], [1., 1., 1.], [-1., 1., 1.]];
const QUADS: [[usize; 4]; 6] = [[0, 1, 2, 3], [5, 4, 7, 6], [4, 0, 3, 7], [1, 5, 6, 2], [4, 5, 1, 0], [3, 2, 6, 7]];
const SHADES: [f32; 6] = [1.0, 0.55, 0.75, 0.85, 0.65, 0.95];

impl Demo {
    pub fn new() -> Demo {
        let tex = rt::tex_create_rgba16(32, 32, &checker());
        log!("demo: texture handle {}", tex);
        Demo { tex, frame: 0, angle: 0, pos: [0.0, 0.0], prev_pad: 0, last_ticks: ticks(), beeps: 0, probe: Some(RdpProbe::new(tex)), verts: Vec::new() }
    }

    /// Projects a cube rotated by the f2-math fixed-point angle (4096 = full turn).
    fn cube(&mut self, center: [f32; 3], size: f32, angle: i32, tint: [f32; 3]) {
        // f2-math rsin/rcos (fixed 1.0 = 4096) drive the rotation: the game's own trig tables.
        let (s1, c1) = (rsin(angle) as f32 / 4096.0, rcos(angle) as f32 / 4096.0);
        let a2 = fmul(angle << 12, 2867) >> 12; // 0.7 x angle, through f2-math fmul
        let (s2, c2) = (rsin(a2) as f32 / 4096.0, rcos(a2) as f32 / 4096.0);
        let mut p = [[0f32; 4]; 8];
        for (i, v) in CUBE.iter().enumerate() {
            let (x, y, z) = (v[0] * size, v[1] * size, v[2] * size);
            let (x, z) = (x * c1 + z * s1, -x * s1 + z * c1);
            let (y, z) = (y * c2 - z * s2, y * s2 + z * c2);
            let (x, y, z) = (x + center[0], y + center[1], z + center[2]);
            let inv = 1.0 / z;
            p[i] = [160.0 + x * 200.0 * inv, 120.0 + y * 200.0 * inv, (z - 2.0) / 10.0, inv];
        }
        for (q, quad) in QUADS.iter().enumerate() {
            let [a, b, c, d] = quad.map(|k| p[k]);
            // Back-face cull (screen-space winding).
            if (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]) <= 0.0 {
                continue;
            }
            let sh = SHADES[q];
            let uv = [[0.0, 0.0], [32.0, 0.0], [32.0, 32.0], [0.0, 32.0]];
            let mk = |p: [f32; 4], k: usize, g: f32| Vertex {
                x: p[0],
                y: p[1],
                z: p[2].clamp(0.0, 1.0),
                r: tint[0] * sh * g,
                g: tint[1] * sh * g,
                b: tint[2] * sh * g,
                a: 1.0,
                s: uv[k][0],
                t: uv[k][1],
                inv_w: 1.0,
            };
            // Gouraud: darken one corner per face.
            let (va, vb, vc, vd) = (mk(a, 0, 1.0), mk(b, 1, 0.8), mk(c, 2, 0.5), mk(d, 3, 0.8));
            self.verts.extend_from_slice(&[va, vb, vc, va, vc, vd]);
        }
    }

    pub fn frame(&mut self) {
        let now = ticks();
        let dt = now.wrapping_sub(self.last_ticks);
        self.last_ticks = now;
        self.frame += 1;

        // A few frames in, run the RDP throughput probes (one per frame).
        if self.frame > 3 {
            if let Some(p) = self.probe.as_mut() {
                if p.frame() {
                    return;
                }
                self.probe = None;
                log!("PROBES DONE");
            }
        }

        let b = rt::pad(0);
        let pressed = b & !self.prev_pad;
        if b != self.prev_pad {
            log!("pad: buttons {:04x} stick ({}, {}) frame {}", b & 0xffff, ((b >> 16) & 0xff) as u8 as i8, (b >> 24) as u8 as i8, self.frame);
        }
        self.prev_pad = b;
        if pressed & pad::Z != 0 {
            // Exercises the panic handler (debugf + libdragon assert screen).
            panic!("test panic requested with Z at frame {}", self.frame);
        }
        if pressed & pad::A != 0 {
            self.beeps += 1;
            rt::beep(880 + 110 * (self.beeps % 4), 250);
            log!("beep {} (A pressed); mixer has pulled {} samples so far", self.beeps, rt::audio_samples());
        }
        let step = 0.06;
        if b & pad::LEFT != 0 {
            self.pos[0] -= step;
        }
        if b & pad::RIGHT != 0 {
            self.pos[0] += step;
        }
        if b & pad::UP != 0 {
            self.pos[1] -= step;
        }
        if b & pad::DOWN != 0 {
            self.pos[1] += step;
        }
        self.angle = (self.angle + 24) & 4095;

        rt::begin_frame(0x183050ff);
        let zt = State { flags: ffi::ZTEST | ffi::ZWRITE | ffi::BILINEAR | ffi::WRAP, blend: ffi::BLEND_NONE, tex: self.tex, alpha_ref: 0 };
        self.verts.clear();
        self.cube([self.pos[0] - 0.5, self.pos[1], 6.0], 1.3, self.angle, [1.0, 1.0, 1.0]);
        self.cube([0.9, 0.2, 6.5], 1.2, 4096 - self.angle, [1.0, 0.6, 0.6]);
        rt::draw_tris(&self.verts, &zt);
        // Translucent untextured overlay (no z).
        let ov = State { flags: 0, blend: ffi::BLEND_SEMI, tex: 0, alpha_ref: 0 };
        let v = |x: f32, y: f32, r: f32, g: f32, b: f32| Vertex { x, y, z: 0.0, r, g, b, a: 0.5, s: 0.0, t: 0.0, inv_w: 1.0 };
        let tri = [v(20.0, 220.0, 1.0, 0.2, 0.2), v(120.0, 225.0, 0.2, 1.0, 0.2), v(60.0, 150.0, 0.2, 0.2, 1.0)];
        rt::draw_tris(&tri, &ov);
        rt::present();

        if self.frame.is_multiple_of(120) {
            log!(
                "frame {}: {} us/frame ({} fps), {} tris, mixer samples {}",
                self.frame,
                ticks_to_us(dt),
                1_000_000 / ticks_to_us(dt).max(1),
                self.verts.len() / 3 + 1,
                rt::audio_samples()
            );
        }
        if self.frame == 30 {
            // Scripted beep so the audio path is exercised without input.
            rt::beep(660, 200);
            log!("beep (scripted at frame 30)");
        }
        if self.frame == 60 {
            log!("audio: mixer pulled {} samples from the beep waveform", rt::audio_samples());
        }
    }
}
