import AppKit
import CBeacon

/// The application: one browser, a menu bar, and however many windows the user opens.
///
/// The menu bar is built in code rather than loaded from a nib so the whole shell stays
/// readable as source. It is not optional on macOS — an app without one looks broken before
/// the user has clicked anything — and it is where the shortcuts live: a menu item with a
/// key equivalent is the Mac way to bind a key, not a hand-rolled key handler.
final class AppDelegate: NSObject, NSApplicationDelegate, NSMenuItemValidation {
    private var browser: Browser?
    /// One private session shared by every private window, created on demand and destroyed
    /// when the last of them closes — which is what actually makes it private: the engine
    /// holds its cookies and storage in memory, so tearing it down is what forgets them.
    private var privateBrowser: Browser?
    private let startURL: String
    /// Held so the window survives being closed and can be reopened.
    private var aboutWindow: AboutWindowController?

    init(startURL: String) {
        self.startURL = startURL
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        guard let browser = Browser(profileDirectory: nil, private: false) else {
            NSLog("beacon: the engine would not start")
            NSApp.terminate(nil)
            return
        }
        self.browser = browser

        buildMenuBar()
        WindowRegistry.shared.onChange = { [weak self] in self?.pruneSessions() }
        newWindow(startURL: startURL)
        NSApp.activate(ignoringOtherApps: true)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }

    // ── windows ───────────────────────────────────────────────────────────

    @discardableResult
    private func newWindow(startURL: String?) -> BrowserWindowController? {
        guard let browser else { return nil }
        let controller = BrowserWindowController(browser: browser, startURL: startURL)
        WindowRegistry.shared.add(controller)
        return controller
    }

    @objc private func newWindowAction(_ sender: Any?) {
        newWindow(startURL: "gosub://home")
    }

    @objc private func newPrivateWindowAction(_ sender: Any?) {
        if privateBrowser == nil {
            // A second engine, not a second window over the same one: privacy is a property
            // of the engine's zone (memory-only cookies and storage, no visits recorded),
            // and there is no way to make one tab of a session private.
            guard let session = Browser(profileDirectory: nil, private: true) else {
                NSLog("beacon: could not start a private session")
                return
            }
            privateBrowser = session
        }
        guard let privateBrowser else { return }
        let controller = BrowserWindowController(browser: privateBrowser, startURL: "gosub://home")
        WindowRegistry.shared.add(controller)
    }

    /// Drop the private session once no window is using it. Everything it held was in
    /// memory, so this is the "forget it" step; the next private window starts clean.
    private func pruneSessions() {
        if !WindowRegistry.shared.hasPrivateWindow {
            privateBrowser = nil
        }
    }

    private var front: BrowserWindowController? { WindowRegistry.shared.frontmost }

    // ── menu actions that need the front window ───────────────────────────
    //
    // Menu items target this delegate rather than a window, because a menu bar belongs to
    // the application and may fire when no window is key at all.

    @objc private func newTab(_ sender: Any?) { front?.newTab(sender) }
    @objc private func closeTab(_ sender: Any?) { front?.closeTab(sender) }
    @objc private func reopenClosedTab(_ sender: Any?) { front?.reopenClosedTab(sender) }
    @objc private func focusAddressBar(_ sender: Any?) { front?.focusAddressBar(sender) }
    @objc private func goBack(_ sender: Any?) { front?.goBack() }
    @objc private func goForward(_ sender: Any?) { front?.goForward() }
    @objc private func reload(_ sender: Any?) { front?.reloadOrStop() }
    @objc private func reloadIgnoringCache(_ sender: Any?) { front?.reloadIgnoringCache() }
    @objc private func stopLoading(_ sender: Any?) { front?.stopLoading() }
    @objc private func home(_ sender: Any?) { front?.openHomePage(sender) }
    @objc private func zoomIn(_ sender: Any?) { front?.zoomIn(sender) }
    @objc private func zoomOut(_ sender: Any?) { front?.zoomOut(sender) }
    @objc private func zoomReset(_ sender: Any?) { front?.zoomReset(sender) }
    @objc private func toggleBookmark(_ sender: Any?) { front?.toggleBookmark() }
    @objc private func toggleBookmarksBar(_ sender: Any?) { front?.toggleBookmarksBar(sender) }
    @objc private func selectNextTab(_ sender: Any?) { front?.selectNextTab(sender) }
    @objc private func selectPreviousTab(_ sender: Any?) { front?.selectPreviousTab(sender) }
    @objc private func engineSettings(_ sender: Any?) { front?.showEngineSettings(sender) }

    @objc private func selectTabByNumber(_ sender: NSMenuItem) {
        front?.selectTab(number: sender.tag)
    }

    /// Menu items point at this delegate rather than at a window, so the window's own
    /// validation is never consulted — greying Back and Forward has to happen here.
    func validateMenuItem(_ item: NSMenuItem) -> Bool {
        guard let front else { return false }
        switch item.action {
        case #selector(goBack(_:)):
            return front.canGoBack
        case #selector(goForward(_:)):
            return front.canGoForward
        default:
            return true
        }
    }

    @objc private func showHelpPage(_ sender: Any?) {
        front?.openTab("gosub://help", activate: true)
    }

    @objc private func showVersionPage(_ sender: Any?) {
        front?.openTab("gosub://version", activate: true)
    }

    // ── the menu bar ──────────────────────────────────────────────────────

    private func buildMenuBar() {
        let mainMenu = NSMenu()

        mainMenu.addItem(applicationMenu())
        mainMenu.addItem(fileMenu())
        mainMenu.addItem(editMenu())
        mainMenu.addItem(viewMenu())
        mainMenu.addItem(historyMenu())
        mainMenu.addItem(bookmarksMenu())
        mainMenu.addItem(windowMenu())
        mainMenu.addItem(helpMenu())

        NSApp.mainMenu = mainMenu
    }

    /// Helper: a menu item wired to this delegate.
    private func item(
        _ title: String,
        _ action: Selector?,
        _ key: String = "",
        _ modifiers: NSEvent.ModifierFlags = .command,
        tag: Int = 0
    ) -> NSMenuItem {
        let menuItem = NSMenuItem(title: title, action: action, keyEquivalent: key)
        menuItem.keyEquivalentModifierMask = modifiers
        menuItem.target = self
        menuItem.tag = tag
        return menuItem
    }

    private func submenu(_ title: String, _ items: [NSMenuItem]) -> NSMenuItem {
        let holder = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        let menu = NSMenu(title: title)
        items.forEach { menu.addItem($0) }
        holder.submenu = menu
        return holder
    }

    private func applicationMenu() -> NSMenuItem {
        // The app menu's title is ignored — macOS always shows the process name in bold —
        // but the standard items are expected in this exact order.
        let about = NSMenuItem(title: "About Gosub Beacon", action: #selector(showAbout), keyEquivalent: "")
        about.target = self

        let hide = NSMenuItem(title: "Hide Gosub Beacon", action: #selector(NSApplication.hide(_:)), keyEquivalent: "h")
        let hideOthers = NSMenuItem(
            title: "Hide Others",
            action: #selector(NSApplication.hideOtherApplications(_:)),
            keyEquivalent: "h"
        )
        hideOthers.keyEquivalentModifierMask = [.command, .option]
        let showAll = NSMenuItem(title: "Show All", action: #selector(NSApplication.unhideAllApplications(_:)), keyEquivalent: "")
        let quit = NSMenuItem(title: "Quit Gosub Beacon", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")

        return submenu("Gosub Beacon", [
            about,
            .separator(),
            item("Engine Settings", #selector(engineSettings(_:))),
            .separator(),
            hide, hideOthers, showAll,
            .separator(),
            quit,
        ])
    }

    private func fileMenu() -> NSMenuItem {
        submenu("File", [
            item("New Window", #selector(newWindowAction(_:)), "n"),
            // Shift-Command-N, as in Safari and Chrome. The GTK shell uses Ctrl+Shift+P.
            item("New Private Window", #selector(newPrivateWindowAction(_:)), "N", [.command, .shift]),
            item("New Tab", #selector(newTab(_:)), "t"),
            .separator(),
            item("Close Tab", #selector(closeTab(_:)), "w"),
            NSMenuItem(title: "Close Window", action: #selector(NSWindow.performClose(_:)), keyEquivalent: "W"),
            item("Reopen Last Closed Tab", #selector(reopenClosedTab(_:)), "T", [.command, .shift]),
        ])
    }

    private func editMenu() -> NSMenuItem {
        // Standard responder-chain actions: they reach the address field when it is focused,
        // which is where text editing actually happens in this shell.
        submenu("Edit", [
            NSMenuItem(title: "Undo", action: Selector(("undo:")), keyEquivalent: "z"),
            NSMenuItem(title: "Redo", action: Selector(("redo:")), keyEquivalent: "Z"),
            .separator(),
            NSMenuItem(title: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x"),
            NSMenuItem(title: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c"),
            NSMenuItem(title: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v"),
            NSMenuItem(title: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a"),
            .separator(),
            item("Open Location…", #selector(focusAddressBar(_:)), "l"),
        ])
    }

    private func viewMenu() -> NSMenuItem {
        let fullScreen = NSMenuItem(
            title: "Enter Full Screen",
            action: #selector(NSWindow.toggleFullScreen(_:)),
            keyEquivalent: "f"
        )
        fullScreen.keyEquivalentModifierMask = [.command, .control]

        return submenu("View", [
            item("Reload Page", #selector(reload(_:)), "r"),
            item("Reload Ignoring Cache", #selector(reloadIgnoringCache(_:)), "R", [.command, .shift]),
            item("Stop", #selector(stopLoading(_:)), "."),
            .separator(),
            item("Actual Size", #selector(zoomReset(_:)), "0"),
            item("Zoom In", #selector(zoomIn(_:)), "+"),
            item("Zoom Out", #selector(zoomOut(_:)), "-"),
            .separator(),
            item("Show Favorites Bar", #selector(toggleBookmarksBar(_:)), "B", [.command, .shift]),
            .separator(),
            fullScreen,
        ])
    }

    private func historyMenu() -> NSMenuItem {
        // Cmd+[ and Cmd+] are the Mac idiom for back and forward. The GTK shell uses
        // Alt+Left/Right, which on a Mac is a word-jump in a text field.
        submenu("History", [
            item("Back", #selector(goBack(_:)), "["),
            item("Forward", #selector(goForward(_:)), "]"),
            .separator(),
            item("Home", #selector(home(_:)), "H", [.command, .shift]),
        ])
    }

    private func bookmarksMenu() -> NSMenuItem {
        submenu("Bookmarks", [
            item("Add Bookmark", #selector(toggleBookmark(_:)), "d"),
            item("Show Favorites Bar", #selector(toggleBookmarksBar(_:)), "B", [.command, .shift]),
        ])
    }

    private func windowMenu() -> NSMenuItem {
        var items: [NSMenuItem] = [
            NSMenuItem(title: "Minimize", action: #selector(NSWindow.performMiniaturize(_:)), keyEquivalent: "m"),
            NSMenuItem(title: "Zoom", action: #selector(NSWindow.performZoom(_:)), keyEquivalent: ""),
            .separator(),
            item("Show Next Tab", #selector(selectNextTab(_:)), "\t", [.control]),
            item("Show Previous Tab", #selector(selectPreviousTab(_:)), "\t", [.control, .shift]),
            .separator(),
        ]
        // Cmd+1..8 select a tab, Cmd+9 the last one — Safari's and Chrome's behaviour.
        for number in 1...9 {
            let title = number == 9 ? "Last Tab" : "Tab \(number)"
            items.append(item(title, #selector(selectTabByNumber(_:)), "\(number)", .command, tag: number))
        }
        let holder = submenu("Window", items)
        NSApp.windowsMenu = holder.submenu
        return holder
    }

    private func helpMenu() -> NSMenuItem {
        let holder = submenu("Help", [
            item("Gosub Beacon Help", #selector(showHelpPage(_:)), "?"),
            item("Engine Version", #selector(showVersionPage(_:))),
        ])
        NSApp.helpMenu = holder.submenu
        return holder
    }

    @objc private func showAbout() {
        if aboutWindow == nil {
            aboutWindow = AboutWindowController()
        }
        aboutWindow?.showWindow(nil)
        aboutWindow?.window?.makeKeyAndOrderFront(nil)
    }
}
