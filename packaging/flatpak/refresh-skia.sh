#!/usr/bin/env bash
#
# Print the `skia-binaries` source stanza for io.gosub.beacon.yml.
#
# skia-bindings picks its prebuilt archive by a key derived from the crate's repository
# hash, the target triple and the enabled features, so the URL changes whenever skia-safe
# is bumped or the feature set shifts. The key is written to key.txt by a native build, so
# run a normal `cargo build --release --no-default-features --features gtk` first, then
# this script, and paste the result into the manifest.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

# The newest build's: target/ keeps the key of every skia-bindings build it has seen, and an
# older one names an archive this Cargo.lock no longer builds against. Dated by its `out`
# directory, which cargo makes when the build runs; key.txt itself comes out of the
# archive with the archive's own date.
key_file="$(find target -path '*/skia-bindings-*/out/skia/key.txt' 2>/dev/null |
    while read -r f; do echo "$(stat -c %Y "$(dirname "$(dirname "$f")")") $f"; done |
    sort -n | tail -1 | cut -d' ' -f2-)"
if [[ -z "$key_file" ]]; then
    echo "no key.txt under target/: build the GTK frontend natively first" >&2
    exit 1
fi
key="$(cat "$key_file")"

version="$(awk '/^name = "skia-bindings"$/{getline; gsub(/[",]/, "", $3); print $3; exit}' Cargo.lock)"
url="https://github.com/rust-skia/skia-binaries/releases/download/${version}/skia-binaries-${key}.tar.gz"

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
echo "fetching $url" >&2
curl -sSfL -o "$tmp" "$url"
sha="$(sha256sum "$tmp" | cut -d' ' -f1)"

cat <<EOF
      - type: file
        url: $url
        sha256: $sha
        dest-filename: skia-binaries-${key}.tar.gz
        only-arches:
          - x86_64
EOF
