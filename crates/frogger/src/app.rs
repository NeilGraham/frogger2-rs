//! The game host: fixed 60 Hz simulation, effect dispatch (audio, video, saves) and
//! interpolated rendering (`docs/architecture.md` §3).

mod export_ui;
mod settings;

pub use settings::{nav as settings_nav, FpsMeter, Page as SettingsPage, SettingsUi, WinInfo};

use crate::controls::{Act, ControlsUi};
use crate::data::Data;
use crate::gameplay::{GAct, GameplayUi, LevelChoice, LevelSelectUi, Speedrun, Tab};
use crate::netplay::{NetNote, NetPlay};
use crate::overlay::{GameFonts, TileScroll};
use crate::render::{self, FlyCam, LevelScene, Snap};
use crate::video::VideoPlayer;
use f2_audio::{AudioEngine, Listener, Play, VoiceId};
use f2_engine::{Camera, Font, FontId, HiRes, Renderer, TexHandle};
use f2_game::define::{mode, SINGLEPLAYER};
use f2_game::ids::ActorId;
use f2_game::{Effect, Game, TickInput};
use f2_platform::{
    dik, Bindings, ControllerFamily, DisplaySettings, Gameplay, HopBuffer, InputMapper, KeyboardState, PadData, Profile, RawEvent, RawPad, TextureFilter,
};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// One logical frame (`gameSpeed == 4096`).
pub const TICK: Duration = Duration::from_nanos(1_000_000_000 / 60);
/// Longest wall-clock gap simulated in one go (slow frames, debugger pauses).
pub const MAX_CATCHUP: Duration = Duration::from_millis(250);

/// Startup/dev sequence steps (until the frontend is ported).
#[derive(Clone, Debug)]
pub enum Step {
    Video(String),
    /// `INGAME_MODE` on a level, single player.
    Level(u32, u32),
    /// The frontend level (start screen) through the game's own session flow.
    Frontend,
    /// The full start-up: `CommonInit` (title backdrop), then the game plays the logo FMVs
    /// and loads the frontend level by itself.
    Boot,
    /// Local multiplayer: world, level, number of frogs (mode from `multiGameTypes`).
    Multi(u32, u32, i32),
    /// The recorded demo of a level (`DEMO_MODE`, `DEMOS\RECORD-W-L.KEY`).
    Demo(u32, u32),
    /// Level select / practice (P6-1): a story chapter or an arcade level through a game
    /// session, as the F10 level-select page starts it.
    Choice(LevelChoice),
    /// The level editor on a level (`--editor`).
    Editor(u32, u32),
}

/// Host-only toggles requested by hotkeys, applied by the window layer.
#[derive(Default)]
pub struct Requests {
    pub quit: bool,
    pub toggle_fullscreen: bool,
    /// Bindings were edited in the controls screen: persist them.
    pub save_bindings: bool,
    /// The game changed the music / sound volume (0..=100): persist them in the config.
    pub save_volumes: Option<(i32, i32)>,
    /// The gameplay tab changed `[gameplay]` / the profile: persist them (P6-1).
    pub save_gameplay: bool,
    /// The settings screen changed `cfg`: persist it.
    pub save_config: bool,
    /// Window mode / resolution / refresh rate changed in `cfg.video`: apply them.
    pub apply_window: bool,
    /// `cfg.video.vsync` changed: reconfigure the swap chain.
    pub apply_vsync: bool,
    /// Rumble settings changed: push them to the gamepads.
    pub apply_input: bool,
}

pub struct App {
    pub data: Data,
    gfx: Gfx,
    pub r: Renderer,
    font: Option<Font>,
    font_id: Option<FontId>,
    /// `bigfont.bmp` / `smallfont.bmp` (`pcFont` / `pcFontSmall`).
    game_font_files: Option<(Font, Font)>,
    /// `monofont.bmp` (`fontWhite`, PC/Main.cpp `InitFont`s): the story book page text.
    book_font: Option<Font>,
    game_fonts: Option<GameFonts>,
    /// The modern versions of `game_font_files` (`textures/font/{bigfont,smallfont}.f2mf`,
    /// built by `f2-fontgen` from the typeface the pixel fonts came from).
    modern_font_files: Option<(HiRes, HiRes)>,
    /// (modern, atlas scale) `game_fonts` were registered with.
    game_fonts_key: (bool, u32),
    /// Full-screen backdrop (`mdxLoadBackdrop`): file and image.
    backdrop: Option<(String, Option<TexHandle>)>,
    tile_scroll: TileScroll,
    pub audio: AudioEngine,
    pub game: Game,
    pub scene: Option<LevelScene>,
    steps: VecDeque<Step>,
    pub video: Option<VideoPlayer>,
    video_queue: VecDeque<String>,
    // input
    pub mapper: InputMapper,
    /// Multi-binding table (keys + pad inputs); keys are mirrored into `mapper`.
    pub bindings: Bindings,
    pub deadzone: f32,
    /// One enhanced-profile hop buffer per player; the window applies in the enhanced profile.
    hop: [HopBuffer; 4],
    pub hop_frames: u32,
    /// Controller family per player (button prompts), set by the host.
    pub families: [ControllerFamily; 4],
    /// The F10 rebinding screen (simulation paused while open).
    pub controls: Option<ControlsUi>,
    /// Rumble requests `(player, strength, frames)` for the host to route to the pads.
    pub rumble: Vec<(u8, u8, u16)>,
    pad: PadData,
    /// Keyboard state accumulated since the last tick (key-down events must not be lost on
    /// frames without a tick).
    kb: KeyboardState,
    // timing
    acc: Duration,
    prev: Vec<Option<Snap>>,
    /// Game camera before the last tick (interpolation).
    prev_cam: Option<Camera>,
    pub ticks: u64,
    /// `ticks` at the last draw: animated textures step once per game frame
    /// (`DrawCtx::tex_anim_steps`), not once per displayed frame.
    tex_anim_ticks: u64,
    // view
    free_cam: Option<FlyCam>,
    last_cam: Option<Camera>,
    show_tiles: bool,
    // audio handles
    handles: HashMap<u32, VoiceId>,
    sfx_loops: HashMap<(ActorId, i16), VoiceId>,
    missing_samples: std::collections::HashSet<u32>,
    pub requests: Requests,
    pub status: String,
    width: u32,
    height: u32,
    // display
    /// Active profile (F5 toggles).
    pub profile: Profile,
    /// Settings used in the enhanced profile (config + F-key changes).
    pub enhanced: DisplaySettings,
    /// On-screen message after a display hotkey, and its remaining time.
    osd: Option<(String, f32)>,
    /// Draw sample overlays in the 4096 space to check anchoring (`--hud-probe`).
    /// The frame generation setting the host's DLSS / FSR path started with (`None` = not
    /// running): the settings screen says when a change needs a restart.
    pub upscale_native: Option<f2_platform::FrameGeneration>,
    /// What DLSS / FSR run (desktop host, `[upscale]`), shown by the settings screen.
    pub upscale_status: Option<String>,
    pub hud_probe: bool,
    /// Prompts as if player 1 used a pad (screenshots of console prompts: `--prompts ps`).
    pub force_pad_prompts: bool,
    /// Scripted pad 0 buttons (`--pad-script`): replaces player 1's input when set.
    pub pad_override: Option<u16>,
    /// `savegame.dat` read at session start and written by `SaveGame` (None: no saves,
    /// e.g. headless runs).
    pub save_path: Option<std::path::PathBuf>,
    /// Save block used instead of `save_path`'s content (`--dev-unlock`).
    pub save_data: Option<Vec<u8>>,
    /// `--pad-script`: sets `pad_override` at the scripted ticks.
    pub pad_script: Option<crate::padscript::PadScript>,
    /// `--record`: records the session for the N64 replay (and gives it the N64's options).
    pub record: Option<crate::record::Recorder>,
    /// The texture last copied onto the arcade screen (`menus.arcade_screen_copy`) and
    /// the source image it came from.
    arcade_copied: Option<(String, TexHandle)>,
    // netplay (P6-2)
    /// Online match (lobby / running / ended), `--host` / `--join`.
    pub net: Option<NetPlay<f2_net::UdpTransport>>,
    /// Clock of the netplay session.
    net_t0: Instant,
    /// After a match ended abnormally: when to return to the frontend.
    net_leave_at: Option<Instant>,
    // ---- P6-1 gameplay QoL
    /// `[gameplay]` options (assists apply in the enhanced profile).
    pub gameplay: Gameplay,
    /// Slot 1's `savegame.dat` (other slots live next to it, `gameplay::slot_path`).
    pub save_slot1: Option<std::path::PathBuf>,
    /// Speedrun timer (shown when `gameplay.speedrun_timer`, enhanced profile).
    pub speedrun: Speedrun,
    /// Page of the F10 settings screen.
    tab: Tab,
    gameplay_ui: GameplayUi,
    level_ui: LevelSelectUi,
    // ---- P7 level editor
    /// The level editor (`--editor`), when active.
    pub editor: Option<crate::editor::Editor>,
    /// Mod folder / id the editor saves to (`--mods-dir`, first `--mods`/`--mod` id).
    pub editor_mods: Option<(std::path::PathBuf, String)>,
    /// Commands run once the editor has loaded (`--editor-cmd`).
    pub editor_cmds: Vec<String>,
    /// Shift held this frame (Shift+F10).
    kb_shift: bool,
    // ---- settings screen
    /// The configuration the settings screen edits (the host saves it).
    pub cfg: f2_platform::Config,
    /// Window state for the Display page (host-updated).
    pub win: WinInfo,
    /// The in-game settings screen (simulation paused while open).
    pub settings: Option<SettingsUi>,
    /// Adds the "Settings" items to the game's menus (`GameOptions::settings_menu`).
    pub settings_menu: bool,
    /// Settings → Export state (export_ui.rs).
    #[cfg_attr(not(feature = "export"), allow(dead_code))]
    exports: export_ui::ExportUi,
    /// Rendered frame rate.
    pub fps: FpsMeter,
    /// Pad bits masked from player 1 until released (after closing the settings screen).
    pad_hold: u16,
    /// `windowActive`.
    focused: bool,
    /// Language columns with text in the rendition (Gameplay page).
    languages: Vec<usize>,
}

/// `--host` / `--join` request from the command line.
#[derive(Clone, Debug)]
pub enum NetRequest {
    Host { world: u32, level: u32, players: u8, port: u16, ip: Option<std::net::IpAddr> },
    Join { code: String },
}

/// What the renderer is (re)created from.
#[cfg(feature = "gpu")]
struct Gfx {
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
}

/// No GPU: the renderer draws on the CPU (`Renderer::render_soft`).
#[cfg(not(feature = "gpu"))]
struct Gfx;

impl Gfx {
    #[cfg(feature = "gpu")]
    fn renderer(&self) -> Renderer {
        Renderer::new(&self.device, &self.queue, self.format)
    }

    #[cfg(not(feature = "gpu"))]
    fn renderer(&self) -> Renderer {
        Renderer::new_soft()
    }
}

impl App {
    #[cfg(feature = "gpu")]
    pub fn new(data: Data, device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, audio: AudioEngine, mapper: InputMapper) -> App {
        Self::with_gfx(data, Gfx { device: device.clone(), queue: queue.clone(), format }, audio, mapper)
    }

    /// An app whose renderer draws on the CPU (builds without the `desktop` feature).
    #[cfg(not(feature = "gpu"))]
    pub fn new_soft(data: Data, audio: AudioEngine, mapper: InputMapper) -> App {
        Self::with_gfx(data, Gfx, audio, mapper)
    }

    fn with_gfx(data: Data, gfx: Gfx, mut audio: AudioEngine, mapper: InputMapper) -> App {
        let font = ["textures/font/smallfont.bmp", "textures/font/monofont.bmp"].iter().find_map(|p| data.read(p).ok().and_then(|b| Font::from_bmp(&b)));
        let load_font = |p: &str| data.read(p).ok().and_then(|b| Font::from_bmp(&b));
        let book_font = load_font("textures/font/monofont.bmp");
        let game_font_files = match (load_font("textures/font/bigfont.bmp"), load_font("textures/font/smallfont.bmp")) {
            (Some(mut b), Some(mut s)) => {
                // Advances for the button-icon glyphs (`button_prompts` on PlayStation pads).
                b.add_icon_glyphs();
                s.add_icon_glyphs();
                Some((b, s))
            }
            _ => {
                eprintln!("frogger: game fonts (bigfont.bmp/smallfont.bmp) missing");
                None
            }
        };
        // Modern fonts: built on first run from the system typeface (desktop), else whatever
        // was generated into the data (consoles get them with the bundled data).
        #[cfg(feature = "fontgen")]
        match f2_fontgen::ensure(&data.rendition) {
            Ok(Some(log)) => log.iter().for_each(|l| eprintln!("frogger: modern font: {l}")),
            Ok(None) => {}
            Err(e) => eprintln!("frogger: modern font: {e}"),
        }
        let load_hires = |p: &str| match data.read(p).map(|b| HiRes::decode(&b)) {
            Ok(Ok(h)) => Some(h),
            Ok(Err(e)) => {
                eprintln!("frogger: {p}: {e}");
                None
            }
            Err(_) => None,
        };
        let modern_font_files = match (load_hires("textures/font/bigfont.f2mf"), load_hires("textures/font/smallfont.f2mf")) {
            (Some(b), Some(s)) => Some((b, s)),
            _ => None,
        };
        audio.set_music_dir(data.rendition.join("music"));
        let mut app = App {
            data,
            r: gfx.renderer(),
            gfx,
            font,
            font_id: None,
            game_font_files,
            book_font,
            game_fonts: None,
            modern_font_files,
            game_fonts_key: (false, 0),
            backdrop: None,
            tile_scroll: TileScroll::default(),
            audio,
            game: Game::new(),
            scene: None,
            steps: VecDeque::new(),
            video: None,
            video_queue: VecDeque::new(),
            mapper,
            bindings: Bindings::default(),
            deadzone: f2_platform::bindings::DEFAULT_DEADZONE,
            hop: Default::default(),
            hop_frames: 0,
            families: [ControllerFamily::Generic; 4],
            controls: None,
            rumble: Vec::new(),
            pad: PadData::default(),
            kb: KeyboardState::default(),
            acc: Duration::ZERO,
            prev: Vec::new(),
            prev_cam: None,
            ticks: 0,
            tex_anim_ticks: 0,
            free_cam: None,
            last_cam: None,
            show_tiles: false,
            handles: HashMap::new(),
            sfx_loops: HashMap::new(),
            missing_samples: Default::default(),
            requests: Requests::default(),
            status: String::new(),
            width: 640,
            height: 480,
            profile: Profile::Enhanced,
            enhanced: DisplaySettings::ENHANCED,
            osd: None,
            upscale_native: None,
            upscale_status: None,
            hud_probe: false,
            force_pad_prompts: false,
            pad_override: None,
            save_path: None,
            save_data: None,
            pad_script: None,
            record: None,
            arcade_copied: None,
            net: None,
            net_t0: Instant::now(),
            net_leave_at: None,
            gameplay: Gameplay::default(),
            save_slot1: None,
            speedrun: Speedrun::default(),
            tab: Tab::default(),
            gameplay_ui: GameplayUi::default(),
            level_ui: LevelSelectUi::default(),
            editor: None,
            editor_mods: None,
            editor_cmds: Vec::new(),
            kb_shift: false,
            cfg: f2_platform::Config::default(),
            win: WinInfo { size: (640, 480), ..Default::default() },
            settings: None,
            settings_menu: true,
            exports: Default::default(),
            fps: FpsMeter::default(),
            pad_hold: 0,
            focused: true,
            languages: Vec::new(),
        };
        if !app.data.levels.is_empty() {
            app.game.install_level_registry(app.data.levels.clone()); // mod levels (P7-1)
        }
        app.rebuild_renderer();
        app.install_game_hooks();
        app
    }

    /// Host hooks of the simulation: font metrics for `CalcStringWidth` (textover.c) and,
    /// after each tick, the view transform for `IsPointVisible` (specfx.c).
    fn install_game_hooks(&mut self) {
        if let Some((b, s)) = &self.game_font_files {
            let w = |f: &Font| (0..=255u8).map(|c| f.char_width(c).min(255) as u8).collect::<Vec<u8>>();
            self.game.textover.metrics = Some(f2_game::textover::FontMetrics { normal: w(b), small: w(s) });
        }
    }

    /// Sets the view `IsPointVisible` uses during the next tick (the camera just computed).
    fn update_game_view(&mut self) {
        let far = match &self.scene {
            Some(s) => render::far_clip(&self.game, s),
            None => return,
        };
        self.game.specfx.view = Some(render::spec_fx_view(&self.game, far));
    }

    /// Selects the profile and the enhanced-profile settings (from `config.toml`).
    pub fn set_display(&mut self, profile: Profile, enhanced: DisplaySettings) {
        self.profile = profile;
        self.enhanced = enhanced;
        self.apply_display();
    }

    /// The settings in effect.
    pub fn display(&self) -> DisplaySettings {
        match self.profile {
            Profile::Authentic => DisplaySettings::AUTHENTIC,
            Profile::Enhanced => self.enhanced,
        }
    }

    /// Temporal upscaling for the next [`draw`](Self::draw): the 3D render size and this
    /// frame's sub-pixel jitter (`None` = off). Motion vectors are recorded while it is on.
    pub fn prepare_upscale(&mut self, up: Option<((u32, u32), [f32; 2])>) {
        self.r.options.motion_vectors = up.is_some();
        self.r.upscale_render = up.map(|u| u.0);
        self.r.jitter = up.map_or([0.0; 2], |u| u.1);
    }

    /// Pushes the display settings into the renderer options (fog and far clip are set
    /// per frame from the level).
    fn apply_display(&mut self) {
        let d = self.display();
        let o = &mut self.r.options;
        o.perspective_correct = d.perspective_correct;
        o.linear_filter = d.filter != TextureFilter::Nearest;
        o.anisotropy = if d.filter == TextureFilter::Anisotropic { d.anisotropy.max(1) } else { 0 };
        o.hor_plus = d.hor_plus;
        o.hud_anchor = d.hud_anchor;
        o.max_aspect = d.max_aspect;
        o.render_scale = d.render_scale;
        o.msaa = d.msaa;
        // Extended culling: clip faces at the near / far planes and keep the ones the screen
        // test (no vertex inside the screen on an axis) would drop although they cover it.
        o.clip_faces = d.clip_faces;
        o.screen_reject = !d.clip_faces;
        // Retail draws translucent triangles in submission order (`sortMode` is never set);
        // the enhanced profile sorts them back to front.
        o.xlu_sort = match self.profile {
            Profile::Authentic => f2_engine::XluSort::Submission,
            Profile::Enhanced => f2_engine::XluSort::BackToFront,
        };
    }

    /// F-key display toggles (quick A/B). Changing an enhanced option switches to the
    /// enhanced profile (authentic is fixed).
    fn display_hotkey(&mut self, k: u8) {
        let e = &mut self.enhanced;
        let msg = match k {
            dik::F5 => {
                self.profile = self.profile.toggled();
                format!("profile: {}", self.profile.name())
            }
            dik::F3 => {
                e.perspective_correct = !e.perspective_correct;
                format!("perspective-correct textures: {}", on_off(e.perspective_correct))
            }
            dik::F4 => {
                e.render_scale = match e.render_scale {
                    s if s < 1.5 => 1.5,
                    s if s < 2.0 => 2.0,
                    _ => 1.0,
                };
                format!("render scale: {}x", e.render_scale)
            }
            dik::F6 => {
                e.msaa = if e.msaa > 1 { 1 } else { 4 };
                format!("MSAA: {}x", e.msaa)
            }
            dik::F7 => {
                e.filter = match e.filter {
                    TextureFilter::Nearest => TextureFilter::Linear,
                    TextureFilter::Linear => TextureFilter::Anisotropic,
                    TextureFilter::Anisotropic => TextureFilter::Nearest,
                };
                format!("texture filter: {:?}", e.filter).to_lowercase()
            }
            dik::F8 => {
                let on = !(e.hor_plus && e.hud_anchor);
                (e.hor_plus, e.hud_anchor) = (on, on);
                format!("widescreen (Hor+ FOV, anchored HUD): {}", on_off(on))
            }
            dik::F9 => {
                e.fog = !e.fog;
                format!("fog: {}", on_off(e.fog))
            }
            dik::F12 => {
                e.draw_distance = match e.draw_distance {
                    d if d < 2.0 => 2.0,
                    d if d < 4.0 => 4.0,
                    _ => 1.0,
                };
                format!("draw distance: {}x", e.draw_distance)
            }
            _ => return,
        };
        if k != dik::F5 {
            self.profile = Profile::Enhanced;
        }
        self.apply_display();
        eprintln!("frogger: {msg}");
        self.osd = Some((msg, 2.5));
    }

    /// (Re-)registers the game fonts when the font setting or the output size changed: the
    /// modern atlases are resampled to about their on-screen size (sampled ~1:1, no mipmaps).
    fn sync_game_fonts(&mut self) {
        let Some((big, small)) = &self.game_font_files else { return };
        // Button icons about as tall as the big font's capitals on screen. The HUD is drawn at
        // the output size (with an upscaler the 3D render size is smaller), so size by that.
        let screen_h = self.r.output_size().1.max(self.r.view.height as u32) as f32;
        self.r.set_button_icons(((big.height as f32 * 0.8 * screen_h / 480.0).ceil() as u32).clamp(12, 256));
        let modern = self.modern_font_files.as_ref().filter(|_| self.display().modern_font);
        // Atlas pixels per original pixel: the 480-line magnification, with headroom for
        // zoomed text.
        let k = modern.map_or(0, |(b, _)| ((screen_h / 480.0 * 1.5).ceil() as u32).clamp(2, b.scale as u32));
        let key = (modern.is_some(), k);
        if self.game_fonts.is_some() && self.game_fonts_key == key {
            return;
        }
        self.game_fonts_key = key;
        self.game_fonts = Some(match modern {
            Some((hb, hs)) => GameFonts { normal: self.r.add_font_hires(big.clone(), hb, k as f32), small: self.r.add_font_hires(small.clone(), hs, k as f32) },
            None => GameFonts { normal: self.r.add_font(big.clone()), small: self.r.add_font(small.clone()) },
        });
    }

    /// Queues the startup / dev sequence.
    pub fn push_steps(&mut self, steps: impl IntoIterator<Item = Step>) {
        self.steps.extend(steps);
    }

    /// Recreates the renderer (the texture registry cannot free images) and reloads the
    /// level's texture banks.
    fn rebuild_renderer(&mut self) {
        self.r = self.gfx.renderer();
        self.apply_display();
        self.font_id = self.font.clone().map(|f| self.r.add_font(f));
        self.game_fonts = None;
        self.sync_game_fonts();
        self.backdrop = None;
        self.arcade_copied = None;
        if let Some(v) = &mut self.video {
            v.forget_image();
        }
        let files = self.scene.as_ref().map(|s| s.tex_files.clone()).unwrap_or_default();
        for f in files {
            // Through the DataSource: the effective bank when mods are enabled.
            match f2_game::assets::DataSource::texture_bank(&self.data, &f) {
                Some(Ok(b)) => self.r.load_tex_bank(&b),
                Some(Err(e)) => eprintln!("frogger: {e}"),
                None => eprintln!("frogger: {f}: not found"),
            }
        }
    }

    // ------------------------------------------------------------ steps / levels

    /// Advances the startup sequence when nothing is playing.
    fn next_step(&mut self) {
        while self.video.is_none() {
            let Some(step) = self.steps.pop_front() else { return };
            match step {
                Step::Video(name) => self.start_video(&name),
                Step::Level(w, l) => {
                    if let Err(e) = self.start_level(w, l) {
                        eprintln!("frogger: level {w}-{l}: {e}");
                        self.status = format!("level {w}-{l} failed: {e}");
                    }
                }
                Step::Frontend => self.start_frontend(),
                Step::Choice(c) => self.start_choice(c),
                Step::Boot => {
                    let cfg = self.session_config();
                    self.game = Game::new_session(cfg);
                    self.install_game_hooks();
                    self.scene = None;
                    self.dispatch_effects();
                }
                Step::Demo(w, l) => {
                    self.game.session.data = Some(std::sync::Arc::new(self.data.clone()));
                    match self.game.init_demo_level(w, l, &self.data) {
                        Ok(assets) => {
                            self.dispatch_effects();
                            self.load_scene(w, l, &assets);
                        }
                        Err(e) => eprintln!("frogger: demo {w}-{l}: {e}"),
                    }
                }
                Step::Editor(w, l) => {
                    if let Err(e) = self.start_editor(w, l) {
                        eprintln!("frogger: editor {w}-{l}: {e}");
                        self.status = format!("editor {w}-{l} failed: {e}");
                    }
                }
                Step::Multi(w, l, n) => {
                    if let Err(e) = self.start_multi(w, l, n) {
                        eprintln!("frogger: multiplayer level {w}-{l}: {e}");
                        self.status = format!("multiplayer level {w}-{l} failed: {e}");
                    }
                }
            }
        }
    }

    fn start_video(&mut self, name: &str) {
        match VideoPlayer::open(&self.data, name, self.audio.is_realtime()) {
            Ok(v) => {
                eprintln!("frogger: playing video {name}");
                self.audio.stop_all();
                self.video = Some(v);
            }
            Err(e) => eprintln!("frogger: video {name}: {e}"),
        }
    }

    pub fn end_video(&mut self) {
        if let Some(mut v) = self.video.take() {
            v.release(&mut self.r);
            self.audio.stop_music();
        }
        // `StartVideoPlayback` calls in one frame play one after the other.
        if let Some(next) = self.video_queue.pop_front() {
            self.start_video(&next);
        }
    }

    /// Starts `INGAME_MODE` on `worldVisualData[world].levelVisualData[level]`, single player
    /// (what the frontend sets up before `InitLevel`).
    pub fn start_level(&mut self, world: u32, level: u32) -> Result<(), String> {
        let g = &mut self.game;
        g.session.data = Some(std::sync::Arc::new(self.data.clone()));
        g.game_state.mode = mode::INGAME_MODE;
        g.game_state.multi = SINGLEPLAYER;
        g.frogger.num_frogs = 1;
        let assets = g.load_level(world, level, &self.data)?;
        self.dispatch_effects();
        self.load_scene(world, level, &assets);
        Ok(())
    }

    /// Loads a level and opens the editor on its files (simulation paused).
    pub fn start_editor(&mut self, world: u32, level: u32) -> Result<(), String> {
        self.start_level(world, level)?;
        let doc = crate::editor::doc::LevelDoc::load(&self.data, world, level)?;
        let cam = FlyCam::from_camera(&render::game_camera(&self.game));
        let mut ed = crate::editor::Editor::new(doc, cam);
        if let Some((dir, id)) = &self.editor_mods {
            ed.mods_dir = dir.clone();
            ed.mod_id = id.clone();
        }
        let start = ed.doc.col.player_start[0].max(0) as usize;
        ed.look_at_tile(start);
        for c in std::mem::take(&mut self.editor_cmds) {
            ed.run(&c);
        }
        self.editor = Some(ed);
        self.process_editor_requests();
        Ok(())
    }

    /// Opens the editor on the loaded level (keeping the camera), or closes it.
    pub fn toggle_editor(&mut self) {
        if self.editor.take().is_some() {
            self.status.clear();
            return;
        }
        let Some(scene) = &self.scene else { return };
        let (w, l) = (scene.world, scene.level);
        match crate::editor::doc::LevelDoc::load(&self.data, w, l) {
            Ok(doc) => {
                let cam = self.last_cam.as_ref().map(FlyCam::from_camera).unwrap_or_else(|| FlyCam::from_camera(&render::game_camera(&self.game)));
                let mut ed = crate::editor::Editor::new(doc, cam);
                if let Some((dir, _)) = &self.editor_mods {
                    ed.mods_dir = dir.clone();
                }
                self.free_cam = None;
                self.editor = Some(ed);
            }
            Err(e) => self.status = format!("editor: {e}"),
        }
    }

    /// Whether the editor is open and the simulation paused.
    pub fn editing(&self) -> bool {
        self.editor.as_ref().is_some_and(|e| !e.playing)
    }

    /// Reloads / opens `$EDITOR` as the editor asked.
    pub fn process_editor_requests(&mut self) {
        let Some(ed) = &mut self.editor else { return };
        let reqs = std::mem::take(&mut ed.requests);
        for r in reqs {
            match r {
                crate::editor::Request::Reload { play } => {
                    let Some(ed) = &self.editor else { return };
                    let (w, l) = (ed.doc.world, ed.doc.level);
                    let data: std::sync::Arc<dyn f2_game::assets::DataSource + Send + Sync> =
                        std::sync::Arc::new(ed.doc.overlay(std::sync::Arc::new(self.data.clone())));
                    let g = &mut self.game;
                    g.session.data = Some(data.clone());
                    g.game_state.mode = mode::INGAME_MODE;
                    g.game_state.multi = SINGLEPLAYER;
                    g.frogger.num_frogs = 1;
                    match g.load_level(w, l, &*data) {
                        Ok(assets) => {
                            self.dispatch_effects();
                            self.load_scene(w, l, &assets);
                            let ed = self.editor.as_mut().unwrap();
                            ed.msg(if play { "play-test: level reloaded from the edited files" } else { "level reloaded from the edited files" });
                        }
                        Err(e) => {
                            let ed = self.editor.as_mut().unwrap();
                            ed.playing = false;
                            ed.msg(format!("error: reload failed: {e}"));
                        }
                    }
                }
                crate::editor::Request::EditScript => self.edit_script_external(),
            }
        }
    }

    /// `script edit`: writes the source to a temp file, runs `$VISUAL`/`$EDITOR`
    /// (blocking) and compiles the result.
    fn edit_script_external(&mut self) {
        let Some(ed) = &mut self.editor else { return };
        let src = match ed.doc.script_source() {
            Ok(s) => s.to_string(),
            Err(e) => return ed.msg(format!("error: {e}")),
        };
        let path = std::env::temp_dir().join(format!("f2-editor-script-{}-{}.etc", ed.doc.world, ed.doc.level));
        if let Err(e) = std::fs::write(&path, &src) {
            return ed.msg(format!("error: {}: {e}", path.display()));
        }
        let editor =
            std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")).unwrap_or_else(|_| if cfg!(windows) { "notepad".into() } else { "vi".into() });
        let mut parts = editor.split_whitespace();
        let status = std::process::Command::new(parts.next().unwrap_or("vi")).args(parts).arg(&path).status();
        match status {
            Ok(st) if st.success() => match std::fs::read_to_string(&path) {
                Ok(new) if new == src => ed.msg("script unchanged"),
                Ok(new) => match ed.doc.set_script_source(new) {
                    Ok(w) => ed.msg(format!("script compiled ({} warnings); R reloads", w.len())),
                    Err(e) => ed.msg(format!("error: script does not compile (kept the old .fev):\n{e}")),
                },
                Err(e) => ed.msg(format!("error: {e}")),
            },
            Ok(st) => ed.msg(format!("error: {editor} exited with {st}")),
            Err(e) => ed.msg(format!("error: running {editor}: {e} (set $EDITOR)")),
        }
        // Tell the user where the edited copy lives.
        ed.msg(format!("script source: {}", path.display()));
        self.acc = Duration::ZERO;
    }

    /// Local multiplayer on `world`/`level` with `n` frogs (what `GoMulti`, options.c:966,
    /// sets up before `InitLevel`): one screen, the camera framing all frogs.
    pub fn start_multi(&mut self, world: u32, level: u32, n: i32) -> Result<(), String> {
        let assets = crate::netplay::setup_multi(&mut self.game, &self.data, world, level, n)?;
        self.dispatch_effects();
        self.load_scene(world, level, &assets);
        Ok(())
    }

    /// The session set-up: game files and `SaveData\savegame.dat` (`LoadGame`).
    fn session_config(&self) -> f2_game::session::SessionConfig {
        let mut cfg = f2_game::session::SessionConfig::new(std::sync::Arc::new(self.data.clone()));
        cfg.save_data = self.save_data.clone().or_else(|| self.save_path.as_ref().and_then(|p| std::fs::read(p).ok()));
        cfg.extra_levels = self.data.levels.clone();
        cfg.scripts = self.data.scripts.clone(); // P7b
        cfg.options = self.game_options();
        cfg.language = settings::lang_index(&self.cfg.language);
        if self.record.is_some() {
            // The N64 replay's session: no save, no mods, the default language.
            cfg = f2_game::session::SessionConfig { options: crate::record::n64_options(), ..f2_game::session::SessionConfig::new(cfg.data) };
        }
        cfg
    }

    // ------------------------------------------------------------ P6-1 gameplay QoL

    /// Sets the `[gameplay]` options and slot 1's save file (host start-up).
    pub fn set_gameplay(&mut self, gp: Gameplay, slot1: Option<std::path::PathBuf>) {
        self.gameplay = gp;
        self.save_slot1 = slot1;
        self.apply_save_slot();
        self.apply_game_options();
    }

    /// The simulation options for the current profile and `[gameplay]`.
    pub fn game_options(&self) -> f2_game::enhance::GameOptions {
        let mut o = crate::gameplay::game_options(self.profile, &self.gameplay);
        // Offline play: the "Settings" entries of the pause menus / Extras (never online:
        // matches use `fresh_game`'s authentic options).
        o.settings_menu = self.settings_menu;
        if self.record.is_some() {
            return crate::record::n64_options();
        }
        o
    }

    /// Sets the configuration (start-up) the settings screen edits.
    pub fn set_config(&mut self, cfg: f2_platform::Config) {
        self.cfg = cfg;
    }

    /// Copies the state other screens changed (bindings, gameplay, profile, volumes) into
    /// `cfg` before it is saved.
    pub fn sync_cfg(&mut self) {
        self.cfg.set_bindings(&self.bindings);
        self.cfg.gameplay = self.gameplay.clone();
        self.cfg.profile = self.profile;
        self.cfg.audio.music = self.audio.music_volume().clamp(0, 100) as u8;
        self.cfg.audio.sfx = self.audio.sfx_volume().clamp(0, 100) as u8;
        self.cfg.input.deadzone = self.deadzone;
    }

    /// Host QoL (FMV skipping, logos, timer, auto-save) is on in the enhanced profile.
    /// Applies [`App::game_options`] to the game, except during an online match (peers
    /// must keep identical options: matches run authentic, see `fresh_game`).
    fn apply_game_options(&mut self) {
        if self.net.is_none() {
            self.game.set_game_options(self.game_options());
        }
    }

    fn qol(&self) -> bool {
        self.profile == Profile::Enhanced
    }

    /// Points `save_path` at the selected slot.
    fn apply_save_slot(&mut self) {
        if let Some(p) = &self.save_slot1 {
            self.save_path = Some(crate::gameplay::slot_path(p, self.gameplay.save_slot));
        }
    }

    /// Writes the current progress (`MakeSaveGameBlock`) to the selected slot.
    pub fn save_now(&mut self) -> Result<std::path::PathBuf, String> {
        let path = self.save_path.clone().ok_or("saving is disabled (no save file)")?;
        let data = self.game.make_save_game_block();
        write_save(&path, &data)?;
        Ok(path)
    }

    /// Starts a story chapter or a practice (arcade) level from the level-select page; a
    /// session (title / frontend) is created first if there is none.
    pub fn start_choice(&mut self, c: LevelChoice) {
        if !self.has_session() {
            self.start_frontend();
        }
        self.end_video();
        self.video_queue.clear();
        match c {
            LevelChoice::Story(n) => self.game.start_story(n),
            LevelChoice::Arcade(w, l) => self.game.start_arcade(w, l),
        }
        self.dispatch_effects();
        self.process_level_loads();
        if let Some(r) = &mut self.record {
            let start = match c {
                LevelChoice::Story(n) => format!("story {n}"),
                LevelChoice::Arcade(w, l) => format!("practice {w} {l}"),
            };
            r.start(&start, &self.game);
        }
        // A fresh level: no interpolation from the previous scene.
        self.prev.clear();
        self.prev_cam = None;
    }

    /// A key on the gameplay / level-select pages.
    fn settings_key(&mut self, k: u8) {
        let act = match self.tab {
            Tab::Gameplay => self.gameplay_ui.key(k, &mut self.profile, &mut self.gameplay),
            _ => self.level_ui.key(k),
        };
        match act {
            GAct::None => {}
            GAct::Close => self.toggle_controls(),
            GAct::Changed => {
                self.apply_display();
                self.apply_save_slot();
                self.apply_game_options();
                self.requests.save_gameplay = true;
                self.gameplay_ui.msg.clear();
            }
            GAct::LoadSlot => {
                self.apply_save_slot();
                self.requests.save_gameplay = true;
                self.toggle_controls();
                self.start_frontend();
                self.osd = Some((format!("save slot {} loaded", self.gameplay.save_slot), 2.5));
            }
            GAct::SaveNow => {
                self.gameplay_ui.msg = match self.save_now() {
                    Ok(p) => format!("saved to slot {} ({})", self.gameplay.save_slot, p.display()),
                    Err(e) => e,
                };
            }
            GAct::Start(c) => {
                self.toggle_controls();
                self.start_choice(c);
            }
        }
    }

    /// The real start-up: a game session (`CommonInit`), straight into the frontend
    /// level (the logo FMVs are host steps).
    pub fn start_frontend(&mut self) {
        let cfg = self.session_config();
        self.game = Game::new_session(cfg);
        self.install_game_hooks();
        self.game.start_frontend();
        self.dispatch_effects();
        self.process_level_loads();
        self.grab_textures();
    }

    /// The PC renderer's texture grabs after a frame (drawloop.cpp:376): the story book
    /// page (`grabToTexture`, `DrawPageB`).
    fn grab_textures(&mut self) {
        if self.game.options.grab_to_texture == 1 || self.game.options.grab_to_texture == 3 {
            let scale = if self.profile == Profile::Authentic { 1 } else { crate::book::ENHANCED_SCALE };
            crate::book::process_grab(&mut self.r, &mut self.game, self.book_font.as_ref(), scale);
        }
    }

    /// Names of player 1's face buttons for the `@X`/`@T`/`@S`/`@C` text codes: the first
    /// key binding on the keyboard, the first pad binding on a joypad.
    fn button_prompts(&self) -> Vec<(char, String)> {
        use f2_game::levplay::{PAD_CIRCLE, PAD_CROSS, PAD_SQUARE, PAD_TRIANGLE};
        let pad = self.force_pad_prompts || !matches!(self.mapper.controllers[0], f2_platform::Controller::Keyboard);
        [('X', PAD_CROSS), ('T', PAD_TRIANGLE), ('S', PAD_SQUARE), ('C', PAD_CIRCLE)]
            .into_iter()
            .filter_map(|(c, a)| {
                let b = self.bindings.get(0, a);
                let pick = b.iter().find(|x| matches!(x, f2_platform::Binding::Key(_)) != pad).or(b.first())?;
                // PlayStation face buttons draw as their icons (✕ ○ □ △).
                use f2_engine::font::{ICON_CIRCLE, ICON_CROSS, ICON_SQUARE, ICON_TRIANGLE};
                use f2_platform::PadButton;
                if self.families[0] == f2_platform::ControllerFamily::PlayStation {
                    if let f2_platform::Binding::Pad(btn) = pick {
                        let icon = match btn {
                            PadButton::South => Some(ICON_CROSS),
                            PadButton::East => Some(ICON_CIRCLE),
                            PadButton::West => Some(ICON_SQUARE),
                            PadButton::North => Some(ICON_TRIANGLE),
                            _ => None,
                        };
                        if let Some(i) = icon {
                            return Some((c, (i as char).to_string()));
                        }
                    }
                }
                Some((c, pick.label(self.families[0])))
            })
            .collect()
    }

    /// `CopyTexture(arcadeScreenTex, FindTexture(arcade_screen_copy))` (menus.c:1219):
    /// the arcade machine's `TEST5` screen shows the selected level / static noise.
    fn sync_arcade_screen(&mut self) {
        let m = &self.game.menus;
        if m.arcade_screen_copy.is_empty() || m.arcade_screen_tex.is_empty() {
            return;
        }
        let src = self.r.textures.current(f2_game::specfx::find_texture(&m.arcade_screen_copy));
        let Some(src) = src else {
            if self.arcade_copied.as_ref().is_none_or(|(n, _)| *n != m.arcade_screen_copy) {
                if std::env::var_os("F2_GAME_LOG").is_some() {
                    eprintln!("frogger: arcade screen texture {} not loaded", m.arcade_screen_copy);
                }
                self.arcade_copied = Some((m.arcade_screen_copy.clone(), TexHandle(u32::MAX)));
            }
            return;
        };
        if self.arcade_copied.as_ref().is_some_and(|(n, h)| *n == m.arcade_screen_copy && *h == src) {
            return;
        }
        let (dest, name) = (m.arcade_screen_tex.clone(), m.arcade_screen_copy.clone());
        if crate::book::copy_texture(&mut self.r, &dest, &name) {
            self.arcade_copied = Some((name, src));
        }
    }

    /// Rebuilds the scene for levels `InitLevel` loaded inside the simulation.
    fn process_level_loads(&mut self) {
        if let Some(l) = self.game.take_level_loads().pop() {
            self.load_scene(l.world, l.level, &l.assets);
        }
    }

    /// Loads the renderer side of a level (object banks, textures, sounds).
    fn load_scene(&mut self, world: u32, level: u32, assets: &f2_game::layout::LevelAssets) {
        let mut banks = Vec::new();
        for f in &assets.object_banks {
            match f2_game::assets::DataSource::object_bank(&self.data, f) {
                Some(Ok(b)) => banks.push(b),
                Some(Err(e)) => eprintln!("frogger: {e}"),
                None => eprintln!("frogger: {f}: not found"),
            }
        }
        for f in &assets.missing_object_banks {
            eprintln!("frogger: object bank {f} missing");
        }
        self.scene = Some(LevelScene::new(world, level, banks, assets.texture_banks.clone(), &self.game));
        self.rebuild_renderer();
        self.load_sfx(world, level);

        self.acc = Duration::ZERO;
        self.prev = render::snapshot(&self.game);
        self.prev_cam = None;
        self.update_game_view();
        self.free_cam = None;
        self.handles.clear();
        self.sfx_loops.clear();
        eprintln!(
            "frogger: level {world}-{level}: {} actors, {} tiles, banks {:?}, textures {:?}",
            self.game.actor2.act_list.ids(&self.game.actor2.actors).len(),
            self.game.map.tiles.len(),
            assets.object_banks,
            assets.texture_banks
        );
    }

    /// Port of the file part of `LoadSfx(worldID)` (pcaudio.c:186): `generic.wus`, the
    /// player voice bank, then the world bank.
    fn load_sfx(&mut self, world: u32, level: u32) {
        self.audio.stop_all();
        self.audio.free_samples();
        let world_id = self.game.layout.world_visual_data[world as usize].world_id as i16;
        let mut files = vec!["generic.wus".to_string()];
        if self.game.game_state.multi != SINGLEPLAYER {
            files.push("multi.wus".into());
        }
        let mut files: Vec<String> = files.into_iter().map(|f| format!("sfx/{f}")).collect();
        // A mod level may name its own world bank (level registry).
        if let Some(b) = self.game.registry_file(world, level, |f| &f.sfx_bank) {
            files.push(b);
        } else if let Some(b) = f2_audio::tables::world_bank(world_id) {
            files.push(format!("sfx/{b}"));
        }
        for path in files {
            match f2_game::assets::DataSource::sound_bank(&self.data, &path) {
                Some(Ok(b)) => self.audio.load_bank(&b),
                Some(Err(e)) => eprintln!("frogger: {e}"),
                None => eprintln!("frogger: {path}: not found"),
            }
        }
        // InitVoices: one voice bank per frog, by `frogPool[player[i].character]` (frogger.c:78;
        // imported file names).
        const VOICE_BANKS: [&str; 8] = ["frogger", "lillie", "tad", "treeby", "wart", "tank", "roobie", "swampy"];
        for i in 0..self.game.frogger.num_frogs.clamp(0, 4) as usize {
            let Some(name) = VOICE_BANKS.get(self.game.frogger.player[i].character as usize) else { continue };
            let path = format!("sfx/{name}.wus");
            match f2_game::assets::DataSource::sound_bank(&self.data, &path) {
                Some(Ok(b)) => self.audio.load_voice_bank(i, &b),
                Some(Err(e)) => eprintln!("frogger: voice bank: {e}"),
                None => eprintln!("frogger: voice bank: {path}: not found"),
            }
        }
    }

    // ------------------------------------------------------------ frame update

    /// Sets the input tables from the config (`Config::bindings` etc.).
    pub fn set_input(&mut self, bindings: Bindings, deadzone: f32, hop_frames: u32) {
        self.bindings = bindings;
        self.deadzone = deadzone;
        self.hop_frames = hop_frames;
    }

    /// Re-applies the keyboard part of `bindings` to the mapper and asks the host to save.
    fn apply_bindings(&mut self) {
        self.mapper.keymap = self.bindings.to_keymap();
        self.mapper.extra_keys = self.bindings.extra_keys();
        self.requests.save_bindings = true;
    }

    /// Opens the settings screen on `tab` (headless screenshots).
    pub fn open_settings(&mut self, tab: Tab) {
        if self.controls.is_none() {
            self.toggle_controls();
        }
        self.tab = tab;
    }

    /// Opens (or closes) the rebinding screen.
    pub fn toggle_controls(&mut self) {
        self.controls = match self.controls {
            Some(_) => None,
            None => Some(ControlsUi::default()),
        };
        if self.controls.is_some() {
            self.level_ui = LevelSelectUi::build(&self.game);
            self.gameplay_ui.msg.clear();
        }
        self.kb.pressed.clear();
        self.kb.down = [false; 256];
        self.acc = Duration::ZERO;
    }

    /// A raw pad event (button / strong stick push) from the host.
    pub fn pad_event(&mut self, ev: RawEvent) {
        if self.settings.is_some() {
            self.settings_raw_pad(ev);
            return;
        }
        if self.video.is_some() && self.controls.is_none() && self.qol() && self.gameplay.skip_fmv_any_key && matches!(ev, RawEvent::PadButton(..)) {
            self.end_video(); // P6-1: skip FMVs with any button
            return;
        }
        if self.tab != Tab::Controls {
            return;
        }
        if let Some(ui) = &mut self.controls {
            if ui.raw(&mut self.bindings, ev) == Act::Changed {
                self.apply_bindings();
            }
        }
    }

    /// A key press routed to the rebinding screen.
    pub fn controls_key(&mut self, k: u8) {
        let Some(ui) = &mut self.controls else { return };
        match ui.key(&mut self.bindings, k) {
            Act::None => {}
            Act::Changed => self.apply_bindings(),
            Act::Close => self.toggle_controls(),
        }
    }

    /// Whether player `i` can start a hop now (the `GameProcessController` condition:
    /// `canJump`, no input pause, not stunned/dead, no pending hop request). Read-only.
    fn can_accept_hop(&self, i: usize) -> bool {
        let g = &self.game;
        if g.game_state.mode != mode::INGAME_MODE || i as i32 >= g.frogger.num_frogs {
            return true;
        }
        let p = &g.frogger.player[i];
        p.can_jump != 0
            && p.input_pause == 0
            && p.stun.time == 0
            && p.dead.time == 0
            && p.frog_state & (f2_game::frogmove::FROGSTATUS_ISDEAD | f2_game::frogmove::FROGSTATUS_ALLHOPFLAGS) == 0
    }

    /// Hotkeys handled by the host (not the game).
    fn hotkeys(&mut self, pressed: &[u8]) {
        for &k in pressed {
            if let Some(ed) = &self.editor {
                // The editor owns the keyboard while editing (Esc quits unless typing).
                let typing = ed.prompt.is_some();
                if !ed.playing && (typing || !matches!(k, dik::ESCAPE | dik::F11)) {
                    continue;
                }
                if ed.playing && k == dik::P {
                    continue;
                }
            }
            if self.settings.is_some() {
                // The settings screen reads the keyboard itself (`settings_input`).
                if k == dik::F11 {
                    self.requests.toggle_fullscreen = true;
                }
                continue;
            }
            if self.controls.is_some() {
                let capturing = self.controls.as_ref().is_some_and(|c| c.rebind.is_some());
                if k == dik::TAB && !capturing {
                    self.tab = self.tab.next();
                } else if self.tab == Tab::Controls {
                    self.controls_key(k);
                } else {
                    self.settings_key(k);
                }
                continue;
            }
            if self.video.is_some() && self.qol() && self.gameplay.skip_fmv_any_key && k != dik::F11 {
                self.end_video(); // P6-1: any key skips an FMV
                continue;
            }
            match k {
                // F10: the settings screen; Shift+F10: the advanced (debug) screen with the
                // level select.
                dik::F10 if self.video.is_none() && (self.kb_shift) => self.toggle_controls(),
                dik::F10 if self.video.is_none() => self.open_settings_page(SettingsPage::Display),
                dik::ESCAPE if self.video.is_some() => self.end_video(),
                dik::RETURN | dik::NUMPADENTER if self.video.is_some() => self.end_video(),
                // Esc is the game's Start/Triangle (pause/back) when the game runs its own
                // flow; it only quits the bare dev scenes (`--level` without a session).
                dik::ESCAPE if !self.has_session() => self.requests.quit = true,
                dik::F1 => {
                    self.free_cam = match self.free_cam {
                        Some(_) => None,
                        None => self.last_cam.as_ref().map(FlyCam::from_camera),
                    }
                }
                dik::F2 => self.show_tiles = !self.show_tiles,
                dik::F11 => self.requests.toggle_fullscreen = true,
                k => self.display_hotkey(k),
            }
        }
    }

    /// One presented frame: input, simulation ticks (fixed 60 Hz with accumulator), video.
    pub fn update(&mut self, dt: Duration, kb: &KeyboardState, joypads: &[Option<RawPad>], focused: bool) {
        // Alt+Enter toggles fullscreen (like F11). While Alt is held, Enter is hidden from
        // the game so the combo never registers as Cross/select.
        let mut filtered;
        let kb = if kb.is_down(dik::LMENU) || kb.is_down(dik::RMENU) {
            filtered = kb.clone();
            if filtered.pressed.iter().any(|&k| k == dik::RETURN || k == dik::NUMPADENTER) {
                self.requests.toggle_fullscreen = true;
            }
            filtered.pressed.retain(|&k| k != dik::RETURN && k != dik::NUMPADENTER);
            filtered.down[dik::RETURN as usize] = false;
            filtered.down[dik::NUMPADENTER as usize] = false;
            &filtered
        } else {
            kb
        };
        self.fps.frame(Instant::now());
        if self.focused != focused {
            self.focused = focused;
            self.apply_master_volume();
        }
        self.kb_shift = kb.is_down(dik::LSHIFT) || kb.is_down(dik::RSHIFT);
        let was_open = self.settings.is_some();
        self.hotkeys(&kb.pressed);
        if self.settings.is_some() {
            // Simulation paused while the settings screen is open.
            if was_open {
                self.settings_input(kb, joypads);
            }
            self.kb.pressed.clear();
            self.acc = Duration::ZERO;
            return;
        }
        if self.controls.is_some() {
            // Simulation paused while the rebinding screen is open.
            self.kb.pressed.clear();
            self.acc = Duration::ZERO;
            return;
        }
        self.kb.down = kb.down;
        self.kb.pressed.extend_from_slice(&kb.pressed);
        self.mapper.window_active = focused;
        if let Some((_, t)) = &mut self.osd {
            *t -= dt.as_secs_f32();
            if *t <= 0.0 {
                self.osd = None;
            }
        }

        if let Some(v) = &mut self.video {
            v.update(dt.as_secs_f64(), &mut self.audio);
            if v.finished {
                self.end_video();
            }
        }
        self.next_step();
        if self.net.is_some() {
            self.net_update(dt, joypads);
            return;
        }
        if self.video.is_some() || (self.scene.is_none() && !self.has_session()) {
            self.kb.pressed.clear();
            return;
        }

        if let Some(fc) = &mut self.free_cam {
            fc.update(kb, dt.as_secs_f32());
        }
        // Dev builds: Ctrl+E opens / closes the editor on the current level.
        let ctrl = kb.is_down(dik::LCONTROL) || kb.is_down(dik::RCONTROL);
        if cfg!(debug_assertions) && ctrl && kb.pressed.contains(&dik::E) && self.editor.as_ref().is_none_or(|e| e.prompt.is_none()) {
            self.toggle_editor();
        }
        if let Some(ed) = &mut self.editor {
            ed.input(kb, dt.as_secs_f32());
            self.process_editor_requests();
        }
        if self.editing() {
            // Simulation paused while editing.
            self.kb.pressed.clear();
            self.acc = Duration::ZERO;
            return;
        }

        self.acc += dt.min(MAX_CATCHUP);
        while self.acc >= TICK {
            // Consume the tick before running it: a tick that loads a level or starts a
            // video resets `acc` to zero, and subtracting afterwards would underflow.
            self.acc = self.acc.saturating_sub(TICK);
            self.tick(joypads);
            if self.video.is_some() {
                // FMVPLAY_MODE: the simulation waits for the video.
                self.acc = Duration::ZERO;
                break;
            }
        }
    }

    /// One 60 Hz simulation step.
    pub fn tick(&mut self, raw: &[Option<RawPad>]) {
        self.prev = render::snapshot(&self.game);
        self.prev_cam = Some(render::game_camera(&self.game));
        let pads = self.read_pads(raw);
        if self.game.frontend.text_entry > 0 {
            // controller: best-time name entry with the pad/arrows (PS1 `PsxNameEntryFrame`).
            self.game.psx_name_entry_frame(self.pad.debounce[0]);
            if let Some(r) = &mut self.record {
                r.name_entry(self.pad.debounce[0]);
            }
        }
        self.apply_game_options(); // P6-1
                                   // `tick_frames(.., 1)` = retail `LoopFunc` timing: in the pause menu `gameSpeed` is 0 and
                                   // `actFrameCount` stays frozen, so path entities (placed from `actFrameCount`) do not run
                                   // on and jump on resume. Trace-identical to `tick` outside pause (research/n64-performance.md §3T).
        self.game.tick_frames(TickInput { pads }, 1);
        self.ticks += 1;
        if self.qol() && self.gameplay.speedrun_timer {
            let p = &self.game.frogger.player[0];
            let (m, w, l) = (self.game.game_state.mode, p.world_num as u32, p.level_num as u32);
            if let Some(s) = self.speedrun.on_tick(m, w, l, std::time::Instant::now()) {
                eprintln!("frogger: split {}-{}: {} (run {})", s.world, s.level, crate::gameplay::fmt_frames(s.igt), crate::gameplay::fmt_frames(s.total_igt));
            }
        }
        self.dispatch_effects();
        self.process_level_loads();
        self.slide_textures();
        self.grab_textures();
        self.update_game_view();
        if self.game.backdrop.tiled_backdrop.is_some() {
            self.tile_scroll.step(self.game.game_speed);
        }
        if let Some(r) = &mut self.record {
            if r.tick(pads[0], &self.game) != crate::record::Outcome::Recording {
                self.requests.quit = true;
            }
        }
    }

    /// `CopyActorList`'s texture sliding (once per game frame, render state).
    fn slide_textures(&mut self) {
        if let Some(scene) = self.scene.as_mut() {
            render::slide_object_textures(scene, &self.game);
        }
    }

    /// This tick's pads from the local devices (keyboard, joypads, hop buffer, pad script).
    /// Online, pad 0 is the local player's (game slot `net.local_slot`).
    fn read_pads(&mut self, raw: &[Option<RawPad>]) -> [u16; 4] {
        let mut kb = std::mem::take(&mut self.kb);
        if self.free_cam.is_some() {
            // The free camera owns the keyboard.
            kb.down = [false; 256];
            kb.pressed.clear();
        }
        let joypads = self.bindings.joypad_states(&self.mapper.controllers, raw, self.deadzone);
        self.mapper.process(&mut self.pad, &kb, &joypads);
        let window = if self.profile == Profile::Enhanced { self.hop_frames } else { 0 };
        let net_slot = self.net.as_ref().filter(|n| n.is_running()).map(|n| n.local_slot as usize);
        for i in 0..4 {
            self.hop[i].window = window;
            let can = self.can_accept_hop(if i == 0 { net_slot.unwrap_or(0) } else { i });
            let p = self.hop[i].apply(self.pad.pad(i), can);
            self.pad.set_pad(i, p);
        }
        self.kb.down = kb.down;
        let mut pads = [0u16; 4];
        pads.copy_from_slice(&self.pad.digital[..4]);
        if let Some(m) = self.pad_script.as_ref().and_then(|s| s.pad_at(self.ticks)) {
            self.pad_override = Some(m);
        }
        if let Some(b) = self.pad_override {
            pads[0] = b;
        }
        // Buttons still held from the settings screen do not reach the game.
        self.pad_hold &= pads[0];
        pads[0] &= !self.pad_hold;
        pads
    }

    // ------------------------------------------------------------ netplay

    /// Starts hosting / joining an online match.
    pub fn start_net(&mut self, req: NetRequest) -> Result<(), String> {
        self.net_t0 = Instant::now();
        match req {
            NetRequest::Host { world, level, players, port, ip } => {
                self.fresh_game();
                self.start_multi(world, level, players as i32)?;
                self.net = Some(crate::netplay::udp_host(self.game.clone(), world, level, players, port, ip)?);
            }
            NetRequest::Join { code } => self.net = Some(crate::netplay::udp_join(&code)?),
        }
        Ok(())
    }

    /// The pristine game every peer starts a match from (`Game::new` + host hooks).
    ///
    /// P6-1 options are part of the checksummed state, so every peer must simulate with
    /// identical `GameOptions`. Online matches always run with `GameOptions::AUTHENTIC`
    /// (whatever the local profile / `[gameplay]`); `net_tick` never re-applies the local
    /// options, and the host leaves them untouched until the match ends. A peer whose start
    /// state differed would fail the start-state fingerprint check.
    fn fresh_game(&mut self) {
        self.game = Game::new();
        self.install_game_hooks();
        self.game.set_game_options(f2_game::enhance::GameOptions::AUTHENTIC);
    }

    /// One presented frame of an online match: network, then fixed 60 Hz ticks.
    fn net_update(&mut self, dt: Duration, joypads: &[Option<RawPad>]) {
        if let Some(t) = self.net_leave_at {
            if Instant::now() >= t {
                self.net_leave_at = None;
                self.net = None;
                self.status.clear();
                self.start_frontend();
            }
            return;
        }
        let now = self.net_t0.elapsed().as_millis() as u64;
        let notes = self.net.as_mut().map(|n| n.poll(now)).unwrap_or_default();
        for n in notes {
            self.net_note(n);
        }
        if !self.net.as_ref().is_some_and(|n| n.is_running()) || self.video.is_some() {
            self.kb.pressed.clear();
            self.acc = Duration::ZERO;
            return;
        }
        self.acc += dt.min(MAX_CATCHUP);
        while self.acc >= TICK {
            self.acc = self.acc.saturating_sub(TICK);
            self.net_tick(joypads);
            if !self.net.as_ref().is_some_and(|n| n.is_running()) {
                break;
            }
        }
    }

    fn net_note(&mut self, n: NetNote) {
        match n {
            NetNote::Started { setup, slot, players } => {
                eprintln!("frogger: online match: level {}-{}, {players} players, we are P{}", setup.world, setup.level, slot + 1);
                if slot != 0 {
                    // Joiner: build the start state the host described.
                    self.fresh_game();
                    let res = self.start_multi(setup.world, setup.level, setup.frogs as i32);
                    let Some(net) = self.net.as_mut() else { return };
                    match res {
                        Ok(()) => {
                            let _ = net.accept_game(self.game.clone());
                        }
                        Err(e) => {
                            let n = net.end(format!("level {}-{}: {e}", setup.world, setup.level));
                            self.net_note(n);
                            return;
                        }
                    }
                }
                self.acc = Duration::ZERO;
            }
            NetNote::Ended { reason } => {
                if reason == "match over" {
                    // The game went back to the frontend by itself: continue offline.
                    self.net = None;
                } else {
                    self.status = format!("online: {reason}");
                    self.net_leave_at = Some(Instant::now() + Duration::from_secs(5));
                }
            }
        }
    }

    /// One online tick: the local pad goes to the session, which simulates (and corrects
    /// mispredicted frames of) the match; effects of the new frame are dispatched.
    fn net_tick(&mut self, raw: &[Option<RawPad>]) {
        let prev = render::snapshot(&self.game);
        let prev_cam = render::game_camera(&self.game);
        let pads = self.read_pads(raw);
        let Some(net) = self.net.as_mut() else { return };
        std::mem::swap(&mut self.game, &mut net.sim.game);
        let (step, note) = net.step(pads[0]);
        std::mem::swap(&mut self.game, &mut net.sim.game);
        if let crate::netplay::Step::Advanced { .. } = step {
            self.prev = prev;
            self.prev_cam = Some(prev_cam);
            self.ticks += 1;
            self.dispatch_effects();
            self.process_level_loads();
            self.slide_textures();
            if self.game.backdrop.tiled_backdrop.is_some() {
                self.tile_scroll.step(self.game.game_speed);
            }
        }
        if let Some(n) = note {
            self.net_note(n);
        }
    }

    // ------------------------------------------------------------ effects

    fn dispatch_effects(&mut self) {
        for e in self.game.take_effects() {
            self.handle_effect(e);
        }
    }

    fn sample_known(&mut self, uid: u32) -> bool {
        if self.audio.find_sample(uid).is_some() {
            return true;
        }
        if self.missing_samples.insert(uid) {
            eprintln!("frogger: sample {uid:#010x} not in loaded banks");
        }
        false
    }

    fn handle_effect(&mut self, e: Effect) {
        let sv = |p: f2_math::SVector| [p.vx, p.vy, p.vz];
        match e {
            Effect::PlaySample { uid, pos, radius, volume, pitch, handle } => {
                if !self.sample_known(uid) {
                    return;
                }
                let v = self.audio.play(uid, Play { volume, pitch: pitch as i16, pos: pos.map(sv), radius, ..Default::default() });
                if let (Some(v), true) = (v, handle != 0) {
                    self.handles.insert(handle, v);
                }
            }
            Effect::StopSample { uid } => {
                self.audio.stop_sample(uid);
            }
            Effect::PlaySfxMapped { actor, uid, anim, pos } => {
                if !self.sample_known(uid) {
                    return;
                }
                let looped = self.audio.find_sample(uid).is_some_and(|s| s.looped);
                if looped {
                    // Once per (actor, anim) like `loopFlags`.
                    if self.sfx_loops.get(&(actor, anim)).is_some_and(|&v| self.audio.is_playing(v)) {
                        return;
                    }
                    if let Some(v) = self.audio.play(uid, Play { pos: Some(sv(pos)), radius: 0, ..Default::default() }) {
                        self.sfx_loops.insert((actor, anim), v);
                    }
                } else {
                    self.audio.play(uid, Play { pos: Some(sv(pos)), radius: 0, ..Default::default() });
                }
            }
            Effect::PlayVoice { player, uid } => {
                self.audio.play_voice(player as usize, uid);
            }
            Effect::StopHandle { handle } => {
                if let Some(v) = self.handles.remove(&handle) {
                    self.audio.stop(v);
                }
            }
            Effect::UpdateHandle { handle, volume, pitch, pos, radius } => {
                if let Some(&v) = self.handles.get(&handle) {
                    self.audio.update_voice(v, volume, pitch as i16, pos.map(sv), radius);
                }
            }
            Effect::PlayMusic { track, looped } => {
                match self.data.music_track(track).filter(|p| !self.data.mods.is_empty() && Some(p) != self.audio.music_path(track).as_ref()) {
                    // A mod's track (topmost VFS layer).
                    Some(p) => self.audio.play_music_stream(
                        f2_audio::wav::WavStream::open_any(&p).map_err(|e| eprintln!("frogger: {}: {e}", p.display())).ok(),
                        track,
                        looped,
                    ),
                    None => self.audio.play_music(track, looped),
                }
            }
            Effect::StopMusic => self.audio.stop_music(),
            Effect::PlayVideo { name } => {
                use f2_game::story::{FMV, FMV_BLITZ_LOGO, FMV_HASBRO_LOGO};
                let logo = [FMV_HASBRO_LOGO, FMV_BLITZ_LOGO].iter().any(|&i| FMV[i as usize].name.eq_ignore_ascii_case(&name));
                if logo && self.qol() && self.gameplay.skip_intro_logos {
                    return; // P6-1: skip the start-up logos
                }
                if let Some(r) = &mut self.record {
                    r.video(&name);
                }
                if self.video.is_some() {
                    self.video_queue.push_back(name);
                } else {
                    self.start_video(&name);
                }
            }
            Effect::SaveGame { data } => {
                let Some(path) = self.save_path.clone() else { return };
                if self.qol() && !self.gameplay.auto_save {
                    // P6-1: auto-save off; "Save now" in the gameplay page writes the slot.
                    eprintln!("frogger: auto-save off, not writing {}", path.display());
                    return;
                }
                if let Err(e) = write_save(&path, &data) {
                    eprintln!("frogger: {e}");
                }
            }
            Effect::Rumble { player, strength, frames } => self.rumble.push((player, strength, frames)),
            Effect::Log { text } => {
                if std::env::var_os("F2_GAME_LOG").is_some() {
                    eprintln!("game: {text}");
                }
            }
            Effect::SetMusicVolume { vol, .. } => {
                self.audio.set_music_volume(vol);
                self.requests.save_volumes = Some((self.audio.music_volume(), self.audio.sfx_volume()));
            }
            Effect::SetSoundVolume { vol } => {
                self.audio.set_sfx_volume(vol);
                self.requests.save_volumes = Some((self.audio.music_volume(), self.audio.sfx_volume()));
            }
            Effect::PauseAudio => self.audio.pause_audio(),
            Effect::UnPauseAudio => self.audio.unpause_audio(),
            Effect::Quit => self.requests.quit = true,
            Effect::OpenSettings => self.open_settings_page(SettingsPage::Display),
        }
    }

    // ------------------------------------------------------------ drawing

    /// Loads (once) the full-screen backdrop the game selected (`mdxLoadBackdrop`).
    fn backdrop_image(&mut self) -> Option<TexHandle> {
        let Some(name) = self.game.backdrop.backdrop.clone() else {
            self.backdrop = None;
            return None;
        };
        if let Some((n, h)) = &self.backdrop {
            if *n == name {
                return *h;
            }
        }
        let path = name.replace('\\', "/");
        let img = self
            .data
            .read(&path)
            .and_then(|d| f2_formats::fla::maybe_decompress(d).map_err(|e| format!("{path}: {e}")))
            .and_then(|d| f2_formats::texture::parse_backdrop(&d).map_err(|e| format!("{path}: {e}")));
        let h = match img {
            Ok(t) => Some(self.r.textures.add_image(t.width as u32, t.height as u32, t.to_rgba8())),
            Err(e) => {
                eprintln!("frogger: backdrop {e}");
                None
            }
        };
        self.backdrop = Some((name, h));
        h
    }

    /// Queues the frame for a `w`×`h` target.
    pub fn draw(&mut self, w: u32, h: u32) {
        self.width = w;
        self.height = h;
        self.r.begin_frame(w, h);
        self.sync_game_fonts();
        if let Some(v) = &mut self.video {
            v.draw(&mut self.r, w as f32, h as f32);
            self.draw_fps();
            return;
        }
        let alpha = (self.acc.as_secs_f32() / TICK.as_secs_f32()).clamp(0.0, 1.0);
        // At most 10 s of catch-up (a level load): the -1-time textures only cycle.
        let tex_anim_steps = self.ticks.saturating_sub(self.tex_anim_ticks).min(600) as u32;
        self.tex_anim_ticks = self.ticks;
        let backdrop = self.backdrop_image();
        if self.scene.is_some() {
            self.sync_arcade_screen();
        }
        crate::overlay::set_button_prompts(self.button_prompts());
        let d = self.display();
        let authentic = self.profile == Profile::Authentic;
        let mut ctx = render::DrawCtx {
            prev: &self.prev,
            alpha,
            cam: render::game_camera(&self.game),
            far_clip: 1200.0,
            show_tiles: self.show_tiles,
            world_only: self.editing() || self.settings.is_some(),
            fonts: self.game_fonts,
            authentic,
            backdrop,
            tile_scroll: self.tile_scroll,
            tex_anim_steps,
        };
        if let Some(scene) = &self.scene {
            let gcam = render::lerp_camera(self.prev_cam.as_ref(), ctx.cam, alpha);
            ctx.cam = match (&self.free_cam, &self.editor) {
                (_, Some(ed)) if !ed.playing => ed.cam.camera(),
                (Some(fc), _) => fc.camera(),
                _ => gcam,
            };
            self.last_cam = Some(ctx.cam);
            ctx.far_clip = render::far_clip(&self.game, scene) * d.draw_distance;
            // Enhanced fog: linear to the level's clear colour, ending at the far clip so
            // geometry fades in instead of popping (retail: fog disabled).
            self.r.options.fog = if d.fog {
                let c = scene.clear;
                f2_engine::Fog { enabled: true, start: ctx.far_clip * 0.5, end: ctx.far_clip, color: [c[0] as f32, c[1] as f32, c[2] as f32] }
            } else {
                f2_engine::Fog::default()
            };
        }
        if self.scene.is_some() || backdrop.is_some() {
            render::draw_loop(&mut self.r, self.scene.as_mut(), &self.game, &ctx);
        }
        if self.scene.is_some() {
            // Listener = camera (`currCamSource` 20.12 game units, `vMatrix`).
            let s = ctx.cam.source;
            self.audio.set_listener(Listener { cam_source: s.map(|x| (x * 10.0 * 4096.0) as i32), view: self.r.view.v_matrix });
        }
        if self.scene.is_some() {
            if let Some(ed) = &mut self.editor {
                ed.draw(&mut self.r, self.font_id, w, h);
            }
        }
        if !self.editing() {
            self.draw_hud();
        }
        if self.qol() && self.gameplay.speedrun_timer && self.controls.is_none() && self.settings.is_none() && !self.editing() {
            if let Some(font) = self.font_id {
                let ow = self.r.output_size().0 as f32;
                let mut y = 72.0;
                for l in self.speedrun.lines(std::time::Instant::now()) {
                    self.r.draw_text(font, ow - 170.0, y, &l, [255, 255, 160, 255], 1.0, None);
                    y += 14.0;
                }
            }
        }
        if let (Some(ui), Some(font)) = (&self.controls, self.font_id) {
            match self.tab {
                Tab::Controls => ui.draw(&mut self.r, font, &self.bindings, self.families[0]),
                tab => {
                    let ow = crate::controls::draw_backdrop(&mut self.r, font, tab.title());
                    if tab == Tab::Gameplay {
                        self.gameplay_ui.draw(&mut self.r, font, self.profile, &self.gameplay, ow);
                    } else {
                        self.level_ui.draw(&mut self.r, font, ow);
                    }
                }
            }
        }
        if self.settings.is_some() {
            self.draw_settings();
        }
        self.draw_fps();
    }

    fn draw_hud(&mut self) {
        let Some(font) = self.font_id else { return };
        let mut lines = Vec::new();
        if let Some(s) = &self.scene {
            if self.free_cam.is_some() || self.show_tiles {
                lines.push(format!(
                    "level {}-{}  frame {}{}",
                    s.world,
                    s.level,
                    self.game.act_frame_count,
                    if self.free_cam.is_some() { "  [free cam: WASD QE arrows]" } else { "" }
                ));
            }
        }
        if !self.status.is_empty() {
            lines.push(self.status.clone());
        }
        if let Some(n) = &self.net {
            lines.extend(n.overlay_lines());
        }
        if let Some((m, _)) = &self.osd {
            lines.push(m.clone());
        }
        if self.hud_probe {
            self.draw_hud_probe(font);
        }
        let mut y = 4.0;
        for l in lines {
            self.r.draw_text(font, 6.0, y, &l, [255, 255, 255, 255], 1.0, None);
            y += 14.0;
        }
    }

    /// Sample overlays in the 4096×4096 space (`--hud-probe`): corner boxes and labels
    /// left/centre/right plus a centred banner, to check widescreen re-anchoring.
    fn draw_hud_probe(&mut self, font: FontId) {
        use f2_engine::Blend;
        let boxes: [(f32, f32, f32, f32, [u8; 4]); 5] = [
            (96.0, 96.0, 900.0, 300.0, [255, 60, 60, 160]),
            (3100.0, 96.0, 900.0, 300.0, [60, 120, 255, 160]),
            (1548.0, 3500.0, 1000.0, 400.0, [255, 220, 60, 160]),
            (96.0, 3600.0, 600.0, 300.0, [60, 255, 60, 160]),
            (3400.0, 3600.0, 600.0, 300.0, [255, 60, 255, 160]),
        ];
        for (x, y, w, h, c) in boxes {
            self.r.draw_sprite_overlay(x, y, w, h, [0.0, 0.0, 1.0, 1.0], None, c, Blend::Semi, 0.0);
        }
        let white = [255, 255, 255, 255];
        self.r.draw_text_overlay(font, 160.0, 200.0, "LIVES 3", white, 1.0);
        self.r.draw_text_overlay(font, 3200.0, 200.0, "SCORE 0", white, 1.0);
        self.r.draw_text_overlay(font, 1800.0, 3650.0, "CENTRE", white, 1.0);
    }

    /// The frame's clear colour. Retail always clears black: `D3DClearView` clears to
    /// `fogR/G/B`, which `SetupFogParams(fog.min,0,0,0,0)` zeroes every frame
    /// (drawloop.cpp:186; the call with the level's fog colour is commented out). Black
    /// shows where no geometry is drawn and around the edges of alpha-blended cut-outs drawn
    /// before what is behind them (vines). The enhanced fog clears to the level's fog
    /// colour instead, so geometry fades into the background.
    pub fn clear_color(&self) -> [f64; 4] {
        match (&self.video, &self.scene) {
            (None, Some(s)) if self.r.options.fog.enabled => s.clear,
            _ => [0.0, 0.0, 0.0, 1.0],
        }
    }

    /// Whether there is anything left to do (videos/levels).
    pub fn idle(&self) -> bool {
        self.video.is_none() && self.scene.is_none() && self.steps.is_empty() && !self.has_session() && self.net.is_none()
    }

    /// Whether the game runs its own flow (title, FMVs, frontend) from a session.
    pub fn has_session(&self) -> bool {
        self.game.session.data.is_some()
    }
}

/// Writes a `savegame.dat` (creating its directory).
fn write_save(path: &std::path::Path, data: &[u8]) -> Result<(), String> {
    let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(path, data)).map_err(|e| format!("saving {}: {e}", path.display()))
}

fn on_off(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}
