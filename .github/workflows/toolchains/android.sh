#!/usr/bin/env bash
# Android SDK for tools/android/build-apk.sh (docs/ANDROID.md "Building"): a fresh SDK at
# $F2_ANDROID_SDK (default $HOME/android-sdk) with exactly cmdline-tools 12.0, platform 35,
# build-tools 35.0.0 and NDK 27.2.12479018 (build-apk.sh takes the newest of each it finds,
# so the runner's preinstalled SDK would not be pinned), the Android Rust targets and
# cargo-ndk 4.1.2. Needs a JDK 17+ on PATH (sdkmanager, javac, keytool).
. "$(dirname "$0")/lib.sh"
SDK="${F2_ANDROID_SDK:-$HOME/android-sdk}"
NDK_VER=27.2.12479018

fetch https://dl.google.com/android/repository/commandlinetools-linux-11076708_latest.zip \
    2d2d50857e4eb553af5a6dc3ad507a17adf43d115264b1afc116f95c92e5e258 "$DL/commandlinetools-linux-11076708_latest.zip"
if [ ! -x "$SDK/cmdline-tools/latest/bin/sdkmanager" ]; then
    rm -rf "$SDK/cmdline-tools"
    mkdir -p "$SDK/cmdline-tools"
    unzip -q "$DL/commandlinetools-linux-11076708_latest.zip" -d "$SDK/cmdline-tools"
    mv "$SDK/cmdline-tools/cmdline-tools" "$SDK/cmdline-tools/latest"
fi
SM="$SDK/cmdline-tools/latest/bin/sdkmanager"
yes | "$SM" --sdk_root="$SDK" --licenses >/dev/null || true
"$SM" --sdk_root="$SDK" "platforms;android-35" "build-tools;35.0.0" "ndk;$NDK_VER" >/dev/null

# The runner presets these to its own SDK/NDK; build-apk.sh reads them.
setenv ANDROID_HOME "$SDK"
setenv ANDROID_SDK_ROOT "$SDK"
setenv ANDROID_NDK_HOME "$SDK/ndk/$NDK_VER"
setenv ANDROID_NDK_ROOT "$SDK/ndk/$NDK_VER"

rustup target add aarch64-linux-android x86_64-linux-android
if [ "$(cargo ndk --version 2>/dev/null)" != "cargo-ndk 4.1.2" ]; then
    cargo install cargo-ndk --version 4.1.2 --locked
fi
ls "$SDK/platforms" "$SDK/build-tools" "$SDK/ndk"
echo "Android SDK ready"
