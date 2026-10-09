#!/usr/bin/env bash
# tools/run/build_f2fix.sh: build the 32-bit f2fix.dll (crates/f2fix) from WSL
# with the Windows Rust toolchain, for i686-pc-windows-msvc.
#
# Usage: tools/run/build_f2fix.sh [--debug]       (default: release)
#
# Adapted from theme-park-world-recomp tools/build.sh (MIT, same owner):
# cargo.exe refuses a UNC (\\wsl$\...) working directory and MSVC link.exe is
# slow/flaky on 9P paths, so the crate's sources are rsynced to a Windows-side
# mirror and built there:
#     C:\f2-build\<checkout>\f2fix    (rsync --delete mirror of crates/f2fix)
#     C:\f2-build\<checkout>\target   (CARGO_TARGET_DIR, persists)
# f2fix is a standalone cargo workspace (no path dependencies), so only its own
# directory is mirrored. The DLL is copied to target/oracle-bin/f2fix.dll
# (gitignored) and checked: PE32, i386, DLL, DirectInput exports present.
#
# Environment overrides:
#   F2_WIN_CARGO    cargo.exe (default /mnt/c/Users/<you>/.cargo/bin/cargo.exe)
#   F2_BUILD_ROOT   WSL path of the Windows build root (default /mnt/c/f2-build)
set -euo pipefail

PROFILE=release
case "${1:-}" in
  "" | --release) ;;
  --debug) PROFILE=debug ;;
  *) echo "usage: $0 [--debug|--release]" >&2; exit 2 ;;
esac

TARGET=i686-pc-windows-msvc
CARGO_EXE="${F2_WIN_CARGO:-/mnt/c/Users/<you>/.cargo/bin/cargo.exe}"
BUILD_ROOT="${F2_BUILD_ROOT:-/mnt/c/f2-build}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
NAME="$(basename "$REPO")"
MIRROR="$BUILD_ROOT/$NAME"
OUT="$REPO/target/oracle-bin"

[[ -x "$CARGO_EXE" ]] || { echo "build_f2fix: Windows cargo not found: $CARGO_EXE" >&2; exit 1; }
case "$BUILD_ROOT" in /mnt/[a-z]/*) ;; *) echo "build_f2fix: F2_BUILD_ROOT must be on a Windows drive" >&2; exit 1 ;; esac
command -v rsync >/dev/null || { echo "build_f2fix: rsync is required" >&2; exit 1; }

mkdir -p "$MIRROR/f2fix" "$MIRROR/target" "$OUT"
rsync -a --checksum --delete --exclude 'target/' "$REPO/crates/f2fix/" "$MIRROR/f2fix/"

WIN_TARGET_DIR="$(wslpath -w "$MIRROR/target")"
ARGS=(build --target "$TARGET")
[[ "$PROFILE" == release ]] && ARGS+=(--release)
echo "build_f2fix: f2fix ($TARGET, $PROFILE) in $(wslpath -w "$MIRROR/f2fix")"
(cd "$MIRROR/f2fix" && CARGO_TARGET_DIR="$WIN_TARGET_DIR" WSLENV="${WSLENV:+$WSLENV:}CARGO_TARGET_DIR" \
    "$CARGO_EXE" "${ARGS[@]}")
if [[ "${F2_WIN_CLIPPY:-0}" == 1 ]]; then
  (cd "$MIRROR/f2fix" && CARGO_TARGET_DIR="$WIN_TARGET_DIR" WSLENV="${WSLENV:+$WSLENV:}CARGO_TARGET_DIR" \
      "$CARGO_EXE" clippy --target "$TARGET" -- -D warnings)
fi

ART="$MIRROR/target/$TARGET/$PROFILE"
cp -f "$ART/f2fix.dll" "$OUT/f2fix.dll"
[[ -f "$ART/f2fix.pdb" ]] && cp -f "$ART/f2fix.pdb" "$OUT/f2fix.pdb"

python3 -I - "$OUT/f2fix.dll" <<'PY'
import struct, sys
path = sys.argv[1]
d = open(path, "rb").read()
pe = struct.unpack_from("<I", d, 0x3C)[0]
assert d[:2] == b"MZ" and d[pe:pe + 4] == b"PE\0\0", "not a PE file"
machine, nsect = struct.unpack_from("<HH", d, pe + 4)
opt_size, chars = struct.unpack_from("<HH", d, pe + 20)
opt = pe + 24
if machine != 0x14C or struct.unpack_from("<H", d, opt)[0] != 0x10B or not chars & 0x2000:
    sys.exit(f"{path}: not a PE32 i386 DLL")
exp_rva = struct.unpack_from("<I", d, opt + 96)[0]
secs = [struct.unpack_from("<8sIIII", d, opt + opt_size + 40 * i) for i in range(nsect)]
def off(rva):
    for _, vs, va, rs, raw in secs:
        if va <= rva < va + max(vs, rs):
            return rva - va + raw
    sys.exit(f"rva {rva:#x} outside sections")
e = off(exp_rva)
n, _, names_rva = struct.unpack_from("<III", d, e + 24)
names = []
for i in range(n):
    p = off(struct.unpack_from("<I", d, off(names_rva) + 4 * i)[0])
    names.append(d[p:d.index(b"\0", p)].decode("ascii"))
need = {"DirectInputCreateEx", "DirectInputCreateA"}
if not need <= set(names):
    sys.exit(f"{path}: missing exports {need - set(names)}; have {names}")
print(f"build_f2fix: OK {path}: PE32 i386 DLL, {len(d)} bytes, exports: {', '.join(names)}")
PY
