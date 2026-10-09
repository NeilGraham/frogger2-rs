//! `config.toml`: replaces the registry settings and the launcher config dialog.

use crate::input::{Controller, InputMapper, KeyBinding, Keymap};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub video: Video,
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
        Video { width: 640, height: 480, fullscreen: false, vsync: true }
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
        Config { video: Video::default(), audio: Audio::default(), language: "us".into(), data_root: None, rendition: None, input: Input::default() }
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

    pub fn input_mapper(&self) -> InputMapper {
        InputMapper { keymap: self.keymap(), controllers: self.input.controllers, ..InputMapper::default() }
    }
}
