#!/usr/bin/env bash
# PS4 home-screen artwork from one image (e.g. the box art or the game's title screen):
#   make-art.sh IMAGE SCE_SYS_DIR
# writes icon0.png (512x512: centred square crop) and pic0.png/pic1.png (1920x1080: the
# image fitted to the height over a blurred, darkened copy of itself).
# Set F2_ICON_CROP_X to move the square crop (pixels from the left of the source).
set -euo pipefail
IMG="$1"
OUT="$2"
mkdir -p "$OUT"
IFS=, read -r W H < <(ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$IMG")
S=$(( W < H ? W : H ))
X="${F2_ICON_CROP_X:-$(( (W - S) / 2 ))}"
Y=$(( (H - S) / 2 ))
ffmpeg -loglevel error -y -i "$IMG" -vf "crop=$S:$S:$X:$Y,scale=512:512:flags=lanczos" -pix_fmt rgb24 "$OUT/icon0.png"
FW=$(( 1080 * W / H ))
ffmpeg -loglevel error -y -i "$IMG" -filter_complex \
    "[0]scale=1920:1080:force_original_aspect_ratio=increase,crop=1920:1080,boxblur=30:3,eq=brightness=-0.15[bg];[0]scale=$FW:1080:flags=lanczos[fg];[bg][fg]overlay=(W-w)/2:0" \
    -pix_fmt rgb24 "$OUT/pic1.png"
cp "$OUT/pic1.png" "$OUT/pic0.png"
