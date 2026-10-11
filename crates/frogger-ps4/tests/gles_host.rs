//! The GLES2 back end (the console's Piglet path) against the CPU back end on a
//! synthetic scene exercising every render state: all frame passes (blends, depth
//! modes, wrap/clamp, alpha references), overlapping depth, alpha-tested texels, fog,
//! affine and perspective-correct interpolation, nearest and bilinear filtering,
//! non-power-of-two textures (also through the shader-wrap path used without
//! `OES_texture_npot`) and the ultrawide viewport scissor.
//!
//! Runs on a host GLES2 context (Mesa EGL, surfaceless); skipped when there is none.

#[path = "../examples/common/egl.rs"]
mod egl;

use f2_engine::gles::{self, GlesBackend};
use f2_engine::{Blend, Fog, Frame, Quad2D, Renderer, TexHandle, TlVertex, Tri};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }
}

fn texture(r: &mut Renderer, w: u32, h: u32, seed: u64) -> TexHandle {
    let mut rng = Rng(seed);
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..w * h {
        let a = if rng.next() < 0.25 {
            0
        } else if rng.next() < 0.3 {
            90
        } else {
            255
        };
        px.extend_from_slice(&[(rng.next() * 255.0) as u8, (rng.next() * 255.0) as u8, (rng.next() * 255.0) as u8, a]);
    }
    r.textures.add_image(w, h, px)
}

fn scene(r: &mut Renderer, w: u32, h: u32, texs: &[TexHandle]) {
    r.begin_frame(w, h);
    let mut rng = Rng(7);
    let frames = [Frame::Normal, Frame::Wrap, Frame::Xlu, Frame::Additive, Frame::Late, Frame::LateAdditive, Frame::Overlay];
    for i in 0..400 {
        let frame = frames[i % frames.len()];
        let (cx, cy) = (rng.next() * w as f32, rng.next() * h as f32);
        let s = 20.0 + rng.next() * w as f32 * 0.25;
        let z = rng.next();
        let mut v = [TlVertex::default(); 3];
        for (k, vx) in v.iter_mut().enumerate() {
            let ang = rng.next() * std::f32::consts::TAU + k as f32 * 2.1;
            vx.sx = cx + ang.cos() * s;
            vx.sy = cy + ang.sin() * s;
            vx.sz = (z + (rng.next() - 0.5) * 0.2).clamp(0.0, 1.0);
            vx.rhw = 0.2 + rng.next() * 2.0;
            vx.color = [rng.next(), rng.next(), rng.next(), 0.3 + rng.next() * 0.7];
            // Wrapped frames get coordinates outside 0..1.
            let span = if frame == Frame::Wrap || frame == Frame::Xlu { 3.0 } else { 1.0 };
            vx.uv = [rng.next() * span - (span - 1.0) / 2.0, rng.next() * span - (span - 1.0) / 2.0];
            vx.fog = rng.next();
        }
        let tex = if i % 11 == 0 { None } else { Some(texs[i % texs.len()]) };
        r.push_tri(frame, Tri { v, tex });
    }
    r.draw_quad(Quad2D::rect(10.0, 10.0, 120.0, 60.0, [0.0, 0.0, 1.0, 1.0], Some(texs[0]), [255, 255, 255, 200], Blend::Semi));
    r.draw_quad(Quad2D::rect(30.0, 40.0, 90.0, 50.0, [0.0, 0.0, 1.0, 1.0], None, [200, 40, 40, 255], Blend::Subtractive));
}

#[test]
fn gles_matches_soft() {
    let (w, h) = (480u32, 270u32);
    let host = match egl::HostGl::new(w, h) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("skipped: no host GLES2 context ({e})");
            return;
        }
    };
    let gl = host.fns();
    let program = gles::program_from_source(&gl).unwrap();
    let mut be = GlesBackend::new(gl, program).unwrap();
    let mut r = Renderer::new_soft();
    // Power-of-two and non-power-of-two (3x5, 20x12) textures.
    let texs = [texture(&mut r, 16, 16, 1), texture(&mut r, 3, 5, 2), texture(&mut r, 20, 12, 3), texture(&mut r, 64, 32, 4)];
    let mut worst = (0.0f64, 0.0f64);
    for case in 0..16 {
        let (persp, linear, fog, clamp) = (case & 1 != 0, case & 2 != 0, case & 4 != 0, case & 8 != 0);
        for emulate in [false, true] {
            r.options.perspective_correct = persp;
            r.options.linear_filter = linear;
            r.options.fog = Fog { enabled: fog, start: 0.0, end: 1.0, color: [0.3, 0.5, 0.7] };
            r.options.max_aspect = if clamp { 1.2 } else { 0.0 };
            be.force_npot_emulation = emulate;
            scene(&mut r, w, h, &texs);
            r.render_gles(&mut be, w, h, [0.1, 0.2, 0.3, 1.0]);
            let gles_px = be.read_pixels(w, h);
            let soft = r.render_to_image([0.1, 0.2, 0.3, 1.0]).unwrap();
            let (mean, share, max) = egl::diff(&gles_px, &soft.pixels, 8);
            eprintln!("persp {persp} linear {linear} fog {fog} clamp {clamp} npot-emulation {emulate}: mean {mean:.3}, >8: {:.3}%, max {max}", share * 100.0);
            assert!(share < 0.006 && mean < 1.0, "GLES differs from the CPU renderer: mean {mean}, share {share}");
            if !emulate {
                worst = (worst.0.max(mean), worst.1.max(share));
            }
        }
    }
    eprintln!("worst (native wrap): mean {:.3}, >8: {:.3}%", worst.0, worst.1 * 100.0);
}

#[test]
fn program_binary_round_trip() {
    let Ok(host) = egl::HostGl::new(16, 16) else { return };
    let gl = host.fns();
    let p = gles::program_from_source(&gl).unwrap();
    if let Some(bin) = gles::program_binary(&gl, p) {
        let q = gles::program_from_binary(&gl, &bin).unwrap();
        assert!(GlesBackend::new(host.fns(), q).is_ok());
    }
    assert!(gles::program_from_binary(&gl, &[1, 2, 3, 4, 5, 6]).is_err());
}
