import AppKit

// A plain AppKit app, launched from the command line rather than a bundle: `swift run`
// should just work, without an Xcode project or an .app to assemble first.

// No address on the command line means "carry on where the last session left off" — the
// tabs Beacon saved as it ran. Only when there is nothing saved does a fresh window open on
// the homepage.
let startURL = CommandLine.arguments.dropFirst().first

let app = NSApplication.shared
// .regular so it gets a Dock icon and a menu bar and can take focus, which an accessory
// app cannot -- a browser you cannot click into is not much of a test.
app.setActivationPolicy(.regular)

let delegate = AppDelegate(startURL: startURL)
app.delegate = delegate
app.run()
