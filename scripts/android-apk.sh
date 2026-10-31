#!/usr/bin/env bash
# Build Gosub Beacon for Android as a signed APK, and install it with --install. arm64 by
# default; ANDROID_ABI=armeabi-v7a builds for 32-bit ARM Android instead.
#
#   scripts/android-apk.sh [--install] [--debug]
#
# Needs: the Android SDK (platform + build-tools), the NDK, `cargo install cargo-ndk`,
# `rustup toolchain install "$(cat scripts/release-toolchain)" --target aarch64-linux-android`
# (`armv7-linux-androideabi` for armeabi-v7a), and a JDK for javac. ANDROID_HOME defaults to
# ~/Android/Sdk and ANDROID_NDK_HOME to the newest NDK under it.
# Signing: BEACON_ANDROID_KEYSTORE (path), BEACON_ANDROID_KEY_ALIAS, BEACON_ANDROID_KEYSTORE_PASS
# and BEACON_ANDROID_KEY_PASS (defaults to the keystore's) sign with the release key. Without
# them it is the debug key, and a phone with a release build on it has to uninstall first,
# which takes the profile with it.
# Output: target/android/gosub-beacon-<version>-<abi>.apk.
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
# Every Rust build from this script uses scripts/release-toolchain; RUSTUP_TOOLCHAIN overrides it.
export RUSTUP_TOOLCHAIN=${RUSTUP_TOOLCHAIN:-$(cat "$ROOT/scripts/release-toolchain")}
export ANDROID_HOME=${ANDROID_HOME:-$HOME/Android/Sdk}
export ANDROID_NDK_HOME=${ANDROID_NDK_HOME:-$(find "$ANDROID_HOME/ndk" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -1)}
BUILD_TOOLS=$(find "$ANDROID_HOME/build-tools" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -1)
PLATFORM=$(find "$ANDROID_HOME/platforms" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -1)

# Target 34 rather than the platform's: 35 forces edge-to-edge, which would put the tab
# strip under the status bar.
# 29: the shared Downloads folder (MediaStore.Downloads) is Android 10 and up.
MIN_SDK=29
ABI=${ANDROID_ABI:-arm64-v8a}
TARGET_SDK=34
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)
# Android installs over a build only with a higher versionCode. The commit count climbs on
# main; a build from a branch behind it counts lower and needs an uninstall first.
VERSION_CODE=$(git -C "$ROOT" rev-list --count HEAD)

OUT=$ROOT/target/android
APK=$OUT/gosub-beacon-$VERSION-$ABI.apk
STAGE=$OUT/stage
rm -rf "$STAGE"
mkdir -p "$STAGE/apk/lib" "$STAGE/res"

PROFILE_FLAG=()
[ "$PROFILE" = release ] && PROFILE_FLAG=(--release)
(cd "$ROOT" && cargo ndk -t "$ABI" -P "$MIN_SDK" -o "$STAGE/apk/lib" build "${PROFILE_FLAG[@]}" -p beacon-android)

# Debug info makes up most of the library; the phone has no use for it.
"$ANDROID_NDK_HOME"/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-strip "$STAGE/apk/lib/$ABI/libbeacon_android.so"

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
# Beacon's own Java: the helper behind the date and time dialogs and the Downloads folder
# (packaging/android/java, called from beacon-egui's android::helper).
if ! command -v javac >/dev/null; then
    echo "javac not found; install a JDK (e.g. sudo apt install openjdk-21-jdk-headless)" >&2
    exit 1
fi
mkdir -p "$STAGE/java/helper"
find "$ROOT/packaging/android/java" -name '*.java' -print0 |
    xargs -0 javac --release 11 -nowarn -classpath "$PLATFORM/android.jar" -d "$STAGE/java/helper"
(cd "$STAGE/java/helper" && jar cf "$STAGE/java/helper.jar" .)

"$BUILD_TOOLS/d8" --release --min-api "$MIN_SDK" --lib "$PLATFORM/android.jar" \
    --output "$STAGE/apk" "$STAGE/java/classes.jar" "$KOTLIN_STDLIB" "$STAGE/java/helper.jar"

unzip -q -o "$AAR" 'res/xml/*' -d "$STAGE"
# Beacon's own resources: the backup and transfer rules the manifest names, and the adaptive
# icon around the foreground below.
cp -R "$ROOT"/packaging/android/res/. "$STAGE/res/"
# The adaptive icon's foreground is 108dp, of which a launcher shows the middle 72dp: the
# circle fills those, and the background colour (the sky's edge) fills a mask's corners.
for density in mdpi:108 hdpi:162 xhdpi:216 xxhdpi:324 xxxhdpi:432; do
    mkdir -p "$STAGE/res/mipmap-${density%:*}"
    (cd "$ROOT" && cargo run -q --release -p beacon-icon -- "${density#*:}" \
        "$STAGE/res/mipmap-${density%:*}/ic_launcher_foreground.png" 0.6667)
done
"$BUILD_TOOLS/aapt2" compile --dir "$STAGE/res" -o "$STAGE/res.zip"
"$BUILD_TOOLS/aapt2" link -o "$STAGE/unaligned.apk" \
    -I "$PLATFORM/android.jar" \
    --manifest "$ROOT/packaging/android/AndroidManifest.xml" \
    --min-sdk-version "$MIN_SDK" --target-sdk-version "$TARGET_SDK" \
    --version-code "$VERSION_CODE" --version-name "$VERSION" \
    "$STAGE/res.zip"

(cd "$STAGE/apk" && zip -qr "$STAGE/unaligned.apk" lib classes.dex)

# -P 16: 16 KB page alignment for the library, which Android 15+ devices may require.
"$BUILD_TOOLS/zipalign" -f -P 16 4 "$STAGE/unaligned.apk" "$STAGE/aligned.apk"

if [ -n "${BEACON_ANDROID_KEYSTORE:-}" ]; then
    : "${BEACON_ANDROID_KEY_ALIAS:?BEACON_ANDROID_KEYSTORE is set, so BEACON_ANDROID_KEY_ALIAS must be too}"
    : "${BEACON_ANDROID_KEYSTORE_PASS:?BEACON_ANDROID_KEYSTORE is set, so BEACON_ANDROID_KEYSTORE_PASS must be too}"
    export BEACON_ANDROID_KEY_PASS=${BEACON_ANDROID_KEY_PASS:-$BEACON_ANDROID_KEYSTORE_PASS}
    # env: keeps the passwords out of the process list.
    "$BUILD_TOOLS/apksigner" sign --ks "$BEACON_ANDROID_KEYSTORE" --ks-key-alias "$BEACON_ANDROID_KEY_ALIAS" \
        --ks-pass env:BEACON_ANDROID_KEYSTORE_PASS --key-pass env:BEACON_ANDROID_KEY_PASS \
        --out "$APK" "$STAGE/aligned.apk"
    SIGNED="the release key"
else
    KEYSTORE=$HOME/.android/debug.keystore
    if [ ! -f "$KEYSTORE" ]; then
        mkdir -p "$(dirname "$KEYSTORE")"
        keytool -genkeypair -keystore "$KEYSTORE" -storepass android -keypass android \
            -alias androiddebugkey -dname "CN=Android Debug,O=Android,C=US" \
            -keyalg RSA -keysize 2048 -validity 10000
    fi
    "$BUILD_TOOLS/apksigner" sign --ks "$KEYSTORE" --ks-pass pass:android --key-pass pass:android \
        --out "$APK" "$STAGE/aligned.apk"
    SIGNED="the debug key; set BEACON_ANDROID_KEYSTORE for a release"
fi
echo "built $APK ($(du -h "$APK" | cut -f1)), versionCode $VERSION_CODE, signed with $SIGNED"

if [ "$INSTALL" = 1 ]; then
    adb install -r "$APK"
    adb shell am start -n io.gosub.beacon/android.app.NativeActivity
fi
