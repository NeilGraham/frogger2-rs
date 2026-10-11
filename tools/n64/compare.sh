#!/usr/bin/env bash
# Side-by-side + PSNR of a PC reference render (320x240, tools/n64/export --ref) and an
# ares capture of the N64 renderer (640x480 window).
#
#   tools/n64/compare.sh pc.png n64.png out_side.png
#
# Prints ffmpeg's PSNR of the 2x-bilinear-upscaled reference against the capture (the
# capture includes the VI resample filter and 16-bit dithering, so expect ~20-30 dB for a
# matching image; mismatched geometry/textures drop well below that).
set -euo pipefail
pc="$1"
n64="$2"
out="$3"
ffmpeg -loglevel error -y -i "$pc" -i "$n64" \
    -filter_complex "[0]scale=640:480:flags=neighbor[a];[a][1]hstack" "$out"
ffmpeg -loglevel info -i "$pc" -i "$n64" \
    -filter_complex "[0]scale=640:480:flags=bilinear,format=rgb24[a];[1]format=rgb24[b];[a][b]psnr" \
    -f null - 2>&1 | grep -o "PSNR.*" || true
echo "wrote $out"
