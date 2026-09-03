import AppKit
import CBeacon

/// The developer panel: a console and the engine's timing table, docked under the page.
///
/// Two tabs rather than two panels, because they answer the same question from opposite
/// ends — *what did the engine do* and *how long did it take* — and a developer switches
/// between them constantly.
///
/// It polls rather than being pushed to. Log records arrive on whatever thread the engine
/// happens to be on and timings are a table being written continuously; a snapshot a few
/// times a second is both cheaper and steadier to read than a view that reflows per record.
/// Nothing polls while the panel is closed.
final class DeveloperPanel: NSView, NSTableViewDataSource, NSTableViewDelegate {
    enum Mode: Int {
        case console
        case timings
    }

    private let browser: Browser
    private let modeControl: NSSegmentedControl
    private let filterField = NSSearchField()
    private let actionButton = NSButton()
    private let summaryLabel = NSTextField(labelWithString: "")
    private let table = NSTableView()
    private let scroller = NSScrollView()

    private var mode: Mode = .console
    private var logs: [Browser.LogLine] = []
    private var timings: [Browser.Timing] = []
    private var filter = ""
    private var refresh: Timer?

    /// The user asked to close the panel from its own header.
    var onClose: (() -> Void)?

    static let defaultHeight: CGFloat = 260

    init(browser: Browser) {
        self.browser = browser
        modeControl = NSSegmentedControl(
            labels: ["Console", "Timings"],
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
        table.style = .plain

        scroller.documentView = table
        scroller.hasVerticalScroller = true
        scroller.autohidesScrollers = true
        scroller.borderType = .noBorder
        scroller.drawsBackground = false

        for view in [modeControl, filterField, actionButton, summaryLabel, closeButton, scroller] as [NSView] {
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

            scroller.topAnchor.constraint(equalTo: modeControl.bottomAnchor, constant: 5),
            scroller.leadingAnchor.constraint(equalTo: leadingAnchor),
            scroller.trailingAnchor.constraint(equalTo: trailingAnchor),
            scroller.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])

        rebuildColumns()
    }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.windowBackgroundColor.setFill()
        dirtyRect.fill()
        // A hairline on top, separating the panel from the page above it.
        NSColor.separatorColor.setStroke()
        let line = NSBezierPath()
        line.move(to: CGPoint(x: bounds.minX, y: bounds.maxY - 0.5))
        line.line(to: CGPoint(x: bounds.maxX, y: bounds.maxY - 0.5))
        line.lineWidth = 1
        line.stroke()
    }

    // ── polling ───────────────────────────────────────────────────────────

    /// Start or stop polling with the panel's visibility. A closed panel costs nothing.
    func setActive(_ active: Bool) {
        refresh?.invalidate()
        refresh = nil
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
        case .timings:
            timings = browser.timings()
            let total = timings.reduce(0) { $0 + $1.totalUs }
            summaryLabel.stringValue = "\(timings.count) namespaces · \(Self.duration(total)) total"
        }
        table.reloadData()

        if mode == .console, wasAtBottom {
            scrollToBottom()
        }
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
            columns = [("level", "Level", 52), ("target", "Source", 170), ("message", "Message", 600)]
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
        return timings.filter { $0.namespace.lowercased().contains(needle) }
    }

    func numberOfRows(in tableView: NSTableView) -> Int {
        mode == .console ? filteredLogs.count : filteredTimings.count
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard let identifier = tableColumn?.identifier else { return nil }

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

        switch mode {
        case .console:
            let entries = filteredLogs
            guard row < entries.count else { return cell }
            let line = entries[row]
            switch identifier.rawValue {
            case "level":
                cell.stringValue = line.levelName
                cell.textColor = Self.colour(for: line.level)
            case "target":
                cell.stringValue = line.target
                cell.textColor = .secondaryLabelColor
            default:
                cell.stringValue = line.message
            }
        case .timings:
            let entries = filteredTimings
            guard row < entries.count else { return cell }
            let timing = entries[row]
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

    // ── actions ───────────────────────────────────────────────────────────

    @objc private func modeChanged() {
        mode = Mode(rawValue: modeControl.selectedSegment) ?? .console
        actionButton.title = mode == .console ? "Clear" : "Reset"
        actionButton.toolTip =
            mode == .console
            ? "Discard the captured log records"
            : "Start timing again from nothing, so the next navigation is measured on its own"
        rebuildColumns()
        reload()
    }

    @objc private func filterChanged() {
        filter = filterField.stringValue
        reload()
    }

    @objc private func actionClicked() {
        switch mode {
        case .console: browser.clearLogs()
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
