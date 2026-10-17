//! Per-platform locations. Data root logic mirrors `f2_import::default_data_root()`.

use std::path::{Path, PathBuf};

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


/// Marker file of a portable install: `portable.txt` in the folder that holds the app. The
/// release zips ship it, so everything the app reads and writes (imported game data, settings,
/// saves, mods, downloaded runtimes) stays inside that folder.
pub const PORTABLE_MARKER: &str = "portable.txt";

/// The portable install folder for an exe at `exe`, if there is one: the folder with
/// [`PORTABLE_MARKER`] among, outermost first: `<root>/versions/<tag>[/engine]/` (a version the
/// launcher installed), the folder holding a macOS `.app`, `<root>/engine/` and the exe's own
/// folder. `forced` (`$F2_PORTABLE_ROOT`) wins without a marker.
pub fn portable_root_for(exe: Option<&Path>, forced: Option<PathBuf>) -> Option<PathBuf> {
    if forced.is_some() {
        return forced;
    }
    let dir = exe?.parent()?;
    // Innermost first: the exe's folder, its parent when that is `engine/`, then the folder
    // holding `versions/<tag>`, and the folder holding a macOS `.app`.
    let mut roots = vec![dir.to_path_buf()];
    let mut base = dir;
    if base.file_name().is_some_and(|n| n == "engine") {
        if let Some(p) = base.parent() {
            roots.push(p.to_path_buf());
            base = p;
        }
    }
    if let Some(versions) = base.parent().filter(|v| v.file_name().is_some_and(|n| n == "versions")) {
        roots.extend(versions.parent().map(Path::to_path_buf));
    }
    if dir.ends_with("Contents/MacOS") {
        roots.extend(dir.parent().and_then(Path::parent).and_then(Path::parent).map(Path::to_path_buf));
    }
    roots.into_iter().rev().find(|r| r.join(PORTABLE_MARKER).is_file())
}

/// [`portable_root_for`] the running exe (computed once).
pub fn portable_root() -> Option<PathBuf> {
    static ROOT: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| portable_root_for(std::env::current_exe().ok().as_deref(), env_path("F2_PORTABLE_ROOT"))).clone()
}

/// Imported game data: `$F2_DATA_ROOT`, else `data/` in the portable folder, else
/// `<data base>/frogger2-rs/data`.
pub fn default_data_root() -> PathBuf {
    if let Some(p) = env_path("F2_DATA_ROOT") {
        return p;
    }
    if let Some(r) = portable_root() {
        return r.join("data");
    }
    data_base().unwrap_or_else(|| PathBuf::from(".")).join(APP_DIR).join("data")
}

static CONFIG_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Fixes the settings directory for this process (hosts without environment variables or a
/// home directory, e.g. consoles). Takes precedence over `$F2_CONFIG_DIR`; first call wins.
pub fn set_config_dir(dir: PathBuf) {
    let _ = CONFIG_DIR.set(dir);
}

/// Settings directory: [`set_config_dir`], else `$F2_CONFIG_DIR`, else the portable folder,
/// else `%APPDATA%\frogger2-rs`,
/// `~/Library/Application Support/frogger2-rs`, or `$XDG_CONFIG_HOME`/`~/.config` + `frogger2-rs`.
pub fn config_dir() -> PathBuf {
    if let Some(p) = CONFIG_DIR.get() {
        return p.clone();
    }
    if let Some(p) = env_path("F2_CONFIG_DIR") {
        return p;
    }
    if let Some(r) = portable_root() {
        return r;
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

#[cfg(test)]
mod portable_tests {
    use super::*;

    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "x").unwrap();
    }

    #[test]
    fn finds_the_portable_folder() {
        let t = std::env::temp_dir().join(format!("f2-portable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        // no marker: not portable
        touch(&t.join("a/frogger2.exe"));
        assert_eq!(portable_root_for(Some(&t.join("a/frogger2.exe")), None), None);
        // marker beside the exe
        touch(&t.join("a").join(PORTABLE_MARKER));
        assert_eq!(portable_root_for(Some(&t.join("a/frogger2.exe")), None), Some(t.join("a")));
        // a launcher-installed version uses the root two levels up, even with its own marker
        touch(&t.join("a/versions/v0.2.0/frogger2.exe"));
        touch(&t.join("a/versions/v0.2.0").join(PORTABLE_MARKER));
        assert_eq!(portable_root_for(Some(&t.join("a/versions/v0.2.0/frogger2.exe")), None), Some(t.join("a")));
        // the engine/ folder of the root, and of a version
        touch(&t.join("a/engine/frogger2.exe"));
        assert_eq!(portable_root_for(Some(&t.join("a/engine/frogger2.exe")), None), Some(t.join("a")));
        touch(&t.join("a/versions/v0.2.0/engine/frogger2.exe"));
        assert_eq!(portable_root_for(Some(&t.join("a/versions/v0.2.0/engine/frogger2.exe")), None), Some(t.join("a")));
        // macOS: the marker sits beside the .app
        touch(&t.join("m/F.app/Contents/MacOS/frogger2"));
        touch(&t.join("m").join(PORTABLE_MARKER));
        assert_eq!(portable_root_for(Some(&t.join("m/F.app/Contents/MacOS/frogger2")), None), Some(t.join("m")));
        // forced
        assert_eq!(portable_root_for(None, Some(t.join("z"))), Some(t.join("z")));
        let _ = std::fs::remove_dir_all(&t);
    }
}
