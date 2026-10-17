# Export to other platforms

Every desktop build of **Frogger 2: Swampy's Revenge (Rust)** can turn the player's imported game into
ready-to-run builds for other platforms, on any desktop OS, with no SDK, compiler, Docker or
WSL:

| Target | Output | Runtime used |
|---|---|---|
| This desktop OS (Windows / macOS / Linux) | a folder / `.app` / `.AppImage` with the game data inside | the running app itself |
| Android | `.apk` with the game data inside (installs and runs directly) | release asset `frogger2-rs-<v>-android.apk` (the shell app) |
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
  `https://github.com/NeilGraham/frogger2-rs/releases/download/v<version>/`, override
  with `F2_RUNTIME_BASE_URL`, or pass `--runtime`), the `--export` command.

## Runtimes

Built by CI (`.github/workflows/release.yml`) for each release tag `v<version>`, from source,
with no game data. Each console runtime is a zip produced by `tools/runtime/build-<target>.sh
OUT_DIR VERSION` (Linux, with that target's toolchain) and contains a `runtime.json`
(`{"target": "...", "version": "...", "format": 1, ...}`) describing its layout; the layout
details are documented in each section below by the packer that consumes it.
Cutting a release, the CI jobs, pinned toolchains and secrets: `docs/BUILDING.md` "Releases / CI".

The desktop packages are the release builds themselves (`tools/package/package.sh`):
`frogger2-rs-<v>-windows-<arch>.zip` (x86_64, aarch64),
`frogger2-rs-<v>-macos-universal.zip`, `Frogger2rs-<v>-<arch>.AppImage`
(x86_64, aarch64). Exporting for the host OS copies the running app, so an export has the
architecture(s) of the build that made it; the console and Android runtimes are the same for
every desktop architecture.

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

## Android

**Runtime** = the shell APK: the normal Android app built by `tools/android/build-apk.sh`
(`target/android/frogger2-rs.apk`; release asset
`frogger2-rs-<v>-android.apk`), package `io.github.neilgraham.frogger2rs`,
no game data. `--runtime` takes the APK, or a folder holding exactly one `.apk`.

**Output**: `<out>/<short title>.apk` (`Frogger 2.apk`), package
`io.github.neilgraham.frogger2rs.game` (installs next to the shell app; its own
`Android/data/<package>/files/` for data, settings and saves).

What `crates/f2-package/src/android.rs` does (pure Rust, no SDK tools):

1. Reads the shell (`android/zip.rs`), drops its v1 signature files (`META-INF/*.SF|RSA|MF`).
2. Rewrites `AndroidManifest.xml` (`android/axml.rs`: the binary XML's string pool is decoded,
   new strings appended, re-encoded with new offsets and chunk sizes; attributes re-pointed):
   `package` = the new id, the activity name fully qualified
   (`io.github.neilgraham.frogger2rs.MainActivity`, the dex is unchanged),
   `application android:label` = **short title**, `versionName` = `branding.version`,
   `versionCode` = major·10⁶ + minor·10³ + patch. `resources.arsc` is unchanged (its
   resource package keeps the shell's name, like `aapt2 --rename-manifest-package`;
   resources resolve by id).
3. Replaces the launcher PNGs at their zip paths (resource ids stay valid), at the shell's
   pixel sizes for every density: `ic_launcher_background.png` = `branding.art` at 80/108 of
   the layer on its average border colour (the masks show the inner 72/108: circles and
   squircles trim only the art's edges), `ic_launcher_foreground.png` transparent,
   `ic_launcher.png` (legacy, TV banner) = `branding.icon`.
4. Adds the game: every file of `util::rendition_files` as a stored, 4-byte-aligned entry
   `assets/game-data/<rendition>/<path>`, then the index `assets/game-data/bundle.json`
   (`{"format":1,"id":<sha256 of names, sizes, manifest.json>,"rendition","bytes","files":[[path,size]...]}`).
5. Zip-aligns (stored entries 4 bytes, stored `lib/**/*.so` 16 KiB pages, padding in a
   `0xd935` extra field) and signs with **APK Signature Scheme v2** only (ECDSA P-256 /
   SHA-256, algorithm 0x0201; minSdk 26 ≥ 24 so no v1 is needed; the signing block is padded
   to 4 KiB like apksigner).

**Signing key**: `<config>/keys/android-export.key` (PKCS#8 PEM, mode 0600) and
`android-export.cer` (self-signed X.509, DER), created on the first export
(`Job::keys_dir`, passed by `frogger::export`). Keep them: an exported APK installs over an
earlier one only when signed with the same key (otherwise uninstall first, which deletes the
app's saves). The certificate is deterministic in the key, so only the `.key` really matters.

**How the app finds the bundled data** (`crates/frogger-android/src/bundle.rs`): at start the
host opens `game-data/bundle.json` through the `AAssetManager`. If
`files/data/<rendition>/.bundle-id` holds the same id and every listed file has its size,
the game starts on that folder. Otherwise a progress screen ("Preparing the game (first
start only)") copies the assets into `files/data/<rendition>/` once (~450 MB, resumable:
`.bundle-partial`), writes `.bundle-id`, and starts the game; no import screen. The game
reads through the filesystem (mod VFS, music and FMV opened by path, fonts generated into
the rendition), which is why the data is unpacked rather than read in place from the APK.
A re-export with different data (new id) installed as an update unpacks again.

**Testing**:

```sh
export ANDROID_HOME=~/android/sdk
tools/android/build-apk.sh --abis x86_64           # the shell
cargo run --release -p frogger -- --data game-data/usa --export android \
    --runtime target/android/frogger2-rs.apk --out /tmp/f2-export
$ANDROID_HOME/build-tools/35.0.0/apksigner verify --verbose "/tmp/f2-export/Frogger 2.apk"
$ANDROID_HOME/build-tools/35.0.0/aapt2 dump badging "/tmp/f2-export/Frogger 2.apk"
adb install -r "/tmp/f2-export/Frogger 2.apk"
adb shell am start -n io.github.neilgraham.frogger2rs.game/io.github.neilgraham.frogger2rs.MainActivity
```

Unit tests (`cargo test -p f2-package`): string pool round trips (UTF-8/UTF-16, styles),
manifest edits, zip alignment, a v2 signature checked like apksigner (block layout, content
digest, ECDSA signature, certificate key), key persistence, and a full export of a fake
shell; `cargo test -p frogger-android` covers the bundle index and unpacking/resume.

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
size (the source exe shows the port's own icon), `ProductName`/`FileDescription` "Frogger 2:
Swampy's Revenge". Run with `--headless --frames 60 --frontend --screenshot` and no `--data`
(empty `F2_CONFIG_DIR`) it loaded `<folder>\game-data\usa` and rendered the front end.

## macOS

`frogger --export macos` (`crates/f2-package/src/macos.rs`) copies the running `.app`
(permissions and symlinks kept) to `<out>/Frogger 2 - Swampy's Revenge.app` and edits it:

- `Contents/Info.plist`: `CFBundleName` and `CFBundleDisplayName` = the title,
  `CFBundleIdentifier` = `io.github.NeilGraham.Frogger2rs.game` (its own identity:
  separate from the port app in Launchpad, privacy prompts and controller permissions),
  `CFBundleIconFile` = `game-icon` (`CFBundleIconName` removed so the asset catalog, if any,
  does not win). Other keys (executable, minimum OS, high-DPI, controllers) are kept.
- `Contents/Resources/game-icon.icns`, written in Rust from the branding icon: PNG entries
  `ic11` 32, `ic12` 64, `ic07` 128, `ic13`/`ic08` 256, `ic14`/`ic09` 512, `ic10` 1024 (macOS
  10.7+). The port's own `.icns` is removed.
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
   which would "update" the export to the plain port), `.sha256_sig` and `.sig_key`
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
zstd) built the port AppImage; that AppImage exported itself in ~7 s
(`--data game-data/usa --export linux`, 437 MB); `unsquashfs -s/-ll` reads the result (gzip,
the layout above); the export run with `--headless --frames 60 --frontend --screenshot` and
no `--data` (empty `F2_CONFIG_DIR`, no `F2_DATA_ROOT`) loaded
`/tmp/.mount_*/usr/share/game-data/usa` via FUSE and, identically, from
`APPIMAGE_EXTRACT_AND_RUN=1`; `--appimage-updateinformation` is empty. The folder export from
`target/release/frogger` ran the same way from `<folder>/game-data/usa`. Unit tests build a
fake AppImage (fake ELF runtime + squashfs AppDir), export it and read it back.

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

The format is reimplemented from LibOrbisPkg (OpenOrbis, LGPL-2.1, C#; algorithms only, no code
copied). The package keys are not in this repository: the first export downloads LibOrbisPkg's
`Util/Keys.cs` (pinned to a commit and checked against its SHA-256, `ps4/keys.rs`) into
`<config>/keys/LibOrbisPkg-Keys.cs`; `--ps4-keys <Keys.cs>` (or `F2_PS4_KEYS_FILE`) uses a local copy instead
(offline exports, no hash check) and `F2_PS4_KEYS_URL` downloads from a mirror (the SHA-256 must
still match). Dependencies are pure Rust (`sha2`,
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

