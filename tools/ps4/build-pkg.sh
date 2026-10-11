#!/usr/bin/env bash
# Builds the PS4 homebrew package (fPKG) of the Frogger 2 port. It installs on a
# jailbroken PS4 (GoldHEN) or a jailbroken PS5 (etaHEN / kstuff, PS4 fPKG support).
#
#   tools/ps4/build-pkg.sh [--data RENDITION_DIR] [--icon IMAGE] [--out DIR]
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
#
# Needs: the OpenOrbis PS4 toolchain ($OO_PS4_TOOLCHAIN, default ~/opt/openorbis/OpenOrbis/
# PS4Toolchain), clang + ld.lld (LLVM 18), rustup target x86_64-unknown-freebsd, ffmpeg
# (artwork). See tools/ps4/README.md.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
export OO_PS4_TOOLCHAIN="${OO_PS4_TOOLCHAIN:-$HOME/opt/openorbis/OpenOrbis/PS4Toolchain}"
TC="$OO_PS4_TOOLCHAIN"
DATA=""
ICON=""
OUT="$ROOT/target/ps4"
while [ $# -gt 0 ]; do
    case "$1" in
        --data) DATA="$(cd "$2" && pwd)"; shift 2 ;;
        --icon) ICON="$(realpath "$2")"; shift 2 ;;
        --out) OUT="$(realpath -m "$2")"; shift 2 ;;
        -h|--help) sed -n '2,22p' "$0"; exit 0 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done

TITLE="Frogger 2: Swampy's Revenge"
TITLE_ID="FROG20000"
CONTENT_ID="IV0000-${TITLE_ID}_00-FROGGER2RUSTPORT"
# Package version (param.sfo "XX.YY"); bump for updates over an installed copy.
VERSION="01.00"

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

echo "== Rust staticlib (x86_64-unknown-freebsd)"
# Jaguar (PS4) feature level; PS5's Zen 2 runs it too. panic=abort: no unwinding across
# the system libraries.
CARGO_TARGET_X86_64_UNKNOWN_FREEBSD_RUSTFLAGS="-C target-cpu=btver2 -C panic=abort" \
    cargo build --manifest-path "$ROOT/crates/frogger-ps4/Cargo.toml" --release \
    --target x86_64-unknown-freebsd --target-dir "$ROOT/target/ps4-cargo"
LIB="$ROOT/target/ps4-cargo/x86_64-unknown-freebsd/release/libfrogger_ps4.a"

echo "== link"
"$CLANG" --target=x86_64-pc-freebsd12-elf -fPIC -O2 -ffreestanding -fno-builtin -c "$ROOT/crates/frogger-ps4/csrc/crt_shim.c" -o "$BUILD/crt_shim.o"
"$LLD" -m elf_x86_64 -pie --script "$ROOT/crates/frogger-ps4/link.x" --eh-frame-hdr --gc-sections --strip-debug -L"$TC/lib" \
    "$TC/lib/crt1.o" "$BUILD/crt_shim.o" "$LIB" \
    -lkernel -lSceLibcInternal -lSceVideoOut -lScePad -lSceAudioOut -lSceUserService -lSceSysmodule -lSceSystemService \
    -o "$BUILD/frogger2.elf"
"$BIN/create-fself" -in="$BUILD/frogger2.elf" -out="$BUILD/frogger2.oelf" --eboot "$STAGE/eboot.bin" --paid 0x3800000000000011

echo "== sce_sys"
cp "$TC/src/modules/right.sprx" "$STAGE/sce_sys/about/right.sprx"
cp "$TC/src/modules/libc.prx" "$TC/src/modules/libSceFios2.prx" "$STAGE/sce_module/"
SFO="$STAGE/sce_sys/param.sfo"
PKGTOOL sfo_new "$SFO"
PKGTOOL sfo_setentry "$SFO" APP_TYPE --type Integer --maxsize 4 --value 1
PKGTOOL sfo_setentry "$SFO" APP_VER --type Utf8 --maxsize 8 --value "$VERSION"
# ATTRIBUTE: bit 23 = supports PS4 Pro mode (4K output, higher clocks on Pro; the PS5
# runs the title in Pro mode), bit 16 = 7-CPU mode, bit 1 = Cross is the enter button.
PKGTOOL sfo_setentry "$SFO" ATTRIBUTE --type Integer --maxsize 4 --value $(( (1 << 23) | (1 << 16) | (1 << 1) ))
PKGTOOL sfo_setentry "$SFO" CATEGORY --type Utf8 --maxsize 4 --value gd
PKGTOOL sfo_setentry "$SFO" CONTENT_ID --type Utf8 --maxsize 48 --value "$CONTENT_ID"
# MiB of /download0 (app-private writable storage): the save fallback when /data is not writable.
PKGTOOL sfo_setentry "$SFO" DOWNLOAD_DATA_SIZE --type Integer --maxsize 4 --value 64
PKGTOOL sfo_setentry "$SFO" SYSTEM_VER --type Integer --maxsize 4 --value 0
PKGTOOL sfo_setentry "$SFO" TITLE --type Utf8 --maxsize 128 --value "$TITLE"
PKGTOOL sfo_setentry "$SFO" TITLE_ID --type Utf8 --maxsize 12 --value "$TITLE_ID"
PKGTOOL sfo_setentry "$SFO" VERSION --type Utf8 --maxsize 8 --value "$VERSION"

if [ -z "$ICON" ] && [ -n "$DATA" ]; then
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
if [ -n "$DATA" ]; then
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
[ -n "$DATA" ] && NAME="$NAME-with-data"
(cd "$STAGE" && PKGTOOL pkg_build "$STAGE/pkg.gp4" "$BUILD" >/dev/null)
mv "$BUILD/$CONTENT_ID.pkg" "$OUT/$NAME.pkg"
ls -la "$OUT/$NAME.pkg"
