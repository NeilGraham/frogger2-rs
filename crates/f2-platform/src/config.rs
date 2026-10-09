//! `config.toml`: replaces the registry settings and the launcher config dialog.

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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Video {
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
    pub vsync: bool,
    /// Frame-rate cap in frames per second (`0` = uncapped; with `vsync` the display rate
    /// caps it anyway). The simulation always ticks at 60 Hz; rendering is interpolated.
    pub frame_cap: u32,
}

/// Behaviour profile (`docs/enhancements.md`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    /// Retail behaviour and look; parity tests and screenshots run in this profile.
    Authentic,
    /// Fixes and QoL (owner decision: the default for players).
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Input {
    /// `Control 1`..`Control 4` registry values.
    pub controllers: [Controller; 4],
    /// The 56-entry keymap, in original order.
    pub keymap: Vec<KeyBinding>,
}

impl Default for Video {
    fn default() -> Self {
        Video { width: 640, height: 480, fullscreen: false, vsync: true, frame_cap: 0 }
    }
}

impl Default for Audio {
    fn default() -> Self {
        Audio { master: 100, music: 100, sfx: 100, muted: false }
    }
}

impl Default for Input {
    fn default() -> Self {
        Input { controllers: [Controller::Keyboard; 4], keymap: Keymap::default().to_bindings() }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            profile: Profile::default(),
            video: Video::default(),
            display: Display::default(),
            audio: Audio::default(), language: "us".into(), data_root: None, rendition: None, input: Input::default() }
    }
}

impl Config {
    pub fn from_toml(s: &str) -> Result<Config, String> {
        toml::from_str(s).map_err(|e| e.to_string())
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

    pub fn input_mapper(&self) -> InputMapper {
        InputMapper { keymap: self.keymap(), controllers: self.input.controllers, ..InputMapper::default() }
    }
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
}
