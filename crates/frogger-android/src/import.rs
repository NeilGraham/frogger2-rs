//! The import screen shown when no game data is installed: looks for a Frogger 2 disc
//! image (or a copied disc / installed game folder) in the app's files directory, imports
//! it with `f2_import` on a worker thread with live progress, then hands over to the game.
//! Drawn with the built-in 5×7 font (no game data exists yet).

use f2_engine::{Blend, FontId, Quad2D, Renderer};
use f2_import::{Event, Options, Sink};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

const IMAGE_EXTS: [&str; 9] = ["cue", "iso", "bin", "img", "ccd", "mds", "nrg", "toc", "zip"];

/// Work the screen runs on its worker thread, reporting `(stage, fraction)`.
type ImportJob = Box<dyn FnOnce(&dyn Fn(String, f32)) -> Result<(), String> + Send>;

enum Msg {
    Log(String),
    Progress(String, f32),
    Done(Result<(), String>),
}

enum State {
    /// Waiting for a source to appear (rescanned every couple of seconds).
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

pub struct Screen {
    r: Renderer,
    font: FontId,
    /// The app's external files directory (where the user copies the disc image).
    files: PathBuf,
    data_root: PathBuf,
    state: State,
    log: Vec<String>,
    progress: Option<(String, f32)>,
    /// The whole screen was tapped (retry after a failure).
    tapped: bool,
    /// "Choose a disc image" button (x, y, w, h) as last drawn; `None` without a picker.
    button: Option<[f32; 4]>,
    /// Opens the system file picker (`MainActivity.pickDiscImage`).
    picker: Option<Box<dyn Fn()>>,
    /// `import/.status` written by the activity while it copies picked files.
    copy_status: String,
    /// Unpacking the game bundled in an exported APK instead of importing a disc.
    unpacking: bool,
}

impl Screen {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        files: PathBuf,
        data_root: PathBuf,
        picker: Option<Box<dyn Fn()>>,
    ) -> Screen {
        let mut r = Renderer::new(device, queue, format);
        let font = r.add_font(frogger::tinyfont::builtin_font());
        let mut s = Screen {
            r,
            font,
            files,
            data_root,
            state: State::Waiting { last_scan: Instant::now() },
            log: Vec::new(),
            progress: None,
            tapped: false,
            button: None,
            picker,
            copy_status: String::new(),
            unpacking: false,
        };
        s.scan();
        s
    }

    /// The first start of an exported APK: `job` unpacks the game bundled in the APK
    /// (reporting `(stage, fraction)`) on a worker thread; no import choices are shown.
    pub fn unpacking(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, files: PathBuf, job: ImportJob) -> Screen {
        let mut r = Renderer::new(device, queue, format);
        let font = r.add_font(frogger::tinyfont::builtin_font());
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
        Screen {
            r,
            font,
            files: files.clone(),
            data_root: files,
            state,
            log: Vec::new(),
            progress: None,
            tapped: false,
            button: None,
            picker: None,
            copy_status: String::new(),
            unpacking: true,
        }
    }

    /// A tap at (x, y) in pixels.
    pub fn tap(&mut self, x: f32, y: f32) {
        match (self.button, &self.picker, &self.state) {
            (Some([bx, by, bw, bh]), Some(pick), State::Waiting { .. }) if x >= bx && x <= bx + bw && y >= by && y <= by + bh => pick(),
            _ => self.tapped = true,
        }
    }

    /// True once the data is imported (the host then locates it and starts the game).
    pub fn update(&mut self) -> bool {
        match &mut self.state {
            State::Waiting { last_scan } if last_scan.elapsed() >= Duration::from_millis(500) => {
                self.copy_status = std::fs::read_to_string(self.files.join("import/.status")).unwrap_or_default();
                self.scan()
            }
            State::Waiting { .. } => {}
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
            // A failed unpack cannot be retried from here (the app restarts it).
            State::Failed(_) if !self.unpacking && std::mem::take(&mut self.tapped) => {
                self.log.clear();
                self.progress = None;
                self.state = State::Waiting { last_scan: Instant::now() };
                self.scan();
            }
            State::Failed(_) => {}
            State::Finished => return true,
        }
        self.tapped = false;
        false
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

    pub fn render(&mut self, view: &wgpu::TextureView, w: u32, h: u32) {
        let (wf, hf) = (w as f32, h as f32);
        let r = &mut self.r;
        let f = self.font;
        r.begin_frame(w, h);
        let u = (hf / 360.0).clamp(1.0, 6.0);
        let s = (1.5 * u).round().max(2.0);
        let line = 8.0 * s + 5.0 * u;
        let rect = |r: &mut Renderer, x: f32, y: f32, ww: f32, hh: f32, c: [u8; 4]| {
            r.draw_quad(Quad2D::rect(x, y, ww, hh, [0.0, 0.0, 1.0, 1.0], None, c, Blend::Semi))
        };
        const TITLE: [u8; 4] = [120, 230, 90, 255];
        const TEXT: [u8; 4] = [230, 230, 230, 255];
        const DIM: [u8; 4] = [160, 170, 160, 255];
        rect(r, 0.0, 0.0, wf, line * 3.0, [20, 60, 20, 255]);
        r.draw_text(f, 0.0, line * 0.5, "FROGGER 2: SWAMPY'S REVENGE (RUST)", TITLE, s * 1.0, Some(wf / 2.0));
        let subtitle = if self.unpacking { "Preparing the game (first start only)" } else { "Import your Frogger 2 game" };
        r.draw_text(f, 0.0, line * 1.9, subtitle, TEXT, s * 0.9, Some(wf / 2.0));
        let x0 = line;
        let cols = ((wf - 2.0 * x0) / (6.0 * s * 0.7)).max(16.0) as usize;
        let mut y = line * 3.8;
        let para = |r: &mut Renderer, text: &str, color: [u8; 4], y: &mut f32| {
            for l in wrap(text, cols) {
                r.draw_text(f, x0, *y, &l, color, s * 0.7, None);
                *y += line * 0.75;
            }
            *y += line * 0.35;
        };
        self.button = None;
        match &self.state {
            State::Waiting { .. } => {
                para(r, "No game data was found. Import Frogger 2 from your own disc image (.iso, .cue + .bin, .zip, ...).", TEXT, &mut y);
                if self.picker.is_some() {
                    let (bw, bh) = ((wf * 0.5).max(line * 12.0).min(wf - 2.0 * x0), line * 1.6);
                    let bx = (wf - bw) / 2.0;
                    rect(r, bx, y, bw, bh, [50, 130, 40, 255]);
                    r.draw_text(f, 0.0, y + (bh - 8.0 * s) / 2.0, "Choose disc image", TEXT, s, Some(wf / 2.0));
                    self.button = Some([bx, y, bw, bh]);
                    y += bh + line * 0.6;
                    if let Some(st) = copy_status(&self.copy_status) {
                        para(r, &st, TITLE, &mut y);
                    }
                    para(r, "(for a .cue + .bin image select both files). Or copy the image or the disc's files into:", DIM, &mut y);
                } else {
                    para(r, "Copy the image or the disc's files into:", DIM, &mut y);
                }
                para(r, &self.files.display().to_string(), TITLE, &mut y);
                para(r, "e.g. adb push Frogger2.iso /sdcard/Android/data/io.github.neilgraham.frogger2rs/files/ - the import starts by itself. Data already imported on a PC can be copied to files/data/ instead.", DIM, &mut y);
            }
            State::Importing { source, .. } => {
                if self.unpacking {
                    para(r, "Unpacking the game data from the app. This happens once and takes a minute.", TEXT, &mut y);
                } else {
                    para(r, &format!("Importing {}", source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()), TEXT, &mut y);
                }
                if let Some((stage, frac)) = &self.progress {
                    let bw = wf - 2.0 * x0;
                    rect(r, x0, y, bw, line * 0.6, [60, 60, 60, 255]);
                    rect(r, x0, y, bw * frac.clamp(0.0, 1.0), line * 0.6, [90, 200, 70, 255]);
                    y += line;
                    para(r, &format!("{stage} {:.0}%", frac * 100.0), DIM, &mut y);
                }
                let room = ((hf - y) / (line * 0.75)).max(0.0) as usize;
                for l in self.log.iter().rev().take(room).rev() {
                    let l: String = l.chars().take(cols).collect();
                    r.draw_text(f, x0, y, &l, DIM, s * 0.7, None);
                    y += line * 0.75;
                }
            }
            State::Failed(e) => {
                para(r, if self.unpacking { "Unpacking the game data failed:" } else { "The import failed:" }, [255, 120, 120, 255], &mut y);
                para(r, e, TEXT, &mut y);
                para(r, if self.unpacking { "Free some storage space and start the app again." } else { "Tap the screen to try again." }, DIM, &mut y);
            }
            State::Finished => para(r, "Import complete. Starting the game...", TEXT, &mut y),
        }
        self.r.render(view, w, h, [0.02, 0.05, 0.02, 1.0]);
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

fn wrap(t: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    // Words longer than a line (paths) are cut into line-sized pieces.
    let pieces = t.split_whitespace().flat_map(|w| {
        let c: Vec<char> = w.chars().collect();
        c.chunks(width.max(1)).map(|p| p.iter().collect::<String>()).collect::<Vec<_>>()
    });
    for word in pieces {
        if !cur.is_empty() && cur.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(&word);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
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
    fn wraps_long_words() {
        assert_eq!(wrap("a bb ccc", 4), vec!["a bb", "ccc"]);
        assert_eq!(wrap("/storage/emulated", 8), vec!["/storage", "/emulate", "d"]);
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
