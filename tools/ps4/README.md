# PS4 / PS5 homebrew package

`tools/ps4/build-pkg.sh` builds a PS4 fake package (fPKG) of the port. It installs on a
jailbroken **PS4** (GoldHEN) and on a jailbroken **PS5** running PS4 homebrew in
backward-compatibility mode (etaHEN / kstuff with PS4 fPKG support, installed through
etaHEN's package installer or Itemzflow). It needs no Sony SDK: the
[OpenOrbis toolchain](https://github.com/OpenOrbis/OpenOrbis-PS4-Toolchain) links
against stubs of the console's own system libraries.

```sh
tools/ps4/build-pkg.sh                          # data copied to the console separately
tools/ps4/build-pkg.sh --data game-data/usa     # your imported data bundled in the package
tools/ps4/build-pkg.sh --icon boxart.png        # home-screen icon / background from an image
```

Output: `target/ps4/IV0000-FROG20000_00-FROGGER2RUSTPORT[-with-data].pkg` (title ID
`FROG20000`). A `--data` package contains copyrighted game data from your own disc: it is
for your console only and must not be shared. Neither the package nor the artwork is
committed (`target/`, `game-data/` are ignored).

## Game data on the console

Without `--data`, import your CD on a PC (`f2-import`, see `docs/BUILDING.md`) and copy
the rendition folder (the one containing `game/`, `music/`, `video/`) to the console,
e.g. over etaHEN's FTP server:

| Looked up in order | |
|---|---|
| `/app0/data` | bundled with `--data` |
| `/data/frogger2` (or `/data/frogger2-rs`) | internal storage |
| `/mnt/usb0/frogger2`, `/mnt/usb1/frogger2` | USB drive, folder `frogger2` at the root |

Until data is found the game shows a dark red screen with a notification and keeps
checking, so you can copy it while it runs. Config, saves (`saves/savegame.dat`) and
`frogger.log` go to `/data/frogger2` (fallback `/download0/frogger2`).

Controls: the DualShock 4 / DualSense works like the PC game's pad (d-pad or left stick
to hop, Cross jump, Circle tongue, Options pause, touch-pad click = the PC pad's
Select). The settings screen (pause menu -> Settings) works with the pad.

## How it works

- **Renderer**: the console GPU APIs (GNM/AGC) are proprietary, so the port uses
  `f2-engine`'s CPU back end (`soft.rs`), which rasterises the same draw list the wgpu
  back end uses (verified near pixel-identical against wgpu). It renders 852x480 (the
  original 480 lines at 16:9, enhanced profile Hor+) on 4-6 threads and scales to a
  1920x1080 VideoOut framebuffer.
- **Rust on the console**: `crates/frogger-ps4` is a staticlib for the stock
  `x86_64-unknown-freebsd` target (Rust std included), linked against `libkernel` and
  `libSceLibcInternal` instead of OpenOrbis' musl:
  - `orbis/shim.rs` translates the calls whose structs changed between FreeBSD 9 (the
    console kernel) and the FreeBSD 12+ ABI std uses (`stat`, `fstat`, `readdir`, ...)
    and supplies symbols the system libraries lack;
  - `orbis/alloc.rs` is the heap (dlmalloc over libkernel `mmap`), libm maths and
    `memcmp`, so the program does not depend on the system libc's heap setup;
  - `csrc/crt_shim.c` replaces musl's `__libc_start_main` and adds `memmove`;
  - `link.x` is OpenOrbis' linker script adapted to Rust's per-symbol sections, with a
    32-byte aligned TLS segment.
- **Packaging**: `create-fself` -> `eboot.bin`, `PkgTool.Core` -> `param.sfo` and the
  `.pkg`; `gp4-dirs.py` fixes the directory tree `create-gp4` writes; `make-art.sh`
  makes `icon0.png` (512x512) and `pic0/pic1.png` (1920x1080).

## Requirements

- OpenOrbis PS4 toolchain v0.5.4 (`toolchain-llvm-18.tar.gz`) extracted, with
  `OO_PS4_TOOLCHAIN` pointing at `.../OpenOrbis/PS4Toolchain` (default
  `~/opt/openorbis/OpenOrbis/PS4Toolchain`).
- `clang-18` and `ld.lld-18` (`apt install clang-18 lld-18`), `ffmpeg`, `python3`.
- `rustup target add x86_64-unknown-freebsd`.
- `PkgTool.Core` is .NET Core 3.1 and needs OpenSSL 1.1, which Ubuntu 24.04+ lacks:
  extract `libssl1.1_1.1.1f-1ubuntu2.24_amd64.deb` (Ubuntu archive) with
  `dpkg-deb -x` and set `F2_LIBSSL11` to its `usr/lib/x86_64-linux-gnu` (default
  `~/opt/libssl11/usr/lib/x86_64-linux-gnu`).

## Testing without a console

The [shadPS4](https://github.com/shadps4-emu/shadPS4) emulator runs the staged build:
`shadPS4.exe -g <stage>/eboot.bin` with the data in `<stage>/data`. Verified (shadPS4
v0.19.x on Windows): boot, Bink intro FMVs with audio, frontend, story book, training
level with HUD and PlayStation button prompts, pad input, steady 60 fps.

Not verified: real PS4/PS5 hardware (no console here). If it fails there, `frogger.log`
in `/data/frogger2` and the kernel log (`[frogger2]` lines) show how far it got.

## Native PS5 (future)

A native PS5 build could use the GPU through the open-source PS5 Vulkan drivers
([mihawk-99/PS5_Vulkan](https://github.com/mihawk-99/PS5_Vulkan), a RADV port, GPL-3.0)
and the desktop wgpu renderer, built with the ps5-payload-sdk. That produces native PS5
homebrew (`eboot.bin` folder / `.ffpkg`), not an installable `.pkg`, and needs the same
std shims for the PS5's libraries.
