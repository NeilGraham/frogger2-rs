# Building and running

Requires Rust 1.88+ (see `rust-version`). No game data is needed to build or run the tests; tests
that need it skip themselves when `F2_GAME_DIR` / `F2_DATA_DIR` are unset.

Platform overview, support matrix and future targets: [PLATFORMS.md](PLATFORMS.md).

## Linux

```sh
sudo apt-get install -y libasound2-dev libudev-dev   # Debian/Ubuntu
sudo dnf install -y alsa-lib-devel systemd-devel     # Fedora
sudo pacman -S --needed alsa-lib systemd-libs        # Arch
cargo build --release -p frogger -p f2-import -p f2-viewer
```

Binaries land in `target/release/`. At run time you need a Vulkan or OpenGL driver (Mesa is fine)
and a Wayland or X11 session: winit uses Wayland natively when `WAYLAND_DISPLAY` is set (unset it
to force X11; the XWayland fallback is automatic only under WSL). Audio goes through ALSA, which
PipeWire/PulseAudio desktops route to the sound server (`pipewire-alsa` or the ALSA pulse
plugin). Gamepads need read access to `/dev/input/event*` (default on desktop distros).

## Windows (MSVC)

Install Rust with the `x86_64-pc-windows-msvc` toolchain and the Visual Studio Build Tools
("Desktop development with C++"), then the same `cargo build --release ...` command.

Native `cargo test` / `cargo run` on Windows: `.cargo/config.toml` sets the Linux memory-cap
runner `tools/memcap.sh`, which a native Windows cargo cannot execute. Remove that runner in a
local, uncommitted edit (CI deletes the file on Windows), or set a target-specific runner, which
takes precedence over the `cfg(all())` one (`CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER`).
`cargo build` is unaffected.

From WSL you can cross-build with the Windows toolchain: `tools/package/build-windows.sh`. It mirrors
sources to `C:\frogger2-rs-dev\build\<checkout>` (cmd.exe rejects UNC working directories and MSVC linking over
9P is slow), builds `frogger2`, `f2-import`, `f2-viewer` for `x86_64-pc-windows-msvc`, and copies the
exes to `target/package/win-bin/`. Override with `F2_WIN_CARGO` / `F2_BUILD_ROOT`.
`crates/f2-upscale` compiles `native/f2_upscale.cpp` (DLSS / FSR glue, docs/UPSCALING.md)
with the MSVC compiler through `cc` on Windows MSVC targets; elsewhere it is a stub and needs no
C++ compiler. The DLSS / FSR libraries are loaded at run time, never linked.
`crates/f2fix` is a separate i686 workspace built by `tools/run/build_f2fix.sh`; it is not packaged.

## macOS

`cargo build --release ...` with the Xcode command line tools (`xcode-select --install`).
`tools/package/package.sh macos` produces a `.app` bundle (`F2_MACOS_UNIVERSAL=1` for an arm64 +
x86_64 universal build); signing and notarisation: `tools/package/README-macos.md`. The macOS
build is compile-checked from Linux and built/tested by CI on `macos-14`, but has not been run
on a Mac yet.

Checking macOS from Linux (no SDK needed, nothing is linked):

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo check --workspace --all-targets --no-default-features --target aarch64-apple-darwin
```

`--no-default-features` turns off the binaries' `audio-device` feature (cpal), because
`coreaudio-sys` runs bindgen against the macOS SDK headers in its build script.

## Android

`tools/android/build-apk.sh [--debug] [--abis arm64-v8a,x86_64]` builds `target/android/frogger2-rs.apk`
(needs the Android SDK/NDK, `cargo-ndk`, a JDK). Setup, data import and emulator testing:
[ANDROID.md](ANDROID.md).

## PS4 / PS5 homebrew

`tools/ps4/build-pkg.sh [--data game-data/usa] [--icon IMAGE]` builds a PS4 fPKG (also
installs on a jailbroken PS5). Toolchain setup and details: [tools/ps4/README.md](../tools/ps4/README.md).
Its renderers can be checked on the host (GLES2 back end: `--example gles_shot`, see the PS4 README):
`cargo run --release --manifest-path crates/frogger-ps4/Cargo.toml --example soft_shot -- game-data/usa out.png`
(its own workspace, see the root `Cargo.toml`).

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
width = 640               # windowed size (any resolution/aspect; resizable; remembered)
height = 480
window_mode = "windowed"  # "windowed" | "borderless" | "fullscreen" (exclusive video mode)
fullscreen_type = "borderless"  # Alt+Enter / F11 toggle windowed <-> this ("borderless" | "exclusive")
# resolution = [1920, 1080]  # absent = auto. Windowed: inner size; exclusive: video mode;
                          # borderless: render-target size (scaled to the screen)
# refresh_mhz = 144000    # exclusive fullscreen refresh rate (absent = highest for the size)
# window_pos = [100, 100] # windowed outer position (remembered)
fullscreen = false        # legacy: true without window_mode starts in fullscreen_type
vsync = true
frame_cap = 0             # fps cap, 0 = uncapped (sim is fixed 60 Hz, rendering interpolated)
brightness = 100          # percent, final-pass multiply (100 = no extra pass)
show_fps = "off"          # FPS counter corner: off | top_right | top_left | bottom_right | bottom_left

[audio]
master = 100              # 0..100
music = 100               # 0..100 (the game's Sound Options write these too)
sfx = 100
muted = false             # null audio output
mute_unfocused = false    # silence while the window is in the background

# All of the above (and [display], [gameplay], [input]) are edited in game: Settings in the
# pause menu / Extras, or F10.

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

# Gameplay / QoL (enhanced profile only, except save_slot). Edited in game: Settings -> Gameplay.
[gameplay]
infinite_lives = false    # assist: dying never costs a life
invulnerable = false      # assist: enemies can't hurt the frog (hazard tiles / falls still kill)
enemy_speed = 100         # assist: enemy + platform speed in percent, 25..100
skip_fmv_any_key = true   # any key / pad button skips an FMV (authentic: Enter/Esc)
skip_intro_logos = false  # skip the Hasbro / Blitz logo FMVs
speedrun_timer = false    # IGT + RTA on screen, splits appended to <config>/saves/splits.csv
save_slot = 1             # 1..4: saves/savegame.dat, saves/slotN/savegame.dat
auto_save = true          # off: the game's own saves are not written; use "Save now"
fix_tube_softlock = true  # known bug #11 fix
```

The in-game Settings screen (pause menu / Extras "Settings", or F10) edits all of this: Display,
Audio, Controls (keyboard / controller mapping), Gameplay. The game-side "Settings" entries are
behind `GameOptions::settings_menu` (off in f2-game, so parity / demos / netplay are unchanged;
the `frogger` host turns it on offline). Shift+F10 opens the older advanced screen: Controls,
Gameplay and Level select (every story chapter and single-player level as practice, regardless
of the save; also `--story N` / `--practice W L` on the command line). Headless screenshots:
`--screenshot-page display|audio|controls|gameplay[:ROW]`, `--screenshot-keymap
normal|capture|conflict`, `--show-fps`. Platform check of the window modes: `F2_WINDOW_TEST=1`
makes the windowed game step through borderless, exclusive fullscreen, Alt+Enter both ways,
windowed 1024x768 / 800x600, exclusive 1280x720, borderless with a 640x480 render size and
back to windowed (3 s each), logging what was requested and what the window became
(`frogger: window now ...`), then exit. Point `F2_CONFIG_DIR` at a scratch directory: the
steps are saved like settings changes.

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
tools/package/package.sh linux     # target/package/*.tar.gz (+ AppImage if appimagetool is found)
tools/package/package.sh windows   # target/package/*.zip
tools/package/package.sh macos     # on macOS: .app in a tar.gz
```

Each archive carries `NOTICE`, the two license texts (`licenses/`) and no game data.
`$CARGO_TARGET_DIR` is honoured with `F2_SKIP_BUILD=1`.

The Windows zip and the Linux tar.gz are **portable installs**:

```
frogger2-rs-<v>-<os>-<arch>/
  launcher(.exe)           the launcher (crates/frogger/src/bin/launcher.rs, f2-launcher)
  engine/                  frogger2(.exe), f2-import, f2-viewer
  portable.txt             marker: data, config.toml, saves/, mods/ live in this folder
  NOTICE  README.md  licenses/ ...
  (frogger2(.exe)          made by the launcher after the import, see below)
  (data/ versions/ mods/ saves/ config.toml: created at run time)
```

`portable_root()` (f2-platform / f2-import) finds the folder with `portable.txt` from the running
exe: its own folder, the `engine/` parent, `versions/<tag>[/engine]`, or beside a macOS `.app`.
The launcher writes `frogger2` into the install folder once the game is imported
(`frogger::setup::ensure_game_exe`): on Windows a copy of `engine/frogger2.exe` whose icon and
version strings are the game's own (the Windows export's `editpe` rebranding, with the artwork cut
from the player's imported data), on Linux/macOS a link to the engine. The macOS zip holds the
`.app` (with `launcher` inside `Contents/MacOS`) next to `portable.txt`.

- **AppImage**: needs [appimagetool](https://github.com/AppImage/appimagetool/releases) on `PATH`
  or in `F2_APPIMAGETOOL`. It downloads the AppImage runtime while building; offline, pass a
  downloaded [type2 runtime](https://github.com/AppImage/type2-runtime/releases) as
  `F2_APPIMAGE_RUNTIME=/path/runtime-x86_64`. Without FUSE (containers, WSL) set
  `APPIMAGE_EXTRACT_AND_RUN=1`. `Frogger2rs-*.AppImage f2-import <disc>` runs the importer.
- **Flatpak**: `packaging/flatpak/io.github.NeilGraham.Frogger2rs.yml` (+ AppStream metainfo); generate
  `cargo-sources.json` with flatpak-cargo-generator first (steps in the manifest header).
- **Desktop entry / icon**: `packaging/frogger2-rs.desktop`, `packaging/icon/` (flat placeholder: SVG, PNG sizes, `.ico`; Android launcher PNGs in `packaging/android/res`)
  (placeholder art, not from the game).

`F2_PKG_VERSION=0.1.0` names the archives with that version instead of `<version>-<commit>`;
`F2_MACOS_TARGET=x86_64-apple-darwin` packages one non-host macOS architecture. Release
builds are made by CI: see the next section.

## Releases / CI

**CI** (`.github/workflows/ci.yml`, every push and pull request; no game data): `lint` (fmt +
clippy), `test` (build + test on ubuntu, windows, macos-14), `cross-check` (macOS targets and the
no_std core — `f2-math`, `f2-formats`, `f2-game`, `f2-engine` without default features — from
Linux), `nostd-mips` (the same core on big-endian MIPS with nightly `-Zbuild-std=core,alloc`,
for the N64 port), `android-check` (`cargo ndk check -p frogger-android` for arm64).

**Release** (`.github/workflows/release.yml`). To cut one:

1. Set `[workspace.package] version` in `Cargo.toml` (e.g. `0.2.0`), commit, push.
2. `git tag v0.2.0 && git push origin v0.2.0`. The workflow fails at once if the tag and the
   Cargo version differ (the desktop app downloads the console runtimes of its own version).
3. When every job passes, the `release` job creates the GitHub release for the tag with all
   assets, `SHA256SUMS.txt` and notes saying that no game data is included and that exported
   builds must not be shared. A version with a `-` (`0.2.0-rc.1`) is marked pre-release.
   Re-running a failed workflow re-uploads (`--clobber`) to the existing release.

Running the workflow by hand (Actions → release → Run workflow, any branch) is a **dry run**:
the same builds at the Cargo version, uploaded as workflow artifacts (`release-*`), no release.

| Job | Runner | Asset |
|---|---|---|
| `windows` (x86_64, aarch64) | windows-latest (MSVC; ARM64 cross-built, not run in CI) | `frogger2-rs-<v>-windows-<arch>.zip` (exe icon via `crates/frogger/build.rs`) |
| `macos` | macos-14 (x86_64 cross-built, `lipo`'d) | `frogger2-rs-<v>-macos-universal.zip` (ad-hoc signed universal `.app`) |
| `linux` (x86_64, aarch64) | ubuntu-22.04 / ubuntu-22.04-arm (glibc 2.35 baseline) | `Frogger2rs-<v>-<arch>.AppImage`, `frogger2-rs-<v>-linux-<arch>.tar.gz` |
| `android` | ubuntu-24.04 | `frogger2-rs-<v>-android.apk` (shell app, `tools/android/build-apk.sh`) |
| `runtime` (ps4, ps5, n64) | ubuntu-24.04 | `runtime-<target>-<v>.zip` from `tools/runtime/build-<target>.sh OUT_DIR VERSION` |
| `release` | ubuntu-latest | checks every asset is there and has no game data, publishes |

Toolchains are pinned in `.github/workflows/toolchains/*.sh` (they install into `~/opt`, the
build scripts' defaults, and are cached between runs; run one with another `HOME` to try it
locally): OpenOrbis PS4 toolchain v0.5.4 + libssl 1.1.1f (PkgTool.Core); ps5-payload-sdk
v0.43, PS5_Vulkan `252bf91`, bc7enc_rdo `b943862`; libdragon `39d0d60` with the 3P/3A patches
on libdragon's GCC 16.2 toolchain; Rust `nightly-2026-10-11` (what `cargo +nightly`
resolves to); Android cmdline-tools 12.0, platform 35, build-tools 35.0.0, NDK
27.2.12479018, cargo-ndk 4.1.2, JDK 17; appimagetool 1.9.0 (x86_64 and aarch64). Downloads are checked against
their SHA-256. libdragon publishes its toolchain only as a rolling release: when it is
replaced, `toolchains/n64.sh` fails on the hash; re-test and update the pin.

**Secrets** (Settings → Secrets and variables → Actions), all optional:

| Secret | Use |
|---|---|
| `ANDROID_KEYSTORE_B64` | release keystore for the APK, base64 (`base64 -w0 release.keystore`). One key in it; its key password the same as the store password. |
| `ANDROID_KEYSTORE_PASS` | that password (`F2_ANDROID_KS_PASS`) |

Without them the APK is signed with a debug key made for that run, so an installed copy cannot
be updated in place by the next release (Android refuses a different signature): set them before
the first public release and keep the keystore safe; it can never change afterwards.
Create one with
`keytool -genkeypair -keystore release.keystore -alias frogger2-rs -keyalg RSA -keysize 4096 -validity 10000`.
The release itself needs no secret (the workflow's `GITHUB_TOKEN` with `contents: write`).

## Memory caps

`.cargo/config.toml` makes `tools/memcap.sh` the cargo runner: every `cargo test`, `run` and
`bench` binary runs in its own cgroup capped at 8 GB (no swap), so a runaway test cannot exhaust
the machine. Over the cap only that binary is killed (exit 137 / SIGKILL). Linux only (macOS has
no cgroups). For a legitimately big job use `MEMCAP=16G cargo test ...`; wrap other heavy
commands (Python oracles, trace dumps, emulators) as `tools/memcap.sh <cmd>`. An exit-137 test
usually means a missing step budget, not a reason to raise the cap.

## WSL2 notes

- Prefer WSLg. Under WSL the game and viewer drop `WAYLAND_DISPLAY` themselves and use X11
  (XWayland), because winit's native Wayland client crashes WSLg's Weston; `F2_WAYLAND=1`
  (viewer: `F2_VIEWER_WAYLAND=1`) opts back in.
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

## Input configuration

`config.toml` `[input]` (see `crates/f2-platform/src/bindings.rs`):

- `controllers = ["joypad0", ...]` per-player device (default keyboard); `deadzone` (0.5 = retail).
- `[[input.bindings]]` (`player` 1-4, `button` e.g. `CROSS`, `inputs = ["key:RETURN", "pad:SOUTH", "axis:LEFTY+"]`)
  gives several keys/buttons/stick directions per action. Absent = the retail keymap plus the
  default pad mapping; the legacy `keymap` list still loads.
- `hop_buffer_frames` (enhanced profile only, default 6, 0 = off), `rumble`, `rumble_scale`.
- In game, Settings -> Controls -> Keyboard / Controller mapping (simulation paused): Cross/Enter
  rebinds (press a key, or a pad button / stick direction on the controller page; Esc cancels),
  Insert adds an extra key, Square/Delete clears, Circle resets the action, "Reset all" restores
  the defaults; a conflict offers Swap / Unbind other / Cancel. Changes save to `config.toml`.
  (Shift+F10 -> Controls is the older raw page.) Pad hot-plug auto-assigns controllers; rumble follows `rumble_scale`.
  `frogger2 --headless --level 0 0 --screenshot-rebind --screenshot out.png` renders the screen.
