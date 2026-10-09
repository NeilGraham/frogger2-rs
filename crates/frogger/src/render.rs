//! Drawing the simulation state: level model, backdrop, actors (interpolated between the
//! last two ticks), debug tile overlay, and the temporary cameras.

use f2_engine::scene::{Frame, TlVertex, Tri, SZ_SCALE};
use f2_engine::{Actor, Camera, Renderer};
use f2_formats::objbank::ObjectBank;
use f2_game::ids::ActorId;
use f2_game::Game;
use f2_math::{IQuaternion, SVector};
use f2_platform::dik;
use f2_platform::KeyboardState;
use std::collections::HashMap;

pub type V3 = [f32; 3];

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn mul(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn norm(a: V3) -> V3 {
    let l = dot(a, a).sqrt();
    if l > 1e-6 {
        mul(a, 1.0 / l)
    } else {
        [0.0, 1.0, 0.0]
    }
}

/// Game units (`SVECTOR`, ×`SCALE`) to render units: the PC renderer divides by 10
/// (`pcaudio.c:335`, `drawloop.cpp:97`).
pub fn sv_to_render(v: &SVector) -> V3 {
    [v.vx as f32 / 10.0, v.vy as f32 / 10.0, v.vz as f32 / 10.0]
}

/// A unit `SVECTOR` direction (4096 = 1.0) as floats.
fn sv_dir(v: &SVector) -> V3 {
    [v.vx as f32 / 4096.0, v.vy as f32 / 4096.0, v.vz as f32 / 4096.0]
}

/// `IQUATERNION` (12-bit fixed) to a float `[x, y, z, w]`.
pub fn iq_to_quat(q: &IQuaternion) -> [f32; 4] {
    if q.x == 0 && q.y == 0 && q.z == 0 && q.w == 0 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    [q.x as f32 / 4096.0, q.y as f32 / 4096.0, q.z as f32 / 4096.0, q.w as f32 / 4096.0]
}

/// Render-relevant state of one actor at a tick (for interpolation).
#[derive(Clone, Copy, Debug)]
pub struct Snap {
    pub pos: V3,
    pub q: [f32; 4],
}

/// Per-slot snapshot of all live actors.
pub fn snapshot(game: &Game) -> Vec<Option<Snap>> {
    game.actor2.actors.slots.iter().map(|s| s.as_ref().map(|a| Snap { pos: sv_to_render(&a.actor.position), q: iq_to_quat(&a.actor.q_rot) })).collect()
}

fn lerp_snap(prev: Option<&Snap>, cur: Snap, alpha: f32) -> Snap {
    let Some(p) = prev else { return cur };
    // Teleports (respawn, level reset) are not interpolated.
    let d = sub(cur.pos, p.pos);
    if dot(d, d) > 400.0 * 400.0 {
        return cur;
    }
    Snap { pos: f2_engine::math::linear_interp(&p.pos, &cur.pos, alpha), q: f2_engine::math::quat_slerp(&p.q, &cur.q, alpha) }
}

// ------------------------------------------------------------------ level scene

/// Renderer-side data of the loaded level (`LevelAssets`).
pub struct LevelScene {
    pub world: u32,
    pub level: u32,
    pub banks: Vec<ObjectBank>,
    /// Model id (`crc("name.obe")`) → (bank, model); first loaded bank wins like `FindObject`.
    pub models: HashMap<u32, (usize, usize)>,
    pub tex_files: Vec<String>,
    pub world_model: Option<(usize, usize)>,
    /// `farClip = fog.max * 0.1` (drawloop.cpp:198), render units.
    pub far_clip: f32,
    pub clear: [f64; 4],
    /// Engine actor per game actor slot (keyframe search hints persist across frames).
    pub actors: Vec<Option<(u32, Actor)>>,
    /// Stand-in frog while `CreateFrogger` is not ported (drawn on the player start tile).
    pub placeholder_frog: Option<((usize, usize), Actor)>,
}

impl LevelScene {
    fn index(banks: &[ObjectBank]) -> HashMap<u32, (usize, usize)> {
        let mut map = HashMap::new();
        for (bi, b) in banks.iter().enumerate() {
            for (mi, m) in b.models.iter().enumerate() {
                map.entry(m.id).or_insert((bi, mi));
            }
        }
        map
    }

    pub fn new(world: u32, level: u32, banks: Vec<ObjectBank>, tex_files: Vec<String>, game: &Game) -> LevelScene {
        let models = Self::index(&banks);
        let world_model = models.get(&f2_formats::crc("world.obe")).copied();
        let lvd = &game.layout.world_visual_data[world as usize].level_visual_data[level as usize];
        // TODO(port): level scripts change `fog.max` (script.c:833); until they run, keep the
        // far plane beyond the follow camera's distance (~776) so the frog is not clipped.
        let far_clip = if lvd.far_clip > 0 { lvd.far_clip as f32 * 0.1 } else { 1200.0 }.max(MIN_FAR_CLIP);
        let fog = lvd.fog.map(|c| c as u8 as f64 / 255.0);
        let clear = [fog[0], fog[1], fog[2], 1.0];
        let mut s = LevelScene { world, level, banks, models, tex_files, world_model, far_clip, clear, actors: Vec::new(), placeholder_frog: None };
        // TODO(port): remove once f2-game creates the frogs (CreateFrogger, frogger.c).
        if !game_has_frog(game) {
            if let (Some(&bm), Some(t)) = (s.models.get(&f2_formats::crc("frogger.obe")), game.map.g_t_start[0]) {
                let tile = game.map.tile(t);
                let mut q = IQuaternion::default();
                f2_game::actor2::orientate_ss(&mut q, &tile.dir_vector[0], &tile.normal);
                let a = Actor { pos: sv_to_render(&tile.centre), q_rot: iq_to_quat(&q), ..Default::default() };
                s.placeholder_frog = Some((bm, a));
            }
        }
        s
    }
}

/// Whether the simulation has a frog actor yet (`frog[0]`).
pub fn game_has_frog(game: &Game) -> bool {
    frog_actor(game).is_some()
}

/// `frog[0]`: until the frogger.c port exposes it, the first live actor using the frog model.
pub fn frog_actor(game: &Game) -> Option<ActorId> {
    let crc = f2_formats::crc("frogger.obe");
    game.actor2.act_list.ids(&game.actor2.actors).into_iter().find(|&id| game.actor2.actors[id].actor.actual_actor.object_controller == Some(crc))
}

// ------------------------------------------------------------------ cameras

/// Free-flying debug camera (F1).
#[derive(Clone, Copy, Debug)]
pub struct FlyCam {
    pub pos: V3,
    pub yaw: f32,
    pub pitch: f32,
}

impl FlyCam {
    pub fn from_camera(c: &Camera) -> FlyCam {
        let d = norm(sub(c.target, c.source));
        FlyCam { pos: c.source, yaw: d[0].atan2(d[2]), pitch: d[1].clamp(-1.0, 1.0).asin() }
    }
    fn forward(&self) -> V3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        [cp * sy, sp, cp * cy]
    }
    pub fn camera(&self) -> Camera {
        Camera { source: self.pos, target: add(self.pos, mul(self.forward(), 100.0)), up: [0.0, 1.0, 0.0] }
    }
    /// WASD move, Q/E down/up, arrow keys look, Shift fast.
    pub fn update(&mut self, kb: &KeyboardState, dt: f32) {
        let turn = 1.8 * dt;
        if kb.is_down(dik::LEFT) {
            self.yaw += turn;
        }
        if kb.is_down(dik::RIGHT) {
            self.yaw -= turn;
        }
        if kb.is_down(dik::UP) {
            self.pitch = (self.pitch + turn).min(1.55);
        }
        if kb.is_down(dik::DOWN) {
            self.pitch = (self.pitch - turn).max(-1.55);
        }
        let f = self.forward();
        let right = norm(cross(f, [0.0, 1.0, 0.0]));
        let mut mv = [0.0; 3];
        for (k, d) in [(dik::W, f), (dik::S, mul(f, -1.0)), (dik::D, right), (dik::A, mul(right, -1.0)), (dik::E, [0.0, 1.0, 0.0]), (dik::Q, [0.0, -1.0, 0.0])]
        {
            if kb.is_down(k) {
                mv = add(mv, d);
            }
        }
        let speed = if kb.is_down(dik::LSHIFT) || kb.is_down(dik::RSHIFT) { 1200.0 } else { 300.0 };
        self.pos = add(self.pos, mul(mv, speed * dt));
    }
}

/// `currCamDist = scaleV * camDist` (cam.c:61/73/579) in render units: `scaleV` = 4506/4096,
/// `camDist` = 680 up, 192 back.
const CAM_DIST: (f32, f32) = (680.0 * 4506.0 / 4096.0, 192.0 * 4506.0 / 4096.0);
/// Lower bound of the far plane while level scripts are not run.
const MIN_FAR_CLIP: f32 = 2000.0;

/// Follow camera standing in for `cam.c` (not ported yet): `CameraSetOffset` with
/// `camFacing = 0` behind the frog (or the player start tile), looking at it.
pub fn follow_camera(game: &Game, scene: &LevelScene, frog_pos: Option<V3>) -> Camera {
    let tile = game.map.g_t_start[0].map(|t| game.map.tile(t));
    let (centre, up, dir) = match tile {
        Some(t) => (sv_to_render(&t.centre), norm(sv_dir(&t.normal)), sv_dir(&t.dir_vector[0])),
        None => ([0.0; 3], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
    };
    let target = frog_pos.or_else(|| scene.placeholder_frog.as_ref().map(|(_, a)| a.pos)).unwrap_or(centre);
    let source = add(add(target, mul(up, CAM_DIST.0)), mul(dir, -CAM_DIST.1));
    Camera { source, target, up: [0.0, 1.0, 0.0] }
}

// ------------------------------------------------------------------ drawing

/// Queues the level for one rendered frame. `prev` is the snapshot before the last tick,
/// `alpha` the fraction of a tick elapsed since it.
/// `far_clip` is the level's far plane times the draw-distance option.
#[allow(clippy::too_many_arguments)]
pub fn draw_level(r: &mut Renderer, scene: &mut LevelScene, game: &Game, prev: &[Option<Snap>], alpha: f32, cam: &Camera, far_clip: f32, show_tiles: bool) {
    r.set_camera(cam);
    r.view.near_clip = 10.0;
    r.view.far_clip = far_clip;
    r.update_animating_textures(game.act_frame_count);

    // DrawBackground (drawloop.cpp:85): the backdrop follows the camera, no clipping.
    if let Some(bg) = game.actor2.back_gnd.and_then(|id| game.actor2.actors.get(id)) {
        if let Some(&(b, m)) = bg.actor.actual_actor.object_controller.and_then(|id| scene.models.get(&id)) {
            let s = bg.actor.size;
            let mut a = Actor {
                pos: cam.source,
                q_rot: iq_to_quat(&bg.actor.q_rot),
                scale: [s.vx as f32 / 4096.0, s.vy as f32 / 4096.0, s.vz as f32 / 4096.0],
                ..Default::default()
            };
            let far = r.view.far_clip;
            r.view.no_clip = true;
            r.view.far_clip = 12400.0;
            r.draw_actor(&scene.banks[b].models[m], &mut a);
            r.view.no_clip = false;
            r.view.far_clip = far;
        }
    }

    // DrawLandscape(world)
    if let Some((b, m)) = scene.world_model {
        r.draw_landscape(&scene.banks[b].models[m]);
    }

    // ActorListDraw(0), then list 1 (shadows / additive).
    if scene.actors.len() < game.actor2.actors.slots.len() {
        scene.actors.resize_with(game.actor2.actors.slots.len(), || None);
    }
    for list in [0, 1] {
        for id in game.actor_draw_list(list) {
            if Some(id) == game.actor2.back_gnd {
                continue;
            }
            let a2 = &game.actor2.actors[id];
            if a2.draw == 0 || a2.flags & f2_game::actor2::ACTOR_DRAW_NEVER != 0 {
                continue;
            }
            let mdx = &a2.actor.actual_actor;
            let Some(model_id) = mdx.object_controller else { continue };
            let Some(&(b, m)) = scene.models.get(&model_id) else { continue };
            let cur = Snap { pos: sv_to_render(&a2.actor.position), q: iq_to_quat(&a2.actor.q_rot) };
            let s = lerp_snap(prev.get(id.idx()).and_then(|p| p.as_ref()), cur, alpha);
            let slot = &mut scene.actors[id.idx()];
            if slot.as_ref().is_none_or(|(mid, _)| *mid != model_id) {
                *slot = Some((model_id, Actor::default()));
            }
            let (_, ea) = slot.as_mut().unwrap();
            ea.pos = s.pos;
            ea.q_rot = s.q;
            let sz = a2.actor.size;
            ea.scale = [sz.vx as f32 / 4096.0, sz.vy as f32 / 4096.0, sz.vz as f32 / 4096.0];
            ea.animated = mdx.animation.is_some();
            ea.anim_time = mdx.animation.as_ref().map_or(0.0, |an| an.anim_time);
            use f2_engine::scene::{OBJECT_FLAGS_ADDITIVE, OBJECT_FLAGS_MODGE, OBJECT_FLAGS_WAVE};
            ea.extra_flags = mdx.root_object_flags & (OBJECT_FLAGS_ADDITIVE | OBJECT_FLAGS_MODGE | OBJECT_FLAGS_WAVE);
            r.draw_actor(&scene.banks[b].models[m], ea);
        }
    }
    if let Some(((b, m), a)) = &mut scene.placeholder_frog {
        r.draw_actor(&scene.banks[*b].models[*m], a);
    }

    if show_tiles {
        draw_tiles(r, game);
    }
}

fn tl(sx: f32, sy: f32, vz: f32, c: [u8; 4]) -> TlVertex {
    TlVertex {
        sx,
        sy,
        // Slight bias towards the camera so the overlay sits on top of coplanar geometry.
        sz: ((vz - 2.0) * SZ_SCALE).clamp(0.0, 1.0),
        rhw: 1.0,
        color: c.map(|x| x as f32 / 255.0),
        uv: [0.0; 2],
        fog: 1.0,
        alpha_ref: 0.0,
    }
}

fn push_world_tri(r: &mut Renderer, p: [V3; 3], color: [u8; 4]) {
    let mut v = [TlVertex::default(); 3];
    for k in 0..3 {
        let Some(s) = r.view.xfm_point(p[k]) else { return };
        v[k] = tl(s[0], s[1], s[2], color);
    }
    r.push_tri(Frame::Xlu, Tri { v, tex: None });
}

/// Debug overlay (F2): one translucent quad per collision tile, coloured by state.
fn draw_tiles(r: &mut Renderer, game: &Game) {
    let half = 7.0;
    for t in &game.map.tiles {
        let c = sv_to_render(&t.centre);
        let n = norm(sv_dir(&t.normal));
        let d = sv_dir(&t.dir_vector[0]);
        let a = if dot(d, d) < 1e-6 { norm(cross(n, [1.0, 0.0, 0.0])) } else { norm(sub(d, mul(n, dot(d, n)))) };
        let b = cross(n, a);
        let o = add(c, mul(n, 1.5));
        let q = [
            add(o, add(mul(a, half), mul(b, half))),
            add(o, sub(mul(a, half), mul(b, half))),
            sub(o, add(mul(a, half), mul(b, half))),
            add(o, sub(mul(b, half), mul(a, half))),
        ];
        let mut col = tile_color(t.state);
        col[3] = 150;
        push_world_tri(r, [q[0], q[1], q[2]], col);
        push_world_tri(r, [q[2], q[3], q[0]], col);
    }
}

fn tile_color(s: u32) -> [u8; 4] {
    match s {
        0 => [60, 200, 60, 255],
        1 => [230, 30, 30, 255],
        2 => [140, 90, 40, 255],
        3 => [170, 240, 255, 255],
        4 => [255, 230, 0, 255],
        5 => [60, 90, 255, 255],
        6 => [160, 255, 160, 255],
        7 => [255, 255, 255, 255],
        8 => [255, 0, 255, 255],
        9 => [255, 140, 0, 255],
        10 => [110, 110, 110, 255],
        11 => [120, 20, 20, 255],
        12..=15 => [160, 80, 220, 255],
        16 => [90, 0, 0, 255],
        17 => [255, 90, 40, 255],
        18 => [240, 240, 120, 255],
        0x20..=0x2f => [0, 160, 255, 255],
        _ => [200, 200, 200, 255],
    }
}
