//! Per-tick state trace written by the oracle (portable, host-tested).
//!
//! One JSON object per line (`f2-trace.jsonl`). The schema is owned by
//! `tools/f2-trace` (`TraceFrame`); this is a dependency-free encoder for the
//! same shape, kept compatible by a test in `tools/f2-trace`. Field order is
//! fixed so identical states give byte-identical lines.

use core::fmt::Write;

/// Schema versions written as `"v"` (`tools/f2-trace` `SCHEMA_V1`/`SCHEMA_V2`):
/// 1 without, 2 with the optional sections of [`V2`].
pub const SCHEMA_V1: u32 = 1;
pub const SCHEMA_V2: u32 = 2;

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
    /// Schema v2 sections (`trace.version=2`).
    pub v2: Option<V2>,
}

// ---- schema v2 sections (field order = tools/f2-trace TraceFrame) -------------

#[derive(Clone, Debug, Default, PartialEq, Eq)]
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GaribExt {
    pub blocks: i16,
    pub max_coins: i16,
    pub pause_flag: i32,
    pub pause_timer: u16,
    pub drop_speed: Vec<i16>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Baby {
    pub is_saved: i32,
    pub idle: i32,
    pub collect: i32,
    pub pos: Option<[i16; 3]>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Camera {
    pub source: [i32; 3],
    pub target: [i32; 3],
    pub cam_vect: [i32; 3],
    pub curr_cam_dist: [i32; 3],
    pub cam_dist: [i16; 3],
    pub curr_cam_offset: [i32; 3],
    pub cam_look_ofs: i32,
    pub trans_cam_speed_mult: i32,
    pub y_fov_new: i32,
    pub fixed: [u8; 3],
    pub cam_shakiness: i32,
    pub trans: i32,
    pub trans_count: i16,
    pub fog: [i32; 6],
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Trigger {
    pub flags: u16,
    pub count: u32,
    pub delay: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpecFx {
    pub kind: i16,
    pub origin: [i16; 3],
    pub lifetime: i32,
    pub update_type: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tongue {
    pub flags: u32,
    pub radius: i32,
    pub progress: i32,
    pub kind: u8,
    pub can_tongue: u8,
    pub source: [i16; 3],
    pub target: [i16; 3],
    pub pos: [i16; 3],
    pub has_thing: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hud {
    pub score_timer: u16,
    pub mode_timer: u16,
    pub go_timer: u16,
    pub demo_timeout: u16,
    pub hand_angle: i32,
    pub store_frame_count: u32,
}

/// All v2 sections (the oracle always writes every one).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct V2 {
    pub players_ext: Vec<PlayerExt>,
    pub garib_ext: GaribExt,
    pub babies: Vec<Baby>,
    pub camera: Camera,
    pub script_flags: [u8; 8],
    pub script_counters: [i16; 16],
    pub trigger_count: i16,
    pub triggers: Vec<Trigger>,
    pub specfx_count: i16,
    pub specfx: Vec<SpecFx>,
    pub sprite_count: i16,
    pub particle_count: i16,
    pub tongue: Vec<Tongue>,
    pub hud: Hud,
}

/// `[a,b,c]` lists of objects: `{` + body + `}` per element.
fn objs<T>(s: &mut String, v: &[T], mut body: impl FnMut(&mut String, &T)) {
    s.push('[');
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push('{');
        body(s, x);
        s.push('}');
    }
    s.push(']');
}

impl V2 {
    /// The v2 sections as `,"players_ext":...,"hud":{...}` (appended before the closing brace).
    fn write(&self, s: &mut String) {
        s.push_str(",\"players_ext\":");
        objs(s, &self.players_ext, |s, p| {
            let _ = write!(
                s,
                "\"frog_facing\":{},\"cam_facing\":{},\"frogon\":{},\"frogunder\":{},\"old_lives\":{},\"input_pause\":{},\
                 \"spawn_timer\":{},\"spawn_score_level\":{},\"idle_time\":{},\"idle_enable\":{},\"has_double_jumped\":{},\
                 \"extended_hop_dir\":{},\"stun_time\":{},\"croak_time\":{},\"autohop_time\":{},\"quickhop_time\":{},\
                 \"slowhop_time\":{},\"jump_start_frame\":{},\"jump_speed\":{},\"jump_multiplier\":{},\"jump_fwd\":",
                p.frog_facing,
                p.cam_facing,
                p.frogon,
                p.frogunder,
                p.old_lives,
                p.input_pause,
                p.spawn_timer,
                p.spawn_score_level,
                p.idle_time,
                p.idle_enable,
                p.has_double_jumped,
                p.extended_hop_dir,
                p.stun_time,
                p.croak_time,
                p.autohop_time,
                p.quickhop_time,
                p.slowhop_time,
                p.jump_start_frame,
                p.jump_speed,
                p.jump_multiplier
            );
            arr(s, &p.jump_fwd);
            s.push_str(",\"jump_up\":");
            arr(s, &p.jump_up);
            s.push_str(",\"jump_origin\":");
            arr(s, &p.jump_origin);
        });
        let g = &self.garib_ext;
        let _ = write!(
            s,
            ",\"garib_ext\":{{\"blocks\":{},\"max_coins\":{},\"pause_flag\":{},\"pause_timer\":{},\"drop_speed\":",
            g.blocks, g.max_coins, g.pause_flag, g.pause_timer
        );
        arr(s, &g.drop_speed);
        s.push_str("},\"babies\":");
        objs(s, &self.babies, |s, b| {
            let _ = write!(s, "\"is_saved\":{},\"idle\":{},\"collect\":{},\"pos\":", b.is_saved, b.idle, b.collect);
            opt_pos(s, &b.pos);
        });
        let c = &self.camera;
        s.push_str(",\"camera\":{\"source\":");
        arr(s, &c.source);
        s.push_str(",\"target\":");
        arr(s, &c.target);
        s.push_str(",\"cam_vect\":");
        arr(s, &c.cam_vect);
        s.push_str(",\"curr_cam_dist\":");
        arr(s, &c.curr_cam_dist);
        s.push_str(",\"cam_dist\":");
        arr(s, &c.cam_dist);
        s.push_str(",\"curr_cam_offset\":");
        arr(s, &c.curr_cam_offset);
        let _ = write!(s, ",\"cam_look_ofs\":{},\"trans_cam_speed_mult\":{},\"y_fov_new\":{},\"fixed\":", c.cam_look_ofs, c.trans_cam_speed_mult, c.y_fov_new);
        arr(s, &c.fixed);
        let _ = write!(s, ",\"cam_shakiness\":{},\"trans\":{},\"trans_count\":{},\"fog\":", c.cam_shakiness, c.trans, c.trans_count);
        arr(s, &c.fog);
        s.push_str("},\"script\":{\"flags\":");
        arr(s, &self.script_flags);
        s.push_str(",\"counters\":");
        arr(s, &self.script_counters);
        let _ = write!(s, "}},\"triggers\":{{\"count\":{},\"list\":", self.trigger_count);
        objs(s, &self.triggers, |s, t| {
            let _ = write!(s, "\"flags\":{},\"count\":{},\"delay\":{}", t.flags, t.count, t.delay);
        });
        let _ = write!(s, "}},\"specfx\":{{\"count\":{},\"list\":", self.specfx_count);
        objs(s, &self.specfx, |s, x| {
            let _ = write!(s, "\"kind\":{},\"origin\":", x.kind);
            arr(s, &x.origin);
            let _ = write!(s, ",\"lifetime\":{},\"update_type\":{}", x.lifetime, x.update_type);
        });
        let _ = write!(s, "}},\"sprites\":{{\"count\":{},\"particles\":{}}},\"tongue\":", self.sprite_count, self.particle_count);
        objs(s, &self.tongue, |s, t| {
            let _ = write!(
                s,
                "\"flags\":{},\"radius\":{},\"progress\":{},\"kind\":{},\"can_tongue\":{},\"source\":",
                t.flags, t.radius, t.progress, t.kind, t.can_tongue
            );
            arr(s, &t.source);
            s.push_str(",\"target\":");
            arr(s, &t.target);
            s.push_str(",\"pos\":");
            arr(s, &t.pos);
            let _ = write!(s, ",\"has_thing\":{}", t.has_thing);
        });
        let h = &self.hud;
        let _ = write!(
            s,
            ",\"hud\":{{\"score_timer\":{},\"mode_timer\":{},\"go_timer\":{},\"demo_timeout\":{},\"hand_angle\":{},\"store_frame_count\":{}}}",
            h.score_timer, h.mode_timer, h.go_timer, h.demo_timeout, h.hand_angle, h.store_frame_count
        );
    }
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
            if self.v2.is_some() { SCHEMA_V2 } else { SCHEMA_V1 },
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
        if let Some(v2) = &self.v2 {
            v2.write(&mut s);
        }
        s.push('}');
        s
    }
}

/// One region of a breakpoint snapshot as read from memory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SnapRegion {
    pub name: String,
    pub addr: u32,
    pub len: u32,
    pub deref: bool,
    /// The pointer read from `addr` (deref regions).
    pub ptr: Option<u32>,
    /// `None` when unreadable (NULL or unmapped pointer).
    pub bytes: Option<Vec<u8>>,
}

/// A breakpoint snapshot (`f2-snap-<tick>.json`; schema in `tools/f2-trace` `snap`).
pub fn snapshot_json(seq: u32, act_frame_count: u32, mode: u32, regions: &[SnapRegion]) -> String {
    let mut s = format!("{{\"snap\":1,\"seq\":{seq},\"act_frame_count\":{act_frame_count},\"mode\":{mode},\"regions\":[");
    for (i, r) in regions.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        // Names come from the ini (CSV symbol names); keep them JSON-safe.
        let name: String = r.name.chars().filter(|c| c.is_ascii_graphic() && *c != '"' && *c != '\\').collect();
        let _ = write!(s, "{{\"name\":\"{}\",\"addr\":\"{:#010x}\",\"len\":{},\"deref\":{},\"ptr\":", name, r.addr, r.len, r.deref);
        match r.ptr {
            Some(p) => {
                let _ = write!(s, "\"{p:#010x}\"");
            }
            None => s.push_str("null"),
        }
        s.push_str(",\"hex\":");
        match &r.bytes {
            Some(b) => {
                s.push('"');
                for x in b {
                    let _ = write!(s, "{x:02x}");
                }
                s.push('"');
            }
            None => s.push_str("null"),
        }
        s.push('}');
    }
    s.push_str("]}");
    s
}

#[doc(hidden)]
pub fn sample_snapshot() -> String {
    snapshot_json(
        41,
        42,
        10,
        &[
            SnapRegion { name: "rseed".into(), addr: 0x479d70, len: 3, deref: false, ptr: None, bytes: Some(vec![1, 2, 0xff]) },
            SnapRegion { name: "frog0".into(), addr: 0x489698, len: 4, deref: true, ptr: Some(0), bytes: None },
        ],
    )
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
        v2: None,
    }
}

/// [`sample`] with every v2 section filled (schema tests in `tools/f2-trace`).
#[doc(hidden)]
pub fn sample_v2() -> Frame {
    let mut f = sample();
    f.v2 = Some(V2 {
        players_ext: vec![PlayerExt {
            frog_facing: 2,
            cam_facing: -1,
            idle_time: 4096,
            jump_fwd: [0, 0, -4096],
            jump_origin: [1, 2, -3],
            slowhop_time: 7,
            ..Default::default()
        }],
        garib_ext: GaribExt { blocks: 28, max_coins: 25, pause_flag: 2, pause_timer: 1, drop_speed: vec![0, -3] },
        babies: vec![Baby { is_saved: 1, idle: 0, collect: 2, pos: Some([9, -9, 0]) }, Baby::default()],
        camera: Camera {
            source: [1, 2, 3],
            target: [4, 5, 6],
            cam_vect: [0, 4096, 0],
            curr_cam_dist: [0, 6800 * 4506, -1],
            cam_dist: [0, 6800, 1920],
            curr_cam_offset: [0, 409600, 409600],
            cam_look_ofs: 50 << 12,
            trans_cam_speed_mult: 4096,
            y_fov_new: 184320,
            fixed: [1, 1, 0],
            cam_shakiness: 0,
            trans: 2,
            trans_count: 3,
            fog: [120, 120, 160, 0, 995, 5000],
        },
        script_flags: [0, 1, 2, 3, 4, 5, 6, 255],
        script_counters: [0, 1, -1, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, i16::MIN],
        trigger_count: 4,
        triggers: vec![Trigger { flags: 0x8011, count: 1, delay: 30 }],
        specfx_count: 1,
        specfx: vec![SpecFx { kind: 21, origin: [1, -2, 3], lifetime: 1234, update_type: 4 }],
        sprite_count: 40,
        particle_count: 3,
        tongue: vec![Tongue {
            flags: 1,
            radius: 0x708000,
            progress: 0,
            kind: 0,
            can_tongue: 1,
            source: [1, 2, 3],
            target: [0; 3],
            pos: [-1, -2, -3],
            has_thing: 0,
        }],
        hud: Hud { score_timer: 90, mode_timer: 8, go_timer: 0, demo_timeout: 25, hand_angle: 61440, store_frame_count: 77 },
    });
    f
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
