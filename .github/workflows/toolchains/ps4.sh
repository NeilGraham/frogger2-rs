#!/usr/bin/env bash
# PS4 toolchain (tools/ps4/README.md "Requirements"): OpenOrbis PS4 toolchain v0.5.4
# (LLVM 18 build) and OpenSSL 1.1 for its PkgTool.Core (.NET Core 3.1; Ubuntu 22.04+ has
# only OpenSSL 3). Needs from the system: clang-18, lld-18, ffmpeg, python3, curl.
. "$(dirname "$0")/lib.sh"

fetch https://github.com/OpenOrbis/OpenOrbis-PS4-Toolchain/releases/download/v0.5.4/toolchain-llvm-18.tar.gz \
    3c7cd5bb593ca74fa1c13fd59f3938dc0fc07985167f7275063019e63abe4526 "$DL/openorbis-v0.5.4-llvm-18.tar.gz"
if [ ! -d "$OPT/openorbis/OpenOrbis/PS4Toolchain/lib" ]; then
    mkdir -p "$OPT/openorbis"
    tar -xzf "$DL/openorbis-v0.5.4-llvm-18.tar.gz" -C "$OPT/openorbis"
fi

fetch http://archive.ubuntu.com/ubuntu/pool/main/o/openssl/libssl1.1_1.1.1f-1ubuntu2.24_amd64.deb \
    7cf39d70a639017d1dd7c8d36daa2258063608688e449fddf40ffdd46f992a78 "$DL/libssl1.1_1.1.1f-1ubuntu2.24_amd64.deb"
[ -f "$OPT/libssl11/usr/lib/x86_64-linux-gnu/libssl.so.1.1" ] ||
    dpkg-deb -x "$DL/libssl1.1_1.1.1f-1ubuntu2.24_amd64.deb" "$OPT/libssl11"

setenv OO_PS4_TOOLCHAIN "$OPT/openorbis/OpenOrbis/PS4Toolchain"
setenv F2_LIBSSL11 "$OPT/libssl11/usr/lib/x86_64-linux-gnu"

# Smoke checks: the tools build-pkg.sh runs.
BIN="$OO_PS4_TOOLCHAIN/bin/linux"
test -x "$BIN/create-fself" && test -x "$BIN/create-gp4"
# PkgTool.Core loads OpenSSL 1.1 only when it signs a package (pkg_build).
test -x "$BIN/PkgTool.Core" && test -f "$F2_LIBSSL11/libssl.so.1.1" && test -f "$F2_LIBSSL11/libcrypto.so.1.1"
command -v clang-18 >/dev/null && command -v ld.lld-18 >/dev/null
echo "PS4 toolchain ready"
