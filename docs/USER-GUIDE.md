# User guide

## What this is (and is not)

`frogger2-rs` is a Rust port of *Frogger 2: Swampy's Revenge* (PC, 2000), built from the preserved
original source. It runs on modern systems (Linux, Windows, macOS) with a widescreen-capable
renderer, gamepad support and rebindable controls.

**It contains no game data.** You need your own copy of the game (the CD in a drive, a disc
image in any common format, or an existing install folder). The importer extracts what the port
needs into a local data directory. Nothing copyrighted is ever distributed with this project.

Status: in development; see the Status section of [PLAN.md](PLAN.md). Some menus/options
screens and FMV flow are still being finished, so expect rough edges. Online play (`--host` /
`--join`) and mods (`--mods`) are available (see Multiplayer and Mods).

## Installing a release and the launcher

1. Download the zip for your system from the GitHub releases page and unzip it anywhere, for
   example into `frogger2-rs/`. That one folder holds everything the game uses: the programs, the
   imported game (`data/`), `config.toml`, `saves/` and `mods/`, because the zip contains a
   `portable.txt` (delete it to use the per-user folders instead). Moving or deleting the folder
   moves or deletes it all.
2. Start **`launcher`** (`launcher.exe`). Its **PLAY** tab imports your own copy of the game
   (disc, image or install folder; nothing from the game is downloaded or shipped) and then makes
   **`frogger2`** (`frogger2.exe`) in the folder. On Windows that exe carries the game's own icon
   and name, cut from your imported artwork on your machine, so only `launcher.exe` is there at
   first. **PLAY** starts the game; you can also start `frogger2` directly any time, which opens
   the game at once (and the launcher instead if you have no data yet).
   Once the game is imported the launcher also shows its own artwork, read from your copy on your
   machine and never shipped: the loading-screen picture as the background of the Play page, the
   game's logo, and the window icon.
3. **VERSIONS** lists the releases on GitHub. **INSTALL** downloads one (checked against the
   release's `SHA256SUMS.txt`) into `versions/<tag>/`, **USE** picks the version PLAY runs
   (your data, settings, saves and mods are shared by all versions), **REMOVE** deletes one.
   The launcher asks GitHub for the list when it starts; turn that off with
   `[launcher] check_updates = false` in `config.toml`, point it at a mirror with
   `[launcher] releases_url` or `$F2_RELEASES_URL`, or set `$F2_NO_UPDATE_CHECK=1`.
4. **MODS** lists the folders in `mods/` and switches them on and off (`[mods] enabled`).
5. **SETTINGS**: *On start, open the* Game (default) or Launcher (`[launcher] start`).
   `frogger2 --game` always opens the game.

On Android the launcher is reached from the swipe-in menu, see [ANDROID.md](ANDROID.md).

## Quick start

1. Build (see [BUILDING.md](BUILDING.md)):

   ```sh
   cargo build --release -p frogger -p f2-import -p f2-viewer
   ```

2. Import your copy. Easiest: just start `frogger2` — with no data it opens an **import screen**
   that lists your CD/DVD drives ("Import from the Frogger 2 disc in D:"), lets you pick an image
   file or folder, or accepts a file dragged onto the window; when the import finishes the game
   starts. Or use the command-line importer (see [Importing your game](#importing-your-game)):

   ```sh
   target/release/f2-import "/path/to/Frogger 2 - Swampy's Revenge (USA).zip"
   target/release/f2-import D:                    # the CD in drive D: (Linux: /dev/sr0)
   target/release/f2-import --list                # show imported renditions
   target/release/f2-import --verify-installed    # check the imported files
   ```

   Data goes to `$F2_DATA_ROOT`, else, in a **portable install** (the release zips: a
   `portable.txt` beside the app), the `data/` folder next to the app, so the game, `config.toml`,
   `saves/` and `mods/` all stay in the one folder you unzipped. Otherwise it goes to the platform
   data dir (`~/.local/share/frogger2-rs/data`, `%APPDATA%\frogger2-rs\data`,
   `~/Library/Application Support/frogger2-rs/data`), in one folder per rendition (`usa`, `eu`).
   Use `--data-root DIR` to import somewhere else. See [TOOLS.md](TOOLS.md) for all importer
   options (`--flac`, `--no-music`, `--as`, `--verify`).

3. Run:

   ```sh
   target/release/frogger2                          # normal start: logos, title, frontend
   target/release/frogger2 --data game-data/usa     # use a specific imported rendition
   ```

   Useful options: `--fullscreen`, `--size 1920x1080`, `--mute`, `--profile authentic|enhanced`.
   `frogger2 --help` prints the full list (including developer shortcuts such as
   `--level WORLD LEVEL`, `--multi WORLD LEVEL FROGS`, `--demo WORLD LEVEL`, `--video NAME`).

## Importing your game

Any of these works (full matrix and technical notes: [asset-import.md](asset-import.md)):

| You have | Do | Music |
|---|---|---|
| The CD in a drive (Windows or Linux) | import screen, or `f2-import D:` / `f2-import /dev/sr0` | ripped from the disc; the drive's read offset is detected and the rip checked against the known Redump hashes ("verified (offset +N)") |
| A disc image: `.cue`/`.bin` (one or many `.bin`), CloneCD `.ccd`/`.img`, Alcohol `.mds`/`.mdf`, Nero `.nrg`, cdrdao `.toc`/`.bin`, or a `.zip` holding one | pick the file (the `.cue`/`.ccd`/`.mds`/`.nrg`/`.toc`, or the `.zip`) | from the image |
| An image mounted in Windows Explorer ("Mount") | pick its drive letter | the importer finds the image file behind the drive; a mounted `.iso` has no music (see below) |
| A plain `.iso`, a copied disc folder, or an installed game folder | pick it | none — add `--music <folder>` |

**Music for data-only sources.** A `.iso` (and Windows' mounted ISOs, disc folders and installs)
contains no CD audio. The game runs fine without it; to add music, import again with
`f2-import <source> --music <folder>`, where the folder holds the 11 tracks as WAV (16-bit) or FLAC
named by track number (`02.wav` … `12.wav`, `Track 02.flac`, `track02.wav`) or by name
(`garden.wav`, `frontend.flac`, …). You can also copy `trackNN.wav`/`.flac` files into the
rendition's `music/` folder yourself. MP3/Ogg are not supported (convert them to FLAC).

**Not supported:** MAME `.chd` (convert with `chdman extractcd -i game.chd -o game.cue`),
DiscJuggler `.cdi`, `.7z`/`.rar` archives (extract them first) — the importer says so and how.

A scratched disc: read errors are retried (slower, sector by sector); sectors that still fail become
silence and the import log/manifest say how many. `f2-import --list-drives` shows what the importer
sees in your drives.

## Controls

### Keyboard defaults

These are the retail defaults (`DEFAULT_KEYMAP` in `crates/f2-platform/src/input.rs`).

| Action | Player 1 | Player 2 | Player 3 | Player 4 |
|---|---|---|---|---|
| Up / Down / Left / Right | Arrow keys | Q / A / S / D | 8 / I / O / P | Numpad 5 / 2 / 1 / 3 |
| Cross | Enter | Tab | 7 | Numpad Enter |
| Square | Right Ctrl | Left Shift | U | (unbound) |
| Circle | Right Shift | Q | (unbound) | (unbound) |
| Start | Esc | (unbound) | (unbound) | (unbound) |
| Triangle | Esc | Esc | Esc | Esc |
| L1 / R1 | Numpad 1 / R | (unbound) | (unbound) | (unbound) |
| L2 / R2 / Select | Numpad 4 / Numpad 6 / Numpad 0 | (unbound) | (unbound) | (unbound) |

In menus, player 1 also gets Up/Down/Left/Right, Enter and Esc. During videos Enter or Esc skips
the clip. Esc is the game's Start/Triangle (pause/back) key. Quit from the game's own menus (Quit on the
frontend, or pause → Quit) or close the window; Esc only quits developer scenes started with
`--level` outside a normal game session.

### Gamepads

Gamepads (Xbox, PlayStation, Switch layouts via gilrs) work out of the box and hot-plug.
Default mapping: D-pad or left stick = move, South = Cross, West = Square, East = Circle,
North = Triangle, Start = Start, Select = Select, L1/R1/L2/R2 = same. Pads are auto-assigned to
players. Rumble follows the `rumble` / `rumble_scale` options.

### Function keys

| Key | Effect |
|---|---|
| F1 | Free camera |
| F2 | Tile overlay (debug) |
| F3 | Toggle perspective-correct texturing |
| F4 | Cycle render scale (1x / 1.5x / 2x) |
| F5 | Toggle profile (authentic / enhanced) |
| F6 | MSAA off / 4x |
| F7 | Cycle texture filter (nearest / linear / anisotropic) |
| F8 | Toggle widescreen (Hor+ FOV and anchored HUD) |
| F9 | Toggle fog |
| F10 | Settings screen (pauses the simulation), same as "Settings" in the pause menu |
| Shift+F10 | Advanced screen (raw controls / gameplay pages and the level select); Tab switches page |
| F11 or Alt+Enter | Toggle between windowed and your fullscreen type (see below) |
| F12 | Cycle draw distance (1x / 2x / 4x) |

Changing an option with F3, F4, F6 to F9 or F12 switches to the enhanced profile, because the
authentic profile is fixed. These toggles are for quick A/B comparison and are not saved; put
permanent choices in `config.toml`.

### Settings

Open the settings screen with **Settings** in the pause menu (Start / Esc during a level, or in
the frontend: directly below *Continue*), with **Settings** at the end of the frontend's
*Extras* list, or with F10. It looks and sounds like the game's own menus. Up/Down choose a
row, Left/Right change the value, Cross/Enter selects, Triangle/Esc goes back; Left/Right on
the page tabs, L1/R1 or Tab switch pages. Everything applies at once and is saved to
`config.toml`; every page has *Reset to defaults*. Closing it returns to the pause menu /
Extras with *Settings* still selected. Rows the authentic profile makes inert are greyed out
with a hint.

- **Display**: window mode (Windowed / Borderless / Fullscreen = exclusive, with the refresh
  rate), resolution (*Auto* shows the live size; a fixed size resizes the window, picks the
  exclusive video mode, or sets the render size in borderless), VSync, frame cap
  (Off/30/60/120/144/240 or a custom value from the file), profile, and in the enhanced profile
  widescreen, render scale, anti-aliasing, texture filter, perspective-correct textures, draw
  distance, fog and font (*Modern* / *Original*, see below); brightness (a final multiply) and the FPS counter (corner of your choice;
  rendered FPS, frame time and the 60 Hz simulation rate; works in menus, levels and videos).
- **Modern font**: the game's pixel fonts were made from Monotype's *Gill Sans Ultra Bold*.
  If that typeface is installed (`GILSANUB.TTF`, which comes with Windows / Microsoft Office),
  or copied into `<data>/fonts/` (or named by `F2_MODERN_FONT=<file>`), the game builds
  high-resolution versions of `bigfont` and `smallfont` on start
  (`game/textures/font/*.f2mf`, beside the bitmaps; `f2-fontgen [RENDITION]` does the same by
  hand), with the same layout and the original outline / shadow style, sharp at any
  resolution. The typeface is never shipped with the port. Console packages built with
  `--data` include the generated files. The story-book typewriter font stays original.
- **Audio**: master, music and effects volume (music/effects are the same values as the game's
  Sound Options), mute while the window is in the background.
- **Controls**: *Keyboard mapping* and *Controller mapping* pages (below), stick dead zone,
  stick as d-pad (4-way / raw), button layout (positional / Nintendo labels), rumble on/off and
  strength, hop input buffer (enhanced profile).
- **Gameplay**: assists (infinite lives, invulnerability, enemy speed), FMV skipping, intro
  logo skip, speedrun timer, save slot (Left/Right choose, Cross loads it and restarts at the
  title), "Save now", auto-save, the tube softlock fix (all but the save slot apply in the
  enhanced profile only, see [enhancements.md](enhancements.md)), and the text language (only
  languages present in your imported data; new text switches at once, menus already on screen
  change when next opened or at the next level).

The level select (practice any story chapter or level) is on the advanced screen (Shift+F10,
Tab to *Level select*; also `--practice WORLD LEVEL` / `--story CHAPTER`).

### Window modes and Alt+Enter

Alt+Enter and F11 switch between windowed and your *fullscreen type*: borderless fullscreen by
default; once you choose *Fullscreen* (exclusive) in Display settings, the toggle goes windowed
<-> exclusive (choosing *Borderless* again switches it back). Enter is hidden from the game
while Alt is held, so Alt+Enter never selects a menu item. The Display page always shows the
real state (also after Alt+Enter, or when the system takes the game out of fullscreen). The
window mode, fullscreen type, resolution, refresh rate and the windowed size and position are
remembered.

### Button mapping (Controls page)

*Keyboard mapping* lists every action (directions, super hop / select, tongue, croak, back,
pause, L1/R1/L2/R2, select) with its keys for the chosen player (1-4). Cross/Enter on an action
waits for a new key (Esc cancels) that replaces its keys; Insert adds an extra key; Square/Delete
clears the action; Circle resets it to the PC default; *Reset all to defaults* restores every
player. If the key is already used, the screen says by which action and offers *Swap* (that
action gets your old key), *Unbind other* or *Cancel*. *Controller mapping* is the same page for
pad buttons and stick directions. Changes apply at once and are saved to `config.toml`.

## Profiles: authentic vs enhanced

- **authentic**: retail look and behaviour. Vert- projection (4:3 content stretched to the
  window), stretched overlays, no anti-aliasing, bilinear filtering without mipmaps, affine
  texturing, retail far clip, no fog. This is what parity tests compare against the original.
- **enhanced** (default): Hor+ widescreen with an anchored HUD, 4x MSAA, anisotropic filtering,
  perspective-correct texturing, hop input buffering (6 frames) and interpolated rendering for
  high refresh rates. Bug fixes are planned here (see [enhancements.md](enhancements.md) for
  the backlog and what is done).

The simulation always runs at a fixed 60 Hz in both profiles.

## Configuration

`config.toml` lives in the config directory: `$F2_CONFIG_DIR`, else the install folder of a
portable install (the release zips), else `~/.config/frogger2-rs/`
(`$XDG_CONFIG_HOME` honoured) on Linux, `%APPDATA%\frogger2-rs\` on Windows,
`~/Library/Application Support/frogger2-rs/` on macOS. A missing file or key means the default.
Command-line options override the file for that run.

```toml
profile = "enhanced"        # "authentic" | "enhanced"
language = "us"             # us, uk, fr, de, it
# data_root = "/path"       # imported data root (default: platform data dir)
# rendition = "usa"         # which imported rendition (default: first installed)

[video]
width = 640                 # windowed size (remembered)
height = 480
window_mode = "windowed"    # "windowed" | "borderless" | "fullscreen" (exclusive)
fullscreen_type = "borderless"  # what Alt+Enter / F11 toggle to: "borderless" | "exclusive"
# resolution = [1920, 1080] # absent = auto; see the Display page
# refresh_mhz = 144000      # exclusive fullscreen refresh rate (absent = highest)
# window_pos = [100, 100]   # windowed position (remembered)
vsync = true
frame_cap = 0               # 0 = uncapped
brightness = 100            # percent
show_fps = "off"            # "off" | "top_right" | "top_left" | "bottom_right" | "bottom_left"

[display]                   # enhanced profile only; defaults shown
hor_plus = true
hud_anchor = true
max_aspect = 0.0            # e.g. 2.37 pillarboxes beyond 21:9; 0 = off
render_scale = 1.0          # 0.25..4
msaa = 4                    # 1 or 4
filter = "anisotropic"      # "nearest" | "linear" | "anisotropic"
anisotropy = 16             # 1..16
perspective_correct = true
draw_distance = 1.0         # 0.25..16
fog = false
modern_font = true          # false = the original pixel fonts

[audio]
master = 100                # 0..100
music = 100
sfx = 100
muted = false

[input]
deadzone = 0.5
hop_buffer_frames = 6       # enhanced profile only; 0 = off
rumble = true
rumble_scale = 1.0

[gameplay]                  # assists/fixes apply in the enhanced profile only
infinite_lives = false
invulnerable = false
enemy_speed = 100           # percent, 25..100
skip_fmv_any_key = true
skip_intro_logos = false
speedrun_timer = false
save_slot = 1               # 1..4
auto_save = true
fix_tube_softlock = true

[mods]
enabled = []                # mod ids in load order (later wins)
# dir = "/path"             # mods directory (default: platform data dir)

[launcher]                  # the launcher (see "Installing a release and the launcher")
start = "game"              # what frogger2 opens on start: "game" | "launcher"
version = ""                # installed release the launcher runs ("" = the folder's own build)
check_updates = true        # ask GitHub for newer releases when the launcher starts
# releases_url = "https://api.github.com/repos/NeilGraham/frogger2-rs/releases"  # or a mirror

[[input.bindings]]          # normally written by the Settings screen
player = 1
button = "CROSS"
inputs = ["key:RETURN", "pad:SOUTH"]
```

`[input] controllers` selects the device per player (default keyboard; pads are named
`joypad0`, ...). Let the game write the file once (it saves when you change a setting in Settings) to
see the exact value spellings. Display options are described further in
[BUILDING.md](BUILDING.md).

Environment variables: `F2_DATA_ROOT` (data location), `F2_CONFIG_DIR` (config/saves location),
`F2_WAYLAND=1` (allow native Wayland under WSLg), `F2_GAME_LOG` (print game log lines).

## Saves

The save file is `savegame.dat` in `saves/` next to `config.toml` (for example
`~/.config/frogger2-rs/saves/savegame.dat`); save slots 2 to 4 live in `saves/slotN/`. Override with `--save FILE`. It uses the original
game's save-block format. `--dev-unlock` starts with every level open without touching your save.
Headless runs never write saves.

## Multiplayer

- **Local**: the original 2 to 4 player modes, with split-screen, are ported (the `multi.c`
  logic). Each player uses their own keyboard block (table above) or gamepad. Developers can jump
  straight in with `frogger2 --multi WORLD LEVEL FROGS`.
- **Online**: `frogger2 --host [--port N] [--host-addr IP] --multi WORLD LEVEL PLAYERS` hosts a
  match and prints a join code; the other players run `frogger2 --join CODE`. Rollback netcode
  over UDP (`f2-net`); see [NETPLAY.md](NETPLAY.md) for ports, NAT and desync dumps. Online
  matches always run with authentic gameplay rules (the `[gameplay]` assists and fixes are
  ignored) so every peer simulates identically; display settings stay local.

## Mods

Mods are layered over the retail data (files, scripts, extra levels). Enable them in
`config.toml` (`[mods] enabled = ["id", ...]`) or per run with `frogger2 --mods a,b`
(`--mods ""` = none) and `--mods-dir DIR`. See [modding.md](modding.md) for the mod format and
`f2-modkit` in [TOOLS.md](TOOLS.md) to create and check mods.
`frogger2 --editor --level W L [--mod ID]` opens the level editor and saves the edited level as
a mod (modding.md, "Level editor"); `--mod ID` is an alias of `--mods` that can be repeated.

## Troubleshooting

- **No game data / nothing starts**: run `f2-import --list`; pass `--data <rendition dir>`
  or set `F2_DATA_ROOT`.
- **Black window or crash on start**: needs a Vulkan (or GL) driver. On Linux install your GPU's
  Vulkan driver or `mesa-vulkan-drivers`.
- **No GPU / llvmpipe (software rendering)**: works but slowly. Force it with
  `WGPU_BACKEND=gl LIBGL_ALWAYS_SOFTWARE=1`, and lower the load with `--profile authentic`,
  `--msaa 1`, `--size 640x480`.
- **WSL2**: use WSLg. The game removes `WAYLAND_DISPLAY` and uses X11 (XWayland) automatically
  on WSL because native Wayland windows crash the WSLg compositor; set `F2_WAYLAND=1` to opt out.
  Keep the checkout and data on the Linux filesystem (not `/mnt/c`).
- **Wayland (native Linux)**: if the window misbehaves, force X11 with
  `env -u WAYLAND_DISPLAY frogger` or `WINIT_UNIX_BACKEND=x11`.
- **No sound**: building needs `libasound2-dev`. Under WSL, route ALSA to PulseAudio
  (`libasound2-plugins` and a `~/.asoundrc` with `pcm.!default { type pulse }`, plus
  `PULSE_SERVER=unix:/mnt/wslg/PulseServer`); details in [BUILDING.md](BUILDING.md). `--mute`
  disables audio output.
- **Gamepad not detected**: on Linux make sure your user can read `/dev/input/event*` (udev
  rules); press a button after plugging in. Assign devices in `[input] controllers`.
- **Controls got messed up**: Settings -> Controls -> Keyboard mapping -> *Reset all to defaults*, or delete the
  `[[input.bindings]]` entries from `config.toml`.

## FAQ

**Does it include the game?** No. Import from your own disc or install.

**Which versions work?** The USA retail release is the reference (`usa` rendition); a European
rendition (`eu`) is supported by the importer when present. Check `f2-import --list`.

**Is it identical to the original?** The simulation is a line-by-line port checked against the
retail exe with deterministic trace comparison (`f2-parity`, [TOOLS.md](TOOLS.md)); the
authentic profile keeps retail rendering. Parity is still being widened, see [PLAN.md](PLAN.md).

**Can I play at 144 Hz / ultrawide?** Yes in the enhanced profile: the sim stays at 60 Hz and
rendering is interpolated (`video.frame_cap = 0` uncaps); Hor+ and `display.max_aspect` handle
ultrawide.

**Will my saves from the original game work?** Not tested; do not assume compatibility.

**Where are original-game bugs documented?** In [research/known-bugs.md](../research/known-bugs.md).
