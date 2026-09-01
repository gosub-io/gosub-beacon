# BeaconMac — a native macOS chrome

An AppKit window over Beacon's C ABI. It has no browser logic of its own: it draws a
window, forwards gestures, and asks `beacon-core` for everything it displays. The page is
rendered by Rust on the GPU straight into an `NSView`, with no copy.

## Build and run

From the repository root, build the Rust side first:

```bash
cargo build -p beacon-ffi
```

Then from this directory:

```bash
swift run BeaconMac https://example.com
```

`Package.swift` links `../target/debug/libbeacon.dylib` and records an rpath, so the binary
finds it without `DYLD_LIBRARY_PATH`. `Sources/CBeacon/module.modulemap` points at
`crates/beacon-ffi/include/beacon.h` directly — there is one declaration of the boundary,
and Swift reads the same one C does.

## What it does

Tabs (open, switch, and a `+`), back/forward/reload/stop, an address bar, link hover in the
status line, mouse clicks and scrolling.

## What it does not

No keyboard input to the page — the ABI has no `beacon_key_down` yet, so typing works in the
address bar (which is AppKit's own field) and nowhere else. No bookmarks, downloads, context
menus, zoom, or a real menu bar. Those are chrome, and chrome is the part that has to be
written once per platform.

## If something looks wrong

- **Nothing in the page area.** The view attaches once the window exists and has a non-zero
  size; if `beacon_attach_view` failed there is a `could not attach a view` line in the log.
- **Page at the wrong scale on a Retina display.** The surface is sized in device pixels and
  the viewport in CSS pixels; those differ by `backingScaleFactor` and conflating them is the
  usual cause.
- **Clicks landing in the wrong place.** `PageView.isFlipped` returns true so AppKit's
  coordinates match the engine's top-left origin. If that is removed, every Y is mirrored.
