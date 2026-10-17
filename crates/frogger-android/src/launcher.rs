//! The launcher screen on Android: the shared UI model (`f2_launcher::ui`) drawn with the
//! host's UI renderer (`frogger::uikit`), plus the touch handling it needs (tap vs. drag-to-scroll) and the
//! state it shows. The parts that need no GPU are unit-tested on the host.

use crate::touch::Phase;
use f2_launcher::mods;
use f2_launcher::ui::{Action, Caps, Launcher, ModRow, State};
use std::path::Path;

/// What this host offers: no in-app updates (an APK is installed by the system), no folder
/// opening; importing only when the APK does not carry the game itself.
pub fn caps(bundled: bool) -> Caps {
    Caps { versions: false, import: !bundled, open_folders: false, updates_page: true, close_label: Some("Back to the game") }
}

/// What the launcher shows besides its own message.
pub struct Inputs<'a> {
    pub game: Option<&'a str>,
    pub data_root: &'a Path,
    pub config_path: &'a Path,
    pub mods_dir: &'a Path,
    pub enabled_mods: &'a [String],
    pub start_in_launcher: bool,
}

/// Refreshes `l.state` from `i`, keeping the message and any running job.
pub fn refresh(l: &mut Launcher, i: &Inputs) {
    let s: &mut State = &mut l.state;
    s.game = i.game.map(str::to_string);
    s.data_dir = i.data_root.display().to_string();
    s.config_path = i.config_path.display().to_string();
    s.app_version = env!("CARGO_PKG_VERSION").to_string();
    s.start_in_launcher = i.start_in_launcher;
    s.mods_dir = i.mods_dir.display().to_string();
    s.mods =
        mods::list(i.mods_dir).into_iter().map(|m| ModRow { enabled: i.enabled_mods.contains(&m.id), id: m.id, name: m.name, version: m.version }).collect();
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
    /// Scroll the list by this many rows (positive = down).
    Scroll(i32),
    /// A tap at this point.
    Tap(f32, f32),
}

/// Tells taps from drags: a finger that stays within the slop is a tap on release; a finger
/// that moves further scrolls the list (a row per `row_px`) and never presses a button.
#[derive(Default)]
pub struct Tracker {
    /// (id, start x, start y, last y, moved)
    finger: Option<(u64, f32, f32, f32, bool)>,
}

impl Tracker {
    pub fn touch(&mut self, id: u64, phase: Phase, x: f32, y: f32, row_px: f32, slop: f32) -> Gesture {
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
                let rows = ((*last - y) / row_px).trunc() as i32;
                if rows != 0 {
                    *last -= rows as f32 * row_px;
                    return Gesture::Scroll(rows);
                }
                Gesture::None
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
        assert_eq!(t.touch(1, Phase::Started, 100.0, 300.0, 90.0, 20.0), Gesture::None);
        assert_eq!(t.touch(1, Phase::Moved, 104.0, 306.0, 90.0, 20.0), Gesture::None, "within the slop");
        assert_eq!(t.touch(1, Phase::Ended, 104.0, 306.0, 90.0, 20.0), Gesture::Tap(100.0, 300.0));
        // Dragging up scrolls down a row per 90 px and is not a tap.
        t.touch(2, Phase::Started, 100.0, 600.0, 90.0, 20.0);
        assert_eq!(t.touch(2, Phase::Moved, 100.0, 500.0, 90.0, 20.0), Gesture::Scroll(1));
        assert_eq!(t.touch(2, Phase::Moved, 100.0, 420.0, 90.0, 20.0), Gesture::Scroll(1), "the rest of the distance");
        assert_eq!(t.touch(2, Phase::Moved, 100.0, 510.0, 90.0, 20.0), Gesture::Scroll(-1));
        assert_eq!(t.touch(2, Phase::Ended, 100.0, 510.0, 90.0, 20.0), Gesture::None);
        // A second finger is ignored while one is down.
        t.touch(3, Phase::Started, 10.0, 10.0, 90.0, 20.0);
        assert_eq!(t.touch(4, Phase::Started, 50.0, 50.0, 90.0, 20.0), Gesture::None);
        assert_eq!(t.touch(4, Phase::Ended, 50.0, 50.0, 90.0, 20.0), Gesture::None);
        assert_eq!(t.touch(3, Phase::Ended, 10.0, 10.0, 90.0, 20.0), Gesture::Tap(10.0, 10.0));
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
        };
        refresh(&mut l, &i);
        assert_eq!(l.state.game.as_deref(), Some("G"));
        assert!(l.state.start_in_launcher);
        assert_eq!(l.state.mods.len(), 1);
        assert!(l.state.mods[0].enabled && l.state.mods[0].name == "One");
        assert_eq!(l.state.message.as_ref().map(|m| m.0.as_str()), Some("hello"));
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn caps_hide_import_for_bundled_apks() {
        assert!(caps(false).import && !caps(true).import);
        assert!(!caps(false).versions && caps(false).updates_page);
        assert!(closes(&Action::Play) && closes(&Action::Close) && !closes(&Action::Import));
    }
}
