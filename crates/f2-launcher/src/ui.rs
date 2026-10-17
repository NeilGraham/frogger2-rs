//! The launcher screen as a model: tabs (Play, Versions, Mods, Settings), buttons and lists
//! laid out in dp, drawn through a [`Painter`] with the game's built-in 6x8 font, and hit
//! tested for pointer/touch input. Hosts (desktop window, Android activity) feed [`State`],
//! call [`Launcher::draw`] every frame, and act on the [`Action`] a press returns.

/// Built-in font cell, in pixels at scale 1.
pub const GLYPH_W: f32 = 6.0;
pub const GLYPH_H: f32 = 8.0;

pub const BG: [u8; 4] = [5, 23, 10, 255];
const BAND: [u8; 4] = [18, 70, 30, 255];
const TITLE: [u8; 4] = [255, 220, 40, 255];
const TEXT: [u8; 4] = [225, 240, 225, 255];
const DIM: [u8; 4] = [130, 160, 135, 255];
const BTN: [u8; 4] = [40, 90, 48, 255];
const BTN_ON: [u8; 4] = [70, 170, 80, 255];
const BTN_OFF: [u8; 4] = [34, 56, 38, 255];
const GOOD: [u8; 4] = [120, 255, 120, 255];
const BAD: [u8; 4] = [255, 110, 100, 255];

/// Draws rectangles and text; the host maps this onto its renderer.
pub trait Painter {
    /// Filled rectangle `[x, y, w, h]` in pixels.
    fn rect(&mut self, r: [f32; 4], color: [u8; 4]);
    /// Text with its top-left at (x, y); each glyph is `GLYPH_W * scale` wide.
    fn text(&mut self, x: f32, y: f32, s: &str, color: [u8; 4], scale: f32);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Play,
    Versions,
    Mods,
    Settings,
}

/// What a press asks the host to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Play,
    Import,
    /// Leave the launcher (Android: back to the game).
    Close,
    SetStartInLauncher(bool),
    RefreshReleases,
    Install(String),
    /// Run this version (`""` = the one in the install folder itself).
    Use(String),
    Remove(String),
    ToggleMod(String),
    OpenModsFolder,
    OpenUpdates,
}

/// What this host can do (tabs and buttons it cannot back are hidden).
#[derive(Clone, Copy, Debug, Default)]
pub struct Caps {
    /// Install and switch between releases downloaded from GitHub (desktop).
    pub versions: bool,
    /// Offer an "Import game" button.
    pub import: bool,
    /// Open folders in the system file manager.
    pub open_folders: bool,
    /// A button that opens the releases page in the browser (Android: no in-app updates).
    pub updates_page: bool,
    /// Label of a button leaving the launcher ("Back to the game"), if any.
    pub close_label: Option<&'static str>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VersionRow {
    /// Release tag; `""` is the build in the install folder itself.
    pub tag: String,
    pub label: String,
    pub installed: bool,
    pub selected: bool,
    /// A download exists for this platform.
    pub available: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModRow {
    pub id: String,
    pub name: String,
    pub version: String,
    pub enabled: bool,
}

/// What the host tells the launcher to show.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// Title of the player's game once imported (`None`: no game data yet).
    pub game: Option<String>,
    pub data_dir: String,
    pub config_path: String,
    pub app_version: String,
    /// A newer release exists (its tag).
    pub update_available: Option<String>,
    pub start_in_launcher: bool,
    pub versions: Vec<VersionRow>,
    /// One line under the version list ("Checking GitHub...", "offline: ...").
    pub versions_note: String,
    pub mods: Vec<ModRow>,
    pub mods_dir: String,
    /// A running job: what it is doing and how far (0..=1).
    pub busy: Option<(String, f32)>,
    /// The last result line and whether it is an error.
    pub message: Option<(String, bool)>,
}

#[derive(Clone, Debug)]
enum Hit {
    Tab(Tab),
    Scroll(i32),
    Act(Action),
}

pub struct Launcher {
    pub state: State,
    pub caps: Caps,
    pub tab: Tab,
    w: f32,
    h: f32,
    dp: f32,
    /// First visible row of the current list.
    scroll: usize,
    hits: Vec<([f32; 4], Hit)>,
}

fn inside([x, y, w, h]: [f32; 4], px: f32, py: f32) -> bool {
    px >= x && px <= x + w && py >= y && py <= y + h
}

/// `s` cut to `max` characters (".." marks the cut).
fn fit(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(2)).collect();
        t.push_str("..");
        t
    }
}

impl Launcher {
    pub fn new(caps: Caps, w: f32, h: f32, dp: f32) -> Launcher {
        Launcher { state: State::default(), caps, tab: Tab::Play, w, h, dp: dp.max(0.5), scroll: 0, hits: Vec::new() }
    }

    pub fn resize(&mut self, w: f32, h: f32, dp: f32) {
        (self.w, self.h, self.dp) = (w, h, dp.max(0.5));
    }

    pub fn tabs(&self) -> Vec<(Tab, &'static str)> {
        let mut t = vec![(Tab::Play, "PLAY")];
        if self.caps.versions {
            t.push((Tab::Versions, "VERSIONS"));
        }
        t.push((Tab::Mods, "MODS"));
        t.push((Tab::Settings, "SETTINGS"));
        t
    }

    /// Scrolls the current list by `rows` (positive = down).
    pub fn scroll_by(&mut self, rows: i32) {
        self.scroll = (self.scroll as i64 + rows as i64).clamp(0, i64::from(i32::MAX)) as usize;
    }

    /// A press at (x, y): tab switches and scrolling are handled here, everything else is
    /// returned for the host.
    pub fn press(&mut self, x: f32, y: f32) -> Action {
        let Some((_, hit)) = self.hits.iter().rev().find(|(r, _)| inside(*r, x, y)).cloned() else { return Action::None };
        match hit {
            Hit::Tab(t) => {
                self.tab = t;
                self.scroll = 0;
                Action::None
            }
            Hit::Scroll(d) => {
                self.scroll_by(d);
                Action::None
            }
            Hit::Act(a) => a,
        }
    }

    fn scale(&self) -> f32 {
        (13.0 * self.dp / GLYPH_H).round().max(1.0)
    }

    fn button(&mut self, p: &mut dyn Painter, r: [f32; 4], label: &str, color: [u8; 4], act: Action) {
        let s = self.scale();
        p.rect(r, color);
        let tw = label.chars().count() as f32 * GLYPH_W * s;
        p.text(r[0] + (r[2] - tw) / 2.0, r[1] + (r[3] - GLYPH_H * s) / 2.0, label, TEXT, s);
        self.hits.push((r, Hit::Act(act)));
    }

    /// A button as wide as its label, placed at (x, y); returns its right edge.
    fn auto_button(&mut self, p: &mut dyn Painter, x: f32, y: f32, label: &str, color: [u8; 4], act: Action) -> f32 {
        let (s, d) = (self.scale(), self.dp);
        let w = label.chars().count() as f32 * GLYPH_W * s + 28.0 * d;
        self.button(p, [x, y, w, 40.0 * d], label, color, act);
        x + w
    }

    /// Draws the whole screen and rebuilds the hit regions.
    pub fn draw(&mut self, p: &mut dyn Painter) {
        self.hits.clear();
        let (w, h, d, s) = (self.w, self.h, self.dp, self.scale());
        let m = 16.0 * d;
        let cols = ((w - 2.0 * m) / (GLYPH_W * s)).floor().max(8.0) as usize;
        p.rect([0.0, 0.0, w, h], BG);

        // Header band.
        let head_h = 52.0 * d;
        p.rect([0.0, 0.0, w, head_h], BAND);
        let ts = (s * 1.3).max(1.0);
        let title = fit("FROGGER 2: SWAMPY'S REVENGE (RUST)", ((w - 2.0 * m) / (GLYPH_W * ts)).floor().max(8.0) as usize);
        p.text(m, (head_h - GLYPH_H * ts) / 2.0, &title, TITLE, ts);

        // Tabs.
        let tabs = self.tabs();
        if !tabs.iter().any(|(t, _)| *t == self.tab) {
            self.tab = Tab::Play;
        }
        let tab_h = 44.0 * d;
        let tw = (160.0 * d).min((w - 2.0 * m) / tabs.len() as f32);
        for (i, (t, label)) in tabs.iter().enumerate() {
            let r = [m + i as f32 * tw, head_h + 8.0 * d, tw - 4.0 * d, tab_h];
            p.rect(r, if *t == self.tab { BTN_ON } else { BTN_OFF });
            let lw = label.len() as f32 * GLYPH_W * s;
            p.text(r[0] + (r[2] - lw) / 2.0, r[1] + (r[3] - GLYPH_H * s) / 2.0, label, TEXT, s);
            self.hits.push((r, Hit::Tab(*t)));
        }

        // Footer: the running job or the last message.
        let foot_h = 40.0 * d;
        let foot_y = h - foot_h;
        p.rect([0.0, foot_y, w, foot_h], BAND);
        let text_y = foot_y + (foot_h - GLYPH_H * s) / 2.0;
        if let Some((what, f)) = self.state.busy.clone() {
            let bw = (w - 2.0 * m) * f.clamp(0.0, 1.0);
            p.rect([m, foot_y + foot_h - 6.0 * d, w - 2.0 * m, 4.0 * d], BTN_OFF);
            p.rect([m, foot_y + foot_h - 6.0 * d, bw, 4.0 * d], GOOD);
            p.text(m, foot_y + 6.0 * d, &fit(&what, cols), TEXT, s);
        } else if let Some((msg, bad)) = self.state.message.clone() {
            p.text(m, text_y, &fit(&msg, cols), if bad { BAD } else { GOOD }, s);
        }

        // Content area.
        let top = head_h + 8.0 * d + tab_h + 14.0 * d;
        let area = [m, top, w - 2.0 * m, (foot_y - top - 8.0 * d).max(0.0)];
        match self.tab {
            Tab::Play => self.draw_play(p, area, cols),
            Tab::Versions => self.draw_versions(p, area, cols),
            Tab::Mods => self.draw_mods(p, area, cols),
            Tab::Settings => self.draw_settings(p, area, cols),
        }
    }

    fn draw_play(&mut self, p: &mut dyn Painter, a: [f32; 4], cols: usize) {
        let (s, d) = (self.scale(), self.dp);
        let line = GLYPH_H * s + 8.0 * d;
        let mut y = a[1];
        match self.state.game.clone() {
            Some(g) => p.text(a[0], y, &fit(&g, cols), GOOD, s * 1.2),
            None => p.text(a[0], y, "No game data yet", BAD, s * 1.2),
        }
        y += line * 1.5;
        let version = match &self.state.update_available {
            Some(v) => format!("Version {}   (newer: {v})", self.state.app_version),
            None => format!("Version {}", self.state.app_version),
        };
        p.text(a[0], y, &fit(&version, cols), TEXT, s);
        y += line;
        p.text(a[0], y, &fit(&format!("Game data: {}", self.state.data_dir), cols), DIM, s);
        y += line * 1.6;
        if self.state.game.is_none() {
            for l in wrap("Import your own copy of Frogger 2 (the disc, a disc image or an install folder). Nothing from the game is downloaded or shipped with this program.", cols) {
                p.text(a[0], y, &l, DIM, s);
                y += line;
            }
            y += line * 0.6;
        }
        let mut x = a[0];
        if self.state.game.is_some() {
            x = self.auto_button(p, x, y, "PLAY", BTN_ON, Action::Play) + 12.0 * d;
        }
        if self.caps.import {
            let label = if self.state.game.is_some() { "RE-IMPORT GAME" } else { "IMPORT GAME" };
            x = self.auto_button(p, x, y, label, if self.state.game.is_some() { BTN } else { BTN_ON }, Action::Import) + 12.0 * d;
        }
        if let Some(label) = self.caps.close_label {
            self.auto_button(p, x, y, label, BTN, Action::Close);
        }
    }

    /// Draws up to the visible rows of a list starting at `y0`; returns the row height and
    /// how many rows fit. Clamps the scroll position.
    fn list_geometry(&mut self, a: [f32; 4], y0: f32, len: usize) -> (f32, usize) {
        let d = self.dp;
        let row_h = 46.0 * d;
        let fit_rows = (((a[1] + a[3] - y0) / row_h).floor() as usize).max(1);
        self.scroll = self.scroll.min(len.saturating_sub(fit_rows));
        (row_h, fit_rows)
    }

    fn scroll_buttons(&mut self, p: &mut dyn Painter, a: [f32; 4], y0: f32, len: usize, fit_rows: usize) {
        if len <= fit_rows {
            return;
        }
        let d = self.dp;
        let bw = 44.0 * d;
        let x = a[0] + a[2] - bw;
        self.button(p, [x, y0, bw, 40.0 * d], "UP", BTN, Action::None);
        self.hits.pop();
        self.hits.push(([x, y0, bw, 40.0 * d], Hit::Scroll(-(fit_rows as i32) + 1)));
        let y1 = a[1] + a[3] - 40.0 * d;
        self.button(p, [x, y1, bw, 40.0 * d], "DN", BTN, Action::None);
        self.hits.pop();
        self.hits.push(([x, y1, bw, 40.0 * d], Hit::Scroll(fit_rows as i32 - 1)));
    }

    fn draw_versions(&mut self, p: &mut dyn Painter, a: [f32; 4], cols: usize) {
        let (s, d) = (self.scale(), self.dp);
        let line = GLYPH_H * s + 8.0 * d;
        p.text(a[0], a[1] + 4.0 * d, "Releases from GitHub; versions live side by side in the versions folder.", DIM, s * 0.8);
        let mut y = a[1] + line;
        let x = self.auto_button(p, a[0], y, "REFRESH", BTN, Action::RefreshReleases);
        let note = fit(&self.state.versions_note, ((a[0] + a[2] - x - 24.0 * d) / (GLYPH_W * s)).floor().max(4.0) as usize);
        p.text(x + 12.0 * d, y + (40.0 * d - GLYPH_H * s) / 2.0, &note, DIM, s);
        y += 40.0 * d + 10.0 * d;
        let rows = self.state.versions.clone();
        let (row_h, n) = self.list_geometry(a, y, rows.len());
        let list_w = a[2] - if rows.len() > n { 52.0 * d } else { 0.0 };
        for (i, v) in rows.iter().enumerate().skip(self.scroll).take(n) {
            let ry = y + (i - self.scroll) as f32 * row_h;
            p.rect([a[0], ry, list_w, row_h - 4.0 * d], if v.selected { BTN } else { BTN_OFF });
            let mut bx = a[0] + list_w - 8.0 * d;
            // Buttons right to left: Remove, then Use / Install / "in use".
            let mut put = |this: &mut Launcher, p: &mut dyn Painter, label: &str, color: [u8; 4], act: Action| {
                let bw = label.len() as f32 * GLYPH_W * s + 20.0 * d;
                bx -= bw;
                this.button(p, [bx, ry + 3.0 * d, bw, row_h - 10.0 * d], label, color, act);
                bx -= 8.0 * d;
            };
            if v.installed && !v.tag.is_empty() && !v.selected {
                put(self, p, "REMOVE", BTN_OFF, Action::Remove(v.tag.clone()));
            }
            if v.selected {
                let label = "IN USE";
                bx -= label.len() as f32 * GLYPH_W * s + 12.0 * d;
                p.text(bx, ry + (row_h - 4.0 * d - GLYPH_H * s) / 2.0, label, GOOD, s);
            } else if v.installed {
                put(self, p, "USE", BTN_ON, Action::Use(v.tag.clone()));
            } else if v.available {
                put(self, p, "INSTALL", BTN_ON, Action::Install(v.tag.clone()));
            } else {
                let label = "NO BUILD";
                bx -= label.len() as f32 * GLYPH_W * s + 12.0 * d;
                p.text(bx, ry + (row_h - 4.0 * d - GLYPH_H * s) / 2.0, label, DIM, s);
            }
            let avail = ((bx - a[0] - 20.0 * d) / (GLYPH_W * s)).floor().max(4.0) as usize;
            p.text(a[0] + 10.0 * d, ry + (row_h - 4.0 * d - GLYPH_H * s) / 2.0, &fit(&v.label, avail), TEXT, s);
        }
        if rows.is_empty() {
            p.text(a[0], y + 8.0 * d, &fit("Nothing to show yet.", cols), DIM, s);
        }
        self.scroll_buttons(p, a, y, rows.len(), n);
    }

    fn draw_mods(&mut self, p: &mut dyn Painter, a: [f32; 4], cols: usize) {
        let (s, d) = (self.scale(), self.dp);
        let line = GLYPH_H * s + 8.0 * d;
        p.text(a[0], a[1] + 4.0 * d, &fit(&format!("Mods folder: {}", self.state.mods_dir), cols), DIM, s * 0.8);
        let mut y = a[1] + line;
        if self.caps.open_folders {
            self.auto_button(p, a[0], y, "OPEN MODS FOLDER", BTN, Action::OpenModsFolder);
            y += 40.0 * d + 10.0 * d;
        }
        let rows = self.state.mods.clone();
        if rows.is_empty() {
            for l in wrap("No mods installed. Put a mod's folder (with its mod.toml) into the mods folder; downloading mods from here is planned.", cols) {
                p.text(a[0], y + 4.0 * d, &l, DIM, s);
                y += line;
            }
            return;
        }
        let (row_h, n) = self.list_geometry(a, y, rows.len());
        let list_w = a[2] - if rows.len() > n { 52.0 * d } else { 0.0 };
        for (i, m) in rows.iter().enumerate().skip(self.scroll).take(n) {
            let ry = y + (i - self.scroll) as f32 * row_h;
            p.rect([a[0], ry, list_w, row_h - 4.0 * d], BTN_OFF);
            let label = if m.enabled { "ON" } else { "OFF" };
            let bw = 70.0 * d;
            self.button(
                p,
                [a[0] + list_w - bw - 8.0 * d, ry + 3.0 * d, bw, row_h - 10.0 * d],
                label,
                if m.enabled { BTN_ON } else { BTN },
                Action::ToggleMod(m.id.clone()),
            );
            let text = if m.version.is_empty() { m.name.clone() } else { format!("{} {}", m.name, m.version) };
            let avail = ((list_w - bw - 30.0 * d) / (GLYPH_W * s)).floor().max(4.0) as usize;
            p.text(a[0] + 10.0 * d, ry + (row_h - 4.0 * d - GLYPH_H * s) / 2.0, &fit(&text, avail), TEXT, s);
        }
        self.scroll_buttons(p, a, y, rows.len(), n);
    }

    fn draw_settings(&mut self, p: &mut dyn Painter, a: [f32; 4], cols: usize) {
        let (s, d) = (self.scale(), self.dp);
        let line = GLYPH_H * s + 8.0 * d;
        let mut y = a[1] + 4.0 * d;
        p.text(a[0], y, "On start, open the:", TEXT, s);
        y += line;
        let start = self.state.start_in_launcher;
        let x = self.auto_button(p, a[0], y, "GAME", if start { BTN_OFF } else { BTN_ON }, Action::SetStartInLauncher(false)) + 12.0 * d;
        self.auto_button(p, x, y, "LAUNCHER", if start { BTN_ON } else { BTN_OFF }, Action::SetStartInLauncher(true));
        y += 40.0 * d + 14.0 * d;
        if self.caps.updates_page {
            self.auto_button(p, a[0], y, "CHECK FOR UPDATES ONLINE", BTN, Action::OpenUpdates);
            y += 40.0 * d + 14.0 * d;
        }
        for t in [format!("Settings file: {}", self.state.config_path), format!("Game data: {}", self.state.data_dir)] {
            p.text(a[0], y, &fit(&t, cols), DIM, s * 0.8);
            y += line;
        }
    }
}

/// Greedy word wrap to `width` characters.
pub fn wrap(t: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in t.split_whitespace() {
        if !cur.is_empty() && cur.len() + 1 + word.len() > width {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Rec {
        rects: Vec<[f32; 4]>,
        texts: Vec<String>,
    }

    impl Painter for Rec {
        fn rect(&mut self, r: [f32; 4], _: [u8; 4]) {
            self.rects.push(r);
        }
        fn text(&mut self, _x: f32, _y: f32, s: &str, _: [u8; 4], _: f32) {
            self.texts.push(s.to_string());
        }
    }

    fn desktop() -> Launcher {
        let mut l = Launcher::new(Caps { versions: true, import: true, open_folders: true, updates_page: false, close_label: None }, 1280.0, 720.0, 1.0);
        l.state.game = Some("Frogger 2: Swampy's Revenge (USA)".into());
        l.state.app_version = "0.1.0".into();
        l.state.data_dir = "/home/x/frogger2-rs/data".into();
        l.state.versions = (0..12)
            .map(|i| VersionRow {
                tag: format!("v0.{i}.0"),
                label: format!("v0.{i}.0  2026-10-{:02}", i + 1),
                installed: i % 3 == 0,
                selected: i == 0,
                available: i != 5,
            })
            .collect();
        l.state.mods = vec![ModRow { id: "hello-mod".into(), name: "Hello, mod!".into(), version: "0.1.0".into(), enabled: true }];
        l
    }

    fn click_label(l: &mut Launcher, label: &str) -> Action {
        // Find the button whose label was drawn: the hit region centred where the text is.
        struct Find<'a> {
            label: &'a str,
            at: Option<(f32, f32)>,
        }
        impl Painter for Find<'_> {
            fn rect(&mut self, _: [f32; 4], _: [u8; 4]) {}
            fn text(&mut self, x: f32, y: f32, s: &str, _: [u8; 4], scale: f32) {
                if s == self.label {
                    self.at = Some((x + s.len() as f32 * GLYPH_W * scale / 2.0, y + GLYPH_H * scale / 2.0));
                }
            }
        }
        let mut f = Find { label, at: None };
        l.draw(&mut f);
        let (x, y) = f.at.unwrap_or_else(|| panic!("no {label} drawn"));
        l.press(x, y)
    }

    #[test]
    fn everything_fits_on_screen_sizes() {
        for (w, h, dp) in [(1280.0, 720.0, 1.0), (1920.0, 1080.0, 2.0), (800.0, 480.0, 1.5), (2400.0, 1080.0, 2.75), (640.0, 480.0, 1.0)] {
            let mut l = desktop();
            l.resize(w, h, dp);
            for (t, _) in l.tabs() {
                l.tab = t;
                let mut r = Rec::default();
                l.draw(&mut r);
                for [x, y, rw, rh] in &r.rects {
                    assert!(*x >= -0.5 && *y >= -0.5 && x + rw <= w + 0.5 && y + rh <= h + 0.5, "{t:?} {w}x{h}@{dp}: rect {x},{y} {rw}x{rh}");
                }
                assert!(!r.texts.is_empty());
            }
        }
    }

    #[test]
    fn tabs_switch_and_buttons_return_actions() {
        let mut l = desktop();
        assert_eq!(l.tab, Tab::Play);
        assert_eq!(click_label(&mut l, "MODS"), Action::None);
        assert_eq!(l.tab, Tab::Mods);
        l.tab = Tab::Play;
        assert_eq!(click_label(&mut l, "PLAY"), Action::Play, "the PLAY button, drawn after the PLAY tab");
    }

    #[test]
    fn play_import_and_settings() {
        let mut l = desktop();
        assert_eq!(click_label(&mut l, "RE-IMPORT GAME"), Action::Import);
        l.state.game = None;
        assert_eq!(click_label(&mut l, "IMPORT GAME"), Action::Import);
        l.state.game = Some("g".into());
        assert_eq!(click_label(&mut l, "SETTINGS"), Action::None);
        assert_eq!(l.tab, Tab::Settings);
        assert_eq!(click_label(&mut l, "LAUNCHER"), Action::SetStartInLauncher(true));
        assert_eq!(click_label(&mut l, "GAME"), Action::SetStartInLauncher(false));
    }

    #[test]
    fn versions_and_mods_actions() {
        let mut l = desktop();
        l.state.versions = vec![
            VersionRow { tag: "v0.3.0".into(), label: "v0.3.0".into(), installed: true, selected: false, available: true },
            VersionRow { tag: "v0.2.0".into(), label: "v0.2.0".into(), installed: false, selected: false, available: true },
            VersionRow { tag: "v0.1.0".into(), label: "v0.1.0".into(), installed: true, selected: true, available: true },
        ];
        click_label(&mut l, "VERSIONS");
        assert_eq!(l.tab, Tab::Versions);
        assert_eq!(click_label(&mut l, "REFRESH"), Action::RefreshReleases);
        assert_eq!(click_label(&mut l, "INSTALL"), Action::Install("v0.2.0".into()));
        assert_eq!(click_label(&mut l, "USE"), Action::Use("v0.3.0".into()));
        assert_eq!(click_label(&mut l, "REMOVE"), Action::Remove("v0.3.0".into()));
        click_label(&mut l, "MODS");
        assert_eq!(click_label(&mut l, "ON"), Action::ToggleMod("hello-mod".into()));
        assert_eq!(click_label(&mut l, "OPEN MODS FOLDER"), Action::OpenModsFolder);
    }

    #[test]
    fn android_style_caps_hide_versions_and_offer_close() {
        let mut l = Launcher::new(
            Caps { versions: false, import: true, open_folders: false, updates_page: true, close_label: Some("BACK TO THE GAME") },
            1920.0,
            1080.0,
            2.0,
        );
        assert!(!l.tabs().iter().any(|(t, _)| *t == Tab::Versions));
        assert_eq!(click_label(&mut l, "BACK TO THE GAME"), Action::Close);
        click_label(&mut l, "SETTINGS");
        assert_eq!(click_label(&mut l, "CHECK FOR UPDATES ONLINE"), Action::OpenUpdates);
    }

    #[test]
    fn long_lists_scroll() {
        let mut l = desktop();
        l.resize(1280.0, 480.0, 1.0);
        click_label(&mut l, "VERSIONS");
        let mut r = Rec::default();
        l.draw(&mut r);
        assert!(r.texts.iter().any(|t| t == "DN"), "scroll buttons on a long list");
        assert!(!r.texts.iter().any(|t| t.starts_with("v0.11.0")));
        l.scroll_by(100);
        let mut r = Rec::default();
        l.draw(&mut r);
        assert!(r.texts.iter().any(|t| t.starts_with("v0.11.0")), "scrolled to the end");
        assert!(!r.texts.iter().any(|t| t.starts_with("v0.0.0")));
    }

    #[test]
    fn wraps_and_fits() {
        assert_eq!(wrap("aaaa bbbb cccc", 9), ["aaaa bbbb", "cccc"]);
        assert_eq!(fit("abcdef", 4), "ab..");
        assert_eq!(fit("abc", 4), "abc");
    }
}
