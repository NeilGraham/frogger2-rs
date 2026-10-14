# Platforms

Where the port runs today, how it is layered, and what each future target would need.
Build instructions: [BUILDING.md](BUILDING.md). Packaging: `tools/package/`, `packaging/`.

## 1. Layering

```
 frogger (bin)         main loop, CLI, settings, menus glue          per platform: thin
   ├─ f2-game          deterministic simulation (60 Hz fixed tick)   PORTABLE, no_std + alloc
   │    └─ f2-math     20.12 fixed point, PSX trig, RNG, detfloat    PORTABLE, no_std
   ├─ f2-formats       pure parsers of the retail data               PORTABLE, no_std + alloc
   ├─ f2-engine        CPU scene/draw list (no_std) + wgpu back end  GPU back end per platform
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
| PS4 / PS5 (homebrew) | **builds; runs in the shadPS4 emulator**; not yet run on hardware | `tools/ps4/build-pkg.sh` (OpenOrbis, `x86_64-unknown-freebsd` std) | VideoOut + CPU renderer (`f2-engine/soft.rs`) | sceAudioOut | scePad | PS4 fPKG (also installs on a jailbroken PS5) |
| PS5 (native homebrew) | **builds; runs in the KytyPS5 emulator** (4K, 120 Hz); not yet run on hardware | `tools/ps5/build.sh` (ps5-payload-sdk stubs, PS5_Vulkan packaging, `x86_64-unknown-freebsd` std) | tiled VideoOut + CPU renderer; optional wgpu on PS5 RADV (`--gpu`, untested) | sceAudioOut | scePad | PS5 app folder / `.ffpkg` |
| Android (arm64, x86_64) | **builds; imports and plays in the Android 15 emulator**; not yet run on a phone | `tools/android/build-apk.sh` (cargo-ndk, no Gradle) | winit NativeActivity / Vulkan or GLES | cpal AAudio (Oboe) | touch overlay + Android gamepad buttons | APK, sideload ([ANDROID.md](ANDROID.md)) |
| Web, iOS, Switch, PS2/PS3, N64 | not started | see §4 | | | | |

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
- **no_std**: `f2-math` (no allocator), `f2-formats`, `f2-game` and `f2-engine`'s render core
  (`alloc`) build `#![no_std]` without their default `std` feature (§5; CI checks
  `thumbv7em-none-eabihf` and big-endian MIPS). Targets with a Rust `std` (Android, iOS, Web,
  Linux-like OSes) do not need this.

### PlayStation 2
- Hardware: Emotion Engine (MIPS R5900, ~295 MHz), 32 MB RAM, GS with 4 MB embedded VRAM, IOP
  (PS1 CPU) for I/O and SPU2 sound (2 MB). The EE FPU is single precision only and not IEEE
  compliant (no denormals/infinities, truncating rounding), so `f64` must be soft-float and the
  float render paths will not be bit-identical; gameplay (integer fixed point) is unaffected.
- Toolchain: PS2SDK (open source homebrew SDK, GCC). Rust has no upstream R5900 target with
  `std`; a custom target spec (MIPS R5900, soft `f64`) with `-Zbuild-std=core,alloc` and a
  PS2SDK-backed allocator/libc shim is the likely route (`f2-game` is no_std, §5).
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
- **Done (homebrew fPKG)**: `crates/frogger-ps4` + `tools/ps4/build-pkg.sh`, details in
  [tools/ps4/README.md](../tools/ps4/README.md). A PS4 fake package built with the OpenOrbis
  toolchain (no Sony SDK); a jailbroken PS5 installs it as a PS4 title (etaHEN/kstuff).
- Rust std works: the crate is a staticlib for the stock `x86_64-unknown-freebsd` target
  linked against the console's `libkernel`/`libSceLibcInternal`, with shims for the calls
  whose structs differ between FreeBSD 9 (console) and the FreeBSD 12+ ABI std uses
  (`stat`, `readdir`, ...), our own heap (dlmalloc over `mmap`) and libm. No `no_std` port
  of the game was needed.
- Rendering: the GPU APIs (GNM/AGC) are proprietary and wgpu has no backend for them, so the
  console build uses `f2-engine`'s CPU rasteriser (`soft.rs`, feature `gpu` off) over the
  same draw list, into a VideoOut framebuffer (852x480 internal, scaled to 1080p). The
  `frogger` crate is a library (`desktop` feature = winit/wgpu/gilrs) driven by the console
  host. Audio: sceAudioOut thread pulling the mixer; pads: scePad -> `RawPad`.
- Verified in the shadPS4 emulator (boot, FMV, frontend, gameplay, pad, 60 fps); real
  hardware untested.
- **Native PS5 (CPU renderer, runs in KytyPS5)**: `crates/frogger-ps5` + `tools/ps5/build.sh`
  ([tools/ps5/README.md](../tools/ps5/README.md)): a PS5 native app (`eboot.bin` +
  `sce_sys/param.json` + `sce_module/libc.prx`, etaHEN/ShadowMountPlus/Itemzflow directory
  title or `.ffpkg`), the same std-on-the-system-libraries approach through the public
  ps5-payload-sdk stubs and PS5_Vulkan's packaging tools; tiled 4K/1440p/1080p VideoOut
  buffers, 120 Hz when the display takes it. Shared host code: `crates/console-host`.
- Native PS5 GPU: the wgpu renderer on mihawk-99/PS5_Vulkan's RADV port (statically linked,
  presenting through `VK_KHR_display` on VideoOut): `f2-engine` features `ps5-vulkan` /
  `ps5-static-driver` (`GpuContext::new_ps5`, `vkraw::VkPresenter`), host-verified; driver
  build, link recipe, emulator results and blockers in `tools/ps5/VULKAN.md`. Not yet run on
  hardware (KytyPS5 cannot run the driver).

### Nintendo 64 (owner's interest)
- Hardware: VR4300 (MIPS III, 93.75 MHz, 64-bit integer + FPU), 4 MB RDRAM (8 MB with the
  Expansion Pak), RCP: RSP microcode + RDP rasteriser with 4 KB TMEM, cartridge ROM up to 64 MB
  on period hardware (flash carts allow more).
- Toolchain: libdragon (open source, GCC, modern rdpq/OpenGL 1.1-style API) or the 1999-era
  libultra (proprietary). Rust on N64 is done with a custom `mips` target spec plus
  `-Zbuild-std=core,alloc`; `f2-game` is no_std for this (§5).
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
- **Done (first cut)**: `crates/frogger-android`, details in [ANDROID.md](ANDROID.md). wgpu
  (Vulkan/GLES), winit (`android-native-activity`), cpal (Oboe, C++ runtime linked statically);
  touch controls and Android gamepad key events become player 1's joypad; a small Java
  `NativeActivity` subclass adds immersive mode and the Storage Access Framework picker for
  importing a disc image on the device. Remaining: physical-device testing, analogue sticks,
  multiple pads, release signing.

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

## 5. no_std (done 2026-10-10)

The portable core builds `#![no_std]` + `alloc` (N64 phase 1A, `docs/N64-PLAN.md`). Each crate
has a default `std` feature; without it the crate is `no_std` (`#![cfg_attr(not(feature =
"std"), no_std)]`, so std builds are unchanged) and uses a crate-private `prelude` module
(`alloc::{vec::Vec, string::{String, ToString}, boxed::Box, borrow::ToOwned, format, vec}`)
glob-imported where needed. New code in these crates must use `core::`/`alloc::` paths (CI
catches `std::`).

| Crate | Without `std` | `std` adds |
|---|---|---|
| `f2-math` | everything (no allocator needed) | `RANDOM_LOG` stderr debug output |
| `f2-formats` | **all parsers and types** (pure bytes → owned data), plus serde derives (`serde` feature, serde without std) so the N64 asset packer can serialize parsed structures (e.g. `postcard`) and the N64 deserialize them | `From<Error> for std::io::Error`, the `F2_FORMATS_DEBUG` env switch |
| `f2-game` | the whole simulation (`Game`, every module except `trace`) | `assets::DirData` (filesystem `DataSource`, tools/tests), `trace` + the `f2-trace` dependency (serde_json) |
| `f2-engine` | render core: `scene` (CPU transform → TL triangle lists), `anim`, `textures`, `font`, `math`, the draw-list builder (`Renderer::build` → `(Vec<TlVertex>, Vec<DrawCmd>)`, now public with `DrawCmd`/`DepthMode` and `TextureRegistry::take_dirty` for external back ends such as the N64 RDP), `soft` (single-threaded, `serial_exec`) | `load` (file helpers), `RgbaImage::write_png` (png), threaded `soft` bands (`scoped_exec`); `gpu` (wgpu) and `gles2` imply `std` |

What changed, per blocker of the audit:

- `Arc` → `alloc::sync::Arc`, `BTreeMap` → `alloc::collections::BTreeMap`, `std::{mem, iter,
  fmt, ops, ...}` → `core::`.
- `HashMap`: `ModelTable.models` (u32 → model facts), `MemData` (tests) and f2-engine's
  `TextureRegistry.by_crc` are keyed lookups only (never iterated; `Game::checksum` never walks
  them), now `BTreeMap`. `objbank`'s `HashSet<u32>` (relocation membership) → `BTreeSet`.
- `OnceLock` (`view.rs` and f2-engine `math.rs` sin/acos tables) → `once_cell::race::OnceBox`
  (a racing second initialiser builds identical tables). Needs pointer-width atomics, which
  `Arc` needs anyway; the N64 target spec has `max-atomic-width: 64` + `singlethread`.
- `std::error::Error` → `core::error::Error` (`HashError`, `f2_formats::Error`,
  `EngineError`); serde accepts it without std.
- Float methods: `f2-game` uses `libm::sqrtf` (`view.rs`, `specfx.rs`; correctly rounded, equal
  to `f32::sqrt` for all 2^31 finite non-negative inputs), `detfloat::acos` (`specfx.rs`; equal
  bits to glibc for every f32 in [-1, 1]) and, for the `DrawFXRing` rotation's `sinf`/`cosf`,
  `detfloat::{sin, cos}` in f64 rounded once to f32. `f2-engine` keeps the inherent methods
  with `std` (bit-identical) and gets them from `libm` through a private `fmath::FloatExt`
  trait without it.
- `actor2.rs:520` (test only) is unchanged (tests build with std).

Parity evidence (2026-10-10, USA data):

- Demo traces `demo_trace 0 0 / 1 0 / 6 1` byte-identical before/after (sha1 `b0818758…`,
  `148391ab…`, `592b9d2a…`, 1556 lines each).
- `f2-parity check --corpus all`: 102 cases, 0 regressed; the 102 Rust traces it writes are
  byte-identical (sha1 of every file) to the ones before the conversion.
- The audit called the ring float calls render-only; they are not: `DrawFXRing` places the
  `FXTYPE_TWINKLE` trails (trace `specfx` section) and calls `Random` from the transformed
  ring. The rotation's `sinf`/`cosf` were the host C library's (glibc ≠ UCRT, so not
  netplay-deterministic either). The retail exe has no `sinf`/`cosf` (MSVC 6 inlines
  `(float)sin((double)x)` as x87 `fsin`/`fcos`), so the port now evaluates in f64 with
  `detfloat` and rounds once, which is closest to retail and host-independent. Over the 8193
  reachable fixed-point normals, glibc's `sinf`/`cosf` and the f64 path differ by 1 ulp for
  208 `normal.vy` values; 29 of the 23,012 map tiles of all levels have such a normal (ring
  normal = tile normal). Screenshot check (`frogger --headless --pad-script`, croak rings on
  0-0, 1-0, 2-1, 4-0, 4-1, 5-1, 6-2; 168 frames): pixel-identical PNGs before/after.

Checks (also in CI, `.github/workflows/ci.yml` jobs `cross-check` and `nostd-mips`):

```bash
cargo check -p f2-math -p f2-formats -p f2-game -p f2-engine --no-default-features --target thumbv7em-none-eabihf
cargo check -p f2-formats --no-default-features --features serde --target thumbv7em-none-eabihf
# big-endian MIPS (N64 stand-in), nightly with rust-src:
cargo +nightly check -Zbuild-std=core,alloc --target mips-unknown-linux-gnu -p f2-math -p f2-formats -p f2-game -p f2-engine --no-default-features
# the real N64 target spec (tools/n64/mips-nintendo64-none.json, phase 1B): full codegen
cargo +nightly build --release -Zbuild-std=core,alloc -Zjson-target-spec --target tools/n64/mips-nintendo64-none.json -p f2-game -p f2-formats -p f2-engine --no-default-features
```

The last one was run against phase 1B's spec (VR4300, n32, big-endian) and builds the rlibs.
`size_of::<Game>()` is 28,408 bytes there (30,688 on 64-bit hosts); heap numbers in
`research/n64-memory.md`.
