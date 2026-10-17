//! The game's own executable, made beside the launcher once the player's game is imported.
//!
//! A release ships `launcher` and the real binaries in `engine/`. After the import the
//! launcher writes `frogger2(.exe)` into the install folder: on Windows a copy of the engine
//! exe whose icon and version strings are the game's own (cut from the player's imported
//! artwork, exactly as the Windows export does), on Linux a link to the engine. The artwork
//! never leaves the player's machine and is not part of any release.

use crate::data::Data;
use std::path::{Path, PathBuf};

/// `frogger2` / `frogger2.exe`.
pub fn game_exe_name() -> String {
    f2_launcher::install::exe_name("frogger2")
}

/// `<install>/frogger2(.exe)` for the engine in `<install>/engine/`, created or refreshed when
/// missing or older than the engine. An install that has no `engine/` folder (a flat
/// layout, a development build) already has its game executable: returned as is.
pub fn ensure_game_exe(install: &Path, data: &Data, log: &dyn Fn(&str)) -> Result<PathBuf, String> {
    let engine = f2_launcher::install::engine_exe(install).ok_or_else(|| format!("no {} in {}", game_exe_name(), install.display()))?;
    let target = install.join(game_exe_name());
    if engine == target || is_fresh(&engine, &target) {
        return Ok(target);
    }
    log(&format!("Making {} from {}", target.display(), engine.display()));
    if cfg!(windows) {
        let b = crate::export::branding(data)?;
        make_branded_copy(&engine, &target, &|exe| f2_package::rebrand_windows_exe(exe, &b.title, &b.version, &b.icon))?;
    } else {
        link_or_copy(&engine, &target)?;
    }
    Ok(target)
}

fn mtime(p: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// `target` exists and is not older than `engine`.
fn is_fresh(engine: &Path, target: &Path) -> bool {
    matches!((mtime(engine), mtime(target)), (Some(e), Some(t)) if t >= e)
        && std::fs::symlink_metadata(target).is_ok_and(|m| m.len() > 0 || m.file_type().is_symlink())
}

/// Turns the engine exe's bytes into the game's own exe.
pub type Rebrand<'a> = &'a dyn Fn(&[u8]) -> Result<Vec<u8>, String>;

/// Writes `rebrand(engine bytes)` to `target` through a temporary file (a running game's exe
/// cannot be replaced on Windows: the error says so).
pub fn make_branded_copy(engine: &Path, target: &Path, rebrand: Rebrand) -> Result<(), String> {
    let bytes = std::fs::read(engine).map_err(|e| format!("{}: {e}", engine.display()))?;
    let out = rebrand(&bytes)?;
    let tmp = target.with_extension("new");
    std::fs::write(&tmp, out).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("replacing {} ({e}); close the game first", target.display())
    })
}

#[cfg(unix)]
fn link_or_copy(engine: &Path, target: &Path) -> Result<(), String> {
    let _ = std::fs::remove_file(target);
    let rel = engine.strip_prefix(target.parent().unwrap_or(Path::new("."))).unwrap_or(engine);
    std::os::unix::fs::symlink(rel, target).or_else(|_| std::fs::copy(engine, target).map(|_| ())).map_err(|e| format!("{}: {e}", target.display()))
}

#[cfg(not(unix))]
fn link_or_copy(engine: &Path, target: &Path) -> Result<(), String> {
    std::fs::copy(engine, target).map(|_| ()).map_err(|e| format!("{}: {e}", target.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let t = std::env::temp_dir().join(format!("f2-setup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        std::fs::create_dir_all(t.join("engine")).unwrap();
        t
    }

    #[test]
    fn branded_copy_is_written_through_the_rebrander() {
        let t = tmp("brand");
        std::fs::write(t.join("engine/frogger2.exe"), b"abc").unwrap();
        make_branded_copy(&t.join("engine/frogger2.exe"), &t.join("frogger2.exe"), &|b| Ok([b, b"!"].concat())).unwrap();
        assert_eq!(std::fs::read(t.join("frogger2.exe")).unwrap(), b"abc!");
        assert!(!t.join("frogger2.new").exists());
        assert!(make_branded_copy(&t.join("engine/frogger2.exe"), &t.join("x.exe"), &|_| Err("bad".into())).is_err());
        let _ = std::fs::remove_dir_all(&t);
    }

    #[cfg(unix)]
    #[test]
    fn linux_gets_a_link_to_the_engine() {
        let t = tmp("link");
        std::fs::write(t.join("engine/frogger2"), b"elf").unwrap();
        link_or_copy(&t.join("engine/frogger2"), &t.join("frogger2")).unwrap();
        assert_eq!(std::fs::read(t.join("frogger2")).unwrap(), b"elf");
        assert_eq!(std::fs::read_link(t.join("frogger2")).unwrap(), Path::new("engine/frogger2"));
        assert!(is_fresh(&t.join("engine/frogger2"), &t.join("frogger2")));
        let _ = std::fs::remove_dir_all(&t);
    }
}
