#!/usr/bin/env bash
# Builds the PS4 export runtime: the PS4 program without any game data, as
#   OUT_DIR/runtime-ps4-<VERSION>.zip
# which the desktop app's "Export to PS4" (crates/f2-package/src/ps4.rs) turns into a fake
# package with the player's own game data, title and icon (docs/EXPORT.md "PS4").
#
#   tools/runtime/build-ps4.sh OUT_DIR VERSION
#
# Zip contents (paths inside the package's /app0):
#   eboot.bin                      the game (fake-signed SELF, tools/ps4/build-pkg.sh)
#   sce_module/libc.prx, libSceFios2.prx, sce_sys/about/right.sprx   OpenOrbis' modules
#   runtime.json                   {"target":"ps4","version":VERSION,"format":1, title/content ID,
#                                   param.sfo values, "data_dir":"data", "files":[package order]}
#
# Prerequisites (Linux x86_64; see tools/ps4/README.md "Requirements"):
#   - OpenOrbis PS4 toolchain v0.5.4 (https://github.com/OpenOrbis/OpenOrbis-PS4-Toolchain/
#     releases, toolchain-llvm-18.tar.gz) extracted; OO_PS4_TOOLCHAIN=<...>/OpenOrbis/PS4Toolchain
#     (default ~/opt/openorbis/OpenOrbis/PS4Toolchain). Only create-fself and the modules in
#     src/modules are used here: no PkgTool / .NET / OpenSSL 1.1.
#   - clang-18 and ld.lld-18 (apt install clang-18 lld-18), python3.
#   - Rust nightly with rust-src (rustup toolchain install nightly --component rust-src); std is
#     rebuilt for the custom console target.
# Env: F2_PS4_VERSION (param.sfo APP_VER/VERSION, default 01.03 from build-pkg.sh; bump it for
# a package that installs over a previous one), OO_PS4_TOOLCHAIN.
set -euo pipefail

[ $# -eq 2 ] || { sed -n '2,25p' "$0"; exit 2; }
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="$(realpath -m "$1")"
VERSION="$2"
WORK="$ROOT/target/runtime-ps4"
mkdir -p "$OUT"

# The conservative game build (title FROG20000): the one verified in shadPS4 and the variant
# hardware testers are given first (tools/ps4/README.md).
"$ROOT/tools/ps4/build-pkg.sh" --stage-only --out "$WORK"
# shellcheck source=/dev/null
source "$WORK/stage.env"

RT="$WORK/runtime"
rm -rf "$RT"
mkdir -p "$RT/sce_module" "$RT/sce_sys/about"
cp "$WORK/stage/eboot.bin" "$RT/"
cp "$WORK/stage/sce_module/libc.prx" "$WORK/stage/sce_module/libSceFios2.prx" "$RT/sce_module/"
cp "$WORK/stage/sce_sys/about/right.sprx" "$RT/sce_sys/about/"

python3 -I - "$RT" "$OUT/runtime-ps4-$VERSION.zip" "$VERSION" "$TITLE_ID" "$CONTENT_ID" "$APP_VER" "$ATTRIBUTE" \
    "$DOWNLOAD_DATA_SIZE" "$SYSTEM_VER" "$APP_TYPE" <<'PY'
import json, os, sys, zipfile
rt, out, version, title_id, content_id, app_ver, attribute, dds, system_ver, app_type = sys.argv[1:]
files = ["eboot.bin", "sce_sys/about/right.sprx", "sce_module/libc.prx", "sce_module/libSceFios2.prx"]
info = {
    "target": "ps4",
    "version": version,
    "format": 1,
    "title_id": title_id,
    "content_id": content_id,
    "app_ver": app_ver,
    "attribute": int(attribute),
    "download_data_size": int(dds),
    "system_ver": int(system_ver),
    "app_type": int(app_type),
    "data_dir": "data",
    "files": files,
}
with open(os.path.join(rt, "runtime.json"), "w") as f:
    json.dump(info, f, indent=2)
    f.write("\n")
# Deterministic zip: fixed order and time stamps.
with zipfile.ZipFile(out + ".part", "w", zipfile.ZIP_DEFLATED) as z:
    for name in ["runtime.json"] + files:
        zi = zipfile.ZipInfo(name, (2000, 1, 1, 0, 0, 0))
        zi.compress_type = zipfile.ZIP_DEFLATED
        zi.external_attr = 0o644 << 16
        with open(os.path.join(rt, name), "rb") as f:
            z.writestr(zi, f.read())
os.replace(out + ".part", out)
PY
ls -la "$OUT/runtime-ps4-$VERSION.zip"
