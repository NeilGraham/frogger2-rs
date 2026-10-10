//! The game host: fixed 60 Hz simulation, effect dispatch (audio, video, saves) and
//! interpolated rendering (`docs/architecture.md` §3).

use crate::controls::{Act, ControlsUi};
use crate::data::Data;
use crate::overlay::{GameFonts, TileScroll};
use crate::render::{self, FlyCam, LevelScene, Snap};
use crate::video::VideoPlayer;
use f2_audio::{AudioEngine, Listener, Play, VoiceId};
use f2_engine::{Camera, Font, FontId, Renderer, TexHandle};
use f2_formats::objbank::ObjectBank;
use f2_game::define::{mode, MULTILOCAL, SINGLEPLAYER};
use f2_game::ids::ActorId;
use f2_game::{Effect, Game, TickInput};
use f2_platform::{dik, Bindings, ControllerFamily, DisplaySettings, HopBuffer, InputMapper, KeyboardState, PadData, Profile, RawEvent, RawPad, TextureFilter};
use std::collections::{HashMap, VecDeque};
use std::time::Duration;

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
}

pub struct App {
    pub data: Data,
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    pub r: Renderer,
    font: Option<Font>,
    font_id: Option<FontId>,
    /// `bigfont.bmp` / `smallfont.bmp` (`pcFont` / `pcFontSmall`).
    game_font_files: Option<(Font, Font)>,
    game_fonts: Option<GameFonts>,
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
    pub hud_probe: bool,
    /// Scripted pad 0 buttons (`--pad-script`): replaces player 1's input when set.
    pub pad_override: Option<u16>,
    /// `savegame.dat` read at session start and written by `SaveGame` (None: no saves,
    /// e.g. headless runs).
    pub save_path: Option<std::path::PathBuf>,
    /// Save block used instead of `save_path`'s content (`--dev-unlock`).
    pub save_data: Option<Vec<u8>>,
    /// `--pad-script`: sets `pad_override` at the scripted ticks.
    pub pad_script: Option<crate::padscript::PadScript>,
    /// The texture last copied onto the arcade screen (`menus.arcade_screen_copy`) and
    /// the source image it came from.
    arcade_copied: Option<(String, TexHandle)>,
    /// The level editor (`--editor`), when active.
    pub editor: Option<crate::editor::Editor>,
    /// Mod folder / id the editor saves to (`--mods-dir`, `--mod`).
    pub editor_mods: Option<(std::path::PathBuf, String)>,
    /// Commands run once the editor has loaded (`--editor-cmd`).
    pub editor_cmds: Vec<String>,
}

impl App {
    pub fn new(data: Data, device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, mut audio: AudioEngine, mapper: InputMapper) -> App {
        let font = ["textures/font/smallfont.bmp", "textures/font/monofont.bmp"].iter().find_map(|p| data.read(p).ok().and_then(|b| Font::from_bmp(&b)));
        let load_font = |p: &str| data.read(p).ok().and_then(|b| Font::from_bmp(&b));
        let game_font_files = match (load_font("textures/font/bigfont.bmp"), load_font("textures/font/smallfont.bmp")) {
            (Some(b), Some(s)) => Some((b, s)),
            _ => {
                eprintln!("frogger: game fonts (bigfont.bmp/smallfont.bmp) missing");
                None
            }
        };
        audio.set_music_dir(data.rendition.join("music"));
        let mut app = App {
            data,
            device: device.clone(),
            queue: queue.clone(),
            format,
            r: Renderer::new(device, queue, format),
            font,
            font_id: None,
            game_font_files,
            game_fonts: None,
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
            hud_probe: false,
            pad_override: None,
            save_path: None,
            save_data: None,
            pad_script: None,
            arcade_copied: None,
            editor: None,
            editor_mods: None,
            editor_cmds: Vec::new(),
        };
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

    /// Queues the startup / dev sequence.
    pub fn push_steps(&mut self, steps: impl IntoIterator<Item = Step>) {
        self.steps.extend(steps);
    }

    /// Recreates the renderer (the texture registry cannot free images) and reloads the
    /// level's texture banks.
    fn rebuild_renderer(&mut self) {
        self.r = Renderer::new(&self.device, &self.queue, self.format);
        self.apply_display();
        self.font_id = self.font.clone().map(|f| self.r.add_font(f));
        self.game_fonts = self.game_font_files.clone().map(|(b, s)| GameFonts { normal: self.r.add_font(b), small: self.r.add_font(s) });
        self.backdrop = None;
        self.arcade_copied = None;
        if let Some(v) = &mut self.video {
            v.forget_image();
        }
        let files = self.scene.as_ref().map(|s| s.tex_files.clone()).unwrap_or_default();
        for f in files {
            match self.data.read(&f).and_then(|d| f2_formats::fla::maybe_decompress(d).map_err(|e| format!("{f}: {e}"))) {
                Ok(d) => match f2_formats::texture::TexBank::parse(&d) {
                    Ok(b) => self.r.load_tex_bank(&b),
                    Err(e) => eprintln!("frogger: {f}: {e}"),
                },
                Err(e) => eprintln!("frogger: {e}"),
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
                    let data: std::sync::Arc<dyn f2_game::assets::DataSource + Send + Sync> = std::sync::Arc::new(ed.doc.overlay(std::sync::Arc::new(self.data.clone())));
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
        let editor = std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")).unwrap_or_else(|_| if cfg!(windows) { "notepad".into() } else { "vi".into() });
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
        let g = &mut self.game;
        g.session.data = Some(std::sync::Arc::new(self.data.clone()));
        g.game_state.mode = mode::INGAME_MODE;
        g.game_state.multi = MULTILOCAL;
        g.frogger.num_frogs = n.clamp(2, 4);
        g.multi.multiplayer_mode = f2_game::multi::MULTI_GAME_TYPES.get(world as usize).copied().filter(|&m| m != 0).unwrap_or(f2_game::multi::MULTIMODE_RACE);
        for i in 0..4 {
            g.frogger.player[i].character = i as _;
        }
        let assets = g.load_level(world, level, &self.data)?;
        self.dispatch_effects();
        self.load_scene(world, level, &assets);
        Ok(())
    }

    /// The session set-up: game files and `SaveData\savegame.dat` (`LoadGame`).
    fn session_config(&self) -> f2_game::session::SessionConfig {
        let mut cfg = f2_game::session::SessionConfig::new(std::sync::Arc::new(self.data.clone()));
        cfg.save_data = self.save_data.clone().or_else(|| self.save_path.as_ref().and_then(|p| std::fs::read(p).ok()));
        cfg
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
            let font = self.game_font_files.as_ref().map(|(b, _)| b);
            crate::book::process_grab(&mut self.r, &mut self.game, font);
        }
    }

    /// Names of player 1's face buttons for the `@X`/`@T`/`@S`/`@C` text codes: the first
    /// key binding on the keyboard, the first pad binding on a joypad.
    fn button_prompts(&self) -> Vec<(char, String)> {
        use f2_game::levplay::{PAD_CIRCLE, PAD_CROSS, PAD_SQUARE, PAD_TRIANGLE};
        let pad = !matches!(self.mapper.controllers[0], f2_platform::Controller::Keyboard);
        [('X', PAD_CROSS), ('T', PAD_TRIANGLE), ('S', PAD_SQUARE), ('C', PAD_CIRCLE)]
            .into_iter()
            .filter_map(|(c, a)| {
                let b = self.bindings.get(0, a);
                let pick = b.iter().find(|x| matches!(x, f2_platform::Binding::Key(_)) != pad).or(b.first())?;
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
            match self.data.read(f).and_then(|d| ObjectBank::parse(&d).map_err(|e| format!("{f}: {e}"))) {
                Ok(b) => banks.push(b),
                Err(e) => eprintln!("frogger: {e}"),
            }
        }
        for f in &assets.missing_object_banks {
            eprintln!("frogger: object bank {f} missing");
        }
        self.scene = Some(LevelScene::new(world, level, banks, assets.texture_banks.clone(), &self.game));
        self.rebuild_renderer();
        self.load_sfx(world);

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
    fn load_sfx(&mut self, world: u32) {
        self.audio.stop_all();
        self.audio.free_samples();
        let world_id = self.game.layout.world_visual_data[world as usize].world_id as i16;
        let mut files = vec!["generic.wus".to_string()];
        if self.game.game_state.multi != SINGLEPLAYER {
            files.push("multi.wus".into());
        }
        if let Some(b) = f2_audio::tables::world_bank(world_id) {
            files.push(b.into());
        }
        for f in files {
            let path = format!("sfx/{f}");
            match self.data.read(&path).and_then(|d| f2_formats::wus::SoundBank::parse(&d).map_err(|e| format!("{path}: {e}"))) {
                Ok(b) => self.audio.load_bank(&b),
                Err(e) => eprintln!("frogger: {e}"),
            }
        }
        // InitVoices: one voice bank per frog, by `frogPool[player[i].character]` (frogger.c:78;
        // imported file names).
        const VOICE_BANKS: [&str; 8] = ["frogger", "lillie", "tad", "treeby", "wart", "tank", "roobie", "swampy"];
        for i in 0..self.game.frogger.num_frogs.clamp(0, 4) as usize {
            let Some(name) = VOICE_BANKS.get(self.game.frogger.player[i].character as usize) else { continue };
            let path = format!("sfx/{name}.wus");
            match self.data.read(&path).and_then(|d| f2_formats::wus::SoundBank::parse(&d).map_err(|e| format!("{path}: {e}"))) {
                Ok(b) => self.audio.load_voice_bank(i, &b),
                Err(e) => eprintln!("frogger: voice bank: {e}"),
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

    /// Opens (or closes) the rebinding screen.
    pub fn toggle_controls(&mut self) {
        self.controls = match self.controls {
            Some(_) => None,
            None => Some(ControlsUi::default()),
        };
        self.kb.pressed.clear();
        self.kb.down = [false; 256];
        self.acc = Duration::ZERO;
    }

    /// A raw pad event (button / strong stick push) from the host.
    pub fn pad_event(&mut self, ev: RawEvent) {
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
            if self.controls.is_some() {
                self.controls_key(k);
                continue;
            }
            match k {
                dik::F10 if self.video.is_none() => self.toggle_controls(),
                dik::ESCAPE if self.video.is_some() => self.end_video(),
                dik::RETURN | dik::NUMPADENTER if self.video.is_some() => self.end_video(),
                dik::ESCAPE => self.requests.quit = true,
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
        self.hotkeys(&kb.pressed);
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
        if self.video.is_some() || (self.scene.is_none() && !self.has_session()) {
            self.kb.pressed.clear();
            return;
        }

        if let Some(fc) = &mut self.free_cam {
            fc.update(kb, dt.as_secs_f32());
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
            self.tick(joypads);
            self.acc -= TICK;
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
        let mut kb = std::mem::take(&mut self.kb);
        if self.free_cam.is_some() {
            // The free camera owns the keyboard.
            kb.down = [false; 256];
            kb.pressed.clear();
        }
        let joypads = self.bindings.joypad_states(&self.mapper.controllers, raw, self.deadzone);
        self.mapper.process(&mut self.pad, &kb, &joypads);
        let window = if self.profile == Profile::Enhanced { self.hop_frames } else { 0 };
        for i in 0..4 {
            self.hop[i].window = window;
            let can = self.can_accept_hop(i);
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
        self.game.tick(TickInput { pads });
        self.ticks += 1;
        self.dispatch_effects();
        self.process_level_loads();
        self.grab_textures();
        self.update_game_view();
        if self.game.backdrop.tiled_backdrop.is_some() {
            self.tile_scroll.step(self.game.game_speed);
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
            Effect::PlayMusic { track, looped } => self.audio.play_music(track, looped),
            Effect::StopMusic => self.audio.stop_music(),
            Effect::PlayVideo { name } => {
                if self.video.is_some() {
                    self.video_queue.push_back(name);
                } else {
                    self.start_video(&name);
                }
            }
            Effect::SaveGame { data } => {
                let Some(path) = self.save_path.clone() else { return };
                let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
                if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, &data)) {
                    eprintln!("frogger: saving {}: {e}", path.display());
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
        if let Some(v) = &mut self.video {
            v.draw(&mut self.r, w as f32, h as f32);
            return;
        }
        let alpha = (self.acc.as_secs_f32() / TICK.as_secs_f32()).clamp(0.0, 1.0);
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
            world_only: self.editing(),
            fonts: self.game_fonts,
            authentic,
            backdrop,
            tile_scroll: self.tile_scroll,
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
        if let (Some(ui), Some(font)) = (&self.controls, self.font_id) {
            ui.draw(&mut self.r, font, &self.bindings, self.families[0]);
        }
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

    pub fn clear_color(&self) -> [f64; 4] {
        match (&self.video, &self.scene) {
            (None, Some(s)) => s.clear,
            _ => [0.0, 0.0, 0.0, 1.0],
        }
    }

    /// Whether there is anything left to do (videos/levels).
    pub fn idle(&self) -> bool {
        self.video.is_none() && self.scene.is_none() && self.steps.is_empty() && !self.has_session()
    }

    /// Whether the game runs its own flow (title, FMVs, frontend) from a session.
    pub fn has_session(&self) -> bool {
        self.game.session.data.is_some()
    }
}

fn on_off(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}
