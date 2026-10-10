# Flatpak packaging (GTK frontend)

The GTK binary needs GTK >= 4.14 from the host, which Ubuntu 22.04, Debian 12 and EL9
cannot provide — no tarball or distro package fixes that. A flatpak brings its own GTK
instead, so one artifact runs on every distribution that has flatpak.

What is here:

| File | What it is |
|---|---|
| `io.gosub.beacon.yml` | the manifest |
| `cargo-sources.json` | ~760 vendored crates + the vello git checkout, generated from `Cargo.lock` |
| `io.gosub.beacon.desktop` | desktop entry |
| `io.gosub.beacon.metainfo.xml` | AppStream metadata |
| `refresh-skia.sh` | prints the prebuilt-Skia source stanza after a skia-safe bump |

## Building

```bash
flatpak install flathub org.flatpak.Builder                  # also provides the linter
flatpak run org.flatpak.Builder --force-clean --user --install \
    --install-deps-from=flathub build-dir packaging/flatpak/io.gosub.beacon.yml
flatpak run io.gosub.beacon
```

The `dir` sources resolve relative to the manifest file, not to the working directory —
Beacon at `../..`, the engine at `../../../gosub-engine` — so only `build-dir` lands where
you run it. `--install-deps-from=flathub` matters on a first run: the GNOME 50 runtime and
SDK, the rust-stable extension and the GL extension (~1.5 GB) are not installed yet. After
that it compiles the engine and Beacon from scratch, so budget for it.

To ship the result:

```bash
flatpak build-bundle ~/.local/share/flatpak/repo beacon.flatpak io.gosub.beacon
```

## Things that will bite

**The engine is a `dir` source by default.** The manifest builds whatever you have checked
out at `../gosub-engine`, which is what you want while iterating. For a reproducible or CI
build, switch to the commented-out `git` source — but then the pinned commit must be the
one `Cargo.lock` was resolved against. `cargo-sources.json` is derived from that lock, and
the build runs `--locked --offline`, so a different engine commit with different
dependencies fails with "the lock file needs to be updated" and no network to fix it.

**`cargo-sources.json` must be regenerated whenever `Cargo.lock` changes:**

```bash
python3 -m venv /tmp/fcg && /tmp/fcg/bin/pip install aiohttp toml tomlkit
curl -sSLO https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
/tmp/fcg/bin/python flatpak-cargo-generator.py Cargo.lock -o packaging/flatpak/cargo-sources.json
```

It vendors the whole lock, including the egui frontend's dependencies. Trimming it would
mean a second lock file; not worth it.

**Skia is not built in the sandbox.** skia-bindings would normally download its prebuilt
binaries from GitHub during the build, and the sandbox has no network. The manifest instead
has flatpak-builder fetch that archive as a regular source and points `SKIA_BINARIES_URL`
at it with a `file://` URL. Building Skia from source is not a fallback here — there is no
gn or ninja in the SDK. After a skia-safe bump, run `./refresh-skia.sh` and paste the new
stanza into the manifest.

**If the build ever dies in skia-bindings looking for libclang**, bindgen is being run,
which it should not be — `bindings.rs` ships inside the prebuilt archive. If it happens
anyway, add `org.freedesktop.Sdk.Extension.llvm20` to `sdk-extensions`,
`/usr/lib/sdk/llvm20/bin` to `append-path`, and `LIBCLANG_PATH: /usr/lib/sdk/llvm20/lib`
to the env.

**x86_64 only.** The Skia archive is per-architecture, hence `only-arches`. aarch64 needs
its own stanza with the key a native aarch64 build prints.

**`file://` browsing is off by default.** The app gets no filesystem grant: `FileDialog`
is portal-backed, so downloads and "save link as" work without one. Browsing local files
does not. Opt in per machine with:

```bash
flatpak override --user --filesystem=host:ro io.gosub.beacon
```

**Profile data** lands in `~/.var/app/io.gosub.beacon/data/gosub-beacon` rather than
`~/.local/share/gosub-beacon`, because `dirs::data_dir()` follows `XDG_DATA_HOME`.

## Flathub

Not submitted. Flathub additionally requires the sources to be remote (no `dir` sources,
so the engine must become the pinned git source), screenshots that resolve, and a passing
`flatpak run --command=flatpak-builder-lint org.flatpak.Builder manifest <manifest>`.

## Icons

The lighthouse exists only as `crates/beacon-core/resources/lighthouse.svg`, the same file
gosub://home shows; no PNG of it is kept in the repository. The flatpak installs it as the
scalable hicolor icon. Packaging that needs pixels renders them at build time with
`beacon-icon`, which puts the circle on 91% of the square:

```bash
cargo run -p beacon-icon -- 256 icon.png
```

`swift/package.sh` builds the macOS `.icns` that way, and the Android script the launcher icon.
