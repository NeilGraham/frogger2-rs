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

## Desktop exports (Windows, macOS, Linux)

Each desktop app exports **its own OS** from itself: the runtime is the running app
(`frogger::export::host_runtime`), so no download is needed. The packers are pure Rust
(`editpe` for PE resources, `plist` for `Info.plist`, `backhand` + `ruzstd` for squashfs) and
build on Windows MSVC, macOS and Linux. Every export ships only the rendition's `game/`,
`music/`, `video/` and `manifest.json` (`f2_package::util::rendition_files`).

### Where an exported build looks for its data

`frogger::data::Data::locate` tries, in order: `--data`; an explicit data root
(`config.toml` `data_root`, `$F2_DATA_ROOT`); **the bundled data**
(`frogger::data::bundled_data_dirs`): `$APPDIR/usr/share/game-data` (AppImage),
`<exe dir>/game-data` (Windows / Linux folder), `<exe dir>/../Resources/game-data` (macOS);
then the default data root, `$F2_GAME_DIR` and `./game-data`. Each is a rendition or a folder
of renditions (`config.toml` `rendition` picks one when several exist). An exported build
therefore runs from its own data on a machine with no import, and uses the player's normal
install only when they pointed the config at it explicitly.

## Windows

`frogger.exe --export windows [--out DIR]` (`crates/f2-package/src/windows.rs`) writes:

```
<out>/Frogger 2 - Swampy's Revenge/
  Frogger 2 - Swampy's Revenge.exe     frogger.exe, rebranded (below)
  NOTICE, README.md, licenses/         copied from the app's folder when present
  game-data/<rendition>/               game/, music/, video/, manifest.json
```

- The exe is the running `frogger.exe` with its resources rewritten by `editpe` (the code is
  untouched): every `RT_ICON`/`RT_GROUP_ICON` is removed and one icon group is added from the
  branding icon at 16, 20, 24, 32, 40, 48, 64, 96, 128 px (32-bit DIBs) and 256 px (PNG); the
  version info's `ProductName` and `FileDescription` become the title (Explorer's "File
  description", Task Manager, the taskbar's pinned name).
- The exe is not signed (neither is the release build): SmartScreen may warn the first time.
- Folder only; zip it to move it.

**Tested** (WSL, cross-built with `tools/package/build-windows.sh`): the Windows `frogger.exe`
exported itself (`--data <usa> --export windows --out C:\f2-export-test\out`, 466 MB, ~8 s).
The shell's view of the exported exe, read with a small native check (`ExtractAssociatedIconW`,
`PrivateExtractIconsW` at 256/48/16 px, `GetFileVersionInfoW`; PowerShell's
`[System.Drawing.Icon]::ExtractAssociatedIcon` does the same): the rounded game art at every
size (the source exe shows the recomp's own icon), `ProductName`/`FileDescription` "Frogger 2:
Swampy's Revenge". Run with `--headless --frames 60 --frontend --screenshot` and no `--data`
(empty `F2_CONFIG_DIR`) it loaded `<folder>\game-data\usa` and rendered the front end.

## macOS

`frogger --export macos` (`crates/f2-package/src/macos.rs`) copies the running `.app`
(permissions and symlinks kept) to `<out>/Frogger 2 - Swampy's Revenge.app` and edits it:

- `Contents/Info.plist`: `CFBundleName` and `CFBundleDisplayName` = the title,
  `CFBundleIdentifier` = `io.github.NeilGraham.SwampysRevengeRecomp.game` (its own identity:
  separate from the recomp app in Launchpad, privacy prompts and controller permissions),
  `CFBundleIconFile` = `game-icon` (`CFBundleIconName` removed so the asset catalog, if any,
  does not win). Other keys (executable, minimum OS, high-DPI, controllers) are kept.
- `Contents/Resources/game-icon.icns`, written in Rust from the branding icon: PNG entries
  `ic11` 32, `ic12` 64, `ic07` 128, `ic13`/`ic08` 256, `ic14`/`ic09` 512, `ic10` 1024 (macOS
  10.7+). The recomp's own `.icns` is removed.
- `Contents/Resources/game-data/<rendition>/`.

**Signing.** Editing the bundle invalidates its code-signature seal (`_CodeSignature/`).
On macOS the export re-signs the copy ad hoc (`codesign --force --deep -s -`, part of base
macOS), which is all an Apple Silicon Mac needs to run it locally. The macOS export only runs
on macOS (`Target::exportable`), but `f2_package::export` itself works anywhere; a bundle
produced elsewhere, or one copied to another Mac (downloads and AirDrop add the quarantine
flag; Gatekeeper then reports it as "damaged"), needs once, in Terminal:

```sh
xattr -dr com.apple.quarantine "Frogger 2 - Swampy's Revenge.app"
codesign --force --deep -s - "Frogger 2 - Swampy's Revenge.app"
```

The executable's own (linker / ad-hoc) signature is untouched by the export, so a copy that
is not quarantined launches even before re-signing; only Gatekeeper's bundle check fails.
No pure-Rust signer is used (the only complete one, `apple-codesign`, is far larger than this
crate).

**Tested**: unit tests on Linux (`Info.plist` round trip with the keys above, `.icns` entry
table decoded back to PNGs of the right sizes, data layout, executable permissions kept) and
`cargo check --target aarch64-apple-darwin -p f2-package -p frogger --no-default-features
--features gpu`. **Not yet run on a Mac** (export, launch, Finder icon, codesign).

## Linux

`frogger --export linux` (`crates/f2-package/src/linux.rs`) has two forms, by what the
running app is:

**From the AppImage** (`$APPIMAGE`): `<out>/Frogger 2 - Swampy's Revenge.AppImage`, made
without `appimagetool` or squashfs-tools:

1. The runtime: the bytes of the source AppImage before its squashfs (the ELF's size from its
   header, as `--appimage-offset` computes it), with the `.upd_info` (update information,
   which would "update" the export to the plain recomp), `.sha256_sig` and `.sig_key`
   sections zeroed.
2. A new squashfs (written with `backhand`, gzip level 6, 128 KiB blocks; the type 2 runtime
   reads zlib, zstd and xz) containing the source AppImage's AppDir (read with `backhand`;
   appimagetool's default zstd is decoded with `ruzstd`) except its `.desktop` files,
   `.DirIcon`, root icon, `usr/share/icons`, `usr/share/applications`, `usr/share/metainfo`
   and any previous `usr/share/game-data`, plus:

```
frogger-2-swampys-revenge.desktop          Name=<title>, Icon=frogger-2-swampys-revenge
frogger-2-swampys-revenge.png, .DirIcon    256 px branding icon
usr/share/applications/frogger-2-swampys-revenge.desktop
usr/share/icons/hicolor/{256x256,512x512}/apps/frogger-2-swampys-revenge.png
usr/share/game-data/<rendition>/           game/, music/, video/, manifest.json
```

`AppRun`, the binaries and the licenses are kept as they are. The game finds the data through
`$APPDIR` (set by the runtime when mounted and with `APPIMAGE_EXTRACT_AND_RUN=1`).

**From a folder** (release tarball, `target/release`): `<out>/Frogger 2 - Swampy's Revenge/`
with `frogger`, `NOTICE`/`README.md`/`licenses/` when present, the `.desktop` file and PNG
icon above, and `game-data/<rendition>/` (found next to the binary).

**Tested** (WSL2, x86_64): `tools/package/package.sh linux` with appimagetool (continuous,
zstd) built the recomp AppImage; that AppImage exported itself in ~7 s
(`--data game-data/usa --export linux`, 437 MB); `unsquashfs -s/-ll` reads the result (gzip,
the layout above); the export run with `--headless --frames 60 --frontend --screenshot` and
no `--data` (empty `F2_CONFIG_DIR`, no `F2_DATA_ROOT`) loaded
`/tmp/.mount_*/usr/share/game-data/usa` via FUSE and, identically, from
`APPIMAGE_EXTRACT_AND_RUN=1`; `--appimage-updateinformation` is empty. The folder export from
`target/release/frogger` ran the same way from `<folder>/game-data/usa`. Unit tests build a
fake AppImage (fake ELF runtime + squashfs AppDir), export it and read it back.
