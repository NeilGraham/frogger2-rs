#!/usr/bin/env bash
# Replays PC recordings (frogger --practice W L --record FILE) in ares, one ROM each, and
# prints one result line per recording (tools/n64/README.md "Replays").
#
#   tools/n64/replay-all.sh FILE_OR_DIR...      e.g. tools/n64/replay-all.sh /mnt/c/Frogger2-rs/recordings
#
# Per recording: build with F2N64_BOOT=replay F2N64_KEEP_DFS=1 F2N64_REPLAY=FILE, run until
# "replay done", a desync or a crash (crash screen saved by run-ares.sh), keep the log.
# Outputs in $OUT (default target/n64/replays/): <name>.z64, <name>.log, <name>*.png.
# Env: N64TEST (default /mnt/c/f2-n64test-replay), DELAY (max seconds per run, default 1800).
#
#   tools/n64/replay-all.sh --chain MODE DIR    chain mode instead: one ROM, the levels back to
#                                               back (tools/n64/replay-chain.sh)
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
case "${1:-}" in --chain | --chain=*) exec "$here/replay-chain.sh" "$@" ;; esac
root="$(cd "$here/../.." && pwd)"
out="${OUT:-$root/target/n64/replays}"
export N64TEST="${N64TEST:-/mnt/c/f2-n64test-replay}"
mkdir -p "$out"
files=()
for a in "$@"; do
    if [ -d "$a" ]; then
        while IFS= read -r f; do files+=("$f"); done < <(find "$a" -maxdepth 1 -name '*.txt' | sort)
    else
        files+=("$a")
    fi
done
[ ${#files[@]} -gt 0 ] || { echo "usage: $0 FILE_OR_DIR..." >&2; exit 1; }
summary=()
for f in "${files[@]}"; do
    name="$(basename "$f" .txt | tr -c 'A-Za-z0-9_.\n-' '_')"
    echo "== $name"
    if ! F2N64_BOOT=replay F2N64_KEEP_DFS=1 F2N64_REPLAY="$f" "$here/build.sh" > "$out/$name.build.log" 2>&1; then
        summary+=("$name: BUILD FAILED ($out/$name.build.log)")
        continue
    fi
    cp "$root/target/n64/frogger2.z64" "$out/$name.z64"
    "$here/run-ares.sh" "$out/$name.z64" -- -delay "${DELAY:-1800}" -stopon "replay done|REPLAY DESYNC|REPLAY FAILED" > "$out/$name.log" 2>&1
    status=$?
    cp "$N64TEST/$name"*.png "$out/" 2>/dev/null
    result="$(grep -a -m1 -E "replay done|REPLAY DESYNC|REPLAY FAILED" "$out/$name.log")"
    complete="$(grep -a -m1 -E "replay: tick [0-9]+ mode 3$" "$out/$name.log" | grep -oE "tick [0-9]+")"
    if [ $status = 3 ]; then
        result="CRASH: $(grep -a -m1 -E "ASSERTION FAILED|CPU EXCEPTION|RUST PANIC|ALLOC FAILED" "$out/$name.log")"
    fi
    summary+=("$name: ${result:-no result (timeout?)}${complete:+ | level complete at $complete}")
    echo "   ${summary[-1]}"
done
echo
echo "== summary"
printf '%s\n' "${summary[@]}"
