//! The launcher screen as a model: a sidebar (Play, Versions, Mods, Settings), the import
//! page, cards, buttons, toggles, choices and lists laid out in dp, drawn through a
//! [`Painter`], and hit tested for pointer and touch input. Hosts (desktop window, Android
//! activity) feed [`State`], call [`Launcher::draw`] every frame, and act on the [`Action`] a
//! press returns. The painter does the pixels (anti-aliased text, rounded shapes, icons, the
//! game's pictures); nothing here knows about a renderer.
//!
//! One look throughout: cards have 16 dp corners, every control (buttons, choices, chips,
//! toggles) is a pill, lists and the settings scroll smoothly under a fixed page header.

#![allow(clippy::too_many_arguments)]

use std::time::{Duration, Instant};

// Palette: charcoal surfaces and the frog green accent.
pub const BG: [u8; 4] = [20, 22, 26, 255];
const SIDEBAR: [u8; 4] = [14, 16, 19, 255];
const CARD: [u8; 4] = [30, 34, 40, 255];
const CARD2: [u8; 4] = [42, 47, 55, 255];
const WELL: [u8; 4] = [22, 25, 30, 255];
const LINE: [u8; 4] = [40, 45, 52, 255];
const TEXT: [u8; 4] = [236, 239, 242, 255];
const SOFT: [u8; 4] = [200, 206, 214, 255];
const DIM: [u8; 4] = [150, 158, 168, 255];
const ACCENT: [u8; 4] = [84, 198, 83, 255];
const ON_ACCENT: [u8; 4] = [8, 28, 12, 255];
const DANGER: [u8; 4] = [240, 100, 90, 255];
const YELLOW: [u8; 4] = [236, 208, 40, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

/// How much of the hero picture (from the top) may show.
const HERO_TOP: f32 = 0.5;
/// Height of buttons that do something (dp).
const BTN: f32 = 40.0;
/// Height of the controls of a settings row and of small header buttons (dp).
const CTRL: f32 = 36.0;
/// Inner padding of cards (dp).
const PAD: f32 = 24.0;
/// Corner radius of cards (dp).
const CARD_R: f32 = 16.0;
/// How long a message that is not an error stays up.
const MESSAGE_TIME: Duration = Duration::from_secs(6);

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
    Disc,
    File,
    Link,
    ChevronLeft,
    ChevronRight,
}

impl Icon {
    pub const ALL: [Icon; 14] = [
        Icon::Play,
        Icon::Layers,
        Icon::Grid,
        Icon::Gear,
        Icon::Check,
        Icon::Download,
        Icon::Refresh,
        Icon::Folder,
        Icon::Back,
        Icon::Disc,
        Icon::File,
        Icon::Link,
        Icon::ChevronLeft,
        Icon::ChevronRight,
    ];
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
/// The game's logo (transparent), registered the same way.
pub const LOGO_KEY: &str = "logo";
/// The game's icon (square, rounded), shown in the sidebar.
pub const ICON_KEY: &str = "icon";

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
    /// The import page was opened (look for drives again).
    Import,
    /// The import page was left without importing.
    CancelImport,
    /// Back to the import choices after a failed import.
    RetryImport,
    /// Import from `state.import.sources[i]`.
    ImportSource(usize),
    /// Ask for a disc image with the system's file dialog.
    PickImage,
    /// Ask for a folder (a copied disc or an installed game).
    PickFolder,
    RescanDrives,
    /// Leave the launcher (Android: back to the game).
    Close,
    RefreshReleases,
    Install(String),
    /// Run this version (`""` = the one in the install folder itself).
    Use(String),
    Remove(String),
    ToggleMod(String),
    OpenModsFolder,
    OpenUpdates,
    /// Make the "Frogger 2: Swampy's Revenge" shortcut to the game.
    CreateShortcut,
    /// A choice of the settings page: (setting id, option index).
    SetChoice(&'static str, usize),
    /// A switch of the settings page.
    SetToggle(&'static str, bool),
    /// A button of the settings page.
    SettingButton(&'static str),
}

/// What this host can do (tabs and buttons it cannot back are hidden).
#[derive(Clone, Copy, Debug, Default)]
pub struct Caps {
    /// Install and switch between releases downloaded from GitHub (desktop).
    pub versions: bool,
    /// Offer importing the game (and importing it again).
    pub import: bool,
    /// The import page lists disc drives (desktop).
    pub drives: bool,
    /// "Choose folder" besides "Choose disc image".
    pub pick_folder: bool,
    /// Files can be dropped onto the window.
    pub drop_files: bool,
    /// Open folders in the system file manager.
    pub open_folders: bool,
    /// The update chip opens the releases page in the browser (Android: no in-app updates).
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

/// One place the game can be imported from (a disc drive, a file found by the host).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceRow {
    pub label: String,
    pub detail: String,
    /// It holds Frogger 2 (marked and offered first).
    pub highlight: bool,
    /// It can be imported now (a drive with a disc in it).
    pub ready: bool,
}

/// A running import (or the unpacking of a bundled game).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    /// "Importing Frogger2.iso".
    pub title: String,
    /// The importer's current stage.
    pub stage: String,
    /// 0..=1.
    pub fraction: f32,
    /// The latest log lines, oldest first.
    pub log: Vec<String>,
}

/// What the import page shows.
#[derive(Clone, Debug, Default)]
pub struct ImportView {
    /// Title of the source list ("Disc drives", "Found in the app's folder"); empty with no
    /// sources hides the list.
    pub sources_title: String,
    pub sources: Vec<SourceRow>,
    /// Shown in the list when it is empty ("No CD or DVD drives found").
    pub empty_note: String,
    /// The host is looking for sources.
    pub scanning: bool,
    pub running: Option<Progress>,
    /// The last import failed with this.
    pub error: Option<String>,
    /// A line under the file buttons ("No file chosen", the Android copy status).
    pub note: Option<String>,
    /// Extra paragraphs under the file buttons (where to copy files on Android).
    pub help: Vec<String>,
    /// The game bundled with the app is being unpacked: no choices, no way back.
    pub unpacking: bool,
}

/// The control of one settings row.
#[derive(Clone, Debug, PartialEq)]
pub enum SettingKind {
    /// Pick one of the options (segmented when they fit, else a stepper).
    Choice {
        options: Vec<String>,
        selected: usize,
    },
    Toggle(bool),
    /// A button with this label.
    Button(String),
    /// Text only.
    Info,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SettingRow {
    pub id: &'static str,
    pub label: String,
    pub detail: String,
    pub kind: SettingKind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SettingGroup {
    pub title: String,
    pub rows: Vec<SettingRow>,
}

/// What the host tells the launcher to show.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// Title of the player's game once imported (`None`: no game data yet).
    pub game: Option<String>,
    pub data_dir: String,
    pub app_version: String,
    /// A newer release exists (its tag).
    pub update_available: Option<String>,
    pub versions: Vec<VersionRow>,
    /// One line beside the Refresh button ("Checking GitHub...", "offline: ...").
    pub versions_note: String,
    pub mods: Vec<ModRow>,
    pub mods_dir: String,
    pub import: ImportView,
    pub settings: Vec<SettingGroup>,
    /// Whether the game's shortcut exists (`None`: this host makes none).
    pub shortcut: Option<bool>,
    /// Size of the picture registered as [`HERO_KEY`] (the game's artwork), if any.
    pub hero: Option<(u32, u32)>,
    /// Size of the picture registered as [`LOGO_KEY`], if any.
    pub logo: Option<(u32, u32)>,
    /// Size of the picture registered as [`ICON_KEY`], if any.
    pub icon: Option<(u32, u32)>,
    /// A running job: what it is doing and how far (0..=1).
    pub busy: Option<(String, f32)>,
    /// The last result line and whether it is an error (other messages go away by themselves).
    pub message: Option<(String, bool)>,
}

#[derive(Clone, Debug)]
enum Hit {
    Tab(Tab),
    Act(Action),
    Dismiss,
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
    /// The import page is open over the Play tab although a game exists.
    import_open: bool,
    /// Scroll offset of the current page in pixels, and its largest value last frame.
    scroll: f32,
    max_scroll: f32,
    hover: Option<(f32, f32)>,
    hits: Vec<([f32; 4], Hit)>,
    /// The scrolling part of the page while it is drawn: hits are cut to it, what lies outside
    /// is skipped.
    clip: Option<[f32; 4]>,
    /// When the current message appeared.
    message_seen: Option<(String, Instant)>,
}

fn inside([x, y, w, h]: [f32; 4], px: f32, py: f32) -> bool {
    px >= x && px <= x + w && py >= y && py <= y + h
}

fn intersect(a: [f32; 4], b: [f32; 4]) -> Option<[f32; 4]> {
    let (x0, y0) = (a[0].max(b[0]), a[1].max(b[1]));
    let (x1, y1) = ((a[0] + a[2]).min(b[0] + b[2]), (a[1] + a[3]).min(b[1] + b[3]));
    (x1 > x0 && y1 > y0).then_some([x0, y0, x1 - x0, y1 - y0])
}

fn alpha(c: [u8; 4], a: u8) -> [u8; 4] {
    [c[0], c[1], c[2], a]
}

fn lighten(c: [u8; 4], by: u8) -> [u8; 4] {
    [c[0].saturating_add(by), c[1].saturating_add(by), c[2].saturating_add(by), c[3]]
}

impl Launcher {
    pub fn new(caps: Caps, w: f32, h: f32, dp: f32) -> Launcher {
        Launcher {
            state: State::default(),
            caps,
            tab: Tab::Play,
            w,
            h,
            dp: dp.max(0.5),
            import_open: false,
            scroll: 0.0,
            max_scroll: 0.0,
            hover: None,
            hits: Vec::new(),
            clip: None,
            message_seen: None,
        }
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

    /// Switches to `t` (scrolled to the top).
    pub fn show_tab(&mut self, t: Tab) {
        if self.tab != t {
            self.scroll = 0.0;
        }
        self.tab = t;
    }

    /// Opens the import page (on the Play tab) or closes it again.
    pub fn show_import(&mut self, open: bool) {
        self.import_open = open;
        if open {
            self.show_tab(Tab::Play);
            self.scroll = 0.0;
        }
    }

    /// The Play tab shows the import page (no game yet, or importing again).
    pub fn import_showing(&self) -> bool {
        let i = &self.state.import;
        self.tab == Tab::Play && (self.state.game.is_none() || self.import_open || i.running.is_some() || i.error.is_some())
    }

    /// Scrolls the current page by `px` pixels (positive = down).
    pub fn scroll_by(&mut self, px: f32) {
        self.scroll = (self.scroll + px).clamp(0.0, self.max_scroll.max(0.0));
    }

    /// The pointer moved (desktop hover highlight).
    pub fn pointer_moved(&mut self, x: f32, y: f32) {
        self.hover = Some((x, y));
    }

    /// The pointer left the window.
    pub fn pointer_left(&mut self) {
        self.hover = None;
    }

    /// A press at (x, y): tab switches and page changes are handled here, everything else
    /// is returned for the host.
    pub fn press(&mut self, x: f32, y: f32) -> Action {
        let Some((_, hit)) = self.hits.iter().rev().find(|(r, _)| inside(*r, x, y)).cloned() else { return Action::None };
        match hit {
            Hit::Tab(t) => {
                self.show_tab(t);
                Action::None
            }
            Hit::Dismiss => {
                self.state.message = None;
                Action::None
            }
            Hit::Act(a) => {
                match a {
                    Action::Import => self.show_import(true),
                    Action::CancelImport => self.show_import(false),
                    _ => {}
                }
                a
            }
        }
    }

    fn d(&self, dp: f32) -> f32 {
        dp * self.dp
    }

    fn hovered(&self, r: [f32; 4]) -> bool {
        self.hover.is_some_and(|(x, y)| inside(r, x, y) && self.clip.is_none_or(|c| inside(c, x, y)))
    }

    fn hit(&mut self, r: [f32; 4], h: Hit) {
        let r = match self.clip {
            Some(c) => match intersect(r, c) {
                Some(r) => r,
                None => return,
            },
            None => r,
        };
        self.hits.push((r, h));
    }

    /// Whether anything of the band `y..y+h` is inside the scrolling part (always true
    /// outside it).
    fn shown(&self, y: f32, h: f32) -> bool {
        self.clip.is_none_or(|c| y + h > c[1] && y < c[1] + c[3])
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

    /// Text at (x, y) (the top of its line), skipped outside the scrolling part.
    fn text(&self, p: &mut dyn Painter, x: f32, y: f32, s: &str, c: [u8; 4], size: f32, bold: bool) {
        if self.shown(y, size * 1.2) {
            p.text(x, y, s, c, size, bold);
        }
    }

    /// Text vertically centred in a box of height `h` starting at `y`.
    fn text_c(&self, p: &mut dyn Painter, x: f32, y: f32, h: f32, s: &str, c: [u8; 4], size: f32, bold: bool) {
        self.text(p, x, y + (h - size * 1.17) / 2.0, s, c, size, bold);
    }

    /// Wrapped text from (x, y) in `w` pixels; returns the y under it.
    fn para(&self, p: &mut dyn Painter, x: f32, mut y: f32, w: f32, s: &str, c: [u8; 4], size: f32) -> f32 {
        for l in wrap_px(s, w, |t| p.text_width(t, size, false)) {
            let l = self.fit(p, &l, w, size, false);
            self.text(p, x, y, &l, c, size, false);
            y += size * 1.5;
        }
        y
    }

    /// A card background; inside the scrolling part it is cut at the edges (the mask over
    /// them hides the cut).
    fn card(&self, p: &mut dyn Painter, r: [f32; 4], color: [u8; 4]) {
        let r = match self.clip {
            Some(c) => {
                let grow = self.d(CARD_R);
                match intersect(r, [c[0] - grow, c[1] - grow, c[2] + 2.0 * grow, c[3] + 2.0 * grow]) {
                    Some(r) => r,
                    None => return,
                }
            }
            None => r,
        };
        p.round_rect(r, self.d(CARD_R), color);
    }

    fn button(&mut self, p: &mut dyn Painter, r: [f32; 4], label: &str, style: Style, icon: Option<Icon>, act: Action) {
        if !self.shown(r[1], r[3]) {
            return;
        }
        let hov = self.hovered(r);
        let (bg, fg) = match style {
            Style::Primary => (if hov { lighten(ACCENT, 20) } else { ACCENT }, ON_ACCENT),
            Style::Secondary => (if hov { lighten(CARD2, 14) } else { CARD2 }, TEXT),
            Style::Ghost => (if hov { alpha(WHITE, 30) } else { alpha(WHITE, 16) }, TEXT),
            Style::Danger => (if hov { alpha(DANGER, 70) } else { alpha(DANGER, 36) }, DANGER),
        };
        p.round_rect(r, r[3] / 2.0, bg);
        let size = self.btn_text(r[3]);
        let tw = p.text_width(label, size, true);
        let isz = (r[3] * 0.42).round();
        let gap = self.d(8.0);
        let total = tw + if icon.is_some() { isz + gap } else { 0.0 };
        let mut x = r[0] + (r[2] - total) / 2.0;
        if let Some(i) = icon {
            p.icon(i, [x, r[1] + (r[3] - isz) / 2.0, isz, isz], fg);
            x += isz + gap;
        }
        self.text_c(p, x, r[1], r[3], label, fg, size, true);
        self.hit(r, Hit::Act(act));
    }

    /// Label size of a button `h` pixels high.
    fn btn_text(&self, h: f32) -> f32 {
        if h >= self.d(48.0) {
            self.d(16.0)
        } else {
            self.d(14.0)
        }
    }

    fn button_w(&self, p: &mut dyn Painter, h: f32, label: &str, icon: bool) -> f32 {
        let size = self.btn_text(h);
        p.text_width(label, size, true) + if icon { (h * 0.42).round() + self.d(8.0) } else { 0.0 } + h * 0.9
    }

    /// A button as wide as its label, at (x, y); returns its right edge.
    fn auto_button(&mut self, p: &mut dyn Painter, x: f32, y: f32, h: f32, label: &str, style: Style, icon: Option<Icon>, act: Action) -> f32 {
        let w = self.button_w(p, h, label, icon.is_some());
        self.button(p, [x, y, w, h], label, style, icon, act);
        x + w
    }

    /// A button as wide as its label, ending at `right`; returns its left edge.
    fn button_left_of(&mut self, p: &mut dyn Painter, right: f32, y: f32, h: f32, label: &str, style: Style, icon: Option<Icon>, act: Action) -> f32 {
        let w = self.button_w(p, h, label, icon.is_some());
        self.button(p, [right - w, y, w, h], label, style, icon, act);
        right - w
    }

    fn chip_w(&self, p: &mut dyn Painter, label: &str) -> f32 {
        p.text_width(label, self.d(11.0), true) + self.d(18.0)
    }

    fn chip(&self, p: &mut dyn Painter, x: f32, y: f32, label: &str, bg: [u8; 4], fg: [u8; 4]) -> f32 {
        let w = self.chip_w(p, label);
        let h = self.d(22.0);
        if self.shown(y, h) {
            p.round_rect([x, y, w, h], h / 2.0, bg);
            self.text_c(p, x + self.d(9.0), y, h, label, fg, self.d(11.0), true);
        }
        x + w
    }

    fn toggle(&mut self, p: &mut dyn Painter, x: f32, y: f32, on: bool, act: Action) {
        let (w, h) = (self.d(48.0), self.d(28.0));
        if !self.shown(y, h) {
            return;
        }
        let r = [x, y, w, h];
        let hov = self.hovered([x - self.d(6.0), y - self.d(6.0), w + self.d(12.0), h + self.d(12.0)]);
        p.round_rect(
            r,
            h / 2.0,
            match (on, hov) {
                (true, false) => ACCENT,
                (true, true) => lighten(ACCENT, 20),
                (false, false) => CARD2,
                (false, true) => lighten(CARD2, 14),
            },
        );
        let k = h - self.d(8.0);
        let kx = if on { x + w - k - self.d(4.0) } else { x + self.d(4.0) };
        p.round_rect([kx, y + self.d(4.0), k, k], k / 2.0, if on { ON_ACCENT } else { DIM });
        self.hit([x - self.d(6.0), y - self.d(6.0), w + self.d(12.0), h + self.d(12.0)], Hit::Act(act));
    }

    /// Width of the control of a setting row (segmented, stepper, toggle, button).
    fn control_w(&self, p: &mut dyn Painter, kind: &SettingKind) -> f32 {
        let size = self.d(13.5);
        match kind {
            SettingKind::Choice { options, .. } => {
                let seg: f32 = options.iter().map(|o| p.text_width(o, size, true) + self.d(28.0)).sum::<f32>() + self.d(8.0);
                if options.len() <= 4 && seg <= self.d(380.0) {
                    seg
                } else {
                    let widest = options.iter().map(|o| p.text_width(o, size, true)).fold(0.0, f32::max);
                    widest + 2.0 * self.d(36.0) + self.d(16.0)
                }
            }
            SettingKind::Toggle(_) => self.d(48.0),
            SettingKind::Button(l) => self.button_w(p, self.d(CTRL), l, false),
            SettingKind::Info => 0.0,
        }
    }

    /// The control of a setting row, right-aligned at `right` and centred on `cy`.
    fn control(&mut self, p: &mut dyn Painter, right: f32, cy: f32, id: &'static str, kind: &SettingKind) {
        let w = self.control_w(p, kind);
        let size = self.d(13.5);
        match kind {
            SettingKind::Choice { options, selected } => {
                let h = self.d(CTRL);
                let (x, y) = (right - w, cy - h / 2.0);
                if !self.shown(y, h) {
                    return;
                }
                p.round_rect([x, y, w, h], h / 2.0, WELL);
                let seg_total: f32 = options.iter().map(|o| p.text_width(o, size, true) + self.d(28.0)).sum::<f32>() + self.d(8.0);
                if (seg_total - w).abs() < 0.5 {
                    // Segmented: every option a pill inside the well.
                    let mut sx = x + self.d(4.0);
                    for (i, o) in options.iter().enumerate() {
                        let sw = p.text_width(o, size, true) + self.d(28.0);
                        let r = [sx, y + self.d(4.0), sw, h - self.d(8.0)];
                        let on = i == *selected;
                        if on {
                            p.round_rect(r, r[3] / 2.0, ACCENT);
                        } else if self.hovered(r) {
                            p.round_rect(r, r[3] / 2.0, alpha(WHITE, 18));
                        }
                        self.text_c(p, sx + self.d(14.0), r[1], r[3], o, if on { ON_ACCENT } else { SOFT }, size, true);
                        self.hit(r, Hit::Act(Action::SetChoice(id, i)));
                        sx += sw;
                    }
                } else {
                    // Stepper: previous / value / next.
                    let n = options.len().max(1);
                    let bw = self.d(36.0);
                    let cur = options.get(*selected).cloned().unwrap_or_default();
                    for (bx, icon, to) in [(x, Icon::ChevronLeft, (*selected + n - 1) % n), (x + w - bw, Icon::ChevronRight, (*selected + 1) % n)] {
                        let r = [bx + self.d(2.0), y + self.d(2.0), bw - self.d(4.0), h - self.d(4.0)];
                        if self.hovered(r) {
                            p.round_rect(r, r[3] / 2.0, alpha(WHITE, 22));
                        }
                        let isz = self.d(16.0);
                        p.icon(icon, [r[0] + (r[2] - isz) / 2.0, r[1] + (r[3] - isz) / 2.0, isz, isz], SOFT);
                        self.hit(r, Hit::Act(Action::SetChoice(id, to)));
                    }
                    let tw = p.text_width(&cur, size, true);
                    self.text_c(p, x + (w - tw) / 2.0, y, h, &cur, TEXT, size, true);
                }
            }
            SettingKind::Toggle(on) => {
                let h = self.d(28.0);
                self.toggle(p, right - w, cy - h / 2.0, *on, Action::SetToggle(id, !on));
            }
            SettingKind::Button(label) => {
                let h = self.d(CTRL);
                self.button(p, [right - w, cy - h / 2.0, w, h], label, Style::Secondary, None, Action::SettingButton(id));
            }
            SettingKind::Info => {}
        }
    }

    /// Draws the whole screen and rebuilds the hit regions.
    pub fn draw(&mut self, p: &mut dyn Painter) {
        self.hits.clear();
        self.clip = None;
        self.expire_message();
        let (w, h) = (self.w, self.h);
        p.rect([0.0, 0.0, w, h], BG);
        let tabs = self.tabs();
        if !tabs.iter().any(|(t, _, _)| *t == self.tab) {
            self.tab = Tab::Play;
        }
        let sb = (w * 0.2).clamp(self.d(204.0), self.d(240.0));
        let m = self.d(28.0);
        let area = [sb + m, self.d(24.0), (w - sb - 2.0 * m).max(0.0), (h - self.d(48.0)).max(0.0)];
        match self.tab {
            Tab::Play if self.import_showing() => self.draw_import(p, area, sb),
            Tab::Play => self.draw_play(p, area),
            Tab::Versions => self.draw_versions(p, area, sb),
            Tab::Mods => self.draw_mods(p, area, sb),
            Tab::Settings => self.draw_settings(p, area, sb),
        }
        self.clip = None;
        self.draw_sidebar(p, sb, &tabs);
        self.draw_toast(p, area);
    }

    fn expire_message(&mut self) {
        match &self.state.message {
            Some((m, false)) => match &self.message_seen {
                Some((seen, t)) if seen == m => {
                    if t.elapsed() >= MESSAGE_TIME {
                        self.state.message = None;
                        self.message_seen = None;
                    }
                }
                _ => self.message_seen = Some((m.clone(), Instant::now())),
            },
            _ => self.message_seen = None,
        }
    }

    fn draw_sidebar(&mut self, p: &mut dyn Painter, sb: f32, tabs: &[(Tab, &'static str, Icon)]) {
        let h = self.h;
        p.rect([0.0, 0.0, sb, h], SIDEBAR);
        let x = self.d(20.0);
        let top = self.d(24.0);
        let mut tx = x;
        if let Some((iw, ih)) = self.state.icon {
            let s = self.d(40.0);
            p.image(ICON_KEY, [x, top, s, s * ih as f32 / iw.max(1) as f32], [0.0, 0.0, 1.0, 1.0]);
            tx += s + self.d(12.0);
        }
        let room = sb - tx - self.d(14.0);
        let name = self.fit(p, "Frogger 2", room, self.d(19.0), true);
        p.text(tx, top + self.d(1.0), &name, TEXT, self.d(19.0), true);
        let sub = self.fit(p, "Swampy's Revenge", room, self.d(12.0), false);
        p.text(tx, top + self.d(26.0), &sub, DIM, self.d(12.0), false);
        let mut y = top + self.d(72.0);
        for (t, label, icon) in tabs {
            let r = [self.d(12.0), y, sb - self.d(24.0), self.d(46.0)];
            let active = *t == self.tab;
            if active {
                p.round_rect(r, self.d(12.0), alpha(ACCENT, 38));
            } else if self.hovered(r) {
                p.round_rect(r, self.d(12.0), alpha(WHITE, 14));
            }
            let c = if active { ACCENT } else { DIM };
            let isz = self.d(20.0);
            p.icon(*icon, [r[0] + self.d(16.0), r[1] + (r[3] - isz) / 2.0, isz, isz], c);
            self.text_c(p, r[0] + self.d(50.0), r[1], r[3], label, if active { TEXT } else { SOFT }, self.d(15.0), active);
            self.hit(r, Hit::Tab(*t));
            y += self.d(52.0);
        }
        // Bottom: back button (Android), update chip, version.
        let mut by = h - self.d(22.0);
        let v = self.fit(p, &format!("Rust port  \u{b7}  v{}", self.state.app_version), sb - 2.0 * x, self.d(12.0), false);
        p.text(x, by - self.d(14.0), &v, DIM, self.d(12.0), false);
        by -= self.d(28.0);
        if let Some(v) = self.state.update_available.clone() {
            let r = [self.d(12.0), by - self.d(36.0), sb - self.d(24.0), self.d(36.0)];
            let hov = self.hovered(r);
            p.round_rect(r, r[3] / 2.0, alpha(ACCENT, if hov { 70 } else { 46 }));
            let isz = self.d(16.0);
            p.icon(Icon::Download, [r[0] + self.d(14.0), r[1] + (r[3] - isz) / 2.0, isz, isz], ACCENT);
            let s = self.fit(p, &format!("{v} available"), r[2] - self.d(52.0), self.d(13.0), true);
            self.text_c(p, r[0] + self.d(38.0), r[1], r[3], &s, ACCENT, self.d(13.0), true);
            let target = if self.caps.versions { Hit::Tab(Tab::Versions) } else { Hit::Act(Action::OpenUpdates) };
            self.hit(r, target);
            by -= self.d(46.0);
        }
        if let Some(label) = self.caps.close_label.filter(|_| self.state.game.is_some() && self.state.import.running.is_none()) {
            let r = [self.d(12.0), by - self.d(42.0), sb - self.d(24.0), self.d(42.0)];
            self.button(p, r, label, Style::Secondary, Some(Icon::Back), Action::Close);
        }
    }

    /// Title and subtitle of a page at the top of `a`.
    fn page_title(&self, p: &mut dyn Painter, a: [f32; 4], title: &str, sub: &str) {
        let t = self.fit(p, title, a[2], self.d(28.0), true);
        p.text(a[0], a[1], &t, TEXT, self.d(28.0), true);
        let s = self.fit(p, sub, a[2], self.d(13.5), false);
        p.text(a[0], a[1] + self.d(40.0), &s, DIM, self.d(13.5), false);
    }

    /// Height of a page title.
    fn title_h(&self) -> f32 {
        self.d(72.0)
    }

    /// Starts the scrolling part `view`; returns where its content begins.
    fn begin_scroll(&mut self, view: [f32; 4]) -> f32 {
        self.clip = Some(view);
        view[1] - self.scroll
    }

    /// Ends the scrolling part whose content ended at `bottom`: covers what was drawn outside
    /// it (above and below, right of the sidebar) and shows a scroll bar.
    fn end_scroll(&mut self, p: &mut dyn Painter, sb: f32, bottom: f32) {
        let Some(v) = self.clip.take() else { return };
        let content = bottom - (v[1] - self.scroll);
        self.max_scroll = (content - v[3]).max(0.0);
        self.scroll = self.scroll.clamp(0.0, self.max_scroll);
        p.rect([sb, 0.0, self.w - sb, v[1]], BG);
        p.rect([sb, v[1] + v[3], self.w - sb, (self.h - v[1] - v[3]).max(0.0)], BG);
        if self.max_scroll > 0.5 {
            let x = v[0] + v[2] + self.d(10.0);
            let tw = self.d(4.0);
            p.round_rect([x, v[1], tw, v[3]], tw / 2.0, alpha(WHITE, 10));
            let th = (v[3] * v[3] / content).max(self.d(32.0)).min(v[3]);
            let ty = v[1] + (v[3] - th) * (self.scroll / self.max_scroll);
            p.round_rect([x, ty, tw, th], tw / 2.0, alpha(WHITE, 60));
        }
    }

    fn draw_play(&mut self, p: &mut dyn Painter, a: [f32; 4]) {
        self.max_scroll = 0.0;
        let gap = self.d(18.0);
        let ch = self.d(92.0);
        let hero_h = (a[3] - ch - gap).min(a[2] / 1.6).max(self.d(200.0)).min(a[3]);
        let r = [a[0], a[1], a[2], hero_h];
        let radius = self.d(20.0);
        match self.state.hero {
            Some((iw, ih)) => {
                // Cover crop of the picture's top part: the frog and the crocodile, not the
                // picture's own logo below them (ours is drawn instead).
                let (iw, ih) = (iw as f32, ih.max(1) as f32);
                let (ra, top) = (r[2] / r[3], HERO_TOP * ih);
                let uv = if iw / ra <= top {
                    [0.0, 0.0, 1.0, iw / ra / ih]
                } else {
                    let k = top * ra / iw;
                    [(1.0 - k) / 2.0, 0.0, (1.0 + k) / 2.0, HERO_TOP]
                };
                p.image(HERO_KEY, r, uv);
                // Darken the lower part so the text reads on any picture.
                p.gradient_v(r, [0, 0, 0, 20], [0, 0, 0, 120]);
                let gh = r[3] * 0.6;
                p.gradient_v([r[0], r[1] + r[3] - gh, r[2], gh], [0, 0, 0, 0], [0, 0, 0, 170]);
            }
            None => p.gradient_v(r, [32, 70, 40, 255], [16, 30, 24, 255]),
        }
        p.round_mask(r, radius, BG);
        let pad = self.d(32.0);
        let bh = self.d(52.0);
        let by = r[1] + r[3] - pad - bh;
        let full = self.state.game.clone().unwrap_or_default();
        let sub = full.split_once(':').map_or(full.clone(), |(_, s)| s.trim().to_string());
        let info_y = by - self.d(38.0);
        let sub_size = self.d(26.0);
        let sub_y = info_y - self.d(38.0);
        if let Some((lw, lh)) = self.state.logo {
            // The game's own logo, then its subtitle in the logo's yellow.
            let room = sub_y - self.d(6.0) - (r[1] + self.d(20.0));
            let lhgt = (r[3] * 0.36).clamp(self.d(64.0), self.d(150.0)).min(room);
            if lhgt >= self.d(40.0) {
                let lwid = lhgt * lw as f32 / lh.max(1) as f32;
                p.image(LOGO_KEY, [r[0] + pad - self.d(6.0), sub_y - self.d(6.0) - lhgt, lwid, lhgt], [0.0, 0.0, 1.0, 1.0]);
            }
            let s = self.fit(p, &sub, r[2] - 2.0 * pad, sub_size, true);
            p.text(r[0] + pad, sub_y, &s, YELLOW, sub_size, true);
        } else {
            let t = self.fit(p, &full, r[2] - 2.0 * pad, self.d(30.0), true);
            p.text(r[0] + pad, sub_y - self.d(6.0), &t, TEXT, self.d(30.0), true);
        }
        let v = self.fit(p, &format!("Rust port  \u{b7}  v{}", self.state.app_version), r[2] - 2.0 * pad, self.d(14.0), false);
        p.text(r[0] + pad, info_y, &v, SOFT, self.d(14.0), false);
        let mut x = r[0] + pad;
        x = self.auto_button(p, x, by, bh, "PLAY", Style::Primary, Some(Icon::Play), Action::Play) + self.d(12.0);
        if self.state.shortcut == Some(false) && x + self.d(150.0) < r[0] + r[2] - pad {
            self.auto_button(p, x, by + (bh - self.d(44.0)) / 2.0, self.d(44.0), "Add shortcut", Style::Ghost, Some(Icon::Link), Action::CreateShortcut);
        }

        // Cards under the hero; each opens what it is about.
        let top = r[1] + r[3] + gap;
        if top + ch > a[1] + a[3] + 0.5 {
            return;
        }
        let cgap = self.d(16.0);
        let cw = (a[2] - 2.0 * cgap) / 3.0;
        let enabled = self.state.mods.iter().filter(|m| m.enabled).count();
        let (version_line, version_color) = match &self.state.update_available {
            Some(v) => (format!("{v} is available"), ACCENT),
            None => (format!("{} \u{b7} up to date", self.state.app_version), TEXT),
        };
        let version_hit =
            if self.caps.versions { Some(Hit::Tab(Tab::Versions)) } else { self.state.update_available.as_ref().map(|_| Hit::Act(Action::OpenUpdates)) };
        let mods_line = match self.state.mods.len() {
            0 => "None installed".to_string(),
            n => format!("{enabled} of {n} enabled"),
        };
        let cards = [
            ("GAME", Icon::Disc, full.clone(), TEXT, Some(Hit::Tab(Tab::Settings))),
            ("VERSION", Icon::Layers, version_line, version_color, version_hit),
            ("MODS", Icon::Grid, mods_line, TEXT, Some(Hit::Tab(Tab::Mods))),
        ];
        for (i, (cap, icon, text, color, hit)) in cards.into_iter().enumerate() {
            let cr = [a[0] + i as f32 * (cw + cgap), top, cw, ch];
            let hov = hit.is_some() && self.hovered(cr);
            p.round_rect(cr, self.d(CARD_R), if hov { lighten(CARD, 8) } else { CARD });
            let isz = self.d(16.0);
            p.icon(icon, [cr[0] + self.d(PAD), cr[1] + self.d(20.0), isz, isz], DIM);
            p.text(cr[0] + self.d(PAD + 24.0), cr[1] + self.d(20.0), cap, DIM, self.d(11.5), true);
            let t = self.fit(p, &text, cw - self.d(2.0 * PAD), self.d(15.0), false);
            p.text(cr[0] + self.d(PAD), cr[1] + self.d(50.0), &t, color, self.d(15.0), false);
            if let Some(h) = hit {
                self.hit(cr, h);
            }
        }
    }

    fn draw_import(&mut self, p: &mut dyn Painter, a: [f32; 4], sb: f32) {
        let i = self.state.import.clone();
        let again = self.state.game.is_some();
        let (title, sub) = if i.unpacking {
            ("Getting ready", "Unpacking the game that came with this app. This happens once.")
        } else if again {
            ("Import again", "Import your copy again, for example after a new version needs more from the disc.")
        } else {
            ("Set up your game", "Import Frogger 2 from your own copy. Nothing is downloaded: the game stays on this device.")
        };
        let y0 = a[1] + self.title_h() + self.d(8.0);
        let view = [a[0], y0, a[2], a[1] + a[3] - y0];
        let mut y = self.begin_scroll(view);
        let pad = self.d(PAD);
        let inner = a[2] - 2.0 * pad;
        if let Some(run) = &i.running {
            // Progress card: stage, bar, the latest log lines.
            let log_rows = 8usize;
            let ch = self.d(150.0) + log_rows as f32 * self.d(19.0);
            let r = [a[0], y, a[2], ch];
            self.card(p, r, CARD);
            let t = self.fit(p, &run.title, inner, self.d(18.0), true);
            self.text(p, r[0] + pad, r[1] + self.d(22.0), &t, TEXT, self.d(18.0), true);
            let pct = format!("{:.0}%", run.fraction.clamp(0.0, 1.0) * 100.0);
            let pw = p.text_width(&pct, self.d(14.0), true);
            let stage = self.fit(p, &run.stage, inner - pw - self.d(16.0), self.d(14.0), false);
            self.text(p, r[0] + pad, r[1] + self.d(56.0), &stage, SOFT, self.d(14.0), false);
            self.text(p, r[0] + r[2] - pad - pw, r[1] + self.d(56.0), &pct, ACCENT, self.d(14.0), true);
            let tr = [r[0] + pad, r[1] + self.d(84.0), inner, self.d(8.0)];
            if self.shown(tr[1], tr[3]) {
                p.round_rect(tr, tr[3] / 2.0, WELL);
                let fw = (tr[2] * run.fraction.clamp(0.0, 1.0)).max(tr[3]);
                p.round_rect([tr[0], tr[1], fw, tr[3]], tr[3] / 2.0, ACCENT);
            }
            let mut ly = r[1] + self.d(114.0);
            for l in run.log.iter().skip(run.log.len().saturating_sub(log_rows)) {
                let l = self.fit(p, l.trim(), inner, self.d(12.5), false);
                self.text(p, r[0] + pad, ly, &l, DIM, self.d(12.5), false);
                ly += self.d(19.0);
            }
            y += ch;
        } else if let Some(err) = &i.error {
            let lines = wrap_px(err, inner, |t| p.text_width(t, self.d(14.0), false)).len().max(1);
            let ch = self.d(136.0) + lines as f32 * self.d(21.0);
            let r = [a[0], y, a[2], ch];
            self.card(p, r, [52, 30, 32, 255]);
            self.text(p, r[0] + pad, r[1] + self.d(22.0), if i.unpacking { "Unpacking failed" } else { "The import failed" }, DANGER, self.d(18.0), true);
            let ey = self.para(p, r[0] + pad, r[1] + self.d(56.0), inner, err, TEXT, self.d(14.0));
            if !i.unpacking {
                self.auto_button(p, r[0] + pad, ey + self.d(14.0), self.d(BTN), "Try again", Style::Secondary, Some(Icon::Refresh), Action::RetryImport);
            } else {
                self.text(p, r[0] + pad, ey + self.d(14.0), "Free some storage space and start the app again.", DIM, self.d(13.5), false);
            }
            y += ch;
        } else if !i.unpacking {
            // Where the game can come from: discs first, then a file or folder.
            if !i.sources_title.is_empty() || !i.sources.is_empty() {
                let row_h = self.d(64.0);
                let rows = i.sources.len().max(1);
                let ch = self.d(64.0) + rows as f32 * row_h + self.d(8.0);
                let r = [a[0], y, a[2], ch];
                self.card(p, r, CARD);
                self.text(p, r[0] + pad, r[1] + self.d(22.0), &i.sources_title, TEXT, self.d(17.0), true);
                if self.caps.drives {
                    let label = if i.scanning { "Looking..." } else { "Look again" };
                    self.button_left_of(
                        p,
                        r[0] + r[2] - pad,
                        r[1] + self.d(14.0),
                        self.d(CTRL),
                        label,
                        Style::Ghost,
                        Some(Icon::Refresh),
                        Action::RescanDrives,
                    );
                }
                let mut ry = r[1] + self.d(64.0);
                if i.sources.is_empty() {
                    let note = if i.scanning { "Looking for drives..." } else { i.empty_note.as_str() };
                    self.text_c(p, r[0] + pad, ry, row_h, note, DIM, self.d(14.0), false);
                }
                for (n, s) in i.sources.iter().enumerate() {
                    if n > 0 {
                        self.card_line(p, r[0] + pad, ry, inner);
                    }
                    let isz = self.d(28.0);
                    if self.shown(ry, row_h) {
                        p.icon(Icon::Disc, [r[0] + pad, ry + (row_h - isz) / 2.0, isz, isz], if s.highlight { ACCENT } else { DIM });
                    }
                    let bx = if s.ready {
                        let (label, style) = if s.highlight { ("Import", Style::Primary) } else { ("Import", Style::Secondary) };
                        self.button_left_of(
                            p,
                            r[0] + r[2] - pad,
                            ry + (row_h - self.d(BTN)) / 2.0,
                            self.d(BTN),
                            label,
                            style,
                            Some(Icon::Download),
                            Action::ImportSource(n),
                        )
                    } else {
                        r[0] + r[2] - pad
                    };
                    let tx = r[0] + pad + isz + self.d(16.0);
                    let mut room = bx - tx - self.d(16.0);
                    let mut lx = tx;
                    let label = self.fit(p, &s.label, room, self.d(15.0), true);
                    self.text(p, lx, ry + self.d(12.0), &label, TEXT, self.d(15.0), true);
                    if s.highlight {
                        lx += p.text_width(&label, self.d(15.0), true) + self.d(10.0);
                        if lx + self.chip_w(p, "FROGGER 2") < bx - self.d(12.0) {
                            self.chip(p, lx, ry + self.d(11.0), "FROGGER 2", alpha(ACCENT, 60), ACCENT);
                        }
                        room = bx - tx - self.d(16.0);
                    }
                    let detail = self.fit(p, &s.detail, room, self.d(12.5), false);
                    self.text(p, tx, ry + self.d(36.0), &detail, DIM, self.d(12.5), false);
                    ry += row_h;
                }
                y += ch + self.d(16.0);
            }
            if self.caps.import {
                let help_lines: usize =
                    i.help.iter().map(|h| wrap_px(h, inner, |t| p.text_width(t, self.d(13.0), false)).len()).sum::<usize>() + i.note.iter().count();
                let intro = "A disc image (.cue with its .bin, .iso, .img, .ccd, .mds, .nrg, .toc or .zip), a copy of the disc, or an installed game.";
                let intro_lines = wrap_px(intro, inner, |t| p.text_width(t, self.d(13.5), false)).len();
                let drop = self.caps.drop_files as usize;
                let ch = self.d(64.0) + intro_lines as f32 * self.d(20.25) + self.d(64.0) + (help_lines + drop) as f32 * self.d(19.5) + self.d(14.0);
                let r = [a[0], y, a[2], ch];
                self.card(p, r, CARD);
                self.text(p, r[0] + pad, r[1] + self.d(22.0), "Disc image or folder", TEXT, self.d(17.0), true);
                let mut ty = self.para(p, r[0] + pad, r[1] + self.d(54.0), inner, intro, SOFT, self.d(13.5));
                ty += self.d(10.0);
                let primary = !i.sources.iter().any(|s| s.highlight && s.ready);
                let mut x = self.auto_button(
                    p,
                    r[0] + pad,
                    ty,
                    self.d(BTN),
                    "Choose disc image",
                    if primary { Style::Primary } else { Style::Secondary },
                    Some(Icon::File),
                    Action::PickImage,
                );
                if self.caps.pick_folder {
                    x += self.d(10.0);
                    self.auto_button(p, x, ty, self.d(BTN), "Choose folder", Style::Secondary, Some(Icon::Folder), Action::PickFolder);
                }
                ty += self.d(BTN) + self.d(14.0);
                if let Some(n) = &i.note {
                    let n = self.fit(p, n, inner, self.d(13.0), true);
                    self.text(p, r[0] + pad, ty, &n, YELLOW, self.d(13.0), true);
                    ty += self.d(19.5);
                }
                if self.caps.drop_files {
                    self.text(p, r[0] + pad, ty, "You can also drop a disc image or folder onto this window.", DIM, self.d(13.0), false);
                    ty += self.d(19.5);
                }
                for h in &i.help {
                    ty = self.para(p, r[0] + pad, ty, inner, h, DIM, self.d(13.0));
                }
                y += ch + self.d(16.0);
            }
            y = self.para(
                p,
                a[0] + self.d(4.0),
                y,
                a[2] - self.d(8.0),
                "The game is read from your copy on this device; the music is ripped from the disc's audio tracks. Nothing from the game is downloaded or shipped with this program.",
                DIM,
                self.d(12.5),
            );
        }
        self.end_scroll(p, sb, y);
        self.page_title(p, a, title, sub);
        if again && i.running.is_none() && !i.unpacking {
            self.button_left_of(p, a[0] + a[2], a[1] + self.d(6.0), self.d(BTN), "Cancel", Style::Ghost, None, Action::CancelImport);
        }
    }

    /// A divider line in a card.
    fn card_line(&self, p: &mut dyn Painter, x: f32, y: f32, w: f32) {
        if self.shown(y, 1.0) {
            p.rect([x, y, w, self.d(1.0).max(1.0)], LINE);
        }
    }

    /// The header row of a list page: a button and a note beside it.
    fn toolbar_y(&self, a: [f32; 4]) -> f32 {
        a[1] + self.title_h() + self.d(4.0)
    }

    fn draw_versions(&mut self, p: &mut dyn Painter, a: [f32; 4], sb: f32) {
        let tb = self.toolbar_y(a);
        let bh = self.d(BTN);
        let y0 = tb + bh + self.d(18.0);
        let view = [a[0], y0, a[2], a[1] + a[3] - y0];
        let mut y = self.begin_scroll(view);
        let rows = self.state.versions.clone();
        let rh = self.d(68.0);
        for v in &rows {
            let r = [a[0], y, a[2], rh];
            if self.shown(r[1], r[3]) {
                self.card(p, r, if v.selected { [33, 48, 38, 255] } else { CARD });
                let bh = self.d(BTN);
                let by = r[1] + (r[3] - bh) / 2.0;
                let mut bx = r[0] + r[2] - self.d(PAD);
                if v.selected {
                    let w = self.chip_w(p, "IN USE");
                    bx -= w;
                    self.chip(p, bx, r[1] + (r[3] - self.d(22.0)) / 2.0, "IN USE", ACCENT, ON_ACCENT);
                } else if v.installed {
                    if !v.tag.is_empty() {
                        bx = self.button_left_of(p, bx, by, bh, "Remove", Style::Danger, None, Action::Remove(v.tag.clone())) - self.d(10.0);
                    }
                    bx = self.button_left_of(p, bx, by, bh, "Use", Style::Secondary, Some(Icon::Check), Action::Use(v.tag.clone()));
                } else if v.available {
                    bx = self.button_left_of(p, bx, by, bh, "Install", Style::Primary, Some(Icon::Download), Action::Install(v.tag.clone()));
                } else {
                    let w = self.chip_w(p, "NO BUILD FOR THIS SYSTEM");
                    bx -= w;
                    self.chip(p, bx, r[1] + (r[3] - self.d(22.0)) / 2.0, "NO BUILD FOR THIS SYSTEM", CARD2, DIM);
                }
                let avail = (bx - r[0] - self.d(40.0)).max(self.d(60.0));
                let label = self.fit(p, &v.label, avail, self.d(16.0), true);
                self.text(p, r[0] + self.d(PAD), r[1] + self.d(13.0), &label, TEXT, self.d(16.0), true);
                let detail = self.fit(p, &v.detail, avail, self.d(12.5), false);
                self.text(p, r[0] + self.d(PAD), r[1] + self.d(38.0), &detail, DIM, self.d(12.5), false);
            }
            y += rh + self.d(10.0);
        }
        if rows.is_empty() {
            self.text(p, a[0], y + self.d(8.0), "Nothing to show yet.", DIM, self.d(14.0), false);
            y += self.d(30.0);
        }
        self.end_scroll(p, sb, y);
        self.page_title(p, a, "Versions", "Releases from GitHub, installed side by side. Your game, saves and mods are shared by all of them.");
        let label = if self.state.versions_note.starts_with("Checking") { "Checking..." } else { "Check for updates" };
        let x = self.auto_button(p, a[0], tb, bh, label, Style::Secondary, Some(Icon::Refresh), Action::RefreshReleases);
        let note = self.fit(p, &self.state.versions_note.clone(), a[0] + a[2] - x - self.d(24.0), self.d(13.0), false);
        self.text_c(p, x + self.d(14.0), tb, bh, &note, DIM, self.d(13.0), false);
    }

    fn draw_mods(&mut self, p: &mut dyn Painter, a: [f32; 4], sb: f32) {
        let tb = self.toolbar_y(a);
        let bh = self.d(BTN);
        let y0 = if self.caps.open_folders { tb + bh + self.d(18.0) } else { tb + self.d(4.0) };
        let view = [a[0], y0, a[2], a[1] + a[3] - y0];
        let mut y = self.begin_scroll(view);
        let rows = self.state.mods.clone();
        if rows.is_empty() {
            let pad = self.d(PAD);
            let text = "No mods installed yet. Put a mod's folder (with its mod.toml) into the mods folder and it shows up here.";
            let lines = wrap_px(text, a[2] - 2.0 * pad, |s| p.text_width(s, self.d(14.0), false)).len();
            let r = [a[0], y, a[2], self.d(44.0) + lines as f32 * self.d(21.0)];
            self.card(p, r, CARD);
            self.para(p, r[0] + pad, r[1] + self.d(22.0), a[2] - 2.0 * pad, text, SOFT, self.d(14.0));
            y += r[3];
        }
        let rh = self.d(68.0);
        for m in &rows {
            let r = [a[0], y, a[2], rh];
            if self.shown(r[1], r[3]) {
                self.card(p, r, CARD);
                let avail = r[2] - self.d(110.0);
                let name = self.fit(p, &m.name, avail, self.d(16.0), true);
                self.text(p, r[0] + self.d(PAD), r[1] + self.d(13.0), &name, TEXT, self.d(16.0), true);
                let sub = if m.version.is_empty() { m.id.clone() } else { format!("{}  \u{b7}  version {}", m.id, m.version) };
                let sub = self.fit(p, &sub, avail, self.d(12.5), false);
                self.text(p, r[0] + self.d(PAD), r[1] + self.d(38.0), &sub, DIM, self.d(12.5), false);
                self.toggle(p, r[0] + r[2] - self.d(PAD) - self.d(48.0), r[1] + (r[3] - self.d(28.0)) / 2.0, m.enabled, Action::ToggleMod(m.id.clone()));
            }
            y += rh + self.d(10.0);
        }
        self.end_scroll(p, sb, y);
        let dir = self.state.mods_dir.clone();
        self.page_title(p, a, "Mods", &format!("Turned on mods load in the order you turn them on. Folder: {dir}"));
        if self.caps.open_folders {
            self.auto_button(p, a[0], tb, bh, "Open mods folder", Style::Secondary, Some(Icon::Folder), Action::OpenModsFolder);
        }
    }

    fn draw_settings(&mut self, p: &mut dyn Painter, a: [f32; 4], sb: f32) {
        let y0 = a[1] + self.title_h() + self.d(8.0);
        let view = [a[0], y0, a[2], a[1] + a[3] - y0];
        let mut y = self.begin_scroll(view);
        let pad = self.d(PAD);
        let groups = self.state.settings.clone();
        for g in &groups {
            // Row heights first (the card is drawn under them).
            let heights: Vec<f32> = g.rows.iter().map(|r| if r.detail.is_empty() { self.d(56.0) } else { self.d(68.0) }).collect();
            let ch = self.d(56.0) + heights.iter().sum::<f32>() + self.d(6.0);
            let r = [a[0], y, a[2], ch];
            self.card(p, r, CARD);
            self.text(p, r[0] + pad, r[1] + self.d(20.0), &g.title, TEXT, self.d(17.0), true);
            let mut ry = r[1] + self.d(56.0);
            for (n, (row, rh)) in g.rows.iter().zip(&heights).enumerate() {
                if n > 0 {
                    self.card_line(p, r[0] + pad, ry, r[2] - 2.0 * pad);
                }
                let cw = self.control_w(p, &row.kind);
                let right = r[0] + r[2] - pad;
                let room = (right - cw - self.d(20.0) - (r[0] + pad)).max(self.d(80.0));
                let label = self.fit(p, &row.label, room, self.d(15.0), false);
                if row.detail.is_empty() {
                    self.text_c(p, r[0] + pad, ry, *rh, &label, TEXT, self.d(15.0), false);
                } else {
                    self.text(p, r[0] + pad, ry + self.d(13.0), &label, TEXT, self.d(15.0), false);
                    let detail = self.fit(p, &row.detail, room, self.d(12.5), false);
                    self.text(p, r[0] + pad, ry + self.d(37.0), &detail, DIM, self.d(12.5), false);
                }
                self.control(p, right, ry + rh / 2.0, row.id, &row.kind);
                ry += rh;
            }
            y += ch + self.d(16.0);
        }
        y = self.para(
            p,
            a[0] + self.d(4.0),
            y,
            a[2] - self.d(8.0),
            "Everything else (controls, graphics details, gameplay options) is in the game's own Settings screen: F10 or the pause menu.",
            DIM,
            self.d(12.5),
        );
        self.end_scroll(p, sb, y);
        self.page_title(p, a, "Settings", "Changes are saved right away and apply the next time the game starts.");
    }

    /// The busy bar or the last message, floating at the bottom right; a press dismisses it.
    fn draw_toast(&mut self, p: &mut dyn Painter, a: [f32; 4]) {
        let w = a[2].min(self.d(480.0));
        let x = a[0] + a[2] - w;
        if let Some((what, f)) = self.state.busy.clone() {
            let h = self.d(64.0);
            let r = [x, self.h - self.d(20.0) - h, w, h];
            p.round_rect([r[0] - 1.0, r[1] - 1.0, r[2] + 2.0, r[3] + 2.0], self.d(CARD_R) + 1.0, LINE);
            p.round_rect(r, self.d(CARD_R), CARD2);
            let t = self.fit(p, &what, r[2] - self.d(40.0), self.d(13.5), false);
            p.text(r[0] + self.d(20.0), r[1] + self.d(12.0), &t, TEXT, self.d(13.5), false);
            let tr = [r[0] + self.d(20.0), r[1] + r[3] - self.d(20.0), r[2] - self.d(40.0), self.d(6.0)];
            p.round_rect(tr, self.d(3.0), WELL);
            let fw = (tr[2] * f.clamp(0.0, 1.0)).max(self.d(6.0));
            p.round_rect([tr[0], tr[1], fw, tr[3]], self.d(3.0), ACCENT);
        } else if let Some((msg, bad)) = self.state.message.clone() {
            let size = self.d(13.5);
            let inner = w - self.d(64.0);
            let lines: Vec<String> = wrap_px(&msg, inner, |t| p.text_width(t, size, false)).into_iter().take(3).collect();
            let h = self.d(30.0) + lines.len().max(1) as f32 * size * 1.5;
            let r = [x, self.h - self.d(20.0) - h, w, h];
            p.round_rect([r[0] - 1.0, r[1] - 1.0, r[2] + 2.0, r[3] + 2.0], self.d(CARD_R) + 1.0, if bad { alpha(DANGER, 120) } else { LINE });
            p.round_rect(r, self.d(CARD_R), CARD2);
            let dot = self.d(10.0);
            p.round_rect([r[0] + self.d(20.0), r[1] + self.d(15.0) + (size * 1.5 - dot) / 2.0, dot, dot], dot / 2.0, if bad { DANGER } else { ACCENT });
            let mut ty = r[1] + self.d(15.0);
            for l in &lines {
                let l = self.fit(p, l, inner, size, false);
                p.text(r[0] + self.d(42.0), ty, &l, if bad { lighten(DANGER, 30) } else { TEXT }, size, false);
                ty += size * 1.5;
            }
            self.hit(r, Hit::Dismiss);
        }
    }
}

/// Greedy word wrap to `width` pixels (a word wider than a line gets a line of its own).
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

    fn desktop_caps() -> Caps {
        Caps { versions: true, import: true, drives: true, pick_folder: true, drop_files: true, open_folders: true, updates_page: false, close_label: None }
    }

    fn settings() -> Vec<SettingGroup> {
        let row = |id, label: &str, detail: &str, kind| SettingRow { id, label: label.into(), detail: detail.into(), kind };
        vec![
            SettingGroup {
                title: "Game".into(),
                rows: vec![
                    row("mode", "Display", "", SettingKind::Choice { options: vec!["Windowed".into(), "Borderless".into(), "Fullscreen".into()], selected: 1 }),
                    row(
                        "res",
                        "Resolution",
                        "4:3 like the original",
                        SettingKind::Choice { options: (0..9).map(|i| format!("{} x {}", 640 + i * 160, 480 + i * 120)).collect(), selected: 2 },
                    ),
                    row("vsync", "VSync", "", SettingKind::Toggle(true)),
                ],
            },
            SettingGroup {
                title: "Starting".into(),
                rows: vec![row(
                    "start",
                    "On start, open",
                    "What frogger2 opens",
                    SettingKind::Choice { options: vec!["Game".into(), "Launcher".into()], selected: 0 },
                )],
            },
            SettingGroup {
                title: "Folders".into(),
                rows: vec![
                    row("open_data", "Game data", "/home/x/frogger2-rs/data", SettingKind::Button("Open".into())),
                    row("cfg", "Settings file", "/home/x/frogger2-rs/config.toml", SettingKind::Info),
                ],
            },
        ]
    }

    fn desktop() -> Launcher {
        let mut l = Launcher::new(desktop_caps(), 1280.0, 720.0, 1.0);
        l.state.game = Some("Frogger 2: Swampy's Revenge (USA)".into());
        l.state.app_version = "0.1.0".into();
        l.state.data_dir = "/home/x/frogger2-rs/data".into();
        l.state.shortcut = Some(false);
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
        l.state.settings = settings();
        l.state.import = ImportView {
            sources_title: "Disc drives".into(),
            sources: vec![
                SourceRow { label: "D:".into(), detail: "FROGGER2".into(), highlight: true, ready: true },
                SourceRow { label: "E:".into(), detail: "No disc".into(), highlight: false, ready: false },
            ],
            empty_note: "No CD or DVD drives found.".into(),
            ..Default::default()
        };
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

    fn drawn(l: &mut Launcher) -> Rec {
        let mut r = Rec::default();
        l.draw(&mut r);
        r
    }

    #[test]
    fn everything_fits_on_screen_sizes() {
        for (w, h, dp) in
            [(1280.0, 720.0, 1.0), (1920.0, 1080.0, 2.0), (800.0, 480.0, 1.0), (2400.0, 1080.0, 2.75), (1000.0, 600.0, 1.25), (1040.0, 660.0, 1.0)]
        {
            for game in [true, false] {
                let mut l = desktop();
                l.resize(w, h, dp);
                if !game {
                    l.state.game = None;
                }
                l.state.busy = Some(("Downloading".into(), 0.5));
                l.state.logo = Some((256, 131));
                l.state.hero = Some((640, 480));
                l.state.icon = Some((64, 64));
                l.state.update_available = Some("v0.2.0".into());
                for (t, _, _) in l.tabs() {
                    l.tab = t;
                    let r = drawn(&mut l);
                    // Content may run under the masks at the bottom of a scrolling page; it
                    // must start on the screen and never leave it sideways.
                    for [x, y, rw, _] in &r.rects {
                        assert!(*x >= -0.5 && *y >= -0.5 && x + rw <= w + 0.5 && *y <= h + 0.5, "{t:?} {w}x{h}@{dp}: rect {x},{y} {rw}");
                    }
                    for (x, y, s, size) in &r.texts {
                        let tw = s.chars().count() as f32 * size * 0.52;
                        assert!(*x >= -0.5 && *y >= -0.5 && x + tw <= w + 0.5 && *y <= h + 0.5, "{t:?} {w}x{h}@{dp}: text {s:?} at {x},{y} ({tw} wide)");
                    }
                    assert!(!r.texts.is_empty());
                }
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
        assert_eq!(click_label(&mut l, "Add shortcut"), Action::CreateShortcut);
    }

    #[test]
    fn import_page_without_a_game() {
        let mut l = desktop();
        l.state.game = None;
        assert!(l.import_showing());
        assert_eq!(click_label(&mut l, "Import"), Action::ImportSource(0));
        assert_eq!(click_label(&mut l, "Choose disc image"), Action::PickImage);
        assert_eq!(click_label(&mut l, "Choose folder"), Action::PickFolder);
        assert_eq!(click_label(&mut l, "Look again"), Action::RescanDrives);
        let r = drawn(&mut l);
        assert!(r.texts.iter().any(|t| t.2 == "FROGGER 2"), "the Frogger disc is marked");
        assert!(!r.texts.iter().any(|t| t.2 == "Cancel"), "nothing to go back to");
        // Running: progress, no choices.
        l.state.import.running =
            Some(Progress { title: "Importing D:".into(), stage: "read CD audio".into(), fraction: 0.42, log: vec!["a".into(), "b".into()] });
        let r = drawn(&mut l);
        assert!(r.texts.iter().any(|t| t.2 == "42%"));
        assert!(!r.texts.iter().any(|t| t.2 == "Choose disc image"));
        // Failed: try again.
        l.state.import.running = None;
        l.state.import.error = Some("FROGGER2/DATA1.CAB not found on the disc".into());
        assert_eq!(click_label(&mut l, "Try again"), Action::RetryImport);
    }

    #[test]
    fn import_again_and_cancel() {
        let mut l = desktop();
        assert!(!l.import_showing());
        l.show_tab(Tab::Settings);
        // Any page can open the import page; Cancel goes back.
        assert_eq!(l.press(-1.0, -1.0), Action::None);
        let mut r = Rec::default();
        l.draw(&mut r);
        l.hits.push(([0.0, 0.0, 1.0, 1.0], Hit::Act(Action::Import)));
        assert_eq!(l.press(0.5, 0.5), Action::Import);
        assert_eq!(l.tab, Tab::Play);
        assert!(l.import_showing());
        assert_eq!(click_label(&mut l, "Cancel"), Action::CancelImport);
        assert!(!l.import_showing());
    }

    #[test]
    fn settings_choices_toggles_and_buttons() {
        let mut l = desktop();
        click_label(&mut l, "Settings");
        assert_eq!(l.tab, Tab::Settings);
        // Segmented: every option is a pill of its own.
        assert_eq!(click_label(&mut l, "Fullscreen"), Action::SetChoice("mode", 2));
        assert_eq!(click_label(&mut l, "Launcher"), Action::SetChoice("start", 1));
        assert_eq!(click_label(&mut l, "Open"), Action::SettingButton("open_data"));
        // Many options: a stepper showing the current one.
        let r = drawn(&mut l);
        assert!(r.texts.iter().any(|t| t.2 == "960 x 720"));
        let next = l.hits.iter().filter(|(_, h)| matches!(h, Hit::Act(Action::SetChoice("res", _)))).map(|(r, h)| (*r, h.clone())).collect::<Vec<_>>();
        assert_eq!(next.len(), 2);
        let acts: Vec<Action> = next.iter().map(|(r, _)| l.press(r[0] + 1.0, r[1] + 1.0)).collect();
        assert_eq!(acts, [Action::SetChoice("res", 1), Action::SetChoice("res", 3)]);
        let tog = l.hits.iter().find(|(_, h)| matches!(h, Hit::Act(Action::SetToggle("vsync", _)))).unwrap().0;
        assert_eq!(l.press(tog[0] + tog[2] / 2.0, tog[1] + tog[3] / 2.0), Action::SetToggle("vsync", false));
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
        assert_eq!(click_label(&mut l, "Check for updates"), Action::RefreshReleases);
        assert_eq!(click_label(&mut l, "Install"), Action::Install("v0.2.0".into()));
        assert_eq!(click_label(&mut l, "Use"), Action::Use("v0.3.0".into()));
        assert_eq!(click_label(&mut l, "Remove"), Action::Remove("v0.3.0".into()));
        click_label(&mut l, "Mods");
        assert_eq!(click_label(&mut l, "Open mods folder"), Action::OpenModsFolder);
        let _ = drawn(&mut l);
        let hit = l.hits.iter().find(|(_, h)| matches!(h, Hit::Act(Action::ToggleMod(_)))).unwrap().0;
        assert_eq!(l.press(hit[0] + hit[2] / 2.0, hit[1] + hit[3] / 2.0), Action::ToggleMod("hello-mod".into()));
    }

    #[test]
    fn android_style_caps_hide_versions_and_offer_close() {
        let caps = Caps { import: true, updates_page: true, close_label: Some("Back to the game"), ..Default::default() };
        let mut l = Launcher::new(caps, 1920.0, 1080.0, 2.0);
        assert!(!l.tabs().iter().any(|(t, _, _)| *t == Tab::Versions));
        // No game: nothing to go back to.
        assert!(!drawn(&mut l).texts.iter().any(|t| t.2 == "Back to the game"));
        let r = drawn(&mut l);
        assert!(!r.texts.iter().any(|t| t.2 == "Choose folder" || t.2 == "Look again"));
        l.state.game = Some("G".into());
        assert_eq!(click_label(&mut l, "Back to the game"), Action::Close);
    }

    #[test]
    fn update_chip_opens_versions_or_the_releases_page() {
        let mut l = desktop();
        l.state.update_available = Some("v0.2.0".into());
        assert_eq!(click_label(&mut l, "v0.2.0 available"), Action::None);
        assert_eq!(l.tab, Tab::Versions);
        let mut a = Launcher::new(Caps { updates_page: true, ..Default::default() }, 1280.0, 720.0, 1.0);
        a.state.game = Some("G".into());
        a.state.update_available = Some("v0.2.0".into());
        assert_eq!(click_label(&mut a, "v0.2.0 available"), Action::OpenUpdates);
    }

    #[test]
    fn long_pages_scroll_smoothly_and_hide_what_is_outside() {
        let mut l = desktop();
        l.resize(1280.0, 480.0, 1.0);
        click_label(&mut l, "Versions");
        let r = drawn(&mut l);
        assert!(!r.texts.iter().any(|t| t.2 == "v0.11.0"));
        assert!(l.max_scroll > 0.0);
        l.scroll_by(10_000.0);
        assert_eq!(l.scroll, l.max_scroll);
        let r = drawn(&mut l);
        assert!(r.texts.iter().any(|t| t.2 == "v0.11.0"), "scrolled to the end");
        assert!(!r.texts.iter().any(|t| t.2 == "v0.0.0"));
        // Rows scrolled under the header cannot be pressed through it.
        assert!(l.hits.iter().all(|(r, _)| r[1] >= 0.0));
        let (title_x, title_y) = r.texts.iter().find(|t| t.2 == "Versions" && t.3 > 20.0).map(|t| (t.0, t.1)).unwrap();
        assert_eq!(l.press(title_x + 4.0, title_y + 4.0), Action::None);
        // A new tab starts at the top.
        click_label(&mut l, "Mods");
        assert_eq!(l.scroll, 0.0);
    }

    #[test]
    fn messages_dismiss_on_press() {
        let mut l = desktop();
        l.state.message = Some(("Installed v0.1.5".into(), false));
        assert_eq!(click_label(&mut l, "Installed v0.1.5"), Action::None);
        assert!(l.state.message.is_none());
    }

    #[test]
    fn wraps() {
        assert_eq!(wrap_px("aaaa bbbb cccc", 9.0, |s| s.len() as f32), ["aaaa bbbb", "cccc"]);
    }
}
