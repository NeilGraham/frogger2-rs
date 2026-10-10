//! Per-platform locations. Data root logic mirrors `f2_import::default_data_root()`.

use std::path::PathBuf;

pub const APP_DIR: &str = "frogger2-rs";

fn env_path(k: &str) -> Option<PathBuf> {
    std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn home() -> Option<PathBuf> {
    env_path("HOME")
}

/// Base of per-user application data: `%APPDATA%`, `~/Library/Application Support`, or
/// `$XDG_DATA_HOME` / `~/.local/share`.
fn data_base() -> Option<PathBuf> {
    if cfg!(windows) {
        env_path("APPDATA")
    } else if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library/Application Support"))
    } else {
        env_path("XDG_DATA_HOME").or_else(|| home().map(|h| h.join(".local/share")))
    }
}

/// Imported game data: `$F2_DATA_ROOT`, else `<data base>/frogger2-rs/data`.
pub fn default_data_root() -> PathBuf {
    if let Some(p) = env_path("F2_DATA_ROOT") {
        return p;
    }
    data_base().unwrap_or_else(|| PathBuf::from(".")).join(APP_DIR).join("data")
}

/// Settings directory: `$F2_CONFIG_DIR`, else `%APPDATA%\frogger2-rs`,
/// `~/Library/Application Support/frogger2-rs`, or `$XDG_CONFIG_HOME`/`~/.config` + `frogger2-rs`.
pub fn config_dir() -> PathBuf {
    if let Some(p) = env_path("F2_CONFIG_DIR") {
        return p;
    }
    let base = if cfg!(windows) || cfg!(target_os = "macos") { data_base() } else { env_path("XDG_CONFIG_HOME").or_else(|| home().map(|h| h.join(".config"))) };
    base.unwrap_or_else(|| PathBuf::from(".")).join(APP_DIR)
}

/// `config.toml` in [`config_dir`].
pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

/// Mods: `$F2_MODS_DIR`, else `mods/` in [`config_dir`].
pub fn default_mods_dir() -> PathBuf {
    env_path("F2_MODS_DIR").unwrap_or_else(|| config_dir().join("mods"))
}

/// Save games and sidecars (next to the config).
pub fn save_dir() -> PathBuf {
    config_dir().join("saves")
}
