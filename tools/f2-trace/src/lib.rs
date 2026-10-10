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
//!
//! Schema v2 ([`SCHEMA_V2`]) is v1 plus optional sections, appended after
//! `cam_target` in this order: `players_ext`, `garib_ext`, `babies`,
//! `camera`, `script`, `triggers`, `specfx`, `sprites`, `tongue`, `hud`, `mp`
//! (local multiplayer, diff section `multi`).
//! A section that is absent (`None`) is not written at all, so a v1 line is
//! a valid frame (all sections `None`) and a v1 frame re-serialises to the
//! same bytes. An emitter may fill any subset of the sections; the diff only
//! compares sections both sides have. Field layouts: `symbols/frogger2_structs.md`.

pub mod binfmt;
pub mod diff;
pub mod snap;

use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};

/// Base schema (no optional sections).
pub const SCHEMA_V1: u32 = 1;
/// v1 + optional sections.
pub const SCHEMA_V2: u32 = 2;
/// Written as `"v"` by [`TraceFrame`] emitters (latest; the oracle writes 1 or 2 per `trace.version`).
pub const SCHEMA_VERSION: u32 = SCHEMA_V2;

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

// ---- v2 sections -------------------------------------------------------------

/// Per player (`i < NUM_FROGS`): `PLAYER` fields beyond [`PlayerState`], plus
/// `frogFacing[i]` / `camFacing[i]`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerExt {
    pub frog_facing: i32,
    pub cam_facing: i32,
    pub frogon: i16,
    pub frogunder: i16,
    pub old_lives: i16,
    pub input_pause: u8,
    pub spawn_timer: i16,
    pub spawn_score_level: i16,
    pub idle_time: i32,
    pub idle_enable: u8,
    pub has_double_jumped: u8,
    pub extended_hop_dir: u8,
    /// `.time` of the `stun`, `isCroaking`, `autohop`, `quickhop`, `slowhop` TIMERs.
    pub stun_time: u16,
    pub croak_time: u16,
    pub autohop_time: u16,
    pub quickhop_time: u16,
    pub slowhop_time: u16,
    pub jump_start_frame: u32,
    pub jump_speed: i32,
    pub jump_multiplier: i32,
    pub jump_fwd: [i32; 3],
    pub jump_up: [i32; 3],
    pub jump_origin: [i16; 3],
}

/// `garibList` bookkeeping and per-garib fields not in [`GaribState`] (same list order).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GaribExt {
    pub blocks: i16,
    pub max_coins: i16,
    /// collect.c `pauseFlag` / `pauseTimer.time`.
    pub pause_flag: i32,
    pub pause_timer: u16,
    /// `garib->dropSpeed` per garib.
    pub drop_speed: Vec<i16>,
}

/// One `babyList[i]`, `i < numBabies` (max 5).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BabyState {
    pub is_saved: i32,
    pub idle: i32,
    pub collect: i32,
    /// `baby->actor->position`; `None` when `baby` is NULL.
    pub pos: Option<[i16; 3]>,
}

/// Camera globals (Common/cam.c).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CameraState {
    /// `camSource` / `camTarget` (the goal the current camera eases to).
    pub source: [i32; 3],
    pub target: [i32; 3],
    /// `camVect` (FVECTOR, 20.12): the camera up vector being slurped to the tile normal.
    pub cam_vect: [i32; 3],
    /// `currCamDist` (FVECTOR) / `camDist` (SVECTOR).
    pub curr_cam_dist: [i32; 3],
    pub cam_dist: [i16; 3],
    /// `currCamOffset` (FVECTOR).
    pub curr_cam_offset: [i32; 3],
    pub cam_look_ofs: i32,
    pub trans_cam_speed_mult: i32,
    pub y_fov_new: i32,
    /// `fixedDir`, `fixedPos`, `fixedUp`.
    pub fixed: [u8; 3],
    pub cam_shakiness: i32,
    /// Transition camera of player 0's tile: the highest index `i < tcList.count`
    /// with `tcList.array[i].tile == currTile[0]` (the entry
    /// `CheckForDynamicCameraChange` applies), -1 for none.
    pub trans: i32,
    /// `tcList.count`.
    pub trans_count: i16,
    /// `fog` (Common/frogger.c): r, g, b, mode, min, max.
    pub fog: [i32; 6],
}

/// Script state (Common/script.c).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptState {
    /// `scriptFlags[8]` (64 flag bits).
    pub flags: Vec<u8>,
    /// `scriptCounters[16]`.
    pub counters: Vec<i16>,
}

/// One active `TRIGGER` (`triggerList.head.next` .. back to `&head`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerState {
    pub flags: u16,
    pub count: u32,
    pub delay: u32,
}

/// `triggerList` (Common/event.c).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggersState {
    /// `triggerList.count` (includes AND/OR/NOT sub-triggers, which are not in `list`).
    pub count: i16,
    pub list: Vec<TriggerState>,
}

/// One `SPECFX` of `sfxList` (Common/specfx.c), list order from `head.next`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecFxState {
    /// `fx->type` (FXTYPE_*).
    pub kind: i16,
    /// `fx->origin` (SVECTOR).
    pub origin: [i16; 3],
    /// `fx->lifetime`: absolute `actFrameCount` at which it dies.
    pub lifetime: i32,
    pub update_type: u8,
}

/// `sfxList`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecFxSection {
    pub count: i16,
    pub list: Vec<SpecFxState>,
}

/// Sprite and particle pool usage.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpritesState {
    /// `sprList.count` / `partList.count`.
    pub count: i16,
    pub particles: i16,
}

/// One `tongue[i]`, `i < NUM_FROGS` (Common/tongue.c).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TongueState {
    pub flags: u32,
    pub radius: i32,
    pub progress: i32,
    /// `tongue->type` / `canTongue`.
    pub kind: u8,
    pub can_tongue: u8,
    /// `source`, `target`, `pos` (SVECTOR).
    pub source: [i16; 3],
    pub target: [i16; 3],
    pub pos: [i16; 3],
    /// `thing != NULL`.
    pub has_thing: u8,
}

/// HUD / level timers (`.time` of each TIMER).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HudState {
    pub score_timer: u16,
    pub mode_timer: u16,
    pub go_timer: u16,
    pub demo_timeout: u16,
    /// hud.c `handAngle`, `storeFrameCount`.
    pub hand_angle: i32,
    pub store_frame_count: u32,
}

/// One `mpl[i]` (Common/multi.h `MPINFO`), `i < NUM_FROGS`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MpPlayerState {
    /// Rounds won (race and battle).
    pub wins: i16,
    pub lap: i16,
    /// Race: lap time and time penalty (frames).
    pub timer: u32,
    pub penalty: u32,
    /// Race: checkpoint.
    pub check: i16,
    /// Battle: score / trail length.
    pub score: i16,
    pub trail: i16,
    pub ready: i8,
    pub lasttile: i16,
}

/// Local multiplayer (Common/multi.c globals), key `mp`; present while
/// `gameState.multi == MULTILOCAL` (2).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MpState {
    /// `multiplayerMode`: 1 race, 2 battle, 3 collect.
    pub mode: i32,
    /// `started`: 0 waiting, 1 countdown, 2 running.
    pub started: i32,
    pub num_multi_items: u32,
    /// `gameWinner` / `matchWinner` (-1, a player, 127 = draw).
    pub game_winner: i8,
    pub match_winner: i8,
    /// `multiTimer.time`, `endTimer.time`.
    pub multi_timer: u16,
    pub end_timer: u16,
    pub initial_cam_height: i32,
    pub mpl: Vec<MpPlayerState>,
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
    // ---- v2 (optional) ----
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub players_ext: Option<Vec<PlayerExt>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub garib_ext: Option<GaribExt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub babies: Option<Vec<BabyState>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<CameraState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<ScriptState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triggers: Option<TriggersState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specfx: Option<SpecFxSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sprites: Option<SpritesState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tongue: Option<Vec<TongueState>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hud: Option<HudState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mp: Option<MpState>,
}

impl TraceFrame {
    /// Does this frame carry any v2 section?
    pub fn has_v2(&self) -> bool {
        self.players_ext.is_some()
            || self.garib_ext.is_some()
            || self.babies.is_some()
            || self.camera.is_some()
            || self.script.is_some()
            || self.triggers.is_some()
            || self.specfx.is_some()
            || self.sprites.is_some()
            || self.tongue.is_some()
            || self.hud.is_some()
            || self.mp.is_some()
    }

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
        v: SCHEMA_V1,
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
        ..Default::default()
    }
}

#[cfg(test)]
pub(crate) fn sample_v2(seq: u32) -> TraceFrame {
    let mut f = sample(seq);
    f.v = SCHEMA_V2;
    f.players_ext = Some(vec![PlayerExt { frog_facing: 2, cam_facing: 1, jump_fwd: [0, 0, 4096], jump_origin: [1, -2, 3], ..Default::default() }]);
    f.garib_ext = Some(GaribExt { blocks: 28, max_coins: 25, drop_speed: vec![0], ..Default::default() });
    f.babies = Some(vec![BabyState { pos: Some([5, 6, 7]), ..Default::default() }, BabyState { is_saved: 1, pos: None, ..Default::default() }]);
    f.camera = Some(CameraState { trans: -1, cam_vect: [0, 4096, -1], fixed: [0, 1, 0], fog: [120, 120, 160, 0, 995, 5000], ..Default::default() });
    f.script = Some(ScriptState { flags: vec![0, 1, 0, 0, 0, 0, 0, 128], counters: vec![0; 16] });
    f.triggers = Some(TriggersState { count: 3, list: vec![TriggerState { flags: 0x11, count: 0, delay: 30 }] });
    f.specfx = Some(SpecFxSection { count: 1, list: vec![SpecFxState { kind: 21, origin: [1, 2, 3], lifetime: 900, update_type: 4 }] });
    f.sprites = Some(SpritesState { count: 40, particles: 0 });
    f.tongue = Some(vec![TongueState { flags: 1, radius: 0x708000, can_tongue: 1, ..Default::default() }]);
    f.hud = Some(HudState { score_timer: 90, go_timer: 0, hand_angle: 4096, ..Default::default() });
    f.mp = Some(MpState {
        mode: 2,
        started: 2,
        game_winner: -1,
        match_winner: 127,
        mpl: vec![MpPlayerState { wins: 1, trail: 8, lasttile: -1, ..Default::default() }],
        ..Default::default()
    });
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v2_round_trip_and_v1_compat() {
        let f = sample_v2(4);
        let line = f.to_json_line();
        assert!(line.contains("\"cam_target\":[0,-4096,-2147483648],\"players_ext\":[{"), "{line}");
        assert_eq!(TraceFrame::from_json_line(&line).unwrap(), f);
        // A v1 line reads with every section absent and re-serialises identically.
        let v1 = sample(4).to_json_line();
        let g = TraceFrame::from_json_line(&v1).unwrap();
        assert!(!g.has_v2());
        assert_eq!(g.to_json_line(), v1);
    }

    /// The oracle's v2 encoder (crates/f2fix `trace::V2`) writes the canonical line.
    #[test]
    fn oracle_v2_encoder_matches_schema() {
        let line = f2fix::trace::sample_v2().to_json();
        let f = TraceFrame::from_json_line(&line).unwrap();
        assert_eq!(f.v, SCHEMA_V2);
        assert!(f.players_ext.is_some() && f.garib_ext.is_some() && f.babies.is_some() && f.camera.is_some());
        assert!(f.script.is_some() && f.triggers.is_some() && f.specfx.is_some() && f.sprites.is_some());
        assert!(f.tongue.is_some() && f.hud.is_some() && f.mp.is_some());
        assert_eq!(f.to_json_line(), line, "v2 field order or formatting drifted");
    }

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
        assert_eq!(f.v, SCHEMA_V1);
        assert_eq!(f2fix::trace::SCHEMA_V1, SCHEMA_V1);
        assert_eq!(f2fix::trace::SCHEMA_V2, SCHEMA_V2);
        assert_eq!(f.to_json_line(), line, "field order or formatting drifted");
        assert_eq!(f.players[0].pos, Some([1, -2, 3]));
        assert_eq!(f.enemies[0].pos, None);
    }
}
