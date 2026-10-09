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
