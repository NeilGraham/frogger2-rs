#!/usr/bin/env bash
# Chain mode: several one-level PC recordings played back to back in ONE N64 boot, so the
# heaps, renderer caches and audio carry over from level to level as in a real session
# (tools/n64/README.md "Replays").
#
#   tools/n64/replay-chain.sh [--chain MODE] FILE_OR_DIR...
#   e.g. tools/n64/replay-chain.sh --chain fastest /mnt/c/Frogger2-rs/recordings
#        tools/n64/replay-chain.sh "story 02 Boulder Canyon #1.txt" "story 03 The Gold Mine #3.txt"
#
# Only recordings that reached level complete (a `# mode T 3` line) are used. Per level (the
# `# f2rec` header: story chapter / practice W L) MODE picks:
#   fastest (default)  the lowest level-complete tick      slowest   the highest
#   random[:SEED]      one at random (the seed is printed)  all       every completed one
# Segments run in story order (chapter number), then the practice / retro levels (W, L).
# Explicit files (not directories) are all chained in the order given, MODE ignored.
# --dry-run: print the selection only.
# Builds with F2N64_BOOT=replay F2N64_KEEP_DFS=1 F2N64_REPLAY_LIST, runs ares until
# "replay chain done", a desync, a failure or a crash.
# Outputs in $OUT (default target/n64/replays/): chain.list, chain.z64, chain.log, chain*.png.
# Env: N64TEST (default /mnt/c/f2-n64test-chain), DELAY (max seconds, default 5400).
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
out="${OUT:-$root/target/n64/replays}"
export N64TEST="${N64TEST:-/mnt/c/f2-n64test-chain}"
mode=fastest
dry=0
args=()
while [ $# -gt 0 ]; do
    case "$1" in
        --chain) mode="${2:?--chain MODE}"; shift 2 ;;
        --chain=*) mode="${1#--chain=}"; shift ;;
        --dry-run) dry=1; shift ;;
        *) args+=("$1"); shift ;;
    esac
done
[ ${#args[@]} -gt 0 ] || { echo "usage: $0 [--chain fastest|slowest|random[:SEED]|all] [--dry-run] FILE_OR_DIR..." >&2; exit 1; }
mkdir -p "$out"
list="$out/chain.list"
# Selection (python3 -I: the recordings are only read).
if ! python3 -I - "$mode" "${args[@]}" > "$list" <<'EOF'; then
import os, random, re, sys
mode, paths = sys.argv[1], sys.argv[2:]
def info(f):
    start, done = None, None
    with open(f, encoding="utf-8", errors="replace") as fh:
        for line in fh:
            if line.startswith("# f2rec "):
                w = line.split()
                if len(w) >= 5 and w[3] == "story":
                    start = (0, int(w[4]), 0)
                elif len(w) >= 6 and w[3] == "practice":
                    start = (1, int(w[4]), int(w[5]))
            elif done is None:
                m = re.match(r"# mode (\d+) 3\s*$", line)
                if m:
                    done = int(m.group(1))
    return start, done
explicit, found = [], []
for p in paths:
    if os.path.isdir(p):
        found += sorted(os.path.join(p, n) for n in os.listdir(p) if n.endswith(".txt"))
    else:
        explicit.append(p)
levels = {}
for f in found:
    start, done = info(f)
    if start is None or done is None:
        continue
    levels.setdefault(start, []).append((done, f))
seed = None
if mode.startswith("random"):
    seed = int(mode.split(":", 1)[1]) if ":" in mode else random.SystemRandom().randrange(1 << 31)
    print(f"seed {seed}", file=sys.stderr)
    rng = random.Random(seed)
elif mode not in ("fastest", "slowest", "all"):
    sys.exit(f"unknown --chain mode {mode} (fastest | slowest | random[:SEED] | all)")
chosen = []
for key in sorted(levels):
    recs = levels[key]
    if mode == "fastest":
        chosen.append(min(recs))
    elif mode == "slowest":
        chosen.append(max(recs))
    elif mode == "all":
        chosen += sorted(recs, key=lambda r: r[1])
    else:
        chosen.append(rng.choice(sorted(recs, key=lambda r: r[1])))
for f in explicit:
    chosen.append((info(f)[1], f))
if not chosen:
    sys.exit("no recording reached level complete")
for done, f in chosen:
    print(f"  {os.path.basename(f)}: level complete at tick {done}", file=sys.stderr)
    print(f)
EOF
    exit 1
fi
what="files as given"
for a in "${args[@]}"; do [ -d "$a" ] && what="$mode"; done
echo "== chain ($what): $(grep -c '' "$list") segments ($list)"
[ $dry = 0 ] || exit 0
if ! F2N64_BOOT=replay F2N64_KEEP_DFS=1 F2N64_REPLAY_LIST="$list" "$here/build.sh" > "$out/chain.build.log" 2>&1; then
    echo "BUILD FAILED ($out/chain.build.log)"
    exit 1
fi
cp "$root/target/n64/frogger2.z64" "$out/chain.z64"
"$here/run-ares.sh" "$out/chain.z64" -- -delay "${DELAY:-5400}" -stopon "replay chain done|REPLAY DESYNC|REPLAY FAILED" > "$out/chain.log" 2>&1
status=$?
cp "$N64TEST/chain"*.png "$out/" 2>/dev/null
echo "== segments"
grep -a -E "replay: segment [0-9]+/[0-9]+ .*(start|done)|replay: tick [0-9]+ mode 3$|REPLAY DESYNC|REPLAY FAILED" "$out/chain.log"
echo "== memory (level loads, segment ends)"
grep -a -E "^mem \[[^]]*\]: .*this load|LEVEL HEAP FULL|ALLOC FAILED|Out of memory|replay: new session" "$out/chain.log" | cut -c1-300
echo "main heap full (play-time blocks to the level heap) reports: $(grep -a -c "main heap full" "$out/chain.log")"
echo "== result"
result="$(grep -a -m1 -E "replay chain done|REPLAY DESYNC|REPLAY FAILED" "$out/chain.log")"
if [ $status = 3 ]; then
    result="CRASH: $(grep -a -m1 -E "ASSERTION FAILED|CPU EXCEPTION|RUST PANIC|ALLOC FAILED" "$out/chain.log") (last segment started: $(grep -a -E "replay: segment .* start" "$out/chain.log" | tail -1))"
fi
echo "${result:-no result (timeout?)}"
echo "log: $out/chain.log"
