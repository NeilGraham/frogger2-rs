//! `launcher`: picks the version to run, installs updates from GitHub releases, manages mods
//! and starts the game (docs/USER-GUIDE.md "Launcher"). The screen itself is
//! `f2-launcher`'s UI model, drawn here with the game's built-in font; the Android app draws
//! the same model.
//!
//! Layout of an install folder (a release zip, `portable.txt` beside the exes):
//! `launcher`, `engine/` (the real binaries), `frogger2(.exe)` (made here once the game is
//! imported, with the game's own icon), `versions/<tag>/` (other installed releases), `data/`,
//! `mods/`, `saves/`, `config.toml`.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use f2_engine::{Blend, FontId, GpuContext, Quad2D, Renderer, WindowSurface};
use f2_launcher::releases::{self, Release};
use f2_launcher::ui::{self, Action, Caps, Launcher, ModRow, Painter, VersionRow};
use f2_launcher::{install, mods};
use f2_platform::window::{self, winit, Flow, WindowApp, WindowConfig};
use f2_platform::Config;
use frogger::data::Data;
use frogger::tinyfont::builtin_font;

enum Msg {
    Releases(Result<Vec<Release>, String>),
    Progress(String, f32),
    Installed(Result<String, String>),
}

struct Gfx {
    ctx: GpuContext,
    surface: WindowSurface,
    r: Renderer,
    font: FontId,
}

struct App {
    root: PathBuf,
    cfg: Config,
    ui: Launcher,
    gfx: Option<Gfx>,
    error: Option<String>,
    releases: Vec<Release>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    checking: bool,
    installing: bool,
    /// The importer window (`frogger2 --import-screen`) while it runs.
    importing: Option<std::process::Child>,
    cursor: (f32, f32),
    exit: bool,
}

struct RPainter<'a> {
    r: &'a mut Renderer,
    font: FontId,
}

impl Painter for RPainter<'_> {
    fn rect(&mut self, [x, y, w, h]: [f32; 4], c: [u8; 4]) {
        self.r.draw_quad(Quad2D::rect(x, y, w, h, [0.0, 0.0, 1.0, 1.0], None, c, Blend::Semi));
    }
    fn text(&mut self, x: f32, y: f32, s: &str, c: [u8; 4], scale: f32) {
        self.r.draw_text(self.font, x, y, s, c, scale, None);
    }
}

/// The install folder: the portable folder, else the exe's folder (not its `engine/`).
fn install_root() -> PathBuf {
    if let Some(r) = f2_platform::paths::portable_root() {
        return r;
    }
    let exe = std::env::current_exe().unwrap_or_default();
    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    if dir.file_name().is_some_and(|n| n == "engine") {
        return dir.parent().map(Path::to_path_buf).unwrap_or(dir);
    }
    dir
}

impl App {
    fn new() -> App {
        let cfg = Config::load_default().unwrap_or_else(|e| {
            eprintln!("launcher: config: {e} (using defaults)");
            Config::default()
        });
        let caps = Caps { versions: true, import: true, open_folders: true, updates_page: false, close_label: None };
        let (tx, rx) = channel();
        let mut a = App {
            root: install_root(),
            ui: Launcher::new(caps, 960.0, 600.0, 1.0),
            cfg,
            gfx: None,
            error: None,
            releases: Vec::new(),
            tx,
            rx,
            checking: false,
            installing: false,
            importing: None,
            cursor: (0.0, 0.0),
            exit: false,
        };
        a.refresh();
        if a.cfg.launcher.check_updates && std::env::var_os("F2_NO_UPDATE_CHECK").is_none() {
            a.check_releases();
        }
        a
    }

    fn selected_dir(&self) -> PathBuf {
        let tag = &self.cfg.launcher.version;
        let d = install::version_dir(&self.root, tag);
        if !tag.is_empty() && install::engine_exe(&d).is_some() {
            d
        } else {
            self.root.clone()
        }
    }

    fn locate(&self) -> Option<Data> {
        Data::locate(&self.cfg, None).ok()
    }

    /// Rebuilds everything the UI shows from the config, the folders and the release list.
    fn refresh(&mut self) {
        let app_version = env!("CARGO_PKG_VERSION").to_string();
        let data = self.locate();
        let s = &mut self.ui.state;
        s.game = data.as_ref().map(|d| frogger::export::title(d).unwrap_or_else(|_| "Frogger 2: Swampy's Revenge".into()));
        s.data_dir = self.cfg.data_root().display().to_string();
        s.config_path = f2_platform::paths::config_file().display().to_string();
        s.start_in_launcher = self.cfg.launcher.start_in_launcher();
        s.mods_dir = self.cfg.mods_dir().display().to_string();
        s.mods = mods::list(&self.cfg.mods_dir())
            .into_iter()
            .map(|m| ModRow { enabled: self.cfg.mods.enabled.contains(&m.id), id: m.id, name: m.name, version: m.version })
            .collect();
        let selected = self.cfg.launcher.version.clone();
        let installed = install::installed_tags(&self.root);
        let (os, arch) = (releases::host_os(), releases::host_arch());
        let mut rows = vec![VersionRow {
            tag: String::new(),
            label: format!("This folder (v{app_version})"),
            installed: true,
            selected: selected.is_empty() || !installed.contains(&selected),
            available: true,
        }];
        for r in &self.releases {
            rows.push(VersionRow {
                tag: r.tag.clone(),
                label: format!("{}  {}{}", r.tag, r.date, if r.prerelease { "  (pre-release)" } else { "" }),
                installed: installed.contains(&r.tag),
                selected: selected == r.tag && installed.contains(&r.tag),
                available: r.asset_for(os, arch).is_some() && r.sums_asset().is_some(),
            });
        }
        for t in &installed {
            if !self.releases.iter().any(|r| &r.tag == t) {
                rows.push(VersionRow { tag: t.clone(), label: t.clone(), installed: true, selected: &selected == t, available: false });
            }
        }
        s.versions = rows;
        s.app_version = app_version.clone();
        s.update_available =
            self.releases.iter().find(|r| !r.prerelease && r.asset_for(os, arch).is_some() && releases::is_newer(&r.tag, &app_version)).map(|r| r.tag.clone());
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
        if let Err(e) = self.cfg.save_default() {
            self.say(&format!("saving settings: {e}"), true);
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
                            self.ui.state.versions_note = format!("{} releases", rs.len());
                            self.releases = rs;
                        }
                        Err(e) => self.ui.state.versions_note = format!("offline: {e}"),
                    }
                    self.refresh();
                }
                Msg::Progress(what, f) => self.ui.state.busy = Some((what, f)),
                Msg::Installed(r) => {
                    self.installing = false;
                    self.ui.state.busy = None;
                    match r {
                        Ok(tag) => self.say(&format!("Installed {tag}"), false),
                        Err(e) => self.say(&e, true),
                    }
                    self.refresh();
                }
            }
        }
        if let Some(c) = &mut self.importing {
            if let Ok(Some(_)) = c.try_wait() {
                self.importing = None;
                self.after_import();
            }
        }
    }

    /// The importer window closed: pick up the new game and make the game's executable.
    fn after_import(&mut self) {
        self.refresh();
        match self.locate() {
            Some(d) => match frogger::setup::ensure_game_exe(&self.selected_dir(), &d, &|l| eprintln!("launcher: {l}")) {
                Ok(_) => self.say("Game imported. Press PLAY.", false),
                Err(e) => self.say(&e, true),
            },
            None => self.say("No game was imported.", true),
        }
    }

    fn act(&mut self, a: Action) {
        match a {
            Action::None | Action::Close | Action::OpenUpdates => {}
            Action::Play => self.play(),
            Action::Import => self.import(),
            Action::SetStartInLauncher(b) => {
                self.cfg.launcher.start = if b { "launcher" } else { "game" }.into();
                self.save_config();
                self.refresh();
            }
            Action::RefreshReleases => self.check_releases(),
            Action::Use(tag) => {
                self.cfg.launcher.version = tag.clone();
                self.save_config();
                self.refresh();
                self.say(&format!("Using {}", if tag.is_empty() { "the version in this folder" } else { &tag }), false);
            }
            Action::Remove(tag) => match install::remove(&self.root, &tag) {
                Ok(()) => {
                    self.say(&format!("Removed {tag}"), false);
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
            Action::OpenModsFolder => {
                let dir = self.cfg.mods_dir();
                let _ = std::fs::create_dir_all(&dir);
                let opener = if cfg!(windows) {
                    "explorer"
                } else if cfg!(target_os = "macos") {
                    "open"
                } else {
                    "xdg-open"
                };
                if let Err(e) = std::process::Command::new(opener).arg(&dir).spawn() {
                    self.say(&format!("opening {}: {e}", dir.display()), true);
                }
            }
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

    fn import(&mut self) {
        if self.importing.is_some() {
            return;
        }
        let dir = self.selected_dir();
        let Some(exe) = install::engine_exe(&dir).or_else(|| install::engine_exe(&self.root)) else {
            return self.say("the game program was not found next to the launcher", true);
        };
        match std::process::Command::new(&exe).arg("--import-screen").current_dir(&dir).spawn() {
            Ok(c) => {
                self.importing = Some(c);
                self.say("Import window open...", false);
            }
            Err(e) => self.say(&format!("starting {}: {e}", exe.display()), true),
        }
    }

    fn play(&mut self) {
        let Some(data) = self.locate() else { return self.say("Import your game first.", true) };
        let dir = self.selected_dir();
        let exe = match frogger::setup::ensure_game_exe(&dir, &data, &|l| eprintln!("launcher: {l}")) {
            Ok(e) => e,
            Err(e) => return self.say(&e, true),
        };
        match std::process::Command::new(&exe).arg("--game").current_dir(&dir).spawn() {
            Ok(_) => self.exit = true,
            Err(e) => self.say(&format!("starting {}: {e}", exe.display()), true),
        }
    }
}

impl WindowApp for App {
    fn init(&mut self, window: &Arc<window::Window>) {
        if self.gfx.is_some() {
            return;
        }
        let icon = frogger::icon::square(&frogger::icon::app_icon(), 64);
        if let Ok(i) = winit::window::Icon::from_rgba(icon.pixels, icon.width, icon.height) {
            window.set_window_icon(Some(i));
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

    fn event(&mut self, window: &Arc<window::Window>, event: &winit::event::WindowEvent) -> Flow {
        use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
        match event {
            WindowEvent::CursorMoved { position, .. } => self.cursor = (position.x as f32, position.y as f32),
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                let a = self.ui.press(self.cursor.0, self.cursor.1);
                self.act(a);
                window.request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let rows = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -y.round() as i32,
                    MouseScrollDelta::PixelDelta(p) => -(p.y / 40.0).round() as i32,
                };
                self.ui.scroll_by(rows);
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
        g.r.begin_frame(w, h);
        self.ui.draw(&mut RPainter { r: &mut g.r, font: g.font });
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

fn main() {
    let app = App::new();
    let mut wc = WindowConfig::from_config(&app.cfg);
    wc.title = "Frogger 2: Swampy's Revenge (Rust) - Launcher".into();
    wc.mode = f2_platform::WindowMode::Windowed;
    wc.resolution = None;
    wc.position = None;
    wc.width = 960;
    wc.height = 600;
    if let Err(e) = window::run(wc, app) {
        eprintln!("launcher: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders each tab headless to `$F2_SHOT_DIR/launcher-*.png` (skips without a GPU).
    #[test]
    fn screenshots() {
        let Some(dir) = std::env::var_os("F2_SHOT_DIR").map(PathBuf::from) else { return };
        let Ok(ctx) = GpuContext::new_headless() else { return };
        let mut r = Renderer::new(&ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8Unorm);
        let font = r.add_font(builtin_font());
        let caps = Caps { versions: true, import: true, open_folders: true, updates_page: false, close_label: None };
        let mut l = Launcher::new(caps, 1280.0, 720.0, 1.5);
        l.state.game = Some("Frogger 2: Swampy's Revenge".into());
        l.state.app_version = "0.1.0".into();
        l.state.update_available = Some("v0.2.0".into());
        l.state.data_dir = "C:\\Games\\frogger2-rs\\data".into();
        l.state.config_path = "C:\\Games\\frogger2-rs\\config.toml".into();
        l.state.mods_dir = "C:\\Games\\frogger2-rs\\mods".into();
        l.state.versions_note = "3 releases".into();
        l.state.versions = vec![
            VersionRow { tag: String::new(), label: "This folder (v0.1.0)".into(), installed: true, selected: true, available: true },
            VersionRow { tag: "v0.2.0".into(), label: "v0.2.0  2026-11-01".into(), installed: false, selected: false, available: true },
            VersionRow { tag: "v0.1.5".into(), label: "v0.1.5  2026-10-28  (pre-release)".into(), installed: true, selected: false, available: true },
            VersionRow { tag: "v0.1.0".into(), label: "v0.1.0  2026-10-20".into(), installed: false, selected: false, available: false },
        ];
        l.state.mods = vec![
            ModRow { id: "hello-mod".into(), name: "Hello, mod!".into(), version: "0.1.0".into(), enabled: true },
            ModRow { id: "dusk".into(), name: "Dusk garden".into(), version: "".into(), enabled: false },
        ];
        l.state.message = Some(("Installed v0.1.5".into(), false));
        for (tab, name) in [(ui::Tab::Play, "play"), (ui::Tab::Versions, "versions"), (ui::Tab::Mods, "mods"), (ui::Tab::Settings, "settings")] {
            l.tab = tab;
            r.begin_frame(1280, 720);
            l.draw(&mut RPainter { r: &mut r, font });
            let bg = ui::BG;
            r.render_to_image([bg[0] as f64 / 255.0, bg[1] as f64 / 255.0, bg[2] as f64 / 255.0, 1.0])
                .unwrap()
                .write_png(dir.join(format!("launcher-{name}.png")))
                .unwrap();
        }
        l.state.game = None;
        l.tab = ui::Tab::Play;
        l.state.busy = Some(("Downloading frogger2-rs-0.2.0-windows-x86_64.zip (40 MB)".into(), 0.4));
        r.begin_frame(1280, 720);
        l.draw(&mut RPainter { r: &mut r, font });
        r.render_to_image([0.02, 0.09, 0.04, 1.0]).unwrap().write_png(dir.join("launcher-nogame.png")).unwrap();
    }
}
