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

## PS4

Output: `<out>/<title stem>.pkg` (e.g. `Frogger 2 - Swampy's Revenge.pkg`), a PS4 **fake package**
(fPKG) for a jailbroken PS4 (GoldHEN) or a jailbroken PS5's PS4 backward-compatibility fPKG
support (etaHEN / kstuff), installed like any homebrew package (GoldHEN / etaHEN package
installer, Itemzflow, or "Debug Settings > Game > Package Installer" from USB / `/data/pkg`). Loaders
read the content ID from the package itself, so the file name is free. It is the same package
`tools/ps4/build-pkg.sh --data` builds with OpenOrbis' PkgTool, without any SDK, .NET or
OpenSSL: `crates/f2-package/src/ps4*`.

**Runtime** `runtime-ps4-<v>.zip` (`tools/runtime/build-ps4.sh OUT_DIR VERSION`: `build-pkg.sh
--stage-only`, the conservative variant FROG20000):

| File | |
|---|---|
| `eboot.bin` | the game, fake-signed SELF (`create-fself`) |
| `sce_module/libc.prx`, `sce_module/libSceFios2.prx`, `sce_sys/about/right.sprx` | OpenOrbis' modules, as every OpenOrbis package carries them |
| `runtime.json` | `target` `ps4`, `version`, `format` 1, `title_id` `FROG20000`, `content_id` `IV0000-FROG20000_00-FROGGER2RUSTPORT`, the param.sfo values (`app_ver` `01.03`, `attribute` 0, `download_data_size` 64, `system_ver` 0, `app_type` 1), `data_dir` `data`, `files` (package order) |

Any other file in the runtime is packed too (a runtime `sce_sys/icon0.png` / `pic0.png` /
`pic1.png` replaces the generated artwork).

**Packing** (`ps4.rs`):

1. `param.sfo` written in Rust (`ps4/sfo.rs`): `APP_TYPE`, `APP_VER`, `ATTRIBUTE`,
   `CATEGORY` `gd`, `CONTENT_ID`, `DOWNLOAD_DATA_SIZE`, `SYSTEM_VER`, `TITLE` = the branding
   title, `TITLE_ID`, `VERSION`, exactly the fields `build-pkg.sh` sets; the package's own
   param.sfo entry adds `PUBTOOLINFO` (creation date, sizes) and `PUBTOOLVER` as PkgTool does.
2. Artwork from the branding art (`ps4/art.rs`): `icon0.png` 512x512 (8-bit RGB) and
   `pic0.png` = `pic1.png` 1920x1080 (the art at full height over a blurred, darkened copy),
   as `tools/ps4/make-art.sh` makes them.
3. Inner PFS (`ps4/pfs.rs`, unsigned, unencrypted, 64 KiB blocks): `eboot.bin`,
   `sce_module/`, `sce_sys/about/right.sprx`, the generated `sce_sys/keystone`, and the game
   (`game/`, `music/`, `video/`, `manifest.json` of the rendition) at `data/`, i.e. `/app0/data`,
   the first place the PS4 host looks (`console-host` `host.rs`). `param.sfo` and the PNGs are
   package entries, not PFS files (as PkgTool does).
4. Outer PFS: `pfs_image.dat` = PFSC header (stored blocks) + inner image, HMAC-SHA256 block
   signatures, AES-XTS encryption with keys derived from the EKPFS.
5. PKG (`ps4/pkg.rs`): header, entry keys / image key (RSA with the public fake keys,
   deterministic Mersenne-Twister padding), general digests, entry table, `license.dat`
   (debug RIF signed with the debug key) / `license.info`, PlayGo chunk files, param.sfo,
   `psreserved.dat`, the PNGs; header digest and signature. All-zero passcode.

The format is reimplemented from LibOrbisPkg (OpenOrbis, LGPL-2.1, C#; algorithms and the
public key material in `ps4/keys.rs`; no code copied). Dependencies are pure Rust (`sha2`,
`hmac`, `aes`, `num-bigint`, `zip`). A 470 MB package is written in about 2 s plus the copy.

**Verified (2026-10-14):**

- `compare_with_pkgtool` (`crates/f2-package/src/ps4/tests.rs`, ignored test taking two
  packages): a package exported from a runtime staged by `build-pkg.sh` (with PkgTool's
  artwork PNGs put into the runtime so the inputs are identical) against `build-pkg.sh --data`
  output for the same data: same size (498,139,136 bytes), header identical apart from the
  digests, every entry at the same offset and size, `ENTRY_KEYS`, `IMAGE_KEY`, `license.dat`,
  `license.info`, `param.sfo`, PlayGo files, `psreserved.dat` and the PNG entries
  byte-identical; outer PFS same layout and inodes; inner PFS (431 inodes, 468 MB)
  byte-identical once the inode time stamps are blanked. The remaining differences are the
  time stamps and the digests / signatures that cover them.
- `export_and_read_back`: a synthetic export read back with the test reader (entry digests,
  body / header / PFS digests, PlayGo chunk hashes, outer block and superblock HMACs, file
  contents, nothing from `oracle/` packed).
- shadPS4 (SDL build 2026-10-10, no package installer): the exported package extracted with
  the test reader (`extract_package`, the way a console installs it: PFS files + sce_sys
  entries) and started with `shadPS4.exe -g <dir>/eboot.bin`: boots from `/app0/data`, and with
  `autostart.txt` = `level 1 1` plays Ancients 1 at 60 fps (shot via `shot.txt`).
- Not verified: installing on real hardware (no console here). The conservative variant is the
  one that boots in shadPS4; the PS5's PS4 mode crashed with the *enhanced* settings before
  (tools/ps4/README.md, Pro bisect).

## PS5

Output: `<out>/<title stem> (PS5)/`, the native PS5 **app folder** (`eboot.bin`,
`sce_module/libc.prx`, `sce_sys/`, `data/`): copy it to the console and launch it with
ShadowMountPlus (e.g. FTP to `/data/homebrew/`), as `tools/ps5/build.sh` documents for its
`target/ps5/PPSA99920/` folder. Re-exporting replaces a previous export folder.

**Runtime** `runtime-ps5-<v>.zip` (`tools/runtime/build-ps5.sh OUT_DIR VERSION`:
`tools/ps5/build.sh --stage-only`, the CPU game build PPSA99920):

| File | |
|---|---|
| `eboot.bin` | the game (PS5 executable in a fake-signed SELF) |
| `sce_module/libc.prx` | the clean-room libc companion module |
| `runtime.json` | `target` `ps5`, `version`, `format` 1, `title_id` `PPSA99920`, `content_id` `UP9000-PPSA99920_00-FROGGER2RUSTPORT`, `content_version`, `param_json` (build.sh's `sce_sys/param.json`, title left empty), `data_dir` `data`, `files` |

**Packing** (`ps5.rs`): the runtime files; `sce_sys/param.json` = `param_json` with
`localizedParameters.<default language>.titleName` = the branding title (byte-identical to
build.sh's for the same title); `icon0.png` 512x512 and `pic0.png` / `pic1.png` 3840x2160
(`ps4/art.rs`), each also as BC7 DDS (`ps5/dds.rs`: mode-6 BC7 encoder, DX10 header identical to
the one build.sh writes with bc7enc_rdo: single level, BC7_UNORM, straight alpha); the game
data in `data/` (`/app0/data`).

**Verified (2026-10-14):** the exported folder has the same file list as `tools/ps5/build.sh
--data` output, the same `param.json` bytes, `libc.prx`, DDS headers and sizes; the BC7 blocks
decode identically with an independent decoder (`bcdec_rs`, test). KytyPS5 (2026-10-10
f63d3be) runs the exported folder (`kyty_emulator.exe --game <folder>`): data from
`/app0/data`, opens 3840x2160 tiled video out and runs at a steady 60 fps (its `frogger.log`;
the PS5 host has no autostart / shot hooks, so no in-game screenshot). Not verified on
hardware.

**Not exported (yet):** the PS5 fake package (`.pkg`, `build.sh --pkg`, LibProsperoPkg / GPL-3,
the form that ran on the owner's console) and the `.ffpkg` (UFS2 image, `build.sh --ffpkg`
via UFS2Tool) / `.ffpfsc` (PFS image, MkPFS) images for ShadowMountPlus. Each needs its own
file-system / package writer (UFS2 superblock, cylinder groups and inode tables; the PS5 PKG's
PFS, PlayGo and keystone), with no way to check the result here short of a console, so the
folder (which ShadowMountPlus also mounts) is the export; `tools/ps5/build.sh` still makes all
three from a source checkout.
