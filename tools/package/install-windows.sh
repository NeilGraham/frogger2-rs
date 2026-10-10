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
powershell.exe -NoProfile -Command "
\$s = New-Object -ComObject WScript.Shell
\$desk = [Environment]::GetFolderPath('Desktop')
foreach (\$p in @('$WDEST\\Frogger 2.lnk', (Join-Path \$desk 'Frogger 2.lnk'))) {
  \$l = \$s.CreateShortcut(\$p); \$l.TargetPath = '$WDEST\\frogger.exe'; \$l.WorkingDirectory = '$WDEST'; \$l.WindowStyle = 7; \$l.Save() }
\$l = \$s.CreateShortcut('$WDEST\\Frogger 2 (with example mod).lnk'); \$l.TargetPath = '$WDEST\\frogger.exe'; \$l.Arguments = '--mods-dir mods --mods hello-mod'; \$l.WorkingDirectory = '$WDEST'; \$l.WindowStyle = 7; \$l.Save()"
echo "installed to $WDEST"
