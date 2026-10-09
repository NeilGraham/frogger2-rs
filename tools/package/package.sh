#!/usr/bin/env bash
# Produce release archives under target/package/.
#   tools/package/package.sh linux     # tar.gz (+ AppImage if appimagetool is on PATH)
#   tools/package/package.sh windows   # zip (cross-builds via tools/package/build-windows.sh unless
#                                      #      F2_WIN_BIN_DIR points at prebuilt .exe files)
#   tools/package/package.sh macos     # .app bundle + tar.gz (UNTESTED; run on macOS)
#   tools/package/package.sh all       # linux + windows (when cargo.exe exists)
# Set F2_SKIP_BUILD=1 to package already-built binaries. No game data is ever included.
set -euo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO"
OUT="$REPO/target/package"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
SHA="$(git rev-parse --short HEAD 2>/dev/null || echo nogit)"
VER="$VERSION-$SHA"
BINS=(frogger f2-import f2-viewer)
mkdir -p "$OUT"

# Copy the docs/licenses common to every archive into directory $1.
stage_docs() {
  local d="$1"
  mkdir -p "$d/licenses"
  cp NOTICE README.md "$d/"
  cp LICENSE-GPL-3.0 LICENSE-MIT LICENSE-APACHE LICENSE-LGPL-2.1 "$d/licenses/"
  [[ -f docs/BUILDING.md ]] && cp docs/BUILDING.md "$d/"
  return 0
}

pkg_linux() {
  [[ -n "${F2_SKIP_BUILD:-}" ]] || cargo build --release -p frogger -p f2-import -p f2-viewer
  local name="frogger2-rs-$VER-linux-x86_64" st="$OUT/stage/linux"
  rm -rf "$st"; mkdir -p "$st/$name"
  for b in "${BINS[@]}"; do install -m755 "target/release/$b" "$st/$name/"; done
  strip "$st/$name"/* 2>/dev/null || true
  install -m644 packaging/frogger2-rs.desktop packaging/frogger2-rs.svg "$st/$name/"
  stage_docs "$st/$name"
  tar -C "$st" -czf "$OUT/$name.tar.gz" "$name"
  echo "wrote $OUT/$name.tar.gz"

  # AppImage: AppDir layout; needs appimagetool (https://github.com/AppImage/appimagetool).
  if command -v appimagetool >/dev/null; then
    local ad="$OUT/stage/Frogger2rs.AppDir"
    rm -rf "$ad"; mkdir -p "$ad/usr/bin" "$ad/usr/share/licenses/frogger2-rs"
    for b in "${BINS[@]}"; do install -m755 "target/release/$b" "$ad/usr/bin/"; done
    cp packaging/frogger2-rs.desktop packaging/frogger2-rs.svg "$ad/"
    cp NOTICE LICENSE-* "$ad/usr/share/licenses/frogger2-rs/"
    printf '#!/bin/sh\nHERE="$(dirname "$(readlink -f "$0")")"\nexec "$HERE/usr/bin/frogger" "$@"\n' > "$ad/AppRun"
    chmod +x "$ad/AppRun"
    ARCH=x86_64 appimagetool "$ad" "$OUT/Frogger2rs-$VER-x86_64.AppImage"
  else
    echo "appimagetool not found; skipping AppImage (download from github.com/AppImage/appimagetool/releases)"
  fi
}

pkg_windows() {
  local bindir="${F2_WIN_BIN_DIR:-$OUT/win-bin}"
  [[ -n "${F2_SKIP_BUILD:-}" || -n "${F2_WIN_BIN_DIR:-}" ]] || tools/package/build-windows.sh
  local name="frogger2-rs-$VER-windows-x86_64" st="$OUT/stage/windows"
  rm -rf "$st"; mkdir -p "$st/$name"
  for b in "${BINS[@]}"; do cp "$bindir/$b.exe" "$st/$name/"; done
  stage_docs "$st/$name"
  rm -f "$OUT/$name.zip"
  if command -v zip >/dev/null; then
    (cd "$st" && zip -qr "$OUT/$name.zip" "$name")
  else
    # no zip(1): use Python's stdlib (ZIP_DEFLATED)
    python3 -I -c 'import shutil,sys;shutil.make_archive(sys.argv[1],"zip",sys.argv[2],sys.argv[3])' \
      "$OUT/$name" "$st" "$name"
  fi
  echo "wrote $OUT/$name.zip"
}

pkg_macos() {
  [[ -n "${F2_SKIP_BUILD:-}" ]] || cargo build --release -p frogger -p f2-import -p f2-viewer
  local arch; arch="$(uname -m)"
  local name="frogger2-rs-$VER-macos-$arch" st="$OUT/stage/macos"
  rm -rf "$st"; mkdir -p "$st/$name"
  local app="$st/$name/Frogger2rs.app/Contents"
  mkdir -p "$app/MacOS" "$app/Resources"
  for b in "${BINS[@]}"; do install -m755 "target/release/$b" "$app/MacOS/"; done
  cat > "$app/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Frogger2rs</string>
  <key>CFBundleDisplayName</key><string>Frogger 2 (Rust port)</string>
  <key>CFBundleIdentifier</key><string>net.frogger2rs.Frogger2</string>
  <key>CFBundleExecutable</key><string>frogger</string>
  <key>CFBundleIconFile</key><string>frogger2-rs</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.arcade-games</string>
</dict></plist>
PLIST
  # Icon: placeholder SVG shipped as-is; convert to .icns (iconutil) for a polished build.
  cp packaging/frogger2-rs.svg "$app/Resources/"
  mkdir -p "$app/Resources/licenses"
  cp NOTICE LICENSE-* "$app/Resources/licenses/"
  stage_docs "$st/$name"
  # Ad-hoc sign so Apple Silicon will run it locally (notarisation is out of scope).
  command -v codesign >/dev/null && codesign --force --deep -s - "$st/$name/Frogger2rs.app" || true
  tar -C "$st" -czf "$OUT/$name.tar.gz" "$name"
  echo "wrote $OUT/$name.tar.gz"
}

case "${1:-}" in
  linux) pkg_linux ;;
  windows) pkg_windows ;;
  macos) pkg_macos ;;
  all) pkg_linux; [[ -x "${F2_WIN_CARGO:-/mnt/c/Users/<you>/.cargo/bin/cargo.exe}" ]] && pkg_windows || true ;;
  *) echo "usage: $0 linux|windows|macos|all" >&2; exit 2 ;;
esac
