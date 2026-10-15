//! Settings → Export: builds the player's game for other platforms and for this desktop OS
//! (`crate::export`, docs/EXPORT.md) on a worker thread, one row per target.

use super::settings::Id;
use super::App;
use std::sync::{Arc, Mutex};

/// Export status shared with the worker threads.
#[derive(Clone, Default)]
#[cfg_attr(not(feature = "export"), allow(dead_code))]
pub struct ExportUi {
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
#[cfg_attr(not(feature = "export"), allow(dead_code))]
struct State {
    /// Per target index (`Target::exportable()`): the last progress / result line.
    status: Vec<Option<String>>,
    /// A worker is running (one export at a time).
    busy: bool,
}

#[cfg_attr(not(feature = "export"), allow(dead_code))]
impl ExportUi {
    fn status(&self, i: usize) -> Option<String> {
        self.state.lock().unwrap().status.get(i).cloned().flatten()
    }

    fn set(&self, i: usize, line: String) {
        let mut s = self.state.lock().unwrap();
        if s.status.len() <= i {
            s.status.resize(i + 1, None);
        }
        s.status[i] = Some(line);
    }

    fn busy(&self) -> bool {
        self.state.lock().unwrap().busy
    }
}

/// Where exports are written.
pub fn exports_dir() -> std::path::PathBuf {
    f2_platform::paths::config_dir().join("exports")
}

impl App {
    /// The Export page's rows.
    pub(crate) fn export_items(&self, add: &mut impl FnMut(Id, &str, String, &str, bool, bool)) {
        #[cfg(feature = "export")]
        {
            use crate::export::Target;
            let title = crate::export::title(&self.data);
            let dir = exports_dir();
            let busy = self.exports.busy();
            for (i, t) in Target::exportable().into_iter().enumerate() {
                let status = self.exports.status(i);
                let value = match &status {
                    Some(s) if s.starts_with("Done") => "Done".to_string(),
                    Some(s) if s.starts_with("Failed") => "Failed".to_string(),
                    Some(_) => "Working...".to_string(),
                    None => String::new(),
                };
                let desc = match (&status, &title) {
                    (Some(s), _) => s.clone(),
                    (None, Ok(title)) => format!("Builds \"{title}\" with your game data. For your own devices only."),
                    (None, Err(e)) => e.clone(),
                };
                add(Id::Export(i as u8), &format!("Export for {}", t.name()), value, &desc, title.is_ok() && !busy, false);
            }
            add(Id::OpenExports, "Open the exports folder", String::new(), &dir.display().to_string(), true, false);
        }
        #[cfg(not(feature = "export"))]
        add(Id::OpenExports, "Export", String::new(), "Exporting for other platforms is done from the desktop app.", false, false);
    }

    /// Starts exporting `Target::exportable()[i]` on a worker thread.
    pub(crate) fn start_export(&mut self, i: usize) {
        #[cfg(feature = "export")]
        {
            use crate::export::Target;
            let Some(&target) = Target::exportable().get(i) else { return };
            {
                let mut s = self.exports.state.lock().unwrap();
                if s.busy {
                    return;
                }
                s.busy = true;
            }
            let ui = self.exports.clone();
            let data = self.data.clone();
            ui.set(i, format!("Starting the {} export...", target.name()));
            let spawned = std::thread::Builder::new().name("export".into()).spawn(move || {
                let log = |l: &str| {
                    eprintln!("frogger: export: {l}");
                    ui.set(i, l.to_string());
                };
                let r = crate::export::export(&data, target, None, &exports_dir(), &log);
                ui.set(
                    i,
                    match r {
                        Ok(p) => format!("Done: {}", p.display()),
                        Err(e) => format!("Failed: {e}"),
                    },
                );
                ui.state.lock().unwrap().busy = false;
            });
            if let Err(e) = spawned {
                self.exports.set(i, format!("Failed: {e}"));
                self.exports.state.lock().unwrap().busy = false;
            }
        }
        #[cfg(not(feature = "export"))]
        let _ = i;
    }

    /// Opens the exports folder in the system file manager.
    pub(crate) fn open_exports_folder(&self) {
        let dir = exports_dir();
        let _ = std::fs::create_dir_all(&dir);
        let opener = if cfg!(windows) {
            "explorer"
        } else if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        if let Err(e) = std::process::Command::new(opener).arg(&dir).spawn() {
            eprintln!("frogger: opening {}: {e}", dir.display());
        }
    }
}
