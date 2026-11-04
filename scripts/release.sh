#!/usr/bin/env bash
# Build and collect the release artifacts of the tagged version, and write SHA256SUMS.
#
#   scripts/release.sh --dmg <path-to-dmg>
#
# Run on Linux, in a clean checkout at the tag v<version> (the version in Cargo.toml), with
# ../gosub-engine at the engine tag beacon-v<version>. Both tags must be pushed: the flatpak
# fetches the engine at its tag, and the Windows zip comes from the CI run of the beacon tag.
# Here it builds the flatpak bundle and the arm64 and armv7 APKs, downloads the Windows zip,
# and takes the DMG built on the Mac with swift/package.sh. Every file name carries the build
# count and both commits, and a name that does not match the tags stops the release.
#
# Needs: flatpak with org.flatpak.Builder, what scripts/android-apk.sh needs, the
# BEACON_ANDROID_* signing variables (a debug-signed APK cannot install over a release), and
# gh. Output: target/release/<version>/ plus the gh command that creates the draft release.
# The whole procedure is in docs/releasing.md.
set -euo pipefail

DMG=
while [ $# -gt 0 ]; do
    case "$1" in
        --dmg) DMG=${2:?--dmg needs a path}; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
[ -n "$DMG" ] || { echo "usage: $0 --dmg <path-to-dmg>" >&2; exit 2; }

ROOT=$(cd "$(dirname "$0")/.." && pwd)
ENGINE_DIR=$(cd "$ROOT/../gosub-engine" && pwd)
REPO=gosub-io/gosub-beacon
ENGINE_REPO=gosub-io/gosub-engine
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)
TAG=v$VERSION
ENGINE_TAG=beacon-v$VERSION
MANIFEST=$ROOT/packaging/flatpak/io.gosub.beacon.yml
NOTES=docs/releases/$VERSION.md

die() { echo "$*" >&2; exit 1; }
say() { printf '\033[1m==>\033[0m %s\n' "$1"; }

# --- the tags ---

[ -f "$DMG" ] || die "no such file: $DMG"
: "${BEACON_ANDROID_KEYSTORE:?the APKs need the release key; see scripts/android-apk.sh}"

for dir in "$ROOT" "$ENGINE_DIR"; do
    [ -z "$(git -C "$dir" status --porcelain --untracked-files=no)" ] || die "$dir has uncommitted changes"
done
COMMIT=$(git -C "$ROOT" rev-parse HEAD)
ENGINE=$(git -C "$ENGINE_DIR" rev-parse HEAD)
[ "$(git -C "$ROOT" rev-parse -q --verify "refs/tags/$TAG^{commit}")" = "$COMMIT" ] ||
    die "$ROOT is not at tag $TAG"
[ "$(git -C "$ENGINE_DIR" rev-parse -q --verify "refs/tags/$ENGINE_TAG^{commit}")" = "$ENGINE" ] ||
    die "$ENGINE_DIR is not at tag $ENGINE_TAG"
# The commit a tag names on GitHub; an annotated tag lists it as <tag>^{}.
remote_tag() {
    git ls-remote --tags "https://github.com/$1.git" "refs/tags/$2" "refs/tags/$2^{}" | sort -k2 | tail -1 | cut -f1
}
[ "$(remote_tag "$REPO" "$TAG")" = "$COMMIT" ] || die "$TAG on $REPO is not $COMMIT: not pushed, or moved"
[ "$(remote_tag "$ENGINE_REPO" "$ENGINE_TAG")" = "$ENGINE" ] ||
    die "$ENGINE_TAG on $ENGINE_REPO is not $ENGINE: not pushed, or moved"

PINNED=$(sed -n 's/^ *commit: \([0-9a-f]\{40\}\)$/\1/p' "$MANIFEST")
[ "$PINNED" = "$ENGINE" ] || die "the flatpak manifest pins engine $PINNED, not $ENGINE_TAG ($ENGINE)"
grep -q "<release version=\"$VERSION\"" "$ROOT/packaging/flatpak/io.gosub.beacon.metainfo.xml" ||
    die "io.gosub.beacon.metainfo.xml has no <release> for $VERSION"
[ -f "$ROOT/$NOTES" ] || die "no release notes at $NOTES"

# The part every artifact name shares, by swift/package.sh's rules.
ID=$VERSION-$(git -C "$ROOT" rev-list --count HEAD)-${COMMIT:0:10}-engine-${ENGINE:0:10}
OUT=$ROOT/target/release/$VERSION
WORK=$ROOT/target/release/work
rm -rf "$OUT" "$WORK"
mkdir -p "$OUT" "$WORK"

# --- Linux: flatpak bundle ---

say "flatpak bundle"
# A copy of the manifest with absolute source paths, the engine tag beside its commit (the
# fetch then fails if they disagree), and the commits build.rs cannot read without .git.
sed -e "s|^\( *path: \)\.\./\.\.$|\1$ROOT|" \
    -e "s|^\( *\)- cargo-sources.json$|\1- $ROOT/packaging/flatpak/cargo-sources.json|" \
    -e "s|^\( *\)commit: $ENGINE$|&\n\1tag: $ENGINE_TAG|" \
    -e "s|^\( *\)CARGO_NET_OFFLINE: 'true'$|&\n\1BEACON_COMMIT: '${COMMIT:0:10}'\n\1BEACON_ENGINE_COMMIT: '${ENGINE:0:10}'|" \
    "$MANIFEST" >"$WORK/io.gosub.beacon.yml"
for line in "path: $ROOT" "- $ROOT/packaging/flatpak/cargo-sources.json" "tag: $ENGINE_TAG" "BEACON_COMMIT: "; do
    grep -qF -- "$line" "$WORK/io.gosub.beacon.yml" || die "could not rewrite the manifest: no '$line'"
done
# The state dir outlives a run: it caches the vendored crates and the Skia archive.
(cd "$WORK" && flatpak run org.flatpak.Builder --force-clean --user --install-deps-from=flathub \
    --state-dir="$ROOT/target/release/flatpak-state" --repo=flatpak-repo flatpak-build io.gosub.beacon.yml)
# --runtime-repo: installing the bundle then fetches the GNOME runtime from Flathub itself.
flatpak build-bundle --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
    "$WORK/flatpak-repo" "$OUT/GosubBeacon-$ID-x86_64.flatpak" io.gosub.beacon

# --- Android: one APK per ABI ---

for abi in arm64-v8a armeabi-v7a; do
    say "APK ($abi)"
    ANDROID_ABI=$abi "$ROOT/scripts/android-apk.sh"
    apk=$ROOT/target/android/GosubBeacon-$ID-$abi.apk
    [ -f "$apk" ] || die "scripts/android-apk.sh did not write $apk"
    cp "$apk" "$OUT/"
done

# --- Windows: the zip CI built from the tag ---

say "Windows zip"
# --branch: a tag push runs under the tag's name; a push to main at the same commit does not
# build against the engine tag.
run=$(gh run list -R "$REPO" --workflow windows.yaml --branch "$TAG" --commit "$COMMIT" --status success \
    --limit 1 --json databaseId --jq '.[0].databaseId // empty')
[ -n "$run" ] || die "no successful Windows run for $TAG ($COMMIT) yet"
gh run download "$run" -R "$REPO" -n gosub-beacon-windows -D "$WORK/windows"
zip=GosubBeacon-$ID-win-x64.zip
# CI builds a v* tag against the engine tag; a run that built something else shows up here
# as a different commit in the name.
[ -f "$WORK/windows/$zip" ] || die "CI run $run made $(ls "$WORK/windows"), not $zip"
cp "$WORK/windows/$zip" "$OUT/"

# --- macOS: the DMG from swift/package.sh ---

say "DMG"
[ "$(basename "$DMG")" = "GosubBeacon-$ID.dmg" ] || die "$DMG is not GosubBeacon-$ID.dmg"
cp "$DMG" "$OUT/"

# --- checksums ---

say "SHA256SUMS"
(cd "$OUT" && sha256sum GosubBeacon-* >SHA256SUMS && cat SHA256SUMS)

prerelease=
[[ "$VERSION" == *-* ]] && prerelease=--prerelease
cat <<EOF

Done: $OUT
Create the draft release (it stays a draft until published on GitHub):

  cd $ROOT && gh release create $TAG -R $REPO --draft $prerelease --verify-tag \\
      --title "Gosub Beacon $VERSION" --notes-file $NOTES $OUT/*
EOF
