import AppKit

// A plain AppKit app, launched from the command line rather than a bundle: `swift run`
// should just work, without an Xcode project or an .app to assemble first.

let startURL = CommandLine.arguments.dropFirst().first ?? "https://example.com"

let app = NSApplication.shared
// .regular so it gets a Dock icon and a menu bar and can take focus, which an accessory
// app cannot -- a browser you cannot click into is not much of a test.
app.setActivationPolicy(.regular)

let delegate = AppDelegate(startURL: startURL)
app.delegate = delegate
app.run()
