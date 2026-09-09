import AppKit
import CBeacon

/// The developer panel: a console, the network log and the engine's timing table, docked
/// under the page.
///
/// Three tabs rather than three panels, because they answer the same question from
/// different ends — *what did the engine do*, *what did it fetch*, *how long did it take* —
/// and a developer switches between them constantly.
///
/// It polls rather than being pushed to. Log records arrive on whatever thread the engine
/// happens to be on, requests are folded together as their events go past, and the timing
/// table is written continuously; a snapshot a few times a second is both cheaper and
/// steadier to read than a view that reflows per record. Nothing polls while the panel is
/// closed, and nothing is captured either: response bodies and unredacted headers are only
/// held while someone can look at them.
final class DeveloperPanel: NSView, NSTableViewDataSource, NSTableViewDelegate {
    enum Mode: Int {
        case console
        case network
        case timings
    }

    private let browser: Browser
    private let modeControl: NSSegmentedControl
    private let filterField = NSSearchField()
    private let actionButton = NSButton()
    private let summaryLabel = NSTextField(labelWithString: "")
    private let table = NSTableView()
    private let scroller = NSScrollView()

    /// The request detail, beside the list. Its own scroller so a long header block does
    /// not push the list off the bottom of the panel.
    ///
    /// A table rather than a block of text: a header list *is* key/value data, and banded
    /// rows are what makes forty of them scannable. The body is the exception — that is one
    /// blob of text, and gets a text view.
    private let split = NSSplitView()
    private let detailScroller = NSScrollView()
    private let detailTable = NSTableView()
    private let bodyScroller = NSScrollView()
    private let bodyView = NSTextView()
    private let detailTabs: NSSegmentedControl

    private var mode: Mode = .console
    private var logs: [Browser.LogLine] = []
    private var timings: [Browser.Timing] = []
    private var requests: [Browser.Request] = []
    private var filter = ""
    private var refresh: Timer?

    /// Which request the reader chose, by something that survives the list being
    /// resnapshotted four times a second. A row index does not: rows are appended as the
    /// page fetches, and the oldest fall off the front once the log is full.
    private var selectedRequest: String?
    private var detailPage = 0
    private var detailRows: [DetailRow] = []

    /// One line of the detail pane. Three kinds, because a request's detail is not a flat
    /// dictionary: it has headings, it has name/value pairs, and it has sentences that
    /// explain what a pair means.
    private enum DetailRow {
        case section(String)
        case pair(String, String)
        case note(String)
    }

    /// The user asked to close the panel from its own header.
    var onClose: (() -> Void)?

    /// Whose requests to show. A network panel mixing several tabs together is a log, not
    /// a panel — so the window keeps this pointed at whatever it is showing.
    var tab: BeaconTabId = 0 {
        didSet {
            guard tab != oldValue, mode == .network else { return }
            selectedRequest = nil
            reload()
        }
    }

    static let defaultHeight: CGFloat = 300

    init(browser: Browser) {
        self.browser = browser
        modeControl = NSSegmentedControl(
            labels: ["Console", "Network", "Timings"],
            trackingMode: .selectOne,
            target: nil,
            action: nil
        )
        detailTabs = NSSegmentedControl(
            labels: ["Overview", "Request", "Response", "Body"],
            trackingMode: .selectOne,
            target: nil,
            action: nil
        )
        super.init(frame: .zero)
        build()
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    deinit { refresh?.invalidate() }

    // ── layout ────────────────────────────────────────────────────────────

    private func build() {
        // Nothing in this panel should ever paint outside it.
        clipsToBounds = true
        modeControl.selectedSegment = 0
        modeControl.target = self
        modeControl.action = #selector(modeChanged)
        modeControl.controlSize = .small

        filterField.placeholderString = "Filter"
        filterField.controlSize = .small
        filterField.font = .systemFont(ofSize: 11)
        filterField.target = self
        filterField.action = #selector(filterChanged)
        // Filter as it is typed; the default only fires on Return, which reads as broken in
        // a panel whose whole job is narrowing a long list.
        filterField.sendsSearchStringImmediately = true
        filterField.sendsWholeSearchString = false

        actionButton.title = "Clear"
        actionButton.bezelStyle = .rounded
        actionButton.controlSize = .small
        actionButton.target = self
        actionButton.action = #selector(actionClicked)

        summaryLabel.font = .systemFont(ofSize: 10)
        summaryLabel.textColor = .secondaryLabelColor
        summaryLabel.lineBreakMode = .byTruncatingTail

        let closeButton = NSButton(
            image: NSImage(systemSymbolName: "xmark", accessibilityDescription: "Close") ?? NSImage(),
            target: self,
            action: #selector(closeClicked)
        )
        closeButton.isBordered = false
        closeButton.controlSize = .small

        table.dataSource = self
        table.delegate = self
        table.usesAlternatingRowBackgroundColors = true
        table.rowSizeStyle = .small
        table.headerView = NSTableHeaderView()
        table.allowsColumnResizing = true
        table.allowsEmptySelection = true
        table.style = .fullWidth

        scroller.documentView = table
        scroller.hasVerticalScroller = true
        scroller.autohidesScrollers = true
        scroller.borderType = .noBorder
        scroller.drawsBackground = true
        scroller.backgroundColor = .windowBackgroundColor

        detailTabs.selectedSegment = 0
        detailTabs.target = self
        detailTabs.action = #selector(detailPageChanged)
        detailTabs.controlSize = .small

        detailTable.dataSource = self
        detailTable.delegate = self
        // The same banding as the request list above it, so the two read as one panel.
        detailTable.usesAlternatingRowBackgroundColors = true
        detailTable.headerView = nil
        detailTable.style = .fullWidth
        detailTable.gridStyleMask = []
        // Nothing here is a choice, so nothing highlights; the cells are selectable text
        // instead, because the answer to "what did the server send" gets copied into a bug
        // report and a pane you cannot copy from fails at that.
        detailTable.selectionHighlightStyle = .none
        detailTable.allowsColumnResizing = true
        for (identifier, width) in [("name", CGFloat(150)), ("value", CGFloat(360))] {
            let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(identifier))
            column.width = width
            column.resizingMask = .userResizingMask
            detailTable.addTableColumn(column)
        }

        detailScroller.documentView = detailTable
        detailScroller.hasVerticalScroller = true
        detailScroller.autohidesScrollers = true
        detailScroller.borderType = .noBorder
        detailScroller.drawsBackground = true
        detailScroller.backgroundColor = .windowBackgroundColor

        bodyView.isEditable = false
        bodyView.isSelectable = true
        bodyView.drawsBackground = false
        bodyView.textContainerInset = NSSize(width: 8, height: 6)
        bodyView.isVerticallyResizable = true
        bodyView.isHorizontallyResizable = false
        bodyView.autoresizingMask = [.width]
        bodyView.textContainer?.widthTracksTextView = true

        bodyScroller.documentView = bodyView
        bodyScroller.hasVerticalScroller = true
        bodyScroller.autohidesScrollers = true
        bodyScroller.borderType = .noBorder
        bodyScroller.drawsBackground = true
        bodyScroller.backgroundColor = .windowBackgroundColor
        bodyScroller.isHidden = true

        // The detail pane and its tab strip travel together, so the split view has one
        // child to place rather than two that have to be kept in step.
        let detailPane = NSView()
        for view in [detailTabs, detailScroller, bodyScroller] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            detailPane.addSubview(view)
        }
        NSLayoutConstraint.activate([
            detailTabs.topAnchor.constraint(equalTo: detailPane.topAnchor, constant: 4),
            detailTabs.leadingAnchor.constraint(equalTo: detailPane.leadingAnchor, constant: 8),
            detailScroller.topAnchor.constraint(equalTo: detailTabs.bottomAnchor, constant: 4),
            detailScroller.leadingAnchor.constraint(equalTo: detailPane.leadingAnchor),
            detailScroller.trailingAnchor.constraint(equalTo: detailPane.trailingAnchor),
            detailScroller.bottomAnchor.constraint(equalTo: detailPane.bottomAnchor),
            // The body sits in the same place, and the two swap rather than stack.
            bodyScroller.topAnchor.constraint(equalTo: detailScroller.topAnchor),
            bodyScroller.leadingAnchor.constraint(equalTo: detailPane.leadingAnchor),
            bodyScroller.trailingAnchor.constraint(equalTo: detailPane.trailingAnchor),
            bodyScroller.bottomAnchor.constraint(equalTo: detailPane.bottomAnchor),
        ])
        self.detailPane = detailPane

        split.isVertical = true
        split.dividerStyle = .thin
        split.addArrangedSubview(scroller)
        split.addArrangedSubview(detailPane)
        // The list gives way first: a wider window should show more of a URL, not more
        // whitespace beside the headers.
        split.setHoldingPriority(NSLayoutConstraint.Priority(rawValue: 249), forSubviewAt: 0)
        split.setHoldingPriority(NSLayoutConstraint.Priority(rawValue: 250), forSubviewAt: 1)

        for view in [modeControl, filterField, actionButton, summaryLabel, closeButton, split] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            addSubview(view)
        }

        NSLayoutConstraint.activate([
            modeControl.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 8),
            modeControl.topAnchor.constraint(equalTo: topAnchor, constant: 5),

            actionButton.leadingAnchor.constraint(equalTo: modeControl.trailingAnchor, constant: 8),
            actionButton.centerYAnchor.constraint(equalTo: modeControl.centerYAnchor),

            summaryLabel.leadingAnchor.constraint(equalTo: actionButton.trailingAnchor, constant: 10),
            summaryLabel.centerYAnchor.constraint(equalTo: modeControl.centerYAnchor),

            filterField.leadingAnchor.constraint(greaterThanOrEqualTo: summaryLabel.trailingAnchor, constant: 8),
            filterField.trailingAnchor.constraint(equalTo: closeButton.leadingAnchor, constant: -8),
            filterField.centerYAnchor.constraint(equalTo: modeControl.centerYAnchor),
            filterField.widthAnchor.constraint(equalToConstant: 180),

            closeButton.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -8),
            closeButton.centerYAnchor.constraint(equalTo: modeControl.centerYAnchor),

            split.topAnchor.constraint(equalTo: modeControl.bottomAnchor, constant: 5),
            split.leadingAnchor.constraint(equalTo: leadingAnchor),
            split.trailingAnchor.constraint(equalTo: trailingAnchor),
            split.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])

        rebuildColumns()
        showDetailPane(false)
    }

    /// The detail side of the split, held so it can be hidden on the pages that have none.
    private var detailPane: NSView?

    private func showDetailPane(_ visible: Bool) {
        guard let detailPane, detailPane.isHidden == visible else { return }
        detailPane.isHidden = !visible
        if visible {
            // Placed once, when the pane appears — not on every refresh, or the divider
            // would spring back four times a second and could never be dragged.
            split.setPosition(bounds.width * 0.55, ofDividerAt: 0)
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.windowBackgroundColor.setFill()
        // `bounds`, intersected with what was asked for — never `dirtyRect` alone. AppKit
        // can hand over a rect larger than the view, and since macOS 14 `clipsToBounds`
        // defaults to false, so filling it paints outside this view: a panel 300 points tall
        // painting the window's background over the page above it, which is exactly what it
        // did.
        dirtyRect.intersection(bounds).fill()
        // A hairline on top, separating the panel from the page above it.
        NSColor.separatorColor.setStroke()
        let line = NSBezierPath()
        line.move(to: CGPoint(x: bounds.minX, y: bounds.maxY - 0.5))
        line.line(to: CGPoint(x: bounds.maxX, y: bounds.maxY - 0.5))
        line.lineWidth = 1
        line.stroke()
    }

    // ── polling ───────────────────────────────────────────────────────────

    /// Start or stop polling with the panel's visibility. A closed panel costs nothing —
    /// and, because capture follows the same switch, costs the page nothing either.
    func setActive(_ active: Bool) {
        refresh?.invalidate()
        refresh = nil
        browser.setInspecting(active)
        guard active else { return }
        reload()
        let timer = Timer(timeInterval: 0.25, repeats: true) { [weak self] _ in
            self?.reload()
        }
        RunLoop.main.add(timer, forMode: .common)
        refresh = timer
    }

    private func reload() {
        // Whether the view was pinned to the bottom *before* reloading, so a console that is
        // being watched keeps following new records and one being read stays put.
        let wasAtBottom = isScrolledToBottom

        switch mode {
        case .console:
            logs = browser.logSnapshot()
            summaryLabel.stringValue = "\(filteredLogs.count) of \(logs.count) records"
        case .network:
            requests = browser.networkSnapshot(tab: tab)
            summaryLabel.stringValue = networkSummary()
        case .timings:
            timings = browser.timings()
            let total = timings.reduce(0) { $0 + $1.totalUs }
            summaryLabel.stringValue = "\(timings.count) namespaces · \(Self.duration(total)) total"
        }
        table.reloadData()

        if mode == .network {
            restoreSelection()
            fillDetail()
        }
        if mode == .console, wasAtBottom {
            scrollToBottom()
        }
    }

    /// What the page is still waiting on, and for how long — the first question anyone asks
    /// of an unresponsive page, answered without having to read down the list.
    private func networkSummary() -> String {
        let transferred = requests.reduce(UInt64(0)) { $0 + $1.receivedBytes }
        var summary = "\(requests.count) requests · \(Self.bytes(transferred)) transferred"

        let held = browser.capturedBodyBytes
        if held > 0 {
            summary += " · \(Self.bytes(UInt64(held))) of bodies held"
        }

        let now = Self.nowMs()
        let inFlight = requests.filter { $0.isInFlight }
        if let longest = inFlight.max(by: { now &- $0.startedMs < now &- $1.startedMs }) {
            summary += " · \(inFlight.count) in flight, longest "
            summary += "\(Self.duration((now &- longest.startedMs) * 1000)) \(longest.phaseLabel)"
        }
        return summary
    }

    private var isScrolledToBottom: Bool {
        let visible = scroller.contentView.documentVisibleRect
        let height = table.bounds.height
        // Within a row of the end counts as "at the bottom": demanding exactness means a
        // panel stops following the moment a partial row is showing.
        return visible.maxY >= height - table.rowHeight - 2
    }

    private func scrollToBottom() {
        guard table.numberOfRows > 0 else { return }
        table.scrollRowToVisible(table.numberOfRows - 1)
    }

    // ── columns ───────────────────────────────────────────────────────────

    private func rebuildColumns() {
        for column in table.tableColumns {
            table.removeTableColumn(column)
        }
        let columns: [(String, String, CGFloat)]
        switch mode {
        case .console:
            columns = [
                ("time", "Time", 78), ("level", "Level", 52), ("target", "Source", 160), ("message", "Message", 600),
            ]
        case .network:
            columns = [
                ("status", "Status", 64), ("method", "Method", 56), ("kind", "Kind", 88),
                ("size", "Size", 72), ("time", "Time", 96), ("waterfall", "Waterfall", 140),
                ("url", "URL", 460),
            ]
        case .timings:
            columns = [
                ("namespace", "Namespace", 220), ("count", "Count", 60), ("total", "Total", 80),
                ("avg", "Avg", 80), ("p50", "p50", 80), ("p95", "p95", 80), ("max", "Max", 80),
            ]
        }
        for (identifier, title, width) in columns {
            let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(identifier))
            column.title = title
            column.width = width
            table.addTableColumn(column)
        }
        table.reloadData()
    }

    // ── data ──────────────────────────────────────────────────────────────

    private var filteredLogs: [Browser.LogLine] {
        guard !filter.isEmpty else { return logs }
        let needle = filter.lowercased()
        return logs.filter {
            $0.message.lowercased().contains(needle)
                || $0.target.lowercased().contains(needle)
                || $0.levelName.lowercased().contains(needle)
        }
    }

    private var filteredTimings: [Browser.Timing] {
        guard !filter.isEmpty else { return timings }
        let needle = filter.lowercased()
        return timings.filter {
            $0.namespace.lowercased().contains(needle) || ($0.describes?.lowercased().contains(needle) ?? false)
        }
    }

    private var filteredRequests: [Browser.Request] {
        guard !filter.isEmpty else { return requests }
        let needle = filter.lowercased()
        return requests.filter { $0.url.lowercased().contains(needle) || $0.kind.lowercased().contains(needle) }
    }

    func numberOfRows(in tableView: NSTableView) -> Int {
        if tableView === detailTable {
            return detailRows.count
        }
        switch mode {
        case .console: return filteredLogs.count
        case .network: return filteredRequests.count
        case .timings: return filteredTimings.count
        }
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard let identifier = tableColumn?.identifier else { return nil }
        if tableView === detailTable {
            return detailCell(identifier.rawValue, row: row)
        }

        // The waterfall is drawn, not written: it is the one column whose value is a shape.
        if mode == .network, identifier.rawValue == "waterfall" {
            let rows = filteredRequests
            guard row < rows.count else { return nil }
            var cell = tableView.makeView(withIdentifier: identifier, owner: self) as? WaterfallCell
            if cell == nil {
                let fresh = WaterfallCell()
                fresh.identifier = identifier
                cell = fresh
            }
            guard let cell else { return nil }
            cell.span = waterfall(for: rows[row])
            cell.needsDisplay = true
            return cell
        }

        let cell: NSTextField
        if let reused = tableView.makeView(withIdentifier: identifier, owner: self) as? NSTextField {
            cell = reused
        } else {
            cell = NSTextField(labelWithString: "")
            cell.identifier = identifier
            cell.lineBreakMode = .byTruncatingTail
        }

        // Monospaced everywhere: log lines and numbers both read better in columns, and
        // tabular figures stop the numbers jittering as they update four times a second.
        cell.font = .monospacedDigitSystemFont(ofSize: 10, weight: .regular)
        cell.textColor = .labelColor
        cell.toolTip = nil

        switch mode {
        case .console:
            let entries = filteredLogs
            guard row < entries.count else { return cell }
            let line = entries[row]
            switch identifier.rawValue {
            case "time":
                cell.stringValue = Self.clock(line.timestampMs)
                cell.textColor = .tertiaryLabelColor
            case "level":
                cell.stringValue = line.levelName
                cell.textColor = Self.colour(for: line.level)
            case "target":
                cell.stringValue = line.target
                cell.textColor = .secondaryLabelColor
            default:
                cell.stringValue = line.message
                // Full text on hover: a truncated message is exactly the one worth reading.
                cell.toolTip = line.message
            }
        case .network:
            let entries = filteredRequests
            guard row < entries.count else { return cell }
            fill(cell, column: identifier.rawValue, request: entries[row])
        case .timings:
            let entries = filteredTimings
            guard row < entries.count else { return cell }
            let timing = entries[row]
            // What the namespace measures, from the engine's own table. A namespace it does
            // not know still gets its full name on hover, which a narrow column truncates.
            cell.toolTip = timing.describes ?? timing.namespace
            switch identifier.rawValue {
            case "namespace": cell.stringValue = timing.namespace
            case "count": cell.stringValue = "\(timing.count)"
            case "total": cell.stringValue = Self.duration(timing.totalUs)
            case "avg": cell.stringValue = Self.duration(timing.avgUs)
            case "p50": cell.stringValue = Self.duration(timing.p50Us)
            case "p95": cell.stringValue = Self.duration(timing.p95Us)
            case "max": cell.stringValue = Self.duration(timing.maxUs)
            default: break
            }
        }
        return cell
    }

    func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat {
        guard tableView === detailTable, row < detailRows.count else { return 17 }
        switch detailRows[row] {
        case .section: return 26
        case .pair: return 18
        case .note(let text):
            // Measured rather than guessed: these are whole sentences, and a clipped
            // explanation is worse than none.
            let width = max(detailTable.bounds.width - 24, 160)
            let box = (text as NSString).boundingRect(
                with: NSSize(width: width, height: .greatestFiniteMagnitude),
                options: [.usesLineFragmentOrigin, .usesFontLeading],
                attributes: [.font: NSFont.systemFont(ofSize: 10)]
            )
            return ceil(box.height) + 8
        }
    }

    /// One cell of the detail table.
    private func detailCell(_ column: String, row: Int) -> NSView? {
        guard row < detailRows.count else { return nil }
        let identifier = NSUserInterfaceItemIdentifier("detail." + column)
        let cell: NSTextField
        if let reused = detailTable.makeView(withIdentifier: identifier, owner: self) as? NSTextField {
            cell = reused
        } else {
            cell = NSTextField(labelWithString: "")
            cell.identifier = identifier
            // Selectable so a header value can be copied; still not editable.
            cell.isSelectable = true
        }
        cell.lineBreakMode = .byTruncatingTail
        cell.maximumNumberOfLines = 1
        cell.toolTip = nil

        switch detailRows[row] {
        case .section(let title):
            // A heading spans the row: it names what follows, and has no value half.
            cell.stringValue = column == "name" ? title : ""
            cell.font = .systemFont(ofSize: 11, weight: .semibold)
            cell.textColor = .labelColor
        case .pair(let name, let value):
            cell.stringValue = column == "name" ? name : value
            if column == "name" {
                cell.font = .systemFont(ofSize: 10)
                cell.textColor = .secondaryLabelColor
            } else {
                // The value is the thing being read, so it gets the full-strength colour
                // and the monospaced digits that keep sizes and times aligned.
                cell.font = .monospacedDigitSystemFont(ofSize: 10, weight: .regular)
                cell.textColor = .labelColor
                cell.toolTip = value
            }
        case .note(let text):
            cell.stringValue = column == "name" ? "" : text
            cell.font = .systemFont(ofSize: 10)
            cell.textColor = .tertiaryLabelColor
            cell.lineBreakMode = .byWordWrapping
            cell.maximumNumberOfLines = 0
        }
        return cell
    }

    private func fill(_ cell: NSTextField, column: String, request: Browser.Request) {
        switch column {
        case "status":
            // A failed request has no status code, and "err" says nothing anyone can act
            // on. The kind the engine derived does: "TLS" and "no connection" send you to
            // different places.
            if let status = request.status {
                cell.stringValue = "\(status)"
            } else if request.state == BEACON_REQUEST_FAILED {
                cell.stringValue = request.failureLabel ?? "failed"
            } else {
                cell.stringValue = "—"
            }
            if request.error != nil {
                cell.textColor = .systemRed
                cell.toolTip = request.failureHint ?? request.error
            }
        case "method":
            // A request that never reached the wire has no method: a file:// load, or one
            // answered from cache before a connection was built.
            cell.stringValue = request.method ?? "—"
        case "kind":
            cell.stringValue = request.kind
            cell.toolTip = "Fetched by: \(request.initiator)"
        case "size":
            if request.receivedBytes > 0 {
                cell.stringValue = Self.bytes(request.receivedBytes)
            } else if let declared = request.contentLength {
                cell.stringValue = Self.bytes(declared)
            } else {
                cell.stringValue = "—"
            }
        case "time":
            if let elapsed = request.elapsedUs {
                cell.stringValue = Self.duration(elapsed)
            } else if request.isInFlight {
                // How long it has been in flight, and in which phase — not the word
                // "loading". A page that is not responding is a page with a request sitting
                // at twelve seconds, and that is only visible if the number is on screen.
                let age = (Self.nowMs() &- request.startedMs) * 1000
                cell.stringValue = "\(request.phaseLabel) \(Self.duration(age))…"
                cell.toolTip = request.phaseHint
            } else {
                cell.stringValue = request.stateLabel
            }
        case "url":
            cell.stringValue = request.url
            cell.toolTip = request.url
        default:
            cell.stringValue = ""
        }
    }

    /// Where a request sits, and how it divides, against the whole page's fetching — the
    /// three numbers a waterfall bar is drawn from, each 0...1 of the page's span.
    private func waterfall(for request: Browser.Request) -> (offset: Double, wait: Double, body: Double) {
        let start = requests.map(\.startedMs).min() ?? request.startedMs
        let end =
            requests
            .map { $0.startedMs + ($0.elapsedUs ?? 0) / 1000 }
            .max() ?? (start + 1)
        let span = Double(max(end &- start, 1))

        let offset = Double(request.startedMs &- start) / span
        let total = Double((request.elapsedUs ?? 0) / 1000) / span
        // Wait is the time to the response headers; the rest of the bar is the body coming
        // in after them. A row that is mostly wait is a slow server, one that is mostly body
        // is a big file — which is the whole reason for splitting the bar in two.
        let wait: Double
        if let headers = request.headersMs, headers >= request.startedMs {
            wait = min(Double(headers &- request.startedMs) / span, total)
        } else {
            wait = total
        }
        return (offset, wait, max(total - wait, 0))
    }

    // ── the selected request ──────────────────────────────────────────────

    /// Requests are identified by when they started and where they went: a row index does
    /// not survive the list being snapshotted again, and the ABI hands out no id.
    private static func identity(_ request: Browser.Request) -> String {
        "\(request.startedMs)|\(request.url)"
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        guard notification.object as? NSTableView === table, mode == .network else { return }
        let rows = filteredRequests
        let row = table.selectedRow
        selectedRequest = row >= 0 && row < rows.count ? Self.identity(rows[row]) : nil
        fillDetail()
    }

    /// Put the highlight back on whatever the reader chose, wherever it now sits.
    private func restoreSelection() {
        guard let selectedRequest else { return }
        let rows = filteredRequests
        guard let row = rows.firstIndex(where: { Self.identity($0) == selectedRequest }) else { return }
        if table.selectedRow != row {
            table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        }
    }

    private var selection: Browser.Request? {
        guard let selectedRequest else { return nil }
        return requests.first { Self.identity($0) == selectedRequest }
    }

    /// Fill the detail pane for the selected request.
    ///
    /// Split the way a developer thinks about a request rather than the way the events
    /// arrive: what was asked for, what came back, what the bytes were, and where the time
    /// went.
    private func fillDetail() {
        showDetailPane(mode == .network)
        guard mode == .network else { return }

        // The body is one blob of text and the rest is key/value data, so they are two
        // different views rather than one view pretending.
        let showingBody = detailPage == 3
        bodyScroller.isHidden = !showingBody
        detailScroller.isHidden = showingBody

        guard let request = selection else {
            let note = selectedRequest == nil ? "Select a request." : "That request is no longer recorded."
            setRows([.note(note)])
            setBody(note)
            return
        }

        switch detailPage {
        case 1: setRows(requestDetail(request))
        case 2: setRows(responseDetail(request))
        case 3: setBody(bodyDetail(request))
        default: setRows(overviewDetail(request))
        }
    }

    private func overviewDetail(_ request: Browser.Request) -> [DetailRow] {
        var rows: [DetailRow] = [
            .section("Request"),
            .pair("URL", request.url),
            // A request that never reached the wire has no method: a file:// load, or one
            // answered from cache before a connection was built.
            .pair("Method", request.method ?? "—"),
            .pair("Kind", request.kind),
            .pair("Initiated by", request.initiator),
            .pair("State", request.stateLabel),
        ]

        rows.append(.section("Response"))
        rows.append(.pair("Status", request.status.map { "\($0)" } ?? request.stateLabel))
        if let type = request.contentType {
            rows.append(.pair("Content-Type", type))
        }
        rows.append(.pair("Received", Self.bytes(request.receivedBytes)))
        if let declared = request.contentLength {
            rows.append(.pair("Declared", Self.bytes(declared)))
        }
        if let elapsed = request.elapsedUs {
            rows.append(.pair("Took", Self.duration(elapsed)))
        }

        if let error = request.error {
            rows.append(.section("Failure"))
            if let label = request.failureLabel {
                rows.append(.pair("Cause", label))
            }
            rows.append(.pair("Error", error))
            if let hint = request.failureHint {
                rows.append(.note(hint))
            }
        }

        if !request.redirects.isEmpty {
            rows.append(.section("Redirects"))
            for (status, target) in request.redirects {
                rows.append(.pair("\(status)", target))
            }
        }

        // ── where the time went ───────────────────────────────────────────
        //
        // Against this request's own total rather than the page's: a different question
        // from the waterfall column's "when did this happen relative to everything else".
        rows.append(.section("Timing"))
        if request.isInFlight {
            // No breakdown yet — the totals it divides up only exist once the request ends.
            // What does exist is the fact worth having: which phase it is sitting in.
            rows.append(.pair("Phase", request.phaseLabel))
            rows.append(.pair("Running for", Self.duration((Self.nowMs() &- request.startedMs) * 1000)))
            rows.append(.note(request.phaseHint))
            if let dns = request.dnsUs { rows.append(.pair("DNS took", Self.duration(dns))) }
            if let connect = request.connectUs { rows.append(.pair("Connect took", Self.duration(connect))) }
        } else if let total = request.elapsedUs {
            if request.dnsUs == nil && request.connectUs == nil {
                rows.append(.note("Connection reused: nothing was resolved or dialled for this request."))
            }
            if let dns = request.dnsUs { rows.append(.pair("DNS", Self.duration(dns))) }
            // Connect encloses DNS rather than following it: resolution happens inside the
            // connector being timed.
            if let connect = request.connectUs { rows.append(.pair("Connect", Self.duration(connect))) }
            let wait = request.headersMs.map { min(($0 &- request.startedMs) * 1000, total) } ?? 0
            rows.append(.pair("Waiting", Self.duration(wait)))
            rows.append(.pair("Receiving", Self.duration(total &- wait)))
            rows.append(.pair("Total", Self.duration(total)))
            rows.append(
                .note(
                    "Waiting is the time to the response headers; receiving is the body after them. "
                        + "Blocked, TLS setup and send time are not reported by the network stack."
                )
            )
        } else {
            rows.append(.note("No timing recorded: this request is \(request.stateLabel)."))
        }

        // What the page went on to fetch because of this document. Grouped by exclusion,
        // not by parentage: the engine records no initiating request id, so this is
        // everything fetched that a navigation did not ask for.
        if request.kind == "document" {
            let children = requests.filter { Self.identity($0) != Self.identity(request) && $0.initiator != "navigation" }
            rows.append(.section("Subresources"))
            if children.isEmpty {
                rows.append(.note("This document pulled in nothing else."))
            } else {
                for child in children {
                    let size = child.error == nil ? Self.bytes(child.receivedBytes) : child.stateLabel
                    rows.append(.pair("\(child.kind) · \(size)", child.url))
                }
                rows.append(
                    .note(
                        "Everything the page fetched that a navigation did not: the engine records "
                            + "no initiating request, so this is grouped by exclusion."
                    )
                )
            }
        }
        return rows
    }

    private func requestDetail(_ request: Browser.Request) -> [DetailRow] {
        var rows: [DetailRow] = [
            .section("\(request.method ?? "GET") \(request.url)")
        ]
        guard !request.requestHeaders.isEmpty else {
            rows.append(.note("No request line recorded: this never reached the network."))
            return rows
        }

        rows.append(.section("Request headers"))
        for (name, value) in request.requestHeaders {
            rows.append(.pair(name, value))
        }
        // Everything the stack sends is listed, so the note is down to the headers that
        // cannot be: `host` is added by the connection itself. Derived rather than written
        // out, so it cannot go on claiming a header is missing after one starts appearing.
        var absent = ["host"]
        if !request.requestHeaders.contains(where: { $0.0.caseInsensitiveCompare("accept-encoding") == .orderedSame }) {
            absent.append("accept-encoding")
        }
        rows.append(.note("Added below this layer and not visible here: \(absent.joined(separator: ", "))."))
        return rows
    }

    private func responseDetail(_ request: Browser.Request) -> [DetailRow] {
        var rows: [DetailRow] = [
            .section(request.status.map { "HTTP/1.1 \($0)" } ?? "No response: \(request.stateLabel)")
        ]
        guard !request.responseHeaders.isEmpty else {
            rows.append(.note("No response headers recorded."))
            return rows
        }
        rows.append(.section("Response headers"))
        for (name, value) in request.responseHeaders {
            rows.append(.pair(name, value))
        }
        return rows
    }

    private func bodyDetail(_ request: Browser.Request) -> String {
        if let body = browser.bodyText(at: request.index) {
            return body
        }
        if request.bodyEvicted {
            return """
                Captured, then dropped.

                The panel keeps a fixed total of body bytes and this was among the oldest
                when that ran out. Reload to capture it again.
                """
        }
        return """
            No body captured for this request.

            Capture starts when this panel opens, so anything that finished earlier has
            none — reload the page to record it.

            Video, audio and downloads are never captured, and neither is a response that
            declares itself larger than the per-request cap.
            """
    }

    /// Rewrite the detail table, but only when something changed: it is refilled four times
    /// a second, and reloading throws away the reader's scroll position and selection.
    private func setRows(_ rows: [DetailRow]) {
        guard rows.map(Self.describe) != detailRows.map(Self.describe) else { return }
        detailRows = rows
        detailTable.reloadData()
    }

    private static func describe(_ row: DetailRow) -> String {
        switch row {
        case .section(let title): return "s\(title)"
        case .pair(let name, let value): return "p\(name)\u{1}\(value)"
        case .note(let text): return "n\(text)"
        }
    }

    private func setBody(_ text: String) {
        guard bodyView.string != text else { return }
        bodyView.string = text
        bodyView.font = .monospacedSystemFont(ofSize: 10, weight: .regular)
        bodyView.textColor = .labelColor
    }

    // ── formatting ────────────────────────────────────────────────────────

    private static func colour(for level: UInt32) -> NSColor {
        switch level {
        case BEACON_LOG_ERROR: return .systemRed
        case BEACON_LOG_WARN: return .systemOrange
        case BEACON_LOG_INFO: return .labelColor
        default: return .tertiaryLabelColor
        }
    }

    /// Microseconds in whatever unit keeps the number readable. A table of "4466000µs" is
    /// technically right and useless.
    static func duration(_ microseconds: UInt64) -> String {
        if microseconds >= 1_000_000 {
            return String(format: "%.2fs", Double(microseconds) / 1_000_000)
        }
        if microseconds >= 1_000 {
            return String(format: "%.1fms", Double(microseconds) / 1_000)
        }
        return "\(microseconds)µs"
    }

    static func bytes(_ count: UInt64) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(count), countStyle: .file)
    }

    private static let clockFormatter: DateFormatter = {
        let formatter = DateFormatter()
        // Seconds and milliseconds, not a locale's idea of a time: this is read against
        // other log lines, and the date is never in question.
        formatter.dateFormat = "HH:mm:ss.SSS"
        return formatter
    }()

    static func clock(_ timestampMs: UInt64) -> String {
        clockFormatter.string(from: Date(timeIntervalSince1970: Double(timestampMs) / 1000))
    }

    private static func nowMs() -> UInt64 {
        UInt64(Date().timeIntervalSince1970 * 1000)
    }

    // ── actions ───────────────────────────────────────────────────────────

    @objc private func modeChanged() {
        mode = Mode(rawValue: modeControl.selectedSegment) ?? .console
        switch mode {
        case .console:
            actionButton.title = "Clear"
            actionButton.toolTip = "Discard the captured log records"
        case .network:
            actionButton.title = "Clear"
            actionButton.toolTip = "Forget the requests recorded so far"
        case .timings:
            actionButton.title = "Reset"
            actionButton.toolTip = "Start timing again from nothing, so the next navigation is measured on its own"
        }
        rebuildColumns()
        reload()
        fillDetail()
    }

    @objc private func detailPageChanged() {
        detailPage = detailTabs.selectedSegment
        fillDetail()
    }

    @objc private func filterChanged() {
        filter = filterField.stringValue
        reload()
    }

    @objc private func actionClicked() {
        switch mode {
        case .console: browser.clearLogs()
        case .network:
            browser.clearRequests()
            selectedRequest = nil
        case .timings: browser.resetTimings()
        }
        reload()
    }

    @objc private func closeClicked() { onClose?() }

    /// Switch tabs from the menu bar.
    func show(_ mode: Mode) {
        modeControl.selectedSegment = mode.rawValue
        modeChanged()
    }
}

/// One request's bar in the waterfall column: where it started against the page's whole
/// span, how much of it was waiting for the server, and how much was the body arriving.
///
/// Drawn rather than written because the shape *is* the information — which requests
/// overlap, which one started late, which one is all wait.
private final class WaterfallCell: NSView {
    var span: (offset: Double, wait: Double, body: Double) = (0, 0, 0)

    override func draw(_ dirtyRect: NSRect) {
        let height: CGFloat = 6
        let y = (bounds.height - height) / 2
        let width = bounds.width

        // A request too fast to have a width still gets a mark: a bar that vanishes reads
        // as a row that did nothing.
        let minimum: CGFloat = 2
        let x = bounds.minX + CGFloat(span.offset) * width
        let waitWidth = max(CGFloat(span.wait) * width, span.body > 0 ? 0 : minimum)
        let bodyWidth = max(CGFloat(span.body) * width, 0)

        NSColor.tertiaryLabelColor.setFill()
        NSBezierPath(rect: NSRect(x: x, y: y, width: waitWidth, height: height)).fill()
        NSColor.controlAccentColor.setFill()
        NSBezierPath(rect: NSRect(x: x + waitWidth, y: y, width: bodyWidth, height: height)).fill()
    }
}
