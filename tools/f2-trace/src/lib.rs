//! Per-tick game-state traces of Frogger 2.
//!
//! The retail oracle (`crates/f2fix`, `trace.enable=1`) writes one
//! [`TraceFrame`] per `GameLoop` tick as a JSON line; the Rust simulation
//! (`f2-game`) emits the same schema so the two can be compared with
//! [`diff`] (`f2-trace diff a.jsonl b.jsonl`).
//!
//! Conventions (schema version [`SCHEMA_VERSION`]):
//! * one frame = state *after* `GameLoop` returned for that tick, in level
//!   ticks only by default (`trace.from=level`): the first frame is the first
//!   tick of the demo/level (`act_frame_count == 1`);
//! * positions are the game's own integer units (`SVECTOR`, `FVECTOR` 20.12);
//! * pointers are indices: tiles are `(ptr - firstTile) / 68`, -1 for NULL;
//!   enemies, platforms and garibs are listed in their linked-list order
//!   (`head.next` first);
//! * field order is fixed: serialising with [`TraceFrame::to_json_line`]
//!   gives byte-identical lines for identical states.

pub mod binfmt;
pub mod diff;

use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};

/// Written as `"v"` in every frame.
pub const SCHEMA_VERSION: u32 = 1;

/// One player (`player[i]`, `frog[i]`, `currTile[i]`), `i < NUM_FROGS`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerState {
    /// `frog[i]->actor->position` (SVECTOR xyz); `None` when `frog[i]` is NULL.
    pub pos: Option<[i16; 3]>,
    pub frog_state: u32,
    pub curr_tile: i32,
    pub lives: i16,
    pub health: u8,
    pub death_by: u8,
    pub can_jump: u8,
    pub has_jumped: u8,
    pub is_super_hopping: u8,
    pub is_sinking: u8,
    pub num_spawn: i16,
    /// `player[i].safe.time`.
    pub safe_time: u16,
    /// `player[i].dead.time`.
    pub dead_time: u16,
    pub jump_time: i32,
    pub height_jumped: i32,
}

/// One `ENEMY` of `enemyList`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnemyState {
    pub uid: i32,
    pub active: u8,
    /// `nmeActor->actor->position`.
    pub pos: Option<[i16; 3]>,
    pub flags: u32,
    pub is_waiting: i16,
    pub speed: i32,
    pub in_tile: i32,
}

/// One `PLATFORM` of `platformList`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformState {
    pub uid: i32,
    pub active: u8,
    /// `pltActor->actor->position`.
    pub pos: Option<[i16; 3]>,
    pub flags: u32,
    pub is_waiting: i16,
    pub curr_speed: i32,
    pub in_tile: [i32; 2],
}

/// One `GARIB` of `garibList`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GaribState {
    /// `garib->type`.
    pub kind: u8,
    pub active: i8,
    pub pos: [i16; 3],
}

/// Game state after one `GameLoop` tick.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceFrame {
    /// Schema version ([`SCHEMA_VERSION`]).
    pub v: u32,
    /// Line index in the trace.
    pub seq: u32,
    pub act_frame_count: u32,
    pub frame_count: i32,
    pub game_speed: i32,
    /// `gameState.mode` (define.h: 0 frontend, 2 ingame, 3 level complete, 9 pause, 10 demo, ...).
    pub mode: u32,
    pub multi: u8,
    pub single: u8,
    /// `player[0].worldNum` / `levelNum`.
    pub world: u8,
    pub level: u8,
    pub rseed: u32,
    /// `padData.digital[0..4]` seen by this tick's logic (PSX button bits).
    pub pads: [u16; 4],
    pub players: Vec<PlayerState>,
    pub enemies: Vec<EnemyState>,
    pub platforms: Vec<PlatformState>,
    /// `garibList.count` / `garibList.total`.
    pub garib_count: i16,
    pub garib_total: i16,
    pub garibs: Vec<GaribState>,
    pub babies_saved: i32,
    pub num_babies: i32,
    /// `endLevelTimer.time`.
    pub end_level_timer: u16,
    /// `currCamSource` / `currCamTarget` (FVECTOR, 20.12).
    pub cam_source: [i32; 3],
    pub cam_target: [i32; 3],
}

impl TraceFrame {
    /// The canonical JSON line (no newline).
    pub fn to_json_line(&self) -> String {
        serde_json::to_string(self).expect("TraceFrame serialises")
    }

    pub fn from_json_line(s: &str) -> Result<TraceFrame, String> {
        serde_json::from_str(s).map_err(|e| e.to_string())
    }
}

/// Read a JSON-lines trace (blank lines skipped).
pub fn read_jsonl<R: BufRead>(r: R) -> Result<Vec<TraceFrame>, String> {
    let mut out = Vec::new();
    for (i, line) in r.lines().enumerate() {
        let line = line.map_err(|e| format!("line {}: {e}", i + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        out.push(TraceFrame::from_json_line(&line).map_err(|e| format!("line {}: {e}", i + 1))?);
    }
    Ok(out)
}

/// Write a JSON-lines trace.
pub fn write_jsonl<W: Write>(mut w: W, frames: &[TraceFrame]) -> std::io::Result<()> {
    for f in frames {
        w.write_all(f.to_json_line().as_bytes())?;
        w.write_all(b"\n")?;
    }
    Ok(())
}

/// Read a trace file: binary if it starts with the [`binfmt::MAGIC`], else JSON lines.
pub fn read_file(path: &std::path::Path) -> Result<Vec<TraceFrame>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.starts_with(binfmt::MAGIC) {
        binfmt::decode(&bytes).map_err(|e| format!("{}: {e}", path.display()))
    } else {
        read_jsonl(&bytes[..]).map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
pub(crate) fn sample(seq: u32) -> TraceFrame {
    TraceFrame {
        v: SCHEMA_VERSION,
        seq,
        act_frame_count: seq + 1,
        frame_count: seq as i32 + 2,
        game_speed: 4096,
        mode: 10,
        multi: 1,
        single: 1,
        world: 0,
        level: 0,
        rseed: 398623u32.wrapping_mul(seq + 1),
        pads: [32, 0, 0, 0],
        players: vec![PlayerState { pos: Some([-500, 0, 10250]), curr_tile: 561, ..Default::default() }],
        enemies: vec![
            EnemyState { uid: 103, active: 1, pos: Some([1, 2, 3]), flags: 0x20_0000, speed: 4096, in_tile: 15, ..Default::default() },
            EnemyState { uid: 0, active: 0, pos: None, in_tile: -1, ..Default::default() },
        ],
        platforms: vec![PlatformState {
            uid: 1,
            active: 1,
            pos: Some([0, 750, -12750]),
            flags: 40,
            curr_speed: 4096,
            in_tile: [420, -1],
            ..Default::default()
        }],
        garib_count: 1,
        garib_total: 28,
        garibs: vec![GaribState { kind: 0, active: 1, pos: [500, 900, -8750] }],
        babies_saved: 0,
        num_babies: 5,
        end_level_timer: 0,
        cam_source: [-2048000, 8651520, 48382520],
        cam_target: [0, -4096, i32::MIN],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonl_round_trip() {
        let frames: Vec<_> = (0..3).map(sample).collect();
        let mut buf = Vec::new();
        write_jsonl(&mut buf, &frames).unwrap();
        assert_eq!(read_jsonl(&buf[..]).unwrap(), frames);
    }

    /// The oracle's dependency-free encoder (crates/f2fix `trace::Frame`)
    /// writes exactly the canonical line of this schema.
    #[test]
    fn oracle_encoder_matches_schema() {
        let line = f2fix::trace::sample().to_json();
        let f = TraceFrame::from_json_line(&line).unwrap();
        assert_eq!(f.v, SCHEMA_VERSION);
        assert_eq!(f2fix::trace::SCHEMA_VERSION, SCHEMA_VERSION);
        assert_eq!(f.to_json_line(), line, "field order or formatting drifted");
        assert_eq!(f.players[0].pos, Some([1, -2, 3]));
        assert_eq!(f.enemies[0].pos, None);
    }
}
