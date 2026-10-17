#!/usr/bin/env bash
# Build the Windows exes from the current checkout and (re)install a playable copy at
# C:\Frogger2-rs (override with F2_WIN_INSTALL), using the locally imported game data
# (game-data/<rendition>, default usa; never distributed). Creates Start shortcuts.
set -euo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DEST="${F2_WIN_INSTALL:-/mnt/c/Frogger2-rs}"
REND="${F2_RENDITION:-usa}"
bash "$REPO/tools/package/build-windows.sh"
mkdir -p "$DEST/game-data" "$DEST/docs" "$DEST/licenses" "$DEST/mods"
cp "$REPO"/target/package/win-bin/*.exe "$DEST/"
rsync -a --delete --exclude /oracle "$REPO/game-data/$REND/" "$DEST/game-data/$REND/"
cp "$REPO"/docs/USER-GUIDE.md "$REPO"/docs/TOOLS.md "$DEST/docs/"
cp "$REPO"/LICENSE-* "$REPO"/NOTICE "$DEST/licenses/"
rsync -a "$REPO/examples/mods/" "$DEST/mods/"
cp "$REPO/tools/package/README-windows.txt" "$DEST/README.txt"
WDEST="$(wslpath -w "$DEST")"
# Shortcuts: "Frogger 2 - Swampy's Revenge (Rust)". Their icon is the exe's own (our artwork) until the game
# has made one from the player's imported artwork (%APPDATA%\frogger2-rs\icons\game-art.ico,
# src/appicon.rs), which it also applies to these shortcuts itself. The old "Frogger 2"
# shortcuts this script used to create are removed.
powershell.exe -NoProfile -Command "
\$s = New-Object -ComObject WScript.Shell
\$desk = [Environment]::GetFolderPath('Desktop')
\$art = Join-Path \$env:APPDATA 'frogger2-rs\\icons\\game-art.ico'
\$icon = if (Test-Path \$art) { \$art + ',0' } else { '$WDEST\\frogger2.exe,0' }
foreach (\$old in @('$WDEST\\Frogger 2.lnk', '$WDEST\\Frogger 2 (with example mod).lnk', (Join-Path \$desk 'Frogger 2.lnk'))) {
  if (Test-Path -LiteralPath \$old) { if (\$s.CreateShortcut(\$old).TargetPath -ieq '$WDEST\\frogger2.exe') { Remove-Item -LiteralPath \$old } } }
foreach (\$p in @('$WDEST\\Frogger 2 - Swampy''s Revenge (Rust).lnk', (Join-Path \$desk 'Frogger 2 - Swampy''s Revenge (Rust).lnk'))) {
  \$l = \$s.CreateShortcut(\$p); \$l.TargetPath = '$WDEST\\frogger2.exe'; \$l.WorkingDirectory = '$WDEST'; \$l.WindowStyle = 7; \$l.IconLocation = \$icon; \$l.Save() }
\$l = \$s.CreateShortcut('$WDEST\\Frogger 2 - Swampy''s Revenge (Rust) (with example mod).lnk'); \$l.TargetPath = '$WDEST\\frogger2.exe'; \$l.Arguments = '--mods-dir mods --mods hello-mod'; \$l.WorkingDirectory = '$WDEST'; \$l.WindowStyle = 7; \$l.IconLocation = \$icon; \$l.Save()"
echo "installed to $WDEST"
