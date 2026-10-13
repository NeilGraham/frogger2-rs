#!/usr/bin/env bash
# Builds the phase-3A libdragon variant: the phase-3P one (libdragon-3p.patch) plus the
# audio patch libdragon-3a.patch, in its own copies (the stock and 3P installs stay untouched):
#   tools/n64/libdragon-3a.sh
# Copies the toolchain prefix ~/opt/n64root -> ~/opt/n64root-3a and the libdragon source
# ~/opt/libdragon (39d0d60) -> ~/opt/libdragon-3a, applies both patches, installs.
# libdragon-3a.patch (research/n64-performance.md, 3A):
# - mixer: a streamed channel's per-round cap uses its ring's real mirrored tail (ULC: 1024
#   samples) instead of the fixed 128-unit margin: the 32 kHz ULC music had capped every
#   mixer round at 111 samples (~290 rounds/s of per-channel CPU bookkeeping, now ~63);
# - ULC: a 4 KiB read-ahead window of compressed bytes refilled by asynchronous PI DMA
#   instead of a synchronous lseek + read of max_block_size per 1024-sample block;
# - profile slots ulc_io / ulc_decode / ulc_coef (main.c F2N64_AUDIO_PROF dumps them);
# - videoconv64: x264 reference frames from VCONV64_H264_REF (f2-n64pack passes 1: the H.264
#   decoder's picture buffers shrink from ~1 MiB to ~230 KiB while a video plays).
# build.sh prefers ~/opt/n64root-3a, then ~/opt/n64root-3p, then the stock prefix; after
# switching prefixes, rm -rf target/n64/build.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
src="${LIBDRAGON_SRC:-$HOME/opt/libdragon}"
root="${N64ROOT:-$HOME/opt/n64root}"
[ -d "$HOME/opt/n64root-3a" ] || cp -a "$root" "$HOME/opt/n64root-3a"
if [ ! -d "$HOME/opt/libdragon-3a" ]; then
    cp -a "$src" "$HOME/opt/libdragon-3a"
    patch -d "$HOME/opt/libdragon-3a" -p1 < "$here/libdragon-3p.patch"
    patch -d "$HOME/opt/libdragon-3a" -p1 < "$here/libdragon-3a.patch"
fi
export N64_INST="$HOME/opt/n64root-3a/opt/libdragon"
make -C "$HOME/opt/libdragon-3a" clean >/dev/null
make -C "$HOME/opt/libdragon-3a" -j"$(nproc)" libdragon
make -C "$HOME/opt/libdragon-3a" install
make -C "$HOME/opt/libdragon-3a/tools" -j"$(nproc)" videoconv64
cp "$HOME/opt/libdragon-3a/tools/videoconv64/videoconv64" "$N64_INST/bin/videoconv64"
grep -q "F2 (3A)" "$HOME/opt/libdragon-3a/src/audio/mixer.c" && echo "libdragon-3a installed at $N64_INST"
