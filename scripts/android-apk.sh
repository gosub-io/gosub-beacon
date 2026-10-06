#!/usr/bin/env bash
# Build Gosub Beacon for Android (arm64) as a debug-signed APK, and install it with --install.
#
#   scripts/android-apk.sh [--install] [--debug]
#
# Needs: the Android SDK (platform + build-tools), the NDK, `cargo install cargo-ndk`, and
# `rustup target add aarch64-linux-android`. ANDROID_HOME defaults to ~/Android/Sdk and
# ANDROID_NDK_HOME to the newest NDK under it. Output: target/android/gosub-beacon.apk.
set -euo pipefail

INSTALL=0
PROFILE=release
for arg in "$@"; do
    case "$arg" in
        --install) INSTALL=1 ;;
        --debug) PROFILE=debug ;;
        *) echo "unknown argument: $arg" >&2; exit 2 ;;
    esac
done

ROOT=$(cd "$(dirname "$0")/.." && pwd)
export ANDROID_HOME=${ANDROID_HOME:-$HOME/Android/Sdk}
export ANDROID_NDK_HOME=${ANDROID_NDK_HOME:-$(find "$ANDROID_HOME/ndk" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -1)}
BUILD_TOOLS=$(find "$ANDROID_HOME/build-tools" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -1)
PLATFORM=$(find "$ANDROID_HOME/platforms" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -1)

# Target 34 rather than the platform's: 35 forces edge-to-edge, which would put the tab
# strip under the status bar.
MIN_SDK=26
TARGET_SDK=34
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)

OUT=$ROOT/target/android
STAGE=$OUT/stage
rm -rf "$STAGE"
mkdir -p "$STAGE/apk/lib" "$STAGE/res/mipmap-xxxhdpi"

PROFILE_FLAG=()
[ "$PROFILE" = release ] && PROFILE_FLAG=(--release)
(cd "$ROOT" && cargo ndk -t arm64-v8a -P "$MIN_SDK" -o "$STAGE/apk/lib" build "${PROFILE_FLAG[@]}" -p beacon-android)

# Debug info makes up most of the library; the phone has no use for it.
"$ANDROID_NDK_HOME"/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-strip "$STAGE/apk/lib/arm64-v8a/libbeacon_android.so"

# The Java half of rustls-platform-verifier, which the engine's network stack checks
# certificates with. Upstream serves it as an .aar from a Maven branch on GitHub, at the
# version of the `rustls-platform-verifier-android` crate in Cargo.lock; pinned by checksum.
# Its classes are dexed into the APK, and its network security config (which lets Android
# fetch CRLs over plain HTTP, the only way some CAs publish them) goes into the resources.
VERIFIER_VERSION=$(grep -A1 '^name = "rustls-platform-verifier-android"$' "$ROOT/Cargo.lock" | sed -n 's/^version = "\(.*\)"/\1/p')
VERIFIER_SHA256=aa021794230fbc2f0be355999e2cf67398dd563de066a2e17001ffbd0b69101b
if [ "$VERIFIER_VERSION" != 0.2.0 ]; then
    echo "Cargo.lock has rustls-platform-verifier-android $VERIFIER_VERSION; update the pin in $0" >&2
    exit 1
fi
AAR=$OUT/rustls-platform-verifier-$VERIFIER_VERSION.aar
if [ ! -f "$AAR" ]; then
    curl -fsSL -o "$AAR.part" "https://github.com/rustls/rustls-platform-verifier/raw/maven-archive/android-release-support/maven/org/rustls/rustls-platform-verifier/$VERIFIER_VERSION/rustls-platform-verifier-$VERIFIER_VERSION.aar"
    echo "$VERIFIER_SHA256  $AAR.part" | sha256sum -c --quiet
    mv "$AAR.part" "$AAR"
fi
unzip -q -o "$AAR" classes.jar -d "$STAGE/java"
# It is Kotlin, so it needs the Kotlin standard library, which a Gradle build would pull in.
KOTLIN_STDLIB=$OUT/kotlin-stdlib-2.0.21.jar
if [ ! -f "$KOTLIN_STDLIB" ]; then
    curl -fsSL -o "$KOTLIN_STDLIB.part" \
        https://repo1.maven.org/maven2/org/jetbrains/kotlin/kotlin-stdlib/2.0.21/kotlin-stdlib-2.0.21.jar
    echo "f31cc53f105a7e48c093683bbd5437561d1233920513774b470805641bedbc09  $KOTLIN_STDLIB.part" | sha256sum -c --quiet
    mv "$KOTLIN_STDLIB.part" "$KOTLIN_STDLIB"
fi
"$BUILD_TOOLS/d8" --release --min-api "$MIN_SDK" --lib "$PLATFORM/android.jar" \
    --output "$STAGE/apk" "$STAGE/java/classes.jar" "$KOTLIN_STDLIB"

cp "$ROOT/packaging/flatpak/icons/io.gosub.beacon-256.png" "$STAGE/res/mipmap-xxxhdpi/ic_launcher.png"
unzip -q -o "$AAR" 'res/xml/*' -d "$STAGE"
"$BUILD_TOOLS/aapt2" compile --dir "$STAGE/res" -o "$STAGE/res.zip"
"$BUILD_TOOLS/aapt2" link -o "$STAGE/unaligned.apk" \
    -I "$PLATFORM/android.jar" \
    --manifest "$ROOT/packaging/android/AndroidManifest.xml" \
    --min-sdk-version "$MIN_SDK" --target-sdk-version "$TARGET_SDK" \
    --version-code 1 --version-name "$VERSION" \
    "$STAGE/res.zip"

(cd "$STAGE/apk" && zip -qr "$STAGE/unaligned.apk" lib classes.dex)

# -P 16: 16 KB page alignment for the library, which Android 15+ devices may require.
"$BUILD_TOOLS/zipalign" -f -P 16 4 "$STAGE/unaligned.apk" "$STAGE/aligned.apk"

KEYSTORE=$HOME/.android/debug.keystore
if [ ! -f "$KEYSTORE" ]; then
    mkdir -p "$(dirname "$KEYSTORE")"
    keytool -genkeypair -keystore "$KEYSTORE" -storepass android -keypass android \
        -alias androiddebugkey -dname "CN=Android Debug,O=Android,C=US" \
        -keyalg RSA -keysize 2048 -validity 10000
fi
"$BUILD_TOOLS/apksigner" sign --ks "$KEYSTORE" --ks-pass pass:android --key-pass pass:android \
    --out "$OUT/gosub-beacon.apk" "$STAGE/aligned.apk"
echo "built $OUT/gosub-beacon.apk ($(du -h "$OUT/gosub-beacon.apk" | cut -f1))"

if [ "$INSTALL" = 1 ]; then
    adb install -r "$OUT/gosub-beacon.apk"
    adb shell am start -n io.gosub.beacon/android.app.NativeActivity
fi
