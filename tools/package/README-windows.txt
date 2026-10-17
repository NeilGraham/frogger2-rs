Frogger 2: Swampy's Revenge (Rust) - a Rust port of the original game (local build)
==========================================================================================

Start: double-click launcher.exe. It imports your copy of the game (below), then makes
       frogger2.exe in this folder with the game's own icon (cut from your copy, never shipped);
       PLAY starts it, and you can start frogger2.exe directly any time. The launcher also installs
       newer releases from GitHub side by side (VERSIONS) and switches mods on and off (MODS).
       The game starts fullscreen at 1024x768 (the original's largest size, 4:3, with black bars);
       change that in the launcher's Settings or the game's Display settings.
       The real programs are in engine\; everything else (data\, saves\, mods\, config.toml) is
       created in this folder, because portable.txt is here.
       It boots like the original: logos, intro, title, then the 3D frontend.
       Game data is YOUR imported copy in data\usa (never redistribute it).

Import (first run, or no data found): the launcher opens on "Set up your game". Choose
  - "Import" next to the Frogger 2 disc in your drive (music is ripped and checked against
    the known disc hashes), or
  - "Choose disc image" (.cue/.bin, .ccd/.img, .mds/.mdf, .nrg, .toc, .iso, .zip) or
    "Choose folder" (a copied disc or an installed game), or
  - drop the image file onto the window.
  Afterwards "Add shortcut" asks where to put a "Frogger 2: Swampy's Revenge" shortcut
  (the desktop is suggested). "Export" builds the game for your other devices.
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
  adds a dusk copy of Garden 1: start it with  frogger2.exe --mods-dir mods --mods hello-mod --level 0 1).
  Level editor: frogger2.exe --editor --level 0 0
Online: frogger2.exe --host --multi WORLD LEVEL PLAYERS   /   frogger2.exe --join CODE
Viewer: f2-viewer.exe (models, levels, textures, sounds, videos).

Settings file and saves: next to the exe (config.toml, saves\, data\, mods\) because portable.txt is here; delete portable.txt to use %APPDATA%\frogger2-rs instead.
Docs: docs\USER-GUIDE.md, docs\TOOLS.md. Licenses: licenses\.
