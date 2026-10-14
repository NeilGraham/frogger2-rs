//! 3D effects drawn by `DrawLoop` after the actors: `ProcessShadows` / `DrawShadow`,
//! `DrawSpecialFX` (decals, rings, lightning, tongues) from `PC/pcgfx.c`, and
//! `PrintSprites` (z-sorted billboard sprites) from `PC/pcsprite.c`.
//!
//! Everything reads the simulation state; the state changes these functions make in the
//! original (sprite rotation, health-fly twinkles) are done by `f2-game` in the tick.
//!
//! `SwapFrame(0)` after `DrawAllFrames` (sprites, special FX) is drawn with `xluZRS` /
//! `xluSemiRS` (z-test, no z-write, semi-transparent: vertex alpha x texture alpha, so the
//! coins' `a = 200` really is ~78% opaque on retail), which is the engine's `Frame::Late`;
//! `SwapFrame(3)` is `Frame::LateAdditive`. Both are flushed after every world/actor
//! frame in submission order (drawloop.cpp:283-292). Shadows are drawn mid-loop on
//! retail and stay in `Frame::Xlu`.

use alloc::collections::BTreeMap;
use f2_engine::math::{mtx_cat, mtx_xfm, quaternion_to_matrix, Mat4};
use f2_engine::scene::{Frame, TlVertex, Tri, DIST, SZ_SCALE};
use f2_engine::{Renderer, TexHandle};
use f2_game::ids::{SpecFxId, SpriteId};
use f2_game::specfx::{FxDraw, FXTYPE_CROAK, FXTYPE_GARIBCOLLECT, FXTYPE_WAKE, FXTYPE_WATERRIPPLE, NUM_RINGSEGS};
use f2_game::sprite::{Sprite, SPRITE_ADDITIVE, SPRITE_FLAGS_ROTATE};
use f2_game::Game;
use f2_math::{FVector, SVector};

// Newer `core` has some of these methods inherently (then the trait goes unused).
#[cfg(not(feature = "std"))]
#[allow(unused_imports)]
use crate::fmath::FloatExt;
use crate::prelude::*;

type V3 = [f32; 3];

/// `FIXED_TO_RADS` (pcsprite.c:28).
pub const FIXED_TO_RADS: f32 = 0.001525879;
/// `MAX_ARRAY_SPRITES` (pcsprite.c:30).
const MAX_ARRAY_SPRITES: usize = 768;
const ONEOVER255: f32 = 1.0 / 255.0;
const UP_V: V3 = [0.0, 1.0, 0.0];

/// A `D3DTLVERTEX` under construction (screen x/y, `sz`, uv, colour).
#[derive(Clone, Copy, Debug, Default)]
struct Tv {
    sx: f32,
    sy: f32,
    sz: f32,
    tu: f32,
    tv: f32,
    color: [f32; 4],
}

impl Tv {
    fn tl(&self) -> TlVertex {
        TlVertex { sx: self.sx, sy: self.sy, sz: self.sz, rhw: 1.0, color: self.color, uv: [self.tu, self.tv], fog: 1.0, alpha_ref: 0.0 }
    }
}

fn rgba(r: f32, g: f32, b: f32, a: f32) -> [f32; 4] {
    // D3DRGBA clamps through the byte conversion.
    [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0), a.clamp(0.0, 1.0)]
}

fn argb(c: u32) -> [f32; 4] {
    [((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0, (c >> 24) as f32 / 255.0]
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
/// `Normalise` (mdxMath.cpp:67): zero vectors stay zero.
fn normalise(a: V3) -> V3 {
    let m = dot(a, a).sqrt();
    if m != 0.0 {
        [a[0] / m, a[1] / m, a[2] / m]
    } else {
        a
    }
}
fn rs(v: &SVector) -> V3 {
    [v.vx as f32, v.vy as f32, v.vz as f32]
}
/// `SetVectorRF`: fixed 20.12 to float.
fn rf(v: &FVector) -> V3 {
    [v.vx as f32 / 4096.0, v.vy as f32 / 4096.0, v.vz as f32 / 4096.0]
}
fn rf_s(v: &SVector) -> V3 {
    [v.vx as f32 / 4096.0, v.vy as f32 / 4096.0, v.vz as f32 / 4096.0]
}
fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

/// `GetQuaternionFromRotation` (mdxMath.cpp:223): axis `a`, angle `w` → quaternion.
fn quat_from_rotation(a: V3, w: f32) -> [f32; 4] {
    let (s, c) = (w / 2.0).sin_cos();
    [s * a[0], s * a[1], s * a[2], c]
}

/// `QuaternionMultiply` (mdxMath.cpp:389).
fn quat_mul(s1: [f32; 4], s2: [f32; 4]) -> [f32; 4] {
    let [a, b, c, d] = s1;
    let [x, y, z, w] = s2;
    let t0 = (d - c) * (z - w);
    let t1 = (a + b) * (x + y);
    let t2 = (a - b) * (z + w);
    let t3 = (c + d) * (x - y);
    let t4 = (d - b) * (y - z);
    let t5 = (d + b) * (y + z);
    let t6 = (a + c) * (x - w);
    let t7 = (a - c) * (x + w);
    let t8 = t5 + t6 + t7;
    let t9 = 0.5 * (t4 + t8);
    [t0 + t9 - t5, t1 + t9 - t8, t2 + t9 - t7, t3 + t9 - t6]
}

/// `quaternion_to_matrix(&rotation_to_normal(normal))`, memoised: shadows, decals and rings
/// lie on a few distinct tile normals (mostly straight up), and the `acos` / `sin_cos` /
/// normalisation behind each is costly on a console FPU. Same values as computing it.
fn normal_rotation(normal: V3) -> Mat4 {
    static CACHE: spin::Mutex<([([u32; 3], Mat4); 4], usize)> = spin::Mutex::new(([([u32::MAX; 3], [[0.0; 4]; 4]); 4], 0));
    let key = normal.map(f32::to_bits);
    let mut c = CACHE.lock();
    if let Some((_, m)) = c.0.iter().find(|(k, _)| *k == key) {
        return *m;
    }
    let m = quaternion_to_matrix(&rotation_to_normal(normal));
    let i = c.1;
    c.0[i] = (key, m);
    c.1 = (i + 1) % 4;
    m
}

/// Rotation that takes `upV` to `normal`: `cross(normal, upV)` axis, `-acos(dot)` angle
/// (the shadow / decal / ring set-up in pcgfx.c).
fn rotation_to_normal(normal: V3) -> [f32; 4] {
    let axis = normalise(cross(normal, UP_V));
    // acos of a dot product a hair above 1 is NaN in the original (the poly is lost);
    // clamped here.
    let t = dot(normal, UP_V).clamp(-1.0, 1.0);
    quat_from_rotation(axis, -t.acos())
}

/// `XfmPoint` + the `sz` the pcgfx functions store: `None` where `m.vz == 0`.
fn xfm(r: &Renderer, p: V3) -> Option<[f32; 3]> {
    r.view.xfm_point(p).filter(|m| m[2] != 0.0)
}

/// `Clip3DPolygon(v, tex)`: one triangle into the current frame (the GPU clips to the
/// screen; with affine `rhw = 1` mapping the result is the same).
fn push(r: &mut Renderer, frame: Frame, v: [Tv; 3], tex: TexHandle) {
    r.push_tri(frame, Tri { v: [v[0].tl(), v[1].tl(), v[2].tl()], tex: Some(tex) });
}

fn tex_of(r: &Renderer, crc: u32) -> Option<TexHandle> {
    if crc == 0 {
        None
    } else {
        r.textures.current(crc)
    }
}

// ------------------------------------------------------------------ shadows

/// Port of `ProcessShadows` (pcgfx.c:72).
pub fn process_shadows(r: &mut Renderer, game: &Game) {
    let Some(tex) = tex_of(r, game.specfx.textures.txtr_solid_ring) else { return };
    let actors = &game.actor2.actors;
    let scale_div = f2_game::map::SCALE as f32;
    let wide = r.view.widened();
    for i in 0..(game.frogger.num_frogs.clamp(0, 4) as usize) {
        let Some(fid) = game.frogger.frog[i] else { continue };
        let Some(f) = actors.get(fid) else { continue };
        let Some(sh) = f.actor.shadow.as_ref() else { continue };
        if f.draw == 0 || sh.draw == 0 {
            continue;
        }
        let (tile_pos, up) = if let Some(p) = game.platform.curr_platform[i] {
            let plt = &game.platform.platform_list.array[p.idx()];
            let Some(pa) = plt.plt_actor.and_then(|a| actors.get(a)) else { continue };
            let Some(t) = plt.in_tile[0] else { continue };
            (rs(&pa.actor.position), rf_s(&game.map.tile(t).normal))
        } else {
            let Some(t) = game.frogmove.curr_tile[i] else { continue };
            let tile = game.map.tile(t);
            (rs(&tile.centre), rf_s(&tile.normal))
        };
        let pos = rs(&f.actor.position);
        let up = normalise(up);
        let height = dot(sub(pos, tile_pos), up) / scale_div;
        let d = (height * 0.025).max(1.0);
        let size = (sh.radius >> 12) as f32 / d;
        let alpha = (sh.alpha as f32 / d) as i16;
        draw_shadow(r, pos, up, size, -height + 1.0, alpha, tex);
    }

    // enemy shadows
    let el = &game.enemies.enemy_list;
    let mut c = el.head.first;
    while let Some(id) = c {
        let nme = &el.array[id.idx()];
        c = nme.links.next;
        if nme.active == 0 {
            continue;
        }
        let Some(a) = nme.nme_actor.and_then(|a| actors.get(a)) else { continue };
        let (Some(sh), Some(t)) = (a.actor.shadow.as_ref(), nme.in_tile) else { continue };
        // `clipped` is the simulation's 640×480 bounding-box test (and stale for actors
        // `ActorListDraw` skipped): a widened Hor+ view draws every shadow and lets the
        // projection clip it.
        if a.clipped != 0 && !wide {
            continue;
        }
        let tile = game.map.tile(t);
        let (tile_pos, pos, up) = (rs(&tile.centre), rs(&a.actor.position), rf_s(&tile.normal));
        let height = dot(sub(pos, tile_pos), up) / scale_div;
        let d = (height * 0.02).max(1.0);
        let size = (sh.radius >> 12) as f32 / d;
        let alpha = (sh.alpha as f32 / d) as i16;
        draw_shadow(r, pos, up, size, -height + 1.0, alpha, tex);
    }

    // platform shadows
    let pl = &game.platform.platform_list;
    let mut c = pl.head.first;
    while let Some(id) = c {
        let plt = &pl.array[id.idx()];
        c = plt.links.next;
        if plt.active == 0 {
            continue;
        }
        let Some(a) = plt.plt_actor.and_then(|a| actors.get(a)) else { continue };
        let (Some(sh), Some(t)) = (a.actor.shadow.as_ref(), plt.in_tile[0]) else { continue };
        // `clipped` is the simulation's 640×480 bounding-box test (and stale for actors
        // `ActorListDraw` skipped): a widened Hor+ view draws every shadow and lets the
        // projection clip it.
        if a.clipped != 0 && !wide {
            continue;
        }
        let tile = game.map.tile(t);
        let (tile_pos, pos, up) = (rs(&tile.centre), rs(&a.actor.position), rf_s(&tile.normal));
        let height = dot(sub(pos, tile_pos), up) / 10.0;
        let d = (height * 0.02).max(1.0);
        let size = (sh.radius >> 12) as f32 / d;
        let alpha = (sh.alpha as f32 / d) as i16;
        draw_shadow(r, pos, up, size, -height + 1.0, alpha, tex);
    }
}

/// Transforms the 4 corners of a quad in object space by `m` and projects them;
/// `sz = (vz + DIST + dz) * 0.00025`. `None` if any corner is clipped (`zeroZ`).
fn xfm_quad(r: &Renderer, m: &Mat4, v: &mut [Tv; 5], corners: [V3; 4], dz: f32) -> bool {
    let mut zero_z = 0;
    for i in 0..4 {
        let p = mtx_xfm(m, corners[i]);
        match xfm(r, p) {
            Some(s) => {
                v[i].sx = s[0];
                v[i].sy = s[1];
                v[i].sz = (s[2] + DIST + dz) * SZ_SCALE;
            }
            None => zero_z += 1,
        }
    }
    v[4] = v[0];
    zero_z == 0
}

/// Port of `DrawShadow` (pcgfx.c:163).
fn draw_shadow(r: &mut Renderer, pos: V3, normal: V3, size: f32, offset: f32, alpha: i16, tex: TexHandle) {
    let pos = scale(pos, 0.1);
    let size = size * 0.1;
    let col = rgba(0.0, 0.0, 0.0, alpha as f32 / 255.0);
    let mut v = [Tv { color: col, ..Default::default() }; 5];
    let uvs = [(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0)];
    for (t, (u, w)) in v.iter_mut().zip(uvs) {
        t.tu = u;
        t.tv = w;
    }
    let rot = normal_rotation(normal);
    // Console fast path: `rot · (translate · I)` is the rotation rows over the translation
    // (up to the sign of zeros: the products add zero terms).
    #[cfg(feature = "lean")]
    let m = [rot[0], rot[1], rot[2], [pos[0], pos[1], pos[2], 1.0]];
    #[cfg(not(feature = "lean"))]
    let m = mtx_cat(&rot, &mtx_cat(&f2_engine::math::gu_translate(pos[0], pos[1], pos[2]), &f2_engine::math::IDENTITY));
    let corners = [[size, offset, size], [size, offset, -size], [-size, offset, -size], [-size, offset, size]];
    if xfm_quad(r, &m, &mut v, corners, -4.0) {
        // Drawn between the list-0 and list-1 actors (the engine's shadow list).
        r.push_shadow(Tri { v: [v[0].tl(), v[1].tl(), v[2].tl()], tex: Some(tex) });
        r.push_shadow(Tri { v: [v[2].tl(), v[3].tl(), v[4].tl()], tex: Some(tex) });
    }
}

// ------------------------------------------------------------------ special effects

/// Port of `DrawSpecialFX` (pcgfx.c:45): the effects' draw functions in list order, then
/// the tongues.
pub fn draw_special_fx(r: &mut Renderer, game: &Game) {
    if game.specfx.count != 0 {
        for (id, fx) in game.specfx.iter_list() {
            match fx.draw {
                Some(FxDraw::Decal) => draw_fx_decal(r, game, id),
                Some(FxDraw::Ring) => draw_fx_ring(r, game, id),
                Some(FxDraw::Lightning) => draw_fx_lightning(r, game, id),
                // UpdateFXTwinkle as a draw function: state only (done in the tick); the
                // fly is drawn through its sprite.
                Some(FxDraw::Twinkle) | None => {}
            }
        }
    }
    for i in 0..(game.frogger.num_frogs.clamp(0, 4) as usize) {
        if game.tongue.tongue[i].flags & f2_game::tongue::TONGUE_BEINGUSED != 0 {
            draw_tongue(r, game, i);
        }
    }
}

/// Port of `DrawFXDecal` (pcgfx.c:268).
fn draw_fx_decal(r: &mut Renderer, game: &Game, id: SpecFxId) {
    let fx = game.specfx.fx(id);
    let Some(tex) = tex_of(r, fx.tex) else { return };
    let sc = scale(rf(&fx.scale), 0.1);
    let normal = rf(&fx.normal);
    let pos = scale(rs(&fx.origin), 0.1);

    let mut v = [Tv::default(); 5];
    let uvs = [(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0)];
    for (t, (u, w)) in v.iter_mut().zip(uvs) {
        t.tu = u;
        t.tv = w;
    }
    let t = f2_engine::math::gu_translate(pos[0], pos[1], pos[2]);
    let q2 = rotation_to_normal(normal);
    let q = if fx.ty == FXTYPE_GARIBCOLLECT {
        // Rotate around the normal by `fx->angle` (a short used as radians).
        let q3 = quat_from_rotation(normal, fx.angle as f32);
        quat_mul(q2, q3)
    } else {
        q2
    };
    let m = mtx_cat(&quaternion_to_matrix(&q), &t);
    let ripple = fx.ty == FXTYPE_WAKE || fx.ty == FXTYPE_WATERRIPPLE;
    let a = fx.a as f32 * ONEOVER255;
    let col = if ripple { rgba(0.2, 0.2, 0.5, a) } else { rgba(fx.r as f32 * ONEOVER255, fx.g as f32 * ONEOVER255, fx.b as f32 * ONEOVER255, a) };
    for t in &mut v {
        t.color = col;
    }
    let corners = [[sc[0], 0.0, sc[2]], [sc[0], 0.0, -sc[2]], [-sc[0], 0.0, -sc[2]], [-sc[0], 0.0, sc[2]]];
    if !xfm_quad(r, &m, &mut v, corners, -4.0) {
        return;
    }
    push(r, Frame::Late, [v[0], v[1], v[2]], tex);
    push(r, Frame::Late, [v[2], v[3], v[4]], tex);
    if ripple {
        // Pseudo bump map: an additive copy shifted 3 pixels right in the real colour.
        let col = rgba(fx.r as f32 * ONEOVER255, fx.g as f32 * ONEOVER255, fx.b as f32 * ONEOVER255, a);
        for t in &mut v {
            t.sx += 3.0;
            t.color = col;
        }
        push(r, Frame::LateAdditive, [v[0], v[1], v[2]], tex);
        push(r, Frame::LateAdditive, [v[2], v[3], v[4]], tex);
    }
}

/// Port of `DrawFXRing` (pcgfx.c:390).
///
/// DEVIATION: the twinkle trail effects the original creates while drawing
/// (`CreateSpecialEffect(FXTYPE_TWINKLE, ...)` every other frame for odd segments) are
/// simulation state and need to happen in `f2-game` (not done there yet).
fn draw_fx_ring(r: &mut Renderer, game: &Game, id: SpecFxId) {
    let fx = game.specfx.fx(id);
    let Some(tex) = tex_of(r, fx.tex) else { return };
    let Some(ring) = game.specfx.ring_vtx.as_ref() else { return };
    if ring.len() < NUM_RINGSEGS * 2 {
        return;
    }
    let sc = scale(rf(&fx.scale), 0.1);
    let normal = rf(&fx.normal);
    let pos = scale(rs(&fx.origin), 0.1);
    let col = rgba(fx.r as f32 * ONEOVER255, fx.g as f32 * ONEOVER255, fx.b as f32 * ONEOVER255, fx.a as f32 * ONEOVER255);

    let mut v = [Tv { color: col, ..Default::default() }; 5];
    let init_uv = [(0.0, 1.0), (0.0, 0.0), (1.0, 0.0), (1.0, 1.0)];
    for (t, (u, w)) in v.iter_mut().zip(init_uv) {
        t.tu = u;
        t.tv = w;
    }
    let mut prev = [Tv::default(); 2];
    let t = f2_engine::math::gu_translate(pos[0], pos[1], pos[2]);
    let m_rt = mtx_cat(&normal_rotation(normal), &t);
    let frame = if fx.ty == FXTYPE_CROAK { Frame::LateAdditive } else { Frame::Late };
    let tilt2 = fx.tilt as f32 * 0.000244;

    let segs = NUM_RINGSEGS * 2;
    for i in 0..NUM_RINGSEGS {
        let vx = i * 2;
        let mut zero_z = 0;
        for j in 0..4 {
            if i != 0 && j < 2 && prev[0].sz != 0.0 && prev[1].sz != 0.0 {
                v[j] = prev[j];
                continue;
            }
            let rv = &ring[(vx + j) % segs];
            let p = [rv.vx as f32 * 0.000244, rv.vy as f32 * 0.000244, rv.vz as f32 * 0.000244];
            // Slant the polys.
            let even = i & 1 == 0;
            let tilt = if (even && (j == 0 || j == 3)) || (!even && (j == 1 || j == 2)) { 1.0 } else { tilt2 };
            v[j].tv = 1.0 - v[j].tv;
            let m = mtx_cat(&f2_engine::math::gu_scale(tilt * sc[0], tilt * sc[1], tilt * sc[2]), &m_rt);
            let w = mtx_xfm(&m, p);
            match xfm(r, w) {
                Some(s) => {
                    v[j].sx = s[0];
                    v[j].sy = s[1];
                    v[j].sz = (s[2] + DIST) * SZ_SCALE;
                }
                None => zero_z += 1,
            }
        }
        if zero_z == 0 {
            v[0].tu = 0.0;
            v[1].tu = 0.0;
            v[2].tu = 1.0;
            v[3].tu = 1.0;
            prev = [v[2], v[3]];
            v[4] = v[0];
            push(r, frame, [v[0], v[1], v[2]], tex);
            push(r, frame, [v[2], v[3], v[4]], tex);
        } else {
            prev[0].sz = 0.0;
            prev[1].sz = 0.0;
        }
    }
}

/// Port of `DrawFXLightning` (pcgfx.c:628).
fn draw_fx_lightning(r: &mut Renderer, game: &Game, id: SpecFxId) {
    let fx = game.specfx.fx(id);
    let Some(tex) = tex_of(r, fx.tex) else { return };
    if game.game_state.mode == f2_game::define::mode::LEVELCOMPLETE_MODE {
        return;
    }
    let parts = game.fx_particles(id);
    let pt = |r: &Renderer, p: &SVector| -> Tv {
        match xfm(r, [p.vx as f32 * 0.1, p.vy as f32 * 0.1, p.vz as f32 * 0.1]) {
            Some(s) => Tv { sx: s[0], sy: s[1], sz: (s[2] + DIST) * SZ_SCALE, ..Default::default() },
            None => Tv::default(),
        }
    };
    let mut prev = [Tv::default(); 2];
    let n = (fx.num_p - 1).max(0) as usize;
    for i in 0..n.min(parts.len().saturating_sub(1)) {
        let p = game.particle.get(parts[i]);
        let pn = game.particle.get(parts[i + 1]);
        let col = rgba(p.r as f32 * ONEOVER255, p.g as f32 * ONEOVER255, p.b as f32 * ONEOVER255, p.a as f32 * ONEOVER255);
        let mut v = [Tv::default(); 5];
        if i != 0 && prev[0].sz != 0.0 && prev[1].sz != 0.0 {
            v[0] = prev[0];
            v[1] = prev[1];
        } else {
            v[0] = pt(r, &p.poly[0]);
            v[0].color = col;
            v[1] = pt(r, &p.poly[1]);
            v[1].color = col;
        }
        v[2] = pt(r, &pn.poly[1]);
        v[2].color = v[0].color;
        v[3] = pt(r, &pn.poly[0]);
        v[3].color = v[0].color;
        prev = [v[3], v[2]];
        v[4] = v[0];
        if v[0].sz != 0.0 && v[1].sz != 0.0 && v[2].sz != 0.0 && v[3].sz != 0.0 {
            let uvs = [(1.0, 1.0), (0.0, 1.0), (0.0, 0.0), (1.0, 0.0), (1.0, 1.0)];
            for (t, (u, w)) in v.iter_mut().zip(uvs) {
                t.tu = u;
                t.tv = w;
            }
            push(r, Frame::LateAdditive, [v[0], v[1], v[2]], tex);
            push(r, Frame::LateAdditive, [v[2], v[3], v[4]], tex);
        }
    }
}

/// Port of `DrawTongue` (pcgfx.c:742) / `CalcTongueNodes` (pcgfx.c:800).
fn draw_tongue(r: &mut Renderer, game: &Game, pl: usize) {
    let t = &game.tongue.tongue[pl];
    let index = (t.progress.wrapping_mul(f2_game::tongue::MAX_TONGUENODES as i32 - 1) >> 12).max(0) as usize;
    let Some(tex) = tex_of(r, game.specfx.textures.txtr_blank) else { return };
    let Some(frog) = game.frogger.frog[pl].and_then(|f| game.actor2.actors.get(f)) else { return };
    if index < 2 || t.segment.len() <= index {
        return;
    }
    let colour = argb(f2_game::tongue::TONGUE_COLOURS[(game.frogger.player[pl].character as usize).min(7)]);
    let cam = r.view.camera.map(|c| c.source).unwrap_or([0.0; 3]);
    let q_rot = frog.actor.q_rot;
    let nodes = |r: &Renderer, i: usize| -> [Tv; 2] {
        let pos = scale(rf(&t.segment[i]), 0.1);
        let normal = normalise(sub(cam, pos));
        let m = normal_rotation(normal);
        let mut out = [Tv::default(); 2];
        for (k, x) in [-2.93 + i as f32 * 0.25, 2.93 - i as f32 * 0.25].into_iter().enumerate() {
            let m1 = mtx_xfm(&m, [x, 0.0, 0.0]);
            // Rotate around the frog (fixed point).
            let p1 = FVector { vx: (m1[0] * 4096.0) as i32, vy: (m1[1] * 4096.0) as i32, vz: (m1[2] * 4096.0) as i32 };
            let mut p2 = FVector::default();
            f2_math::rotate_vector_by_quaternion_ff(&mut p2, &p1, &q_rot);
            let p = [p2.vx as f32 / 4096.0 + pos[0], p2.vy as f32 / 4096.0 + pos[1], p2.vz as f32 / 4096.0 + pos[2]];
            out[k] = match xfm(r, p) {
                Some(s) => Tv { sx: s[0], sy: s[1], sz: (s[2] + DIST) * SZ_SCALE, color: colour, ..Default::default() },
                None => Tv { color: colour, ..Default::default() },
            };
        }
        out
    };
    let mut prev = [Tv::default(); 2];
    for i in 0..index {
        let mut v = [Tv::default(); 4];
        if i != 0 && prev[0].sz != 0.0 && prev[1].sz != 0.0 {
            v[0] = prev[0];
            v[1] = prev[1];
        } else {
            let n = nodes(r, i);
            v[0] = n[0];
            v[1] = n[1];
        }
        let n = nodes(r, i + 1);
        v[2] = n[0];
        v[3] = n[1];
        prev = [v[2], v[3]];
        if v.iter().all(|x| x.sz != 0.0) {
            let uvs = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
            for (x, (u, w)) in v.iter_mut().zip(uvs) {
                x.tu = u;
                x.tv = w;
            }
            push(r, Frame::Late, [v[0], v[1], v[2]], tex);
            push(r, Frame::Late, [v[1], v[2], v[3]], tex);
        }
    }
}

// ------------------------------------------------------------------ sprites

/// Port of `PrintSprites` / `ZSortSpriteList` / `PrintSprite` (pcsprite.c:107/290/392).
/// `far_clip` is `farClip`. `posed` replaces sprites of the list by a render-side copy
/// (object sprites of actors the simulation culled but a widened Hor+ view shows, see
/// [`crate::render::widened_object_sprites`]); empty = retail.
pub fn print_sprites(r: &mut Renderer, game: &Game, far_clip: f32, posed: &BTreeMap<SpriteId, Sprite>) {
    if game.sprite.count == 0 {
        return;
    }
    // BUG(original): ZSortSpriteList transforms `(MDX_VECTOR *)&frog[0]->actor->position`,
    // an SVECTOR read as floats: tiny denormals, i.e. the world origin.
    let frog_z = if game.frogger.frog[0].is_some() { r.view.xfm_point([0.0; 3]).map_or(0.0, |m| m[2]) } else { 0.0 };
    let mut list: Vec<(f32, f32, f32, SprIdx)> = Vec::new();
    // Out of memory (consoles): no sprites this frame rather than an abort.
    if list.try_reserve(game.sprite.count.clamp(0, MAX_ARRAY_SPRITES as i32) as usize).is_err() {
        return;
    }
    for (id, s) in game.sprite.iter_list().take(MAX_ARRAY_SPRITES) {
        let s = posed.get(&id).unwrap_or(s);
        let p = [s.pos.vx as f32 * 0.1, s.pos.vy as f32 * 0.1, s.pos.vz as f32 * 0.1];
        let sc = r.view.xfm_point(p).unwrap_or([0.0; 3]);
        if sc[2] - frog_z < far_clip {
            list.push((sc[0], sc[1], sc[2], SprIdx(id.0)));
        }
    }
    // qsort by z ascending, drawn from the end (far to near).
    list.sort_by(|a, b| a.2.total_cmp(&b.2));
    let fov = r.view.fov;
    let sprite_mul = r.view.proj_scale();
    for &(sx, sy, sz, SprIdx(idx)) in list.iter().rev() {
        let id = f2_game::ids::SpriteId(idx);
        let s = posed.get(&id).unwrap_or_else(|| game.sprite.get(id));
        if s.draw == 0 || s.texture == 0 || s.scale_x == 0 || s.scale_y == 0 || sz == 0.0 {
            continue;
        }
        let Some(tex) = r.textures.current(s.texture) else { continue };
        let d = fov / (sz + DIST);
        let distx = d * (s.scale_x as f32 / 64.0) * sprite_mul;
        let disty = d * (s.scale_y as f32 / 64.0) * sprite_mul;
        let frame = if s.flags & SPRITE_ADDITIVE != 0 { Frame::LateAdditive } else { Frame::Late };
        let col = rgba(s.r as f32 / 255.0, s.g as f32 / 255.0, s.b as f32 / 255.0, s.a as f32 / 255.0);
        let x = sx + s.offset_x as f32 * distx;
        let y = sy + s.offset_y as f32 * disty;
        let z = sz * SZ_SCALE;
        let (xs, ys) = (32.0 * distx, 32.0 * disty);
        let (x2, y2) = (x + xs, y + ys);
        let mut v = [
            Tv { sx: x, sy: y, sz: z, tu: 0.0, tv: 0.0, color: col },
            Tv { sx: x2, sy: y, sz: z, tu: 1.0, tv: 0.0, color: col },
            Tv { sx: x2, sy: y2, sz: z, tu: 1.0, tv: 1.0, color: col },
            Tv { sx: x, sy: y2, sz: z, tu: 0.0, tv: 1.0, color: col },
        ];
        if s.flags & SPRITE_FLAGS_ROTATE != 0 {
            // DrawAlphaSpriteRotating (mdxSprite.cpp:107): rotate about the sprite centre point.
            let a = s.angle as f32 * FIXED_TO_RADS;
            let (sn, cs) = a.sin_cos();
            for t in &mut v {
                let (dx, dy) = (t.sx - sx, t.sy - sy);
                t.sx = dx * cs + dy * sn + sx;
                t.sy = dy * cs - dx * sn + sy;
            }
        }
        push(r, frame, [v[0], v[1], v[2]], tex);
        push(r, frame, [v[2], v[3], v[0]], tex);
    }
}

/// Sprite index carried through the sort.
#[derive(Clone, Copy)]
struct SprIdx(u32);
