#!/usr/bin/env bash
# Builds a ROM that boots straight into a benchmark scene, runs it in ares and prints the
# per-window perf summary (research/n64-performance.md).
#
#   tools/n64/bench.sh NAME "BOOT" [DELAY_S]
#     NAME   ROM / log name (target/n64/NAME.z64, $BENCH_OUT/NAME.txt, NAME.png)
#     BOOT   F2N64_BOOT, e.g. "level-hop 0 0" (Garden 1, frog hopping: moving camera)
#     DELAY  seconds of emulation before the screenshot and exit (default 40)
#   Env: N64TEST (ares scratch dir, default /mnt/c/f2-n64test-bench; use your own when several
#        runs happen at once), BENCH_OUT (logs, default target/n64/bench), F2N64_FEATURES etc.
#        pass through to build.sh. Reuses target/n64/filesystem (F2N64_KEEP_DFS=1).
# Emulated time only: results do not depend on host load. Expect +-5 % between builds from
# I-cache layout alone (tools/n64/README.md).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
name=$1
boot=$2
delay=${3:-40}
out="${BENCH_OUT:-$root/target/n64/bench}"
mkdir -p "$out"
F2N64_KEEP_DFS=1 F2N64_BOOT="$boot" "$here/build.sh" > "$out/build-$name.log" 2>&1 || { tail -40 "$out/build-$name.log"; exit 1; }
cp "$root/target/n64/frogger2.z64" "$root/target/n64/$name.z64"
cp "$root/target/n64/build/frogger2.elf" "$out/$name.elf"
export N64TEST="${N64TEST:-/mnt/c/f2-n64test-bench}"
timeout 600 "$here/run-ares.sh" "$root/target/n64/$name.z64" -- -delay "$delay" -shots 1 > /dev/null 2>&1 || true
tr -d '\r' < "$N64TEST/run.txt" > "$out/$name.txt"
cp "$N64TEST/$name.png" "$out/$name.png" 2>/dev/null || true
grep -E '^perf' "$out/$name.txt" | tail -4 | python3 -I -c '
import re, sys
for l in sys.stdin:
    g = lambda p: (re.search(p, l).group(1) if re.search(p, l) else "?")
    print(g(r"=> ([\d.]+) fps"), "fps | sim", g(r"sim (\d+) ms"), "draw", g(r"draw (\d+) ms"), "loop", g(r"draw loop (\d+) ms"),
          "submit", g(r"CPU-side submit (\d+) ms"), "queue", g(r"r3d queue (\d+) us"), "flush", g(r"flush (\d+) us"),
          "| tick", g(r"Game::tick (\d+) us"), "poll", g(r"audio poll (\d+) us"), "worst", g(r"worst frame (\d+) ms"))
'
grep -E "^sim phases" "$out/$name.txt" | tail -"${BENCH_PHASE_LINES:-1}" || true
echo "log: $out/$name.txt  elf: $out/$name.elf  shot: $out/$name.png"
