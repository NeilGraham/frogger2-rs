//! Input planner for the "finish" parity corpus: finds pad input that drives a scenario
//! to its end state (single player: every baby saved, the level-complete screen, continue
//! and save; local multiplayer: rounds until a match winner and back to the frontend),
//! by searching with the simulation itself.
//!
//! Single player, race and collect: Go-Explore ([`go_explore`]): an archive of distinct
//! situations (cells: the driven frog's tile/platform, babies saved, script flags, rounds)
//! with their game states; promising, rarely tried cells are revisited and explored with
//! random hops. In a race the frogs run one at a time (the first one not finished; the
//! idle ones die off screen and respawn next to the leader), so each run is a single-frog
//! problem. Battle, and anything the archive search gives up on, uses receding-horizon
//! random shooting: from the committed state, `K` candidate input
//! sequences of `H` ticks are simulated on clones of the game and scored; the first `C`
//! ticks of the best are committed (replayed on the committed game) and the search
//! repeats. Candidates are hop presses (a direction, optionally with CROSS for a superhop)
//! with seeded hold and gap lengths, plus waits; candidate 0 is "no input", candidate 1
//! the rest of the previous winner. When the score stalls, the horizon and the number of
//! candidates grow. Scores use tile-graph distances (`GameTile::tile_ptrs`, Dijkstra
//! towards the targets: unsaved babies, race finish line) and penalise deaths.
//!
//! Everything is deterministic (`--plan-seed`), and the simulation matches the retail
//! exe, so the produced `.key` replays to the same end state on the oracle.

use f2_game::define::mode::{INGAME_MODE, LEVELCOMPLETE_MODE};
use f2_game::define::{tilestate, MULTILOCAL};
use f2_game::frogmove::FROGSTATUS_ISDEAD;
use f2_game::ids::TileId;
use f2_game::multi::{MULTIMODE_BATTLE, MULTIMODE_COLLECT, MULTIMODE_RACE};
use f2_game::Game;
use std::collections::{BinaryHeap, HashMap};
use std::str::FromStr;

const PAD_UP: u16 = 1 << 4;
const PAD_RIGHT: u16 = 1 << 5;
const PAD_DOWN: u16 = 1 << 6;
const PAD_LEFT: u16 = 1 << 7;
const PAD_CIRCLE: u16 = 1 << 13;
const PAD_CROSS: u16 = 1 << 14;
const DIRS: [u16; 4] = [PAD_UP, PAD_RIGHT, PAD_DOWN, PAD_LEFT];

/// How the frogs other than player 0 are driven in multiplayer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rivals {
    /// Planned with player 0 (shared objective; frogs jostle and collide).
    Plan,
    /// Seeded random hops (fixed per seed; not searched).
    Random,
    /// No input.
    Idle,
    /// Race `Plan`, battle and collect `Random` (default).
    Auto,
}

impl FromStr for Rivals {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "plan" => Ok(Rivals::Plan),
            "random" => Ok(Rivals::Random),
            "idle" => Ok(Rivals::Idle),
            "auto" => Ok(Rivals::Auto),
            _ => Err(format!("--plan-rivals {s}: plan|random|idle|auto")),
        }
    }
}

pub struct PlanOpts {
    pub seed: u64,
    /// Maximum committed ticks.
    pub budget: u32,
    pub rivals: Rivals,
    /// Hand-authored waypoints (single player): stages of tiles visited in order before
    /// the babies (`--plan-waypoints FILE`, [`parse_waypoints`]).
    pub waypoints: Vec<Vec<u32>>,
}

/// Parses a waypoint file (`tools/f2-parity/finish/waypoints/W-L.toml`):
/// `stages = [[t, t, ..], [t], ..]`, tile indices (`firstTile[n]`), `#` comments. The
/// archive search drives the frog to any tile of stage 0, then of stage 1, ... and then
/// to the babies (a switch, a lift ride, the far side of a gate the distance cannot see).
pub fn parse_waypoints(text: &str) -> Result<Vec<Vec<u32>>, String> {
    let body: String = text.lines().map(|l| l.split('#').next().unwrap_or("")).collect::<Vec<_>>().join(" ");
    let rest = body.split_once("stages").ok_or("waypoints: no `stages = [...]`")?.1;
    let rest = rest.trim_start().strip_prefix('=').ok_or("waypoints: expected `=`")?;
    let mut stages = Vec::new();
    let mut depth = 0;
    let mut cur: Option<Vec<u32>> = None;
    let mut num = String::new();
    for ch in rest.chars() {
        if ch.is_ascii_digit() {
            num.push(ch);
            continue;
        }
        if !num.is_empty() {
            cur.as_mut().ok_or("waypoints: number outside a stage")?.push(num.parse().map_err(|e| format!("waypoints: {e}"))?);
            num.clear();
        }
        match ch {
            '[' => {
                depth += 1;
                if depth == 2 {
                    cur = Some(Vec::new());
                }
            }
            ']' => {
                if depth == 2 {
                    stages.push(cur.take().unwrap_or_default());
                }
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
    }
    Ok(stages)
}

impl Default for PlanOpts {
    fn default() -> Self {
        PlanOpts { seed: 1, budget: 60_000, rivals: Rivals::Auto, waypoints: Vec::new() }
    }
}

/// splitmix64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn range(&mut self, lo: u32, hi: u32) -> u32 {
        lo + (self.next() % u64::from(hi - lo + 1)) as u32
    }
}

/// One full scenario tick; false when the game went back to the frontend.
fn step(g: &mut Game, prev: [u16; 4], held: [u16; 4]) -> bool {
    if !super::tick_first(g, prev, held) {
        return false;
    }
    super::tick_second(g);
    true
}

/// Tile graph distances (reverse Dijkstra), cached per target set and tile-state set.
///
/// Besides `tile_ptrs` the graph has "map knowledge" edges the static tile graph does not
/// show: script teleports and springs (`EV_TELEPORT_P`/`EV_SPRING_P` under a
/// `TR_FROGONTILE`/`OnPath` trigger: trigger tiles -> placeholder tile), and rides along
/// platform paths (lifts, carts, tube rides: consecutive node tiles, both ways). Barred
/// tiles are impassable *in the current state* (`EV_SETTILE_P` gates open as switches are
/// pressed), so a baby behind a closed gate is far until the switch is found.
struct Dist {
    /// `rev[n]` = (tile with an edge into `n`, extra cost of that edge).
    rev: Vec<Vec<(u32, u32)>>,
    cache: HashMap<(Vec<u32>, u64), Vec<u32>>,
    /// Number of map-knowledge edges (script jumps, platform rides).
    extra_edges: usize,
}

const FAR: u32 = 1 << 20;
/// Distance of a frog behind a closed gate: this plus the distance to a switch.
const SWITCH_DIST: u32 = 5000;

fn tile_cost(state: u32) -> Option<u32> {
    match state {
        tilestate::DEADLY | tilestate::DEADLYFALL | tilestate::HOT | tilestate::ELECTRIC | tilestate::NOSUPERHOT => Some(4),
        tilestate::BARRED => None,
        _ => Some(1),
    }
}

/// Tiles where a script trigger fires for the frog (`FrogOnTile`, `OnPath`), recursively.
fn trigger_tiles(g: &mut Game, t: &f2_formats::fev::Trigger, out: &mut Vec<u32>) {
    use f2_formats::fev::Trigger as T;
    match t {
        T::FrogOnTile { tile, .. } => {
            if let Some(x) = g.get_tile_from_number(*tile as i32) {
                out.push(x.0);
            }
        }
        T::OnPath { enemy, .. } => {
            if let Some(e) = g.get_enemy_from_uid(*enemy as i32) {
                let path = g.enemies.enemy_list.array[e.idx()].path;
                out.extend(g.path.nodes(&path).iter().filter_map(|n| n.world_tile.map(|t| t.0)));
            }
        }
        T::FrogOnPlatform { platform, .. } => {
            if let Some(pid) = g.get_platform_from_uid(*platform as i32) {
                let path = g.platform.platform_list.array[pid.idx()].path;
                out.extend(g.path.nodes(&path).iter().filter_map(|n| n.world_tile.map(|t| t.0)));
            }
        }
        T::And(a, b) | T::Or(a, b) => {
            trigger_tiles(g, a, out);
            trigger_tiles(g, b, out);
        }
        _ => {}
    }
}

/// Teleport/spring destinations in a command block (recursively).
fn jump_dests(g: &Game, cmds: &[f2_formats::fev::Command], out: &mut Vec<u32>) {
    use f2_formats::fev::Command as C;
    for c in cmds {
        match c {
            C::TeleportP { id, .. } | C::SpringP { id, .. } => {
                if let Some(e) = g.get_enemy_from_uid(*id as i32) {
                    let path = g.enemies.enemy_list.array[e.idx()].path;
                    if let Some(t) = g.path.nodes(&path).first().and_then(|n| n.world_tile) {
                        out.push(t.0);
                    }
                }
            }
            C::On { block, .. } | C::If { block, .. } => jump_dests(g, block, out),
            _ => {}
        }
    }
}

fn script_edges(g: &mut Game, cmds: &[f2_formats::fev::Command], edges: &mut Vec<(u32, u32, u32)>) {
    use f2_formats::fev::Command as C;
    for c in cmds {
        if let C::On { trigger, block, .. } | C::If { trigger, block } = c {
            let mut from = Vec::new();
            trigger_tiles(g, trigger, &mut from);
            let mut to = Vec::new();
            jump_dests(g, block, &mut to);
            for &f in &from {
                for &t in &to {
                    edges.push((f, t, 3));
                }
            }
            script_edges(g, block, edges);
        }
    }
}

fn has_end_level(cmds: &[f2_formats::fev::Command]) -> bool {
    use f2_formats::fev::Command as C;
    cmds.iter().any(|c| match c {
        C::EndLevel => true,
        C::On { block, .. } | C::If { block, .. } => has_end_level(block),
        _ => false,
    })
}

/// Tiles of the script triggers that end the level (`EV_ENDLEVEL` in their block, also
/// nested): the targets of a level without babies (6-2 ends after a tube ride).
fn end_level_targets(g: &Game) -> Vec<u32> {
    fn walk(g: &mut Game, cmds: &[f2_formats::fev::Command], out: &mut Vec<u32>) {
        use f2_formats::fev::Command as C;
        for c in cmds {
            if let C::On { trigger, block, .. } | C::If { trigger, block } = c {
                if has_end_level(block) {
                    trigger_tiles(g, trigger, out);
                }
                walk(g, block, out);
            }
        }
    }
    let mut out = Vec::new();
    if let Some(s) = g.script.script_buffer.clone() {
        walk(&mut g.clone(), &s.commands, &mut out);
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// A hop from tile `i` through `tilePtrs[k]` as `GetNextTile` resolves it (a `JOIN` tile
/// leads on to its neighbour closest to the current tile's up/down) and whether
/// `GameTileTooHigh` allows it: `(dest, extra cost)`; too high for a superhop = extra 40,
/// superhop only = extra cost 1. Barred tiles are left to the per-state costs.
fn hop_edge(g: &Game, i: usize, k: usize) -> Option<(u32, u32)> {
    use f2_game::frogmove::{MAX_HOP_HEIGHT, MAX_SUPERHOP_HEIGHT};
    let n = g.map.tiles.len();
    let ct = &g.map.tiles[i];
    let dot = |a: &f2_math::SVector, b: &f2_math::SVector| (a.vx as i64 * b.vx as i64 + a.vy as i64 * b.vy as i64 + a.vz as i64 * b.vz as i64) >> 12;
    let mut dest = ct.tile_ptrs[k]?;
    if dest.idx() >= n {
        return None;
    }
    if g.map.tiles[dest.idx()].state == tilestate::JOIN {
        let join = &g.map.tiles[dest.idx()];
        let mut best: Option<(i64, usize)> = None;
        for (m, tp) in join.tile_ptrs.iter().enumerate() {
            if let Some(tp) = tp {
                let t = dot(&ct.normal, &join.dir_vector[m]).abs();
                if tp.idx() != i && best.is_none_or(|(bt, _)| t > bt) {
                    best = Some((t, m));
                }
            }
        }
        dest = join.tile_ptrs[best?.1]?;
        if dest.idx() >= n || g.map.tiles[dest.idx()].state == tilestate::JOIN {
            return None;
        }
    }
    let dt = &g.map.tiles[dest.idx()];
    let diff = f2_math::SVector::new(
        (dt.centre.vx as i32 - ct.centre.vx as i32) as i16,
        (dt.centre.vy as i32 - ct.centre.vy as i32) as i16,
        (dt.centre.vz as i32 - ct.centre.vz as i32) as i16,
    );
    // height in world units (normal is 4.12), compared like the 20.12 constants.
    let height = dot(&diff, &ct.normal) << 12;
    if height > MAX_SUPERHOP_HEIGHT as i64 {
        // Too high from the tile centre; the static model is not exact (walls, frog
        // position), so keep the edge at a high price instead of cutting the graph.
        Some((dest.0, 40))
    } else if height > MAX_HOP_HEIGHT as i64 {
        Some((dest.0, 1))
    } else {
        Some((dest.0, 0))
    }
}

/// Gate switches: (gate tile, trigger tiles) for every `EV_SETTILE_P` that sets a tile to
/// a passable state under a frog trigger (`OnPath`, `FrogOnTile`, `FrogOnPlatform`).
fn gate_switches(g: &Game) -> Vec<Switch> {
    fn sets(g: &Game, cmds: &[f2_formats::fev::Command], out: &mut Vec<u32>) {
        use f2_formats::fev::Command as C;
        for c in cmds {
            match c {
                C::SetTileP { id, state } if u32::from(*state) != tilestate::BARRED => {
                    if let Some(e) = g.get_enemy_from_uid(*id as i32) {
                        let path = g.enemies.enemy_list.array[e.idx()].path;
                        if let Some(t) = g.path.nodes(&path).first().and_then(|n| n.world_tile) {
                            out.push(t.0);
                        }
                    }
                }
                C::On { block, .. } | C::If { block, .. } => sets(g, block, out),
                _ => {}
            }
        }
    }
    fn flags_of(t: &f2_formats::fev::Trigger, out: &mut Vec<u8>) {
        use f2_formats::fev::Trigger as T;
        match t {
            T::FlagSet { flag } => out.push(*flag),
            T::And(a, b) | T::Or(a, b) => {
                flags_of(a, out);
                flags_of(b, out);
            }
            _ => {}
        }
    }
    /// Trigger tiles of the blocks that `SetFlag` `flag`.
    fn setters(g: &mut Game, cmds: &[f2_formats::fev::Command], flag: u8, out: &mut Vec<u32>) {
        use f2_formats::fev::Command as C;
        for c in cmds {
            if let C::On { trigger, block, .. } | C::If { trigger, block } = c {
                if block.iter().any(|b| matches!(b, C::SetFlag { flag: f } if *f == flag)) {
                    trigger_tiles(g, trigger, out);
                }
                setters(g, block, flag, out);
            }
        }
    }
    fn walk(g: &mut Game, all: &[f2_formats::fev::Command], cmds: &[f2_formats::fev::Command], out: &mut Vec<Switch>) {
        use f2_formats::fev::Command as C;
        for c in cmds {
            if let C::On { trigger, block, .. } | C::If { trigger, block } = c {
                let mut gates = Vec::new();
                sets(g, block, &mut gates);
                if !gates.is_empty() {
                    let mut tiles = Vec::new();
                    trigger_tiles(g, trigger, &mut tiles);
                    for &gt in &gates {
                        if !tiles.is_empty() {
                            out.push(Switch { gate: gt, flag: None, tiles: tiles.clone() });
                        }
                        // A gate opened when script flags are set: the flags' setters.
                        let mut flags = Vec::new();
                        flags_of(trigger, &mut flags);
                        for f in flags {
                            let mut ts = Vec::new();
                            setters(g, all, f, &mut ts);
                            if !ts.is_empty() {
                                out.push(Switch { gate: gt, flag: Some(f), tiles: ts });
                            }
                        }
                    }
                }
                walk(g, all, block, out);
            }
        }
    }
    let mut out = Vec::new();
    if let Some(s) = g.script.script_buffer.clone() {
        walk(&mut g.clone(), &s.commands, &s.commands, &mut out);
    }
    out
}

/// A gate switch ([`gate_switches`]).
struct Switch {
    gate: u32,
    /// Script flag the gate's trigger waits for (the switch sets it), if any.
    flag: Option<u8>,
    tiles: Vec<u32>,
}

impl Switch {
    /// Still to be pressed: the gate is closed and the flag not set yet.
    fn open(&self, g: &Game) -> bool {
        g.map.tiles.get(self.gate as usize).is_some_and(|x| x.state == tilestate::BARRED)
            && self.flag.is_none_or(|f| g.script.script_flags.get((f >> 3) as usize).is_none_or(|b| b & (1 << (f & 7)) == 0))
    }
}

impl Dist {
    fn new(g: &Game) -> Dist {
        let n = g.map.tiles.len();
        let mut rev = vec![Vec::new(); n];
        for i in 0..n {
            for k in 0..4 {
                if let Some((d, extra)) = hop_edge(g, i, k) {
                    rev[d as usize].push((i as u32, extra));
                }
            }
        }
        let mut edges: Vec<(u32, u32, u32)> = Vec::new();
        // Platform rides: consecutive path nodes, both ways (cost ~ waiting for the ride).
        for p in &g.platform.platform_list.array {
            let tiles: Vec<u32> = g.path.nodes(&p.path).iter().filter_map(|n| n.world_tile.map(|t| t.0)).collect();
            for w in tiles.windows(2) {
                if w[0] != w[1] {
                    edges.push((w[0], w[1], 4));
                    edges.push((w[1], w[0], 4));
                }
            }
        }
        if let Some(s) = g.script.script_buffer.clone() {
            let mut gm = g.clone();
            script_edges(&mut gm, &s.commands, &mut edges);
        }
        let extra_edges = edges.len();
        for (f, t, c) in edges {
            if (f as usize) < n && (t as usize) < n {
                rev[t as usize].push((f, c));
            }
        }
        Dist { rev, cache: HashMap::new(), extra_edges }
    }

    /// Distances of every tile to the nearest of `targets` in the tile states of `g`
    /// (entering a tile costs its state's cost; barred tiles cannot be entered).
    fn to(&mut self, g: &Game, targets: &[u32]) -> &Vec<u32> {
        let mut key = targets.to_vec();
        key.sort_unstable();
        key.dedup();
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for t in &g.map.tiles {
            h = (h ^ u64::from(t.state == tilestate::BARRED)).wrapping_mul(0x100_0000_01b3);
        }
        let ck = (key, h);
        if !self.cache.contains_key(&ck) {
            if self.cache.len() > 4096 {
                self.cache.clear();
            }
            let cost: Vec<Option<u32>> = g.map.tiles.iter().map(|t| tile_cost(t.state)).collect();
            let mut d = vec![FAR; self.rev.len()];
            let mut heap = BinaryHeap::new();
            for &t in &ck.0 {
                if (t as usize) < d.len() {
                    d[t as usize] = 0;
                    heap.push(std::cmp::Reverse((0u32, t)));
                }
            }
            while let Some(std::cmp::Reverse((dv, n))) = heap.pop() {
                if dv > d[n as usize] {
                    continue;
                }
                // Targets themselves are always enterable (a baby on a barred tile).
                let c = match cost[n as usize] {
                    Some(c) => c,
                    None if dv == 0 => 1,
                    None => continue,
                };
                for &(p, extra) in &self.rev[n as usize] {
                    let nd = dv + c + extra;
                    if nd < d[p as usize] {
                        d[p as usize] = nd;
                        heap.push(std::cmp::Reverse((nd, p)));
                    }
                }
            }
            self.cache.insert(ck.clone(), d);
        }
        &self.cache[&ck]
    }
}

/// The tile nearest to a world position (fallback when an actor has no tile).
fn nearest_tile(g: &Game, x: i32, y: i32, z: i32) -> u32 {
    let mut best = (i64::MAX, 0u32);
    for (i, t) in g.map.tiles.iter().enumerate() {
        let (dx, dy, dz) = (t.centre.vx as i64 - x as i64, t.centre.vy as i64 - y as i64, t.centre.vz as i64 - z as i64);
        let d = dx * dx + dy * dy + dz * dz;
        if d < best.0 {
            best = (d, i as u32);
        }
    }
    best.1
}

/// Tiles of the babies not saved yet (`babyList`, enemy tile or nearest to the actor).
fn baby_targets(g: &Game) -> Vec<u32> {
    let nb = g.map.num_babies.clamp(0, g.babyfrog.baby_list.len() as i32) as usize;
    let mut v = Vec::new();
    for b in &g.babyfrog.baby_list[..nb] {
        if b.is_saved != 0 {
            continue;
        }
        let tile = b.enemy.and_then(|e| g.enemies.enemy_list.array.get(e.idx())).and_then(|e| e.in_tile).map(|t| t.0);
        let tile = tile.or_else(|| {
            b.baby.and_then(|a| g.actor2.actors.get(a)).map(|a| {
                let p = a.actor.position;
                nearest_tile(g, p.vx as i32, p.vy as i32, p.vz as i32)
            })
        });
        if let Some(t) = tile {
            v.push(t);
        }
    }
    v
}

fn is_dead(g: &Game, p: usize) -> bool {
    g.frogger.player[p].frog_state & FROGSTATUS_ISDEAD != 0 || g.frogger.player[p].dead.time != 0
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Single,
    Race,
    Battle,
    Collect,
}

struct Planner {
    kind: Kind,
    n: usize,
    /// Players whose input is searched.
    planned: Vec<usize>,
    /// Players with fixed seeded random input.
    random: Vec<usize>,
    dist: Dist,
    finish: Vec<u32>,
    rng: Rng,
    rival_rng: Rng,
    /// Fixed random rival input, generated ahead (`ticks -> held`).
    rival_tape: Vec<[u16; 4]>,
    /// Tiles player 0 stood on in the committed run (exploration bonus when stalled).
    visited: Vec<bool>,
    /// Score per newly visited tile.
    explore: f64,
    /// Rounds finished on the archive search's current path.
    rounds: u32,
    /// Tiles of the triggers that end the level by script (levels without babies).
    end_targets: Vec<u32>,
    waypoints: Vec<Vec<u32>>,
    /// Waypoint stage reached on the archive search's current path.
    wp: usize,
    /// [`gate_switches`].
    switches: Vec<Switch>,
}

/// Outcome of a candidate.
struct Eval {
    score: f64,
    /// Tick (relative) at which the target was reached.
    goal: Option<usize>,
}

impl Planner {
    /// Single player targets: the unsaved babies, else the level-ending script triggers.
    fn targets(&self, g: &Game) -> Vec<u32> {
        if let Some(w) = self.waypoints.get(self.wp) {
            return w.clone();
        }
        let t = baby_targets(g);
        if t.is_empty() {
            self.end_targets.clone()
        } else {
            t
        }
    }

    /// Is the scenario's end state reached?
    fn goal(&self, g: &Game) -> bool {
        match self.kind {
            Kind::Single => g.game.end_level_timer.time != 0 || g.game_state.mode == LEVELCOMPLETE_MODE,
            _ => g.multi.match_winner != -1,
        }
    }

    fn player_dist(&mut self, g: &Game, p: usize, targets: &[u32]) -> f64 {
        if targets.is_empty() {
            return 0.0;
        }
        let tile = g.frogmove.curr_tile[p].or(g.frogmove.dest_tile[p]);
        let Some(TileId(t)) = tile else { return FAR as f64 };
        let d = self.dist.to(g, targets).get(t as usize).copied().unwrap_or(FAR);
        if d < FAR || self.kind != Kind::Single {
            return d as f64;
        }
        // Behind a closed gate: head for a switch that opens one (then the target is near).
        let sw: Vec<u32> = self.switches.iter().filter(|s| s.open(g)).flat_map(|s| s.tiles.iter().copied()).collect();
        if sw.is_empty() {
            return FAR as f64;
        }
        let d2 = self.dist.to(g, &sw).get(t as usize).copied().unwrap_or(FAR);
        if d2 < FAR {
            (SWITCH_DIST + d2) as f64
        } else {
            FAR as f64
        }
    }

    fn score(&mut self, g: &Game, base: &Game) -> f64 {
        match self.kind {
            Kind::Single => {
                let lost = (base.frogger.player[0].lives as i32 - g.frogger.player[0].lives as i32).max(0) as f64;
                let saved = g.babyfrog.baby_list.iter().filter(|b| b.is_saved != 0).count() as f64;
                let t = self.targets(g);
                let d = self.player_dist(g, 0, &t);
                let dead = if is_dead(g, 0) { 1.0 } else { 0.0 };
                saved * 1e6 - (lost + dead) * 3e5 - d * 1000.0
            }
            Kind::Race => {
                let mut s = 0.0;
                let fin = self.finish.clone();
                for p in 0..self.n {
                    let m = &g.multi.mpl[p];
                    let w = if p == 0 { 1.5 } else { 1.0 };
                    s += 1e7 * m.wins as f64 * if p == 0 { 2.0 } else { 1.0 };
                    if g.game_state.mode != INGAME_MODE {
                        continue;
                    }
                    let d = if m.ready != 0 { 0.0 } else { self.player_dist(g, p, &fin) };
                    let dead = if is_dead(g, p) { 1.0 } else { 0.0 };
                    s += w * (m.ready as f64 * 1e5 - d * 1000.0 - m.penalty as f64 * 50.0 - dead * 2e4);
                }
                s
            }
            Kind::Battle => {
                let mut s = 1e7 * g.multi.mpl[0].wins as f64;
                for p in 1..self.n {
                    s -= 5e6 * g.multi.mpl[p].wins as f64;
                }
                if g.game_state.mode == INGAME_MODE {
                    s += if is_dead(g, 0) { 0.0 } else { 1e5 };
                    for p in 1..self.n {
                        s -= if is_dead(g, p) { 0.0 } else { 1e4 };
                    }
                    // Stay away from walls: prefer tiles with many neighbours.
                    if let Some(t) = g.frogmove.curr_tile[0] {
                        s += 100.0 * g.map.tile(t).tile_ptrs.iter().flatten().count() as f64;
                    }
                }
                s
            }
            Kind::Collect => {
                let mut s = 1e7 * g.multi.mpl[0].wins as f64;
                for p in 1..self.n {
                    s -= 5e6 * g.multi.mpl[p].wins as f64;
                }
                if g.game_state.mode == INGAME_MODE {
                    let t = baby_targets(g);
                    for &p in &self.planned.clone() {
                        let w = if p == 0 { 1.0 } else { 0.3 };
                        let d = self.player_dist(g, p, &t);
                        s += w * (1e5 * g.multi.mpl[p].score as f64 - d * 1000.0);
                    }
                }
                s
            }
        }
    }

    /// A random input sequence of `len` ticks for player `p` (held state per tick).
    fn random_seq(rng: &mut Rng, len: usize, kind: Kind) -> Vec<u16> {
        let mut v = Vec::with_capacity(len);
        while v.len() < len {
            let roll = rng.range(0, 99);
            if roll < 12 && kind != Kind::Battle {
                // Mostly short pauses; sometimes a long one (lifts, babies on moving paths).
                let w = if roll < 2 { rng.range(60, 300) } else { rng.range(4, 24) };
                v.extend(std::iter::repeat_n(0, w as usize));
                continue;
            }
            let mut b = DIRS[rng.range(0, 3) as usize];
            if kind != Kind::Battle && (88..96).contains(&roll) {
                b |= PAD_CROSS; // superhop
            } else if kind != Kind::Battle && roll >= 96 {
                b = PAD_CIRCLE; // tongue (grapple points, items)
            }
            let hold = rng.range(2, 4);
            v.extend(std::iter::repeat_n(b, hold as usize));
            let gap = if kind == Kind::Battle { rng.range(4, 30) } else { rng.range(1, 10) };
            v.extend(std::iter::repeat_n(0, gap as usize));
        }
        v.truncate(len);
        v
    }

    /// The frog the archive search drives: player 0, or in a race the first frog that
    /// has not finished yet (the others idle; left behind, they die off screen and
    /// respawn next to the leader, so each frog's run is a single-frog problem).
    fn active(&self, g: &Game) -> usize {
        match self.kind {
            Kind::Race => (0..self.n).find(|&p| g.multi.mpl[p].ready == 0).unwrap_or(0),
            _ => 0,
        }
    }

    /// Archive search progress: (level, distance) — higher level first, then nearer.
    fn ge_progress(&mut self, g: &Game) -> (u32, f64) {
        match self.kind {
            Kind::Single => {
                let saved = g.babyfrog.baby_list.iter().filter(|b| b.is_saved != 0).count() as u32;
                let t = self.targets(g);
                (self.wp as u32 * 8 + saved, self.player_dist(g, 0, &t))
            }
            Kind::Race => {
                // Rounds (draws included) rather than wins, so a drawn round is progress too.
                let wins = self.rounds;
                let ready = (0..self.n).filter(|&p| g.multi.mpl[p].ready != 0).count() as u32;
                // All ready = the round just ended (already counted in `rounds`); the level
                // must not drop when the next round resets `ready`.
                let ready = if ready == self.n as u32 { 0 } else { ready };
                let a = self.active(g);
                let fin = self.finish.clone();
                let d = if g.game_state.mode == INGAME_MODE && g.multi.mpl[a].ready == 0 { self.player_dist(g, a, &fin) } else { 0.0 };
                (wins * 8 + ready, d)
            }
            _ => {
                let t = baby_targets(g);
                ((g.multi.mpl[0].wins.max(0) as u32) * 8 + g.multi.mpl[0].score.max(0) as u32, self.player_dist(g, 0, &t))
            }
        }
    }

    /// Dead for the archive search: in collect a frog that picked up a baby waits on the
    /// respawn timer, which is progress, not a death.
    fn ge_dead(&self, g: &Game, p: usize) -> bool {
        match self.kind {
            Kind::Collect => g.frogger.player[p].frog_state & FROGSTATUS_ISDEAD != 0,
            _ => is_dead(g, p),
        }
    }

    fn ge_key(&mut self, g: &Game) -> (u32, u32, u64) {
        match self.kind {
            Kind::Single => {
                let (a, b, mut h) = cell_key(g);
                // On a target tile without the pickup (a baby moving on a path, e.g.
                // up and down): waiting there is progress, so time makes new cells.
                let t = self.targets(g);
                if !t.is_empty() && self.player_dist(g, 0, &t) == 0.0 {
                    h = (h ^ u64::from(g.act_frame_count / 32 + 1)).wrapping_mul(0x100_0000_01b3);
                }
                (a, b, h ^ (self.wp as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15))
            }
            _ => {
                let a = self.active(g);
                let (lvl, _) = self.ge_progress(g);
                let tile = g.frogmove.curr_tile[a].map_or(u32::MAX, |t| t.0);
                let mut mask = 0u64;
                for (i, b) in g.babyfrog.baby_list.iter().enumerate() {
                    if b.is_saved != 0 {
                        mask |= 1 << i;
                    }
                }
                // Countdowns, round ends and win screens: nothing moves, so time itself
                // must make new cells (else no candidate gets past them).
                let wait = if g.multi.started != 2 || g.game_state.mode != INGAME_MODE || g.multi.end_timer.time != 0 {
                    u64::from(g.act_frame_count / 32 + 1)
                } else {
                    0
                };
                (tile, lvl, (a as u64) << 32 | mask | u64::from(g.game_state.mode) << 40 | wait << 44)
            }
        }
    }

    fn ge_input(&mut self, g: &Game, b: u16, t: usize) -> [u16; 4] {
        let mut held = if self.random.is_empty() { [0; 4] } else { self.rival_input(t) };
        held[self.active(g)] = b;
        held
    }

    fn rival_input(&mut self, t: usize) -> [u16; 4] {
        while self.rival_tape.len() <= t {
            let mut chunk: Vec<[u16; 4]> = vec![[0; 4]; 256];
            for &p in &self.random {
                let s = Self::random_seq(&mut self.rival_rng, 256, self.kind);
                for (i, b) in s.into_iter().enumerate() {
                    chunk[i][p] = b;
                }
            }
            self.rival_tape.extend(chunk);
        }
        self.rival_tape[t]
    }
}

/// Archive cell of [`go_explore`]: a distinct situation (frog tile, babies saved, script
/// flags) with the first game state that reached it.
struct Cell {
    state: Box<Game>,
    prev: [u16; 4],
    t: usize,
    parent: Option<usize>,
    /// Input from the parent's state to this one.
    seg: Vec<[u16; 4]>,
    visits: u32,
    saved: u32,
    dist: f64,
    /// Multiplayer rounds finished (wins and draws) on the way to this cell.
    rounds: u32,
    /// Waypoint stage reached on the way to this cell.
    wp: usize,
}

fn cell_key(g: &Game) -> (u32, u32, u64) {
    let tile = g.frogmove.curr_tile[0].map_or(u32::MAX, |t| t.0);
    let mut mask = 0u32;
    for (i, b) in g.babyfrog.baby_list.iter().enumerate() {
        if b.is_saved != 0 {
            mask |= 1 << i;
        }
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |v: u64| h = (h ^ v).wrapping_mul(0x100_0000_01b3);
    for &b in g.script.script_flags.iter() {
        mix(u64::from(b));
    }
    for &c in g.script.script_counters.iter() {
        mix(c as u16 as u64);
    }
    // Gates opened or closed by switches (`EV_SETTILE_P`).
    for (i, t) in g.map.tiles.iter().enumerate() {
        if t.state == tilestate::BARRED {
            mix(i as u64 + 0x10_0000);
        }
    }
    // On a platform the tile alone does not tell where the frog is (moving platforms,
    // lifts): add the platform and the coarse position.
    if let Some(p) = g.platform.curr_platform[0] {
        let pos = g.frog_position(0);
        mix(0x1000 + u64::from(p.0));
        mix((pos.vx >> 8) as u64);
        mix((pos.vy >> 8) as u64);
        mix((pos.vz >> 8) as u64);
    }
    (tile, mask, h)
}

/// Go-Explore over the deterministic simulation (single player): keep an archive of cells
/// with their game states, repeatedly return to a promising, rarely tried cell and explore
/// from it with random input; new cells join the archive. Selection favours the most
/// babies saved and the tile-graph distance to the next baby. Returns the input from the
/// start to the tick the level is won (end-of-level timer started).
/// Result of [`go_explore`].
enum Explore {
    Found(Vec<[u16; 4]>),
    /// Won, but under the level's par time by this many ticks: the level-complete screen
    /// would then ask for a name (keyboard text entry, which neither the oracle's pad
    /// script nor `scenario_trace` can type), so the run could not continue and save.
    TooFast(u32),
    GaveUp,
}

/// Follows a won game (no input) to the level-complete screen: ticks under par, if any.
fn under_par(g: &Game) -> Option<u32> {
    let mut g = g.clone();
    for _ in 0..3000 {
        if g.game_state.mode == LEVELCOMPLETE_MODE {
            if g.frontend.grade == 0 {
                let w = g.frogger.player[0].world_num as usize;
                let l = g.frogger.player[0].level_num as usize;
                let par = g.layout.world_visual_data[w].level_visual_data[l].par_time;
                let t = g.frontend.time_for_level as u32;
                return Some(par.saturating_sub(t) * 6 + 6);
            }
            return None;
        }
        if !step(&mut g, [0; 4], [0; 4]) {
            return None;
        }
    }
    None
}

fn go_explore(pl: &mut Planner, g0: &Game, o: &PlanOpts) -> Explore {
    let max_cells: usize = std::env::var("F2_PLAN_MAX_CELLS").ok().and_then(|s| s.parse().ok()).unwrap_or(6000);
    let sim_budget: u64 = std::env::var("F2_PLAN_SIM_BUDGET").ok().and_then(|s| s.parse().ok()).unwrap_or(20_000_000);
    let mut index: HashMap<(u32, u32, u64), usize> = HashMap::new();
    pl.wp = 0;
    let (l0, d0) = pl.ge_progress(g0);
    let mut cells = vec![Cell { state: Box::new(g0.clone()), prev: [0; 4], t: 0, parent: None, seg: Vec::new(), visits: 0, saved: l0, dist: d0, rounds: 0, wp: 0 }];
    index.insert(pl.ge_key(g0), 0);
    let mut sim: u64 = 0;
    let mut iter = 0u64;
    let path_of = |cells: &Vec<Cell>, mut i: usize, tail: Vec<[u16; 4]>| -> Vec<[u16; 4]> {
        let mut segs = vec![tail];
        loop {
            segs.push(cells[i].seg.clone());
            match cells[i].parent {
                Some(p) => i = p,
                None => break,
            }
        }
        segs.into_iter().rev().flatten().collect()
    };
    while sim < sim_budget {
        iter += 1;
        // Select: best progress first, damped by visits.
        let max_saved = cells.iter().map(|c| c.saved).max().unwrap_or(0);
        let min_d = cells.iter().filter(|c| c.saved == max_saved).map(|c| c.dist).fold(f64::MAX, f64::min);
        let weights: Vec<f64> = cells
            .iter()
            .map(|c| {
                let lvl = if c.saved == max_saved { 1.0 } else { 0.15f64.powi((max_saved - c.saved) as i32) };
                let near = (-(c.dist - min_d).max(0.0) / 20.0).exp().max(0.02);
                lvl * near / (1.0 + c.visits as f64)
            })
            .collect();
        let total: f64 = weights.iter().sum();
        let mut r = (pl.rng.next() as f64 / u64::MAX as f64) * total;
        let mut i = 0;
        for (k, w) in weights.iter().enumerate() {
            r -= w;
            if r <= 0.0 {
                i = k;
                break;
            }
        }
        cells[i].visits += 1;
        let mut g = (*cells[i].state).clone();
        let mut prev = cells[i].prev;
        let mut t = cells[i].t;
        let lives0 = g.frogger.player[0].lives;
        let active0 = pl.active(&g);
        pl.rounds = cells[i].rounds;
        pl.wp = cells[i].wp;
        let saved0 = g.babyfrog.baby_list.iter().filter(|b| b.is_saved != 0).count() as u32;
        let len = pl.rng.range(40, 160) as usize;
        let seq = Planner::random_seq(&mut pl.rng, len, if pl.kind == Kind::Battle { Kind::Battle } else { Kind::Single });
        let mut parent = i;
        let mut seg: Vec<[u16; 4]> = Vec::new();
        for b in seq {
            let held = pl.ge_input(&g, b, t);
            let n = pl.n;
            let all_ready = |g: &Game| g.game_state.mode == INGAME_MODE && (0..n).all(|p| g.multi.mpl[p].ready != 0);
            let before = all_ready(&g);
            let ok = step(&mut g, prev, held);
            if !before && all_ready(&g) {
                pl.rounds += 1; // a race round ended (won or drawn)
            }
            sim += 1;
            prev = held;
            t += 1;
            seg.push(held);
            if !ok {
                break;
            }
            if pl.kind == Kind::Single && pl.wp < pl.waypoints.len() && g.frogmove.curr_tile[0].is_some_and(|c| pl.waypoints[pl.wp].contains(&c.0)) && !is_dead(&g, 0) {
                pl.wp += 1;
                eprintln!("plan: waypoint stage {} reached at t={t}", pl.wp);
            }
            if pl.goal(&g) {
                eprintln!("plan: go-explore goal at t={t} after {iter} iterations, {sim} sim ticks, {} cells", cells.len());
                if pl.kind != Kind::Single {
                    return Explore::Found(path_of(&cells, parent, seg));
                }
                if let Some(d) = under_par(&g) {
                    eprintln!("plan: under par by {d} ticks");
                    return Explore::TooFast(d);
                }
                return Explore::Found(path_of(&cells, parent, seg));
            }
            // A death right after a rescue (a baby on a deadly tile) still counts: go on
            // through the respawn.
            let sacrificed = pl.kind == Kind::Single && g.frogger.player[0].lives > 1 && g.babyfrog.baby_list.iter().filter(|b| b.is_saved != 0).count() as u32 > saved0;
            if sacrificed {
            } else if (pl.kind == Kind::Single && g.frogger.player[0].lives < lives0) || (pl.active(&g) == active0 && pl.ge_dead(&g, active0)) {
                break;
            }
            if t as u32 >= o.budget {
                break;
            }
            let key = pl.ge_key(&g);
            if !index.contains_key(&key) && cells.len() < max_cells && !(pl.kind == Kind::Single && is_dead(&g, 0)) {
                let (saved, dist) = pl.ge_progress(&g);
                cells.push(Cell { state: Box::new(g.clone()), prev, t, parent: Some(parent), seg: std::mem::take(&mut seg), visits: 0, saved, dist, rounds: pl.rounds, wp: pl.wp });
                parent = cells.len() - 1;
                index.insert(key, parent);
            }
        }
        if iter % 2000 == 0 {
            eprintln!("plan: go-explore iter {iter} sim {sim} cells {} best saved {max_saved} dist {min_d}", cells.len());
        }
    }
    eprintln!("plan: go-explore gave up ({sim} sim ticks, {} cells)", cells.len());
    if std::env::var_os("F2_PLAN_DEBUG").is_some() {
        // The most advanced cell: where the frog is and what is left.
        let best = (0..cells.len()).max_by(|&a, &b| (cells[a].saved, -cells[a].dist).partial_cmp(&(cells[b].saved, -cells[b].dist)).unwrap()).unwrap_or(0);
        let g = &cells[best].state;
        let p = &g.frogger.player[0];
        eprintln!("plan: best cell t={} tile {:?} plat {:?} pos {:?} frog_state {:#x} lives {}", cells[best].t, g.frogmove.curr_tile[0], g.platform.curr_platform[0], g.frog_position(0), p.frog_state, p.lives);
        for b in g.babyfrog.baby_list.iter().take(g.map.num_babies.max(0) as usize).filter(|b| b.is_saved == 0) {
            if let Some(e) = b.enemy.and_then(|e| g.enemies.enemy_list.array.get(e.idx())) {
                let ap = b.baby.and_then(|a| g.actor2.actors.get(a)).map(|a| (a.actor.position, a.radius));
                eprintln!("plan:   unsaved baby active {} flags {:#x} tile {:?} state {:?} actor {:?}", e.active, e.flags, e.in_tile, e.in_tile.map(|t| g.map.tile(t).state), ap);
            }
        }
    }
    Explore::GaveUp
}

/// Simulates `seq` from a clone of `base`.
fn simulate(pl: &mut Planner, base: &Game, prev: [u16; 4], seq: &[[u16; 4]]) -> Eval {
    let mut g = base.clone();
    let mut prev = prev;
    let mut new_tiles: Vec<u32> = Vec::new();
    for (i, &held) in seq.iter().enumerate() {
        if !step(&mut g, prev, held) {
            // Back in the frontend: the multiplayer match end, or a single-player game over.
            return if pl.kind == Kind::Single { Eval { score: -1e12, goal: None } } else { Eval { score: 1e12 - i as f64, goal: Some(i) } };
        }
        prev = held;
        if pl.goal(&g) {
            return Eval { score: 1e12 - i as f64, goal: Some(i) };
        }
        if pl.explore > 0.0 {
            if let Some(t) = g.frogmove.curr_tile[0] {
                if !pl.visited.get(t.idx()).copied().unwrap_or(true) && !new_tiles.contains(&t.0) && !is_dead(&g, 0) {
                    new_tiles.push(t.0);
                }
            }
        }
    }
    let alive_bonus = if is_dead(&g, 0) { 0.0 } else { 1.0 };
    Eval { score: pl.score(&g, base) + pl.explore * new_tiles.len() as f64 * alive_bonus, goal: None }
}

pub fn plan(g0: Game, players: usize, o: &PlanOpts) -> (Vec<(u32, u16, usize)>, String) {
    let kind = if g0.game_state.multi != MULTILOCAL {
        Kind::Single
    } else {
        match g0.multi.multiplayer_mode {
            MULTIMODE_RACE => Kind::Race,
            MULTIMODE_BATTLE => Kind::Battle,
            MULTIMODE_COLLECT => Kind::Collect,
            _ => Kind::Single,
        }
    };
    let n = if kind == Kind::Single { 1 } else { players };
    let rivals = match (o.rivals, kind) {
        (Rivals::Auto, Kind::Battle | Kind::Collect) => Rivals::Random,
        (Rivals::Auto, _) => Rivals::Plan,
        (r, _) => r,
    };
    let planned: Vec<usize> = (0..n).filter(|&p| p == 0 || rivals == Rivals::Plan).collect();
    let random: Vec<usize> = (1..n).filter(|_| rivals == Rivals::Random).collect();
    let finish: Vec<u32> = g0.map.tiles.iter().enumerate().filter(|(_, t)| t.state == tilestate::FINISHLINE).map(|(i, _)| i as u32).collect();
    let mut pl = Planner {
        kind,
        n,
        planned,
        random,
        dist: Dist::new(&g0),
        finish,
        rng: Rng(o.seed ^ 0x504c_414e),
        rival_rng: Rng(o.seed.wrapping_mul(0xa076_1d64_78bd_642f) ^ 0x5249_5641),
        rival_tape: Vec::new(),
        visited: vec![false; g0.map.tiles.len()],
        explore: 0.0,
        rounds: 0,
        end_targets: end_level_targets(&g0),
        waypoints: o.waypoints.clone(),
        wp: 0,
        switches: gate_switches(&g0),
    };
    if std::env::var_os("F2_PLAN_DEBUG").is_some() {
        let t = pl.targets(&g0);
        let d: Vec<u32> = t.iter().map(|&x| pl.dist.to(&g0, &[x]).get(g0.frogmove.curr_tile[0].map_or(0, |c| c.idx())).copied().unwrap_or(FAR)).collect();
        let pd = pl.player_dist(&g0, 0, &pl.targets(&g0));
        eprintln!("plan: gate switches {:?} start dist {pd}", pl.switches.iter().map(|s| (s.gate, s.flag, s.tiles.len())).collect::<Vec<_>>());
        if let (Ok(v), Some(start)) = (std::env::var("F2_PLAN_DEBUG_ROUTE"), g0.frogmove.curr_tile[0]) {
            // BFS over the raw tile_ptrs to tile v, with each hop as the planner models it.
            let target: usize = v.parse().unwrap_or(0);
            let n = g0.map.tiles.len();
            let mut prev = vec![usize::MAX; n];
            let mut q = std::collections::VecDeque::from([start.idx()]);
            prev[start.idx()] = start.idx();
            while let Some(c) = q.pop_front() {
                for nb in g0.map.tiles[c].tile_ptrs.iter().flatten() {
                    if nb.idx() < n && prev[nb.idx()] == usize::MAX {
                        prev[nb.idx()] = c;
                        q.push_back(nb.idx());
                    }
                }
            }
            let mut path = vec![target];
            while prev.get(*path.last().unwrap()).is_some_and(|&p| p != usize::MAX && p != *path.last().unwrap()) {
                path.push(prev[*path.last().unwrap()]);
            }
            path.reverse();
            for w in path.windows(2) {
                let k = g0.map.tiles[w[0]].tile_ptrs.iter().position(|p| p.is_some_and(|p| p.idx() == w[1])).unwrap_or(9);
                let t = g0.map.tile(TileId(w[1] as u32));
                eprintln!("plan:   hop {} -> {} (state {} y {} normal {:?}) model {:?}", w[0], w[1], t.state, t.centre.vy, (g0.map.tiles[w[0]].normal.vx, g0.map.tiles[w[0]].normal.vy, g0.map.tiles[w[0]].normal.vz), if k < 4 { hop_edge(&g0, w[0], k) } else { None });
            }
        }
        if std::env::var_os("F2_PLAN_DEBUG_SCRIPT").is_some() {
            for Switch { gate: gt, tiles: ts, .. } in &pl.switches {
                let c = |t: u32| {
                    let x = g0.map.tile(TileId(t));
                    (t, x.state, x.centre.vx, x.centre.vy, x.centre.vz, x.tile_ptrs.iter().flatten().map(|p| p.0).collect::<Vec<_>>())
                };
                eprintln!("plan:   gate {:?} switch tiles {:?}", c(*gt), ts.iter().map(|&t| c(t)).collect::<Vec<_>>());
                // Greedy route from the start (tile, state, height).
                let d = pl.dist.to(&g0, ts).clone();
                let mut cur = g0.frogmove.curr_tile[0].map_or(0, |t| t.0);
                let mut route = vec![(cur, g0.map.tile(TileId(cur)).state, g0.map.tile(TileId(cur)).centre.vy)];
                while d[cur as usize] > 0 && d[cur as usize] < FAR && route.len() < 200 {
                    let nx = g0.map.tiles[cur as usize].tile_ptrs.iter().flatten().map(|p| p.0).chain(pl.dist.rev.iter().enumerate().filter(|(_, r)| r.iter().any(|&(f, _)| f == cur)).map(|(i, _)| i as u32)).min_by_key(|&n| d[n as usize]);
                    match nx {
                        Some(n) if d[n as usize] < d[cur as usize] => cur = n,
                        _ => break,
                    }
                    route.push((cur, g0.map.tile(TileId(cur)).state, g0.map.tile(TileId(cur)).centre.vy));
                }
                eprintln!("plan:   route {route:?}");
            }
        }
        eprintln!("plan: kind {kind:?} tiles {} (+{} map edges) frog tile {:?} baby tiles {t:?} dist {d:?} finish {}", g0.map.tiles.len(), pl.dist.extra_edges, g0.frogmove.curr_tile[0], pl.finish.len());
        if !pl.finish.is_empty() {
            let fin = pl.finish.clone();
            let d: Vec<u32> = (0..n).map(|p| g0.frogmove.curr_tile[p].map_or(FAR, |c| pl.dist.to(&g0, &fin)[c.idx()])).collect();
            let mut states = std::collections::BTreeMap::new();
            for t in &g0.map.tiles {
                *states.entry(t.state).or_insert(0) += 1;
            }
            eprintln!("plan: finish dist {d:?} tile states {states:?}");
        }
        if std::env::var_os("F2_PLAN_DEBUG_SCRIPT").is_some() {
            if let Some(s) = &g0.script.script_buffer {
                eprintln!("plan: script {:#?}", s.commands);
            }
        }
        for b in g0.babyfrog.baby_list.iter().take(g0.map.num_babies.max(0) as usize) {
            if let Some(e) = b.enemy.and_then(|e| g0.enemies.enemy_list.array.get(e.idx())) {
                let ap = b.baby.and_then(|a| g0.actor2.actors.get(a)).map(|a| a.actor.position);
                let tc = e.in_tile.map(|t| g0.map.tile(t).centre);
                eprintln!("plan:   baby enemy active {} flags {:#x} visible {} tile {:?} state {:?} pos {:?} tile centre {:?}", e.active, e.flags, e.visible, e.in_tile, e.in_tile.map(|t| g0.map.tile(t).state), ap, tc);
            }
        }
    }

    let mut g = g0;
    let mut prev = [0u16; 4];
    let mut recs: Vec<(u32, u16, usize)> = Vec::new();
    let mut t = 0usize; // committed ticks
    let mut prev_best: Vec<[u16; 4]> = Vec::new();
    let mut best_seen = f64::MIN;
    let mut stall = 0u32;
    let mut goal_tick: Option<usize> = None;
    let mut alive = true;

    // Commits one tick of input.
    let commit = |g: &mut Game, held: [u16; 4], prev: &mut [u16; 4], recs: &mut Vec<(u32, u16, usize)>, t: &mut usize| -> bool {
        for p in 0..4 {
            if held[p] != prev[p] {
                recs.push((g.act_frame_count, held[p], p));
            }
        }
        let ok = step(g, *prev, held);
        *prev = held;
        *t += 1;
        ok
    };

    // Single player: archive search first (puzzles, platforms); the receding-horizon loop
    // below only runs when it gives up.
    if matches!(kind, Kind::Single | Kind::Race | Kind::Collect) {
        // Under par: wait at the start (the whole search then runs from the later start).
        let mut wait = 0u32;
        let mut found = None;
        for _ in 0..5 {
            let mut root = g.clone();
            for _ in 0..wait {
                step(&mut root, [0; 4], [0; 4]);
            }
            pl.visited = vec![false; root.map.tiles.len()];
            match go_explore(&mut pl, &root, o) {
                Explore::Found(p) => {
                    let mut v = vec![[0u16; 4]; wait as usize];
                    v.extend(p);
                    found = Some(v);
                    break;
                }
                Explore::TooFast(d) => wait += d + 120,
                Explore::GaveUp => break,
            }
        }
        if let Some(path) = found {
            for held in path {
                if !commit(&mut g, held, &mut prev, &mut recs, &mut t) {
                    alive = false;
                    break;
                }
            }
            if alive && pl.goal(&g) {
                goal_tick = Some(t);
            } else {
                eprintln!("plan: go-explore path did not replay to the goal");
            }
        }
    }

    while alive && goal_tick.is_none() && (t as u32) < o.budget {
        let level = (stall / 6).min(3);
        let h = 48usize << level;
        let k = 16usize + 8 * level as usize;
        let c = 16usize.max(h / 4);
        // Rival tape for this window.
        let rival: Vec<[u16; 4]> = (0..h).map(|i| pl.rival_input(t + i)).collect();
        let mut cands: Vec<Vec<[u16; 4]>> = Vec::with_capacity(k);
        cands.push(rival.clone()); // no input from the planned players
        if !prev_best.is_empty() {
            let mut c1 = prev_best.clone();
            c1.resize(h, [0; 4]);
            for (i, r) in rival.iter().enumerate() {
                for &p in &pl.random {
                    c1[i][p] = r[p];
                }
            }
            cands.push(c1);
        }
        // Several planned frogs: half of the candidates vary one frog only (the others
        // keep the previous winner), so a good move of one frog is not lost to the noise
        // of the others.
        let planned = pl.planned.clone();
        let mut ci = 0usize;
        while cands.len() < k {
            ci += 1;
            let only = if planned.len() > 1 && ci % 2 == 0 { Some(planned[(ci / 2) % planned.len()]) } else { None };
            let mut s = rival.clone();
            if only.is_some() && !prev_best.is_empty() {
                for (i, row) in s.iter_mut().enumerate() {
                    for &p in &planned {
                        row[p] = prev_best.get(i).map_or(0, |r| r[p]);
                    }
                }
            }
            for &p in &planned {
                if only.is_some_and(|o| o != p) {
                    continue;
                }
                let seq = Planner::random_seq(&mut pl.rng, h, kind);
                for (i, b) in seq.into_iter().enumerate() {
                    s[i][p] = b;
                }
            }
            cands.push(s);
        }
        let mut best = (f64::MIN, 0usize, None);
        for (ci, cand) in cands.iter().enumerate() {
            let e = simulate(&mut pl, &g, prev, cand);
            if e.score > best.0 {
                best = (e.score, ci, e.goal);
            }
        }
        let win = cands.swap_remove(best.1);
        let take = match best.2 {
            Some(gt) => gt + 1,
            None => c.min(win.len()),
        };
        for &held in &win[..take] {
            if !commit(&mut g, held, &mut prev, &mut recs, &mut t) {
                alive = false;
                break;
            }
            if let Some(tile) = g.frogmove.curr_tile[0] {
                if let Some(v) = pl.visited.get_mut(tile.idx()) {
                    *v = true;
                }
            }
        }
        if !alive && kind == Kind::Single {
            break; // game over
        }
        if !alive || pl.goal(&g) {
            goal_tick = Some(t);
            break;
        }
        prev_best = win[take..].to_vec();
        let now = pl.score(&g, &g.clone());
        if now > best_seen + 1.0 {
            best_seen = now;
            stall = 0;
        } else {
            stall += 1;
        }
        // Stuck (a puzzle, a route the distance does not see): reward new ground.
        pl.explore = if stall >= 8 { 1500.0 } else { 0.0 };
        if t % 600 < take {
            eprintln!("plan: t={t} score={now:.0} stall={stall} afc={}", g.act_frame_count);
        }
    }

    // After the goal: single player continues through the level-complete screen (CROSS
    // until the game saves and moves on); multiplayer idles to the frontend.
    let mut end_note = String::new();
    if let Some(_) = goal_tick {
        let start_level = (g.frogger.player[0].world_num, g.frogger.player[0].level_num);
        let mut saved_at: Option<usize> = None;
        let mut tail = 0usize;
        let mut since_cross = 0usize;
        while alive && (t as u32) < o.budget + 4000 {
            let mut held = if pl.random.is_empty() { [0u16; 4] } else { pl.rival_input(t) };
            if kind == Kind::Single {
                let in_complete = g.game_state.mode == LEVELCOMPLETE_MODE;
                if in_complete && saved_at.is_none() {
                    since_cross += 1;
                    if since_cross % 40 < 3 {
                        held[0] = PAD_CROSS;
                    }
                }
            } else if g.multi.match_winner != -1 && g.game_state.mode != INGAME_MODE {
                // DoEndMulti's menu: DOWN to "quit" (menuOption 1), then CROSS -> frontend.
                match g.frontend.menu_option {
                    0 => held[0] = PAD_DOWN,
                    1 if prev[0] == 0 => held[0] = PAD_CROSS,
                    _ => {}
                }
            }
            if !commit(&mut g, held, &mut prev, &mut recs, &mut t) {
                end_note.push_str(" frontend");
                break;
            }
            if kind == Kind::Single {
                let lvl = (g.frogger.player[0].world_num, g.frogger.player[0].level_num);
                if saved_at.is_none() && (lvl != start_level || g.game_state.mode == INGAME_MODE && t > goal_tick.unwrap() + 600) {
                    saved_at = Some(t);
                    end_note.push_str(&format!(" moved-on@{t} level {}-{}", lvl.0, lvl.1));
                }
                if saved_at.is_some() {
                    tail += 1;
                    if tail > 240 {
                        break;
                    }
                }
            }
        }
        // Release everything at the end.
        for p in 0..4 {
            if prev[p] != 0 {
                recs.push((g.act_frame_count, 0, p));
            }
        }
    }
    let wins: Vec<i16> = (0..n).map(|p| g.multi.mpl[p].wins).collect();
    let summary = format!(
        "kind {:?} players {n} goal {} ticks {t} afc {} babies {} wins {:?} match_winner {}{}",
        kind,
        goal_tick.map_or("no".to_string(), |x| format!("@{x}")),
        g.act_frame_count,
        g.babyfrog.baby_list.iter().filter(|b| b.is_saved != 0).count(),
        wins,
        g.multi.match_winner,
        end_note
    );
    (recs, summary)
}
