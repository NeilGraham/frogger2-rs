//! The viewer state machine: five modes, each queueing a frame into the engine renderer.

use crate::data::{stem, Data, TexSource};
use f2_audio::{AudioEngine, Play};
use f2_engine::anim::for_each_object;
use f2_engine::scene::{Frame, TlVertex, Tri, SZ_SCALE};
use f2_engine::{actor_bounds, Actor, Blend, Camera, Font, FontId, Quad2D, Renderer, TexHandle};
use f2_formats::level::{Collision, Scenic};
use f2_formats::objbank::{Model, ObjectBank};
use f2_formats::wus::SoundBank;
use f2_platform::dik;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

pub type V3 = [f32; 3];

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mul(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn len(a: V3) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}
fn norm(a: V3) -> V3 {
    let l = len(a);
    if l > 1e-6 {
        mul(a, 1.0 / l)
    } else {
        [0.0, 1.0, 0.0]
    }
}

/// Per-frame input, in DirectInput scan codes plus mouse deltas.
#[derive(Clone)]
pub struct Input {
    pub pressed: Vec<u8>,
    pub down: [bool; 256],
    /// Left-button drag delta (pixels).
    pub drag: [f32; 2],
    /// Right-button drag delta (pixels).
    pub rdrag: [f32; 2],
    /// Wheel (lines, positive = away from the user).
    pub wheel: f32,
}

impl Default for Input {
    fn default() -> Self {
        Input { pressed: Vec::new(), down: [false; 256], drag: [0.0; 2], rdrag: [0.0; 2], wheel: 0.0 }
    }
}

impl Input {
    fn hit(&self, k: u8) -> bool {
        self.pressed.contains(&k)
    }
    fn held(&self, k: u8) -> bool {
        self.down[k as usize]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Models,
    Levels,
    Textures,
    Sounds,
    Videos,
}

const MODES: [Mode; 5] = [Mode::Models, Mode::Levels, Mode::Textures, Mode::Sounds, Mode::Videos];

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Models => "Models",
            Mode::Levels => "Levels",
            Mode::Textures => "Textures",
            Mode::Sounds => "Sounds",
            Mode::Videos => "Videos",
        }
    }
    pub fn parse(s: &str) -> Option<Mode> {
        Some(match s.to_ascii_lowercase().as_str() {
            "model" | "models" | "1" => Mode::Models,
            "level" | "levels" | "2" => Mode::Levels,
            "texture" | "textures" | "3" => Mode::Textures,
            "sound" | "sounds" | "4" => Mode::Sounds,
            "video" | "videos" | "5" => Mode::Videos,
            _ => return None,
        })
    }
}

fn step(i: usize, n: usize, d: i32) -> usize {
    if n == 0 {
        return 0;
    }
    (i as i64 + d as i64).clamp(0, n as i64 - 1) as usize
}

fn wrap_step(i: usize, n: usize, d: i32) -> usize {
    if n == 0 {
        return 0;
    }
    (i as i64 + d as i64).rem_euclid(n as i64) as usize
}

/// Up/Down ±1, PageUp/PageDown ±10.
fn list_nav(input: &Input, i: usize, n: usize) -> usize {
    let mut i = i;
    for &k in &input.pressed {
        i = match k {
            dik::UP => step(i, n, -1),
            dik::DOWN => step(i, n, 1),
            dik::PRIOR => step(i, n, -10),
            dik::NEXT => step(i, n, 10),
            dik::HOME => 0,
            dik::END => n.saturating_sub(1),
            _ => i,
        };
    }
    i
}

fn lr(input: &Input) -> i32 {
    input
        .pressed
        .iter()
        .map(|&k| {
            if k == dik::LEFT {
                -1
            } else if k == dik::RIGHT {
                1
            } else {
                0
            }
        })
        .sum()
}

/// Number of faces drawn for a model (the skin once for skinned models).
pub fn triangle_count(model: &Model) -> usize {
    if model.is_skinned {
        return model.root.mesh.as_ref().map_or(0, |m| m.faces.len());
    }
    let mut n = 0;
    for_each_object(model, |_, o| n += o.mesh.as_ref().map_or(0, |m| m.faces.len()));
    n
}

// ------------------------------------------------------------------ cameras

#[derive(Clone, Copy, Debug)]
pub struct Orbit {
    pub target: V3,
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
}

impl Orbit {
    fn camera(&self) -> Camera {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        let off = [cp * sy * self.dist, sp * self.dist, cp * cy * self.dist];
        Camera { source: add(self.target, off), target: self.target, up: [0.0, 1.0, 0.0] }
    }
    fn update(&mut self, input: &Input) {
        self.yaw -= input.drag[0] * 0.01;
        self.pitch = (self.pitch + input.drag[1] * 0.01).clamp(-1.5, 1.5);
        self.dist = (self.dist * 0.9f32.powf(input.wheel)).max(1.0);
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FlyCam {
    pub pos: V3,
    pub yaw: f32,
    pub pitch: f32,
    pub speed: f32,
}

impl FlyCam {
    fn forward(&self) -> V3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        [cp * sy, sp, cp * cy]
    }
    fn camera(&self) -> Camera {
        Camera { source: self.pos, target: add(self.pos, mul(self.forward(), 100.0)), up: [0.0, 1.0, 0.0] }
    }
    fn update(&mut self, input: &Input, dt: f32) {
        self.yaw -= input.drag[0] * 0.005;
        self.pitch = (self.pitch - input.drag[1] * 0.005).clamp(-1.55, 1.55);
        self.speed = (self.speed * 1.2f32.powf(input.wheel)).clamp(1.0, 100_000.0);
        let f = self.forward();
        let right = norm(cross(f, [0.0, 1.0, 0.0]));
        let mut mv = [0.0; 3];
        if input.held(dik::W) {
            mv = add(mv, f);
        }
        if input.held(dik::S) {
            mv = sub(mv, f);
        }
        if input.held(dik::D) {
            mv = add(mv, right);
        }
        if input.held(dik::A) {
            mv = sub(mv, right);
        }
        if input.held(dik::E) {
            mv[1] += 1.0;
        }
        if input.held(dik::Q) {
            mv[1] -= 1.0;
        }
        let boost = if input.held(dik::LSHIFT) || input.held(dik::RSHIFT) { 4.0 } else { 1.0 };
        self.pos = add(self.pos, mul(mv, self.speed * boost * dt));
    }
}

// ------------------------------------------------------------------ mode states

#[derive(Default)]
struct ModelsState {
    banks: Vec<String>,
    bank_i: usize,
    bank: Option<(usize, Arc<ObjectBank>)>,
    tex_files: Vec<String>,
    model_i: usize,
    framed: Option<(usize, usize)>,
    anim_i: usize,
    paused: bool,
    speed: f32,
    anim_time: f32,
    actor: Actor,
    orbit: Option<Orbit>,
    radius: f32,
}

struct PlacedActor {
    bank: usize,
    model: usize,
    actor: Actor,
}

struct LevelScene {
    name: String,
    bank_names: Vec<String>,
    banks: Vec<Arc<ObjectBank>>,
    tex_files: Vec<String>,
    world: Option<(usize, usize)>,
    backdrop: Option<(usize, usize)>,
    scenics: Vec<PlacedActor>,
    scenic_count: usize,
    missing: Vec<String>,
    coll: Option<Collision>,
    tile_size: f32,
    extent: f32,
    lo: V3,
    hi: V3,
}

#[derive(Default)]
struct LevelsState {
    levels: Vec<String>,
    i: usize,
    scene: Option<LevelScene>,
    cam: Option<FlyCam>,
    hide_tiles: bool,
    hide_links: bool,
    hide_scenics: bool,
    hide_world: bool,
    hide_backdrop: bool,
}

#[derive(Default)]
struct TexState {
    files: Vec<String>,
    i: usize,
    src: Option<(usize, Arc<TexSource>)>,
    tex_i: usize,
    zoom: f32,
}

#[derive(Default)]
struct SoundState {
    banks: Vec<String>,
    music: Vec<String>,
    i: usize,
    bank: Option<(usize, SoundBank)>,
    audio_bank: Option<usize>,
    sample_i: usize,
}

/// In-memory WAV of a video's soundtrack and its sample rate.
type AudioResult = Result<(Vec<u8>, u32), String>;

struct VideoPlay {
    name: String,
    bink: f2_bink::BinkFile,
    image: Option<TexHandle>,
    decoded: u32,
    finished: bool,
    paused: bool,
    /// Wall-clock playback position (seconds) when not driven by audio.
    clock: f64,
    wait: f64,
    audio_rx: Option<Receiver<AudioResult>>,
    audio_rate: Option<u32>,
    audio_offset: u64,
    images: u32,
}

#[derive(Default)]
struct VideoState {
    files: Vec<String>,
    i: usize,
    play: Option<VideoPlay>,
}

// ------------------------------------------------------------------ app

pub struct App {
    pub data: Data,
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    pub r: Renderer,
    font: Option<Font>,
    font_id: Option<FontId>,
    scene_key: String,
    pub mode: Mode,
    pub time: f64,
    pub audio: AudioEngine,
    models: ModelsState,
    levels: LevelsState,
    tex: TexState,
    snd: SoundState,
    vid: VideoState,
    status: String,
    show_help: bool,
    pub quit: bool,
    w: f32,
    h: f32,
    /// Right edge for [`App::text`] clipping.
    clip_right: f32,
}

const WHITE: [u8; 4] = [255, 255, 255, 255];
const YELLOW: [u8; 4] = [255, 230, 90, 255];
const GREY: [u8; 4] = [170, 170, 170, 255];
const CYAN: [u8; 4] = [120, 220, 255, 255];
const RED: [u8; 4] = [255, 110, 110, 255];

impl App {
    pub fn new(data: Data, device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, audio: AudioEngine) -> App {
        let font = ["textures/font/smallfont.bmp", "textures/font/monofont.bmp"].iter().find_map(|p| data.read(p).ok().and_then(|b| Font::from_bmp(&b)));
        if font.is_none() {
            eprintln!("f2-viewer: no bitmap font found; on-screen text disabled");
        }
        let mut audio = audio;
        audio.set_music_dir(data.rendition.join("music"));
        let models = ModelsState { banks: data.list("objects", "dll"), speed: 1.0, ..Default::default() };
        let levels = LevelsState { levels: data.list("collision", "col").iter().map(|n| stem(n).to_string()).collect(), ..Default::default() };
        let tex = TexState { files: data.list("textures", "fla"), ..Default::default() };
        let snd = SoundState { banks: data.list("sfx", "wus"), music: data.list("music", "wav"), ..Default::default() };
        let vid = VideoState { files: data.list("video", "bik"), ..Default::default() };
        let r = Renderer::new(device, queue, format);
        let mut app = App {
            data,
            device: device.clone(),
            queue: queue.clone(),
            format,
            r,
            font,
            font_id: None,
            scene_key: String::from("<none>"),
            mode: Mode::Models,
            time: 0.0,
            audio,
            models,
            levels,
            tex,
            snd,
            vid,
            status: String::new(),
            show_help: false,
            quit: false,
            w: 640.0,
            h: 480.0,
            clip_right: f32::INFINITY,
        };
        app.reset_renderer();
        app
    }

    fn reset_renderer(&mut self) {
        self.r = Renderer::new(&self.device, &self.queue, self.format);
        self.font_id = self.font.clone().map(|f| self.r.add_font(f));
    }

    // ------------------------------------------------------------ selection helpers (CLI)

    pub fn select_model(&mut self, bank: &str, model: &str) -> Result<(), String> {
        self.mode = Mode::Models;
        let b = bank.trim_end_matches(".dll");
        self.models.bank_i = self.models.banks.iter().position(|n| stem(n).eq_ignore_ascii_case(b)).ok_or_else(|| format!("object bank {bank} not found"))?;
        self.load_model_bank()?;
        let bk = self.models.bank.as_ref().unwrap().1.clone();
        let m = f2_engine::find_model(&bk, model).ok_or_else(|| format!("model {model} not in {bank}"))?;
        self.models.model_i = bk.models.iter().position(|x| std::ptr::eq(x, m)).unwrap_or(0);
        Ok(())
    }

    pub fn set_anim(&mut self, anim: usize, frame: f32) {
        self.models.anim_i = anim;
        self.models.anim_time = frame;
        self.models.paused = true;
    }

    pub fn select_level(&mut self, name: &str) -> Result<(), String> {
        self.mode = Mode::Levels;
        let n = stem(name);
        self.levels.i =
            self.levels.levels.iter().position(|l| l.eq_ignore_ascii_case(n)).ok_or_else(|| format!("level {name} not found (collision/{n}.col)"))?;
        self.load_level()
    }

    pub fn select_texture(&mut self, bank: &str, tex: &str) -> Result<(), String> {
        self.mode = Mode::Textures;
        let b = bank.trim_end_matches(".fla");
        self.tex.i = self.tex.files.iter().position(|n| stem(n).eq_ignore_ascii_case(b)).ok_or_else(|| format!("texture bank {bank} not found"))?;
        self.load_tex_file()?;
        let src = self.tex.src.as_ref().unwrap().1.clone();
        self.tex.tex_i = tex
            .parse::<usize>()
            .ok()
            .or_else(|| src.textures().iter().position(|t| t.name.eq_ignore_ascii_case(tex) || stem(&t.name).eq_ignore_ascii_case(tex)))
            .ok_or_else(|| format!("texture {tex} not in {bank}"))?;
        Ok(())
    }

    pub fn select_video(&mut self, name: &str, frame: u32) -> Result<(), String> {
        self.mode = Mode::Videos;
        let n = stem(name);
        self.vid.i = self.vid.files.iter().position(|v| stem(v).eq_ignore_ascii_case(n)).ok_or_else(|| format!("video {name} not found"))?;
        self.start_video(false)?;
        let p = self.vid.play.as_mut().unwrap();
        p.paused = true;
        let mut last = None;
        while p.decoded <= frame {
            match p.bink.decode_next() {
                Some(Ok(f)) => {
                    p.decoded += 1;
                    last = Some(f);
                }
                Some(Err(e)) => return Err(e.to_string()),
                None => break,
            }
        }
        let f = last.ok_or("video has no frames")?;
        self.ensure_scene();
        let p = self.vid.play.as_mut().unwrap();
        p.image = Some(self.r.textures.add_image(f.yuv.width, f.yuv.height, f.to_rgba8()));
        Ok(())
    }

    // ------------------------------------------------------------ loading

    fn load_model_bank(&mut self) -> Result<(), String> {
        let i = self.models.bank_i;
        if self.models.bank.as_ref().is_some_and(|(b, _)| *b == i) {
            return Ok(());
        }
        let name = self.models.banks.get(i).cloned().ok_or("no object banks")?;
        let bank = Arc::new(self.data.object_bank(&name)?);
        self.models.tex_files = self.data.tex_banks_for(&[stem(&name)], &[&bank]);
        self.models.bank = Some((i, bank));
        self.models.model_i = 0;
        self.models.framed = None;
        Ok(())
    }

    fn load_tex_file(&mut self) -> Result<(), String> {
        let i = self.tex.i;
        if self.tex.src.as_ref().is_some_and(|(b, _)| *b == i) {
            return Ok(());
        }
        let name = self.tex.files.get(i).cloned().ok_or("no texture files")?;
        self.tex.src = None;
        let src = self.data.tex_source(&name)?;
        self.tex.src = Some((i, src));
        self.tex.tex_i = 0;
        Ok(())
    }

    fn load_level(&mut self) -> Result<(), String> {
        let name = self.levels.levels.get(self.levels.i).cloned().ok_or("no levels")?;
        let coll = match self.data.read(&format!("collision/{name}.col")).and_then(|d| Collision::parse(&d).map_err(|e| e.to_string())) {
            Ok(c) => Some(c),
            Err(e) => {
                self.status = format!("collision: {e}");
                None
            }
        };
        let scenics: Vec<Scenic> = if self.data.exists(&format!("scenics/{name}.sce")) {
            self.data.read(&format!("scenics/{name}.sce")).and_then(|d| f2_formats::level::parse_scenics(&d).map_err(|e| e.to_string())).unwrap_or_else(|e| {
                self.status = format!("scenics: {e}");
                Vec::new()
            })
        } else {
            Vec::new()
        };
        let bank_names = self.data.level_object_banks(&name);
        let mut banks = Vec::new();
        let mut names = Vec::new();
        for b in &bank_names {
            match self.data.object_bank(b) {
                Ok(bk) => {
                    banks.push(Arc::new(bk));
                    names.push(b.clone());
                }
                Err(e) => self.status = e,
            }
        }
        let find = |key: &str| -> Option<(usize, usize)> {
            let crc = f2_formats::crc(&format!("{}.obe", stem(key).to_ascii_lowercase()));
            banks
                .iter()
                .enumerate()
                .find_map(|(bi, b)| b.models.iter().position(|m| m.id == crc || m.name().eq_ignore_ascii_case(stem(key))).map(|mi| (bi, mi)))
        };
        let world = find("world");
        let backdrop = find("backdrop");
        let mut placed = Vec::new();
        let mut missing = Vec::new();
        for s in &scenics {
            let n = s.name.to_ascii_lowercase();
            let base = stem(&n);
            if base == "world" || base == "backdrop" || base.starts_with("null_") || base.starts_with("nothing") || base.starts_with("transobj") {
                continue;
            }
            // `CreateActor`: `.psi` names map to the `.obe` CRC.
            match find(base) {
                Some((bi, mi)) => {
                    let q = [s.rot.x as f32 / 4096.0, s.rot.y as f32 / 4096.0, s.rot.z as f32 / 4096.0, s.rot.w as f32 / 4096.0];
                    let q = if q == [0.0; 4] { [0.0, 0.0, 0.0, 1.0] } else { q };
                    // Raw `.sce`/`.col` values are render units (the game ×10s them into
                    // game units and the PC renderer divides by 10 again).
                    let actor = Actor { pos: [s.pos.x as f32, s.pos.y as f32, s.pos.z as f32], q_rot: q, ..Default::default() };
                    placed.push(PlacedActor { bank: bi, model: mi, actor });
                }
                None => missing.push(n.clone()),
            }
        }
        missing.sort();
        missing.dedup();
        // Bounds: world model, else tiles.
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        if let Some((bi, mi)) = world {
            if let Some((a, b)) = actor_bounds(&banks[bi].models[mi], &mut Actor { animated: false, ..Default::default() }) {
                (lo, hi) = (a, b);
            }
        }
        let mut tile_size = 20.0;
        if let Some(c) = &coll {
            if lo[0] > hi[0] {
                for t in &c.tiles {
                    let p = [t.centre.x as f32, t.centre.y as f32, t.centre.z as f32];
                    for k in 0..3 {
                        lo[k] = lo[k].min(p[k]);
                        hi[k] = hi[k].max(p[k]);
                    }
                }
            }
            let mut sum = 0.0;
            let mut n = 0;
            for t in &c.tiles {
                for &j in &t.adj {
                    if let Some(u) = (j >= 0).then(|| c.tiles.get(j as usize)).flatten() {
                        sum += len(sub(svec(&t.centre), svec(&u.centre)));
                        n += 1;
                    }
                }
            }
            if n > 0 {
                tile_size = sum / n as f32;
            }
        }
        if lo[0] > hi[0] {
            (lo, hi) = ([-500.0; 3], [500.0; 3]);
        }
        let extent = (hi[0] - lo[0]).max(hi[2] - lo[2]).max(100.0);
        let refs: Vec<&ObjectBank> = banks.iter().map(|b| b.as_ref()).collect();
        let name_refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        let tex_files = self.data.tex_banks_for(&name_refs, &refs);
        let c = mul(add(lo, hi), 0.5);
        // Look at the level from above and behind (−z), like the in-game camera.
        let pos = [c[0], hi[1] + extent * 0.45, c[2] - extent * 0.7];
        let to = sub([c[0], lo[1], c[2] - extent * 0.05], pos);
        let yaw = to[0].atan2(to[2]);
        let pitch = (to[1] / len(to)).asin();
        self.levels.cam = Some(FlyCam { pos, yaw, pitch, speed: extent * 0.3 });
        self.levels.scene = Some(LevelScene {
            name,
            bank_names: names,
            banks,
            tex_files,
            world,
            backdrop,
            scenic_count: scenics.len(),
            scenics: placed,
            missing,
            coll,
            tile_size,
            extent,
            lo,
            hi,
        });
        Ok(())
    }

    fn start_video(&mut self, with_audio: bool) -> Result<(), String> {
        self.stop_video();
        let name = self.vid.files.get(self.vid.i).cloned().ok_or("no videos")?;
        let bytes = self.data.read(&format!("video/{name}"))?;
        let bink = f2_bink::BinkFile::open(bytes.clone()).map_err(|e| format!("{name}: {e}"))?;
        let audio_rx = (with_audio && !bink.audio_tracks().is_empty()).then(|| {
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(decode_bink_audio(bytes));
            });
            rx
        });
        self.vid.play = Some(VideoPlay {
            name,
            bink,
            image: None,
            decoded: 0,
            finished: false,
            paused: false,
            clock: 0.0,
            wait: 0.0,
            audio_rx,
            audio_rate: None,
            audio_offset: 0,
            images: 0,
        });
        Ok(())
    }

    fn stop_video(&mut self) {
        if self.vid.play.take().is_some() {
            self.audio.stop_music();
        }
    }

    /// Recreates the renderer when the mode's texture set changes.
    fn ensure_scene(&mut self) {
        let key = match self.mode {
            Mode::Models => format!("m:{:?}", self.models.bank.as_ref().map(|b| b.0)),
            Mode::Levels => format!("l:{:?}", self.levels.scene.as_ref().map(|s| s.name.clone())),
            Mode::Textures => format!("t:{:?}", self.tex.src.as_ref().map(|s| s.0)),
            Mode::Sounds => "s".into(),
            Mode::Videos => format!("v:{:?}", self.vid.play.as_ref().map(|p| p.name.clone())),
        };
        if key == self.scene_key {
            return;
        }
        self.scene_key = key;
        self.reset_renderer();
        let files: Vec<String> = match self.mode {
            Mode::Models => self.models.tex_files.clone(),
            Mode::Levels => self.levels.scene.as_ref().map(|s| s.tex_files.clone()).unwrap_or_default(),
            Mode::Textures => self.tex.src.as_ref().map(|(i, _)| vec![self.tex.files[*i].clone()]).unwrap_or_default(),
            _ => Vec::new(),
        };
        for f in files {
            match self.data.tex_source(&f) {
                Ok(src) => match &*src {
                    TexSource::Bank(b) => self.r.load_tex_bank(b),
                    TexSource::Backdrop(t) => {
                        self.r.add_texture(t);
                    }
                },
                Err(e) => self.status = e,
            }
        }
    }

    // ------------------------------------------------------------ text

    fn line_h(&self) -> f32 {
        self.font.as_ref().map_or(12.0, |f| f.height as f32 + 2.0)
    }

    fn text(&mut self, x: f32, y: f32, s: &str, color: [u8; 4]) -> f32 {
        let Some(f) = self.font_id else { return x };
        // Clip to the window width.
        let max_w = (self.clip_right.min(self.w) - x - 4.0).max(0.0) as i32;
        let font = self.font.as_ref().unwrap();
        let mut s = s.to_string();
        while !s.is_empty() && font.string_width(&s.chars().map(|c| if (c as u32) < 256 { c as u8 } else { 0 }).collect::<Vec<u8>>(), 1.0) > max_w {
            s.pop();
        }
        let s = s.as_str();
        self.r.draw_text(f, x + 1.0, y + 1.0, s, [0, 0, 0, 200], 1.0, None);
        self.r.draw_text(f, x, y, s, color, 1.0, None)
    }

    fn panel(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.r.draw_quad(Quad2D::rect(x, y, w, h, [0.0; 4], None, [0, 0, 0, 150], Blend::Semi));
    }

    /// Draws a scrolling list with the selection highlighted.
    fn list(&mut self, x: f32, y: f32, items: &[String], sel: usize, rows: usize) {
        let lh = self.line_h();
        let rows = rows.max(1);
        let first = sel.saturating_sub(rows / 2).min(items.len().saturating_sub(rows));
        for (k, it) in items.iter().enumerate().skip(first).take(rows) {
            let yy = y + (k - first) as f32 * lh;
            if k == sel {
                self.r.draw_quad(Quad2D::rect(x - 2.0, yy - 1.0, 260.0, lh, [0.0; 4], None, [60, 90, 160, 200], Blend::Semi));
            }
            self.text(x, yy, it, if k == sel { YELLOW } else { WHITE });
        }
    }

    fn list_rows(&self, y: f32) -> usize {
        (((self.h - y - 8.0) / self.line_h()) as usize).max(1)
    }

    // ------------------------------------------------------------ frame

    /// Handles input, advances time by `dt` seconds and queues a `w`×`h` frame.
    pub fn frame(&mut self, input: &Input, dt: f32, w: u32, h: u32) {
        self.w = w as f32;
        self.h = h as f32;
        self.time += dt as f64;
        for &k in &input.pressed {
            let new_mode = match k {
                dik::KEY_1 => Some(Mode::Models),
                dik::KEY_2 => Some(Mode::Levels),
                dik::KEY_3 => Some(Mode::Textures),
                dik::KEY_4 => Some(Mode::Sounds),
                dik::KEY_5 => Some(Mode::Videos),
                dik::TAB => {
                    let i = MODES.iter().position(|&m| m == self.mode).unwrap_or(0);
                    let d = if input.held(dik::LSHIFT) || input.held(dik::RSHIFT) { -1 } else { 1 };
                    Some(MODES[wrap_step(i, MODES.len(), d)])
                }
                dik::F1 => {
                    self.show_help = !self.show_help;
                    None
                }
                dik::ESCAPE => {
                    if self.vid.play.is_some() && self.mode == Mode::Videos {
                        self.stop_video();
                    } else {
                        self.quit = true;
                    }
                    None
                }
                _ => None,
            };
            if let Some(m) = new_mode {
                if m != self.mode && self.mode == Mode::Videos {
                    self.stop_video();
                }
                self.mode = m;
                self.status.clear();
            }
        }
        match self.mode {
            Mode::Models => self.models_update(input, dt),
            Mode::Levels => self.levels_update(input, dt),
            Mode::Textures => self.tex_update(input),
            Mode::Sounds => self.sounds_update(input),
            Mode::Videos => self.videos_update(input, dt),
        }
        self.ensure_scene();
        self.r.begin_frame(w, h);
        // Nearest filtering to inspect texels; the game's bilinear elsewhere.
        self.r.options.linear_filter = self.mode != Mode::Textures;
        self.r.update_animating_textures((self.time * 60.0) as u32);
        match self.mode {
            Mode::Models => self.models_draw(),
            Mode::Levels => self.levels_draw(),
            Mode::Textures => self.tex_draw(),
            Mode::Sounds => self.sounds_draw(),
            Mode::Videos => self.videos_draw(),
        }
        self.draw_header();
    }

    pub fn clear_color(&self) -> [f64; 4] {
        match self.mode {
            Mode::Models => [0.15, 0.2, 0.3, 1.0],
            Mode::Levels => [0.05, 0.05, 0.1, 1.0],
            _ => [0.08, 0.08, 0.1, 1.0],
        }
    }

    fn draw_header(&mut self) {
        let lh = self.line_h();
        let mut x = 6.0;
        self.panel(0.0, 0.0, self.w, lh + 4.0);
        for (i, m) in MODES.iter().enumerate() {
            let c = if *m == self.mode { YELLOW } else { GREY };
            x = self.text(x, 2.0, &format!("{} {}", i + 1, m.name()), c) + 14.0;
        }
        self.text(x + 10.0, 2.0, "F1 help  Esc quit", GREY);
        if !self.status.is_empty() {
            let s = self.status.clone();
            self.panel(0.0, self.h - lh - 4.0, self.w, lh + 4.0);
            self.text(6.0, self.h - lh - 2.0, &s, RED);
        }
        if self.show_help {
            let help: &[&str] = match self.mode {
                Mode::Models => &[
                    "Left/Right: object bank   Up/Down/PgUp/PgDn: model",
                    "[ ]: animation   Space: play/pause   -/=: speed",
                    "Mouse drag: orbit   Wheel: zoom   Home: reset camera",
                ],
                Mode::Levels => &[
                    "Up/Down/PgUp/PgDn: pick level   Enter: load",
                    "WASD + Q/E: fly   Shift: fast   Mouse drag: look   Wheel: speed",
                    "T: tiles  L: links  G: scenics  B: backdrop  V: world geometry",
                ],
                Mode::Textures => &["Left/Right: bank   Up/Down/PgUp/PgDn: texture", "-/=: zoom   0: fit"],
                Mode::Sounds => &["Left/Right: bank (last = music)   Up/Down: sample/track", "Enter: play   Space: stop all"],
                Mode::Videos => &["Up/Down: pick video   Enter: play   Space: pause   Esc: stop"],
            };
            let y0 = self.h * 0.5;
            self.panel(20.0, y0 - 4.0, self.w - 40.0, lh * help.len() as f32 + 8.0);
            for (i, l) in help.iter().enumerate() {
                self.text(30.0, y0 + i as f32 * lh, l, CYAN);
            }
        }
    }

    // ------------------------------------------------------------ models

    fn models_update(&mut self, input: &Input, dt: f32) {
        let d = lr(input);
        if d != 0 {
            self.models.bank_i = wrap_step(self.models.bank_i, self.models.banks.len(), d);
        }
        if let Err(e) = self.load_model_bank() {
            self.status = e;
            return;
        }
        let bank = self.models.bank.as_ref().unwrap().1.clone();
        let n = bank.models.len();
        self.models.model_i = list_nav(input, self.models.model_i.min(n.saturating_sub(1)), n);
        let Some(model) = bank.models.get(self.models.model_i) else { return };
        let key = (self.models.bank_i, self.models.model_i);
        if self.models.framed != Some(key) {
            self.models.framed = Some(key);
            self.models.anim_i = self.models.anim_i.min(model.animations.len().saturating_sub(1));
            if !self.models.paused {
                self.models.anim_time = model.animations.get(self.models.anim_i).map_or(0.0, |a| a.start as f32);
            }
            self.models.actor = Actor::default();
            self.models.orbit = None;
        }
        let na = model.animations.len();
        for &k in &input.pressed {
            match k {
                dik::LBRACKET | dik::RBRACKET if na > 0 => {
                    self.models.anim_i = wrap_step(self.models.anim_i, na, if k == dik::LBRACKET { -1 } else { 1 });
                    self.models.anim_time = model.animations[self.models.anim_i].start as f32;
                }
                dik::SPACE => self.models.paused = !self.models.paused,
                dik::MINUS => self.models.speed = (self.models.speed * 0.5).max(1.0 / 64.0),
                dik::EQUALS => self.models.speed = (self.models.speed * 2.0).min(16.0),
                dik::HOME => self.models.orbit = None,
                _ => {}
            }
        }
        if let Some(a) = model.animations.get(self.models.anim_i) {
            let (s, e) = (a.start as f32, a.end as f32);
            if !self.models.paused {
                self.models.anim_time += dt * 60.0 * a.speed.max(0.01) * self.models.speed;
            }
            if e > s && self.models.anim_time > e {
                self.models.anim_time = s + (self.models.anim_time - s) % (e - s);
            }
            if self.models.anim_time < s {
                self.models.anim_time = s;
            }
        }
        self.models.actor.anim_time = self.models.anim_time;
        if self.models.orbit.is_none() {
            let mut probe = self.models.actor.clone();
            let (lo, hi) = actor_bounds(model, &mut probe).unwrap_or(([-10.0; 3], [10.0; 3]));
            let c = mul(add(lo, hi), 0.5);
            let rad = len(sub(hi, lo)) / 2.0;
            self.models.radius = rad.max(1.0);
            self.models.orbit = Some(Orbit { target: c, yaw: 0.6, pitch: 0.4, dist: (rad * 2.6).max(40.0) });
        }
        if let Some(o) = &mut self.models.orbit {
            o.update(input);
        }
    }

    fn models_draw(&mut self) {
        let lh = self.line_h();
        let y0 = lh + 10.0;
        let Some((_, bank)) = self.models.bank.clone() else {
            self.text(10.0, y0, "no object banks", RED);
            return;
        };
        let mut info = Vec::new();
        if let (Some(model), Some(orbit)) = (bank.models.get(self.models.model_i), self.models.orbit) {
            let cam = orbit.camera();
            self.r.view.far_clip = orbit.dist * 4.0 + self.models.radius * 4.0;
            self.r.view.near_clip = 10.0;
            self.r.set_camera(&cam);
            self.r.draw_actor(model, &mut self.models.actor);
            let a = model.animations.get(self.models.anim_i);
            info.push(format!("{}  (#{}, id {:08x})", model.name(), self.models.model_i, model.id));
            info.push(format!(
                "{} triangles ({} drawn), {}radius {:.1}",
                triangle_count(model),
                self.r.queued_tris(),
                if model.is_skinned { "skinned, " } else { "" },
                model.radius
            ));
            match a {
                Some(a) => info.push(format!(
                    "anim {}/{}  frames {}..{}  t {:.1}  speed {:.2}x{}  {}",
                    self.models.anim_i,
                    model.animations.len(),
                    a.start,
                    a.end,
                    self.models.anim_time,
                    self.models.speed,
                    if a.looped != 0 { " loop" } else { "" },
                    if self.models.paused { "[paused]" } else { "" }
                )),
                None => info.push("no animations".into()),
            }
            info.push(format!("textures: {}", self.models.tex_files.join(", ")));
        }
        let bank_name = self.models.banks[self.models.bank_i].clone();
        let names: Vec<String> = bank.models.iter().enumerate().map(|(i, m)| format!("{i:3} {}", m.name())).collect();
        self.panel(0.0, y0 - 4.0, 270.0, self.h - y0 + 4.0);
        self.text(6.0, y0, &format!("< {bank_name} >  ({} models)", names.len()), CYAN);
        let rows = self.list_rows(y0 + lh);
        self.list(6.0, y0 + lh, &names, self.models.model_i, rows);
        for (i, l) in info.iter().enumerate() {
            self.text(280.0, y0 + i as f32 * lh, l, WHITE);
        }
    }

    // ------------------------------------------------------------ levels

    fn levels_update(&mut self, input: &Input, dt: f32) {
        let n = self.levels.levels.len();
        self.levels.i = list_nav(input, self.levels.i, n);
        let want_load = input.hit(dik::RETURN) || input.hit(dik::NUMPADENTER) || (self.levels.scene.is_none() && n > 0 && self.status.is_empty());
        if want_load {
            if let Err(e) = self.load_level() {
                self.status = e;
            }
        }
        for &k in &input.pressed {
            match k {
                dik::T => self.levels.hide_tiles = !self.levels.hide_tiles,
                dik::L => self.levels.hide_links = !self.levels.hide_links,
                dik::G => self.levels.hide_scenics = !self.levels.hide_scenics,
                dik::B => self.levels.hide_backdrop = !self.levels.hide_backdrop,
                dik::V => self.levels.hide_world = !self.levels.hide_world,
                _ => {}
            }
        }
        if let Some(c) = &mut self.levels.cam {
            c.update(input, dt);
        }
        let t = self.time as f32 * 60.0;
        if let Some(s) = &mut self.levels.scene {
            for p in &mut s.scenics {
                let m = &s.banks[p.bank].models[p.model];
                if let Some(a) = m.animations.first() {
                    let (st, en) = (a.start as f32, a.end as f32);
                    p.actor.anim_time = if en > st { st + (t * a.speed.max(0.01)) % (en - st) } else { st };
                }
            }
        }
    }

    fn levels_draw(&mut self) {
        let lh = self.line_h();
        let y0 = lh + 10.0;
        let mut info: Vec<(String, [u8; 4])> = Vec::new();
        if let (Some(scene), Some(cam)) = (self.levels.scene.take(), self.levels.cam) {
            let camera = cam.camera();
            self.r.set_camera(&camera);
            self.r.view.near_clip = 10.0;
            self.r.view.far_clip = scene.extent * 3.0;
            if let (Some((bi, mi)), false) = (scene.backdrop, self.levels.hide_backdrop) {
                // `DrawBackground`: the sky follows the camera, far plane ignored, size 5.
                self.r.view.no_clip = true;
                let mut a = Actor { pos: camera.source, scale: [5.0; 3], ..Default::default() };
                self.r.draw_actor(&scene.banks[bi].models[mi], &mut a);
                self.r.view.no_clip = false;
            }
            if let (Some((bi, mi)), false) = (scene.world, self.levels.hide_world) {
                self.r.draw_landscape(&scene.banks[bi].models[mi]);
            }
            let mut scene = scene;
            if !self.levels.hide_scenics {
                for p in &mut scene.scenics {
                    let m = &scene.banks[p.bank].models[p.model];
                    self.r.draw_actor(m, &mut p.actor);
                }
            }
            let geo = self.r.queued_tris();
            if let Some(coll) = &scene.coll {
                self.draw_tiles(coll, scene.tile_size);
            }
            info.push((format!("{}  banks: {}", scene.name, scene.bank_names.join(", ")), WHITE));
            info.push((format!("textures: {}", scene.tex_files.join(", ")), WHITE));
            info.push((
                format!(
                    "world: {}  backdrop: {}  scenics: {}/{} placed  tiles: {}  tris: {}",
                    if scene.world.is_some() { "yes" } else { "MISSING" },
                    if scene.backdrop.is_some() { "yes" } else { "no" },
                    scene.scenics.len(),
                    scene.scenic_count,
                    scene.coll.as_ref().map_or(0, |c| c.tiles.len()),
                    geo
                ),
                WHITE,
            ));
            if !scene.missing.is_empty() {
                let mut s = format!("unresolved scenics: {}", scene.missing.join(" "));
                s.truncate(110);
                info.push((s, RED));
            }
            info.push((
                format!("cam {:.0},{:.0},{:.0}  speed {:.0}  bounds {:.0?}..{:.0?}", cam.pos[0], cam.pos[1], cam.pos[2], cam.speed, scene.lo, scene.hi),
                GREY,
            ));
            self.levels.scene = Some(scene);
        }
        // Legend.
        let legend: Vec<(String, [u8; 4])> =
            [0u32, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 16, 17, 18, 0x20].iter().map(|&s| (tile_state_name(s).to_string(), tile_color(s))).collect();
        let names = self.levels.levels.clone();
        self.panel(0.0, y0 - 4.0, 150.0, self.h - y0 + 4.0);
        self.text(6.0, y0, "levels (Enter)", CYAN);
        let rows = self.list_rows(y0 + lh);
        self.list(6.0, y0 + lh, &names, self.levels.i, rows);
        let lx = self.w - 135.0;
        self.clip_right = if self.levels.hide_tiles { f32::INFINITY } else { lx };
        for (i, (l, c)) in info.iter().enumerate() {
            self.text(160.0, y0 + i as f32 * lh, l, *c);
        }
        self.clip_right = f32::INFINITY;
        if !self.levels.hide_tiles {
            for (i, (n, c)) in legend.iter().enumerate() {
                let y = y0 + i as f32 * lh;
                self.r.draw_quad(Quad2D::rect(lx, y + 2.0, lh - 4.0, lh - 4.0, [0.0; 4], None, *c, Blend::Semi));
                self.text(lx + lh, y, n, WHITE);
            }
        }
    }

    fn push_world_tri(&mut self, p: [V3; 3], color: [u8; 4]) {
        let mut v = [TlVertex::default(); 3];
        for k in 0..3 {
            let Some(s) = self.r.view.xfm_point(p[k]) else { return };
            v[k] = tl(s[0], s[1], s[2], color);
        }
        self.r.push_tri(Frame::Xlu, Tri { v, tex: None });
    }

    fn draw_tiles(&mut self, coll: &Collision, tile_size: f32) {
        let show_tiles = !self.levels.hide_tiles;
        let show_links = !self.levels.hide_links;
        if !show_tiles && !show_links {
            return;
        }
        let lift = tile_size * 0.08;
        let half = tile_size * 0.3;
        let proj: Vec<Option<[f32; 3]>> = coll
            .tiles
            .iter()
            .map(|t| {
                let n = norm(svec(&t.normal));
                self.r.view.xfm_point(add(svec(&t.centre), mul(n, lift)))
            })
            .collect();
        for t in &coll.tiles {
            let c = svec(&t.centre);
            let n = norm(svec(&t.normal));
            let n = if n[1] < 0.0 { mul(n, -1.0) } else { n };
            if show_tiles {
                let d = svec(&t.dir_vector[0]);
                let a = norm(sub(d, mul(n, d[0] * n[0] + d[1] * n[1] + d[2] * n[2])));
                let a = if len(svec(&t.dir_vector[0])) < 1e-3 { norm(cross(n, [1.0, 0.0, 0.0])) } else { a };
                let b = cross(n, a);
                let o = add(c, mul(n, lift));
                let q = [
                    add(o, add(mul(a, half), mul(b, half))),
                    add(o, sub(mul(a, half), mul(b, half))),
                    sub(o, add(mul(a, half), mul(b, half))),
                    add(o, sub(mul(b, half), mul(a, half))),
                ];
                let mut col = tile_color(t.state);
                col[3] = 170;
                self.push_world_tri([q[0], q[1], q[2]], col);
                self.push_world_tri([q[2], q[3], q[0]], col);
            }
        }
        if show_links {
            for (i, t) in coll.tiles.iter().enumerate() {
                let Some(p0) = proj[i] else { continue };
                for (dir, &j) in t.adj.iter().enumerate() {
                    if j < 0 || j as usize >= proj.len() {
                        continue;
                    }
                    let back = coll.tiles[j as usize].adj.contains(&(i as i32));
                    if back && (j as usize) < i {
                        continue; // two-way link drawn once
                    }
                    let Some(p1) = proj[j as usize] else { continue };
                    // N/S/E/W tint; one-way links in red.
                    let col =
                        if !back { [255, 60, 60, 220] } else { [[255, 255, 255, 170], [255, 255, 255, 170], [200, 200, 255, 170], [200, 200, 255, 170]][dir] };
                    self.push_line(p0, p1, col);
                }
            }
        }
    }

    fn push_line(&mut self, a: [f32; 3], b: [f32; 3], col: [u8; 4]) {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = (dx * dx + dy * dy).sqrt();
        if l < 0.5 || l > 4000.0 {
            return;
        }
        let (nx, ny) = (-dy / l * 0.8, dx / l * 0.8);
        let v0 = tl(a[0] + nx, a[1] + ny, a[2], col);
        let v1 = tl(a[0] - nx, a[1] - ny, a[2], col);
        let v2 = tl(b[0] - nx, b[1] - ny, b[2], col);
        let v3 = tl(b[0] + nx, b[1] + ny, b[2], col);
        self.r.push_tri(Frame::Xlu, Tri { v: [v0, v1, v2], tex: None });
        self.r.push_tri(Frame::Xlu, Tri { v: [v2, v3, v0], tex: None });
    }

    // ------------------------------------------------------------ textures

    fn tex_update(&mut self, input: &Input) {
        let d = lr(input);
        if d != 0 {
            self.tex.i = wrap_step(self.tex.i, self.tex.files.len(), d);
        }
        if let Err(e) = self.load_tex_file() {
            self.status = e;
            return;
        }
        let n = self.tex.src.as_ref().map_or(0, |s| s.1.textures().len());
        self.tex.tex_i = list_nav(input, self.tex.tex_i.min(n.saturating_sub(1)), n);
        for &k in &input.pressed {
            match k {
                dik::EQUALS => self.tex.zoom = if self.tex.zoom <= 0.0 { 2.0 } else { (self.tex.zoom * 2.0).min(32.0) },
                dik::MINUS => self.tex.zoom = if self.tex.zoom <= 0.0 { 0.5 } else { (self.tex.zoom * 0.5).max(0.125) },
                dik::KEY_0 => self.tex.zoom = 0.0,
                _ => {}
            }
        }
    }

    fn tex_draw(&mut self) {
        let lh = self.line_h();
        let y0 = lh + 10.0;
        let Some((fi, src)) = self.tex.src.clone() else { return };
        let texes = src.textures();
        let names: Vec<String> = texes.iter().enumerate().map(|(i, t)| format!("{i:3} {}", t.name)).collect();
        self.panel(0.0, y0 - 4.0, 270.0, self.h - y0 + 4.0);
        self.text(6.0, y0, &format!("< {} >  ({} textures)", self.tex.files[fi], texes.len()), CYAN);
        let rows = self.list_rows(y0 + lh);
        self.list(6.0, y0 + lh, &names, self.tex.tex_i, rows);
        let Some(t) = texes.get(self.tex.tex_i) else { return };
        let entry = self.r.textures.entry(t.crc).cloned();
        let handle = entry.as_ref().and_then(|e| e.current.or(Some(e.image)));
        let (tw, th) = (t.width as f32, t.height as f32);
        let (ax, ay, aw, ah) = (290.0, y0 + lh * 3.0, self.w - 300.0, self.h - y0 - lh * 3.0 - 10.0);
        let fit = (aw / tw).min(ah / th);
        let zoom = if self.tex.zoom > 0.0 {
            self.tex.zoom
        } else if fit >= 1.0 {
            fit.floor().min(16.0)
        } else {
            fit
        };
        let (dw, dh) = (tw * zoom, th * zoom);
        // Checkerboard behind (alpha visible).
        let cell = 8.0f32.max(zoom * 2.0).min(32.0);
        let (nx, ny) = ((dw / cell).ceil() as i32, (dh / cell).ceil() as i32);
        for j in 0..ny {
            for i in 0..nx {
                let c = if (i + j) % 2 == 0 { [90, 90, 90, 255] } else { [150, 150, 150, 255] };
                let (x, y) = (ax + i as f32 * cell, ay + j as f32 * cell);
                self.r.draw_quad(Quad2D::rect(x, y, cell.min(ax + dw - x), cell.min(ay + dh - y), [0.0; 4], None, c, Blend::Semi));
            }
        }
        if let Some(hd) = handle {
            self.r.draw_quad(Quad2D::rect(ax, ay, dw, dh, [0.0, 0.0, 1.0, 1.0], Some(hd), WHITE, Blend::Semi));
        }
        let anim = entry.as_ref().map_or(0, |e| e.frames.len());
        let anim_s = if anim > 1 {
            format!("  animated: {anim} frames (times {:?})", entry.as_ref().unwrap().frames.iter().map(|f| f.1).collect::<Vec<_>>())
        } else {
            String::new()
        };
        let opaque = t.pixels.iter().filter(|&&p| p != f2_formats::texture::TEX_TRANSVAL).count() * 100 / t.pixels.len().max(1);
        self.text(
            ax,
            y0,
            &format!("{}  {}x{}  {:?}  flags {:08x}  crc {:08x}  {opaque}% opaque", t.name, t.width, t.height, t.tex_type(), t.flags, t.crc),
            WHITE,
        );
        let mut s = format!("zoom {:.2}{}{anim_s}", zoom, if self.tex.zoom <= 0.0 { " (fit)" } else { "" });
        s.truncate(120);
        self.text(ax, y0 + lh, &s, GREY);
    }

    // ------------------------------------------------------------ sounds

    fn sound_bank_count(&self) -> usize {
        self.snd.banks.len() + 1
    }

    fn is_music(&self) -> bool {
        self.snd.i >= self.snd.banks.len()
    }

    fn sounds_update(&mut self, input: &Input) {
        let d = lr(input);
        if d != 0 {
            self.snd.i = wrap_step(self.snd.i, self.sound_bank_count(), d);
            self.snd.sample_i = 0;
        }
        if !self.is_music() && self.snd.bank.as_ref().is_none_or(|b| b.0 != self.snd.i) {
            let name = self.snd.banks[self.snd.i].clone();
            match self.data.read(&format!("sfx/{name}")).and_then(|d| SoundBank::parse(&d).map_err(|e| e.to_string())) {
                Ok(b) => self.snd.bank = Some((self.snd.i, b)),
                Err(e) => {
                    self.status = e;
                    self.snd.bank = None;
                }
            }
        }
        let n = if self.is_music() { self.snd.music.len() } else { self.snd.bank.as_ref().map_or(0, |b| b.1.samples.len()) };
        self.snd.sample_i = list_nav(input, self.snd.sample_i, n);
        if input.hit(dik::SPACE) {
            self.audio.stop_all();
            self.audio.stop_music();
        }
        if (input.hit(dik::RETURN) || input.hit(dik::NUMPADENTER)) && n > 0 {
            if self.is_music() {
                let name = &self.snd.music[self.snd.sample_i];
                let track: u32 = name.trim_start_matches(|c: char| !c.is_ascii_digit()).trim_end_matches(|c: char| !c.is_ascii_digit()).parse().unwrap_or(0);
                self.audio.play_music(track, false);
            } else if let Some((bi, bank)) = &self.snd.bank {
                if self.snd.audio_bank != Some(*bi) {
                    self.audio.free_samples();
                    self.audio.load_bank(bank);
                    self.snd.audio_bank = Some(*bi);
                }
                let uid = bank.samples[self.snd.sample_i].uid;
                if self.audio.play(uid, Play { looped: Some(false), ..Default::default() }).is_none() {
                    self.status = format!("could not play {uid:08x}");
                }
            }
        }
    }

    fn sounds_draw(&mut self) {
        let lh = self.line_h();
        let y0 = lh + 10.0;
        let (title, items, detail): (String, Vec<String>, String) = if self.is_music() {
            ("music tracks".into(), self.snd.music.clone(), format!("music: {:?}  playing: {}", self.audio.music_track(), self.audio.is_song_playing()))
        } else if let Some((bi, b)) = &self.snd.bank {
            let items = b
                .samples
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let frames = s.pcm.len() as f32 / (s.channels.max(1) as f32 * (s.bits.max(8) as f32 / 8.0));
                    format!(
                        "{i:3} {:08x} {:5}Hz {}ch {:2}b {:5.2}s{}",
                        s.uid,
                        s.sample_rate,
                        s.channels,
                        s.bits,
                        frames / s.sample_rate.max(1) as f32,
                        if s.looped { " loop" } else { "" }
                    )
                })
                .collect();
            (self.snd.banks[*bi].clone(), items, String::new())
        } else {
            ("(no bank)".into(), Vec::new(), String::new())
        };
        self.panel(0.0, y0 - 4.0, 420.0, self.h - y0 + 4.0);
        self.text(6.0, y0, &format!("< {title} >  ({} entries)   Enter: play  Space: stop", items.len()), CYAN);
        let rows = self.list_rows(y0 + lh);
        let saved = items;
        self.list(6.0, y0 + lh, &saved, self.snd.sample_i, rows);
        let out = if self.audio.is_realtime() { "audio device" } else { "muted (no audio device)" };
        let s = format!("{out}  voices: {}  {detail}", self.audio.active_voices());
        self.text(430.0, y0, &s, WHITE);
    }

    // ------------------------------------------------------------ videos

    fn videos_update(&mut self, input: &Input, dt: f32) {
        if self.vid.play.is_none() {
            self.vid.i = list_nav(input, self.vid.i, self.vid.files.len());
        }
        if input.hit(dik::RETURN) || input.hit(dik::NUMPADENTER) {
            if let Err(e) = self.start_video(true) {
                self.status = e;
            }
        }
        if input.hit(dik::SPACE) {
            if let Some(p) = &mut self.vid.play {
                p.paused = !p.paused;
                if p.audio_rate.is_some() {
                    if p.paused {
                        self.audio.pause_audio();
                    } else {
                        self.audio.unpause_audio();
                    }
                }
            }
        }
        let Some(mut p) = self.vid.play.take() else { return };
        // Audio: start the stream once the background decode is done.
        if let Some(rx) = &p.audio_rx {
            match rx.try_recv() {
                Ok(Ok((wav, rate))) => {
                    p.audio_rx = None;
                    match f2_audio::wav::WavStream::from_reader(Box::new(std::io::Cursor::new(wav))) {
                        Ok(s) => {
                            eprintln!("f2-viewer: {} audio ready ({} Hz) after {:.2}s", p.name, rate, p.wait);
                            self.audio.play_music_stream(Some(s), 99, false);
                            p.audio_rate = Some(rate);
                            // Video time already elapsed (if any) is the audio offset.
                            p.audio_offset = (p.clock * rate as f64) as u64;
                        }
                        Err(e) => self.status = format!("video audio: {e}"),
                    }
                }
                Ok(Err(e)) => {
                    p.audio_rx = None;
                    self.status = format!("video audio: {e}");
                }
                Err(mpsc::TryRecvError::Empty) => {
                    // Hold the first frame for up to 3 s while the audio decodes.
                    p.wait += dt as f64;
                    if p.wait < 3.0 {
                        if p.image.is_none() {
                            self.decode_video_frames(&mut p, 1);
                        }
                        self.vid.play = Some(p);
                        return;
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => p.audio_rx = None,
            }
        }
        if !p.paused && !p.finished {
            p.clock += dt as f64;
        }
        let (num, den) = p.bink.fps();
        let fps = if den > 0 { num as f64 / den as f64 } else { 25.0 };
        let fps = if fps > 0.0 { fps } else { 25.0 };
        // Audio clock when a real device is playing the track.
        let t = match (p.audio_rate, self.audio.is_realtime(), self.audio.music_position()) {
            (Some(rate), true, Some(pos)) => (pos + p.audio_offset) as f64 / rate as f64,
            _ => p.clock,
        };
        let target = (t * fps) as u32 + 1;
        if p.decoded < target {
            let n = (target - p.decoded).min(8);
            self.decode_video_frames(&mut p, n);
        }
        self.vid.play = Some(p);
    }

    fn decode_video_frames(&mut self, p: &mut VideoPlay, n: u32) {
        let mut last = None;
        for _ in 0..n {
            match p.bink.decode_next() {
                Some(Ok(f)) => {
                    p.decoded += 1;
                    last = Some(f);
                }
                Some(Err(e)) => {
                    self.status = format!("{}: {e}", p.name);
                    p.finished = true;
                    break;
                }
                None => {
                    eprintln!("f2-viewer: {} finished after {} frames", p.name, p.decoded);
                    p.finished = true;
                    break;
                }
            }
        }
        if let Some(f) = last {
            // `p` is detached from `self.vid` here; make sure the renderer is the video's.
            let key = format!("v:{:?}", Some(p.name.clone()));
            if self.scene_key != key {
                self.scene_key = key;
                self.reset_renderer();
            }
            // The registry cannot replace images, so start a fresh renderer every
            // 25 frames to bound GPU memory.
            p.images += 1;
            if p.images > 25 {
                p.images = 0;
                self.reset_renderer();
            }
            p.image = Some(self.r.textures.add_image(f.yuv.width, f.yuv.height, f.to_rgba8()));
        }
    }

    fn videos_draw(&mut self) {
        let lh = self.line_h();
        let y0 = lh + 10.0;
        if let Some(p) = &self.vid.play {
            let (vw, vh) = (p.bink.width() as f32, p.bink.height() as f32);
            let s = (self.w / vw).min((self.h - y0 - lh) / vh);
            let (dw, dh) = (vw * s, vh * s);
            if let Some(img) = p.image {
                self.r.draw_quad(Quad2D::rect(
                    (self.w - dw) / 2.0,
                    y0 + (self.h - y0 - lh - dh) / 2.0,
                    dw,
                    dh,
                    [0.0, 0.0, 1.0, 1.0],
                    Some(img),
                    WHITE,
                    Blend::Semi,
                ));
            }
            let (num, den) = p.bink.fps();
            let state = if p.audio_rx.is_some() {
                "decoding audio..."
            } else if p.finished {
                "finished"
            } else if p.paused {
                "paused"
            } else {
                "playing"
            };
            let s = format!(
                "{}  {}x{}  frame {}/{}  {}/{} fps  audio: {}  {state}",
                p.name,
                p.bink.width(),
                p.bink.height(),
                p.decoded,
                p.bink.frame_count(),
                num,
                den,
                match p.audio_rate {
                    Some(r) if self.audio.is_realtime() => format!("{r} Hz"),
                    Some(_) => "muted".into(),
                    None => "none".into(),
                }
            );
            self.text(6.0, self.h - lh - 2.0, &s, WHITE);
            return;
        }
        let items = self.vid.files.clone();
        self.panel(0.0, y0 - 4.0, 270.0, self.h - y0 + 4.0);
        self.text(6.0, y0, "videos   Enter: play", CYAN);
        let rows = self.list_rows(y0 + lh);
        self.list(6.0, y0 + lh, &items, self.vid.i, rows);
    }
}

fn svec(v: &f2_formats::level::SVec) -> V3 {
    [v.x as f32, v.y as f32, v.z as f32]
}

fn tl(sx: f32, sy: f32, vz: f32, c: [u8; 4]) -> TlVertex {
    TlVertex {
        sx,
        sy,
        // Slight bias towards the camera so overlays sit on top of coplanar geometry.
        sz: ((vz - 2.0) * SZ_SCALE).clamp(0.0, 1.0),
        rhw: 1.0,
        color: c.map(|x| x as f32 / 255.0),
        uv: [0.0; 2],
        fog: 1.0,
        alpha_ref: 0.0,
    }
}

/// `TILESTATE_*` names (`Common/define.h:86`).
pub fn tile_state_name(s: u32) -> &'static str {
    const N: [&str; 23] = [
        "normal",
        "deadly",
        "sink",
        "ice",
        "superhop",
        "join",
        "safe",
        "finish",
        "grapple",
        "smash",
        "barred",
        "fall",
        "frog1area",
        "frog2area",
        "frog3area",
        "frog4area",
        "deadlyfall",
        "hot",
        "electric",
        "nosuper",
        "pants",
        "nosuperhot",
        "occupied",
    ];
    if (0x20..0x30).contains(&s) {
        return "conveyor";
    }
    N.get(s as usize).copied().unwrap_or("?")
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

/// Decodes a whole Bink file's first audio track to an in-memory WAV.
fn decode_bink_audio(bytes: Vec<u8>) -> AudioResult {
    let mut b = f2_bink::BinkFile::open(bytes).map_err(|e| e.to_string())?;
    let info = *b.audio_tracks().first().ok_or("no audio track")?;
    let mut pcm: Vec<i16> = Vec::new();
    while let Some(f) = b.decode_next() {
        let f = f.map_err(|e| e.to_string())?;
        if let Some(a) = f.audio.first() {
            pcm.extend(a.iter().map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16));
        }
    }
    Ok((f2_audio::wav::encode_wav(info.channels as u16, info.sample_rate, &pcm), info.sample_rate))
}
