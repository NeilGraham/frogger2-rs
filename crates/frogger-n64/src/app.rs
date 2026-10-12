//! The N64 front end: a lean counterpart of the desktop `frogger::app::App` around the
//! unchanged `f2-game` simulation and the shared `f2-draw` draw loop.
//!
//! - **Loop**: fixed 60 Hz ticks paced by the VR4300 count register; each frame runs as many
//!   ticks as real time requires (at most [`MAX_TICKS_PER_FRAME`]; beyond that the backlog is
//!   dropped and counted) and then renders once, so the simulation keeps real time and the
//!   frame rate is whatever rendering allows.
//! - **Data**: [`N64Data`] over the DFS (`data.rs`); levels load from inside the
//!   simulation (`InitLevel` → `take_level_loads`).
//! - **Video**: logo/story FMVs (`Effect::PlayVideo`) play blocking with libdragon's
//!   `fmv_play` over the game's display (`rom:/video/<name>.h264` + `.wav64`).
//! - **Saves**: the PC save block in cartridge SRAM (`save.rs`): loaded at boot,
//!   written on `Effect::SaveGame`.
//! - **Rendering**: `render_r3d.rs` (meshes on the RSP, the rest through `render_cpu.rs`);
//!   with the `render-cpu` feature the phase-2G CPU renderer draws everything.

use crate::audio::Audio;
use crate::data::{norm_path, AssetStore, DfsStore, N64Data};

/// The ROM's asset store: the 1C pack (default) or the phase-2G loose layout.
#[cfg(not(feature = "prep-data"))]
pub type Store = crate::data::PackStore<DfsStore>;
#[cfg(feature = "prep-data")]
pub type Store = DfsStore;

fn new_data() -> N64Data<Store> {
    #[cfg(not(feature = "prep-data"))]
    return N64Data::new_pack(crate::data::PackStore::new(DfsStore));
    #[cfg(feature = "prep-data")]
    return N64Data::new(DfsStore);
}
use crate::input;
use crate::log;
use crate::render_cpu::RenderCpu;
use crate::rt::{self, ticks, ticks_to_us, TICKS_PER_SEC};
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use f2_draw::overlay::{GameFonts, TileScroll};
use f2_draw::render::{self, DrawCtx, LevelScene, Snap};
use f2_engine::{Camera, Font, HiRes, Renderer, TexHandle};
use f2_game::assets::DataSource;
use f2_game::session::SessionConfig;
use f2_game::{Effect, Game, TickInput};

/// C0 COUNT ticks per 60 Hz game tick.
pub const TICK: u32 = TICKS_PER_SEC / 60;
/// Most simulation ticks run before one rendered frame (catch-up limit).
pub const MAX_TICKS_PER_FRAME: u32 = 6;
/// Whether frames are capped to an even divisor of 60 ([`pick_fps`]; build-time
/// `F2N64_PACE=on`). Off: frames are shown as soon as they are ready.
pub const PACE: bool = match option_env!("F2N64_PACE") {
    Some(s) => matches!(s.as_bytes(), b"on"),
    None => false,
};
/// Frame rates the pacing picks from (frames shown per second, divisors of 60).
pub const PACED_FPS: [u32; 4] = [60, 30, 20, 15];

/// Frame pacing: the highest of [`PACED_FPS`] at which the simulation keeps real time and
/// the frames fit into `budget_us` per second, from running averages of the cost of a tick
/// and of a frame; 0 (no limit: as fast as possible) if even the lowest does not fit
/// (`App::pace` adds hysteresis and requires a stable choice before switching).
/// Fixed timestep: 60 ticks a second whatever the frame rate. Variable timestep
/// ([`crate::pacing::VARIABLE`]): one tick per shown frame (covering `60 / fps` game frames, at
/// most [`crate::pacing::MAX_FRAMES_PER_TICK`]), so a frame costs a tick plus the drawing.
pub fn pick_fps(tick_us: u32, draw_us: u32, budget_us: u32) -> u32 {
    crate::pacing::pick_fps_mode(&PACED_FPS, tick_us, draw_us, budget_us, crate::pacing::VARIABLE, crate::pacing::MAX_FRAMES_PER_TICK)
}

/// Rank of a pacing choice (0 = 60 fps ... 4 = unlimited / overloaded).
fn fps_rank(f: u32) -> usize {
    PACED_FPS.iter().position(|&p| p == f).unwrap_or(PACED_FPS.len())
}
/// Render size (the RDP framebuffer).
pub const WIDTH: u32 = 320;
pub const HEIGHT: u32 = 240;
/// Strip pixels -> screen pixels of the game fonts at 320 wide (`PrintText`: x/640); a modern
/// atlas packed at a larger scale is drawn that much larger.
const N64_FONT_BASE: f32 = WIDTH as f32 / 640.0;

/// How the ROM starts (build-time `F2N64_BOOT`): `frontend` (default: logos → title →
/// frontend like the PC executable, progress from the SRAM save: a new player's story book
/// starts the training level), `frontend-trained` (testing: the training level marked done,
/// as a save after it would have it), `save-test` (frontend, and the save block written to
/// SRAM right after boot: the next boot must report it loaded), `level W L` (straight into
/// an arcade level), `story N` (story mode at `storySequence[N]`, with its FMV) or
/// `story-finish N` (testing: `story N`, the level completed after 4 s and the results
/// screens pressed through, into the next story level) or `frontend-start` (testing: the
/// frontend with Start pressed on the title screen, into the hub).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boot {
    Frontend { trained: bool },
    SaveTest,
    Level(u32, u32),
    Story(u32),
    StoryFinish(u32),
    FrontendStart,
}

impl Boot {
    pub fn parse(s: Option<&str>) -> Boot {
        let Some(s) = s else { return Boot::Frontend { trained: false } };
        let mut it = s.split_whitespace();
        match (it.next(), it.next().and_then(|w| w.parse().ok()), it.next().and_then(|l| l.parse().ok())) {
            (Some("level"), Some(w), Some(l)) => Boot::Level(w, l),
            (Some("story"), Some(n), _) => Boot::Story(n),
            (Some("story-finish"), Some(n), _) => Boot::StoryFinish(n),
            (Some("frontend-start"), _, _) => Boot::FrontendStart,
            (Some("frontend-trained"), _, _) => Boot::Frontend { trained: true },
            (Some("save-test"), _, _) => Boot::SaveTest,
            _ => Boot::Frontend { trained: false },
        }
    }
}

/// Frame-time accounting over a logging window.
#[derive(Default)]
struct Window {
    frames: u32,
    /// Inside `Game::tick` only / in `audio_poll` between ticks.
    game_tick: u32,
    poll: u32,
    ticks: u32,
    sim: u32,
    draw: u32,
    build: u32,
    submit: u32,
    tris: u32,
    rects: u32,
    max_frame: u32,
    /// `f2_draw::render::draw_loop` (the PC draw loop: matrices, sprites, effects, HUD).
    draw_loop: u32,
    /// Texture uploads / registry sync.
    upload: u32,
    /// Waiting for a free framebuffer (RSP/RDP or VI still busy).
    fb_wait: u32,
    /// r3d: mesh draws, triangles queued / sent to the RSP after culling.
    draws: u32,
    rsp_tris: u32,
    rsp_sent: u32,
    rsp_groups: u32,
    tex_loads: u32,
    /// r3d: queueing the recorded draws / flushing the lists (CPU).
    r3d_queue: u32,
    r3d_flush: u32,
    r3d_reused: u32,
    r3d_view_reused: u32,
}

/// The renderer side of the loaded level, outside [`App`] so the data source can free it
/// when the simulation starts loading the next level from inside `Game::tick` (`InitLevel`):
/// the PC order (load the new level, then drop the old scene) does not fit in 8 MiB.
pub struct Level {
    pub scene: Option<LevelScene>,
    pub gfx: RenderCpu,
    #[cfg(not(feature = "render-cpu"))]
    pub r3d: crate::render_r3d::R3dLevel,
    /// Registry images below this index (white, fonts) survive a level unload.
    pub keep_images: u32,
    /// The story book's page canvas (frontend only; 256 KiB).
    pub page_canvas: Option<f2_draw::book::Canvas>,
}

pub static LEVEL: spin::Mutex<Option<Level>> = spin::Mutex::new(None);

/// Set once a level's renderer side is loaded; the first data-source access after it (the
/// next `InitLevel`) frees that side ([`unload_level_render`]).
static ARMED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Frees the loaded level's object banks and textures (keeps the fonts). Called by the data
/// source at the start of a level load inside the simulation.
pub fn unload_level_render() {
    if !ARMED.swap(false, core::sync::atomic::Ordering::Relaxed) {
        return;
    }
    if let Some(l) = LEVEL.lock().as_mut() {
        l.scene = None;
        l.page_canvas = None;
        let keep = l.keep_images;
        l.gfx.free_from(keep);
        #[cfg(not(feature = "render-cpu"))]
        l.r3d.unload();
        log!("app: level render assets freed for the next load");
    }
}

fn lv() -> spin::MutexGuard<'static, Option<Level>> {
    LEVEL.lock()
}

pub struct App {
    data: Arc<N64Data<Store>>,
    /// NTX files of the level being loaded (pack layout).
    ntx_cache: crate::ntxload::NtxCache,
    game: Game,
    r: Renderer,
    audio: Audio,
    /// `bigfont.bmp` / `smallfont.bmp` found (reloaded from the ROM at each renderer rebuild
    /// so their pixels are not kept twice).
    have_fonts: bool,
    game_fonts: Option<GameFonts>,
    /// The game fonts' metrics after the first load (their N64 textures stay loaded).
    fonts_cache: Option<(Font, Font)>,
    /// The modern font atlases' layout (pixels dropped) when the game fonts are modern.
    modern_cache: Option<(HiRes, HiRes)>,
    backdrop: Option<(String, Option<TexHandle>)>,
    tile_scroll: TileScroll,
    prev: Vec<Option<Snap>>,
    prev_cam: Option<Camera>,
    acc: u32,
    last: u32,
    prev_pad: u16,
    // statistics
    pub total_ticks: u64,
    pub frames: u64,
    pub dropped_ticks: u64,
    win: Window,
    win_start: u32,
    level_name: String,
    level_peak: (u32, u32),
    /// A level loaded during this frame: its time is not frame time.
    loaded: bool,
    /// Phase 3T (`pacing.rs`): 60 Hz frames covered by the last tick, and game frames
    /// simulated in this perf window.
    step_frames: u32,
    pace_frames: u32,
    /// The story book's `fontWhite` (`monofont.bmp`) cut down to the glyphs the chapter
    /// pages use (the whole strip is ~635 KiB as RGBA8; this is a few tens of KiB).
    book_font: Option<Font>,
    last_mode: i32,
    last_pad: u16,
    /// `Boot::StoryFinish`: frames since boot while it is driving the game.
    autofinish: Option<u32>,
    /// `Boot::FrontendStart`: frames spent on the title screen.
    autostart: Option<u32>,
    /// Frame pacing: running averages (C0 ticks, x16 fixed point) of a simulation tick
    /// incl. audio polling and of a frame's drawing; the limit in force and a candidate.
    avg_tick: u32,
    avg_draw: u32,
    fps_limit: u32,
    fps_cand: (u32, u32),
}

fn load_font(d: &N64Data<Store>, p: &str) -> Option<Font> {
    d.store.load(p).and_then(|b| Font::from_bmp(&b))
}

impl App {
    pub fn new(boot: Boot) -> App {
        let mut data = new_data();
        data.before_access = Some(unload_level_render);
        let data = Arc::new(data);
        *LEVEL.lock() = Some(Level {
            scene: None,
            gfx: RenderCpu::new(),
            #[cfg(not(feature = "render-cpu"))]
            r3d: crate::render_r3d::R3dLevel::new(),
            keep_images: 0,
            page_canvas: None,
        });
        let mut app = App {
            data,
            ntx_cache: Default::default(),
            game: Game::new(),
            r: Renderer::new_soft(),
            audio: Audio::new(),
            have_fonts: true,
            game_fonts: None,
            fonts_cache: None,
            modern_cache: None,
            backdrop: None,
            tile_scroll: TileScroll::default(),
            prev: Vec::new(),
            prev_cam: None,
            acc: 0,
            last: ticks(),
            prev_pad: 0,
            total_ticks: 0,
            frames: 0,
            dropped_ticks: 0,
            win: Window::default(),
            win_start: ticks(),
            level_name: String::from("boot"),
            level_peak: (0, 0),
            loaded: false,
            step_frames: 1,
            pace_frames: 0,
            book_font: None,
            last_mode: -1,
            last_pad: 0,
            autofinish: None,
            autostart: None,
            avg_tick: 0,
            avg_draw: 0,
            fps_limit: 0,
            fps_cand: (0, 0),
        };
        app.rebuild_renderer();
        f2_draw::overlay::set_button_prompts(vec![('X', "A".into()), ('C', "B".into()), ('S', "Z".into()), ('T', "C-Up".into())]);
        let mut cfg = SessionConfig::new(app.data.clone());
        cfg.save_data = crate::save::load();
        log!("app: save game {}", if cfg.save_data.is_some() { "loaded from SRAM" } else { "not found (new player)" });
        let t0 = ticks();
        app.game = Game::new_session(cfg);
        app.game.set_game_options(f2_game::enhance::GameOptions { console: true, ..f2_game::enhance::GameOptions::AUTHENTIC });
        app.install_game_hooks();
        app.book_font = load_font(&app.data, "textures/font/monofont.bmp").map(|f| font_subset(&f, &book_chars(&app.game)));
        log!("app: session up in {} ms", ticks_to_us(ticks().wrapping_sub(t0)) / 1000);
        match boot {
            Boot::Frontend { trained } => {
                if trained {
                    let fe = f2_game::layout::worldid::FRONTEND as usize;
                    app.game.layout.world_visual_data[fe].level_visual_data[f2_game::layout::LEVELID_FRONTEND4 as usize].level_completed = 1;
                }
            }
            Boot::SaveTest => {
                app.game.save_game();
            }
            Boot::FrontendStart => app.autostart = Some(0),
            Boot::Level(w, l) => {
                log!("app: booting straight into level {w}-{l}");
                app.game.start_arcade(w, l);
            }
            Boot::Story(n) | Boot::StoryFinish(n) => {
                log!("app: booting straight into story level {n}");
                app.game.start_story(n);
                if matches!(boot, Boot::StoryFinish(_)) {
                    app.autofinish = Some(0);
                }
            }
        }
        app.dispatch_effects();
        app.process_level_loads();
        app.heap_log("boot");
        #[cfg(feature = "profile")]
        unsafe {
            crate::ffi::n64_prof(0);
            if cfg!(any(feature = "profile-sim", feature = "profile-draw")) {
                crate::ffi::n64_prof(2);
            }
        };
        app.last = ticks();
        app
    }

    /// `CalcStringWidth` font metrics for the simulation (textover.c), from the fonts the
    /// renderer holds.
    fn install_game_hooks(&mut self) {
        if let Some(f) = self.game_fonts {
            let w = |f: &Font| (0..=255u8).map(|c| f.char_width(c).min(255) as u8).collect::<Vec<u8>>();
            self.game.textover.metrics = Some(f2_game::textover::FontMetrics { normal: w(self.r.font(f.normal)), small: w(self.r.font(f.small)) });
        }
    }

    /// The game fonts' metrics (pixels dropped) with their modern atlases, when the pack has
    /// them.
    fn load_modern_fonts(&self) -> Option<((Font, HiRes), (Font, HiRes))> {
        let one = |bmp: &str, raw: &str| -> Option<(Font, HiRes)> {
            let hr = HiRes::decode_raw(&self.data.store.load(raw)?, true)?;
            let mut f = load_font(&self.data, bmp)?;
            f.rgba = Vec::new();
            Some((f, hr))
        };
        Some((one("textures/font/bigfont.bmp", "textures/font/bigfont.f2mr")?, one("textures/font/smallfont.bmp", "textures/font/smallfont.f2mr")?))
    }

    /// A fresh `Renderer` (its texture registry cannot free images) with the fonts.
    ///
    /// The game fonts' N64 textures survive (registry images 1 and 2 again): reloading
    /// `bigfont.bmp` needs a 427 KiB RGBA8 block, which a fragmented heap may not have.
    fn rebuild_renderer(&mut self) {
        let mut g = lv();
        let l = g.as_mut().unwrap();
        if self.fonts_cache.is_some() {
            l.gfx.free_from(l.keep_images);
        } else {
            l.gfx.reset();
        }
        self.r = Renderer::new_soft();
        #[cfg(not(feature = "render-cpu"))]
        {
            self.r.mesh_sink = Some(f2_engine::MeshSink::default());
        }
        self.backdrop = None;
        self.game_fonts = None;
        if let (Some((b, s)), Some((hb, hs))) = (&self.fonts_cache, &self.modern_cache) {
            let normal = self.r.add_font_hires_placeholder(b.clone(), hb);
            let small = self.r.add_font_hires_placeholder(s.clone(), hs);
            self.r.set_font_size_mul(normal, hb.scale / N64_FONT_BASE);
            self.r.set_font_size_mul(small, hs.scale / N64_FONT_BASE);
            self.game_fonts = Some(GameFonts { normal, small });
        } else if let Some((b, s)) = &self.fonts_cache {
            let normal = self.r.add_font_placeholder(b.clone());
            let small = self.r.add_font_placeholder(s.clone());
            self.game_fonts = Some(GameFonts { normal, small });
        } else if let Some(((b, hb), (s, hs))) = self.have_fonts.then(|| self.load_modern_fonts()).flatten() {
            // Modern fonts (f2-n64pack `textures/font/*.f2mr`): atlases already at the N64's
            // on-screen size; metrics from the bitmaps.
            let normal = self.r.add_font_hires(b.clone(), &hb, hb.scale);
            l.gfx.upload(&mut self.r.textures);
            let small = self.r.add_font_hires(s.clone(), &hs, hs.scale);
            l.gfx.upload(&mut self.r.textures);
            self.r.set_font_size_mul(normal, hb.scale / N64_FONT_BASE);
            self.r.set_font_size_mul(small, hs.scale / N64_FONT_BASE);
            self.game_fonts = Some(GameFonts { normal, small });
            let meta = |h: &HiRes| HiRes { rgba: Vec::new(), ..h.clone() };
            self.modern_cache = Some((meta(&hb), meta(&hs)));
            self.fonts_cache = Some((b, s));
            log!("app: modern fonts {}x{} + {}x{}", hb.width, hb.height, hs.width, hs.height);
        } else if self.have_fonts {
            let b = load_font(&self.data, "textures/font/bigfont.bmp").map(|f| self.r.add_font(f));
            l.gfx.upload(&mut self.r.textures);
            let s = load_font(&self.data, "textures/font/smallfont.bmp").map(|f| self.r.add_font(f));
            l.gfx.upload(&mut self.r.textures);
            match (b, s) {
                (Some(normal), Some(small)) => {
                    self.game_fonts = Some(GameFonts { normal, small });
                    let meta = |f: &Font| Font { width: f.width, height: f.height, glyphs: f.glyphs, rgba: Vec::new() };
                    self.fonts_cache = Some((meta(self.r.font(normal)), meta(self.r.font(small))));
                }
                _ => {
                    log!("app: game fonts (bigfont.bmp/smallfont.bmp) missing");
                    self.have_fonts = false;
                }
            }
        }
        l.keep_images = self.r.textures.images.len() as u32;
    }

    /// One line of the level-load memory breakdown (`heap step`).
    fn heap_step(&self, what: &str) {
        let (live, _) = rt::rust_heap();
        let h = rt::heap_stats();
        log!("heap step [{}] after {what}: rust live {} KiB, libdragon used {} KiB", self.level_name, live / 1024, h.used / 1024);
    }

    fn heap_log(&mut self, what: &str) {
        let (live, peak) = rt::rust_heap();
        let h = rt::heap_stats();
        log!(
            "heap [{}] {what}: rust live {} KiB peak {} KiB; libdragon used {} / {} KiB; N64 textures {} KiB",
            self.level_name,
            live / 1024,
            peak / 1024,
            h.used / 1024,
            h.total / 1024,
            lv().as_ref().map_or(0, |l| l.gfx.tex_bytes) / 1024
        );
        self.level_peak = (self.level_peak.0.max(peak), self.level_peak.1.max(h.used));
    }

    // ------------------------------------------------------------ levels

    fn process_level_loads(&mut self) {
        if let Some(l) = self.game.take_level_loads().pop() {
            self.load_scene(l.world, l.level, &l.assets);
        }
    }

    fn load_scene(&mut self, world: u32, level: u32, assets: &f2_game::layout::LevelAssets) {
        let t0 = ticks();
        if self.level_name != "boot" {
            log!("heap [{}] level peak: rust {} KiB, libdragon used {} KiB", self.level_name, self.level_peak.0 / 1024, self.level_peak.1 / 1024);
        }
        self.level_name = format!("{world}-{level}");
        self.level_peak = (0, 0);
        // Free the previous level's renderer side first (RDRAM).
        ARMED.store(false, core::sync::atomic::Ordering::Relaxed);
        if let Some(l) = lv().as_mut() {
            l.scene = None;
            #[cfg(not(feature = "render-cpu"))]
            l.r3d.unload();
        }
        self.rebuild_renderer();
        self.audio.free_samples();
        // The story book's page canvas (256 KiB) exists only in the frontend, allocated
        // first while the heap is emptiest (later a block that size can fail to fit).
        if world == f2_game::layout::worldid::FRONTEND {
            if let Some(l) = lv().as_mut() {
                l.page_canvas.get_or_insert_with(|| f2_draw::book::page_canvas(1));
            }
        }
        rt::reset_peak();
        // Textures first: the NTX bank files are the largest single blocks of a level and
        // need contiguous free memory, which the many small allocations of the object banks
        // and meshes fragment.
        for f in &assets.texture_banks {
            if self.data.pack {
                let p = crate::data::ntx_bank_path(f);
                let mut g = lv();
                let gfx = &mut g.as_mut().unwrap().gfx;
                if let Err(e) = crate::ntxload::load_bank(&self.data.store, &p, &mut self.r.textures, gfx, &mut self.ntx_cache) {
                    log!("app: {e}");
                }
                continue;
            }
            match self.data.texture_bank(f) {
                Some(Ok(b)) => {
                    let mut g = lv();
                    let gfx = &mut g.as_mut().unwrap().gfx;
                    self.r.textures.load_bank_with(&b, |h, img, t| gfx.convert_image(h.0, img, t));
                }
                Some(Err(e)) => log!("app: {e}"),
                None => log!("app: {f}: not found"),
            }
            rt::audio_poll();
        }
        // The textures hold on to the files they use.
        self.ntx_cache.clear();
        self.heap_step("textures");
        let mut banks = Vec::new();
        for f in &assets.object_banks {
            match self.data.object_bank(f) {
                Some(Ok(b)) => banks.push(b),
                Some(Err(e)) => log!("app: {e}"),
                None => log!("app: {f}: not found"),
            }
            rt::audio_poll();
        }
        self.heap_step("object banks");
        #[cfg(not(feature = "render-cpu"))]
        let banks = self.load_meshes(&assets.object_banks, banks);
        self.heap_step("r3d meshes");
        let scene = LevelScene::new(world, level, banks, assets.texture_banks.clone(), &self.game);
        lv().as_mut().unwrap().scene = Some(scene);
        self.load_sfx(world, level);
        self.heap_step("sound effects");
        self.prev = render::snapshot(&self.game);
        self.prev_cam = None;
        self.update_game_view();
        log!(
            "app: level {world}-{level} loaded in {} ms: {} actors, {} tiles, banks {:?}, textures {:?}",
            ticks_to_us(ticks().wrapping_sub(t0)) / 1000,
            self.game.actor2.act_list.ids(&self.game.actor2.actors).len(),
            self.game.map.tiles.len(),
            assets.object_banks,
            assets.texture_banks
        );
        self.heap_log("loaded");
        ARMED.store(true, core::sync::atomic::Ordering::Relaxed);
        // The load took real time: do not try to catch it up.
        self.acc = 0;
        self.last = ticks();
        self.loaded = true;
    }

    /// The banks' F2NM render meshes into r3d; returns the banks without their render
    /// geometry (the draw loop only needs hierarchies, keys and flags from them).
    #[cfg(not(feature = "render-cpu"))]
    fn load_meshes(&mut self, files: &[String], mut banks: Vec<f2_formats::objbank::ObjectBank>) -> Vec<f2_formats::objbank::ObjectBank> {
        // The model index LevelScene builds (first bank wins).
        let mut models = alloc::collections::BTreeMap::new();
        for (bi, b) in banks.iter().enumerate() {
            for (mi, m) in b.models.iter().enumerate() {
                models.entry(m.id).or_insert((bi, mi));
            }
        }
        let mut g = lv();
        let l = g.as_mut().unwrap();
        let mut total = 0;
        // `banks` holds the banks that loaded, in `files` order.
        let mut bi = 0;
        for f in files {
            if bi >= banks.len() {
                break;
            }
            let p = norm_path(f) + crate::data::F2NM_SUFFIX;
            let Some(bytes) = self.data.store.load(&p) else {
                log!("app: {p}: not found");
                continue;
            };
            match l.r3d.add_bank(bi, bytes, &banks, &models) {
                Ok(n) => total += n,
                Err(e) => log!("app: {p}: {e:?}"),
            }
            bi += 1;
            rt::audio_poll();
        }
        // Models showing the story book's render-to-texture pages (256x256: more than r3d's
        // one TMEM load) stay on the CPU path, which samples large textures in TMEM tiles.
        let targets = [f2_formats::crc("page256a.bmp"), f2_formats::crc("page256b.bmp")];
        for b in &mut banks {
            for m in &mut b.models {
                if f2_n64fmt::bank::uses_texture(m, &targets) {
                    self.r.cpu_models.push(m.id);
                } else {
                    f2_n64fmt::bank::strip_model(m);
                }
            }
        }
        log!("app: {total} r3d meshes for {} banks", banks.len());
        banks
    }

    /// `LoadSfx(worldID)` file part (pcaudio.c:186) + `InitVoices` (frogger.c:78).
    fn load_sfx(&mut self, world: u32, level: u32) {
        let world_id = self.game.layout.world_visual_data[world as usize].world_id as i16;
        let mut files = vec![String::from("sfx/generic.wus")];
        if self.game.game_state.multi != f2_game::define::SINGLEPLAYER {
            files.push("sfx/multi.wus".into());
        }
        if let Some(b) = self.game.registry_file(world, level, |f| &f.sfx_bank) {
            files.push(b);
        } else if let Some(b) = world_bank(world_id) {
            files.push(format!("sfx/{b}"));
        }
        for path in files {
            match self.data.sfx_refs(&path) {
                Some(r) => self.audio.load_bank(&r),
                None => log!("app: {path}: not found"),
            }
        }
        const VOICE_BANKS: [&str; 8] = ["frogger", "lillie", "tad", "treeby", "wart", "tank", "roobie", "swampy"];
        for i in 0..self.game.frogger.num_frogs.clamp(0, 4) as usize {
            let Some(name) = VOICE_BANKS.get(self.game.frogger.player[i].character as usize) else { continue };
            let path = format!("sfx/{name}.wus");
            if let Some(r) = self.data.sfx_refs(&path) {
                self.audio.load_voice_bank(i, &r);
            }
        }
    }

    fn update_game_view(&mut self) {
        let g = lv();
        let Some(s) = g.as_ref().and_then(|l| l.scene.as_ref()) else { return };
        let far = render::far_clip(&self.game, s);
        self.game.specfx.view = Some(render::spec_fx_view(&self.game, far));
    }

    // ------------------------------------------------------------ simulation

    /// One simulation tick; `last`: the last one before the next drawn frame (only its
    /// starting state is needed for interpolation, so only then is it snapshotted).
    fn tick(&mut self, pad: u16, last: bool, frames: u32) {
        if last {
            self.prev = render::snapshot(&self.game);
            self.prev_cam = Some(render::game_camera(&self.game));
        }
        if self.game.frontend.text_entry > 0 {
            self.game.psx_name_entry_frame(pad & !self.prev_pad);
        }
        self.prev_pad = pad;
        let t0 = ticks();
        #[cfg(feature = "profile-sim")]
        unsafe {
            crate::ffi::n64_prof(3)
        };
        let variable = self.variable_step();
        crate::pacing::tick(&mut self.game, TickInput { pads: [pad, 0, 0, 0] }, frames, variable);
        self.step_frames = frames;
        self.pace_frames += frames;
        #[cfg(feature = "profile-sim")]
        unsafe {
            crate::ffi::n64_prof(2)
        };
        self.win.game_tick += ticks().wrapping_sub(t0);
        self.total_ticks += 1;
        if self.game.game_state.mode as i32 != self.last_mode || pad != self.last_pad {
            let p = &self.game.frogger.player[0];
            log!(
                "game: tick {} mode {} single {} world {} level {} pad {:04x} tile {}",
                self.total_ticks,
                self.game.game_state.mode,
                self.game.game_state.single,
                p.world_num,
                p.level_num,
                pad,
                self.game.game.curr_tile_num
            );
            self.last_mode = self.game.game_state.mode as i32;
            self.last_pad = pad;
        }
        self.dispatch_effects();
        self.process_level_loads();
        self.grab_textures();
        self.update_game_view();
        if self.game.backdrop.tiled_backdrop.is_some() {
            self.tile_scroll.step(self.game.game_speed);
        }
    }

    /// The PC renderer's texture grab after a frame (drawloop.cpp:376): the story book page
    /// composed on the CPU (`f2_draw::book`) into `page256a/b.bmp`.
    ///
    /// `f2_draw::book::process_grab` without the registry's RGBA8 copy of the page (256 KiB
    /// per grab, RDRAM is tight): the composed page goes straight into the N64 texture.
    fn grab_textures(&mut self) {
        let Some(target) = f2_draw::book::grab_target(&self.game) else { return };
        // The frontend's page buffer (allocated by load_scene, freed with the level).
        let mut g = lv();
        let Some(l) = g.as_mut() else { return };
        let c = l.page_canvas.get_or_insert_with(|| f2_draw::book::page_canvas(1));
        f2_draw::book::draw_page_b_into(&self.r, &self.game, self.book_font.as_ref(), c);
        self.game.options.grab_to_texture = if self.game.options.grab_to_texture == 1 { 0 } else { 2 };
        let Some(h) = self.r.textures.entry(f2_formats::crc(target)).map(|e| e.image) else {
            log!("app: book page texture {target} not loaded");
            return;
        };
        // Half size (128x128 RGBA16, 32 KiB instead of 128 per page): the full-size pages
        // do not fit the frontend's RDRAM next to everything else (measured: out of memory
        // on the second page). The page shows ~110 pixels tall at 320x240.
        let (w, h2, half) = crate::texconv::halve_rgba8(c.w, c.h, &c.rgba);
        if let Some(img) = self.r.textures.images.get_mut(h.0 as usize) {
            img.width = w;
            img.height = h2;
            img.rgba = Vec::new();
        }
        l.gfx.grab_rgba(h.0, w, h2, &half);
    }

    fn dispatch_effects(&mut self) {
        for e in self.game.take_effects() {
            self.handle_effect(e);
        }
    }

    fn handle_effect(&mut self, e: Effect) {
        let sv = |p: f2_math::SVector| [p.vx, p.vy, p.vz];
        match e {
            Effect::PlaySample { uid, pos, radius, volume, pitch, handle } => {
                if let Some(v) = self.audio.play(uid, volume, pitch, pos.map(sv), radius) {
                    if handle != 0 {
                        self.audio.bind(handle, v);
                    }
                }
            }
            Effect::StopSample { uid } => self.audio.stop_sample(uid),
            Effect::PlaySfxMapped { uid, pos, .. } => {
                self.audio.play(uid, crate::audio::SAMPLE_VOLUME, -1, Some(sv(pos)), 0);
            }
            Effect::PlayVoice { player, uid } => {
                self.audio.play_voice(player as usize, uid);
            }
            Effect::StopHandle { handle } => self.audio.stop_handle(handle),
            Effect::UpdateHandle { handle, volume, pitch, pos, radius } => self.audio.update_handle(handle, volume, pitch, pos.map(sv), radius),
            Effect::PlayMusic { track, looped } => self.audio.play_music(track, looped),
            Effect::StopMusic => self.audio.stop_music(),
            Effect::PlayVideo { name } => self.play_video(&name),
            Effect::SaveGame { data } => {
                let ok = crate::save::store(&data);
                log!("app: save game ({} bytes) {}", data.len(), if ok { "written to SRAM" } else { "NOT written (no SRAM)" });
            }
            Effect::Rumble { .. } => {}
            Effect::Log { text } => {
                if cfg!(feature = "game-log") {
                    log!("game: {text}");
                }
            }
            Effect::SetMusicVolume { vol, .. } => self.audio.set_music_volume(vol),
            Effect::SetSoundVolume { vol } => self.audio.sfx_volume = vol.clamp(0, crate::audio::MAX_SOUND_VOL),
            Effect::PauseAudio => self.audio.pause(true),
            Effect::UnPauseAudio => self.audio.pause(false),
            // "Quit" from the frontend pause menu: no OS to return to, so a soft reset
            // (start-up logos, then the title, as at power-on).
            Effect::Quit => {
                log!("app: quit: back to start-up");
                self.game.common_init();
            }
            Effect::OpenSettings => {}
        }
    }

    /// `StartVideoPlayback`: plays `rom:/video/<name>.h264` (blocking; skippable with A, B
    /// or Start). The time it took is not caught up by the simulation.
    fn play_video(&mut self, name: &str) {
        if cfg!(feature = "small") {
            log!("app: video {name} skipped (small ROM without FMV)");
            return;
        }
        let path = alloc::format!("rom:/video/{}.h264", name.to_ascii_lowercase());
        let t0 = ticks();
        let ok = rt::fmv_play(&path);
        log!("app: video {path} {} ({} ms)", if ok { "played" } else { "missing" }, ticks_to_us(ticks().wrapping_sub(t0)) / 1000);
        self.audio.music_track = None;
        self.acc = 0;
        self.last = ticks();
        self.loaded = true;
    }

    // ------------------------------------------------------------ drawing

    fn backdrop_image(&mut self) -> Option<TexHandle> {
        let name = self.game.backdrop.backdrop.clone();
        if let Some((n, h)) = &self.backdrop {
            if Some(n) == name.as_ref() {
                return *h;
            }
            // Backdrop changed or gone: release the old image (RDRAM).
            if let Some(h) = *h {
                self.r.textures.free_image(h);
            }
            self.backdrop = None;
        }
        let name = name?;
        if self.data.pack {
            let p = crate::data::ntx_backdrop_path(&name);
            let h = lv().as_mut().and_then(|l| match crate::ntxload::load_backdrop(&self.data.store, &p, &mut self.r.textures, &mut l.gfx) {
                Ok(h) => Some(h),
                Err(e) => {
                    log!("app: backdrop {e}");
                    None
                }
            });
            self.backdrop = Some((name, h));
            return h;
        }
        let path = name.replace('\\', "/");
        // Straight from the store: not a level load (no `before_access` hook).
        let img = self
            .data
            .store
            .load(&norm_path(&path))
            .ok_or_else(|| format!("{path}: not found"))
            .and_then(|d| f2_formats::fla::maybe_decompress(d).map_err(|e| format!("{path}: {e}")))
            .and_then(|d| f2_formats::texture::parse_backdrop(&d).map_err(|e| format!("{path}: {e}")));
        let h = match img {
            Ok(t) => {
                // A full-screen 640x480 image: convert it in strips straight to RDP texels
                // (no whole RGBA8 copy) and register a placeholder image for its handle.
                let tex = crate::texconv::convert_strips(&t);
                let h = self.r.textures.add_image(1, 1, vec![0; 4]);
                if let Some(l) = lv().as_mut() {
                    l.gfx.adopt(h.0, &mut self.r.textures.images[h.0 as usize], tex);
                }
                Some(h)
            }
            Err(e) => {
                log!("app: backdrop {e}");
                None
            }
        };
        self.backdrop = Some((name, h));
        h
    }

    fn draw(&mut self) {
        #[cfg(feature = "profile-draw")]
        unsafe {
            crate::ffi::n64_prof(3)
        };
        self.draw_inner();
        #[cfg(feature = "profile-draw")]
        unsafe {
            crate::ffi::n64_prof(2)
        };
    }

    fn draw_inner(&mut self) {
        self.r.begin_frame(WIDTH, HEIGHT);
        let alpha = crate::pacing::alpha(self.acc, TICK, self.step_frames);
        let backdrop = self.backdrop_image();
        let mut ctx = DrawCtx {
            prev: &self.prev,
            alpha,
            cam: render::game_camera(&self.game),
            far_clip: 1200.0,
            show_tiles: false,
            world_only: false,
            fonts: self.game_fonts,
            authentic: true,
            backdrop,
            tile_scroll: self.tile_scroll,
            // Once per draw, as before.
            tex_anim_steps: 1,
        };
        let mut g = lv();
        let l = g.as_mut().unwrap();
        if let Some(scene) = &l.scene {
            ctx.cam = render::lerp_camera(self.prev_cam.as_ref(), ctx.cam, alpha);
            ctx.far_clip = render::far_clip(&self.game, scene);
        }
        // Procedural textures (water, fire): refilled in place from their ARGB4444 surfaces
        // (f2-draw's `lean` draw loop leaves them to the front end).
        for pt in &self.game.ptexture.list {
            let Some(surf) = pt.surface.as_deref() else { continue };
            let Some(h) = self.r.textures.entry(pt.crc).map(|e| e.image) else { continue };
            let side = (1..=256u32).find(|s| (s * s) as usize >= surf.len()).unwrap_or(0);
            if side * side != surf.len() as u32 || l.gfx.update_argb4444(h.0, side, surf) {
                continue;
            }
            if let Some(rgba) = pt.surface_rgba8() {
                self.r.textures.replace_image(h, side, side, rgba);
            }
        }
        let t_loop = ticks();
        // Meshes go straight to r3d while the loop records them: begin its frame with the
        // view the loop sets first (camera, near 10, far clip).
        #[cfg(not(feature = "render-cpu"))]
        {
            self.r.set_camera(&ctx.cam);
            self.r.view.near_clip = 10.0;
            self.r.view.far_clip = ctx.far_clip;
            l.r3d.begin_queue(&self.r);
            if let Some(s) = self.r.mesh_sink.as_mut() {
                // SAFETY: removed below, before anything else touches `l.r3d` / `l.gfx`.
                s.backend = Some(alloc::boxed::Box::new(unsafe { crate::render_r3d::R3dBackend::new(&mut l.r3d, &l.gfx) }));
            }
        }
        if l.scene.is_some() || backdrop.is_some() {
            render::draw_loop(&mut self.r, l.scene.as_mut(), &self.game, &ctx);
        }
        #[cfg(not(feature = "render-cpu"))]
        if let Some(s) = self.r.mesh_sink.as_mut() {
            s.backend = None;
        }
        self.win.draw_loop += ticks().wrapping_sub(t_loop);
        if l.scene.is_some() {
            let s = ctx.cam.source;
            self.audio.listener = crate::audio::Listener { cam_source: s.map(|x| (x * 10.0 * 4096.0) as i32), view: self.r.view.v_matrix };
        }
        let clear = match &l.scene {
            Some(s) => s.clear,
            None => [0.0, 0.0, 0.0, 1.0],
        };
        let t_up = ticks();
        l.gfx.upload(&mut self.r.textures);
        #[cfg(not(feature = "render-cpu"))]
        l.r3d.sync_textures(&self.r.textures, &l.gfx);
        self.win.upload += ticks().wrapping_sub(t_up);
        #[cfg(feature = "render-cpu")]
        l.gfx.submit(&mut self.r, clear);
        #[cfg(not(feature = "render-cpu"))]
        l.r3d.draw(&mut self.r, &mut l.gfx, clear);
        self.win.fb_wait += rt::FB_WAIT.load(core::sync::atomic::Ordering::Relaxed);
    }

    // ------------------------------------------------------------ frame

    /// Variable timestep ([`crate::pacing::VARIABLE`]) unless a demo is playing: demos replay
    /// recorded pads frame by frame and only follow the original path in 60 Hz steps.
    fn variable_step(&self) -> bool {
        crate::pacing::VARIABLE && self.game.game_state.mode as u32 != f2_game::define::mode::DEMO_MODE
    }

    /// One iteration of the main loop: catch the simulation up with real time, then draw.
    pub fn frame(&mut self) {
        let t0 = ticks();
        self.acc = self.acc.saturating_add(t0.wrapping_sub(self.last));
        self.last = t0;
        // Phase 3T (`pacing.rs`): one tick of k frames when behind (variable timestep), or up
        // to MAX_TICKS_PER_FRAME 60 Hz ticks (F2N64_TIMESTEP=fixed).
        let variable = self.variable_step();
        let plan = crate::pacing::plan(&mut self.acc, TICK, MAX_TICKS_PER_FRAME, variable);
        self.dropped_ticks += plan.dropped as u64;
        let (n, k) = (plan.ticks, plan.frames);
        let mut pad = input::psx_pad(rt::pad(0));
        if let Some(c) = &mut self.autofinish {
            *c += 1;
            if *c == 240 {
                log!("app: story-finish: completing the level");
                // As saving the last baby: the end-of-level timer, then `StartLevelComplete`.
                self.game.babyfrog.babies_saved = self.game.map.num_babies as u32;
                self.game.game.end_level_timer.init(1);
            }
            // Results screens (name entry included): Start then X, pressed for 10 frames every 40.
            if *c > 300 && *c % 40 < 10 {
                pad |= if *c % 80 < 40 { f2_game::levplay::PAD_START } else { f2_game::levplay::PAD_CROSS };
            }
            if *c > 1500 {
                self.autofinish = None;
            }
        }
        if let Some(c) = &mut self.autostart {
            use f2_game::define::mode::{FRONTEND_MODE, STARTUP_MODE};
            let m = self.game.game_state.mode as u32;
            if m == STARTUP_MODE {
                *c += 1;
                if *c > 30 && *c % 60 < 10 {
                    pad |= f2_game::levplay::PAD_START;
                }
            } else if m == FRONTEND_MODE && *c > 0 {
                self.autostart = None;
            }
        }
        for i in 0..n {
            self.tick(pad, i + 1 == n, k);
            self.acc -= (k * TICK).min(self.acc);
            let tp = ticks();
            rt::audio_poll();
            self.win.poll += ticks().wrapping_sub(tp);
        }
        let t1 = ticks();
        self.draw();
        let t2 = ticks();
        self.frames += 1;
        if !self.loaded {
            self.pace(n, t1.wrapping_sub(t0), t2.wrapping_sub(t1));
        }
        if core::mem::take(&mut self.loaded) {
            self.win = Window::default();
            self.pace_frames = 0;
            self.win_start = t2;
            return;
        }
        let w = &mut self.win;
        w.frames += 1;
        w.ticks += n;
        w.sim += t1.wrapping_sub(t0);
        w.draw += t2.wrapping_sub(t1);
        let st = lv().as_ref().map(|l| l.gfx.stats).unwrap_or_default();
        w.build += st.build_ticks;
        w.submit += st.submit_ticks;
        w.tris += st.tris;
        w.rects += st.rects;
        #[cfg(not(feature = "render-cpu"))]
        if let Some(l) = lv().as_ref() {
            w.draws += l.r3d.stats.draws;
            w.rsp_tris += l.r3d.stats.queued_tris;
            w.rsp_sent += l.r3d.stats.sent_tris;
            w.rsp_groups += l.r3d.stats.groups;
            w.tex_loads += l.r3d.stats.tex_loads;
            w.r3d_queue += l.r3d.stats.queue_ticks;
            w.r3d_flush += l.r3d.stats.flush_ticks;
            w.r3d_reused += l.r3d.stats.reused;
            w.r3d_view_reused += l.r3d.stats.view_reused;
        }
        w.max_frame = w.max_frame.max(t2.wrapping_sub(t0));
        let span = t2.wrapping_sub(self.win_start);
        if span >= 5 * TICKS_PER_SEC {
            self.log_window(span);
        }
    }

    /// Frame pacing ([`pick_fps`]): updates the running costs with this frame's `n` ticks
    /// (`sim` C0 ticks) and its drawing (`draw`; without the wait for a framebuffer, which
    /// is the pacing itself), and sets the display's frame limit.
    fn pace(&mut self, n: u32, sim: u32, draw: u32) {
        let ema = |avg: &mut u32, v: u32| {
            let v = v.min(u32::MAX / 32) << 4;
            *avg = if *avg == 0 { v } else { *avg - *avg / 8 + v / 8 };
        };
        if n > 0 {
            ema(&mut self.avg_tick, sim / n);
        }
        ema(&mut self.avg_draw, draw.saturating_sub(rt::FB_WAIT.load(core::sync::atomic::Ordering::Relaxed)));
        let (tick_us, draw_us) = (ticks_to_us(self.avg_tick >> 4), ticks_to_us(self.avg_draw >> 4));
        // Unlocked by default (owner's choice: the console can be overclocked); the even-rate
        // cap is opt-in with `F2N64_PACE=on`.
        if !PACE {
            return;
        }
        // Slower as soon as 90 % of a second is exceeded, faster only below 80 % (no
        // oscillation between two rates).
        let down = pick_fps(tick_us, draw_us, 900_000);
        let up = pick_fps(tick_us, draw_us, 800_000);
        let cur = fps_rank(self.fps_limit);
        let want = if fps_rank(down) > cur {
            down
        } else if fps_rank(up) < cur {
            up
        } else {
            self.fps_limit
        };
        if want == self.fps_limit {
            self.fps_cand = (want, 0);
            return;
        }
        self.fps_cand = if self.fps_cand.0 == want { (want, self.fps_cand.1 + 1) } else { (want, 1) };
        if self.fps_cand.1 >= 8 {
            self.fps_limit = want;
            rt::set_fps_limit(want);
            log!("app: frame pacing {} fps (tick {} us, frame {} us)", want, ticks_to_us(self.avg_tick >> 4), ticks_to_us(self.avg_draw >> 4));
        }
    }

    fn log_window(&mut self, span: u32) {
        let w = core::mem::take(&mut self.win);
        let (rsp, rdp) = rt::rcp_load();
        let ms = |t: u32| ticks_to_us(t) / 1000;
        let f = w.frames.max(1);
        let secs_x100 = ticks_to_us(span) / 10_000;
        log!(
            "perf [{}] mode {}: {} frames, {} sim ticks in {}.{:02} s => {}.{} fps, {} ticks/s (dropped total {}); per frame: sim {} ms, draw {} ms (draw loop {} ms, textures {} ms, build {} ms, CPU-side submit {} ms, framebuffer wait {} ms, r3d queue {} us, flush {} us), worst frame {} ms, pacing {} fps; RSP busy {}.{}%, RDP busy {}.{}%; Game::tick {} us/tick, audio poll {} us/tick; {} cpu tris + {} rects/frame; r3d {} draws ({} reused, {} view-only), {} tris ({} sent to the RSP, {} batches, {} texture loads)/frame; audio: {} voices, {} sfx started ({} unknown uids), music track {:?} playing {}",
            self.level_name,
            self.game.game_state.mode,
            w.frames,
            w.ticks,
            secs_x100 / 100,
            secs_x100 % 100,
            w.frames * 100 / secs_x100.max(1),
            (w.frames * 1000 / secs_x100.max(1)) % 10,
            w.ticks * 100 / secs_x100.max(1),
            self.dropped_ticks,
            ms(w.sim / f),
            ms(w.draw / f),
            ms(w.draw_loop / f),
            ms(w.upload / f),
            ms(w.build / f),
            ms(w.submit / f),
            ms(w.fb_wait / f),
            ticks_to_us(w.r3d_queue / f),
            ticks_to_us(w.r3d_flush / f),
            ms(w.max_frame),
            self.fps_limit,
            rsp / 10,
            rsp % 10,
            rdp / 10,
            rdp % 10,
            ticks_to_us(w.game_tick / w.ticks.max(1)),
            ticks_to_us(w.poll / w.ticks.max(1)),
            w.tris / f,
            w.rects / f,
            w.draws / f,
            w.r3d_reused / f,
            w.r3d_view_reused / f,
            w.rsp_tris / f,
            w.rsp_sent / f,
            w.rsp_groups / f,
            w.tex_loads / f,
            self.audio.active(),
            self.audio.plays,
            self.audio.missing,
            self.audio.music_track,
            self.audio.music_playing()
        );
        self.heap_log("running");
        // Phase 3T: game time vs wall clock (variable timestep: one tick may cover k frames).
        let gf = core::mem::take(&mut self.pace_frames);
        log!(
            "pace [{}]: timestep {} (max {} frames/tick), {} game frames in {}.{:02} s => {}% real time, {}.{:02} frames/tick",
            self.level_name,
            if crate::pacing::VARIABLE { "variable" } else { "fixed" },
            crate::pacing::MAX_FRAMES_PER_TICK,
            gf,
            secs_x100 / 100,
            secs_x100 % 100,
            gf * 10_000 / 60 / secs_x100.max(1),
            gf / w.ticks.max(1),
            gf * 100 / w.ticks.max(1) % 100
        );
        #[cfg(feature = "profile")]
        unsafe {
            crate::ffi::n64_prof(1)
        };
        self.win_start = ticks();
    }
}

/// Bytes of every string `f2_draw::book::draw_page_b` prints (all chapters).
fn book_chars(game: &Game) -> [bool; 256] {
    use f2_game::language::{c_sprintf, FmtArg};
    use f2_game::menus::strs::{STR_CHAPTER_1a, STR_CHAPTER};
    let mut used = [false; 256];
    let mut mark = |t: &str| t.chars().filter(|&c| (c as u32) < 256).for_each(|c| used[c as usize] = true);
    for pn in 0..f2_game::menus::BOOK_NUM_PAGES as i32 {
        mark(&c_sprintf(&game.gamestring(STR_CHAPTER), &[FmtArg::Int(pn + 1)]));
        mark(&game.gamestring(pn * 2 + STR_CHAPTER_1a));
        mark(&game.gamestring(1 + pn * 2 + STR_CHAPTER_1a));
    }
    used
}

/// `f` with only the glyphs in `keep`, packed side by side (the CPU text drawing samples
/// each glyph inside its own columns, so packing does not change the result).
fn font_subset(f: &Font, keep: &[bool; 256]) -> Font {
    let mut out = Font { width: 0, height: f.height, glyphs: [None; 256], rgba: Vec::new() };
    let cols: Vec<(usize, f2_engine::font::Glyph)> = (0..256).filter_map(|c| f.glyphs[c].filter(|_| keep[c]).map(|g| (c, g))).collect();
    let w: u32 = cols.iter().map(|(_, g)| if g.left < f.width { g.width.min(f.width - g.left) } else { 0 }).sum::<u32>().max(1);
    out.width = w;
    out.rgba = vec![0; (w * f.height * 4) as usize];
    let mut x = 0;
    for (c, g) in cols {
        if g.left >= f.width {
            // No image (the space): keep the advance, point past the strip like the original.
            out.glyphs[c] = Some(f2_engine::font::Glyph { left: u32::MAX / 2, width: g.width });
            continue;
        }
        let gw = g.width.min(f.width - g.left);
        for y in 0..f.height {
            let src = ((y * f.width + g.left) * 4) as usize;
            let dst = ((y * w + x) * 4) as usize;
            out.rgba[dst..dst + (gw * 4) as usize].copy_from_slice(&f.rgba[src..src + (gw * 4) as usize]);
        }
        out.glyphs[c] = Some(f2_engine::font::Glyph { left: x, width: g.width });
        x += gw;
    }
    out
}

/// `LoadSfx` world bank (pcaudio.c:232; `f2_audio::tables::world_bank`).
fn world_bank(world: i16) -> Option<&'static str> {
    Some(match world {
        0 => "garden.wus",
        1 => "ancients.wus",
        2 => "space.wus",
        3 => "city.wus",
        4 => "sub.wus",
        5 => "lab.wus",
        6 => "halloween.wus",
        7 => "superretro.wus",
        8 => "frontend.wus",
        9 => "test.wus",
        _ => return None,
    })
}
