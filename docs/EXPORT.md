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
Cutting a release, the CI jobs, pinned toolchains and secrets: `docs/BUILDING.md` "Releases / CI".

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

## Android

**Runtime** = the shell APK: the normal Android app built by `tools/android/build-apk.sh`
(`target/android/swampys-revenge-recomp.apk`; release asset
`swampys-revenge-recomp-<v>-android.apk`), package `io.github.neilgraham.swampysrevengerecomp`,
no game data. `--runtime` takes the APK, or a folder holding exactly one `.apk`.

**Output**: `<out>/<short title>.apk` (`Frogger 2.apk`), package
`io.github.neilgraham.swampysrevengerecomp.game` (installs next to the shell app; its own
`Android/data/<package>/files/` for data, settings and saves).

What `crates/f2-package/src/android.rs` does (pure Rust, no SDK tools):

1. Reads the shell (`android/zip.rs`), drops its v1 signature files (`META-INF/*.SF|RSA|MF`).
2. Rewrites `AndroidManifest.xml` (`android/axml.rs`: the binary XML's string pool is decoded,
   new strings appended, re-encoded with new offsets and chunk sizes; attributes re-pointed):
   `package` = the new id, the activity name fully qualified
   (`io.github.neilgraham.swampysrevengerecomp.MainActivity`, the dex is unchanged),
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
    --runtime target/android/swampys-revenge-recomp.apk --out /tmp/f2-export
$ANDROID_HOME/build-tools/35.0.0/apksigner verify --verbose "/tmp/f2-export/Frogger 2.apk"
$ANDROID_HOME/build-tools/35.0.0/aapt2 dump badging "/tmp/f2-export/Frogger 2.apk"
adb install -r "/tmp/f2-export/Frogger 2.apk"
adb shell am start -n io.github.neilgraham.swampysrevengerecomp.game/io.github.neilgraham.swampysrevengerecomp.MainActivity
```

Unit tests (`cargo test -p f2-package`): string pool round trips (UTF-8/UTF-16, styles),
manifest edits, zip alignment, a v2 signature checked like apksigner (block layout, content
digest, ECDSA signature, certificate key), key persistence, and a full export of a fake
shell; `cargo test -p frogger-android` covers the bundle index and unpacking/resume.
