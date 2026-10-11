//! Heap budget of the simulation for the N64 port (`docs/N64-PLAN.md` §3,
//! `research/n64-memory.md`). A counting global allocator around the system one measures,
//! for the frontend and every single-player level:
//!
//! * `loadpk`: peak heap while `InitLevel` runs (whole files read and parsed, then mostly
//!   dropped), `xient`: the largest single `DataSource` call's own peak (which file);
//! * `live`: heap the simulation keeps after the load, split per `Game` component;
//! * `playpk`: peak over `ticks` of scripted play (`reloads` = level restarts in that run,
//!   each one a fresh `InitLevel`);
//! * `objbank`/`texbank`: live bytes of the parsed object/texture banks a renderer keeps for
//!   the level (`f2-formats` structures), `tex16`: their texels as stored (16 bit).
//!
//! With `F2_MEM_STRIP_WUS=1` the sound banks are pre-parsed (outside the measurement) with their
//! PCM dropped: the simulation reads only `uid`/`looped` (`load_sfx_set`), which is what an
//! N64 data source would hand it, so `loadpk` then shows the load peak without the PCM.
//!
//! `cargo run --release -p f2-game --example mem_budget [ticks]` (default 1800 ticks/level).
//! Game data: `F2_GAME_DIR` (default `game-data/usa/game`; `.../usa` is accepted).
//! Sizes are for this host (64-bit pointers); see the research note for 32-bit scaling.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

use f2_formats::{lang::StringTable, objbank::ObjectBank, texture::TexBank, wus::SoundBank};
use f2_game::assets::{DataSource, DirData};
use f2_game::layout::{worldid, LevelAssets, LEVELID_FRONTEND1, ORIG_WORLD_VISUAL_DATA};
use f2_game::levplay::{PAD_CROSS, PAD_DOWN, PAD_LEFT, PAD_RIGHT, PAD_UP};
use f2_game::session::SessionConfig;
use f2_game::{Game, TickInput};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards to `System` unchanged; only counts sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            let now = LIVE.fetch_add(l.size(), Relaxed) + l.size();
            PEAK.fetch_max(now, Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        LIVE.fetch_sub(l.size(), Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, new) };
        if !q.is_null() {
            if new >= l.size() {
                let now = LIVE.fetch_add(new - l.size(), Relaxed) + (new - l.size());
                PEAK.fetch_max(now, Relaxed);
            } else {
                LIVE.fetch_sub(l.size() - new, Relaxed);
            }
        }
        q
    }
}

#[global_allocator]
static A: Counting = Counting;

fn live() -> usize {
    LIVE.load(Relaxed)
}
fn peak() -> usize {
    PEAK.load(Relaxed)
}

/// Runs `f` and returns its own peak above the heap at entry; the outer peak is kept.
fn scoped_peak<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let outer = peak();
    let base = live();
    PEAK.store(base, Relaxed);
    let r = f();
    let p = peak() - base;
    PEAK.fetch_max(outer, Relaxed);
    (r, p)
}

/// `DirData` that records the largest transient (own peak) of any single call.
struct Probe {
    inner: DirData,
    worst: Mutex<(usize, String)>,
    /// `F2_MEM_STRIP_WUS`: sample banks without PCM, by lowercase path.
    wus: Option<BTreeMap<String, SoundBank>>,
}

impl Probe {
    fn note<T>(&self, what: &str, path: &str, f: impl FnOnce() -> T) -> T {
        let (r, p) = scoped_peak(f);
        let mut w = self.worst.lock().unwrap();
        if p > w.0 {
            *w = (p, format!("{what} {path}"));
        }
        r
    }
    fn take_worst(&self) -> (usize, String) {
        std::mem::take(&mut *self.worst.lock().unwrap())
    }
}

impl DataSource for Probe {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.inner.read(path)
    }
    fn object_bank(&self, path: &str) -> Option<Result<ObjectBank, String>> {
        self.note("objbank", path, || self.inner.object_bank(path))
    }
    fn texture_bank(&self, path: &str) -> Option<Result<TexBank, String>> {
        self.note("texbank", path, || self.inner.texture_bank(path))
    }
    fn sound_bank(&self, path: &str) -> Option<Result<SoundBank, String>> {
        if let Some(w) = &self.wus {
            return w.get(&path.to_ascii_lowercase()).cloned().map(Ok);
        }
        self.note("wus", path, || self.inner.sound_bank(path))
    }
    fn string_table(&self, path: &str) -> Option<Result<StringTable, String>> {
        self.note("lang", path, || self.inner.string_table(path))
    }
}

fn game_dir() -> PathBuf {
    let p = std::env::var("F2_GAME_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("game-data/usa/game"));
    if p.join("game").join("collision").is_dir() {
        p.join("game")
    } else {
        p
    }
}

fn kb(b: usize) -> String {
    format!("{:.0}", b as f64 / 1024.0)
}

/// Live bytes released by dropping each component in turn. Actors hold `Arc`s to model
/// facts, so `actor2` goes before `models` (shared data is counted under `models`).
fn components(mut g: Game) -> Vec<(&'static str, usize)> {
    let mut out = Vec::new();
    let mut step = |name: &'static str, g: &mut Game, f: &dyn Fn(&mut Game)| {
        let before = live();
        f(g);
        out.push((name, before - live()));
    };
    step("actor2", &mut g, &|g| drop(std::mem::take(&mut g.actor2)));
    step("models", &mut g, &|g| drop(std::mem::take(&mut g.models)));
    step("map", &mut g, &|g| drop(std::mem::take(&mut g.map)));
    step("level_files", &mut g, &|g| drop(std::mem::take(&mut g.level_files)));
    step("language", &mut g, &|g| drop(std::mem::take(&mut g.language)));
    step("script+event", &mut g, &|g| {
        drop(std::mem::take(&mut g.script));
        drop(std::mem::take(&mut g.event));
    });
    step("enemies+platform+path", &mut g, &|g| {
        drop(std::mem::take(&mut g.enemies));
        drop(std::mem::take(&mut g.platform));
        drop(std::mem::take(&mut g.path));
    });
    step("specfx+sprite+particle", &mut g, &|g| {
        drop(std::mem::take(&mut g.specfx));
        drop(std::mem::take(&mut g.sprite));
        drop(std::mem::take(&mut g.particle));
    });
    step("ptexture", &mut g, &|g| drop(std::mem::take(&mut g.ptexture)));
    step("pcaudio+general", &mut g, &|g| {
        drop(std::mem::take(&mut g.pcaudio));
        drop(std::mem::take(&mut g.general));
    });
    let before = live();
    drop(g);
    out.push(("rest", before - live()));
    out
}

/// Live bytes of the parsed banks a renderer keeps for the level, and their texel bytes.
fn render_banks(data: &dyn DataSource, a: &LevelAssets) -> (usize, usize, usize) {
    let base = live();
    let objs: Vec<_> = a.object_banks.iter().filter_map(|p| data.object_bank(p)).filter_map(Result::ok).collect();
    let obj_bytes = live() - base;
    let base = live();
    let texs: Vec<_> = a.texture_banks.iter().filter_map(|p| data.texture_bank(p)).filter_map(Result::ok).collect();
    let tex_bytes = live() - base;
    let texels: usize = texs.iter().flat_map(|b| &b.textures).map(|t| t.pixels.len()).sum();
    drop((objs, texs));
    (obj_bytes, tex_bytes, texels * 2)
}

/// Deterministic pad stream: a direction (sometimes with a superhop) every 12 ticks.
fn pad(t: u32, s: &mut u32) -> u16 {
    if !t.is_multiple_of(12) {
        return 0;
    }
    *s ^= *s << 13;
    *s ^= *s >> 17;
    *s ^= *s << 5;
    const DIRS: [u16; 4] = [PAD_UP, PAD_DOWN, PAD_LEFT, PAD_RIGHT];
    DIRS[(*s % 4) as usize] | if *s & 0x30 == 0 { PAD_CROSS } else { 0 }
}

fn start(probe: &Arc<Probe>, w: u32, l: u32) -> (Game, usize, Option<LevelAssets>) {
    let m0 = live();
    let mut g = Game::new_session(SessionConfig::new(probe.clone()));
    let session = live() - m0;
    if w == worldid::FRONTEND {
        g.start_frontend();
    } else {
        g.start_arcade(w, l);
    }
    let assets = g.take_level_loads().pop().map(|ld| ld.assets);
    g.take_effects();
    (g, session, assets)
}

fn main() -> Result<(), String> {
    let ticks: u32 = std::env::args().nth(1).map_or(Ok(1800), |s| s.parse()).map_err(|e| format!("ticks: {e}"))?;
    let dir = game_dir();
    if !dir.join("collision").is_dir() {
        return Err(format!("no game data at {}", dir.display()));
    }
    println!("size_of::<Game>() = {} bytes (this host, {}-bit)", std::mem::size_of::<Game>(), usize::BITS);
    let wus = std::env::var_os("F2_MEM_STRIP_WUS").map(|_| {
        let mut m = BTreeMap::new();
        for e in std::fs::read_dir(dir.join("sfx")).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().to_ascii_lowercase();
            if let (true, Ok(raw)) = (name.ends_with(".wus"), std::fs::read(e.path())) {
                if let Ok(mut b) = SoundBank::parse(&raw) {
                    b.samples.iter_mut().for_each(|s| s.pcm = Vec::new());
                    m.insert(format!("sfx\\{name}"), b);
                }
            }
        }
        m
    });
    println!("sound banks: {}", if wus.is_some() { "pre-parsed, PCM stripped (F2_MEM_STRIP_WUS)" } else { "parsed at load with PCM" });
    let probe = Arc::new(Probe { inner: DirData(dir), worst: Mutex::new((0, String::new())), wus });
    let mut levels: Vec<(u32, u32, String)> = vec![(worldid::FRONTEND, LEVELID_FRONTEND1, "frontend".into())];
    for w in ORIG_WORLD_VISUAL_DATA.iter() {
        if w.world_id == worldid::FRONTEND || w.world_id == worldid::TEST {
            continue;
        }
        for l in 0..w.num_single_player_levels as usize {
            let name: String = w.level_visual_data[l].tex_crc.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect();
            levels.push((w.world_id, l as u32, name));
        }
    }
    println!(
        "{:<6} {:<9} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} | largest transient; live components (KiB)",
        "level", "name", "session", "loadpk", "xient", "live", "playpk", "objbank", "texbank", "tex16"
    );
    let base = live();
    for (w, l, name) in levels {
        // 1. load: peak, live, components
        let m0 = live();
        PEAK.store(m0, Relaxed);
        probe.take_worst();
        let (g, session, assets) = start(&probe, w, l);
        let load_peak = peak() - m0;
        let (xient, xfile) = probe.take_worst();
        let Some(assets) = assets else {
            println!("{w}-{l}: not loaded");
            continue;
        };
        let live_after = live() - m0;
        let parts = components(g);
        // 2. play from a fresh load
        let (mut g, _, _) = start(&probe, w, l);
        let m1 = live();
        PEAK.store(m1, Relaxed);
        let mut s = 0x1234_5678u32;
        let mut reloads = 0;
        for t in 0..ticks {
            g.tick(TickInput { pads: [pad(t, &mut s), 0, 0, 0] });
            reloads += g.take_level_loads().len();
            g.take_effects();
        }
        let play_peak = peak() - m1 + (m1 - m0);
        drop(g);
        let (ob, tb, tex16) = render_banks(&*probe, &assets);
        let comp: Vec<String> = parts.iter().filter(|(_, b)| *b >= 1024).map(|(n, b)| format!("{n} {}", kb(*b))).collect();
        println!(
            "{:<6} {:<9} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} | {} ({}); reloads {}; {}",
            format!("{w}-{l}"),
            name.trim_start_matches("LEV_"),
            kb(session),
            kb(load_peak),
            kb(xient),
            kb(live_after),
            kb(play_peak),
            kb(ob),
            kb(tb),
            kb(tex16),
            xfile,
            kb(xient),
            reloads,
            comp.join(", ")
        );
    }
    println!("live heap outside sessions: {} KiB at start, {} KiB at end", kb(base), kb(live()));
    Ok(())
}
