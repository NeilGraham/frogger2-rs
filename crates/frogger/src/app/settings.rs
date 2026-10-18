//! The in-game Settings screen (host-side): Display, Audio, Controls (with the keyboard
//! mapping page) and Gameplay. Opened from the game's pause menus / Extras list
//! ([`f2_game::Effect::OpenSettings`], `GameOptions::settings_menu`) or with F10. Drawn with
//! the game's own fonts (`bigfont` titles and items, `smallfont` values and descriptions),
//! colours (green items, the pause menu's pulsing highlight) and sounds (hop on move,
//! super hop on select), centred in the 4:3 area like the original menus.
//!
//! Every change applies at once and is saved to `config.toml` (`App::cfg`, the host saves
//! and applies window / swap-chain changes). Items the authentic profile makes inert are
//! greyed out with a hint.
//!
//! Mapping pages: [`MapPage`] is generic over [`Device`]: Controls -> Keyboard mapping
//! (`Device::Keyboard`, key captures) and Controls -> Controller mapping (`Device::Gamepad`,
//! pad button / stick captures from [`App::settings_raw_pad`], labels through the player's
//! `ControllerFamily`). Both use f2-platform's `Bindings` / `Rebind` API.

use super::App;
use crate::overlay::expand_prompts;
use f2_engine::{Blend, Font, FontId, Quad2D, Renderer};
use f2_game::levplay::{
    PAD_CIRCLE, PAD_CROSS, PAD_DOWN, PAD_L1, PAD_L2, PAD_LEFT, PAD_R1, PAD_R2, PAD_RIGHT, PAD_SELECT, PAD_SQUARE, PAD_START, PAD_TRIANGLE, PAD_UP,
};
use f2_platform::bindings::{ConflictPolicy, RebindMode, ACTIONS, DEFAULT_DEADZONE};
use f2_platform::config::VideoMode;
use f2_platform::config::ENHANCED_HOP_BUFFER_FRAMES;
use f2_platform::controller::{FaceLayout, StickMode};
use f2_platform::{dik, Binding, ControllerFamily, FpsCorner, KeyboardState, PadData, Profile, RawEvent, Rebind, RebindStatus, TextureFilter, WindowMode};
use std::time::Instant;

// ---------------------------------------------------------------- window info

/// What the host knows about the window (shown and edited on the Display page).
#[derive(Clone, Debug, Default)]
pub struct WinInfo {
    /// Actual window mode.
    pub mode: WindowMode,
    /// Current surface (backbuffer) size.
    pub size: (u32, u32),
    /// Video modes of the window's monitor (empty headless / unknown).
    pub modes: Vec<VideoMode>,
    /// The monitor's desktop mode.
    pub desktop: Option<VideoMode>,
    /// A console host (PS4 / PS5): always fullscreen. `modes` are the output modes the host
    /// can switch to (Resolution / Refresh rate pick among them; `requests.apply_window`
    /// asks the host to switch), `desktop` the mode in use. The window mode, VSync and frame
    /// cap rows are hidden.
    pub console: bool,
    /// The host has a single, fullscreen window mode (Android): `mode` is borderless and the
    /// Window mode and Refresh rate rows are hidden; Resolution picks the render size,
    /// scaled to the screen with black bars.
    pub fixed_mode: bool,
}

/// Common resolutions offered next to the monitor's video modes.
const COMMON: [(u32, u32); 10] =
    [(640, 480), (800, 600), (1024, 768), (1280, 720), (1280, 960), (1366, 768), (1600, 900), (1920, 1080), (2560, 1440), (3840, 2160)];

impl WinInfo {
    /// Explicit resolutions for `mode` (smallest first): the monitor's video modes (exclusive
    /// fullscreen: only those), plus 640x480 and common sizes that fit the desktop.
    pub fn resolutions(&self, mode: WindowMode) -> Vec<(u32, u32)> {
        let mut v: Vec<(u32, u32)> = self.modes.iter().map(|m| (m.width, m.height)).collect();
        if !self.console && (mode != WindowMode::Fullscreen || v.is_empty()) {
            let max = self.desktop.map(|d| (d.width, d.height)).unwrap_or((u32::MAX, u32::MAX));
            v.extend(COMMON.iter().filter(|(w, h)| *w <= max.0 && *h <= max.1));
        }
        v.sort_by_key(|&(w, h)| (w * h, w));
        v.dedup();
        v
    }

    /// Refresh rates (mHz, fastest first) of `res` (`None` = desktop size).
    pub fn rates(&self, res: Option<[u32; 2]>) -> Vec<u32> {
        let Some([w, h]) = res.or(self.desktop.map(|d| [d.width, d.height])) else { return Vec::new() };
        let mut v: Vec<u32> = self.modes.iter().filter(|m| m.width == w && m.height == h).map(|m| m.refresh_mhz).collect();
        v.sort_unstable_by(|a, b| b.cmp(a));
        v.dedup();
        v
    }
}

// ---------------------------------------------------------------- FPS counter

/// Rendered frames per second (and frame time) over half-second windows.
#[derive(Debug)]
pub struct FpsMeter {
    t0: Option<Instant>,
    frames: u32,
    pub fps: f32,
    pub ms: f32,
}

impl Default for FpsMeter {
    fn default() -> Self {
        // Until measured: the simulation rate.
        FpsMeter { t0: None, frames: 0, fps: 60.0, ms: 1000.0 / 60.0 }
    }
}

impl FpsMeter {
    pub fn frame(&mut self, now: Instant) {
        let t0 = *self.t0.get_or_insert(now);
        self.frames += 1;
        let dt = (now - t0).as_secs_f32();
        if dt >= 0.5 {
            self.fps = self.frames as f32 / dt;
            self.ms = dt * 1000.0 / self.frames as f32;
            self.frames = 0;
            self.t0 = Some(now);
        }
    }
}

// ---------------------------------------------------------------- model

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Display,
    Audio,
    Controls,
    Gameplay,
    /// "Export to…" other platforms (desktop builds; docs/EXPORT.md).
    Export,
}

/// Number of settings pages.
const PAGES: usize = 5;

impl Page {
    pub const ALL: [Page; PAGES] = [Page::Display, Page::Audio, Page::Controls, Page::Gameplay, Page::Export];

    pub fn title(self) -> &'static str {
        match self {
            Page::Display => "Display",
            Page::Audio => "Audio",
            Page::Controls => "Controls",
            Page::Gameplay => "Gameplay",
            Page::Export => "Export",
        }
    }

    pub fn parse(s: &str) -> Option<Page> {
        Page::ALL.into_iter().find(|p| p.title().eq_ignore_ascii_case(s))
    }
}

/// One setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Id {
    WindowMode,
    Resolution,
    Refresh,
    VSync,
    FrameCap,
    Profile,
    Widescreen,
    RenderScale,
    Msaa,
    Culling,
    Upscaler,
    UpscaleQuality,
    DlssPreset,
    Sharpness,
    FrameGen,
    FgTarget,
    Reflex,
    Filter,
    Perspective,
    DrawDistance,
    Fog,
    ModernFont,
    Brightness,
    ShowFps,
    ResetDisplay,
    Master,
    Music,
    Sfx,
    MuteUnfocused,
    ResetAudio,
    KeyboardMap,
    ControllerMap,
    Deadzone,
    StickMode,
    FaceLayout,
    Rumble,
    RumbleStrength,
    HopBuffer,
    ResetControls,
    InfiniteLives,
    Invulnerable,
    EnemySpeed,
    SkipFmv,
    SkipLogos,
    SpeedrunTimer,
    SaveSlot,
    SaveNow,
    AutoSave,
    TubeFix,
    Language,
    ResetGameplay,
    // mapping page
    MapPlayer,
    MapAction(u16),
    MapResetAll,
    // export page
    /// Export for `Target::exportable()[n]`.
    Export(u8),
    OpenExports,
}

/// A row as drawn.
#[derive(Clone, Debug)]
pub struct Item {
    pub id: Id,
    pub label: String,
    pub value: String,
    pub desc: String,
    pub enabled: bool,
    /// Left/Right change the value (arrows drawn).
    pub arrows: bool,
}

/// Input device a mapping page edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    Keyboard,
    /// Pad bindings (buttons and stick directions) of the player's controller.
    Gamepad,
}

impl Device {
    pub fn accepts(self, b: &Binding) -> bool {
        matches!(b, Binding::Key(_)) == (self == Device::Keyboard)
    }

    fn title(self) -> &'static str {
        match self {
            Device::Keyboard => "Keyboard",
            Device::Gamepad => "Controller",
        }
    }
}

/// A capture that hit inputs already in use: what to do about them.
#[derive(Clone, Debug)]
pub struct Conflict {
    pub rb: Rebind,
    pub binding: Binding,
    /// Index into [`Conflict::choices`].
    pub choice: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolve {
    /// The other action gets this action's old input.
    Swap,
    /// The other action loses the input.
    Unbind,
    Cancel,
}

impl Conflict {
    pub fn choices(&self) -> &'static [Resolve] {
        if self.rb.mode == RebindMode::Replace {
            &[Resolve::Swap, Resolve::Unbind, Resolve::Cancel]
        } else {
            &[Resolve::Unbind, Resolve::Cancel]
        }
    }
}

/// The button-mapping page (keyboard now; controller later, same widgets).
#[derive(Clone, Debug)]
pub struct MapPage {
    pub device: Device,
    pub player: u8,
    pub sel: usize,
    pub scroll: usize,
    pub capture: Option<Rebind>,
    pub conflict: Option<Conflict>,
    pub msg: String,
}

impl MapPage {
    pub fn new(device: Device) -> MapPage {
        MapPage { device, player: 0, sel: 1, scroll: 0, capture: None, conflict: None, msg: String::new() }
    }
}

/// Navigation events of one frame.
pub mod nav {
    pub const UP: u16 = 1;
    pub const DOWN: u16 = 2;
    pub const LEFT: u16 = 4;
    pub const RIGHT: u16 = 8;
    pub const OK: u16 = 16;
    pub const BACK: u16 = 32;
    pub const CLEAR: u16 = 64;
    pub const RESET: u16 = 128;
    pub const PREV: u16 = 256;
    pub const NEXT: u16 = 512;
    pub const ADD: u16 = 1024;
    pub const CLOSE: u16 = 2048;
}

/// Screen state.
#[derive(Debug)]
pub struct SettingsUi {
    pub page: usize,
    /// Selected row per page (`None` = the page tabs).
    pub row: [Option<usize>; PAGES],
    pub scroll: [usize; PAGES],
    pub map: Option<MapPage>,
    /// Pad bits held when the screen opened / last frame (debounce).
    prev: u16,
    /// The key / button press that opened the screen (Cross on "Settings" in Extras or
    /// the pause menu) is still in this frame's input: it must not act on the first row.
    fresh: bool,
    /// Held direction and when it repeats next.
    repeat: Option<(u16, Instant)>,
    t0: Instant,
    pub msg: String,
}

/// Rows shown at once.
const VISIBLE: usize = 8;
const ROW_Y0: f32 = 960.0;
const ROW_H: f32 = 255.0;

impl SettingsUi {
    pub fn new(page: Page, held: u16) -> SettingsUi {
        let p = Page::ALL.iter().position(|&x| x == page).unwrap_or(0);
        SettingsUi {
            page: p,
            row: [Some(0); PAGES],
            scroll: [0; PAGES],
            map: None,
            prev: held,
            fresh: true,
            repeat: None,
            t0: Instant::now(),
            msg: String::new(),
        }
    }

    pub fn page(&self) -> Page {
        Page::ALL[self.page]
    }

    /// `pauseFrameCount`-like 60 Hz counter for the highlight pulse.
    fn pulse(&self) -> i32 {
        (self.t0.elapsed().as_millis() as u64 * 60 / 1000) as i32
    }
}

fn on_off(b: bool) -> String {
    if b { "On" } else { "Off" }.into()
}

/// Steps through `list` from the entry nearest `cur` by `dir`, wrapping.
/// Dynamic frame generation targets (0 = the display's refresh rate).
const FG_TARGETS: [u32; 7] = [0, 120, 144, 165, 240, 360, 480];

fn step<T: PartialEq + Copy>(list: &[T], cur: T, dir: i32) -> T {
    let n = list.len() as i32;
    if n == 0 {
        return cur;
    }
    let i = list.iter().position(|&x| x == cur).map_or(if dir > 0 { -1 } else { 0 }, |i| i as i32);
    list[((i + dir).rem_euclid(n)) as usize]
}

/// Steps a number within `min..=max` by `inc` (clamped, not wrapping).
fn step_num(cur: f32, dir: i32, inc: f32, min: f32, max: f32) -> f32 {
    let v = ((cur / inc).round() + dir as f32) * inc;
    (v.clamp(min, max) * 1000.0).round() / 1000.0
}

pub fn action_label(a: u16) -> &'static str {
    match a {
        PAD_UP => "Up",
        PAD_DOWN => "Down",
        PAD_LEFT => "Left",
        PAD_RIGHT => "Right",
        PAD_CROSS => "Super hop / Select",
        PAD_SQUARE => "Tongue",
        PAD_START => "Pause / Start",
        PAD_CIRCLE => "Croak",
        PAD_TRIANGLE => "Back",
        PAD_L2 => "L2",
        PAD_R2 => "R2",
        PAD_SELECT => "Select",
        PAD_L1 => "L1",
        PAD_R1 => "R1",
        _ => "?",
    }
}

/// PSX button of an action (shown next to the action name).
fn action_button(a: u16) -> &'static str {
    match a {
        PAD_CROSS => " (Cross)",
        PAD_SQUARE => " (Square)",
        PAD_CIRCLE => " (Circle)",
        PAD_TRIANGLE => " (Triangle)",
        _ => "",
    }
}

/// Languages of the PC string table (`LANG_UK..LANG_US` column order) and config codes.
const LANGS: [(&str, &str); 5] = [("uk", "English (UK)"), ("fr", "Fran\u{e7}ais"), ("de", "Deutsch"), ("it", "Italiano"), ("us", "English (US)")];

/// `LANG_*` index of a config language code (`us` when unknown).
pub fn lang_index(code: &str) -> i32 {
    LANGS.iter().position(|(c, _)| c.eq_ignore_ascii_case(code)).map_or(f2_game::language::LANG_US, |i| i as i32)
}

const FRAME_CAPS: [u32; 6] = [0, 30, 60, 120, 144, 240];
const RENDER_SCALES: [f32; 7] = [0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0];
const DRAW_DISTANCES: [f32; 5] = [1.0, 1.5, 2.0, 3.0, 4.0];
/// Texture filter choices: (filter, anisotropy).
const FILTERS: [(TextureFilter, u16); 6] = [
    (TextureFilter::Nearest, 1),
    (TextureFilter::Linear, 1),
    (TextureFilter::Anisotropic, 1),
    (TextureFilter::Anisotropic, 4),
    (TextureFilter::Anisotropic, 8),
    (TextureFilter::Anisotropic, 16),
];

const ENH_HINT: &str = "Enhanced profile only: the authentic profile keeps the retail behaviour.";

impl App {
    // ------------------------------------------------------------ open / close

    /// Opens the settings screen on `page`.
    pub fn open_settings_page(&mut self, page: Page) {
        if self.settings.is_none() {
            // Everything counts as held until released (`prev` follows the pad each frame).
            self.settings = Some(SettingsUi::new(page, u16::MAX));
            self.languages = self.available_languages();
            self.acc = std::time::Duration::ZERO;
        } else if let Some(s) = &mut self.settings {
            s.page = Page::ALL.iter().position(|&x| x == page).unwrap_or(0);
        }
    }

    /// Closes the screen; the game resumes where it was (pause menu / Extras), with the
    /// buttons still held masked until released.
    pub fn close_settings(&mut self) {
        if self.settings.take().is_some() {
            self.pad_hold = u16::MAX;
            self.kb.pressed.clear();
            self.acc = std::time::Duration::ZERO;
            self.menu_sfx(false);
        }
    }

    /// Player 1's pad bits from the bindings (keyboard + joypad), without touching the
    /// game's input state.
    fn settings_pad(&self, kb: &KeyboardState, raw: &[Option<f2_platform::RawPad>]) -> u16 {
        let joypads = self.bindings.joypad_states(&self.mapper.controllers, raw, self.deadzone);
        let mut pd = PadData::default();
        self.mapper.process(&mut pd, kb, &joypads);
        pd.digital[0]
    }

    /// The menu sounds of the options screens: hop when moving, super hop when selecting.
    fn menu_sfx(&mut self, select: bool) {
        use f2_game::pcaudio::{gen_sfx, GEN_FROG_HOP, GEN_SUPER_HOP};
        let uid = gen_sfx(if select { GEN_SUPER_HOP } else { GEN_FROG_HOP });
        if self.audio.find_sample(uid).is_some() {
            self.audio.play(uid, f2_audio::Play::default());
        }
    }

    /// Languages with text in the rendition's string table.
    fn available_languages(&self) -> Vec<usize> {
        let t = f2_game::assets::DataSource::string_table(&self.data, f2_game::language::LANGUAGE_FILE).and_then(|r| r.ok());
        let Some(t) = t else { return vec![lang_index(&self.cfg.language) as usize] };
        (0..t.languages.len().min(LANGS.len()))
            .filter(|&i| t.rows.iter().take(120).filter(|r| r.2.get(i).is_some_and(|s| !s.trim().is_empty())).count() > 60)
            .collect()
    }

    // ------------------------------------------------------------ input

    /// A raw pad event while the screen is open (capture on a controller mapping page).
    pub fn settings_raw_pad(&mut self, ev: RawEvent) {
        let Some(m) = self.settings.as_mut().and_then(|s| s.map.as_mut()) else { return };
        if m.device == Device::Gamepad && m.capture.is_some() && !matches!(ev, RawEvent::Key(_)) {
            self.map_capture(ev);
        }
    }

    /// One presented frame with the screen open.
    pub fn settings_input(&mut self, kb: &KeyboardState, raw: &[Option<f2_platform::RawPad>]) {
        let dig = self.settings_pad(kb, raw);
        let Some(ui) = self.settings.as_mut() else { return };
        let deb = dig & !ui.prev;
        ui.prev = dig;
        if std::mem::take(&mut ui.fresh) {
            return;
        }
        // A capture prompt takes raw keys.
        if ui.map.as_ref().is_some_and(|m| m.capture.is_some()) {
            let keyboard = ui.map.as_ref().is_some_and(|m| m.device == Device::Keyboard);
            if keyboard {
                for &k in &kb.pressed.clone() {
                    self.map_capture(RawEvent::Key(k));
                    if self.settings.as_ref().and_then(|s| s.map.as_ref()).is_none_or(|m| m.capture.is_none()) {
                        break;
                    }
                }
            } else if kb.pressed.contains(&dik::ESCAPE) {
                self.map_capture(RawEvent::Key(dik::ESCAPE));
            }
            return;
        }
        let mut ev = 0u16;
        for (bit, n) in [
            (PAD_UP, nav::UP),
            (PAD_DOWN, nav::DOWN),
            (PAD_LEFT, nav::LEFT),
            (PAD_RIGHT, nav::RIGHT),
            (PAD_CROSS, nav::OK),
            (PAD_TRIANGLE, nav::BACK),
            (PAD_START, nav::BACK),
            (PAD_SQUARE, nav::CLEAR),
            (PAD_CIRCLE, nav::RESET),
            (PAD_L1, nav::PREV),
            (PAD_R1, nav::NEXT),
        ] {
            if deb & bit != 0 {
                ev |= n;
            }
        }
        for &k in &kb.pressed {
            ev |= match k {
                dik::UP => nav::UP,
                dik::DOWN => nav::DOWN,
                dik::LEFT => nav::LEFT,
                dik::RIGHT => nav::RIGHT,
                dik::RETURN | dik::NUMPADENTER => nav::OK,
                dik::ESCAPE => nav::BACK,
                dik::BACK | dik::DELETE => nav::CLEAR,
                dik::TAB | dik::NEXT => nav::NEXT,
                dik::PRIOR => nav::PREV,
                dik::INSERT => nav::ADD,
                dik::F10 => nav::CLOSE,
                _ => 0,
            };
        }
        // Held directions repeat.
        let dirs = (if dig & PAD_UP != 0 || kb.is_down(dik::UP) { nav::UP } else { 0 })
            | (if dig & PAD_DOWN != 0 || kb.is_down(dik::DOWN) { nav::DOWN } else { 0 })
            | (if dig & PAD_LEFT != 0 || kb.is_down(dik::LEFT) { nav::LEFT } else { 0 })
            | (if dig & PAD_RIGHT != 0 || kb.is_down(dik::RIGHT) { nav::RIGHT } else { 0 });
        let now = Instant::now();
        let ui = self.settings.as_mut().unwrap();
        match ui.repeat {
            Some((d, _)) if d != dirs => ui.repeat = (dirs != 0).then(|| (dirs, now + std::time::Duration::from_millis(400))),
            Some((d, t)) if now >= t => {
                ev |= d;
                ui.repeat = Some((d, now + std::time::Duration::from_millis(70)));
            }
            None if dirs != 0 => ui.repeat = Some((dirs, now + std::time::Duration::from_millis(400))),
            _ => {}
        }
        if ev != 0 {
            self.settings_nav(ev);
        }
    }

    /// Applies navigation events.
    pub fn settings_nav(&mut self, ev: u16) {
        if ev & nav::CLOSE != 0 {
            self.close_settings();
            return;
        }
        if self.settings.as_ref().is_some_and(|s| s.map.is_some()) {
            self.map_nav(ev);
            return;
        }
        let items = self.settings_items();
        let Some(ui) = self.settings.as_mut() else { return };
        let p = ui.page;
        let n = items.len();
        let mut moved = false;
        if ev & (nav::PREV | nav::NEXT) != 0 || (ui.row[p].is_none() && ev & (nav::LEFT | nav::RIGHT) != 0) {
            let d = if ev & (nav::PREV | nav::LEFT) != 0 { -1 } else { 1 };
            ui.page = (p as i32 + d).rem_euclid(PAGES as i32) as usize;
            self.menu_sfx(false);
            return;
        }
        if ev & nav::UP != 0 {
            ui.row[p] = match ui.row[p] {
                None => Some(n - 1),
                Some(0) => None,
                Some(r) => Some(r - 1),
            };
            moved = true;
        }
        if ev & nav::DOWN != 0 {
            ui.row[p] = match ui.row[p] {
                None => Some(0),
                Some(r) if r + 1 >= n => None,
                Some(r) => Some(r + 1),
            };
            moved = true;
        }
        if let Some(r) = ui.row[p] {
            let s = &mut ui.scroll[p];
            if r < *s {
                *s = r;
            } else if r >= *s + VISIBLE {
                *s = r + 1 - VISIBLE;
            }
        }
        if moved {
            self.menu_sfx(false);
        }
        if ev & nav::BACK != 0 {
            self.close_settings();
            return;
        }
        let Some(r) = self.settings.as_ref().and_then(|s| s.row[s.page]) else {
            if ev & nav::OK != 0 {
                self.settings.as_mut().unwrap().row[p] = Some(0);
            }
            return;
        };
        let it = &items[r];
        if !it.enabled {
            return;
        }
        let dir = if ev & nav::LEFT != 0 {
            -1
        } else if ev & nav::RIGHT != 0 {
            1
        } else {
            0
        };
        let ok = ev & nav::OK != 0;
        if (dir != 0 && it.arrows) || ok {
            let changed = self.settings_change(it.id, if dir == 0 { 1 } else { dir }, ok);
            if changed {
                self.menu_sfx(ok);
                self.requests.save_config = true;
            }
        }
    }

    // ------------------------------------------------------------ items

    /// Rows of the current page.
    pub fn settings_items(&self) -> Vec<Item> {
        let Some(ui) = &self.settings else { return Vec::new() };
        if let Some(m) = &ui.map {
            return self.map_items(m);
        }
        let enh = self.profile == Profile::Enhanced;
        let c = &self.cfg;
        let v = &c.video;
        let d = self.enhanced;
        let mut out = Vec::new();
        let mut add = |id, label: &str, value: String, desc: &str, enabled: bool, arrows: bool| {
            out.push(Item { id, label: label.into(), value, desc: desc.to_string(), enabled, arrows });
        };
        let gate = |e: bool, desc: &str| if e { desc.to_string() } else { format!("{desc} {ENH_HINT}") };
        match ui.page() {
            Page::Display => {
                let console = self.win.console;
                if !console && !self.win.fixed_mode {
                    add(
                        Id::WindowMode,
                        "Window mode",
                        v.window_mode.name().into(),
                        "Windowed, borderless fullscreen or exclusive fullscreen. Alt+Enter / F11 toggle windowed and your fullscreen choice.",
                        true,
                        true,
                    );
                }
                // Consoles: the output mode in use, not the (dynamic) render size.
                let auto = match self.win.desktop {
                    Some(m) if console => (m.width, m.height),
                    _ => self.win.size,
                };
                let res = match v.resolution {
                    None => format!("Auto ({}x{})", auto.0, auto.1),
                    Some([w, h]) => format!("{w}x{h}"),
                };
                let res_desc = match self.win.mode {
                    WindowMode::Windowed => "Auto follows the window size; a fixed size resizes the window.",
                    WindowMode::Borderless => "Auto renders at the desktop size; a fixed size renders at that size, scaled to the screen.",
                    WindowMode::Fullscreen => "The exclusive fullscreen video mode (Auto = desktop size).",
                };
                let res_desc = if self.win.fixed_mode {
                    "Auto renders at the screen's resolution; a fixed size renders at that size, scaled to fit with black bars."
                } else {
                    res_desc
                };
                let res_desc = if console {
                    "Size of the frames sent out (Auto = largest). The console scales them to its video output resolution, set in its Screen and Video settings."
                } else {
                    res_desc
                };
                add(Id::Resolution, "Resolution", res, res_desc, !console || self.win.resolutions(self.win.mode).len() > 1, true);
                let fs = console || self.win.mode == WindowMode::Fullscreen;
                let hz = |r: u32| format!("{:.2} Hz", r as f32 / 1000.0).replace(".00", "");
                let rate = match v.refresh_mhz {
                    _ if !fs => "N/A".into(),
                    // Consoles: the rate in use.
                    None if console => self.win.desktop.map_or("Auto".into(), |m| format!("Auto ({})", hz(m.refresh_mhz))),
                    None => match self.win.rates(v.resolution).first() {
                        Some(r) => format!("Highest ({:.0} Hz)", *r as f32 / 1000.0),
                        None => "Highest".into(),
                    },
                    Some(r) => hz(r),
                };
                let rate_desc = if console {
                    "Auto = 120 Hz when the TV and the console's video output allow it. The game runs at 60 Hz; 120 Hz adds in-between frames."
                } else if fs {
                    "Refresh rate of the exclusive fullscreen mode."
                } else {
                    "Exclusive fullscreen only."
                };
                if !self.win.fixed_mode {
                    add(Id::Refresh, "Refresh rate", rate, rate_desc, fs, true);
                }
                if !console {
                    add(Id::VSync, "VSync", on_off(v.vsync), "Wait for the display refresh (no tearing).", true, true);
                    let cap = match v.frame_cap {
                        0 => "Off".to_string(),
                        n if FRAME_CAPS.contains(&n) => format!("{n} fps"),
                        n => format!("{n} fps (custom)"),
                    };
                    add(Id::FrameCap, "Frame cap", cap, "Limits rendered frames. The game itself always runs at 60 Hz.", true, true);
                }
                add(
                    Id::Profile,
                    "Profile",
                    if enh { "Enhanced" } else { "Authentic" }.into(),
                    "Authentic: the retail look and behaviour. Enhanced: widescreen, smoother graphics and the gameplay options.",
                    true,
                    true,
                );
                add(Id::Widescreen, "Widescreen", on_off(d.hor_plus), &gate(enh, "Hor+ field of view and edge-anchored HUD on wide screens."), enh, true);
                add(
                    Id::RenderScale,
                    "Render scale",
                    format!("{}x", d.render_scale),
                    &gate(enh, "Internal resolution multiplier (2x = supersampling)."),
                    enh,
                    true,
                );
                add(Id::Msaa, "Anti-aliasing", if d.msaa > 1 { "4x MSAA".into() } else { "Off".into() }, &gate(enh, "Multisample anti-aliasing."), enh, true);
                add(
                    Id::Culling,
                    "Polygon culling",
                    if d.clip_faces { "Extended" } else { "Original" }.into(),
                    &gate(enh, "Extended draws the polygons the original drops at the screen edges and near the camera (wide screens)."),
                    enh,
                    true,
                );
                if !console {
                    self.upscale_items(&mut add, enh);
                }
                let filt = match d.filter {
                    TextureFilter::Nearest => "Nearest".to_string(),
                    TextureFilter::Linear => "Bilinear".into(),
                    TextureFilter::Anisotropic if d.anisotropy <= 1 => "Trilinear".into(),
                    TextureFilter::Anisotropic => format!("Anisotropic {}x", d.anisotropy),
                };
                add(
                    Id::Filter,
                    "Texture filter",
                    filt,
                    &gate(enh, "Nearest = blocky, bilinear = retail, trilinear / anisotropic = sharper at a distance."),
                    enh,
                    true,
                );
                add(
                    Id::Perspective,
                    "Perspective textures",
                    on_off(d.perspective_correct),
                    &gate(enh, "Perspective-correct texturing (retail: affine, wobbly)."),
                    enh,
                    true,
                );
                add(Id::DrawDistance, "Draw distance", format!("{}x", d.draw_distance), &gate(enh, "Far clip multiplier."), enh, true);
                add(Id::Fog, "Fog", on_off(d.fog), &gate(enh, "Distance fog so far geometry fades in instead of popping."), enh, true);
                let have_modern = self.modern_font_files.is_some();
                let font_val = match (d.modern_font, have_modern) {
                    (true, true) => "Modern",
                    (true, false) => "Original (no typeface)",
                    _ => "Original",
                };
                let font_desc = if have_modern {
                    "Modern = the game's typeface (Gill Sans Ultra Bold) sharp at any resolution; Original = the pixel fonts."
                } else {
                    "Needs Gill Sans Ultra Bold (GILSANUB.TTF, with Windows/Office) installed or in <data>/fonts/; built on next start."
                };
                add(Id::ModernFont, "Font", font_val.into(), &gate(enh, font_desc), enh, true);
                add(Id::Brightness, "Brightness", format!("{}%", v.brightness), "Final brightness multiply.", true, true);
                add(Id::ShowFps, "FPS counter", v.show_fps.name().into(), "Shows rendered frames per second and frame time.", true, true);
                add(
                    Id::ResetDisplay,
                    "Reset to defaults",
                    String::new(),
                    if console {
                        "Restores every Display setting (auto resolution and refresh rate, enhanced profile)."
                    } else {
                        "Restores every Display setting (windowed, auto resolution, enhanced profile)."
                    },
                    true,
                    false,
                );
            }
            Page::Audio => {
                add(Id::Master, "Master volume", format!("{}", c.audio.master), "Overall volume.", true, true);
                add(Id::Music, "Music volume", format!("{}", c.audio.music), "Same setting as the game's Sound Options.", true, true);
                add(Id::Sfx, "Effects volume", format!("{}", c.audio.sfx), "Same setting as the game's Sound Options.", true, true);
                add(Id::MuteUnfocused, "Mute in background", on_off(c.audio.mute_unfocused), "Silence the game while its window is not focused.", true, true);
                add(Id::ResetAudio, "Reset to defaults", String::new(), "Restores every Audio setting.", true, false);
            }
            Page::Controls => {
                add(Id::KeyboardMap, "Keyboard mapping", String::new(), "Rebind the keys of each action and player.", true, false);
                add(Id::ControllerMap, "Controller mapping", String::new(), "Rebind the controller buttons and stick directions of each player.", true, false);
                add(
                    Id::Deadzone,
                    "Stick dead zone",
                    format!("{}%", (c.input.deadzone * 100.0).round()),
                    "How far the stick must move before it counts.",
                    true,
                    true,
                );
                let sm = match c.input.stick_mode {
                    StickMode::FourWay => "4-way",
                    StickMode::Raw => "Raw (PC)",
                };
                add(
                    Id::StickMode,
                    "Stick as d-pad",
                    sm.into(),
                    "4-way: one direction at a time, like a PS1 d-pad. Raw: each axis on its own (PC retail; diagonals press two).",
                    true,
                    true,
                );
                let fl = match c.input.face_layout {
                    FaceLayout::Positional => "Positional",
                    FaceLayout::NintendoLabels => "Nintendo labels",
                };
                add(
                    Id::FaceLayout,
                    "Button layout",
                    fl.into(),
                    "Positional: the bottom face button is Cross on every pad. Nintendo labels: A confirms, B goes back (Nintendo pads only).",
                    true,
                    true,
                );
                add(Id::Rumble, "Rumble", on_off(c.input.rumble), "Controller vibration.", true, true);
                add(
                    Id::RumbleStrength,
                    "Rumble strength",
                    format!("{}%", (c.input.rumble_scale * 100.0).round()),
                    "Vibration intensity.",
                    c.input.rumble,
                    true,
                );
                let hop = c.hop_buffer_frames(Profile::Enhanced);
                add(
                    Id::HopBuffer,
                    "Hop input buffer",
                    if hop == 0 { "Off".into() } else { format!("{hop} frames") },
                    &gate(enh, "Remembers a hop pressed just before the frog can hop again."),
                    enh,
                    true,
                );
                add(Id::ResetControls, "Reset to defaults", String::new(), "Restores all key bindings and controller settings.", true, false);
            }
            Page::Export => self.export_items(&mut add),
            Page::Gameplay => {
                let g = &self.gameplay;
                add(Id::InfiniteLives, "Infinite lives", on_off(g.infinite_lives), &gate(enh, "Assist: dying never costs a life."), enh, true);
                add(Id::Invulnerable, "Invulnerable", on_off(g.invulnerable), &gate(enh, "Assist: enemies cannot hurt the frog."), enh, true);
                add(Id::EnemySpeed, "Enemy speed", format!("{}%", g.enemy_speed), &gate(enh, "Assist: slower enemies and platforms."), enh, true);
                add(Id::SkipFmv, "Any key skips FMVs", on_off(g.skip_fmv_any_key), &gate(enh, "Otherwise only Enter / Esc skip a video."), enh, true);
                add(Id::SkipLogos, "Skip intro logos", on_off(g.skip_intro_logos), &gate(enh, "Skip the logo videos at start-up."), enh, true);
                add(Id::SpeedrunTimer, "Speedrun timer", on_off(g.speedrun_timer), &gate(enh, "In-game and real time with splits."), enh, true);
                add(Id::SaveSlot, "Save slot", format!("{}", g.save_slot), "Left / Right choose a slot; @X loads it (restarts at the title).", true, true);
                add(Id::SaveNow, "Save now", String::new(), "Writes your progress to the selected slot.", true, false);
                add(Id::AutoSave, "Auto-save", on_off(g.auto_save), &gate(enh, "Save automatically after each level."), enh, true);
                add(
                    Id::TubeFix,
                    "Fix tube softlock",
                    on_off(g.fix_tube_softlock),
                    &gate(enh, "Fixes a softlock after a vacuum tube on the last level."),
                    enh,
                    true,
                );
                let li = lang_index(&c.language) as usize;
                add(
                    Id::Language,
                    "Language",
                    LANGS.get(li).map_or("?", |l| l.1).into(),
                    "Game text language. New text uses it at once; menus already on screen change when next opened or at the next level.",
                    self.languages.len() > 1,
                    true,
                );
                add(Id::ResetGameplay, "Reset to defaults", String::new(), "Restores every Gameplay setting.", true, false);
            }
        }
        out
    }

    /// The DLSS / FSR rows of the Display page (docs/UPSCALING.md).
    fn upscale_items(&self, add: &mut impl FnMut(Id, &str, String, &str, bool, bool), enh: bool) {
        use f2_platform::{FrameGeneration, UpscalerKind};
        let u = &self.cfg.upscale;
        let windows = cfg!(windows);
        let on = enh && windows;
        // What the running native path was started with (`None` = not running).
        let restart = match self.upscale_native {
            _ if !windows => "",
            None if u.active() => " Restart to turn it on.",
            Some(fg) if (fg == FrameGeneration::Off) != (u.frame_generation == FrameGeneration::Off) || fg.is_dlss() != u.frame_generation.is_dlss() => {
                " Restart to switch library."
            }
            _ => "",
        };
        let running = self.upscale_status.as_deref().map(|s| format!(" Now: {s}.")).unwrap_or_default();
        let base = if windows {
            "DLSS (GeForce RTX), FSR 3.1 (any GPU), FSR 4 (Radeon RX 7000 / 9000). DLLs go in upscalers."
        } else {
            "NVIDIA DLSS and AMD FSR need Windows (Direct3D 12)."
        };
        add(Id::Upscaler, "Upscaler", u.upscaler.name().into(), &format!("{base}{restart}{running}"), on, true);
        let up = u.upscaler != UpscalerKind::Off;
        add(
            Id::UpscaleQuality,
            "Upscaling mode",
            u.quality.name(u.upscaler).into(),
            "DLAA / Native AA renders at full size; Quality 1.5x, Balanced 1.7x, Performance 2x, Ultra Performance 3x smaller.",
            on && up,
            true,
        );
        if u.upscaler == UpscalerKind::Dlss {
            add(
                Id::DlssPreset,
                "DLSS preset",
                u.dlss_preset.name().into(),
                "DLSS model: Default = NVIDIA's choice per mode (K, L, M); J / K / L / M forced.",
                on,
                true,
            );
        }
        if matches!(u.upscaler, UpscalerKind::Fsr3 | UpscalerKind::Fsr4) {
            let sharp = if u.sharpness == 0 { "Off".to_string() } else { format!("{}%", u.sharpness) };
            add(Id::Sharpness, "FSR sharpening", sharp, "Extra sharpening after FSR (RCAS).", on, true);
        }
        add(
            Id::FrameGen,
            "Frame generation",
            u.frame_generation.name().into(),
            &format!("DLSS 2x (RTX 40 / 50), 3x / 4x / dynamic (RTX 50), FSR 2x (any GPU). Best with VSync off.{restart}"),
            on,
            true,
        );
        if u.frame_generation == FrameGeneration::DlssDynamic {
            let t = if u.target_fps == 0 { "Display refresh".to_string() } else { format!("{} fps", u.target_fps) };
            add(Id::FgTarget, "Target frame rate", t, "Dynamic frame generation picks 2x, 3x or 4x to reach this.", on, true);
        }
        if u.upscaler == UpscalerKind::Dlss || u.frame_generation.is_dlss() {
            add(Id::Reflex, "NVIDIA Reflex", u.reflex.name().into(), "Lower input latency (always on with DLSS frame generation).", on, true);
        }
    }

    /// Changes `id` by `dir` (Left/Right) or confirms it (`ok`). Returns whether anything
    /// changed (sound + save).
    fn settings_change(&mut self, id: Id, dir: i32, ok: bool) -> bool {
        let v = &mut self.cfg.video;
        match id {
            Id::Upscaler | Id::UpscaleQuality | Id::DlssPreset | Id::Sharpness | Id::FrameGen | Id::FgTarget | Id::Reflex => {
                use f2_platform::{DlssPreset, FrameGeneration, ReflexMode, UpscaleQuality, UpscalerKind};
                let u = &mut self.cfg.upscale;
                match id {
                    Id::Upscaler => u.upscaler = step(&UpscalerKind::ALL, u.upscaler, dir),
                    Id::UpscaleQuality => u.quality = step(&UpscaleQuality::ALL, u.quality, dir),
                    Id::DlssPreset => u.dlss_preset = step(&DlssPreset::ALL, u.dlss_preset, dir),
                    Id::Sharpness => u.sharpness = step_num(u.sharpness as f32, dir, 10.0, 0.0, 100.0) as u32,
                    Id::FrameGen => u.frame_generation = step(&FrameGeneration::ALL, u.frame_generation, dir),
                    Id::FgTarget => u.target_fps = step(&FG_TARGETS, u.target_fps, dir),
                    _ => u.reflex = step(&ReflexMode::ALL, u.reflex, dir),
                }
            }
            Id::WindowMode => {
                // Step from the requested mode: the window follows asynchronously, and
                // repeated presses must not restart from a stale state.
                let m = step(&WindowMode::ALL, v.window_mode, dir);
                v.select_mode(m);
                if m != WindowMode::Fullscreen {
                    v.refresh_mhz = None;
                }
                self.requests.apply_window = true;
            }
            Id::Resolution => {
                let list: Vec<Option<[u32; 2]>> =
                    std::iter::once(None).chain(self.win.resolutions(self.win.mode).into_iter().map(|(w, h)| Some([w, h]))).collect();
                v.resolution = step(&list, v.resolution, dir);
                v.refresh_mhz = None;
                self.requests.apply_window = true;
            }
            Id::Refresh => {
                let list: Vec<Option<u32>> = std::iter::once(None).chain(self.win.rates(v.resolution).into_iter().map(Some)).collect();
                v.refresh_mhz = step(&list, v.refresh_mhz, dir);
                self.requests.apply_window = true;
            }
            Id::VSync => {
                v.vsync = !v.vsync;
                self.requests.apply_vsync = true;
            }
            Id::FrameCap => {
                let mut list = FRAME_CAPS.to_vec();
                if !list.contains(&v.frame_cap) {
                    list.push(v.frame_cap);
                    list.sort_unstable();
                }
                v.frame_cap = step(&list, v.frame_cap, dir);
            }
            Id::Profile => {
                self.profile = self.profile.toggled();
                self.cfg.profile = self.profile;
                self.apply_display();
                self.apply_game_options();
            }
            Id::Brightness => v.brightness = step_num(v.brightness as f32, dir, 5.0, 50.0, 150.0) as u32,
            Id::ShowFps => v.show_fps = step(&FpsCorner::ALL, v.show_fps, dir),
            Id::ResetDisplay => {
                let keep = (v.width, v.height, v.window_pos);
                *v = f2_platform::Video { width: keep.0, height: keep.1, window_pos: keep.2, ..Default::default() };
                self.cfg.display = Default::default();
                self.cfg.profile = Profile::Enhanced;
                self.set_display(Profile::Enhanced, self.cfg.display_settings(Profile::Enhanced));
                self.apply_game_options();
                self.requests.apply_window = true;
                self.requests.apply_vsync = true;
            }
            Id::Widescreen | Id::RenderScale | Id::Msaa | Id::Culling | Id::Filter | Id::Perspective | Id::DrawDistance | Id::Fog | Id::ModernFont => {
                let e = &mut self.enhanced;
                let o = &mut self.cfg.display;
                match id {
                    Id::Widescreen => {
                        let on = !e.hor_plus;
                        (e.hor_plus, e.hud_anchor) = (on, on);
                        (o.hor_plus, o.hud_anchor) = (Some(on), Some(on));
                    }
                    Id::RenderScale => {
                        e.render_scale = step(&RENDER_SCALES, e.render_scale, dir);
                        o.render_scale = Some(e.render_scale);
                    }
                    Id::Msaa => {
                        e.msaa = if e.msaa > 1 { 1 } else { 4 };
                        o.msaa = Some(e.msaa);
                    }
                    Id::Culling => {
                        e.clip_faces = !e.clip_faces;
                        o.clip_faces = Some(e.clip_faces);
                    }
                    Id::Filter => {
                        let cur = (e.filter, if e.filter == TextureFilter::Anisotropic { e.anisotropy.max(1) } else { 1 });
                        let cur = FILTERS.iter().copied().find(|f| *f == cur).unwrap_or((e.filter, 1));
                        (e.filter, e.anisotropy) = step(&FILTERS, cur, dir);
                        e.anisotropy = e.anisotropy.max(1);
                        (o.filter, o.anisotropy) = (Some(e.filter), Some(e.anisotropy));
                    }
                    Id::Perspective => {
                        e.perspective_correct = !e.perspective_correct;
                        o.perspective_correct = Some(e.perspective_correct);
                    }
                    Id::DrawDistance => {
                        e.draw_distance = step(&DRAW_DISTANCES, e.draw_distance, dir);
                        o.draw_distance = Some(e.draw_distance);
                    }
                    Id::ModernFont => {
                        e.modern_font = !e.modern_font;
                        o.modern_font = Some(e.modern_font);
                    }
                    _ => {
                        e.fog = !e.fog;
                        o.fog = Some(e.fog);
                    }
                }
                self.apply_display();
            }
            Id::Master => {
                self.cfg.audio.master = step_num(self.cfg.audio.master as f32, dir, 5.0, 0.0, 100.0) as u8;
                self.apply_master_volume();
            }
            Id::Music | Id::Sfx => {
                let music = id == Id::Music;
                let cur = if music { self.cfg.audio.music } else { self.cfg.audio.sfx };
                let vol = step_num(cur as f32, dir, 5.0, 0.0, 100.0) as i32;
                self.set_game_volume(music, vol);
            }
            Id::MuteUnfocused => {
                self.cfg.audio.mute_unfocused = !self.cfg.audio.mute_unfocused;
                self.apply_master_volume();
            }
            Id::ResetAudio => {
                self.cfg.audio = f2_platform::config::Audio { muted: self.cfg.audio.muted, ..Default::default() };
                self.apply_master_volume();
                self.set_game_volume(true, 100);
                self.set_game_volume(false, 100);
            }
            Id::KeyboardMap | Id::ControllerMap => {
                if !ok {
                    return false;
                }
                let d = if id == Id::KeyboardMap { Device::Keyboard } else { Device::Gamepad };
                if let Some(ui) = &mut self.settings {
                    ui.map = Some(MapPage::new(d));
                }
            }
            Id::Deadzone => {
                self.cfg.input.deadzone = step_num(self.cfg.input.deadzone, dir, 0.05, 0.05, 0.95);
                self.deadzone = self.cfg.input.deadzone;
                self.requests.apply_input = true;
            }
            Id::StickMode => {
                self.cfg.input.stick_mode = step(&[StickMode::FourWay, StickMode::Raw], self.cfg.input.stick_mode, dir);
                self.requests.apply_input = true;
            }
            Id::FaceLayout => {
                self.cfg.input.face_layout = step(&[FaceLayout::Positional, FaceLayout::NintendoLabels], self.cfg.input.face_layout, dir);
                self.requests.apply_input = true;
            }
            Id::Rumble => {
                self.cfg.input.rumble = !self.cfg.input.rumble;
                self.requests.apply_input = true;
                if self.cfg.input.rumble {
                    self.rumble.push((0, 160, 20));
                }
            }
            Id::RumbleStrength => {
                self.cfg.input.rumble_scale = step_num(self.cfg.input.rumble_scale, dir, 0.1, 0.1, 1.0);
                self.requests.apply_input = true;
                self.rumble.push((0, 200, 15));
            }
            Id::HopBuffer => {
                let cur = self.cfg.hop_buffer_frames(Profile::Enhanced);
                let n = step_num(cur as f32, dir, 1.0, 0.0, 12.0) as u32;
                self.cfg.input.hop_buffer_frames = Some(n);
                self.hop_frames = n;
            }
            Id::ResetControls => {
                self.bindings.reset_all();
                self.cfg.input.deadzone = DEFAULT_DEADZONE;
                self.cfg.input.rumble = true;
                self.cfg.input.rumble_scale = 1.0;
                self.cfg.input.hop_buffer_frames = None;
                self.cfg.input.stick_mode = Default::default();
                self.cfg.input.face_layout = Default::default();
                self.deadzone = DEFAULT_DEADZONE;
                self.hop_frames = ENHANCED_HOP_BUFFER_FRAMES;
                self.apply_bindings();
                self.requests.apply_input = true;
            }
            Id::InfiniteLives | Id::Invulnerable | Id::EnemySpeed | Id::SkipFmv | Id::SkipLogos | Id::SpeedrunTimer | Id::AutoSave | Id::TubeFix => {
                let g = &mut self.gameplay;
                let flip = |b: &mut bool| *b = !*b;
                match id {
                    Id::InfiniteLives => flip(&mut g.infinite_lives),
                    Id::Invulnerable => flip(&mut g.invulnerable),
                    Id::EnemySpeed => g.enemy_speed = step_num(g.enemy_speed as f32, dir, 25.0, 25.0, 100.0) as u32,
                    Id::SkipFmv => flip(&mut g.skip_fmv_any_key),
                    Id::SkipLogos => flip(&mut g.skip_intro_logos),
                    Id::SpeedrunTimer => flip(&mut g.speedrun_timer),
                    Id::AutoSave => flip(&mut g.auto_save),
                    _ => flip(&mut g.fix_tube_softlock),
                }
                self.cfg.gameplay = self.gameplay.clone();
                self.apply_game_options();
            }
            Id::SaveSlot => {
                if ok {
                    self.close_settings();
                    self.apply_save_slot();
                    self.start_frontend();
                    self.osd = Some((format!("save slot {} loaded", self.gameplay.save_slot), 2.5));
                    return true;
                }
                let s = self.gameplay.save_slot.clamp(1, crate::gameplay::NUM_SLOTS) as i32;
                self.gameplay.save_slot = ((s - 1 + dir).rem_euclid(crate::gameplay::NUM_SLOTS as i32) + 1) as u8;
                self.cfg.gameplay = self.gameplay.clone();
                self.apply_save_slot();
            }
            Id::SaveNow => {
                if !ok {
                    return false;
                }
                let msg = match self.save_now() {
                    Ok(_) => format!("Saved to slot {}", self.gameplay.save_slot),
                    Err(e) => e,
                };
                if let Some(ui) = &mut self.settings {
                    ui.msg = msg;
                }
                return true;
            }
            Id::Language => {
                let langs = self.languages.clone();
                let cur = lang_index(&self.cfg.language) as usize;
                let next = step(&langs, cur, dir);
                self.cfg.language = LANGS[next].0.into();
                self.apply_language();
            }
            Id::ResetGameplay => {
                self.gameplay = f2_platform::Gameplay::default();
                self.cfg.gameplay = self.gameplay.clone();
                self.apply_save_slot();
                self.apply_game_options();
            }
            Id::MapPlayer | Id::MapAction(_) | Id::MapResetAll => return false,
            Id::Export(n) => {
                if !ok {
                    return false;
                }
                self.start_export(n as usize);
            }
            Id::OpenExports => {
                if !ok {
                    return false;
                }
                self.open_exports_folder();
            }
        }
        true
    }

    /// Master volume (0 while unfocused with "Mute in background").
    pub fn apply_master_volume(&mut self) {
        let mute = self.cfg.audio.mute_unfocused && !self.focused;
        self.audio.set_master_volume(if mute { 0.0 } else { self.cfg.audio.master as f32 / 100.0 });
    }

    /// Music / effects volume through the game's own state (`globalMusicVol` /
    /// `globalSoundVol`, what the Sound Options menu changes), so the game keeps it.
    fn set_game_volume(&mut self, music: bool, vol: i32) {
        let vol = vol.clamp(0, 100);
        if music {
            self.game.savegame.global_music_vol = vol;
            self.handle_effect(f2_game::Effect::SetMusicVolume { vol, cd_volume: (vol as u32 * 65535) / 100 });
        } else {
            self.game.savegame.global_sound_vol = vol;
            self.game.pcaudio.global_sound_vol = vol;
            self.handle_effect(f2_game::Effect::SetSoundVolume { vol });
        }
        self.cfg.audio.music = self.audio.music_volume().clamp(0, 100) as u8;
        self.cfg.audio.sfx = self.audio.sfx_volume().clamp(0, 100) as u8;
        self.requests.save_volumes = None;
    }

    /// Loads the game text of `cfg.language` (`gameTextInit`).
    pub fn apply_language(&mut self) {
        let lang = lang_index(&self.cfg.language);
        if self.game.language.game_text_lang != lang && self.has_session() {
            let data = self.data.clone();
            self.game.load_game_text(&data, lang);
            self.install_game_hooks();
        }
    }

    // ------------------------------------------------------------ mapping page

    fn map_items(&self, m: &MapPage) -> Vec<Item> {
        let fam = if m.device == Device::Keyboard { ControllerFamily::Generic } else { self.families[m.player as usize] };
        let mut out = vec![Item {
            id: Id::MapPlayer,
            label: "Player".into(),
            value: format!("{}", m.player + 1),
            desc: "Whose bindings to edit (players 2-4: local multiplayer).".into(),
            enabled: true,
            arrows: true,
        }];
        for &a in ACTIONS.iter() {
            let capturing = m.capture.as_ref().is_some_and(|c| c.action == a);
            let keys: Vec<String> = self.bindings.get(m.player, a).iter().filter(|b| m.device.accepts(b)).map(|b| b.label(fam)).collect();
            let value = if capturing {
                if m.device == Device::Keyboard { "Press a key..." } else { "Press a button..." }.into()
            } else if keys.is_empty() {
                "-".into()
            } else {
                keys.join(", ")
            };
            out.push(Item {
                id: Id::MapAction(a),
                label: format!("{}{}", action_label(a), action_button(a)),
                value,
                desc: String::new(),
                enabled: true,
                arrows: false,
            });
        }
        out.push(Item {
            id: Id::MapResetAll,
            label: "Reset all to defaults".into(),
            value: String::new(),
            desc: "Restores every player's original PC keys.".into(),
            enabled: true,
            arrows: false,
        });
        out
    }

    fn map_nav(&mut self, ev: u16) {
        let Some(ui) = self.settings.as_mut() else { return };
        let Some(m) = ui.map.as_mut() else { return };
        if let Some(c) = &mut m.conflict {
            let n = c.choices().len();
            if ev & nav::LEFT != 0 {
                c.choice = (c.choice + n - 1) % n;
                self.menu_sfx(false);
            } else if ev & nav::RIGHT != 0 {
                c.choice = (c.choice + 1) % n;
                self.menu_sfx(false);
            } else if ev & nav::BACK != 0 {
                m.conflict = None;
                m.msg = "Cancelled".into();
                self.menu_sfx(false);
            } else if ev & nav::OK != 0 {
                let c = m.conflict.take().unwrap();
                let r = c.choices()[c.choice];
                self.map_commit(c.rb, c.binding, r);
                self.menu_sfx(true);
            }
            return;
        }
        let n = ACTIONS.len() + 2;
        let mut moved = false;
        if ev & nav::UP != 0 {
            m.sel = (m.sel + n - 1) % n;
            moved = true;
        }
        if ev & nav::DOWN != 0 {
            m.sel = (m.sel + 1) % n;
            moved = true;
        }
        if m.sel < m.scroll {
            m.scroll = m.sel;
        } else if m.sel >= m.scroll + VISIBLE {
            m.scroll = m.sel + 1 - VISIBLE;
        }
        if moved {
            m.msg.clear();
            self.menu_sfx(false);
            return;
        }
        if ev & nav::BACK != 0 {
            ui.map = None;
            self.menu_sfx(false);
            return;
        }
        let (sel, player, device) = (m.sel, m.player, m.device);
        if sel == 0 {
            let d = if ev & nav::LEFT != 0 {
                -1
            } else if ev & (nav::RIGHT | nav::OK) != 0 {
                1
            } else {
                0
            };
            if d != 0 {
                m.player = (player as i32 + d).rem_euclid(4) as u8;
                self.menu_sfx(false);
            }
            return;
        }
        if sel == n - 1 {
            if ev & nav::OK != 0 {
                self.bindings.reset_all();
                m.msg = "All bindings restored to the PC defaults".into();
                self.apply_bindings();
                self.menu_sfx(true);
            }
            return;
        }
        let a = ACTIONS[sel - 1];
        if ev & (nav::OK | nav::ADD) != 0 {
            let mode = if ev & nav::ADD != 0 { RebindMode::Add } else { RebindMode::Replace };
            m.capture = self.bindings.begin_rebind_mode(player, a, mode);
            m.msg.clear();
            self.menu_sfx(true);
        } else if ev & nav::CLEAR != 0 {
            for b in self.bindings.get(player, a).to_vec() {
                if device.accepts(&b) {
                    self.bindings.remove(player, a, b);
                }
            }
            m.msg = format!("{} cleared", action_label(a));
            self.apply_bindings();
            self.menu_sfx(false);
        } else if ev & nav::RESET != 0 {
            self.bindings.reset_action(player, a);
            m.msg = format!("{} restored to default", action_label(a));
            self.apply_bindings();
            self.menu_sfx(false);
        }
    }

    /// A raw key for the capture prompt (headless screenshots, tests).
    pub fn settings_key_raw(&mut self, k: u8) {
        self.map_capture(RawEvent::Key(k));
    }

    /// Feeds a raw event to the capture prompt.
    fn map_capture(&mut self, ev: RawEvent) {
        let Some(m) = self.settings.as_mut().and_then(|s| s.map.as_mut()) else { return };
        let Some(rb) = m.capture.as_mut() else { return };
        match rb.feed(&self.bindings, ev) {
            RebindStatus::Waiting | RebindStatus::Finished => {}
            RebindStatus::Cancelled => {
                m.capture = None;
                m.msg = "Cancelled".into();
            }
            RebindStatus::Captured(c) => {
                let rb = m.capture.take().unwrap();
                if !m.device.accepts(&c.binding) {
                    m.msg = "That input belongs on the other mapping page".into();
                } else if c.conflicts.is_empty() {
                    self.map_commit(rb, c.binding, Resolve::Unbind);
                } else {
                    let fam = self.families[rb.player as usize];
                    let who: Vec<String> = c
                        .conflicts
                        .iter()
                        .map(|&(p, a)| if p == rb.player { action_label(a).to_string() } else { format!("P{} {}", p + 1, action_label(a)) })
                        .collect();
                    m.msg = format!("{} is already used by {}", c.binding.label(fam), who.join(", "));
                    m.conflict = Some(Conflict { rb, binding: c.binding, choice: 0 });
                }
                // The captured key must not also navigate.
                if let Some(ui) = self.settings.as_mut() {
                    ui.prev = u16::MAX;
                }
            }
        }
    }

    /// Applies a captured binding with the chosen conflict resolution.
    fn map_commit(&mut self, rb: Rebind, b: Binding, how: Resolve) {
        let Some(m) = self.settings.as_mut().and_then(|s| s.map.as_mut()) else { return };
        let fam = if m.device == Device::Keyboard { ControllerFamily::Generic } else { self.families[rb.player as usize] };
        if how == Resolve::Cancel {
            m.msg = "Cancelled".into();
            return;
        }
        let device = m.device;
        let old = self.bindings.get(rb.player, rb.action).iter().copied().find(|x| device.accepts(x) && *x != b);
        let others = self.bindings.commit_rebind(&rb, b, ConflictPolicy::Unbind);
        let mut msg = format!("{} -> {}", action_label(rb.action), b.label(fam));
        if !others.is_empty() {
            match (how, old) {
                (Resolve::Swap, Some(old)) => {
                    for &(p, a) in &others {
                        self.bindings.add(p, a, old);
                    }
                    msg = format!("{msg}; swapped: {} -> {}", others.iter().map(|&(_, a)| action_label(a)).collect::<Vec<_>>().join(", "), old.label(fam));
                }
                _ => msg = format!("{msg}; unbound from {}", others.iter().map(|&(_, a)| action_label(a)).collect::<Vec<_>>().join(", ")),
            }
        }
        if let Some(m) = self.settings.as_mut().and_then(|s| s.map.as_mut()) {
            m.msg = msg;
        }
        self.apply_bindings();
    }

    // ------------------------------------------------------------ drawing

    /// Draws the screen over the frozen game frame.
    pub fn draw_settings(&mut self) {
        let (Some(fonts), Some((big, small))) = (self.game_fonts, self.game_font_files.clone()) else { return };
        let items = self.settings_items();
        let Some(ui) = &self.settings else { return };
        let pulse = ui.pulse();
        let (pr, pg) = (f2_game::menus::pulse_r(pulse), f2_game::menus::pulse_g(pulse));
        let hi = [pr, pg, 10, 255];
        let green = [64, 160, 64, 255];
        let white = [255, 255, 255, 255];
        let grey = [96, 96, 96, 255];
        let r = &mut self.r;
        let c = Canvas::new(r);
        // Dim the game like a menu backdrop (`draw_quad` takes output pixels).
        let (ow, oh) = r.output_size();
        r.draw_quad(Quad2D::rect(0.0, 0.0, ow as f32, oh as f32, [0.0; 4], None, [0, 0, 0, 215], Blend::Semi));
        let t = |r: &mut Renderer, big_font: bool, x: f32, y: f32, s: &str, col: [u8; 4], align: i32| {
            let (id, f) = if big_font { (fonts.normal, &big) } else { (fonts.small, &small) };
            c.text(r, id, f, big_font, x, y, s, col, align, 1.0);
        };
        let map = ui.map.as_ref();
        let title = match map {
            Some(m) => format!("{} mapping", m.device.title()),
            None => "Settings".to_string(),
        };
        t(r, true, 2048.0, 260.0, &title, white, 0);
        // Page tabs.
        if map.is_none() {
            let on_tabs = ui.row[ui.page].is_none();
            for (i, p) in Page::ALL.iter().enumerate() {
                let x = 2048.0 + (i as f32 - (PAGES as f32 - 1.0) / 2.0) * 740.0;
                let col = if i == ui.page {
                    if on_tabs {
                        hi
                    } else {
                        white
                    }
                } else {
                    green
                };
                t(r, false, x, 640.0, p.title(), col, 0);
            }
            let ac = if on_tabs { hi } else { green };
            c.arrow(r, 2048.0 - 1880.0, 640.0 + 70.0, -1, ac);
            c.arrow(r, 2048.0 + 1880.0, 640.0 + 70.0, 1, ac);
        } else if let Some(m) = map {
            t(r, false, 2048.0, 640.0, &format!("Player {}", m.player + 1), white, 0);
        }
        // Rows.
        let (sel, scroll) = match map {
            Some(m) => (Some(m.sel), m.scroll),
            None => (ui.row[ui.page], ui.scroll[ui.page]),
        };
        for (i, it) in items.iter().enumerate().skip(scroll).take(VISIBLE) {
            let y = ROW_Y0 + (i - scroll) as f32 * ROW_H;
            let selected = sel == Some(i);
            let col = match (it.enabled, selected) {
                (false, true) => [150, 150, 150, 255],
                (false, false) => grey,
                (true, true) => hi,
                (true, false) => green,
            };
            let centred = it.value.is_empty() && !it.arrows;
            if centred {
                t(r, true, 2048.0, y, &it.label, col, 0);
                continue;
            }
            c.text(r, fonts.normal, &big, true, 420.0, y, &it.label, col, -1, 0.82);
            let vcol = if !it.enabled {
                grey
            } else if selected {
                hi
            } else {
                white
            };
            let vy = y + 40.0;
            let vx = 3560.0;
            let value = it.value.clone();
            let w = c.width(&small, &value, 1.0).min(1500.0);
            c.text(r, fonts.small, &small, false, vx, vy, &value, vcol, 1, 1.0);
            if it.arrows && it.enabled && selected {
                c.arrow(r, vx - w - 110.0, vy + 70.0, -1, hi);
                c.arrow(r, vx + 110.0, vy + 70.0, 1, hi);
            }
        }
        // Scroll markers.
        if scroll > 0 {
            c.arrow_v(r, 2048.0, ROW_Y0 - 130.0, -1, green);
        }
        if scroll + VISIBLE < items.len() {
            c.arrow_v(r, 2048.0, ROW_Y0 + VISIBLE as f32 * ROW_H + 10.0, 1, green);
        }
        // Description / prompt.
        let mut desc = sel.and_then(|s| items.get(s)).map(|it| it.desc.clone()).unwrap_or_else(|| "Left / Right choose a page.".into());
        let mut dcol = white;
        if let Some(m) = map {
            if let Some(cf) = &m.conflict {
                let opts: Vec<String> = cf
                    .choices()
                    .iter()
                    .enumerate()
                    .map(|(i, ch)| {
                        let n = match ch {
                            Resolve::Swap => "Swap",
                            Resolve::Unbind => "Unbind other",
                            Resolve::Cancel => "Cancel",
                        };
                        if i == cf.choice {
                            format!("[{n}]")
                        } else {
                            n.to_string()
                        }
                    })
                    .collect();
                desc = format!("{}.  {}", m.msg, opts.join("   "));
                dcol = hi;
            } else if let Some(cp) = &m.capture {
                desc = format!(
                    "Press the new {} for {} ({}).  Esc cancels.",
                    if m.device == Device::Keyboard { "key" } else { "button" },
                    action_label(cp.action),
                    if cp.mode == RebindMode::Add { "added" } else { "replaces the current one" }
                );
                dcol = hi;
            } else if !m.msg.is_empty() {
                desc = m.msg.clone();
                dcol = [120, 255, 120, 255];
            } else if sel.is_some_and(|s| s >= 1 && s <= ACTIONS.len()) {
                desc = if m.device == Device::Keyboard {
                    "@X Change   Ins Add key   @S / Del Clear   @C Reset action"
                } else {
                    "@X Change   @S Clear   @C Reset action"
                }
                .into();
            }
        } else if !ui.msg.is_empty() {
            desc = ui.msg.clone();
            dcol = [120, 255, 120, 255];
        }
        let desc = expand_prompts(&desc).into_owned();
        for (i, line) in c.wrap(&small, &desc, 3500.0).iter().take(3).enumerate() {
            t(r, false, 2048.0, 3140.0 + i as f32 * 170.0, line, dcol, 0);
        }
        let pad = !matches!(self.mapper.controllers[0], f2_platform::Controller::Keyboard);
        let hint = match (map.is_some(), pad) {
            (true, _) => " @X - Select    @T - Back ",
            (false, true) => " @X - Select    @T - Back    L1 / R1 - Page ",
            (false, false) => " @X - Select    @T - Back    Tab - Page ",
        };
        t(r, false, 2048.0, 3660.0, &expand_prompts(hint), white, 0);
    }

    /// The FPS counter (`[video] show_fps`), in the small game font in a screen corner.
    pub fn draw_fps(&mut self) {
        let corner = self.cfg.video.show_fps;
        if corner == FpsCorner::Off {
            return;
        }
        let (Some(fonts), Some((_, small))) = (self.game_fonts, self.game_font_files.as_ref()) else { return };
        let r = &mut self.r;
        let (ow, oh) = r.output_size();
        let k = (oh as f32 / 480.0).max(1.0);
        let lines = [format!("{:.0} FPS", self.fps.fps), format!("{:.1} ms  sim 60 Hz", self.fps.ms)];
        let lh = small.height as f32 * k + 2.0;
        let pad = 8.0 * k;
        // The game's HUD icons (lives, time/coin, counters) occupy every corner (anchored to
        // the real screen edges in enhanced widescreen), about 80 font pixels tall, so the
        // counter sits just below the top HUD row / above the bottom HUD row.
        let hud = 84.0 * k;
        for (i, l) in lines.iter().enumerate() {
            let w = small.string_width(l.as_bytes(), k) as f32;
            let x = match corner {
                FpsCorner::TopLeft | FpsCorner::BottomLeft => pad,
                _ => ow as f32 - pad - w,
            };
            let y = match corner {
                FpsCorner::TopLeft | FpsCorner::TopRight => hud + i as f32 * lh,
                _ => oh as f32 - hud - (lines.len() - i) as f32 * lh,
            };
            r.draw_text(fonts.small, x + k, y + k, l, [0, 0, 0, 180], k, None);
            r.draw_text(fonts.small, x, y, l, if i == 0 { [255, 255, 0, 255] } else { [255, 255, 255, 255] }, k, None);
        }
    }
}

/// The centred 4:3 area of the output, in the game's 4096×4096 overlay units.
struct Canvas {
    x0: f32,
    sw: f32,
    oh: f32,
    /// Output pixels per 640×480 font pixel.
    k: f32,
}

impl Canvas {
    fn new(r: &Renderer) -> Canvas {
        let (ow, oh) = r.output_size();
        let (ow, oh) = (ow as f32, oh as f32);
        let sw = ow.min(oh * 4.0 / 3.0);
        Canvas { x0: ((ow - sw) / 2.0).floor(), sw, oh, k: sw / 640.0 }
    }

    fn px(&self, ux: f32) -> f32 {
        self.x0 + ux * self.sw / 4096.0
    }

    fn py(&self, uy: f32) -> f32 {
        uy * self.oh / 4096.0
    }

    /// Width of `s` in overlay units.
    fn width(&self, f: &Font, s: &str, scale: f32) -> f32 {
        let b: Vec<u8> = s.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).collect();
        f.string_width(&b, 1.0) as f32 * 4096.0 / 640.0 * scale
    }

    /// Text with the game's drop shadow; `align` -1 left, 0 centred, 1 right.
    #[allow(clippy::too_many_arguments)]
    fn text(&self, r: &mut Renderer, id: FontId, f: &Font, big: bool, ux: f32, uy: f32, s: &str, col: [u8; 4], align: i32, scale: f32) {
        let k = self.k * scale;
        let b: Vec<u8> = s.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).collect();
        let w = f.string_width(&b, k) as f32;
        let x = self.px(ux)
            - match align {
                -1 => 0.0,
                0 => w / 2.0,
                _ => w,
            };
        let y = self.py(uy);
        let dy = if big { 6.0 } else { 4.0 } * self.k.max(1.0);
        r.draw_text(id, x, y + dy, s, [0, 0, 0, (col[3] as f32 * 0.7) as u8], k, None);
        r.draw_text(id, x, y, s, col, k, None);
    }

    /// Word-wraps `s` to `max` overlay units.
    fn wrap(&self, f: &Font, s: &str, max: f32) -> Vec<String> {
        let mut out = Vec::new();
        let mut line = String::new();
        for w in s.split(' ') {
            let cand = if line.is_empty() { w.to_string() } else { format!("{line} {w}") };
            if !line.is_empty() && self.width(f, &cand, 1.0) > max {
                out.push(std::mem::take(&mut line));
                line = w.to_string();
            } else {
                line = cand;
            }
        }
        if !line.is_empty() {
            out.push(line);
        }
        out
    }

    fn tri(&self, r: &mut Renderer, p: [[f32; 2]; 3], col: [u8; 4]) {
        // Output pixels: `draw_quad` maps them to the render size itself.
        let mut quad = Quad2D::rect(0.0, 0.0, 1.0, 1.0, [0.0; 4], None, col, Blend::Semi);
        quad.pos = [p[0], p[1], p[2], p[2]];
        r.draw_quad(quad);
    }

    /// A ◄ / ► arrow centred at (`ux`, `uy`).
    fn arrow(&self, r: &mut Renderer, ux: f32, uy: f32, dir: i32, col: [u8; 4]) {
        let (x, y) = (self.px(ux), self.py(uy));
        let h = 7.0 * self.k;
        let w = 6.0 * self.k * dir as f32;
        let sh = 2.0 * self.k.max(1.0);
        self.tri(r, [[x - w / 2.0 + sh, y - h + sh], [x + w / 2.0 + sh, y + sh], [x - w / 2.0 + sh, y + h + sh]], [0, 0, 0, 160]);
        self.tri(r, [[x - w / 2.0, y - h], [x + w / 2.0, y], [x - w / 2.0, y + h]], col);
    }

    /// A ▲ / ▼ scroll marker.
    fn arrow_v(&self, r: &mut Renderer, ux: f32, uy: f32, dir: i32, col: [u8; 4]) {
        let (x, y) = (self.px(ux), self.py(uy));
        let w = 8.0 * self.k;
        let h = 5.0 * self.k * dir as f32;
        self.tri(r, [[x - w, y - h], [x + w, y - h], [x, y + h]], col);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps() {
        assert_eq!(step(&[0u32, 30, 60], 30, 1), 60);
        assert_eq!(step(&[0u32, 30, 60], 60, 1), 0);
        assert_eq!(step(&[0u32, 30, 60], 0, -1), 60);
        assert_eq!(step(&[0u32, 30, 60], 45, 1), 0);
        assert_eq!(step_num(100.0, 1, 5.0, 0.0, 100.0), 100.0);
        assert_eq!(step_num(0.5, -1, 0.05, 0.05, 0.95), 0.45);
        assert_eq!(lang_index("de"), 2);
        assert_eq!(lang_index("xx"), f2_game::language::LANG_US);
    }

    #[test]
    fn resolutions_follow_mode() {
        let m = |w, h, r| VideoMode { width: w, height: h, refresh_mhz: r };
        let wi = WinInfo {
            mode: WindowMode::Windowed,
            size: (800, 600),
            modes: vec![m(1920, 1080, 60000), m(1920, 1080, 144000), m(1280, 720, 60000)],
            desktop: Some(m(1920, 1080, 144000)),
            console: false,
            fixed_mode: false,
        };
        let fs = wi.resolutions(WindowMode::Fullscreen);
        assert_eq!(fs, vec![(1280, 720), (1920, 1080)]);
        let win = wi.resolutions(WindowMode::Windowed);
        assert!(win.contains(&(640, 480)) && win.contains(&(1600, 900)) && !win.contains(&(2560, 1440)));
        assert_eq!(wi.rates(Some([1920, 1080])), vec![144000, 60000]);
        assert_eq!(wi.rates(None), vec![144000, 60000]);
        // Consoles: only the host's output modes.
        let con = WinInfo {
            mode: WindowMode::Fullscreen,
            size: (960, 540),
            modes: vec![m(3840, 2160, 119880), m(3840, 2160, 59940), m(2560, 1440, 119880), m(2560, 1440, 59940)],
            desktop: Some(m(3840, 2160, 119880)),
            console: true,
            fixed_mode: false,
        };
        assert_eq!(con.resolutions(WindowMode::Windowed), vec![(2560, 1440), (3840, 2160)]);
        assert_eq!(con.rates(None), vec![119880, 59940]);
    }

    #[test]
    fn conflict_choices() {
        let b = f2_platform::Bindings::default();
        let rb = b.begin_rebind_mode(0, PAD_UP, RebindMode::Replace).unwrap();
        let c = Conflict { rb, binding: Binding::Key(dik::A), choice: 0 };
        assert_eq!(c.choices(), &[Resolve::Swap, Resolve::Unbind, Resolve::Cancel]);
    }
}
