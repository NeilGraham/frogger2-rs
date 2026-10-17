//! The "Frogger 2: Swampy's Revenge" shortcut the launcher makes once the game is imported.
//! It points at the game's own executable (`frogger2(.exe)` in the install folder), so it
//! starts what the player chose to open on start (the game or the launcher).
//!
//! - Windows: `.lnk` files on the desktop and in the Start menu (made by PowerShell through
//!   `WScript.Shell`), with `frogger2.exe`'s own icon (the game's, see setup.rs).
//! - Linux: a `.desktop` entry in the applications menu (and on the desktop when there is
//!   one), with an icon cut from the player's artwork beside the executable. Inside an
//!   AppImage it points at the AppImage.
//! - macOS: a link to the app on the desktop.
//!
//! Windows and macOS file names cannot hold a `:`, so the file names use the look-alike
//! U+A789 (MODIFIER LETTER COLON); the name shown inside a shortcut is the real one.

use std::path::{Path, PathBuf};

/// What the shortcut is called.
pub const NAME: &str = "Frogger 2: Swampy's Revenge";

/// [`NAME`] as a file name (`:` is not allowed on Windows and shows as `/` in the macOS
/// Finder).
pub fn file_name() -> String {
    if cfg!(any(windows, target_os = "macos")) {
        NAME.replace(':', "\u{a789}")
    } else {
        NAME.to_string()
    }
}

fn env_dir(k: &str) -> Option<PathBuf> {
    std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn home() -> Option<PathBuf> {
    env_dir("HOME").or_else(|| env_dir("USERPROFILE"))
}

/// Where the shortcuts go; the first one decides whether "the shortcut exists".
fn places() -> Vec<PathBuf> {
    let name = file_name();
    if cfg!(windows) {
        let mut v = Vec::new();
        if let Some(a) = env_dir("APPDATA") {
            v.push(a.join("Microsoft/Windows/Start Menu/Programs").join(format!("{name}.lnk")));
        }
        v
    } else if cfg!(target_os = "macos") {
        home().map(|h| vec![h.join("Desktop").join(name)]).unwrap_or_default()
    } else {
        let apps = env_dir("XDG_DATA_HOME").or_else(|| home().map(|h| h.join(".local/share"))).map(|d| d.join("applications/frogger2-rs.desktop"));
        let mut v: Vec<PathBuf> = apps.into_iter().collect();
        if let Some(d) = desktop_dir() {
            v.push(d.join("frogger2-rs.desktop"));
        }
        v
    }
}

/// The user's desktop folder on Linux (`xdg-user-dir DESKTOP`, else `~/Desktop`), if it
/// exists (looked up once).
fn desktop_dir() -> Option<PathBuf> {
    static DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(find_desktop_dir).clone()
}

fn find_desktop_dir() -> Option<PathBuf> {
    let from_xdg = std::process::Command::new("xdg-user-dir")
        .arg("DESKTOP")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()));
    let d = from_xdg.filter(|p| p.is_absolute() && Some(p.as_path()) != home().as_deref()).or_else(|| home().map(|h| h.join("Desktop")))?;
    d.is_dir().then_some(d)
}

/// Whether this system gets a shortcut.
pub fn supported() -> bool {
    cfg!(any(windows, target_os = "linux", target_os = "macos")) && !places().is_empty()
}

/// Whether the shortcut was made.
pub fn exists() -> bool {
    places().first().is_some_and(|p| std::fs::symlink_metadata(p).is_ok())
}

/// The macOS app bundle holding `exe`, if any.
fn app_bundle(exe: &Path) -> Option<PathBuf> {
    exe.ancestors().find(|a| a.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf)
}

/// Makes (or remakes) the shortcut to `exe` (the game's executable); `icon` is the game's
/// artwork for systems that need a picture file. Returns where it was put.
pub fn create(exe: &Path, icon: Option<&crate::icon::Rgba>) -> Result<String, String> {
    let exe = std::fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    if cfg!(windows) {
        create_windows(&exe, &dir)
    } else if cfg!(target_os = "macos") {
        let target = app_bundle(&exe).unwrap_or(exe);
        let link = places().into_iter().next().ok_or("no desktop folder")?;
        let _ = std::fs::remove_file(&link);
        symlink(&target, &link).map_err(|e| format!("{}: {e}", link.display()))?;
        Ok("on the desktop".into())
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
        let text = desktop_entry(&target, &dir, icon_path.as_deref());
        let mut put = Vec::new();
        for (n, p) in places().into_iter().enumerate() {
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
            }
            std::fs::write(&p, &text).map_err(|e| format!("{}: {e}", p.display()))?;
            set_executable(&p);
            if n > 0 {
                // GNOME runs desktop launchers only once they are marked trusted.
                let _ = std::process::Command::new("gio").args(["set"]).arg(&p).args(["metadata::trusted", "true"]).output();
            }
            put.push(if n == 0 { "in the applications menu" } else { "on the desktop" });
        }
        Ok(put.join(" and "))
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

/// The PowerShell script making the `.lnk` files (desktop and Start menu). `WScript.Shell`
/// saves only file names of the ANSI code page, so each shortcut is saved under a plain name
/// and then renamed to [`file_name`].
pub fn windows_script(exe: &Path, dir: &Path) -> String {
    let file = format!("{}.lnk", file_name());
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         $sh = New-Object -ComObject WScript.Shell\n\
         $dirs = @([Environment]::GetFolderPath('Desktop'), (Join-Path $env:APPDATA 'Microsoft\\Windows\\Start Menu\\Programs'))\n\
         foreach ($d in $dirs) {{\n\
           if (-not $d) {{ continue }}\n\
           $tmp = Join-Path $d 'frogger2-rs-shortcut.lnk'\n\
           $l = $sh.CreateShortcut($tmp)\n\
           $l.TargetPath = {exe}\n\
           $l.WorkingDirectory = {dir}\n\
           $l.IconLocation = {icon}\n\
           $l.Description = {name}\n\
           $l.Save()\n\
           Move-Item -LiteralPath $tmp -Destination (Join-Path $d {file}) -Force\n\
         }}\n",
        file = ps_quote(&file),
        exe = ps_quote(&exe.display().to_string()),
        dir = ps_quote(&dir.display().to_string()),
        icon = ps_quote(&format!("{},0", exe.display())),
        name = ps_quote(NAME),
    )
}

fn create_windows(exe: &Path, dir: &Path) -> Result<String, String> {
    // A path from `canonicalize` starts with `\\?\`, which shortcuts do not understand.
    let strip = |p: &Path| PathBuf::from(p.display().to_string().trim_start_matches(r"\\?\"));
    let script = windows_script(&strip(exe), &strip(dir));
    // -EncodedCommand (UTF-16LE, base64) keeps the non-ASCII name and any quotes intact.
    let utf16: Vec<u8> = script.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let mut cmd = std::process::Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &base64(&utf16)]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|e| format!("running PowerShell: {e}"))?;
    if !out.status.success() {
        return Err(format!("making the shortcut failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok("on the desktop and in the Start menu".into())
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
        let s = windows_script(Path::new(r"C:\Games\Bob's frogger2-rs\frogger2.exe"), Path::new(r"C:\Games\Bob's frogger2-rs"));
        assert!(s.contains(r"$l.TargetPath = 'C:\Games\Bob''s frogger2-rs\frogger2.exe'"), "{s}");
        assert!(s.contains("$l.Description = 'Frogger 2: Swampy''s Revenge'"), "{s}");
        assert!(s.contains("[Environment]::GetFolderPath('Desktop')"));
        assert!(s.contains("Move-Item -LiteralPath $tmp"), "renamed after saving: {s}");
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
    fn file_names_avoid_the_colon_where_it_is_not_allowed() {
        let f = file_name();
        if cfg!(any(windows, target_os = "macos")) {
            assert!(!f.contains(':') && f.contains('\u{a789}'));
        } else {
            assert_eq!(f, NAME);
        }
    }

    /// Makes the shortcut for real: run alone with `HOME` (and `XDG_DATA_HOME`) pointing at
    /// a scratch folder (`cargo test -p frogger --lib shortcut::tests::creates -- --ignored`).
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore]
    fn creates() {
        let home = env_dir("HOME").unwrap();
        assert!(home.starts_with(std::env::temp_dir()) || home.to_string_lossy().contains("scratch"), "point HOME at a scratch folder");
        let dir = home.join("frogger2-rs");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("frogger2"), b"#!/bin/sh\n").unwrap();
        assert!(supported() && !exists());
        let art = crate::icon::Rgba { width: 2, height: 2, pixels: vec![255; 16] };
        let w = create(&dir.join("frogger2"), Some(&art)).unwrap();
        assert!(exists(), "{w}");
        let entry = std::fs::read_to_string(&places()[0]).unwrap();
        assert!(entry.contains("Name=Frogger 2: Swampy's Revenge") && entry.contains("frogger2.png"), "{entry}");
        assert!(dir.join("frogger2.png").is_file());
    }
}
