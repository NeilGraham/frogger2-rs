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

/// `Boot::BookSelectTest` in the frontend: right and up onto the story book, then select the
/// chapter it opens on right away (frames, PSX pad bits). Selecting a chapter within a fraction
/// of a second of arriving ran out of memory on hardware (research/n64-performance.md).
const BOOK_SELECT_SCRIPT: &[(u32, u16)] = {
    use f2_game::levplay::{PAD_CROSS, PAD_RIGHT, PAD_UP};
    &[(90, 0), (6, PAD_RIGHT), (30, 0), (6, PAD_UP), (45, 0), (6, PAD_CROSS), (20, 0), (6, PAD_CROSS)]
};


/// `Game::phase_hook`: C0 ticks per simulation phase (`f2_game::simbench::phase`) in the
/// current perf window, and the time of the last mark.
static PHASES: [core::sync::atomic::AtomicU32; f2_game::simbench::phase::COUNT] = [const { core::sync::atomic::AtomicU32::new(0) }; f2_game::simbench::phase::COUNT];
static PHASE_LAST: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

fn phase_hook(id: u32) {
    use core::sync::atomic::Ordering::Relaxed;
    let t = ticks();
    PHASES[id as usize].fetch_add(t.wrapping_sub(PHASE_LAST.swap(t, Relaxed)), Relaxed);
}

/// `f2_draw::render::set_phase_hook`: C0 ticks per draw-loop phase in the current perf window.
static DRAW_PHASES: [core::sync::atomic::AtomicU32; f2_draw::render::draw_phase::COUNT] = [const { core::sync::atomic::AtomicU32::new(0) }; f2_draw::render::draw_phase::COUNT];
static DRAW_LAST: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

fn draw_phase_hook(id: u32) {
    use core::sync::atomic::Ordering::Relaxed;
    let t = ticks();
    DRAW_PHASES[id as usize].fetch_add(t.wrapping_sub(DRAW_LAST.swap(t, Relaxed)), Relaxed);
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
/// frontend with Start pressed on the title screen, into the hub) or `settings-test`
/// (testing: `frontend-start`, then the settings page open with the FPS counter on and a
/// 30 fps limit, not saved) or `book-test` (testing: `frontend-start`, then Right, Up: the
/// story book open on its chapter page) or `level-book-test` (testing: story Garden 1 (its FMV first),
/// back to the frontend after 300 frames as the pause menu's Quit does, then Right, Up).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boot {
    Frontend { trained: bool },
    SaveTest,
    Level(u32, u32),
    /// Benchmark: `level W L` with the frog hopping in a loop (up, right, down, left).
    LevelHop(u32, u32),
    Story(u32),
    StoryFinish(u32),
    FrontendStart,
    SettingsTest,
    BookTest,
    BookSelectTest(u32),
    /// Testing (memory): every story level, then the retro levels, leaving each after 240
    /// frames by Quit, a game over or straight into the next level in turn (FMVs skipped);
    /// `mem` log lines at every load (research/n64-memory.md "Level heap").
    MemTour(u32),
    LevelBookTest,
    GameOverBookTest,
}

impl Boot {
    pub fn parse(s: Option<&str>) -> Boot {
        let Some(s) = s else { return Boot::Frontend { trained: false } };
        let mut it = s.split_whitespace();
        match (it.next(), it.next().and_then(|w| w.parse().ok()), it.next().and_then(|l| l.parse().ok())) {
            (Some("level"), Some(w), Some(l)) => Boot::Level(w, l),
            (Some("level-hop"), Some(w), Some(l)) => Boot::LevelHop(w, l),
            (Some("story"), Some(n), _) => Boot::Story(n),
            (Some("story-finish"), Some(n), _) => Boot::StoryFinish(n),
            (Some("frontend-start"), _, _) => Boot::FrontendStart,
            (Some("settings-test"), _, _) => Boot::SettingsTest,
            (Some("book-test"), _, _) => Boot::BookTest,
            (Some("book-select-test"), n, _) => Boot::BookSelectTest(n.unwrap_or(1)),
            (Some("level-book-test"), _, _) => Boot::LevelBookTest,
            (Some("mem-tour"), n, _) => Boot::MemTour(n.unwrap_or(0)),
            (Some("gameover-book-test"), _, _) => Boot::GameOverBookTest,
            (Some("frontend-trained"), _, _) => Boot::Frontend { trained: true },
            (Some("save-test"), _, _) => Boot::SaveTest,
            _ => Boot::Frontend { trained: false },
        }
    }
}

/// Frame-time accounting over a logging window.
#[derive(Default, Clone, Copy)]
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
    /// Procedural texture refill, `render::snapshot`, host overlays (C0 ticks).
    refill: u32,
    snap: u32,
    host2d: u32,
    /// Mixer polls (all of them, the C main loop's included) and the longest main-loop
    /// period (frame end to frame end, so the C polls between frames count).
    audio: u32,
    max_period: u32,
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
    /// The story book (frontend only, allocated at its load while the heap is clean): a
    /// 64-row strip canvas the page is composed through (64 KiB) and each page texture's
    /// RGBA16 texels (128 KiB, the textures point into them). A page grabbed later (after a
    /// level and a game over) then needs no large allocation (a fragmented heap had none:
    /// a crash in the texture's memcpy).
    pub page_strip: Option<f2_draw::book::Canvas>,
    pub page_bufs: [Option<alloc::sync::Arc<Vec<u8>>>; 2],
}

pub static LEVEL: spin::Mutex<Option<Level>> = spin::Mutex::new(None);

/// Set once a level's renderer side is loaded; the first data-source access after it (the
/// next `InitLevel`) frees that side ([`unload_level_render`]).
static ARMED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Set when no level load is in progress; the first data-source access after it (InitLevel's)
/// switches allocations to the level heap ([`rt::MEM_LEVEL`]) until [`App::load_scene`] ends.
/// False until the session is up (its reads at boot are not a level load).
static PLAYING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Frees the loaded level's object banks and textures (keeps the fonts). Called by the data
/// source at the start of a level load inside the simulation.
pub fn unload_level_render() {
    if PLAYING.swap(false, core::sync::atomic::Ordering::Relaxed) {
        rt::mem_phase(rt::MEM_LEVEL);
        log!("app: level load starts (level heap)");
    }
    if !ARMED.swap(false, core::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let t0 = rt::ticks();
    if let Some(l) = LEVEL.lock().as_mut() {
        l.scene = None;
        l.page_strip = None;
        l.page_bufs = [None, None];
        let keep = l.keep_images;
        l.gfx.free_from(keep);
        #[cfg(not(feature = "render-cpu"))]
        l.r3d.unload();
        log!("app: level render assets freed for the next load ({} ms)", rt::ticks_to_us(rt::ticks().wrapping_sub(t0)) / 1000);
        rt::audio_poll();
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
    /// Per procedural texture: (crc, `timer`, registry image, its N64 texture generation) of its last refill (`timer` changes
    /// exactly when the game blits a new surface).
    proc_seen: Vec<(u32, u32, u32, u32)>,
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
    /// `autostart` presses Start every 20 frames instead of 60 (`Boot::BookSelectTest`).
    fast_start: bool,
    /// `Boot::LevelHop`: game frames since boot (a hop every 32, after 3 s).
    hop_bench: Option<u32>,
    /// The settings page (`settings.rs`) and its values (SRAM).
    settings: crate::settings::Settings,
    menu: crate::settings::Menu,
    /// After the page closes: game input stays off until every button is released.
    menu_release: bool,
    /// `Boot::SettingsTest`: open the page once the hub is up.
    settings_test: bool,
    /// Test boots: pad presses after the hub is up, as (frames, pad) steps; (frame, steps).
    pad_script: Option<(u32, &'static [(u32, u16)])>,
    /// `Boot::LevelBookTest`: frames in the level before quitting to the frontend.
    level_quit: Option<u32>,
    /// [`App::free_last_level`] ran for the load in progress (before its video).
    last_level_freed: bool,
    /// `Boot::MemTour`: (step, frames in it, waiting in the frontend).
    tour: Option<(u32, u32, bool)>,
    /// `Boot::GameOverBookTest`: leave the level by a game over instead of Quit.
    game_over_test: bool,
    /// Start of the level load in progress (load step timing).
    load_t0: u32,
    /// FPS counter: frames and start of the current second, the last second's count.
    fps_count: (u32, u32, u32),
    /// Frame pacing: running averages (C0 ticks, x16 fixed point) of a simulation tick
    /// incl. audio polling and of a frame's drawing; the limit in force and a candidate.
    avg_tick: u32,
    avg_draw: u32,
    fps_limit: u32,
    fps_cand: (u32, u32),
    /// Spike log: end of the last frame, `rt::audio_ticks_total` then, running average of
    /// the main-loop period (C0 ticks x16), spikes logged in this window.
    last_end: u32,
    audio_mark: u32,
    avg_period: u32,
    spikes: u32,
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
            page_strip: None,
            page_bufs: [None, None],
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
            proc_seen: Vec::new(),
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
            fast_start: false,
            hop_bench: None,
            settings: Default::default(),
            menu: Default::default(),
            menu_release: false,
            settings_test: false,
            pad_script: None,
            level_quit: None,
            tour: None,
            last_level_freed: false,
            load_t0: 0,
            game_over_test: false,
            fps_count: (0, 0, 0),
            avg_tick: 0,
            avg_draw: 0,
            fps_limit: 0,
            fps_cand: (0, 0),
            last_end: 0,
            audio_mark: 0,
            avg_period: 0,
            spikes: 0,
        };
        app.rebuild_renderer();
        f2_draw::overlay::set_button_prompts(vec![('X', "A".into()), ('C', "B".into()), ('S', "Z".into()), ('T', "C-Up".into())]);
        let mut cfg = SessionConfig::new(app.data.clone());
        cfg.save_data = crate::save::load();
        log!("app: save game {}", if cfg.save_data.is_some() { "loaded from SRAM" } else { "not found (new player)" });
        let t0 = ticks();
        app.game = Game::new_session(cfg);
        app.game.set_game_options(f2_game::enhance::GameOptions { console: true, settings_menu: true, ..f2_game::enhance::GameOptions::AUTHENTIC });
        app.settings = crate::settings::Settings::load();
        // F2N64_FRAME_LIMIT=30|20|0 (build time, measurement): the frame limit whatever SRAM says.
        if let Some(v) = option_env!("F2N64_FRAME_LIMIT").and_then(|v| v.trim().parse::<u8>().ok()) {
            app.settings.frame_limit = v;
        }
        log!("app: settings {:?}", app.settings);
        app.apply_settings();
        app.install_game_hooks();
        app.book_font = load_font(&app.data, "textures/font/monofont.bmp").map(|f| font_subset(&f, &book_chars(&app.game)));
        log!("app: session up in {} ms", ticks_to_us(ticks().wrapping_sub(t0)) / 1000);
        // Lazily built tables would otherwise land in the level heap at the first animation.
        f2_engine::math::warm_tables();
        rt::mem_report("session up", false);
        // Boot and session setup were MEM_MAIN (csrc/main.c big_init).
        rt::mem_phase(rt::MEM_PLAY);
        PLAYING.store(true, core::sync::atomic::Ordering::Relaxed);
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
            Boot::LevelBookTest => {
                app.game.start_story(1);
                app.level_quit = Some(0);
            }
            Boot::GameOverBookTest => {
                // Story Subterranean 1 (The Gold Mine), game over after 300 frames.
                app.game.start_story(3);
                app.level_quit = Some(0);
                app.game_over_test = true;
            }
            Boot::BookTest => {
                app.autostart = Some(0);
                app.pad_script = Some((0, &[(90, 0), (10, f2_game::levplay::PAD_RIGHT), (90, 0), (10, f2_game::levplay::PAD_UP)]));
            }
            Boot::BookSelectTest(chapters) => {
                // Every FMV skipped at once and the title pressed through quickly, then
                // BOOK_SELECT_SCRIPT. Chapters
                // 1..=N open (the book opens on chapter N; 1: the training level, no FMV;
                // 2: Ancients 1 after its FMV).
                if chapters >= 2 {
                    let i = f2_game::options::PAGE_TO_STORY_LEVEL[(chapters as usize - 1).min(f2_game::menus::BOOK_NUM_PAGES - 1)] as usize;
                    let s = f2_game::story::STORY_SEQUENCE[i];
                    let w = &mut app.game.layout.world_visual_data[s.world_num as usize];
                    w.world_open = 1;
                    w.level_visual_data[s.level_num as usize].level_open = 1;
                }
                rt::set_fmv_autoskip(true);
                app.autostart = Some(0);
                app.fast_start = true;
                app.pad_script = Some((0, BOOK_SELECT_SCRIPT));
            }
            Boot::MemTour(first) => {
                rt::set_fmv_autoskip(true);
                app.tour = Some((first, 0, false));
                app.tour_start(first);
            }
            Boot::SettingsTest => {
                app.autostart = Some(0);
                app.settings_test = true;
            }
            Boot::Level(w, l) => {
                log!("app: booting straight into level {w}-{l}");
                app.game.start_arcade(w, l);
            }
            Boot::LevelHop(w, l) => {
                log!("app: booting straight into level {w}-{l}, hopping");
                app.game.start_arcade(w, l);
                app.hop_bench = Some(0);
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
        self.game.phase_hook = Some(phase_hook);
        render::set_phase_hook(Some(draw_phase_hook));
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
        // The first time it loads the game fonts, which stay for the whole run: main heap.
        let phase = self.fonts_cache.is_none().then(|| rt::mem_phase(rt::MEM_MAIN));
        self.rebuild_renderer_inner();
        if let Some(p) = phase {
            rt::mem_phase_resume(p);
        }
    }

    fn rebuild_renderer_inner(&mut self) {
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
        log!(
            "heap step [{}] after {what}: rust live {} KiB, libdragon used {} KiB; at {} ms, longest audio gap {} ms",
            self.level_name,
            live / 1024,
            h.used / 1024,
            ticks_to_us(ticks().wrapping_sub(self.load_t0)) / 1000,
            rt::audio_gap_take_ms()
        );
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
        rt::mem_report_quick(&self.level_name);
    }

    // ------------------------------------------------------------ levels

    /// `Boot::MemTour` step `i`: story levels, then the retro levels (arcade).
    fn tour_start(&mut self, i: u32) -> bool {
        let n = f2_game::story::NUM_STORY_LEVELS as u32;
        if i < n {
            log!("app: mem-tour step {i}: story level {i}");
            self.game.start_story(i);
        } else if i < n + 10 {
            log!("app: mem-tour step {i}: arcade 7-{}", i - n);
            self.game.start_arcade(7, i - n);
        } else {
            log!("app: mem-tour done");
            rt::mem_report("mem-tour done", true);
            return false;
        }
        true
    }

    fn tour_step(&mut self) {
        use f2_game::define::mode::FRONTEND_MODE;
        let Some((i, f, fe)) = self.tour else { return };
        let mut t = (i, f + 1, fe);
        let in_frontend = self.game.game_state.mode == FRONTEND_MODE;
        if fe {
            // Frames since the frontend is up (a game over shows its screen first).
            t.1 = if in_frontend { t.1 } else { 0 };
            if t.1 > 180 {
                t = (i + 1, 0, false);
                if !self.tour_start(i + 1) {
                    self.tour = None;
                    return;
                }
            }
        } else if t.1 == 240 {
            match i % 3 {
                0 => {
                    log!("app: mem-tour: Quit to the frontend");
                    let g = &mut self.game;
                    g.frogger.player[0].character = f2_game::frogger::FROG_FROGGER;
                    g.frogger.num_frogs = 1;
                    g.game_state.mode = FRONTEND_MODE;
                    g.init_level(f2_game::layout::worldid::FRONTEND, f2_game::layout::LEVELID_FRONTEND1);
                    t = (i, 0, true);
                }
                1 => {
                    log!("app: mem-tour: game over");
                    self.game.start_game_over();
                    t = (i, 0, true);
                }
                _ => {
                    log!("app: mem-tour: straight into the next level");
                    t = (i + 1, 0, false);
                    if !self.tour_start(i + 1) {
                        self.tour = None;
                        return;
                    }
                }
            }
        }
        self.tour = Some(t);
    }

    fn process_level_loads(&mut self) {
        if let Some(l) = self.game.take_level_loads().pop() {
            self.load_scene(l.world, l.level, &l.assets);
        }
    }

    /// Frees the rest of the previous level's renderer side (the scene, r3d storage, the image
    /// registry, the sound tables) and starts the renderer's side of the load (from the level
    /// heap's top; InitLevel filled it from the bottom). Before a story video too, which
    /// plays between InitLevel and [`load_scene`](Self::load_scene) and needs large blocks.
    fn free_last_level(&mut self) {
        PLAYING.store(false, core::sync::atomic::Ordering::Relaxed);
        rt::mem_phase(rt::MEM_LEVEL_TOP);
        ARMED.store(false, core::sync::atomic::Ordering::Relaxed);
        if let Some(l) = lv().as_mut() {
            l.scene = None;
            #[cfg(not(feature = "render-cpu"))]
            l.r3d.unload();
        }
        self.rebuild_renderer();
        self.audio.free_samples();
        self.last_level_freed = true;
    }

    fn load_scene(&mut self, world: u32, level: u32, assets: &f2_game::layout::LevelAssets) {
        let t0 = ticks();
        if self.level_name != "boot" {
            log!("heap [{}] level peak: rust {} KiB, libdragon used {} KiB", self.level_name, self.level_peak.0 / 1024, self.level_peak.1 / 1024);
            rt::mem_report(&format!("{} played + next InitLevel", self.level_name), false);
        }
        self.level_name = format!("{world}-{level}");
        self.level_peak = (0, 0);
        self.load_t0 = t0;
        // The audio gap so far covers the simulation's side of the load (InitLevel reading
        // the level's files) and any FMV before it.
        self.heap_step("the game's level setup");
        if !self.last_level_freed {
            self.free_last_level();
        }
        self.last_level_freed = false;
        rt::reset_peak();
        self.heap_step("freeing the last level");
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
        // The story book's page memory, while the heap is still in one piece (see `Level`).
        if world == f2_game::layout::worldid::FRONTEND {
            if let Some(l) = lv().as_mut() {
                let (w, h) = (f2_draw::book::PAGE_SIZE, f2_draw::book::PAGE_SIZE);
                l.page_strip.get_or_insert_with(|| f2_draw::book::Canvas::strip(w, h, 64));
                for b in &mut l.page_bufs {
                    b.get_or_insert_with(|| alloc::sync::Arc::new(alloc::vec![0u8; (w * h * 2) as usize]));
                }
            }
        }
        self.heap_step("textures");
        // The F2NM mesh files next (up to ~300 KiB each, kept by r3d): large blocks while the
        // level heap is still in one piece, before the object banks' many small ones.
        #[cfg(not(feature = "render-cpu"))]
        let f2nm: Vec<Option<Vec<u8>>> = assets
            .object_banks
            .iter()
            .map(|f| {
                let b = self.data.store.load(&(norm_path(f) + crate::data::F2NM_SUFFIX));
                rt::audio_poll();
                b
            })
            .collect();
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
        let banks = self.load_meshes(&assets.object_banks, f2nm, banks);
        self.heap_step("r3d meshes");
        let mut scene = LevelScene::new(world, level, banks, assets.texture_banks.clone(), &self.game);
        // The per-actor draw state (~350 bytes an actor: ~100 KiB in Ancients 3) now, while
        // the level heap is in one piece, instead of at the first frame; room for a quarter
        // more actors spawned in play.
        let n = self.game.actor2.actors.slots.len();
        scene.actors.reserve_exact(n + n / 4 + 8);
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
        // Everything the previous level allocated while loading must be gone by now (the
        // level heap is then free for this level only); the check lists what is not.
        rt::mem_report(&self.level_name, true);
        rt::mem_phase(rt::MEM_PLAY);
        rt::mem_reset_peaks();
        PLAYING.store(true, core::sync::atomic::Ordering::Relaxed);
        ARMED.store(true, core::sync::atomic::Ordering::Relaxed);
        // The load took real time: do not try to catch it up.
        self.acc = 0;
        self.last = ticks();
        self.loaded = true;
    }

    /// The banks' F2NM render meshes into r3d; returns the banks without their render
    /// geometry (the draw loop only needs hierarchies, keys and flags from them).
    #[cfg(not(feature = "render-cpu"))]
    fn load_meshes(&mut self, files: &[String], f2nm: Vec<Option<Vec<u8>>>, mut banks: Vec<f2_formats::objbank::ObjectBank>) -> Vec<f2_formats::objbank::ObjectBank> {
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
        for (f, bytes) in files.iter().zip(f2nm) {
            if bi >= banks.len() {
                break;
            }
            let p = norm_path(f) + crate::data::F2NM_SUFFIX;
            let Some(bytes) = bytes else {
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
                    log!("app: model {:08x} ({}) on the CPU path (book page texture)", m.id, m.name());
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
            let ts = ticks();
            render::snapshot_into(&self.game, &mut self.prev);
            self.win.snap += ticks().wrapping_sub(ts);
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
        PHASE_LAST.store(ticks(), core::sync::atomic::Ordering::Relaxed);
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
        let Some(h) = self.r.textures.entry(f2_formats::crc(target)).map(|e| e.image) else {
            log!("app: book page texture {target} not loaded");
            return;
        };
        let mut g = lv();
        let Some(l) = g.as_mut() else { return };
        let t0 = ticks();
        // The old page texture holds a reference to its buffer: freed first.
        l.gfx.free_image(h.0);
        let slot = if target == "page256a.bmp" { 0 } else { 1 };
        let (w, ph) = (f2_draw::book::PAGE_SIZE, f2_draw::book::PAGE_SIZE);
        let strip = l.page_strip.get_or_insert_with(|| f2_draw::book::Canvas::strip(w, ph, 64));
        let buf = l.page_bufs[slot].get_or_insert_with(|| alloc::sync::Arc::new(alloc::vec![0u8; (w * ph * 2) as usize]));
        let Some(texels) = alloc::sync::Arc::get_mut(buf) else {
            log!("app: book page {target}: buffer still in use");
            return;
        };
        // Strip by strip into the page's RGBA16 texels (the page is opaque).
        for y0 in (0..ph).step_by(strip.rows as usize) {
            strip.window(y0);
            f2_draw::book::draw_page_b_into(&self.r, &self.game, self.book_font.as_ref(), strip);
            let out = &mut texels[(y0 * w * 2) as usize..((y0 + strip.rows) * w * 2) as usize];
            for (o, p) in out.as_chunks_mut::<2>().0.iter_mut().zip(strip.rgba.as_chunks::<4>().0) {
                *o = (((p[0] as u16 >> 3) << 11) | ((p[1] as u16 >> 3) << 6) | ((p[2] as u16 >> 3) << 1) | 1).to_be_bytes();
            }
            rt::audio_poll();
        }
        self.game.options.grab_to_texture = if self.game.options.grab_to_texture == 1 { 0 } else { 2 };
        let desc = crate::ffi::NtxDesc {
            fmt: 0, // RGBA16
            width: w,
            height: ph,
            stride: w * 2,
            texels: buf.as_ptr(),
            tlut: core::ptr::null(),
            tlut_count: 0,
            flags: crate::ffi::NTX_TILED, // sharp: the full page in TMEM tiles
            tile_w: 0,
            tile_h: 0,
        };
        let ok = l.gfx.adopt_ntx(h.0, buf.clone(), &desc);
        if let Some(img) = self.r.textures.images.get_mut(h.0 as usize) {
            img.width = w;
            img.height = ph;
            img.rgba = Vec::new();
        }
        log!("app: book page {target}: {} ms{}", ticks_to_us(ticks().wrapping_sub(t0)) / 1000, if ok { "" } else { " (no texture slot)" });
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
            Effect::OpenSettings => self.menu.show(),
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
        // A story video between InitLevel and the renderer's load: the previous level's last
        // tables go first (the decoder needs a few hundred KiB in large blocks).
        if !PLAYING.load(core::sync::atomic::Ordering::Relaxed) && !self.last_level_freed && self.level_name != "boot" {
            self.free_last_level();
        }
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
        let t_refill = ticks();
        self.proc_seen.resize(self.game.ptexture.list.len(), (0, 0, u32::MAX, 0));
        for (pt, seen) in self.game.ptexture.list.iter().zip(&mut self.proc_seen) {
            let Some(surf) = pt.surface.as_deref() else { continue };
            let Some(h) = self.r.textures.entry(pt.crc).map(|e| e.image) else { continue };
            // Unchanged surface (no blit since the last refill) into the same texture.
            let key = (pt.crc, pt.timer, h.0, l.gfx.texture_of(h.0).1);
            if *seen == key {
                continue;
            }
            *seen = key;
            let side = (1..=256u32).find(|s| (s * s) as usize >= surf.len()).unwrap_or(0);
            if side * side != surf.len() as u32 || l.gfx.update_argb4444(h.0, side, surf) {
                continue;
            }
            if let Some(rgba) = pt.surface_rgba8() {
                self.r.textures.replace_image(h, side, side, rgba);
            }
        }
        let t_loop = ticks();
        self.win.refill += t_loop.wrapping_sub(t_refill);
        DRAW_LAST.store(t_loop, core::sync::atomic::Ordering::Relaxed);
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
                // F2N64_NO_BACKEND=1 (measurement): no r3d backend, the meshes are recorded
                // into the sink and dropped: the draw loop's own CPU cost without r3d.
                if option_env!("F2N64_NO_BACKEND").is_none() {
                    s.backend = Some(alloc::boxed::Box::new(unsafe { crate::render_r3d::R3dBackend::new(&mut l.r3d, &l.gfx) }));
                }
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
        let t_h = ticks();
        self.draw_host_overlays();
        self.win.host2d += ticks().wrapping_sub(t_h);
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

    /// Applies the settings that act on the host: the display frame cap.
    fn apply_settings(&mut self) {
        if !PACE {
            self.fps_limit = self.settings.frame_limit as u32;
            rt::set_fps_limit(self.fps_limit);
        }
    }

    /// The settings page and the FPS counter, over everything the game drew (2D overlays).
    fn draw_host_overlays(&mut self) {
        let Some(fonts) = self.game_fonts else { return };
        let now = ticks();
        let fc = &mut self.fps_count;
        fc.0 += 1;
        if ticks_to_us(now.wrapping_sub(fc.1)) >= 1_000_000 {
            *fc = (0, now, fc.0);
        }
        use f2_draw::overlay::print_text;
        if self.settings.fps_counter {
            let s = alloc::format!("{} fps", self.fps_count.2);
            print_text(&mut self.r, fonts.small, 3350, 3780, &s, 4096, false, [255, 255, 0, 255], 0.0);
        }
        if self.menu.open {
            let (w, h) = self.r.output_size();
            let (w, h) = (w as f32, h as f32);
            self.r.draw_quad(f2_engine::renderer::Quad2D::rect(
                w * 0.08,
                h * 0.22,
                w * 0.84,
                h * 0.56,
                [0.0; 4],
                None,
                [0, 0, 40, 200],
                f2_engine::renderer::Blend::Semi,
            ));
            let title = self.game.settings_label();
            print_text(&mut self.r, fonts.normal, 2048, 1050, &title, 4096, true, [255, 255, 255, 255], 0.0);
            for i in 0..crate::settings::ROWS {
                let row = crate::settings::Menu::row(&self.settings, i);
                let col = if i == self.menu.sel { [255, 255, 0, 255] } else { [160, 220, 160, 255] };
                print_text(&mut self.r, fonts.normal, 2048, 1550 + i as i16 * 360, &row, 3600, true, col, 0.0);
            }
            print_text(&mut self.r, fonts.small, 2048, 2950, "Up/Down: choose   Left/Right/A: change   B: back", 4096, true, [200, 200, 200, 255], 0.0);
        }
    }

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
        // Test boots' scripted presses count as the real pad (the settings page reads it too).
        let mut script = 0u16;
        if let (None, Some((f, steps))) = (self.autostart, &mut self.pad_script) {
            // Game frames (60 Hz), whatever the frame rate.
            *f += (n * k).max(1);
            let mut t = *f;
            let mut done = true;
            for &(n, p) in steps.iter() {
                if t <= n {
                    script |= p;
                    done = false;
                    break;
                }
                t -= n;
            }
            if done {
                self.pad_script = None;
            }
        }
        let std_pad = input::psx_pad(rt::pad(0)) | script;
        let mut pad = if self.settings.swap_ab { input::swap_ab(std_pad) } else { std_pad };
        if self.menu.open {
            let before = self.settings;
            let closed = self.menu.input(&mut self.settings, std_pad);
            if self.settings != before {
                self.apply_settings();
            }
            if closed {
                let ok = self.settings.store();
                log!("app: settings {:?} {}", self.settings, if ok { "saved" } else { "not saved (no SRAM)" });
                self.menu_release = true;
            }
            pad = 0;
        } else if self.menu_release {
            // The button that closed the page must not also act in the pause menu.
            if std_pad == 0 {
                self.menu_release = false;
            }
            pad = 0;
        }
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
        if let Some(c) = &mut self.hop_bench {
            // Counted in game frames (whatever the frame rate), held for 8 of every 32.
            *c += n * k;
            // The scripted pad only: a stray host key (ares' keyboard pad) must not pause
            // the benchmark.
            pad = 0;
            if *c > 180 && *c % 32 < 8 {
                use f2_game::levplay::{PAD_DOWN, PAD_LEFT, PAD_RIGHT, PAD_UP};
                pad |= [PAD_UP, PAD_RIGHT, PAD_DOWN, PAD_LEFT][(*c / 32 % 4) as usize];
            }
        }
        self.tour_step();
        if let Some(c) = &mut self.level_quit {
            *c += 1;
            if *c == 300 {
                // The pause menu's Quit from a level (menus.rs, quitting_level 2).
                log!("app: level-book-test: quitting to the frontend");
                self.level_quit = None;
                if self.game_over_test {
                    log!("app: gameover-book-test: game over");
                    self.game.start_game_over();
                    // The book after the game over screen and the frontend load.
                    self.pad_script = Some((0, &[(400, 0), (10, f2_game::levplay::PAD_RIGHT), (90, 0), (10, f2_game::levplay::PAD_UP)]));
                } else {
                    let g = &mut self.game;
                    g.frogger.player[0].character = f2_game::frogger::FROG_FROGGER;
                    g.frogger.num_frogs = 1;
                    g.game_state.mode = f2_game::define::mode::FRONTEND_MODE;
                    g.init_level(f2_game::layout::worldid::FRONTEND, f2_game::layout::LEVELID_FRONTEND1);
                    self.pad_script = Some((0, &[(90, 0), (10, f2_game::levplay::PAD_RIGHT), (90, 0), (10, f2_game::levplay::PAD_UP)]));
                }
            }
        }
        let every = if self.fast_start { 20 } else { 60 };
        if let Some(c) = &mut self.autostart {
            use f2_game::define::mode::{FRONTEND_MODE, STARTUP_MODE};
            let m = self.game.game_state.mode as u32;
            if m == STARTUP_MODE {
                *c += 1;
                if *c > 30 && *c % every < 6 {
                    pad |= f2_game::levplay::PAD_START;
                }
            } else if m == FRONTEND_MODE && *c > 0 {
                self.autostart = None;
                if self.settings_test {
                    self.settings.fps_counter = true;
                    self.settings.frame_limit = 30;
                    self.apply_settings();
                    self.menu.show();
                }
            }
        }
        let before = self.win;
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
        let audio_now = rt::audio_ticks_total();
        let audio = audio_now.wrapping_sub(self.audio_mark);
        self.audio_mark = audio_now;
        let period = t2.wrapping_sub(self.last_end);
        let first = self.last_end == 0;
        self.last_end = t2;
        if core::mem::take(&mut self.loaded) {
            self.win = Window::default();
            self.pace_frames = 0;
            self.win_start = t2;
            self.avg_period = 0;
            return;
        }
        if !first {
            self.spike_check(&before, n, k, period, t1.wrapping_sub(t0), t2.wrapping_sub(t1), audio);
        }
        let w = &mut self.win;
        w.audio += audio;
        w.max_period = w.max_period.max(period);
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

    /// Frame-time spikes: a main-loop period over 50 ms and 1.5x the running average (both)
    /// is logged with its breakdown (`spike` lines, at most 8 per perf window), to find what
    /// lands in one frame (texture uploads, audio refills, level streaming, allocations).
    /// `before`: the window counters before this frame's ticks.
    fn spike_check(&mut self, before: &Window, n: u32, k: u32, period: u32, sim: u32, draw: u32, audio: u32) {
        let avg = self.avg_period >> 4;
        self.avg_period = if self.avg_period == 0 { period << 4 } else { self.avg_period - (self.avg_period >> 4) + period };
        // (No average yet: the first frame after a level load, often the slowest.)
        if period < TICKS_PER_SEC / 20 || (avg != 0 && period < avg + avg / 2) || self.spikes >= 8 {
            return;
        }
        self.spikes += 1;
        let st = lv().as_ref().map(|l| l.gfx.stats).unwrap_or_default();
        let w = &self.win;
        let d = |a: u32, b: u32| ticks_to_us(a.wrapping_sub(b)) / 100;
        let f = |x: u32| alloc::format!("{}.{}", x / 10, x % 10);
        #[cfg(not(feature = "render-cpu"))]
        let (q, fl) = lv().as_ref().map_or((0, 0), |l| (l.r3d.stats.queue_ticks, l.r3d.stats.flush_ticks));
        #[cfg(feature = "render-cpu")]
        let (q, fl) = (0, 0);
        log!(
            "spike [{}] mode {}: {} ms (average {} ms): sim {} ({} tick(s) x {} frames, Game::tick {}, polls {}, snapshot {}), draw {} (loop {}, refill {}, textures {}, build {}, submit {}, r3d queue {} flush {}, fb wait {}), audio {} ms in polls, rest {}",
            self.level_name,
            self.game.game_state.mode,
            f(ticks_to_us(period) / 100),
            f(ticks_to_us(avg) / 100),
            f(ticks_to_us(sim) / 100),
            n,
            k,
            f(d(w.game_tick, before.game_tick)),
            f(d(w.poll, before.poll)),
            f(d(w.snap, before.snap)),
            f(ticks_to_us(draw) / 100),
            f(d(w.draw_loop, before.draw_loop)),
            f(d(w.refill, before.refill)),
            f(d(w.upload, before.upload)),
            f(ticks_to_us(st.build_ticks) / 100),
            f(ticks_to_us(st.submit_ticks) / 100),
            f(ticks_to_us(q) / 100),
            f(ticks_to_us(fl) / 100),
            f(ticks_to_us(rt::FB_WAIT.load(core::sync::atomic::Ordering::Relaxed)) / 100),
            f(ticks_to_us(audio) / 100),
            f(ticks_to_us(period.saturating_sub(sim + draw)) / 100),
        );
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
        self.spikes = 0;
        {
            use core::sync::atomic::Ordering::Relaxed;
            use f2_game::simbench::phase::{COUNT, NAMES};
            let mut s = String::new();
            let mut sum = 0;
            for i in 0..COUNT {
                let us = ticks_to_us(PHASES[i].swap(0, Relaxed)) / w.ticks.max(1);
                sum += us;
                let _ = core::fmt::Write::write_fmt(&mut s, format_args!(" {} {}", NAMES[i], us));
            }
            // F2N64_TUNE_AB=mask (build time): `f2_game::simbench::TUNE` alternates between 0
            // and `mask` every window, an A/B of experiment code in one ROM (same layout).
            let tune = f2_game::simbench::TUNE.load(Relaxed);
            if let Some(mask) = option_env!("F2N64_TUNE_AB").and_then(|m| m.trim().parse::<u32>().ok()) {
                f2_game::simbench::TUNE.store(if tune == 0 { mask } else { 0 }, Relaxed);
            }
            log!("sim phases [{}] tune {} sum {} us/tick:{}", self.level_name, tune, sum, s);
            let f = w.frames.max(1);
            let mut s = String::new();
            for i in 0..f2_draw::render::draw_phase::COUNT {
                let us = ticks_to_us(DRAW_PHASES[i].swap(0, core::sync::atomic::Ordering::Relaxed)) / f;
                let _ = core::fmt::Write::write_fmt(&mut s, format_args!(" {} {}", f2_draw::render::draw_phase::NAMES[i], us));
            }
            log!(
                "draw phases [{}] us/frame:{} | refill {} snapshot {} host2d {}",
                self.level_name,
                s,
                ticks_to_us(w.refill / f),
                ticks_to_us(w.snap / f),
                ticks_to_us(w.host2d / f)
            );
        }
        let (rsp, rdp) = rt::rcp_load();
        let ms = |t: u32| ticks_to_us(t) / 1000;
        let f = w.frames.max(1);
        let secs_x100 = ticks_to_us(span) / 10_000;
        log!(
            "perf [{}] mode {}: {} frames, {} sim ticks in {}.{:02} s => {}.{} fps, {} ticks/s (dropped total {}); per frame: sim {} ms, draw {} ms (draw loop {} ms, textures {} ms, build {} ms, CPU-side submit {} ms, framebuffer wait {} ms, r3d queue {} us, flush {} us), worst frame {} ms (longest loop {} ms), audio {} us/frame, pacing {} fps; RSP busy {}.{}%, RDP busy {}.{}%; Game::tick {} us/tick, audio poll {} us/tick; {} cpu tris + {} rects/frame; r3d {} draws ({} reused, {} view-only), {} tris ({} sent to the RSP, {} batches, {} texture loads)/frame; audio: {} voices ({} silent), {} sfx started ({} unknown uids), music track {:?} playing {}",
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
            ms(w.max_period),
            ticks_to_us(w.audio / f),
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
            self.audio.silent(),
            self.audio.plays,
            self.audio.missing,
            self.audio.music_track,
            self.audio.music_playing()
        );
        let fl = rt::flip_stats();
        log!(
            "display [{}]: {} flips, the previous image shown 1 vblank x{}, 2 x{}, 3 x{}, 4 x{}, 5+ x{} (frame limit {})",
            self.level_name,
            fl[1..].iter().sum::<u32>(),
            fl[1],
            fl[2],
            fl[3],
            fl[4],
            fl[5],
            self.fps_limit
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
