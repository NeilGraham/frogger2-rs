//! Deterministic oracle mode: fixed 60 Hz timestep, start control (demo or
//! level), scenario input and the per-tick state trace.
//!
//! Code patches in the retail exe's `.text` (in memory only; the bytes are
//! verified first, so another exe version is left alone):
//!
//! | site | original | patched to | when |
//! |---|---|---|---|
//! | `RunWindowsLoop` `call UpdateTiming` (0x455b6e) | real-time `UpdateTiming` | [`h_tick_timing`]: exactly one frame, `speed = 1.0` | `time.fixed` |
//! | `UpdateTiming` entry (0x44b430) | — | `jmp` [`h_hold_timing`] (no-op: virtual time does not pass while loading backdrops) | `time.fixed` |
//! | `LoopFunc` `call GameLoop` (0x43cdd0) | `GameLoop` | [`h_game_loop`]: start control + scenario pads, `GameLoop`, trace | trace / start |
//!
//! With `time.fixed`, `LoopFunc` (PC/Main.cpp) sees `timeInfo.frameCount`
//! grow by one and `timeInfo.speed == 1.0` per main-loop iteration, so
//! `actFrameCount` advances by exactly 1 and `gameSpeed == 4096`, and the
//! loop runs as fast as the machine allows.

use super::log::flog;
use super::{cfg, ffi, run_dir};
use crate::addrs as A;
use crate::keys;
use crate::trace::{self, Frame};
use core::ffi::c_void;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::Mutex;

struct Oracle {
    trace: Option<BufWriter<File>>,
    seq: u32,
    fired: bool,
    in_level: bool,
    keys: Option<keys::Player>,
    loops: u64,
    /// Demo key list copy with two defined words in front (see [`fix_demo_keys`]).
    demo_keys: Option<(Vec<u32>, u32)>,
    /// `act_frame_count` values already snapshotted.
    snapped: Vec<u32>,
}

static STATE: Mutex<Option<Oracle>> = Mutex::new(None);

#[inline]
unsafe fn rd<T: Copy>(a: u32) -> T {
    (a as usize as *const T).read_unaligned()
}

#[inline]
unsafe fn wr<T: Copy>(a: u32, v: T) {
    (a as usize as *mut T).write_unaligned(v)
}

unsafe fn patch_bytes(addr: u32, bytes: &[u8]) -> bool {
    let p = addr as usize as *mut c_void;
    let mut old = 0u32;
    if ffi::VirtualProtect(p, bytes.len(), ffi::PAGE_EXECUTE_READWRITE, &mut old) == 0 {
        return false;
    }
    core::ptr::copy_nonoverlapping(bytes.as_ptr(), p as *mut u8, bytes.len());
    let mut tmp = 0u32;
    ffi::VirtualProtect(p, bytes.len(), old, &mut tmp);
    ffi::FlushInstructionCache(ffi::GetCurrentProcess(), p, bytes.len());
    true
}

/// Redirect the `call rel32` at `site` (which must call `expect`) to `to`.
unsafe fn patch_call(site: u32, expect: u32, to: usize, what: &str) -> bool {
    let op: u8 = rd(site);
    let rel: i32 = rd(site + 1);
    let target = (site as i32).wrapping_add(5).wrapping_add(rel) as u32;
    if op != 0xE8 || target != expect {
        flog!("oracle: {} at {:#x}: unexpected code ({:#04x} -> {:#x}); NOT patched", what, site, op, target);
        return false;
    }
    let new = (to as i32).wrapping_sub(site as i32 + 5);
    let mut b = [0xE8u8, 0, 0, 0, 0];
    b[1..].copy_from_slice(&new.to_le_bytes());
    let ok = patch_bytes(site, &b);
    flog!("oracle: {} at {:#x} -> {:#x} {}", what, site, to, if ok { "patched" } else { "FAILED" });
    ok
}

/// Overwrite the entry of a function (whose first bytes must be `expect`) with `jmp to`.
unsafe fn patch_entry_jmp(func: u32, expect: &[u8], to: usize, what: &str) -> bool {
    let cur = core::slice::from_raw_parts(func as usize as *const u8, expect.len());
    if cur != expect {
        flog!("oracle: {} at {:#x}: unexpected prologue {:02x?}; NOT patched", what, func, cur);
        return false;
    }
    let rel = (to as i32).wrapping_sub(func as i32 + 5);
    let mut b = [0xE9u8, 0, 0, 0, 0];
    b[1..].copy_from_slice(&rel.to_le_bytes());
    let ok = patch_bytes(func, &b);
    flog!("oracle: {} at {:#x} -> jmp {:#x} {}", what, func, to, if ok { "patched" } else { "FAILED" });
    ok
}

/// Install the code patches the settings ask for (DllMain: the exe image is
/// mapped and no game code has run yet).
pub fn install() {
    let c = cfg();
    let want_loop = c.trace_enable || !c.snap_ticks.is_empty() || c.start_demo.is_some() || c.start_level.is_some();
    if !c.time_fixed && !want_loop {
        return;
    }
    let mut st = Oracle { trace: None, seq: 0, fired: false, in_level: false, keys: None, loops: 0, demo_keys: None, snapped: Vec::new() };
    if c.trace_enable {
        let path = run_dir().join(&c.trace_file);
        match File::create(&path) {
            Ok(f) => {
                flog!("oracle: trace -> {}", path.display());
                st.trace = Some(BufWriter::with_capacity(1 << 20, f));
            }
            Err(e) => flog!("oracle: cannot create trace {}: {}", path.display(), e),
        }
    }
    if !c.start_keys.is_empty() {
        let p = std::path::Path::new(&c.start_keys);
        let p = if p.is_absolute() { p.to_path_buf() } else { run_dir().join(p) };
        match std::fs::read(&p).map_err(|e| e.to_string()).and_then(|b| keys::parse(&b)) {
            Ok(r) => {
                flog!("oracle: scenario keys {}: {} records", p.display(), r.len());
                st.keys = Some(keys::Player::new(r));
            }
            Err(e) => flog!("oracle: scenario keys {}: {}", p.display(), e),
        }
    }
    if let Ok(mut g) = STATE.lock() {
        *g = Some(st);
    }
    // SAFETY: addresses of the retail USA exe, verified before writing.
    unsafe {
        if c.time_fixed {
            patch_call(A::RUN_WINDOWS_LOOP_CALL_UPDATE_TIMING, A::UPDATE_TIMING, h_tick_timing as *const () as usize, "RunWindowsLoop/UpdateTiming");
            patch_entry_jmp(A::UPDATE_TIMING, &A::UPDATE_TIMING_PROLOGUE, h_hold_timing as *const () as usize, "UpdateTiming");
        }
        if want_loop {
            patch_call(A::LOOP_FUNC_CALL_GAME_LOOP, A::GAME_LOOP, h_game_loop as *const () as usize, "LoopFunc/GameLoop");
        }
    }
}

/// End of a main-loop iteration: one 60 Hz frame passes.
unsafe extern "C" fn h_tick_timing() {
    let ti = A::TIME_INFO;
    let frame = rd::<u32>(ti + A::time_info::FRAME_COUNT).wrapping_add(1);
    wr(ti + A::time_info::FRAME_COUNT, frame);
    wr(ti + A::time_info::TICK_COUNT, ((frame as u64 * 1000) / 60) as u32);
    wr(ti + A::time_info::SPEED, 1.0f32);
}

/// `UpdateTiming` called from elsewhere (backdrop loading): no time passes.
unsafe extern "C" fn h_hold_timing() {}

fn flush_and_terminate(why: &str) -> ! {
    if let Ok(mut g) = STATE.lock() {
        if let Some(st) = g.as_mut() {
            if let Some(t) = st.trace.as_mut() {
                let _ = t.flush();
            }
            flog!("oracle: {} trace lines, {} loops", st.seq, st.loops);
        }
    }
    super::terminate(why)
}

fn level_mode(m: u32) -> bool {
    m == A::mode::DEMO || m == A::mode::INGAME
}

unsafe extern "C" fn h_game_loop() {
    let c = cfg();
    let mode: u32 = rd(A::GAME_STATE + A::game_state::MODE);
    let mut skip = false;
    if let Ok(mut g) = STATE.lock() {
        let Some(st) = g.as_mut() else { return };
        st.loops += 1;
        if !st.fired && mode == A::mode::FRONTEND && rd::<i32>(A::FRAME_COUNT) >= c.start_after as i32 && rd::<i32>(A::FADING_OUT) == 0 {
            if let Some((w, l)) = c.start_demo {
                // menus.c RunFrontendGameLoop: goingToDemo -> InitDemoMode()
                // -> InitLevel(demoLevels[nextDemo]) -> LoadDemoFile.
                wr::<i32>(A::DEMO_LEVELS, w as i32);
                wr::<i32>(A::DEMO_LEVELS + 4, l as i32);
                wr::<i32>(A::NEXT_DEMO, 0);
                wr::<u8>(A::GOING_TO_DEMO, 1);
                st.fired = true;
                flog!("oracle: start demo {}-{} (loop {}, frameCount {}, rseed before InitLevel {})", w, l, st.loops, rd::<i32>(A::FRAME_COUNT), rd::<u32>(A::RSEED));
            } else if let Some((w, l)) = c.start_level {
                // As levplay.c InitDemoMode, but an arcade game in INGAME_MODE.
                wr::<u8>(A::GAME_STATE + A::game_state::MULTI, 1);
                wr::<u8>(A::GAME_STATE + A::game_state::SINGLE, 1);
                wr::<u32>(A::GAME_STATE + A::game_state::MODE, A::mode::INGAME);
                wr::<u8>(A::GAME_STATE + A::game_state::DIFFICULTY, 1);
                st.fired = true;
                flog!("oracle: start level {}-{} (loop {}, rseed before InitLevel {})", w, l, st.loops, rd::<u32>(A::RSEED));
                skip = true;
            }
        }
        if st.fired && !st.in_level && level_mode(mode) {
            st.in_level = true;
            flog!("oracle: level running (loop {}, actFrameCount {}, rseed {})", st.loops, rd::<u32>(A::ACT_FRAME_COUNT), rd::<u32>(A::RSEED));
        }
        if st.fired && c.start_demo.is_some() {
            fix_demo_keys(st, mode);
        }
        if st.in_level {
            if let Some(k) = st.keys.as_mut() {
                let (dig, deb) = k.step(rd(A::ACT_FRAME_COUNT));
                for i in 0..4u32 {
                    wr::<u16>(A::PAD_DIGITAL + 2 * i, dig[i as usize]);
                    wr::<u16>(A::PAD_DEBOUNCE + 2 * i, deb[i as usize]);
                }
            }
        }
    }
    if skip {
        if let Some((w, l)) = c.start_level {
            let init_level: extern "C" fn(u32, u32) = core::mem::transmute(A::INIT_LEVEL as usize);
            init_level(w, l);
            // GameLoop's epilogue for this iteration.
            wr::<i32>(A::FRAME_COUNT, rd::<i32>(A::FRAME_COUNT).wrapping_add(1));
        }
        return;
    }

    let game_loop: extern "C" fn() = core::mem::transmute(A::GAME_LOOP as usize);
    game_loop();

    let after: u32 = rd(A::GAME_STATE + A::game_state::MODE);
    let mut end = None;
    if let Ok(mut g) = STATE.lock() {
        let Some(st) = g.as_mut() else { return };
        if st.in_level && after == A::mode::FRONTEND {
            if c.exit_level_end {
                end = Some("level end (back in the frontend)");
            }
        } else if st.in_level || !c.trace_from_level {
            let afc: u32 = rd(A::ACT_FRAME_COUNT);
            if c.snap_ticks.contains(&afc) && !st.snapped.contains(&afc) {
                st.snapped.push(afc);
                write_snapshot(&c.snap_regions, st.seq, afc, after);
            }
        }
        if st.trace.is_some() && (st.in_level || !c.trace_from_level) && !(st.in_level && after == A::mode::FRONTEND) {
            let mut f = read_frame();
            if c.trace_version >= 2 {
                f.v2 = Some(read_v2(&f));
            }
            f.seq = st.seq;
            st.seq += 1;
            if let Some(t) = st.trace.as_mut() {
                let mut line = f.to_json();
                line.push('\n');
                let _ = t.write_all(line.as_bytes());
            }
            if st.seq % 600 == 0 {
                flog!("oracle: {} trace lines (actFrameCount {}, mode {})", st.seq, f.act_frame_count, f.mode);
            }
            if c.exit_ticks > 0 && st.seq >= c.exit_ticks {
                end = Some("exit.ticks reached");
            }
        }
    }
    if let Some(why) = end {
        flush_and_terminate(why);
    }
}

/// BUG(original): levplay.c `RunDemoMode` computes the first replayed key's
/// debounce from `playKeyList[(curPlayKey-1)*3+1]` with `curPlayKey == 0`,
/// i.e. from the heap block header 8 bytes before the `fileLoad` buffer,
/// which differs from run to run (the first press of a demo is sometimes
/// lost). For a deterministic oracle the list is replayed from a copy whose
/// two leading words are 0 ("nothing was pressed before"), so the first
/// press always registers; the game's own buffer is put back before
/// `RunDemoMode` frees it.
unsafe fn fix_demo_keys(st: &mut Oracle, mode: u32) {
    let list: u32 = rd(A::PLAY_KEY_LIST);
    match &st.demo_keys {
        None if mode == A::mode::DEMO && list != 0 && rd::<u32>(A::CUR_PLAY_KEY) == 0 => {
            let n: u32 = rd(A::PLAY_KEY_COUNT);
            let mut v = vec![0u32; 2 + 3 * n as usize];
            core::ptr::copy_nonoverlapping(list as usize as *const u32, v.as_mut_ptr().add(2), 3 * n as usize);
            let ours = v.as_ptr().add(2) as usize as u32;
            wr(A::PLAY_KEY_LIST, ours);
            flog!("oracle: demo key list ({} keys) replayed from a copy with defined playKeyList[-1]", n);
            st.demo_keys = Some((v, list));
        }
        Some((v, orig)) if rd::<i32>(A::QUITTING_DEMO) != 0 || mode != A::mode::DEMO => {
            if list == v.as_ptr().add(2) as usize as u32 {
                wr(A::PLAY_KEY_LIST, *orig);
                flog!("oracle: demo key list restored before the game frees it");
            }
            st.demo_keys = None;
        }
        _ => {}
    }
}

/// Can `len` bytes at `addr` be read?
unsafe fn readable(addr: u32, len: u32) -> bool {
    addr != 0 && ffi::IsBadReadPtr(addr as usize as *const c_void, len as usize) == 0
}

/// Breakpoint snapshot: raw bytes of `snap.regions` -> `f2-snap-<tick>.json`.
unsafe fn write_snapshot(regions: &[crate::cfg::SnapRegion], seq: u32, afc: u32, mode: u32) {
    let mut out = Vec::with_capacity(regions.len());
    for r in regions {
        let (ptr, base) = if r.deref {
            let p = if readable(r.addr, 4) { Some(rd::<u32>(r.addr)) } else { None };
            (p, p.unwrap_or(0))
        } else {
            (None, r.addr)
        };
        let bytes = readable(base, r.len).then(|| core::slice::from_raw_parts(base as usize as *const u8, r.len as usize).to_vec());
        out.push(trace::SnapRegion { name: r.name.clone(), addr: r.addr, len: r.len, deref: r.deref, ptr, bytes });
    }
    let path = run_dir().join(format!("f2-snap-{afc}.json"));
    match std::fs::write(&path, trace::snapshot_json(seq, afc, mode, &out)) {
        Ok(()) => flog!("oracle: snapshot {} ({} regions, line {})", path.display(), out.len(), seq),
        Err(e) => flog!("oracle: snapshot {}: {}", path.display(), e),
    }
}

unsafe fn actor_pos(actor2: u32) -> Option<[i16; 3]> {
    if actor2 == 0 {
        return None;
    }
    let actor: u32 = rd(actor2);
    if actor == 0 {
        return None;
    }
    let p = actor + A::ACTOR_POSITION;
    Some([rd(p), rd(p + 2), rd(p + 4)])
}

/// Nodes of an intrusive circular list, starting at `head.next`.
unsafe fn walk(head: u32, next_off: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut p: u32 = rd(head + next_off);
    while p != 0 && p != head && out.len() < 4096 {
        out.push(p);
        p = rd(p + next_off);
    }
    out
}

unsafe fn read_frame() -> Frame {
    let first_tile: u32 = rd(A::FIRST_TILE);
    let tile_count: i32 = rd(A::TILE_COUNT);
    let tile = |p: u32| trace::tile_index(p, first_tile, tile_count, A::GAMETILE_SIZE);
    let mut f = Frame {
        act_frame_count: rd(A::ACT_FRAME_COUNT),
        frame_count: rd(A::FRAME_COUNT),
        game_speed: rd(A::GAME_SPEED),
        mode: rd(A::GAME_STATE + A::game_state::MODE),
        multi: rd(A::GAME_STATE + A::game_state::MULTI),
        single: rd(A::GAME_STATE + A::game_state::SINGLE),
        world: rd(A::PLAYER + A::player::WORLD_NUM),
        level: rd(A::PLAYER + A::player::LEVEL_NUM),
        rseed: rd(A::RSEED),
        babies_saved: rd(A::BABIES_SAVED),
        num_babies: rd(A::NUM_BABIES),
        end_level_timer: rd(A::END_LEVEL_TIMER),
        ..Default::default()
    };
    for i in 0..4u32 {
        f.pads[i as usize] = rd(A::PAD_DIGITAL + 2 * i);
    }
    for i in 0..3u32 {
        f.cam_source[i as usize] = rd(A::CURR_CAM_SOURCE + 4 * i);
        f.cam_target[i as usize] = rd(A::CURR_CAM_TARGET + 4 * i);
    }
    let n = rd::<i32>(A::NUM_FROGS).clamp(0, 4) as u32;
    for i in 0..n {
        let p = A::PLAYER + i * A::PLAYER_SIZE;
        use A::player as P;
        f.players.push(trace::Player {
            pos: actor_pos(rd(A::FROG + 4 * i)),
            frog_state: rd(p + P::FROG_STATE),
            curr_tile: tile(rd(A::CURR_TILE + 4 * i)),
            lives: rd(p + P::LIVES),
            health: rd(p + P::HEALTH_POINTS),
            death_by: rd(p + P::DEATH_BY),
            can_jump: rd(p + P::CAN_JUMP),
            has_jumped: rd(p + P::HAS_JUMPED),
            is_super_hopping: rd(p + P::IS_SUPER_HOPPING),
            is_sinking: rd(p + P::IS_SINKING),
            num_spawn: rd(p + P::NUM_SPAWN),
            safe_time: rd(p + P::SAFE_TIME),
            dead_time: rd(p + P::DEAD_TIME),
            jump_time: rd(p + P::JUMP_TIME),
            height_jumped: rd(p + P::HEIGHT_JUMPED),
        });
    }
    for e in walk(A::ENEMY_LIST + A::LIST_HEAD, A::enemy::NEXT) {
        use A::enemy as E;
        f.enemies.push(trace::Enemy {
            uid: rd(e + E::UID),
            active: rd(e + E::ACTIVE),
            pos: actor_pos(rd(e + E::NME_ACTOR)),
            flags: rd(e + E::FLAGS),
            is_waiting: rd(e + E::IS_WAITING),
            speed: rd(e + E::SPEED),
            in_tile: tile(rd(e + E::IN_TILE)),
        });
    }
    for p in walk(A::PLATFORM_LIST + A::LIST_HEAD, A::platform::NEXT) {
        use A::platform as PL;
        f.platforms.push(trace::Platform {
            uid: rd(p + PL::UID),
            active: rd(p + PL::ACTIVE),
            pos: actor_pos(rd(p + PL::PLT_ACTOR)),
            flags: rd(p + PL::FLAGS),
            is_waiting: rd(p + PL::IS_WAITING),
            curr_speed: rd(p + PL::CURR_SPEED),
            in_tile: [tile(rd(p + PL::IN_TILE)), tile(rd(p + PL::IN_TILE + 4))],
        });
    }
    f.garib_count = rd(A::GARIB_LIST + A::garib_list::COUNT);
    f.garib_total = rd(A::GARIB_LIST + A::garib_list::TOTAL);
    for g in walk(A::GARIB_LIST + A::garib_list::HEAD, A::garib::NEXT) {
        let p = g + A::garib::POS;
        f.garibs.push(trace::Garib { kind: rd(g + A::garib::TYPE), active: rd(g + A::garib::ACTIVE), pos: [rd(p), rd(p + 2), rd(p + 4)] });
    }
    f
}

unsafe fn svec(p: u32) -> [i16; 3] {
    [rd(p), rd(p + 2), rd(p + 4)]
}

unsafe fn fvec(p: u32) -> [i32; 3] {
    [rd(p), rd(p + 4), rd(p + 8)]
}

unsafe fn read_camera() -> trace::Camera {
    let tcs: u32 = rd(A::TC_LIST);
    let n = rd::<i16>(A::TC_LIST + A::TC_LIST_COUNT).clamp(0, 4096) as u32;
    let tile: u32 = rd(A::CURR_TILE);
    let mut trans = -1;
    if tcs != 0 && tile != 0 && readable(tcs, n * A::TRANSCAMERA_SIZE) {
        // cam.c CheckForDynamicCameraChange: for(i=count; i; i--) cur=&array[i-1] -> highest index first.
        if let Some(i) = (0..n).rev().find(|&i| rd::<u32>(tcs + i * A::TRANSCAMERA_SIZE) == tile) {
            trans = i as i32;
        }
    }
    trace::Camera {
        source: fvec(A::CAM_SOURCE),
        target: fvec(A::CAM_TARGET),
        cam_vect: fvec(A::CAM_VECT),
        curr_cam_dist: fvec(A::CURR_CAM_DIST),
        cam_dist: svec(A::CAM_DIST),
        curr_cam_offset: fvec(A::CURR_CAM_OFFSET),
        cam_look_ofs: rd(A::CAM_LOOK_OFS),
        trans_cam_speed_mult: rd(A::TRANS_CAM_SPEED_MULT),
        y_fov_new: rd(A::Y_FOV_NEW),
        fixed: [rd(A::FIXED_DIR), rd(A::FIXED_POS), rd(A::FIXED_UP)],
        cam_shakiness: rd(A::CAM_SHAKINESS),
        trans,
        trans_count: rd(A::TC_LIST + A::TC_LIST_COUNT),
        fog: [
            rd::<u8>(A::FOG) as i32,
            rd::<u8>(A::FOG + 1) as i32,
            rd::<u8>(A::FOG + 2) as i32,
            rd::<u8>(A::FOG + 3) as i32,
            rd::<i16>(A::FOG + 4) as i32,
            rd::<i16>(A::FOG + 6) as i32,
        ],
    }
}

/// Schema v2 sections (`trace.version=2`); `f` is this tick's v1 frame.
unsafe fn read_v2(f: &Frame) -> trace::V2 {
    let mut v = trace::V2::default();
    for i in 0..f.players.len() as u32 {
        let p = A::PLAYER + i * A::PLAYER_SIZE;
        use A::player_ext as P;
        v.players_ext.push(trace::PlayerExt {
            frog_facing: rd(A::FROG_FACING + 4 * i),
            cam_facing: rd(A::CAM_FACING + 4 * i),
            frogon: rd(p + P::FROGON),
            frogunder: rd(p + P::FROGUNDER),
            old_lives: rd(p + P::OLD_LIVES),
            input_pause: rd(p + P::INPUT_PAUSE),
            spawn_timer: rd(p + P::SPAWN_TIMER),
            spawn_score_level: rd(p + P::SPAWN_SCORE_LEVEL),
            idle_time: rd(p + P::IDLE_TIME),
            idle_enable: rd(p + P::IDLE_ENABLE),
            has_double_jumped: rd(p + P::HAS_DOUBLE_JUMPED),
            extended_hop_dir: rd(p + P::EXTENDED_HOP_DIR),
            stun_time: rd(p + P::STUN_TIME),
            croak_time: rd(p + P::CROAK_TIME),
            autohop_time: rd(p + P::AUTOHOP_TIME),
            quickhop_time: rd(p + P::QUICKHOP_TIME),
            slowhop_time: rd(p + P::SLOWHOP_TIME),
            jump_start_frame: rd(p + P::JUMP_START_FRAME),
            jump_speed: rd(p + P::JUMP_SPEED),
            jump_multiplier: rd(p + P::JUMP_MULTIPLIER),
            jump_fwd: fvec(p + P::JUMP_FWD),
            jump_up: fvec(p + P::JUMP_UP),
            jump_origin: svec(p + P::JUMP_ORIGIN),
        });
        let t = A::TONGUE + i * A::TONGUE_SIZE;
        use A::tongue as T;
        v.tongue.push(trace::Tongue {
            flags: rd(t + T::FLAGS),
            radius: rd(t + T::RADIUS),
            progress: rd(t + T::PROGRESS),
            kind: rd(t + T::TYPE),
            can_tongue: rd(t + T::CAN_TONGUE),
            source: svec(t + T::SOURCE),
            target: svec(t + T::TARGET),
            pos: svec(t + T::POS),
            has_thing: (rd::<u32>(t + T::THING) != 0) as u8,
        });
    }
    v.garib_ext = trace::GaribExt {
        blocks: rd(A::GARIB_LIST + A::GARIB_LIST_BLOCKS),
        max_coins: rd(A::GARIB_LIST + A::garib_list::MAX_COINS),
        pause_flag: rd(A::GARIB_PAUSE_FLAG),
        pause_timer: rd(A::GARIB_PAUSE_TIMER),
        drop_speed: walk(A::GARIB_LIST + A::garib_list::HEAD, A::garib::NEXT).into_iter().map(|g| rd(g + A::GARIB_DROP_SPEED)).collect(),
    };
    let nb = rd::<i32>(A::NUM_BABIES).clamp(0, A::NUM_BABIES_MAX as i32) as u32;
    for i in 0..nb {
        let b = A::BABY_LIST + i * A::BABY_SIZE;
        v.babies.push(trace::Baby {
            is_saved: rd(b + A::baby::IS_SAVED),
            idle: rd(b + A::baby::IDLE),
            collect: rd(b + A::baby::COLLECT),
            pos: actor_pos(rd(b + A::baby::BABY)),
        });
    }
    v.camera = read_camera();
    for i in 0..8u32 {
        v.script_flags[i as usize] = rd(A::SCRIPT_FLAGS + i);
    }
    for i in 0..16u32 {
        v.script_counters[i as usize] = rd(A::SCRIPT_COUNTERS + 2 * i);
    }
    v.trigger_count = rd(A::TRIGGER_LIST + A::TRIGGER_LIST_COUNT);
    for t in walk(A::TRIGGER_LIST, A::trigger::NEXT) {
        v.triggers.push(trace::Trigger { flags: rd(t + A::trigger::FLAGS), count: rd(t + A::trigger::COUNT), delay: rd(t + A::trigger::DELAY) });
    }
    v.specfx_count = rd(A::SFX_LIST + A::SFX_LIST_COUNT);
    for x in walk(A::SFX_LIST + A::SFX_LIST_HEAD, A::specfx::NEXT) {
        use A::specfx as S;
        v.specfx.push(trace::SpecFx { kind: rd(x + S::TYPE), origin: svec(x + S::ORIGIN), lifetime: rd(x + S::LIFETIME), update_type: rd(x + S::UPDATE_TYPE) });
    }
    v.sprite_count = rd(A::SPR_LIST + A::POOL_COUNT);
    v.particle_count = rd(A::PART_LIST + A::POOL_COUNT);
    v.hud = trace::Hud {
        score_timer: rd(A::SCORE_TIMER),
        mode_timer: rd(A::MODE_TIMER),
        go_timer: rd(A::GO_TIMER),
        demo_timeout: rd(A::DEMO_TIMEOUT),
        hand_angle: rd(A::HAND_ANGLE),
        store_frame_count: rd(A::STORE_FRAME_COUNT),
    };
    v
}
