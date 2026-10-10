# Platforms

Where the port runs today, how it is layered, and what each future target would need.
Build instructions: [BUILDING.md](BUILDING.md). Packaging: `tools/package/`, `packaging/`.

## 1. Layering

```
 frogger (bin)         main loop, CLI, settings, menus glue          per platform: thin
   ├─ f2-game          deterministic simulation (60 Hz fixed tick)   PORTABLE (pure Rust)
   │    └─ f2-math     20.12 fixed point, PSX trig, RNG, detfloat    PORTABLE, no_std today
   ├─ f2-formats       pure parsers of the retail data               portable (std today)
   ├─ f2-engine        wgpu renderer (MDX replacement)               needs a GPU backend
   ├─ f2-audio         mixer (portable) + output (cpal)              output backend per platform
   ├─ f2-bink          Bink 1 video/audio decoder (pure Rust)        portable; CPU-heavy
   ├─ f2-platform      winit window, gilrs pads, config, paths       per platform
   └─ f2-mods / DataSource / Vfs   all file access                   per platform storage
 f2-import / f2-media  disc/ISO/CAB import (pure Rust)               portable (std::fs)
```

- **Simulation** (`f2-game`, `f2-math`) has no window, GPU, audio, clock or file access: it
  consumes per-player pad bits (`TickInput`) and produces state + events. Gameplay is integer
  fixed point; the few float paths use `f2_math::detfloat` (libm), so results do not depend on
  the host C library. This is the part every target reuses unchanged, and parity traces
  (`docs/ORACLE.md`) prove it on any host that can run the tests.
- **Rendering** goes through wgpu (Vulkan, Metal, DX12, GLES/WebGL2, WebGPU). Required features
  are empty and limits are `downlevel_defaults()`, so any wgpu backend works. A console without
  a wgpu backend needs its own renderer behind the same draw interface (MDX-style poly lists:
  opaque, sorted translucent, additive, sprites, 2D overlays).
- **Audio**: `f2-audio`'s mixer produces PCM; the `AudioOutput` trait has a cpal backend (feature
  `cpal`, binaries' `audio-device` feature) and a null backend. A platform adds one
  `AudioOutput` impl.
- **Input**: everything is reduced to the original PSX pad bits (`padData`/`PAD_*`) in
  `f2-platform`; a new platform maps its pad API to that.
- **Files**: game code reads through `DataSource` / `f2_mods::Vfs` (case-insensitive, layered).
  A platform without a POSIX-like filesystem (cartridge, browser, console sandbox) provides a
  `DataSource` over its storage (ROM filesystem, IndexedDB/OPFS, save-data API).

## 2. Support matrix

| Platform | Status | Build | Window / GPU | Audio | Pads | Packaging |
|---|---|---|---|---|---|---|
| Windows x86_64 | **supported** (owner's main target) | native MSVC or WSL cross-build | winit / DX12, Vulkan | cpal WASAPI | gilrs (XInput, WGI) | zip (`package.sh windows`) |
| Linux x86_64 | **supported**, CI-tested | native | winit Wayland (native) or X11 / Vulkan, GL | cpal ALSA (PipeWire/Pulse via ALSA plugins) | gilrs (evdev + udev) | tar.gz, AppImage, Flatpak manifest |
| Linux aarch64 | expected to work, untested | native | same | same | same | tar.gz/AppImage (`uname -m`) |
| macOS arm64 / x86_64 | **compile-checked from Linux; CI builds + tests on macos-14**; not run on a Mac yet | native (Xcode CLT) | winit / Metal; Retina-aware | cpal CoreAudio | gilrs (IOKit) | `.app` (universal), ad-hoc signed |
| WSL2 (WSLg) | works (dev setup) | native Linux | X11 via XWayland (native Wayland crashes WSLg's Weston), llvmpipe/lavapipe | Pulse via ALSA plugin | usbipd only | — |
| Web, Android, iOS, Switch, PlayStation, N64 | not started | see §4 | | | | |

Verified for this document (2026-10-10): Linux release build, workspace test suite (no game
data, 63 test binaries green), windowed smoke run under WSLg (llvmpipe Vulkan, audio on),
AppImage built locally and run headless; `cargo check --workspace --all-targets
--no-default-features` for `aarch64-apple-darwin` and `x86_64-apple-darwin`; `f2-math` for
`thumbv7em-none-eabihf` (no_std).

## 3. Desktop notes

### Linux
- **Display**: winit picks Wayland when `WAYLAND_DISPLAY` is set, else X11. The WSL workaround
  (drop `WAYLAND_DISPLAY` so XWayland is used) only triggers when `/proc/version` says
  `microsoft` (`f2_platform::window::is_wsl`); `F2_WAYLAND=1` overrides it. Real Linux desktops
  use Wayland natively. `WINIT_UNIX_BACKEND` is not read by winit 0.30; unset `WAYLAND_DISPLAY`
  to force X11.
- **Audio**: cpal uses ALSA. On PipeWire/PulseAudio systems the default ALSA device is routed
  to the sound server by `pipewire-alsa` / `alsa-plugins-pulseaudio` (installed by default on
  mainstream distros). Build dependency: `libasound2-dev` (Debian/Ubuntu), `alsa-lib-devel` (Fedora).
- **Gamepads**: gilrs reads `/dev/input/event*` via udev (`libudev-dev` to build). The user needs
  read access to those devices (default with systemd-logind seats; Flatpak: `--device=all`
  or `--device=input`).
- **Paths (XDG)**: data `$XDG_DATA_HOME/frogger2-rs/data` (default `~/.local/share/...`), config,
  saves and mods `$XDG_CONFIG_HOME/frogger2-rs` (default `~/.config/...`); `F2_DATA_ROOT`,
  `F2_CONFIG_DIR`, `F2_MODS_DIR` override.
- **Case sensitivity**: imported data is written lowercase. All game reads go through
  `f2_mods::Vfs`, which matches every path component ASCII-case-insensitively (the retail game
  asks for `MAPS\SCRIPT-1-0.FEV`, `objects\Garden1.dll`, ...), including the layer directories
  themselves (a mod's `Game/` or `MUSIC/` works). `f2_game::assets::DirData` (tests/tools only)
  lowercases the request and therefore expects lowercase data (as imported).
- **Packaging**: `tools/package/package.sh linux` -> tar.gz and, with appimagetool, an AppImage
  (`<AppImage> f2-import ...` runs the importer). Flatpak: `packaging/flatpak/` (generate
  `cargo-sources.json` first, see the manifest header).

### macOS
- `cargo check` for both Apple targets passes from Linux with `--no-default-features` (drops the
  binaries' `audio-device` feature = cpal's CoreAudio backend, whose `coreaudio-sys` build script
  runs bindgen against the macOS SDK). Linking needs the SDK: only CI (`macos-14`) and a real Mac
  can link.
- Metal is wgpu's default backend there (no code change). The window size from `config.toml`
  is in points on macOS (`LogicalSize`), so a 640x480 window is not tiny on Retina; the surface
  uses physical pixels (`inner_size()`), so rendering is at full Retina resolution.
- Shortcuts (`f2-platform` window runner): Cmd+Q / Cmd+W quit, Cmd+F, Ctrl+Cmd+F, Cmd+Enter toggle
  fullscreen (delivered as F11); other Cmd+key presses never reach the game. winit provides the
  default app menu (About/Hide/Quit). F-keys may need Fn on Mac keyboards.
- Paths: `~/Library/Application Support/frogger2-rs/` (`data/`, `config.toml`, `saves/`, `mods/`).
- Bundle, signing, notarisation: `tools/package/README-macos.md`.

## 4. Future targets

General constraints:
- **Legal**: official console SDKs (Sony, Nintendo, Microsoft) are proprietary and under NDA;
  shipping on a console requires a publisher/developer agreement and, for this game, a licence
  from the rights holder. Everything below assumes **homebrew** toolchains on the owner's own
  hardware with the owner's own game data, the same model as the PC port. Homebrew on
  PS3/PS4/PS5/Switch needs custom/exploited firmware; on PS2 and N64 it runs from
  memory card / flash cart without modifying the console. No proprietary SDK code or keys may
  enter this repository.
- **Reusable as-is**: `f2-game` + `f2-math` (simulation), `f2-formats` (parsers), `f2-bink`
  (decoder, if the CPU can afford it), `f2-audio`'s mixer, the trace/parity tooling. Per target:
  renderer, audio output, input, storage, main loop, packaging.
- **no_std**: `f2-math` already builds `#![no_std]` (no allocator needed; CI checks it on
  `thumbv7em-none-eabihf`). `f2-game` still needs `std`; the blockers are listed in §5.
  Targets with a Rust `std` (Android, iOS, Web, Linux-like OSes) do not need this.

### PlayStation 2
- Hardware: Emotion Engine (MIPS R5900, ~295 MHz), 32 MB RAM, GS with 4 MB embedded VRAM, IOP
  (PS1 CPU) for I/O and SPU2 sound (2 MB). The EE FPU is single precision only and not IEEE
  compliant (no denormals/infinities, truncating rounding), so `f64` must be soft-float and the
  float render paths will not be bit-identical; gameplay (integer fixed point) is unaffected.
- Toolchain: PS2SDK (open source homebrew SDK, GCC). Rust has no upstream R5900 target with
  `std`; a custom target spec (MIPS R5900, soft `f64`) with `-Zbuild-std=core,alloc` and a
  PS2SDK-backed allocator/libc shim is the likely route, so `f2-game` would need no_std (§5).
- Data: the PC data (75 MB of game files, plus 256 MB music and 116 MB video) does not fit in
  RAM at once and the textures assume a D3D7 card. The **PSX version's assets** (the original
  `PSX/` platform tree in `frogger2-vss/teamSpirit/FroggerProject/PSX`) were built for 2 MB RAM +
  1 MB VRAM and are the realistic source; that means new parsers for the PSX formats in
  `f2-formats` and a GS renderer (no texture perspective issues on GS; 4 MB VRAM needs
  texture streaming per level). Note the PS2 already runs the PS1 game disc natively.
- Effort: high (new renderer, audio via SPU2/IOP modules, PSX asset path, memory budget work).

### PlayStation 3
- Hardware: Cell (PowerPC 64 PPU **big-endian** + 6 usable SPUs), 256 MB XDR + 256 MB VRAM, RSX
  GPU. Homebrew: PSL1GHT / ps3toolchain (GCC), requires CFW or HEN.
- Rust: `powerpc64-unknown-linux-gnu` exists, but GameOS is not Linux: custom target + no_std +
  alloc over PSL1GHT's newlib. All parsers use explicit little-endian reads (`from_le_bytes`),
  so big-endian hosts should parse correctly; this needs a test on a BE target (e.g.
  `cargo test --target powerpc64-unknown-linux-gnu` under QEMU user mode).
- RAM is ample for the PC data. Renderer: RSX through PSL1GHT's `rsx` library (a new backend).

### PlayStation 4 / PlayStation 5
- x86-64 (Jaguar / Zen 2), FreeBSD-derived OS. Official SDKs: NDA only. Homebrew: OpenOrbis PS4
  toolchain (clang/LLVM, jailbroken firmware); PS5 homebrew is limited to specific exploitable
  firmware versions with ELF loaders and little GPU access.
- Rust code generation is a non-issue (x86_64); the OS needs a custom target (no upstream `std`),
  so no_std + alloc over the homebrew libc. GPU APIs (GNM/AGC) are proprietary; homebrew GPU
  access is limited (OpenOrbis exposes the system's GLES2 library, PiGlet, on PS4). wgpu has no
  backend for these. Treat as research only; legal exposure is highest here.

### Nintendo 64 (owner's interest)
- Hardware: VR4300 (MIPS III, 93.75 MHz, 64-bit integer + FPU), 4 MB RDRAM (8 MB with the
  Expansion Pak), RCP: RSP microcode + RDP rasteriser with 4 KB TMEM, cartridge ROM up to 64 MB
  on period hardware (flash carts allow more).
- Toolchain: libdragon (open source, GCC, modern rdpq/OpenGL 1.1-style API) or the 1999-era
  libultra (proprietary). Rust on N64 is done with a custom `mips` target spec plus
  `-Zbuild-std=core,alloc`; so `f2-game` must be no_std (§5).
- History: `frogger2-vss/teamSpirit/FroggerII/N64/` is the 1999 N64 platform layer of the
  *early* Frogger 2 codebase (47 files: libultra scheduler `Nnsched.c`, RDP output, `libmus`
  sequenced music, `.ntb` texture banks, built with `ccn64`/`psylink`). The N64 version was not
  released; that code targets the older `FroggerII/Common`, not the final `FroggerProject` game
  this port reproduces, so it is reference for budgets and techniques (texture banks sized for
  TMEM, sequenced music), not a drop-in layer. `research/community-and-legal.md` lists the
  known N64 prototype builds. (`third_party/frogger2-n64` was not present when this was written.)
- Feasibility: the simulation (fixed point, PSX angles) suits the VR4300. Memory is the issue:
  8 MB must hold code, the simulation state, one level's geometry and the audio buffers. Data
  has to be converted at import time into an N64 package: textures re-quantised to fit 4 KB
  TMEM per draw (e.g. 32x32 16-bit, 64x64 4-bit CI), models reduced, FMV dropped or replaced by
  stills, music as compressed or sequenced audio. A 64 MB ROM is plausible for a full game
  without video. Effort: very high; a renderer on libdragon's `rdpq`, an audio mixer on the RSP
  or CPU, and a `DataSource` over ROM (DragonFS).

### Nintendo Switch
- Official SDK: NDA. Homebrew: devkitPro / libnx (aarch64), custom firmware required. Rust has a
  tier-3 `aarch64-nintendo-switch-freestanding` target (no std). Rendering via deko3d or the
  devkitPro Mesa port (OpenGL/EGL), which wgpu's GLES backend could target only with work.
  The CPU (4x Cortex-A57) and 4 GB RAM handle the PC data comfortably.

### Android
- wgpu (Vulkan/GLES), winit (`android-activity`), cpal (AAudio/OpenSL ES) are all supported
  upstream; gilrs does not support Android, so pads need the Android input events through winit.
  Storage: app-private dir for data; import via the Storage Access Framework (file picker) since
  raw paths to the user's ISO are not readable. Packaging: `cargo-apk`/`xbuild` or Gradle with
  `cargo-ndk`. Touch controls would be a new input source mapped to pad bits. Effort: medium.

### iOS
- wgpu Metal, winit iOS, cpal CoreAudio all supported; gilrs has no iOS backend (needs a small
  GameController.framework binding). Import via the document picker into the app container.
  Distribution: App Store review of a game port that needs user-supplied data is uncertain;
  sideloading/AltStore/EU alternative stores are the realistic route. Effort: medium.

### Web (WebAssembly)
- `wasm32-unknown-unknown` with wgpu's WebGPU (or WebGL2 via `downlevel` limits), winit's web
  backend, cpal's `wasm-bindgen` (WebAudio) feature, gilrs (Gamepad API). No filesystem: import
  the user's ISO/zip with the File API (the importer is pure Rust) into IndexedDB/OPFS and
  serve it as a `DataSource`. The Bink decoder thread and netplay sockets need rework (Web
  Workers with SharedArrayBuffer/COOP+COEP headers; WebRTC/WebSocket instead of UDP). The game
  data must never be hosted: the page only ever reads what the visitor imports locally.
  Effort: medium; the most practical "new platform" after the desktops.

## 5. no_std audit (2026-10-10)

`f2-math`: **done**. `default = ["std"]`; without it the crate is `#![no_std]` with no
allocator; `serde` is used without its `std` feature; the only std use (`RANDOM_LOG` stderr
debug output, `maths.rs`) is compiled out. Checked: `cargo check -p f2-math
--no-default-features --target thumbv7em-none-eabihf`.

`f2-game`: **not converted** (it would touch most files of a crate other agents edit daily).
Blockers, all mechanical:

| What | Where | Fix |
|---|---|---|
| prelude: `Vec`, `String`, `Box`, `format!`, `vec!`, `ToString` used without imports | 56 of 64 files | `extern crate alloc;` + a crate `prelude` module (`alloc::{vec::Vec, string::String, boxed::Box, format, vec, ...}`) glob-imported per file |
| `std::sync::Arc` | `assets.rs:8`, `event.rs:14`, `script.rs:15`, `session.rs:9`, `modscript/mod.rs:30` | `alloc::sync::Arc` (targets without atomics: `portable-atomic-util`) |
| `std::collections::BTreeMap` | `pcaudio.rs:18`, `modscript/lang.rs:8` | `alloc::collections::BTreeMap` |
| `HashMap` (iteration-order independent, keyed lookups) | `assets.rs:7,52,153` | `BTreeMap` (or `hashbrown`) |
| filesystem `DirData` | `assets.rs:62-66` | gate behind `std` (tools/tests only) |
| `std::sync::OnceLock` table cache | `view.rs:58` | `once_cell::race::OnceBox` or compute eagerly into `Game` |
| `impl std::error::Error` | `netsync.rs:206` | `core::error::Error` (stable since 1.81) |
| float methods `sqrt`/`sin`/`cos`/`acos` (not in `core`) | `view.rs:120,381`, `specfx.rs:1249-1258`, `actor2.rs:520` (test) | `f2_math::detfloat` / `libm`. Render-side only (no trace fields), and also removes the dependence on the host libm; `sqrt` is correctly rounded either way |
| `std::mem/iter/fmt/ops` paths | many files | `core::` (same items) |
| dependency `f2-trace` (serde_json, `std::io`, `std::fs`) | `trace.rs` (whole module), `Cargo.toml` | make `trace` + `f2-trace` optional under `std`, or split the schema types from the I/O |
| dependency `f2-formats` | `read.rs:23-27` (`std::error::Error`, `From<Error> for std::io::Error`), `level.rs:169` (`std::env` debug switch), `objbank.rs:29,83` (`HashSet`), `fev_write.rs:416-421` (test only) | `core::error::Error`, gate the io/env bits behind `std`, `BTreeSet` |
| `serde` with default features | `Cargo.toml` (workspace dep) | `default-features = false, features = ["derive", "alloc"]` + `std` feature forwarding |

Parity guard for that conversion: the three demo traces and `f2-parity check --corpus all`
must stay identical; replacing the four float calls changes only the special-effect ring
rendering, so screenshot comparisons of scenes with those effects are the thing to watch.
