//! The launcher screen on Android: the shared UI model (`f2_launcher::ui`) drawn with the
//! host's UI renderer (`frogger::uikit`), plus the touch handling it needs (tap vs. drag-to-scroll) and the
//! state it shows. The parts that need no GPU are unit-tested on the host.

use crate::touch::Phase;
use f2_launcher::mods;
use f2_launcher::ui::{Action, Caps, Launcher, ModRow, SettingGroup, SettingKind, SettingRow, State};
use std::path::Path;

/// What this host offers: no in-app updates (an APK is installed by the system), no folder
/// opening, no disc drives; importing (a disc image from the file picker) only when the APK
/// does not carry the game itself.
pub fn caps(bundled: bool) -> Caps {
    Caps { import: !bundled, updates_page: true, close_label: Some("Back to the game"), ..Caps::default() }
}

/// What the launcher shows besides its own message.
pub struct Inputs<'a> {
    pub game: Option<&'a str>,
    pub data_root: &'a Path,
    pub config_path: &'a Path,
    pub mods_dir: &'a Path,
    pub enabled_mods: &'a [String],
    pub start_in_launcher: bool,
    /// The game came with the APK (no importing).
    pub bundled: bool,
}

/// Refreshes `l.state` from `i`, keeping the message and any running job.
pub fn refresh(l: &mut Launcher, i: &Inputs) {
    let s: &mut State = &mut l.state;
    s.game = i.game.map(str::to_string);
    s.data_dir = i.data_root.display().to_string();
    s.app_version = env!("CARGO_PKG_VERSION").to_string();
    s.mods_dir = i.mods_dir.display().to_string();
    s.settings = settings(i);
    s.mods =
        mods::list(i.mods_dir).into_iter().map(|m| ModRow { enabled: i.enabled_mods.contains(&m.id), id: m.id, name: m.name, version: m.version }).collect();
}

/// The settings page.
fn settings(i: &Inputs) -> Vec<SettingGroup> {
    let row = |id: &'static str, label: &str, detail: String, kind: SettingKind| SettingRow { id, label: label.into(), detail, kind };
    let mut game = Vec::new();
    if let Some(t) = i.game {
        let kind = if i.bundled { SettingKind::Info } else { SettingKind::Button("Import again".into()) };
        game.push(row("import", "Your game", t.to_string(), kind));
    }
    game.push(row("data", "Game data", i.data_root.display().to_string(), SettingKind::Info));
    game.push(row("config", "Settings file", i.config_path.display().to_string(), SettingKind::Info));
    vec![
        SettingGroup { title: "Game".into(), rows: game },
        SettingGroup {
            title: "This app".into(),
            rows: vec![
                row(
                    "start",
                    "Starting the app opens",
                    "The side menu (swipe from the left) always has the launcher.".into(),
                    SettingKind::Choice { options: vec!["The game".into(), "This launcher".into()], selected: i.start_in_launcher as usize },
                ),
                row("updates", "New versions", "Opens the releases page in the browser.".into(), SettingKind::Button("Check".into())),
            ],
        },
    ]
}

/// `id` toggled in the enabled list (appended = loads last, as `[mods] enabled` says).
pub fn toggle_mod(enabled: &mut Vec<String>, id: &str) {
    match enabled.iter().position(|e| e == id) {
        Some(i) => {
            enabled.remove(i);
        }
        None => enabled.push(id.to_string()),
    }
}

/// What a finger did on the launcher.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gesture {
    None,
    /// Scroll the page by this many pixels (positive = down).
    Scroll(f32),
    /// A tap at this point.
    Tap(f32, f32),
}

/// Tells taps from drags: a finger that stays within the slop is a tap on release; a finger
/// that moves further scrolls the page with it and never presses a button.
#[derive(Default)]
pub struct Tracker {
    /// (id, start x, start y, last y, moved)
    finger: Option<(u64, f32, f32, f32, bool)>,
}

impl Tracker {
    pub fn touch(&mut self, id: u64, phase: Phase, x: f32, y: f32, slop: f32) -> Gesture {
        match phase {
            Phase::Started => {
                if self.finger.is_none() {
                    self.finger = Some((id, x, y, y, false));
                }
                Gesture::None
            }
            Phase::Moved => {
                let Some((fid, sx, sy, last, moved)) = &mut self.finger else { return Gesture::None };
                if *fid != id {
                    return Gesture::None;
                }
                if !*moved && ((x - *sx).abs() > slop || (y - *sy).abs() > slop) {
                    *moved = true;
                }
                if !*moved {
                    return Gesture::None;
                }
                // Dragging up scrolls down.
                let d = *last - y;
                *last = y;
                if d != 0.0 {
                    Gesture::Scroll(d)
                } else {
                    Gesture::None
                }
            }
            Phase::Ended => match self.finger {
                Some((fid, sx, sy, _, moved)) if fid == id => {
                    self.finger = None;
                    if moved {
                        Gesture::None
                    } else {
                        Gesture::Tap(sx, sy)
                    }
                }
                _ => Gesture::None,
            },
        }
    }

    pub fn clear(&mut self) {
        self.finger = None;
    }
}

/// Whether `a` closes the launcher.
pub fn closes(a: &Action) -> bool {
    matches!(a, Action::Play | Action::Close)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_vs_drag() {
        let mut t = Tracker::default();
        assert_eq!(t.touch(1, Phase::Started, 100.0, 300.0, 20.0), Gesture::None);
        assert_eq!(t.touch(1, Phase::Moved, 104.0, 306.0, 20.0), Gesture::None, "within the slop");
        assert_eq!(t.touch(1, Phase::Ended, 104.0, 306.0, 20.0), Gesture::Tap(100.0, 300.0));
        // Dragging up scrolls down with the finger and is not a tap.
        t.touch(2, Phase::Started, 100.0, 600.0, 20.0);
        assert_eq!(t.touch(2, Phase::Moved, 100.0, 500.0, 20.0), Gesture::Scroll(100.0));
        assert_eq!(t.touch(2, Phase::Moved, 100.0, 420.0, 20.0), Gesture::Scroll(80.0));
        assert_eq!(t.touch(2, Phase::Moved, 100.0, 510.0, 20.0), Gesture::Scroll(-90.0));
        assert_eq!(t.touch(2, Phase::Ended, 100.0, 510.0, 20.0), Gesture::None);
        // A second finger is ignored while one is down.
        t.touch(3, Phase::Started, 10.0, 10.0, 20.0);
        assert_eq!(t.touch(4, Phase::Started, 50.0, 50.0, 20.0), Gesture::None);
        assert_eq!(t.touch(4, Phase::Ended, 50.0, 50.0, 20.0), Gesture::None);
        assert_eq!(t.touch(3, Phase::Ended, 10.0, 10.0, 20.0), Gesture::Tap(10.0, 10.0));
    }

    #[test]
    fn mods_toggle_in_load_order() {
        let mut e = vec!["a".to_string()];
        toggle_mod(&mut e, "b");
        assert_eq!(e, ["a", "b"]);
        toggle_mod(&mut e, "a");
        assert_eq!(e, ["b"]);
    }

    #[test]
    fn state_comes_from_the_inputs_and_keeps_the_message() {
        let t = std::env::temp_dir().join(format!("f2-android-launcher-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        std::fs::create_dir_all(t.join("m1")).unwrap();
        std::fs::write(t.join("m1/mod.toml"), "id = \"m1\"\nname = \"One\"\n").unwrap();
        let mut l = Launcher::new(caps(false), 1920.0, 1080.0, 2.0);
        l.state.message = Some(("hello".into(), false));
        let enabled = vec!["m1".to_string()];
        let i = Inputs {
            game: Some("G"),
            data_root: Path::new("/d"),
            config_path: Path::new("/c.toml"),
            mods_dir: &t,
            enabled_mods: &enabled,
            start_in_launcher: true,
            bundled: false,
        };
        refresh(&mut l, &i);
        assert_eq!(l.state.game.as_deref(), Some("G"));
        let start = l.state.settings.iter().flat_map(|g| &g.rows).find(|r| r.id == "start").unwrap();
        assert_eq!(start.kind, SettingKind::Choice { options: vec!["The game".into(), "This launcher".into()], selected: 1 });
        assert_eq!(l.state.mods.len(), 1);
        assert!(l.state.mods[0].enabled && l.state.mods[0].name == "One");
        assert_eq!(l.state.message.as_ref().map(|m| m.0.as_str()), Some("hello"));
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn caps_hide_import_for_bundled_apks() {
        assert!(caps(false).import && !caps(true).import);
        assert!(!caps(false).versions && caps(false).updates_page && !caps(false).drives && !caps(false).pick_folder);
        assert!(closes(&Action::Play) && closes(&Action::Close) && !closes(&Action::Import));
    }
}
