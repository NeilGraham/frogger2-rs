#!/usr/bin/env bash
# Builds the native PS5 homebrew app of the Frogger 2 port: a directory title
# (eboot.bin + sce_sys/ + sce_module/libc.prx) for etaHEN / ShadowMountPlus / Itemzflow
# under /data/homebrew/<TITLE_ID>, optionally also a mountable .ffpkg image.
#
#   tools/ps5/build.sh [--data RENDITION_DIR] [--icon IMAGE] [--out DIR] [--smoke] [--ffpkg] [--gpu]
#
#   --data DIR   bundle your own imported game data (an f2-import rendition, e.g.
#                game-data/usa) at /app0/data. The result contains copyrighted data: keep it
#                for your own console, never share it. Without it the game looks in
#                /data/frogger2 or on USB (/mnt/usbN/frogger2) at run time.
#   --icon IMG   artwork for the 512x512 launcher icon (centred square crop; set
#                F2_ICON_CROP_X to move it). Default: the game's title backdrop exported
#                from --data, else a plain green tile.
#   --out DIR    output directory (default target/ps5).
#   --smoke      build the Rust std smoke test instead of the game (feature `smoke`).
#   --ffpkg      also build <TITLE_ID>.ffpkg (UFS2 image, needs UFS2Tool: $F2_UFS2TOOL or
#                PS5_Vulkan's .deps/UFS2Tool build).
#   --gpu        eboot.bin = the wgpu renderer on PS5_Vulkan's RADV driver (needs
#                tools/ps5/build-vulkan-driver.sh and lld-21; tools/ps5/VULKAN.md), with the
#                CPU build as eboot_cpu.bin, started instead when Vulkan does not come up.
#
# Needs (see tools/ps5/README.md): the ps5-payload-sdk ($PS5_PAYLOAD_SDK, default
# ~/opt/ps5-payload-sdk; its stubs, ld.lld wrapper), a PS5_Vulkan checkout for its native
# packaging tools ($PS5_VULKAN, default ~/opt/PS5_Vulkan), clang/clang++ 18, rustup target
# x86_64-unknown-freebsd, ffmpeg (icon), python3. zlib for the packaging tool is fetched
# and built into the output's tools/ directory when $F2_ZLIB does not point at one.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SDK="${PS5_PAYLOAD_SDK:-$HOME/opt/ps5-payload-sdk}"
PS5VK="${PS5_VULKAN:-$HOME/opt/PS5_Vulkan}"
DATA=""
ICON=""
OUT="$ROOT/target/ps5"
SMOKE=0
FFPKG=0
GPU=0
while [ $# -gt 0 ]; do
    case "$1" in
        --data) DATA="$(cd "$2" && pwd)"; shift 2 ;;
        --icon) ICON="$(realpath "$2")"; shift 2 ;;
        --out) OUT="$(realpath -m "$2")"; shift 2 ;;
        --smoke) SMOKE=1; shift ;;
        --ffpkg) FFPKG=1; shift ;;
        --gpu) GPU=1; shift ;;
        -h|--help) sed -n '2,28p' "$0"; exit 0 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done

TITLE="Frogger 2: Swampy's Revenge"
TITLE_ID="PPSA99920"
CONTENT_ID="IV0000-${TITLE_ID}_00-FROGGER2RUSTPORT"
# Application version (param.json NN.NNN.NNN); bump for updates over an installed copy.
VERSION="01.000.000"
if [ "$SMOKE" = 1 ]; then
    TITLE="Frogger 2 PS5 std smoke test"
    TITLE_ID="PPSA99921"
    CONTENT_ID="IV0000-${TITLE_ID}_00-FROGGER2SMOKETST"
fi
# PS5_Vulkan revision whose tooling/native this was written against (libc.prx is checked
# by hash below, so another revision fails loudly if the runtime changed).
PS5VK_REV="252bf91"
LIBC_PRX_SHA256="e6ff45d16adf687855cc3b33b0c8a4132b6504360b221e0a34c7e99fb3ba0036"
# Loader constants PS5_Vulkan validated on firmware 6.02 and 12.70 (its tools/build.sh).
MODULE_SDK=0x02000009
COMPANION_SDK=0x08050001
FSELF_MAGIC=0x1D3D154F

CLANG="$(command -v clang-18 || command -v clang)"
CLANGXX="$(command -v clang++-18 || command -v clang++)"
NATIVE="$PS5VK/tooling/native"
[ -x "$SDK/bin/prospero-lld" ] || { echo "ps5-payload-sdk not found at $SDK (set PS5_PAYLOAD_SDK)" >&2; exit 1; }
[ -f "$NATIVE/sce_module_writer.cpp" ] || { echo "PS5_Vulkan tooling not found at $PS5VK (set PS5_VULKAN)" >&2; exit 1; }
rev="$(git -C "$PS5VK" rev-parse --short HEAD 2>/dev/null || echo unknown)"
[ "$rev" = "$PS5VK_REV" ] || echo "note: PS5_Vulkan is at $rev, this script was tested with $PS5VK_REV" >&2

TOOLS="$OUT/tools"
BUILD="$OUT/build"
APP="$OUT/$TITLE_ID"
mkdir -p "$TOOLS" "$BUILD"

echo "== packaging tools (PS5_Vulkan tooling/native + tools/ps5/ps5-native-tool-tls.patch)"
ZLIB="${F2_ZLIB:-$TOOLS/zlib}"
if [ ! -f "$ZLIB/lib/libz.a" ]; then
    ZLIB="$TOOLS/zlib"
    zv=1.3.2
    zsrc="$TOOLS/zlib-src"
    rm -rf "$zsrc" && mkdir -p "$zsrc"
    wget -q -O "$zsrc/zlib.tar.gz" "https://zlib.net/fossils/zlib-$zv.tar.gz"
    echo "bb329a0a2cd0274d05519d61c667c062e06990d72e125ee2dfa8de64f0119d16  $zsrc/zlib.tar.gz" | sha256sum -c --quiet
    tar -xzf "$zsrc/zlib.tar.gz" -C "$zsrc"
    (cd "$zsrc/zlib-$zv" && CC="$CLANG" ./configure --static --prefix="$ZLIB" >/dev/null && make -s -j8 >/dev/null && make -s install >/dev/null)
fi
SRC="$TOOLS/native-src"
NT="$TOOLS/ps5-native-tool"
stamp="$(cat "$NATIVE"/*.cpp "$NATIVE"/*.hpp "$ROOT/tools/ps5/ps5-native-tool-tls.patch" | sha256sum | cut -c1-16)"
if [ ! -x "$NT" ] || [ "$(cat "$TOOLS/.stamp" 2>/dev/null)" != "$stamp" ]; then
    rm -rf "$SRC" && mkdir -p "$SRC"
    cp "$NATIVE"/*.cpp "$NATIVE"/*.hpp "$SRC/"
    patch -s -d "$SRC" -p1 < "$ROOT/tools/ps5/ps5-native-tool-tls.patch"
    "$CLANGXX" -std=c++20 -O2 -Wall -Wextra -Werror -I "$ZLIB/include" \
        "$SRC/native_app_builder.cpp" "$SRC/self_container.cpp" "$SRC/elf_object.cpp" "$SRC/sce_module_writer.cpp" \
        "$ZLIB/lib/libz.a" -o "$NT"
    "$CLANGXX" -std=c++20 -O2 -Wall -Wextra -Werror "$SRC/libc_builder.cpp" -o "$TOOLS/libc-builder"
    "$TOOLS/libc-builder" "$NATIVE/runtime/api-surface.txt" "$NATIVE/runtime/imports.txt" "$TOOLS/libc.raw.elf" >/dev/null
    "$NT" self --sign --in "$TOOLS/libc.raw.elf" --out "$TOOLS/libc.prx" >/dev/null
    echo "$stamp" > "$TOOLS/.stamp"
fi
echo "$LIBC_PRX_SHA256  $TOOLS/libc.prx" | sha256sum -c --quiet

echo "== Rust staticlib (x86_64-unknown-freebsd)"
FEATURES=()
[ "$SMOKE" = 1 ] && FEATURES=(--features smoke)
# Zen 2 (the PS5 CPU; also the PS5 clang target's default). panic=abort: no unwinding
# across the system libraries.
CARGO_TARGET_X86_64_UNKNOWN_FREEBSD_RUSTFLAGS="-C target-cpu=znver2 -C panic=abort" \
    "$ROOT/tools/memcap.sh" cargo build --manifest-path "$ROOT/crates/frogger-ps5/Cargo.toml" --release \
    --target x86_64-unknown-freebsd --target-dir "$ROOT/target/ps5-cargo" "${FEATURES[@]}"
LIB="$ROOT/target/ps5-cargo/x86_64-unknown-freebsd/release/libfrogger_ps5.a"

echo "== link"
"$CLANG" --target=x86_64-pc-freebsd12-elf -fPIC -O2 -ffreestanding -fno-builtin -funwind-tables \
    -c "$ROOT/crates/frogger-ps5/csrc/crt.c" -o "$BUILD/crt.o"
echo '{ local: *; };' > "$BUILD/local.map"
L="$SDK/target/lib"
# Every symbol stays local: the converter only writes imports. The stubs are the
# payload SDK's (public symbol names of the console's modules; NIDs are derived from them).
# --wrap=_umtx_op: std's futex calls go through crates/frogger-ps5/src/compat.rs.
"$SDK/bin/prospero-lld" -T "$ROOT/crates/frogger-ps5/ps5-app.ld" --gc-sections --strip-debug \
    --version-script "$BUILD/local.map" -e _start -o "$BUILD/frogger2.pie" --wrap=_umtx_op \
    "$BUILD/crt.o" "$LIB" \
    "$L/libkernel.so" "$L/libSceLibcInternal.so" "$L/libSceVideoOut.so" "$L/libScePad.so" \
    "$L/libSceAudioOut.so" "$L/libSceUserService.so" "$L/libSceSystemService.so"
"$NT" link --in "$BUILD/frogger2.pie" --out "$BUILD/eboot.elf" --stub-dir "$L" \
    --module-sdk "$MODULE_SDK" --companion-sdk "$COMPANION_SDK" --file-name eboot.elf >/dev/null

echo "== app folder $APP"
rm -rf "$APP"
mkdir -p "$APP/sce_sys" "$APP/sce_module"
"$NT" self --sign --in "$BUILD/eboot.elf" --out "$APP/eboot.bin" --magic "$FSELF_MAGIC" >/dev/null
cp "$TOOLS/libc.prx" "$APP/sce_module/libc.prx"

if [ "$GPU" = 1 ]; then
    echo "== GPU build (wgpu on PS5_Vulkan's RADV; tools/ps5/VULKAN.md)"
    # The CPU build stays as the fallback the GPU build starts when Vulkan fails.
    mv "$APP/eboot.bin" "$APP/eboot_cpu.bin"
    SDKF="$PS5VK/.deps/native/ps5-payload-sdk"     # the driver author's SDK fork (v0.42 + libps5platform)
    ARCHIVE="${RADV_ARCHIVE:-$PS5VK/.deps/native/radv-release/lib/libvulkan_radeon.ps5.a}"
    LLD21_BIN="${F2_LLD21_BIN:-/usr/lib/llvm-21/bin}"
    for f in "$SDKF/target/lib/libps5platform.a" "$ARCHIVE" "$LLD21_BIN/ld.lld"; do
        [ -e "$f" ] || { echo "--gpu: missing $f (tools/ps5/build-vulkan-driver.sh; lld-21)" >&2; exit 1; }
    done
    CARGO_TARGET_X86_64_UNKNOWN_FREEBSD_RUSTFLAGS="-C target-cpu=znver2 -C panic=abort" \
        "$ROOT/tools/memcap.sh" cargo build --manifest-path "$ROOT/crates/frogger-ps5/Cargo.toml" --release \
        --target x86_64-unknown-freebsd --target-dir "$ROOT/target/ps5-cargo-gpu" --features gpu
    # std is built for the FreeBSD 12 stat/dirent layouts and our shims translate them; the
    # driver's C code uses the console's layouts (libkernel stat, the platform's readdir).
    # Rename our side of those symbols so each side keeps its own.
    printf '%s\n' stat fstat lstat opendir fdopendir readdir closedir dirfd clock_nanosleep arc4random_buf |
        awk '{print $1, "f2_" $1}' > "$BUILD/rename.syms"
    "$SDK/bin/llvm-objcopy" --redefine-syms="$BUILD/rename.syms" \
        "$ROOT/target/ps5-cargo-gpu/x86_64-unknown-freebsd/release/libfrogger_ps5.a" "$BUILD/libfrogger_ps5_gpu.a"
    # AGC host-link stubs (the winsys submits through the console's AGC driver functions).
    for s in libSceAgc:agc_canary_link_stub libSceAgcDriver:agc_driver_canary_link_stub; do
        PS5_PAYLOAD_SDK="$SDKF" sh "$PS5VK/tooling/prospero-clang18" -std=c11 -O2 -fPIC -c "$PS5VK/vendor/ps5/sdk/stubs/${s#*:}.c" -o "$BUILD/${s%%:*}.o"
        "$SDKF/bin/prospero-lld" --shared -soname "${s%%:*}.prx" -o "$BUILD/${s%%:*}.so" "$BUILD/${s%%:*}.o"
    done
    printf '#!/bin/sh\nif [ "$1" = "--bindir" ]; then echo %s; else exec llvm-config-18 "$@"; fi\n' "$LLD21_BIN" > "$TOOLS/llvm-config-lld21"
    chmod +x "$TOOLS/llvm-config-lld21"
    # shellcheck source=/dev/null
    source "$PS5VK/tools/radv-link.sh"
    radv_link_recipe "$PS5VK" "$SDKF" "$ARCHIVE"
    LF="$SDKF/target/lib"
    LLVM_CONFIG="$TOOLS/llvm-config-lld21" "$SDKF/bin/prospero-lld" -T "$ROOT/crates/frogger-ps5/ps5-app.ld" \
        --eh-frame-hdr --strip-debug -z nodynamic-undefined-weak "${radv_link_flags[@]}" --wrap=_umtx_op \
        --version-script "$BUILD/local.map" --exclude-libs=ALL -e _start -o "$BUILD/frogger2-gpu.pie" \
        "$BUILD/crt.o" "$BUILD/libSceAgc.so" "$BUILD/libSceAgcDriver.so" "$BUILD/libfrogger_ps5_gpu.a" \
        "${radv_link_inputs[@]}" \
        "$LF/libkernel.so" "$LF/libSceLibcInternal.so" "$LF/libSceVideoOut.so" "$LF/libScePad.so" \
        "$LF/libSceAudioOut.so" "$LF/libSceUserService.so" "$LF/libSceSystemService.so" \
        --as-needed "$LF"/*.so
    "$NT" link --in "$BUILD/frogger2-gpu.pie" --out "$BUILD/eboot-gpu.elf" --stub-dir "$LF" \
        --stub "$BUILD/libSceAgc.so" --stub "$BUILD/libSceAgcDriver.so" \
        --module-sdk "$MODULE_SDK" --companion-sdk "$COMPANION_SDK" --file-name eboot.elf >/dev/null
    "$NT" self --sign --in "$BUILD/eboot-gpu.elf" --out "$APP/eboot.bin" --magic "$FSELF_MAGIC" >/dev/null
fi
# attribute3 0x80040 declares high-frame-rate (120 Hz) support; without it VideoOut refuses
# the 120 Hz output mode (0x80290016), as PS5_Vulkan measured.
python3 -I - "$APP/sce_sys/param.json" "$TITLE" "$TITLE_ID" "$CONTENT_ID" "$VERSION" <<'PY'
import json, sys
out, title, title_id, content_id, version = sys.argv[1:]
param = {
    "ageLevel": {"default": 0},
    "applicationCategoryType": 0,
    "applicationDrmType": "free",
    "attribute": 0,
    "attribute2": 0,
    "attribute3": 0x80040,
    "conceptId": title_id[4:],
    "contentBadgeType": 1,
    "contentId": content_id,
    "contentVersion": version,
    "downloadDataSize": 64,
    "gameIntent": {"permittedIntents": [{"intentType": "launchActivity"}]},
    "localizedParameters": {"defaultLanguage": "en-US", "en-US": {"titleName": title}},
    "masterVersion": "01.00",
    "pubtools": {"creationDate": "2026-10-10 00:00:00", "loudnessSnd0": "-28.00", "toolVersion": "2.00"},
    "requiredSystemSoftwareVersion": "0x0000000000000000",
    "sdkVersion": "0x0000000000000000",
    "titleId": title_id,
    "versionFileUri": "",
}
with open(out, "w", encoding="utf-8") as f:
    json.dump(param, f, indent=2)
    f.write("\n")
PY

if [ -z "$ICON" ] && [ -n "$DATA" ]; then
    # The game's own title screen (exported from the user's data).
    cargo run --manifest-path "$ROOT/Cargo.toml" --release -q -p f2-export -- "$DATA" "$BUILD/export" --kind textures >/dev/null
    ICON="$BUILD/export/backdrops/title.png"
fi
if [ -n "$ICON" ]; then
    "$ROOT/tools/ps4/make-art.sh" "$ICON" "$BUILD/art"
    cp "$BUILD/art/icon0.png" "$APP/sce_sys/icon0.png"
else
    ffmpeg -loglevel error -y -f lavfi -i "color=c=0x2e8b3a:s=512x512" -frames:v 1 "$APP/sce_sys/icon0.png"
fi

if [ -n "$DATA" ]; then
    echo "== bundling game data from $DATA"
    mkdir -p "$APP/data"
    for d in game music video; do
        [ -d "$DATA/$d" ] && cp -r "$DATA/$d" "$APP/data/"
    done
    [ -f "$DATA/manifest.json" ] && cp "$DATA/manifest.json" "$APP/data/"
fi
"$NT" self --inspect --file "$APP/eboot.bin" | grep -E "integrity|container"

if [ "$FFPKG" = 1 ]; then
    UFS2="${F2_UFS2TOOL:-$(ls "$PS5VK"/.deps/UFS2Tool/.ufs2tool-run-linux "$PS5VK"/.deps/UFS2Tool/*/UFS2Tool* 2>/dev/null | head -1)}"
    [ -x "$UFS2" ] || { echo "--ffpkg: UFS2Tool not found (set F2_UFS2TOOL; PS5_Vulkan's tools/setup-packaging-dependencies.sh ffpkg builds it)" >&2; exit 1; }
    rm -f "$OUT/$TITLE_ID.ffpkg"
    "$UFS2" makefs -S 4096 -b 20% -t ffs \
        -o version=2,bsize=32768,fsize=4096,minfree=0,softupdates=0,optimization=space \
        "$OUT/$TITLE_ID.ffpkg" "$APP"
    echo "ffpkg: $OUT/$TITLE_ID.ffpkg"
fi
du -sh "$APP" | sed 's/^/app folder: /'
