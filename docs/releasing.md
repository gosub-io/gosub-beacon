# Releasing Beacon

Releases are drafts on [GitHub Releases](https://github.com/gosub-io/gosub-beacon/releases):
a flatpak bundle, a DMG, a Windows zip, two APKs and `SHA256SUMS`, with the notes from
`docs/releases/<version>.md`. Every artifact name carries the build count and both
commits: `GosubBeacon-<version>-<build>-<commit>-engine-<engine>`.

## 1. Before tagging

- `version` in `Cargo.toml` is the release version.
- `docs/releases/<version>.md` holds the notes. Its links point at the tag, so they keep
  working after main moves on.
- `packaging/flatpak/io.gosub.beacon.metainfo.xml` has a `<release>` for the version, with
  the release date.
- `packaging/flatpak/io.gosub.beacon.yml` pins the engine commit that will be tagged, and
  `packaging/flatpak/cargo-sources.json` matches `Cargo.lock`.
- CI is green on main.

## 2. Tags

The engine gets `beacon-v<version>` on the commit of its `beacon` branch that Beacon is
built against; Beacon gets `v<version>`. Push the engine tag first: the Beacon tag starts
the Windows workflow, which builds against the engine tag and uploads the zip.

```bash
git -C ../gosub-engine tag -a beacon-v<version> -m "Beacon <version>" <engine-commit>
git -C ../gosub-engine push https://github.com/gosub-io/gosub-engine.git beacon-v<version>
git tag -a v<version> -m "Gosub Beacon <version>"
git push https://github.com/gosub-io/gosub-beacon.git v<version>
```

## 3. The DMG

On the Mac, in a clean checkout of both tags, with `swift/packaging/signing.env` holding a
Developer ID and notary credentials (`swift/packaging/setup-signing.sh`), so the DMG is
signed, notarized and stapled. Universal, which needs the `x86_64-apple-darwin` target:

```bash
swift/package.sh
```

Copy `swift/build/GosubBeacon-<version>-...dmg` to the Linux machine.

## 4. Everything else, and the checksums

On Linux, in a clean checkout of both tags, with the `BEACON_ANDROID_*` signing variables
set (see `scripts/android-apk.sh`):

```bash
scripts/release.sh --dmg <path-to-dmg>
```

It builds the flatpak bundle and both APKs, downloads the Windows zip from the tag's CI run,
checks every name against the tags, and writes `target/release/<version>/` with
`SHA256SUMS`. It ends with the `gh release create --draft` command; run that to create the
draft.

## 5. Smoke tests

On the files in the draft, not on local builds.

**Linux (flatpak):** install the bundle on a machine that has never had Beacon; first run,
browse, download a file, relaunch (session restored). Once on a session without a Secret
Service: Beacon stops with the error (KI-007).

**macOS (DMG):** on a clean user account: open the DMG, drag to Applications, first start
past Gatekeeper; the keychain prompt; a second window; quit and relaunch. On an Intel Mac if
one is available.

**Android (APKs):** on a non-Mali arm64 device: install, browse, download. Install over the
previous release: the profile is kept. On a Mali device: the notice shows (KI-001). The
armeabi-v7a APK on a 32-bit device.

**Windows (zip),** on physical hardware, at 125% and at 150% display scaling:

1. unzip, first start past SmartScreen, no runtime installed
2. home page renders sharp, window fits the screen
3. type a URL + Enter, page loads, status line ends at Done
4. click a link; Back/Forward (buttons, Alt+Left/Right)
5. Ctrl+T, switch tabs, Ctrl+W, Ctrl+Shift+T
6. Ctrl++ to 150%: page fills the view, a click hits the right link; Ctrl+0
7. scroll: wheel, Space, End, Home
8. right-click a link, an image, an image inside a link: the right menu items
9. Ctrl+D bookmark; a menu entry with an underscore shows it
10. Ctrl+H history window, search
11. download: dialog starts in Downloads with the extension; status ends at Downloaded
12. Ctrl+U view source opens on its own page, not the previous frame
13. F12 Engine Room console shows lines
14. About; icon in title bar, taskbar, Explorer; file properties show version info
15. quit and relaunch: session restored; beacon.log and beacon.log.1 in the profile

## 6. Publish

Publish the draft on GitHub. It is a pre-release when the version has a suffix
(`-preview.1`).
