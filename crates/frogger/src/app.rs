//! The game host: fixed 60 Hz simulation, effect dispatch (audio, video, saves) and
//! interpolated rendering (`docs/architecture.md` §3).

use crate::data::Data;
use crate::render::{self, FlyCam, LevelScene, Snap};
use crate::video::VideoPlayer;
use f2_audio::{AudioEngine, Listener, Play, VoiceId};
use f2_engine::{Camera, Font, FontId, Renderer};
use f2_formats::objbank::ObjectBank;
use f2_game::define::{mode, SINGLEPLAYER};
use f2_game::ids::ActorId;
use f2_game::{Effect, Game, TickInput};
use f2_platform::input::JoypadState;
use f2_platform::{dik, DisplaySettings, InputMapper, KeyboardState, PadData, Profile, TextureFilter};
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
    Level(u32, u32),
}

/// Host-only toggles requested by hotkeys, applied by the window layer.
#[derive(Default)]
pub struct Requests {
    pub quit: bool,
    pub toggle_fullscreen: bool,
}

pub struct App {
    pub data: Data,
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    pub r: Renderer,
    font: Option<Font>,
    font_id: Option<FontId>,
    pub audio: AudioEngine,
    pub game: Game,
    pub scene: Option<LevelScene>,
    steps: VecDeque<Step>,
    pub video: Option<VideoPlayer>,
    // input
    pub mapper: InputMapper,
    pad: PadData,
    /// Keyboard state accumulated since the last tick (key-down events must not be lost on
    /// frames without a tick).
    kb: KeyboardState,
    // timing
    acc: Duration,
    prev: Vec<Option<Snap>>,
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
}

impl App {
    pub fn new(data: Data, device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, mut audio: AudioEngine, mapper: InputMapper) -> App {
        let font = ["textures/font/smallfont.bmp", "textures/font/monofont.bmp"].iter().find_map(|p| data.read(p).ok().and_then(|b| Font::from_bmp(&b)));
        audio.set_music_dir(data.rendition.join("music"));
        let mut app = App {
            data,
            device: device.clone(),
            queue: queue.clone(),
            format,
            r: Renderer::new(device, queue, format),
            font,
            font_id: None,
            audio,
            game: Game::new(),
            scene: None,
            steps: VecDeque::new(),
            video: None,
            mapper,
            pad: PadData::default(),
            kb: KeyboardState::default(),
            acc: Duration::ZERO,
            prev: Vec::new(),
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
        };
        app.rebuild_renderer();
        app
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
            }
        }
    }

    fn start_video(&mut self, name: &str) {
        match VideoPlayer::open(&self.data, name, self.audio.is_realtime()) {
            Ok(v) => {
                self.audio.stop_all();
                self.video = Some(v);
            }
            Err(e) => eprintln!("frogger: video {name}: {e}"),
        }
    }

    fn end_video(&mut self) {
        if let Some(mut v) = self.video.take() {
            v.release(&mut self.r);
            self.audio.stop_music();
        }
    }

    /// Starts `INGAME_MODE` on `worldVisualData[world].levelVisualData[level]`, single player
    /// (what the frontend sets up before `InitLevel`).
    pub fn start_level(&mut self, world: u32, level: u32) -> Result<(), String> {
        let g = &mut self.game;
        g.game_state.mode = mode::INGAME_MODE;
        g.game_state.multi = SINGLEPLAYER;
        g.frogger.num_frogs = 1;
        let assets = g.load_level(world, level, &self.data)?;
        self.dispatch_effects();

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
        Ok(())
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

    /// Hotkeys handled by the host (not the game).
    fn hotkeys(&mut self, pressed: &[u8]) {
        for &k in pressed {
            match k {
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
    pub fn update(&mut self, dt: Duration, kb: &KeyboardState, joypads: &[Option<JoypadState>], focused: bool) {
        self.hotkeys(&kb.pressed);
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
        if self.video.is_some() || self.scene.is_none() {
            self.kb.pressed.clear();
            return;
        }

        if let Some(fc) = &mut self.free_cam {
            fc.update(kb, dt.as_secs_f32());
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
    pub fn tick(&mut self, joypads: &[Option<JoypadState>]) {
        self.prev = render::snapshot(&self.game);
        let mut kb = std::mem::take(&mut self.kb);
        if self.free_cam.is_some() {
            // The free camera owns the keyboard.
            kb.down = [false; 256];
            kb.pressed.clear();
        }
        self.mapper.process(&mut self.pad, &kb, joypads);
        self.kb.down = kb.down;
        let mut pads = [0u16; 4];
        pads.copy_from_slice(&self.pad.digital[..4]);
        self.game.tick(TickInput { pads });
        self.ticks += 1;
        self.dispatch_effects();
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
            Effect::PlayVideo { name } => self.start_video(&name),
            Effect::SaveGame { data } => {
                let dir = f2_platform::paths::save_dir();
                let path = dir.join("savegame.dat");
                if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, &data)) {
                    eprintln!("frogger: saving {}: {e}", path.display());
                }
            }
            Effect::Rumble { .. } => {} // TODO: gilrs force feedback.
            Effect::Log { text } => {
                if std::env::var_os("F2_GAME_LOG").is_some() {
                    eprintln!("game: {text}");
                }
            }
            // wave G: TODO(host) music/sfx volume, audio pause, quit.
            Effect::SetMusicVolume { .. } | Effect::SetSoundVolume { .. } | Effect::PauseAudio | Effect::UnPauseAudio | Effect::Quit => {}
        }
    }

    // ------------------------------------------------------------ drawing

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
        if let Some(scene) = &mut self.scene {
            let frog = render::frog_actor(&self.game).map(|id| {
                let cur = &self.game.actor2.actors[id].actor.position;
                let c = render::sv_to_render(cur);
                match self.prev.get(id.idx()).and_then(|p| p.as_ref()) {
                    Some(p) => f2_engine::math::linear_interp(&p.pos, &c, alpha),
                    None => c,
                }
            });
            // TODO(port): use the game's camera (cam.c) once ported.
            let cam = match &self.free_cam {
                Some(fc) => fc.camera(),
                None => render::follow_camera(&self.game, scene, frog),
            };
            self.last_cam = Some(cam);
            let d = match self.profile {
                Profile::Authentic => DisplaySettings::AUTHENTIC,
                Profile::Enhanced => self.enhanced,
            };
            let far = scene.far_clip * d.draw_distance;
            // Enhanced fog: linear to the level's clear colour, ending at the far clip so
            // geometry fades in instead of popping (retail: fog disabled).
            self.r.options.fog = if d.fog {
                let c = scene.clear;
                f2_engine::Fog { enabled: true, start: far * 0.5, end: far, color: [c[0] as f32, c[1] as f32, c[2] as f32] }
            } else {
                f2_engine::Fog::default()
            };
            render::draw_level(&mut self.r, scene, &self.game, &self.prev, alpha, &cam, far, self.show_tiles);
            // Listener = camera (`currCamSource` 20.12 game units, `vMatrix`).
            let s = cam.source;
            self.audio.set_listener(Listener { cam_source: s.map(|x| (x * 10.0 * 4096.0) as i32), view: self.r.view.v_matrix });
        }
        self.draw_hud();
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
        self.video.is_none() && self.scene.is_none() && self.steps.is_empty()
    }
}

fn on_off(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}
