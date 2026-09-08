import AppKit

/// The open browser windows.
///
/// Windows are a shell concept — `beacon-core` has tabs and no windows at all — so this is
/// one of the very few things the shell is allowed to remember. It holds the controllers
/// alive (AppKit does not) and answers "which window is the user looking at" for the menu
/// bar, whose items have no window of their own.
final class WindowRegistry {
    static let shared = WindowRegistry()

    private(set) var windows: [BrowserWindowController] = []

    /// Fired whenever a window opens or closes. The application uses it to tear the private
    /// session down once the last private window is gone.
    var onChange: (() -> Void)?

    func add(_ controller: BrowserWindowController) {
        windows.append(controller)
        onChange?()
    }

    func remove(_ controller: BrowserWindowController) {
        windows.removeAll { $0 === controller }
        onChange?()
    }

    var hasPrivateWindow: Bool {
        windows.contains { $0.isPrivate }
    }

    /// The window a menu command should act on: the key window, or the last one to exist.
    var frontmost: BrowserWindowController? {
        windows.first { $0.window?.isKeyWindow == true } ?? windows.last
    }
}
