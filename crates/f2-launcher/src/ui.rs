//! The launcher screen as a model: a sidebar (Play, Versions, Mods, Settings), cards, buttons,
//! toggles and lists laid out in dp, drawn through a [`Painter`], and hit tested for pointer
//! and touch input. Hosts (desktop window, Android activity) feed [`State`], call
//! [`Launcher::draw`] every frame, and act on the [`Action`] a press returns. The painter does
//! the pixels (anti-aliased text, rounded shapes, icons, the hero picture); nothing here knows
//! about a renderer.

#![allow(clippy::too_many_arguments)]

// Palette: charcoal surfaces and the frog green accent.
pub const BG: [u8; 4] = [20, 22, 26, 255];
const SIDEBAR: [u8; 4] = [14, 16, 19, 255];
const CARD: [u8; 4] = [30, 34, 40, 255];
const CARD2: [u8; 4] = [40, 45, 53, 255];
const TEXT: [u8; 4] = [236, 239, 242, 255];
const DIM: [u8; 4] = [150, 158, 168, 255];
const ACCENT: [u8; 4] = [84, 198, 83, 255];
const ACCENT_DARK: [u8; 4] = [46, 140, 60, 255];
const ON_ACCENT: [u8; 4] = [8, 28, 12, 255];
const DANGER: [u8; 4] = [240, 100, 90, 255];

/// Simple icons the painter draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Play,
    Layers,
    Grid,
    Gear,
    Check,
    Download,
    Refresh,
    Folder,
    Back,
}

/// Draws shapes and text; the host maps this onto its renderer. Sizes and positions are
/// pixels; `y` of text is the top of its line box.
pub trait Painter {
    fn rect(&mut self, r: [f32; 4], color: [u8; 4]);
    fn round_rect(&mut self, r: [f32; 4], radius: f32, color: [u8; 4]);
    /// Vertical gradient from `top` to `bottom` colour.
    fn gradient_v(&mut self, r: [f32; 4], top: [u8; 4], bottom: [u8; 4]);
    fn text(&mut self, x: f32, y: f32, s: &str, color: [u8; 4], size: f32, bold: bool);
    fn text_width(&mut self, s: &str, size: f32, bold: bool) -> f32;
    fn icon(&mut self, icon: Icon, r: [f32; 4], color: [u8; 4]);
    /// The picture registered under `key`; `uv` = the part of it to show ([u1,v1,u2,v2]).
    fn image(&mut self, key: &str, dst: [f32; 4], uv: [f32; 4]);
    /// Rounds the corners of what was drawn in `r` by painting the outside of the corner arcs
    /// with the flat colour `bg` that surrounds it.
    fn round_mask(&mut self, r: [f32; 4], radius: f32, bg: [u8; 4]);
}

/// The picture the hero card shows (the host registers it with its painter under this key).
pub const HERO_KEY: &str = "hero";

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
    /// Main text ("v0.2.0", "This folder").
    pub label: String,
    /// Dim text after it (date, "pre-release").
    pub detail: String,
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
    /// Size of the picture registered as [`HERO_KEY`] (the game's artwork), if any.
    pub hero: Option<(u32, u32)>,
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    Primary,
    Secondary,
    Ghost,
    Danger,
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
    hover: Option<(f32, f32)>,
    hits: Vec<([f32; 4], Hit)>,
}

fn inside([x, y, w, h]: [f32; 4], px: f32, py: f32) -> bool {
    px >= x && px <= x + w && py >= y && py <= y + h
}

fn alpha(c: [u8; 4], a: u8) -> [u8; 4] {
    [c[0], c[1], c[2], a]
}

fn lighten(c: [u8; 4], by: u8) -> [u8; 4] {
    [c[0].saturating_add(by), c[1].saturating_add(by), c[2].saturating_add(by), c[3]]
}

impl Launcher {
    pub fn new(caps: Caps, w: f32, h: f32, dp: f32) -> Launcher {
        Launcher { state: State::default(), caps, tab: Tab::Play, w, h, dp: dp.max(0.5), scroll: 0, hover: None, hits: Vec::new() }
    }

    pub fn resize(&mut self, w: f32, h: f32, dp: f32) {
        (self.w, self.h, self.dp) = (w, h, dp.max(0.5));
    }

    pub fn tabs(&self) -> Vec<(Tab, &'static str, Icon)> {
        let mut t = vec![(Tab::Play, "Play", Icon::Play)];
        if self.caps.versions {
            t.push((Tab::Versions, "Versions", Icon::Layers));
        }
        t.push((Tab::Mods, "Mods", Icon::Grid));
        t.push((Tab::Settings, "Settings", Icon::Gear));
        t
    }

    /// Scrolls the current list by `rows` (positive = down).
    pub fn scroll_by(&mut self, rows: i32) {
        self.scroll = (self.scroll as i64 + rows as i64).clamp(0, i64::from(i32::MAX)) as usize;
    }

    /// The pointer moved (desktop hover highlight).
    pub fn pointer_moved(&mut self, x: f32, y: f32) {
        self.hover = Some((x, y));
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

    fn d(&self, dp: f32) -> f32 {
        dp * self.dp
    }

    fn hovered(&self, r: [f32; 4]) -> bool {
        self.hover.is_some_and(|(x, y)| inside(r, x, y))
    }

    /// `s` cut to `max_w` pixels with an ellipsis.
    fn fit(&self, p: &mut dyn Painter, s: &str, max_w: f32, size: f32, bold: bool) -> String {
        if p.text_width(s, size, bold) <= max_w {
            return s.to_string();
        }
        let mut t: Vec<char> = s.chars().collect();
        while !t.is_empty() {
            t.pop();
            let cand: String = t.iter().collect::<String>() + "\u{2026}";
            if p.text_width(&cand, size, bold) <= max_w {
                return cand;
            }
        }
        String::new()
    }

    /// Text vertically centred in a box of height `h` starting at `y`.
    fn text_c(&self, p: &mut dyn Painter, x: f32, y: f32, h: f32, s: &str, c: [u8; 4], size: f32, bold: bool) {
        p.text(x, y + (h - size * 1.17) / 2.0, s, c, size, bold);
    }

    fn button(&mut self, p: &mut dyn Painter, r: [f32; 4], label: &str, style: Style, icon: Option<Icon>, act: Action) {
        let hov = self.hovered(r);
        let (bg, fg) = match style {
            Style::Primary => (if hov { lighten(ACCENT, 20) } else { ACCENT }, ON_ACCENT),
            Style::Secondary => (if hov { lighten(CARD2, 14) } else { CARD2 }, TEXT),
            Style::Ghost => (if hov { alpha([255, 255, 255, 255], 22) } else { alpha([255, 255, 255, 255], 10) }, TEXT),
            Style::Danger => (if hov { alpha(DANGER, 70) } else { alpha(DANGER, 36) }, DANGER),
        };
        let radius = if style == Style::Primary { r[3] / 2.0 } else { self.d(10.0) };
        p.round_rect(r, radius, bg);
        let size = if style == Style::Primary { self.d(15.0) } else { self.d(13.5) };
        let tw = p.text_width(label, size, true);
        let isz = self.d(18.0);
        let total = tw + if icon.is_some() { isz + self.d(8.0) } else { 0.0 };
        let mut x = r[0] + (r[2] - total) / 2.0;
        if let Some(i) = icon {
            p.icon(i, [x, r[1] + (r[3] - isz) / 2.0, isz, isz], fg);
            x += isz + self.d(8.0);
        }
        self.text_c(p, x, r[1], r[3], label, fg, size, true);
        self.hits.push((r, Hit::Act(act)));
    }

    /// A button as wide as its label, at (x, y); returns its right edge.
    fn auto_button(&mut self, p: &mut dyn Painter, x: f32, y: f32, h: f32, label: &str, style: Style, icon: Option<Icon>, act: Action) -> f32 {
        let size = if style == Style::Primary { self.d(15.0) } else { self.d(13.5) };
        let w = p.text_width(label, size, true) + if icon.is_some() { self.d(26.0) } else { 0.0 } + self.d(if style == Style::Primary { 44.0 } else { 32.0 });
        self.button(p, [x, y, w, h], label, style, icon, act);
        x + w
    }

    fn chip(&self, p: &mut dyn Painter, x: f32, y: f32, label: &str, bg: [u8; 4], fg: [u8; 4]) -> f32 {
        let size = self.d(11.0);
        let w = p.text_width(label, size, true) + self.d(16.0);
        let h = self.d(22.0);
        p.round_rect([x, y, w, h], h / 2.0, bg);
        self.text_c(p, x + self.d(8.0), y, h, label, fg, size, true);
        x + w
    }

    fn toggle(&mut self, p: &mut dyn Painter, x: f32, y: f32, on: bool, act: Action) {
        let (w, h) = (self.d(48.0), self.d(26.0));
        let r = [x, y, w, h];
        p.round_rect(r, h / 2.0, if on { ACCENT } else { CARD2 });
        let k = h - self.d(6.0);
        let kx = if on { x + w - k - self.d(3.0) } else { x + self.d(3.0) };
        p.round_rect([kx, y + self.d(3.0), k, k], k / 2.0, if on { ON_ACCENT } else { DIM });
        self.hits.push(([x - self.d(6.0), y - self.d(6.0), w + self.d(12.0), h + self.d(12.0)], Hit::Act(act)));
    }

    /// Draws the whole screen and rebuilds the hit regions.
    pub fn draw(&mut self, p: &mut dyn Painter) {
        self.hits.clear();
        let (w, h) = (self.w, self.h);
        p.rect([0.0, 0.0, w, h], BG);
        let tabs = self.tabs();
        if !tabs.iter().any(|(t, _, _)| *t == self.tab) {
            self.tab = Tab::Play;
        }
        let sb = (w * 0.22).clamp(self.d(172.0), self.d(236.0));
        self.draw_sidebar(p, sb, &tabs);

        let m = self.d(28.0);
        let toast_h = if self.state.busy.is_some() || self.state.message.is_some() { self.d(64.0) } else { 0.0 };
        let area = [sb + m, self.d(24.0), (w - sb - 2.0 * m).max(0.0), (h - self.d(48.0) - toast_h).max(0.0)];
        match self.tab {
            Tab::Play => self.draw_play(p, area),
            Tab::Versions => self.draw_versions(p, area),
            Tab::Mods => self.draw_mods(p, area),
            Tab::Settings => self.draw_settings(p, area),
        }
        self.draw_toast(p, [area[0], h - self.d(24.0) - self.d(52.0), area[2], self.d(52.0)]);
    }

    fn draw_sidebar(&mut self, p: &mut dyn Painter, sb: f32, tabs: &[(Tab, &'static str, Icon)]) {
        let (h, d) = (self.h, self.dp);
        p.rect([0.0, 0.0, sb, h], SIDEBAR);
        let x = self.d(22.0);
        p.text(x, self.d(24.0), "FROGGER 2", ACCENT, self.d(22.0), true);
        let sub = self.fit(p, "Swampy's Revenge (Rust)", sb - 2.0 * x, self.d(12.5), false);
        p.text(x, self.d(52.0), &sub, DIM, self.d(12.5), false);
        let mut y = self.d(96.0);
        for (t, label, icon) in tabs {
            let r = [self.d(12.0), y, sb - self.d(24.0), self.d(46.0)];
            let active = *t == self.tab;
            if active {
                p.round_rect(r, self.d(12.0), alpha(ACCENT, 38));
                p.round_rect([r[0], r[1] + self.d(10.0), self.d(4.0), r[3] - self.d(20.0)], self.d(2.0), ACCENT);
            } else if self.hovered(r) {
                p.round_rect(r, self.d(12.0), alpha([255, 255, 255, 255], 14));
            }
            let c = if active { ACCENT } else { DIM };
            let isz = self.d(22.0);
            p.icon(*icon, [r[0] + self.d(18.0), r[1] + (r[3] - isz) / 2.0, isz, isz], c);
            self.text_c(p, r[0] + self.d(54.0), r[1], r[3], label, if active { TEXT } else { c }, self.d(15.0), active);
            self.hits.push((r, Hit::Tab(*t)));
            y += self.d(52.0);
        }
        // Bottom: back button (Android), version, update chip.
        let mut by = h - self.d(24.0);
        let _ = d;
        p.text(x, by - self.d(16.0), &format!("v{}", self.state.app_version), DIM, self.d(12.0), false);
        by -= self.d(30.0);
        if let Some(v) = self.state.update_available.clone() {
            let r = [self.d(12.0), by - self.d(34.0), sb - self.d(24.0), self.d(34.0)];
            p.round_rect(r, self.d(10.0), alpha(ACCENT, 46));
            let s = self.fit(p, &format!("Update {v}"), r[2] - self.d(24.0), self.d(12.5), true);
            self.text_c(p, r[0] + self.d(12.0), r[1], r[3], &s, ACCENT, self.d(12.5), true);
            let target = if self.caps.versions { Hit::Tab(Tab::Versions) } else { Hit::Act(Action::OpenUpdates) };
            self.hits.push((r, target));
            by -= self.d(44.0);
        }
        if let Some(label) = self.caps.close_label {
            let r = [self.d(12.0), by - self.d(42.0), sb - self.d(24.0), self.d(42.0)];
            self.button(p, r, label, Style::Secondary, Some(Icon::Back), Action::Close);
        }
    }

    fn page_title(&self, p: &mut dyn Painter, a: [f32; 4], title: &str, sub: &str) -> f32 {
        p.text(a[0], a[1], title, TEXT, self.d(28.0), true);
        let s = self.fit(p, sub, a[2], self.d(13.5), false);
        p.text(a[0], a[1] + self.d(40.0), &s, DIM, self.d(13.5), false);
        a[1] + self.d(76.0)
    }

    fn draw_play(&mut self, p: &mut dyn Painter, a: [f32; 4]) {
        let d = self.dp;
        let hero_h = (a[3] * 0.56).clamp(self.d(190.0), self.d(380.0)).min(a[3]);
        let r = [a[0], a[1], a[2], hero_h];
        let radius = self.d(18.0);
        let have_game = self.state.game.is_some();
        match (self.state.hero, have_game) {
            (Some((iw, ih)), true) => {
                // Cover crop, keeping the lower-middle (the game's logo and frog).
                let (ra, ia) = (r[2] / r[3], iw as f32 / ih as f32);
                let uv = if ia > ra {
                    let k = ra / ia;
                    [(1.0 - k) / 2.0, 0.0, (1.0 + k) / 2.0, 1.0]
                } else {
                    let k = ia / ra;
                    [0.0, (1.0 - k) * 0.55, 1.0, (1.0 - k) * 0.55 + k]
                };
                p.image(HERO_KEY, r, uv);
                // Darken the lower part so the text reads on any picture.
                let gh = r[3] * 0.7;
                p.gradient_v([r[0], r[1] + r[3] - gh, r[2], gh], [0, 0, 0, 0], [0, 0, 0, 215]);
            }
            _ => p.gradient_v(r, [32, 70, 40, 255], [16, 30, 24, 255]),
        }
        p.round_mask(r, radius, BG);
        let pad = self.d(32.0);
        let bh = self.d(52.0);
        let by = r[1] + r[3] - pad - bh + self.d(8.0);
        if have_game {
            p.text(r[0] + pad, by - self.d(112.0), "FROGGER 2", ACCENT, self.d(13.0), true);
            let title = self.state.game.clone().unwrap_or_default();
            let title = self.fit(p, &title, r[2] - 2.0 * pad, self.d(32.0), true);
            p.text(r[0] + pad, by - self.d(92.0), &title, TEXT, self.d(32.0), true);
            p.text(r[0] + pad, by - self.d(48.0), &format!("Rust port  \u{b7}  v{}", self.state.app_version), [200, 206, 214, 255], self.d(14.0), false);
            let mut x = r[0] + pad;
            x = self.auto_button(p, x, by, bh, "PLAY", Style::Primary, Some(Icon::Play), Action::Play) + self.d(12.0);
            if self.caps.import {
                self.auto_button(p, x, by, bh, "Re-import game", Style::Ghost, None, Action::Import);
            }
        } else {
            p.text(r[0] + pad, r[1] + pad, "WELCOME", ACCENT, self.d(13.0), true);
            p.text(r[0] + pad, r[1] + pad + self.d(22.0), "Let's set up your game", TEXT, self.d(30.0), true);
            let mut y = r[1] + pad + self.d(70.0);
            let tw = r[2] - 2.0 * pad;
            for l in wrap_px("Import your own copy of Frogger 2. Nothing from the game is downloaded or shipped with this program.", tw, |s| {
                p.text_width(s, self.d(14.5), false)
            }) {
                p.text(r[0] + pad, y, &l, [214, 220, 226, 255], self.d(14.5), false);
                y += self.d(22.0);
            }
            y += self.d(8.0);
            for (n, step) in [
                "Pick your disc drive, a disc image (.cue, .bin, .iso, .zip) or an install folder.",
                "It is imported on this computer; the music is ripped from the disc.",
                "Press PLAY.",
            ]
            .iter()
            .enumerate()
            {
                if y + self.d(22.0) > by - self.d(8.0) {
                    break;
                }
                let dot = self.d(22.0);
                p.round_rect([r[0] + pad, y - self.d(2.0), dot, dot], dot / 2.0, alpha(ACCENT, 60));
                let num = (n + 1).to_string();
                let nw = p.text_width(&num, self.d(12.5), true);
                p.text(r[0] + pad + (dot - nw) / 2.0, y + self.d(2.0), &num, ACCENT, self.d(12.5), true);
                let s = self.fit(p, step, tw - dot - self.d(12.0), self.d(14.0), false);
                p.text(r[0] + pad + dot + self.d(12.0), y, &s, [200, 206, 214, 255], self.d(14.0), false);
                y += self.d(30.0);
            }
            if self.caps.import {
                self.auto_button(p, r[0] + pad, by, bh, "IMPORT GAME", Style::Primary, Some(Icon::Download), Action::Import);
            }
        }
        if let Some(label) = self.caps.close_label.filter(|_| false) {
            let _ = label;
        }

        // Info cards under the hero.
        let top = r[1] + r[3] + self.d(18.0);
        let ch = self.d(84.0);
        if top + ch > a[1] + a[3] {
            return;
        }
        let gap = self.d(14.0);
        let cw = (a[2] - 2.0 * gap) / 3.0;
        let enabled = self.state.mods.iter().filter(|m| m.enabled).count();
        let version_line = match &self.state.update_available {
            Some(v) => format!("Update available: {v}"),
            None => "Up to date".to_string(),
        };
        let cards = [
            ("GAME DATA", self.state.data_dir.clone(), DIM),
            ("VERSION", format!("v{}   {}", self.state.app_version, version_line), if self.state.update_available.is_some() { ACCENT } else { DIM }),
            ("MODS", format!("{} of {} enabled", enabled, self.state.mods.len()), DIM),
        ];
        let _ = d;
        for (i, (cap, text, color)) in cards.iter().enumerate() {
            let cr = [a[0] + i as f32 * (cw + gap), top, cw, ch];
            p.round_rect(cr, self.d(14.0), CARD);
            p.text(cr[0] + self.d(18.0), cr[1] + self.d(16.0), cap, DIM, self.d(11.0), true);
            let t = self.fit(p, text, cw - self.d(36.0), self.d(14.0), false);
            p.text(cr[0] + self.d(18.0), cr[1] + self.d(42.0), &t, if *color == DIM { TEXT } else { *color }, self.d(14.0), false);
        }
    }

    /// The row pitch and how many of `len` rows are visible below `y0` (one slot goes to the
    /// scroll controls when the list does not fit); clamps the scroll position.
    fn list_geometry(&mut self, a: [f32; 4], y0: f32, len: usize) -> (f32, usize) {
        let pitch = self.d(72.0);
        let fit_rows = (((a[1] + a[3] - y0) / pitch).floor() as usize).max(1);
        let n = if len > fit_rows { fit_rows.saturating_sub(1).max(1) } else { fit_rows };
        self.scroll = self.scroll.min(len.saturating_sub(n));
        (pitch, n)
    }

    fn scroll_controls(&mut self, p: &mut dyn Painter, a: [f32; 4], len: usize, fit_rows: usize) {
        if len <= fit_rows {
            return;
        }
        let (bh, bw) = (self.d(34.0), self.d(64.0));
        let y = a[1] + a[3] - bh;
        let x = a[0] + a[2] - 2.0 * bw - self.d(8.0);
        let first = self.scroll + 1;
        let last = (self.scroll + fit_rows).min(len);
        let s = format!("{first}-{last} of {len}");
        let tw = p.text_width(&s, self.d(12.5), false);
        p.text(x - tw - self.d(14.0), y + (bh - self.d(12.5) * 1.17) / 2.0, &s, DIM, self.d(12.5), false);
        self.button(p, [x, y, bw, bh], "Prev", Style::Ghost, None, Action::None);
        self.hits.pop();
        self.hits.push(([x, y, bw, bh], Hit::Scroll(-(fit_rows as i32))));
        self.button(p, [x + bw + self.d(8.0), y, bw, bh], "Next", Style::Ghost, None, Action::None);
        self.hits.pop();
        self.hits.push(([x + bw + self.d(8.0), y, bw, bh], Hit::Scroll(fit_rows as i32)));
    }

    fn draw_versions(&mut self, p: &mut dyn Painter, a: [f32; 4]) {
        let y = self.page_title(p, a, "Versions", "Releases from GitHub. Versions are installed side by side; your game data, saves and mods are shared.");
        let bh = self.d(40.0);
        let x = self.auto_button(p, a[0], y, bh, "Refresh", Style::Secondary, Some(Icon::Refresh), Action::RefreshReleases);
        let note = self.fit(p, &self.state.versions_note.clone(), a[0] + a[2] - x - self.d(24.0), self.d(13.0), false);
        self.text_c(p, x + self.d(14.0), y, bh, &note, DIM, self.d(13.0), false);
        let y0 = y + bh + self.d(16.0);
        let rows = self.state.versions.clone();
        let (pitch, n) = self.list_geometry(a, y0, rows.len());
        for (i, v) in rows.iter().enumerate().skip(self.scroll).take(n) {
            let ry = y0 + (i - self.scroll) as f32 * pitch;
            let r = [a[0], ry, a[2], pitch - self.d(10.0)];
            p.round_rect(r, self.d(14.0), if v.selected { [34, 52, 40, 255] } else { CARD });
            if v.selected {
                p.round_rect([r[0], r[1] + self.d(14.0), self.d(4.0), r[3] - self.d(28.0)], self.d(2.0), ACCENT);
            }
            let mut bx = r[0] + r[2] - self.d(18.0);
            let by = r[1] + (r[3] - self.d(38.0)) / 2.0;
            let bh = self.d(38.0);
            let mut put = |this: &mut Launcher, p: &mut dyn Painter, label: &str, st: Style, icon: Option<Icon>, act: Action| {
                let size = this.d(13.5);
                let bw = p.text_width(label, size, true) + this.d(32.0) + if icon.is_some() { this.d(26.0) } else { 0.0 };
                bx -= bw;
                this.button(p, [bx, by, bw, bh], label, st, icon, act);
                bx -= this.d(10.0);
            };
            if v.installed && !v.tag.is_empty() && !v.selected {
                put(self, p, "Remove", Style::Danger, None, Action::Remove(v.tag.clone()));
            }
            if v.selected {
                let w = p.text_width("IN USE", self.d(11.0), true) + self.d(16.0);
                bx -= w;
                self.chip(p, bx, r[1] + (r[3] - self.d(22.0)) / 2.0, "IN USE", ACCENT, ON_ACCENT);
            } else if v.installed {
                put(self, p, "Use", Style::Secondary, Some(Icon::Check), Action::Use(v.tag.clone()));
            } else if v.available {
                put(self, p, "Install", Style::Primary, Some(Icon::Download), Action::Install(v.tag.clone()));
            } else {
                let w = p.text_width("NO BUILD FOR THIS SYSTEM", self.d(11.0), true) + self.d(16.0);
                bx -= w;
                self.chip(p, bx, r[1] + (r[3] - self.d(22.0)) / 2.0, "NO BUILD FOR THIS SYSTEM", CARD2, DIM);
            }
            let avail = (bx - r[0] - self.d(40.0)).max(self.d(60.0));
            let label = self.fit(p, &v.label, avail, self.d(16.0), true);
            p.text(r[0] + self.d(22.0), r[1] + self.d(12.0), &label, TEXT, self.d(16.0), true);
            let detail = self.fit(p, &v.detail, avail, self.d(12.5), false);
            p.text(r[0] + self.d(22.0), r[1] + self.d(36.0), &detail, DIM, self.d(12.5), false);
        }
        if rows.is_empty() {
            p.text(a[0], y0 + self.d(8.0), "Nothing to show yet.", DIM, self.d(14.0), false);
        }
        self.scroll_controls(p, a, rows.len(), n);
    }

    fn draw_mods(&mut self, p: &mut dyn Painter, a: [f32; 4]) {
        let dir = self.state.mods_dir.clone();
        let mut y = self.page_title(p, a, "Mods", &format!("Mods folder: {dir}"));
        let bh = self.d(40.0);
        if self.caps.open_folders {
            self.auto_button(p, a[0], y, bh, "Open mods folder", Style::Secondary, Some(Icon::Folder), Action::OpenModsFolder);
            y += bh + self.d(16.0);
        }
        let rows = self.state.mods.clone();
        if rows.is_empty() {
            let r = [a[0], y, a[2], self.d(110.0).min((a[1] + a[3] - y).max(0.0))];
            p.round_rect(r, self.d(14.0), CARD);
            let mut ty = r[1] + self.d(20.0);
            for l in wrap_px(
                "No mods installed. Put a mod's folder (with its mod.toml) into the mods folder and it shows up here. Downloading mods from here is planned.",
                r[2] - self.d(40.0),
                |s| p.text_width(s, self.d(14.0), false),
            ) {
                p.text(r[0] + self.d(20.0), ty, &l, DIM, self.d(14.0), false);
                ty += self.d(22.0);
            }
            return;
        }
        let (pitch, n) = self.list_geometry(a, y, rows.len());
        for (i, m) in rows.iter().enumerate().skip(self.scroll).take(n) {
            let ry = y + (i - self.scroll) as f32 * pitch;
            let r = [a[0], ry, a[2], pitch - self.d(10.0)];
            p.round_rect(r, self.d(14.0), CARD);
            let avail = r[2] - self.d(110.0);
            let name = self.fit(p, &m.name, avail, self.d(16.0), true);
            p.text(r[0] + self.d(22.0), r[1] + self.d(12.0), &name, TEXT, self.d(16.0), true);
            let sub = if m.version.is_empty() { m.id.clone() } else { format!("{}  \u{b7}  v{}", m.id, m.version) };
            let sub = self.fit(p, &sub, avail, self.d(12.5), false);
            p.text(r[0] + self.d(22.0), r[1] + self.d(36.0), &sub, DIM, self.d(12.5), false);
            self.toggle(p, r[0] + r[2] - self.d(70.0), r[1] + (r[3] - self.d(26.0)) / 2.0, m.enabled, Action::ToggleMod(m.id.clone()));
        }
        self.scroll_controls(p, a, rows.len(), n);
    }

    fn draw_settings(&mut self, p: &mut dyn Painter, a: [f32; 4]) {
        let mut y = self.page_title(p, a, "Settings", "Saved in config.toml.");
        // On start.
        let ch = self.d(116.0);
        let r = [a[0], y, a[2], ch];
        p.round_rect(r, self.d(14.0), CARD);
        p.text(r[0] + self.d(22.0), r[1] + self.d(18.0), "On start, open", TEXT, self.d(16.0), true);
        p.text(r[0] + self.d(22.0), r[1] + self.d(42.0), "What this app shows when you start it.", DIM, self.d(12.5), false);
        let start = self.state.start_in_launcher;
        let (sw, sh) = (self.d(110.0), self.d(38.0));
        let sx = r[0] + self.d(22.0);
        let sy = r[1] + ch - sh - self.d(18.0);
        p.round_rect([sx, sy, 2.0 * sw + self.d(6.0), sh + self.d(6.0)], (sh + self.d(6.0)) / 2.0, BG);
        let seg = |this: &mut Launcher, p: &mut dyn Painter, i: f32, label: &str, on: bool, act: Action| {
            let rr = [sx + this.d(3.0) + i * sw, sy + this.d(3.0), sw, sh];
            this.button(p, rr, label, if on { Style::Primary } else { Style::Ghost }, None, act);
        };
        seg(self, p, 0.0, "Game", !start, Action::SetStartInLauncher(false));
        seg(self, p, 1.0, "Launcher", start, Action::SetStartInLauncher(true));
        y += ch + self.d(14.0);
        if self.caps.updates_page {
            let r = [a[0], y, a[2], self.d(84.0)];
            p.round_rect(r, self.d(14.0), CARD);
            p.text(r[0] + self.d(22.0), r[1] + self.d(16.0), "Updates", TEXT, self.d(16.0), true);
            self.auto_button(
                p,
                r[0] + self.d(22.0),
                r[1] + r[3] - self.d(46.0),
                self.d(34.0),
                "Check for updates online",
                Style::Secondary,
                Some(Icon::Download),
                Action::OpenUpdates,
            );
            y += r[3] + self.d(14.0);
        }
        let r = [a[0], y, a[2], self.d(96.0).min((a[1] + a[3] - y).max(0.0))];
        p.round_rect(r, self.d(14.0), CARD);
        p.text(r[0] + self.d(22.0), r[1] + self.d(16.0), "Locations", TEXT, self.d(16.0), true);
        for (i, (k, v)) in [("Settings file", self.state.config_path.clone()), ("Game data", self.state.data_dir.clone())].iter().enumerate() {
            let t = self.fit(p, &format!("{k}: {v}"), r[2] - self.d(44.0), self.d(12.5), false);
            p.text(r[0] + self.d(22.0), r[1] + self.d(44.0) + i as f32 * self.d(22.0), &t, DIM, self.d(12.5), false);
        }
    }

    fn draw_toast(&mut self, p: &mut dyn Painter, r: [f32; 4]) {
        if let Some((what, f)) = self.state.busy.clone() {
            p.round_rect(r, self.d(14.0), CARD2);
            let t = self.fit(p, &what, r[2] - self.d(40.0), self.d(13.5), false);
            p.text(r[0] + self.d(20.0), r[1] + self.d(10.0), &t, TEXT, self.d(13.5), false);
            let tr = [r[0] + self.d(20.0), r[1] + r[3] - self.d(16.0), r[2] - self.d(40.0), self.d(6.0)];
            p.round_rect(tr, self.d(3.0), BG);
            let fw = (tr[2] * f.clamp(0.0, 1.0)).max(self.d(6.0));
            p.round_rect([tr[0], tr[1], fw, tr[3]], self.d(3.0), ACCENT);
        } else if let Some((msg, bad)) = self.state.message.clone() {
            p.round_rect(r, self.d(14.0), CARD2);
            let dot = self.d(10.0);
            p.round_rect([r[0] + self.d(20.0), r[1] + (r[3] - dot) / 2.0, dot, dot], dot / 2.0, if bad { DANGER } else { ACCENT_DARK });
            let t = self.fit(p, &msg, r[2] - self.d(64.0), self.d(13.5), false);
            self.text_c(p, r[0] + self.d(42.0), r[1], r[3], &t, if bad { DANGER } else { TEXT }, self.d(13.5), false);
        }
    }
}

/// Greedy word wrap to `width` pixels.
pub fn wrap_px(t: &str, width: f32, mut text_width: impl FnMut(&str) -> f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in t.split_whitespace() {
        let cand = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
        if !cur.is_empty() && text_width(&cand) > width {
            lines.push(std::mem::take(&mut cur));
            cur = word.to_string();
        } else {
            cur = cand;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records what is drawn; text is `0.52 * size` wide per character.
    #[derive(Default)]
    struct Rec {
        rects: Vec<[f32; 4]>,
        texts: Vec<(f32, f32, String, f32)>,
    }

    impl Painter for Rec {
        fn rect(&mut self, r: [f32; 4], _: [u8; 4]) {
            self.rects.push(r);
        }
        fn round_rect(&mut self, r: [f32; 4], _: f32, _: [u8; 4]) {
            self.rects.push(r);
        }
        fn gradient_v(&mut self, r: [f32; 4], _: [u8; 4], _: [u8; 4]) {
            self.rects.push(r);
        }
        fn text(&mut self, x: f32, y: f32, s: &str, _: [u8; 4], size: f32, _: bool) {
            self.texts.push((x, y, s.to_string(), size));
        }
        fn text_width(&mut self, s: &str, size: f32, _: bool) -> f32 {
            s.chars().count() as f32 * size * 0.52
        }
        fn icon(&mut self, _: Icon, r: [f32; 4], _: [u8; 4]) {
            self.rects.push(r);
        }
        fn image(&mut self, _: &str, r: [f32; 4], _: [f32; 4]) {
            self.rects.push(r);
        }
        fn round_mask(&mut self, _: [f32; 4], _: f32, _: [u8; 4]) {}
    }

    fn desktop() -> Launcher {
        let mut l = Launcher::new(Caps { versions: true, import: true, open_folders: true, updates_page: false, close_label: None }, 1280.0, 720.0, 1.0);
        l.state.game = Some("Frogger 2: Swampy's Revenge (USA)".into());
        l.state.app_version = "0.1.0".into();
        l.state.data_dir = "/home/x/frogger2-rs/data".into();
        l.state.versions = (0..12)
            .map(|i| VersionRow {
                tag: format!("v0.{i}.0"),
                label: format!("v0.{i}.0"),
                detail: format!("2026-10-{:02}", i + 1),
                installed: i % 3 == 0,
                selected: i == 0,
                available: i != 5,
            })
            .collect();
        l.state.mods = vec![ModRow { id: "hello-mod".into(), name: "Hello, mod!".into(), version: "0.1.0".into(), enabled: true }];
        l
    }

    /// Draws and presses at the centre of the first text equal to `label` (a button's label
    /// is centred in its button).
    fn click_label(l: &mut Launcher, label: &str) -> Action {
        let mut r = Rec::default();
        l.draw(&mut r);
        let (x, y, s, size) = r.texts.iter().find(|t| t.2 == label).unwrap_or_else(|| panic!("no {label} drawn")).clone();
        l.press(x + s.chars().count() as f32 * size * 0.26, y + size * 0.5)
    }

    #[test]
    fn everything_fits_on_screen_sizes() {
        for (w, h, dp) in [(1280.0, 720.0, 1.0), (1920.0, 1080.0, 2.0), (800.0, 480.0, 1.0), (2400.0, 1080.0, 2.75), (1000.0, 600.0, 1.25)] {
            let mut l = desktop();
            l.resize(w, h, dp);
            l.state.busy = Some(("Downloading".into(), 0.5));
            l.state.update_available = Some("v0.2.0".into());
            for (t, _, _) in l.tabs() {
                l.tab = t;
                let mut r = Rec::default();
                l.draw(&mut r);
                for [x, y, rw, rh] in &r.rects {
                    assert!(*x >= -0.5 && *y >= -0.5 && x + rw <= w + 0.5 && y + rh <= h + 0.5, "{t:?} {w}x{h}@{dp}: rect {x},{y} {rw}x{rh}");
                }
                for (x, y, s, size) in &r.texts {
                    let tw = s.chars().count() as f32 * size * 0.52;
                    assert!(
                        *x >= -0.5 && *y >= -0.5 && x + tw <= w + 0.5 && y + size * 1.17 <= h + 0.5,
                        "{t:?} {w}x{h}@{dp}: text {s:?} at {x},{y} ({tw} wide)"
                    );
                }
                assert!(!r.texts.is_empty());
            }
        }
    }

    #[test]
    fn tabs_switch() {
        let mut l = desktop();
        assert_eq!(l.tab, Tab::Play);
        assert_eq!(click_label(&mut l, "Mods"), Action::None);
        assert_eq!(l.tab, Tab::Mods);
        l.tab = Tab::Play;
        assert_eq!(click_label(&mut l, "PLAY"), Action::Play);
    }

    #[test]
    fn play_import_and_settings() {
        let mut l = desktop();
        assert_eq!(click_label(&mut l, "Re-import game"), Action::Import);
        l.state.game = None;
        assert_eq!(click_label(&mut l, "IMPORT GAME"), Action::Import);
        l.state.game = Some("g".into());
        click_label(&mut l, "Settings");
        assert_eq!(l.tab, Tab::Settings);
        assert_eq!(click_label(&mut l, "Launcher"), Action::SetStartInLauncher(true));
        assert_eq!(click_label(&mut l, "Game"), Action::SetStartInLauncher(false));
    }

    #[test]
    fn versions_and_mods_actions() {
        let mut l = desktop();
        l.state.versions = vec![
            VersionRow { tag: "v0.3.0".into(), label: "v0.3.0".into(), installed: true, selected: false, available: true, ..Default::default() },
            VersionRow { tag: "v0.2.0".into(), label: "v0.2.0".into(), installed: false, selected: false, available: true, ..Default::default() },
            VersionRow { tag: "v0.1.0".into(), label: "v0.1.0".into(), installed: true, selected: true, available: true, ..Default::default() },
        ];
        click_label(&mut l, "Versions");
        assert_eq!(l.tab, Tab::Versions);
        assert_eq!(click_label(&mut l, "Refresh"), Action::RefreshReleases);
        assert_eq!(click_label(&mut l, "Install"), Action::Install("v0.2.0".into()));
        assert_eq!(click_label(&mut l, "Use"), Action::Use("v0.3.0".into()));
        assert_eq!(click_label(&mut l, "Remove"), Action::Remove("v0.3.0".into()));
        click_label(&mut l, "Mods");
        assert_eq!(click_label(&mut l, "Open mods folder"), Action::OpenModsFolder);
        // The toggle has no label: press its centre.
        let mut r = Rec::default();
        l.draw(&mut r);
        let hit = l.hits.iter().find(|(_, h)| matches!(h, Hit::Act(Action::ToggleMod(_)))).unwrap().0;
        assert_eq!(l.press(hit[0] + hit[2] / 2.0, hit[1] + hit[3] / 2.0), Action::ToggleMod("hello-mod".into()));
    }

    #[test]
    fn android_style_caps_hide_versions_and_offer_close() {
        let mut l = Launcher::new(
            Caps { versions: false, import: true, open_folders: false, updates_page: true, close_label: Some("Back to the game") },
            1920.0,
            1080.0,
            2.0,
        );
        assert!(!l.tabs().iter().any(|(t, _, _)| *t == Tab::Versions));
        assert_eq!(click_label(&mut l, "Back to the game"), Action::Close);
        click_label(&mut l, "Settings");
        assert_eq!(click_label(&mut l, "Check for updates online"), Action::OpenUpdates);
    }

    #[test]
    fn update_chip_opens_versions_or_the_releases_page() {
        let mut l = desktop();
        l.state.update_available = Some("v0.2.0".into());
        assert_eq!(click_label(&mut l, "Update v0.2.0"), Action::None);
        assert_eq!(l.tab, Tab::Versions);
        let mut a = Launcher::new(Caps { versions: false, import: false, open_folders: false, updates_page: true, close_label: None }, 1280.0, 720.0, 1.0);
        a.state.update_available = Some("v0.2.0".into());
        assert_eq!(click_label(&mut a, "Update v0.2.0"), Action::OpenUpdates);
    }

    #[test]
    fn long_lists_scroll() {
        let mut l = desktop();
        l.resize(1280.0, 480.0, 1.0);
        click_label(&mut l, "Versions");
        let mut r = Rec::default();
        l.draw(&mut r);
        assert!(r.texts.iter().any(|t| t.2 == "Next"), "scroll controls on a long list");
        assert!(!r.texts.iter().any(|t| t.2 == "v0.11.0"));
        l.scroll_by(100);
        let mut r = Rec::default();
        l.draw(&mut r);
        assert!(r.texts.iter().any(|t| t.2 == "v0.11.0"), "scrolled to the end");
        assert!(!r.texts.iter().any(|t| t.2 == "v0.0.0"));
    }

    #[test]
    fn wraps() {
        assert_eq!(wrap_px("aaaa bbbb cccc", 9.0, |s| s.len() as f32), ["aaaa bbbb", "cccc"]);
    }
}
