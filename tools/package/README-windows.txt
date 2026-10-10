Frogger 2: Swampy's Revenge - Rust port (local build)
======================================================

Play:  double-click "Frogger 2" (here or on the Desktop).
       It boots like the original: logos, intro, title, then the 3D frontend.
       Game data is YOUR imported copy in game-data\usa (never redistribute it).

Keyboard (player 1): arrows = hop, Enter = select / hop confirm (Cross),
  Right Ctrl = Square, Right Shift = Circle, Esc = pause / back.
  Gamepads (Xbox/PlayStation/Switch) work automatically.
Quit: the game's own Quit menu, or close the window.

Hotkeys: F10 settings (Controls / Gameplay assists / Level select), F11 / Alt+Enter fullscreen,
  F5 authentic <-> enhanced profile, F8 widescreen, F6 MSAA, F7 texture filter,
  F4 render scale, F12 draw distance, F9 fog, F1 free camera, F2 tile overlay.
  "Authentic" plays exactly like the 2000 PC release; "enhanced" adds widescreen,
  better image quality and optional assists/fixes.

Mods: "Frogger 2 (with example mod)" enables mods\hello-mod (changes a menu string and
  adds a dusk copy of Garden 1: start it with  frogger.exe --mods-dir mods --mods hello-mod --level 0 1).
  Level editor: frogger.exe --editor --level 0 0
Online: frogger.exe --host --multi WORLD LEVEL PLAYERS   /   frogger.exe --join CODE
Viewer: f2-viewer.exe (models, levels, textures, sounds, videos).

Settings file and saves: %APPDATA%\frogger2-rs (config.toml, saves\).
Docs: docs\USER-GUIDE.md, docs\TOOLS.md. Licenses: licenses\.
