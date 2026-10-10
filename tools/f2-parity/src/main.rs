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
}

#[derive(Serialize, Deserialize, Default)]
struct Baseline {
    generator: String,
    seed: u64,
    ticks: u32,
    trace_version: u32,
    /// Level `W-L` -> leading ticks identical to the oracle (`matched`) and the oracle length.
    levels: BTreeMap<String, BaseLevel>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Default)]
struct BaseLevel {
    matched: usize,
    oracle_ticks: usize,
}

#[derive(Serialize, Deserialize, Default)]
struct OracleMeta {
    level: String,
    key: String,
    rseed: Option<u32>,
    lines: usize,
}

/// One corpus entry.
struct Case {
    level: String,
    world: u32,
    lvl: u32,
    keys: PathBuf,
    key: String,
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
  f2-parity run   [--levels W-L,..] [--ticks N] [--seed S] [--oracle-jobs N] [--jobs N] [--refresh] [--no-oracle] [--update-baseline] [--sim PATH] [--dll PATH] [--cache DIR]
  f2-parity check [--baseline FILE] [--levels W-L,..] [--update] [--jobs N] [--sim PATH] [--cache DIR]
  f2-parity gen   [--levels W-L,..] [--ticks N] [--seed S]";

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

fn select_levels(ctx: &Ctx, o: &Opts) -> Result<Vec<(u32, u32)>, String> {
    let out = Command::new(&ctx.sim).arg("--list-levels").output().map_err(|e| format!("{}: {e}", ctx.sim.display()))?;
    if !out.status.success() {
        return Err("scenario_trace --list-levels failed".into());
    }
    let all: Vec<(u32, u32)> = String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| parse_level(l.split_whitespace().next()?)).collect();
    match &o.levels {
        None => Ok(all),
        Some(want) => want.iter().map(|w| parse_level(w).ok_or(format!("bad level {w} (want W-L)"))).collect(),
    }
}

fn parse_level(s: &str) -> Option<(u32, u32)> {
    let (w, l) = s.split_once('-')?;
    Some((w.parse().ok()?, l.parse().ok()?))
}

fn gen_scripts(ctx: &Ctx, o: &Opts, levels: &[(u32, u32)]) -> Result<Vec<Case>, String> {
    let mut v = Vec::new();
    for &(w, l) in levels {
        let level = format!("{w}-{l}");
        let bytes = script::to_key_bytes(&script::generate(w, l, o.seed, o.ticks));
        let mut h = Sha1::new();
        h.update(format!("{}\n{level}\nticks {}\ntrace v{TRACE_VERSION}\nseed {}\n", script::GENERATOR, o.ticks, o.seed).as_bytes());
        h.update(&bytes);
        let key: String = h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect();
        let keys = ctx.dir.join("scripts").join(format!("{level}-{key}.key"));
        if !keys.is_file() {
            write(&keys, &bytes)?;
        }
        v.push(Case { level, world: w, lvl: l, keys, key });
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
        let r = run_oracle(ctx, o, &dll, c);
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

fn run_oracle(ctx: &Ctx, o: &Opts, dll: &Path, c: &Case) -> Result<usize, String> {
    let id = format!("parity-{}-{}", c.level, &c.key[..8]);
    let st = memcap(&ctx.root)
        .args(["python3", "-I", "tools/run/run.py", "--id", &id, "--trace", "--trace-version", &TRACE_VERSION.to_string()])
        .args(["--level", &c.level, "--keys"])
        .arg(&c.keys)
        .args(["--exit-ticks", &o.ticks.to_string(), "--exit-seconds", "150", "--timeout", "200", "--dll"])
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
    let (tp, mp) = oracle_path(ctx, c);
    write(&tp, text.as_bytes())?;
    let meta = OracleMeta { level: c.level.clone(), key: c.key.clone(), rseed, lines };
    write(&mp, serde_json::to_string_pretty(&meta).unwrap().as_bytes())?;
    let _ = std::fs::remove_dir_all(&run);
    Ok(lines)
}

// ---- simulation + diff -------------------------------------------------------

fn compare(ctx: &Ctx, o: &Opts, cases: &[Case]) -> Vec<Row> {
    eprintln!("f2-parity: {} simulation run(s), {} at a time", cases.len(), o.jobs);
    par_for(cases, o.jobs, |c| {
        let r = compare_one(ctx, o, c);
        eprintln!("  rust   {:<5} {}", c.level, r.matched.map_or(r.note.clone(), |m| format!("{m} ticks matched")));
        r
    })
}

fn load(p: &Path) -> Result<Vec<TraceFrame>, String> {
    f2_trace::read_file(p)
}

fn compare_one(ctx: &Ctx, o: &Opts, c: &Case) -> Row {
    let mut row = Row {
        level: c.level.clone(),
        oracle_ticks: None,
        rust_ticks: None,
        matched: None,
        identical: false,
        first: None,
        sections: Vec::new(),
        note: String::new(),
    };
    let (tp, mp) = oracle_path(ctx, c);
    let meta: OracleMeta = std::fs::read(&mp).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let out = ctx.dir.join("rust").join(format!("{}.jsonl", c.level));
    let _ = std::fs::create_dir_all(out.parent().unwrap());
    let mut cmd = memcap(&ctx.root);
    cmd.arg(&ctx.sim)
        .args([c.world.to_string(), c.lvl.to_string()])
        .arg(&c.keys)
        .arg(&out)
        .arg(o.ticks.to_string())
        .env("F2_GAME_DIR", &ctx.game_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(s) = meta.rseed {
        cmd.env("F2_START_RSEED", s.to_string());
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
    row
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
        "Generator `{}`, seed {}, {} ticks, trace v{TRACE_VERSION}. Oracle: `run.py --trace --level W-L --keys` (cached by script hash); \
         Rust: `f2-game` example `scenario_trace`. *matched* = leading ticks identical on every section both sides emit.\n\n",
        script::GENERATOR,
        o.seed,
        o.ticks
    ));
    s.push_str("| level | oracle | rust | matched | first divergence (afc) | section | field: oracle != rust | likely module |\n");
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
    s.push_str(&format!("\n{full}/{total} levels identical.\n\n## First divergence per section (afc, frames differing)\n\n"));
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
            levels.insert(r.level.clone(), BaseLevel { matched: m, oracle_ticks: n });
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
    }
}
