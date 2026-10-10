//! Level editor mode (`frogger --editor --level W L`, or F3+Ctrl... see `KEYS`), a
//! modern take on the original debug-build editor (`FroggerProject/Editor/`): a free
//! camera over the level, the collision tile graph colour-coded by state with its
//! links, entity markers and paths, scenics, the level script, a command line, a
//! play-test toggle and saving to a mod folder in the retail formats.
//!
//! The simulation is paused while editing; play-test reloads the level from the
//! edited files in memory ([`doc::EditedData`]).

pub mod doc;

use crate::render::{self, FlyCam};
use doc::{next_tile_state, tile_state_name, LevelDoc, DIRS};
use f2_engine::scene::{Frame, TlVertex, Tri};
use f2_engine::{FontId, Renderer};
use f2_formats::entity::{ActorKind, Item};
use f2_platform::{dik, KeyboardState};
use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Tiles,
    Entities,
    Paths,
    Scenics,
    Script,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Tiles => "TILES",
            Mode::Entities => "ENTITIES",
            Mode::Paths => "PATHS",
            Mode::Scenics => "SCENICS",
            Mode::Script => "SCRIPT",
        }
    }
}

/// What the editor asks the host to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Reload the level from the edited data; `play` runs the simulation (play-test).
    Reload { play: bool },
    /// Open the script source in `$EDITOR` (window mode).
    EditScript,
}

/// Panel text scale (the game's small font is wide).
const TEXT_SCALE: f32 = 0.75;

pub const KEYS: &[&str] = &[
    "WASD/QE move, arrows look, Shift fast | mouse/crosshair points | Space select, Ctrl+Space add to selection",
    "1 tiles 2 entities 3 paths 4 scenics 5 script | Enter command line | P play-test | R reload | Ctrl+S save | F1 help",
    "tiles: PgUp/PgDn state, L link sel->pointer, U unlink towards pointer, Ins extrude towards pointer, Del delete",
    "entities: Tab next, M move to pointer, N new enemy, G garib, C camera case, Del delete, K toggle camera tile",
    "paths: [ ] node, N add node at pointer, M move node, Del delete node, PgUp/PgDn speed | scenics: M move, Ins copy, Del",
];

/// The editor state (one level).
pub struct Editor {
    pub doc: LevelDoc,
    pub mode: Mode,
    pub cam: FlyCam,
    pub sel_tiles: BTreeSet<usize>,
    pub sel_item: Option<usize>,
    pub sel_node: Option<usize>,
    pub sel_scenic: Option<usize>,
    /// Mouse position in pixels (`None`: the screen centre crosshair).
    pub pointer: Option<(f32, f32)>,
    pub hover_tile: Option<usize>,
    pub hover_item: Option<usize>,
    pub hover_scenic: Option<usize>,
    /// The command line being typed.
    pub prompt: Option<String>,
    pub log: VecDeque<String>,
    /// Play-testing (simulation running from the edited data).
    pub playing: bool,
    pub mods_dir: PathBuf,
    pub mod_id: String,
    pub show_help: bool,
    pub script_scroll: usize,
    pub requests: Vec<Request>,
    /// Show the tile links.
    pub show_links: bool,
    /// Half size of the drawn tile squares (from the level's tile spacing).
    tile_half: f32,
    shift: bool,
    ctrl: bool,
    click: bool,
}

impl Editor {
    pub fn new(doc: LevelDoc, cam: FlyCam) -> Editor {
        let mod_id = format!("level-{}-{}", doc.world, doc.level);
        let tile_half = (doc.tile_spacing() * 0.4).clamp(2.0, 200.0);
        let mut e = Editor {
            doc,
            mode: Mode::Tiles,
            cam,
            sel_tiles: BTreeSet::new(),
            sel_item: None,
            sel_node: None,
            sel_scenic: None,
            pointer: None,
            hover_tile: None,
            hover_item: None,
            hover_scenic: None,
            prompt: None,
            log: VecDeque::new(),
            playing: false,
            mods_dir: PathBuf::from("mods"),
            mod_id,
            show_help: true,
            script_scroll: 0,
            requests: Vec::new(),
            show_links: true,
            tile_half,
            shift: false,
            ctrl: false,
            click: false,
        };
        e.msg(format!(
            "editor: level {}-{}: {} tiles, {} entities, {} paths, {} scenics ({})",
            e.doc.world,
            e.doc.level,
            e.doc.col.tiles.len(),
            e.doc.ents.entities.items.len(),
            e.doc.ents.entities.paths.len(),
            e.doc.scenics.len(),
            if e.doc.ents.entities.fast { "fast entity file" } else { "slow entity file" }
        ));
        e
    }

    pub fn msg(&mut self, s: impl Into<String>) {
        let s = s.into();
        eprintln!("{s}");
        for l in s.lines() {
            self.log.push_back(l.to_string());
        }
        while self.log.len() > 6 {
            self.log.pop_front();
        }
    }

    /// The tile a command acts on when none is given: the pointer's tile.
    fn pointer_tile(&self) -> Result<usize, String> {
        self.hover_tile.ok_or_else(|| "no tile under the pointer".to_string())
    }

    fn sel_tile(&self) -> Result<usize, String> {
        self.sel_tiles.iter().next().copied().ok_or_else(|| "no tile selected".to_string())
    }

    fn item(&self) -> Result<usize, String> {
        self.sel_item.ok_or_else(|| "no entity selected".to_string())
    }

    fn path_node(&self) -> Result<(usize, usize), String> {
        let i = self.item()?;
        let p = self.doc.item_path(i).ok_or("the selected entity has no path")?;
        Ok((p, self.sel_node.unwrap_or(0)))
    }

    // ------------------------------------------------------------ input

    /// Keyboard input for one frame (`kb.pressed` = key-down events).
    pub fn input(&mut self, kb: &KeyboardState, dt: f32) {
        self.shift = kb.is_down(dik::LSHIFT) || kb.is_down(dik::RSHIFT);
        self.ctrl = kb.is_down(dik::LCONTROL) || kb.is_down(dik::RCONTROL);
        for &k in &kb.pressed {
            self.key(k);
        }
        if self.prompt.is_none() && !self.playing && !self.ctrl {
            self.cam.update(kb, dt);
        }
    }

    /// Mouse move (window pixels) / left click.
    pub fn mouse(&mut self, pos: Option<(f32, f32)>, click: bool) {
        if pos.is_some() {
            self.pointer = pos;
        }
        if click {
            self.click = true;
        }
    }

    pub fn key(&mut self, k: u8) {
        if let Some(p) = &mut self.prompt {
            match k {
                dik::RETURN | dik::NUMPADENTER => {
                    let line = std::mem::take(p);
                    self.prompt = None;
                    self.run(&line);
                }
                dik::ESCAPE => self.prompt = None,
                dik::BACK => {
                    p.pop();
                }
                k => {
                    if let Some(c) = key_char(k, self.shift) {
                        p.push(c);
                    }
                }
            }
            return;
        }
        if self.playing {
            if k == dik::P {
                self.playing = false;
                self.requests.push(Request::Reload { play: false });
                self.msg("editing (simulation paused)");
            }
            return;
        }
        let r = self.key_command(k);
        if let Err(e) = r {
            self.msg(format!("error: {e}"));
        }
    }

    fn key_command(&mut self, k: u8) -> Result<(), String> {
        match k {
            dik::RETURN => self.prompt = Some(String::new()),
            dik::KEY_1 => self.mode = Mode::Tiles,
            dik::KEY_2 => self.mode = Mode::Entities,
            dik::KEY_3 => self.mode = Mode::Paths,
            dik::KEY_4 => self.mode = Mode::Scenics,
            dik::KEY_5 => self.mode = Mode::Script,
            dik::F1 => self.show_help = !self.show_help,
            dik::P => self.run("play"),
            dik::R => self.run("reload"),
            dik::S if self.ctrl => self.run("save"),
            dik::SPACE => self.select_at_pointer(self.ctrl),
            _ => match self.mode {
                Mode::Tiles => self.tile_key(k)?,
                Mode::Entities => self.entity_key(k)?,
                Mode::Paths => self.path_key(k)?,
                Mode::Scenics => self.scenic_key(k)?,
                Mode::Script => self.script_key(k)?,
            },
        }
        Ok(())
    }

    fn tile_key(&mut self, k: u8) -> Result<(), String> {
        match k {
            dik::PRIOR | dik::NEXT => {
                let t = self.sel_tile()?;
                let s = next_tile_state(self.doc.col.tiles[t].state, k == dik::PRIOR);
                for t in self.sel_tiles.clone() {
                    self.doc.set_tile_state(t, s)?;
                }
                self.msg(format!("state {s} {}", tile_state_name(s)));
            }
            dik::L => {
                let (a, b) = (self.sel_tile()?, self.pointer_tile()?);
                let d = self.doc.dir_towards(a, b);
                self.doc.link(a, d, Some(b))?;
                self.msg(format!("tile {a} {} -> {b}", DIRS[d]));
            }
            dik::U => {
                let (a, b) = (self.sel_tile()?, self.pointer_tile()?);
                let d = self.doc.dir_towards(a, b);
                self.doc.link(a, d, None)?;
                self.msg(format!("tile {a} {} unlinked", DIRS[d]));
            }
            dik::INSERT => {
                let a = self.sel_tile()?;
                let d = match self.hover_tile.filter(|&b| b != a) {
                    Some(b) => self.doc.dir_towards(a, b),
                    None => 0,
                };
                let n = self.doc.add_tile(a, d)?;
                self.sel_tiles = [n].into();
                self.msg(format!("added tile {n} ({} of {a})", DIRS[d]));
            }
            dik::DELETE => {
                let t = self.sel_tile()?;
                self.doc.delete_tile(t)?;
                self.sel_tiles.clear();
                self.msg(format!("deleted tile {t} (later tiles renumbered)"));
            }
            _ => {}
        }
        Ok(())
    }

    fn entity_key(&mut self, k: u8) -> Result<(), String> {
        let n = self.doc.ents.entities.items.len();
        match k {
            dik::TAB if n > 0 => {
                self.sel_item = Some(self.sel_item.map_or(0, |i| if self.shift { (i + n - 1) % n } else { (i + 1) % n }));
                self.sel_node = None;
            }
            dik::M => self.run("move"),
            dik::N => self.run("new enemy"),
            dik::G => self.run("new garib"),
            dik::C => self.run("new camera"),
            dik::K => self.run("camtile"),
            dik::DELETE => self.run("del"),
            _ => {}
        }
        Ok(())
    }

    fn path_key(&mut self, k: u8) -> Result<(), String> {
        match k {
            dik::LBRACKET | dik::RBRACKET => {
                let (p, n) = self.path_node()?;
                let len = self.doc.ents.entities.paths[p].nodes.len();
                self.sel_node = Some(if k == dik::RBRACKET { (n + 1) % len } else { (n + len - 1) % len });
            }
            dik::N => self.run("node add"),
            dik::M => self.run("node move"),
            dik::DELETE => self.run("node del"),
            dik::PRIOR | dik::NEXT => {
                let (p, n) = self.path_node()?;
                let s = self.doc.ents.entities.paths[p].nodes[n].speed;
                let step = if self.shift { 4096 } else { 512 };
                let v = if k == dik::PRIOR { s + step } else { (s - step).max(0) };
                self.doc.set_node_field(p, n, "speed", &v.to_string())?;
                self.msg(format!("node {n} speed {:.3}", v as f32 / 4096.0));
            }
            _ => {}
        }
        Ok(())
    }

    fn scenic_key(&mut self, k: u8) -> Result<(), String> {
        match k {
            dik::TAB if !self.doc.scenics.is_empty() => {
                let n = self.doc.scenics.len();
                self.sel_scenic = Some(self.sel_scenic.map_or(0, |i| (i + 1) % n));
            }
            dik::M => self.run("move"),
            dik::INSERT => {
                let i = self.sel_scenic.ok_or("no scenic selected")?;
                let name = self.doc.scenics[i].name.clone();
                self.run(&format!("new scenic {name}"));
            }
            dik::DELETE => self.run("del"),
            _ => {}
        }
        Ok(())
    }

    fn script_key(&mut self, k: u8) -> Result<(), String> {
        match k {
            dik::PRIOR => self.script_scroll = self.script_scroll.saturating_sub(20),
            dik::NEXT => self.script_scroll += 20,
            dik::O => self.run("script edit"),
            _ => {}
        }
        Ok(())
    }

    /// Space: select what is under the pointer in the current mode.
    fn select_at_pointer(&mut self, add: bool) {
        match self.mode {
            Mode::Tiles => {
                if let Some(t) = self.hover_tile {
                    if !add {
                        self.sel_tiles.clear();
                    }
                    if !self.sel_tiles.remove(&t) {
                        self.sel_tiles.insert(t);
                    }
                }
            }
            Mode::Entities | Mode::Paths => {
                if let Some(i) = self.hover_item {
                    self.sel_item = Some(i);
                    self.sel_node = None;
                } else if self.mode == Mode::Paths {
                    // A node of the selected path under the pointer.
                    if let (Ok((p, _)), Some(t)) = (self.path_node(), self.hover_tile) {
                        let nodes = &self.doc.ents.entities.paths[p].nodes;
                        if let Some(n) = nodes.iter().position(|n| self.doc.place_tile(&n.place) == Some(t)) {
                            self.sel_node = Some(n);
                        }
                    }
                }
            }
            Mode::Scenics => self.sel_scenic = self.hover_scenic.or(self.sel_scenic),
            Mode::Script => {}
        }
    }

    // ------------------------------------------------------------ commands

    /// Runs a command line (several separated by `;`), logging the result.
    pub fn run(&mut self, line: &str) {
        for cmd in line.split(';') {
            let cmd = cmd.trim();
            if cmd.is_empty() {
                continue;
            }
            match self.exec(cmd) {
                Ok(m) if m.is_empty() => {}
                Ok(m) => self.msg(m),
                Err(e) => self.msg(format!("error: {cmd}: {e}")),
            }
        }
    }

    /// Optional trailing tile argument, else the pointer's tile.
    fn tile_arg(&self, a: Option<&&str>) -> Result<usize, String> {
        match a {
            Some(s) => s.parse::<usize>().map_err(|_| format!("bad tile {s}")),
            None => self.pointer_tile(),
        }
    }

    /// Executes one command. See `help`.
    pub fn exec(&mut self, cmd: &str) -> Result<String, String> {
        let w: Vec<&str> = cmd.split_whitespace().collect();
        let rest = |n: usize| -> String { cmd.split_whitespace().skip(n).collect::<Vec<_>>().join(" ") };
        match w.as_slice() {
            ["help"] => Ok("commands: mode M | sel tile|item|scenic|node N | state V | link DIR [T] | unlink DIR | addtile DIR | deltile | start [T] | move [T] | set FIELD VALUE | new enemy|platform|placeholder [MODEL [T]] | new garib [TYPE [T]] | new camera [T] | new scenic NAME [T] | del | node add|move|del [T] | camtile [T] | script show|edit|compile|load FILE | save [ID] | saveall [ID] | play | reload | cam top|T | goto [T] | links".into()),
            ["mode", m] => {
                self.mode = match *m {
                    "tiles" | "1" => Mode::Tiles,
                    "ents" | "entities" | "2" => Mode::Entities,
                    "paths" | "3" => Mode::Paths,
                    "scenics" | "4" => Mode::Scenics,
                    "script" | "5" => Mode::Script,
                    _ => return Err(format!("unknown mode {m}")),
                };
                Ok(String::new())
            }
            ["sel", "tile", rest @ ..] => {
                self.mode = Mode::Tiles;
                self.sel_tiles.clear();
                for t in rest {
                    let t: usize = t.parse().map_err(|_| format!("bad tile {t}"))?;
                    if t >= self.doc.col.tiles.len() {
                        return Err(format!("no tile {t}"));
                    }
                    self.sel_tiles.insert(t);
                }
                Ok(String::new())
            }
            ["sel", "item", n] => {
                let i: usize = n.parse().map_err(|_| format!("bad item {n}"))?;
                if i >= self.doc.ents.entities.items.len() {
                    return Err(format!("no item {i}"));
                }
                if self.mode != Mode::Paths {
                    self.mode = Mode::Entities;
                }
                self.sel_item = Some(i);
                self.sel_node = None;
                Ok(self.doc.item_label(i))
            }
            ["sel", "node", n] => {
                let (p, _) = self.path_node()?;
                let n: usize = n.parse().map_err(|_| format!("bad node {n}"))?;
                if n >= self.doc.ents.entities.paths[p].nodes.len() {
                    return Err(format!("no node {n}"));
                }
                self.mode = Mode::Paths;
                self.sel_node = Some(n);
                Ok(String::new())
            }
            ["sel", "scenic", n] => {
                let i: usize = n.parse().map_err(|_| format!("bad scenic {n}"))?;
                if i >= self.doc.scenics.len() {
                    return Err(format!("no scenic {i}"));
                }
                self.mode = Mode::Scenics;
                self.sel_scenic = Some(i);
                Ok(self.doc.scenics[i].name.clone())
            }
            ["state", v] => {
                let s = match tile_state_by_name(v) {
                    Some(s) => s,
                    None => doc_num(v)? as u32,
                };
                if self.sel_tiles.is_empty() {
                    return Err("no tile selected".into());
                }
                for t in self.sel_tiles.clone() {
                    self.doc.set_tile_state(t, s)?;
                }
                Ok(format!("{} tile(s) -> {s} {}", self.sel_tiles.len(), tile_state_name(s)))
            }
            ["link", d, rest @ ..] => {
                let a = self.sel_tile()?;
                let d = dir_index(d)?;
                let b = self.tile_arg(rest.first())?;
                self.doc.link(a, d, Some(b))?;
                Ok(format!("tile {a} {} -> {b}", DIRS[d]))
            }
            ["unlink", d] => {
                let a = self.sel_tile()?;
                let d = dir_index(d)?;
                self.doc.link(a, d, None)?;
                Ok(format!("tile {a} {} unlinked", DIRS[d]))
            }
            ["addtile", d] => {
                let a = self.sel_tile()?;
                let n = self.doc.add_tile(a, dir_index(d)?)?;
                self.sel_tiles = [n].into();
                Ok(format!("added tile {n}"))
            }
            ["deltile"] => {
                let t = self.sel_tile()?;
                self.doc.delete_tile(t)?;
                self.sel_tiles.clear();
                Ok(format!("deleted tile {t}"))
            }
            ["start", rest @ ..] => {
                let t = self.tile_arg(rest.first())?;
                self.doc.set_player_start(0, t)?;
                Ok(format!("player start -> tile {t}"))
            }
            ["move", rest @ ..] => {
                let t = self.tile_arg(rest.first())?;
                match self.mode {
                    Mode::Scenics => {
                        let i = self.sel_scenic.ok_or("no scenic selected")?;
                        let p = self.doc.tile_pos(t).ok_or("bad tile")?;
                        self.doc.move_scenic(i, p)?;
                        Ok(format!("scenic {i} -> tile {t}"))
                    }
                    Mode::Paths => self.exec(&format!("node move {t}")),
                    _ => {
                        let i = self.item()?;
                        self.doc.move_item(i, t)?;
                        Ok(format!("{} -> tile {t}", self.doc.item_label(i)))
                    }
                }
            }
            ["set", field, ..] if w.len() >= 3 => {
                let value = rest(2);
                match self.mode {
                    Mode::Scenics => {
                        let i = self.sel_scenic.ok_or("no scenic selected")?;
                        self.doc.set_scenic_field(i, field, &value)?;
                    }
                    Mode::Paths => {
                        let (p, n) = self.path_node()?;
                        self.doc.set_node_field(p, n, field, &value)?;
                    }
                    _ => {
                        let i = self.item()?;
                        self.doc.set_item_field(i, field, &value)?;
                    }
                }
                Ok(format!("{field} = {value}"))
            }
            ["new", kind @ ("enemy" | "platform" | "placeholder"), rest @ ..] => {
                let model = rest.first().copied().unwrap_or(match *kind {
                    "platform" => "blank.obe",
                    _ => "bfly.obe",
                });
                let t = self.tile_arg(rest.get(1))?;
                let k = match *kind {
                    "enemy" => ActorKind::Enemy,
                    "platform" => ActorKind::Platform,
                    _ => ActorKind::Placeholder,
                };
                let i = self.doc.add_actor(k, model, t)?;
                if !matches!(self.mode, Mode::Entities | Mode::Paths) {
                    self.mode = Mode::Entities;
                }
                self.sel_item = Some(i);
                self.sel_node = None;
                Ok(format!("item {i}: {}", self.doc.item_label(i)))
            }
            ["new", "garib", rest @ ..] => {
                let ty = rest.first().map(|s| doc_num(s)).transpose()?.unwrap_or(0) as u8;
                let t = self.tile_arg(rest.get(1))?;
                let i = self.doc.add_garib(ty, t)?;
                if !matches!(self.mode, Mode::Entities | Mode::Paths) {
                    self.mode = Mode::Entities;
                }
                self.sel_item = Some(i);
                Ok(format!("item {i}: garib on tile {t}"))
            }
            ["new", "camera", rest @ ..] => {
                let t = self.tile_arg(rest.first())?;
                let i = self.doc.add_camera_case(t)?;
                if !matches!(self.mode, Mode::Entities | Mode::Paths) {
                    self.mode = Mode::Entities;
                }
                self.sel_item = Some(i);
                Ok(format!("item {i}: camera case on tile {t}"))
            }
            ["new", "scenic", name, rest @ ..] => {
                let t = self.tile_arg(rest.first())?;
                let p = self.doc.tile_pos(t).ok_or("bad tile")?;
                let i = self.doc.add_scenic(name, p);
                self.sel_scenic = Some(i);
                self.mode = Mode::Scenics;
                Ok(format!("scenic {i}: {name}"))
            }
            ["del"] => match self.mode {
                Mode::Scenics => {
                    let i = self.sel_scenic.take().ok_or("no scenic selected")?;
                    self.doc.delete_scenic(i)?;
                    Ok(format!("deleted scenic {i}"))
                }
                Mode::Tiles => self.exec("deltile"),
                Mode::Paths => self.exec("node del"),
                _ => {
                    let i = self.sel_item.take().ok_or("no entity selected")?;
                    let l = self.doc.item_label(i);
                    self.doc.delete_item(i)?;
                    Ok(format!("deleted item {i} ({l})"))
                }
            },
            ["node", "add", rest @ ..] => {
                let (p, _) = self.path_node()?;
                let t = self.tile_arg(rest.first())?;
                let n = self.doc.add_node(p, self.sel_node, t)?;
                self.sel_node = Some(n);
                Ok(format!("path {p}: node {n} on tile {t}"))
            }
            ["node", "move", rest @ ..] => {
                let (p, n) = self.path_node()?;
                let t = self.tile_arg(rest.first())?;
                self.doc.move_node(p, n, t)?;
                Ok(format!("path {p} node {n} -> tile {t}"))
            }
            ["node", "del"] => {
                let (p, n) = self.path_node()?;
                self.doc.delete_node(p, n)?;
                self.sel_node = Some(n.saturating_sub(1));
                Ok(format!("path {p}: deleted node {n}"))
            }
            ["camtile", rest @ ..] => {
                let i = self.item()?;
                let t = self.tile_arg(rest.first())?;
                self.doc.toggle_camera_tile(i, t)?;
                Ok(format!("camera case {i}: toggled tile {t}"))
            }
            ["script", "show"] => {
                self.mode = Mode::Script;
                self.doc.script_source()?;
                Ok(String::new())
            }
            ["script", "edit"] => {
                self.doc.script_source()?;
                self.requests.push(Request::EditScript);
                Ok(String::new())
            }
            ["script", "compile"] => {
                let src = self.doc.script_source()?.to_string();
                let w = self.doc.set_script_source(src)?;
                Ok(format!("script compiled ({} warnings)", w.len()))
            }
            ["script", "load", file] => {
                let src = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
                let w = self.doc.set_script_source(src)?;
                Ok(format!("script compiled from {file} ({} warnings)", w.len()))
            }
            ["save", rest @ ..] | ["saveall", rest @ ..] => {
                if let Some(id) = rest.first() {
                    self.mod_id = id.to_string();
                }
                let all = w[0] == "saveall";
                let files = self.doc.save_mod(&self.mods_dir, &self.mod_id.clone(), all)?;
                Ok(format!(
                    "saved {} file(s) to {} (play with: frogger --mod {} --level {} {})",
                    files.len(),
                    self.mods_dir.join(&self.mod_id).display(),
                    self.mod_id,
                    self.doc.world,
                    self.doc.level
                ))
            }
            ["play"] => {
                self.playing = true;
                self.requests.push(Request::Reload { play: true });
                Ok("play-test (P returns to the editor)".into())
            }
            ["reload"] => {
                self.requests.push(Request::Reload { play: false });
                Ok(String::new())
            }
            ["links"] => {
                self.show_links = !self.show_links;
                Ok(String::new())
            }
            ["cam", "top"] => {
                let t = self.hover_tile.or(self.sel_tiles.iter().next().copied()).unwrap_or(self.doc.col.player_start[0].max(0) as usize);
                self.look_at_tile(t);
                Ok(String::new())
            }
            ["goto"] => {
                let t = match self.mode {
                    Mode::Scenics => self.sel_scenic.and_then(|i| self.doc.nearest_tile(doc::sv3(&self.doc.scenics[i].pos))),
                    Mode::Tiles => self.sel_tiles.iter().next().copied(),
                    _ => self.sel_item.and_then(|i| self.doc.item_tile(i)),
                }
                .ok_or("nothing selected")?;
                self.look_at_tile(t);
                Ok(String::new())
            }
            ["goto", t] | ["cam", t] => {
                let t: usize = t.parse().map_err(|_| format!("bad tile {t}"))?;
                self.look_at_tile(t);
                Ok(String::new())
            }
            _ => Err("unknown command (try `help`)".into()),
        }
    }

    /// Puts the camera above and behind tile `t`, looking down at it.
    pub fn look_at_tile(&mut self, t: usize) {
        if let Some(p) = self.doc.tile_pos(t) {
            let p = [p[0] as f32, p[1] as f32, p[2] as f32];
            let n = self.doc.col.tiles[t].normal;
            let up = norm3([n.x as f32, n.y as f32, n.z as f32]);
            let up = if up == [0.0; 3] { [0.0, 1.0, 0.0] } else { up };
            let src = [p[0] + up[0] * 220.0, p[1] + up[1] * 220.0 - 0.0, p[2] + up[2] * 220.0 - 160.0];
            let d = norm3([p[0] - src[0], p[1] - src[1], p[2] - src[2]]);
            self.cam = FlyCam { pos: src, yaw: d[0].atan2(d[2]), pitch: d[1].clamp(-1.0, 1.0).asin() };
        }
    }

    // ------------------------------------------------------------ drawing

    /// Picks what is under the pointer with the current view (call after the scene's
    /// camera is set), then draws the overlays and the text panel.
    pub fn draw(&mut self, r: &mut Renderer, font: Option<FontId>, w: u32, h: u32) {
        if self.playing {
            if let Some(font) = font {
                r.draw_text(font, 6.0, 4.0, "PLAY-TEST (edited level) - P: back to the editor", [255, 230, 90, 255], 1.0, None);
            }
            return;
        }
        let ptr = self.pointer.unwrap_or((w as f32 / 2.0, h as f32 / 2.0));
        let proj = |r: &Renderer, p: [i32; 3]| r.view.xfm_point([p[0] as f32, p[1] as f32, p[2] as f32]);
        // ---- picking
        let near = |s: [f32; 3], best: &mut Option<(usize, f32)>, i: usize, lim: f32| {
            let d = ((s[0] - ptr.0).powi(2) + (s[1] - ptr.1).powi(2)).sqrt();
            if d < lim && best.is_none_or(|(_, bd)| d < bd) {
                *best = Some((i, d));
            }
        };
        let mut bt = None;
        for (i, t) in self.doc.col.tiles.iter().enumerate() {
            if let Some(s) = proj(r, doc::sv3(&t.centre)) {
                near(s, &mut bt, i, 40.0);
            }
        }
        self.hover_tile = bt.map(|b| b.0);
        let mut bi = None;
        for i in 0..self.doc.ents.entities.items.len() {
            if let Some(s) = self.doc.item_pos(i).and_then(|p| proj(r, marker_pos(p))) {
                near(s, &mut bi, i, 14.0);
            }
        }
        self.hover_item = bi.map(|b| b.0);
        let mut bs = None;
        for (i, s) in self.doc.scenics.iter().enumerate() {
            if let Some(p) = proj(r, doc::sv3(&s.pos)) {
                near(p, &mut bs, i, 16.0);
            }
        }
        self.hover_scenic = bs.map(|b| b.0);
        if std::mem::take(&mut self.click) {
            self.select_at_pointer(self.ctrl);
        }

        // ---- tiles
        let dim = !matches!(self.mode, Mode::Tiles);
        for (i, t) in self.doc.col.tiles.iter().enumerate() {
            let mut col = render::tile_color(t.state);
            col[3] = if dim { 70 } else { 140 };
            let sel = self.sel_tiles.contains(&i);
            if sel {
                col = [255, 255, 255, 220];
            } else if Some(i) == self.hover_tile {
                col = [255, 255, 0, 200];
            }
            render::draw_tile_quad(r, &tile_geo(t), self.tile_half, col);
        }
        if self.show_links && self.mode == Mode::Tiles {
            for (i, t) in self.doc.col.tiles.iter().enumerate() {
                let close = self.sel_tiles.contains(&i) || Some(i) == self.hover_tile;
                for (d, &a) in t.adj.iter().enumerate() {
                    if a < 0 || a as usize >= self.doc.col.tiles.len() {
                        continue;
                    }
                    let (pa, pb) = (lift(doc::sv3(&t.centre)), lift(doc::sv3(&self.doc.col.tiles[a as usize].centre)));
                    // Half-way arrows: each tile draws its own outgoing link.
                    let mid = [(pa[0] + pb[0]) / 2.0, (pa[1] + pb[1]) / 2.0, (pa[2] + pb[2]) / 2.0];
                    let c = [[255, 80, 80, 255], [80, 255, 80, 255], [80, 160, 255, 255], [255, 255, 80, 255]][d];
                    line3(r, pa, mid, if close { 2.5 } else { 1.0 }, c);
                }
            }
        }

        // ---- paths and entities
        for (pi, p) in self.doc.ents.entities.paths.iter().enumerate() {
            let selp = self.sel_item.and_then(|i| self.doc.item_path(i)) == Some(pi);
            if !selp && !matches!(self.mode, Mode::Paths | Mode::Entities) {
                continue;
            }
            let pts: Vec<[f32; 3]> = p.nodes.iter().filter_map(|n| self.doc.place_pos(&n.place)).map(|q| lift(marker_pos(q))).collect();
            let c = if selp { [255, 140, 0, 255] } else { [255, 140, 0, 90] };
            for w2 in pts.windows(2) {
                line3(r, w2[0], w2[1], if selp { 2.5 } else { 1.0 }, c);
            }
            if selp {
                for (k, q) in pts.iter().enumerate() {
                    let s = if Some(k) == self.sel_node && self.mode == Mode::Paths { 7.0 } else { 4.0 };
                    marker(r, *q, s, if Some(k) == self.sel_node { [255, 255, 255, 255] } else { [255, 140, 0, 255] });
                }
            }
        }
        for i in 0..self.doc.ents.entities.items.len() {
            let Some(p) = self.doc.item_pos(i) else { continue };
            let base = match &self.doc.ents.entities.items[i] {
                Item::Actor(a) => match a.kind {
                    ActorKind::Enemy => [255, 40, 40, 255],
                    ActorKind::Platform => [60, 120, 255, 255],
                    ActorKind::Placeholder => [170, 170, 170, 255],
                },
                Item::Garib(_) => [255, 220, 0, 255],
                Item::CameraCase(_) => [255, 0, 255, 255],
            };
            let sel = Some(i) == self.sel_item;
            let col = if sel {
                [255, 255, 255, 255]
            } else if Some(i) == self.hover_item {
                [255, 255, 160, 255]
            } else if matches!(self.mode, Mode::Entities | Mode::Paths) {
                base
            } else {
                [base[0], base[1], base[2], 110]
            };
            marker(r, lift(marker_pos(p)), if sel { 8.0 } else { 5.0 }, col);
            if sel {
                if let Item::CameraCase(c) = &self.doc.ents.entities.items[i] {
                    for n in &c.nodes {
                        if let Some(t) = self.doc.place_tile(n) {
                            render::draw_tile_quad(r, &tile_geo(&self.doc.col.tiles[t]), self.tile_half * 0.7, [255, 0, 255, 200]);
                        }
                    }
                }
            }
        }
        // ---- scenics
        if self.mode == Mode::Scenics {
            for (i, s) in self.doc.scenics.iter().enumerate() {
                let c = if Some(i) == self.sel_scenic {
                    [255, 255, 255, 255]
                } else if Some(i) == self.hover_scenic {
                    [255, 255, 160, 255]
                } else {
                    [0, 230, 230, 255]
                };
                marker(r, lift(doc::sv3(&s.pos)), if Some(i) == self.sel_scenic { 8.0 } else { 4.0 }, c);
            }
        }
        // Crosshair / pointer.
        let (x, y) = ptr;
        screen_quad(r, x - 8.0, y - 0.5, 16.0, 1.0, [255, 255, 255, 230]);
        screen_quad(r, x - 0.5, y - 8.0, 1.0, 16.0, [255, 255, 255, 230]);

        if let Some(font) = font {
            self.draw_panel(r, font, w, h);
        }
    }

    fn panel_lines(&mut self) -> Vec<(String, [u8; 4])> {
        let white = [255, 255, 255, 255];
        let grey = [190, 190, 190, 255];
        let yellow = [255, 230, 90, 255];
        let mut l: Vec<(String, [u8; 4])> = Vec::new();
        let d = &self.doc;
        l.push((
            format!(
                "EDITOR {}-{}  [{}]{}  mod: {}{}",
                d.world,
                d.level,
                self.mode.name(),
                if self.playing { "  PLAY-TEST (P: back)" } else { "" },
                self.mod_id,
                if d.unsaved() { "  *unsaved*" } else { "" }
            ),
            yellow,
        ));
        if let Some(t) = self.hover_tile {
            let tl = &d.col.tiles[t];
            l.push((format!("pointer: tile {t} state {} {}  links n{} s{} e{} w{}", tl.state, tile_state_name(tl.state), tl.adj[0], tl.adj[1], tl.adj[2], tl.adj[3]), grey));
        }
        match self.mode {
            Mode::Tiles => {
                let n = self.sel_tiles.len();
                if let Some(&t) = self.sel_tiles.iter().next() {
                    let tl = &d.col.tiles[t];
                    let c = doc::sv3(&tl.centre);
                    l.push((format!("selected: {n} tile(s); tile {t} at {} {} {} state {} {}", c[0], c[1], c[2], tl.state, tile_state_name(tl.state)), white));
                }
                l.push((format!("{} tiles, player start {:?}", d.col.tiles.len(), d.col.player_start), grey));
            }
            Mode::Entities | Mode::Paths => {
                if let Some(i) = self.sel_item {
                    l.push((format!("item {i}: {}", d.item_label(i)), white));
                    let f: Vec<String> = d.item_fields(i).iter().map(|(k, v)| format!("{k}={v}")).collect();
                    for chunk in f.chunks(6) {
                        l.push((format!("  {}", chunk.join("  ")), white));
                    }
                    if let Some(p) = d.item_path(i) {
                        let nodes = &d.ents.entities.paths[p].nodes;
                        l.push((format!("  path {p}: {} node(s)", nodes.len()), grey));
                        if self.mode == Mode::Paths {
                            let k = self.sel_node.unwrap_or(0).min(nodes.len().saturating_sub(1));
                            if let Some(n) = nodes.get(k) {
                                l.push((
                                    format!(
                                        "  node {k}: tile {:?} speed={} ({:.3}) wait={} offset={} offset2={} sample={:#x}",
                                        d.place_tile(&n.place),
                                        n.speed,
                                        n.speed as f32 / 4096.0,
                                        n.wait_time,
                                        n.offset,
                                        n.offset2,
                                        n.sample_uid
                                    ),
                                    white,
                                ));
                            }
                        }
                    }
                } else {
                    l.push((format!("{} entities (Tab / Space to select)", d.ents.entities.items.len()), grey));
                }
                if let Some(i) = self.hover_item.filter(|&i| Some(i) != self.sel_item) {
                    l.push((format!("pointer: item {i}: {}", d.item_label(i)), grey));
                }
            }
            Mode::Scenics => {
                if let Some(i) = self.sel_scenic {
                    let s = &d.scenics[i];
                    l.push((format!("scenic {i}: {} pos {} {} {} rot {} {} {} {}", s.name, s.pos.x, s.pos.y, s.pos.z, s.rot.x, s.rot.y, s.rot.z, s.rot.w), white));
                }
                l.push((format!("{} scenics", d.scenics.len()), grey));
            }
            Mode::Script => {}
        }
        l
    }

    fn draw_panel(&mut self, r: &mut Renderer, font: FontId, w: u32, h: u32) {
        let mut lines = self.panel_lines();
        let mut y = 4.0;
        let bg = [0, 0, 0, 150];
        let lh = 11.0;
        if self.mode == Mode::Script {
            let src = match self.doc.script_source() {
                Ok(s) => s.to_string(),
                Err(e) => format!("({e})"),
            };
            let max = ((h as f32 - 160.0) / lh).max(4.0) as usize;
            let total = src.lines().count();
            self.script_scroll = self.script_scroll.min(total.saturating_sub(1));
            lines.push((format!("{}  lines {}-{} of {total}  (PgUp/PgDn, O: $EDITOR, `script compile`)", self.doc.fev_path.replace(".fev", ".etc"), self.script_scroll + 1, (self.script_scroll + max).min(total)), [255, 230, 90, 255]));
            for s in src.lines().skip(self.script_scroll).take(max) {
                lines.push((s.replace('\t', "    "), [210, 255, 210, 255]));
            }
        }
        screen_quad(r, 0.0, 0.0, w as f32, lines.len() as f32 * lh + 8.0, bg);
        for (s, c) in lines {
            r.draw_text(font, 6.0, y, &s, c, TEXT_SCALE, None);
            y += lh;
        }
        // Bottom: messages, prompt, help.
        let mut bottom: Vec<(String, [u8; 4])> = self.log.iter().map(|m| (m.clone(), [200, 230, 255, 255])).collect();
        if let Some(p) = &self.prompt {
            bottom.push((format!("> {p}_"), [255, 255, 255, 255]));
        }
        if self.show_help {
            bottom.extend(KEYS.iter().map(|k| (k.to_string(), [170, 170, 170, 255])));
        }
        let top = h as f32 - bottom.len() as f32 * lh - 6.0;
        screen_quad(r, 0.0, top - 2.0, w as f32, h as f32 - top + 2.0, bg);
        let mut y = top;
        for (s, c) in bottom {
            r.draw_text(font, 6.0, y, &s, c, TEXT_SCALE, None);
            y += lh;
        }
    }
}

fn norm3(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l < 1e-6 {
        [0.0; 3]
    } else {
        [v[0] / l, v[1] / l, v[2] / l]
    }
}

/// Entity markers float a little above their tile.
fn marker_pos(p: [i32; 3]) -> [i32; 3] {
    [p[0], p[1] + 8, p[2]]
}

fn lift(p: [i32; 3]) -> [f32; 3] {
    [p[0] as f32, p[1] as f32 + 1.5, p[2] as f32]
}

fn tile_geo(t: &f2_formats::level::Tile) -> render::TileGeo {
    let s = |v: &f2_formats::level::SVec| [v.x as f32, v.y as f32, v.z as f32];
    render::TileGeo { centre: s(&t.centre), normal: [t.normal.x as f32 / 4096.0, t.normal.y as f32 / 4096.0, t.normal.z as f32 / 4096.0], dir: [t.dir_vector[0].x as f32 / 4096.0, t.dir_vector[0].y as f32 / 4096.0, t.dir_vector[0].z as f32 / 4096.0] }
}

fn vtx(x: f32, y: f32, z: f32, c: [u8; 4]) -> TlVertex {
    render::tl(x, y, z, c)
}

/// A screen-space rectangle (text panel backgrounds, crosshair), on top of everything.
fn screen_quad(r: &mut Renderer, x: f32, y: f32, w: f32, h: f32, c: [u8; 4]) {
    r.draw_quad(f2_engine::Quad2D::rect(x, y, w, h, [0.0, 0.0, 1.0, 1.0], None, c, f2_engine::Blend::Semi));
}

/// A world-space line as a screen-space quad `width` pixels wide.
fn line3(r: &mut Renderer, a: [f32; 3], b: [f32; 3], width: f32, c: [u8; 4]) {
    let (Some(p), Some(q)) = (r.view.xfm_point(a), r.view.xfm_point(b)) else { return };
    let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
    let l = (dx * dx + dy * dy).sqrt().max(1e-3);
    let (nx, ny) = (-dy / l * width / 2.0, dx / l * width / 2.0);
    let v = [vtx(p[0] + nx, p[1] + ny, p[2], c), vtx(p[0] - nx, p[1] - ny, p[2], c), vtx(q[0] - nx, q[1] - ny, q[2], c), vtx(q[0] + nx, q[1] + ny, q[2], c)];
    r.push_tri(Frame::Xlu, Tri { v: [v[0], v[1], v[2]], tex: None });
    r.push_tri(Frame::Xlu, Tri { v: [v[2], v[3], v[0]], tex: None });
}

/// A diamond marker of `size` pixels at a world point.
fn marker(r: &mut Renderer, p: [f32; 3], size: f32, c: [u8; 4]) {
    let Some(s) = r.view.xfm_point(p) else { return };
    let z = s[2] - 4.0;
    let v = [vtx(s[0], s[1] - size, z, c), vtx(s[0] + size, s[1], z, c), vtx(s[0], s[1] + size, z, c), vtx(s[0] - size, s[1], z, c)];
    r.push_tri(Frame::Xlu, Tri { v: [v[0], v[1], v[2]], tex: None });
    r.push_tri(Frame::Xlu, Tri { v: [v[2], v[3], v[0]], tex: None });
}

fn dir_index(d: &str) -> Result<usize, String> {
    match d {
        "n" | "0" => Ok(0),
        "s" | "1" => Ok(1),
        "e" | "2" => Ok(2),
        "w" | "3" => Ok(3),
        _ => Err(format!("bad direction {d} (n s e w)")),
    }
}

fn doc_num(s: &str) -> Result<i64, String> {
    let t = s.trim();
    match t.strip_prefix("0x") {
        Some(h) => i64::from_str_radix(h, 16),
        None => t.parse(),
    }
    .map_err(|_| format!("not a number: {s}"))
}

fn tile_state_by_name(s: &str) -> Option<u32> {
    let u = s.to_ascii_uppercase();
    (0..=22u32).chain([0x20, 0x24, 0x28, 0x2c]).find(|&v| tile_state_name(v) == u)
}

/// US-layout character for a scan code (the command line).
fn key_char(k: u8, shift: bool) -> Option<char> {
    const ROWS: [(u8, &str, &str); 4] = [(dik::KEY_1, "1234567890-=", "!@#$%^&*()_+"), (dik::Q, "qwertyuiop[]", "QWERTYUIOP{}"), (dik::A, "asdfghjkl;'`", "ASDFGHJKL:\"~"), (dik::Z, "zxcvbnm,./", "ZXCVBNM<>?")];
    if k == dik::SPACE {
        return Some(' ');
    }
    if k == dik::BACKSLASH {
        return Some(if shift { '|' } else { '\\' });
    }
    for (start, lo, hi) in ROWS {
        let s = if shift { hi } else { lo };
        if k >= start && ((k - start) as usize) < s.len() {
            return s.chars().nth((k - start) as usize);
        }
    }
    match k {
        dik::NUMPAD0 => Some('0'),
        dik::NUMPAD1 => Some('1'),
        dik::NUMPAD2 => Some('2'),
        dik::NUMPAD3 => Some('3'),
        dik::NUMPAD4 => Some('4'),
        dik::NUMPAD5 => Some('5'),
        dik::NUMPAD6 => Some('6'),
        dik::NUMPAD7 => Some('7'),
        dik::NUMPAD8 => Some('8'),
        dik::NUMPAD9 => Some('9'),
        dik::DECIMAL => Some('.'),
        dik::SUBTRACT => Some('-'),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
