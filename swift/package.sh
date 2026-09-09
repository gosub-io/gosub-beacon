#!/usr/bin/env bash
#
# Assemble Gosub Beacon into a .app bundle and wrap it in a DMG.
#
# `swift run` produces a bare executable that finds its Rust dylib through an rpath into
# this working copy -- fine on the machine that built it, dead anywhere else. This makes the
# thing you can actually hand to someone: the dylib travels inside the bundle, the binary
# looks for it at @executable_path/../Frameworks, and the whole app is signed (ad hoc) so
# Apple Silicon will run it at all.
#
#     ./package.sh            release build, .app and .dmg into swift/build/
#     ./package.sh --app      stop after the .app
#
# NOT notarized and NOT signed with a Developer ID, which is deliberate for now: this is a
# demo build. macOS quarantines anything downloaded, so whoever you send it to opens it the
# first time with right-click -> Open (or `xattr -dr com.apple.quarantine "Gosub Beacon.app"`).
# Double-clicking a quarantined unsigned app gives "cannot be opened because the developer
# cannot be verified", which looks like a broken download and is not one.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/.." && pwd)"
out="$here/build"
app="$out/Gosub Beacon.app"
dmg="$out/GosubBeacon.dmg"
app_only=false
volume="Gosub Beacon"
[[ "${1:-}" == "--app" ]] && app_only=true

# The version the About window shows, taken from the workspace so there is one answer.
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"

say() { printf '\033[1m==>\033[0m %s\n' "$1"; }

# ── build ─────────────────────────────────────────────────────────────────────
#
# Release on both sides. A debug build works but ships an unoptimised browser engine, which
# is a poor first impression of an engine.

say "building the Rust side (release)"
cargo build --manifest-path "$root/Cargo.toml" -p beacon-ffi --release

say "building the Swift side (release)"
swift build --package-path "$here" -c release
bin="$(swift build --package-path "$here" -c release --show-bin-path)"

# ── assemble ──────────────────────────────────────────────────────────────────

say "assembling $(basename "$app")"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Frameworks" "$app/Contents/Resources"

cp "$bin/BeaconMac" "$app/Contents/MacOS/BeaconMac"
cp "$root/target/release/libbeacon.dylib" "$app/Contents/Frameworks/libbeacon.dylib"

# SwiftPM keeps the target's resources in its own bundle, and `Bundle.module` looks for it
# beside the executable or in Contents/Resources. Forget this and the About window's artwork
# is not merely missing -- Bundle.module traps, and the app dies the moment you open it.
if [[ -d "$bin/BeaconMac_BeaconMac.bundle" ]]; then
    cp -R "$bin/BeaconMac_BeaconMac.bundle" "$app/Contents/Resources/"
else
    echo "warning: no SwiftPM resource bundle found in $bin -- the About window will crash" >&2
fi

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>              <string>Gosub Beacon</string>
    <key>CFBundleDisplayName</key>       <string>Gosub Beacon</string>
    <key>CFBundleIdentifier</key>        <string>io.gosub.beacon</string>
    <key>CFBundleExecutable</key>        <string>BeaconMac</string>
    <key>CFBundleIconFile</key>          <string>AppIcon</string>
    <key>CFBundlePackageType</key>       <string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key>           <string>$version</string>
    <key>LSMinimumSystemVersion</key>    <string>13.0</string>
    <key>NSHighResolutionCapable</key>   <true/>
    <!-- The page is rendered on the GPU; let macOS keep a laptop on its integrated one. -->
    <key>NSSupportsAutomaticGraphicsSwitching</key> <true/>
</dict>
</plist>
PLIST

printf 'APPL????' > "$app/Contents/PkgInfo"

# ── icon ──────────────────────────────────────────────────────────────────────
#
# packaging/icon.png is the Beacon lighthouse on a 1024 square. iconutil wants every size
# named exactly so, and both scales of each, or it refuses the set.

if [[ -f "$here/packaging/icon.png" ]]; then
    say "building the icon"
    iconset="$out/AppIcon.iconset"
    rm -rf "$iconset"; mkdir -p "$iconset"
    for size in 16 32 128 256 512; do
        sips -z $size $size "$here/packaging/icon.png" --out "$iconset/icon_${size}x${size}.png" >/dev/null
        sips -z $((size * 2)) $((size * 2)) "$here/packaging/icon.png" \
            --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
    done
    iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"
    rm -rf "$iconset"
fi

# ── make it relocatable ───────────────────────────────────────────────────────
#
# As built, the executable names the dylib by its absolute path in this working copy and
# carries rpaths pointing at target/debug. Both have to go, or the app runs only here.

say "pointing the binary at its own copy of the dylib"
install_name_tool -id "@rpath/libbeacon.dylib" "$app/Contents/Frameworks/libbeacon.dylib"

# Whatever the linker recorded -- an absolute path, or @rpath already.
old="$(otool -L "$app/Contents/MacOS/BeaconMac" | awk '/libbeacon\.dylib/ {print $1; exit}')"
if [[ -n "$old" && "$old" != "@rpath/libbeacon.dylib" ]]; then
    install_name_tool -change "$old" "@rpath/libbeacon.dylib" "$app/Contents/MacOS/BeaconMac"
fi

install_name_tool -add_rpath "@executable_path/../Frameworks" "$app/Contents/MacOS/BeaconMac"
# Drop the build-tree rpaths: they cannot resolve elsewhere, and they leak the path this was
# built in to anyone who runs otool on it.
while read -r stale; do
    [[ -n "$stale" ]] && install_name_tool -delete_rpath "$stale" "$app/Contents/MacOS/BeaconMac" 2>/dev/null || true
done < <(otool -l "$app/Contents/MacOS/BeaconMac" | awk '/ path /{print $2}' | grep 'target/debug' || true)

# ── sign ──────────────────────────────────────────────────────────────────────
#
# Ad hoc, and not optional: install_name_tool invalidates the signature SwiftPM applied, and
# an arm64 binary with a broken signature is killed on launch rather than merely warned about.
# Inside out -- the dylib first, then the bundle that contains it.

say "signing (ad hoc)"
codesign --force --sign - "$app/Contents/Frameworks/libbeacon.dylib"
codesign --force --sign - "$app"
codesign --verify --deep --strict "$app" && say "signature verifies"

if $app_only; then
    say "done: $app"
    exit 0
fi

# ── dmg ───────────────────────────────────────────────────────────────────────────
#
# The window a DMG opens in -- background picture, size, where the two icons sit -- lives in
# a .DS_Store inside the image, and Finder is the only thing that writes one macOS still
# believes. Tools that compose the file themselves (dmgbuild and friends) produce records
# that were correct for years and are now ignored: on macOS 26 the background simply does
# not appear. Comparing a working image (Firefox's) with a composed one shows why -- Finder
# writes a ~950 byte alias and a ~1500 byte bookmark in a format mac_alias cannot even
# parse, where the composed file has 364 and 656.
#
# So this drives Finder over AppleScript, which is what every shipping app's toolchain does.
# It needs the running user's Automation permission for Finder; the first run may raise a
# prompt, and in a session that cannot show one (CI) the styling is skipped and a plain
# image is built instead.

say "building the disk image"
staging="$out/dmg"
rm -rf "$staging"; mkdir -p "$staging/.background"
cp -R "$app" "$staging/"
ln -s /Applications "$staging/Applications"

# A DMG window is measured in points, so the 1536x1024 artwork is the 2x representation of a
# 768x512 window. tiffutil folds both into one file and Finder picks the right one, which is
# what keeps it sharp on a Retina display instead of upscaled.
background=""
if [[ -f "$here/packaging/dmg-background.png" ]]; then
    sips -z 512 768 "$here/packaging/dmg-background.png" --out "$staging/.background/1x.png" >/dev/null
    if tiffutil -cathidpicheck "$staging/.background/1x.png" "$here/packaging/dmg-background.png" \
        -out "$staging/.background/background.tiff" >/dev/null 2>&1; then
        background="background.tiff"
    else
        cp "$staging/.background/1x.png" "$staging/.background/background.png"
        background="background.png"
    fi
    rm -f "$staging/.background/1x.png"
fi

# Read-write first: Finder has to be able to write its .DS_Store into the mounted volume,
# which a compressed image cannot do. It is converted at the end.
rw="$out/rw.dmg"
rm -f "$rw" "$dmg"
hdiutil create -volname "$volume" -srcfolder "$staging" -ov -format UDRW -quiet "$rw"
hdiutil attach "$rw" -nobrowse -quiet
mounted="/Volumes/$volume"

styled=false
if [[ -n "$background" ]] && osascript -e 'tell application "Finder" to count windows' >/dev/null 2>&1; then
    say "asking Finder to lay the window out"
    # {left, top, right, bottom}: 768x512 at (200, 120). The icons go in the clear water band
    # either side of centre, where neither covers the submarine below nor the wordmark above.
    osascript <<APPLESCRIPT >/dev/null || true
tell application "Finder"
    tell disk "$volume"
        open
        set current view of container window to icon view
        set toolbar visible of container window to false
        set statusbar visible of container window to false
        set the bounds of container window to {200, 120, 968, 632}
        set options to the icon view options of container window
        set arrangement of options to not arranged
        set icon size of options to 128
        set text size of options to 12
        set background picture of options to file ".background:$background"
        set position of item "$(basename "$app")" of container window to {210, 235}
        set position of item "Applications" of container window to {558, 235}
        close
        open
        update without registering applications
        delay 2
    end tell
end tell
APPLESCRIPT
    styled=true
else
    echo "note: Finder is not scriptable from this session -- building a plain image" >&2
fi

sync
# Finder can hold the volume for a moment after it is told to close.
for _ in 1 2 3 4 5; do
    hdiutil detach "$mounted" -quiet 2>/dev/null && break || sleep 2
done

hdiutil convert "$rw" -format UDZO -o "$dmg" -quiet
rm -f "$rw"
rm -rf "$staging"

$styled && say "styled with packaging/dmg-background.png"
say "done: $dmg ($(du -h "$dmg" | cut -f1))"
