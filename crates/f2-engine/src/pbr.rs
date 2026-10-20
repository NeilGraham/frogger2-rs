//! Physically based shading of modded models' glTF materials ([`crate::textures::Pbr`]) on the
//! GPU mesh pass (Enhanced / Path-Traced): normal, metallic-roughness, occlusion, emissive and
//! height maps, parallax occlusion mapping. Its WGSL is appended to the mesh shader only for
//! the PBR pipelines (compiled when a frame first has a modded material), so the other mesh
//! pipelines carry none of it.
//!
//! - Tangent frame: from screen derivatives of the world position and texture coordinates
//!   (∂P/∂u, ∂P/∂v per pixel; no tangent attribute, mirrored and skinned meshes alike).
//!   Normal maps are glTF / Blender's (OpenGL): +Y (green) points to −v (up in the image).
//! - Light: the mesh pass's inputs (`light()`: hemisphere ambient, sun with the shadow map and
//!   the ray-traced shadows, point lights, ray-traced ambient), Lambert diffuse in the game's
//!   units (no 1/π: a white surface facing the sun gets the sun's colour) and GGX specular to
//!   match, an environment of the sky above and the ground below (a split-sum approximation),
//!   so metals show the bright cartoon sky instead of going black. Occlusion darkens the
//!   ambient and the environment. Colours stay in the game's gamma space like the rest.
//! - Parallax occlusion: linear search of 8 (seen head on) to 32 (grazing) steps, then a
//!   secant step; the sun's light is shadowed by the height field (8 steps). Depth is not
//!   corrected (the silhouette stays the mesh's).
//! - Smart tiling ([`f2_formats::material::Tiling`]): each repeat of the texture (one cell of
//!   its texture grid) is a tile with its own look from a seed: turned / mirrored maps, tint,
//!   a tilted, sunk setting, and wear made up on the spot (chipped corners, nicks in the
//!   edges, a crack, pits, dirt in them), all in the height field (so the parallax, its
//!   shadows and the normals show them). The mesh tells each tile what its sides meet
//!   ([`cell_codes`]): a drop wears them more, a wall gets grime along them.

/// Pipeline kind of the PBR mesh pipelines (`Gpu::pipelines`).
pub(crate) const PBR_KIND: u8 = 16;

/// Map bits of `Pbr.maps.x`.
pub(crate) const HAS_NORMAL: u32 = 1;
pub(crate) const HAS_MR: u32 = 2;
pub(crate) const HAS_AO: u32 = 4;
pub(crate) const HAS_EMISSIVE: u32 = 8;
pub(crate) const HAS_HEIGHT: u32 = 16;

/// The material uniform block of `p` (`Pbr` in [`WGSL`]).
pub(crate) fn uniform(p: &crate::textures::Pbr) -> [f32; 24] {
    let bits = p.normal.map_or(0, |_| HAS_NORMAL)
        | p.metal_rough.map_or(0, |_| HAS_MR)
        | p.occlusion.map_or(0, |_| HAS_AO)
        | p.emissive_map.map_or(0, |_| HAS_EMISSIVE)
        | p.height.filter(|_| p.parallax_scale > 0.0).map_or(0, |_| HAS_HEIGHT);
    let [r, g, b, a] = p.base_color;
    let [er, eg, eb] = p.emissive;
    let t = &p.tiling;
    let turn = t.variants.min(2) as f32 + if t.turn_color { 4.0 } else { 0.0 };
    #[rustfmt::skip]
    let u = [
        r, g, b, a, er, eg, eb, p.parallax_scale, p.metallic, p.roughness, p.normal_scale, p.occlusion_strength, bits as f32, 0.0, 0.0, 0.0,
        turn, t.wear, t.tint, t.uneven, t.edges, t.grime, if t.is_on() { 1.0 } else { 0.0 }, 0.0,
    ];
    u
}

pub(crate) const WGSL: &str = r#"
// PBR-BEGIN (crate::pbr)
struct Pbr {
    base: vec4<f32>,      // baseColorFactor
    emissive: vec4<f32>,  // emissive factor; w: parallax scale
    mr: vec4<f32>,        // metallic, roughness, normal scale, occlusion strength
    maps: vec4<f32>,      // x: which maps (crate::pbr::HAS_*)
    tile: vec4<f32>,      // smart tiling: turns (0 none, 1 mirror, 2 quarter turns; +4 the colour too), wear, tint, uneven
    tile2: vec4<f32>,     // edges, grime, on
};
@group(1) @binding(2) var t_normal: texture_2d<f32>;
@group(1) @binding(3) var t_mr: texture_2d<f32>;
@group(1) @binding(4) var t_ao: texture_2d<f32>;
@group(1) @binding(5) var t_emit: texture_2d<f32>;
@group(1) @binding(6) var t_height: texture_2d<f32>;
@group(1) @binding(7) var<uniform> pbr: Pbr;

const PI: f32 = 3.14159265;

fn has_map(bit: u32) -> bool {
    return (u32(pbr.maps.x) & bit) != 0u;
}

// Smart tiling (crate::pbr): one repeat of the texture, a tile, and what it looks like.
struct Cell {
    on: bool,
    // Its turn (of the tile's coordinates about its middle, `tile_uv`).
    m: mat2x2<f32>,
    // Two bits a side (u-, u+, v-, v+): 0 a tile flush with it (or not known), 1 a drop, 2 a
    // wall, 3 nothing.
    sides: u32,
    seed: u32,
    // Chipped corners (0,0), (1,0), (0,1), (1,1): how far in (0: whole); the break's lean.
    chip: vec4<f32>,
    chipa: vec4<f32>,
    // Nicks in the sides u-, u+, v-, v+: where along; how big (0: none).
    nick: vec4<f32>,
    nickr: vec4<f32>,
    // A crack from xy to zw by way of `crackm.xy`, `crackm.z` wide (0: none).
    crack: vec4<f32>,
    crackm: vec4<f32>,
    // Pits: xy, radius (0: none).
    pit: vec4<f32>,
    pit2: vec4<f32>,
    // Setting: tilt (x, y), sunk (z: a little darker, dirt gathers); its face worn unevenly (w).
    lay: vec4<f32>,
    // Colour ×.
    tint: vec3<f32>,
};

// The tile being shaded (`make_cell`; one per fragment, read by the functions below).
var<private> tc: Cell;

fn tile_pcg(v: u32) -> u32 {
    let st = v * 747796405u + 2891336453u;
    let w = ((st >> ((st >> 28u) + 4u)) ^ st) * 277803737u;
    return (w >> 22u) ^ w;
}

// Random 0..1: the `k`th of `seed`.
fn tile_rnd(seed: u32, k: u32) -> f32 {
    return f32(tile_pcg(seed ^ tile_pcg(k + 0x9e37u)) >> 8u) / 16777216.0;
}

// A lattice point's value (one hash: the noise is in the parallax's loop).
fn tile_lattice(seed: u32, i: vec2<f32>) -> f32 {
    let k = (u32(i32(i.x)) & 1023u) | ((u32(i32(i.y)) & 1023u) << 10u);
    return f32(tile_pcg(seed ^ (k * 2654435769u)) >> 8u) / 16777216.0;
}

// Value noise 0..1.
fn tile_vnoise(p: vec2<f32>, seed: u32) -> f32 {
    let i = floor(p);
    let f = p - i;
    let u = f * f * (3.0 - 2.0 * f);
    let a = tile_lattice(seed, i);
    let b = tile_lattice(seed, i + vec2<f32>(1.0, 0.0));
    let c = tile_lattice(seed, i + vec2<f32>(0.0, 1.0));
    let d = tile_lattice(seed, i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// Distance from p to the segment a-b.
fn tile_seg(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let ab = b - a;
    let t = clamp(dot(p - a, ab) / max(dot(ab, ab), 1e-8), 0.0, 1.0);
    return length(p - a - ab * t);
}

// What side `k` (u-, u+, v-, v+) of a tile meets.
fn tile_side_kind(k: u32) -> u32 {
    return (tc.sides >> (k * 2u)) & 3u;
}

fn tile_side_kind_of(c: Cell, k: u32) -> u32 {
    return (c.sides >> (k * 2u)) & 3u;
}

fn tile_exposed_of(c: Cell, k: u32) -> bool {
    let kind = tile_side_kind_of(c, k);
    return kind == 1u || kind == 3u;
}

// Side k is an outer edge (a drop, or nothing there).
fn tile_exposed(k: u32) -> bool {
    let kind = tile_side_kind(k);
    return kind == 1u || kind == 3u;
}

// How far p (the tile's 0..1) is from side k.
fn tile_side_dist(p: vec2<f32>, k: u32) -> f32 {
    switch k {
        case 0u: { return p.x; }
        case 1u: { return 1.0 - p.x; }
        case 2u: { return p.y; }
        default: { return 1.0 - p.y; }
    }
}

// A point of side k, `t` along it.
fn tile_side_point(k: u32, t: f32) -> vec2<f32> {
    switch k {
        case 0u: { return vec2<f32>(0.0, t); }
        case 1u: { return vec2<f32>(1.0, t); }
        case 2u: { return vec2<f32>(t, 0.0); }
        default: { return vec2<f32>(t, 1.0); }
    }
}

// The tile under texture coordinate `uv` (the mesh's code: `crate::pbr::cell_codes`).
fn make_cell(code: u32, uv: vec2<f32>) {
    var c: Cell;
    c.m = mat2x2<f32>(1.0, 0.0, 0.0, 1.0);
    c.tint = vec3<f32>(1.0);
    c.on = pbr.tile2.z > 0.5;
    if (!c.on) {
        tc = c;
        return;
    }
    let known = (code >> 23u) & 1u;
    c.sides = select(0u, code & 255u, known == 1u);
    let b = floor(uv);
    c.seed = tile_pcg(((code >> 8u) & 0x7fffu) * 0x10001u ^ tile_pcg(u32(i32(b.x)) * 1973u ^ u32(i32(b.y)) * 9277u));
    let sd = c.seed;
    let wear = pbr.tile.y;
    let edges = pbr.tile2.x;
    // Turn: bit 0 mirrors u, bit 1 swaps u and v, bit 2 mirrors v.
    let turns = u32(pbr.tile.x) & 3u;
    var k = tile_pcg(sd ^ 0x51u);
    if (turns == 1u) { k &= 5u; } else if (turns == 0u) { k = 0u; }
    var m = mat2x2<f32>(select(1.0, -1.0, (k & 1u) != 0u), 0.0, 0.0, select(1.0, -1.0, (k & 4u) != 0u));
    if ((k & 2u) != 0u) { m = mat2x2<f32>(0.0, 1.0, 1.0, 0.0) * m; }
    c.m = m;
    // Corners: chipped now and then, more where a side drops away.
    let corner_sides = array<vec2<u32>, 4>(vec2<u32>(0u, 2u), vec2<u32>(1u, 2u), vec2<u32>(0u, 3u), vec2<u32>(1u, 3u));
    for (var j = 0u; j < 4u; j++) {
        let cs = corner_sides[j];
        let drop = f32(tile_exposed_of(c, cs.x)) + f32(tile_exposed_of(c, cs.y));
        let p = wear * 0.3 + drop * edges * 0.3;
        if (tile_rnd(sd, 10u + j) < p) {
            c.chip[j] = mix(0.06, 0.14 + 0.12 * wear, tile_rnd(sd, 20u + j)) * (1.0 + 0.3 * drop);
            c.chipa[j] = (tile_rnd(sd, 30u + j) - 0.5) * 1.1;
        }
    }
    // Nicks along the sides.
    for (var j = 0u; j < 4u; j++) {
        let drop = tile_exposed_of(c, j);
        let p = select(wear * 0.3, 0.25 + edges * 0.6, drop);
        if (tile_rnd(sd, 40u + j) < p) {
            c.nick[j] = mix(0.18, 0.82, tile_rnd(sd, 50u + j));
            c.nickr[j] = mix(0.025, 0.05 + 0.04 * wear, tile_rnd(sd, 60u + j)) * select(1.0, 1.4, drop);
        }
    }
    // A crack across, side to side.
    if (tile_rnd(sd, 70u) < wear * 0.4) {
        let s0 = tile_pcg(sd ^ 0x71u) & 3u;
        let s1 = (s0 + 1u + (tile_pcg(sd ^ 0x72u) % 3u)) & 3u;
        let a = tile_side_point(s0, mix(0.15, 0.85, tile_rnd(sd, 73u)));
        let e = tile_side_point(s1, mix(0.15, 0.85, tile_rnd(sd, 74u)));
        // Not all the way across, sometimes.
        let stop = select(1.0, mix(0.45, 0.8, tile_rnd(sd, 75u)), tile_rnd(sd, 76u) < 0.5);
        let mid = mix(a, e, 0.5 * stop) + vec2<f32>(tile_rnd(sd, 77u) - 0.5, tile_rnd(sd, 78u) - 0.5) * 0.18;
        c.crack = vec4<f32>(a, mix(a, e, stop));
        c.crackm = vec4<f32>(mid, mix(0.012, 0.022, tile_rnd(sd, 79u)), 0.0);
    }
    // Pits.
    if (tile_rnd(sd, 80u) < wear * 0.6) { c.pit = vec4<f32>(tile_rnd(sd, 81u) * 0.7 + 0.15, tile_rnd(sd, 82u) * 0.7 + 0.15, mix(0.012, 0.03, tile_rnd(sd, 83u)), 0.0); }
    if (tile_rnd(sd, 84u) < wear * 0.35) { c.pit2 = vec4<f32>(tile_rnd(sd, 85u) * 0.7 + 0.15, tile_rnd(sd, 86u) * 0.7 + 0.15, mix(0.01, 0.022, tile_rnd(sd, 87u)), 0.0); }
    // Setting.
    let un = pbr.tile.w;
    c.lay = vec4<f32>((tile_rnd(sd, 90u) - 0.5) * un * 0.3, (tile_rnd(sd, 91u) - 0.5) * un * 0.3, tile_rnd(sd, 92u) * un * 0.12, wear * 0.12);
    // Colour: lighter or darker, warmer or cooler.
    let tn = pbr.tile.z;
    let lum = 1.0 + (tile_rnd(sd, 93u) - 0.5) * 0.24 * tn;
    let warm = (tile_rnd(sd, 94u) - 0.5) * 2.0 * tn;
    c.tint = lum * vec3<f32>(1.0 + 0.05 * warm, 1.0 + 0.01 * warm, 1.0 - 0.07 * warm);
    tc = c;
}

// Where the maps of the tile under uv are read (turned about the tile's middle).
fn tile_uv(uv: vec2<f32>) -> vec2<f32> {
    if (!tc.on) { return uv; }
    let b = floor(uv);
    return b + vec2<f32>(0.5) + tc.m * (uv - b - vec2<f32>(0.5));
}

// How much of the tile's crack is at p (its 0..1; 0 none, 1 its middle).
fn tile_crack(p: vec2<f32>) -> f32 {
    if (tc.crackm.z <= 0.0) { return 0.0; }
    // Nowhere near it (most of the tile): no noise to make.
    let lo = min(min(tc.crack.xy, tc.crack.zw), tc.crackm.xy) - vec2<f32>(0.06);
    let hi = max(max(tc.crack.xy, tc.crack.zw), tc.crackm.xy) + vec2<f32>(0.06);
    if (any(p < lo) || any(p > hi)) { return 0.0; }
    let w = tc.crackm.z;
    // Wandering off the straight line, a little wider and narrower along it.
    let q = p + (vec2<f32>(tile_vnoise(p * 13.0, tc.seed ^ 0x77u), tile_vnoise(p * 13.0, tc.seed ^ 0x78u)) - 0.5) * 0.035;
    let d = min(tile_seg(q, tc.crack.xy, tc.crackm.xy), tile_seg(q, tc.crackm.xy, tc.crack.zw));
    // V-shaped: its walls slope down to the dark middle.
    return clamp(1.0 - d / (w * mix(0.7, 1.4, tile_vnoise(p * 30.0, tc.seed ^ 0x79u))), 0.0, 1.0);
}

// The tile's height at uv from the map's `hm` (0..1, white high): x the height, y dirt (0..1:
// cracks, pits, along walls), z fresh breaks (0..1: chips, nicks).
fn tile_h(uv: vec2<f32>, hm: f32) -> vec3<f32> {
    if (!tc.on) { return vec3<f32>(hm, 0.0, 0.0); }
    let p = fract(uv);
    let face = smoothstep(0.2, 0.75, hm);
    var h = hm;
    var dirt = 0.0;
    var fresh = 0.0;
    let ragged = tile_vnoise(p * 34.0, tc.seed ^ 0x99u) - 0.5;
    // Chipped corners: broken off along a ragged line, the break sloping down to the corner.
    for (var j = 0u; j < 4u; j++) {
        if (tc.chip[j] <= 0.0) { continue; }
        let corner = vec2<f32>(f32(j & 1u), f32(j >> 1u));
        let a = tc.chipa[j];
        let d0 = vec2<f32>(0.5) - corner;
        let dir = normalize(vec2<f32>(d0.x * cos(a) - d0.y * sin(a), d0.x * sin(a) + d0.y * cos(a)));
        let sd = dot(p - corner, dir) - tc.chip[j] + ragged * 0.03;
        let lip = 0.9 + sd * 6.0;
        if (lip < h) {
            fresh = max(fresh, clamp((h - lip) * 2.5, 0.0, 1.0));
            h = min(h, max(lip, 0.03));
        }
    }
    // Nicks: small bowls out of the edges.
    for (var j = 0u; j < 4u; j++) {
        if (tc.nickr[j] <= 0.0) { continue; }
        let d = length(p - tile_side_point(j, tc.nick[j])) + ragged * 0.015;
        let bowl = 0.9 - (tc.nickr[j] - d) * 10.0;
        if (bowl < h) {
            fresh = max(fresh, clamp((h - bowl) * 2.5, 0.0, 1.0));
            h = min(h, max(bowl, 0.04));
        }
    }
    // The crack: a thin wavering groove, dirty (shallow here: too fine for the parallax's
    // steps; `tile_extra` deepens it for the normals).
    let g = tile_crack(p);
    h -= g * 0.15 * face;
    dirt = max(dirt, g * 0.9);
    // Pits.
    for (var j = 0u; j < 2u; j++) {
        let pt = select(tc.pit2, tc.pit, j == 0u);
        if (pt.z <= 0.0) { continue; }
        let g = 1.0 - smoothstep(pt.z * 0.3, pt.z, length(p - pt.xy) + ragged * pt.z * 0.5);
        h -= g * 0.35 * face;
        dirt = max(dirt, g * 0.7);
    }
    // The sides: worn round where they drop away, grimy along walls.
    for (var j = 0u; j < 4u; j++) {
        let kind = tile_side_kind(j);
        let d = tile_side_dist(p, j);
        if (tile_exposed(j)) {
            h = min(h, 0.25 + (d + ragged * 0.02) * (14.0 - 7.0 * pbr.tile2.x));
        } else if (kind == 2u) {
            dirt = max(dirt, (1.0 - smoothstep(0.0, 0.22, d + ragged * 0.05)) * 0.8 * pbr.tile2.y);
        }
    }
    return vec3<f32>(clamp(h, 0.0, 1.0), dirt, fresh);
}

// The height map at uv (the tile's turn).
fn height_map(uv: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>) -> f32 {
    return textureSampleGrad(t_height, s, tile_uv(uv), tc.m * dx, tc.m * dy).r;
}

// What the tile adds to the height map's height at uv.
fn tile_extra(uv: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>) -> f32 {
    let hm = height_map(uv, dx, dy);
    // With the stone's grain (the normals only: too fine for the parallax).
    let p = fract(uv);
    let grain = tile_vnoise(p * 70.0, tc.seed ^ 0x3du) * 0.6 + tile_vnoise(p * 150.0, tc.seed ^ 0x3eu) * 0.4;
    // The setting too: too slight to show in the parallax (and a face below the top would cost
    // the parallax its quick way out on every flat face), it shows in the light.
    let lay = dot(tc.lay.xy, p - vec2<f32>(0.5)) + (tile_vnoise(p * 6.0, tc.seed) - 0.5) * tc.lay.w;
    return tile_h(uv, hm).x - hm + (lay + grain * 0.012 - tile_crack(p) * 0.3) * smoothstep(0.2, 0.75, hm);
}

// How deep the height field is at uv (0: its top; the map's white is high).
fn pom_depth(uv: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>) -> f32 {
    return 1.0 - tile_h(uv, height_map(uv, dx, dy)).x;
}

// Where the view ray first meets the height field under uv, and how deep (z). `vt`: the way to
// the eye in the surface's frame (x along +u, y along +v, z out of the surface).
fn parallax(uv: vec2<f32>, vt: vec3<f32>, dx: vec2<f32>, dy: vec2<f32>) -> vec3<f32> {
    let n = mix(32.0, 8.0, clamp(vt.z, 0.0, 1.0));
    let step = 1.0 / n;
    // Texture coordinates per unit of depth.
    let delta = -vt.xy / max(vt.z, 0.08) * pbr.emissive.w;
    var prev_uv = uv;
    var prev_d = 0.0;
    var prev_h = pom_depth(uv, dx, dy);
    var cur_uv = uv;
    var cur_d = 0.0;
    var cur_h = prev_h;
    if (cur_h <= 0.0) { return vec3<f32>(uv, 0.0); }
    for (var k = 0; k < 32; k++) {
        if (f32(k) >= n) { break; }
        prev_uv = cur_uv;
        prev_d = cur_d;
        prev_h = cur_h;
        cur_d += step;
        cur_uv = uv + delta * cur_d;
        cur_h = pom_depth(cur_uv, dx, dy);
        if (cur_h <= cur_d) { break; }
    }
    // The secant between the last two: where the ray's depth crosses the field's.
    let a = prev_h - prev_d;
    let b = cur_h - cur_d;
    let t = clamp(a / max(a - b, 1e-5), 0.0, 1.0);
    return vec3<f32>(mix(prev_uv, cur_uv, t), mix(prev_d, cur_d, t));
}

// Light reaching depth d at uv along `lt_t` (to the light, the surface's frame) past the field.
fn parallax_shadow(uv: vec2<f32>, d: f32, lt_t: vec3<f32>, dx: vec2<f32>, dy: vec2<f32>) -> f32 {
    if (d <= 0.0 || lt_t.z <= 0.0) { return 1.0; }
    let delta = lt_t.xy / max(lt_t.z, 0.08) * pbr.emissive.w;
    var block = 0.0;
    for (var k = 1; k <= 8; k++) {
        let f = f32(k) / 8.0;
        let dk = d * (1.0 - f);
        let over = dk - pom_depth(uv + delta * (d - dk), dx, dy);
        block = max(block, over * (1.0 - f) * 6.0);
    }
    return 1.0 - clamp(block, 0.0, 1.0);
}

// GGX specular (Smith-Schlick visibility, Schlick Fresnel) × π (the diffuse has no 1/π).
fn ggx(n: vec3<f32>, v: vec3<f32>, l: vec3<f32>, a: f32, f0: vec3<f32>) -> vec3<f32> {
    let h = normalize(l + v);
    let ndl = max(dot(n, l), 0.0);
    let ndv = max(dot(n, v), 1e-3);
    let ndh = max(dot(n, h), 0.0);
    let a2 = a * a;
    let q = ndh * ndh * (a2 - 1.0) + 1.0;
    let d = a2 / (PI * q * q);
    let k = a * 0.5;
    let vis = 0.25 / ((ndl * (1.0 - k) + k) * (ndv * (1.0 - k) + k));
    let f = f0 + (1.0 - f0) * pow(1.0 - max(dot(v, h), 0.0), 5.0);
    return min(d * vis * PI, 64.0) * f;
}

// The environment's split-sum BRDF (Karis' fit): what of the environment a surface reflects.
fn env_brdf(f0: vec3<f32>, rough: f32, ndv: f32) -> vec3<f32> {
    let r = rough * vec4<f32>(-1.0, -0.0275, -0.572, 0.022) + vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let a004 = min(r.x * r.x, exp2(-9.28 * ndv)) * r.x + r.y;
    let ab = vec2<f32>(-1.04, 1.04) * a004 + r.zw;
    return f0 * ab.x + ab.y;
}

// The surface lit as `light()` lights the others, physically based. `n`: the shading normal;
// `sun_sh`: the height field's shadow on the sun.
fn light_pbr(i: VOut, albedo: vec3<f32>, n: vec3<f32>, metal: f32, rough: f32, ao: f32, sun_sh: f32) -> vec3<f32> {
    let v = normalize(lt.eye.xyz - i.world);
    let seed = i.pos.xy;
    let q = i.world - vec3<f32>(0.0, i.lift, 0.0);
    let amb = rt_ambient(q, n, seed);
    let hemi = mix(lt.ground.rgb, lt.sky.rgb, n.y * 0.5 + 0.5) * amb.a + amb.rgb;
    let a = max(rough * rough, 0.002);
    let f0 = mix(vec3<f32>(0.04), albedo, metal);
    let diffuse = albedo * (1.0 - metal);
    let ndv = max(dot(n, v), 1e-3);
    var l = vec3<f32>(0.0);
    var spec = vec3<f32>(0.0);
    let sun_on = lt.sun_dir.w;
    if (sun_on > 0.5) {
        let sd = lt.sun_dir.xyz;
        let sh = min(shadow(q + n * 2.0), rt_sun(q, n, seed)) * sun_sh;
        let ndl = max(dot(n, sd), 0.0);
        l += lt.sun_color.rgb * ndl * sh;
        l += lt.sun_color.rgb * rt_caustics(i.world, ndl) * sh;
        spec += lt.sun_color.rgb * ggx(n, v, sd, a, f0) * ndl * sh;
    } else {
        l += lt.sky.rgb * caustics(i.world, vec3<f32>(0.0, 1.0, 0.0)) * 0.8;
    }
    let np = u32(lt.ground.w);
    for (var k = 0u; k < np; k++) {
        let p = points[k];
        let d = p.pos.xyz - i.world;
        let dist = length(d);
        let att = clamp(1.0 - dist / p.pos.w, 0.0, 1.0);
        if (att <= 0.0) { continue; }
        let dl = d / max(dist, 0.001);
        let w = clamp(dot(n, dl) * 0.75 + 0.25, 0.0, 1.0);
        let c = p.color.rgb * att * att * rt_point(q, n, p.pos.xyz, k, seed);
        l += c * w;
        spec += c * ggx(n, v, dl, a, f0) * max(dot(n, dl), 0.0);
    }
    // The environment: the sky above and the ground below, both lit by the sun (a sunny
    // cartoon world is bright all round); rough surfaces see the ambient's blur of it.
    let r = reflect(-v, n);
    let up = lt.sky.rgb + lt.sun_color.rgb * (0.35 * sun_on);
    let down = lt.ground.rgb + lt.sun_color.rgb * (0.15 * sun_on);
    var env = mix(down, up, smoothstep(-0.25, 0.25, r.y));
    env += lt.sun_color.rgb * pow(max(dot(r, lt.sun_dir.xyz), 0.0), mix(400.0, 4.0, rough)) * (sun_on * (1.0 - rough));
    env = mix(env * amb.a + amb.rgb, hemi, rough * rough);
    // The baked colours are the art's occlusion: dark corners get no highlight either.
    let baked = dot(i.color.rgb, vec3<f32>(1.0 / 3.0));
    return diffuse * (hemi * ao + l) + (spec + env * env_brdf(f0, rough, ndv) * ao) * mix(1.0, baked, 0.5);
}

fn shade_pbr(i: VOut) -> vec4<f32> {
    // Derivatives first (uniform control flow).
    let sx = dpdx(i.world);
    let sy = dpdy(i.world);
    let dx = dpdx(i.uv);
    let dy = dpdy(i.uv);
    var face = cross(sx, sy);
    face = select(normalize(i.normal), normalize(face), dot(face, face) > 1e-12);
    if (dot(face, lt.eye.xyz - i.world) < 0.0) { face = -face; }
    if ((i.flags & 1u) == 0u && i.vz - 10.0 >= cam.fog.w) { discard; }
    // The smoothed normal, mirrored when it points behind the face (as `light()`).
    var n = normalize(i.normal);
    let behind = dot(n, face);
    if (behind < 0.0) { n = normalize(n - face * (2.0 * behind)); }
    let v = normalize(lt.eye.xyz - i.world);
    // ∂P/∂u and ∂P/∂v in the surface's plane.
    let det = dx.x * dy.y - dy.x * dx.y;
    var tu = vec3<f32>(0.0);
    var tv = vec3<f32>(0.0);
    var framed = false;
    if (abs(det) > 1e-14) {
        tu = (sx * dy.y - sy * dx.y) / det;
        tv = (sy * dx.x - sx * dy.x) / det;
        tu -= n * dot(n, tu);
        tv -= n * dot(n, tv);
        if (dot(tu, tu) > 1e-20 && dot(tv, tv) > 1e-20) {
            tu = normalize(tu);
            tv = normalize(tv);
            framed = true;
        }
    }
    var uv = i.uv;
    var depth = 0.0;
    make_cell(i.cell, i.uv);
    // The maps' derivatives in the tile's turn.
    let mdx = tc.m * dx;
    let mdy = tc.m * dy;
    if (framed && has_map(16u)) {
        let hit = parallax(uv, vec3<f32>(dot(v, tu), dot(v, tv), dot(v, n)), dx, dy);
        uv = hit.xy;
        depth = hit.z;
    }
    let muv = tile_uv(uv);
    let turn_color = (u32(pbr.tile.x) & 4u) != 0u;
    var c = textureSampleGrad(t, s, select(uv, muv, turn_color), select(dx, mdx, turn_color), select(dy, mdy, turn_color)) * i.color * pbr.base;
    if (floor(c.a * 255.0 + 0.5) <= cam.depth.y) { discard; }
    // The tile's own colour: its tint, the stone's grain, breaks and dirt darker.
    var dirt = 0.0;
    var tile_grad = vec2<f32>(0.0);
    if (tc.on) {
        let hm = height_map(uv, dx, dy);
        let th = tile_h(uv, hm);
        dirt = th.y;
        // Grain: the stone's own fine texture where the art is a blur up close.
        let p = fract(uv);
        let grain = tile_vnoise(p * 70.0, tc.seed ^ 0x3du) * 0.6 + tile_vnoise(p * 150.0, tc.seed ^ 0x3eu) * 0.4 - 0.5;
        let broken = th.z * (0.82 + grain * 0.5);
        c = vec4<f32>(c.rgb * tc.tint * (1.0 - tc.lay.z) * (1.0 + grain * 0.07) * mix(1.0, 0.72, broken) * mix(vec3<f32>(1.0), vec3<f32>(0.42, 0.36, 0.3), dirt), c.a);
        // The slope the tile adds to the map's (its turn, setting and wear), half a texel each way.
        let e = 0.6 / f32(textureDimensions(t).x);
        let ox = vec2<f32>(e, 0.0);
        let oy = vec2<f32>(0.0, e);
        let px = tile_extra(uv + ox, dx, dy);
        let nx = tile_extra(uv - ox, dx, dy);
        let py = tile_extra(uv + oy, dx, dy);
        let ny = tile_extra(uv - oy, dx, dy);
        tile_grad = vec2<f32>(px - nx, py - ny) / (2.0 * e);
    }
    var emissive = pbr.emissive.rgb;
    if (has_map(8u)) { emissive *= textureSampleGrad(t_emit, s, muv, mdx, mdy).rgb; }
    if (lt.sky.w > 0.5 && (i.flags & 1u) == 0u) {
        if (framed && (has_map(1u) || tc.on)) {
            var m = vec3<f32>(0.0, 0.0, 1.0);
            if (has_map(1u)) {
                m = textureSampleGrad(t_normal, s, muv, mdx, mdy).xyz * 2.0 - 1.0;
                m = vec3<f32>(m.xy * pbr.mr.z, m.z);
            }
            // Back from the tile's turn (in u, v: the map's green is -v), with the tile's slope.
            let g = transpose(tc.m) * vec2<f32>(m.x, -m.y) - tile_grad * (pbr.emissive.w * 2.5);
            m = vec3<f32>(g.x, -g.y, m.z);
            // Green is up in the image: towards -v.
            let bent = tu * m.x - tv * m.y + n * m.z;
            if (dot(bent, bent) > 1e-12) { n = normalize(bent); }
        }
        var metal = pbr.mr.x;
        var rough = pbr.mr.y;
        if (has_map(2u)) {
            let mr = textureSampleGrad(t_mr, s, muv, mdx, mdy);
            rough *= mr.g;
            metal *= mr.b;
        }
        var ao = 1.0;
        if (has_map(4u)) { ao = mix(1.0, textureSampleGrad(t_ao, s, muv, mdx, mdy).r, pbr.mr.w); }
        rough = mix(rough, 1.0, dirt);
        ao *= 1.0 - 0.35 * dirt;
        var sun_sh = 1.0;
        if (framed && has_map(16u) && lt.sun_dir.w > 0.5) {
            let sd = lt.sun_dir.xyz;
            sun_sh = parallax_shadow(uv, depth, vec3<f32>(dot(sd, tu), dot(sd, tv), dot(sd, n)), dx, dy);
        }
        c = vec4<f32>(light_pbr(i, c.rgb, n, clamp(metal, 0.0, 1.0), clamp(rough, 0.045, 1.0), ao, sun_sh) + emissive, c.a);
    } else {
        c = vec4<f32>(c.rgb + emissive, c.a);
    }
    if (cam.fog.x > 0.5) { c = vec4<f32>(mix(cam.fog_color.rgb, c.rgb, i.fog), c.a); }
    return c;
}

@fragment fn fs_pbr(i: VOut) -> @location(0) vec4<f32> {
    return shade_pbr(i);
}

@fragment fn fs_pbr_mrt(i: VOut) -> FOut {
    var o: FOut;
    o.c = shade_pbr(i);
    let half = vec2<f32>(cam.size.x * 0.5, -cam.size.y * 0.5);
    o.mv = (i.prev.xy / i.prev.w - i.cur.xy / i.cur.w) * half;
    o.d = clamp(cam.depth.x / (i.vz - 10.0), 0.0, 1.0);
    return o;
}
// PBR-END
"#;

/// One triangle for [`cell_codes`]: its corners (model space), their texture coordinates and
/// normals, and its texture.
pub(crate) struct Tri {
    pub pos: [[f32; 3]; 3],
    pub uv: [[f32; 2]; 3],
    pub normal: [f32; 3],
    pub crc: u32,
}

/// What a smart-tiled texture's tiles are, for the shader (`make_cell`; carried in the vertex
/// normal's w): a code per triangle. The triangles of one texture joined by edges that carry
/// their texture coordinates on are one patch; a patch of at most one repeat of the texture is
/// a tile (the retail landscape lays its paving that way: a quad a slab), whose sides are told
/// what they meet across them: another face flush with it (0), one going down (1: a drop) or
/// up (2: a wall), or nothing (3). Bits 0..7: the four sides (u-, u+, v-, v+; 0 for a patch of
/// many repeats), 8..22: the patch's seed (from where it is in the model: the same every run,
/// whatever moves it), 23: set.
pub(crate) fn cell_codes(tris: &[Tri]) -> Vec<u32> {
    use std::collections::HashMap;
    type Q = (i32, i32, i32);
    let q = |p: [f32; 3]| -> Q { ((p[0] * 16.0).round() as i32, (p[1] * 16.0).round() as i32, (p[2] * 16.0).round() as i32) };
    let key = |a: Q, b: Q| if a < b { (a, b) } else { (b, a) };
    let mut edges: HashMap<(Q, Q), Vec<(u32, u8)>> = HashMap::with_capacity(tris.len() * 3);
    for (f, t) in tris.iter().enumerate() {
        for e in 0..3 {
            let (a, b) = (q(t.pos[e]), q(t.pos[(e + 1) % 3]));
            if a != b {
                edges.entry(key(a, b)).or_default().push((f as u32, e as u8));
            }
        }
    }
    // Patches: union-find over edges whose two sides agree on texture and coordinates.
    let mut parent: Vec<u32> = (0..tris.len() as u32).collect();
    fn find(p: &mut [u32], mut x: u32) -> u32 {
        while p[x as usize] != x {
            p[x as usize] = p[p[x as usize] as usize];
            x = p[x as usize];
        }
        x
    }
    let uv_at = |t: &Tri, p: Q| (0..3).find(|&k| q(t.pos[k]) == p).map(|k| t.uv[k]);
    let near = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3;
    for ((a, b), fs) in &edges {
        for (i, &(f, _)) in fs.iter().enumerate() {
            for &(g, _) in &fs[i + 1..] {
                let (tf, tg) = (&tris[f as usize], &tris[g as usize]);
                let same = tf.crc == tg.crc && [*a, *b].iter().all(|&p| matches!((uv_at(tf, p), uv_at(tg, p)), (Some(x), Some(y)) if near(x, y)));
                if same {
                    let (rf, rg) = (find(&mut parent, f), find(&mut parent, g));
                    if rf != rg {
                        parent[rf as usize] = rg;
                    }
                }
            }
        }
    }
    let root: Vec<u32> = (0..tris.len() as u32).map(|f| find(&mut parent, f)).collect();
    // Each patch's coordinates' extent, centre, and facing.
    struct Patch {
        lo: [f32; 2],
        hi: [f32; 2],
        sum: [f32; 3],
        n: f32,
        normal: [f32; 3],
        sides: [Option<u8>; 4],
    }
    let mut patches: HashMap<u32, Patch> = HashMap::new();
    let cross = |t: &Tri| {
        let (a, b, c) = (t.pos[0], t.pos[1], t.pos[2]);
        let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
        let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        // Facing up if it is more floor than wall (+y is up; the art's normals are not always
        // the right way round), else the way its normals do.
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        let s = if n[1].abs() > 0.3 * len {
            n[1].signum()
        } else if n[0] * t.normal[0] + n[1] * t.normal[1] + n[2] * t.normal[2] < 0.0 {
            -1.0
        } else {
            1.0
        };
        n.map(|x| x * s)
    };
    for (f, t) in tris.iter().enumerate() {
        let p = patches.entry(root[f]).or_insert(Patch { lo: [f32::MAX; 2], hi: [f32::MIN; 2], sum: [0.0; 3], n: 0.0, normal: [0.0; 3], sides: [None; 4] });
        let n = cross(t);
        for k in 0..3 {
            for a in 0..2 {
                p.lo[a] = p.lo[a].min(t.uv[k][a]);
                p.hi[a] = p.hi[a].max(t.uv[k][a]);
            }
            for a in 0..3 {
                p.sum[a] += t.pos[k][a];
            }
            p.n += 1.0;
        }
        p.normal = std::array::from_fn(|a| p.normal[a] + n[a]);
    }
    let unit = |v: [f32; 3]| {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
        v.map(|x| x / l)
    };
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    // The tiles' sides: which side an outer edge is (both ends on it), what is across.
    for (f, t) in tris.iter().enumerate() {
        let p = &patches[&root[f]];
        let ext = [p.hi[0] - p.lo[0], p.hi[1] - p.lo[1]];
        if ext[0] > 1.02 || ext[1] > 1.02 {
            continue;
        }
        let (lo, hi, pn) = (p.lo, p.hi, unit(p.normal));
        let mut found = Vec::new();
        for e in 0..3 {
            let (a, b) = (e, (e + 1) % 3);
            // (A collapsed edge has no entry: no side.)
            let Some(across) = edges.get(&key(q(t.pos[a]), q(t.pos[b]))) else { continue };
            let others: Vec<u32> = across.iter().map(|x| x.0).filter(|&g| root[g as usize] != root[f]).collect();
            let on = |k: usize, v: f32| (t.uv[a][k] - v).abs() < 0.02 * ext[k].max(1e-3) && (t.uv[b][k] - v).abs() < 0.02 * ext[k].max(1e-3);
            let side = if on(0, lo[0]) {
                0
            } else if on(0, hi[0]) {
                1
            } else if on(1, lo[1]) {
                2
            } else if on(1, hi[1]) {
                3
            } else {
                continue;
            };
            // Across: flush if any face there lies in the tile's plane, else up or down.
            let kind = if others.is_empty() {
                3
            } else if others.iter().any(|&g| dot(unit(cross(&tris[g as usize])), pn).abs() > 0.85) {
                0
            } else {
                let g = &tris[others[0] as usize];
                let (ea, eb) = (q(t.pos[a]), q(t.pos[b]));
                let far = (0..3).map(|k| g.pos[k]).find(|&v| q(v) != ea && q(v) != eb).unwrap_or(g.pos[0]);
                let d = [far[0] - t.pos[a][0], far[1] - t.pos[a][1], far[2] - t.pos[a][2]];
                if dot(d, pn) < 0.0 {
                    1
                } else {
                    2
                }
            };
            found.push((side, kind));
        }
        let p = patches.get_mut(&root[f]).unwrap();
        // A drop wins over a wall, a wall over flush, flush over nothing.
        let rank = |k: u8| [1u8, 3, 2, 0][k as usize];
        for (side, kind) in found {
            p.sides[side] = Some(p.sides[side].filter(|&o| rank(o) >= rank(kind)).unwrap_or(kind));
        }
    }
    (0..tris.len())
        .map(|f| {
            let p = &patches[&root[f]];
            let c = p.sum.map(|x| (x / p.n * 4.0).round() as i32 as u32);
            let h = c[0].wrapping_mul(0x9e37_79b1) ^ c[1].wrapping_mul(0x85eb_ca77) ^ c[2].wrapping_mul(0xc2b2_ae3d);
            let h = (h ^ (h >> 15)).wrapping_mul(0x2c1b_3c6d);
            let seed = (h ^ (h >> 13)) & 0x7fff;
            let sides = p.sides.iter().enumerate().fold(0u32, |a, (k, s)| a | (s.unwrap_or(0) as u32) << (k * 2));
            1 << 23 | seed << 8 | sides
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two triangles of a quad: corners `p` (in order round it) with texture coordinates `uv`.
    fn quad(p: [[f32; 3]; 4], uv: [[f32; 2]; 4], normal: [f32; 3], crc: u32) -> [Tri; 2] {
        [Tri { pos: [p[0], p[1], p[2]], uv: [uv[0], uv[1], uv[2]], normal, crc }, Tri { pos: [p[0], p[2], p[3]], uv: [uv[0], uv[2], uv[3]], normal, crc }]
    }

    /// A row of three slabs (a quad each, one repeat of the texture): a drop at one end, a wall
    /// at the other, nothing in front or behind. Each slab's sides say so, the slabs between
    /// them are flush, and every slab has its own seed.
    #[test]
    fn tiles_know_their_sides() {
        let up = [0.0, 1.0, 0.0];
        let uv = [[0.0, 0.0], [0.998, 0.0], [0.998, 0.998], [0.0, 0.998]];
        let mut tris = Vec::new();
        for k in 0..3 {
            let x = k as f32 * 50.0;
            tris.extend(quad([[x, 0.0, 0.0], [x + 50.0, 0.0, 0.0], [x + 50.0, 0.0, 50.0], [x, 0.0, 50.0]], uv, up, 7));
        }
        // The drop under the first slab's u- side, the wall above the last's u+.
        let side = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        tris.extend(quad([[0.0, 0.0, 0.0], [0.0, 0.0, 50.0], [0.0, -50.0, 50.0], [0.0, -50.0, 0.0]], side, [-1.0, 0.0, 0.0], 8));
        tris.extend(quad([[150.0, 0.0, 0.0], [150.0, 50.0, 0.0], [150.0, 50.0, 50.0], [150.0, 0.0, 50.0]], side, [-1.0, 0.0, 0.0], 8));
        // A collapsed triangle (two corners in one place, as some retail models have): no side.
        tris.push(Tri { pos: [[0.0; 3], [0.0; 3], [9.0, 0.0, 9.0]], uv: [[0.0; 2], [0.5, 0.0], [0.5, 0.5]], normal: up, crc: 9 });
        let codes = cell_codes(&tris);
        let sides = |f: usize| [0, 1, 2, 3].map(|k| (codes[f] >> (k * 2)) & 3);
        // u-, u+, v-, v+: 0 flush, 1 drop, 2 wall, 3 nothing.
        assert_eq!(sides(0), [1, 0, 3, 3]);
        assert_eq!(sides(2), [0, 0, 3, 3]);
        assert_eq!(sides(4), [0, 2, 3, 3]);
        for k in 0..3 {
            assert_eq!(codes[k * 2], codes[k * 2 + 1], "both halves of slab {k} are one tile");
            assert_ne!(codes[k * 2] & 1 << 23, 0);
        }
        let seed = |f: usize| (codes[f] >> 8) & 0x7fff;
        assert!(seed(0) != seed(2) && seed(2) != seed(4) && seed(0) != seed(4));
        // The same every time (the seeds come from where the slabs are in the model).
        assert_eq!(cell_codes(&tris), codes);
    }
}
