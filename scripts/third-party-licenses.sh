#!/usr/bin/env bash
# Regenerate crates/beacon-core/resources/licenses.html, served as gosub://licenses.
#
# Needs cargo-about (cargo install --locked cargo-about --features cli) and the engine
# checked out at ../gosub-engine. Run after any Cargo.lock change and before a release.
#
# Rust crates come from cargo-about (about.toml, about.hbs), for every shipped target.
# Native code the crates build or bundle (SQLCipher, OpenSSL, Skia and its third-party
# libraries, Roboto) declares no licence cargo-about can see, so it is added here from
# the sources cargo already downloaded.
set -euo pipefail

cd "$(dirname "$0")/.."
out=crates/beacon-core/resources/licenses.html
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

cargo about generate --workspace --all-features --locked about.hbs -o "$tmp/crates.html"
cargo metadata --format-version 1 --locked > "$tmp/metadata.json"

python3 - "$tmp/metadata.json" "$tmp/crates.html" "$out" <<'EOF'
import html, json, os, sys

metadata, crates_html, out = sys.argv[1:]
dirs = {p["name"]: os.path.dirname(p["manifest_path"]) for p in json.load(open(metadata))["packages"]}

def src(package, path):
    return os.path.join(dirs[package], path)

skia = "skia-bindings"
ext = "skia/third_party/externals"
native = [
    ("SQLCipher", src("libsqlite3-sys", "sqlcipher/LICENSE")),
    ("OpenSSL", src("openssl-src", "openssl/LICENSE.txt")),
    ("Skia", src(skia, "skia/LICENSE")),
    ("FreeType (in Skia)", src(skia, f"{ext}/freetype/LICENSE.TXT")),
    ("HarfBuzz (in Skia)", src(skia, f"{ext}/harfbuzz/COPYING")),
    ("ICU (in Skia)", src(skia, f"{ext}/icu/LICENSE")),
    ("libjpeg-turbo (in Skia)", src(skia, f"{ext}/libjpeg-turbo/LICENSE.md")),
    ("libpng (in Skia)", src(skia, f"{ext}/libpng/LICENSE")),
    ("libwebp (in Skia)", src(skia, f"{ext}/libwebp/COPYING")),
    ("zlib (in Skia)", src(skia, f"{ext}/zlib/LICENSE")),
    ("Expat (in Skia)", src(skia, "skia/third_party/expat/LICENSE")),
    ("Wuffs (in Skia)", src(skia, "skia/third_party/wuffs/LICENSE")),
    ("Brotli (in Skia)", src(skia, f"{ext}/brotli/LICENSE")),
]

parts = ["<h2>Native libraries</h2>"]
for name, path in native:
    text = open(path, encoding="utf-8", errors="replace").read()
    parts.append(f"<h3>{html.escape(name)}</h3>\n<pre>{html.escape(text)}</pre>")
parts.append(
    "<h3>Roboto</h3>\n<pre>Copyright 2011 Google Inc. All Rights Reserved.\n"
    "Licensed under the Apache License, Version 2.0 (text under Rust crates below).</pre>"
)

page = open(crates_html, encoding="utf-8").read()
assert "<!-- NATIVE -->" in page
open(out, "w", encoding="utf-8").write(page.replace("<!-- NATIVE -->", "\n".join(parts)))
EOF

echo "wrote $out"
