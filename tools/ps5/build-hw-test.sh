#!/usr/bin/env bash
# Builds the PS4/PS5 hardware test ladder into target/console-test/: six titles with their
# own title IDs (they can all be installed side by side), launch payloads, the klog server
# payload and README.txt (tools/ps5/hw-test-README.txt) saying what to run and report.
#
#   tools/ps5/build-hw-test.sh [--data RENDITION_DIR] [--icon IMAGE] [--out DIR] [--copy-to DIR]
#
#   --data DIR     bundle your imported game data in the game builds (copyrighted: for your
#                  own console only, never share the result). Without it the games look for
#                  the data in /data/frogger2 or on USB at run time.
#   --icon IMG     artwork for the games' icons (e.g. a box-art scan; F2_ICON_CROP_X).
#   --out DIR      default target/console-test.
#   --copy-to DIR  also copy the result there (e.g. a Windows desktop folder under /mnt/c).
#
#   1 PS5  PPSA99922  C hello, PS5_Vulkan packaging          PS5-1-hello.pkg
#   2 PS5  PPSA99921  Rust std smoke test (emulated TLS)      PS5-2-smoke.pkg
#   3 PS5  PPSA99920  the game                                PS5-3-frogger.pkg
#   4 PS4  FROG20001  Rust smoke test (klog + colour)         PS4-4-smoke.pkg
#   5 PS4  FROG20000  the game, conservative settings         PS4-5-frogger-conservative.pkg
#   6 PS4  FROG20002  the game, Pro/7-CPU/Piglet auth info    PS4-6-frogger-enhanced.pkg
# plus ps5-other-formats/ (each PS5 title as a folder for /data/homebrew or /mnt/ext*, and
# the game as .ffpkg / .ffpfsc images) and payloads/ (klogsrv, launchers).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SDK="${PS5_PAYLOAD_SDK:-$HOME/opt/ps5-payload-sdk}"
OUT="$ROOT/target/console-test"
COPY=""
DATA_ARGS=()
ICON_ARGS=()
while [ $# -gt 0 ]; do
    case "$1" in
        --data) DATA_ARGS=(--data "$(cd "$2" && pwd)"); shift 2 ;;
        --icon) ICON_ARGS=(--icon "$(realpath "$2")"); shift 2 ;;
        --out) OUT="$(realpath -m "$2")"; shift 2 ;;
        --copy-to) COPY="$2"; shift 2 ;;
        -h|--help) sed -n '2,24p' "$0"; exit 0 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
PS5OUT="$ROOT/target/ps5"
PS4OUT="$ROOT/target/ps4"
rm -rf "$OUT"
mkdir -p "$OUT/ps5-other-formats" "$OUT/payloads" "$OUT/validation"

echo "#### 1. PS5 C hello"
"$ROOT/tools/ps5/build.sh" --hello --pkg --out "$PS5OUT"
echo "#### 2. PS5 Rust smoke"
"$ROOT/tools/ps5/build.sh" --smoke --pkg --out "$PS5OUT"
echo "#### 3. PS5 game"
"$ROOT/tools/ps5/build.sh" "${DATA_ARGS[@]}" "${ICON_ARGS[@]}" --pkg --ffpkg --ffpfsc --out "$PS5OUT"
for t in 1:PPSA99922:FROGGER2HELLOTST:hello 2:PPSA99921:FROGGER2SMOKETST:smoke 3:PPSA99920:FROGGER2RUSTPORT:frogger; do
    IFS=: read -r n id label name <<< "$t"
    cid="UP9000-${id}_00-${label}"
    cp "$PS5OUT/$cid.pkg" "$OUT/PS5-$n-$name.pkg"
    cp "$PS5OUT/$cid.pkg.validate.txt" "$OUT/validation/PS5-$n-$name.pkg.validate.txt"
    cp -r "$PS5OUT/$id" "$OUT/ps5-other-formats/$id"
done
cp "$PS5OUT/PPSA99920.ffpkg" "$PS5OUT/PPSA99920.ffpfsc" "$OUT/ps5-other-formats/"

echo "#### 4-6. PS4 packages"
"$ROOT/tools/ps4/build-pkg.sh" --smoke --out "$PS4OUT"
cp "$PS4OUT/IV0000-FROG20001_00-FROGGER2SMOKETST.pkg" "$OUT/PS4-4-smoke.pkg"
suffix=""
[ ${#DATA_ARGS[@]} -gt 0 ] && suffix="-with-data"
"$ROOT/tools/ps4/build-pkg.sh" "${DATA_ARGS[@]}" "${ICON_ARGS[@]}" --out "$PS4OUT"
cp "$PS4OUT/IV0000-FROG20000_00-FROGGER2RUSTPORT$suffix.pkg" "$OUT/PS4-5-frogger-conservative.pkg"
"$ROOT/tools/ps4/build-pkg.sh" "${DATA_ARGS[@]}" "${ICON_ARGS[@]}" --enhanced --out "$PS4OUT"
cp "$PS4OUT/IV0000-FROG20002_00-FROGGER2RUSTPRO0$suffix.pkg" "$OUT/PS4-6-frogger-enhanced.pkg"

echo "#### payloads (elfldr, port 9021)"
PCC="$SDK/bin/prospero-clang"
for id in PPSA99922 PPSA99921 PPSA99920 FROG20001 FROG20000 FROG20002; do
    "$PCC" -Wall -O2 "-DBOOTSTRAP_TITLE_ID=\"$id\"" -o "$OUT/payloads/launch-$id.elf" \
        "$ROOT/tools/ps5/launch/launch.c" -lSceSystemService -lSceUserService
done
# klogsrv (ps5-payload-dev, GPL-3.0-or-later): serves /dev/klog on TCP port 3232.
KLOG_REV=c1b64bd3d3b069a09b1a173a82858cba97a5656f
KSRC="$PS5OUT/tools/klogsrv"
if [ "$(git -C "$KSRC" rev-parse HEAD 2>/dev/null)" != "$KLOG_REV" ]; then
    rm -rf "$KSRC"
    git clone -q https://github.com/ps5-payload-dev/klogsrv.git "$KSRC"
    git -C "$KSRC" checkout -q --detach "$KLOG_REV"
fi
"$PCC" -Wall -Werror -DVERSION_TAG="\"${KLOG_REV:0:7}\"" -o "$OUT/payloads/klogsrv-ps5.elf" "$KSRC/main.c"

cp "$ROOT/tools/ps5/hw-test-README.txt" "$OUT/README.txt"
(cd "$OUT" && find . -type f ! -path './ps5-other-formats/PPSA*/*' ! -name MANIFEST.txt -printf '%P\t%s\n' | sort > MANIFEST.txt)
du -sh "$OUT"
if [ -n "$COPY" ]; then
    echo "#### copying to $COPY"
    mkdir -p "$COPY"
    cp -r "$OUT/." "$COPY/"
fi
