# Gosub Beacon - Windows shell

A native Windows chrome for [Beacon](../README.md), written in C#/WPF over `beacon-ffi`.

This is the third shell against the same C ABI, alongside the GTK one (`crates/beacon-gtk`)
and the macOS one (`swift/`). The browser itself - tabs, navigation, history, downloads,
rendering - lives in Rust and is shared; everything here is chrome.

## Status

Working:

- page rendering, mouse and scroll input, zoom
- tab strip with favicons, close buttons, pinning, drag-to-reorder and a context menu
- menu bar: File, Edit, View, History, Bookmarks, Help
- bookmarks bar, bookmark manager, and the Bookmarks menu, all read from the engine
- history window with live search
- page context menu driven by `beacon_hit_test` (open/copy link, open/copy image)
- forward-branch menu (right-click Forward) when the history has forked
- session restore on a bare start, matching the GTK shell
- view-source, reopen-closed-tab, crash reporting in the status line

- developer tools (F12): network, console and timing
- settings editor over the engine's settings store
- downloads: save dialog, progress, open when finished
- the About dialog, sharing the macOS shell's artwork and credits crossfade

This shell now binds **all 129 functions** of the C ABI.

### Developer tools

`F12`, or Tools -> Developer Tools. Three tabs, over the developer-panel families in
`beacon.h`:

- **Network** - status, method, type, URL, size, time, phase and initiator, with request and
  response headers, the captured body, the redirect chain, and a Diagnosis tab that collects
  the engine's own failure and phase hints. Scoped to the current tab by default.
- **Console** - engine log records with level filtering. What gets captured follows
  `BEACON_LOG` / `RUST_LOG`, which default to **warnings only**, so an empty console usually
  means the level rather than a broken panel. The panel says so rather than looking empty.
- **Timing** - the engine's timing table, slowest namespace first, with count/total/avg and
  p50/p95/p99. Empty when the engine was built without its `timing` feature, which compiles
  the subsystem out; that is nothing to show rather than an error.

Body capture and sensitive headers (`Cookie`, `Authorization`) are off by default and follow
the window's visibility, so nothing is held in memory while the panel is closed.

Fields the engine never reported show as `-` rather than `0`: a request on a pooled
connection resolves nothing, which is not the same as resolving instantly.

Set `BEACON_DEVTOOLS=1` to open the panel at launch, or `BEACON_DEVTOOLS=all` to raise the
settings and downloads windows with it. Both exist because over ssh a GUI lands in session 0,
where windows can be created but not seen - opening them at launch is the only way to put
their marshalling through its paces without a desktop.

### About

Help -> About. A port of the macOS dialog rather than a re-imagining: the same two pages, the
same 250 ms crossfade between artwork and credits, the same column of names over the right
half of the picture.

The artwork is **not copied into this project**. The csproj references
`swift/Sources/BeaconMac/Resources/` directly and packs it with `Link=`, so there is one copy
in the repository and the two shells cannot drift apart.

Text drawn over the picture uses explicit colours rather than theme ones, since a label that
turned white on a dark theme would vanish into the picture's pale left half, and carries a
drop shadow to stay legible where the lighthouse beam passes behind it.

### Settings

Tools -> Settings. The ABI hands over rows - key, description, type, value, default,
constraint - and this shell decides what each looks like: a checkbox for a bool, a popup
where the schema restricts the value to literal choices, a text field otherwise with the
accepted range beside it. Modified keys are bold and get a working Reset.

A write can be refused for an unknown key or a value outside its constraint, so every commit
checks the return and reverts the editor on false. Settings under `net.*` are read once when
the engine starts, so changes take effect next launch; the window says so.

### Downloads

`Ctrl+J`, or Tools -> Downloads. A `BEACON_DOWNLOAD_OFFERED` event raises a Windows save
dialog; the offer id keeps the answer attached to the right offer if a second download
arrives while the dialog is open.

The dialog is raised through the dispatcher rather than from inside the event pump, because a
modal dialog runs its own message loop and would tick `CompositionTarget.Rendering`,
re-entering the pump.

Progress of -1 means the server sent no length, so those rows show an indeterminate bar and
the bytes received.

## Building

Both halves cross-compile **from Linux**, so the build loop does not need Windows at all -
only running does.

```bash
./windows/build.sh            # debug
./windows/build.sh release
```

Prerequisites, none of which need root or Visual Studio:

```bash
cargo install cargo-xwin                    # fetches the MS CRT + Windows SDK itself
ln -s "$(command -v clang)" ~/bin/clang-cl  # clang picks its MSVC mode from argv[0]
# lld-link ships with lld, usually already at /usr/lib/llvm-*/bin/lld-link
```

plus the .NET SDK (10.0+). `EnableWindowsTargeting` in `Directory.Build.props` is what lets
the SDK build a WPF project on a non-Windows host.

### Why the MSVC target, not mingw

Two reasons, both hard requirements:

1. It is the ABI .NET's P/Invoke expects.
2. `skia-safe` publishes prebuilt binaries for `x86_64-pc-windows-msvc` and not for
   `x86_64-pc-windows-gnu`. Without a prebuilt, skia-bindings compiles Skia from source,
   which asserts `win_vc != ""` and requires a Visual Studio install.

The prebuilt is keyed on target **and features**. Enabling a Linux-only skia feature
(`x11`, `wayland`, `egl`) produces a key that was never published; the download 404s and the
source build begins, so the `win_vc` error is three steps downstream of the real cause. Keep
the Windows feature set to `svg` + `gl`.

## Running

Copy the output directory to a Windows machine and run `GosubBeacon.exe`:

```
windows/src/BeaconWindows/bin/Debug/net10.0-windows/win-x64/
```

`beacon.dll` is copied next to the exe by the build and must stay there. The MSVC build
imports `VCRUNTIME140.dll`, so the target needs the Visual C++ redistributable - or build
with `-C target-feature=+crt-static` to avoid it.

URLs on the command line become the startup tabs.

## Rendering: the CPU path, on purpose

The page is drawn with `beacon_acquire_frame`, which hands over finished BGRA pixels that
`PageView` blits into a `WriteableBitmap`. Beacon's frames are premultiplied BGRA, which is
exactly `PixelFormats.Pbgra32`, so nothing is converted.

`beacon-ffi` also offers a GPU path: `beacon_attach_view` takes an HWND and Vello draws into
it with no copy, which is faster on real hardware. The copy is used here because it needs no
graphics adapter. On a machine whose display is a basic driver rather than a real one (a VM),
there is no hardware D3D12, wgpu falls back to WARP, and Vello's compute shaders crash inside
`d3d10warp.dll` with an access violation. Switching to the GPU path later is a change to
`PageView.cs` and nothing else.

## Shape of the code

```
windows/
  Directory.Build.props        EnableWindowsTargeting and shared settings
  build.sh                     builds beacon.dll and the exe, from Linux
  src/BeaconWindows/
    Interop/BeaconNative.cs    raw P/Invoke, one-to-one with beacon.h
    Interop/BeaconBrowser.cs   safe wrapper: owns the handle, copies event strings
    PageView.cs                the page: frame blitting, input, hit-test requests
    TabStrip.cs                favicons, close, pin, drag-reorder, context menu
    BookmarksBar.cs            the bar, plus a clipboard helper that cannot throw
    BookmarksWindow.cs         bookmark manager
    HistoryWindow.cs           visited pages with live search
    DeveloperPanel.cs          network, console and timing
    SettingsWindow.cs          the engine's settings store, typed per row
    DownloadsWindow.cs         progress, and open when finished
    AboutWindow.cs             the artwork, and the credits crossfade
    MainWindow.xaml[.cs]       menu bar, toolbar, the event pump, context menus
    App.xaml[.cs]              startup and the "beacon.dll is missing" message
```

The C ABI has 129 functions and this shell binds every one of them.

### Rules from `beacon.h` that this shell must keep

1. **The shell keeps no state.** The tab strip asks the engine for the list every time
   rather than tracking one.
2. **Events are pulled, not pushed.** `CompositionTarget.Rendering` drains
   `beacon_poll_events` once a frame.
3. **One thread, the UI thread.** Every call in this shell is made from it.

Two more that are specific to C#: a `char*` returned to you must be freed with
`beacon_string_free`, so those functions return `IntPtr` and are read through
`BeaconStrings.Take` - marshalling them as `string` would free them with `CoTaskMemFree`.
And C's `bool` is one byte, so every bool is `[MarshalAs(UnmanagedType.U1)]`; the .NET
default is the four-byte Win32 `BOOL`.

## Keyboard

| | |
|---|---|
| Ctrl+T | new tab |
| Ctrl+Shift+T | reopen closed tab |
| Ctrl+W | close tab |
| Ctrl+L | focus address bar |
| Ctrl+D | bookmark this page |
| Ctrl+H | history window |
| Ctrl+Shift+O | bookmark manager |
| Ctrl+U | view source |
| F12 | developer tools |
| Ctrl+J | downloads |
| F5 / Ctrl+F5 | reload / reload ignoring cache |
| Esc | stop |
| Ctrl++ / Ctrl+- / Ctrl+0 | zoom in, out, actual size |
| Alt+Left / Alt+Right | back / forward |
| Middle-click a tab | close it |
| Middle-click a bookmark | open in a new tab |
| Right-click Forward | forward branches, when the history forked |
