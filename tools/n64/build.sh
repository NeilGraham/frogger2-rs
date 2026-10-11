#!/usr/bin/env bash
# Builds the Frogger 2 N64 ROM: target/n64/frogger2.z64.
#
#   tools/n64/build.sh [--debug]
#
# Steps: (1) Rust staticlib crates/frogger-n64 for the custom VR4300 n32 target
# (nightly, -Zbuild-std=core,alloc); (2) relabel its objects n32 -> o64 so GNU ld accepts
# them (tools/n64/n32-to-o64.py); (3) DFS filesystem (hello.txt + probe data at mkasset
# levels 0-3); (4) libdragon link + n64tool via tools/n64/Makefile (title "FROGGER 2",
# Expansion Pak required, 16 KiB padding); (5) size check against the 64 MiB cartridge.
#
# Env: N64_INST (default ~/opt/n64root/opt/libdragon), RUST_TOOLCHAIN (default nightly).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
export N64_INST="${N64_INST:-$HOME/opt/n64root/opt/libdragon}"
toolchain="${RUST_TOOLCHAIN:-nightly}"
profile=release
[ "${1:-}" = "--debug" ] && profile=dev
out="$root/target/n64"
mkdir -p "$out/filesystem"

echo "== [1/5] Rust staticlib ($profile)"
cargo "+$toolchain" build --manifest-path "$root/crates/frogger-n64/Cargo.toml" \
    --config "$here/cargo-config.toml" \
    --target "$here/mips-nintendo64-none.json" \
    --target-dir "$out/cargo" \
    $([ "$profile" = release ] && echo --release)
pdir=$([ "$profile" = release ] && echo release || echo debug)
lib="$out/cargo/mips-nintendo64-none/$pdir/libfrogger_n64.a"

echo "== [2/5] n32 -> o64 relabel"
python3 -I "$here/n32-to-o64.py" "$lib" "$out/libfrogger_n64_o64.a"
"$N64_INST/bin/mips64-elf-ranlib" "$out/libfrogger_n64_o64.a"

echo "== [3/5] DFS filesystem"
printf 'Hello from the Frogger 2 N64 DFS!\n' > "$out/filesystem/hello.txt"
python3 -I "$here/gen-probe-data.py" "$out/probe.bin"
for lvl in 0 1 2 3; do
    rm -rf "$out/mkasset"
    mkdir -p "$out/mkasset"
    "$N64_INST/bin/mkasset" -c "$lvl" -w 256 -o "$out/mkasset" "$out/probe.bin"
    mv "$out/mkasset/probe.bin" "$out/filesystem/probe_c$lvl.bin"
    echo "   probe_c$lvl.bin: $(stat -c %s "$out/filesystem/probe_c$lvl.bin") bytes (of 1048576)"
done
rm -rf "$out/mkasset"

echo "== [4/5] libdragon link + ROM"
rm -f "$out/frogger2.z64"
make -C "$out" -f "$here/Makefile" CSRC="$root/crates/frogger-n64/csrc" \
    RUST_LIB="$out/libfrogger_n64_o64.a" N64_INST="$N64_INST"

echo "== [5/5] size check"
size=$(stat -c %s "$out/frogger2.z64")
max=$((64 * 1024 * 1024))
isv=$((0x3FF0000))
echo "   target/n64/frogger2.z64: $size bytes ($((size / 1024)) KiB) of $max"
if [ "$size" -gt "$max" ]; then
    echo "ERROR: ROM exceeds the 64 MiB cartridge" >&2
    exit 1
fi
if [ "$size" -gt "$isv" ]; then
    echo "WARNING: ROM > 63.94 MiB: ares disables ISViewer (debugf) logging" >&2
fi
