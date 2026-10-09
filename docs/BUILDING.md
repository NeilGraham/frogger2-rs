# Building and running

Requires Rust 1.88+ (see `rust-version`). No game data is needed to build or run the tests; tests
that need it skip themselves when `F2_GAME_DIR` / `F2_DATA_DIR` are unset.

## Linux

```sh
sudo apt-get install -y libasound2-dev libudev-dev   # + mesa-vulkan-drivers / a Vulkan or GL driver
cargo build --release -p frogger -p f2-import -p f2-viewer
```

Binaries land in `target/release/`.

## Windows (MSVC)

Install Rust with the `x86_64-pc-windows-msvc` toolchain and the Visual Studio Build Tools
("Desktop development with C++"), then the same `cargo build --release ...` command.

From WSL you can cross-build with the Windows toolchain: `tools/package/build-windows.sh`. It mirrors
sources to `C:\f2-build\<checkout>` (cmd.exe rejects UNC working directories and MSVC linking over
9P is slow), builds `frogger`, `f2-import`, `f2-viewer` for `x86_64-pc-windows-msvc`, and copies the
exes to `target/package/win-bin/`. Override with `F2_WIN_CARGO` / `F2_BUILD_ROOT`.
`crates/f2fix` is a separate i686 workspace built by `tools/run/build_f2fix.sh`; it is not packaged.

## macOS

`cargo build --release ...` with the Xcode command line tools. `tools/package/package.sh macos`
produces an untested `.app` bundle.

## Importing your game data

```sh
f2-import "/path/to/disc-image.zip|.cue|.iso|disc folder|install folder"
f2-import --list
```

Data goes to `$F2_DATA_ROOT` or the platform data dir (`~/.local/share/frogger2-rs/data`,
`%APPDATA%\frogger2-rs\data`, `~/Library/Application Support/frogger2-rs/data`). Use
`--data-root game-data` to keep it in the checkout (gitignored). Then `cargo run --release -p frogger`.

## Configuration (`config.toml`)

`config.toml` lives in the platform config dir (`~/.config/frogger2-rs/`,
`%APPDATA%\frogger2-rs\`, …); a missing file or key means the default. Display part:

```toml
profile = "enhanced"      # "authentic" = retail look (parity screenshots), "enhanced" = default

[video]
width = 640               # window size (any resolution/aspect; resizable)
height = 480
fullscreen = false        # borderless fullscreen; F11 toggles
vsync = true
frame_cap = 0             # fps cap, 0 = uncapped (sim is fixed 60 Hz, rendering interpolated)

# Overrides of the enhanced profile only (authentic is fixed). Values shown are the defaults.
[display]
hor_plus = true           # Hor+ widescreen: keep the 4:3 vertical FOV, widen horizontally
hud_anchor = true         # overlays re-anchored to the screen edges; full-screen ones pillarboxed
max_aspect = 0.0          # ultrawide clamp, e.g. 2.37 (21:9): wider screens get a pillarboxed 3D view; 0 = off
render_scale = 1.0        # internal resolution multiplier (2.0 = 4x supersampling), 0.25..4
msaa = 4                  # 1 or 4 samples
filter = "anisotropic"    # "nearest" (PSX look) | "linear" (retail bilinear) | "anisotropic" (trilinear mips)
anisotropy = 16           # 1..16, used by filter = "anisotropic"
perspective_correct = true  # retail: affine texturing
draw_distance = 1.0       # far-clip multiplier (0.25..16)
fog = false               # linear fog to the level's clear colour, ending at the far clip
```

Authentic profile = Vert- projection (`FOV = 450` scaled by `width/640`), overlays stretched
over the whole screen, no AA, bilinear without mipmaps, affine texturing, retail far clip, fog
off. It renders through the original single-pass path and is pixel-identical to the
pre-enhancement renderer (`cargo test -p frogger authentic_profile_is_pixel_identical`).
Colour is 32-bit (RGBA8) throughout in both profiles.

Command-line overrides: `--profile authentic|enhanced`, `--msaa N`, `--render-scale S`,
`--size WxH`; `--hud-probe` draws sample overlays at the 4096-space corners/centre to check
anchoring. In game: F5 profile, F3 perspective-correct, F4 render scale (1/1.5/2), F6 MSAA,
F7 texture filter, F8 widescreen (Hor+ + HUD anchoring), F9 fog, F12 draw distance (1/2/4×).
Changing an option with F3–F12 switches to the enhanced profile. Example parity/enhanced shots:

```sh
cargo run -p frogger -- --headless --frames 120 --level 0 0 --profile authentic --screenshot a.png
cargo run -p frogger -- --headless --frames 120 --level 0 0 --size 2560x1080 --profile enhanced --msaa 4 --screenshot e.png
```

## Packaging

```sh
tools/package/package.sh linux     # target/package/*.tar.gz (+ AppImage if appimagetool is on PATH)
tools/package/package.sh windows   # target/package/*.zip
tools/package/package.sh macos     # on macOS
```

Each archive carries `NOTICE`, the four license texts (`licenses/`) and no game data. A Flatpak draft
is in `packaging/flatpak/`. The CI `release` job (manual dispatch) runs these scripts.

## WSL2 notes

- Prefer WSLg. If Wayland misbehaves, force X11 by unsetting the Wayland display:
  `env -u WAYLAND_DISPLAY frogger` (or `WINIT_UNIX_BACKEND=x11`).
- No GPU Vulkan under WSL? Use the software rasteriser: `sudo apt install mesa-vulkan-drivers`
  (lavapipe) or force GL with `WGPU_BACKEND=gl LIBGL_ALWAYS_SOFTWARE=1` (llvmpipe). Slow but works.
- Audio: WSLg exposes PulseAudio, not an ALSA card. Route ALSA through Pulse with
  `sudo apt install libasound2-plugins` and `~/.asoundrc`:

  ```
  pcm.!default { type pulse }
  ctl.!default { type pulse }
  ```

  Set `PULSE_SERVER=unix:/mnt/wslg/PulseServer` if it is not already exported.
- Keep the checkout on the Linux filesystem; builds on `/mnt/c` are slow.
