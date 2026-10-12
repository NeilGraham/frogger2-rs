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
    // Not moved since the last tick: `quat_slerp(q, q, alpha)` is `q` up to rounding.
    #[cfg(feature = "lean")]
    if p.pos == cur.pos && p.q == cur.q {
        return cur;
    }
    // Teleports (respawn, level reset, a path entity wrapping back to its first node) are not
    // interpolated: more than 1.5 tiles (90 render units) in one 60 Hz tick is faster than
    // anything moves in the game. (400 let a mower's lane wrap through: one display frame at
    // the halfway point on high-refresh screens; the original drew only whole ticks.)
    let d = sub(cur.pos, p.pos);
    if dot(d, d) > 90.0 * 90.0 {
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
    pub models: BTreeMap<u32, (usize, usize)>,
    pub tex_files: Vec<String>,
    pub world_model: Option<(usize, usize)>,
    /// `farClip = fog.max * 0.1` (drawloop.cpp:198), render units.
    pub far_clip: f32,
    pub clear: [f64; 4],
    /// Engine actor per game actor slot (keyframe search hints persist across frames), with
    /// its model id and that model's `models` entry (looked up once, not every frame).
    pub actors: Vec<Option<(u32, (usize, usize), Actor)>>,
    /// Stand-in frog while `CreateFrogger` is not ported (drawn on the player start tile).
    pub placeholder_frog: Option<((usize, usize), Actor)>,
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
            draw_actor_list(r, scene, game, ctx, 0);
            if ctx.backdrop.is_none() && tiled.is_none() {
                fx::process_shadows(r, game);
            }
            draw_actor_list(r, scene, game, ctx, 1);
            if let Some(((b, m), a)) = &mut scene.placeholder_frog {
                r.draw_actor(&scene.banks[*b].models[*m], a);
            }
            let posed = if r.view.widened() { widened_object_sprites(game) } else { BTreeMap::new() };
            fx::print_sprites(r, game, ctx.far_clip, &posed);
            if !matches!(mode, MULTI_WINRACE_MODE | MULTI_WINBATTLE_MODE | MULTI_WINCOLLECT_MODE | LEVELCOMPLETE_MODE) {
                fx::draw_special_fx(r, game);
            }
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
}

/// Port of `DrawBackground` (drawloop.cpp:85): the sky actor follows the camera, no
/// far clipping.
fn draw_background(r: &mut Renderer, scene: &LevelScene, game: &Game, cam: &Camera) {
    let Some(bg) = game.actor2.back_gnd.and_then(|id| game.actor2.actors.get(id)) else { return };
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
fn draw_actor_list(r: &mut Renderer, scene: &mut LevelScene, game: &Game, ctx: &DrawCtx, list: u32) {
    if scene.actors.len() < game.actor2.actors.slots.len() {
        scene.actors.resize_with(game.actor2.actors.slots.len(), || None);
    }
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
        let slot = &mut scene.actors[id.idx()];
        if slot.as_ref().is_none_or(|(mid, _, _)| *mid != model_id) {
            let Some(&bm) = scene.models.get(&model_id) else { continue };
            *slot = Some((model_id, bm, Actor::default()));
        }
        let (_, (b, m), ea) = slot.as_mut().unwrap();
        let (b, m) = (*b, *m);
        let cur = Snap { pos: sv_to_render(&a2.actor.position), q: iq_to_quat(&a2.actor.q_rot) };
        let s = lerp_snap(ctx.prev.get(id.idx()).and_then(|p| p.as_ref()), cur, ctx.alpha);
        ea.pos = s.pos;
        ea.q_rot = s.q;
        let sz = a2.actor.size;
        ea.scale = [sz.vx as f32 / 4096.0, sz.vy as f32 / 4096.0, sz.vz as f32 / 4096.0];
        ea.animated = mdx.animation.is_some();
        ea.anim_time = mdx.animation.as_ref().map_or(0.0, |an| an.anim_time);
        use f2_engine::scene::{OBJECT_FLAGS_ADDITIVE, OBJECT_FLAGS_MODGE, OBJECT_FLAGS_WAVE};
        ea.extra_flags = mdx.root_object_flags & (OBJECT_FLAGS_ADDITIVE | OBJECT_FLAGS_MODGE | OBJECT_FLAGS_WAVE);
        ea.wrap_tc = mdx.flags & f2_game::mdx_actor::mdx_flags::ACTOR_WRAPTC != 0;
        ea.modge_frame = modge_frame(game);
        // `overrideTex` (e.g. `multihidden.bmp` on locked portraits, baby frog colours).
        ea.override_tex = a2.override_tex.as_deref().and_then(|n| r.textures.by_name(&n.to_ascii_lowercase()));
        // `SetActorGouraudValues` on the multiplayer portraits (options.c:685).
        ea.gouraud = game.options.portrait_actor.iter().position(|&p| p == Some(id)).map(|i| game.options.portrait_gouraud[i].map(|c| c as f32 / 255.0));
        r.draw_actor(&scene.banks[b].models[m], ea);
    }
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
    use f2_game::actor2::ACTOR_SLOWSLIDE;
    let gs = game.game_speed;
    for id in game.actor2.act_list.ids(&game.actor2.actors) {
        let a2 = &game.actor2.actors[id];
        let slide = ((a2.flags >> 5) & 3) as usize;
        if slide == 0 && a2.flags & ACTOR_SLOWSLIDE == 0 {
            continue;
        }
        let Some(&(b, m)) = a2.actor.actual_actor.object_controller.and_then(|mid| scene.models.get(&mid)) else { continue };
        let model = &mut scene.banks[b].models[m];
        if slide != 0 {
            slide_model(model, SLIDE_SPEEDS[slide].wrapping_mul(gs) >> 12);
        }
        if a2.flags & ACTOR_SLOWSLIDE != 0 {
            slide_model(model, gs >> 8);
        }
    }
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
