#!/usr/bin/env bash
# N64 toolchain (tools/n64/README.md "Prerequisites"): libdragon's GCC 16.2 mips64-elf
# toolchain (.deb, extracted, no root needed) + libdragon preview at 39d0d60 with its tools,
# then the phase-3A variant (tools/n64/libdragon-3a.sh: 3P + 3A patches, videoconv64) that
# tools/n64/build.sh prefers. Result: N64_INST=$HOME/opt/n64root-3a/opt/libdragon.
# Needs from the system: gcc/g++/make (build-essential), curl, git, patch.
. "$(dirname "$0")/lib.sh"
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"

LIBDRAGON=39d0d6096130836a65710da6730290701d060272
# libdragon publishes the toolchain only as a rolling release ("toolchain-continuous-
# prerelease"); this is the build tested here (gcc-toolchain-mips64 16.2.0.34909605681).
# When it is replaced upstream the hash check fails: re-test and update both lines.
TC_URL=https://github.com/DragonMinded/libdragon/releases/download/toolchain-continuous-prerelease/gcc-toolchain-mips64-x86_64.deb
TC_SHA=fac8e6572493a66468b7d45df41b042f90b1a630ec96aa218bd5fb1f320a0a4d
stamp="$LIBDRAGON $TC_SHA $(cat "$repo"/tools/n64/libdragon-3p.patch "$repo"/tools/n64/libdragon-3a.patch "$repo"/tools/n64/libdragon-3a.sh | sha256sum | cut -c1-16)"
N64_3A="$OPT/n64root-3a/opt/libdragon"

if [ "$(cat "$OPT/n64root-3a/.f2-stamp" 2>/dev/null)" != "$stamp" ]; then
    rm -rf "$OPT/n64root" "$OPT/n64root-3a" "$OPT/libdragon-3a"
    fetch "$TC_URL" "$TC_SHA" "$DL/gcc-toolchain-mips64-x86_64.deb"
    dpkg-deb -x "$DL/gcc-toolchain-mips64-x86_64.deb" "$OPT/n64root"
    checkout https://github.com/DragonMinded/libdragon "$LIBDRAGON" "$OPT/libdragon"
    # Stock libdragon + tools (n64tool, mkdfs, mkasset, audioconv64, ...) into the stock prefix,
    # as libdragon's own build.sh does; libdragon-3a.sh copies that prefix and patches a copy.
    export N64_INST="$OPT/n64root/opt/libdragon"
    make -C "$OPT/libdragon" -j"$(nproc)" libdragon tools >/dev/null
    make -C "$OPT/libdragon" install tools-install >/dev/null
    LIBDRAGON_SRC="$OPT/libdragon" N64ROOT="$OPT/n64root" "$repo/tools/n64/libdragon-3a.sh"
    unset N64_INST
    echo "$stamp" > "$OPT/n64root-3a/.f2-stamp"
fi

setenv N64_INST "$N64_3A"
test -f "$N64_INST/mips64-elf/lib/libdragon.a"
for t in mips64-elf-gcc n64tool mkdfs mkasset audioconv64 videoconv64; do test -x "$N64_INST/bin/$t"; done
"$N64_INST/bin/mips64-elf-gcc" --version | head -1
echo "N64 toolchain ready"
