#!/usr/bin/env bash
# Builds the Android APK (no Gradle): native library via cargo-ndk, resources via aapt2,
# the Java activity via javac + d8, then zipalign + apksigner.
#
#   tools/android/build-apk.sh [--debug] [--abis arm64-v8a,x86_64] [--out DIR]
#
# Needs: ANDROID_HOME (default ~/android/sdk) with build-tools, a platform and the NDK;
# `cargo install cargo-ndk`; `rustup target add aarch64-linux-android x86_64-linux-android`;
# a JDK (javac, keytool). Signs with $F2_ANDROID_KEYSTORE (default: the Android debug key
# ~/.android/debug.keystore, created if missing; password $F2_ANDROID_KS_PASS or "android").
# Output: target/android/swampys-revenge-recomp.apk. See docs/ANDROID.md.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
HERE="$ROOT/tools/android"
PROFILE=release
ABIS=arm64-v8a,x86_64
OUT="$ROOT/target/android"
while [ $# -gt 0 ]; do
    case "$1" in
        --debug) PROFILE=dev ;;
        --abis) ABIS="$2"; shift ;;
        --out) OUT="$2"; shift ;;
        *) echo "usage: $0 [--debug] [--abis arm64-v8a,x86_64] [--out DIR]" >&2; exit 2 ;;
    esac
    shift
done

export ANDROID_HOME="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/android/sdk}}"
[ -d "$ANDROID_HOME" ] || { echo "ANDROID_HOME ($ANDROID_HOME) not found" >&2; exit 1; }
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$(ls -d "$ANDROID_HOME"/ndk/* | sort -V | tail -1)}"
BT=$(ls -d "$ANDROID_HOME"/build-tools/* | sort -V | tail -1)
JAR=$(ls -d "$ANDROID_HOME"/platforms/android-* | sort -V | tail -1)/android.jar
MIN_API=26
STRIP=$(ls "$ANDROID_NDK_HOME"/toolchains/llvm/prebuilt/*/bin/llvm-strip | head -1)

WORK="$OUT/build"
rm -rf "$WORK"
mkdir -p "$WORK/apk/lib" "$WORK/res" "$WORK/classes" "$WORK/dex"

echo "== native library ($ABIS, $PROFILE)"
targets=()
for a in ${ABIS//,/ }; do targets+=(-t "$a"); done
(cd "$ROOT" && cargo ndk --platform "$MIN_API" "${targets[@]}" -o "$WORK/apk/lib" build -p frogger-android --profile "$PROFILE")
# cargo-ndk copies libc++_shared.so when something links it (oboe); keep only what is used.
for so in "$WORK"/apk/lib/*/libfrogger_android.so; do
    [ "$PROFILE" = release ] && "$STRIP" --strip-debug "$so"
done

echo "== resources"
# Launcher icon (legacy + adaptive), generated from the SVG master by tools/icon/build-icons.sh.
cp -r "$ROOT/packaging/android/res/." "$WORK/res/"
"$BT/aapt2" compile --dir "$WORK/res" -o "$WORK/res.zip"
"$BT/aapt2" link -o "$WORK/base.apk" -I "$JAR" --manifest "$ROOT/packaging/android/AndroidManifest.xml" \
    --min-sdk-version "$MIN_API" "$WORK/res.zip"

echo "== java"
find "$ROOT/packaging/android/java" -name '*.java' > "$WORK/sources.txt"
javac -nowarn -source 11 -target 11 -classpath "$JAR" -d "$WORK/classes" @"$WORK/sources.txt" 2>&1 | grep -v "bootstrap classpath" || true
"$BT/d8" --release --min-api "$MIN_API" --lib "$JAR" --output "$WORK/apk" $(find "$WORK/classes" -name '*.class')

echo "== package"
python3 -I "$HERE/apk_add.py" "$WORK/base.apk" "$WORK/apk" lib classes.dex
"$BT/zipalign" -f -P 16 4 "$WORK/base.apk" "$WORK/aligned.apk"
KS="${F2_ANDROID_KEYSTORE:-$HOME/.android/debug.keystore}"
PASS="${F2_ANDROID_KS_PASS:-android}"
if [ ! -f "$KS" ]; then
    mkdir -p "$(dirname "$KS")"
    keytool -genkeypair -keystore "$KS" -storepass "$PASS" -keypass "$PASS" -alias androiddebugkey \
        -keyalg RSA -keysize 2048 -validity 10000 -dname "CN=Android Debug,O=Android,C=US" >/dev/null
fi
"$BT/apksigner" sign --ks "$KS" --ks-pass "pass:$PASS" --out "$OUT/swampys-revenge-recomp.apk" "$WORK/aligned.apk"
"$BT/apksigner" verify "$OUT/swampys-revenge-recomp.apk"
ls -l "$OUT/swampys-revenge-recomp.apk"
