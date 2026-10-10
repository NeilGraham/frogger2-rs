# User guide

## What this is (and is not)

`frogger2-rs` is a Rust port of *Frogger 2: Swampy's Revenge* (PC, 2000), built from the preserved
original source. It runs on modern systems (Linux, Windows, macOS) with a widescreen-capable
renderer, gamepad support and rebindable controls.

**It contains no game data.** You need your own copy of the game (a disc image such as a Redump
dump, a physical disc's files, or an existing install folder). The importer extracts what the port
needs into a local data directory. Nothing copyrighted is ever distributed with this project.

Status: in development; see the Status section of [PLAN.md](../PLAN.md). Some menus/options
screens and FMV flow are still being finished, so expect rough edges. Online play (`--host` /
`--join`) and mods (`--mods`) are available (see Multiplayer and Mods).

## Quick start

1. Build (see [BUILDING.md](BUILDING.md)):

   ```sh
   cargo build --release -p frogger -p f2-import -p f2-viewer
   ```

2. Import your copy. The argument can be a `.zip`, `.cue`/`.bin`, `.iso`, a disc folder or an
   install folder:

   ```sh
   target/release/f2-import "/path/to/Frogger 2 - Swampy's Revenge (USA).zip"
   target/release/f2-import --list                # show imported renditions
   target/release/f2-import --verify-installed    # check the imported files
   ```

   Data goes to `$F2_DATA_ROOT`, else the platform data dir
   (`~/.local/share/frogger2-rs/data`, `%APPDATA%\frogger2-rs\data`,
   `~/Library/Application Support/frogger2-rs/data`), in one folder per rendition (`usa`, `eu`).
   Use `--data-root DIR` to import somewhere else. See [TOOLS.md](TOOLS.md) for all importer
   options (`--flac`, `--no-music`, `--as`, `--verify`).

3. Run:

   ```sh
   target/release/frogger                          # normal start: logos, title, frontend
   target/release/frogger --data game-data/usa     # use a specific imported rendition
   ```

   Useful options: `--fullscreen`, `--size 1920x1080`, `--mute`, `--profile authentic|enhanced`.
   `frogger --help` prints the full list (including developer shortcuts such as
   `--level WORLD LEVEL`, `--multi WORLD LEVEL FROGS`, `--demo WORLD LEVEL`, `--video NAME`).

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
the clip. Esc is the game's Start/Triangle (pause/back) key; the host also treats Esc outside
videos as a quit request, so use F10 to move it if that bothers you.

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
| F10 | Settings (pauses the simulation); Tab switches page: Controls, Gameplay, Level select |
| F11 | Toggle fullscreen |
| F12 | Cycle draw distance (1x / 2x / 4x) |

Changing an option with F3, F4, F6 to F9 or F12 switches to the enhanced profile, because the
authentic profile is fixed. These toggles are for quick A/B comparison and are not saved; put
permanent choices in `config.toml`.

### Settings (F10)

F10 opens the settings screen; Tab cycles its pages:

- **Controls**: rebinding (below).
- **Gameplay**: assists (infinite lives, invulnerability, enemy speed), FMV skipping, intro logo
  skip, speedrun timer, save slot (1 to 4, Enter loads it, "Save now"), auto-save, the tube
  softlock fix. Assists and fixes only apply in the enhanced profile; see
  [enhancements.md](enhancements.md).
- **Level select**: start any story chapter or level for practice through a normal game session
  (also `--practice WORLD LEVEL` / `--story CHAPTER`).

Changes are saved to `config.toml` (`[gameplay]`).

### Rebinding (Controls page)

Up/Down select an action, Enter rebinds (press a key, pad button or stick direction; Esc
cancels), A adds an extra binding, Backspace resets that action, R resets everything. Conflicts
are moved to the new action and reported. Changes are saved to `config.toml`.

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

`config.toml` lives in the config directory: `$F2_CONFIG_DIR`, else `~/.config/frogger2-rs/`
(`$XDG_CONFIG_HOME` honoured) on Linux, `%APPDATA%\frogger2-rs\` on Windows,
`~/Library/Application Support/frogger2-rs/` on macOS. A missing file or key means the default.
Command-line options override the file for that run.

```toml
profile = "enhanced"        # "authentic" | "enhanced"
language = "us"             # us, uk, fr, de, it
# data_root = "/path"       # imported data root (default: platform data dir)
# rendition = "usa"         # which imported rendition (default: first installed)

[video]
width = 640
height = 480
fullscreen = false
vsync = true
frame_cap = 0               # 0 = uncapped

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

[[input.bindings]]          # normally written by the F10 screen
player = 1
button = "CROSS"
inputs = ["key:RETURN", "pad:SOUTH"]
```

`[input] controllers` selects the device per player (default keyboard; pads are named
`joypad0`, ...). Let the game write the file once (it saves when you change a setting in F10) to
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
  straight in with `frogger --multi WORLD LEVEL FROGS`.
- **Online**: `frogger --host [--port N] [--host-addr IP] --multi WORLD LEVEL PLAYERS` hosts a
  match and prints a join code; the other players run `frogger --join CODE`. Rollback netcode
  over UDP (`f2-net`); see [NETPLAY.md](NETPLAY.md) for ports, NAT and desync dumps. Online
  matches always run with authentic gameplay rules (the `[gameplay]` assists and fixes are
  ignored) so every peer simulates identically; display settings stay local.

## Mods

Mods are layered over the retail data (files, scripts, extra levels). Enable them in
`config.toml` (`[mods] enabled = ["id", ...]`) or per run with `frogger --mods a,b`
(`--mods ""` = none) and `--mods-dir DIR`. See [modding.md](modding.md) for the mod format and
`f2-modkit` in [TOOLS.md](TOOLS.md) to create and check mods.
`frogger --editor --level W L [--mod ID]` opens the level editor and saves the edited level as
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
- **Controls got messed up**: press R in the F10 screen to reset all bindings, or delete the
  `[[input.bindings]]` entries from `config.toml`.

## FAQ

**Does it include the game?** No. Import from your own disc or install.

**Which versions work?** The USA retail release is the reference (`usa` rendition); a European
rendition (`eu`) is supported by the importer when present. Check `f2-import --list`.

**Is it identical to the original?** The simulation is a line-by-line port checked against the
retail exe with deterministic trace comparison (`f2-parity`, [TOOLS.md](TOOLS.md)); the
authentic profile keeps retail rendering. Parity is still being widened, see PLAN.md.

**Can I play at 144 Hz / ultrawide?** Yes in the enhanced profile: the sim stays at 60 Hz and
rendering is interpolated (`video.frame_cap = 0` uncaps); Hor+ and `display.max_aspect` handle
ultrawide.

**Will my saves from the original game work?** Not tested; do not assume compatibility.

**Where are original-game bugs documented?** In [research/known-bugs.md](../research/known-bugs.md).
