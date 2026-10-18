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

/// The install folder of the program at `exe`: the portable folder, else the folder holding
/// it, leaving `engine/`, `versions/<tag>/` and a macOS `.app` (a version the launcher
/// installed runs with the outer folder's game, settings and versions).
pub fn install_root_for(exe: &Path) -> PathBuf {
    if let Some(r) = f2_platform::paths::portable_root() {
        return r;
    }
    let mut dir = exe.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    if dir.ends_with("Contents/MacOS") {
        if let Some(p) = dir.ancestors().nth(3) {
            dir = p.to_path_buf();
        }
    }
    if dir.file_name().is_some_and(|n| n == "engine") {
        dir = dir.parent().map(Path::to_path_buf).unwrap_or(dir);
    }
    match dir.parent() {
        Some(v) if v.file_name().is_some_and(|n| n == "versions") => v.parent().map(Path::to_path_buf).unwrap_or(dir),
        _ => dir,
    }
}

/// [`install_root_for`] the running program.
pub fn install_root() -> PathBuf {
    install_root_for(&std::env::current_exe().unwrap_or_default())
}

/// The folder of the version the launcher was told to use (`[launcher] version`): the install
/// folder itself, or `versions/<tag>/` once that is installed.
pub fn version_dir(root: &Path, tag: &str) -> PathBuf {
    let d = f2_launcher::install::version_dir(root, tag);
    if !tag.is_empty() && f2_launcher::install::engine_exe(&d).is_some() {
        d
    } else {
        root.to_path_buf()
    }
}

/// The version folder the launcher was told to use, when this program is not already running
/// from it (the install folder itself, or `versions/<tag>/`).
pub fn other_version(root: &Path, tag: &str) -> Option<PathBuf> {
    let want = version_dir(root, tag);
    let me = std::env::current_exe().ok()?;
    let mine = match me.strip_prefix(f2_launcher::install::versions_dir(root)).ok().and_then(|rest| rest.components().next()) {
        Some(t) => f2_launcher::install::version_dir(root, &t.as_os_str().to_string_lossy()),
        None => root.to_path_buf(),
    };
    (want != mine).then_some(want)
}

/// The game program of a version folder: its own `frogger2` (with the game's icon) if made,
/// else the engine.
pub fn game_exe(dir: &Path) -> Option<PathBuf> {
    Some(dir.join(game_exe_name())).filter(|p| p.is_file()).or_else(|| f2_launcher::install::engine_exe(dir))
}

/// The launcher of an install folder (beside the programs, or in the macOS `.app`).
pub fn launcher_exe(dir: &Path) -> Option<PathBuf> {
    let name = f2_launcher::install::exe_name("launcher");
    [dir.join(&name), dir.join("Frogger2rs.app/Contents/MacOS").join(&name)].into_iter().find(|p| p.is_file())
}

/// Set on a program started by [`hand_over`], so it never hands over again.
const HANDED_OVER: &str = "F2_HANDED_OVER";

/// Runs `target` instead of this program (with its arguments) when it is a different file and
/// this program was not itself started that way: how the install folder's `launcher` and
/// `frogger2` start the version chosen in the launcher (its own launcher and game, so a new
/// version's launcher takes effect). Returns true when the other program was started.
pub fn hand_over(target: Option<PathBuf>, args: &[std::ffi::OsString]) -> bool {
    let Some(target) = target else { return false };
    if std::env::var_os(HANDED_OVER).is_some() {
        return false;
    }
    let me = std::env::current_exe().ok().and_then(|e| std::fs::canonicalize(e).ok());
    if me.is_some() && std::fs::canonicalize(&target).ok() == me {
        return false;
    }
    match std::process::Command::new(&target).args(args).env(HANDED_OVER, "1").current_dir(target.parent().unwrap_or(Path::new("."))).spawn() {
        Ok(_) => true,
        Err(e) => {
            eprintln!("frogger: starting {}: {e}", target.display());
            false
        }
    }
}

/// Keeps a helper program (`curl`, PowerShell) from opening a console window on Windows (the
/// launcher and the game have none of their own).
pub fn no_window(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

/// Windows: the game and the launcher are window programs (no console window of their own);
/// started from a terminal they print into that terminal.
pub fn attach_parent_console() {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn AttachConsole(process: u32) -> i32;
            fn GetStdHandle(which: u32) -> isize;
        }
        const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
        const STD_ERROR_HANDLE: u32 = -12i32 as u32;
        // SAFETY: plain Win32 calls without pointers; failure (no parent console) is fine.
        unsafe {
            // Redirected output (`frogger2 2> log.txt`) already has a handle: attaching to the
            // console would send the log there instead.
            let h = GetStdHandle(STD_ERROR_HANDLE);
            if h == 0 || h == -1 {
                AttachConsole(ATTACH_PARENT_PROCESS);
            }
        }
    }
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

    #[test]
    fn install_root_leaves_engine_versions_and_apps() {
        if f2_platform::paths::portable_root().is_some() {
            return;
        }
        let r = |p: &str| install_root_for(Path::new(p));
        assert_eq!(r("/g/frogger2-rs/launcher"), Path::new("/g/frogger2-rs"));
        assert_eq!(r("/g/frogger2-rs/engine/frogger2"), Path::new("/g/frogger2-rs"));
        assert_eq!(r("/g/frogger2-rs/versions/v0.9.0/launcher"), Path::new("/g/frogger2-rs"));
        assert_eq!(r("/g/frogger2-rs/versions/v0.9.0/engine/frogger2"), Path::new("/g/frogger2-rs"));
        assert_eq!(r("/g/frogger2-rs/Frogger2rs.app/Contents/MacOS/launcher"), Path::new("/g/frogger2-rs"));
        assert_eq!(r("/g/frogger2-rs/versions/v0.9.0/Frogger2rs.app/Contents/MacOS/launcher"), Path::new("/g/frogger2-rs"));
    }

    #[test]
    fn version_dirs_fall_back_to_the_install_folder() {
        let t = tmp("ver");
        assert_eq!(version_dir(&t, ""), t);
        assert_eq!(version_dir(&t, "v9.9.9"), t, "not installed");
        std::fs::create_dir_all(t.join("versions/v9.9.9/engine")).unwrap();
        std::fs::write(t.join("versions/v9.9.9/engine").join(game_exe_name()), b"x").unwrap();
        std::fs::write(t.join("versions/v9.9.9").join(f2_launcher::install::exe_name("launcher")), b"x").unwrap();
        assert_eq!(version_dir(&t, "v9.9.9"), t.join("versions/v9.9.9"));
        assert_eq!(launcher_exe(&t.join("versions/v9.9.9")), Some(t.join("versions/v9.9.9").join(f2_launcher::install::exe_name("launcher"))));
        assert!(!hand_over(None, &[]));
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
