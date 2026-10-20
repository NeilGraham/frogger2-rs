//! The desktop launcher (docs/USER-GUIDE.md "Launcher"): imports the game, starts it, picks
//! the version to run, installs updates from GitHub releases, manages mods, makes the game's
//! shortcut and edits the main settings. The screen is `f2-launcher`'s UI model drawn with
//! [`crate::uikit`]; the Android app draws the same model.
//!
//! `launcher(.exe)` runs this; `frogger2` runs it too when it finds no game and no launcher
//! beside it (development builds).
//!
//! Layout of an install folder (a release zip, `portable.txt` beside the exes):
//! `launcher`, `engine/` (the real binaries), `frogger2(.exe)` (made here once the game is
//! imported, with the game's own icon), `versions/<tag>/` (other installed releases), `data/`,
//! `mods/`, `saves/`, `config.toml`.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use crate::data::Data;
use crate::export::Target;
use crate::uikit::UiKit;
use f2_engine::{GpuContext, Renderer, WindowSurface};
use f2_import::{Event, Manifest, Options, Sink};
use f2_launcher::releases::{self, Release};
use f2_launcher::ui::{self, Action, Caps, ExportRow, ExportState, Launcher, ModRow, Progress, SettingGroup, SettingKind, SettingRow, SourceRow, VersionRow};
use f2_launcher::{install, mods};
use f2_media::cdrom::DriveInfo;
use f2_modkit::archive::{self, Installed};
use f2_platform::window::{self, winit, Flow, WindowApp, WindowConfig};
use f2_platform::{Config, Profile, WindowMode};

enum Msg {
    Releases(Result<Vec<Release>, String>),
    Progress(String, f32),
    Installed(Result<String, String>),
    Drives(Vec<DriveInfo>),
    /// The file dialog closed (`None`: nothing chosen).
    Picked(Option<PathBuf>),
    Import(Event),
    Imported(Result<Box<Manifest>, String>),
    /// The shortcut's save dialog closed (`None`: cancelled).
    ShortcutPath(Option<PathBuf>),
    /// A progress line of the export of `Target::ALL[i]`.
    ExportLog(usize, String),
    Exported(usize, Result<PathBuf, String>),
    /// The mod file dialog closed (`None`: nothing chosen).
    PickedMod(Option<PathBuf>),
    ModInstalled(Result<Box<Installed>, String>),
}

struct Gfx {
    ctx: GpuContext,
    surface: WindowSurface,
    r: Renderer,
    kit: UiKit,
}

/// A picture for the painter, waiting for the GPU.
type Pending = Option<(u32, u32, Vec<u8>)>;

struct App {
    root: PathBuf,
    cfg: Config,
    /// Where `cfg` is saved (`config.toml` in the settings folder).
    config_file: PathBuf,
    ui: Launcher,
    gfx: Option<Gfx>,
    error: Option<String>,
    releases: Vec<Release>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    checking: bool,
    installing: bool,
    /// Drives in the order the import page lists them.
    drives: Vec<DriveInfo>,
    scanning: bool,
    dialog_open: bool,
    importing: bool,
    /// A mod is being installed.
    installing_mod: bool,
    /// The game's pictures waiting to be given to the painter.
    hero_pending: Pending,
    logo_pending: Pending,
    icon_pending: Pending,
    /// The game's icon for the window (made from the artwork).
    window_icon: Option<crate::icon::Rgba>,
    art_loaded: bool,
    /// Per `Target::ALL`: where its export stands and its last line.
    exports: Vec<(ExportState, Option<String>)>,
    cursor: (f32, f32),
    exit: bool,
}

use crate::setup::install_root;

/// Where exports are written (as the game's own Export page).
fn exports_dir() -> PathBuf {
    f2_platform::paths::config_dir().join("exports")
}

/// What an export of `t` makes.
fn export_blurb(t: Target) -> &'static str {
    match t {
        Target::Windows => "A folder with frogger2.exe and your game, to copy to another PC",
        Target::MacOs => "An app with your game inside",
        Target::Linux => "An AppImage with your game inside",
        Target::Android => "An .apk with your game inside, to sideload",
        Target::Ps4 => "A fake package (.pkg) for a jailbroken PS4 or PS5",
        Target::Ps5 => "An app folder (FW 13.60 and lower) and fake package .pkg (11.60 and lower) for a jailbroken PS5",
        Target::N64 => "A .z64 ROM for flash carts and emulators (experimental)",
    }
}

/// Resolutions offered (`None` = the screen's own size).
const RESOLUTIONS: [Option<[u32; 2]>; 12] = [
    None,
    Some([640, 480]),
    Some([800, 600]),
    Some([1024, 768]),
    Some([1280, 960]),
    Some([1440, 1080]),
    Some([1600, 1200]),
    Some([1920, 1440]),
    Some([1280, 720]),
    Some([1920, 1080]),
    Some([2560, 1440]),
    Some([3840, 2160]),
];

const LANGUAGES: [(&str, &str); 5] = [("us", "English (US)"), ("uk", "English (UK)"), ("fr", "Fran\u{e7}ais"), ("de", "Deutsch"), ("it", "Italiano")];

fn res_label(r: Option<[u32; 2]>) -> String {
    match r {
        None => "Auto (screen size)".into(),
        Some(r) if r == f2_platform::config::Video::ORIGINAL_MAX => format!("{} \u{d7} {} (original)", r[0], r[1]),
        Some([w, h]) => format!("{w} \u{d7} {h}"),
    }
}

/// The resolutions to offer with the current one among them.
fn resolutions(current: Option<[u32; 2]>) -> Vec<Option<[u32; 2]>> {
    let mut v = RESOLUTIONS.to_vec();
    if !v.contains(&current) {
        v.insert(1, current);
    }
    v
}

fn volume_options() -> Vec<String> {
    (0..=10).map(|i| if i == 0 { "Off".to_string() } else { format!("{}%", i * 10) }).collect()
}

fn volume_index(v: u8) -> usize {
    ((v as usize + 5) / 10).min(10)
}

impl App {
    fn new() -> App {
        let cfg = Config::load_desktop().unwrap_or_else(|e| {
            eprintln!("launcher: config: {e} (using defaults)");
            Config::desktop()
        });
        let mut a = App::with(cfg, install_root());
        a.refresh();
        if a.ui.state.game.is_none() {
            a.rescan();
        }
        if a.cfg.launcher.check_updates && std::env::var_os("F2_NO_UPDATE_CHECK").is_none() {
            a.check_releases();
        }
        a
    }

    /// The launcher for the install folder `root` with the settings `cfg` (nothing looked up yet).
    fn with(cfg: Config, root: PathBuf) -> App {
        let caps = Caps {
            versions: true,
            import: true,
            drives: true,
            pick_folder: true,
            drop_files: true,
            open_folders: true,
            mod_files: true,
            export: true,
            updates_page: false,
            close_label: None,
        };
        let (tx, rx) = channel();
        let mut a = App {
            root,
            ui: Launcher::new(caps, 1100.0, 700.0, 1.0),
            cfg,
            config_file: f2_platform::paths::config_file(),
            gfx: None,
            error: None,
            releases: Vec::new(),
            tx,
            rx,
            checking: false,
            installing: false,
            drives: Vec::new(),
            scanning: false,
            dialog_open: false,
            importing: false,
            installing_mod: false,
            hero_pending: None,
            logo_pending: None,
            icon_pending: None,
            window_icon: None,
            art_loaded: false,
            exports: vec![(ExportState::Ready, None); Target::ALL.len()],
            cursor: (0.0, 0.0),
            exit: false,
        };
        a.ui.state.import.sources_title = "Disc drives".into();
        a.ui.state.import.empty_note = "No CD or DVD drives found.".into();
        a
    }

    fn selected_dir(&self) -> PathBuf {
        crate::setup::version_dir(&self.root, &self.cfg.launcher.version)
    }

    fn locate(&self) -> Option<Data> {
        Data::locate(&self.cfg, None).ok()
    }

    /// Loads the game's pictures (artwork, logo, icon) once there is a game.
    fn load_art(&mut self, d: &Data) {
        if let Some(art) = crate::icon::game_art(d) {
            let icon = crate::icon::rounded(&crate::icon::square(&art, 128), 0.2);
            self.window_icon = Some(crate::icon::square(&icon, 64));
            self.icon_pending = Some((icon.width, icon.height, icon.pixels));
            self.hero_pending = Some((art.width, art.height, art.pixels));
        }
        self.logo_pending = crate::icon::game_logo(d).map(|l| (l.width, l.height, l.pixels));
        self.art_loaded = true;
    }

    /// Rebuilds everything the UI shows from the config, the folders and the release list.
    fn refresh(&mut self) {
        let app_version = env!("CARGO_PKG_VERSION").to_string();
        let data = self.locate();
        match &data {
            Some(d) if !self.art_loaded => self.load_art(d),
            None => self.art_loaded = false,
            _ => {}
        }
        let game = data.as_ref().map(|d| crate::export::title(d).unwrap_or_else(|_| "Frogger 2: Swampy's Revenge".into()));
        let s = &mut self.ui.state;
        s.game = game;
        s.data_dir = self.cfg.data_root().display().to_string();
        s.mods_dir = self.cfg.mods_dir().display().to_string();
        s.mods = mods::list(&self.cfg.mods_dir())
            .into_iter()
            .map(|m| ModRow { enabled: self.cfg.mods.enabled.contains(&m.id), id: m.id, name: m.name, version: m.version })
            .collect();
        let made = self.cfg.launcher.shortcut.as_ref().is_some_and(|p| std::fs::symlink_metadata(p).is_ok());
        s.shortcut = (s.game.is_some() && crate::shortcut::supported()).then_some(made);
        s.exports_dir = exports_dir().display().to_string();
        s.exports = Target::ALL
            .iter()
            .zip(&self.exports)
            .map(|(&t, (state, line))| {
                let state = if Target::exportable().contains(&t) {
                    state.clone()
                } else {
                    let from = match t {
                        Target::Windows => "Windows",
                        Target::MacOs => "macOS",
                        _ => "Linux",
                    };
                    ExportState::Unavailable(format!("From {from} only"))
                };
                let detail = line.clone().unwrap_or_else(|| export_blurb(t).to_string());
                ExportRow { name: t.name().to_string(), detail, state }
            })
            .collect();
        let selected = self.cfg.launcher.version.clone();
        let installed = install::installed_tags(&self.root);
        let (os, arch) = (releases::host_os(), releases::host_arch());
        let mut rows = vec![VersionRow {
            tag: String::new(),
            label: format!("v{app_version} (this folder)"),
            detail: "The version that came with this launcher".into(),
            installed: true,
            selected: selected.is_empty() || !installed.contains(&selected),
            available: true,
        }];
        for r in &self.releases {
            rows.push(VersionRow {
                tag: r.tag.clone(),
                label: r.tag.clone(),
                detail: format!("{}{}", r.date, if r.prerelease { "  \u{b7}  pre-release" } else { "" }),
                installed: installed.contains(&r.tag),
                selected: selected == r.tag && installed.contains(&r.tag),
                available: r.asset_for(os, arch).is_some() && r.sums_asset().is_some(),
            });
        }
        for t in &installed {
            if !self.releases.iter().any(|r| &r.tag == t) {
                rows.push(VersionRow {
                    tag: t.clone(),
                    label: t.clone(),
                    detail: "Installed, no longer in the release list".into(),
                    installed: true,
                    selected: &selected == t,
                    available: false,
                });
            }
        }
        s.versions = rows;
        s.app_version = app_version.clone();
        s.update_available =
            self.releases.iter().find(|r| !r.prerelease && r.asset_for(os, arch).is_some() && releases::is_newer(&r.tag, &app_version)).map(|r| r.tag.clone());
        s.import.sources = self
            .drives
            .iter()
            .map(|d| SourceRow {
                label: d.device.clone(),
                detail: match (d.media, &d.label, d.model.is_empty()) {
                    (false, _, true) => "No disc".into(),
                    (false, _, false) => format!("No disc  \u{b7}  {}", d.model),
                    (true, Some(l), _) => format!("Disc: {l}"),
                    (true, None, _) => "A disc without a label".into(),
                },
                highlight: d.frogger,
                ready: d.media,
            })
            .collect();
        s.import.scanning = self.scanning;
        self.ui.state.settings = self.settings();
    }

    /// The settings page.
    fn settings(&self) -> Vec<SettingGroup> {
        let row = |id: &'static str, label: &str, detail: String, kind: SettingKind| SettingRow { id, label: label.into(), detail, kind };
        let choice = |options: Vec<String>, selected: usize| SettingKind::Choice { options, selected };
        let mut groups = Vec::new();
        let v = &self.cfg.video;
        let mut game = Vec::new();
        if let Some(t) = &self.ui.state.game {
            game.push(row("import", "Your game", t.clone(), SettingKind::Button("Import again".into())));
            if let Some(made) = self.ui.state.shortcut {
                let detail = match (&self.cfg.launcher.shortcut, made) {
                    (Some(p), true) => p.display().to_string(),
                    _ => format!("\"{}\", wherever you choose", crate::shortcut::NAME),
                };
                game.push(row("shortcut", "Shortcut", detail, SettingKind::Button(if made { "Add another" } else { "Add" }.into())));
            }
        } else {
            game.push(row("import", "Your game", "Not imported yet".into(), SettingKind::Button("Import".into())));
        }
        game.push(row("open_data", "Game data", self.cfg.data_root().display().to_string(), SettingKind::Button("Open".into())));
        game.push(row("open_config", "Settings, saves and mods", f2_platform::paths::config_dir().display().to_string(), SettingKind::Button("Open".into())));
        groups.push(SettingGroup { title: "Game".into(), rows: game });

        let modes = [WindowMode::Windowed, WindowMode::Borderless, WindowMode::Fullscreen];
        let res = resolutions(v.resolution);
        groups.push(SettingGroup {
            title: "Display".into(),
            rows: vec![
                row(
                    "mode",
                    "Window mode",
                    "Borderless covers the whole screen; Fullscreen also switches the monitor to the resolution.".into(),
                    choice(vec!["Windowed".into(), "Borderless".into(), "Fullscreen".into()], modes.iter().position(|m| *m == v.window_mode).unwrap_or(1)),
                ),
                row(
                    "res",
                    "Resolution",
                    "The game renders at this size, scaled to the screen; 4:3 sizes keep the original picture.".into(),
                    choice(res.iter().map(|r| res_label(*r)).collect(), res.iter().position(|r| *r == v.resolution).unwrap_or(0)),
                ),
                row("vsync", "VSync", "Waits for the display: no tearing.".into(), SettingKind::Toggle(v.vsync)),
                row(
                    "profile",
                    "Style",
                    "Enhanced adds fixes and sharper graphics; Authentic matches the 2000 release.".into(),
                    choice(vec!["Enhanced".into(), "Authentic".into()], (self.cfg.profile == Profile::Authentic) as usize),
                ),
            ],
        });
        let lang = LANGUAGES.iter().position(|(c, _)| c.eq_ignore_ascii_case(&self.cfg.language)).unwrap_or(0);
        groups.push(SettingGroup {
            title: "Sound and text".into(),
            rows: vec![
                row("music", "Music", String::new(), choice(volume_options(), volume_index(self.cfg.audio.music))),
                row("sfx", "Sound effects", String::new(), choice(volume_options(), volume_index(self.cfg.audio.sfx))),
                row("language", "Text language", String::new(), choice(LANGUAGES.iter().map(|(_, n)| n.to_string()).collect(), lang)),
                row("intro", "Skip the intro logos", String::new(), SettingKind::Toggle(self.cfg.gameplay.skip_intro_logos)),
            ],
        });
        groups.push(SettingGroup {
            title: "Launcher".into(),
            rows: vec![
                row(
                    "start",
                    "Starting frogger2 opens",
                    "The launcher is always one click away: launcher(.exe).".into(),
                    choice(vec!["The game".into(), "This launcher".into()], self.cfg.launcher.start_in_launcher() as usize),
                ),
                row(
                    "updates",
                    "Look for new versions",
                    "Asks GitHub once when the launcher starts.".into(),
                    SettingKind::Toggle(self.cfg.launcher.check_updates),
                ),
            ],
        });
        groups
    }

    fn set_choice(&mut self, id: &str, i: usize) {
        let c = &mut self.cfg;
        match id {
            "mode" => c.video.select_mode([WindowMode::Windowed, WindowMode::Borderless, WindowMode::Fullscreen][i.min(2)]),
            "res" => {
                let res = resolutions(c.video.resolution);
                if let Some(r) = res.get(i) {
                    c.video.resolution = *r;
                    if let Some([w, h]) = r {
                        (c.video.width, c.video.height) = (*w, *h);
                    }
                }
            }
            "profile" => c.profile = if i == 1 { Profile::Authentic } else { Profile::Enhanced },
            "music" => c.audio.music = (i.min(10) * 10) as u8,
            "sfx" => c.audio.sfx = (i.min(10) * 10) as u8,
            "language" => c.language = LANGUAGES[i.min(LANGUAGES.len() - 1)].0.into(),
            "start" => c.launcher.start = if i == 1 { "launcher" } else { "game" }.into(),
            _ => return,
        }
        self.save_config();
        self.refresh();
    }

    fn set_toggle(&mut self, id: &str, on: bool) {
        match id {
            "vsync" => self.cfg.video.vsync = on,
            "intro" => self.cfg.gameplay.skip_intro_logos = on,
            "updates" => self.cfg.launcher.check_updates = on,
            _ => return,
        }
        self.save_config();
        self.refresh();
    }

    fn setting_button(&mut self, id: &str) {
        match id {
            "import" => {
                self.ui.show_import(true);
                self.rescan();
            }
            "shortcut" => self.create_shortcut(),
            "open_data" => self.open_folder(&self.cfg.data_root()),
            "open_config" => self.open_folder(&f2_platform::paths::config_dir()),
            _ => {}
        }
    }

    fn releases_url(&self) -> String {
        std::env::var("F2_RELEASES_URL")
            .ok()
            .filter(|u| !u.is_empty())
            .or_else(|| self.cfg.launcher.releases_url.clone())
            .unwrap_or_else(|| releases::DEFAULT_RELEASES_URL.to_string())
    }

    fn check_releases(&mut self) {
        if self.checking {
            return;
        }
        self.checking = true;
        self.ui.state.versions_note = "Checking GitHub...".into();
        let (url, tx) = (self.releases_url(), self.tx.clone());
        std::thread::spawn(move || {
            let r = install::fetch_text(&url).and_then(|t| releases::parse_releases(&t));
            let _ = tx.send(Msg::Releases(r));
        });
    }

    fn save_config(&mut self) {
        if let Err(e) = self.cfg.save(&self.config_file) {
            self.say(&format!("Saving the settings failed: {e}"), true);
        }
    }

    fn say(&mut self, msg: &str, bad: bool) {
        eprintln!("launcher: {msg}");
        self.ui.state.message = Some((msg.to_string(), bad));
    }

    fn poll(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Releases(r) => {
                    self.checking = false;
                    match r {
                        Ok(rs) => {
                            self.ui.state.versions_note = match rs.len() {
                                1 => "1 release".into(),
                                n => format!("{n} releases"),
                            };
                            self.releases = rs;
                        }
                        Err(e) => self.ui.state.versions_note = format!("Offline: {e}"),
                    }
                    self.refresh();
                }
                Msg::Progress(what, f) => self.ui.state.busy = Some((what, f)),
                Msg::Installed(r) => {
                    self.installing = false;
                    self.ui.state.busy = None;
                    match r {
                        Ok(tag) => self.say(&format!("Installed {tag}. Choose Use to run it."), false),
                        Err(e) => self.say(&e, true),
                    }
                    self.refresh();
                }
                Msg::Drives(d) => {
                    self.scanning = false;
                    // Frogger discs first, then drives with a disc, then the empty ones.
                    let mut d = d;
                    d.sort_by_key(|d| (!d.frogger, !d.media));
                    self.drives = d;
                    self.refresh();
                }
                Msg::Picked(p) => {
                    self.dialog_open = false;
                    match p {
                        Some(p) => self.start_import(ImportFrom::Path(p)),
                        None => self.ui.state.import.note = Some("Nothing was chosen.".into()),
                    }
                }
                Msg::Import(e) => {
                    if let Some(run) = &mut self.ui.state.import.running {
                        match e {
                            Event::Progress { stage, fraction } => {
                                run.stage = stage;
                                run.fraction = fraction;
                            }
                            Event::Log(l) => {
                                run.log.push(l);
                                if run.log.len() > 50 {
                                    run.log.remove(0);
                                }
                            }
                        }
                    }
                }
                Msg::Imported(r) => self.import_done(r),
                Msg::ShortcutPath(p) => {
                    self.dialog_open = false;
                    if let Some(p) = p {
                        self.make_shortcut(&p);
                    }
                }
                Msg::ExportLog(i, l) => {
                    if let Some(e) = self.exports.get_mut(i) {
                        e.1 = Some(l);
                    }
                    self.refresh();
                }
                Msg::Exported(i, r) => {
                    self.exports[i] = match r {
                        Ok(p) => {
                            self.say(&format!("Exported for {}: {}", Target::ALL[i].name(), p.display()), false);
                            (ExportState::Done, Some(format!("Done: {}", p.display())))
                        }
                        Err(e) => (ExportState::Failed, Some(format!("Failed: {e}"))),
                    };
                    self.refresh();
                }
                Msg::PickedMod(p) => {
                    self.dialog_open = false;
                    if let Some(p) = p {
                        self.import_mod(p);
                    }
                }
                Msg::ModInstalled(r) => self.mod_installed(r.map(|i| *i)),
            }
        }
    }

    fn rescan(&mut self) {
        if self.scanning {
            return;
        }
        self.scanning = true;
        self.ui.state.import.scanning = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Drives(f2_media::cdrom::list_drives()));
        });
    }

    fn pick(&mut self, folder: bool) {
        if self.dialog_open || self.importing {
            return;
        }
        self.dialog_open = true;
        self.ui.state.import.note =
            Some(if folder { "Choose the folder in the window that opened." } else { "Choose the disc image in the window that opened." }.into());
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Picked(native_dialog(if folder { Pick::Folder } else { Pick::Image })));
        });
    }

    /// "Import mod": asks for a mod package (or a folder's mod.toml) with the system's dialog.
    fn pick_mod(&mut self) {
        if self.dialog_open || self.installing_mod {
            return;
        }
        self.dialog_open = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::PickedMod(native_dialog(Pick::Mod)));
        });
    }

    /// Installs the mod package or folder `src` on a worker thread (`pack`'s checks decode
    /// every file); [`App::mod_installed`] turns it on.
    fn import_mod(&mut self, src: PathBuf) {
        if self.installing_mod {
            return;
        }
        let src = archive::source_of(&src);
        self.installing_mod = true;
        self.ui.show_tab(ui::Tab::Mods);
        let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| src.display().to_string());
        self.ui.state.busy = Some((format!("Installing the mod {name}"), 0.5));
        let (dir, tx) = (self.cfg.mods_dir(), self.tx.clone());
        std::thread::spawn(move || {
            let r = archive::install(&src, &dir).map(Box::new).map_err(|e| e.to_string());
            let _ = tx.send(Msg::ModInstalled(r));
        });
    }

    fn mod_installed(&mut self, r: Result<Installed, String>) {
        self.installing_mod = false;
        self.ui.state.busy = None;
        match r {
            Ok(i) => {
                let msg = enable_installed(&mut self.cfg, &i);
                self.save_config();
                self.refresh();
                self.say(&msg, false);
            }
            Err(e) => self.say(&format!("The mod was not installed: {e}"), true),
        }
    }

    fn remove_mod(&mut self, id: &str) {
        match archive::remove(&self.cfg.mods_dir(), id) {
            Ok(backup) => {
                self.cfg.mods.enabled.retain(|m| m != id);
                self.save_config();
                self.refresh();
                self.say(&format!("Removed {id} (a copy is kept in {}).", backup.display()), false);
            }
            Err(e) => self.say(&e.to_string(), true),
        }
    }

    fn start_import(&mut self, from: ImportFrom) {
        if self.importing {
            return;
        }
        self.importing = true;
        let title = match &from {
            ImportFrom::Path(p) => format!("Importing {}", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())),
            ImportFrom::Drive(d) => format!("Importing the disc in {}", d.device),
        };
        let imp = &mut self.ui.state.import;
        imp.running = Some(Progress { title, stage: "Starting".into(), fraction: 0.0, log: Vec::new() });
        imp.error = None;
        imp.note = None;
        let (root, tx) = (self.cfg.data_root(), self.tx.clone());
        let events = self.tx.clone();
        let sink = Sink(Arc::new(move |e| {
            let _ = events.send(Msg::Import(e));
        }));
        let opts = Options { sink: Some(sink), ..Default::default() };
        std::thread::spawn(move || {
            let r = match from {
                ImportFrom::Path(p) => f2_import::import(&p, &root, &opts),
                ImportFrom::Drive(d) => f2_import::import_drive(&d, &root, &opts),
            };
            let _ = tx.send(Msg::Imported(r.map(Box::new).map_err(|e| e.to_string())));
        });
    }

    /// The import finished: show the game, make its executable.
    fn import_done(&mut self, r: Result<Box<Manifest>, String>) {
        self.importing = false;
        self.ui.state.import.running = None;
        let m = match r {
            Ok(m) => m,
            Err(e) => {
                self.ui.state.import.error = Some(e);
                return;
            }
        };
        self.art_loaded = false;
        self.ui.show_import(false);
        self.refresh();
        let Some(d) = self.locate() else {
            return self.say(&format!("Imported {}, but the game was not found in {}.", m.title, self.cfg.data_root().display()), true);
        };
        if let Err(e) = crate::setup::ensure_game_exe(&self.selected_dir(), &d, &|l| eprintln!("launcher: {l}")) {
            return self.say(&e, true);
        }
        let music = m.music_note.as_deref().map(|n| format!(" {n}")).unwrap_or_default();
        self.say(&format!("Imported {}. Press PLAY.{music}", m.title), false);
        self.refresh();
    }

    /// "Add shortcut": asks where (the system's save dialog), then makes it.
    fn create_shortcut(&mut self) {
        if self.locate().is_none() {
            return self.say("Import your game first.", true);
        }
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::ShortcutPath(crate::shortcut::ask_path()));
        });
    }

    fn make_shortcut(&mut self, dest: &Path) {
        let Some(d) = self.locate() else { return };
        let exe = match crate::setup::ensure_game_exe(&self.root, &d, &|l| eprintln!("launcher: {l}")) {
            Ok(e) => e,
            Err(e) => return self.say(&e, true),
        };
        let art = crate::icon::game_art(&d).map(|a| crate::icon::rounded(&crate::icon::square(&a, 256), 0.18));
        match crate::shortcut::create(&exe, dest, art.as_ref()) {
            Ok(()) => {
                self.cfg.launcher.shortcut = Some(dest.to_path_buf());
                self.save_config();
                self.say(&format!("Made the shortcut: {}", dest.display()), false);
            }
            Err(e) => self.say(&e, true),
        }
        self.refresh();
    }

    /// Exports for `Target::ALL[i]` on a worker thread (one at a time).
    fn export(&mut self, i: usize) {
        let Some(&target) = Target::ALL.get(i) else { return };
        if self.exports.iter().any(|e| e.0 == ExportState::Working) || !Target::exportable().contains(&target) {
            return;
        }
        let Some(data) = self.locate() else { return self.say("Import your game first.", true) };
        // This desktop's own build is made from the game the launcher runs.
        let runtime = if Some(target) == Target::host() {
            match install::engine_exe(&self.selected_dir()).map(|e| crate::export::host_runtime_for(&e)) {
                Some(Ok(r)) => Some(r),
                Some(Err(e)) => return self.say(&e, true),
                None => None,
            }
        } else {
            None
        };
        self.exports[i] = (ExportState::Working, Some(format!("Starting the {} export...", target.name())));
        self.refresh();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let log = |l: &str| {
                eprintln!("launcher: export: {l}");
                let _ = tx.send(Msg::ExportLog(i, l.to_string()));
            };
            let r = crate::export::export(&data, target, runtime.as_deref(), &exports_dir(), &log);
            let _ = tx.send(Msg::Exported(i, r));
        });
    }

    /// Starts the launcher of the version just chosen (a new version brings its own launcher)
    /// and closes this one; false when this one already is it.
    fn restart_in_chosen_version(&mut self) -> bool {
        let Some(dir) = crate::setup::other_version(&self.root, &self.cfg.launcher.version) else { return false };
        let Some(l) = crate::setup::launcher_exe(&dir) else { return false };
        match std::process::Command::new(&l).env("F2_HANDED_OVER", "1").current_dir(&dir).spawn() {
            Ok(_) => {
                self.exit = true;
                true
            }
            Err(e) => {
                self.say(&format!("Starting {} failed: {e}", l.display()), true);
                false
            }
        }
    }

    fn open_folder(&mut self, dir: &Path) {
        let _ = std::fs::create_dir_all(dir);
        let opener = if cfg!(windows) {
            "explorer"
        } else if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        if let Err(e) = std::process::Command::new(opener).arg(dir).spawn() {
            self.say(&format!("Opening {} failed: {e}", dir.display()), true);
        }
    }

    fn act(&mut self, a: Action) {
        match a {
            Action::None | Action::Close | Action::OpenUpdates => {}
            Action::Play => self.play(),
            Action::Import | Action::RescanDrives => self.rescan(),
            Action::CancelImport => self.ui.state.import.note = None,
            Action::RetryImport => {
                self.ui.state.import.error = None;
                self.rescan();
            }
            Action::ImportSource(i) => {
                if let Some(d) = self.drives.get(i).cloned() {
                    self.start_import(ImportFrom::Drive(d));
                }
            }
            Action::Export(i) => self.export(i),
            Action::OpenExportsFolder => self.open_folder(&exports_dir()),
            Action::PickImage => self.pick(false),
            Action::PickFolder => self.pick(true),
            Action::CreateShortcut => self.create_shortcut(),
            Action::SetChoice(id, i) => self.set_choice(id, i),
            Action::SetToggle(id, on) => self.set_toggle(id, on),
            Action::SettingButton(id) => self.setting_button(id),
            Action::RefreshReleases => self.check_releases(),
            Action::Use(tag) => {
                self.cfg.launcher.version = tag.clone();
                self.save_config();
                if self.restart_in_chosen_version() {
                    return;
                }
                self.refresh();
                self.say(&format!("PLAY now runs {}.", if tag.is_empty() { "the version in this folder" } else { &tag }), false);
            }
            Action::Remove(tag) => match install::remove(&self.root, &tag) {
                Ok(()) => {
                    self.say(&format!("Removed {tag}."), false);
                    self.refresh();
                }
                Err(e) => self.say(&e, true),
            },
            Action::Install(tag) => self.install(&tag),
            Action::ToggleMod(id) => {
                let en = &mut self.cfg.mods.enabled;
                match en.iter().position(|m| *m == id) {
                    Some(i) => {
                        en.remove(i);
                    }
                    None => en.push(id),
                }
                self.save_config();
                self.refresh();
            }
            Action::OpenModsFolder => self.open_folder(&self.cfg.mods_dir()),
            Action::ImportMod => self.pick_mod(),
            Action::RemoveMod(id) => self.remove_mod(&id),
        }
    }

    fn install(&mut self, tag: &str) {
        let Some(rel) = self.releases.iter().find(|r| r.tag == tag).cloned() else { return };
        if self.installing {
            return;
        }
        self.installing = true;
        self.ui.state.busy = Some((format!("Installing {tag}"), 0.0));
        let (root, tx) = (self.root.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let report = |what: &str, f: f32| {
                let _ = tx.send(Msg::Progress(what.to_string(), f));
            };
            let r = install::install(&root, &rel, releases::host_os(), releases::host_arch(), &report).map(|_| rel.tag.clone());
            let _ = tx.send(Msg::Installed(r));
        });
    }

    fn play(&mut self) {
        let Some(data) = self.locate() else { return self.say("Import your game first.", true) };
        let dir = self.selected_dir();
        let exe = match crate::setup::ensure_game_exe(&dir, &data, &|l| eprintln!("launcher: {l}")) {
            Ok(e) => e,
            Err(e) => return self.say(&e, true),
        };
        match std::process::Command::new(&exe).arg("--game").current_dir(&dir).spawn() {
            Ok(_) => self.exit = true,
            Err(e) => self.say(&format!("Starting {} failed: {e}", exe.display()), true),
        }
    }
}

enum ImportFrom {
    Path(PathBuf),
    Drive(DriveInfo),
}

/// Turns the mod just installed on (appended to `[mods] enabled`: loads last); what to tell
/// the player. Warnings from the checks go to the log.
pub fn enable_installed(cfg: &mut Config, i: &Installed) -> String {
    let m = &i.manifest;
    for w in &i.warnings {
        eprintln!("launcher: mod {}: warning: {w}", m.id);
    }
    if !cfg.mods.enabled.contains(&m.id) {
        cfg.mods.enabled.push(m.id.clone());
    }
    let version = if m.version.is_empty() { String::new() } else { format!(" {}", m.version) };
    let mut msg = match (&i.replaced, &i.backup) {
        (Some(old), Some(b)) => {
            let (what, was) = match old.as_str() {
                o if o == m.version => ("Reinstalled", String::new()),
                "" => ("Updated", String::new()),
                o => ("Updated", format!(" (was {o})")),
            };
            format!("{what} {}{version}{was} and turned it on. The old copy is in {}.", m.name, b.display())
        }
        _ => format!("Installed {}{version} and turned it on.", m.name),
    };
    if !i.warnings.is_empty() {
        msg += &format!(" {} warning(s) in the log.", i.warnings.len());
    }
    msg
}

/// `frogger2 --import-mod FILE`: installs a mod package or folder into the mods folder, turns
/// it on and saves the settings; the exit code.
pub fn import_mod_cli(cfg: &mut Config, src: &Path) -> i32 {
    match archive::install(&archive::source_of(src), &cfg.mods_dir()) {
        Ok(i) => {
            let msg = enable_installed(cfg, &i);
            if let Err(e) = cfg.save_default() {
                eprintln!("frogger: saving the settings failed: {e}");
                return 1;
            }
            println!("{msg} ({})", i.dir.display());
            0
        }
        Err(e) => {
            eprintln!("frogger: the mod was not installed: {e}");
            1
        }
    }
}

/// The mod to install from the command line: `--import-mod FILE`, or a lone mod package or
/// folder (a `.f2mod` opened with the program).
pub fn mod_arg(args: impl IntoIterator<Item = std::ffi::OsString>) -> Option<PathBuf> {
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if a == "--import-mod" {
            return it.next().map(PathBuf::from);
        }
        let p = PathBuf::from(&a);
        if !a.to_string_lossy().starts_with('-') && archive::is_mod_source(&p) {
            return Some(p);
        }
    }
    None
}

/// What a system file dialog asks for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pick {
    Image,
    Folder,
    /// A `.f2mod`/`.zip`, or a folder's `mod.toml`.
    Mod,
}

/// Asks for a disc image, a folder or a mod with the system's own dialog.
fn native_dialog(what: Pick) -> Option<PathBuf> {
    let pick = |out: std::io::Result<std::process::Output>| -> Option<PathBuf> {
        let o = out.ok()?;
        let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
        (o.status.success() && !s.is_empty()).then(|| PathBuf::from(s))
    };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let script = match what {
            Pick::Folder => {
                "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.FolderBrowserDialog; \
                 $d.Description = 'Frogger 2 disc or installed game'; if ($d.ShowDialog() -eq 'OK') { $d.SelectedPath }"
            }
            Pick::Image => {
                "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.OpenFileDialog; \
                 $d.Title = 'Frogger 2 disc image'; \
                 $d.Filter = 'Disc images|*.cue;*.bin;*.iso;*.img;*.ccd;*.mds;*.mdf;*.nrg;*.toc;*.zip|All files|*.*'; \
                 if ($d.ShowDialog() -eq 'OK') { $d.FileName }"
            }
            Pick::Mod => {
                "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.OpenFileDialog; \
                 $d.Title = 'Frogger 2 mod (a .f2mod or .zip, or the mod.toml in a mod folder)'; \
                 $d.Filter = 'Frogger 2 mods|*.f2mod;*.zip;mod.toml|All files|*.*'; \
                 if ($d.ShowDialog() -eq 'OK') { $d.FileName }"
            }
        };
        pick(std::process::Command::new("powershell.exe").args(["-NoProfile", "-STA", "-Command", script]).creation_flags(0x0800_0000).output())
    }
    #[cfg(target_os = "macos")]
    {
        let script = match what {
            Pick::Folder => "POSIX path of (choose folder with prompt \"Frogger 2 disc or installed game\")",
            Pick::Image => "POSIX path of (choose file with prompt \"Frogger 2 disc image\")",
            Pick::Mod => "POSIX path of (choose file with prompt \"Frogger 2 mod (.f2mod, .zip, or a mod folder's mod.toml)\")",
        };
        pick(std::process::Command::new("osascript").args(["-e", script]).output())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let zen = match what {
            Pick::Folder => vec!["--file-selection", "--directory", "--title=Frogger 2 disc or installed game"],
            Pick::Image => vec![
                "--file-selection",
                "--title=Frogger 2 disc image",
                "--file-filter=Disc images | *.cue *.bin *.iso *.img *.ccd *.mds *.mdf *.nrg *.toc *.zip",
                "--file-filter=All files | *",
            ],
            Pick::Mod => vec![
                "--file-selection",
                "--title=Frogger 2 mod (.f2mod, .zip, or a mod folder's mod.toml)",
                "--file-filter=Frogger 2 mods | *.f2mod *.zip mod.toml",
                "--file-filter=All files | *",
            ],
        };
        if let Some(p) = pick(std::process::Command::new("zenity").args(&zen).output()) {
            return Some(p);
        }
        let kd = match what {
            Pick::Folder => vec!["--getexistingdirectory", "."],
            Pick::Image => vec!["--getopenfilename", ".", "*.cue *.bin *.iso *.img *.ccd *.mds *.mdf *.nrg *.toc *.zip"],
            Pick::Mod => vec!["--getopenfilename", ".", "*.f2mod *.zip mod.toml"],
        };
        pick(std::process::Command::new("kdialog").args(&kd).output())
    }
}

impl WindowApp for App {
    fn init(&mut self, window: &Arc<window::Window>) {
        if self.gfx.is_some() {
            return;
        }
        let icon = self.window_icon.clone().unwrap_or_else(|| crate::icon::square(&crate::icon::app_icon(), 64));
        if let Ok(i) = winit::window::Icon::from_rgba(icon.pixels, icon.width, icon.height) {
            window.set_window_icon(Some(i));
        }
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(window.clone())));
        let res = (|| -> Result<Gfx, String> {
            let surface = instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
            let ctx = GpuContext::with_instance(instance, Some(&surface)).map_err(|e| e.to_string())?;
            let surface = WindowSurface::new(&ctx, surface, size.width, size.height).map_err(|e| e.to_string())?;
            let r = Renderer::new(&ctx.device, &ctx.queue, surface.config.format);
            Ok(Gfx { ctx, surface, r, kit: UiKit::new() })
        })();
        match res {
            Ok(g) => self.gfx = Some(g),
            Err(e) => self.error = Some(e),
        }
    }

    fn event(&mut self, window: &Arc<window::Window>, event: &winit::event::WindowEvent) -> Flow {
        use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);
                self.ui.pointer_moved(self.cursor.0, self.cursor.1);
            }
            WindowEvent::CursorLeft { .. } => self.ui.pointer_left(),
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                let a = self.ui.press(self.cursor.0, self.cursor.1);
                self.act(a);
                window.request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let px = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -y * 60.0 * window.scale_factor() as f32,
                    MouseScrollDelta::PixelDelta(p) => -p.y as f32,
                };
                self.ui.scroll_by(px);
            }
            // A mod package or folder (a .zip only on the Mods page: it may be a disc image).
            WindowEvent::DroppedFile(p)
                if archive::is_mod_source(p) && (self.ui.tab == ui::Tab::Mods || !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"))) =>
            {
                self.import_mod(p.clone());
            }
            WindowEvent::DroppedFile(p) if self.ui.state.import.running.is_none() => {
                self.ui.show_import(true);
                self.start_import(ImportFrom::Path(p.clone()));
            }
            WindowEvent::Focused(true) => {
                // Back from the file manager or the desktop: the shortcut, the mods folder or a
                // disc may have changed.
                self.refresh();
            }
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
            if let Some(e) = &self.error {
                eprintln!("launcher: {e}");
            }
            return Flow::Exit;
        }
        self.poll();
        let Some(g) = self.gfx.as_mut() else { return Flow::Continue };
        let (w, h) = (g.surface.config.width, g.surface.config.height);
        self.ui.resize(w as f32, h as f32, frame.window.scale_factor() as f32);
        for (key, pending, size) in [
            (ui::HERO_KEY, &mut self.hero_pending, &mut self.ui.state.hero),
            (ui::LOGO_KEY, &mut self.logo_pending, &mut self.ui.state.logo),
            (ui::ICON_KEY, &mut self.icon_pending, &mut self.ui.state.icon),
        ] {
            if let Some((iw, ih, px)) = pending.take() {
                g.kit.set_image(&mut g.r, key, iw, ih, px);
                *size = Some((iw, ih));
            }
        }
        if let Some(i) = self.window_icon.take() {
            if let Ok(icon) = winit::window::Icon::from_rgba(i.pixels, i.width, i.height) {
                frame.window.set_window_icon(Some(icon));
            }
        }
        g.r.begin_frame(w, h);
        self.ui.draw(&mut g.kit.painter(&mut g.r));
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
        let bg = ui::BG;
        g.r.render(&view, w, h, [bg[0] as f64 / 255.0, bg[1] as f64 / 255.0, bg[2] as f64 / 255.0, 1.0]);
        frame.window.pre_present_notify();
        g.ctx.queue.present(tex);
        std::thread::sleep(Duration::from_millis(16));
        Flow::Continue
    }
}

/// Runs the launcher window until it closes (or starts the game). Returns false when the
/// window could not be opened. `launcher FILE.f2mod` / `launcher --import-mod FILE` opens on
/// the Mods page installing that mod.
pub fn run() -> bool {
    run_with(mod_arg(std::env::args_os().skip(1)))
}

/// [`run`], installing the mod `import` (if any) once the window is up.
pub fn run_with(import: Option<PathBuf>) -> bool {
    crate::setup::attach_parent_console();
    // The version chosen in the launcher brings its own launcher: start that one instead
    // (mods and settings are shared, so a mod to install is installed here).
    let cfg = Config::load_desktop().unwrap_or_default();
    let root = install_root();
    if import.is_none() && crate::setup::hand_over(crate::setup::other_version(&root, &cfg.launcher.version).and_then(|d| crate::setup::launcher_exe(&d)), &[])
    {
        return true;
    }
    let mut app = App::new();
    if let Some(p) = import {
        app.import_mod(p);
    }
    let mut wc = WindowConfig::from_config(&app.cfg);
    wc.title = "Frogger 2: Swampy's Revenge (Rust) - Launcher".into();
    wc.mode = WindowMode::Windowed;
    wc.resolution = None;
    wc.position = None;
    wc.width = 1100;
    wc.height = 700;
    match window::run(wc, app) {
        Ok(_) => true,
        Err(e) => {
            eprintln!("launcher: {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolutions_keep_the_current_one() {
        assert_eq!(resolutions(Some([1024, 768])).len(), RESOLUTIONS.len());
        let v = resolutions(Some([1366, 768]));
        assert_eq!(v[1], Some([1366, 768]));
        assert_eq!(res_label(Some([1024, 768])), "1024 \u{d7} 768 (original)");
        assert_eq!(res_label(None), "Auto (screen size)");
        assert_eq!((volume_index(100), volume_index(0), volume_index(74)), (10, 0, 7));
    }

    fn sample() -> Launcher {
        let caps = Caps {
            versions: true,
            import: true,
            drives: true,
            pick_folder: true,
            drop_files: true,
            open_folders: true,
            mod_files: true,
            export: true,
            updates_page: false,
            close_label: None,
        };
        let mut l = Launcher::new(caps, 1280.0, 760.0, 1.25);
        l.state.game = Some("Frogger 2: Swampy's Revenge".into());
        l.state.app_version = "0.8.1".into();
        l.state.update_available = Some("v0.9.0".into());
        l.state.data_dir = "C:\\Games\\frogger2-rs\\data".into();
        l.state.mods_dir = "C:\\Games\\frogger2-rs\\mods".into();
        l.state.shortcut = Some(false);
        l.state.versions_note = "3 releases".into();
        l.state.versions = vec![
            VersionRow {
                tag: String::new(),
                label: "v0.8.1 (this folder)".into(),
                detail: "The version that came with this launcher".into(),
                installed: true,
                selected: true,
                available: true,
            },
            VersionRow { tag: "v0.9.0".into(), label: "v0.9.0".into(), detail: "2026-11-01".into(), installed: false, selected: false, available: true },
            VersionRow {
                tag: "v0.8.5".into(),
                label: "v0.8.5".into(),
                detail: "2026-10-28  \u{b7}  pre-release".into(),
                installed: true,
                selected: false,
                available: true,
            },
            VersionRow { tag: "v0.8.0".into(), label: "v0.8.0".into(), detail: "2026-10-16".into(), installed: false, selected: false, available: false },
        ];
        l.state.mods = vec![
            ModRow { id: "hello-mod".into(), name: "Hello, mod!".into(), version: "0.1.0".into(), enabled: true },
            ModRow { id: "dusk".into(), name: "Dusk garden".into(), version: "".into(), enabled: false },
        ];
        l.state.import.sources_title = "Disc drives".into();
        l.state.import.sources = vec![
            SourceRow { label: "D:".into(), detail: "Disc: FROGGER2".into(), highlight: true, ready: true },
            SourceRow { label: "E:".into(), detail: "No disc  \u{b7}  HL-DT-ST DVDRAM".into(), highlight: false, ready: false },
        ];
        l
    }

    /// Renders the pages headless to `$F2_SHOT_DIR/launcher-*.png` (skips without a GPU).
    /// With `F2_SHOT_DATA=<your imported game>` the real artwork, logo and icon are used.
    #[test]
    fn screenshots() {
        let Some(dir) = std::env::var_os("F2_SHOT_DIR").map(PathBuf::from) else { return };
        let Ok(ctx) = GpuContext::new_headless() else { return };
        let mut r = Renderer::new(&ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm);
        let mut kit = UiKit::new();
        let mut l = sample();
        let mut app = App::with(Config::desktop(), PathBuf::from("C:\\Games\\frogger2-rs"));
        app.art_loaded = true;
        app.ui.state.game = l.state.game.clone();
        app.ui.state.shortcut = Some(false);
        l.state.settings = app.settings();
        app.exports[3] = (ExportState::Done, Some("Done: C:\\Games\\frogger2-rs\\exports\\frogger2-rs-android.apk".into()));
        app.exports[4] = (ExportState::Working, Some("Signing the package...".into()));
        app.exports[6] = (ExportState::Failed, Some("Failed: downloading the runtime failed (offline?)".into()));
        app.refresh();
        l.state.exports = app.ui.state.exports.clone();
        l.state.exports_dir = app.ui.state.exports_dir.clone();
        if let Some(d) = std::env::var_os("F2_SHOT_DATA").and_then(|p| Data::locate(&Config::default(), Some(PathBuf::from(p))).ok()) {
            app.load_art(&d);
            for (key, pending, size) in [
                (ui::HERO_KEY, app.hero_pending.take(), &mut l.state.hero),
                (ui::LOGO_KEY, app.logo_pending.take(), &mut l.state.logo),
                (ui::ICON_KEY, app.icon_pending.take(), &mut l.state.icon),
            ] {
                if let Some((w, h, px)) = pending {
                    kit.set_image(&mut r, key, w, h, px);
                    *size = Some((w, h));
                }
            }
        }
        let bg = ui::BG;
        let clear = [bg[0] as f64 / 255.0, bg[1] as f64 / 255.0, bg[2] as f64 / 255.0, 1.0];
        let (sw, sh, dp) = (1375u32, 875u32, 1.25f32);
        let mut shot = |l: &mut Launcher, name: &str| {
            r.begin_frame(sw, sh);
            l.resize(sw as f32, sh as f32, dp);
            l.draw(&mut kit.painter(&mut r));
            r.render_to_image(clear).unwrap().write_png(dir.join(format!("launcher-{name}.png"))).unwrap();
        };
        for (tab, name) in
            [(ui::Tab::Play, "play"), (ui::Tab::Versions, "versions"), (ui::Tab::Mods, "mods"), (ui::Tab::Export, "export"), (ui::Tab::Settings, "settings")]
        {
            l.show_tab(tab);
            shot(&mut l, name);
        }
        l.show_tab(ui::Tab::Mods);
        l.state.message =
            Some(("Updated Dusk garden 1.1.0 (was 1.0.0) and turned it on. The old copy is in C:\\Games\\frogger2-rs\\mods\\.backup\\dusk.".into(), false));
        shot(&mut l, "mods-installed");
        l.state.message = None;
        l.show_tab(ui::Tab::Settings);
        l.scroll_by(10_000.0);
        shot(&mut l, "settings-end");
        l.show_tab(ui::Tab::Play);
        l.state.message = Some(("Imported Frogger 2: Swampy's Revenge (USA). Press PLAY.".into(), false));
        shot(&mut l, "play-message");
        l.state.message = None;
        let (game, hero, logo) = (l.state.game.take(), l.state.hero.take(), l.state.logo.take());
        shot(&mut l, "import");
        l.state.import.running = Some(Progress {
            title: "Importing the disc in D:".into(),
            stage: "Ripping CD audio: track 7 of 26".into(),
            fraction: 0.42,
            log: (0..12).map(|i| format!("track {i:02}: 3:12 read, 33.8 MB")).collect(),
        });
        shot(&mut l, "import-running");
        l.state.import.running = None;
        l.state.import.error = Some("FROGGER2/DATA1.CAB was not found on the disc in D: (is it the Frogger 2 disc?)".into());
        shot(&mut l, "import-failed");
        l.state.import.error = None;
        (l.state.game, l.state.hero, l.state.logo) = (game, hero, logo);
        l.show_import(true);
        shot(&mut l, "import-again");
        l.show_import(false);
        l.state.busy = Some(("Downloading frogger2-rs-0.9.0-windows-x86_64.zip (14 MB)".into(), 0.4));
        l.show_tab(ui::Tab::Versions);
        shot(&mut l, "versions-busy");
        // Android: a phone in landscape (no drives, no folders, no versions).
        let caps = Caps { import: true, updates_page: true, close_label: Some("Back to the game"), ..Caps::default() };
        let mut a = Launcher::new(caps, 1.0, 1.0, 1.0);
        a.state = l.state.clone();
        a.state.busy = None;
        a.state.shortcut = None;
        a.state.import = ui::ImportView {
            help: vec![
                "For a .cue with its .bin, pick both files.".into(),
                "Or copy the image or the disc's files into /storage/emulated/0/Android/data/io.github.neilgraham.frogger2rs/files (for example with adb push): the import starts by itself.".into(),
            ],
            ..Default::default()
        };
        let (pw, ph, pdp) = (2400u32, 1080u32, 2.75f32);
        let mut phone = |l: &mut Launcher, name: &str| {
            r.begin_frame(pw, ph);
            l.resize(pw as f32, ph as f32, pdp);
            l.draw(&mut kit.painter(&mut r));
            r.render_to_image(clear).unwrap().write_png(dir.join(format!("launcher-android-{name}.png"))).unwrap();
        };
        phone(&mut a, "play");
        a.show_tab(ui::Tab::Settings);
        phone(&mut a, "settings");
        a.show_tab(ui::Tab::Play);
        a.state.game = None;
        phone(&mut a, "import");
    }

    /// "Import mod" without a window: a package is installed into the mods folder, turned on
    /// (saved) and listed; importing it again updates it; Remove takes it out.
    #[test]
    fn imports_and_removes_mods() {
        let t = std::env::temp_dir().join(format!("f2-launcher-mods-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let mut cfg = Config { data_root: Some(t.join("data")), ..Config::desktop() };
        cfg.mods.dir = Some(t.join("mods"));
        let mut app = App::with(cfg, t.clone());
        app.config_file = t.join("config.toml");
        app.refresh();
        assert!(app.ui.state.mods.is_empty());
        let src = f2_modkit::pack::scaffold(&t.join("src"), "hi").unwrap();
        let pkg = t.join("hi.f2mod");
        archive::write(&src, &pkg).unwrap();
        assert_eq!(mod_arg(["--mute".into(), pkg.clone().into_os_string()]), Some(pkg.clone()));
        assert_eq!(mod_arg(["--import-mod".into(), "x.f2mod".into()]), Some(PathBuf::from("x.f2mod")));
        assert_eq!(mod_arg(["--level".into(), "1".into(), "2".into()]), None);
        let wait = |app: &mut App| {
            assert!(app.installing_mod && app.ui.state.busy.is_some());
            while app.installing_mod {
                app.poll();
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(app.ui.state.busy.is_none());
        };
        app.import_mod(pkg.clone());
        assert_eq!(app.ui.tab, ui::Tab::Mods);
        wait(&mut app);
        assert_eq!(app.ui.state.mods, [ModRow { id: "hi".into(), name: "hi".into(), version: "0.1.0".into(), enabled: true }]);
        assert_eq!(app.ui.state.message, Some(("Installed hi 0.1.0 and turned it on.".into(), false)));
        assert!(Config::load(&app.config_file).unwrap().mods.enabled == ["hi"], "saved");
        // A folder's mod.toml (what the file dialog picks) stands for the folder: an update.
        std::fs::write(src.join("mod.toml"), f2_modkit::pack::manifest_template("hi").replace("0.1.0", "0.2.0")).unwrap();
        app.import_mod(src.join("mod.toml"));
        wait(&mut app);
        assert_eq!(app.ui.state.mods.iter().map(|m| (m.version.as_str(), m.enabled)).collect::<Vec<_>>(), [("0.2.0", true)]);
        assert!(app.ui.state.message.as_ref().is_some_and(|(m, bad)| !bad && m.starts_with("Updated hi 0.2.0 (was 0.1.0)")), "{:?}", app.ui.state.message);
        // A broken package: an error, nothing changes.
        std::fs::write(t.join("bad.f2mod"), b"nope").unwrap();
        app.import_mod(t.join("bad.f2mod"));
        wait(&mut app);
        assert!(app.ui.state.message.as_ref().is_some_and(|(m, bad)| *bad && m.contains("not a mod package")), "{:?}", app.ui.state.message);
        assert_eq!(app.ui.state.mods.len(), 1);
        app.act(Action::RemoveMod("hi".into()));
        assert!(app.ui.state.mods.is_empty() && app.cfg.mods.enabled.is_empty());
        assert!(t.join("mods/.backup/hi/mod.toml").is_file());
        let _ = std::fs::remove_dir_all(&t);
    }

    /// Imports a disc image through the launcher (`F2_TEST_IMAGE=<your Frogger 2 image>`,
    /// skipped without it) into a scratch install folder: the page shows the progress, then
    /// the game, and `frogger2` is made beside the launcher.
    #[test]
    fn imports_through_the_launcher() {
        let Some(image) = std::env::var_os("F2_TEST_IMAGE").map(PathBuf::from) else { return };
        let root = std::env::temp_dir().join(format!("f2-launcher-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("engine")).unwrap();
        std::fs::write(root.join("engine").join(crate::setup::game_exe_name()), b"engine").unwrap();
        let cfg = Config { data_root: Some(root.join("data")), ..Config::desktop() };
        let mut app = App::with(cfg, root.clone());
        app.refresh();
        assert!(app.ui.state.game.is_none() && app.ui.import_showing());
        app.start_import(ImportFrom::Path(image));
        assert!(app.ui.state.import.running.is_some());
        let mut saw_progress = false;
        while app.importing {
            app.poll();
            saw_progress |= app.ui.state.import.running.as_ref().is_some_and(|r| r.fraction > 0.0);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(app.ui.state.import.error, None);
        assert!(saw_progress);
        assert!(app.ui.state.game.as_deref().is_some_and(|t| t.starts_with("Frogger 2")), "{:?}", app.ui.state.game);
        assert!(!app.ui.import_showing(), "back on the Play page");
        assert!(app.hero_pending.is_some() && app.logo_pending.is_some() && app.icon_pending.is_some(), "the game's pictures");
        assert!(root.join(crate::setup::game_exe_name()).exists(), "frogger2 made");
        assert!(app.ui.state.message.as_ref().is_some_and(|(m, bad)| !bad && m.contains("Press PLAY")), "{:?}", app.ui.state.message);
        let _ = std::fs::remove_dir_all(&root);
    }
}
