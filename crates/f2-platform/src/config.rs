//! `config.toml`: replaces the registry settings and the launcher config dialog.

use crate::bindings::{BindingEntry, Bindings, DEFAULT_DEADZONE};
use crate::input::{Controller, InputMapper, KeyBinding, Keymap};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// `authentic` (retail look, parity screenshots) or `enhanced` (default for players).
    pub profile: Profile,
    pub video: Video,
    /// Enhanced-profile display overrides (`[display]`); ignored in the authentic profile.
    pub display: Display,
    pub audio: Audio,
    /// Text language code (`us`, `uk`, `fr`, `de`, `it`).
    pub language: String,
    /// Imported data root; `None` = [`crate::paths::default_data_root`].
    pub data_root: Option<PathBuf>,
    /// Selected imported rendition (e.g. `usa`, `eu`); `None` = first installed.
    pub rendition: Option<String>,
    pub input: Input,
    /// `[mods]`: enabled mods (`docs/modding.md`).
    pub mods: Mods,
    /// Gameplay / QoL options (`[gameplay]`, P6-1); they apply in the enhanced profile only.
    pub gameplay: Gameplay,
    /// `[launcher]`: what the app opens on start, and which installed version runs.
    pub launcher: LauncherCfg,
    /// `[upscale]`: DLSS / FSR super resolution and frame generation (docs/UPSCALING.md).
    pub upscale: Upscale,
}

/// `[upscale]` (enhanced profile, Windows / Direct3D 12; docs/UPSCALING.md).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Upscale {
    pub upscaler: UpscalerKind,
    /// Render size per axis: `native` (DLAA / FSR Native AA) to `ultra_performance` (3x).
    pub quality: UpscaleQuality,
    /// DLSS model preset (`default` = the driver's choice per mode).
    pub dlss_preset: DlssPreset,
    /// FSR sharpening, 0 (off) to 100.
    pub sharpness: u32,
    pub frame_generation: FrameGeneration,
    /// `dlss_dynamic`: the frame rate to generate up to (0 = the display's refresh rate).
    pub target_fps: u32,
    pub reflex: ReflexMode,
    /// Folder with the DLSS / Streamline and FSR DLLs; `None` = `upscalers` beside the
    /// executable, else the executable's own folder.
    pub runtime_dir: Option<PathBuf>,
    /// GPU to run on (a substring of its name, e.g. `Radeon`); `None` = the fastest.
    pub adapter: Option<String>,
    /// DLSS 5 Neural Rendering through RenoDX's ReShade add-on: preload its runtime and nudge
    /// the add-on to attach (docs/UPSCALING.md "DLSS 5").
    pub dlss5_helpers: bool,
}

impl Default for Upscale {
    fn default() -> Self {
        Upscale {
            upscaler: UpscalerKind::Off,
            quality: UpscaleQuality::Quality,
            dlss_preset: DlssPreset::Default,
            sharpness: 0,
            frame_generation: FrameGeneration::Off,
            target_fps: 0,
            reflex: ReflexMode::Off,
            runtime_dir: None,
            adapter: None,
            dlss5_helpers: false,
        }
    }
}

#[cfg(test)]
mod upscale_tests {
    use super::*;

    #[test]
    fn upscale_section_round_trips() {
        let mut c = Config::default();
        c.upscale.upscaler = UpscalerKind::Fsr4;
        c.upscale.quality = UpscaleQuality::UltraPerformance;
        for f in FrameGeneration::ALL {
            c.upscale.frame_generation = f;
            let t = c.to_toml();
            assert_eq!(Config::from_toml(&t).unwrap().upscale, c.upscale, "{t}");
        }
        let c = Config::from_toml("[upscale]\nupscaler = \"dlss\"\nquality = \"native\"\nframe_generation = \"dlss_4x\"\nreflex = \"boost\"\n").unwrap();
        assert_eq!(c.upscale.frame_generation, FrameGeneration::Dlss4x);
        assert_eq!(c.upscale.reflex, ReflexMode::Boost);
        assert!(c.upscale.active());
    }
}

impl Upscale {
    /// Whether the native (Direct3D 12, own swapchain) path is needed.
    pub fn active(&self) -> bool {
        self.upscaler != UpscalerKind::Off || self.frame_generation != FrameGeneration::Off
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpscalerKind {
    #[default]
    Off,
    /// NVIDIA DLSS (GeForce RTX).
    Dlss,
    /// AMD FSR 3.1 (any GPU).
    Fsr3,
    /// AMD FSR 4 (Radeon RX 7000 / 9000).
    Fsr4,
}

impl UpscalerKind {
    pub const ALL: [UpscalerKind; 4] = [UpscalerKind::Off, UpscalerKind::Dlss, UpscalerKind::Fsr3, UpscalerKind::Fsr4];

    pub fn name(self) -> &'static str {
        match self {
            UpscalerKind::Off => "Off",
            UpscalerKind::Dlss => "NVIDIA DLSS",
            UpscalerKind::Fsr3 => "AMD FSR 3.1",
            UpscalerKind::Fsr4 => "AMD FSR 4",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpscaleQuality {
    /// Render at the output size: DLAA / FSR Native AA.
    Native,
    #[default]
    Quality,
    Balanced,
    Performance,
    UltraPerformance,
}

impl UpscaleQuality {
    pub const ALL: [UpscaleQuality; 5] =
        [UpscaleQuality::Native, UpscaleQuality::Quality, UpscaleQuality::Balanced, UpscaleQuality::Performance, UpscaleQuality::UltraPerformance];

    /// The mode's name for `upscaler` (DLSS calls native anti-aliasing DLAA, FSR "Native AA").
    pub fn name(self, upscaler: UpscalerKind) -> &'static str {
        match self {
            UpscaleQuality::Native if upscaler == UpscalerKind::Dlss => "DLAA",
            UpscaleQuality::Native => "Native AA",
            UpscaleQuality::Quality => "Quality",
            UpscaleQuality::Balanced => "Balanced",
            UpscaleQuality::Performance => "Performance",
            UpscaleQuality::UltraPerformance => "Ultra Performance",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DlssPreset {
    #[default]
    Default,
    J,
    K,
    L,
    M,
}

impl DlssPreset {
    pub const ALL: [DlssPreset; 5] = [DlssPreset::Default, DlssPreset::J, DlssPreset::K, DlssPreset::L, DlssPreset::M];

    pub fn name(self) -> &'static str {
        match self {
            DlssPreset::Default => "Default",
            DlssPreset::J => "J",
            DlssPreset::K => "K",
            DlssPreset::L => "L",
            DlssPreset::M => "M",
        }
    }

    /// `sl::DLSSPreset`.
    pub fn streamline(self) -> u32 {
        match self {
            DlssPreset::Default => 0,
            DlssPreset::J => 10,
            DlssPreset::K => 11,
            DlssPreset::L => 12,
            DlssPreset::M => 13,
        }
    }
}

/// Frame generation: which library and how many frames per rendered one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameGeneration {
    #[default]
    Off,
    /// DLSS Frame Generation (GeForce RTX 40 / 50).
    #[serde(rename = "dlss_2x")]
    Dlss2x,
    /// DLSS Multi Frame Generation (GeForce RTX 50).
    #[serde(rename = "dlss_3x")]
    Dlss3x,
    #[serde(rename = "dlss_4x")]
    Dlss4x,
    /// DLSS dynamic Multi Frame Generation to `target_fps` (GeForce RTX 50).
    #[serde(rename = "dlss_dynamic")]
    DlssDynamic,
    /// AMD FSR Frame Generation (any GPU).
    #[serde(rename = "fsr_2x")]
    Fsr2x,
}

impl FrameGeneration {
    pub const ALL: [FrameGeneration; 6] =
        [FrameGeneration::Off, FrameGeneration::Dlss2x, FrameGeneration::Dlss3x, FrameGeneration::Dlss4x, FrameGeneration::DlssDynamic, FrameGeneration::Fsr2x];

    pub fn name(self) -> &'static str {
        match self {
            FrameGeneration::Off => "Off",
            FrameGeneration::Dlss2x => "DLSS 2x",
            FrameGeneration::Dlss3x => "DLSS 3x",
            FrameGeneration::Dlss4x => "DLSS 4x",
            FrameGeneration::DlssDynamic => "DLSS dynamic",
            FrameGeneration::Fsr2x => "FSR 2x",
        }
    }

    pub fn is_dlss(self) -> bool {
        matches!(self, FrameGeneration::Dlss2x | FrameGeneration::Dlss3x | FrameGeneration::Dlss4x | FrameGeneration::DlssDynamic)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReflexMode {
    #[default]
    Off,
    On,
    Boost,
}

impl ReflexMode {
    pub const ALL: [ReflexMode; 3] = [ReflexMode::Off, ReflexMode::On, ReflexMode::Boost];

    pub fn name(self) -> &'static str {
        match self {
            ReflexMode::Off => "Off",
            ReflexMode::On => "On",
            ReflexMode::Boost => "On + Boost",
        }
    }
}

/// `[launcher]` (docs/USER-GUIDE.md "Launcher").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LauncherCfg {
    /// `game` (default) or `launcher`: what `frogger2` opens on start.
    pub start: String,
    /// Release tag of the version the launcher runs (`v0.1.0`); empty = the build in the
    /// install folder itself.
    pub version: String,
    /// The releases API to list versions from; `None` = the project's GitHub repository.
    pub releases_url: Option<String>,
    /// Ask GitHub for newer releases when the launcher starts (one small request).
    pub check_updates: bool,
    /// Where the launcher last put the game's shortcut.
    pub shortcut: Option<PathBuf>,
}

impl Default for LauncherCfg {
    fn default() -> Self {
        LauncherCfg { start: "game".into(), version: String::new(), releases_url: None, check_updates: true, shortcut: None }
    }
}

impl LauncherCfg {
    pub fn start_in_launcher(&self) -> bool {
        self.start.eq_ignore_ascii_case("launcher")
    }
}

/// `[mods]`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mods {
    /// Mod ids in load order (later mods win).
    pub enabled: Vec<String>,
    /// Directory holding the mods; `None` = [`crate::paths::default_mods_dir`].
    pub dir: Option<PathBuf>,
}

/// `[gameplay]`: enhanced-profile gameplay and QoL options (`docs/enhancements.md`). The
/// authentic profile ignores everything here except `save_slot`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gameplay {
    /// Assist: dying never costs a life.
    pub infinite_lives: bool,
    /// Assist: enemies cannot hurt the frog.
    pub invulnerable: bool,
    /// Assist: enemy/platform speed in percent (25..=100).
    pub enemy_speed: u32,
    /// Any key or pad button skips an FMV (authentic: Enter/Esc only).
    pub skip_fmv_any_key: bool,
    /// Skip the Hasbro / Blitz logo FMVs at start-up.
    pub skip_intro_logos: bool,
    /// Show the speedrun timer (in-game time + real time) and write splits.
    pub speedrun_timer: bool,
    /// Save slot 1..=4 (slot 1 = `saves/savegame.dat`, slot N = `saves/slotN/savegame.dat`).
    pub save_slot: u8,
    /// Write the game's automatic saves (after a level); off = only "Save now" in the menu.
    pub auto_save: bool,
    /// Fix known bug #11 (softlock after a vacuum tube on the last level).
    pub fix_tube_softlock: bool,
}

impl Default for Gameplay {
    fn default() -> Self {
        Gameplay {
            infinite_lives: false,
            invulnerable: false,
            enemy_speed: 100,
            skip_fmv_any_key: true,
            skip_intro_logos: false,
            speedrun_timer: false,
            save_slot: 1,
            auto_save: true,
            fix_tube_softlock: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Video {
    /// Windowed-mode inner size (remembered across runs; follows the user's resizing).
    pub width: u32,
    pub height: u32,
    /// Legacy switch (`true` = fullscreen). Kept in sync with `window_mode`; a config with
    /// only `fullscreen = true` starts in `fullscreen_type`.
    pub fullscreen: bool,
    /// Windowed / borderless / exclusive fullscreen.
    pub window_mode: WindowMode,
    /// What Alt+Enter / F11 switch to from windowed: borderless unless exclusive fullscreen
    /// was chosen in the settings screen.
    pub fullscreen_type: FullscreenType,
    /// `None` = auto (window / desktop size). Explicit: the window's inner size (windowed),
    /// the video mode (exclusive fullscreen) or the render-target size (borderless).
    pub resolution: Option<[u32; 2]>,
    /// Exclusive fullscreen refresh rate in mHz (`None` = highest for the resolution).
    pub refresh_mhz: Option<u32>,
    /// Windowed-mode outer position (remembered).
    pub window_pos: Option<[i32; 2]>,
    pub vsync: bool,
    /// Frame-rate cap in frames per second (`0` = uncapped; with `vsync` the display rate
    /// caps it anyway). The simulation always ticks at 60 Hz; rendering is interpolated.
    pub frame_cap: u32,
    /// Final-pass brightness in percent (100 = unchanged).
    pub brightness: u32,
    /// FPS counter corner (`off` = hidden).
    pub show_fps: FpsCorner,
}

/// How the game window covers the screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowMode {
    #[default]
    Windowed,
    /// Borderless window covering the monitor (desktop video mode).
    Borderless,
    /// Exclusive fullscreen with a chosen video mode.
    Fullscreen,
}

impl WindowMode {
    pub const ALL: [WindowMode; 3] = [WindowMode::Windowed, WindowMode::Borderless, WindowMode::Fullscreen];

    pub fn name(self) -> &'static str {
        match self {
            WindowMode::Windowed => "Windowed",
            WindowMode::Borderless => "Borderless",
            WindowMode::Fullscreen => "Fullscreen",
        }
    }
}

/// Fullscreen flavour Alt+Enter / F11 toggle to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FullscreenType {
    #[default]
    Borderless,
    Exclusive,
}

impl FullscreenType {
    pub fn mode(self) -> WindowMode {
        match self {
            FullscreenType::Borderless => WindowMode::Borderless,
            FullscreenType::Exclusive => WindowMode::Fullscreen,
        }
    }
}

/// Corner of the FPS counter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FpsCorner {
    #[default]
    Off,
    TopRight,
    TopLeft,
    BottomRight,
    BottomLeft,
}

impl FpsCorner {
    pub const ALL: [FpsCorner; 5] = [FpsCorner::Off, FpsCorner::TopRight, FpsCorner::TopLeft, FpsCorner::BottomRight, FpsCorner::BottomLeft];

    pub fn name(self) -> &'static str {
        match self {
            FpsCorner::Off => "Off",
            FpsCorner::TopRight => "Top right",
            FpsCorner::TopLeft => "Top left",
            FpsCorner::BottomRight => "Bottom right",
            FpsCorner::BottomLeft => "Bottom left",
        }
    }
}

impl Video {
    /// Alt+Enter / F11: windowed <-> the fullscreen type.
    pub fn toggle_fullscreen(&mut self) {
        self.window_mode = match self.window_mode {
            WindowMode::Windowed => self.fullscreen_type.mode(),
            _ => WindowMode::Windowed,
        };
        self.fullscreen = self.window_mode != WindowMode::Windowed;
    }

    /// An explicit choice in the settings screen; a fullscreen choice becomes what the
    /// toggle switches to.
    pub fn select_mode(&mut self, m: WindowMode) {
        self.window_mode = m;
        match m {
            WindowMode::Borderless => self.fullscreen_type = FullscreenType::Borderless,
            WindowMode::Fullscreen => self.fullscreen_type = FullscreenType::Exclusive,
            WindowMode::Windowed => {}
        }
        self.fullscreen = m != WindowMode::Windowed;
    }

    /// Follows the actual window state (the OS or the window manager changed it). Returns
    /// whether anything changed. The fullscreen type is left alone.
    pub fn observe_mode(&mut self, actual: WindowMode) -> bool {
        if self.window_mode == actual {
            return false;
        }
        self.window_mode = actual;
        self.fullscreen = actual != WindowMode::Windowed;
        true
    }

    /// Legacy `fullscreen = true` without a `window_mode`: start in the fullscreen type.
    pub fn migrate(&mut self) {
        if self.fullscreen && self.window_mode == WindowMode::Windowed {
            self.window_mode = self.fullscreen_type.mode();
        }
    }
}

/// Behaviour profile (`docs/enhancements.md`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    /// Retail behaviour and look; parity tests and screenshots run in this profile.
    Authentic,
    /// Fixes and QoL (project decision: the default for players).
    #[default]
    Enhanced,
}

impl Profile {
    pub fn toggled(self) -> Profile {
        match self {
            Profile::Authentic => Profile::Enhanced,
            Profile::Enhanced => Profile::Authentic,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Profile::Authentic => "authentic",
            Profile::Enhanced => "enhanced",
        }
    }
}

/// Texture filtering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextureFilter {
    /// Point sampling ("PSX look").
    Nearest,
    /// Bilinear without mipmaps (retail `D3DTFN_LINEAR`).
    Linear,
    /// Trilinear mipmaps + anisotropic filtering (`anisotropy` level).
    Anisotropic,
}

/// `[display]`: per-option overrides of the enhanced profile. Unset options take the
/// enhanced defaults ([`DisplaySettings::enhanced`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Display {
    pub hor_plus: Option<bool>,
    pub hud_anchor: Option<bool>,
    pub max_aspect: Option<f32>,
    pub render_scale: Option<f32>,
    pub msaa: Option<u32>,
    pub filter: Option<TextureFilter>,
    pub anisotropy: Option<u16>,
    pub perspective_correct: Option<bool>,
    pub draw_distance: Option<f32>,
    pub fog: Option<bool>,
    pub modern_font: Option<bool>,
}

/// Concrete display settings of a profile (what the host applies to the renderer).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplaySettings {
    /// Hor+ widescreen (keep the 4:3 vertical FOV); off = original Vert- scaling.
    pub hor_plus: bool,
    /// Re-anchor 4096×4096 overlays to the screen edges; pillarbox full-screen ones.
    pub hud_anchor: bool,
    /// Widest 3D aspect ratio before pillarboxing (`0` = no clamp, e.g. `2.37` for 21:9).
    pub max_aspect: f32,
    /// Internal resolution multiplier (`2` = 4× supersampling).
    pub render_scale: f32,
    /// MSAA samples (1 or 4).
    pub msaa: u32,
    pub filter: TextureFilter,
    /// Anisotropy level for [`TextureFilter::Anisotropic`] (1 = trilinear only, max 16).
    pub anisotropy: u16,
    /// Perspective-correct texturing (retail: affine).
    pub perspective_correct: bool,
    /// Far-clip multiplier (`1` = the level's `farClip`).
    pub draw_distance: f32,
    /// Linear distance fog to the level's clear colour ending at the far clip (retail
    /// leaves fog disabled).
    pub fog: bool,
    /// Draw the game fonts from the high-resolution atlases built from their typeface
    /// (`f2-fontgen`) when present; off = the original pixel fonts.
    pub modern_font: bool,
}

impl DisplaySettings {
    /// Retail: Vert- projection, stretched overlays, no AA, bilinear, affine, no fog.
    pub const AUTHENTIC: DisplaySettings = DisplaySettings {
        hor_plus: false,
        hud_anchor: false,
        max_aspect: 0.0,
        render_scale: 1.0,
        msaa: 1,
        filter: TextureFilter::Linear,
        anisotropy: 0,
        perspective_correct: false,
        draw_distance: 1.0,
        fog: false,
        modern_font: false,
    };

    /// Enhanced defaults.
    pub const ENHANCED: DisplaySettings = DisplaySettings {
        hor_plus: true,
        hud_anchor: true,
        max_aspect: 0.0,
        render_scale: 1.0,
        msaa: 4,
        filter: TextureFilter::Anisotropic,
        anisotropy: 16,
        perspective_correct: true,
        draw_distance: 1.0,
        fog: false,
        modern_font: true,
    };

    /// The enhanced defaults with `d`'s overrides applied.
    pub fn enhanced(d: &Display) -> DisplaySettings {
        let e = Self::ENHANCED;
        DisplaySettings {
            hor_plus: d.hor_plus.unwrap_or(e.hor_plus),
            hud_anchor: d.hud_anchor.unwrap_or(e.hud_anchor),
            max_aspect: d.max_aspect.unwrap_or(e.max_aspect).max(0.0),
            render_scale: d.render_scale.unwrap_or(e.render_scale).clamp(0.25, 4.0),
            msaa: d.msaa.unwrap_or(e.msaa).max(1),
            filter: d.filter.unwrap_or(e.filter),
            anisotropy: d.anisotropy.unwrap_or(e.anisotropy).clamp(1, 16),
            perspective_correct: d.perspective_correct.unwrap_or(e.perspective_correct),
            draw_distance: d.draw_distance.unwrap_or(e.draw_distance).clamp(0.25, 16.0),
            fog: d.fog.unwrap_or(e.fog),
            modern_font: d.modern_font.unwrap_or(e.modern_font),
        }
    }
}

/// Volumes in the game's 0..=100 units (`globalSoundVol`/`globalMusicVol`), plus a master
/// gain and a mute switch (muted = null audio output, e.g. for background parity runs).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Audio {
    pub master: u8,
    pub music: u8,
    pub sfx: u8,
    pub muted: bool,
    /// Silence the game while its window is not focused.
    pub mute_unfocused: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Input {
    /// `Control 1`..`Control 4` registry values.
    pub controllers: [Controller; 4],
    /// The 56-entry keymap, in original order.
    pub keymap: Vec<KeyBinding>,
    /// Multiple bindings per action (`[[input.bindings]]`: keys, pad buttons, stick
    /// directions). Empty = derive from `keymap` + the retail pad mapping, so old configs
    /// load unchanged. When present it wins over `keymap` (which is kept in sync with the
    /// first key of each action on save).
    pub bindings: Vec<BindingEntry>,
    /// Stick dead zone, 0..1 (retail: 0.5).
    pub deadzone: f32,
    /// Hop input buffer in frames (enhanced profile only): `None` = profile default
    /// ([`Config::hop_buffer_frames`]), `Some(0)` = off.
    pub hop_buffer_frames: Option<u32>,
    /// Pad rumble on/off and intensity (0..1).
    pub rumble: bool,
    pub rumble_scale: f32,
    // controller: PS1-layout pad options (docs/CONTROLLERS.md).
    /// Left stick as d-pad: `fourway` (one direction, hysteresis) or `raw` (PC retail).
    pub stick_mode: crate::controller::StickMode,
    /// `positional` (PS1 layout by position) or `nintendolabels` (Nintendo A confirms).
    pub face_layout: crate::controller::FaceLayout,
    /// Remembered pad per player (device id from `Gamepads::device_id`; empty = none), so a
    /// replugged pad returns to its player.
    pub pad_devices: [String; 4],
}

/// Hop buffer window of the enhanced profile when not overridden.
pub const ENHANCED_HOP_BUFFER_FRAMES: u32 = 6;

impl Default for Video {
    fn default() -> Self {
        Video {
            width: 640,
            height: 480,
            fullscreen: false,
            window_mode: WindowMode::Windowed,
            fullscreen_type: FullscreenType::Borderless,
            resolution: None,
            refresh_mhz: None,
            window_pos: None,
            vsync: true,
            frame_cap: 0,
            brightness: 100,
            show_fps: FpsCorner::Off,
        }
    }
}

impl Video {
    /// The original game's largest resolution (its setup offered 640x480 up to 1024x768, 4:3).
    pub const ORIGINAL_MAX: [u32; 2] = [1024, 768];

    /// What a desktop install starts with: fullscreen (a borderless window over the whole
    /// screen) showing the game at [`Video::ORIGINAL_MAX`], scaled up with black bars at the
    /// sides. Other hosts keep [`Video::default`] (their own screen size).
    pub fn desktop() -> Video {
        let [width, height] = Self::ORIGINAL_MAX;
        Video { width, height, fullscreen: true, window_mode: WindowMode::Borderless, resolution: Some(Self::ORIGINAL_MAX), ..Video::default() }
    }
}

impl Default for Audio {
    fn default() -> Self {
        Audio { master: 100, music: 100, sfx: 100, muted: false, mute_unfocused: false }
    }
}

impl Default for Input {
    fn default() -> Self {
        Input {
            controllers: [Controller::Keyboard; 4],
            keymap: Keymap::default().to_bindings(),
            bindings: Vec::new(),
            deadzone: DEFAULT_DEADZONE,
            hop_buffer_frames: None,
            rumble: true,
            rumble_scale: 1.0,
            stick_mode: Default::default(),
            face_layout: Default::default(),
            pad_devices: Default::default(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            profile: Profile::default(),
            video: Video::default(),
            display: Display::default(),
            audio: Audio::default(),
            language: "us".into(),
            data_root: None,
            rendition: None,
            input: Input::default(),
            mods: Mods::default(),
            gameplay: Gameplay::default(),
            launcher: LauncherCfg::default(),
            upscale: Upscale::default(),
        }
    }
}

impl Config {
    pub fn from_toml(s: &str) -> Result<Config, String> {
        let mut c: Config = toml::from_str(s).map_err(|e| e.to_string())?;
        c.video.migrate();
        Ok(c)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("config serialises")
    }

    /// Loads `path`; a missing file gives the defaults.
    pub fn load(path: &Path) -> Result<Config, String> {
        match std::fs::read_to_string(path) {
            Ok(s) => Config::from_toml(&s).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Loads the user's `config.toml` (defaults if absent).
    pub fn load_default() -> Result<Config, String> {
        Config::load(&crate::paths::config_file())
    }

    /// The defaults of a desktop install ([`Video::desktop`]).
    pub fn desktop() -> Config {
        Config { video: Video::desktop(), ..Config::default() }
    }

    /// [`Config::load_default`] for the desktop app and launcher: a missing file gives
    /// [`Config::desktop`].
    pub fn load_desktop() -> Result<Config, String> {
        let path = crate::paths::config_file();
        if !path.exists() {
            return Ok(Config::desktop());
        }
        Config::load(&path)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        std::fs::write(path, self.to_toml()).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn save_default(&self) -> Result<(), String> {
        self.save(&crate::paths::config_file())
    }

    pub fn data_root(&self) -> PathBuf {
        self.data_root.clone().unwrap_or_else(crate::paths::default_data_root)
    }

    /// The mods directory (`[mods] dir`, else [`crate::paths::default_mods_dir`]).
    pub fn mods_dir(&self) -> PathBuf {
        self.mods.dir.clone().unwrap_or_else(crate::paths::default_mods_dir)
    }

    /// The keymap; an invalid one falls back to the defaults (like a rubbish registry value).
    pub fn keymap(&self) -> Keymap {
        Keymap::from_bindings(&self.input.keymap).unwrap_or_default()
    }

    pub fn set_keymap(&mut self, k: &Keymap) {
        self.input.keymap = k.to_bindings();
    }

    /// Display settings of `profile`: authentic is fixed (parity), enhanced takes the
    /// `[display]` overrides.
    pub fn display_settings(&self, profile: Profile) -> DisplaySettings {
        match profile {
            Profile::Authentic => DisplaySettings::AUTHENTIC,
            Profile::Enhanced => DisplaySettings::enhanced(&self.display),
        }
    }

    /// All bindings: `[[input.bindings]]` if present and valid, else the legacy `keymap`
    /// with the default pad mapping.
    pub fn bindings(&self) -> Bindings {
        if self.input.bindings.is_empty() {
            return Bindings::from_keymap(&self.keymap());
        }
        Bindings::from_entries(&self.input.bindings).unwrap_or_else(|_| Bindings::from_keymap(&self.keymap()))
    }

    pub fn set_bindings(&mut self, b: &Bindings) {
        self.input.bindings = b.to_entries();
        self.input.keymap = b.to_keymap().to_bindings();
    }

    /// Hop buffer window for `profile`: always off in authentic.
    pub fn hop_buffer_frames(&self, profile: Profile) -> u32 {
        match profile {
            Profile::Authentic => 0,
            Profile::Enhanced => self.input.hop_buffer_frames.unwrap_or(ENHANCED_HOP_BUFFER_FRAMES),
        }
    }

    pub fn input_mapper(&self) -> InputMapper {
        let b = self.bindings();
        InputMapper { keymap: b.to_keymap(), extra_keys: b.extra_keys(), controllers: self.input.controllers, ..InputMapper::default() }
    }
}

/// A video mode of the window's monitor (`window::video_modes`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoMode {
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_and_display_overrides() {
        let c = Config::from_toml("profile = \"authentic\"\n[display]\nmsaa = 1\nfilter = \"nearest\"\n").unwrap();
        assert_eq!(c.profile, Profile::Authentic);
        // Authentic ignores the overrides.
        assert_eq!(c.display_settings(Profile::Authentic), DisplaySettings::AUTHENTIC);
        let e = c.display_settings(Profile::Enhanced);
        assert_eq!((e.msaa, e.filter, e.hor_plus), (1, TextureFilter::Nearest, true));
        assert_eq!(Config::default().profile, Profile::Enhanced);
        let back = Config::from_toml(&c.to_toml()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn desktop_defaults_are_fullscreen_at_the_original_resolution() {
        let c = Config::desktop();
        assert_eq!((c.video.window_mode, c.video.resolution, c.video.fullscreen), (WindowMode::Borderless, Some([1024, 768]), true));
        // Leaving fullscreen gives a window of that size.
        let mut v = c.video.clone();
        v.toggle_fullscreen();
        assert_eq!(v.window_mode, WindowMode::Windowed);
        assert_eq!(Config::from_toml(&c.to_toml()).unwrap(), c);
        // Other hosts keep their screen size.
        assert_eq!(Config::default().video.resolution, None);
    }

    #[test]
    fn window_mode_toggle_state_machine() {
        let mut v = Video::default();
        assert_eq!((v.window_mode, v.fullscreen_type), (WindowMode::Windowed, FullscreenType::Borderless));
        // Default: Alt+Enter goes windowed <-> borderless.
        v.toggle_fullscreen();
        assert_eq!(v.window_mode, WindowMode::Borderless);
        assert!(v.fullscreen);
        v.toggle_fullscreen();
        assert_eq!(v.window_mode, WindowMode::Windowed);
        // Choosing exclusive fullscreen in the settings makes the toggle use it.
        v.select_mode(WindowMode::Fullscreen);
        assert_eq!(v.fullscreen_type, FullscreenType::Exclusive);
        v.toggle_fullscreen();
        assert_eq!(v.window_mode, WindowMode::Windowed);
        v.toggle_fullscreen();
        assert_eq!(v.window_mode, WindowMode::Fullscreen);
        // Choosing windowed keeps the type; choosing borderless resets it.
        v.select_mode(WindowMode::Windowed);
        assert_eq!(v.fullscreen_type, FullscreenType::Exclusive);
        v.select_mode(WindowMode::Borderless);
        v.toggle_fullscreen();
        v.toggle_fullscreen();
        assert_eq!(v.window_mode, WindowMode::Borderless);
        // The real window state wins (e.g. the OS left fullscreen).
        assert!(v.observe_mode(WindowMode::Windowed));
        assert!(!v.observe_mode(WindowMode::Windowed));
        assert_eq!((v.window_mode, v.fullscreen), (WindowMode::Windowed, false));
        // Legacy `fullscreen = true`.
        let c = Config::from_toml("[video]\nfullscreen = true\n").unwrap();
        assert_eq!(c.video.window_mode, WindowMode::Borderless);
    }

    #[test]
    fn settings_persist_round_trip() {
        let mut c = Config::default();
        c.video.select_mode(WindowMode::Fullscreen);
        c.video.resolution = Some([1920, 1080]);
        c.video.refresh_mhz = Some(144_000);
        c.video.window_pos = Some([-10, 20]);
        c.video.width = 1280;
        c.video.height = 720;
        c.video.brightness = 120;
        c.video.show_fps = FpsCorner::BottomLeft;
        c.video.frame_cap = 144;
        c.audio.mute_unfocused = true;
        c.audio.master = 40;
        c.display.render_scale = Some(1.5);
        c.language = "de".into();
        c.input.rumble_scale = 0.5;
        let path = std::env::temp_dir().join(format!("f2-settings-{}.toml", std::process::id()));
        c.save(&path).unwrap();
        let back = Config::load(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back, c);
        // A config without the new keys loads with the defaults.
        let old = Config::from_toml("[video]\nwidth = 800\nheight = 600\nvsync = false\n").unwrap();
        assert_eq!((old.video.window_mode, old.video.resolution, old.video.brightness, old.video.show_fps), (WindowMode::Windowed, None, 100, FpsCorner::Off));
    }

    #[test]
    fn bindings_config_compat() {
        // Legacy config (no [[input.bindings]]) loads as the authentic defaults.
        let c = Config::default();
        assert_eq!(c.bindings(), Bindings::default());
        assert_eq!(c.input_mapper().keymap, Keymap::default());
        assert_eq!(c.hop_buffer_frames(Profile::Authentic), 0);
        assert_eq!(c.hop_buffer_frames(Profile::Enhanced), ENHANCED_HOP_BUFFER_FRAMES);
        // Round trip with extra bindings.
        let mut c = Config::default();
        let mut b = c.bindings();
        b.add(0, crate::input::PAD_UP, crate::bindings::Binding::Key(crate::dik::W));
        c.set_bindings(&b);
        c.input.deadzone = 0.25;
        let back = Config::from_toml(&c.to_toml()).unwrap();
        assert_eq!(back, c);
        assert_eq!(back.bindings(), b);
        assert_eq!(back.input_mapper().extra_keys.len(), 1);
        // A file with only the old fields parses.
        let old = Config::from_toml("[input]\ncontrollers = [\"keyboard\", \"keyboard\", \"keyboard\", \"keyboard\"]\n").unwrap();
        assert_eq!(old.bindings(), Bindings::default());
    }
}
