# Gosub Beacon

Beacon is a browser built on the [Gosub engine](https://github.com/gosub-io/gosub-engine).
The engine does the actual work (networking, cookies, storage, history, rendering); Beacon
is the native chrome around it: a GTK4 shell on Linux and a Swift/AppKit shell on macOS,
both over the same Rust core. It exists to test the engine in a real application, so
don't expect a daily driver — but basic browsing works.

![Gosub Beacon with three tabs loaded](./docs/screenshots/beacon-2026-08.png)

More in [docs/screenshots](./docs/screenshots/).

## Platforms

- **Linux**: the GTK shell, the most complete one. It is also packaged as a Flatpak,
  which brings its own GTK and so runs on distributions whose GTK is too old; see
  [packaging/flatpak](./packaging/flatpak/README.md). A second, minimal frontend in egui
  (Vello on wgpu, no GTK or Skia) does tabs, navigation and rendering and little else.
- **macOS**: a native Swift/AppKit shell in [swift/](./swift/README.md), over the C ABI in
  `crates/beacon-ffi`. Signed and notarized DMGs are published at
  [gosub.io/build](https://gosub.io/build).
- **Windows**: on hold. A C#/WPF shell over the same C ABI exists on the unmerged
  `windows` branch, but it is not being worked on, is behind `main` and is not built in CI.

## Getting the source

Beacon uses path dependencies into the engine, so check out
[gosub-engine](https://github.com/gosub-io/gosub-engine) next to this repository
(`../gosub-engine`), on the `beacon` branch (upstream main plus engine work Beacon
needs that has not merged yet):

```bash
git clone https://github.com/gosub-io/gosub-beacon.git
git clone -b beacon https://github.com/gosub-io/gosub-engine.git
cd gosub-beacon
```

You need Rust from [rustup](https://rustup.rs); `rust-toolchain.toml` selects stable. The
first build compiles the engine and takes a few minutes.

## Building and running on Linux

Dependencies on Debian/Ubuntu, or similar on other systems:

```bash
sudo apt install libgtk-4-dev libglib2.0-dev libcairo2-dev libgdk-pixbuf-2.0-dev \
                 libpango1.0-dev libsqlite3-dev libssl-dev pkg-config \
                 clang libclang-dev libgl-dev libegl-dev libfontconfig-dev libfreetype-dev
```

These are for the GTK frontend. The egui one needs none of them:

```bash
cargo build                                          # both binaries
cargo build --no-default-features --features egui   # just the egui one
```

```bash
cargo run --bin gosub-beacon-gtk                          # opens the default startup tabs
cargo run --bin gosub-beacon-gtk -- https://example.com   # URLs become the startup tabs
cargo run --bin gosub-beacon-egui -- https://example.com  # the other frontend
```

## Building and running on macOS

Skip the Linux commands: a bare `cargo build` (or `make build`) builds the GTK shell, which
does not compile on a Mac; it stops with these directions instead. The Mac app is the Rust C ABI (`beacon-ffi`) plus a Swift package. Besides Rust
you need the Xcode Command Line Tools (`xcode-select --install`), which bring Swift and git.
From the `gosub-beacon` checkout:

```bash
cargo build -p beacon-ffi                  # the Rust side
cd swift
swift run BeaconMac https://example.com    # builds and starts the Swift app
```

Run it from a session on the Mac's own screen: started over ssh, the window has nowhere to
draw and stays blank. `swift/package.sh` turns the build into a universal (arm64 and
x86_64) `.app` and DMG; that, and everything else about the Mac shell, is in
[swift/README.md](./swift/README.md).

## Development

`make test` runs the unit tests, clippy and the format check; `make fix-format` applies
`cargo fmt` and clippy's fixes. CI runs the same on Ubuntu, builds and tests the isolation
build under Xvfb (below), and on `macos-14` builds the egui frontend, the C ABI with a
headless C consumer, and the Swift app.

## More on running

An `isolation` build (`--features isolation`, GTK, Linux) runs the engine with every
component in its own sandboxed process: the network stack, the cookie vault, the
localStorage service, a throwaway decoder per image, and one renderer per site; Beacon's
own process is confined to its profile, `~/Downloads` and the temp dir. It is isolated by
default; `--single-process` turns it off for a run. A default build is the one-process
engine; `--isolated` there gets the service processes but renders in-process. The
isolated mode keeps its own localStorage, and a download saved outside `~/Downloads` fails
under the lockdown. See [docs/isolation.md](docs/isolation.md) for what runs where, how it
is verified and the known limits.

### Watching what the engine is doing

View > Show Activity (Ctrl+Shift+A) puts up to four status lines over the page, each with
a running clock: the document request as it resolves, connects, waits and receives; the
page's other fetches folded into one line; and the engine's stages as they run (parsing,
the render tree, layout, tiling, raster, paint) or, under `--isolated`, the render pass in
the site's process with the renderer's own lap times when it answers. Finished lines stay
a moment with their total, then go. The strip always has four lines; a line keeps its place until it is done, and when
more is going on than fits, the oldest keep their lines and the last says how many more.
It costs nothing while hidden: showing it is what makes the engine announce its stages.

### A game while you wait

When a page cannot be reached, press space on the error page: a submarine, kelp to
thread through, bubbles, fish and axolotls on the sand, in the spirit of the dinosaur.
`gosub://dive` opens it directly, and the game follows the dark-mode toggle. It lives
in `beacon-core` (rules, art and a pixel painter), so every shell shows the same
frames; the GTK shell draws them into a `DrawingArea`.

### Testing the renderer tier

`scripts/isolation-smoke.sh` runs an `isolation` build twice under Xvfb against the
fixture page in `tests/fixtures/isolation`, plain and `--isolated`, and checks the
component processes, the log and the recorded page title (it comes back from the
renderer process, so it proves the remote render). Each run then types into the page's
field, scrolls down and back, narrows the window and widens it again, and follows its
link; after every step the two runs' screenshots must agree within a small antialiasing
margin, and scrolling back and widening back must restore the page exactly. CI runs it as the `isolation-smoke` job. Locally:

```bash
cargo build --bin gosub-beacon-gtk --no-default-features --features gtk,isolation
xvfb-run -a -s "-screen 0 1400x900x24" scripts/isolation-smoke.sh
```

It needs `xvfb`, `xdotool`, ImageMagick and `python3`; artefacts (logs, screenshots, a
diff image) land in the directory it prints.

### Running in a container

To (re-)create a docker/podman image, you can use the supplied [Dockerfile](./Dockerfile) to build a local image with dependencies installed.

#### Building an image

First build the image.

```shell
# docker
docker build --tag gosub-beacon .
# podman
podman build --tag gosub-beacon .
```

Use args to use specific branches and/or forks for the engine.

- `ENGINE_REMOTE`
- `ENGINE_BRANCH`

```shell
# docker
docker build --tag gosub-beacon --build-arg='ENGINE_BRANCH=main' .

# podman 
podman build --tag gosub-beacon --build-arg='ENGINE_BRANCH=main' .
```

#### Running the image

Run this image using Wayland (X11 should also work)

```shell
# docker
docker run --rm -it \
       --user="$(id -u):$(id -g)" \
       --workdir=/tmp \
       \
       -e WAYLAND_DISPLAY="$WAYLAND_DISPLAY" \
       -e DISPLAY="$DISPLAY" \
       \
       -e XDG_RUNTIME_DIR=/tmp/runtime \
       -v tmpfs:/tmp/runtime \
       -v /tmp/.X11-unix:/tmp/.X11-unix:ro \
       -v "$XDG_RUNTIME_DIR"/"$WAYLAND_DISPLAY":/tmp/runtime/"$WAYLAND_DISPLAY":ro \
       \
       gosub-beacon

# podman
podman run --rm -it \
       --userns=keep-id \
       --workdir=/tmp \
       \
       -e WAYLAND_DISPLAY="$WAYLAND_DISPLAY" \
       -e DISPLAY="$DISPLAY" \
       \
       -e XDG_RUNTIME_DIR=/tmp/runtime \
       -v tmpfs:/tmp/runtime \
       -v /tmp/.X11-unix:/tmp/.X11-unix:ro \
       -v "$XDG_RUNTIME_DIR"/"$WAYLAND_DISPLAY":/tmp/runtime/"$WAYLAND_DISPLAY":ro \
       \
       gosub-beacon

```

