#!/usr/bin/env bash
# Native PS5 toolchain (tools/ps5/README.md): ps5-payload-sdk v0.43 (stub libraries,
# prospero-lld), PS5_Vulkan at 252bf91 (tooling/native: the ELF converter, fake-SELF signer,
# libc.prx builder), bc7enc_rdo at b943862 (DDS artwork). Needs from the system: clang-18,
# clang++-18, lld-18, llvm-18 (llvm-config-18 for prospero-lld), cmake, ffmpeg, python3, wget.
# Not set up (not needed by a runtime): the --pkg/--ffpkg/--ffpfsc packers and the --gpu
# RADV driver (tools/ps5/build-vulkan-driver.sh, lld-21).
. "$(dirname "$0")/lib.sh"

fetch https://github.com/ps5-payload-dev/sdk/releases/download/v0.43/ps5-payload-sdk.zip \
    a9cc9929f21b2b2c5d5b309f3bab4997067c45281c0622cf4838b1aecba66fcb "$DL/ps5-payload-sdk-v0.43.zip"
if [ ! -x "$OPT/ps5-payload-sdk/bin/prospero-lld" ]; then
    rm -rf "$OPT/ps5-payload-sdk"
    unzip -q "$DL/ps5-payload-sdk-v0.43.zip" -d "$OPT"
fi

checkout https://github.com/mihawk-99/PS5_Vulkan 252bf912a745e3566a93b69b098bdc8ec4048837 "$OPT/PS5_Vulkan"

checkout https://github.com/richgel999/bc7enc_rdo b9438627eef73a1157e84201b6fa6eb2ffd6d9f0 "$OPT/ref/bc7enc_rdo"
if [ ! -x "$OPT/ref/bc7enc_rdo/build/bc7enc" ]; then
    cmake -S "$OPT/ref/bc7enc_rdo" -B "$OPT/ref/bc7enc_rdo/build" -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_CXX_FLAGS="-include cstdint" -DCMAKE_EXE_LINKER_FLAGS=-no-pie >/dev/null
    cmake --build "$OPT/ref/bc7enc_rdo/build" -j"$(nproc)" >/dev/null
fi

setenv PS5_PAYLOAD_SDK "$OPT/ps5-payload-sdk"
setenv PS5_VULKAN "$OPT/PS5_Vulkan"
setenv F2_BC7ENC "$OPT/ref/bc7enc_rdo/build/bc7enc"

test -f "$PS5_VULKAN/tooling/native/sce_module_writer.cpp"
"$PS5_PAYLOAD_SDK/bin/prospero-llvm-config" --version >/dev/null
command -v clang-18 >/dev/null && command -v clang++-18 >/dev/null
echo "PS5 toolchain ready"
