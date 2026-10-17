//! The "Frogger 2: Swampy's Revenge" shortcut the launcher makes once the game is imported,
//! wherever the player chooses (a save dialog that starts on the desktop). It points at the
//! game's own executable (`frogger2(.exe)` in the install folder), so it starts what the
//! player chose to open on start (the game or the launcher).
//!
//! - Windows: a `.lnk` (made by PowerShell through `WScript.Shell`) with `frogger2.exe`'s own
//!   icon (the game's, see setup.rs).
//! - Linux: a `.desktop` launcher with an icon cut from the player's artwork beside the
//!   executable. Inside an AppImage it points at the AppImage.
//! - macOS: a link to the app.
//!
//! Windows and macOS file names cannot hold a `:`, so the suggested file name uses the
//! look-alike U+A789 (MODIFIER LETTER COLON); the name shown inside a shortcut is the real one.

use std::path::{Path, PathBuf};

/// What the shortcut is called.
pub const NAME: &str = "Frogger 2: Swampy's Revenge";

/// The suggested file name, with its extension (`:` is not allowed on Windows and shows as
/// `/` in the macOS Finder).
pub fn file_name() -> String {
    if cfg!(windows) {
        format!("{}.lnk", NAME.replace(':', "\u{a789}"))
    } else if cfg!(target_os = "macos") {
        NAME.replace(':', "\u{a789}")
    } else {
        format!("{NAME}.desktop")
    }
}

fn env_dir(k: &str) -> Option<PathBuf> {
    std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn home() -> Option<PathBuf> {
    env_dir("HOME").or_else(|| env_dir("USERPROFILE"))
}

/// The user's desktop folder (Linux: `xdg-user-dir DESKTOP`), else the home folder.
pub fn desktop_dir() -> Option<PathBuf> {
    let from_xdg = if cfg!(target_os = "linux") {
        std::process::Command::new("xdg-user-dir")
            .arg("DESKTOP")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
            .filter(|p| p.is_absolute())
    } else {
        None
    };
    let desk = from_xdg.or_else(|| home().map(|h| h.join("Desktop"))).filter(|d| d.is_dir());
    desk.or_else(home)
}

/// Whether this system gets a shortcut.
pub fn supported() -> bool {
    cfg!(any(windows, target_os = "linux", target_os = "macos"))
}

/// Asks where to put the shortcut (the system's save dialog, starting on the desktop with
/// [`file_name`]); `None` when cancelled or no dialog is available. Blocks: call it off the
/// UI thread.
pub fn ask_path() -> Option<PathBuf> {
    let pick = |out: std::io::Result<std::process::Output>| -> Option<PathBuf> {
        let o = out.ok()?;
        let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
        (o.status.success() && !s.is_empty()).then(|| PathBuf::from(s))
    };
    let name = file_name();
    let p = if cfg!(windows) {
        // UTF-8 output so the look-alike colon survives the trip back.
        let script = format!(
            "[Console]::OutputEncoding = [Text.Encoding]::UTF8; Add-Type -AssemblyName System.Windows.Forms; \
             $d = New-Object System.Windows.Forms.SaveFileDialog; $d.Title = {}; \
             $d.InitialDirectory = [Environment]::GetFolderPath('Desktop'); $d.FileName = {}; \
             $d.Filter = 'Shortcut|*.lnk'; $d.DefaultExt = 'lnk'; $d.AddExtension = $true; \
             if ($d.ShowDialog() -eq 'OK') {{ $d.FileName }}",
            ps_quote(&format!("Where should the \"{NAME}\" shortcut go?")),
            ps_quote(&name)
        );
        let mut cmd = std::process::Command::new("powershell.exe");
        cmd.args(["-NoProfile", "-STA", "-EncodedCommand", &encode_ps(&script)]);
        crate::setup::no_window(&mut cmd);
        pick(cmd.output())
    } else if cfg!(target_os = "macos") {
        let script = format!(
            "POSIX path of (choose file name with prompt \"Where should the shortcut go?\" default name \"{}\" default location (path to desktop folder))",
            name.replace('"', "\\\"")
        );
        pick(std::process::Command::new("osascript").args(["-e", &script]).output())
    } else {
        let start = desktop_dir().unwrap_or_default().join(&name);
        let zen = pick(
            std::process::Command::new("zenity")
                .args(["--file-selection", "--save", "--confirm-overwrite", "--title=Where should the shortcut go?"])
                .arg(format!("--filename={}", start.display()))
                .output(),
        );
        zen.or_else(|| pick(std::process::Command::new("kdialog").arg("--getsavefilename").arg(&start).arg("*.desktop").output()))
    }?;
    Some(with_extension(p))
}

/// `p` with the shortcut file extension of this system (a name typed without it).
pub fn with_extension(p: PathBuf) -> PathBuf {
    let ext = if cfg!(windows) {
        "lnk"
    } else if cfg!(target_os = "macos") {
        return p;
    } else {
        "desktop"
    };
    if p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)) {
        p
    } else {
        let mut s = p.into_os_string();
        s.push(format!(".{ext}"));
        PathBuf::from(s)
    }
}

/// The macOS app bundle holding `exe`, if any.
fn app_bundle(exe: &Path) -> Option<PathBuf> {
    exe.ancestors().find(|a| a.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf)
}

/// Makes (or replaces) the shortcut at `dest` to `exe` (the game's executable); `icon` is the
/// game's artwork for systems that need a picture file.
pub fn create(exe: &Path, dest: &Path, icon: Option<&crate::icon::Rgba>) -> Result<(), String> {
    let exe = std::fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    if cfg!(windows) {
        create_windows(&exe, &dir, dest)
    } else if cfg!(target_os = "macos") {
        let target = app_bundle(&exe).unwrap_or(exe);
        let _ = std::fs::remove_file(dest);
        symlink(&target, dest).map_err(|e| format!("{}: {e}", dest.display()))
    } else {
        // Inside an AppImage the executable lives in a temporary mount: point at the AppImage.
        let target = env_dir("APPIMAGE").unwrap_or(exe);
        let icon_path = match icon {
            Some(i) => {
                let p = dir.join("frogger2.png");
                std::fs::write(&p, crate::icon::encode_png(&crate::icon::square(i, 256))).map_err(|e| format!("{}: {e}", p.display()))?;
                Some(p)
            }
            None => None,
        };
        std::fs::write(dest, desktop_entry(&target, &dir, icon_path.as_deref())).map_err(|e| format!("{}: {e}", dest.display()))?;
        set_executable(dest);
        // GNOME runs desktop launchers only once they are marked trusted.
        let _ = std::process::Command::new("gio").args(["set"]).arg(dest).args(["metadata::trusted", "true"]).output();
        Ok(())
    }
}

/// A freedesktop.org `.desktop` entry starting `exe` in `dir`.
pub fn desktop_entry(exe: &Path, dir: &Path, icon: Option<&Path>) -> String {
    let quote = |p: &Path| format!("\"{}\"", p.display().to_string().replace('\\', "\\\\").replace('"', "\\\"").replace('`', "\\`").replace('$', "\\$"));
    let mut s = format!(
        "[Desktop Entry]\nType=Application\nName={NAME}\nComment=Frogger 2: Swampy's Revenge (Rust port)\nExec={}\nPath={}\nTerminal=false\nCategories=Game;ArcadeGame;\n",
        quote(exe),
        dir.display()
    );
    if let Some(i) = icon {
        s.push_str(&format!("Icon={}\n", i.display()));
    }
    s
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
fn symlink(_: &Path, _: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other("not supported"))
}

#[cfg(unix)]
fn set_executable(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755));
}

#[cfg(not(unix))]
fn set_executable(_: &Path) {}

/// PowerShell text for a single-quoted string.
fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// A script for `powershell -EncodedCommand` (UTF-16LE, base64): keeps non-ASCII text and
/// quotes intact.
fn encode_ps(script: &str) -> String {
    let utf16: Vec<u8> = script.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    base64(&utf16)
}

/// The PowerShell script making the `.lnk` at `dest`. `WScript.Shell` saves only file names
/// of the ANSI code page, so it is saved under a plain name beside it and then renamed.
pub fn windows_script(exe: &Path, dir: &Path, dest: &Path) -> String {
    let folder = dest.parent().unwrap_or(Path::new("."));
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         $sh = New-Object -ComObject WScript.Shell\n\
         $tmp = Join-Path {folder} 'frogger2-rs-shortcut.lnk'\n\
         $l = $sh.CreateShortcut($tmp)\n\
         $l.TargetPath = {exe}\n\
         $l.WorkingDirectory = {dir}\n\
         $l.IconLocation = {icon}\n\
         $l.Description = {name}\n\
         $l.Save()\n\
         Move-Item -LiteralPath $tmp -Destination {dest} -Force\n",
        folder = ps_quote(&folder.display().to_string()),
        dest = ps_quote(&dest.display().to_string()),
        exe = ps_quote(&exe.display().to_string()),
        dir = ps_quote(&dir.display().to_string()),
        icon = ps_quote(&format!("{},0", exe.display())),
        name = ps_quote(NAME),
    )
}

fn create_windows(exe: &Path, dir: &Path, dest: &Path) -> Result<(), String> {
    // A path from `canonicalize` starts with `\\?\`, which shortcuts do not understand.
    let strip = |p: &Path| PathBuf::from(p.display().to_string().trim_start_matches(r"\\?\"));
    let script = windows_script(&strip(exe), &strip(dir), dest);
    let mut cmd = std::process::Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &encode_ps(&script)]);
    crate::setup::no_window(&mut cmd);
    let out = cmd.output().map_err(|e| format!("running PowerShell: {e}"))?;
    if !out.status.success() {
        return Err(format!("making the shortcut failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(())
}

/// Standard base64 with padding.
pub fn base64(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn windows_script_quotes_paths() {
        let s = windows_script(
            Path::new(r"C:\Games\Bob's frogger2-rs\frogger2.exe"),
            Path::new(r"C:\Games\Bob's frogger2-rs"),
            Path::new("C:\\Users\\Bob\\Desktop\\Frogger 2\u{a789} Swampy's Revenge.lnk"),
        );
        assert!(s.contains(r"$l.TargetPath = 'C:\Games\Bob''s frogger2-rs\frogger2.exe'"), "{s}");
        assert!(s.contains("$l.Description = 'Frogger 2: Swampy''s Revenge'"), "{s}");
        assert!(s.contains("Move-Item -LiteralPath $tmp -Destination 'C:\\Users\\Bob\\Desktop\\Frogger 2\u{a789} Swampy''s Revenge.lnk'"), "{s}");
    }

    #[test]
    fn desktop_entry_has_the_name_and_quotes_exec() {
        let e = desktop_entry(
            Path::new("/home/a b/frogger2-rs/frogger2"),
            Path::new("/home/a b/frogger2-rs"),
            Some(Path::new("/home/a b/frogger2-rs/frogger2.png")),
        );
        assert!(e.contains("\nName=Frogger 2: Swampy's Revenge\n"));
        assert!(e.contains("\nExec=\"/home/a b/frogger2-rs/frogger2\"\n"));
        assert!(e.contains("\nIcon=/home/a b/frogger2-rs/frogger2.png\n"));
    }

    #[test]
    fn file_names_and_extensions() {
        let f = file_name();
        if cfg!(any(windows, target_os = "macos")) {
            assert!(!f.contains(':') && f.contains('\u{a789}'));
        } else {
            assert_eq!(f, "Frogger 2: Swampy's Revenge.desktop");
            assert_eq!(with_extension(PathBuf::from("/d/Frog")), PathBuf::from("/d/Frog.desktop"));
            assert_eq!(with_extension(PathBuf::from("/d/Frog.desktop")), PathBuf::from("/d/Frog.desktop"));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn creates_a_desktop_launcher() {
        let t = std::env::temp_dir().join(format!("f2-shortcut-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        std::fs::create_dir_all(t.join("frogger2-rs")).unwrap();
        std::fs::write(t.join("frogger2-rs/frogger2"), b"#!/bin/sh\n").unwrap();
        let art = crate::icon::Rgba { width: 2, height: 2, pixels: vec![255; 16] };
        let dest = t.join("Desktop/Frogger 2: Swampy's Revenge.desktop");
        create(&t.join("frogger2-rs/frogger2"), &dest, Some(&art)).unwrap();
        let entry = std::fs::read_to_string(&dest).unwrap();
        assert!(entry.contains("Name=Frogger 2: Swampy's Revenge") && entry.contains("frogger2.png"), "{entry}");
        assert!(t.join("frogger2-rs/frogger2.png").is_file());
        let _ = std::fs::remove_dir_all(&t);
    }

    #[cfg(windows)]
    #[test]
    fn creates_a_windows_shortcut() {
        let t = std::env::temp_dir().join(format!("f2-shortcut-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        std::fs::create_dir_all(&t).unwrap();
        let exe = std::env::current_exe().unwrap();
        let dest = t.join(file_name());
        create(&exe, &dest, None).unwrap();
        assert!(dest.is_file(), "{}", dest.display());
        assert!(!t.join("frogger2-rs-shortcut.lnk").exists());
        let _ = std::fs::remove_dir_all(&t);
    }
}
