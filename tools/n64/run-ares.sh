#!/usr/bin/env bash
# Runs an N64 ROM in ares (Windows, via WSL interop), captures screenshots and the
# ISViewer/debugf log.
#
#   tools/n64/run-ares.sh [rom] [-- ares-shot.ps1 args...]
#     rom   default target/n64/frogger2.z64
#   e.g. tools/n64/run-ares.sh target/n64/frogger2.z64 -- -delay 10 -shots 3 -gap 2   (no -keys: the harness never sends input)
#   -delay is a maximum: a crash in the log (ASSERTION FAILED / CPU EXCEPTION / RUST PANIC)
#   stops the run at once (capture <name>-crash.png, exit code 3); -stopon REGEX stops it
#   early on a log line, e.g. -- -delay 600 -stopon "mem-tour done".
#
# Outputs in C:\f2-n64test\ (not in the repo): <name>.png (+ _1.png ...), run.txt (stdout = debugf).
# Env: ARES_DIR (default C:\Users\<you>\Emulation\emulators\ares-v148, as a /mnt path),
#      N64TEST (default /mnt/c/f2-n64test).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
rom="${1:-$root/target/n64/frogger2.z64}"
[ $# -gt 0 ] && shift
[ "${1:-}" = "--" ] && shift
ares_dir="${ARES_DIR:-/mnt/c/Users/<you>/Emulation/emulators/ares-v148}"
test_dir="${N64TEST:-/mnt/c/f2-n64test}"
mkdir -p "$test_dir"

# ares writes its settings.bml next to ares.exe on first start; seed it if missing.
if [ ! -f "$ares_dir/settings.bml" ]; then
    timeout 30 "$ares_dir/ares.exe" --dump-all-settings >/dev/null 2>&1 || true
fi
python3 -I "$here/ares-settings.py" "$ares_dir/settings.bml" "$test_dir/settings.bml"
cp "$here/ares-shot.ps1" "$test_dir/ares-shot.ps1"
name="$(basename "$rom" .z64)"
cp "$rom" "$test_dir/$name.z64"
win_dir="$(wslpath -w "$test_dir")"
status=0
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$win_dir\\ares-shot.ps1" \
    -rom "$win_dir\\$name.z64" -out "$win_dir\\$name.png" -log "$win_dir\\run.txt" \
    -settings "$win_dir\\settings.bml" "$@" || status=$?
echo "--- debugf log ($test_dir/run.txt) ---"
tr -d '\r' < "$test_dir/run.txt" || true
exit $status
