#!/usr/bin/env bash
# Builds the native PS5 homebrew app of the Frogger 2 port: a directory title
# (eboot.bin + sce_sys/ + sce_module/libc.prx), and optionally the same content as an
# installable PS5 fake package (.pkg), a mountable .ffpkg (UFS2) or .ffpfsc (PFS) image.
#
#   tools/ps5/build.sh [--data RENDITION_DIR] [--icon IMAGE] [--out DIR]
#                      [--smoke | --hello] [--pkg] [--ffpkg] [--ffpfsc] [--gpu]
#
#   --data DIR   bundle your own imported game data (an f2-import rendition, e.g.
#                game-data/usa) at /app0/data. The result contains copyrighted data: keep it
#                for your own console, never share it. Without it the game looks in
#                /data/frogger2 or on USB (/mnt/usbN/frogger2) at run time.
#   --icon IMG   artwork for the launcher icon and backgrounds (centred square crop for the
#                icon; set F2_ICON_CROP_X to move it). Default: the game's title backdrop
#                exported from --data, else a plain green tile.
#   --out DIR    output directory (default target/ps5).
#   --smoke      build the Rust std smoke test instead of the game (title PPSA99921).
#   --hello      build a minimal C title packaged exactly like PS5_Vulkan's hardware-proven
#                ones (title PPSA99922; tools/ps5/hello/hello.c). Needs PS5_Vulkan's SDK fork.
#   --pkg        also build <CONTENT_ID>.pkg, a PS5 fake package (kstuff fPKG support), with
#                ultrapack's ffpfsc-pkg-tool (LibProsperoPkg; $F2_PS5_PKG_TOOL).
#   --ffpkg      also build <TITLE_ID>.ffpkg (UFS2 image, ShadowMountPlus; UFS2Tool:
#                $F2_UFS2TOOL or PS5_Vulkan's .deps/UFS2Tool build).
#   --ffpfsc     also build <TITLE_ID>.ffpfsc (PFS image, MkPFS: $F2_MKPFS, PS5 UltraPack's
#                vendored copy in $F2_ULTRAPACK (default ~/opt/ref/ps5-ultrapack) or PS5_Vulkan's).
#   --gpu        eboot.bin = the wgpu renderer on PS5_Vulkan's RADV driver (needs
#                tools/ps5/build-vulkan-driver.sh and lld-21; tools/ps5/VULKAN.md), with the
#                CPU build as eboot_cpu.bin, started instead when Vulkan does not come up.
#   --stage-only stop after the program files (eboot.bin, sce_module/libc.prx) and
#                sce_sys/param.json are in OUT/TITLE_ID and OUT/stage.env is written: no
#                artwork, data or images (no bc7enc / ffmpeg needed). Used by
#                tools/runtime/build-ps5.sh for the export runtime (docs/EXPORT.md).
#
# Needs (see tools/ps5/README.md): the ps5-payload-sdk ($PS5_PAYLOAD_SDK, default
# ~/opt/ps5-payload-sdk; its stubs, ld.lld wrapper), a PS5_Vulkan checkout for its native
# packaging tools ($PS5_VULKAN, default ~/opt/PS5_Vulkan), clang/clang++ 18, the nightly
# Rust toolchain with rust-src (std is rebuilt with emulated TLS), ffmpeg (artwork), the
# bc7enc_rdo encoder for the DDS artwork ($F2_BC7ENC, default ~/opt/ref/bc7enc_rdo/build/bc7enc
# or PS5_Vulkan's .deps build), python3. zlib for the packaging tool is fetched and built
# into the output's tools/ directory when $F2_ZLIB does not point at one.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SDK="${PS5_PAYLOAD_SDK:-$HOME/opt/ps5-payload-sdk}"
PS5VK="${PS5_VULKAN:-$HOME/opt/PS5_Vulkan}"
DATA=""
ICON=""
OUT="$ROOT/target/ps5"
KIND=game
PKG=0
FFPKG=0
FFPFSC=0
GPU=0
STAGE_ONLY=0
while [ $# -gt 0 ]; do
    case "$1" in
        --data) DATA="$(cd "$2" && pwd)"; shift 2 ;;
        --icon) ICON="$(realpath "$2")"; shift 2 ;;
        --out) OUT="$(realpath -m "$2")"; shift 2 ;;
        --smoke) KIND=smoke; shift ;;
        --hello) KIND=hello; shift ;;
        --pkg) PKG=1; shift ;;
        --ffpkg) FFPKG=1; shift ;;
        --ffpfsc) FFPFSC=1; shift ;;
        --gpu) GPU=1; shift ;;
        --stage-only) STAGE_ONLY=1; shift ;;
        -h|--help) sed -n '2,41p' "$0"; exit 0 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done

# Content IDs follow PS5_Vulkan's / UnleashedRecomp-Prospero's hardware-proven titles
# (UP9000 prefix; 16-character label).
case "$KIND" in
    game) TITLE="Frogger 2: Swampy's Revenge"; TITLE_ID="PPSA99920"; LABEL="FROGGER2RUSTPORT" ;;
    smoke) TITLE="Frogger 2 PS5 test 2: Rust smoke"; TITLE_ID="PPSA99921"; LABEL="FROGGER2SMOKETST" ;;
    hello) TITLE="Frogger 2 PS5 test 1: C hello"; TITLE_ID="PPSA99922"; LABEL="FROGGER2HELLOTST" ;;
esac
CONTENT_ID="UP9000-${TITLE_ID}_00-${LABEL}"
# Application version (param.json NN.NNN.NNN); bump for updates over an installed copy
# (01.001.000: output-mode settings; 01.002.000: audio as system user + desktop render fixes; each installs over the previous).
VERSION="${F2_PS5_VERSION:-01.004.000}"
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
SDKF="$PS5VK/.deps/native/ps5-payload-sdk"     # PS5_Vulkan's SDK fork (v0.42 + libps5platform)
[ -x "$SDK/bin/prospero-lld" ] || { echo "ps5-payload-sdk not found at $SDK (set PS5_PAYLOAD_SDK)" >&2; exit 1; }
[ -f "$NATIVE/sce_module_writer.cpp" ] || { echo "PS5_Vulkan tooling not found at $PS5VK (set PS5_VULKAN)" >&2; exit 1; }
rev="$(git -C "$PS5VK" rev-parse --short HEAD 2>/dev/null || echo unknown)"
[ "$rev" = "$PS5VK_REV" ] || echo "note: PS5_Vulkan is at $rev, this script was tested with $PS5VK_REV" >&2
BC7ENC="${F2_BC7ENC:-}"
for c in "$HOME/opt/ref/bc7enc_rdo/build/bc7enc" "$PS5VK/.deps/native/bc7enc_rdo/build/bc7enc"; do
    [ -z "$BC7ENC" ] && [ -x "$c" ] && BC7ENC="$c"
done
[ -x "$BC7ENC" ] || [ "$STAGE_ONLY" = 1 ] || { echo "bc7enc_rdo not found (set F2_BC7ENC; see tools/ps5/README.md)" >&2; exit 1; }

TOOLS="$OUT/tools"
BUILD="$OUT/build/$TITLE_ID"
APP="$OUT/$TITLE_ID"
mkdir -p "$TOOLS" "$BUILD"

echo "== packaging tools (PS5_Vulkan tooling/native, unmodified)"
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
NT="$TOOLS/ps5-native-tool"
stamp="unpatched-$(cat "$NATIVE"/*.cpp "$NATIVE"/*.hpp | sha256sum | cut -c1-16)"
if [ ! -x "$NT" ] || [ "$(cat "$TOOLS/.stamp" 2>/dev/null)" != "$stamp" ]; then
    "$CLANGXX" -std=c++20 -O2 -Wall -Wextra -Werror -I "$ZLIB/include" \
        "$NATIVE/native_app_builder.cpp" "$NATIVE/self_container.cpp" "$NATIVE/elf_object.cpp" "$NATIVE/sce_module_writer.cpp" \
        "$ZLIB/lib/libz.a" -o "$NT"
    "$CLANGXX" -std=c++20 -O2 -Wall -Wextra -Werror "$NATIVE/libc_builder.cpp" -o "$TOOLS/libc-builder"
    "$TOOLS/libc-builder" "$NATIVE/runtime/api-surface.txt" "$NATIVE/runtime/imports.txt" "$TOOLS/libc.raw.elf" >/dev/null
    "$NT" self --sign --in "$TOOLS/libc.raw.elf" --out "$TOOLS/libc.prx" >/dev/null
    echo "$stamp" > "$TOOLS/.stamp"
fi
echo "$LIBC_PRX_SHA256  $TOOLS/libc.prx" | sha256sum -c --quiet

# The intermediate PIE must have no TLS data: everything is built with emulated TLS (as
# every native PS5 homebrew title is), so the converter writes its empty PT_TLS record.
check_no_tls() {
    if readelf -lW "$1" | grep -q ' TLS '; then
        echo "error: $1 has a PT_TLS segment (native TLS); everything must use emulated TLS" >&2
        exit 1
    fi
}
# The converted executable's PT_TLS (type 7) must be the empty record (memsz 0).
check_empty_tls() {
    python3 -I - "$1" <<'PY'
import struct, sys
d = open(sys.argv[1], "rb").read()
phoff, = struct.unpack_from("<Q", d, 0x20)
phentsize, phnum = struct.unpack_from("<HH", d, 0x36)
for i in range(phnum):
    t, _f, _off, _va, _pa, filesz, memsz, _al = struct.unpack_from("<IIQQQQQQ", d, phoff + i * phentsize)
    if t == 7 and (filesz or memsz):
        raise SystemExit(f"error: {sys.argv[1]}: PT_TLS has filesz {filesz:#x} memsz {memsz:#x}; expected the empty record")
print(f"{sys.argv[1]}: no TLS image (empty PT_TLS record), as PS5_Vulkan's titles")
PY
}

L="$SDK/target/lib"
if [ "$KIND" = hello ]; then
    echo "== C hello (PS5_Vulkan boilerplate: app_crt.cpp, PS5 Clang target, SDK fork, ps5-pie.ld)"
    [ -f "$SDKF/target/lib/libps5platform.a" ] || { echo "--hello needs PS5_Vulkan's SDK fork at $SDKF" >&2; exit 1; }
    PCC() { PS5_PAYLOAD_SDK="$SDKF" sh "$PS5VK/tooling/prospero-clang18" "$@"; }
    PCC -std=c11 -O2 -Wall -Wextra -ffunction-sections -fdata-sections -c "$ROOT/tools/ps5/hello/hello.c" -o "$BUILD/hello.o"
    PCC -std=c++20 -O2 -Wall -Wextra -fno-exceptions -fno-rtti -ffunction-sections -fdata-sections \
        -c "$NATIVE/app_crt.cpp" -o "$BUILD/app_crt.o"
    PCC -std=c++20 -O2 -Wall -Wextra -fno-exceptions -fno-rtti -ffunction-sections -fdata-sections \
        -c "$NATIVE/app_cpp_runtime.cpp" -o "$BUILD/app_cpp_runtime.o"
    "$SDKF/bin/prospero-lld" -T "$NATIVE/ps5-pie.ld" --eh-frame-hdr --version-script "$NATIVE/app-symbols.map" \
        --exclude-libs=ALL -e _start -o "$BUILD/app.pie" \
        "$BUILD/app_crt.o" "$BUILD/app_cpp_runtime.o" "$BUILD/hello.o" "$SDKF/target/lib/libps5platform.a" \
        --as-needed "$SDKF"/target/lib/*.so
    check_no_tls "$BUILD/app.pie"
    "$NT" link --in "$BUILD/app.pie" --out "$BUILD/eboot.elf" --stub-dir "$SDKF/target/lib" \
        --module-sdk "$MODULE_SDK" --companion-sdk "$COMPANION_SDK" --file-name eboot.elf >/dev/null
else
    echo "== Rust staticlib (custom target: x86_64-unknown-freebsd + emulated TLS, std rebuilt)"
    FEATURES=()
    [ "$KIND" = smoke ] && FEATURES=(--features smoke)
    # Zen 2 (the PS5 CPU), panic=abort, emulated TLS: tools/ps4/console-target.py.
    SPEC="$ROOT/target/ps5-cargo/x86_64-ps5-freebsd.json"
    mkdir -p "$(dirname "$SPEC")"
    rustc +nightly -Z unstable-options --print target-spec-json --target x86_64-unknown-freebsd |
        python3 -I "$ROOT/tools/ps4/console-target.py" znver2 "$SPEC.new"
    cmp -s "$SPEC.new" "$SPEC" && rm "$SPEC.new" || mv "$SPEC.new" "$SPEC"
    CARGO_CONSOLE=(cargo +nightly build -Zbuild-std=std,panic_abort -Zjson-target-spec --release --target "$SPEC")
    "$ROOT/tools/memcap.sh" "${CARGO_CONSOLE[@]}" --manifest-path "$ROOT/crates/frogger-ps5/Cargo.toml" \
        --target-dir "$ROOT/target/ps5-cargo" "${FEATURES[@]}"
    LIB="$ROOT/target/ps5-cargo/x86_64-ps5-freebsd/release/libfrogger_ps5.a"

    echo "== link"
    CFLAGS=(--target=x86_64-pc-freebsd12-elf -fPIC -O2 -ffreestanding -fno-builtin -funwind-tables)
    "$CLANG" "${CFLAGS[@]}" -c "$ROOT/crates/frogger-ps5/csrc/crt.c" -o "$BUILD/crt.o"
    "$CLANG" "${CFLAGS[@]}" -c "$ROOT/crates/console-host/csrc/emutls.c" -o "$BUILD/emutls.o"
    echo '{ local: *; };' > "$BUILD/local.map"
    # Every symbol stays local: the converter only writes imports. The stubs are the
    # payload SDK's (public symbol names of the console's modules; NIDs are derived from them).
    # --wrap=_umtx_op: std's futex calls go through crates/frogger-ps5/src/compat.rs.
    "$SDK/bin/prospero-lld" -T "$ROOT/crates/frogger-ps5/ps5-app.ld" --gc-sections --strip-debug \
        --version-script "$BUILD/local.map" -e _start -o "$BUILD/frogger2.pie" --wrap=_umtx_op \
        "$BUILD/crt.o" "$BUILD/emutls.o" "$LIB" \
        "$L/libkernel.so" "$L/libSceLibcInternal.so" "$L/libSceVideoOut.so" "$L/libScePad.so" \
        "$L/libSceAudioOut.so" "$L/libSceUserService.so" "$L/libSceSystemService.so"
    check_no_tls "$BUILD/frogger2.pie"
    "$NT" link --in "$BUILD/frogger2.pie" --out "$BUILD/eboot.elf" --stub-dir "$L" \
        --module-sdk "$MODULE_SDK" --companion-sdk "$COMPANION_SDK" --file-name eboot.elf >/dev/null
fi
check_empty_tls "$BUILD/eboot.elf"

echo "== app folder $APP"
rm -rf "$APP"
mkdir -p "$APP/sce_sys" "$APP/sce_module"
"$NT" self --sign --in "$BUILD/eboot.elf" --out "$APP/eboot.bin" --magic "$FSELF_MAGIC" >/dev/null
cp "$TOOLS/libc.prx" "$APP/sce_module/libc.prx"

if [ "$GPU" = 1 ]; then
    [ "$KIND" = game ] || { echo "--gpu builds the game only" >&2; exit 2; }
    echo "== GPU build (wgpu on PS5_Vulkan's RADV; tools/ps5/VULKAN.md)"
    # The CPU build stays as the fallback the GPU build starts when Vulkan fails.
    mv "$APP/eboot.bin" "$APP/eboot_cpu.bin"
    ARCHIVE="${RADV_ARCHIVE:-$PS5VK/.deps/native/radv-release/lib/libvulkan_radeon.ps5.a}"
    LLD21_BIN="${F2_LLD21_BIN:-/usr/lib/llvm-21/bin}"
    for f in "$SDKF/target/lib/libps5platform.a" "$ARCHIVE" "$LLD21_BIN/ld.lld"; do
        [ -e "$f" ] || { echo "--gpu: missing $f (tools/ps5/build-vulkan-driver.sh; lld-21)" >&2; exit 1; }
    done
    "$ROOT/tools/memcap.sh" "${CARGO_CONSOLE[@]}" --manifest-path "$ROOT/crates/frogger-ps5/Cargo.toml" \
        --target-dir "$ROOT/target/ps5-cargo-gpu" --features gpu
    # std is built for the FreeBSD 12 stat/dirent layouts and our shims translate them; the
    # driver's C code uses the console's layouts (libkernel stat, the platform's readdir).
    # Rename our side of those symbols so each side keeps its own.
    printf '%s\n' stat fstat lstat opendir fdopendir readdir closedir dirfd clock_nanosleep arc4random_buf |
        awk '{print $1, "f2_" $1}' > "$BUILD/rename.syms"
    "$SDK/bin/llvm-objcopy" --redefine-syms="$BUILD/rename.syms" \
        "$ROOT/target/ps5-cargo-gpu/x86_64-ps5-freebsd/release/libfrogger_ps5.a" "$BUILD/libfrogger_ps5_gpu.a"
    # AGC host-link stubs (the winsys submits through the console's AGC driver functions).
    for s in libSceAgc:agc_canary_link_stub libSceAgcDriver:agc_driver_canary_link_stub; do
        PS5_PAYLOAD_SDK="$SDKF" sh "$PS5VK/tooling/prospero-clang18" -std=c11 -O2 -fPIC -c "$PS5VK/vendor/ps5/sdk/stubs/${s#*:}.c" -o "$BUILD/${s%%:*}.o"
        "$SDKF/bin/prospero-lld" --shared -soname "${s%%:*}.prx" -o "$BUILD/${s%%:*}.so" "$BUILD/${s%%:*}.o"
    done
    printf '#!/bin/sh\nif [ "$1" = "--bindir" ]; then echo %s; else exec llvm-config-18 "$@"; fi\n' "$LLD21_BIN" > "$TOOLS/llvm-config-lld21"
    chmod +x "$TOOLS/llvm-config-lld21"
    # shellcheck source=/dev/null
    source "$PS5VK/tools/radv-link.sh"
    # The recipe brings its own __emutls_get_address (ps5-emutls-cxa.o, same algorithm and
    # destructor ordering as crates/console-host/csrc/emutls.c), so emutls.o is left out.
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
    check_no_tls "$BUILD/frogger2-gpu.pie"
    "$NT" link --in "$BUILD/frogger2-gpu.pie" --out "$BUILD/eboot-gpu.elf" --stub-dir "$LF" \
        --stub "$BUILD/libSceAgc.so" --stub "$BUILD/libSceAgcDriver.so" \
        --module-sdk "$MODULE_SDK" --companion-sdk "$COMPANION_SDK" --file-name eboot.elf >/dev/null
    check_empty_tls "$BUILD/eboot-gpu.elf"
    "$NT" self --sign --in "$BUILD/eboot-gpu.elf" --out "$APP/eboot.bin" --magic "$FSELF_MAGIC" >/dev/null
fi

# param.json: field for field the layout of PS5_Vulkan's and UnleashedRecomp-Prospero's
# titles. attribute3 0x80040 declares high-frame-rate (120 Hz) support (Unleashed's value);
# without it VideoOut refuses the 120 Hz output mode (0x80290016), as PS5_Vulkan measured.
ATTR3=0
[ "$KIND" = game ] && ATTR3=$((0x80040))
python3 -I - "$APP/sce_sys/param.json" "$TITLE" "$TITLE_ID" "$CONTENT_ID" "$VERSION" "$ATTR3" <<'PY'
import json, sys
out, title, title_id, content_id, version, attr3 = sys.argv[1:]
param = {
    "ageLevel": {"default": 0},
    "applicationCategoryType": 0,
    "applicationDrmType": "free",
    "attribute": 0,
    "attribute2": 0,
    "attribute3": int(attr3),
    "conceptId": title_id[4:],
    "contentBadgeType": 1,
    "contentId": content_id,
    "contentVersion": version,
    "downloadDataSize": 256,
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
if [ "$STAGE_ONLY" = 1 ]; then
    # What the export packer needs besides the files (crates/f2-package/src/ps5.rs).
    printf '%s\n' "TITLE_ID=$TITLE_ID" "CONTENT_ID=$CONTENT_ID" "CONTENT_VERSION=$VERSION" "KIND=$KIND" "GPU=$GPU" > "$OUT/stage.env"
    echo "staged: $APP ($OUT/stage.env)"
    exit 0
fi

echo "== artwork (icon0.png + icon0.dds 512x512, pic0/pic1 .dds + .png 3840x2160)"
if [ -z "$ICON" ] && [ -n "$DATA" ] && [ "$KIND" = game ]; then
    # The game's own title screen (exported from the user's data).
    cargo run --manifest-path "$ROOT/Cargo.toml" --release -q -p f2-export -- "$DATA" "$BUILD/export" --kind textures >/dev/null
    ICON="$BUILD/export/backdrops/title.png"
fi
ART="$BUILD/art"
mkdir -p "$ART"
if [ -n "$ICON" ]; then
    "$ROOT/tools/ps4/make-art.sh" "$ICON" "$ART"
else
    # Plain tiles: green (game), blue (smoke), grey (hello), so the ladder's titles differ.
    case "$KIND" in game) c=0x2e8b3a ;; smoke) c=0x2e5a8b ;; *) c=0x707070 ;; esac
    ffmpeg -loglevel error -y -f lavfi -i "color=c=$c:s=512x512" -frames:v 1 -pix_fmt rgb24 "$ART/icon0.png"
    ffmpeg -loglevel error -y -f lavfi -i "color=c=$c:s=1920x1080" -frames:v 1 -pix_fmt rgb24 "$ART/pic1.png"
fi
cp "$ART/icon0.png" "$APP/sce_sys/icon0.png"
# BC7 DX10 DDS as PS5_Vulkan's tools/prepare-assets.sh / Unleashed's png_to_bc7_dds write
# them (single level, BC7_UNORM, mip count 1, straight alpha), the form the shell accepts;
# icon0.dds is what LibProsperoPkg / ultrapack generate for packages too.
dds() { # SRC.png W H OUT.dds
    ffmpeg -loglevel error -y -i "$1" -vf "scale=$2:$3:flags=lanczos" -frames:v 1 -pix_fmt rgba "$ART/dds-src.png"
    "$BC7ENC" -q -g "$ART/dds-src.png" "$4" >/dev/null
    python3 -I - "$4" "$2" "$3" <<'PY'
import struct, sys
path, w, h = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
d = bytearray(open(path, "rb").read())
assert d[:4] == b"DDS " and d[84:88] == b"DX10", "not a DX10 DDS"
assert struct.unpack_from("<II", d, 12) == (h, w), "wrong size"
assert struct.unpack_from("<I", d, 128)[0] == 98 and len(d) == 148 + (w // 4) * (h // 4) * 16, "not single-level BC7_UNORM"
struct.pack_into("<I", d, 8, struct.unpack_from("<I", d, 8)[0] | 0x20000)
struct.pack_into("<I", d, 24, 1)
struct.pack_into("<I", d, 28, 1)
struct.pack_into("<I", d, 144, 1)
open(path, "wb").write(d)
PY
}
dds "$ART/icon0.png" 512 512 "$APP/sce_sys/icon0.dds"
dds "$ART/pic1.png" 3840 2160 "$APP/sce_sys/pic0.dds"
cp "$APP/sce_sys/pic0.dds" "$APP/sce_sys/pic1.dds"
# The PNG forms beside them (8-bit RGB, as LibProsperoPkg regenerates them when missing).
ffmpeg -loglevel error -y -i "$ART/pic1.png" -vf "scale=3840:2160:flags=lanczos" -frames:v 1 -pix_fmt rgb24 "$APP/sce_sys/pic0.png"
cp "$APP/sce_sys/pic0.png" "$APP/sce_sys/pic1.png"

if [ -n "$DATA" ] && [ "$KIND" = game ]; then
    echo "== bundling game data from $DATA"
    mkdir -p "$APP/data"
    for d in game music video; do
        [ -d "$DATA/$d" ] && cp -r "$DATA/$d" "$APP/data/"
    done
    [ -f "$DATA/manifest.json" ] && cp "$DATA/manifest.json" "$APP/data/"
fi
"$NT" self --inspect --file "$APP/eboot.bin" | grep -E "integrity|container"

if [ "$PKG" = 1 ]; then
    echo "== PS5 fake package (ffpfsc-pkg-tool / LibProsperoPkg)"
    PT="${F2_PS5_PKG_TOOL:-$HOME/opt/ffpfsc-pkg-tool/ffpfsc-pkg-tool}"
    [ -x "$PT" ] || { echo "--pkg: ffpfsc-pkg-tool not found at $PT (set F2_PS5_PKG_TOOL; tools/ps5/README.md)" >&2; exit 1; }
    # The package path follows LibProsperoPkg's hardware-tested HomebrewTest: raw ELF
    # modules in, fake-signed by the package builder itself.
    STAGE="$BUILD/pkg-src"
    rm -rf "$STAGE" && mkdir -p "$STAGE"
    cp -r "$APP/." "$STAGE/"
    rm -f "$STAGE/eboot_cpu.bin"
    if [ "$GPU" = 1 ]; then
        cp "$BUILD/eboot-gpu.elf" "$STAGE/eboot.bin"
        cp "$BUILD/eboot.elf" "$STAGE/eboot_cpu.bin"
    else
        cp "$BUILD/eboot.elf" "$STAGE/eboot.bin"
    fi
    cp "$TOOLS/libc.raw.elf" "$STAGE/sce_module/libc.prx"
    rm -f "$OUT/$CONTENT_ID.pkg"
    mkdir -p "$BUILD/pkg-tmp" "$BUILD/pkg-out"
    "$PT" build "$STAGE" "$BUILD/pkg-out" --content-id "$CONTENT_ID" --title-id "$TITLE_ID" --title "$TITLE" \
        --version "$VERSION" --deterministic --temp "$BUILD/pkg-tmp" | grep -v '^\[####\]' || true
    built="$(ls -t "$BUILD/pkg-out"/*.pkg 2>/dev/null | head -1)"
    [ -f "$built" ] || { echo "--pkg: no package was written" >&2; exit 1; }
    mv "$built" "$OUT/$CONTENT_ID.pkg"
    rm -rf "$STAGE" "$BUILD/pkg-tmp" "$BUILD/pkg-out"
    "$PT" validate "$OUT/$CONTENT_ID.pkg" --temp "$BUILD" | tee "$OUT/$CONTENT_ID.pkg.validate.txt" | grep -E '^\s*\[(FAIL|WARN)\]|^summary' || true
    echo "pkg: $OUT/$CONTENT_ID.pkg"
fi
if [ "$FFPKG" = 1 ]; then
    UFS2="${F2_UFS2TOOL:-$(ls "$PS5VK"/.deps/UFS2Tool/.ufs2tool-run-linux 2>/dev/null | head -1)}"
    [ -x "$UFS2" ] || { echo "--ffpkg: UFS2Tool not found (set F2_UFS2TOOL; PS5_Vulkan's tools/setup-packaging-dependencies.sh ffpkg builds it)" >&2; exit 1; }
    rm -f "$OUT/$TITLE_ID.ffpkg"
    "$UFS2" makefs -S 4096 -b 20% -t ffs \
        -o version=2,bsize=32768,fsize=4096,minfree=0,softupdates=0,optimization=space \
        "$OUT/$TITLE_ID.ffpkg" "$APP" >/dev/null
    echo "ffpkg: $OUT/$TITLE_ID.ffpkg"
fi
if [ "$FFPFSC" = 1 ]; then
    # MkPFS: $F2_MKPFS, else the copy PS5 UltraPack vendors (backend/mkpfs), else PS5_Vulkan's.
    MKPFS="${F2_MKPFS:-}"
    UP="${F2_ULTRAPACK:-$HOME/opt/ref/ps5-ultrapack}"
    if [ -z "$MKPFS" ] && [ -f "$UP/backend/mkpfs/__main__.py" ]; then
        MKPFS="$TOOLS/mkpfs"
        printf '#!/bin/sh\ncd "%s/backend" && exec python3 -m mkpfs "$@"\n' "$UP" > "$MKPFS"
        chmod +x "$MKPFS"
    fi
    [ -n "$MKPFS" ] || MKPFS="$(ls "$PS5VK"/.deps/MkPFS/.mkpfs-run-linux 2>/dev/null | head -1)"
    [ -x "$MKPFS" ] || { echo "--ffpfsc: MkPFS not found (set F2_MKPFS or F2_ULTRAPACK)" >&2; exit 1; }
    rm -f "$OUT/$TITLE_ID.ffpfsc"
    # 64 KiB PFS blocks: the size the console reads (PS5 UltraPack: 4 KiB images crash on launch).
    "$MKPFS" pack folder --no-adjust-output-file-extension --version PS5 --block-size 65536 --verify \
        "$APP" "$OUT/$TITLE_ID.ffpfsc" 2>&1 | tr "\r" "\n" | grep -v "%" | tail -3
    echo "ffpfsc: $OUT/$TITLE_ID.ffpfsc"
fi
du -sh "$APP" | sed 's/^/app folder: /'
