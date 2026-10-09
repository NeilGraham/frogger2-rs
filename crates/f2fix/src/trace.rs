//! Per-tick state trace written by the oracle (portable, host-tested).
//!
//! One JSON object per line (`f2-trace.jsonl`). The schema is owned by
//! `tools/f2-trace` (`TraceFrame`); this is a dependency-free encoder for the
//! same shape, kept compatible by a test in `tools/f2-trace`. Field order is
//! fixed so identical states give byte-identical lines.

use core::fmt::Write;

/// Schema version written as `"v"`; bump together with `tools/f2-trace`.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Player {
    /// `frog[i]->actor->position`, `None` when `frog[i]` is NULL.
    pub pos: Option<[i16; 3]>,
    pub frog_state: u32,
    /// `(currTile[i] - firstTile) / sizeof(GAMETILE)`, -1 for NULL/outside.
    pub curr_tile: i32,
    pub lives: i16,
    pub health: u8,
    pub death_by: u8,
    pub can_jump: u8,
    pub has_jumped: u8,
    pub is_super_hopping: u8,
    pub is_sinking: u8,
    pub num_spawn: i16,
    pub safe_time: u16,
    pub dead_time: u16,
    pub jump_time: i32,
    pub height_jumped: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Enemy {
    pub uid: i32,
    pub active: u8,
    pub pos: Option<[i16; 3]>,
    pub flags: u32,
    pub is_waiting: i16,
    pub speed: i32,
    pub in_tile: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Platform {
    pub uid: i32,
    pub active: u8,
    pub pos: Option<[i16; 3]>,
    pub flags: u32,
    pub is_waiting: i16,
    pub curr_speed: i32,
    pub in_tile: [i32; 2],
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Garib {
    pub kind: u8,
    pub active: i8,
    pub pos: [i16; 3],
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    /// Index of this line in the trace (0-based).
    pub seq: u32,
    pub act_frame_count: u32,
    pub frame_count: i32,
    pub game_speed: i32,
    pub mode: u32,
    pub multi: u8,
    pub single: u8,
    pub world: u8,
    pub level: u8,
    pub rseed: u32,
    /// `padData.digital[0..4]` as seen by this tick's game logic.
    pub pads: [u16; 4],
    pub players: Vec<Player>,
    pub enemies: Vec<Enemy>,
    pub platforms: Vec<Platform>,
    pub garib_count: i16,
    pub garib_total: i16,
    pub garibs: Vec<Garib>,
    pub babies_saved: i32,
    pub num_babies: i32,
    pub end_level_timer: u16,
    pub cam_source: [i32; 3],
    pub cam_target: [i32; 3],
}

fn arr<T: core::fmt::Display>(s: &mut String, a: &[T]) {
    s.push('[');
    for (i, v) in a.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(s, "{v}");
    }
    s.push(']');
}

fn opt_pos(s: &mut String, p: &Option<[i16; 3]>) {
    match p {
        Some(p) => arr(s, p),
        None => s.push_str("null"),
    }
}

impl Frame {
    /// One JSON line (no trailing newline).
    pub fn to_json(&self) -> String {
        let mut s = String::with_capacity(512 + 160 * (self.enemies.len() + self.platforms.len()));
        let _ = write!(
            s,
            "{{\"v\":{},\"seq\":{},\"act_frame_count\":{},\"frame_count\":{},\"game_speed\":{},\"mode\":{},\
             \"multi\":{},\"single\":{},\"world\":{},\"level\":{},\"rseed\":{},\"pads\":",
            SCHEMA_VERSION,
            self.seq,
            self.act_frame_count,
            self.frame_count,
            self.game_speed,
            self.mode,
            self.multi,
            self.single,
            self.world,
            self.level,
            self.rseed
        );
        arr(&mut s, &self.pads);
        s.push_str(",\"players\":[");
        for (i, p) in self.players.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str("{\"pos\":");
            opt_pos(&mut s, &p.pos);
            let _ = write!(
                s,
                ",\"frog_state\":{},\"curr_tile\":{},\"lives\":{},\"health\":{},\"death_by\":{},\
                 \"can_jump\":{},\"has_jumped\":{},\"is_super_hopping\":{},\"is_sinking\":{},\
                 \"num_spawn\":{},\"safe_time\":{},\"dead_time\":{},\"jump_time\":{},\"height_jumped\":{}}}",
                p.frog_state,
                p.curr_tile,
                p.lives,
                p.health,
                p.death_by,
                p.can_jump,
                p.has_jumped,
                p.is_super_hopping,
                p.is_sinking,
                p.num_spawn,
                p.safe_time,
                p.dead_time,
                p.jump_time,
                p.height_jumped
            );
        }
        s.push_str("],\"enemies\":[");
        for (i, e) in self.enemies.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            let _ = write!(s, "{{\"uid\":{},\"active\":{},\"pos\":", e.uid, e.active);
            opt_pos(&mut s, &e.pos);
            let _ = write!(s, ",\"flags\":{},\"is_waiting\":{},\"speed\":{},\"in_tile\":{}}}", e.flags, e.is_waiting, e.speed, e.in_tile);
        }
        s.push_str("],\"platforms\":[");
        for (i, p) in self.platforms.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            let _ = write!(s, "{{\"uid\":{},\"active\":{},\"pos\":", p.uid, p.active);
            opt_pos(&mut s, &p.pos);
            let _ = write!(s, ",\"flags\":{},\"is_waiting\":{},\"curr_speed\":{},\"in_tile\":", p.flags, p.is_waiting, p.curr_speed);
            arr(&mut s, &p.in_tile);
            s.push('}');
        }
        let _ = write!(s, "],\"garib_count\":{},\"garib_total\":{},\"garibs\":[", self.garib_count, self.garib_total);
        for (i, g) in self.garibs.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            let _ = write!(s, "{{\"kind\":{},\"active\":{},\"pos\":", g.kind, g.active);
            arr(&mut s, &g.pos);
            s.push('}');
        }
        let _ = write!(
            s,
            "],\"babies_saved\":{},\"num_babies\":{},\"end_level_timer\":{},\"cam_source\":",
            self.babies_saved, self.num_babies, self.end_level_timer
        );
        arr(&mut s, &self.cam_source);
        s.push_str(",\"cam_target\":");
        arr(&mut s, &self.cam_target);
        s.push('}');
        s
    }
}

/// Map a `GAMETILE*` to its index in `firstTile[0..tile_count]`, -1 otherwise.
pub fn tile_index(ptr: u32, first_tile: u32, tile_count: i32, tile_size: u32) -> i32 {
    if ptr == 0 || first_tile == 0 || ptr < first_tile {
        return -1;
    }
    let off = ptr - first_tile;
    if !off.is_multiple_of(tile_size) {
        return -1;
    }
    let i = off / tile_size;
    if tile_count >= 0 && i < tile_count as u32 {
        i as i32
    } else {
        -1
    }
}

/// A fixed example frame (schema-compatibility tests in `tools/f2-trace`).
#[doc(hidden)]
pub fn sample() -> Frame {
    Frame {
        seq: 3,
        act_frame_count: 42,
        frame_count: 43,
        game_speed: 4096,
        mode: 10,
        multi: 1,
        single: 1,
        world: 0,
        level: 0,
        rseed: 398623,
        pads: [32, 0, 0, 0],
        players: vec![Player { pos: Some([1, -2, 3]), curr_tile: 7, lives: 3, ..Default::default() }],
        enemies: vec![Enemy { uid: 5, active: 1, pos: None, flags: 9, ..Default::default() }],
        platforms: vec![Platform { uid: 6, in_tile: [1, -1], ..Default::default() }],
        garib_count: 1,
        garib_total: 2,
        garibs: vec![Garib { kind: 1, active: 1, pos: [4, 5, 6] }],
        babies_saved: 0,
        num_babies: 5,
        end_level_timer: 0,
        cam_source: [1, 2, 3],
        cam_target: [-4, 5, -6],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_shape() {
        let j = sample().to_json();
        assert!(j.starts_with("{\"v\":1,\"seq\":3,\"act_frame_count\":42,"), "{j}");
        assert!(j.contains("\"players\":[{\"pos\":[1,-2,3],\"frog_state\":0,\"curr_tile\":7,\"lives\":3,"));
        assert!(j.contains("\"enemies\":[{\"uid\":5,\"active\":1,\"pos\":null,\"flags\":9,"));
        assert!(j.contains("\"in_tile\":[1,-1]}"));
        assert!(j.ends_with("\"cam_target\":[-4,5,-6]}"));
        assert!(!j.contains('\n'));
    }

    #[test]
    fn tile_indices() {
        assert_eq!(tile_index(0x1000 + 68 * 3, 0x1000, 10, 68), 3);
        assert_eq!(tile_index(0, 0x1000, 10, 68), -1);
        assert_eq!(tile_index(0x1000 + 68 * 10, 0x1000, 10, 68), -1);
        assert_eq!(tile_index(0x1001, 0x1000, 10, 68), -1);
        assert_eq!(tile_index(0x0fff, 0x1000, 10, 68), -1);
    }
}
