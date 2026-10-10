# Tools reference

All Rust tools build with cargo; run from the repo root, e.g. `cargo run --release -p f2-import -- ARGS`
or use the binaries in `target/release/`. None of them need game data except where stated. Never
commit extracted data; write outputs to `game-data/` (gitignored) or a scratch directory. Run any
Python on extracted data with `python3 -I`.

## f2-import (`crates/f2-import`)

Imports your own copy of the game into a rendition directory (`game/`, `video/`, `music/`, and
`oracle/` with `--oracle`).

```sh
f2-import <image.cue|.zip|.bin|.iso|disc dir|install dir> [--data-root DIR] [--oracle] [--no-music] [--verify] [--flac] [--as NAME]
f2-import --list [--data-root DIR]
f2-import --verify-installed [rendition] [--data-root DIR]
```

- `--data-root DIR`: output root (default `$F2_DATA_ROOT` or the platform data dir).
- `--as NAME`: rendition name override (otherwise detected, e.g. `usa`, `eu`).
- `--oracle`: also extract the retail exe and Bink DLL for developer parity runs.
- `--no-music`: skip CD audio tracks. `--flac`: write music as lossless FLAC instead of WAV.
- `--verify`: verify the CD audio track hash while importing.
- `--list`: show imported renditions. `--verify-installed`: re-check imported files.

Example: `cargo run --release -p f2-import -- --data-root game-data "path/to/disc.zip"`.

## f2-viewer (`tools/f2-viewer`)

Interactive asset browser (models, levels, textures, sounds, videos) on the engine renderer.

```sh
f2-viewer [rendition-dir|data-root] [--size WxH] [--mode N]
f2-viewer game-data/usa --mode level
```

Modes: `model` (1), `level` (2), `texture` (3), `sound` (4), `video` (5). Non-interactive
screenshots for docs and tests:

```sh
f2-viewer game-data/usa --screenshot model <bank> <model> [anim] [frame] out.png
f2-viewer game-data/usa --screenshot level <name> out.png
f2-viewer game-data/usa --screenshot texture <bank> <index|name> out.png
f2-viewer game-data/usa --screenshot video <name> <frame> out.png
```

## f2-export (`tools/f2-export`)

Converts imported assets to open formats for inspection and as modding references (output is
derived from your data; keep it out of git).

```sh
f2-export <rendition dir, e.g. game-data/usa> <out dir> [--kind textures|sounds|models|levels|text|all]
f2-export game-data/usa game-data/export --kind textures
```

## f2-etc (`tools/f2-etc`)

Port of the original event-script compiler (`etc.exe`) plus a decompiler: `.etc` source to
`.fev` event files and back.

```sh
f2-etc compile <in.etc> [-o out.fev] [-d] [-q] [-t] [-I dir]
f2-etc decompile <in.fev> [-o out.etc]
f2-etc check <in.fev>...
f2-etc header [-o codes.h] [--created "Thu May 11 19:09:59 2000"]
```

`-d` writes debug line records, `-q` suppresses warnings, `-t` test-compiles without saving, `-I`
adds include directories; `--tables <dir>` loads `triggers.ec`/`commands.ec` from a directory
instead of the built-in tables.

## f2-modkit (`crates/f2-modkit`)

Scaffold and validate mods ([modding.md](modding.md) section 6 for layout and file conventions).

```sh
f2-modkit new <id> [--dir mods]                       # create mods/<id>/ with mod.toml and folders
f2-modkit pack <mod dir> [--rendition game-data/usa]  # validate; exit 1 on errors
```

`pack` checks the manifest and engine compatibility, decodes every override via the engine's own
resolver, and (given a rendition) warns about overrides identical to retail. It validates only;
it does not build an archive yet.

## f2-trace (`tools/f2-trace`)

Reads and compares deterministic simulation traces (JSON lines or binary) from the retail oracle
and from the Rust sim.

```sh
f2-trace diff A B [--ignore PAT]... [--tol PAT=N]... [--align index|act] [--max-fields N] [--sections S,S...]
f2-trace summary FILE
f2-trace convert IN OUT            # OUT ending in .bin = binary, else JSON lines
f2-trace snap A.json B.json        # breakpoint snapshots: differing bytes per region
```

Sections: `core players enemies platforms garibs babies camera hud script triggers specfx sprites
tongue`. Exit status: 0 identical or within tolerance, 1 divergent, 2 usage/IO error. Schema and
how traces are produced: [ORACLE.md](ORACLE.md) (Trace mode).

## f2-parity (`tools/f2-parity`)

Per-level parity corpus: generates seeded input scripts (`.key`) for every single-player level,
runs the retail oracle on them (cached by script hash under `target/parity/oracle/`), runs the
Rust sim (`f2-game` example `scenario_trace`), diffs, and writes `target/parity/report.md`.

```sh
f2-parity run   [--levels W-L,..] [--ticks N] [--seed S] [--oracle-jobs N] [--jobs N] [--refresh] [--no-oracle] [--update-baseline] [--sim PATH] [--dll PATH]
f2-parity check [--baseline FILE] [--levels W-L,..] [--update] [--jobs N] [--sim PATH]
f2-parity gen   [--levels W-L,..] [--ticks N] [--seed S]
```

Defaults: 1800 ticks, seed 1, 2 oracle jobs, 4 sim jobs. `run` needs Windows (the oracle is the
retail exe); `check` does not: it compares fresh Rust traces with the cached oracle traces and
exits 1 if any level matches fewer ticks than in `tools/f2-parity/baseline.json`. Example:
`f2-parity check --levels 0-0,1-0`.

## Oracle runner (`tools/run/run.py`)

Runs the user's own retail `frogger2.exe` headless and muted through the `f2fix` proxy DLL, for
screenshots and deterministic traces. Details, shims and known issues: [ORACLE.md](ORACLE.md).

```sh
tools/run/build_f2fix.sh                        # builds target/oracle-bin/f2fix.dll (i686, Windows cargo)
python3 -I tools/run/run.py --id smoke --watch  # 60 s run, a dump every second
python3 -I tools/run/run.py --id g1 --trace --demo 0-0                       # trace the Garden 1 demo
python3 -I tools/run/run.py --id s1 --trace --level 0-0 --keys record.key --exit-ticks 2000
f2-trace diff target/oracle-runs/g1/f2-trace.jsonl rust-trace.jsonl
```

Requires `f2-import --oracle` to have run (`game-data/<region>/oracle/`). Other options:
`--exit-seconds`, `--exit-frames`, `--dump-interval-ms`, `--window offscreen|hidden|off`,
`--timeout`, `--keep-root`, `--game-dir`.

## Other helpers

- `python3 -I tools/tasks.py status`: project task board summary.
- `tools/package/package.sh linux|windows|macos` and `tools/package/build-windows.sh`: release
  archives ([BUILDING.md](BUILDING.md)).
- `tools/memcap.sh <cmd>`: run a heavy command under a memory cap.
