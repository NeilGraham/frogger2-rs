#!/usr/bin/env bash
# Builds the data-free N64 runtime for the desktop app's "Export to N64" (docs/EXPORT.md "N64"):
#
#   tools/runtime/build-n64.sh OUT_DIR VERSION      -> OUT_DIR/runtime-n64-<VERSION>.zip
#
# Contents: runtime.json (format 1: ROM layout, alignment, size limit, pack profile, build
# provenance), ipl3-header.bin (libdragon IPL3 + ROM header as n64tool/ed64romconfig wrote it:
# Expansion Pak required, SRAM 256 Kbit, region free), frogger2.elf.stripped (the linked engine,
# stripped and n64elfcompress'ed), frogger2.elf.sym (n64sym). No game data: the exporter adds the
# player's DragonFS image (f2-package, pure Rust) and replaces the title.
#
# Prerequisites (Linux; what tools/n64/README.md "Prerequisites" installs):
#   - libdragon preview (pinned 39d0d60) + its mips64-elf toolchain at N64_INST, ideally the
#     phase-3A variant (tools/n64/libdragon-3a.sh -> ~/opt/n64root-3a/opt/libdragon; build.sh
#     picks it up automatically, else -3p, else the stock prefix) — the variant is recorded in
#     runtime.json "build";
#   - Rust nightly with rust-src (`rustup toolchain install nightly --component rust-src`;
#     RUST_TOOLCHAIN overrides "nightly") and the stable toolchain of the workspace;
#   - python3, make, git, coreutils.
# No game data is read. Runs `tools/n64/build.sh --runtime` (which replaces
# target/n64/frogger2.z64 with the data-free ROM), then
# `cargo run -p f2-package --example n64_runtime -- make`, which cuts the ROM into the runtime
# and checks that re-assembling the parts reproduces it byte for byte.
set -euo pipefail
if [ $# -ne 2 ]; then
    echo "usage: $0 OUT_DIR VERSION" >&2
    exit 2
fi
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
mkdir -p "$1"
out_dir="$(cd "$1" && pwd)"
version="$2"
zip="$out_dir/runtime-n64-$version.zip"

"$root/tools/n64/build.sh" --runtime

# Which libdragon build.sh used (the same selection it makes).
n64_inst="${N64_INST:-}"
if [ -z "$n64_inst" ]; then
    for v in 3a 3p; do
        if [ -f "$HOME/opt/n64root-$v/opt/libdragon/mips64-elf/lib/libdragon.a" ]; then
            n64_inst="$HOME/opt/n64root-$v/opt/libdragon"
            break
        fi
    done
fi
n64_inst="${n64_inst:-$HOME/opt/n64root/opt/libdragon}"
case "$n64_inst" in
*n64root-3a*) variant=3a ;;
*n64root-3p*) variant=3p ;;
*) variant=stock ;;
esac
rev="$(git -C "$root" rev-parse HEAD 2>/dev/null || echo unknown)"
dirty="$(git -C "$root" status --porcelain 2>/dev/null | grep -q . && echo yes || echo no)"

cargo run --release -q --manifest-path "$root/Cargo.toml" -p f2-package --example n64_runtime -- \
    make "$root/target/n64/frogger2.z64" "$zip" "$version" \
    "git=$rev" "dirty=$dirty" "libdragon=$variant" "rust=$(rustc "+${RUST_TOOLCHAIN:-nightly}" --version)"
echo "runtime: $zip ($(stat -c %s "$zip") bytes)"
