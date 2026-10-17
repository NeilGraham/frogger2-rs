# Android

The Android port runs the same `frogger::app::App` as the desktop executable (simulation,
renderer, audio, FMV, menus, saves) inside a NativeActivity. Status (2026-10-14): **plays on
an AYANEO Pocket EVO** (Android 13, Adreno 740, Vulkan; imported from the Redump zip with the
file picker) and in the Android 15 emulator (x86_64, SwiftShader Vulkan): boot FMVs, frontend,
story levels, touch controls, gamepad, background/resume.

## Layout

| Piece | Where |
|---|---|
| Native host (`android_main`) | `crates/frogger-android/src/host.rs`: `android-activity` main loop (not winit, see below), wgpu surface lifecycle, input, render-resolution pass, frame loop |
| Gamepad / keyboard mapping | `crates/frogger-android/src/pad.rs`: `KEYCODE_*` and joystick axes (sticks, HAT d-pad, triggers) → player 1's `RawPad`, keyboard → DirectInput codes |
| Overlay menu | `crates/frogger-android/src/menu.rs`: swipe in from the left edge; touch controls on/off (`android.toml`) |
| Touch controls | `crates/frogger-android/src/touch.rs`: d-pad, PlayStation face buttons, L1/L2/R1/R2, Select/Start; procedurally drawn atlas |
| Import screen | `crates/frogger-android/src/import.rs`: finds a disc image in the app folder, runs `f2_import` on a thread with progress |
| Bundled game (exported APKs) | `crates/frogger-android/src/bundle.rs`: `assets/game-data/bundle.json` index, one-time unpack into `files/data/<rendition>/` |
| Logging | `crates/frogger-android/src/logcat.rs`: stdout/stderr → logcat tag `frogger` |
| Java activity | `packaging/android/java/.../MainActivity.java`: `NativeActivity` subclass for immersive full screen, keep-screen-on, the system file picker and a system-gesture exclusion on the left edge (the menu swipe) |
| Manifest | `packaging/android/AndroidManifest.xml` (package `io.github.neilgraham.frogger2rs`, min API 26, target 35, landscape, launcher + Android TV launcher) |
| Build | `tools/android/build-apk.sh` (no Gradle), `apk_add.py`; launcher icon from `packaging/android/res` (placeholder) |

Why not winit: winit 0.30's Android backend drains the input queue itself and turns every
motion event into touch positions (AXIS_X/Y only), so analogue sticks lost their meaning and
d-pads that report HAT axes (most built-in and Xbox-style controllers) never reached the game.
The host runs `android-activity` directly and reads the motion events' source and axes.

Shared-crate changes made for Android: `frogger`'s `fontgen` feature (separate from
`desktop`); the built-in 5×7 font moved to `frogger::tinyfont`; the present pass moved into the
library (`frogger::present`); `WinInfo::fixed_mode` (Display page without window mode /
refresh rate); `Renderer::overlay` in `f2-engine` (a UI pass that draws over the presented
frame instead of clearing it; off by default, so the game's rendering is unchanged).

## Building

One-time setup (what this repo's dev machine uses):

```sh
sudo apt-get install openjdk-17-jdk-headless
# Android command-line tools into ~/android/sdk/cmdline-tools/latest, then:
sdkmanager "platform-tools" "platforms;android-35" "build-tools;35.0.0" "ndk;27.2.12479018"
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk
```

Then:

```sh
tools/android/build-apk.sh                 # release, arm64-v8a + x86_64 -> target/android/frogger2-rs.apk
tools/android/build-apk.sh --abis x86_64   # emulator-only, faster
```

The script builds `libfrogger_android.so` with cargo-ndk (API 26; the C++ runtime that cpal's
Oboe backend needs is linked statically, see `build.rs`), links the manifest and icon with
aapt2, compiles the activity with javac + d8, page-aligns (`zipalign -P 16`, 16 KB pages) and
signs. Without `F2_ANDROID_KEYSTORE` it signs with the standard debug key
(`~/.android/debug.keystore`, created if missing): fine for sideloading, not for a store.

## Installing and the game data

```sh
adb install -r target/android/frogger2-rs.apk
```

No game data ships in the APK. Everything lives in the app's external files folder,
`/sdcard/Android/data/io.github.neilgraham.frogger2rs/files/` (`data/`, `config.toml`, `saves/`, `mods/`).
On first start with no data the import screen offers:

- **Choose disc image**: the system file picker. Pick the Redump `.zip`, an `.iso`, or for
  BIN/CUE select both the `.cue` and the `.bin`. The activity copies them into `files/import/`
  (shown as "Copying ... MB"), the import runs, and `files/import/` is deleted afterwards.
- **adb**: `adb push "Frogger 2 - Swampy's Revenge (USA).zip" /sdcard/Android/data/io.github.neilgraham.frogger2rs/files/import/`.
  Files are picked up once they have stopped changing for 3 s.
- **Already imported on a PC**: copy the rendition folder (e.g. `game-data/usa`, the one with
  `game/`, `music/`, `video/`) to `files/data/usa/` and start the app.

**Exported APKs** (`frogger --export android` on a desktop, docs/EXPORT.md "Android") are
this app with the player's game inside, package `io.github.neilgraham.frogger2rs.game`,
label "Frogger 2" and the game-art icon. On the first start they unpack the bundled data
into their own `files/data/<rendition>/` (`crates/frogger-android/src/bundle.rs`, progress
screen) and go straight to the game; no import screen, no shortcut offer.

The modern (hi-res) fonts are generated from the system typeface on the desktop; on Android
they are only built if the typeface file is put in `files/data/<rendition>/fonts/`. Data
imported on a PC that already ran the game has them.

## Controls

- **Touch**: d-pad bottom left (slide between directions), face buttons bottom right (× ○ □ △
  in PlayStation colours; button prompts in the game use PlayStation icons), shoulders on the
  left/right edges, Select/Start at the top. They hide while a gamepad or keyboard is used and
  come back on the next touch.
- **Gamepads** (built-in, Bluetooth, USB) are player 1's joypad: buttons (`KEYCODE_BUTTON_*`),
  the d-pad as HAT axes or `KEYCODE_DPAD_*` keys, the left stick (moves, like the desktop's
  default bindings), right stick, analogue triggers as L2/R2. Rebinding works on the Controls
  settings page as on the desktop.
- **Keyboards**: mapped to the desktop's keys (F10 opens the settings screen).
- **Back** = Escape (closes menus; on the start screen it quits, like Esc on the desktop); with
  the overlay menu open it closes the menu.
- **Overlay menu**: swipe in from the middle of the left edge. The game pauses while it is
  open; it has one switch for now, **Touch controls on/off** (saved in `files/android.toml`).
  Tap outside the panel, "Close" or Back to return.
- Player 1 is `Joypad(0)` with the keyboard still active; local multiplayer needs more pads,
  which are not mapped yet.

## Resolution

Settings → Display → Resolution: **Auto** renders at the screen's resolution (shown, e.g.
`Auto (1920x1080)`). A fixed size (640x480 up to the screen size) renders at that size and is
scaled to fit with black bars, e.g. 640x480 shows the 4:3 picture centred on a 16:9 screen.
The touch controls and the overlay menu are drawn afterwards at the screen's resolution, so
they can sit in the bars.

## Lifecycle

Backgrounding destroys the native window: the host drops the wgpu surface (keeping the device,
renderer and game), mutes audio, lifts all touches and saves `config.toml`; on resume it makes a
new surface and continues. The simulation is paused while in the background (no frames run).

## Testing in the emulator (WSL2 works: needs `/dev/kvm` access)

```sh
sdkmanager "emulator" "system-images;android-35;google_apis;x86_64"
avdmanager create avd -n f2test -k "system-images;android-35;google_apis;x86_64" -d pixel_6
emulator -avd f2test -no-window -no-snapshot -gpu swiftshader_indirect -memory 4096 &
adb install -r target/android/frogger2-rs.apk
adb shell am start -n io.github.neilgraham.frogger2rs/.MainActivity
adb logcat -s frogger                       # game log
adb exec-out screencap -p > shot.png        # screenshot
adb shell input tap X Y                     # touch (2400x1080 landscape on pixel_6)
```

## Known gaps / next steps

- Not yet run on a physical device (Mali/Adreno Vulkan drivers, real AAudio latency).
- Rumble; more than one gamepad (local multiplayer).
- Touch layout is fixed (no editor, no opacity setting); Select/Start overlap the menu title.
- The overlay menu has only the touch-controls switch.
- Release signing / Play Store packaging (AAB) not done; distribution is sideloading only.
- Online play needs the INTERNET permission (present) but was not tested on Android.
