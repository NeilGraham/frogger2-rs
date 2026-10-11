#!/usr/bin/env bash
# Builds the phase-3P libdragon variant next to the stock install (which stays untouched):
#   tools/n64/libdragon-3p.sh
# Copies the toolchain prefix ~/opt/n64root -> ~/opt/n64root-3p and the libdragon source
# ~/opt/libdragon (39d0d60) -> ~/opt/libdragon-3p, applies tools/n64/libdragon-3p.patch
# (rspq low-priority ring 2 x 32 KiB instead of 2 x 2 KiB, plus rspq_set_lowpri_window()
# for the H.264 decoder), and installs libdragon into the copy. Use it with
#   N64_INST=~/opt/n64root-3p/opt/libdragon tools/n64/build.sh
# (build.sh prefers it automatically when present; see tools/n64/README.md).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
src="${LIBDRAGON_SRC:-$HOME/opt/libdragon}"
root="${N64ROOT:-$HOME/opt/n64root}"
[ -d "$HOME/opt/n64root-3p" ] || cp -a "$root" "$HOME/opt/n64root-3p"
if [ ! -d "$HOME/opt/libdragon-3p" ]; then
    cp -a "$src" "$HOME/opt/libdragon-3p"
    patch -d "$HOME/opt/libdragon-3p" -p1 < "$here/libdragon-3p.patch"
fi
export N64_INST="$HOME/opt/n64root-3p/opt/libdragon"
make -C "$HOME/opt/libdragon-3p" clean >/dev/null
make -C "$HOME/opt/libdragon-3p" -j"$(nproc)" libdragon
make -C "$HOME/opt/libdragon-3p" install
grep LOWPRI_BUFFER_SIZE "$N64_INST/mips64-elf/include/rspq_constants.h"
