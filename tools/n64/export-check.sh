#!/usr/bin/env bash
# Byte-for-byte check of the desktop app's N64 export (docs/EXPORT.md "N64") against
# tools/n64/build.sh, for one engine build and one imported game:
#
#   F2_GAME_DIR=/path/to/game-data/usa/game tools/n64/export-check.sh [--skip-default] [SCRATCH]
#
# 1. tools/runtime/build-n64.sh SCRATCH 0.0.0-check  -> the data-free runtime (build.sh --runtime)
# 2. build.sh (default pack: ULC music, H.264 FMV, made with the libdragon tools) -> ROM A;
#    the runtime + mkdfs-equivalent DFS of target/n64/filesystem (f2-package, pure Rust) -> A'.
#    A == A' checks the DFS writer and the ROM assembly (n64tool + ed64romconfig) on the real ROM.
#    (--skip-default skips this step.)
# 3. build.sh --portable -> ROM B (f2-n64pack --portable, then mkdfs + n64tool)
# 4. frogger --export n64 --runtime <zip> -> ROM C (all pure Rust). B == C.
# Prints the sizes, hashes and the result; exit status 1 on a difference. Game-derived outputs
# stay in SCRATCH (default /tmp/claude-$UID/n64-export-check) and target/: never commit them.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
skip_default=0
if [ "${1:-}" = --skip-default ]; then
    skip_default=1
    shift
fi
scratch="${1:-/tmp/claude-$(id -u)/n64-export-check}"
mkdir -p "$scratch"
game="${F2_GAME_DIR:-$root/game-data/usa/game}"
[ -d "$game/objects" ] || { echo "ERROR: game data not found at $game (set F2_GAME_DIR)" >&2; exit 1; }
rendition="$(cd "$game/.." && pwd)"
tool=(cargo run --release -q --manifest-path "$root/Cargo.toml" -p f2-package --example n64_runtime --)
fail=0
same() { # A B label
    if cmp -s "$1" "$2"; then
        echo "IDENTICAL  $3: $(stat -c %s "$1") bytes, sha256 $(sha256sum "$1" | cut -c1-16)"
    else
        echo "DIFFERENT  $3: $1 ($(stat -c %s "$1")) vs $2 ($(stat -c %s "$2")); first difference: $(cmp "$1" "$2" | head -1)"
        fail=1
    fi
}

echo "=== 1. runtime"
"$root/tools/runtime/build-n64.sh" "$scratch" 0.0.0-check
rt="$scratch/runtime-n64-0.0.0-check.zip"
"${tool[@]}" info "$rt"

if [ $skip_default = 0 ]; then
    echo "=== 2. default build.sh ROM vs runtime + Rust DFS"
    F2_GAME_DIR="$game" "$here/build.sh"
    cp "$root/target/n64/frogger2.z64" "$scratch/A-buildsh-default.z64"
    "${tool[@]}" rom "$rt" "$root/target/n64/filesystem" "$scratch/A2-rust-default.z64" "FROGGER 2"
    same "$scratch/A-buildsh-default.z64" "$scratch/A2-rust-default.z64" "default ROM (build.sh) vs runtime + f2-package DFS/ROM"
fi

echo "=== 3. build.sh --portable"
F2_GAME_DIR="$game" "$here/build.sh" --portable
cp "$root/target/n64/frogger2.z64" "$scratch/B-buildsh-portable.z64"

echo "=== 4. frogger --export n64"
rm -rf "$scratch/export"
cargo run --release -q --manifest-path "$root/Cargo.toml" -p frogger -- --data "$rendition" --export n64 --runtime "$rt" --out "$scratch/export"
c="$(ls "$scratch/export"/*.z64)"
same "$scratch/B-buildsh-portable.z64" "$c" "build.sh --portable vs frogger --export n64"
"${tool[@]}" info "$c"
exit $fail
