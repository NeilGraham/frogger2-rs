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
    let want_loop = c.trace_enable || c.start_demo.is_some() || c.start_level.is_some();
    if !c.time_fixed && !want_loop {
        return;
    }
    let mut st = Oracle { trace: None, seq: 0, fired: false, in_level: false, keys: None, loops: 0, demo_keys: None };
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
        } else if st.trace.is_some() && (st.in_level || !c.trace_from_level) {
            let mut f = read_frame();
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
