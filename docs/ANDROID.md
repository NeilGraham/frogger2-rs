# Android

The Android port runs the same `frogger::app::App` as the desktop executable (simulation,
renderer, audio, FMV, menus, saves) inside a NativeActivity. Status (2026-10-14): **builds,
imports a disc image on the device and plays in the Android 15 emulator** (x86_64, SwiftShader
Vulkan): boot FMVs, frontend, story levels, touch controls, background/resume.
Not yet run on a physical phone.

## Layout

| Piece | Where |
|---|---|
| Native host (`android_main`) | `crates/frogger-android/src/host.rs`: winit `android-native-activity` event loop, wgpu surface lifecycle, input, frame loop |
| Touch controls | `crates/frogger-android/src/touch.rs`: d-pad, PlayStation face buttons, L1/L2/R1/R2, Select/Start; procedurally drawn atlas |
| Import screen | `crates/frogger-android/src/import.rs`: finds a disc image in the app folder, runs `f2_import` on a thread with progress |
| Logging | `crates/frogger-android/src/logcat.rs`: stdout/stderr → logcat tag `frogger` |
| Java activity | `packaging/android/java/.../FroggerActivity.java`: `NativeActivity` subclass for immersive full screen, keep-screen-on and the system file picker |
| Manifest | `packaging/android/AndroidManifest.xml` (package `org.frogger2rs.game`, min API 26, target 35, landscape, launcher + Android TV launcher) |
| Build | `tools/android/build-apk.sh` (no Gradle), `make_icon.py` (procedural launcher icon), `apk_add.py` |

Shared-crate changes made for Android: `f2_platform::window::run_with` (host-built event loop)
and `WindowApp::suspended` (the window/surface goes away in the background); the Android Back
button maps to Escape; `frogger`'s `fontgen` feature (separate from `desktop`); the built-in
5×7 font moved to `frogger::tinyfont`.

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
tools/android/build-apk.sh                 # release, arm64-v8a + x86_64 -> target/android/frogger2.apk
tools/android/build-apk.sh --abis x86_64   # emulator-only, faster
```

The script builds `libfrogger_android.so` with cargo-ndk (API 26; the C++ runtime that cpal's
Oboe backend needs is linked statically, see `build.rs`), links the manifest and icon with
aapt2, compiles the activity with javac + d8, page-aligns (`zipalign -P 16`, 16 KB pages) and
signs. Without `F2_ANDROID_KEYSTORE` it signs with the standard debug key
(`~/.android/debug.keystore`, created if missing): fine for sideloading, not for a store.

## Installing and the game data

```sh
adb install -r target/android/frogger2.apk
```

No game data ships in the APK. Everything lives in the app's external files folder,
`/sdcard/Android/data/org.frogger2rs.game/files/` (`data/`, `config.toml`, `saves/`, `mods/`).
On first start with no data the import screen offers:

- **Choose disc image**: the system file picker. Pick the Redump `.zip`, an `.iso`, or for
  BIN/CUE select both the `.cue` and the `.bin`. The activity copies them into `files/import/`
  (shown as "Copying ... MB"), the import runs, and `files/import/` is deleted afterwards.
- **adb**: `adb push "Frogger 2 - Swampy's Revenge (USA).zip" /sdcard/Android/data/org.frogger2rs.game/files/import/`.
  Files are picked up once they have stopped changing for 3 s.
- **Already imported on a PC**: copy the rendition folder (e.g. `game-data/usa`, the one with
  `game/`, `music/`, `video/`) to `files/data/usa/` and start the app.

The modern (hi-res) fonts are generated from the system typeface on the desktop; on Android
they are only built if the typeface file is put in `files/data/<rendition>/fonts/`. Data
imported on a PC that already ran the game has them.

## Controls

- **Touch**: d-pad bottom left (slide between directions), face buttons bottom right (× ○ □ △
  in PlayStation colours; button prompts in the game use PlayStation icons), shoulders on the
  left/right edges, Select/Start at the top. They hide while a gamepad or keyboard is used and
  come back on the next touch.
- **Gamepads** (Bluetooth/USB): buttons arrive as Android key events (`KEYCODE_BUTTON_*`) and are
  player 1's joypad; the d-pad arrives as arrow keys. Analogue sticks are not read yet (winit
  0.30 does not forward joystick motion events).
- **Back** = Escape (closes menus; on the start screen it quits, like Esc on the desktop).
- Player 1 is `Joypad(0)` with the keyboard still active; local multiplayer needs more pads,
  which are not mapped yet.

## Lifecycle

Backgrounding destroys the native window: the host drops the wgpu surface (keeping the device,
renderer and game), mutes audio, lifts all touches and saves `config.toml`; on resume it makes a
new surface and continues. The simulation is paused while in the background (no frames run).

## Testing in the emulator (WSL2 works: needs `/dev/kvm` access)

```sh
sdkmanager "emulator" "system-images;android-35;google_apis;x86_64"
avdmanager create avd -n f2test -k "system-images;android-35;google_apis;x86_64" -d pixel_6
emulator -avd f2test -no-window -no-snapshot -gpu swiftshader_indirect -memory 4096 &
adb install -r target/android/frogger2.apk
adb shell am start -n org.frogger2rs.game/.FroggerActivity
adb logcat -s frogger                       # game log
adb exec-out screencap -p > shot.png        # screenshot
adb shell input tap X Y                     # touch (2400x1080 landscape on pixel_6)
```

## Known gaps / next steps

- Not yet run on a physical device (Mali/Adreno Vulkan drivers, real AAudio latency).
- Analogue sticks, rumble, multiple gamepads (local multiplayer).
- Touch layout is fixed (no editor, no opacity setting); Select/Start overlap the menu title.
- Release signing / Play Store packaging (AAB) not done; distribution is sideloading only.
- Online play needs the INTERNET permission (present) but was not tested on Android.
