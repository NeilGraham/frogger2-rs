//! First-run import screen: shown when [`crate::data::Data::locate`] finds no game data.
//!
//! No game data exists yet, so it draws with a built-in 5×7 bitmap font and plain colours.
//! It lists optical drives (a Frogger 2 disc is highlighted), offers a native file/folder
//! dialog (PowerShell on Windows, zenity/kdialog on Linux; drag & drop and a typed path
//! always work), runs `f2_import` on a worker thread with live progress, and on success
//! the game restarts itself ([`relaunch`]) because the window event loop can only be
//! created once per process.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Duration;

use f2_engine::font::{Font, Glyph};
use f2_engine::{Blend, FontId, GpuContext, Quad2D, Renderer, WindowSurface};
use f2_import::{Event, Manifest, Options, Sink};
use f2_media::cdrom::DriveInfo;
use f2_platform::window::{self, winit, Flow, WindowApp, WindowConfig};
use f2_platform::Config;

/// Classic 5×7 font (columns, bit 0 = top row, bit 7 = descender) for ASCII 0x20..=0x7E.
const FONT5X7: [[u8; 5]; 95] = [
    [0x00, 0x00, 0x00, 0x00, 0x00], [0x00, 0x00, 0x5F, 0x00, 0x00], [0x00, 0x07, 0x00, 0x07, 0x00], [0x14, 0x7F, 0x14, 0x7F, 0x14],
    [0x24, 0x2A, 0x7F, 0x2A, 0x12], [0x23, 0x13, 0x08, 0x64, 0x62], [0x36, 0x49, 0x56, 0x20, 0x50], [0x00, 0x08, 0x07, 0x03, 0x00],
    [0x00, 0x1C, 0x22, 0x41, 0x00], [0x00, 0x41, 0x22, 0x1C, 0x00], [0x2A, 0x1C, 0x7F, 0x1C, 0x2A], [0x08, 0x08, 0x3E, 0x08, 0x08],
    [0x00, 0x80, 0x70, 0x30, 0x00], [0x08, 0x08, 0x08, 0x08, 0x08], [0x00, 0x00, 0x60, 0x60, 0x00], [0x20, 0x10, 0x08, 0x04, 0x02],
    [0x3E, 0x51, 0x49, 0x45, 0x3E], [0x00, 0x42, 0x7F, 0x40, 0x00], [0x72, 0x49, 0x49, 0x49, 0x46], [0x21, 0x41, 0x49, 0x4D, 0x33],
    [0x18, 0x14, 0x12, 0x7F, 0x10], [0x27, 0x45, 0x45, 0x45, 0x39], [0x3C, 0x4A, 0x49, 0x49, 0x31], [0x41, 0x21, 0x11, 0x09, 0x07],
    [0x36, 0x49, 0x49, 0x49, 0x36], [0x46, 0x49, 0x49, 0x29, 0x1E], [0x00, 0x00, 0x14, 0x00, 0x00], [0x00, 0x40, 0x34, 0x00, 0x00],
    [0x00, 0x08, 0x14, 0x22, 0x41], [0x14, 0x14, 0x14, 0x14, 0x14], [0x00, 0x41, 0x22, 0x14, 0x08], [0x02, 0x01, 0x59, 0x09, 0x06],
    [0x3E, 0x41, 0x5D, 0x59, 0x4E], [0x7C, 0x12, 0x11, 0x12, 0x7C], [0x7F, 0x49, 0x49, 0x49, 0x36], [0x3E, 0x41, 0x41, 0x41, 0x22],
    [0x7F, 0x41, 0x41, 0x41, 0x3E], [0x7F, 0x49, 0x49, 0x49, 0x41], [0x7F, 0x09, 0x09, 0x09, 0x01], [0x3E, 0x41, 0x41, 0x51, 0x73],
    [0x7F, 0x08, 0x08, 0x08, 0x7F], [0x00, 0x41, 0x7F, 0x41, 0x00], [0x20, 0x40, 0x41, 0x3F, 0x01], [0x7F, 0x08, 0x14, 0x22, 0x41],
    [0x7F, 0x40, 0x40, 0x40, 0x40], [0x7F, 0x02, 0x1C, 0x02, 0x7F], [0x7F, 0x04, 0x08, 0x10, 0x7F], [0x3E, 0x41, 0x41, 0x41, 0x3E],
    [0x7F, 0x09, 0x09, 0x09, 0x06], [0x3E, 0x41, 0x51, 0x21, 0x5E], [0x7F, 0x09, 0x19, 0x29, 0x46], [0x26, 0x49, 0x49, 0x49, 0x32],
    [0x03, 0x01, 0x7F, 0x01, 0x03], [0x3F, 0x40, 0x40, 0x40, 0x3F], [0x1F, 0x20, 0x40, 0x20, 0x1F], [0x3F, 0x40, 0x38, 0x40, 0x3F],
    [0x63, 0x14, 0x08, 0x14, 0x63], [0x03, 0x04, 0x78, 0x04, 0x03], [0x61, 0x59, 0x49, 0x4D, 0x43], [0x00, 0x7F, 0x41, 0x41, 0x41],
    [0x02, 0x04, 0x08, 0x10, 0x20], [0x00, 0x41, 0x41, 0x41, 0x7F], [0x04, 0x02, 0x01, 0x02, 0x04], [0x40, 0x40, 0x40, 0x40, 0x40],
    [0x00, 0x03, 0x07, 0x08, 0x00], [0x20, 0x54, 0x54, 0x78, 0x40], [0x7F, 0x28, 0x44, 0x44, 0x38], [0x38, 0x44, 0x44, 0x44, 0x28],
    [0x38, 0x44, 0x44, 0x28, 0x7F], [0x38, 0x54, 0x54, 0x54, 0x18], [0x00, 0x08, 0x7E, 0x09, 0x02], [0x18, 0xA4, 0xA4, 0x9C, 0x78],
    [0x7F, 0x08, 0x04, 0x04, 0x78], [0x00, 0x44, 0x7D, 0x40, 0x00], [0x20, 0x40, 0x40, 0x3D, 0x00], [0x7F, 0x10, 0x28, 0x44, 0x00],
    [0x00, 0x41, 0x7F, 0x40, 0x00], [0x7C, 0x04, 0x78, 0x04, 0x78], [0x7C, 0x08, 0x04, 0x04, 0x78], [0x38, 0x44, 0x44, 0x44, 0x38],
    [0xFC, 0x18, 0x24, 0x24, 0x18], [0x18, 0x24, 0x24, 0x18, 0xFC], [0x7C, 0x08, 0x04, 0x04, 0x08], [0x48, 0x54, 0x54, 0x54, 0x24],
    [0x04, 0x04, 0x3F, 0x44, 0x24], [0x3C, 0x40, 0x40, 0x20, 0x7C], [0x1C, 0x20, 0x40, 0x20, 0x1C], [0x3C, 0x40, 0x30, 0x40, 0x3C],
    [0x44, 0x28, 0x10, 0x28, 0x44], [0x4C, 0x90, 0x90, 0x90, 0x7C], [0x44, 0x64, 0x54, 0x4C, 0x44], [0x00, 0x08, 0x36, 0x41, 0x00],
    [0x00, 0x00, 0x77, 0x00, 0x00], [0x00, 0x41, 0x36, 0x08, 0x00], [0x02, 0x01, 0x02, 0x04, 0x02],
];

/// The built-in font as an engine [`Font`]: 6×8 cells (5 columns of ink + 1 spacing).
pub fn builtin_font() -> Font {
    let (cw, h) = (6usize, 8usize);
    let w = cw * FONT5X7.len();
    let mut rgba = vec![0u8; w * h * 4];
    let mut glyphs = [None; 256];
    for (i, g) in FONT5X7.iter().enumerate() {
        glyphs[0x20 + i] = Some(Glyph { left: (i * cw) as u32, width: cw as u32 });
        for (x, col) in g.iter().enumerate() {
            for y in 0..h {
                if col >> y & 1 == 1 {
                    let o = (y * w + i * cw + x) * 4;
                    rgba[o..o + 4].copy_from_slice(&[255, 255, 255, 255]);
                }
            }
        }
    }
    Font { width: w as u32, height: h as u32, glyphs, rgba }
}

const BG: [f64; 4] = [0.02, 0.09, 0.04, 1.0];
const BAND: [u8; 4] = [18, 70, 30, 255];
const TITLE: [u8; 4] = [255, 220, 40, 255];
const TEXT: [u8; 4] = [225, 240, 225, 255];
const DIM: [u8; 4] = [130, 160, 135, 255];
const SEL: [u8; 4] = [60, 150, 70, 255];
const GOOD: [u8; 4] = [120, 255, 120, 255];
const BAD: [u8; 4] = [255, 110, 100, 255];

#[derive(Clone, Debug)]
enum Item {
    Drive(DriveInfo),
    ImageFile,
    Folder,
    TypePath,
    Rescan,
    Quit,
}

enum Msg {
    Event(Event),
    Done(Result<Manifest, String>),
}

enum Status {
    Menu,
    Running { stage: String, fraction: f32, log: Vec<String> },
    Done(Manifest),
    Failed(String),
}

struct Gfx {
    ctx: GpuContext,
    surface: WindowSurface,
    r: Renderer,
    font: FontId,
}

struct Screen {
    data_root: PathBuf,
    reason: String,
    gfx: Option<Gfx>,
    error: Option<String>,
    drives: Vec<DriveInfo>,
    scan: Option<Receiver<Vec<DriveInfo>>>,
    sel: usize,
    path: String,
    editing: bool,
    dialog: Option<Receiver<Option<PathBuf>>>,
    job: Option<Receiver<Msg>>,
    status: Status,
    note: Option<String>,
    imported: bool,
    exit: bool,
    cursor: (f32, f32),
    hit: Vec<(f32, f32, usize)>,
}

impl Screen {
    fn items(&self) -> Vec<Item> {
        let mut v: Vec<Item> = self.drives.iter().filter(|d| d.media).cloned().map(Item::Drive).collect();
        v.extend([Item::ImageFile, Item::Folder, Item::TypePath, Item::Rescan, Item::Quit]);
        v
    }

    fn rescan(&mut self) {
        if self.scan.is_some() {
            return;
        }
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(f2_media::cdrom::list_drives());
        });
        self.scan = Some(rx);
    }

    fn start(&mut self, what: Source) {
        let (tx, rx) = channel();
        let root = self.data_root.clone();
        let tx2 = tx.clone();
        let sink = Sink(Arc::new(move |e| {
            let _ = tx2.send(Msg::Event(e));
        }));
        let opts = Options { sink: Some(sink), ..Default::default() };
        std::thread::spawn(move || {
            let r = match what {
                Source::Path(p) => f2_import::import(&p, &root, &opts),
                Source::Drive(d) => f2_import::import_drive(&d, &root, &opts),
            };
            let _ = tx.send(Msg::Done(r.map_err(|e| e.to_string())));
        });
        self.job = Some(rx);
        self.status = Status::Running { stage: "starting".into(), fraction: 0.0, log: Vec::new() };
        self.note = None;
    }

    fn activate(&mut self, item: Item) {
        match item {
            Item::Drive(d) => self.start(Source::Drive(d)),
            Item::ImageFile => self.open_dialog(false),
            Item::Folder => self.open_dialog(true),
            Item::TypePath => self.editing = true,
            Item::Rescan => self.rescan(),
            Item::Quit => self.exit = true,
        }
    }

    fn open_dialog(&mut self, folder: bool) {
        if self.dialog.is_some() {
            return;
        }
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(native_dialog(folder));
        });
        self.dialog = Some(rx);
    }

    fn poll(&mut self) {
        if let Some(rx) = &self.scan {
            if let Ok(d) = rx.try_recv() {
                self.drives = d;
                self.scan = None;
                self.sel = self.sel.min(self.items().len() - 1);
            }
        }
        if let Some(rx) = &self.dialog {
            match rx.try_recv() {
                Ok(Some(p)) => {
                    self.dialog = None;
                    self.start(Source::Path(p));
                }
                Ok(None) => {
                    self.dialog = None;
                    self.note = Some("No file chosen. You can also type a path or drag a file onto this window.".into());
                }
                Err(_) => {}
            }
        }
        let mut done = None;
        if let Some(rx) = &self.job {
            while let Ok(m) = rx.try_recv() {
                match m {
                    Msg::Event(e) => {
                        if let Status::Running { stage, fraction, log } = &mut self.status {
                            match e {
                                Event::Progress { stage: s, fraction: f } => {
                                    *stage = s;
                                    *fraction = f;
                                }
                                Event::Log(l) => {
                                    log.push(l);
                                    if log.len() > 200 {
                                        log.remove(0);
                                    }
                                }
                            }
                        }
                    }
                    Msg::Done(r) => done = Some(r),
                }
            }
        }
        if let Some(r) = done {
            self.job = None;
            self.status = match r {
                Ok(m) => Status::Done(m),
                Err(e) => Status::Failed(e),
            };
        }
    }

    fn key(&mut self, k: &winit::keyboard::Key, text: Option<&str>) {
        use winit::keyboard::{Key, NamedKey};
        match &mut self.status {
            Status::Running { .. } => return,
            Status::Done(_) => {
                if matches!(k, Key::Named(NamedKey::Enter | NamedKey::Space)) {
                    self.imported = true;
                    self.exit = true;
                }
                return;
            }
            Status::Failed(_) => {
                if matches!(k, Key::Named(NamedKey::Enter | NamedKey::Escape | NamedKey::Space)) {
                    self.status = Status::Menu;
                }
                return;
            }
            Status::Menu => {}
        }
        if self.editing {
            match k {
                Key::Named(NamedKey::Enter) => {
                    let p = self.path.trim().trim_matches('"').to_string();
                    if !p.is_empty() {
                        self.editing = false;
                        self.start(Source::Path(PathBuf::from(p)));
                    }
                }
                Key::Named(NamedKey::Escape) => self.editing = false,
                Key::Named(NamedKey::Backspace) => {
                    self.path.pop();
                }
                _ => {
                    if let Some(t) = text {
                        self.path.extend(t.chars().filter(|c| !c.is_control()));
                    }
                }
            }
            return;
        }
        let n = self.items().len();
        match k {
            Key::Named(NamedKey::ArrowUp) => self.sel = (self.sel + n - 1) % n,
            Key::Named(NamedKey::ArrowDown | NamedKey::Tab) => self.sel = (self.sel + 1) % n,
            Key::Named(NamedKey::Enter | NamedKey::Space) => {
                let it = self.items()[self.sel].clone();
                self.activate(it);
            }
            Key::Named(NamedKey::Escape) => self.exit = true,
            _ => {}
        }
    }

    fn draw(&mut self, w: u32, h: u32) {
        let Some(mut g) = self.gfx.take() else { return };
        self.draw_with(&mut g.r, g.font, w, h);
        self.gfx = Some(g);
    }

    fn draw_with(&mut self, r: &mut Renderer, f: FontId, w: u32, h: u32) {
        r.begin_frame(w, h);
        let (wf, hf) = (w as f32, h as f32);
        let u = (hf / 480.0).clamp(1.0, 4.0);
        let s = (1.6 * u).round().max(2.0);
        let line = 8.0 * s + 6.0 * u;
        let rect = |r: &mut Renderer, x: f32, y: f32, ww: f32, hh: f32, c: [u8; 4]| r.draw_quad(Quad2D::rect(x, y, ww, hh, [0.0, 0.0, 1.0, 1.0], None, c, Blend::Semi));
        let fit = |t: &str, scale: f32| -> String {
            let max = ((wf - 4.0 * line) / (6.0 * scale)).max(8.0) as usize;
            if t.chars().count() > max {
                format!("...{}", t.chars().skip(t.chars().count() - max + 3).collect::<String>())
            } else {
                t.to_string()
            }
        };
        rect(r, 0.0, 0.0, wf, line * 3.2, BAND);
        r.draw_text(f, 0.0, line * 0.6, "FROGGER 2: SWAMPY'S REVENGE", TITLE, s * 1.6, Some(wf / 2.0));
        r.draw_text(f, 0.0, line * 2.1, "Import your game", TEXT, s, Some(wf / 2.0));
        let x0 = line * 1.5;
        let mut y = line * 4.0;
        self.hit.clear();
        match &self.status {
            Status::Menu => {
                let intro = "No game data was found. Import Frogger 2 from your own disc, disc image or installed copy:";
                for l in wrap(intro, ((wf - 2.0 * x0) / (6.0 * s * 0.75)) as usize) {
                    r.draw_text(f, x0, y, &l, DIM, s * 0.75, None);
                    y += line * 0.8;
                }
                y += line * 0.5;
                let items = self.items();
                for (i, it) in items.iter().enumerate() {
                    let label = match it {
                        Item::Drive(d) if d.frogger => format!("Import from the Frogger 2 disc in {}", d.device),
                        Item::Drive(d) => format!("Import from the disc in {} ({})", d.device, d.label.clone().unwrap_or_else(|| "no label".into())),
                        Item::ImageFile => "Import from an image file... (.cue .bin .iso .ccd .mds .nrg .toc .zip)".into(),
                        Item::Folder => "Import from a folder... (copied disc or installed game)".into(),
                        Item::TypePath if self.editing => format!("Path: {}_", self.path),
                        Item::TypePath => format!("Type a path{}", if self.path.is_empty() { String::new() } else { format!(": {}", self.path) }),
                        Item::Rescan => if self.scan.is_some() { "Looking for drives...".into() } else { "Rescan drives".into() },
                        Item::Quit => "Quit".into(),
                    };
                    if i == self.sel {
                        rect(r, x0 - line * 0.4, y - 3.0 * u, wf - 2.0 * x0 + line * 0.8, line, SEL);
                    }
                    let col = match it {
                        Item::Drive(d) if d.frogger && i != self.sel => GOOD,
                        _ => TEXT,
                    };
                    r.draw_text(f, x0, y, &fit(&label, s), col, s, None);
                    self.hit.push((y - 3.0 * u, y - 3.0 * u + line, i));
                    y += line;
                }
                y += line * 0.5;
                for d in self.drives.iter().filter(|d| !d.media) {
                    r.draw_text(f, x0, y, &fit(&format!("{}: no disc", d.device), s * 0.75), DIM, s * 0.75, None);
                    y += line * 0.8;
                }
                if self.drives.is_empty() && self.scan.is_none() {
                    r.draw_text(f, x0, y, "No CD/DVD drives found.", DIM, s * 0.75, None);
                    y += line * 0.8;
                }
                if let Some(n) = &self.note {
                    r.draw_text(f, x0, y, &fit(n, s * 0.75), TITLE, s * 0.75, None);
                }
                let foot = "Arrows + Enter to choose, or drag a disc image onto this window.  Esc quits.";
                r.draw_text(f, 0.0, hf - line * 1.2, &fit(foot, s * 0.75), DIM, s * 0.75, Some(wf / 2.0));
            }
            Status::Running { stage, fraction, log } => {
                r.draw_text(f, x0, y, &format!("Importing: {stage}"), TEXT, s, None);
                y += line * 1.2;
                let bw = wf - 2.0 * x0;
                rect(r, x0, y, bw, line * 0.8, [40, 60, 45, 255]);
                rect(r, x0, y, bw * fraction.clamp(0.0, 1.0), line * 0.8, [90, 200, 90, 255]);
                y += line * 1.6;
                let rows = (((hf - y) / (line * 0.8)) as usize).saturating_sub(1);
                for l in log.iter().skip(log.len().saturating_sub(rows)) {
                    r.draw_text(f, x0, y, &fit(l.trim(), s * 0.75), DIM, s * 0.75, None);
                    y += line * 0.8;
                }
            }
            Status::Done(m) => {
                r.draw_text(f, x0, y, &fit(&format!("Imported: {}", m.title), s), GOOD, s, None);
                y += line * 1.3;
                let mut lines = vec![
                    format!("{} files, {} videos, {} music tracks", m.files.len(), m.videos.len(), m.music_tracks.len()),
                    format!("Source: {} ({})", m.source, m.source_kind),
                ];
                if let Some(rip) = &m.rip {
                    lines.push(format!("Disc audio: {}", rip.status()));
                }
                if let Some(n) = &m.music_note {
                    lines.push(format!("Music: {n}"));
                }
                for l in lines {
                    for chunk in wrap(&l, ((wf - 2.0 * x0) / (6.0 * s * 0.75)) as usize) {
                        r.draw_text(f, x0, y, &chunk, TEXT, s * 0.75, None);
                        y += line * 0.8;
                    }
                }
                r.draw_text(f, 0.0, hf - line * 1.5, "Press Enter to start the game", TITLE, s, Some(wf / 2.0));
            }
            Status::Failed(e) => {
                r.draw_text(f, x0, y, "Import failed:", BAD, s, None);
                y += line * 1.2;
                for chunk in wrap(e, ((wf - 2.0 * x0) / (6.0 * s * 0.75)) as usize) {
                    r.draw_text(f, x0, y, &chunk, TEXT, s * 0.75, None);
                    y += line * 0.8;
                }
                r.draw_text(f, 0.0, hf - line * 1.5, "Press Enter to go back", TITLE, s, Some(wf / 2.0));
            }
        }
    }
}

fn wrap(t: &str, width: usize) -> Vec<String> {
    let width = width.max(10);
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in t.split_whitespace() {
        if !cur.is_empty() && cur.len() + 1 + word.len() > width {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

enum Source {
    Path(PathBuf),
    Drive(DriveInfo),
}

/// Asks the user for a file (or folder) with the platform's own dialog.
fn native_dialog(folder: bool) -> Option<PathBuf> {
    let pick = |out: std::io::Result<std::process::Output>| -> Option<PathBuf> {
        let o = out.ok()?;
        let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
        (o.status.success() && !s.is_empty()).then(|| PathBuf::from(s))
    };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let script = if folder {
            "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.FolderBrowserDialog; \
             $d.Description = 'Frogger 2 disc or install folder'; if ($d.ShowDialog() -eq 'OK') { $d.SelectedPath }"
        } else {
            "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.OpenFileDialog; \
             $d.Title = 'Frogger 2 disc image'; \
             $d.Filter = 'Disc images|*.cue;*.bin;*.iso;*.img;*.ccd;*.mds;*.mdf;*.nrg;*.toc;*.zip|All files|*.*'; \
             if ($d.ShowDialog() -eq 'OK') { $d.FileName }"
        };
        pick(std::process::Command::new("powershell.exe").args(["-NoProfile", "-STA", "-Command", script]).creation_flags(0x0800_0000).output())
    }
    #[cfg(not(windows))]
    {
        let zen = if folder { vec!["--file-selection", "--directory", "--title=Frogger 2 disc or install folder"] } else { vec!["--file-selection", "--title=Frogger 2 disc image"] };
        if let Some(p) = pick(std::process::Command::new("zenity").args(&zen).output()) {
            return Some(p);
        }
        let kd = if folder { vec!["--getexistingdirectory", "."] } else { vec!["--getopenfilename", ".", "*.cue *.bin *.iso *.img *.ccd *.mds *.mdf *.nrg *.toc *.zip"] };
        pick(std::process::Command::new("kdialog").args(&kd).output())
    }
}

impl WindowApp for Screen {
    fn init(&mut self, window: &Arc<window::Window>) {
        if self.gfx.is_some() {
            return;
        }
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(window.clone())));
        let res = (|| -> Result<Gfx, String> {
            let surface = instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
            let ctx = GpuContext::with_instance(instance, Some(&surface)).map_err(|e| e.to_string())?;
            let surface = WindowSurface::new(&ctx, surface, size.width, size.height).map_err(|e| e.to_string())?;
            let mut r = Renderer::new(&ctx.device, &ctx.queue, surface.config.format);
            let font = r.add_font(builtin_font());
            Ok(Gfx { ctx, surface, r, font })
        })();
        match res {
            Ok(g) => self.gfx = Some(g),
            Err(e) => self.error = Some(e),
        }
    }

    fn event(&mut self, _w: &Arc<window::Window>, event: &winit::event::WindowEvent) -> Flow {
        use winit::event::{ElementState, MouseButton, WindowEvent};
        match event {
            WindowEvent::KeyboardInput { event: k, .. } if k.state == ElementState::Pressed => self.key(&k.logical_key, k.text.as_deref()),
            WindowEvent::DroppedFile(p) if matches!(self.status, Status::Menu) => self.start(Source::Path(p.clone())),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);
                if let Some(&(_, _, i)) = self.hit.iter().find(|(a, b, _)| self.cursor.1 >= *a && self.cursor.1 < *b) {
                    self.sel = i;
                }
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => match self.status {
                Status::Menu => {
                    if let Some(&(_, _, i)) = self.hit.iter().find(|(a, b, _)| self.cursor.1 >= *a && self.cursor.1 < *b) {
                        self.sel = i;
                        let it = self.items()[i].clone();
                        self.activate(it);
                    }
                }
                Status::Done(_) => {
                    self.imported = true;
                    self.exit = true;
                }
                Status::Failed(_) => self.status = Status::Menu,
                Status::Running { .. } => {}
            },
            _ => {}
        }
        Flow::Continue
    }

    fn resized(&mut self, _w: &Arc<window::Window>, size: winit::dpi::PhysicalSize<u32>) {
        if let Some(g) = &mut self.gfx {
            g.surface.resize(&g.ctx.device, size.width, size.height);
        }
    }

    fn frame(&mut self, frame: window::Frame<'_>) -> Flow {
        if self.error.is_some() || self.exit {
            return Flow::Exit;
        }
        self.poll();
        let Some((w, h)) = self.gfx.as_ref().map(|g| (g.surface.config.width, g.surface.config.height)) else { return Flow::Continue };
        self.draw(w, h);
        let g = self.gfx.as_mut().unwrap();
        let tex = match g.surface.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                let s = frame.window.inner_size();
                g.surface.resize(&g.ctx.device, s.width, s.height);
                return Flow::Continue;
            }
            _ => {
                std::thread::sleep(Duration::from_millis(5));
                return Flow::Continue;
            }
        };
        let view = tex.texture.create_view(&Default::default());
        g.r.render(&view, w, h, BG);
        frame.window.pre_present_notify();
        g.ctx.queue.present(tex);
        std::thread::sleep(Duration::from_millis(16));
        Flow::Continue
    }
}

impl Screen {
    fn new(data_root: PathBuf, reason: &str) -> Screen {
        Screen {
            data_root,
            reason: reason.to_string(),
            gfx: None,
            error: None,
            drives: Vec::new(),
            scan: None,
            sel: 0,
            path: String::new(),
            editing: false,
            dialog: None,
            job: None,
            status: Status::Menu,
            note: None,
            imported: false,
            exit: false,
            cursor: (0.0, 0.0),
            hit: Vec::new(),
        }
    }
}

/// Runs the import screen. Returns true when data was imported (the caller restarts).
pub fn run(cfg: &Config, reason: &str) -> bool {
    let mut wc = WindowConfig::from_config(cfg);
    wc.title = "Frogger 2: Swampy's Revenge - Import".into();
    wc.fullscreen = false;
    let mut s = Screen::new(cfg.data_root(), reason);
    s.rescan();
    match window::run(wc, s) {
        Ok(s) => {
            if let Some(e) = &s.error {
                eprintln!("frogger: import screen: {e}");
            }
            let _ = &s.reason;
            s.imported
        }
        Err(e) => {
            eprintln!("frogger: import screen: {e}");
            false
        }
    }
}

/// Starts the game again with the same arguments (now that data exists) and exits.
pub fn relaunch() -> ! {
    match std::env::current_exe() {
        Ok(exe) => match std::process::Command::new(exe).args(std::env::args_os().skip(1)).spawn() {
            Ok(_) => std::process::exit(0),
            Err(e) => eprintln!("frogger: restarting failed ({e}); please start the game again"),
        },
        Err(e) => eprintln!("frogger: restarting failed ({e}); please start the game again"),
    }
    std::process::exit(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_font_has_printable_ascii() {
        let f = builtin_font();
        assert_eq!((f.width, f.height), (570, 8));
        for c in 0x20u8..=0x7e {
            assert!(f.glyphs[c as usize].is_some(), "{}", c as char);
        }
        // 'A' column 0 = 0x7C: rows 2..=6 set.
        let a = f.glyphs[b'A' as usize].unwrap().left as usize;
        let px = |x: usize, y: usize| f.rgba[(y * f.width as usize + x) * 4 + 3];
        assert_eq!((px(a, 1), px(a, 2), px(a, 6), px(a, 7)), (0, 255, 255, 0));
        assert_eq!(f.string_width(b"ab", 1.0), 12);
    }

    /// Renders each state headless to `$F2_SHOT_DIR/firstrun-*.png` (skips without a GPU).
    #[test]
    fn screenshots() {
        let Some(dir) = std::env::var_os("F2_SHOT_DIR").map(PathBuf::from) else { return };
        let Ok(ctx) = GpuContext::new_headless() else { return };
        let mut r = Renderer::new(&ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm);
        let font = r.add_font(builtin_font());
        let mut s = Screen::new(PathBuf::from("/tmp/x"), "none");
        let drive = |dev: &str, label: Option<&str>, frogger: bool, media: bool| DriveInfo {
            device: dev.into(),
            root: None,
            label: label.map(String::from),
            media,
            frogger,
            kind: f2_media::cdrom::DriveKind::Physical,
            model: String::new(),
        };
        s.drives = vec![drive("D:", Some("FROGGER2"), true, true), drive("E:", Some("MUSIC"), false, true), drive("F:", None, false, false)];
        let mut shot = |s: &mut Screen, name: &str| {
            s.draw_with(&mut r, font, 1280, 720);
            r.render_to_image(BG).unwrap().write_png(dir.join(format!("firstrun-{name}.png"))).unwrap();
        };
        shot(&mut s, "menu");
        s.status = Status::Running { stage: "read CD audio".into(), fraction: 0.42, log: (0..30).map(|i| format!("log line {i}")).collect() };
        shot(&mut s, "running");
        let mut m = f2_import::installed(std::path::Path::new("/nonexistent")).pop().unwrap_or_else(|| Manifest {
            importer_version: "0".into(),
            rendition: "usa".into(),
            title: "Frogger 2: Swampy's Revenge (USA)".into(),
            verified: true,
            source: "D:".into(),
            source_kind: "drive".into(),
            cab_sha1: None,
            track1_sha1: None,
            redump_verified: None,
            music_tracks: vec![],
            music_source: "none".into(),
            music_note: None,
            rip: None,
            videos: vec![],
            stale_files: vec![],
            files: Default::default(),
        });
        m.music_note = Some(f2_import::NO_AUDIO_NOTE.into());
        s.status = Status::Done(m);
        shot(&mut s, "done");
        s.status = Status::Failed("FROGGER2/DATA1.CAB not found on disc".into());
        shot(&mut s, "failed");
    }

    #[test]
    fn wraps_words() {
        assert_eq!(wrap("aaaa bbbb cccc", 10), vec!["aaaa bbbb", "cccc"]);
    }
}
