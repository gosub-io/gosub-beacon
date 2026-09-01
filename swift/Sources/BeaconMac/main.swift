import AppKit

// A plain AppKit app, launched from the command line rather than a bundle: `swift run`
// should just work, without an Xcode project or an .app to assemble first.

let startURL = CommandLine.arguments.dropFirst().first ?? "https://example.com"

let app = NSApplication.shared
// .regular so it gets a Dock icon and a menu bar and can take focus, which an accessory
// app cannot -- a browser you cannot click into is not much of a test.
app.setActivationPolicy(.regular)

final class AppDelegate: NSObject, NSApplicationDelegate {
    var window: BrowserWindow?
    private let startURL: String

    init(startURL: String) {
        self.startURL = startURL
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        guard let browser = Browser(profileDirectory: nil, private: false) else {
            NSLog("beacon: the engine would not start")
            NSApp.terminate(nil)
            return
        }
        window = BrowserWindow(browser: browser, startURL: startURL)
        NSApp.activate(ignoringOtherApps: true)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}

let delegate = AppDelegate(startURL: startURL)
app.delegate = delegate
app.run()
