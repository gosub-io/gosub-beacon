# BeaconMac — a native macOS chrome

An AppKit browser over Beacon's C ABI. It has no browser logic of its own: it draws a
window, forwards gestures, and asks `beacon-core` for everything it displays. The page is
rendered by Rust on the GPU straight into an `NSView`, with no copy.

## Build and run

From the repository root, build the Rust side first:

```bash
cargo build -p beacon-ffi
```

**Only that crate.** A bare `cargo build` builds the whole workspace, including
`beacon-gtk` — the Linux frontend, which wants GTK4 from Homebrew and rasterizes through
Skia. Skia will not compile here: the engine asks `skia-safe` for the `wayland` feature on
every platform, `wayland` implies `egl`, and `egl` needs `EGL/egl.h`, which macOS does not
have. `beacon-ffi` renders through Vello instead and pulls none of it.

Then from this directory:

```bash
swift run BeaconMac https://example.com
```

`Package.swift` links `../target/debug/libbeacon.dylib` and records an rpath, so the binary
finds it without `DYLD_LIBRARY_PATH`. `Sources/CBeacon/module.modulemap` points at
`crates/beacon-ffi/include/beacon.h` directly — there is one declaration of the boundary,
and Swift reads the same one C does.

## What it does

Tabs with favicons, close buttons, pinning, drag-to-reorder, an overflow scroller and a `+`
that never scrolls away. Back/forward/reload/stop, an address bar that takes what a person
would type, completes from history, selects on click and restores on Escape. Bookmarks with
a favourites bar, downloads through an `NSSavePanel`, per-tab zoom, keyboard input to pages,
two-finger swipe navigation, pinch to zoom, page and tab context menus, a hovered-link
overlay, multiple windows, private windows, and the About window with the shell's artwork.

**Private windows** (`⇧⌘N`) run a *second engine*, because privacy is a property of the
engine's zone — memory-only cookies and storage, no visits recorded — and there is no way to
make one tab of a session private. All private windows share one such session, and it is
destroyed when the last of them closes; since everything it held was in memory, that is what
actually forgets it. Bookmarks and settings stay shared and persistent, as they do in Safari
and Chrome. The window carries dark chrome, a `Private` label in the tab strip and a title
suffix: colour alone is not a label.

The About artwork lives in `Sources/BeaconMac/Resources/` as a **copy** of the GTK shell's
`crates/beacon-gtk/resources/`: SwiftPM will not take resources from outside its own target
directory. Change the art and both need updating.

## Where it deliberately differs from the GTK shell

The GTK frontend is the reference for *what* the browser does. How it is offered follows
macOS, not GNOME:

| GTK | here | why |
|---|---|---|
| Headerbar with a hamburger | a real `NSMenu` bar | not optional on macOS; an app without one reads as broken |
| `Ctrl` shortcuts | `⌘` shortcuts | `Ctrl+W` on a Mac does nothing a user expects |
| `Alt+←` / `Alt+→` for history | `⌘[` / `⌘]` | Alt+arrow is a word-jump in every Mac text field |
| `F11` full screen | `⌃⌘F`, native full screen | F11 is Mission Control's |
| `Ctrl+Shift+D` dark-mode toggle | follows the system | appearance is a System Settings choice on macOS; apps do not each ship a toggle |
| `Ctrl+Shift+P` private window | `⇧⌘N` | what Safari and Chrome both bind; `⌘P` is Print and must stay that way |
| Permanent status bar | a floating overlay over the page | Safari's placement; a strip of window is a waste on a laptop |
| `F1` opens About | the application menu | where every Mac user looks first; F1 is a brightness key |
| `Ctrl+A` in the address bar moves to line start | selects all | the emacs binding is macOS's, but a browser address bar is not a text editor, and this is what the gesture means everywhere else |
| Log pane | `NSLog` to the console | `Console.app` is where a Mac developer already looks |
| — | two-finger swipe to navigate, pinch to zoom | expected on a trackpad, and absent from the GTK shell |

`beacon-core` owns every actual rule, so behaviour that *is* the browser — the last tab
refusing to close, pinned tabs holding the left of the strip, what counts as a URL — is
identical on both.

## What it does not do

- **No find-in-page.** The engine has no search API; this is not a shell gap.
- **No history menu of past pages.** `beacon-core` exposes the forward branches of its
  history tree but not a flat back list, so a long-press menu on Back has nothing to show.
  Address-bar completion is a different thing and does work: it searches the *visited pages*
  store, which the engine fills on every `http`/`https` navigation.
- **No preferences window.** There is no settings ABI yet; *Engine Settings* in the
  application menu opens `gosub://config`, which is the engine's own read-only dump.
- **No session restore.** `beacon-core` has the pieces; the ABI does not expose them.
- **Marked text is accepted and dropped.** A CJK composition commits correctly through
  `insertText`, but there is no inline candidate display, because the engine has no API for
  one.

## If something looks wrong

- **Nothing in the page area.** The view attaches once the window exists and has a non-zero
  size; if `beacon_attach_view` failed there is a `could not attach a view` line in the log.
  Run with `BEACON_LOG=info` to see the reason.
- **Blurry text on a Retina display.** The scale is sent with the viewport; the log line
  reports it. `@1.0x` on a Retina panel means `backingScaleFactor` was read before the
  window had a screen.
- **Clicks landing in the wrong place.** `PageView.isFlipped` returns true so AppKit's
  coordinates match the engine's top-left origin, and page coordinates are divided by the
  zoom. If either is removed, clicks drift.
- **Typing does nothing.** The page view has to be first responder — click the page once.
  Anything with `⌘` held is left to the menu bar on purpose.
