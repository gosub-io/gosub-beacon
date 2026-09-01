import CBeacon
import Foundation

/// Swift's view of the browser. A thin wrapper over the C ABI that turns pointers into
/// values and nothing else.
///
/// Deliberately holds no state of its own — no tab array, no current URL. Everything is
/// asked for when needed. That is the discipline the ABI is built around: two lists that
/// can disagree is the bug this architecture exists to avoid, and the shell is where it
/// would creep back in.
final class Browser {
    private let handle: OpaquePointer

    init?(profileDirectory: String? = nil, private isPrivate: Bool = false) {
        // The config's string must outlive the call, hence the explicit scope.
        var created: OpaquePointer?
        if let dir = profileDirectory {
            dir.withCString { cdir in
                var config = BeaconConfig(user_data_dir: cdir, private_mode: isPrivate)
                created = beacon_new(&config)
            }
        } else {
            var config = BeaconConfig(user_data_dir: nil, private_mode: isPrivate)
            created = beacon_new(&config)
        }
        guard let created else { return nil }
        handle = created
    }

    deinit {
        beacon_free(handle)
    }

    // ── tabs ──────────────────────────────────────────────────────────────

    @discardableResult
    func openTab(_ url: String) -> BeaconTabId {
        url.withCString { beacon_open_tab(handle, $0) }
    }

    func closeTab(_ tab: BeaconTabId) { beacon_close_tab(handle, tab) }
    func activateTab(_ tab: BeaconTabId) { beacon_activate_tab(handle, tab) }

    var tabCount: Int { beacon_tab_count(handle) }
    var activeTab: BeaconTabId { beacon_active_tab(handle) }
    func tab(at index: Int) -> BeaconTabId { beacon_tab_at(handle, index) }

    /// Every open tab, in strip order. Asked for fresh each time the strip is rebuilt.
    var tabs: [BeaconTabId] { (0..<tabCount).map { tab(at: $0) } }

    func title(of tab: BeaconTabId) -> String { takeString(beacon_tab_title(handle, tab)) }
    func url(of tab: BeaconTabId) -> String { takeString(beacon_tab_url(handle, tab)) }
    func isLoading(_ tab: BeaconTabId) -> Bool { beacon_tab_is_loading(handle, tab) }
    func canGoBack(_ tab: BeaconTabId) -> Bool { beacon_tab_can_go_back(handle, tab) }
    func canGoForward(_ tab: BeaconTabId) -> Bool { beacon_tab_can_go_forward(handle, tab) }

    // ── commands ──────────────────────────────────────────────────────────

    func navigate(_ tab: BeaconTabId, to url: String) {
        url.withCString { beacon_navigate(handle, tab, $0) }
    }

    func back() { beacon_back(handle) }
    func forward() { beacon_forward(handle) }
    func reload(ignoringCache: Bool = false) { beacon_reload(handle, ignoringCache) }
    func stop() { beacon_stop(handle) }

    // ── input ─────────────────────────────────────────────────────────────

    /// Page area in CSS pixels. Nothing renders until the engine knows this.
    func setViewport(_ tab: BeaconTabId, width: UInt32, height: UInt32) {
        beacon_set_viewport(handle, tab, width, height)
    }

    func mouseMoved(_ tab: BeaconTabId, x: Float, y: Float) { beacon_mouse_move(handle, tab, x, y) }
    func mouseDown(_ tab: BeaconTabId, x: Float, y: Float) { beacon_mouse_down(handle, tab, x, y, BEACON_BUTTON_LEFT) }
    func scroll(_ tab: BeaconTabId, dx: Float, dy: Float) { beacon_scroll(handle, tab, dx, dy) }

    // ── the page, drawn into a view we own ────────────────────────────────

    /// Hand Beacon an NSView to render into. Sizes are device pixels.
    @discardableResult
    func attach(_ tab: BeaconTabId, to view: UnsafeMutableRawPointer, width: UInt32, height: UInt32) -> Bool {
        beacon_attach_view(handle, tab, view, width, height)
    }

    func detach(_ tab: BeaconTabId) { beacon_detach_view(handle, tab) }
    func resizeView(_ tab: BeaconTabId, width: UInt32, height: UInt32) { beacon_resize_view(handle, tab, width, height) }

    @discardableResult
    func draw(_ tab: BeaconTabId) -> Bool { beacon_draw_view(handle, tab) }

    // ── events ────────────────────────────────────────────────────────────

    /// Drain everything the browser has said since the last call. Must run on the main
    /// thread — the ABI is pulled, never pushed, precisely so this is the only thread it
    /// ever touches.
    func pollEvents() -> [Event] {
        var out: [Event] = []
        var buffer = [BeaconEvent](repeating: BeaconEvent(), count: 64)
        while true {
            let n = buffer.withUnsafeMutableBufferPointer { beacon_poll_events(handle, $0.baseAddress, 64) }
            if n == 0 { break }
            for i in 0..<n {
                let raw = buffer[i]
                // `text` is borrowed until the next poll, so copy it now.
                let text = raw.text.map { String(cString: $0) }
                out.append(Event(kind: raw.kind, tab: raw.tab, text: text, number: raw.number))
            }
            if n < 64 { break }
        }
        return out
    }

    struct Event {
        let kind: BeaconEventKind
        let tab: BeaconTabId
        let text: String?
        let number: Double
    }

    /// Strings from the ABI are ours to free.
    private func takeString(_ pointer: UnsafeMutablePointer<CChar>?) -> String {
        guard let pointer else { return "" }
        defer { beacon_string_free(pointer) }
        return String(cString: pointer)
    }
}
