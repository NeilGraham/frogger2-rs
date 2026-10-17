#!/usr/bin/env bash
# Produce release archives under target/package/.
#   tools/package/package.sh linux     # tar.gz (+ AppImage if appimagetool is found)
#   tools/package/package.sh windows   # zip (cross-builds via tools/package/build-windows.sh unless
#                                      #      F2_WIN_BIN_DIR points at prebuilt .exe files;
#                                      #      F2_WIN_ARCH=aarch64 names an ARM64 build)
#   tools/package/package.sh macos     # .app bundle in a zip + tar.gz (run on macOS; CI: macos-14)
#   tools/package/package.sh all       # linux + windows (when cargo.exe exists)
# Set F2_SKIP_BUILD=1 to package already-built binaries. No game data is ever included.
# F2_PKG_VERSION=0.1.0 names the archives with that version instead of <version>-<commit>
# (the release workflow, .github/workflows/release.yml, passes the tag's version).
set -euo pipefail
WINHOME="$(wslpath "$(cmd.exe /c "echo %USERPROFILE%" 2>/dev/null | tr -d "\\r")" 2>/dev/null || true)"   # your Windows home (WSL)
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO"
OUT="$REPO/target/package"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
SHA="$(git rev-parse --short HEAD 2>/dev/null || echo nogit)"
VER="${F2_PKG_VERSION:-$VERSION-$SHA}"
# The engine programs (in engine/ of the portable archives) and the launcher beside them.
BINS=(frogger2 f2-import f2-viewer)
LAUNCHER=launcher
TGT="${CARGO_TARGET_DIR:-target}"
mkdir -p "$OUT"

# Copy the docs/licenses common to every archive into directory $1.
stage_docs() {
  local d="$1"
  mkdir -p "$d/licenses"
  cp NOTICE README.md "$d/"
  cp LICENSE-GPL-3.0 LICENSE-LGPL-2.1 "$d/licenses/"
  [[ -f docs/BUILDING.md ]] && cp docs/BUILDING.md "$d/"
  cat > "$d/portable.txt" <<'TXT'
This file makes this folder a portable install of frogger2-rs.

Everything the game reads and writes stays in this folder:
  data/        your imported game (made from your own copy of Frogger 2)
  config.toml  settings          saves/   save games          mods/   mods
Delete this file to use the per-user folders instead (%APPDATA%\frogger2-rs,
~/.local/share/frogger2-rs, ~/Library/Application Support/frogger2-rs).
TXT
  return 0
}

pkg_linux() {
  [[ -n "${F2_SKIP_BUILD:-}" ]] || cargo build --release -p frogger -p f2-import -p f2-viewer
  local arch; arch="$(uname -m)"
  local name="frogger2-rs-$VER-linux-$arch" st="$OUT/stage/linux"
  rm -rf "$st"; mkdir -p "$st/$name/engine"
  for b in "${BINS[@]}"; do install -m755 "$TGT/release/$b" "$st/$name/engine/"; done
  install -m755 "$TGT/release/$LAUNCHER" "$st/$name/"
  strip "$st/$name"/engine/* "$st/$name/$LAUNCHER" 2>/dev/null || true
  install -m644 packaging/frogger2-rs.desktop packaging/icon/frogger2-rs.svg "$st/$name/"
  stage_docs "$st/$name"
  tar -C "$st" -czf "$OUT/$name.tar.gz" "$name"
  echo "wrote $OUT/$name.tar.gz"

  # AppImage (AppDir layout). Needs appimagetool on PATH or $F2_APPIMAGETOOL
  # (https://github.com/AppImage/appimagetool/releases, "continuous"). Recent appimagetool
  # downloads the type-2 runtime at build time; offline, pass it with
  # F2_APPIMAGE_RUNTIME=/path/to/runtime-x86_64 (github.com/AppImage/type2-runtime/releases).
  # No FUSE (containers/CI): export APPIMAGE_EXTRACT_AND_RUN=1.
  local tool="${F2_APPIMAGETOOL:-$(command -v appimagetool || true)}"
  if [[ -z "$tool" ]]; then
    echo "appimagetool not found; skipping AppImage (see tools/package/package.sh pkg_linux)"
    return 0
  fi
  local ad="$OUT/stage/Frogger2rs.AppDir"
  rm -rf "$ad"
  mkdir -p "$ad/usr/bin" "$ad/usr/share/licenses/frogger2-rs" "$ad/usr/share/applications" \
    "$ad/usr/share/icons/hicolor/scalable/apps"
  for b in "${BINS[@]}"; do install -m755 "$st/$name/engine/$b" "$ad/usr/bin/"; done
  install -m755 "$st/$name/$LAUNCHER" "$ad/usr/bin/"
  install -m644 packaging/frogger2-rs.desktop "$ad/usr/share/applications/"
  install -m644 packaging/icon/frogger2-rs.svg "$ad/usr/share/icons/hicolor/scalable/apps/"
  cp packaging/frogger2-rs.desktop packaging/icon/frogger2-rs.svg "$ad/"
  ln -sf frogger2-rs.svg "$ad/.DirIcon"
  cp NOTICE LICENSE-* "$ad/usr/share/licenses/frogger2-rs/"
  # AppRun: `frogger2` by default; `<AppImage> launcher` / `f2-import ...` / `f2-viewer` run the other programs.
  cat > "$ad/AppRun" <<'APPRUN'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
case "$1" in
  f2-import|f2-viewer|launcher) tool="$1"; shift; exec "$HERE/usr/bin/$tool" "$@" ;;
esac
exec "$HERE/usr/bin/frogger2" "$@"
APPRUN
  chmod +x "$ad/AppRun"
  local rt=()
  [[ -n "${F2_APPIMAGE_RUNTIME:-}" ]] && rt=(--runtime-file "$F2_APPIMAGE_RUNTIME")
  ARCH="$arch" "$tool" ${rt[@]+"${rt[@]}"} --no-appstream "$ad" "$OUT/Frogger2rs-$VER-$arch.AppImage"
  echo "wrote $OUT/Frogger2rs-$VER-$arch.AppImage"
}

pkg_windows() {
  local bindir="${F2_WIN_BIN_DIR:-$OUT/win-bin}"
  [[ -n "${F2_SKIP_BUILD:-}" || -n "${F2_WIN_BIN_DIR:-}" ]] || tools/package/build-windows.sh
  local name="frogger2-rs-$VER-windows-${F2_WIN_ARCH:-x86_64}" st="$OUT/stage/windows"
  rm -rf "$st"; mkdir -p "$st/$name/engine"
  for b in "${BINS[@]}"; do cp "$bindir/$b.exe" "$st/$name/engine/"; done
  cp "$bindir/$LAUNCHER.exe" "$st/$name/"
  stage_docs "$st/$name"
  zip_dir "$st" "$name"
}

# zip_dir PARENT NAME: $OUT/NAME.zip holding the folder PARENT/NAME.
zip_dir() {
  rm -f "$OUT/$2.zip"
  if command -v zip >/dev/null; then
    (cd "$1" && zip -qr "$OUT/$2.zip" "$2")
  elif command -v 7z >/dev/null; then
    # Git Bash on Windows (CI runners) has no zip(1) but 7-Zip.
    (cd "$1" && 7z a -tzip -bso0 -bsp0 "$OUT/$2.zip" "$2")
  else
    # no zip(1): use Python's stdlib (ZIP_DEFLATED)
    local py; py="$(command -v python3 || command -v python)"
    "$py" -I -c 'import shutil,sys;shutil.make_archive(sys.argv[1],"zip",sys.argv[2],sys.argv[3])' \
      "$OUT/$2" "$1" "$2"
  fi
  echo "wrote $OUT/$2.zip"
}

# macOS .app bundle (run on macOS; see tools/package/README-macos.md).
#   F2_MACOS_UNIVERSAL=1  build aarch64 + x86_64 and lipo them into universal binaries
#   F2_MACOS_TARGET=x86_64-apple-darwin  one architecture other than the host's (cross-build)
#   F2_CODESIGN_ID="Developer ID Application: ..."  sign with a real identity (default: ad-hoc "-")
pkg_macos() {
  local arch bindir="$TGT/release"
  if [[ -n "${F2_MACOS_UNIVERSAL:-}" ]]; then
    arch=universal bindir="$TGT/universal-apple-darwin/release"
    if [[ -z "${F2_SKIP_BUILD:-}" ]]; then
      for t in aarch64-apple-darwin x86_64-apple-darwin; do
        cargo build --release --target "$t" -p frogger -p f2-import -p f2-viewer
      done
    fi
    mkdir -p "$bindir"
    for b in "${BINS[@]}" "$LAUNCHER"; do
      lipo -create -output "$bindir/$b" "$TGT/aarch64-apple-darwin/release/$b" "$TGT/x86_64-apple-darwin/release/$b"
    done
  elif [[ -n "${F2_MACOS_TARGET:-}" ]]; then
    bindir="$TGT/$F2_MACOS_TARGET/release"
    arch="${F2_MACOS_TARGET%%-*}"; [[ "$arch" == aarch64 ]] && arch=arm64
    [[ -n "${F2_SKIP_BUILD:-}" ]] || cargo build --release --target "$F2_MACOS_TARGET" -p frogger -p f2-import -p f2-viewer
  else
    arch="$(uname -m)"
    [[ -n "${F2_SKIP_BUILD:-}" ]] || cargo build --release -p frogger -p f2-import -p f2-viewer
  fi
  local name="frogger2-rs-$VER-macos-$arch" st="$OUT/stage/macos"
  rm -rf "$st"; mkdir -p "$st/$name"
  local bundle="$st/$name/Frogger2rs.app" app="$st/$name/Frogger2rs.app/Contents"
  mkdir -p "$app/MacOS" "$app/Resources"
  for b in "${BINS[@]}" "$LAUNCHER"; do install -m755 "$bindir/$b" "$app/MacOS/"; done
  cat > "$app/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Frogger2rs</string>
  <key>CFBundleDisplayName</key><string>Frogger 2: Swampy's Revenge (Rust)</string>
  <key>CFBundleIdentifier</key><string>io.github.NeilGraham.Frogger2rs</string>
  <key>CFBundleExecutable</key><string>frogger2</string>
  <key>CFBundleIconFile</key><string>frogger2-rs</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.arcade-games</string>
  <key>GCSupportsControllerUserInteraction</key><true/>
</dict></plist>
PLIST
  if command -v plutil >/dev/null; then plutil -lint "$app/Info.plist"; fi
  # Icon: the pre-rendered PNGs (packaging/icon/png) -> .icns with
  # iconutil (macOS); the SVG is shipped as well.
  cp packaging/icon/frogger2-rs.svg "$app/Resources/"
  if command -v iconutil >/dev/null; then
    local set="$st/frogger2-rs.iconset"; mkdir -p "$set"
    for s in 16 32 128 256 512; do
      cp "packaging/icon/png/$s.png" "$set/icon_${s}x${s}.png"
      cp "packaging/icon/png/$((s * 2)).png" "$set/icon_${s}x${s}@2x.png"
    done
    iconutil -c icns "$set" -o "$app/Resources/frogger2-rs.icns"
  fi
  mkdir -p "$app/Resources/licenses"
  cp NOTICE LICENSE-* "$app/Resources/licenses/"
  stage_docs "$st/$name"
  # (Empty-array expansions are written ${a[@]+"${a[@]}"} for macOS's bash 3.2 under set -u.)
  # Ad-hoc signature by default: required for arm64 binaries to run at all; Gatekeeper still
  # quarantines downloads (right-click > Open, or `xattr -dr com.apple.quarantine`). Inner
  # binaries first, then the bundle (--deep is deprecated). Notarisation: README-macos.md.
  if command -v codesign >/dev/null; then
    local id="${F2_CODESIGN_ID:--}" opts=()
    [[ "$id" != "-" ]] && opts=(--options runtime --timestamp)
    # The helper tools first; the main executable (`frogger2`) is signed with the bundle:
    # codesign treats a bundle's main executable as the bundle, so signing it first would seal
    # the bundle while the tools are still unsigned (x86_64 builds from an arm64 host are not
    # linker-signed).
    for b in "${BINS[@]}" "$LAUNCHER"; do
      [[ "$b" == frogger2 ]] || codesign --force ${opts[@]+"${opts[@]}"} -s "$id" "$app/MacOS/$b"
    done
    codesign --force ${opts[@]+"${opts[@]}"} -s "$id" "$bundle"
    codesign --verify --strict "$bundle"
  fi
  tar -C "$st" -czf "$OUT/$name.tar.gz" "$name"
  echo "wrote $OUT/$name.tar.gz"
  # The zip (the release asset) with ditto, which keeps the bundle's signature, symlinks and
  # extended attributes intact.
  if command -v ditto >/dev/null; then
    rm -f "$OUT/$name.zip"
    ditto -c -k --keepParent "$st/$name" "$OUT/$name.zip"
    echo "wrote $OUT/$name.zip"
  else
    zip_dir "$st" "$name"
  fi
}

case "${1:-}" in
  linux) pkg_linux ;;
  windows) pkg_windows ;;
  macos) pkg_macos ;;
  all) pkg_linux; [[ -x "${F2_WIN_CARGO:-$WINHOME/.cargo/bin/cargo.exe}" ]] && pkg_windows || true ;;
  *) echo "usage: $0 linux|windows|macos|all" >&2; exit 2 ;;
esac
