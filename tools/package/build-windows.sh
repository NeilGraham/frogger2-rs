#!/usr/bin/env bash
# Cross-build frogger, f2-import, f2-viewer for x86_64-pc-windows-msvc from WSL using the
# Windows Rust toolchain. cmd.exe rejects UNC cwd and MSVC link on 9P is slow, so sources are
# mirrored to a Windows drive (default /mnt/c/f2-build/<checkout>/src); the target dir persists there.
# Output: exes copied to target/package/win-bin/. Env: F2_WIN_CARGO, F2_BUILD_ROOT.
set -euo pipefail
WINHOME="$(wslpath "$(cmd.exe /c "echo %USERPROFILE%" 2>/dev/null | tr -d "\\r")" 2>/dev/null || true)"   # your Windows home (WSL)
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CARGO_EXE="${F2_WIN_CARGO:-$WINHOME/.cargo/bin/cargo.exe}"
BUILD_ROOT="${F2_BUILD_ROOT:-/mnt/c/f2-build}"
MIRROR="$BUILD_ROOT/$(basename "$REPO")"
OUT="$REPO/target/package/win-bin"
[[ -x "$CARGO_EXE" ]] || { echo "Windows cargo not found: $CARGO_EXE" >&2; exit 1; }
mkdir -p "$MIRROR/src" "$MIRROR/target" "$OUT"
rsync -a --checksum --delete --exclude /target --exclude .git --exclude /game-data \
  --exclude /work --exclude /research --exclude /crates/f2fix/target --exclude /frogger2-vss \
  --exclude /.claude "$REPO/" "$MIRROR/src/"
cd "$MIRROR/src"
export CARGO_TARGET_DIR="$(wslpath -w "$MIRROR/target")"
export WSLENV="${WSLENV:-}:CARGO_TARGET_DIR"
"$CARGO_EXE" build --release --target x86_64-pc-windows-msvc \
  -p frogger -p f2-import -p f2-viewer
for b in frogger2 f2-import f2-viewer launcher; do
  cp "$MIRROR/target/x86_64-pc-windows-msvc/release/$b.exe" "$OUT/"
done
ls -l "$OUT"
