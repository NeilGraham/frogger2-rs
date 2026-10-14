//! Drawing the simulation state (`PC/drawloop.cpp`): level model, sky, actors
//! (interpolated between the last two ticks), then the effects (`fx`) and 2D layers
//! (`overlay`); the game camera and the debug tile overlay.

// Newer `core` has some of these methods inherently (then the trait goes unused).
#[cfg(not(feature = "std"))]
#[allow(unused_imports)]
use crate::fmath::FloatExt;
use crate::fx;
use crate::overlay::{self, GameFonts, TileScroll};
use crate::prelude::*;
use alloc::collections::BTreeMap;
use f2_engine::scene::{Frame, TlVertex, Tri, SZ_SCALE};
use f2_engine::{Actor, Camera, Renderer, TexHandle};
use f2_formats::objbank::ObjectBank;
use f2_game::ids::ActorId;
use f2_game::Game;
use f2_math::{IQuaternion, SVector};

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
pub fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub fn norm(a: V3) -> V3 {
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

/// Render-relevant state of one actor at a tick (for interpolation): the simulation's own
/// position and rotation, converted to render units only when drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snap {
    pub pos: [i16; 3],
    pub q: IQuaternion,
}

impl Snap {
    fn of(a: &f2_game::actor2::Actor2) -> Snap {
        let p = &a.actor.position;
        Snap { pos: [p.vx, p.vy, p.vz], q: a.actor.q_rot }
    }

    /// Render units ([`sv_to_render`], [`iq_to_quat`]).
    fn render(&self) -> (V3, [f32; 4]) {
        let p = self.pos;
        ([p[0] as f32 / 10.0, p[1] as f32 / 10.0, p[2] as f32 / 10.0], iq_to_quat(&self.q))
    }
}

/// Per-slot snapshot of all live actors.
pub fn snapshot(game: &Game) -> Vec<Option<Snap>> {
    let mut v = Vec::new();
    snapshot_into(game, &mut v);
    v
}

/// [`snapshot`] into `out`, reusing its allocation.
pub fn snapshot_into(game: &Game, out: &mut Vec<Option<Snap>>) {
    out.clear();
    out.extend(game.actor2.actors.slots.iter().map(|s| s.as_ref().map(Snap::of)));
}

/// An actor's position and rotation `alpha` of a tick after `prev` (render units).
fn lerp_snap(prev: Option<&Snap>, cur: &Snap, alpha: f32) -> (V3, [f32; 4]) {
    let (cp, cq) = cur.render();
    let Some(p) = prev else { return (cp, cq) };
    // Not moved since the last tick: `quat_slerp(q, q, alpha)` is `q` up to rounding.
    #[cfg(feature = "lean")]
    if p == cur {
        return (cp, cq);
    }
    let (pp, pq) = p.render();
    // Teleports (respawn, level reset, a path entity wrapping back to its first node) are not
    // interpolated: more than 1.5 tiles (90 render units) in one 60 Hz tick is faster than
    // anything moves in the game. (400 let a mower's lane wrap through: one display frame at
    // the halfway point on high-refresh screens; the original drew only whole ticks.)
    let d = sub(cp, pp);
    if dot(d, d) > 90.0 * 90.0 {
        return (cp, cq);
    }
    let pos = f2_engine::math::linear_interp(&pp, &cp, alpha);
    // Console fast path: an unchanged rotation (moving, not turning) needs no slerp.
    #[cfg(feature = "lean")]
    if p.q == cur.q {
        return (pos, cq);
    }
    (pos, f2_engine::math::quat_slerp(&pq, &cq, alpha))
}

// ------------------------------------------------------------------ level scene

/// An engine actor with its model id and that model's `LevelScene::models` entry.
/// `repr(C)`: what every actor reads each frame (a culled one nothing else) comes first.
#[repr(C)]
pub struct ActorSlot {
    pub model_id: u32,
    /// [`Renderer::actor_bound`] of the model: actors wholly outside the view are left out.
    #[cfg_attr(not(feature = "lean"), allow(dead_code))]
    bound: Option<f32>,
    /// How many draws the actor made when it was last drawn (`u32::MAX`: not yet): what a
    /// culled actor tells the back end it leaves out.
    #[cfg_attr(not(feature = "lean"), allow(dead_code))]
    ndraws: u32,
    /// The simulation's position / rotation / size the actor's `pos`, `q_rot` and `scale`
    /// were last converted from (skips the conversions while they stay the same).
    last: Option<(Snap, [i32; 3])>,
    pub bm: (usize, usize),
    /// The actor's `overrideTex` name and the CRC of its lowercase form.
    override_crc: Option<(String, u32)>,
    pub actor: Actor,
}

/// Renderer-side data of the loaded level (`LevelAssets`).
pub struct LevelScene {
    pub world: u32,
    pub level: u32,
    pub banks: Vec<ObjectBank>,
    /// Model id (`crc("name.obe")`) → (bank, model); first loaded bank wins like `FindObject`.
    pub models: BTreeMap<u32, (usize, usize)>,
    pub tex_files: Vec<String>,
    pub world_model: Option<(usize, usize)>,
    /// `farClip = fog.max * 0.1` (drawloop.cpp:198), render units.
    pub far_clip: f32,
    pub clear: [f64; 4],
    /// Engine actor per game actor slot (keyframe search hints persist across frames), with
    /// its model id and that model's `models` entry (looked up once, not every frame).
    pub actors: Vec<Option<ActorSlot>>,
    /// Stand-in frog while `CreateFrogger` is not ported (drawn on the player start tile).
    pub placeholder_frog: Option<((usize, usize), Actor)>,
    /// Draw list 1's actors, collected while drawing list 0 (one pass over `actList`).
    list1: Vec<ActorId>,
    /// [`Renderer::actor_bound`] per model id (computed when an actor slot first needs it).
    bounds: BTreeMap<u32, Option<f32>>,
}

impl LevelScene {
    fn index(banks: &[ObjectBank]) -> BTreeMap<u32, (usize, usize)> {
        let mut map = BTreeMap::new();
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
        // Fallback far plane (`far_clip`): the game's `fog.max` normally overrides it.
        let far_clip = if lvd.far_clip > 0 { lvd.far_clip as f32 * 0.1 } else { 1200.0 };
        let fog = lvd.fog.map(|c| c as u8 as f64 / 255.0);
        let clear = [fog[0], fog[1], fog[2], 1.0];
        let mut s = LevelScene {
            world,
            level,
            banks,
            models,
            tex_files,
            world_model,
            far_clip,
            clear,
            actors: Vec::new(),
            placeholder_frog: None,
            list1: Vec::new(),
            bounds: BTreeMap::new(),
        };
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

/// `frog[0]`.
pub fn frog_actor(game: &Game) -> Option<ActorId> {
    game.frogger.frog[0].filter(|&id| game.actor2.actors.get(id).is_some())
}

// ------------------------------------------------------------------ cameras

/// `CAMVECTSCALE` (drawloop.cpp:63): 20.12 game units to render units.
const CAMVECTSCALE: f32 = 1.0 / 40960.0;

/// The game camera (`CalcViewMatrix`, drawloop.cpp:65): `currCamSource`,
/// `currCamTarget`, `camVect`.
pub fn game_camera(game: &Game) -> Camera {
    let (s, t, u) = game.camera_source_target();
    let f = |v: f2_math::FVector| [v.vx as f32 * CAMVECTSCALE, v.vy as f32 * CAMVECTSCALE, v.vz as f32 * CAMVECTSCALE];
    let up = f(u);
    Camera { source: f(s), target: f(t), up: if dot(up, up) > 0.0 { up } else { [0.0, 1.0, 0.0] } }
}

/// The game camera between the last two ticks (cuts are not interpolated).
pub fn lerp_camera(prev: Option<&Camera>, cur: Camera, alpha: f32) -> Camera {
    let Some(p) = prev else { return cur };
    let d = sub(cur.source, p.source);
    let e = sub(cur.target, p.target);
    if dot(d, d) > 400.0 * 400.0 || dot(e, e) > 400.0 * 400.0 {
        return cur;
    }
    let l = |a: &V3, b: &V3| f2_engine::math::linear_interp(a, b, alpha);
    Camera { source: l(&p.source, &cur.source), target: l(&p.target, &cur.target), up: l(&p.up, &cur.up) }
}

/// `farClip = fog.max * 0.1` (drawloop.cpp:198) in render units; the level table's far
/// clip until a script sets the fog.
pub fn far_clip(game: &Game, scene: &LevelScene) -> f32 {
    let (_, f) = game.render_fog();
    if f > 0.0 {
        f
    } else {
        scene.far_clip
    }
}

/// The view transform `IsPointVisible` / `OutcodeCheck` use in the simulation: the game
/// camera at 640×480 (independent of the window and the free camera, so the simulation
/// stays deterministic).
pub fn spec_fx_view(game: &Game, far_clip: f32) -> f2_game::specfx::ViewXform {
    let cam = game_camera(game);
    let v = f2_engine::View::new(640, 480);
    f2_game::specfx::ViewXform {
        matrix: cam.view_matrix(),
        x_res: 640.0,
        y_res: 480.0,
        near_clip: v.near_clip,
        far_clip,
        horiz_clip: v.horiz_clip,
        vert_clip: v.vert_clip,
        fov: v.fov as i32,
        dist: f2_engine::scene::DIST as i32,
    }
}

// ------------------------------------------------------------------ drawing

/// Host-side inputs of [`draw_loop`].
pub struct DrawCtx<'a> {
    pub prev: &'a [Option<Snap>],
    /// Fraction of a tick elapsed since `prev`.
    pub alpha: f32,
    pub cam: Camera,
    /// The level's far plane times the draw-distance option.
    pub far_clip: f32,
    pub show_tiles: bool,
    /// Level editor: the world only (no fades, HUD overlays or text).
    pub world_only: bool,
    pub fonts: Option<GameFonts>,
    pub authentic: bool,
    /// The loaded full-screen backdrop (`game.backdrop.backdrop`).
    pub backdrop: Option<TexHandle>,
    pub tile_scroll: TileScroll,
    /// Game frames (60 Hz ticks, i.e. retail `LoopFunc`s) since the previous `draw_loop`:
    /// how many times `UpdateAnimatingTextures` runs. The retail game draws once per frame,
    /// and textures whose frame time is -1 (e.g. the Garden bee's `00bwing.bmp`, which makes
    /// its wings flutter) step once per such call; with the host drawing at the display rate
    /// (0, 1 or several draws per tick) one step per draw would tie the flutter to the
    /// monitor (steady half-transparent wings at 144 Hz) instead of to game frames.
    pub tex_anim_steps: u32,
}

/// Optional timing hook: called with a [`draw_phase`] id at the end of each part of
/// [`draw_loop`] (console profiling; unset on the PC).
static PHASE_HOOK: spin::Mutex<Option<fn(u32)>> = spin::Mutex::new(None);

/// Installs (or removes) the [`draw_loop`] timing hook.
pub fn set_phase_hook(h: Option<fn(u32)>) {
    *PHASE_HOOK.lock() = h;
}

/// Phase ids passed to the hook of [`set_phase_hook`] (each marks the end of a part).
pub mod draw_phase {
    pub const SETUP: u32 = 0;
    pub const WORLD: u32 = 1;
    pub const ACTORS0: u32 = 2;
    pub const SHADOWS: u32 = 3;
    pub const ACTORS1: u32 = 4;
    pub const SPRITES: u32 = 5;
    pub const SPECFX: u32 = 6;
    pub const OVERLAYS: u32 = 7;
    pub const COUNT: usize = 8;
    pub const NAMES: [&str; COUNT] = ["setup", "world", "actors0", "shadows", "actors1", "sprites", "specfx", "overlays"];
}

fn phase(id: u32) {
    if let Some(h) = *PHASE_HOOK.lock() {
        h(id);
    }
}

/// Port of `DrawLoop` (PC/drawloop.cpp:132): backdrop, sky, landscape, actors, shadows,
/// sprites, special effects, then fades and the sprite/text overlays.
pub fn draw_loop(r: &mut Renderer, scene: Option<&mut LevelScene>, game: &Game, ctx: &DrawCtx) {
    use f2_game::define::mode::*;
    let mode = game.game_state.mode;
    overlay::set_sprite_clip(r, ctx.authentic);
    if matches!(mode, ARTVIEWER_MODE | STARTUP_MODE | TEASERSCREEN_MODE) {
        if let Some(t) = ctx.backdrop {
            r.draw_backdrop(t);
        }
        overlay::draw_screen_transition(r, game);
        return;
    }

    r.set_camera(&ctx.cam);
    r.view.near_clip = 10.0;
    r.view.far_clip = ctx.far_clip;
    // `UpdateAnimatingTextures` (end of `DrawLoop`, drawloop.cpp:404): once
    // per game frame since the last draw (see `DrawCtx::tex_anim_steps`).
    for _ in 0..ctx.tex_anim_steps {
        r.update_animating_textures(game.act_frame_count);
    }
    // Console front ends refill the procedural textures themselves (straight into their
    // texture format, without the RGBA8 registry copy).
    #[cfg(not(feature = "lean"))]
    overlay::upload_proc_textures(r, game);

    if let Some(t) = ctx.backdrop {
        r.draw_backdrop(t);
    }
    phase(draw_phase::SETUP);
    let tiled = game.backdrop.tiled_backdrop.as_deref();
    if let Some(name) = tiled {
        overlay::draw_tiled_backdrop(r, name, ctx.tile_scroll);
    }
    let draw_game = game.backdrop.draw_game;

    if let Some(scene) = scene {
        if draw_game {
            draw_background(r, scene, game, &ctx.cam);
            // DrawLandscape(world)
            if game.backdrop.draw_landscape {
                if let Some((b, m)) = scene.world_model {
                    r.draw_landscape(&scene.banks[b].models[m]);
                }
            }
            phase(draw_phase::WORLD);
            draw_actor_list(r, scene, game, ctx, 0);
            phase(draw_phase::ACTORS0);
            if ctx.backdrop.is_none() && tiled.is_none() {
                fx::process_shadows(r, game);
            }
            phase(draw_phase::SHADOWS);
            draw_actor_list(r, scene, game, ctx, 1);
            if let Some(((b, m), a)) = &mut scene.placeholder_frog {
                r.draw_actor(&scene.banks[*b].models[*m], a);
            }
            phase(draw_phase::ACTORS1);
            let posed = if r.view.widened() { widened_object_sprites(game) } else { BTreeMap::new() };
            fx::print_sprites(r, game, ctx.far_clip, &posed);
            phase(draw_phase::SPRITES);
            if !matches!(mode, MULTI_WINRACE_MODE | MULTI_WINBATTLE_MODE | MULTI_WINCOLLECT_MODE | LEVELCOMPLETE_MODE) {
                fx::draw_special_fx(r, game);
            }
            phase(draw_phase::SPECFX);
            if ctx.show_tiles {
                draw_tiles(r, game);
            }
        }
    }

    if ctx.world_only {
        return;
    }
    if !game.fadeout.fade_text {
        overlay::draw_screen_transition(r, game);
    }
    overlay::print_sprite_overlays(r, game, 0);
    overlay::print_ui_sprite_overlays(r, game, 0);
    if let Some(f) = &ctx.fonts {
        overlay::print_text_overlays(r, game, f, ctx.authentic);
        overlay::print_ui_text_overlays(r, game, f, ctx.authentic);
    }
    overlay::print_sprite_overlays(r, game, 1);
    overlay::print_ui_sprite_overlays(r, game, 1);
    overlay::print_sprite_overlays(r, game, 2);
    overlay::print_ui_sprite_overlays(r, game, 2);
    if game.fadeout.fade_text {
        overlay::draw_screen_transition(r, game);
    }
    phase(draw_phase::OVERLAYS);
}

/// Port of `DrawBackground` (drawloop.cpp:85): the sky actor follows the camera, no
/// far clipping.
fn draw_background(r: &mut Renderer, scene: &LevelScene, game: &Game, cam: &Camera) {
    let Some(bg) = game.actor2.back_gnd.and_then(|id| game.actor2.actors.get(id)) else { return };
    // DrawActor (mdxActor.cpp:232) returns for an invisible MDX actor: CopyActorList
    // (PC/Main.cpp:645) clears `visible` while the actor's `draw` is 0, e.g. after
    // StartLevelComplete hid every actor in actList, backGnd included. (DrawBackground's
    // `backGnd->actor->visible = 1` sets the game ACTOR's field, not the MDX_ACTOR's.)
    if bg.actor.actual_actor.placement.visible == 0 {
        return;
    }
    let Some(&(b, m)) = bg.actor.actual_actor.object_controller.and_then(|id| scene.models.get(&id)) else { return };
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

/// `ActorListDraw(list)`: the actors of draw list `list`, interpolated between ticks.
/// List 0 walks `actList` once and keeps list 1's actors (in list order) for the list-1 call
/// that follows it in `draw_loop`.
#[cfg_attr(target_vendor = "nintendo64", allow(unsafe_code), link_section = ".text.f2hot_d")]
fn draw_actor_list(r: &mut Renderer, scene: &mut LevelScene, game: &Game, ctx: &DrawCtx, list: u32) {
    if scene.actors.len() < game.actor2.actors.slots.len() {
        scene.actors.resize_with(game.actor2.actors.slots.len(), || None);
    }
    let pc = PerList { modge_frame: modge_frame(game), portraits: game.options.portrait_actor.iter().any(Option::is_some), back_gnd: game.actor2.back_gnd };
    if list == 0 {
        let mut list1 = core::mem::take(&mut scene.list1);
        list1.clear();
        let actors = &game.actor2.actors;
        let mut c = game.actor2.act_list.first;
        while let Some(id) = c {
            let a2 = &actors[id];
            c = a2.links.next;
            match a2.actor.actual_actor.list_num {
                0 => draw_listed_actor(r, scene, game, ctx, &pc, id),
                1 => list1.push(id),
                _ => {}
            }
        }
        scene.list1 = list1;
    } else {
        let list1 = core::mem::take(&mut scene.list1);
        for &id in &list1 {
            draw_listed_actor(r, scene, game, ctx, &pc, id);
        }
        scene.list1 = list1;
    }
}

/// Per-frame inputs of [`draw_listed_actor`].
struct PerList {
    modge_frame: u32,
    /// Any multiplayer portrait actor set (else no actor has Gouraud values).
    portraits: bool,
    back_gnd: Option<ActorId>,
}

/// One actor of `ActorListDraw`.
#[cfg_attr(target_vendor = "nintendo64", allow(unsafe_code), link_section = ".text.f2hot_d")]
fn draw_listed_actor(r: &mut Renderer, scene: &mut LevelScene, game: &Game, ctx: &DrawCtx, pc: &PerList, id: ActorId) {
    if Some(id) == pc.back_gnd {
        return;
    }
    // `actors[id]` without the out-of-line `Index` call (the per-actor code stays in one
    // I-cache-resident region on the N64).
    let a2 = game.actor2.actors.slots[id.idx()].as_ref().expect("freed actor");
    if a2.draw == 0 || a2.flags & f2_game::actor2::ACTOR_DRAW_NEVER != 0 {
        return;
    }
    let mdx = &a2.actor.actual_actor;
    let Some(model_id) = mdx.object_controller else { return };
    let slot = &mut scene.actors[id.idx()];
    if slot.as_ref().is_none_or(|s| s.model_id != model_id) && !new_slot(r, slot, &scene.models, &mut scene.bounds, &scene.banks, model_id) {
        return;
    }
    let sl = slot.as_mut().unwrap();
    let (b, m) = sl.bm;
    let ea = &mut sl.actor;
    let cur = Snap::of(a2);
    let sz = a2.actor.size;
    let size = [sz.vx, sz.vy, sz.vz];
    let prev = ctx.prev.get(id.idx()).and_then(|p| p.as_ref());
    // Console fast path: not moved since the last tick and the same as at the last draw:
    // position, rotation and scale as converted then.
    let same = cfg!(feature = "lean") && prev == Some(&cur) && sl.last == Some((cur, size));
    use f2_engine::scene::{OBJECT_FLAGS_ADDITIVE, OBJECT_FLAGS_MODGE, OBJECT_FLAGS_WAVE};
    // Console fast path: an actor whose every draw the back end would cull (wholly outside
    // the view: most of a level's actors) is left out before any interpolation or transform.
    // `bound` encloses every vertex the back end draws for any pose (MODGE/WAVE included).
    // The drawn position lies on the segment from the previous position to the current one
    // (or is the current one), so within their Manhattan distance of the current one.
    #[cfg(feature = "lean")]
    let mut culled = false;
    #[cfg(feature = "lean")]
    if let Some(rb) = sl.bound {
        let p = cur.pos;
        // ×0.1 instead of /10: within the slack.
        let c = [p[0] as f32 * 0.1, p[1] as f32 * 0.1, p[2] as f32 * 0.1];
        let moved = prev.map_or(0, |q| (0..3).map(|k| (p[k] as i32 - q.pos[k] as i32).unsigned_abs()).sum::<u32>());
        let amax = size.iter().map(|&c| c.unsigned_abs()).max().unwrap_or(0) as f32 / 4096.0;
        // Slack for rounding (the back end's matrices, the conversions).
        if r.actor_culled(&c, rb * amax * 1.01 + moved as f32 * 0.1 + 1.0) {
            if r.cull_check() {
                // Testing: drawn anyway, the back end checks it culls every draw.
                culled = true;
                r.mark_culled(true);
            } else {
                if sl.ndraws != u32::MAX {
                    r.skip_draws(sl.ndraws);
                } else {
                    skip_first(r, &scene.banks[b].models[m], mdx, &mut sl.actor);
                    sl.ndraws = sl.actor.mesh_draws.as_ref().map_or(u32::MAX, |d| d.1.len() as u32);
                }
                return;
            }
        }
    }
    if !same {
        let (pos, q) = lerp_snap(prev, &cur, ctx.alpha);
        ea.pos = pos;
        ea.q_rot = q;
        ea.scale = [sz.vx as f32 / 4096.0, sz.vy as f32 / 4096.0, sz.vz as f32 / 4096.0];
        // Only an unmoved actor's conversion is reusable (an interpolated one depends on
        // alpha).
        sl.last = (prev == Some(&cur)).then_some((cur, size));
    }
    let anim = mdx.animation.as_ref();
    ea.animated = anim.is_some();
    ea.anim_time = anim.map_or(0.0, |an| an.anim_time);
    ea.extra_flags = mdx.root_object_flags & (OBJECT_FLAGS_ADDITIVE | OBJECT_FLAGS_MODGE | OBJECT_FLAGS_WAVE);
    ea.wrap_tc = mdx.flags & f2_game::mdx_actor::mdx_flags::ACTOR_WRAPTC != 0;
    ea.modge_frame = pc.modge_frame;
    // `overrideTex` (e.g. `multihidden.bmp` on locked portraits, baby frog colours).
    ea.override_tex = match a2.override_tex.as_deref() {
        None => None,
        Some(n) => override_tex(r, &mut sl.override_crc, n),
    };
    // `SetActorGouraudValues` on the multiplayer portraits (options.c:685).
    ea.gouraud = if pc.portraits { portrait_gouraud(game, id) } else { None };
    r.draw_actor(&scene.banks[b].models[m], &mut sl.actor);
    #[cfg(feature = "lean")]
    {
        sl.ndraws = sl.actor.mesh_draws.as_ref().map_or(1, |d| d.1.len() as u32);
        if culled {
            r.mark_culled(false);
        }
    }
}

/// A culled actor that has never been drawn: its draw count from its draw list.
#[cfg(feature = "lean")]
#[cold]
#[inline(never)]
fn skip_first(r: &mut Renderer, model: &f2_formats::objbank::Model, mdx: &f2_game::mdx_actor::MdxActor, ea: &mut Actor) {
    use f2_engine::scene::{OBJECT_FLAGS_ADDITIVE, OBJECT_FLAGS_MODGE, OBJECT_FLAGS_WAVE};
    ea.extra_flags = mdx.root_object_flags & (OBJECT_FLAGS_ADDITIVE | OBJECT_FLAGS_MODGE | OBJECT_FLAGS_WAVE);
    ea.wrap_tc = mdx.flags & f2_game::mdx_actor::mdx_flags::ACTOR_WRAPTC != 0;
    r.skip_actor(model, ea);
}

/// A new [`ActorSlot`] for `model_id` (false: the model is not loaded). Out of line: rare.
#[cold]
#[inline(never)]
fn new_slot(
    r: &Renderer,
    slot: &mut Option<ActorSlot>,
    models: &BTreeMap<u32, (usize, usize)>,
    bounds: &mut BTreeMap<u32, Option<f32>>,
    banks: &[ObjectBank],
    model_id: u32,
) -> bool {
    let Some(&bm) = models.get(&model_id) else { return false };
    let bound = if cfg!(feature = "lean") { *bounds.entry(model_id).or_insert_with(|| r.actor_bound(&banks[bm.0].models[bm.1])) } else { None };
    *slot = Some(ActorSlot { model_id, bound, ndraws: u32::MAX, last: None, bm, override_crc: None, actor: Actor::default() });
    true
}

/// `textures.by_name(lowercase(name))`, with the name's CRC kept while the name stays.
#[inline(never)]
fn override_tex(r: &Renderer, cache: &mut Option<(String, u32)>, n: &str) -> Option<TexHandle> {
    let crc = match cache {
        Some((s, c)) if s == n => *c,
        _ => {
            let c = f2_formats::crc(&n.to_ascii_lowercase());
            *cache = Some((n.into(), c));
            c
        }
    };
    r.textures.current(crc)
}

/// The Gouraud colour of a multiplayer portrait actor (`SetActorGouraudValues`).
#[inline(never)]
fn portrait_gouraud(game: &Game, id: ActorId) -> Option<[f32; 3]> {
    game.options.portrait_actor.iter().position(|&p| p == Some(id)).map(|i| game.options.portrait_gouraud[i].map(|c| c as f32 / 255.0))
}

/// `timeInfo.frameCount` (60 Hz frames since `InitTiming`, held in the pause menu), which
/// drives the MODGE/WAVE water wobble; `actFrameCount` under the fixed-step `Game::tick`.
fn modge_frame(game: &Game) -> u32 {
    if game.timing.active {
        game.timing.time_info.frame_count
    } else {
        game.act_frame_count
    }
}

/// `slideSpeeds[]` of the retail PC exe (`0x47c278`; the PSX table is `{0, 1, 2, 4}`).
const SLIDE_SPEEDS: [i32; 4] = [0, 16, 32, 64];

/// The texture sliding of `CopyActorList` (PC/Main.cpp:621, retail `0x43cac0`), run once
/// per game frame after the tick: every actor with `ACTOR_SLIDYTEX`/`ACTOR_SLIDYTEX2`
/// (`slideSpeeds[(flags>>5)&3]·gameSpeed>>12`) or `ACTOR_SLOWSLIDE` (`gameSpeed>>8`) scrolls
/// its model's texture v coordinates (`SlideObjectTextures`). The original edits the mesh,
/// which all actors of a model share (`MakeUniqueObject` copies objects, not meshes), so
/// each such actor slides it again, in `actList` order. Flowing water / sludge / lava
/// (`wat_f*`, `slu_f*`, `xx_g_*`) and the scenery strips `spl_*`/`spm_*`/`spf_*` (e.g. the
/// Subterranean ant columns) move this way. Render state only; the simulation is read.
pub fn slide_object_textures(scene: &mut LevelScene, game: &Game) {
    texture_slides(game, |mid, speed| {
        if let Some(&(b, m)) = scene.models.get(&mid) {
            slide_model(&mut scene.banks[b].models[m], speed);
        }
    });
}

/// The slides of [`slide_object_textures`] in order, as `f(model id, speed)` (`speed`: what
/// `SlideObjectTextures` subtracts from every `faceTC.v` of the model).
pub fn texture_slides(game: &Game, mut f: impl FnMut(u32, i32)) {
    use f2_game::actor2::ACTOR_SLOWSLIDE;
    let gs = game.game_speed;
    for id in game.actor2.act_list.ids(&game.actor2.actors) {
        let a2 = &game.actor2.actors[id];
        if !actor_slides_texture(a2) {
            continue;
        }
        let Some(mid) = a2.actor.actual_actor.object_controller else { continue };
        let slide = ((a2.flags >> 5) & 3) as usize;
        if slide != 0 {
            f(mid, SLIDE_SPEEDS[slide].wrapping_mul(gs) >> 12);
        }
        if a2.flags & ACTOR_SLOWSLIDE != 0 {
            f(mid, gs >> 8);
        }
    }
}

/// Whether actor `a2`'s texture slides (`ACTOR_SLIDYTEX`/`ACTOR_SLIDYTEX2`/`ACTOR_SLOWSLIDE`,
/// set when the actor is created, from its name).
pub fn actor_slides_texture(a2: &f2_game::actor2::Actor2) -> bool {
    a2.flags & (3 << 5 | f2_game::actor2::ACTOR_SLOWSLIDE) != 0
}

/// The total `faceTC.v` slide of one game frame at game speed `gs` of an actor with `flags`
/// (the sum of its [`texture_slides`]): for back ends that do not edit the meshes (the N64
/// offsets the texture coordinates per draw) and keep their own list of the sliding actors.
pub fn texture_slide_speed(flags: u16, gs: i32) -> i32 {
    let slide = ((flags >> 5) & 3) as usize;
    let mut speed = 0i32;
    if slide != 0 {
        speed = speed.wrapping_add(SLIDE_SPEEDS[slide].wrapping_mul(gs) >> 12);
    }
    if flags & f2_game::actor2::ACTOR_SLOWSLIDE != 0 {
        speed = speed.wrapping_add(gs >> 8);
    }
    speed
}

/// `SlideObjectTextures(objectController->object, speed)` on the root and (`next`) its
/// siblings, recursing into the children.
fn slide_model(model: &mut f2_formats::objbank::Model, speed: i32) {
    slide_object(&mut model.root, speed);
    for o in &mut model.root_siblings {
        slide_object(o, speed);
    }
}

/// Port of `SlideObjectTextures` (mdxObject.cpp:51, retail `0x44b540`, hardware
/// thresholds): each face's three `faceTC.v` move by `-speed`; when one drops below 4096
/// all three go up by 8192 (8 texture repeats, invisible with wrapping coordinates).
fn slide_object(o: &mut f2_formats::objbank::Object, speed: i32) {
    const THRESH: i32 = 4096;
    const ADD: i32 = 8192;
    if let Some(mesh) = &mut o.mesh {
        for f in mesh.face_uv.as_chunks_mut::<3>().0 {
            for t in f.iter_mut() {
                t[1] = (t[1] as i32).wrapping_sub(speed) as i16;
            }
            if f.iter().any(|t| (t[1] as i32) < THRESH) {
                for t in f.iter_mut() {
                    t[1] = (t[1] as i32).wrapping_add(ADD) as i16;
                }
            }
        }
    }
    for c in &mut o.children {
        slide_object(c, speed);
    }
}

/// Enhanced Hor+ wider than 4:3: the object sprites (e.g. the sarge ants' bodies) of the
/// actors the simulation's `ActorListDraw` culled against its 640×480 view, posed render-side.
/// The simulation kills such sprites (`KillObjectSprites`, `draw = 0`) and does not move
/// them, but their meshes are drawn regardless, so without this a wide screen shows the
/// legs without the body. Those actors also do not animate (`UpdateAnims` is skipped), so
/// mesh and sprites keep the pose of their last on-screen frame, as in retail.
/// The simulation state is only read.
pub fn widened_object_sprites(game: &Game) -> BTreeMap<f2_game::ids::SpriteId, f2_game::sprite::Sprite> {
    let mut out = BTreeMap::new();
    for list in 0..2 {
        for id in game.actor_draw_list(list) {
            let a2 = &game.actor2.actors[id];
            if a2.draw == 0 || a2.flags & f2_game::actor2::ACTOR_DRAW_NEVER != 0 || a2.actor.actual_actor.placement.draw == 0 {
                continue;
            }
            let Some((_, ptrs)) = a2.obj_sprites.as_ref() else { continue };
            // `ActorListDraw` transformed (and so placed) the sprites of every actor it drew;
            // all of them off means it culled this one.
            if ptrs.iter().flatten().any(|&sid| game.sprite.get(sid).draw != 0) {
                continue;
            }
            out.extend(game.posed_object_sprites(id));
        }
    }
    out
}

pub fn tl(sx: f32, sy: f32, vz: f32, c: [u8; 4]) -> TlVertex {
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
    for t in &game.map.tiles {
        let geo = TileGeo { centre: sv_to_render(&t.centre), normal: sv_dir(&t.normal), dir: sv_dir(&t.dir_vector[0]) };
        let mut col = tile_color(t.state);
        col[3] = 150;
        draw_tile_quad(r, &geo, 7.0, col);
    }
}

/// A tile's frame in render units: centre, unit normal, first direction vector.
pub struct TileGeo {
    pub centre: V3,
    pub normal: V3,
    pub dir: V3,
}

/// A translucent square of half-size `half` lying on the tile.
pub fn draw_tile_quad(r: &mut Renderer, t: &TileGeo, half: f32, col: [u8; 4]) {
    let n = norm(t.normal);
    let d = t.dir;
    let a = if dot(d, d) < 1e-6 { norm(cross(n, [1.0, 0.0, 0.0])) } else { norm(sub(d, mul(n, dot(d, n)))) };
    let b = cross(n, a);
    let o = add(t.centre, mul(n, 1.5));
    let q = [
        add(o, add(mul(a, half), mul(b, half))),
        add(o, sub(mul(a, half), mul(b, half))),
        sub(o, add(mul(a, half), mul(b, half))),
        add(o, sub(mul(b, half), mul(a, half))),
    ];
    push_world_tri(r, [q[0], q[1], q[2]], col);
    push_world_tri(r, [q[2], q[3], q[0]], col);
}

pub fn tile_color(s: u32) -> [u8; 4] {
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
