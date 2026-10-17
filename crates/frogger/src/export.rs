//! "Export to…" (docs/EXPORT.md): builds ready-to-run packages of the player's imported game
//! for other platforms and for this desktop OS, with `f2-package`.
//!
//! The name and icon come from the player's own disc: the installer's product name
//! (`DATA.TAG` `Application=Frogger2`, recorded by the importer as `disc_app_name`) spaced
//! into "Frogger 2", the game's own subtitle string (`STR_STORYMODE`, "Swampy's Revenge"),
//! and the loading-screen artwork squared with rounded corners.

use crate::data::Data;
use crate::icon;
use f2_formats::lang::Language;
use f2_game::assets::DataSource;
pub use f2_package::{Branding, Image, Target};
use std::path::{Path, PathBuf};

/// Where release runtimes are downloaded from (`F2_RUNTIME_BASE_URL` overrides; `{v}` is
/// the version).
pub const RUNTIME_BASE_URL: &str = "https://github.com/NeilGraham/frogger2-rs/releases/download/v{v}/";

/// This app's version (the runtimes must match it).
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// "Frogger2" -> "Frogger 2": a space between letters and the number that follows them.
pub fn spaced_name(s: &str) -> String {
    let mut out = String::new();
    let mut prev: Option<char> = None;
    for c in s.trim().chars() {
        if c.is_ascii_digit() && prev.is_some_and(|p| p.is_alphabetic()) {
            out.push(' ');
        }
        out.push(c);
        prev = Some(c);
    }
    out
}

/// The disc's product name recorded at import (`manifest.json` `disc_app_name`), spaced.
pub fn short_title(rendition: &Path) -> Option<String> {
    let text = std::fs::read_to_string(rendition.join("manifest.json")).ok()?;
    let m: serde_json::Value = serde_json::from_str(&text).ok()?;
    m.get("disc_app_name").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()).map(spaced_name)
}

/// The game's subtitle from its own string table (`STR_STORYMODE`, English).
pub fn subtitle(data: &Data) -> Option<String> {
    let lang = if data.rendition.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("usa")) { Language::Us } else { Language::Uk };
    let table = data.string_table("maps/language.fla")?.ok()?;
    table.get("STR_STORYMODE", lang).or_else(|| table.get("STR_STORYMODE", Language::Uk)).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// The full title from the player's disc ("Frogger 2: Swampy's Revenge"); errors when the
/// import predates `disc_app_name`.
pub fn title(data: &Data) -> Result<String, String> {
    let short = short_title(&data.rendition).ok_or("the imported game has no disc product name (DATA.TAG): re-import your Frogger 2 disc with this version")?;
    Ok(match subtitle(data) {
        Some(sub) => format!("{short}: {sub}"),
        None => short,
    })
}

/// Name and icon for exported builds, from the player's disc. Errors when the data lacks
/// what is needed (re-import the disc with this version to record its product name).
pub fn branding(data: &Data) -> Result<Branding, String> {
    let title = title(data)?;
    let short = short_title(&data.rendition).unwrap_or_else(|| title.clone());
    let art = icon::game_art(data).ok_or_else(|| format!("{} not found in the imported game", icon::GAME_ART))?;
    let square = icon::square(&art, 1024);
    let rounded = icon::rounded(&square, 0.18);
    let img = |i: icon::Rgba| Image { width: i.width, height: i.height, rgba: i.pixels };
    Ok(Branding { title, short_title: short, art: img(square), icon: img(rounded), version: version().to_string() })
}

/// The running app as the runtime of its own desktop OS: Windows the exe's folder, macOS the
/// enclosing `.app`, Linux the AppImage (`$APPIMAGE`) or the binary's folder.
pub fn host_runtime() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("locating this app: {e}"))?;
    host_runtime_for(&exe)
}

/// [`host_runtime`] for the game program at `exe` (the launcher exports with the game it runs).
pub fn host_runtime_for(exe: &Path) -> Result<PathBuf, String> {
    let exe = exe.to_path_buf();
    if cfg!(target_os = "linux") {
        if let Some(a) = std::env::var_os("APPIMAGE") {
            return Ok(PathBuf::from(a));
        }
    }
    if cfg!(target_os = "macos") {
        if let Some(app) = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")) {
            return Ok(app.to_path_buf());
        }
    }
    exe.parent().map(Path::to_path_buf).ok_or_else(|| "this app has no folder".into())
}

/// The runtime for `target`: `explicit` if given, the running app for this OS, else the
/// release asset from the runtime cache (`<config>/runtimes/`), downloaded (with `curl`) if
/// missing.
pub fn runtime(target: Target, explicit: Option<&Path>, log: &dyn Fn(&str)) -> Result<PathBuf, String> {
    if let Some(p) = explicit {
        return if p.exists() { Ok(p.to_path_buf()) } else { Err(format!("runtime {} not found", p.display())) };
    }
    if Some(target) == Target::host() {
        return host_runtime();
    }
    let name = target.runtime_asset(version()).ok_or_else(|| format!("{} can only be exported from {} itself", target.name(), target.name()))?;
    let dir = f2_platform::paths::config_dir().join("runtimes");
    let path = dir.join(&name);
    if path.is_file() {
        return Ok(path);
    }
    let base = std::env::var("F2_RUNTIME_BASE_URL").unwrap_or_else(|_| RUNTIME_BASE_URL.to_string()).replace("{v}", version());
    let url = format!("{}{}", base, name);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    log(&format!("Downloading the {} runtime: {url}", target.name()));
    let part = dir.join(format!("{name}.part"));
    let mut curl = std::process::Command::new("curl");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        curl.creation_flags(0x0800_0000);
    }
    let status = curl
        .args(["-fL", "--retry", "3", "-o"])
        .arg(&part)
        .arg(&url)
        .status()
        .map_err(|e| format!("running curl: {e} (download {url} by hand and pass --runtime, or set F2_RUNTIME_BASE_URL to a mirror)"))?;
    if !status.success() {
        let _ = std::fs::remove_file(&part);
        return Err(format!("downloading {url} failed ({status}); pass --runtime with a local copy, or set F2_RUNTIME_BASE_URL to a mirror"));
    }
    std::fs::rename(&part, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Exports `target` into `out_dir`; returns the written package.
pub fn export(data: &Data, target: Target, runtime_path: Option<&Path>, out_dir: &Path, log: &dyn Fn(&str)) -> Result<PathBuf, String> {
    let branding = branding(data)?;
    log(&format!("Exporting \"{}\" for {}", branding.title, target.name()));
    let runtime = runtime(target, runtime_path, log)?;
    let keys = f2_platform::paths::config_dir().join("keys");
    let job = f2_package::Job { target, rendition: &data.rendition, runtime: &runtime, out_dir, branding: &branding, keys_dir: Some(&keys), log };
    f2_package::export(&job)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(spaced_name("Frogger2"), "Frogger 2");
        assert_eq!(spaced_name("Frogger 2"), "Frogger 2");
        assert_eq!(spaced_name(" Doom64 "), "Doom 64");
    }

    /// With imported data: the branding is "Frogger 2: Swampy's Revenge" / "Frogger 2".
    #[test]
    fn branding_from_disc() {
        let base = std::env::var("F2_GAME_DIR").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../game-data/usa").into());
        let Ok(data) = Data::locate(&f2_platform::Config::default(), Some(base.into())) else {
            eprintln!("no game data: skipped");
            return;
        };
        if short_title(&data.rendition).is_none() {
            eprintln!("imported before disc_app_name existed: skipped");
            return;
        }
        let b = branding(&data).unwrap();
        assert_eq!(b.short_title, "Frogger 2");
        assert_eq!(b.title, "Frogger 2: Swampy's Revenge");
        assert_eq!((b.icon.width, b.icon.height), (1024, 1024));
        assert_eq!(b.icon.rgba[3], 0, "rounded corner");
        assert_eq!(b.art.rgba[3], 255, "art is square");
        if let Some(dir) = std::env::var_os("F2_ICON_PREVIEW") {
            std::fs::write(std::path::Path::new(&dir).join("branding-icon.png"), f2_package::util::png(&b.icon)).unwrap();
        }
    }
}
