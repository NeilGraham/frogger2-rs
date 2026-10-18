#!/usr/bin/env bash
# Sets up a Windows game folder (the one with frogger2.exe) for DLSS / DLSS 5 Neural Rendering
# (docs/UPSCALING.md "DLSS 5"): the DLSS / Streamline runtimes into upscalers/, ReShade with
# add-on support (ReShade64.dll, which the game loads itself), RenoDX's DLSS add-on, and ReShade.ini with RenoDX's hook method
# on "Upscaled" (Neural Rendering on our DLSS output, before frame generation and the HUD).
#
#   tools/run/setup-dlss5.sh GAME_DIR [DOWNLOADS_DIR]
#
# DOWNLOADS_DIR holds the files from the RenoDX Discord / reshade.me (not redistributed here):
#   DLSS*-Streamline*.zip, ReShade_Setup_*_Addon.exe, renodx-dlss.addon64
# Run from WSL (paths may be /mnt/c/... ) or Git Bash. Nothing is executed from the downloads.
set -euo pipefail
game="${1:?usage: setup-dlss5.sh GAME_DIR [DOWNLOADS_DIR]}"
dl="${2:-.}"
[[ -f "$game/frogger2.exe" ]] || { echo "no frogger2.exe in $game" >&2; exit 1; }
pick() { ls -1 "$dl"/$1 2>/dev/null | sort | tail -1; }

sl_zip="$(pick 'DLSS*Streamline*.zip')"
reshade="$(pick 'ReShade_Setup_*Addon*.exe')"
renodx="$(pick 'renodx-dlss*.addon64')"

if [[ -n "$sl_zip" ]]; then
    mkdir -p "$game/upscalers"
    unzip -o -q "$sl_zip" -d "$game/upscalers"
    # RenoDX loads the Neural Rendering runtime from beside the executable.
    [[ -f "$game/upscalers/nvngx_dlssnr.dll" ]] && cp -f "$game/upscalers/nvngx_dlssnr.dll" "$game/"
    echo "runtimes: $(basename "$sl_zip") -> upscalers/"
else
    echo "warning: no DLSS*Streamline*.zip in $dl (DLSS needs it)" >&2
fi

if [[ -n "$reshade" ]]; then
    # The setup is a self-extracting zip.
    tmp="$(mktemp -d)"
    unzip -o -q "$reshade" ReShade64.dll -d "$tmp" 2>/dev/null || true
    [[ -f "$tmp/ReShade64.dll" ]] || { echo "ReShade64.dll not found in $reshade" >&2; exit 1; }
    cp -f "$tmp/ReShade64.dll" "$game/ReShade64.dll"
    # The game loads ReShade64.dll itself (a dxgi.dll proxy crashes in wgpu's start-up).
    rm -f "$game/dxgi.dll"
    rm -rf "$tmp"
    echo "ReShade: $(basename "$reshade") -> ReShade64.dll"
else
    echo "warning: no ReShade_Setup_*_Addon.exe in $dl (DLSS 5 needs ReShade with add-on support)" >&2
fi

if [[ -n "$renodx" ]]; then
    cp -f "$renodx" "$game/"
    echo "RenoDX: $(basename "$renodx")"
else
    echo "warning: no renodx-dlss.addon64 in $dl (DLSS 5 Neural Rendering needs it)" >&2
fi

# ReShade.ini: set our keys, keep everything else.
ini="$game/ReShade.ini"
touch "$ini"
set_key() { # section key value
    python3 -I - "$ini" "$1" "$2" "$3" <<'EOF'
import sys
path, sec, key, val = sys.argv[1:]
lines = open(path, encoding="utf-8", errors="replace").read().splitlines()
out, cur, done, seen = [], None, False, False
for l in lines:
    s = l.strip()
    if s.startswith("[") and s.endswith("]"):
        if cur == sec and not done:
            out.append(f"{key}={val}"); done = True
        cur = s[1:-1]; seen = seen or cur == sec
    elif cur == sec and s.split("=", 1)[0].strip() == key:
        if not done:
            out.append(f"{key}={val}"); done = True
        continue
    out.append(l)
if not done:
    if not seen:
        out += ["", f"[{sec}]"]
    out.append(f"{key}={val}")
open(path, "w", encoding="utf-8").write("\n".join(out) + "\n")
EOF
}
# Neural Rendering on the DLSS output, before the HUD and frame generation, with real DLSS
# inputs. Builds from October 2026 call it HookStage (0 off, 1 Render = DLSS input, 2 Upscaled,
# 3 Present); September builds HookPoint (2 = Upscaled, its default Auto takes the back buffer).
set_key RENODX-DLSS DirectNeuralRenderingHookStage 2
set_key RENODX-DLSS DirectNeuralRenderingHookPoint 2
set_key RENODX-DLSS DirectNeuralRenderingRequireDlss 1
set_key GENERAL NoReloadOnInit 0
echo "ReShade.ini: [RENODX-DLSS] hook method Upscaled"
cat <<EOF

Done. In config.toml ([upscale]): upscaler = "dlss", and dlss5_helpers = true (preloads the
Neural Rendering runtime and nudges RenoDX to attach). Start the game, press Home for the ReShade
overlay; RenoDX's tab shows Neural Rendering's state. ReShade.log next to the exe says whether it
attached ("DLSS-NR direct: attached snippet", "NR Upscaled", "NR vendor evaluate end: result=0x00000001").
EOF
