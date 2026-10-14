#!/usr/bin/env bash
# Builds the Frogger 2 N64 ROM: target/n64/frogger2.z64.
#
#   tools/n64/build.sh [--debug] [--small] [--bringup | --r3d-test]
#
# Steps: (1) Rust staticlib crates/frogger-n64 for the custom VR4300 n32 target
# (nightly, -Zbuild-std=core,alloc); (2) relabel its objects n32 -> o64 so GNU ld accepts
# them (tools/n64/n32-to-o64.py); (3) DFS filesystem: the game data pre-pass (below), or
# with --bringup the phase-1B demo's hello.txt + probe data; (4) libdragon link + n64tool
# via tools/n64/Makefile (title "FROGGER 2", Expansion Pak required); (5) size check
# against the 64 MiB cartridge.
#
# Game data (phase 3I, default; never committed, it lives in target/n64/): tools/f2-n64pack
# builds the 1C asset pack from the rendition (F2_GAME_DIR/..: game/, music/, video/) into
# target/n64/pack (levels F2Z, object banks + F2NM meshes and NTX1 textures aPLib, VADPCM
# sound effects with the F2SX index, Opus music, H.264 FMV), copied to the DFS root.
# --small: the pack without music and FMV (target/n64/pack-small): a ROM under ares'
# 0x3FF0000-byte ISViewer limit, so debugf logging works.
#
# F2N64_DATA=prep: the phase-2G layout instead: tools/f2-n64prep lays out
# target/n64/filesystem from F2_GAME_DIR (object banks pre-parsed to postcard, sound banks
# split into metadata + per-sample WAVs); audioconv64 turns the samples into VADPCM wav64
# (streamed from ROM) and the music tracks F2N64_MUSIC into 22 kHz VADPCM wav64; mkasset
# LZ4-compresses the bigger files (asset_load decompresses them transparently).
#
# Env: N64_INST (default ~/opt/n64root-3a/opt/libdragon if built, else -3p, else ~/opt/n64root/opt/libdragon), RUST_TOOLCHAIN (default nightly),
#      F2_GAME_DIR (rendition game/ dir; default <repo>/game-data/usa/game),
#      F2_MUSIC_DIR (default $F2_GAME_DIR/../music), F2N64_MUSIC (track numbers, default
#      "02 09 10 12": Garden, frontend, level complete), F2N64_BOOT (e.g. "level 0 0" to
#      boot straight into Garden 1), F2N64_KEEP_DFS=1 (reuse the last filesystem),
#      F2N64_FEATURES (crate features: profile = sampling profiler, game-log),
#      F2N64_REPLAY=FILE (a PC recording for F2N64_BOOT=replay, as rom:/replay.txt),
#      F2N64_REPLAY_LIST=FILE (chain mode: recordings, one path per line, as rom:/replay/).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
# The phase-3A libdragon variant (tools/n64/libdragon-3a.sh: 3P + mixer/ULC audio patch) when
# installed, else the phase-3P one (tools/n64/libdragon-3p.sh: 2 x 32 KiB rspq ring), else
# the stock one.
if [ -z "${N64_INST:-}" ]; then
    for v in 3a 3p; do
        if [ -f "$HOME/opt/n64root-$v/opt/libdragon/mips64-elf/lib/libdragon.a" ]; then
            N64_INST="$HOME/opt/n64root-$v/opt/libdragon"
            break
        fi
    done
fi
export N64_INST="${N64_INST:-$HOME/opt/n64root/opt/libdragon}"
case "$N64_INST" in *n64root-3a*) ;; *) echo "NOTE: libdragon without the 3A audio patch at $N64_INST (the mixer costs ~2x the CPU; tools/n64/libdragon-3a.sh)" >&2 ;; esac
grep -q "LOWPRI_BUFFER_SIZE   0x2000" "$N64_INST/mips64-elf/include/rspq_constants.h" 2>/dev/null ||
    echo "NOTE: stock libdragon at $N64_INST (2 x 2 KiB rspq ring: the CPU waits for the RSP; tools/n64/libdragon-3p.sh)" >&2
toolchain="${RUST_TOOLCHAIN:-nightly}"
profile=release
bringup=0
r3dtest=0
small=0
for a in "$@"; do
    case "$a" in
    --debug) profile=dev ;;
    --bringup) bringup=1 ;;
    --r3d-test) r3dtest=1 ;;
    --small) small=1 ;;
    *) echo "unknown option $a" >&2; exit 2 ;;
    esac
done
out="$root/target/n64"
fs="$out/filesystem"
mkdir -p "$out"

features="${F2N64_FEATURES:-}"
[ "${F2N64_DATA:-pack}" = prep ] && features="$features prep-data"
[ $small = 1 ] && features="$features small"
[ $bringup = 1 ] && features="$features bringup"
[ $r3dtest = 1 ] && features="$features r3d-test"
echo "== [1/5] Rust staticlib ($profile; features:${features:- none})"
# F2N64_BOOT is read at compile time (option_env!).
# Source paths in panic messages, assert texts and the debug info behind the crash screen's
# backtraces: relative to the repo (crates/...), the standard library and crates.io sources
# under /rust and /cargo (absolute, so tools do not join them onto a crate's directory), no
# home directory or user name in the ROM. rustc lets the last matching mapping win.
sysroot_src="$(rustc "+$toolchain" --print sysroot)/lib/rustlib/src/rust"
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME=~ --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}/registry/src=/cargo --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}/git/checkouts=/cargo-git --remap-path-prefix=$sysroot_src=/rust --remap-path-prefix=$root/="
cargo "+$toolchain" build --manifest-path "$root/crates/frogger-n64/Cargo.toml" \
    --config "$here/cargo-config.toml" \
    --target "$here/mips-nintendo64-none.json" \
    --target-dir "$out/cargo" \
    $([ "$profile" = release ] && echo --release) \
    --features "$features"
pdir=$([ "$profile" = release ] && echo release || echo debug)
lib="$out/cargo/mips-nintendo64-none/$pdir/libfrogger_n64.a"

echo "== [2/5] n32 -> o64 relabel"
python3 -I "$here/n32-to-o64.py" "$lib" "$out/libfrogger_n64_o64.a"
# compiler_builtins defines memcpy & co. as weak symbols; linked first, they would also
# replace newlib's optimised MIPS versions for the C side. Localise them so every caller
# (Rust and libdragon) links newlib's (phase 3P: ~2x faster block copies on the VR4300).
"$N64_INST/bin/mips64-elf-objcopy" -L memcpy -L memmove -L memset -L memcmp -L bcmp "$out/libfrogger_n64_o64.a"
"$N64_INST/bin/mips64-elf-ranlib" "$out/libfrogger_n64_o64.a"

echo "== [3/5] DFS filesystem"
if [ $bringup = 1 ]; then
    rm -rf "$fs"
    mkdir -p "$fs"
    printf 'Hello from the Frogger 2 N64 DFS!\n' > "$fs/hello.txt"
    python3 -I "$here/gen-probe-data.py" "$out/probe.bin"
    for lvl in 0 1 2 3; do
        rm -rf "$out/mkasset"
        mkdir -p "$out/mkasset"
        "$N64_INST/bin/mkasset" -c "$lvl" -w 256 -o "$out/mkasset" "$out/probe.bin"
        mv "$out/mkasset/probe.bin" "$fs/probe_c$lvl.bin"
        echo "   probe_c$lvl.bin: $(stat -c %s "$fs/probe_c$lvl.bin") bytes (of 1048576)"
    done
    rm -rf "$out/mkasset"
elif [ $r3dtest = 1 ]; then
    # Phase-2R renderer test app: scenes written by tools/n64/export (n64r-export) into the
    # filesystem directory (tools/n64/README.md, "RSP renderer test ROM").
    mkdir -p "$fs"
    echo "   r3d test: $(ls "$fs"/scene*.f2nm 2>/dev/null | wc -l) scene(s) in $fs"
elif [ "${F2N64_KEEP_DFS:-0}" = 1 ] && [ -d "$fs/objects" ]; then
    echo "   reusing $fs"
elif [ "${F2N64_DATA:-pack}" = pack ]; then
    game="${F2_GAME_DIR:-$root/game-data/usa/game}"
    [ -d "$game/objects" ] || { echo "ERROR: game data not found at $game (set F2_GAME_DIR)" >&2; exit 1; }
    rendition="$(cd "$game/.." && pwd)"
    pack="$out/pack"
    only=()
    if [ $small = 1 ]; then
        pack="$out/pack-small"
        only=(--only levels,models,textures,sfx)
    fi
    cargo build --release -q --manifest-path "$root/Cargo.toml" -p f2-n64pack
    (cd "$root" && "$root/target/release/f2-n64pack" --rendition "$rendition" --out "$pack" --cache "$out/cache" "${only[@]}")
    rm -rf "$fs"
    cp -r "$pack" "$fs"
    echo "   filesystem (pack): $(du -sk "$fs" | cut -f1) KiB"
else
    game="${F2_GAME_DIR:-$root/game-data/usa/game}"
    [ -d "$game/objects" ] || { echo "ERROR: game data not found at $game (set F2_GAME_DIR)" >&2; exit 1; }
    music_dir="${F2_MUSIC_DIR:-$game/../music}"
    rm -rf "$fs" "$out/wav"
    mkdir -p "$fs"
    cargo build --release -q --manifest-path "$root/Cargo.toml" -p f2-n64prep
    "$root/target/release/f2-n64prep" "$game" "$fs" "$out/wav"
    for d in "$out/wav/sfx"/*/; do
        bank="$(basename "$d")"
        mkdir -p "$fs/sfx/$bank"
        "$N64_INST/bin/audioconv64" --wav-compress vadpcm,huffman=false -o "$fs/sfx/$bank" "$d" >/dev/null
    done
    echo "   sfx: $(find "$fs/sfx" -name '*.wav64' | wc -l) wav64 files, $(du -sk "$fs/sfx" | cut -f1) KiB"
    mkdir -p "$fs/music"
    for t in ${F2N64_MUSIC:-02 09 10 12}; do
        w="$music_dir/track$t.wav"
        if [ -f "$w" ]; then
            "$N64_INST/bin/audioconv64" --wav-compress vadpcm,huffman=false --wav-resample 22050 -o "$fs/music" "$w" >/dev/null
            echo "   music track$t.wav64: $(stat -c %s "$fs/music/track$t.wav64") bytes"
        else
            echo "   music $w missing (skipped)"
        fi
    done
    # LZ4 (level 1, 256 KiB window: asset_load only) for the files the N64 loads whole.
    for d in objects collision maps scenics; do
        [ -d "$fs/$d" ] || continue
        rm -rf "$out/mkasset"
        mkdir -p "$out/mkasset"
        "$N64_INST/bin/mkasset" -c 1 -w 256 -o "$out/mkasset" "$fs/$d"/*
        mv "$out/mkasset"/* "$fs/$d/"
    done
    rm -rf "$out/mkasset" "$out/wav"
    echo "   filesystem: $(du -sk "$fs" | cut -f1) KiB"
fi
# F2N64_REPLAY=FILE: a PC recording (frogger --practice W L --record FILE) as rom:/replay.txt
# for F2N64_BOOT=replay; other builds carry none.
# F2N64_REPLAY_LIST=FILE (chain mode): FILE names recordings, one path per line (blank lines and
# lines starting with '#' skipped), copied to rom:/replay/000.txt, 001.txt, ... and listed in
# rom:/replay/list.txt ("000.txt <name>"); F2N64_BOOT=replay plays them back to back.
rm -rf "$fs/replay.txt" "$fs/replay"
if [ -n "${F2N64_REPLAY:-}" ] && [ -n "${F2N64_REPLAY_LIST:-}" ]; then
    echo "ERROR: F2N64_REPLAY and F2N64_REPLAY_LIST both set" >&2
    exit 1
fi
if [ -n "${F2N64_REPLAY:-}" ]; then
    cp "$F2N64_REPLAY" "$fs/replay.txt"
    echo "   replay: $F2N64_REPLAY ($(stat -c %s "$fs/replay.txt") bytes)"
fi
if [ -n "${F2N64_REPLAY_LIST:-}" ]; then
    mkdir -p "$fs/replay"
    n=0
    while IFS= read -r f || [ -n "$f" ]; do
        f="${f%$'\r'}"
        case "$f" in '' | '#'*) continue ;; esac
        seg="$(printf '%03d.txt' "$n")"
        cp "$f" "$fs/replay/$seg"
        printf '%s %s\n' "$seg" "$(basename "$f" .txt)" >> "$fs/replay/list.txt"
        echo "   replay chain $seg: $f ($(stat -c %s "$fs/replay/$seg") bytes)"
        n=$((n + 1))
    done < "$F2N64_REPLAY_LIST"
    [ $n -gt 0 ] || { echo "ERROR: F2N64_REPLAY_LIST names no recording" >&2; exit 1; }
fi

echo "== [4/5] libdragon link + ROM"
rm -f "$out/frogger2.z64" "$out/build/frogger2.dfs"
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
