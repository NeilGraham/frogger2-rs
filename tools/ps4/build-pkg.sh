#!/usr/bin/env bash
# Builds the PS4 homebrew package (fPKG) of the Frogger 2 port. It installs on a
# jailbroken PS4 (GoldHEN) or a jailbroken PS5 (etaHEN / kstuff, PS4 fPKG support).
#
#   tools/ps4/build-pkg.sh [--data RENDITION_DIR] [--icon IMAGE] [--out DIR]
#                          [--enhanced | --smoke]
#
#   --data DIR   bundle your own imported game data (an f2-import rendition, e.g.
#                game-data/usa) into the package at /app0/data. The resulting .pkg
#                contains copyrighted data: keep it for your own console, never share it.
#                Without it the game looks for data in /data/frogger2 or on USB
#                (/mnt/usbN/frogger2) at run time.
#   --icon IMG   artwork for the home-screen icon (512x512 icon0.png, cropped square) and
#                background (1920x1080 pic1.png). Default: the game's own title backdrop
#                exported from --data, else the OpenOrbis placeholder.
#   --out DIR    output directory (default target/ps4).
#   --enhanced   the "enhanced" variant (title FROG20002): param.sfo ATTRIBUTE with PS4 Pro
#                mode + 7-CPU mode + Cross-is-enter, and the auth info that lets the program
#                load the system's Piglet (OpenGL ES) module. Default: the conservative
#                variant (title FROG20000): ATTRIBUTE 0 and create-fself's default auth info,
#                as the first package. Both use the CPU renderer unless renderer.txt says
#                "piglet" (README). Override with F2_PS4_ATTRIBUTE=<int> and
#                F2_PS4_AUTHINFO=default|piglet|<hex>.
#   --smoke      a minimal Rust smoke test instead of the game (title FROG20001, conservative
#                settings): klog lines, std checks, a pulsing green (pass) or red screen.
#
# Needs: the OpenOrbis PS4 toolchain ($OO_PS4_TOOLCHAIN, default ~/opt/openorbis/OpenOrbis/
# PS4Toolchain), clang + ld.lld (LLVM 18), the nightly Rust toolchain with rust-src (std is
# rebuilt with emulated TLS), ffmpeg (artwork). See tools/ps4/README.md.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
export OO_PS4_TOOLCHAIN="${OO_PS4_TOOLCHAIN:-$HOME/opt/openorbis/OpenOrbis/PS4Toolchain}"
TC="$OO_PS4_TOOLCHAIN"
DATA=""
ICON=""
OUT="$ROOT/target/ps4"
VARIANT=conservative
while [ $# -gt 0 ]; do
    case "$1" in
        --data) DATA="$(cd "$2" && pwd)"; shift 2 ;;
        --icon) ICON="$(realpath "$2")"; shift 2 ;;
        --out) OUT="$(realpath -m "$2")"; shift 2 ;;
        --enhanced) VARIANT=enhanced; shift ;;
        --smoke) VARIANT=smoke; shift ;;
        -h|--help) sed -n '2,33p' "$0"; exit 0 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done

# ATTRIBUTE bits: 23 = supports PS4 Pro mode (4K output, higher clocks on a Pro; a PS5 runs
# the title in Pro mode), 16 = 7-CPU mode, 1 = Cross is the enter button.
ENHANCED_ATTRIBUTE=$(( (1 << 23) | (1 << 16) | (1 << 1) ))
# Capabilities (auth info) the OpenOrbis Piglet sample and RetroArch's PS4 build use, which
# let the program load and use the system's Piglet (OpenGL ES) library.
PIGLET_AUTHINFO="000000000000000000000000001C004000FF000000000080000000000000000000000000000000000000008000400040000000000000008000000000000000080040FFFF000000F000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"
FEATURES=()
case "$VARIANT" in
    conservative) TITLE="Frogger 2: Swampy's Revenge"; TITLE_ID="FROG20000"; LABEL="FROGGER2RUSTPORT"
        ATTRIBUTE=0; AUTH=default ;;
    enhanced) TITLE="Frogger 2: Swampy's Revenge (Pro)"; TITLE_ID="FROG20002"; LABEL="FROGGER2RUSTPRO0"
        ATTRIBUTE=$ENHANCED_ATTRIBUTE; AUTH=piglet ;;
    smoke) TITLE="Frogger 2 PS4 test: Rust smoke"; TITLE_ID="FROG20001"; LABEL="FROGGER2SMOKETST"
        ATTRIBUTE=0; AUTH=default; FEATURES=(--features smoke) ;;
esac
ATTRIBUTE="${F2_PS4_ATTRIBUTE:-$ATTRIBUTE}"
AUTH="${F2_PS4_AUTHINFO:-$AUTH}"
# Bisect builds (tools/ps4/build-pro-bisect.sh): their own title ID (9 chars), content-ID
# label (16 chars) and title so they install beside the others. F2_PS4_FB_1080=1 (read by
# the Rust build) keeps the framebuffers at 1920x1080 in Pro mode.
TITLE_ID="${F2_PS4_TITLE_ID:-$TITLE_ID}"
LABEL="${F2_PS4_LABEL:-$LABEL}"
TITLE="${F2_PS4_TITLE:-$TITLE}"
[ ${#TITLE_ID} -eq 9 ] && [ ${#LABEL} -eq 16 ] || { echo "title ID must be 9 and label 16 characters" >&2; exit 2; }
CONTENT_ID="IV0000-${TITLE_ID}_00-${LABEL}"
# Package version (param.sfo "XX.YY"); bump for updates over an installed copy.
VERSION="${F2_PS4_VERSION:-01.02}"

CLANG="$(command -v clang-18 || command -v clang)"
LLD="$(command -v ld.lld-18 || command -v ld.lld)"
[ -d "$TC/lib" ] || { echo "OpenOrbis toolchain not found at $TC (set OO_PS4_TOOLCHAIN)" >&2; exit 1; }
BIN="$TC/bin/linux"
# PkgTool.Core is .NET Core 3.1: run without ICU, and it needs OpenSSL 1.1 (not in
# Ubuntu 24.04+): point F2_LIBSSL11 at an extracted libssl1.1 package's lib dir.
F2_LIBSSL11="${F2_LIBSSL11:-$HOME/opt/libssl11/usr/lib/x86_64-linux-gnu}"
PKGTOOL() { LD_LIBRARY_PATH="$F2_LIBSSL11${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" DOTNET_SYSTEM_GLOBALIZATION_INVARIANT=1 "$BIN/PkgTool.Core" "$@"; }

BUILD="$OUT/build"
STAGE="$OUT/stage"
rm -rf "$STAGE"
mkdir -p "$BUILD" "$STAGE/sce_sys/about" "$STAGE/sce_module"

echo "== Rust staticlib (custom target: x86_64-unknown-freebsd + emulated TLS, std rebuilt)"
# Jaguar (PS4) feature level (PS5's Zen 2 runs it too), panic=abort, emulated TLS: no TLS
# segment, so nothing depends on the loader's static TLS layout (tools/ps4/console-target.py).
SPEC="$ROOT/target/ps4-cargo/x86_64-ps4-freebsd.json"
mkdir -p "$(dirname "$SPEC")"
rustc +nightly -Z unstable-options --print target-spec-json --target x86_64-unknown-freebsd |
    python3 -I "$ROOT/tools/ps4/console-target.py" btver2 "$SPEC.new"
cmp -s "$SPEC.new" "$SPEC" && rm "$SPEC.new" || mv "$SPEC.new" "$SPEC"
"$ROOT/tools/memcap.sh" cargo +nightly build -Zbuild-std=std,panic_abort -Zjson-target-spec --release --target "$SPEC" \
    --manifest-path "$ROOT/crates/frogger-ps4/Cargo.toml" --target-dir "$ROOT/target/ps4-cargo" "${FEATURES[@]}"
LIB="$ROOT/target/ps4-cargo/x86_64-ps4-freebsd/release/libfrogger_ps4.a"

echo "== link"
"$CLANG" --target=x86_64-pc-freebsd12-elf -fPIC -O2 -ffreestanding -fno-builtin -c "$ROOT/crates/frogger-ps4/csrc/crt_shim.c" -o "$BUILD/crt_shim.o"
"$CLANG" --target=x86_64-pc-freebsd12-elf -fPIC -O2 -ffreestanding -fno-builtin -c "$ROOT/crates/console-host/csrc/emutls.c" -o "$BUILD/emutls.o"
"$LLD" -m elf_x86_64 -pie --script "$ROOT/crates/frogger-ps4/link.x" --eh-frame-hdr --gc-sections --strip-debug -L"$TC/lib" \
    "$TC/lib/crt1.o" "$BUILD/crt_shim.o" "$BUILD/emutls.o" "$LIB" \
    -lkernel -lSceLibcInternal -lSceVideoOut -lScePad -lSceAudioOut -lSceUserService -lSceSysmodule -lSceSystemService \
    -o "$BUILD/frogger2.elf"
if readelf -lW "$BUILD/frogger2.elf" | grep -q ' TLS '; then
    echo "error: frogger2.elf has a PT_TLS segment; everything must use emulated TLS" >&2
    exit 1
fi
case "$AUTH" in
    default) AUTH_ARGS=() ;;
    piglet) AUTH_ARGS=(--authinfo "$PIGLET_AUTHINFO") ;;
    *) AUTH_ARGS=(--authinfo "$AUTH") ;;
esac
echo "variant $VARIANT: title $TITLE_ID, ATTRIBUTE $ATTRIBUTE, framebuffers $([ -n "${F2_PS4_FB_1080:-}" ] && echo "1920x1080 forced" || echo "output mode"), auth info $([ "$AUTH" = default ] && echo "create-fself default" || echo "$AUTH")"
"$BIN/create-fself" -in="$BUILD/frogger2.elf" -out="$BUILD/frogger2.oelf" --eboot "$STAGE/eboot.bin" --paid 0x3800000000000011 "${AUTH_ARGS[@]}"

echo "== sce_sys"
cp "$TC/src/modules/right.sprx" "$STAGE/sce_sys/about/right.sprx"
cp "$TC/src/modules/libc.prx" "$TC/src/modules/libSceFios2.prx" "$STAGE/sce_module/"
SFO="$STAGE/sce_sys/param.sfo"
PKGTOOL sfo_new "$SFO"
PKGTOOL sfo_setentry "$SFO" APP_TYPE --type Integer --maxsize 4 --value 1
PKGTOOL sfo_setentry "$SFO" APP_VER --type Utf8 --maxsize 8 --value "$VERSION"
PKGTOOL sfo_setentry "$SFO" ATTRIBUTE --type Integer --maxsize 4 --value "$ATTRIBUTE"
PKGTOOL sfo_setentry "$SFO" CATEGORY --type Utf8 --maxsize 4 --value gd
PKGTOOL sfo_setentry "$SFO" CONTENT_ID --type Utf8 --maxsize 48 --value "$CONTENT_ID"
# MiB of /download0 (app-private writable storage): the save fallback when /data is not writable.
PKGTOOL sfo_setentry "$SFO" DOWNLOAD_DATA_SIZE --type Integer --maxsize 4 --value 64
PKGTOOL sfo_setentry "$SFO" SYSTEM_VER --type Integer --maxsize 4 --value 0
PKGTOOL sfo_setentry "$SFO" TITLE --type Utf8 --maxsize 128 --value "$TITLE"
PKGTOOL sfo_setentry "$SFO" TITLE_ID --type Utf8 --maxsize 12 --value "$TITLE_ID"
PKGTOOL sfo_setentry "$SFO" VERSION --type Utf8 --maxsize 8 --value "$VERSION"

if [ -z "$ICON" ] && [ -n "$DATA" ] && [ "$VARIANT" != smoke ]; then
    # The game's own title screen (exported from the user's data).
    cargo run --manifest-path "$ROOT/Cargo.toml" --release -q -p f2-export -- "$DATA" "$BUILD/export" --kind textures >/dev/null
    ICON="$BUILD/export/backdrops/title.png"
fi
if [ -n "$ICON" ]; then
    "$ROOT/tools/ps4/make-art.sh" "$ICON" "$STAGE/sce_sys"
else
    cp "$TC/samples/hello_world/sce_sys/icon0.png" "$STAGE/sce_sys/icon0.png"
fi

FILES=(eboot.bin sce_sys/about/right.sprx sce_sys/param.sfo sce_module/libc.prx sce_module/libSceFios2.prx)
for f in icon0.png pic0.png pic1.png; do
    [ -f "$STAGE/sce_sys/$f" ] && FILES+=("sce_sys/$f")
done
if [ -n "$DATA" ] && [ "$VARIANT" != smoke ]; then
    echo "== bundling game data from $DATA"
    mkdir -p "$STAGE/data"
    for d in game music video; do
        [ -d "$DATA/$d" ] && cp -r "$DATA/$d" "$STAGE/data/"
    done
    [ -f "$DATA/manifest.json" ] && cp "$DATA/manifest.json" "$STAGE/data/"
    while IFS= read -r f; do FILES+=("$f"); done < <(cd "$STAGE" && find data -type f | sort)
fi

echo "== pkg (${#FILES[@]} files)"
(cd "$STAGE" && "$BIN/create-gp4" -out "$STAGE/pkg.gp4" --content-id="$CONTENT_ID" --files "${FILES[*]}" >/dev/null)
python3 -I "$ROOT/tools/ps4/gp4-dirs.py" "$STAGE" "$STAGE/pkg.gp4"
NAME="$CONTENT_ID"
[ -n "$DATA" ] && [ "$VARIANT" != smoke ] && NAME="$NAME-with-data"
(cd "$STAGE" && PKGTOOL pkg_build "$STAGE/pkg.gp4" "$BUILD" >/dev/null)
mv "$BUILD/$CONTENT_ID.pkg" "$OUT/$NAME.pkg"
ls -la "$OUT/$NAME.pkg"
