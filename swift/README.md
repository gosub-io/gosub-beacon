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

## Packaging

```bash
./package.sh          # release build -> build/Gosub Beacon.app and build/GosubBeacon.dmg
./package.sh --app    # stop after the .app
```

`swift run` gives you a bare executable that finds `libbeacon.dylib` through an rpath into
this working copy, so it runs on the machine that built it and nowhere else. `package.sh`
makes the thing you can hand to someone: the dylib travels inside `Contents/Frameworks`, the
binary is repointed at `@executable_path/../Frameworks`, the build-tree rpaths are deleted,
and the bundle is signed ad hoc — not optional, because `install_name_tool` invalidates the
signature SwiftPM applied and an arm64 binary with a broken one is killed on launch rather
than warned about.

Two things inside the bundle are easy to get wrong and are worth knowing about. The SwiftPM
resource bundle (`BeaconMac_BeaconMac.bundle`) has to be copied into `Contents/Resources`,
or `Bundle.module` does not merely come up empty — it traps, and the About window takes the
app down with it. And `Info.plist` is what finally gives the menu bar its name and the About
window its version; without a bundle both fall back to the process name and a hard-coded
string.

The icon is `packaging/icon.png`, the Beacon lighthouse on a 1024 square; the script turns it
into an `.icns` (16 through 512, each at 1x and 2x) with `sips` and `iconutil`.

The disk image opens on `packaging/dmg-background.png`, with the app and the Applications
symlink placed in the clear water band either side of centre, where neither covers the
submarine below nor the wordmark above. That layout lives in a `.DS_Store` inside the image,
which Finder normally writes — and every recipe for it drives Finder over AppleScript, which
cannot work from a headless or ssh session: automating Finder needs a TCC grant, and without
it the attempt dies with "Not authorized to send Apple events". So the script uses
**`dmgbuild`** (`pip3 install --user dmgbuild`), which writes the `.DS_Store` itself and
therefore works anywhere, CI included. Without it installed the image still builds, just
plain: no background, Finder's own layout.

The background is supplied at 1536x1024 and the window is 768x512 **points**; the script
derives the 1x representation with `sips` and hands dmgbuild both, so the artwork stays sharp
on a Retina display rather than being upscaled.

**It is not signed with a Developer ID and not notarized**, which is deliberate for a demo
build but has a consequence worth expecting: macOS quarantines anything downloaded, so the
first launch has to be right-click -> Open, or `xattr -dr com.apple.quarantine "Gosub
Beacon.app"`. Double-clicking gives "cannot be opened because the developer cannot be
verified", which looks like a broken download and is not one. Notarization needs a paid
Apple Developer account and can be added without redoing any of the above.

The DMG is Apple Silicon only. A universal build means compiling the Rust side for
`x86_64-apple-darwin` as well and `lipo`-ing the two together.

## What it does

Tabs with favicons, close buttons, pinning, drag-to-reorder, an overflow scroller and a `+`
that never scrolls away. Back/forward/reload/stop, an address bar that takes what a person
would type, completes from history, selects on click and restores on Escape. Bookmarks with
a favourites bar, downloads through an `NSSavePanel`, per-tab zoom, keyboard input to pages,
two-finger swipe navigation, pinch to zoom, page and tab context menus, a hovered-link
overlay, multiple windows, private windows, a developer panel, a Settings window, session
restore, view source, and the About window with the shell's artwork.

Right-click on the page asks the engine what is actually under the pointer, so the menu
offers the link, the image and the text that are there rather than whatever the pointer last
hovered. `⌘`-click and middle-click open a link in a background tab, `⇧⌘`-click in front of
you. Press and hold Forward when the history has forked and it offers the branches.

**Developer panel** (`⌥⌘I`, or the Develop menu) docks under the page with three tabs:

- **Console** — the engine's own `log` records, not just Beacon's. Level, source crate and
  message, filterable, and it follows new records only while you are already scrolled to the
  bottom. What is captured depends on `BEACON_LOG` / `RUST_LOG`, which **default to warnings
  only** — an empty console usually means the level, not a broken panel. Try
  `BEACON_LOG=info swift run BeaconMac`.
- **Network** (`⌥⌘N`) — every request this tab made, with status, method, kind, size, how
  long it took and a waterfall bar split into waiting for the server and receiving the body.
  A request still in flight shows *which phase* it has been stuck in and for how long, which
  is the difference between "loading for twelve seconds" and an answer. Selecting one fills
  the pane beside it: overview and where the time went, the request line and its headers,
  the response and its headers, and the captured body.
- **Timings** (`⌥⌘T`) — the engine's timing table, slowest namespace first, with count,
  total, average, p50, p95 and max. Each namespace explains itself on hover, from the
  engine's own table. *Reset* starts again from nothing, which is how you time one
  navigation rather than every navigation since launch.

It polls four times a second while open and not at all while closed. All three tabs snapshot
rather than read a live table: log records arrive on whatever thread the engine is on,
requests are folded together as their events go past, and the timing table is written
continuously.

Opening the panel is also what turns on **body capture and unredacted headers** — a page
nobody is inspecting should not be paying to have its responses copied into memory, and a
panel nobody has open has no business holding your `Cookie` header. The flip side is that a
request which finished before you opened the panel has no body to show, and says so.

The buffer, the request log and the timing wrapper all live in `beacon_core::devtools`, not
here — the GTK shell has the same pane over the same data. This file only draws it.

**Settings** (`⌘,`) is the engine's own settings store, one row per key with the editor its
type asks for: a switch for a boolean, a popup for a setting restricted to named values, a
number field for a bounded number, text for the rest. A changed key is shown in bold and can
be put back with the arrow beside it — which forgets the override rather than storing a copy
of the default. Some settings (`net.*`) are read once when the engine starts, so a change may
only take effect next launch; the window says so. The GTK shell shows the same store as its
`gosub://config` page.

**Session restore.** Beacon writes the open tabs as it runs, so launching with no address on
the command line brings back the last session — pinned tabs still pinned, the same tab in
front. Give it a URL and that is what you get instead. Private windows neither restore nor
contribute.

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
| `gosub://config` settings page | a Settings window at `⌘,` | where every Mac app keeps them; a browser page that is really a preferences panel is a thing people have learned to distrust |
| `F2` shortcuts window | the menu bar | every shortcut is already written beside the command it runs |
| Crashed tab renders a page | an overlay over the page area | there is no engine worker left to render one with |
| — | two-finger swipe to navigate, pinch to zoom | expected on a trackpad, and absent from the GTK shell |

`beacon-core` owns every actual rule, so behaviour that *is* the browser — the last tab
refusing to close, pinned tabs holding the left of the strip, what counts as a URL — is
identical on both.

## What it does not do

- **No find-in-page.** The engine has no search API; this is not a shell gap.
- **No Mute Tab.** The GTK tab menu has the item, but it is a `@todo` that logs "Tab should
  be muted": the engine plays no audio yet. An item that does nothing is worse than no item.
- **No page console.** The engine runs no scripts yet, so there is nothing for a page's own
  `console.log` to have written. The Console tab shows the *engine's* log, which is a
  different and, for now, more useful thing.
- **No back-history menu.** `beacon-core` exposes the forward branches of its history tree
  but not a flat back list, so press-and-hold works on Forward and has nothing to show on
  Back. Address-bar completion is a different thing and does work: it searches the *visited
  pages* store, which the engine fills on every `http`/`https` navigation.
- **View source fetches the page again**, outside the engine, carrying no cookies and
  sharing no cache with it — so the source of a page behind a login is the logged-out HTML.
  The engine has no embedder-facing "fetch me this URL"; the stopgap lives in
  `beacon_core::fetch` and the GTK shell has exactly the same limitation.
- **No downloads window**, only the toolbar button's list: filename, progress, and click to
  open. The GTK shell's popover shows the same three things.
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
