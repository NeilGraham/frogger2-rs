//! `f2-trace`: inspect, convert and compare per-tick game-state traces.
//!
//! ```text
//! f2-trace diff A B [--ignore PAT]... [--tol PAT=N]... [--align index|act] [--max-fields N]
//!                [--sections S[,S]...]
//! f2-trace summary FILE
//! f2-trace convert IN OUT        # OUT ending in .bin -> binary, else JSON lines
//! f2-trace snap A.json B.json     # compare two breakpoint snapshots (f2-snap-<tick>.json)
//! ```
//! Sections: core players enemies platforms garibs babies camera hud script
//! triggers specfx sprites tongue (see `diff::section_of`).
//! Inputs may be JSON lines or the binary encoding (detected by magic).
//! Exit status: 0 identical / within tolerance, 1 divergent, 2 usage or I/O error.

use f2_trace::diff::{self, Align, Options};
use f2_trace::{binfmt, read_file, write_jsonl, TraceFrame};
use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "usage:
  f2-trace diff A B [--ignore PAT]... [--tol PAT=N]... [--align index|act] [--max-fields N] [--sections S,S...]
  f2-trace summary FILE
  f2-trace convert IN OUT   (OUT ending in .bin = binary, else JSON lines)
  f2-trace snap A.json B.json   (breakpoint snapshots: differing bytes per region)
sections: core players enemies platforms garibs babies camera hud script triggers specfx sprites tongue";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("f2-trace: {e}");
            ExitCode::from(2)
        }
    }
}

fn load(p: &str) -> Result<Vec<TraceFrame>, String> {
    read_file(Path::new(p))
}

fn run(args: &[String]) -> Result<u8, String> {
    let Some(cmd) = args.first() else { return Err(USAGE.into()) };
    match cmd.as_str() {
        "diff" => {
            let mut files = Vec::new();
            let mut opt = Options { max_fields: 20, ..Default::default() };
            let mut it = args[1..].iter();
            while let Some(a) = it.next() {
                let mut val = |name: &str| it.next().cloned().ok_or(format!("{name} needs a value"));
                match a.as_str() {
                    "--ignore" => opt.ignore.push(val("--ignore")?),
                    "--tol" => {
                        let v = val("--tol")?;
                        let (p, n) = v.rsplit_once('=').ok_or("--tol takes PATTERN=N")?;
                        let n: f64 = n.parse().map_err(|_| format!("bad tolerance {n}"))?;
                        opt.tolerance.push((p.to_string(), n));
                    }
                    "--align" => {
                        opt.align = match val("--align")?.as_str() {
                            "index" => Align::Index,
                            "act" | "act_frame_count" => Align::ActFrame,
                            o => return Err(format!("bad --align {o}")),
                        }
                    }
                    "--sections" => {
                        for sec in val("--sections")?.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                            if !diff::SECTIONS.contains(&sec) {
                                return Err(format!("unknown section {sec}\n{USAGE}"));
                            }
                            opt.sections.push(sec.to_string());
                        }
                    }
                    "--max-fields" => opt.max_fields = val("--max-fields")?.parse().map_err(|_| "bad --max-fields")?,
                    s if s.starts_with("--") => return Err(format!("unknown option {s}\n{USAGE}")),
                    s => files.push(s.to_string()),
                }
            }
            let [a, b] = files.as_slice() else { return Err(USAGE.into()) };
            let rep = diff::diff(&load(a)?, &load(b)?, &opt);
            print!("{}", diff::render(&rep, a, b));
            Ok(if rep.identical() { 0 } else { 1 })
        }
        "summary" => {
            let [_, f] = args else { return Err(USAGE.into()) };
            let frames = load(f)?;
            println!("{}", summary(&frames));
            Ok(0)
        }
        "convert" => {
            let [_, i, o] = args else { return Err(USAGE.into()) };
            let frames = load(i)?;
            if o.ends_with(".bin") {
                std::fs::write(o, binfmt::encode(&frames)).map_err(|e| format!("{o}: {e}"))?;
            } else {
                let f = std::fs::File::create(o).map_err(|e| format!("{o}: {e}"))?;
                write_jsonl(std::io::BufWriter::new(f), &frames).map_err(|e| format!("{o}: {e}"))?;
            }
            println!("{} frames -> {o}", frames.len());
            Ok(0)
        }
        "snap" => {
            let [_, a, b] = args else { return Err(USAGE.into()) };
            let (text, same) = f2_trace::snap::compare_files(Path::new(a), Path::new(b))?;
            print!("{text}");
            Ok(if same { 0 } else { 1 })
        }
        _ => Err(USAGE.into()),
    }
}

fn summary(frames: &[TraceFrame]) -> String {
    let mut s = format!("frames: {}\n", frames.len());
    let (Some(first), Some(last)) = (frames.first(), frames.last()) else { return s };
    s += &format!("act_frame_count: {}..{}\n", first.act_frame_count, last.act_frame_count);
    let mut levels: Vec<(u8, u8, u32, usize)> = Vec::new();
    for f in frames {
        match levels.last_mut() {
            Some(l) if (l.0, l.1, l.2) == (f.world, f.level, f.mode) => l.3 += 1,
            _ => levels.push((f.world, f.level, f.mode, 1)),
        }
    }
    for (w, l, m, n) in levels {
        s += &format!("  world {w} level {l} mode {m}: {n} frames\n");
    }
    let steps = frames.windows(2).filter(|p| p[1].act_frame_count != p[0].act_frame_count.wrapping_add(1)).count();
    let speed = frames.iter().filter(|f| f.game_speed != 4096).count();
    s += &format!("non-unit act_frame_count steps: {steps}\nframes with game_speed != 4096: {speed}\n");
    let v2 = frames.iter().filter(|f| f.v >= 2).count();
    s += &format!("v2 frames: {v2}\n");
    s += &format!(
        "last: rseed {}, players {}, enemies {}, platforms {}, garibs {}/{}",
        last.rseed,
        last.players.len(),
        last.enemies.len(),
        last.platforms.len(),
        last.garib_count,
        last.garib_total
    );
    s
}
