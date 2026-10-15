# Export to other platforms

Every desktop build of **Swampy's Revenge Recomp** can turn the player's imported game into
ready-to-run builds for other platforms, on any desktop OS, with no SDK, compiler, Docker or
WSL:

| Target | Output | Runtime used |
|---|---|---|
| This desktop OS (Windows / macOS / Linux) | a folder / `.app` / `.AppImage` with the game data inside | the running app itself |
| Android | `.apk` with the game data inside (installs and runs directly) | release asset `swampys-revenge-recomp-<v>-android.apk` (the shell app) |
| PS4 | fake PKG (`.pkg`) | `runtime-ps4-<v>.zip` |
| PS5 | app folder (+ `.ffpkg` where supported) | `runtime-ps5-<v>.zip` |
| N64 | `.z64` ROM | `runtime-n64-<v>.zip` |

```sh
frogger --export ps4,ps5,n64,android [--out DIR]            # runtimes from the matching GitHub release
frogger --export n64 --runtime runtime-n64-0.1.0.zip         # a local runtime
frogger --export all                                         # every target this OS can export
```

The builds contain the player's game data: they are for the player's own devices and must not
be shared. Runtimes contain no game data and are published with each release.

## Branding (from the player's disc)

`frogger::export::branding` (`crates/frogger/src/export.rs`):

- **Short title** = the disc installer's product name, recorded at import as
  `manifest.json` `disc_app_name` (`FROGGER2/DATA.TAG` `Application=Frogger2`, else
  `SETUP.INI` `AppName=`), with the number spaced: **"Frogger 2"**.
- **Title** = short title + `": "` + the game's own subtitle string `STR_STORYMODE`
  (`maps/language.fla`, English column): **"Frogger 2: Swampy's Revenge"**.
- **Art** = the game's loading-screen artwork (`textures/loading.fla`, 640×480), centre
  square, 1024×1024 (`Branding::art`, opaque) and the same with rounded corners
  (`Branding::icon`, radius 18 %, transparent outside).

Where each target shows them:

| Target | Name | Icon |
|---|---|---|
| Windows | folder / exe file name and window title: title | `.ico` from `icon` (exe resource + shortcut) |
| macOS | `CFBundleName`/`CFBundleDisplayName`: title | `.icns` from `icon` |
| Linux | `.desktop` `Name`: title; AppImage file name | `icon` PNG |
| Android | launcher label: **short title** | adaptive icon from `art`, legacy from `icon` |
| PS4 / PS5 | `param.sfo` / `param.json` `TITLE`: title | `icon0.png` (512×512) from `art` |
| N64 | ROM header name (20 chars): short title, upper case | (none) |

Imports made before `disc_app_name` existed must be re-imported (the export says so).

## Code

- `crates/f2-package`: the packers, one module per target (`android.rs`, `ps4.rs`, `ps5.rs`,
  `n64.rs`, `windows.rs`, `macos.rs`, `linux.rs`), behind `f2_package::export(&Job)`. Pure Rust,
  builds and runs on Windows, macOS and Linux.
- `crates/frogger/src/export.rs`: branding, runtime lookup (`--runtime`, the running app for
  its own OS, else `<config>/runtimes/<asset>` downloaded with `curl` from
  `https://github.com/NeilGraham/frogger-2-recomp/releases/download/v<version>/`, override
  with `F2_RUNTIME_BASE_URL`), the `--export` command.

## Runtimes

Built by CI (`.github/workflows/release.yml`) for each release tag `v<version>`, from source,
with no game data. Each console runtime is a zip produced by `tools/runtime/build-<target>.sh
OUT_DIR VERSION` (Linux, with that target's toolchain) and contains a `runtime.json`
(`{"target": "...", "version": "...", "format": 1, ...}`) describing its layout; the layout
details are documented in each section below by the packer that consumes it.

The desktop packages are the release builds themselves (`tools/package/package.sh`):
`swampys-revenge-recomp-<v>-windows-x86_64.zip`, `swampys-revenge-recomp-<v>-macos-<arch>.zip`,
`SwampysRevengeRecomp-<v>-x86_64.AppImage`.

### Where an exported build finds its data

| Target | Data location |
|---|---|
| Windows | `<folder>/game-data/<rendition>/` next to the exe |
| macOS | `<App>.app/Contents/Resources/game-data/<rendition>/` |
| Linux AppImage | `usr/share/game-data/<rendition>/` inside the AppImage (`$APPDIR`) |
| Android | bundled in the APK (see the Android section) |
| PS4 / PS5 | inside the package (see their sections) |
| N64 | the ROM's asset pack (`f2-n64pack`) |

<!-- Per-target sections (runtime layout, packing steps, testing) follow, written with each packer. -->

## N64

`frogger --export n64` writes `<title>.z64` ("Frogger 2 - Swampy's Revenge.z64"), a ROM for
a 64 MiB cartridge with the Expansion Pak required (ares, flash carts; Expansion Pak +
SRAM 256 Kbit save in the header). Code: `crates/f2-package/src/n64.rs` (+ `n64/rom.rs`,
`n64/runtime.rs`), the pack and DFS writer in `tools/f2-n64pack` (library, pure Rust).

### Runtime `runtime-n64-<v>.zip`

Built by `tools/runtime/build-n64.sh OUT_DIR VERSION` (Linux: libdragon preview + mips64-elf
toolchain at `N64_INST`, preferably the 3A variant `tools/n64/libdragon-3a.sh`; Rust nightly
with `rust-src`; prerequisites at the top of the script). It runs `tools/n64/build.sh
--runtime` (the default release engine, `F2N64_BOOT`/`F2N64_FEATURES` refused, linked with an
empty DFS) and cuts the ROM into:

| File | What |
|---|---|
| `runtime.json` | `{"target":"n64","version":"<v>","format":1,"rom":{...},"pack":{...},"build":{...}}` |
| `ipl3-header.bin` | libdragon's IPL3 (6444 bytes) with the ROM header as `n64tool` + `ed64romconfig` wrote it (iQue entry point, category `N`, region 0, "ED" advanced header: Expansion Pak required, SRAM 256K, region free) |
| `frogger2.elf.stripped` | the linked engine, stripped and `n64elfcompress`ed (TOC entry 0) |
| `frogger2.elf.sym` | `n64sym` symbols (crash-screen backtraces) |
| `libdragon.version` | libdragon's version file (`n64.mk` `N64_TOOLFILES`) |

`runtime.json` `rom`: `header`, `toc` (true), `pad_align` (16384), `max_size` (67108864),
`files` = the TOC entries in ROM order, each `{name, path | dfs: true, align}` (n64.mk: 256
for the ELF, 16 for the rest; the DFS entry `frogger2.dfs` is filled by the exporter). `pack`:
`{profile: "portable", cart_bytes: 67108864, code_reserve: 2621440}` (f2-n64pack's budget).
`build`: git revision, dirty flag, libdragon variant, nightly rustc. The runtime tool
(`cargo run -p f2-package --example n64_runtime -- make ROM ZIP VERSION`) only accepts a ROM
whose parts re-assemble into it byte for byte (which also pins the exact IPL3 size).

### Assembly (`f2_package::n64`)

1. **Pack**: `f2-n64pack`'s pipeline in the **portable** profile into a work folder next to
   the output (`.<title>.n64-work`, removed afterwards): levels F2Z with trained dictionaries
   (as `build.sh`), models/textures/fonts aPLib (`f2_n64pack::{apultra,dca}`, a Rust port of
   libdragon's apultra: byte-identical to `mkasset -c 2 -w 256`), sound effects VADPCM
   (`f2_n64pack::{vadpcm,wav64}`, byte-identical to `audioconv64 --wav-compress
   vadpcm,huffman=false`), **music VADPCM** (stereo, resampled with a deterministic
   windowed-sinc filter to the highest of 32000/28000/24000/22050/… Hz that fits the space
   left; `build.sh`'s default is ULC at 128 kbps, which has no Rust encoder), **no FMV**
   (`build.sh` encodes H.264 with libdragon's videoconv64/x264, no Rust encoder exists; the
   engine skips missing videos: `n64_fmv_play` returns 0, logged `app: video … missing`).
2. **DFS**: `f2_n64pack::dfs`, a port of `mkdfs` (byte-identical, including the hash lookup
   table).
3. **ROM**: like `n64tool --toc --title … --align 256 ELF SYM DFS VERSION` + `ed64romconfig`:
   the header block, zeros to 16 bytes, the 1024-byte TOC, each file aligned, zero padding to
   16 KiB; title at 0x20 = the disc's short title upper-cased, 20 characters max (`FROGGER 2`).
   There is no cartridge CRC to compute: libdragon's IPL3 does not check it (header CRC words
   stay zero); the only checksum is the TOC cookie, `~crc32` over everything after the header
   (padding included) then the TOC with a zero cookie, as n64tool computes it.
4. **Size**: the ROM must fit 64 MiB (`max_size`, `build.sh`'s check). The pack is budgeted
   for 64 MiB minus a 2.5 MiB code reserve (as `build.sh`); if it still does not fit, the
   export retries without music (the `--small` variant, as `build.sh --small` but with the
   full engine, which skips missing music/videos) and says so.

### Byte-for-byte checks

`F2_GAME_DIR=…/game-data/usa/game tools/n64/export-check.sh` builds the runtime, then compares
(a) `build.sh`'s default ROM with the runtime + the Rust DFS of the same `target/n64/filesystem`
and (b) `build.sh --portable` (the portable pack, then the libdragon tools `mkdfs`, `n64tool`,
`ed64romconfig`) with `frogger --export n64`. Both must be identical for one checkout (the
ELF differs between checkouts: Rust symbol hashes include the source path).

### Test status (2026-10-14, USA data, libdragon 3A)

- `export-check.sh`: (a) default ROM 64,765,952 bytes **identical** to runtime + Rust DFS
  (62,151,546-byte DFS, identical to `mkdfs`) + Rust ROM assembly; (b) `build.sh --portable`
  vs `frogger --export n64`: 62,226,432 bytes **identical**.
- Portable pack: models 2,963,715 B and textures 2,846,178 B (aPLib), levels 386,176 B, sound
  effects 12,307,394 B: the same sizes as the default pack (byte-identical encoders); music
  VADPCM 24 kHz stereo 41,053,840 B (the default: ULC 128 kbps 24.3 MB), no FMV (default
  19.3 MB). Export time ~1 min on a 16-thread desktop.
- Encoder parity: `f2_n64pack::dca` vs `mkasset -c 2 -w 256` (synthetic incl. > 1 MiB
  multi-block inputs, binaries, the game's object/texture files), `f2_n64pack::wav64` vs
  `audioconv64` (208 synthetic cases, all 417 unique sound effects, all 11 music tracks), DFS
  vs `mkdfs`, ROM vs `n64tool` (unit tests compare with the real tools when installed and
  carry golden values otherwise).
- ares v148 (`tools/n64/run-ares.sh`): the exported ROM boots to the same frontend as the
  `build.sh` ROM (title screen with Options / Start Game), the missing videos are skipped
  (`app: video rom:/video/hasbros.h264 missing`), the VADPCM music plays (tracks 2, 3, 7, 9 in
  the attract loop), the demo levels run.
- Not tested: real hardware / flash carts; export on Windows and macOS hosts (the code is
  pure Rust; byte-identity across host OSes is expected for the integer encoders, the texture
  converter's floating point is not separately verified across OSes).

