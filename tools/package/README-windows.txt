Frogger 2: Swampy's Revenge (Rust) - a Rust port of the original game (local build)
==========================================================================================

Play:  double-click "Frogger 2 - Swampy's Revenge (Rust)" (here or on the Desktop).
       The icon switches to your game's own artwork after the first start with imported data.
       It boots like the original: logos, intro, title, then the 3D frontend.
       Game data is YOUR imported copy in game-data\usa (never redistribute it).

Import (first run, or no data found): the game opens an import screen. Choose
  - "Import from the Frogger 2 disc in D:" with the CD in a drive (music is ripped and
    checked against the known disc hashes), or
  - "Import from an image file..." (.cue/.bin, .ccd/.img, .mds/.mdf, .nrg, .toc, .iso, .zip), or
  - drag the image file onto the window.
  An image mounted with Explorer's "Mount" works too (pick its drive letter; the image file
  behind it is used). A plain .iso has no music: add it later with
      f2-import.exe your.iso --music C:\path\to\tracks   (02.wav ... 12.wav or .flac)
  Command line: f2-import.exe D:   |   f2-import.exe "C:\Games\Frogger 2.cue"   |   f2-import.exe --list-drives
  Not supported: .chd, .cdi, .7z, .rar (convert/extract first; the importer tells you how).

Keyboard (player 1): arrows = hop, Enter = select / hop confirm (Cross),
  Right Ctrl = Square, Right Shift = Circle, Esc = pause / back.
  Gamepads (Xbox/PlayStation/Switch) work automatically.
Quit: the game's own Quit menu, or close the window.

Settings: "Settings" in the pause menu (Esc) or at the end of the frontend's Extras list, or F10:
  Display (window mode, resolution, refresh rate, VSync, frame cap, graphics, brightness,
  FPS counter), Audio, Controls (keyboard / controller mapping, dead zone, rumble), Gameplay.
  Alt+Enter / F11 toggle windowed <-> borderless fullscreen (or exclusive fullscreen once you
  pick "Fullscreen" in Display settings).
Hotkeys: F10 settings, Shift+F10 advanced screen (Level select), F11 / Alt+Enter fullscreen,
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
