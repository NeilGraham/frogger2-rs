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

/// Pipeline kind of the PBR mesh pipelines (`Gpu::pipelines`).
pub(crate) const PBR_KIND: u8 = 16;

/// Map bits of `Pbr.maps.x`.
pub(crate) const HAS_NORMAL: u32 = 1;
pub(crate) const HAS_MR: u32 = 2;
pub(crate) const HAS_AO: u32 = 4;
pub(crate) const HAS_EMISSIVE: u32 = 8;
pub(crate) const HAS_HEIGHT: u32 = 16;

/// The material uniform block of `p` (`Pbr` in [`WGSL`]).
pub(crate) fn uniform(p: &crate::textures::Pbr) -> [f32; 16] {
    let bits = p.normal.map_or(0, |_| HAS_NORMAL)
        | p.metal_rough.map_or(0, |_| HAS_MR)
        | p.occlusion.map_or(0, |_| HAS_AO)
        | p.emissive_map.map_or(0, |_| HAS_EMISSIVE)
        | p.height.filter(|_| p.parallax_scale > 0.0).map_or(0, |_| HAS_HEIGHT);
    let [r, g, b, a] = p.base_color;
    let [er, eg, eb] = p.emissive;
    [r, g, b, a, er, eg, eb, p.parallax_scale, p.metallic, p.roughness, p.normal_scale, p.occlusion_strength, bits as f32, 0.0, 0.0, 0.0]
}

pub(crate) const WGSL: &str = r#"
// PBR-BEGIN (crate::pbr)
struct Pbr {
    base: vec4<f32>,      // baseColorFactor
    emissive: vec4<f32>,  // emissive factor; w: parallax scale
    mr: vec4<f32>,        // metallic, roughness, normal scale, occlusion strength
    maps: vec4<f32>,      // x: which maps (crate::pbr::HAS_*)
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

// How deep the height field is at uv (0: its top; the map's white is high).
fn pom_depth(uv: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>) -> f32 {
    return 1.0 - textureSampleGrad(t_height, s, uv, dx, dy).r;
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
    if (framed && has_map(16u)) {
        let hit = parallax(uv, vec3<f32>(dot(v, tu), dot(v, tv), dot(v, n)), dx, dy);
        uv = hit.xy;
        depth = hit.z;
    }
    var c = textureSampleGrad(t, s, uv, dx, dy) * i.color * pbr.base;
    if (floor(c.a * 255.0 + 0.5) <= cam.depth.y) { discard; }
    var emissive = pbr.emissive.rgb;
    if (has_map(8u)) { emissive *= textureSampleGrad(t_emit, s, uv, dx, dy).rgb; }
    if (lt.sky.w > 0.5 && (i.flags & 1u) == 0u) {
        if (framed && has_map(1u)) {
            var m = textureSampleGrad(t_normal, s, uv, dx, dy).xyz * 2.0 - 1.0;
            m = vec3<f32>(m.xy * pbr.mr.z, m.z);
            // Green is up in the image: towards -v.
            let bent = tu * m.x - tv * m.y + n * m.z;
            if (dot(bent, bent) > 1e-12) { n = normalize(bent); }
        }
        var metal = pbr.mr.x;
        var rough = pbr.mr.y;
        if (has_map(2u)) {
            let mr = textureSampleGrad(t_mr, s, uv, dx, dy);
            rough *= mr.g;
            metal *= mr.b;
        }
        var ao = 1.0;
        if (has_map(4u)) { ao = mix(1.0, textureSampleGrad(t_ao, s, uv, dx, dy).r, pbr.mr.w); }
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
