#!/usr/bin/env python3
"""tools/run/run.py: one headless, muted oracle run of the user's retail Frogger 2.

Run with `python3 -I tools/run/run.py --id <id> [options]` from WSL.

Flow (patterned on theme-park-world-recomp tools/run/run.py, MIT, same owner):

  1. stage the imported game data once on the Windows side:
     C:\\f2-runs\\_stage\\<region>-<fingerprint>\\  (game-data/<region>/game/* and video/ as Video\\),
     made read-only so a run can never modify it;
  2. build a run root C:\\f2-runs\\<id>\\: hardlinks to the staged files, copies of the
     retail frogger2.exe and binkw32.dll (game-data/<region>/oracle/), f2fix.dll as
     dinput.dll (target/oracle-bin/, built by tools/run/build_f2fix.sh) and f2fix.ini;
  3. launch frogger2.exe from WSL (cwd = root) with an optional read-only desktop
     watcher (fgwatch.ps1); wait for it to exit (exit.frames / exit.seconds in the
     ini end it) or kill it at --timeout;
  4. collect f2fix.log, f2-shot-*.png and fgwatch.jsonl into the gitignored
     target/oracle-runs/<id>/ with a result.json summary; delete the root
     (unless --keep-root). The stage is kept for the next run.

Nothing from game-data/ is executed except the retail exe inside the run root.
"""

import argparse
import csv
import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import time

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
RUNS = os.environ.get("F2_RUNS_ROOT", "/mnt/c/f2-runs")
SYS32 = "/mnt/c/Windows/System32"
PS = SYS32 + "/WindowsPowerShell/v1.0/powershell.exe"
STAGE_MARKER = ".f2-stage.json"
EXE = "frogger2.exe"


def default_game_dir():
    """$F2_GAME_DIR, else game-data/usa of this checkout, else of the main checkout
    (a git worktree has no gitignored game data of its own)."""
    if os.environ.get("F2_GAME_DIR"):
        return os.environ["F2_GAME_DIR"]
    own = os.path.join(REPO, "game-data/usa")
    if os.path.isdir(own):
        return own
    try:
        common = subprocess.run(["git", "-C", REPO, "rev-parse", "--path-format=absolute", "--git-common-dir"],
                                check=True, capture_output=True, text=True).stdout.strip()
        main = os.path.join(os.path.dirname(common), "game-data/usa")
        if os.path.isdir(main):
            return main
    except (OSError, subprocess.CalledProcessError):
        pass
    return own


def load_symbols(path=None):
    """symbols/frogger2_usa.csv globals/data -> {name: (address, size or None)}."""
    path = path or os.path.join(REPO, "symbols/frogger2_usa.csv")
    out = {}
    with open(path, newline="") as f:
        for row in csv.DictReader(f):
            try:
                size = int(row["size"]) if row["size"].strip() else None
            except ValueError:
                size = None
            if row["kind"] in ("global", "data"):
                out[row["name"]] = (int(row["address"], 16), size)
    return out


def snap_regions(specs, symbols):
    """--snap items -> f2fix `snap.regions` tokens.

    NAME            a CSV global with its CSV size
    NAME:LEN        a CSV global, LEN bytes (hex 0x.. or decimal)
    *NAME:LEN       LEN bytes at the pointer stored in global NAME
    0xADDR:LEN      a raw address (named after the address)
    LABEL=SPEC      any of the above under another name
    """
    out = []
    for item in [x for spec in specs for x in re.split(r"[,\s]+", spec) if x]:
        label, _, spec = item.rpartition("=")
        deref = spec.startswith("*")
        spec = spec.lstrip("*")
        target, _, length = spec.partition(":")
        if re.fullmatch(r"0[xX][0-9a-fA-F]+", target):
            addr, size = int(target, 16), None
        elif target in symbols:
            addr, size = symbols[target]
        else:
            raise SystemExit(f"run: --snap: unknown symbol {target!r} (not in symbols/frogger2_usa.csv)")
        size = int(length, 0) if length else size
        if not size:
            raise SystemExit(f"run: --snap: {item!r} needs a length (NAME:LEN)")
        name = label or (("*" if deref else "") + target).replace("*", "p_")
        out.append(f"{name}@{'*' if deref else ''}{addr:#x}:{size}")
    return out


def log(msg):
    print(f"run: {msg}", file=sys.stderr, flush=True)


def wpath(p):
    return subprocess.run(["wslpath", "-w", p], check=True, capture_output=True,
                          text=True).stdout.strip()


def ps(script, *args, timeout=60):
    """Run a PowerShell script file or command with a /mnt/c cwd (no UNC)."""
    argv = [PS, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass"]
    argv += ["-File", script, *args] if script.endswith(".ps1") else ["-Command", script]
    r = subprocess.run(argv, cwd="/mnt/c/", capture_output=True, text=True,
                       stdin=subprocess.DEVNULL, timeout=timeout)
    return (r.stdout + r.stderr).replace("\r", "")


def walk(top):
    out = []
    for d, dirs, files in os.walk(top):
        dirs.sort()
        for f in sorted(files):
            p = os.path.join(d, f)
            out.append((os.path.relpath(p, top), os.path.getsize(p)))
    return out


def stage(game_dir, region):
    """Copy game/ and video/ once to a read-only Windows-side stage."""
    sources = [(os.path.join(game_dir, "game"), ""), (os.path.join(game_dir, "video"), "Video")]
    files = []
    for src, prefix in sources:
        if not os.path.isdir(src):
            raise SystemExit(f"run: missing {src} (run f2-import first)")
        files += [(src, rel, os.path.join(prefix, rel) if prefix else rel, size)
                  for rel, size in walk(src)]
    fp = hashlib.sha1("\n".join(f"{dst}:{size}" for _, _, dst, size in files).encode()).hexdigest()[:12]
    st = os.path.join(RUNS, "_stage", f"{region}-{fp}")
    marker = os.path.join(st, STAGE_MARKER)
    if os.path.isfile(marker):
        return st, [dst for _, _, dst, _ in files]
    log(f"staging {len(files)} files to {wpath(os.path.dirname(st))}\\{os.path.basename(st)} (once)")
    if os.path.isdir(st):
        rmtree(st)
    for src, rel, dst, _ in files:
        out = os.path.join(st, dst)
        os.makedirs(os.path.dirname(out), exist_ok=True)
        shutil.copyfile(os.path.join(src, rel), out)
        os.chmod(out, stat.S_IREAD)  # FILE_ATTRIBUTE_READONLY: runs can't write through links
    with open(marker, "w") as f:
        json.dump({"region": region, "files": len(files), "fingerprint": fp}, f)
    return st, [dst for _, _, dst, _ in files]


def rmtree(top):
    def onerror(func, path, _exc):
        os.chmod(path, stat.S_IWRITE | stat.S_IREAD)
        func(path)
    shutil.rmtree(top, onerror=onerror)


def write_ini(path, args, root_w):
    lines = [
        "# f2fix.ini written by tools/run/run.py",
        f"window={args.window}",
        f"audio.mute={0 if args.audible else 1}",
        "input.virtual=1",
        f"reg.install_dir={root_w}\\",
        f"reg.language={args.language}",
        "reg.resolution=640x480",
        "reg.fullscreen=0",
        f"reg.video_device={args.video_device}",
        "cd.drive=Q",
        f"cd.root={root_w}\\",
        "dump.first=1",
        f"dump.every={args.dump_every}",
        f"dump.interval_ms={args.dump_interval_ms}",
        f"dump.max={args.dump_max}",
        f"exit.frames={args.exit_frames}",
        f"exit.seconds={args.exit_seconds}",
        f"log.presents={args.log_presents}",
        f"time.fixed={1 if args.fixed_time else 0}",
        f"fmv.skip={1 if args.skip_fmv else 0}",
        f"start.demo={args.demo or ''}",
        f"start.level={args.level or ''}",
        f"start.keys={'scenario.key' if args.keys else ''}",
        f"start.after={args.start_after}",
        f"start.multi={args.multi}",
        f"start.chars={args.chars}",
        f"start.mp_mode={args.mp_mode}",
        f"trace.enable={1 if args.trace else 0}",
        f"trace.from={args.trace_from}",
        f"trace.version={args.trace_version}",
        f"snap.ticks={','.join(str(t) for t in args.snap_ticks)}",
        f"snap.regions={' '.join(args.snap_regions)}",
        f"exit.ticks={args.exit_ticks}",
        f"exit.level_end={0 if args.no_level_end_exit else 1}",
    ]
    with open(path, "w", encoding="latin-1") as f:
        f.write("\r\n".join(lines) + "\r\n")


def make_root(args, root, st, rels):
    if os.path.exists(root):
        rmtree(root)
    os.makedirs(root)
    for rel in rels:
        out = os.path.join(root, rel)
        os.makedirs(os.path.dirname(out), exist_ok=True)
        os.link(os.path.join(st, rel), out)
    oracle = os.path.join(args.game_dir, "oracle")
    for name in (EXE, "binkw32.dll"):
        shutil.copyfile(os.path.join(oracle, name), os.path.join(root, name))
    shutil.copyfile(args.dll, os.path.join(root, "dinput.dll"))
    if args.keys:
        shutil.copyfile(args.keys, os.path.join(root, "scenario.key"))
    write_ini(os.path.join(root, "f2fix.ini"), args, wpath(root))


def kill_game(root_w):
    cmd = (f"Get-Process -Name frogger2 -ErrorAction SilentlyContinue | Where-Object {{ $_.Path -and "
           f"$_.Path.StartsWith('{root_w}\\', [StringComparison]::OrdinalIgnoreCase) }} | "
           f"ForEach-Object {{ Stop-Process -Id $_.Id -Force; 'killed ' + $_.Id }}")
    return ps(cmd).strip()


def summarize_watch(path):
    rows = []
    if os.path.isfile(path):
        with open(path, encoding="utf-8", errors="replace") as f:
            for line in f:
                line = line.strip()
                if line.startswith("{"):
                    try:
                        rows.append(json.loads(line))
                    except ValueError:
                        pass
    wins = [w for r in rows for w in r.get("game_windows", [])]
    return {
        "samples": len(rows),
        "samples_with_game": sum(1 for r in rows if r.get("game_count")),
        "foreground_ever_game": any(r.get("fg_is_game") for r in rows),
        "foreground_procs": sorted({r.get("fg_proc", "") for r in rows}),
        "display_modes": sorted({tuple(r.get("mode", [])) for r in rows}),
        "game_windows_visible": sum(1 for w in wins if w.get("visible")),
        "game_windows_on_screen": sum(1 for w in wins if w.get("on_screen")),
    }


def summarize_trace(path):
    """Line count, in-level ticks and a digest of the trace (determinism checks)."""
    out = {"file": os.path.basename(path), "lines": 0, "level_ticks": 0, "sha1": None,
           "first_act_frame": None, "last_act_frame": None, "worlds": []}
    if not os.path.isfile(path):
        return out
    h = hashlib.sha1()
    worlds = set()
    with open(path, "rb") as f:
        for raw in f:
            h.update(raw)
            out["lines"] += 1
            try:
                fr = json.loads(raw)
            except ValueError:
                continue
            if fr.get("mode") in (2, 10):
                out["level_ticks"] += 1
            worlds.add(f'{fr.get("world")}-{fr.get("level")}')
            if out["first_act_frame"] is None:
                out["first_act_frame"] = fr.get("act_frame_count")
            out["last_act_frame"] = fr.get("act_frame_count")
    out["sha1"] = h.hexdigest()
    out["worlds"] = sorted(worlds)
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--id", required=True, help="run id (letters, digits, - and _)")
    ap.add_argument("--game-dir", "--data", dest="game_dir", default=default_game_dir(),
                    help="rendition root with game/, video/, oracle/ (default $F2_GAME_DIR, game-data/usa, "
                         "or the main checkout's game-data/usa from a git worktree)")
    ap.add_argument("--dll", default=os.path.join(REPO, "target/oracle-bin/f2fix.dll"))
    ap.add_argument("--timeout", type=float, default=90, help="kill after S seconds (default 90)")
    ap.add_argument("--exit-frames", type=int, default=0, help="f2fix ends the run after N presents")
    ap.add_argument("--exit-seconds", type=int, default=60, help="f2fix ends the run after S s (default 60; 0 = off)")
    ap.add_argument("--dump-every", type=int, default=0, help="dump every N presents (default 0 = off)")
    ap.add_argument("--dump-interval-ms", type=int, default=1000,
                    help="dump at most every N ms of wall time (default 1000)")
    ap.add_argument("--compat", default="16BitColor",
                    help="per-process AppCompat layer via __COMPAT_LAYER (default 16BitColor: 16-bit 565 surfaces as on a 2000-era desktop, no display-mode change; '' = none)")
    ap.add_argument("--dump-max", type=int, default=100)
    ap.add_argument("--log-presents", type=int, default=0)
    ap.add_argument("--window", choices=["offscreen", "hidden", "off"], default="offscreen",
                    help="off = visible and activating (debug only; takes focus)")
    ap.add_argument("--audible", action="store_true", help="do not mute (debug only)")
    ap.add_argument("--language", default="US")
    ap.add_argument("--video-device", default="auto")
    ap.add_argument("--watch", action="store_true", help="sample foreground/display mode (fgwatch.ps1)")
    ap.add_argument("--keep-root", action="store_true")
    g = ap.add_argument_group("deterministic oracle (docs/ORACLE.md: trace mode)")
    g.add_argument("--trace", action="store_true",
                   help="write f2-trace.jsonl (one line of game state per GameLoop tick); implies --fixed-time --skip-fmv")
    g.add_argument("--fixed-time", action="store_true", help="exactly one 60 Hz frame per main-loop iteration, uncapped")
    g.add_argument("--skip-fmv", action="store_true", help="skip every Bink FMV")
    g.add_argument("--demo", metavar="W-L", help="start the attract demo of world W level L (DEMOS\\RECORD-W-L.KEY)")
    g.add_argument("--level", metavar="W-L", help="start arcade level W-L directly (scenario mode)")
    g.add_argument("--keys", metavar="FILE", help="scenario: .key recording replayed as held pad input (with --level)")
    g.add_argument("--start-after", type=int, default=2, help="frontend frames before --demo/--level fires")
    g.add_argument("--multi", type=int, choices=[0, 2, 3, 4], default=0,
                   help="with --level: start a local multiplayer match with N frogs (frontend GoMulti)")
    g.add_argument("--chars", default="0,1,2,3", help="--multi: frog characters per player (frogger.h FROG_*)")
    g.add_argument("--mp-mode", type=int, choices=[0, 1, 2, 3], default=0,
                   help="--multi: override multiGameTypes[world] (1 race, 2 battle, 3 collect; 0 = level default)")
    g.add_argument("--trace-from", choices=["level", "all"], default="level")
    g.add_argument("--exit-ticks", type=int, default=0, help="end after N traced ticks")
    g.add_argument("--trace-version", type=int, choices=[1, 2], default=1,
                   help="trace schema: 1 = base, 2 = + script/triggers/camera/specfx/sprites/tongue/babies/hud")
    g.add_argument("--snap-tick", metavar="N[,N]", action="append", default=[],
                   help="breakpoint snapshot after the GameLoop of act_frame_count N -> f2-snap-N.json")
    g.add_argument("--snap", metavar="SPEC", action="append", default=[],
                   help="snapshot regions: NAME | NAME:LEN | *NAME:LEN | 0xADDR:LEN | LABEL=SPEC (CSV names; "
                        "comma lists ok; default: every CSV global/data entry with a size)")
    g.add_argument("--no-level-end-exit", action="store_true", help="keep running after the demo/level is left")
    args = ap.parse_args()
    args.snap_ticks = [int(t, 0) for spec in args.snap_tick for t in spec.split(",") if t.strip()]
    if args.snap_ticks:
        syms = load_symbols()
        if args.snap:
            args.snap_regions = snap_regions(args.snap, syms)
        else:
            args.snap_regions = snap_regions([n for n, (_, size) in syms.items() if size], syms)
        args.fixed_time = args.skip_fmv = True
    else:
        args.snap_regions = []
    if args.trace:
        args.fixed_time = args.skip_fmv = True
    for wl in (args.demo, args.level):
        if wl and not re.fullmatch(r"\d{1,2}-\d{1,2}", wl):
            raise SystemExit("run: --demo/--level take W-L, e.g. 0-0")
    if args.demo and args.level:
        raise SystemExit("run: --demo and --level are exclusive")
    if args.multi and not args.level:
        raise SystemExit("run: --multi needs --level")
    if args.keys and not args.level:
        raise SystemExit("run: --keys needs --level")

    if not re.fullmatch(r"[A-Za-z0-9_-]{1,64}", args.id) or args.id.startswith("_"):
        raise SystemExit("run: bad --id")
    if not os.path.isfile(args.dll):
        raise SystemExit(f"run: {args.dll} missing; build it with tools/run/build_f2fix.sh")
    region = os.path.basename(os.path.normpath(args.game_dir))
    collect = os.path.join(REPO, "target/oracle-runs", args.id)
    os.makedirs(collect, exist_ok=True)
    for old in os.listdir(collect):
        if os.path.isfile(os.path.join(collect, old)):
            os.remove(os.path.join(collect, old))

    t0 = time.time()
    st, rels = stage(args.game_dir, region)
    root = os.path.join(RUNS, args.id)
    make_root(args, root, st, rels)
    root_w = wpath(root)
    log(f"root {root_w} ready ({len(rels)} data files linked) in {time.time() - t0:.1f}s")

    watch = None
    watch_out = os.path.join(collect, "fgwatch.jsonl")
    stop_file = os.path.join(root, "f2-watch.stop")
    if args.watch:
        script = wpath(os.path.join(REPO, "tools/run/fgwatch.ps1"))
        watch = subprocess.Popen(
            [PS, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", script,
             "-Prefix", root_w + "\\", "-Seconds", str(int(args.timeout) + 15), "-IntervalMs", "1000",
             "-StopFile", wpath(root) + "\\f2-watch.stop"],
            cwd="/mnt/c/", stdout=open(watch_out, "w"), stderr=subprocess.DEVNULL,
            stdin=subprocess.DEVNULL)
        time.sleep(3)  # baseline samples before launch

    log(f"starting {root_w}\\{EXE} (window {args.window}, mute {not args.audible}, timeout {args.timeout}s)")
    t_launch = time.time()
    env = dict(os.environ)
    if args.compat:
        env["__COMPAT_LAYER"] = args.compat
        env["WSLENV"] = ":".join([e for e in env.get("WSLENV", "").split(":") if e] + ["__COMPAT_LAYER"])
    proc = subprocess.Popen([os.path.join(root, EXE)], cwd=root, env=env, stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    exit_code, killed = None, ""
    while time.time() - t_launch < args.timeout:
        exit_code = proc.poll()
        if exit_code is not None:
            break
        time.sleep(0.5)
    duration = time.time() - t_launch
    if exit_code is None:
        killed = kill_game(root_w)
        log(f"timeout: {killed or 'nothing to kill'}")
        try:
            exit_code = proc.wait(timeout=20)
        except subprocess.TimeoutExpired:
            proc.kill()
    if watch:
        time.sleep(2)
        open(stop_file, "w").close()
        try:
            watch.wait(timeout=20)
        except subprocess.TimeoutExpired:
            watch.kill()

    shots = []
    for name in sorted(os.listdir(root)):
        if re.fullmatch(r"f2-shot-.*\.png|f2-snap-\d+\.json", name) or name in ("f2fix.log", "f2-trace.jsonl"):
            shutil.copyfile(os.path.join(root, name), os.path.join(collect, name))
            if name.endswith(".png"):
                shots.append(name)
    text = ""
    if os.path.isfile(os.path.join(collect, "f2fix.log")):
        with open(os.path.join(collect, "f2fix.log"), encoding="latin-1") as f:
            text = f.read()
    dumps = [{"file": m.group(1), "frame": int(m.group(2)), "mean": float(m.group(3)),
              "non_black_pct": float(m.group(4))}
             for m in re.finditer(r"fbdump: wrote (\S+) \(\d+x\d+, frame (\d+), t \d+ ms, digest \w+, "
                                  r"mean ([0-9.]+), non-black ([0-9.]+)%\)", text)]
    result = {
        "id": args.id,
        "root": root_w,
        "exit_code": exit_code,
        "killed_at_timeout": bool(killed),
        "duration_s": round(duration, 1),
        "screenshots": shots,
        "dumps": dumps,
        "non_black_dumps": sum(1 for d in dumps if d["non_black_pct"] > 1.0),
        "muted": "audio: this process's audio session is muted" in text,
        "dialogs_suppressed": len(re.findall(r"ui: (MessageBoxA|DialogBoxParamA)", text)),
        "watch": summarize_watch(watch_out) if args.watch else None,
    }
    if args.trace:
        result["trace"] = summarize_trace(os.path.join(collect, "f2-trace.jsonl"))
    if args.snap_ticks:
        result["snapshots"] = sorted(n for n in os.listdir(collect) if re.fullmatch(r"f2-snap-\d+\.json", n))
    with open(os.path.join(collect, "result.json"), "w") as f:
        json.dump(result, f, indent=2)
    if not args.keep_root:
        time.sleep(1)
        try:
            rmtree(root)
        except OSError as e:
            log(f"could not remove {root_w}: {e}")
    print(json.dumps(result, indent=2))
    if args.trace:
        return 0 if result["trace"]["lines"] else 1
    return 0 if result["non_black_dumps"] else 1


if __name__ == "__main__":
    sys.exit(main())
