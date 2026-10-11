#!/usr/bin/env bash
# Builds the PS4 Pro bisect packages: the "enhanced" package (FROG20002: ATTRIBUTE Pro mode
# bit 23 + 7-CPU bit 16 + Cross-enter bit 1, Piglet auth info) crashed on a PS5 (FW 11.40,
# CE-108255-1) while the native PS5 build runs. Each package below changes one thing
# against the conservative one (FROG20000: ATTRIBUTE 0, default auth info), with its own
# title ID so all install side by side:
#
#   A  FROG20003  Pro mode only (bit 23), default auth info     framebuffers at the output mode (4K on a 4K TV)
#   B  FROG20004  7-CPU mode only (bit 16), default auth info
#   C  FROG20005  ATTRIBUTE 0, Piglet auth info
#   D  FROG20006  Pro mode only, default auth info, framebuffers forced to 1920x1080
#   E  FROG20007  enhanced ATTRIBUTE (bits 23+16+1), default auth info
#
#   tools/ps4/build-pro-bisect.sh [--data DIR] [--icon IMG] [--out DIR] [--copy-to DIR]
#
# Output: <out>/PS4-<letter>-<what>.pkg plus README.txt (tools/ps4/pro-bisect-README.txt).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="$ROOT/target/ps4-bisect"
COPY=""
ARGS=()
while [ $# -gt 0 ]; do
    case "$1" in
        --data) ARGS+=(--data "$(cd "$2" && pwd)"); shift 2 ;;
        --icon) ARGS+=(--icon "$(realpath "$2")"); shift 2 ;;
        --out) OUT="$(realpath -m "$2")"; shift 2 ;;
        --copy-to) COPY="$2"; shift 2 ;;
        -h|--help) sed -n '2,17p' "$0"; exit 0 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
BUILD="$ROOT/target/ps4"
rm -rf "$OUT"
mkdir -p "$OUT"

PRO=$((1 << 23))
CPU7=$((1 << 16))
ENH=$(((1 << 23) | (1 << 16) | (1 << 1)))
# letter:title id:label:file name:ATTRIBUTE:auth info:force 1080p framebuffers:title suffix
VARIANTS=(
    "A:FROG20003:FROGGER2PROMODE0:pro-only:$PRO:default::Pro mode"
    "B:FROG20004:FROGGER27CPUMODE:7cpu-only:$CPU7:default::7-CPU"
    "C:FROG20005:FROGGER2PIGLETAU:piglet-authinfo-only:0:piglet::Piglet auth"
    "D:FROG20006:FROGGER2PRO1080P:pro-only-fb1080:$PRO:default:1:Pro 1080p"
    "E:FROG20007:FROGGER2PRO7CPU0:pro-7cpu-default-authinfo:$ENH:default::Pro+7CPU"
)
for v in "${VARIANTS[@]}"; do
    IFS=: read -r letter id label name attr auth fb1080 suffix <<< "$v"
    echo "#### $letter $id $name"
    if [ -n "$fb1080" ]; then export F2_PS4_FB_1080=1; else unset F2_PS4_FB_1080; fi
    F2_PS4_TITLE_ID="$id" F2_PS4_LABEL="$label" F2_PS4_TITLE="Frogger 2 test $letter ($suffix)" \
        F2_PS4_ATTRIBUTE="$attr" F2_PS4_AUTHINFO="$auth" \
        "$ROOT/tools/ps4/build-pkg.sh" "${ARGS[@]}" --out "$BUILD"
    pkg=$(ls -t "$BUILD"/IV0000-"$id"_00-"$label"*.pkg | head -1)
    cp "$pkg" "$OUT/PS4-$letter-$name.pkg"
done
unset F2_PS4_FB_1080
cp "$ROOT/tools/ps4/pro-bisect-README.txt" "$OUT/README.txt"
(cd "$OUT" && sha256sum ./*.pkg > SHA256SUMS.txt)
ls -la "$OUT"
if [ -n "$COPY" ]; then
    mkdir -p "$COPY"
    cp -r "$OUT"/. "$COPY"/
    echo "copied to $COPY"
fi
