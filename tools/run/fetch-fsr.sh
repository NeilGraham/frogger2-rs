#!/usr/bin/env bash
# Downloads AMD's FSR runtime (FidelityFX SDK prebuilt, MIT) and puts its three DX12 DLLs in
# GAME_DIR/upscalers (docs/UPSCALING.md): amd_fidelityfx_loader_dx12.dll,
# amd_fidelityfx_upscaler_dx12.dll (FSR 3.1 + FSR 4), amd_fidelityfx_framegeneration_dx12.dll.
#
#   tools/run/fetch-fsr.sh GAME_DIR [SDK_TAG]     (default tag v2.3.0)
set -euo pipefail
game="${1:?usage: fetch-fsr.sh GAME_DIR [SDK_TAG]}"
tag="${2:-v2.3.0}"
url="https://github.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK/releases/download/$tag/FidelityFX-Samples-$tag-prebuilt.zip"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "downloading $url"
curl -fL --progress-bar -o "$tmp/sdk.zip" "$url"
unzip -q -o "$tmp/sdk.zip" 'Samples/Upscalers/FidelityFX_FSR/dx12/x64/Release/amd_fidelityfx_*_dx12.dll' -d "$tmp/x"
mkdir -p "$game/upscalers"
cp -f "$tmp"/x/Samples/Upscalers/FidelityFX_FSR/dx12/x64/Release/amd_fidelityfx_*_dx12.dll "$game/upscalers/"
ls -1 "$game/upscalers" | grep amd_fidelityfx
