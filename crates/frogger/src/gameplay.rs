//! P6-1 gameplay QoL on the host side (`docs/enhancements.md`): the `[gameplay]` config
//! mapped to the simulation's [`GameOptions`], save slots, the speedrun timer with its
//! splits file, and the "Gameplay" / "Level select" tabs of the F10 settings screen.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use f2_engine::{FontId, Renderer};
use f2_game::define::mode;
use f2_game::enhance::{GameOptions, GameProfile, ENEMY_SPEED_MIN};
use f2_game::layout::worldid;
use f2_game::story::{NUM_STORY_LEVELS, STORY_SEQUENCE};
use f2_game::Game;
use f2_platform::{dik, Gameplay, Profile};

/// Number of save slots.
pub const NUM_SLOTS: u8 = 4;

/// The simulation options for `profile` and the `[gameplay]` config.
pub fn game_options(profile: Profile, gp: &Gameplay) -> GameOptions {
    match profile {
        Profile::Authentic => GameOptions::AUTHENTIC,
        Profile::Enhanced => GameOptions {
            profile: GameProfile::Enhanced,
            infinite_lives: gp.infinite_lives,
            invulnerable: gp.invulnerable,
            enemy_speed: gp.enemy_speed.clamp(ENEMY_SPEED_MIN, 100),
            fix_tube_softlock: gp.fix_tube_softlock,
            settings_menu: false,
        },
    }
}

/// `savegame.dat` of `slot` given slot 1's file: slot N lives in `slotN/` next to it.
/// The file format is unchanged (`MakeSaveGameBlock`).
pub fn slot_path(slot1: &Path, slot: u8) -> PathBuf {
    if slot <= 1 {
        return slot1.to_path_buf();
    }
    let dir = slot1.parent().unwrap_or(Path::new("."));
    let name = slot1.file_name().map_or_else(|| "savegame.dat".into(), |n| n.to_os_string());
    dir.join(format!("slot{slot}")).join(name)
}

/// `m:ss.cc` of a frame count (60 Hz).
pub fn fmt_frames(frames: u64) -> String {
    let cs = frames * 100 / 60;
    format!("{}:{:02}.{:02}", cs / 6000, (cs / 100) % 60, cs % 100)
}

fn fmt_dur(d: Duration) -> String {
    let cs = d.as_millis() as u64 / 10;
    format!("{}:{:02}.{:02}", cs / 6000, (cs / 100) % 60, cs % 100)
}

/// One finished level.
#[derive(Clone, Debug, PartialEq)]
pub struct Split {
    pub world: u32,
    pub level: u32,
    /// Level in-game time (frames).
    pub igt: u64,
    /// Run in-game time at the split (frames).
    pub total_igt: u64,
    /// Real time since the run started.
    pub rta: Duration,
    /// Previous best IGT of this level (from the splits file), if any.
    pub best: Option<u64>,
}

/// Speedrun timer. In-game time counts the 60 Hz ticks spent in `INGAME_MODE` (the
/// `actFrameCount` advance while playing: pause, level-complete and loading are excluded);
/// a level's split is taken when `LEVELCOMPLETE_MODE` starts (`levelTime = actFrameCount`).
/// Real time runs from the first level of the run until the last split. Returning to the
/// frontend ends the run.
#[derive(Debug, Default)]
pub struct Speedrun {
    pub level: Option<(u32, u32)>,
    pub level_igt: u64,
    pub total_igt: u64,
    pub run_start: Option<Instant>,
    pub rta_end: Option<Duration>,
    pub last: Option<Split>,
    prev_mode: u32,
    /// Best IGT per level from the splits file.
    pub best: HashMap<(u32, u32), u64>,
    /// `splits.csv` (None: not written, e.g. headless).
    pub file: Option<PathBuf>,
}

impl Speedrun {
    pub fn new(file: Option<PathBuf>) -> Speedrun {
        let mut s = Speedrun { file, ..Default::default() };
        if let Some(text) = s.file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()) {
            for line in text.lines().skip(1) {
                let v: Vec<&str> = line.split(',').collect();
                if let (Some(w), Some(l), Some(f)) = (v.get(1), v.get(2), v.get(3)) {
                    if let (Ok(w), Ok(l), Ok(f)) = (w.parse(), l.parse(), f.parse::<u64>()) {
                        let b = s.best.entry((w, l)).or_insert(f);
                        *b = (*b).min(f);
                    }
                }
            }
        }
        s
    }

    /// After every simulation tick. Returns the split taken this tick, if any.
    pub fn on_tick(&mut self, mode_now: u32, world: u32, level: u32, now: Instant) -> Option<Split> {
        let prev = std::mem::replace(&mut self.prev_mode, mode_now);
        let mut split = None;
        match mode_now {
            mode::INGAME_MODE if world != worldid::FRONTEND => {
                if self.level != Some((world, level)) {
                    self.level = Some((world, level));
                    self.level_igt = 0;
                }
                if self.run_start.is_none() || self.rta_end.is_some() {
                    if self.rta_end.is_some() {
                        // A new run after a finished one.
                        self.total_igt = 0;
                        self.last = None;
                    }
                    self.run_start = Some(now);
                    self.rta_end = None;
                }
                self.level_igt += 1;
                self.total_igt += 1;
            }
            mode::LEVELCOMPLETE_MODE if prev != mode::LEVELCOMPLETE_MODE => {
                if let (Some((w, l)), Some(start)) = (self.level.take(), self.run_start) {
                    let s =
                        Split { world: w, level: l, igt: self.level_igt, total_igt: self.total_igt, rta: now - start, best: self.best.get(&(w, l)).copied() };
                    let b = self.best.entry((w, l)).or_insert(s.igt);
                    *b = (*b).min(s.igt);
                    self.write_split(&s);
                    self.last = Some(s.clone());
                    split = Some(s);
                }
            }
            mode::FRONTEND_MODE => {
                if self.run_start.is_some() && self.rta_end.is_none() {
                    self.rta_end = self.run_start.map(|s| now - s);
                }
                self.level = None;
            }
            _ => {}
        }
        split
    }

    fn write_split(&self, s: &Split) {
        let Some(path) = &self.file else { return };
        let new = !path.exists();
        let res = (|| -> std::io::Result<()> {
            if let Some(d) = path.parent() {
                std::fs::create_dir_all(d)?;
            }
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
            if new {
                writeln!(f, "unix_time,world,level,level_igt_frames,level_igt,total_igt,rta")?;
            }
            let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            writeln!(f, "{t},{},{},{},{},{},{}", s.world, s.level, s.igt, fmt_frames(s.igt), fmt_frames(s.total_igt), fmt_dur(s.rta))
        })();
        if let Err(e) = res {
            eprintln!("frogger: splits {}: {e}", path.display());
        }
    }

    /// HUD lines (top right).
    pub fn lines(&self, now: Instant) -> Vec<String> {
        let rta = match (self.run_start, self.rta_end) {
            (_, Some(d)) => d,
            (Some(s), None) => now - s,
            _ => Duration::ZERO,
        };
        let mut v = vec![format!("IGT {}", fmt_frames(self.total_igt)), format!("RTA {}", fmt_dur(rta))];
        if let Some((w, l)) = self.level {
            v.push(format!("{w}-{l} {}", fmt_frames(self.level_igt)));
        }
        if let Some(s) = &self.last {
            let delta = match s.best {
                Some(b) if s.igt <= b => format!(" (-{})", fmt_frames(b - s.igt)),
                Some(b) => format!(" (+{})", fmt_frames(s.igt - b)),
                None => String::new(),
            };
            v.push(format!("split {}-{} {}{delta}", s.world, s.level, fmt_frames(s.igt)));
        }
        v
    }
}

// ---------------------------------------------------------------- settings tabs

/// Tabs of the F10 settings screen (Tab cycles).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Controls,
    Gameplay,
    Levels,
}

impl Tab {
    pub fn next(self) -> Tab {
        match self {
            Tab::Controls => Tab::Gameplay,
            Tab::Gameplay => Tab::Levels,
            Tab::Levels => Tab::Controls,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Tab::Controls => "CONTROLS",
            Tab::Gameplay => "GAMEPLAY",
            Tab::Levels => "LEVEL SELECT",
        }
    }
}

/// What the host does after a key in the gameplay / level tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GAct {
    None,
    /// Options changed: push them to the game and save the config.
    Changed,
    /// Restart at the title with the selected save slot.
    LoadSlot,
    SaveNow,
    Start(LevelChoice),
    Close,
}

/// Rows of the gameplay tab.
const ROWS: [&str; 11] = [
    "Profile",
    "Infinite lives",
    "Invulnerable",
    "Enemy speed",
    "Any key skips FMVs",
    "Skip intro logos",
    "Speedrun timer",
    "Auto-save",
    "Fix tube softlock",
    "Save slot",
    "Save now",
];

#[derive(Default)]
pub struct GameplayUi {
    pub sel: usize,
    pub msg: String,
}

fn on_off(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

impl GameplayUi {
    fn value(&self, row: usize, profile: Profile, gp: &Gameplay) -> String {
        match row {
            0 => profile.name().into(),
            1 => on_off(gp.infinite_lives).into(),
            2 => on_off(gp.invulnerable).into(),
            3 => format!("{}%", gp.enemy_speed),
            4 => on_off(gp.skip_fmv_any_key).into(),
            5 => on_off(gp.skip_intro_logos).into(),
            6 => on_off(gp.speedrun_timer).into(),
            7 => on_off(gp.auto_save).into(),
            8 => on_off(gp.fix_tube_softlock).into(),
            9 => format!("{}  (Enter: load)", gp.save_slot),
            _ => "Enter".into(),
        }
    }

    /// A key went down.
    pub fn key(&mut self, k: u8, profile: &mut Profile, gp: &mut Gameplay) -> GAct {
        let n = ROWS.len();
        let (left, right, enter) = (k == dik::LEFT, k == dik::RIGHT, k == dik::RETURN || k == dik::NUMPADENTER);
        let change = left || right || enter;
        match k {
            dik::ESCAPE | dik::F10 => return GAct::Close,
            dik::UP => self.sel = (self.sel + n - 1) % n,
            dik::DOWN => self.sel = (self.sel + 1) % n,
            _ if !change => {}
            _ => {
                let flip = |b: &mut bool| *b = !*b;
                match self.sel {
                    0 => *profile = profile.toggled(),
                    1 => flip(&mut gp.infinite_lives),
                    2 => flip(&mut gp.invulnerable),
                    3 => {
                        let s = gp.enemy_speed.clamp(ENEMY_SPEED_MIN, 100);
                        gp.enemy_speed = if left {
                            s.saturating_sub(25).max(ENEMY_SPEED_MIN)
                        } else if s >= 100 {
                            ENEMY_SPEED_MIN
                        } else {
                            s + 25
                        };
                    }
                    4 => flip(&mut gp.skip_fmv_any_key),
                    5 => flip(&mut gp.skip_intro_logos),
                    6 => flip(&mut gp.speedrun_timer),
                    7 => flip(&mut gp.auto_save),
                    8 => flip(&mut gp.fix_tube_softlock),
                    9 if enter => return GAct::LoadSlot,
                    9 => {
                        let s = gp.save_slot.clamp(1, NUM_SLOTS);
                        gp.save_slot = if left {
                            if s == 1 {
                                NUM_SLOTS
                            } else {
                                s - 1
                            }
                        } else if s == NUM_SLOTS {
                            1
                        } else {
                            s + 1
                        };
                    }
                    _ if enter => return GAct::SaveNow,
                    _ => return GAct::None,
                }
                return GAct::Changed;
            }
        }
        GAct::None
    }

    pub fn draw(&self, r: &mut Renderer, font: FontId, profile: Profile, gp: &Gameplay, ow: f32) {
        let (white, grey, yellow) = ([255, 255, 255, 255], [170, 170, 170, 255], [255, 230, 80, 255]);
        let (x0, x1, mut y) = (24.0, 330.0, 64.0);
        for (i, name) in ROWS.iter().enumerate() {
            let sel = i == self.sel;
            let col = if sel { yellow } else { white };
            r.draw_text(font, x0 - 12.0, y, if sel { ">" } else { " " }, col, 1.0, None);
            r.draw_text(font, x0, y, name, col, 1.0, None);
            r.draw_text(font, x1, y, &self.value(i, profile, gp), if sel { yellow } else { grey }, 1.0, None);
            y += 18.0;
        }
        if profile == Profile::Authentic {
            r.draw_text(font, 0.0, y + 8.0, "authentic profile: everything off (retail)", [255, 160, 120, 255], 1.0, Some(ow / 2.0));
        }
        if !self.msg.is_empty() {
            r.draw_text(font, 0.0, y + 26.0, &self.msg, [120, 255, 120, 255], 1.0, Some(ow / 2.0));
        }
        r.draw_text(font, 0.0, y + 48.0, "Arrows/Enter change   Tab page   Esc close", grey, 1.0, Some(ow / 2.0));
    }
}

/// A level to start from the level-select tab (practice).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LevelChoice {
    /// Story mode from `storySequence[n]` (`BookStart`).
    Story(u32),
    /// One arcade level (`ArcadeStart`), unlocked or not.
    Arcade(u32, u32),
}

/// The level-select / practice tab: every story chapter and single-player level, whatever
/// the save has unlocked. Built from the game's own tables (`storySequence`,
/// `worldVisualData`) and level names (game text) when the screen opens.
#[derive(Default)]
pub struct LevelSelectUi {
    pub sel: usize,
    pub entries: Vec<(String, LevelChoice)>,
}

/// Rows shown at once.
const PAGE: usize = 18;

impl LevelSelectUi {
    pub fn build(g: &Game) -> LevelSelectUi {
        let mut entries = Vec::new();
        let name = |w: u32, l: u32| -> String {
            let s = g
                .layout
                .world_visual_data
                .get(w as usize)
                .and_then(|wv| wv.level_visual_data.get(l as usize))
                .map(|lv| g.language.gamestring(lv.description_str).trim().to_string())
                .unwrap_or_default();
            if s.is_empty() {
                format!("world {w} level {l}")
            } else {
                s
            }
        };
        for n in 1..NUM_STORY_LEVELS as u32 {
            let s = STORY_SEQUENCE[n as usize];
            entries.push((format!("Story {n:2}: {}", name(s.world_num, s.level_num)), LevelChoice::Story(n)));
        }
        for (w, wv) in g.layout.world_visual_data.iter().enumerate() {
            let w = w as u32;
            if w == worldid::FRONTEND || w == worldid::TEST {
                continue;
            }
            for l in 0..wv.num_single_player_levels.min(wv.level_visual_data.len() as u32) {
                entries.push((format!("Practice {w}-{l}: {}", name(w, l)), LevelChoice::Arcade(w, l)));
            }
        }
        LevelSelectUi { sel: 0, entries }
    }

    pub fn key(&mut self, k: u8) -> GAct {
        let n = self.entries.len().max(1);
        match k {
            dik::ESCAPE | dik::F10 => return GAct::Close,
            dik::UP => self.sel = (self.sel + n - 1) % n,
            dik::DOWN => self.sel = (self.sel + 1) % n,
            dik::PRIOR => self.sel = self.sel.saturating_sub(PAGE),
            dik::NEXT => self.sel = (self.sel + PAGE).min(n - 1),
            dik::RETURN | dik::NUMPADENTER => {
                if let Some(&(_, c)) = self.entries.get(self.sel) {
                    return GAct::Start(c);
                }
            }
            _ => {}
        }
        GAct::None
    }

    pub fn draw(&self, r: &mut Renderer, font: FontId, ow: f32) {
        let (white, grey, yellow) = ([255, 255, 255, 255], [170, 170, 170, 255], [255, 230, 80, 255]);
        let first = self.sel.saturating_sub(PAGE / 2).min(self.entries.len().saturating_sub(PAGE));
        let mut y = 64.0;
        for (i, (label, _)) in self.entries.iter().enumerate().skip(first).take(PAGE) {
            let sel = i == self.sel;
            let col = if sel { yellow } else { white };
            r.draw_text(font, 12.0, y, if sel { ">" } else { " " }, col, 1.0, None);
            r.draw_text(font, 24.0, y, label, col, 1.0, None);
            y += 18.0;
        }
        r.draw_text(font, 0.0, y + 12.0, "PgUp/PgDn   Enter play   Tab page   Esc close", grey, 1.0, Some(ow / 2.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_follow_profile() {
        let gp = Gameplay { infinite_lives: true, invulnerable: true, enemy_speed: 50, ..Gameplay::default() };
        assert_eq!(game_options(Profile::Authentic, &gp), GameOptions::AUTHENTIC);
        let o = game_options(Profile::Enhanced, &gp);
        assert!(o.infinite_lives_on() && o.invulnerable_on());
        assert_eq!(o.enemy_speed_pct(), 50);
        assert_eq!(game_options(Profile::Enhanced, &Gameplay::default()), GameOptions::ENHANCED);
    }

    #[test]
    fn slots() {
        let p = Path::new("/x/saves/savegame.dat");
        assert_eq!(slot_path(p, 1), p);
        assert_eq!(slot_path(p, 3), Path::new("/x/saves/slot3/savegame.dat"));
    }

    #[test]
    fn timer_splits() {
        let dir = std::env::temp_dir().join(format!("f2-splits-{}", std::process::id()));
        let file = dir.join("splits.csv");
        let _ = std::fs::remove_file(&file);
        let t0 = Instant::now();
        let mut s = Speedrun::new(Some(file.clone()));
        assert_eq!(s.on_tick(mode::FRONTEND_MODE, worldid::FRONTEND, 0, t0), None);
        for i in 0..120 {
            s.on_tick(mode::INGAME_MODE, 0, 0, t0 + Duration::from_millis(i * 16));
        }
        for _ in 0..30 {
            s.on_tick(mode::PAUSE_MODE, 0, 0, t0); // not counted
        }
        let sp = s.on_tick(mode::LEVELCOMPLETE_MODE, 0, 0, t0 + Duration::from_secs(3)).expect("split");
        assert_eq!((sp.world, sp.level, sp.igt, sp.total_igt, sp.best), (0, 0, 120, 120, None));
        assert_eq!(sp.rta, Duration::from_secs(3));
        assert_eq!(s.on_tick(mode::LEVELCOMPLETE_MODE, 0, 0, t0), None, "one split per completion");
        assert_eq!(fmt_frames(sp.igt), "0:02.00");
        // The best time is read back from the splits file.
        let s2 = Speedrun::new(Some(file.clone()));
        assert_eq!(s2.best.get(&(0, 0)), Some(&120));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gameplay_tab_edits() {
        let mut ui = GameplayUi::default();
        let (mut p, mut gp) = (Profile::Enhanced, Gameplay::default());
        ui.sel = 1;
        assert_eq!(ui.key(dik::RETURN, &mut p, &mut gp), GAct::Changed);
        assert!(gp.infinite_lives);
        ui.sel = 3;
        assert_eq!(ui.key(dik::LEFT, &mut p, &mut gp), GAct::Changed);
        assert_eq!(gp.enemy_speed, 75);
        ui.sel = 8;
        assert_eq!(ui.key(dik::RETURN, &mut p, &mut gp), GAct::Changed);
        assert!(!gp.fix_tube_softlock);
        assert!(!game_options(p, &gp).fix_tube_softlock_on());
        ui.sel = 9;
        assert_eq!(ui.key(dik::RIGHT, &mut p, &mut gp), GAct::Changed);
        assert_eq!(gp.save_slot, 2);
        assert_eq!(ui.key(dik::RETURN, &mut p, &mut gp), GAct::LoadSlot);
        ui.sel = 10;
        assert_eq!(ui.key(dik::RETURN, &mut p, &mut gp), GAct::SaveNow);
        ui.sel = 0;
        assert_eq!(ui.key(dik::RETURN, &mut p, &mut gp), GAct::Changed);
        assert_eq!(p, Profile::Authentic);
        assert_eq!(ui.key(dik::ESCAPE, &mut p, &mut gp), GAct::Close);
    }

    #[test]
    fn level_select_lists_story_and_practice() {
        let g = Game::new();
        let mut ui = LevelSelectUi::build(&g);
        assert_eq!(ui.entries[0].1, LevelChoice::Story(1));
        assert!(ui.entries.iter().any(|e| e.1 == LevelChoice::Arcade(worldid::GARDEN, 0)));
        assert!(!ui.entries.iter().any(|e| matches!(e.1, LevelChoice::Arcade(w, _) if w == worldid::FRONTEND)));
        assert_eq!(ui.key(dik::RETURN), GAct::Start(LevelChoice::Story(1)));
    }
}
