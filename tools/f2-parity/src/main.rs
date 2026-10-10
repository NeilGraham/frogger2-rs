//! `f2-parity`: a reproducible parity corpus beyond the attract demos.
//!
//! For every single-player level of the PC level table it generates a seeded input script
//! (`.key`), runs the retail oracle in scenario mode on it (`tools/run/run.py --trace
//! --trace-version 2 --level W-L --keys K --exit-ticks N`, cached by script hash), runs the
//! Rust simulation with the same start (`f2-game` example `scenario_trace`), diffs the two
//! traces and writes a summary table to `target/parity/report.md`.
//!
//! ```text
//! f2-parity run   [--levels W-L,..] [--ticks N] [--seed S] [--oracle-jobs 2] [--jobs 4]
//!                 [--refresh] [--no-oracle] [--update-baseline] [--sim PATH] [--dll PATH] [--cache DIR]
//! f2-parity check [--baseline FILE] [--levels W-L,..] [--update] [--sim PATH] [--cache DIR]
//! f2-parity gen   [--levels W-L,..] [--ticks N] [--seed S]
//! ```
//!
//! Local multiplayer cases (`W-L-mpN`, N = 2 or 4 frogs) cover every level of the
//! frontend's multiplayer table (`scenario_trace --list-multi`: 1-3/3-0 race, 5-3..5 battle,
//! 7-10..12 collect) in its own mode (`W-L-mpN`) and in the two other modes
//! (`W-L-mpN-race|battle|collect`, `multiGameTypes[W]` overridden before `GoMulti`), with
//! one seeded script stream per player
//! (`script::GENERATOR_MP`); the oracle starts them with `run.py --level W-L --multi N`
//! (f2fix `start.multi`, the retail `GoMulti`) and the traces carry the `mp` section.
//! `--corpus single|multi|finish|all` (default all) selects the case families.
//!
//! The **finish** family (`fin-W-L`, `fin-W-L-mpN`) covers the end states random input
//! never reaches: single-player levels completed (every baby saved, the level-complete
//! screen, continue, `SaveGame`, the next start) and local multiplayer matches played to a
//! match winner (race laps finished, battle/collect rounds won, back to the frontend).
//! Their scripts are planned with the Rust simulation itself (`scenario_trace --plan`, a
//! receding-horizon search over the deterministic sim) and committed in
//! `tools/f2-parity/finish/` (`manifest.json` + `.key`), so the corpus does not move when
//! the simulation changes; `f2-parity plan [--levels fin-..]` regenerates them. The report
//! adds per case the end state seen in each trace (level complete, babies, next level,
//! wins, match winner, frontend) and whether the `savegame.dat` the oracle wrote (run.py
//! collects it) equals the Rust `SaveGame` block (`F2_SAVE_OUT`).
//!
//! `check` needs no Windows: it compares fresh Rust traces with the cached oracle traces
//! (`target/parity/oracle/`, or `--cache DIR`; a worktree falls back to the main checkout's)
//! and exits 1 when a level matches fewer ticks than recorded in
//! `tools/f2-parity/baseline.json` (levels without a cached oracle trace are skipped).
//! Exit status: 0 ok, 1 regression (check), 2 usage / I/O error.

mod script;

use f2_trace::diff::{self, FieldDiff, Options, Report};
use f2_trace::TraceFrame;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

const TRACE_VERSION: u32 = 2;
const DEFAULT_TICKS: u32 = 1800;
const DEFAULT_SEED: u64 = 1;

struct Ctx {
    root: PathBuf,
    /// `target/parity` (or `$F2_PARITY_DIR`).
    dir: PathBuf,
    /// Oracle trace cache (`<dir>/oracle`, `--cache DIR`; a worktree without its own
    /// cache uses the main checkout's `target/parity/oracle`).
    oracle_dir: PathBuf,
    sim: PathBuf,
    game_dir: PathBuf,
}

#[derive(Clone, Debug)]
struct Opts {
    levels: Option<Vec<String>>,
    ticks: u32,
    seed: u64,
    oracle_jobs: usize,
    jobs: usize,
    refresh: bool,
    no_oracle: bool,
    update: bool,
    baseline: Option<PathBuf>,
    sim: Option<PathBuf>,
    dll: Option<PathBuf>,
    cache: Option<PathBuf>,
    corpus: Corpus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Corpus {
    Single,
    Multi,
    Finish,
    All,
}

/// Generator id of the planned finish scripts (cache key; bump when re-planned).
const GENERATOR_FINISH: &str = "f2-parity-finish-1";

/// `tools/f2-parity/finish/manifest.json`: the planned finish cases.
#[derive(Serialize, Deserialize, Default)]
struct Manifest {
    generator: String,
    cases: BTreeMap<String, FinishCase>,
}

#[derive(Serialize, Deserialize, Clone)]
struct FinishCase {
    world: u32,
    level: u32,
    players: u32,
    #[serde(default)]
    mp_mode: u32,
    /// Trace length (ticks) the case is run for.
    ticks: u32,
    /// Script file in `tools/f2-parity/finish/`.
    key: String,
    /// Planner options used (`scenario_trace --plan-seed/--plan-rivals`).
    seed: u64,
    rivals: String,
    /// The planner's summary line.
    plan: String,
}

fn manifest_path(root: &Path) -> PathBuf {
    root.join("tools/f2-parity/finish/manifest.json")
}

fn read_manifest(root: &Path) -> Manifest {
    std::fs::read(manifest_path(root)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Player counts of the multiplayer corpus.
const MP_PLAYERS: [u32; 2] = [2, 4];

#[derive(Serialize, Deserialize, Default)]
struct Baseline {
    generator: String,
    seed: u64,
    ticks: u32,
    trace_version: u32,
    /// Level `W-L` -> leading ticks identical to the oracle (`matched`) and the oracle length.
    levels: BTreeMap<String, BaseLevel>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct BaseLevel {
    matched: usize,
    oracle_ticks: usize,
    /// Finish cases: the oracle's end state and whether the Rust end state (and save) matched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    end_match: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    save: Option<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct OracleMeta {
    level: String,
    key: String,
    rseed: Option<u32>,
    lines: usize,
    /// Multiplayer: the frontend's `fogStore` when `GoMulti` ran (f2fix log).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fog_store: Option<i32>,
}

/// One corpus entry: `level` is the case id (`W-L`, or `W-L-mpN` for N local players).
struct Case {
    level: String,
    world: u32,
    lvl: u32,
    /// 0 = single-player arcade, 2..=4 = local multiplayer frogs.
    players: u32,
    /// Multiplayer mode override (1 race, 2 battle, 3 collect; 0 = the level's own).
    mp_mode: u32,
    keys: PathBuf,
    key: String,
    /// Trace length (`--ticks`, or the finish manifest's).
    ticks: u32,
    /// A planned finish case (`fin-...`).
    finish: bool,
}

struct Row {
    level: String,
    oracle_ticks: Option<usize>,
    rust_ticks: Option<usize>,
    matched: Option<usize>,
    identical: bool,
    first: Option<First>,
    sections: Vec<(String, u32, usize)>,
    note: String,
    finish: bool,
    /// End state seen in the oracle / Rust trace (finish cases).
    end_oracle: Option<String>,
    end_rust: Option<String>,
    /// `savegame.dat` comparison (finish cases).
    save: Option<String>,
}

struct First {
    afc: u32,
    section: String,
    field: String,
    a: String,
    b: String,
    nfields: usize,
    module: String,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match real_main(&args) {
        Ok(c) => ExitCode::from(c),
        Err(e) => {
            eprintln!("f2-parity: {e}");
            ExitCode::from(2)
        }
    }
}

const USAGE: &str = "usage:
  f2-parity run   [--levels W-L[-mpN],..] [--corpus single|multi|finish|all] [--ticks N] [--seed S] [--oracle-jobs N] [--jobs N] [--refresh] [--no-oracle] [--update-baseline] [--sim PATH] [--dll PATH] [--cache DIR]
  f2-parity check [--baseline FILE] [--levels W-L[-mpN],..] [--corpus single|multi|finish|all] [--update] [--jobs N] [--sim PATH] [--cache DIR]
  f2-parity gen   [--levels W-L[-mpN],..] [--corpus single|multi|finish|all] [--ticks N] [--seed S]
  f2-parity plan  [--levels fin-W-L[-mpN[-MODE]],..] [--jobs N] [--sim PATH]   (re-plan the finish scripts)";

fn parse_opts(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        levels: None,
        ticks: DEFAULT_TICKS,
        seed: DEFAULT_SEED,
        oracle_jobs: 2,
        jobs: 4,
        refresh: false,
        no_oracle: false,
        update: false,
        baseline: None,
        sim: None,
        dll: None,
        cache: None,
        corpus: Corpus::All,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--levels" => o.levels = Some(val()?.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()),
            "--ticks" => o.ticks = val()?.parse().map_err(|_| "bad --ticks")?,
            "--seed" => o.seed = val()?.parse().map_err(|_| "bad --seed")?,
            "--oracle-jobs" => o.oracle_jobs = val()?.parse::<usize>().map_err(|_| "bad --oracle-jobs")?.clamp(1, 4),
            "--jobs" => o.jobs = val()?.parse::<usize>().map_err(|_| "bad --jobs")?.max(1),
            "--refresh" => o.refresh = true,
            "--no-oracle" => o.no_oracle = true,
            "--update" | "--update-baseline" => o.update = true,
            "--baseline" => o.baseline = Some(PathBuf::from(val()?)),
            "--sim" => o.sim = Some(PathBuf::from(val()?)),
            "--dll" => o.dll = Some(PathBuf::from(val()?)),
            "--cache" => o.cache = Some(PathBuf::from(val()?)),
            "--corpus" => {
                o.corpus = match val()?.as_str() {
                    "single" => Corpus::Single,
                    "multi" => Corpus::Multi,
                    "finish" => Corpus::Finish,
                    "all" => Corpus::All,
                    s => return Err(format!("bad --corpus {s} (single|multi|finish|all)")),
                }
            }
            s => return Err(format!("unknown argument {s}\n{USAGE}")),
        }
    }
    Ok(o)
}

fn real_main(args: &[String]) -> Result<u8, String> {
    let Some(cmd) = args.first() else { return Err(USAGE.into()) };
    let mut o = parse_opts(&args[1..])?;
    let root = repo_root()?;
    let dir = std::env::var_os("F2_PARITY_DIR").map(PathBuf::from).unwrap_or_else(|| root.join("target/parity"));
    let baseline_path = o.baseline.clone().unwrap_or_else(|| root.join("tools/f2-parity/baseline.json"));
    if cmd == "check" {
        // The corpus is defined by the baseline.
        let b = read_baseline(&baseline_path)?;
        if b.generator != script::GENERATOR || b.trace_version != TRACE_VERSION {
            return Err(format!("{}: generator/trace version mismatch ({} v{})", baseline_path.display(), b.generator, b.trace_version));
        }
        o.seed = b.seed;
        o.ticks = b.ticks;
        if o.levels.is_none() {
            o.levels = Some(b.levels.keys().cloned().collect());
        }
    }
    let sim = match &o.sim {
        Some(p) => p.clone(),
        None => build_sim(&root)?,
    };
    let oracle_dir = match &o.cache {
        Some(c) => c.clone(),
        None => default_cache(&root, &dir),
    };
    let ctx = Ctx { game_dir: game_dir(&root), root, dir, oracle_dir, sim };
    let levels = select_levels(&ctx, &o)?;
    if cmd == "plan" {
        return plan_finish(&ctx, &o, &levels);
    }
    let cases = gen_scripts(&ctx, &o, &levels)?;
    match cmd.as_str() {
        "gen" => {
            for c in &cases {
                println!("{} {} {}", c.level, c.key, c.keys.display());
            }
            Ok(0)
        }
        "run" => {
            if !o.no_oracle {
                run_oracles(&ctx, &o, &cases)?;
            }
            let rows = compare(&ctx, &o, &cases);
            let md = render(&o, &rows);
            write(&ctx.dir.join("report.md"), md.as_bytes())?;
            println!("{md}");
            println!("report: {}", ctx.dir.join("report.md").display());
            if o.update {
                let b = baseline_from(&o, &rows, Baseline::default());
                write_baseline(&baseline_path, &b)?;
                println!("baseline: {} ({} levels)", baseline_path.display(), b.levels.len());
            }
            Ok(0)
        }
        "check" => {
            let base = read_baseline(&baseline_path)?;
            let rows = compare(&ctx, &o, &cases);
            let md = render(&o, &rows);
            write(&ctx.dir.join("check.md"), md.as_bytes())?;
            let mut regress = 0;
            let mut improved = 0;
            let mut skipped = 0;
            for r in &rows {
                let Some(b) = base.levels.get(&r.level) else { continue };
                match r.matched {
                    None => {
                        skipped += 1;
                        println!("skip     {:<5} no cached oracle trace", r.level);
                    }
                    Some(m) if m < b.matched => {
                        regress += 1;
                        println!("REGRESS  {:<5} matched {} < baseline {}{}", r.level, m, b.matched, first_txt(r));
                    }
                    Some(_) if b.end_match == Some(true) && (r.end_rust != r.end_oracle || !r.save.as_deref().is_some_and(|s| s == "identical" || s == "none")) => {
                        regress += 1;
                        println!(
                            "REGRESS  {:<5} end state {} != oracle {} (save {})",
                            r.level,
                            r.end_rust.as_deref().unwrap_or("-"),
                            r.end_oracle.as_deref().unwrap_or("-"),
                            r.save.as_deref().unwrap_or("-")
                        );
                    }
                    Some(m) if m > b.matched => {
                        improved += 1;
                        println!("improved {:<5} matched {} > baseline {}", r.level, m, b.matched);
                    }
                    Some(m) => println!("ok       {:<5} matched {m}", r.level),
                }
            }
            println!("{} levels: {regress} regressed, {improved} improved, {skipped} skipped (details: {})", rows.len(), ctx.dir.join("check.md").display());
            if o.update && regress == 0 {
                let b = baseline_from(&o, &rows, base);
                write_baseline(&baseline_path, &b)?;
                println!("baseline updated: {}", baseline_path.display());
            } else if improved > 0 {
                println!("hint: `f2-parity check --update` records the improvements");
            }
            Ok(u8::from(regress > 0))
        }
        _ => Err(USAGE.into()),
    }
}

fn first_txt(r: &Row) -> String {
    r.first.as_ref().map_or(String::new(), |f| format!(" (first: afc {} {} {} != {})", f.afc, f.field, f.a, f.b))
}

// ---- paths, build ----------------------------------------------------------

fn repo_root() -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let mut p = cwd.as_path();
    loop {
        if p.join("tools/run/run.py").is_file() && p.join("Cargo.toml").is_file() {
            return Ok(p.to_path_buf());
        }
        p = p.parent().ok_or("not inside the frogger-2-recomp checkout (tools/run/run.py not found)")?;
    }
}

/// The main checkout (for gitignored inputs a worktree does not have).
fn main_checkout(root: &Path) -> Option<PathBuf> {
    let out = Command::new("git").args(["rev-parse", "--path-format=absolute", "--git-common-dir"]).current_dir(root).output().ok()?;
    let s = String::from_utf8(out.stdout).ok()?;
    Path::new(s.trim()).parent().map(Path::to_path_buf)
}

fn game_dir(root: &Path) -> PathBuf {
    if let Some(p) = std::env::var_os("F2_GAME_DIR") {
        return PathBuf::from(p);
    }
    let here = root.join("game-data/usa/game");
    if here.is_dir() {
        return here;
    }
    main_checkout(root).map(|m| m.join("game-data/usa/game")).filter(|p| p.is_dir()).unwrap_or(here)
}

/// `<dir>/oracle`, or the main checkout's cache when this (worktree) checkout has none.
fn default_cache(root: &Path, dir: &Path) -> PathBuf {
    let here = dir.join("oracle");
    if here.is_dir() || std::env::var_os("F2_PARITY_DIR").is_some() {
        return here;
    }
    main_checkout(root).map(|m| m.join("target/parity/oracle")).filter(|p| p.is_dir()).unwrap_or(here)
}

fn oracle_dll(root: &Path, o: &Opts) -> PathBuf {
    if let Some(d) = &o.dll {
        return d.clone();
    }
    let here = root.join("target/oracle-bin/f2fix.dll");
    if here.is_file() {
        return here;
    }
    main_checkout(root).map(|m| m.join("target/oracle-bin/f2fix.dll")).filter(|p| p.is_file()).unwrap_or(here)
}

/// `cargo build --release -p f2-game --example scenario_trace`; returns the executable.
fn build_sim(root: &Path) -> Result<PathBuf, String> {
    eprintln!("f2-parity: building scenario_trace (release)");
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["build", "--release", "-j8", "-p", "f2-game", "--example", "scenario_trace", "--message-format=json"])
        .current_dir(root)
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("cargo: {e}"))?;
    if !out.status.success() {
        return Err("building scenario_trace failed".into());
    }
    let mut exe = None;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if v["target"]["name"] == "scenario_trace" {
            if let Some(e) = v["executable"].as_str() {
                exe = Some(PathBuf::from(e));
            }
        }
    }
    exe.ok_or_else(|| "cargo did not report the scenario_trace executable".into())
}

fn memcap(root: &Path) -> Command {
    let m = root.join("tools/memcap.sh");
    if m.is_file() {
        Command::new(m)
    } else {
        Command::new("env")
    }
}

fn write(p: &Path, b: &[u8]) -> Result<(), String> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    std::fs::write(p, b).map_err(|e| format!("{}: {e}", p.display()))
}

// ---- corpus ------------------------------------------------------------------

/// Multiplayer modes (`multiGameTypes`, multi.h `MULTIMODE_*`) by id 1..=3.
const MP_MODES: [&str; 3] = ["race", "battle", "collect"];

/// `(world, level, players, mp_mode)` of the selected cases (players 0 = single-player;
/// mp_mode 0 = the level's own `multiGameTypes` mode, else an override).
fn select_levels(ctx: &Ctx, o: &Opts) -> Result<Vec<Sel>, String> {
    let list = |flag: &str| -> Result<Vec<(u32, u32, String)>, String> {
        let out = Command::new(&ctx.sim).arg(flag).output().map_err(|e| format!("{}: {e}", ctx.sim.display()))?;
        if !out.status.success() {
            return Err(format!("scenario_trace {flag} failed"));
        }
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| {
                let mut it = l.split_whitespace();
                let (w, lv) = parse_level(it.next()?)?;
                Some((w, lv, it.next().unwrap_or("").to_string()))
            })
            .collect())
    };
    let keep = |c: &Sel| match o.corpus {
        Corpus::All => true,
        Corpus::Single => !c.4 && c.2 == 0,
        Corpus::Multi => !c.4 && c.2 != 0,
        Corpus::Finish => c.4,
    };
    let all = match &o.levels {
        None => {
            let mut v = Vec::new();
            if o.corpus != Corpus::Multi {
                v.extend(list("--list-levels")?.into_iter().map(|(w, l, _)| (w, l, 0, 0, false)));
            }
            if matches!(o.corpus, Corpus::Multi | Corpus::All) {
                // Every multiplayer level in its own mode, plus race and collect on the others.
                // Battle on a non-battle level is left out: the retail exe dies in
                // GoMulti/InitLevel (no trace line; probably the battle trail cube model, which only the
                // laboratory banks load), a combination the frontend never starts.
                for (w, l, native) in list("--list-multi")? {
                    for &p in &MP_PLAYERS {
                        v.push((w, l, p, 0, false));
                        for (i, m) in MP_MODES.iter().enumerate() {
                            if *m != native && *m != "battle" {
                                v.push((w, l, p, i as u32 + 1, false));
                            }
                        }
                    }
                }
            }
            if matches!(o.corpus, Corpus::Finish | Corpus::All) {
                for id in read_manifest(&ctx.root).cases.keys() {
                    v.push(parse_case(id).ok_or(format!("manifest: bad case {id}"))?);
                }
            }
            v
        }
        Some(want) => want.iter().map(|w| parse_case(w).ok_or(format!("bad case {w} (want W-L, W-L-mpN or W-L-mpN-MODE)"))).collect::<Result<_, _>>()?,
    };
    Ok(all.into_iter().filter(|c| keep(c)).collect())
}

fn parse_level(s: &str) -> Option<(u32, u32)> {
    let (w, l) = s.split_once('-')?;
    Some((w.parse().ok()?, l.parse().ok()?))
}

/// A selected case: `(world, level, players, mp_mode, finish)`.
type Sel = (u32, u32, u32, u32, bool);

/// `W-L` (single-player), `W-L-mpN` (N = 2..=4 local players, the level's own mode) or
/// `W-L-mpN-MODE` (MODE = race|battle|collect overrides `multiGameTypes[W]`); a `fin-`
/// prefix selects the planned finish script of that case.
fn parse_case(s: &str) -> Option<Sel> {
    if let Some(rest) = s.strip_prefix("fin-") {
        return parse_case(rest).filter(|c| !c.4).map(|c| (c.0, c.1, c.2, c.3, true));
    }
    match s.split_once("-mp") {
        Some((wl, rest)) => {
            let (w, l) = parse_level(wl)?;
            let (n, mode) = match rest.split_once('-') {
                Some((n, m)) => (n, MP_MODES.iter().position(|x| *x == m)? as u32 + 1),
                None => (rest, 0),
            };
            let n: u32 = n.parse().ok().filter(|n| (2..=4).contains(n))?;
            Some((w, l, n, mode, false))
        }
        None => parse_level(s).map(|(w, l)| (w, l, 0, 0, false)),
    }
}

fn case_id(w: u32, l: u32, players: u32, mode: u32, finish: bool) -> String {
    if finish {
        return format!("fin-{}", case_id(w, l, players, mode, false));
    }
    match (players, mode) {
        (0, _) => format!("{w}-{l}"),
        (p, 0) => format!("{w}-{l}-mp{p}"),
        (p, m) => format!("{w}-{l}-mp{p}-{}", MP_MODES[(m - 1) as usize]),
    }
}

fn gen_scripts(ctx: &Ctx, o: &Opts, levels: &[Sel]) -> Result<Vec<Case>, String> {
    let mut v = Vec::new();
    let manifest = read_manifest(&ctx.root);
    for &(w, l, players, mp_mode, finish) in levels {
        let level = case_id(w, l, players, mp_mode, finish);
        if finish {
            let fc = manifest.cases.get(&level).ok_or(format!("{level}: not in {} (f2-parity plan)", manifest_path(&ctx.root).display()))?;
            let keys = manifest_path(&ctx.root).with_file_name(&fc.key);
            let bytes = std::fs::read(&keys).map_err(|e| format!("{}: {e}", keys.display()))?;
            let mut h = Sha1::new();
            h.update(format!("{GENERATOR_FINISH}\n{level}\nticks {}\ntrace v{TRACE_VERSION}\n", fc.ticks).as_bytes());
            h.update(&bytes);
            let key: String = h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect();
            v.push(Case { level, world: w, lvl: l, players, mp_mode, keys, key, ticks: fc.ticks, finish: true });
            continue;
        }
        let (generator, bytes) = if players == 0 {
            (script::GENERATOR, script::to_key_bytes(&script::generate(w, l, o.seed, o.ticks)))
        } else {
            (script::GENERATOR_MP, script::to_key_bytes_multi(&script::generate_multi(w, l, players, o.seed, o.ticks)))
        };
        let mut h = Sha1::new();
        h.update(format!("{generator}\n{level}\nticks {}\ntrace v{TRACE_VERSION}\nseed {}\n", o.ticks, o.seed).as_bytes());
        h.update(&bytes);
        let key: String = h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect();
        let keys = ctx.dir.join("scripts").join(format!("{level}-{key}.key"));
        if !keys.is_file() {
            write(&keys, &bytes)?;
        }
        v.push(Case { level, world: w, lvl: l, players, mp_mode, keys, key, ticks: o.ticks, finish: false });
    }
    Ok(v)
}

fn oracle_path(ctx: &Ctx, c: &Case) -> (PathBuf, PathBuf) {
    let base = ctx.oracle_dir.join(format!("{}-{}", c.level, c.key));
    (base.with_extension("jsonl"), base.with_extension("meta.json"))
}

/// Runs `jobs` workers over `items`.
fn par_for<T: Sync, R: Send>(items: &[T], jobs: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<Option<R>>> = Mutex::new((0..items.len()).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..jobs.min(items.len()).max(1) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(it) = items.get(i) else { break };
                let r = f(it);
                out.lock().unwrap()[i] = Some(r);
            });
        }
    });
    out.into_inner().unwrap().into_iter().map(|r| r.expect("worker result")).collect()
}

// ---- oracle ------------------------------------------------------------------

fn run_oracles(ctx: &Ctx, o: &Opts, cases: &[Case]) -> Result<(), String> {
    let todo: Vec<&Case> = cases.iter().filter(|c| o.refresh || !oracle_path(ctx, c).0.is_file()).collect();
    if todo.is_empty() {
        eprintln!("f2-parity: all {} oracle traces cached", cases.len());
        return Ok(());
    }
    let dll = oracle_dll(&ctx.root, o);
    if !dll.is_file() {
        return Err(format!("{} missing (tools/run/build_f2fix.sh, or --dll / --no-oracle)", dll.display()));
    }
    eprintln!("f2-parity: {} oracle run(s), {} at a time", todo.len(), o.oracle_jobs);
    let res = par_for(&todo, o.oracle_jobs, |c| {
        let r = run_oracle(ctx, &dll, c);
        match &r {
            Ok(n) => eprintln!("  oracle {:<5} {n} ticks", c.level),
            Err(e) => eprintln!("  oracle {:<5} FAILED: {e}", c.level),
        }
        r
    });
    let failed = res.iter().filter(|r| r.is_err()).count();
    if failed > 0 {
        eprintln!("f2-parity: {failed} oracle run(s) failed (reported as missing)");
    }
    Ok(())
}

fn run_oracle(ctx: &Ctx, dll: &Path, c: &Case) -> Result<usize, String> {
    let id = format!("parity-{}-{}", c.level, &c.key[..8]);
    let mut cmd = memcap(&ctx.root);
    cmd.args(["python3", "-I", "tools/run/run.py", "--id", &id, "--trace", "--trace-version", &TRACE_VERSION.to_string()])
        .args(["--level", &format!("{}-{}", c.world, c.lvl)]);
    if c.players >= 2 {
        cmd.args(["--multi", &c.players.to_string()]);
    }
    if c.mp_mode != 0 {
        cmd.args(["--mp-mode", &c.mp_mode.to_string()]);
    }
    // ~600 ticks/s in a level; FMV-free.
    let secs = 150.max(c.ticks / 150);
    let st = cmd
        .arg("--keys")
        .arg(&c.keys)
        .args(["--exit-ticks", &c.ticks.to_string(), "--exit-seconds", &secs.to_string(), "--timeout", &(secs + 50).to_string(), "--dll"])
        .arg(dll)
        .current_dir(&ctx.root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("run.py: {e}"))?;
    let run = ctx.root.join("target/oracle-runs").join(&id);
    let trace = run.join("f2-trace.jsonl");
    let text = std::fs::read_to_string(&trace).map_err(|e| format!("{} (run.py exit {st}): {e}", trace.display()))?;
    let lines = text.lines().filter(|l| !l.trim().is_empty()).count();
    if lines == 0 {
        return Err(format!("empty trace (run.py exit {st})"));
    }
    let log = std::fs::read_to_string(run.join("f2fix.log")).unwrap_or_default();
    let rseed = log.lines().find_map(|l| {
        let i = l.find("rseed before InitLevel ")?;
        l[i + 23..].trim_end_matches(')').trim().parse().ok()
    });
    let fog_store = log.lines().find_map(|l| {
        let i = l.find(", fogStore ")?;
        l[i + 11..].split(',').next()?.trim().parse().ok()
    });
    let (tp, mp) = oracle_path(ctx, c);
    write(&tp, text.as_bytes())?;
    // savegame.dat the game wrote (run.py collects it), for the finish cases.
    let save = tp.with_extension("save");
    let _ = std::fs::remove_file(&save);
    if let Ok(b) = std::fs::read(run.join("savegame.dat")) {
        write(&save, &b)?;
    }
    let meta = OracleMeta { level: c.level.clone(), key: c.key.clone(), rseed, lines, fog_store };
    write(&mp, serde_json::to_string_pretty(&meta).unwrap().as_bytes())?;
    let _ = std::fs::remove_dir_all(&run);
    Ok(lines)
}

// ---- finish scripts ------------------------------------------------------------

/// Default finish cases (`f2-parity plan` without `--levels`): every single-player level
/// and every multiplayer level in its own mode with 2 and 4 frogs.
fn default_finish(ctx: &Ctx) -> Result<Vec<Sel>, String> {
    let o = Opts { levels: None, corpus: Corpus::All, ..parse_opts(&[])? };
    Ok(select_levels(ctx, &o)?.into_iter().filter(|c| !c.4 && c.3 == 0).map(|c| (c.0, c.1, c.2, c.3, true)).collect())
}

/// `f2-parity plan`: (re)plans the finish scripts with `scenario_trace --plan` and records
/// them in the manifest. A case whose plan does not reach its goal is still recorded (its
/// summary says `goal no`), so the corpus covers how far the planner got.
fn plan_finish(ctx: &Ctx, o: &Opts, levels: &[Sel]) -> Result<u8, String> {
    let sel: Vec<Sel> = if o.levels.is_some() { levels.iter().map(|c| (c.0, c.1, c.2, c.3, true)).collect() } else { default_finish(ctx)? };
    let dir = manifest_path(&ctx.root).parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    eprintln!("f2-parity: planning {} finish case(s), {} at a time", sel.len(), o.jobs);
    let res = par_for(&sel, o.jobs, |&(w, l, players, mp_mode, _)| {
        let id = case_id(w, l, players, mp_mode, true);
        let key = format!("{}.key", &id[4..]);
        let mut cmd = memcap(&ctx.root);
        cmd.arg(&ctx.sim);
        if players >= 2 {
            cmd.args(["--multi".to_string(), players.to_string()]);
        }
        if mp_mode != 0 {
            cmd.args(["--mp-mode".to_string(), mp_mode.to_string()]);
        }
        let seed = 1u64;
        let rivals = "auto";
        let tmp = ctx.dir.join("plan").join(format!("{id}.jsonl"));
        let _ = std::fs::create_dir_all(tmp.parent().unwrap());
        cmd.args(["--plan".into(), dir.join(&key).display().to_string(), "--plan-seed".into(), seed.to_string(), "--plan-rivals".into(), rivals.to_string()])
            .args([w.to_string(), l.to_string(), "-".into()])
            .arg(&tmp)
            .arg("1")
            .env("F2_GAME_DIR", &ctx.game_dir)
            .stderr(Stdio::null());
        let r = match cmd.output() {
            Ok(out) if out.status.success() => {
                let summary = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let ticks = summary.split_whitespace().skip_while(|w| *w != "ticks").nth(1).and_then(|t| t.parse::<u32>().ok()).unwrap_or(0);
                Ok(FinishCase { world: w, level: l, players, mp_mode, ticks, key, seed, rivals: rivals.into(), plan: summary })
            }
            Ok(out) => Err(format!("scenario_trace --plan: {}", out.status)),
            Err(e) => Err(e.to_string()),
        };
        match &r {
            Ok(fc) => eprintln!("  plan {id}: {}", fc.plan),
            Err(e) => eprintln!("  plan {id}: FAILED {e}"),
        }
        (id, r)
    });
    let mut m = read_manifest(&ctx.root);
    m.generator = GENERATOR_FINISH.into();
    for (id, r) in res {
        if let Ok(fc) = r {
            m.cases.insert(id, fc);
        }
    }
    let mut s = serde_json::to_string_pretty(&m).map_err(|e| e.to_string())?;
    s.push('\n');
    write(&manifest_path(&ctx.root), s.as_bytes())?;
    println!("manifest: {} ({} cases)", manifest_path(&ctx.root).display(), m.cases.len());
    Ok(0)
}

// ---- simulation + diff -------------------------------------------------------

fn compare(ctx: &Ctx, o: &Opts, cases: &[Case]) -> Vec<Row> {
    eprintln!("f2-parity: {} simulation run(s), {} at a time", cases.len(), o.jobs);
    par_for(cases, o.jobs, |c| {
        let r = compare_one(ctx, c);
        eprintln!("  rust   {:<5} {}", c.level, r.matched.map_or(r.note.clone(), |m| format!("{m} ticks matched")));
        r
    })
}

fn load(p: &Path) -> Result<Vec<TraceFrame>, String> {
    f2_trace::read_file(p)
}

fn compare_one(ctx: &Ctx, c: &Case) -> Row {
    let mut row = Row {
        level: c.level.clone(),
        oracle_ticks: None,
        rust_ticks: None,
        matched: None,
        identical: false,
        first: None,
        sections: Vec::new(),
        note: String::new(),
        finish: c.finish,
        end_oracle: None,
        end_rust: None,
        save: None,
    };
    let (tp, mp) = oracle_path(ctx, c);
    let meta: OracleMeta = std::fs::read(&mp).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let out = ctx.dir.join("rust").join(format!("{}.jsonl", c.level));
    let rust_save = out.with_extension("save");
    let _ = std::fs::remove_file(&rust_save);
    let _ = std::fs::create_dir_all(out.parent().unwrap());
    let mut cmd = memcap(&ctx.root);
    cmd.arg(&ctx.sim);
    if c.players >= 2 {
        cmd.args(["--multi".to_string(), c.players.to_string()]);
    }
    if c.mp_mode != 0 {
        cmd.args(["--mp-mode".to_string(), c.mp_mode.to_string()]);
    }
    if let Some(f) = meta.fog_store {
        cmd.env("F2_FOG_STORE", f.to_string());
    }
    cmd.args([c.world.to_string(), c.lvl.to_string()])
        .arg(&c.keys)
        .arg(&out)
        .arg(c.ticks.to_string())
        .env("F2_GAME_DIR", &ctx.game_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(s) = meta.rseed {
        cmd.env("F2_START_RSEED", s.to_string());
    }
    if c.finish {
        cmd.env("F2_SAVE_OUT", &rust_save);
    }
    let rust = match cmd.output() {
        Ok(r) if r.status.success() => load(&out),
        Ok(r) => Err(format!("scenario_trace {}: {}", r.status, String::from_utf8_lossy(&r.stderr).lines().last().unwrap_or(""))),
        Err(e) => Err(e.to_string()),
    };
    let rust = match rust {
        Ok(v) => v,
        Err(e) => {
            row.note = format!("rust failed: {e}");
            if let Ok(a) = load(&tp) {
                row.oracle_ticks = Some(a.len());
                row.matched = Some(0);
            }
            return row;
        }
    };
    row.rust_ticks = Some(rust.len());
    let oracle = match load(&tp) {
        Ok(v) => v,
        Err(_) => {
            row.note = "no cached oracle trace".into();
            return row;
        }
    };
    row.oracle_ticks = Some(oracle.len());
    let rep = diff::diff(&oracle, &rust, &Options { max_fields: 0, ..Default::default() });
    fill_row(&mut row, &rep, &oracle);
    if c.finish {
        row.end_oracle = Some(end_state(&oracle, c.ticks));
        row.end_rust = Some(end_state(&rust, c.ticks));
        let a = std::fs::read(tp.with_extension("save")).ok();
        let b = std::fs::read(&rust_save).ok();
        row.save = Some(
            match (a, b) {
                (None, None) => "none",
                (Some(a), Some(b)) if a == b => "identical",
                (Some(_), Some(_)) => "DIFFERS",
                (Some(_), None) => "oracle only",
                (None, Some(_)) => "rust only",
            }
            .to_string(),
        );
    }
    row
}

/// The end state a trace reached: single player `complete@AFC babies S/N` (first
/// `LEVELCOMPLETE_MODE` line), `next W-L@LINE` (a level started again afterwards);
/// multiplayer `rounds R wins [..] match P`; `frontend@N` when the run ended in the
/// frontend before its tick limit.
fn end_state(fr: &[TraceFrame], ticks: u32) -> String {
    const LEVELCOMPLETE_MODE: u32 = 3;
    let mut parts: Vec<String> = Vec::new();
    let Some(last) = fr.last() else { return "empty".into() };
    if fr.iter().all(|f| f.mp.is_none()) {
        let max_saved = fr.iter().map(|f| f.babies_saved).max().unwrap_or(0);
        match fr.iter().position(|f| f.mode == LEVELCOMPLETE_MODE) {
            Some(i) => {
                parts.push(format!("complete@{} babies {}/{}", fr[i].act_frame_count, fr[i].babies_saved, fr[i].num_babies));
                if let Some(j) = (i + 1..fr.len()).find(|&j| fr[j].act_frame_count < fr[j - 1].act_frame_count) {
                    parts.push(format!("next {}-{}@{}", fr[j].world, fr[j].level, j));
                }
            }
            None => parts.push(format!("babies {max_saved}/{} lives {}", last.num_babies, last.players.first().map_or(0, |p| p.lives))),
        }
    } else {
        let mut rounds = 0;
        let mut prev = -1;
        for f in fr {
            if let Some(m) = &f.mp {
                if m.game_winner != -1 && prev == -1 {
                    rounds += 1;
                }
                prev = m.game_winner as i32;
            }
        }
        let m = fr.iter().rev().find_map(|f| f.mp.as_ref());
        let wins: Vec<i16> = m.map_or(Vec::new(), |m| m.mpl.iter().map(|p| p.wins).collect());
        parts.push(format!("rounds {rounds} wins {wins:?} match {}", m.map_or(-1, |m| m.match_winner as i32)));
    }
    if (fr.len() as u32) < ticks {
        parts.push(format!("frontend@{}", fr.len()));
    }
    parts.join(" ")
}

fn fill_row(row: &mut Row, rep: &Report, oracle: &[TraceFrame]) {
    row.identical = rep.identical();
    row.matched = Some(match &rep.first {
        None => rep.compared,
        Some(d) => d.index_a.or(d.index_b).unwrap_or(0),
    });
    for (name, s) in &rep.sections {
        if let Some(f) = &s.first {
            row.sections.push((name.clone(), f.act_frame_count, s.diverged_frames));
        }
    }
    row.sections.sort_by_key(|s| s.1);
    if let Some(d) = &rep.first {
        let f = pick_field(&d.fields);
        let mode = d.index_a.and_then(|i| oracle.get(i)).map(|fr| fr.mode);
        row.first = Some(First {
            afc: d.act_frame_count,
            section: diff::section_of(&f.path).to_string(),
            field: f.path.clone(),
            a: short(&f.a),
            b: short(&f.b),
            nfields: d.total_fields,
            module: classify(&d.fields, mode, d.act_frame_count),
        });
    }
}

/// The most telling field of a divergent frame: not `rseed` when something else differs.
fn pick_field(fields: &[FieldDiff]) -> &FieldDiff {
    fields.iter().find(|f| f.path != "rseed").unwrap_or(&fields[0])
}

fn short(v: &serde_json::Value) -> String {
    let s = v.to_string();
    if s.len() > 40 {
        format!("{}…", &s[..s.char_indices().nth(39).map_or(s.len(), |c| c.0)])
    } else {
        s
    }
}

/// Likely `f2-game` module of a first divergence (heuristic over the differing fields).
fn classify(fields: &[FieldDiff], mode: Option<u32>, afc: u32) -> String {
    let paths: Vec<&str> = fields.iter().map(|f| f.path.as_str()).collect();
    if afc <= 1 && paths.iter().any(|p| p.starts_with("enemies") || p.starts_with("platforms") || p.starts_with("garibs")) {
        return "level load (layout.rs / memload.rs: entities, InitLevel)".into();
    }
    if paths.iter().all(|p| *p == "<frame>") {
        return format!("level end / mode change: game.rs, levplay.rs (oracle mode {})", mode.map_or("-".into(), |m| m.to_string()));
    }
    let non_rng: Vec<&str> = paths.iter().copied().filter(|p| *p != "rseed").collect();
    if non_rng.is_empty() {
        return "RNG call order (Random() consumer; snapshot/F2_RANDOM_LOG)".into();
    }
    // Priority: the frog's own state explains most knock-on effects.
    let has = |pre: &str| non_rng.iter().any(|p| p.starts_with(pre));
    let leaf = |p: &str| p.rsplit('.').next().unwrap_or(p).to_string();
    let player_fields: Vec<String> = non_rng.iter().filter(|p| p.starts_with("players")).map(|p| leaf(p)).collect();
    let m = if !player_fields.is_empty() {
        let pf = |names: &[&str]| player_fields.iter().any(|f| names.iter().any(|n| f.starts_with(n)));
        if pf(&["lives", "dead_time", "death_by", "health", "is_sinking", "num_spawn", "safe_time", "spawn"]) {
            "frogger.rs / frogmove.rs (death, respawn: KillFrog, CheckForDynamicCollision)"
        } else if pf(&["frog_facing", "cam_facing"]) {
            "frogmove.rs / cam.rs (facing, camera-relative input)"
        } else if pf(&["croak"]) {
            "frogger.rs / game.rs (croak: isCroaking timer, GameProcessController game.c:457)"
        } else if pf(&["stun", "autohop", "quickhop", "slowhop", "input_pause", "idle"]) {
            "frogger.rs / game.rs (player timers, GameProcessController)"
        } else {
            "frogmove.rs (hop: UpdateFroggerPos, MoveToRequestedDestination, superhop)"
        }
    } else if has("mp") {
        "multi.rs (RunMultiplayer: race/battle/collect rules, mpl[], winners)"
    } else if has("tongue") {
        "tongue.rs (UpdateFrogTongue, tongue targets)"
    } else if has("platforms") {
        "platform.rs (UpdatePlatforms, paths)"
    } else if has("enemies") {
        "enemies.rs (UpdateEnemies, paths, homing)"
    } else if has("garib") {
        "collect.rs (garibs, coins)"
    } else if has("babies") || has("num_babies") {
        "babyfrog.rs (baby pick-up)"
    } else if has("triggers") {
        "event.rs / evenfunc.rs (triggers)"
    } else if has("script") {
        "script.rs (event scripts)"
    } else if has("camera") || has("cam_") {
        "cam.rs (camera)"
    } else if has("specfx") {
        "specfx.rs (special effects)"
    } else if has("sprites") {
        "sprite.rs / particle.rs"
    } else if has("hud") || has("end_level_timer") {
        "hud.rs / game.rs (timers)"
    } else {
        "game.rs / layout.rs (core state)"
    };
    let rng = if paths.contains(&"rseed") { " +rseed" } else { "" };
    format!("{m}{rng}")
}

// ---- report / baseline -------------------------------------------------------

fn render(o: &Opts, rows: &[Row]) -> String {
    let mut s = String::new();
    s.push_str("# Parity corpus report\n\n");
    s.push_str(&format!(
        "Generator `{}` (multiplayer `W-L-mpN`: `{}`, one stream per player), seed {}, {} ticks, trace v{TRACE_VERSION}. \
         Oracle: `run.py --trace --level W-L [--multi N] --keys` (cached by script hash); \
         Rust: `f2-game` example `scenario_trace [--multi N]`. *matched* = leading ticks identical on every section both sides emit.\n\n",
        script::GENERATOR,
        script::GENERATOR_MP,
        o.seed,
        o.ticks
    ));
    s.push_str("| case | oracle | rust | matched | first divergence (afc) | section | field: oracle != rust | likely module |\n");
    s.push_str("|---|---:|---:|---:|---:|---|---|---|\n");
    let num = |v: Option<usize>| v.map_or("-".to_string(), |n| n.to_string());
    let (mut full, mut total) = (0, 0);
    for r in rows {
        total += 1;
        if r.identical {
            full += 1;
        }
        let (afc, sec, fld, module) = match (&r.first, r.identical) {
            (_, true) => ("-".into(), "identical".into(), String::new(), String::new()),
            (Some(f), _) => (
                f.afc.to_string(),
                f.section.clone(),
                format!("`{}`: {} != {}{}", f.field, f.a, f.b, if f.nfields > 1 { format!(" (+{})", f.nfields - 1) } else { String::new() }),
                f.module.clone(),
            ),
            (None, _) => ("-".into(), "-".into(), r.note.clone(), String::new()),
        };
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            r.level,
            num(r.oracle_ticks),
            num(r.rust_ticks),
            num(r.matched),
            afc,
            sec,
            fld.replace('|', "\\|"),
            module
        ));
    }
    s.push_str(&format!("\n{full}/{total} cases identical.\n"));
    let fin: Vec<&Row> = rows.iter().filter(|r| r.finish).collect();
    if !fin.is_empty() {
        s.push_str(&format!(
            "\n## Finish cases (`{GENERATOR_FINISH}`, planned scripts in `tools/f2-parity/finish/`)\n\n\
             | case | ticks | matched | end state (oracle) | end state (rust) | end | savegame.dat |\n|---|---:|---:|---|---|---|---|\n"
        ));
        for r in &fin {
            let eq = match (&r.end_oracle, &r.end_rust) {
                (Some(a), Some(b)) if a == b => "match",
                (Some(_), Some(_)) => "DIFFERS",
                _ => "-",
            };
            s.push_str(&format!(
                "| {} | {} | {} | {} | {} | {eq} | {} |\n",
                r.level,
                num(r.oracle_ticks),
                num(r.matched),
                r.end_oracle.as_deref().unwrap_or("-"),
                r.end_rust.as_deref().unwrap_or("-"),
                r.save.as_deref().unwrap_or("-")
            ));
        }
    }
    s.push_str("\n## First divergence per section (afc, frames differing)\n\n");
    for r in rows.iter().filter(|r| !r.sections.is_empty()) {
        let v: Vec<String> = r.sections.iter().map(|(n, a, k)| format!("{n}@{a} ({k})")).collect();
        s.push_str(&format!("- {}: {}\n", r.level, v.join(", ")));
    }
    s
}

fn read_baseline(p: &Path) -> Result<Baseline, String> {
    let b = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", p.display()))
}

fn write_baseline(p: &Path, b: &Baseline) -> Result<(), String> {
    let mut s = serde_json::to_string_pretty(b).map_err(|e| e.to_string())?;
    s.push('\n');
    write(p, s.as_bytes())
}

/// Baseline from this run's rows; levels without a result keep their old entry.
fn baseline_from(o: &Opts, rows: &[Row], old: Baseline) -> Baseline {
    let mut levels = if old.seed == o.seed && old.ticks == o.ticks && old.generator == script::GENERATOR { old.levels } else { BTreeMap::new() };
    for r in rows {
        if let (Some(m), Some(n)) = (r.matched, r.oracle_ticks) {
            let end_match = r.end_oracle.as_ref().map(|a| Some(a) == r.end_rust.as_ref() && r.save.as_deref().is_some_and(|s| s == "identical" || s == "none"));
            levels.insert(r.level.clone(), BaseLevel { matched: m, oracle_ticks: n, end: r.end_oracle.clone(), end_match, save: r.save.clone() });
        }
    }
    Baseline { generator: script::GENERATOR.into(), seed: o.seed, ticks: o.ticks, trace_version: TRACE_VERSION, levels }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fd(p: &str) -> FieldDiff {
        FieldDiff { path: p.into(), a: json!(1), b: json!(2) }
    }

    #[test]
    fn classify_prefers_frog_state() {
        assert!(classify(&[fd("rseed")], None, 5).starts_with("RNG"));
        assert!(classify(&[fd("rseed"), fd("players[0].pos[0]")], None, 5).starts_with("frogmove.rs"));
        assert!(classify(&[fd("players[0].lives")], None, 5).starts_with("frogger.rs"));
        assert!(classify(&[fd("enemies[3].pos[1]"), fd("tongue[0].flags")], None, 5).starts_with("tongue.rs"));
        assert!(classify(&[fd("<frame>")], Some(2), 5).starts_with("level end"));
        assert!(classify(&[fd("enemies.len")], None, 1).starts_with("level load"));
    }

    #[test]
    fn level_parse() {
        assert_eq!(parse_level("7-9"), Some((7, 9)));
        assert_eq!(parse_level("x"), None);
        assert_eq!(parse_case("7-10-mp4"), Some((7, 10, 4, 0, false)));
        assert_eq!(parse_case("7-10-mp2-battle"), Some((7, 10, 2, 2, false)));
        assert_eq!(parse_case("1-3"), Some((1, 3, 0, 0, false)));
        assert_eq!(parse_case("1-3-mp1"), None);
        assert_eq!(parse_case("1-3-mp2-golf"), None);
        assert_eq!(case_id(5, 3, 2, 0, false), "5-3-mp2");
        assert_eq!(case_id(5, 3, 4, 1, false), "5-3-mp4-race");
        assert_eq!(parse_case("fin-1-3-mp2"), Some((1, 3, 2, 0, true)));
        assert_eq!(parse_case("fin-0-0"), Some((0, 0, 0, 0, true)));
        assert_eq!(parse_case("fin-fin-0-0"), None);
        assert_eq!(case_id(1, 3, 4, 0, true), "fin-1-3-mp4");
        assert!(classify(&[fd("mp.mpl[1].timer")], None, 9).starts_with("multi.rs"));
    }
}
