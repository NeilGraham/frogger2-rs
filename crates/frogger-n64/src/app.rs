//! The N64 front end: a lean counterpart of the desktop `frogger::app::App` around the
//! unchanged `f2-game` simulation and the shared `f2-draw` draw loop.
//!
//! - **Loop**: fixed 60 Hz ticks paced by the VR4300 count register; each frame runs as many
//!   ticks as real time requires (at most [`MAX_TICKS_PER_FRAME`]; beyond that the backlog is
//!   dropped and counted) and then renders once, so the simulation keeps real time and the
//!   frame rate is whatever rendering allows.
//! - **Data**: [`N64Data`] over the DFS (`data.rs`); levels load from inside the
//!   simulation (`InitLevel` → `take_level_loads`).
//! - **Video**: logo/story FMVs are skipped for now (`Effect::PlayVideo` ignored).
//! - **Rendering**: `render_cpu.rs` (to be replaced by the RSP renderer).

use crate::audio::Audio;
use crate::data::{norm_path, AssetStore, DfsStore, N64Data};
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
use f2_engine::{Camera, Font, Renderer, TexHandle};
use f2_game::assets::DataSource;
use f2_game::session::SessionConfig;
use f2_game::{Effect, Game, TickInput};

/// C0 COUNT ticks per 60 Hz game tick.
pub const TICK: u32 = TICKS_PER_SEC / 60;
/// Most simulation ticks run before one rendered frame (catch-up limit).
pub const MAX_TICKS_PER_FRAME: u32 = 6;
/// Render size (the RDP framebuffer).
pub const WIDTH: u32 = 320;
pub const HEIGHT: u32 = 240;

/// How the ROM starts (build-time `F2N64_BOOT`): `frontend` (default: title → frontend, like
/// the PC executable without its logo FMVs, with the training level marked done as an
/// existing save would have it, since the N64 build has no save medium yet: the story book
/// starts Garden 1 and the arcade machine is open), `frontend-fresh` (a new player: the book
/// starts the training level) or `level W L` (straight into an arcade level).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boot {
    Frontend { trained: bool },
    Level(u32, u32),
}

impl Boot {
    pub fn parse(s: Option<&str>) -> Boot {
        let Some(s) = s else { return Boot::Frontend { trained: true } };
        let mut it = s.split_whitespace();
        match (it.next(), it.next().and_then(|w| w.parse().ok()), it.next().and_then(|l| l.parse().ok())) {
            (Some("level"), Some(w), Some(l)) => Boot::Level(w, l),
            (Some("frontend-fresh"), _, _) => Boot::Frontend { trained: false },
            _ => Boot::Frontend { trained: true },
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
}

/// The renderer side of the loaded level, outside [`App`] so the data source can free it
/// when the simulation starts loading the next level from inside `Game::tick` (`InitLevel`):
/// the PC order (load the new level, then drop the old scene) does not fit in 8 MiB.
pub struct Level {
    pub scene: Option<LevelScene>,
    pub gfx: RenderCpu,
    /// Registry images below this index (white, fonts) survive a level unload.
    pub keep_images: u32,
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
        let keep = l.keep_images;
        l.gfx.free_from(keep);
        log!("app: level render assets freed for the next load");
    }
}

fn lv() -> spin::MutexGuard<'static, Option<Level>> {
    LEVEL.lock()
}

pub struct App {
    data: Arc<N64Data<DfsStore>>,
    game: Game,
    r: Renderer,
    audio: Audio,
    /// `bigfont.bmp` / `smallfont.bmp` found (reloaded from the ROM at each renderer rebuild
    /// so their pixels are not kept twice).
    have_fonts: bool,
    game_fonts: Option<GameFonts>,
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
    last_mode: i32,
    last_pad: u16,
}

fn load_font(d: &N64Data<DfsStore>, p: &str) -> Option<Font> {
    d.store.load(p).and_then(|b| Font::from_bmp(&b))
}

impl App {
    pub fn new(boot: Boot) -> App {
        let mut data = N64Data::new(DfsStore);
        data.before_access = Some(unload_level_render);
        let data = Arc::new(data);
        *LEVEL.lock() = Some(Level { scene: None, gfx: RenderCpu::new(), keep_images: 0 });
        let mut app = App {
            data,
            game: Game::new(),
            r: Renderer::new_soft(),
            audio: Audio::new(),
            have_fonts: true,
            game_fonts: None,
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
            last_mode: -1,
            last_pad: 0,
        };
        app.rebuild_renderer();
        f2_draw::overlay::set_button_prompts(vec![('X', "A".into()), ('C', "B".into()), ('S', "Z".into()), ('T', "C-Up".into())]);
        let cfg = SessionConfig::new(app.data.clone());
        let t0 = ticks();
        app.game = Game::new_session(cfg);
        app.install_game_hooks();
        log!("app: session up in {} ms", ticks_to_us(ticks().wrapping_sub(t0)) / 1000);
        match boot {
            Boot::Frontend { trained } => {
                if trained {
                    let fe = f2_game::layout::worldid::FRONTEND as usize;
                    app.game.layout.world_visual_data[fe].level_visual_data[f2_game::layout::LEVELID_FRONTEND4 as usize].level_completed = 1;
                }
            }
            Boot::Level(w, l) => {
                log!("app: booting straight into level {w}-{l}");
                app.game.start_arcade(w, l);
            }
        }
        app.dispatch_effects();
        app.process_level_loads();
        app.heap_log("boot");
        #[cfg(feature = "profile")]
        unsafe {
            crate::ffi::n64_prof(0);
            if cfg!(feature = "profile-sim") {
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

    /// A fresh `Renderer` (its texture registry cannot free images) with the fonts.
    fn rebuild_renderer(&mut self) {
        let mut g = lv();
        let l = g.as_mut().unwrap();
        l.gfx.reset();
        self.r = Renderer::new_soft();
        self.backdrop = None;
        self.game_fonts = None;
        if self.have_fonts {
            let b = load_font(&self.data, "textures/font/bigfont.bmp").map(|f| self.r.add_font(f));
            l.gfx.upload(&mut self.r.textures);
            let s = load_font(&self.data, "textures/font/smallfont.bmp").map(|f| self.r.add_font(f));
            l.gfx.upload(&mut self.r.textures);
            match (b, s) {
                (Some(normal), Some(small)) => self.game_fonts = Some(GameFonts { normal, small }),
                _ => {
                    log!("app: game fonts (bigfont.bmp/smallfont.bmp) missing");
                    self.have_fonts = false;
                }
            }
        }
        l.keep_images = self.r.textures.images.len() as u32;
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
        }
        self.rebuild_renderer();
        self.audio.free_samples();
        rt::reset_peak();
        let mut banks = Vec::new();
        for f in &assets.object_banks {
            match self.data.object_bank(f) {
                Some(Ok(b)) => banks.push(b),
                Some(Err(e)) => log!("app: {e}"),
                None => log!("app: {f}: not found"),
            }
            rt::audio_poll();
        }
        let scene = LevelScene::new(world, level, banks, assets.texture_banks.clone(), &self.game);
        lv().as_mut().unwrap().scene = Some(scene);
        for f in &assets.texture_banks {
            match self.data.texture_bank(f) {
                Some(Ok(b)) => {
                    let mut g = lv();
                    let gfx = &mut g.as_mut().unwrap().gfx;
                    self.r.textures.load_bank_with(&b, |h, img| gfx.convert_image(h.0, img));
                }
                Some(Err(e)) => log!("app: {e}"),
                None => log!("app: {f}: not found"),
            }
            rt::audio_poll();
        }
        self.load_sfx(world, level);
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
            match self.data.sound_meta(&path) {
                Some(Ok(b)) => self.audio.load_bank(&norm_path(&path), &b),
                Some(Err(e)) => log!("app: {e}"),
                None => log!("app: {path}: not found"),
            }
        }
        const VOICE_BANKS: [&str; 8] = ["frogger", "lillie", "tad", "treeby", "wart", "tank", "roobie", "swampy"];
        for i in 0..self.game.frogger.num_frogs.clamp(0, 4) as usize {
            let Some(name) = VOICE_BANKS.get(self.game.frogger.player[i].character as usize) else { continue };
            let path = format!("sfx/{name}.wus");
            if let Some(Ok(b)) = self.data.sound_meta(&path) {
                self.audio.load_voice_bank(i, &path, &b);
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

    fn tick(&mut self, pad: u16) {
        self.prev = render::snapshot(&self.game);
        self.prev_cam = Some(render::game_camera(&self.game));
        if self.game.frontend.text_entry > 0 {
            self.game.psx_name_entry_frame(pad & !self.prev_pad);
        }
        self.prev_pad = pad;
        let t0 = ticks();
        #[cfg(feature = "profile-sim")]
        unsafe {
            crate::ffi::n64_prof(3)
        };
        self.game.tick(TickInput { pads: [pad, 0, 0, 0] });
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
        self.update_game_view();
        if self.game.backdrop.tiled_backdrop.is_some() {
            self.tile_scroll.step(self.game.game_speed);
        }
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
            Effect::PlayVideo { name } => log!("app: video {name} skipped (FMV not ported yet)"),
            Effect::SaveGame { data } => log!("app: save game ({} bytes) not stored (no save medium yet)", data.len()),
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
            Effect::Quit | Effect::OpenSettings => {}
        }
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
        self.r.begin_frame(WIDTH, HEIGHT);
        let alpha = (self.acc as f32 / TICK as f32).clamp(0.0, 1.0);
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
        };
        let mut g = lv();
        let l = g.as_mut().unwrap();
        if let Some(scene) = &l.scene {
            ctx.cam = render::lerp_camera(self.prev_cam.as_ref(), ctx.cam, alpha);
            ctx.far_clip = render::far_clip(&self.game, scene);
        }
        if l.scene.is_some() || backdrop.is_some() {
            render::draw_loop(&mut self.r, l.scene.as_mut(), &self.game, &ctx);
        }
        if l.scene.is_some() {
            let s = ctx.cam.source;
            self.audio.listener = crate::audio::Listener { cam_source: s.map(|x| (x * 10.0 * 4096.0) as i32), view: self.r.view.v_matrix };
        }
        let clear = match &l.scene {
            Some(s) => s.clear,
            None => [0.0, 0.0, 0.0, 1.0],
        };
        l.gfx.upload(&mut self.r.textures);
        l.gfx.submit(&mut self.r, clear);
    }

    // ------------------------------------------------------------ frame

    /// One iteration of the main loop: catch the simulation up with real time, then draw.
    pub fn frame(&mut self) {
        let t0 = ticks();
        self.acc = self.acc.saturating_add(t0.wrapping_sub(self.last));
        self.last = t0;
        let mut n = self.acc / TICK;
        if n > MAX_TICKS_PER_FRAME {
            self.dropped_ticks += (n - MAX_TICKS_PER_FRAME) as u64;
            self.acc -= (n - MAX_TICKS_PER_FRAME) * TICK;
            n = MAX_TICKS_PER_FRAME;
        }
        let pad = input::psx_pad(rt::pad(0));
        for _ in 0..n {
            self.tick(pad);
            self.acc -= TICK.min(self.acc);
            let tp = ticks();
            rt::audio_poll();
            self.win.poll += ticks().wrapping_sub(tp);
        }
        let t1 = ticks();
        self.draw();
        let t2 = ticks();
        self.frames += 1;
        if core::mem::take(&mut self.loaded) {
            self.win = Window::default();
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
        w.max_frame = w.max_frame.max(t2.wrapping_sub(t0));
        let span = t2.wrapping_sub(self.win_start);
        if span >= 5 * TICKS_PER_SEC {
            self.log_window(span);
        }
    }

    fn log_window(&mut self, span: u32) {
        let w = core::mem::take(&mut self.win);
        let ms = |t: u32| ticks_to_us(t) / 1000;
        let f = w.frames.max(1);
        let secs_x100 = ticks_to_us(span) / 10_000;
        log!(
            "perf [{}] mode {}: {} frames, {} sim ticks in {}.{:02} s => {}.{} fps, {} ticks/s (dropped total {}); per frame: sim {} ms, draw {} ms (build {} ms, rdp submit {} ms), worst frame {} ms; Game::tick {} us/tick, audio poll {} us/tick; {} tris + {} rects/frame; audio: {} voices, {} sfx started ({} unknown uids), music track {:?} playing {}",
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
            ms(w.build / f),
            ms(w.submit / f),
            ms(w.max_frame),
            ticks_to_us(w.game_tick / w.ticks.max(1)),
            ticks_to_us(w.poll / w.ticks.max(1)),
            w.tris / f,
            w.rects / f,
            self.audio.active(),
            self.audio.plays,
            self.audio.missing,
            self.audio.music_track,
            self.audio.music_playing()
        );
        self.heap_log("running");
        #[cfg(feature = "profile")]
        unsafe {
            crate::ffi::n64_prof(1)
        };
        self.win_start = ticks();
    }
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
