//! `f2-etc` command-line tool.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use f2_etc::compiler::asctime_utc;
use f2_etc::{Compiler, Options, Roundtrip};

const USAGE: &str = "\
f2-etc: Frogger 2 event-script compiler (ETC port) and decompiler

Usage:
  f2-etc compile <in.etc> [-o out.fev] [-d] [-q] [-t] [-I dir]
  f2-etc decompile <in.fev> [-o out.etc]
  f2-etc check <in.fev>...
  f2-etc header [-o codes.h] [--created \"Thu May 11 19:09:59 2000\"]

Common options:
  --tables <dir>  load triggers.ec/commands.ec from <dir> instead of the built-in tables

compile options (as in the original etc.exe):
  -o <file>  output file (default: input with extension replaced by .fev)
  -d         write C_DEBUG line-number records
  -q         quiet: suppress warnings
  -t         test compile, do not save
  -I <dir>   include search directory";

struct Args {
    cmd: String,
    inputs: Vec<String>,
    out: Option<PathBuf>,
    tables: Option<PathBuf>,
    created: Option<String>,
    opts: Options,
    test: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let cmd = it.next().ok_or_else(|| USAGE.to_string())?;
    let mut a = Args { cmd, inputs: vec![], out: None, tables: None, created: None, opts: Options::default(), test: false };
    while let Some(x) = it.next() {
        let mut val = |n: &str| it.next().ok_or_else(|| format!("{n} needs a value"));
        match x.as_str() {
            "-o" => a.out = Some(val("-o")?.into()),
            "--tables" => a.tables = Some(val("--tables")?.into()),
            "--created" => a.created = Some(val("--created")?),
            "-I" => a.opts.include_path = Some(val("-I")?.into()),
            "-d" => a.opts.write_debug = true,
            "-q" => a.opts.quiet = true,
            "-t" => a.test = true,
            "-h" | "--help" => return Err(USAGE.to_string()),
            s if s.starts_with('-') && s.len() > 1 => return Err(format!("unrecognised switch '{s}'\n\n{USAGE}")),
            _ => a.inputs.push(x),
        }
    }
    Ok(a)
}

fn compiler(a: &Args) -> Result<Compiler, String> {
    let r = match &a.tables {
        None => Compiler::new(a.opts.clone()),
        Some(dir) => {
            let t = std::fs::read(dir.join("triggers.ec")).map_err(|e| format!("{}: {e}", dir.join("triggers.ec").display()))?;
            let c = std::fs::read(dir.join("commands.ec")).map_err(|e| format!("{}: {e}", dir.join("commands.ec").display()))?;
            Compiler::with_tables(a.opts.clone(), &t, &c)
        }
    };
    r.map_err(|d| d.iter().map(|x| format!("{x}")).collect::<Vec<_>>().join("\n"))
}

fn one_input(a: &Args) -> Result<&str, String> {
    match a.inputs.as_slice() {
        [x] => Ok(x),
        _ => Err(format!("{} takes exactly one input file\n\n{USAGE}", a.cmd)),
    }
}

fn write(path: &Path, data: &[u8]) -> Result<(), String> {
    std::fs::write(path, data).map_err(|e| format!("couldn't write {}: {e}", path.display()))
}

fn run() -> Result<bool, String> {
    let a = parse_args()?;
    match a.cmd.as_str() {
        "compile" => {
            let input = one_input(&a)?;
            let mut c = compiler(&a)?;
            let src = std::fs::read(input).map_err(|e| format!("Couldn't read '{input}': {e}"))?;
            match c.compile_source(input, &src) {
                Ok(out) => {
                    for w in &out.warnings {
                        eprintln!("{w}");
                    }
                    if !a.test {
                        let path = a.out.clone().unwrap_or_else(|| Path::new(input).with_extension("fev"));
                        write(&path, &out.data)?;
                        if !a.opts.quiet {
                            println!("Saved {} (version 3, {} bytes)", path.display(), out.data.len());
                        }
                    }
                    Ok(true)
                }
                Err(diags) => {
                    for d in diags {
                        eprintln!("{d}");
                    }
                    Ok(false)
                }
            }
        }
        "decompile" => {
            let input = one_input(&a)?;
            let c = compiler(&a)?;
            let data = std::fs::read(input).map_err(|e| format!("Couldn't read '{input}': {e}"))?;
            let title = Path::new(input).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let dec = f2_etc::decompile_with(&c, &data, &title).map_err(|e| format!("{input}: {e}"))?;
            if dec.lossy {
                eprintln!("{input}: warning: C_DEBUG records written as comments; output will not recompile identically");
            }
            match &a.out {
                Some(p) => write(p, &dec.source)?,
                None => {
                    use std::io::Write;
                    std::io::stdout().write_all(&dec.source).map_err(|e| e.to_string())?;
                }
            }
            Ok(true)
        }
        "check" => {
            if a.inputs.is_empty() {
                return Err(format!("check needs at least one file\n\n{USAGE}"));
            }
            let mut ok = true;
            for input in &a.inputs {
                let data = std::fs::read(input).map_err(|e| format!("Couldn't read '{input}': {e}"))?;
                let game = f2_formats::fev::Script::parse(&data);
                let rt = f2_etc::roundtrip(&data);
                match (&game, &rt) {
                    (Ok(s), Ok((_, Roundtrip::Identical))) => {
                        println!("{input}: ok ({} bytes, {} top-level events, round-trips)", data.len(), s.commands.len())
                    }
                    _ => {
                        ok = false;
                        if let Err(e) = &game {
                            println!("{input}: game loader rejects it: {e}");
                        }
                        match &rt {
                            Err(e) => println!("{input}: decompile/recompile failed: {e}"),
                            Ok((_, Roundtrip::Differs { offset, original_len, recompiled_len })) => {
                                println!("{input}: recompiled bytes differ at 0x{offset:x} ({original_len} vs {recompiled_len} bytes)")
                            }
                            Ok(_) => {}
                        }
                    }
                }
            }
            Ok(ok)
        }
        "header" => {
            let c = compiler(&a)?;
            let created = match &a.created {
                Some(s) => format!("{s}\n"),
                None => {
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                    asctime_utc(now as i64)
                }
            };
            let text = c.output_header(&created);
            match &a.out {
                Some(p) => write(p, text.as_bytes())?,
                None => print!("{text}"),
            }
            Ok(true)
        }
        _ => Err(USAGE.to_string()),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(2)
        }
    }
}
