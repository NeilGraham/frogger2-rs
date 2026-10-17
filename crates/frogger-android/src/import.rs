//! Importing the game on Android: looks for a Frogger 2 disc image (or a copied disc /
//! installed game folder) in the app's files directory, where the system file picker
//! (`MainActivity.pickDiscImage`) or `adb push` puts it, imports it with `f2_import` on a
//! worker thread with live progress, then hands over to the game. The first start of an
//! exported APK unpacks the bundled game the same way. What it shows is the launcher's import
//! page ([`Importer::view`]), drawn by the host with the rest of the launcher.

use f2_import::{Event, Options, Sink};
use f2_launcher::ui::{ImportView, Progress};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

const IMAGE_EXTS: [&str; 9] = ["cue", "iso", "bin", "img", "ccd", "mds", "nrg", "toc", "zip"];

/// Work the importer runs on its worker thread, reporting `(stage, fraction)`.
pub type ImportJob = Box<dyn FnOnce(&dyn Fn(String, f32)) -> Result<(), String> + Send>;

enum Msg {
    Log(String),
    Progress(String, f32),
    Done(Result<(), String>),
}

enum State {
    /// Waiting for a source to appear (rescanned twice a second).
    Waiting {
        last_scan: Instant,
    },
    Importing {
        source: PathBuf,
        rx: Receiver<Msg>,
    },
    Failed(String),
    Finished,
}

pub struct Importer {
    /// The app's external files directory (where the user copies the disc image).
    files: PathBuf,
    data_root: PathBuf,
    state: State,
    log: Vec<String>,
    progress: Option<(String, f32)>,
    /// The system file picker can be opened.
    picker: bool,
    /// `import/.status` written by the activity while it copies picked files.
    copy_status: String,
    /// Unpacking the game bundled in an exported APK instead of importing a disc.
    unpacking: bool,
}

impl Importer {
    pub fn new(files: PathBuf, data_root: PathBuf, picker: bool) -> Importer {
        let mut s = Importer {
            files,
            data_root,
            state: State::Waiting { last_scan: Instant::now() },
            log: Vec::new(),
            progress: None,
            picker,
            copy_status: String::new(),
            unpacking: false,
        };
        s.scan();
        s
    }

    /// The first start of an exported APK: `job` unpacks the game bundled in the APK
    /// (reporting `(stage, fraction)`) on a worker thread; no import choices are shown.
    pub fn unpacking(files: PathBuf, job: ImportJob) -> Importer {
        let (tx, rx) = channel();
        let progress_tx = std::sync::Mutex::new(tx.clone());
        let spawned = std::thread::Builder::new().name("unpack".into()).spawn(move || {
            let report = |stage: String, f: f32| {
                let _ = progress_tx.lock().unwrap().send(Msg::Progress(stage, f));
            };
            let _ = tx.send(Msg::Done(job(&report)));
        });
        let state = match spawned {
            Ok(_) => State::Importing { source: PathBuf::from("the game data"), rx },
            Err(e) => State::Failed(e.to_string()),
        };
        Importer { files: files.clone(), data_root: files, state, log: Vec::new(), progress: None, picker: false, copy_status: String::new(), unpacking: true }
    }

    /// Back to waiting for a source after a failure (a failed unpack is retried by starting
    /// the app again).
    pub fn retry(&mut self) {
        if matches!(self.state, State::Failed(_)) && !self.unpacking {
            self.log.clear();
            self.progress = None;
            self.state = State::Waiting { last_scan: Instant::now() };
            self.scan();
        }
    }

    /// True once the data is imported (the host then locates it and starts the game).
    pub fn update(&mut self) -> bool {
        match &mut self.state {
            State::Waiting { last_scan } if last_scan.elapsed() >= Duration::from_millis(500) => {
                self.copy_status = std::fs::read_to_string(self.files.join("import/.status")).unwrap_or_default();
                self.scan()
            }
            State::Waiting { .. } | State::Failed(_) => {}
            State::Importing { rx, .. } => {
                while let Ok(m) = rx.try_recv() {
                    match m {
                        Msg::Log(l) => {
                            self.log.push(l);
                            if self.log.len() > 200 {
                                self.log.drain(..100);
                            }
                        }
                        Msg::Progress(s, f) => self.progress = Some((s, f)),
                        Msg::Done(Ok(())) => {
                            // `import/` is the staging folder for picked files: free the space.
                            let _ = std::fs::remove_dir_all(self.files.join("import"));
                            self.state = State::Finished;
                            return true;
                        }
                        Msg::Done(Err(e)) => {
                            eprintln!("frogger: import failed: {e}");
                            self.state = State::Failed(e);
                            break;
                        }
                    }
                }
            }
            State::Finished => return true,
        }
        false
    }

    /// The launcher's import page for the current state.
    pub fn view(&self) -> ImportView {
        let mut v = ImportView { unpacking: self.unpacking, ..ImportView::default() };
        match &self.state {
            State::Waiting { .. } => {
                v.note = copy_status(&self.copy_status);
                if self.picker {
                    v.help.push("For a .cue with its .bin, pick both files.".into());
                }
                v.help.push(format!(
                    "Or copy the image or the disc's files into {} (for example with adb push): the import starts by itself. A game imported on a PC can be copied into its data folder instead.",
                    self.files.display()
                ));
            }
            State::Importing { source, .. } => {
                let title = if self.unpacking {
                    "Unpacking the game".to_string()
                } else {
                    format!("Importing {}", source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
                };
                let (stage, fraction) = self.progress.clone().unwrap_or_else(|| ("Starting".into(), 0.0));
                v.running = Some(Progress { title, stage, fraction, log: self.log.iter().rev().take(8).rev().cloned().collect() });
            }
            State::Failed(e) => v.error = Some(e.clone()),
            State::Finished => v.running = Some(Progress { title: "Starting the game".into(), stage: "Done".into(), fraction: 1.0, log: Vec::new() }),
        }
        v
    }

    /// Starts importing the first source found.
    fn scan(&mut self) {
        let found = candidates(&self.files);
        let Some(source) = found.into_iter().next() else {
            self.state = State::Waiting { last_scan: Instant::now() };
            return;
        };
        eprintln!("frogger: importing {}", source.display());
        let (tx, rx) = channel();
        let tx2 = std::sync::Mutex::new(tx.clone());
        let sink = Sink(Arc::new(move |e: Event| {
            let m = match e {
                Event::Log(l) => Msg::Log(l),
                Event::Progress { stage, fraction } => Msg::Progress(stage, fraction),
            };
            let _ = tx2.lock().unwrap().send(m);
        }));
        let (src, root) = (source.clone(), self.data_root.clone());
        let opts = Options { sink: Some(sink), ..Options::default() };
        let spawned = std::thread::Builder::new().name("import".into()).spawn(move || {
            let r = f2_import::import(&src, &root, &opts).map(|_| ()).map_err(|e| e.to_string());
            let _ = tx.send(Msg::Done(r));
        });
        self.state = match spawned {
            Ok(_) => State::Importing { source, rx },
            Err(e) => State::Failed(e.to_string()),
        };
    }
}

/// Disc images and disc/install folders in `dir` and `dir/import` (not `data/`): images
/// first, a `.bin` only when no `.cue` describes it. Files still being written (changed in
/// the last few seconds: an `adb push` in progress) are left for a later scan.
pub fn candidates(dir: &Path) -> Vec<PathBuf> {
    candidates_settled(dir, Duration::from_secs(3))
}

fn candidates_settled(dir: &Path, settle: Duration) -> Vec<PathBuf> {
    let busy = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_none_or(|age| age < settle);
    let mut out = Vec::new();
    for d in [dir.to_path_buf(), dir.join("import")] {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        let has_cue = entries.iter().any(|p| ext(p).as_deref() == Some("cue"));
        if entries.iter().any(|p| p.is_file() && IMAGE_EXTS.contains(&ext(p).unwrap_or_default().as_str()) && busy(p)) {
            continue;
        }
        for p in &entries {
            if p.is_file() {
                match ext(p).as_deref() {
                    Some("bin" | "img") if has_cue => {}
                    Some(e) if IMAGE_EXTS.contains(&e) => out.push(p.clone()),
                    _ => {}
                }
            }
        }
        for p in entries {
            if p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
                continue;
            }
            let name = p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if p.is_dir() && !matches!(name.as_str(), "data" | "import" | "saves" | "mods") && f2_import::classify(&p).is_ok() {
                out.push(p);
            }
        }
    }
    out
}

/// The activity's copy progress line (`copy NAME DONE TOTAL`, `copied N`, `error MSG`,
/// `cancelled`) for display.
fn copy_status(s: &str) -> Option<String> {
    let w: Vec<&str> = s.split_whitespace().collect();
    match w.as_slice() {
        ["copy", name @ .., done, total] => {
            let (d, t) = (done.parse::<f64>().ok()? / 1048576.0, total.parse::<f64>().ok()?);
            Some(if t > 0.0 {
                format!("Copying {}: {:.0} of {:.0} MB", name.join(" "), d, t / 1048576.0)
            } else {
                format!("Copying {}: {d:.0} MB", name.join(" "))
            })
        }
        ["error", msg @ ..] => Some(format!("Copy failed: {}", msg.join(" "))),
        _ => None,
    }
}

fn ext(p: &Path) -> Option<String> {
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_status_lines() {
        assert_eq!(copy_status("copy Frogger 2.bin 8388608 16777216").as_deref(), Some("Copying Frogger 2.bin: 8 of 16 MB"));
        assert_eq!(copy_status("error disk full").as_deref(), Some("Copy failed: disk full"));
        assert_eq!(copy_status("copied 2"), None);
    }

    #[test]
    fn view_follows_the_state() {
        let d = std::env::temp_dir().join(format!("f2-android-view-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let mut i = Importer::new(d.clone(), d.join("data"), true);
        let v = i.view();
        assert!(v.running.is_none() && v.error.is_none() && !v.unpacking);
        assert!(v.help.iter().any(|h| h.contains(&d.display().to_string())), "says where to copy files");
        i.state = State::Failed("bad".into());
        assert_eq!(i.view().error.as_deref(), Some("bad"));
        i.retry();
        assert!(i.view().error.is_none());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn finds_images() {
        let d = std::env::temp_dir().join(format!("f2-android-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("import")).unwrap();
        std::fs::create_dir_all(d.join("data")).unwrap();
        std::fs::create_dir_all(d.join("import/.incoming")).unwrap();
        for f in ["Frogger2.cue", "Frogger2.bin", "notes.txt", "import/other.ISO", "import/.status", "import/.incoming/half.iso"] {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        // Just written: nothing yet.
        assert!(candidates(&d).is_empty());
        let c = candidates_settled(&d, Duration::ZERO);
        assert_eq!(c, vec![d.join("Frogger2.cue"), d.join("import/other.ISO")]);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
