#!/usr/bin/env bash
# Builds the PS5 export runtime: the native PS5 program without any game data, as
#   OUT_DIR/runtime-ps5-<VERSION>.zip
# which the desktop app's "Export to PS5" (crates/f2-package/src/ps5.rs) turns into an app
# folder with the player's own game data, title and icon (docs/EXPORT.md "PS5").
#
#   tools/runtime/build-ps5.sh OUT_DIR VERSION
#
# Zip contents (paths inside the app folder, /app0 on the console):
#   eboot.bin             the game (PS5 executable in a fake-signed SELF, tools/ps5/build.sh)
#   sce_module/libc.prx   the libc companion module every title carries
#   runtime.json          {"target":"ps5","version":VERSION,"format":1, title/content ID,
#                          "param_json": sce_sys/param.json without the title, "data_dir":"data",
#                          "files":[...]}
#
# Prerequisites (Linux x86_64; see tools/ps5/README.md "How it works"):
#   - ps5-payload-sdk v0.43 (https://github.com/ps5-payload-dev/sdk/releases) extracted;
#     PS5_PAYLOAD_SDK=<dir> (default ~/opt/ps5-payload-sdk).
#   - a checkout of https://github.com/mihawk-99/PS5_Vulkan at 252bf91 (only its
#     tooling/native sources are compiled: the ELF converter / fake-SELF signer and the
#     libc.prx builder); PS5_VULKAN=<dir> (default ~/opt/PS5_Vulkan).
#   - clang-18, clang++-18, ld.lld-18 (apt install clang-18 lld-18), python3, wget (zlib
#     1.3.2 is downloaded and built for the tool unless F2_ZLIB points at an install).
#   - Rust nightly with rust-src (rustup toolchain install nightly --component rust-src).
#   Not needed here: bc7enc_rdo, ffmpeg, .NET / LibProsperoPkg, UFS2Tool, MkPFS (the export
#   writes the artwork and folder itself).
# Env: F2_PS5_VERSION (param.json contentVersion, default from build.sh; bump it for a build
# that installs over a previous one), PS5_PAYLOAD_SDK, PS5_VULKAN, F2_ZLIB.
set -euo pipefail

[ $# -eq 2 ] || { sed -n '2,29p' "$0"; exit 2; }
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="$(realpath -m "$1")"
VERSION="$2"
WORK="$ROOT/target/runtime-ps5"
mkdir -p "$OUT"

"$ROOT/tools/ps5/build.sh" --stage-only --out "$WORK"
# shellcheck source=/dev/null
source "$WORK/stage.env"
APP="$WORK/$TITLE_ID"

python3 -I - "$APP" "$OUT/runtime-ps5-$VERSION.zip" "$VERSION" "$TITLE_ID" "$CONTENT_ID" "$CONTENT_VERSION" <<'PY'
import json, os, sys, zipfile
app, out, version, title_id, content_id, content_version = sys.argv[1:]
files = ["eboot.bin", "sce_module/libc.prx"]
with open(os.path.join(app, "sce_sys", "param.json")) as f:
    param = json.load(f)
# The title comes from the player's disc at export time.
param["localizedParameters"]["en-US"]["titleName"] = ""
info = {
    "target": "ps5",
    "version": version,
    "format": 1,
    "title_id": title_id,
    "content_id": content_id,
    "content_version": content_version,
    "param_json": param,
    "data_dir": "data",
    "files": files,
}
data = (json.dumps(info, indent=2) + "\n").encode()
with zipfile.ZipFile(out + ".part", "w", zipfile.ZIP_DEFLATED) as z:
    zi = zipfile.ZipInfo("runtime.json", (2000, 1, 1, 0, 0, 0))
    zi.compress_type = zipfile.ZIP_DEFLATED
    zi.external_attr = 0o644 << 16
    z.writestr(zi, data)
    for name in files:
        zi = zipfile.ZipInfo(name, (2000, 1, 1, 0, 0, 0))
        zi.compress_type = zipfile.ZIP_DEFLATED
        zi.external_attr = 0o644 << 16
        with open(os.path.join(app, name), "rb") as f:
            z.writestr(zi, f.read())
os.replace(out + ".part", out)
PY
ls -la "$OUT/runtime-ps5-$VERSION.zip"
