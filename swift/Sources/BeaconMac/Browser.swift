import AppKit
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

    /// A private session: cookies and storage in memory only, no visited history recorded.
    /// Bookmarks and settings remain the shared persistent ones.
    var isPrivate: Bool { beacon_is_private(handle) }

    // ── tabs ──────────────────────────────────────────────────────────────

    @discardableResult
    func openTab(_ url: String) -> BeaconTabId {
        url.withCString { beacon_open_tab(handle, $0) }
    }

    func closeTab(_ tab: BeaconTabId) { beacon_close_tab(handle, tab) }
    func activateTab(_ tab: BeaconTabId) { beacon_activate_tab(handle, tab) }

    /// Bring back the most recently closed tab. 0 when there is nothing to reopen.
    @discardableResult
    func reopenClosedTab() -> BeaconTabId { beacon_reopen_closed_tab(handle) }

    func isPinned(_ tab: BeaconTabId) -> Bool { beacon_tab_is_pinned(handle, tab) }
    func setPinned(_ tab: BeaconTabId, _ pinned: Bool) { beacon_set_tab_pinned(handle, tab, pinned) }
    func moveTab(_ tab: BeaconTabId, to index: Int) { beacon_move_tab(handle, tab, index) }

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

    /// 0...1, or nil when nothing is loading or the server never sent a length. nil means
    /// an indeterminate indicator, not an empty bar.
    func progress(of tab: BeaconTabId) -> Double? {
        let value = beacon_tab_progress(handle, tab)
        return value < 0 ? nil : value
    }

    /// The favicon, decoded. The ABI lends the bytes only until the next call, so this
    /// copies them into an NSImage immediately and never holds the pointer.
    func favicon(of tab: BeaconTabId) -> NSImage? {
        var length = 0
        guard let bytes = beacon_tab_favicon(handle, tab, &length), length > 0 else { return nil }
        let data = Data(bytes: bytes, count: length)
        return NSImage(data: data)
    }

    // ── commands ──────────────────────────────────────────────────────────

    func navigate(_ tab: BeaconTabId, to url: String) {
        url.withCString { beacon_navigate(handle, tab, $0) }
    }

    func back() { beacon_back(handle) }
    func forward() { beacon_forward(handle) }
    func reload(ignoringCache: Bool = false) { beacon_reload(handle, ignoringCache) }
    func stop() { beacon_stop(handle) }

    // ── input ─────────────────────────────────────────────────────────────

    /// Page area in CSS pixels, and device pixels per CSS pixel. The scale is what keeps
    /// text sharp on a Retina display.
    func setViewport(_ tab: BeaconTabId, width: UInt32, height: UInt32, scale: Float) {
        beacon_set_viewport(handle, tab, width, height, scale)
    }

    func mouseMoved(_ tab: BeaconTabId, x: Float, y: Float) { beacon_mouse_move(handle, tab, x, y) }
    func mouseDown(_ tab: BeaconTabId, x: Float, y: Float, button: BeaconButton = BEACON_BUTTON_LEFT) {
        beacon_mouse_down(handle, tab, x, y, button)
    }
    func mouseUp(_ tab: BeaconTabId, x: Float, y: Float, button: BeaconButton = BEACON_BUTTON_LEFT) {
        beacon_mouse_up(handle, tab, x, y, button)
    }
    func scroll(_ tab: BeaconTabId, dx: Float, dy: Float) { beacon_scroll(handle, tab, dx, dy) }

    // ── keyboard ──────────────────────────────────────────────────────────

    func keyDown(_ tab: BeaconTabId, key: String, code: String?, modifiers: UInt32) {
        key.withCString { k in
            if let code {
                code.withCString { c in beacon_key_down(handle, tab, k, c, modifiers) }
            } else {
                beacon_key_down(handle, tab, k, nil, modifiers)
            }
        }
    }

    func keyUp(_ tab: BeaconTabId, key: String, code: String?, modifiers: UInt32) {
        key.withCString { k in
            if let code {
                code.withCString { c in beacon_key_up(handle, tab, k, c, modifiers) }
            } else {
                beacon_key_up(handle, tab, k, nil, modifiers)
            }
        }
    }

    /// Committed text from the input method — the only correct route for dead keys, CJK
    /// and emoji. Never synthesised from key names.
    func textInput(_ tab: BeaconTabId, text: String) {
        text.withCString { beacon_text_input(handle, tab, $0) }
    }

    // ── zoom ──────────────────────────────────────────────────────────────

    /// The ladder mainstream browsers step through, so Cmd+= lands on familiar numbers.
    static let zoomLevels: [Float] = [0.25, 0.33, 0.5, 0.67, 0.75, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0]

    func zoom(of tab: BeaconTabId) -> Float { beacon_zoom(handle, tab) }
    func setZoom(_ tab: BeaconTabId, _ zoom: Float) { beacon_set_zoom(handle, tab, zoom) }

    /// Step one notch up (+1) or down (-1) the ladder.
    func stepZoom(_ tab: BeaconTabId, by direction: Int) {
        let current = zoom(of: tab)
        let index = Self.zoomLevels.firstIndex { abs($0 - current) < 0.001 } ?? 6
        let next = min(max(index + direction, 0), Self.zoomLevels.count - 1)
        setZoom(tab, Self.zoomLevels[next])
    }

    // ── bookmarks ─────────────────────────────────────────────────────────

    func isBookmarked(_ tab: BeaconTabId) -> Bool { beacon_tab_is_bookmarked(handle, tab) }
    @discardableResult
    func toggleBookmark(_ tab: BeaconTabId) -> Bool { beacon_toggle_bookmark(handle, tab) }

    struct Bookmark {
        let url: String
        let title: String
    }

    var bookmarks: [Bookmark] {
        (0..<beacon_bookmark_count(handle)).map { index in
            Bookmark(
                url: takeString(beacon_bookmark_url(handle, index)),
                title: takeString(beacon_bookmark_title(handle, index))
            )
        }
    }

    // ── history ───────────────────────────────────────────────────────────

    struct VisitedPage {
        let url: String
        let title: String
        let visitCount: UInt64
    }

    /// Visited pages matching `query`, best first. An empty query matches nothing — a
    /// suggestion list for "" would be a history browser, not an autocomplete.
    func searchHistory(_ query: String, limit: Int = 8) -> [VisitedPage] {
        let count = query.withCString { beacon_history_search(handle, $0, limit) }
        return (0..<count).map { index in
            VisitedPage(
                url: takeString(beacon_history_url(handle, index)),
                title: takeString(beacon_history_title(handle, index)),
                visitCount: beacon_history_visit_count(handle, index)
            )
        }
    }

    // ── developer panel ───────────────────────────────────────────────────

    struct LogLine {
        let level: UInt32
        let target: String
        let message: String

        var levelName: String {
            switch level {
            case BEACON_LOG_ERROR: return "ERROR"
            case BEACON_LOG_WARN: return "WARN"
            case BEACON_LOG_INFO: return "INFO"
            case BEACON_LOG_DEBUG: return "DEBUG"
            default: return "TRACE"
            }
        }
    }

    /// The newest `max` log records. What is captured depends on `BEACON_LOG`/`RUST_LOG`,
    /// which default to warnings only.
    func logSnapshot(max: Int = 1000) -> [LogLine] {
        let count = beacon_log_snapshot(handle, max)
        return (0..<count).map { index in
            LogLine(
                level: beacon_log_level(handle, index),
                target: takeString(beacon_log_target(handle, index)),
                message: takeString(beacon_log_message(handle, index))
            )
        }
    }

    func clearLogs() { beacon_log_clear(handle) }

    struct Timing {
        let namespace: String
        let count: UInt64
        let totalUs: UInt64
        let minUs: UInt64
        let maxUs: UInt64
        let avgUs: UInt64
        let p50Us: UInt64
        let p95Us: UInt64
        let p99Us: UInt64
    }

    /// The engine's timing table, slowest namespace first. Empty when the engine was built
    /// without its `timing` feature.
    func timings() -> [Timing] {
        let count = beacon_timing_snapshot(handle)
        return (0..<count).compactMap { index in
            var row = BeaconTiming()
            guard beacon_timing_at(handle, index, &row) else { return nil }
            return Timing(
                namespace: takeString(beacon_timing_namespace(handle, index)),
                count: row.count,
                totalUs: row.total_us,
                minUs: row.min_us,
                maxUs: row.max_us,
                avgUs: row.avg_us,
                p50Us: row.p50_us,
                p95Us: row.p95_us,
                p99Us: row.p99_us
            )
        }
    }

    func resetTimings() { beacon_timing_reset(handle) }

    // ── downloads ─────────────────────────────────────────────────────────

    struct Download {
        let id: UInt64
        let filename: String
        let path: String
        let progress: Double?
        let received: UInt64
        let state: BeaconDownloadState
    }

    var downloads: [Download] {
        (0..<beacon_download_count(handle)).compactMap { index in
            let id = beacon_download_at(handle, index)
            guard id != 0 else { return nil }
            let fraction = beacon_download_progress(handle, id)
            return Download(
                id: id,
                filename: takeString(beacon_download_filename(handle, id)),
                path: takeString(beacon_download_path(handle, id)),
                progress: fraction < 0 ? nil : fraction,
                received: beacon_download_received(handle, id),
                state: beacon_download_state(handle, id)
            )
        }
    }

    func downloadOfferURL(_ offer: UInt64) -> String {
        takeString(beacon_download_offer_url(handle, offer))
    }

    @discardableResult
    func acceptDownload(_ offer: UInt64, to path: String) -> UInt64 {
        path.withCString { beacon_download_accept(handle, offer, $0) }
    }

    func rejectDownload(_ offer: UInt64) { beacon_download_reject(handle, offer) }
    func openDownload(_ id: UInt64) { beacon_download_open(handle, id) }

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
