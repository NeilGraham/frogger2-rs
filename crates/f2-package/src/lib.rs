//! `f2-package`: "Export to…" for the desktop app (docs/EXPORT.md).
//!
//! A build for a target = a **runtime** (the engine for that platform, built by CI and
//! published with each release; it contains no game data) + the player's **imported game**
//! (`<data root>/<rendition>`) + **branding** derived from the player's own disc: the title
//! ("Frogger 2: Swampy's Revenge", short "Frogger 2") and an icon cut from the game's own
//! artwork. Everything happens on the player's machine; the result contains their game data
//! and must not be shared.
//!
//! No compiler or SDK is needed: each module only packs (zip/APK signing, PKG/PFS writing,
//! ROM assembly, app bundles), in pure Rust, so every desktop OS can export every target.

use std::path::{Path, PathBuf};

mod android;
mod linux;
mod macos;
pub mod n64;
mod ps4;
mod ps5;
pub mod util;
mod windows;

/// An RGBA8 image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Name and icon of the exported build, derived from the player's disc (`frogger::export`).
#[derive(Clone, Debug)]
pub struct Branding {
    /// Full title, e.g. "Frogger 2: Swampy's Revenge" (window titles, PS4/PS5 title,
    /// desktop app names).
    pub title: String,
    /// Short title, e.g. "Frogger 2" (Android launcher label, N64 ROM header (20 chars)).
    pub short_title: String,
    /// The game artwork, square, 1024×1024, opaque (for platforms that shape icons
    /// themselves: Android adaptive icons, PS4/PS5 `icon0.png`).
    pub art: Image,
    /// The same with rounded corners (transparent outside), 1024×1024 (desktop icons:
    /// Windows `.ico`, macOS `.icns`, Linux `.desktop` icon, legacy Android icon).
    pub icon: Image,
    /// Version of the app doing the export (e.g. "0.1.0"), used for package versions.
    pub version: String,
}

impl Branding {
    /// [`Branding::title`] as a file/folder name on every OS (`:` and other reserved
    /// characters replaced): "Frogger 2 - Swampy's Revenge".
    pub fn file_stem(&self) -> String {
        util::file_stem(&self.title)
    }
}

/// A Windows executable with its icons replaced by `icon` and its version strings naming
/// `title`; the code is untouched (what the Windows export does, `docs/EXPORT.md`).
pub fn rebrand_windows_exe(exe: &[u8], title: &str, version: &str, icon: &Image) -> Result<Vec<u8>, String> {
    windows::rebrand_exe(exe, title, version, icon)
}

/// A platform to export for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Windows,
    MacOs,
    Linux,
    Android,
    Ps4,
    Ps5,
    N64,
}

impl Target {
    pub const ALL: [Target; 7] = [Target::Windows, Target::MacOs, Target::Linux, Target::Android, Target::Ps4, Target::Ps5, Target::N64];

    /// Command-line / file name id.
    pub fn id(self) -> &'static str {
        match self {
            Target::Windows => "windows",
            Target::MacOs => "macos",
            Target::Linux => "linux",
            Target::Android => "android",
            Target::Ps4 => "ps4",
            Target::Ps5 => "ps5",
            Target::N64 => "n64",
        }
    }

    /// Display name.
    pub fn name(self) -> &'static str {
        match self {
            Target::Windows => "Windows",
            Target::MacOs => "macOS",
            Target::Linux => "Linux (AppImage)",
            Target::Android => "Android (APK)",
            Target::Ps4 => "PlayStation 4 (fPKG)",
            Target::Ps5 => "PlayStation 5 (app folder)",
            Target::N64 => "Nintendo 64 (ROM)",
        }
    }

    pub fn parse(s: &str) -> Option<Target> {
        Target::ALL.into_iter().find(|t| t.id().eq_ignore_ascii_case(s))
    }

    /// The desktop OS this program runs on (it exports its own OS from itself).
    pub fn host() -> Option<Target> {
        if cfg!(windows) {
            Some(Target::Windows)
        } else if cfg!(target_os = "macos") {
            Some(Target::MacOs)
        } else if cfg!(target_os = "linux") {
            Some(Target::Linux)
        } else {
            None
        }
    }

    /// The targets this host can export: consoles, Android and its own desktop OS (built
    /// from the running app itself).
    pub fn exportable() -> Vec<Target> {
        let mut v: Vec<Target> = Target::host().into_iter().collect();
        v.extend([Target::Android, Target::Ps4, Target::Ps5, Target::N64]);
        v
    }

    /// File name of the target's runtime in a GitHub release (docs/EXPORT.md "Runtimes");
    /// `None` for the host desktop OS, whose runtime is the running app.
    pub fn runtime_asset(self, version: &str) -> Option<String> {
        Some(match self {
            Target::Android => format!("frogger2-rs-{version}-android.apk"),
            Target::Ps4 => format!("runtime-ps4-{version}.zip"),
            Target::Ps5 => format!("runtime-ps5-{version}.zip"),
            Target::N64 => format!("runtime-n64-{version}.zip"),
            Target::Windows | Target::MacOs | Target::Linux => return None,
        })
    }
}

/// One export.
pub struct Job<'a> {
    pub target: Target,
    /// The imported rendition: `<data root>/<rendition>` with `game/`, `music/`, `video/`,
    /// `manifest.json`.
    pub rendition: &'a Path,
    /// The target's runtime (docs/EXPORT.md "Runtimes"): an extracted directory or the
    /// release asset file itself (APK / zip). For the host desktop OS: the running app
    /// (Windows: the exe's folder, macOS: the `.app`, Linux: the AppImage file or the
    /// folder holding the `frogger2` binary).
    pub runtime: &'a Path,
    /// Directory the result is written into (created if missing).
    pub out_dir: &'a Path,
    pub branding: &'a Branding,
    /// Where per-machine signing keys are kept (created on first use), e.g.
    /// `<config>/keys`: Android `android-export.key` / `.cer`, so re-exports install as
    /// updates. `None`: a one-off key.
    pub keys_dir: Option<&'a Path>,
    /// Progress lines for the UI / terminal.
    pub log: &'a dyn Fn(&str),
}

/// Builds the package; returns the path of what was written (file or directory).
pub fn export(job: &Job) -> Result<PathBuf, String> {
    std::fs::create_dir_all(job.out_dir).map_err(|e| format!("{}: {e}", job.out_dir.display()))?;
    match job.target {
        Target::Windows => windows::export(job),
        Target::MacOs => macos::export(job),
        Target::Linux => linux::export(job),
        Target::Android => android::export(job),
        Target::Ps4 => ps4::export(job),
        Target::Ps5 => ps5::export(job),
        Target::N64 => n64::export(job),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets() {
        for t in Target::ALL {
            assert_eq!(Target::parse(t.id()), Some(t));
        }
        assert_eq!(Target::parse("PS4"), Some(Target::Ps4));
        assert!(Target::exportable().contains(&Target::N64));
        assert_eq!(Target::Ps4.runtime_asset("0.1.0").as_deref(), Some("runtime-ps4-0.1.0.zip"));
        assert_eq!(Target::Windows.runtime_asset("0.1.0"), None);
    }
}
