#!/usr/bin/env bash
# Downloads Microsoft's DirectX Shader Compiler (DXC release, redistributable) and puts
# dxcompiler.dll and dxil.dll beside the game (GAME_DIR). With it wgpu compiles DX12 shaders at
# shader model 6.5+, which the Path-Traced graphics (ray queries) needs; without it the game uses
# FXC and Path-Traced falls back to Enhanced (docs/PATH-TRACING.md).
#
#   tools/run/fetch-dxc.sh GAME_DIR [TAG]     (default tag v1.8.2505.1)
set -euo pipefail
game="${1:?usage: fetch-dxc.sh GAME_DIR [TAG]}"
tag="${2:-v1.8.2505.1}"
url="$(curl -fsSL "https://api.github.com/repos/microsoft/DirectXShaderCompiler/releases/tags/$tag" \
  | grep -o '"browser_download_url": *"[^"]*/dxc_[0-9_]*\.zip"' | sed 's/.*"\(http[^"]*\)"/\1/' | head -1)"
[[ -n "$url" ]] || { echo "no dxc_*.zip in release $tag" >&2; exit 1; }
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "downloading $url"
curl -fL --progress-bar -o "$tmp/dxc.zip" "$url"
unzip -q -o "$tmp/dxc.zip" 'bin/x64/dxcompiler.dll' 'bin/x64/dxil.dll' -d "$tmp/x"
cp -f "$tmp/x/bin/x64/dxcompiler.dll" "$tmp/x/bin/x64/dxil.dll" "$game/"
ls -l "$game"/dxcompiler.dll "$game"/dxil.dll
