import AppKit

/// The history suggestions under the address bar.
///
/// Not AppKit's own completion list: while that is open it runs its own event loop and
/// takes Return to close itself, so the field never sees it and a typed address that is in
/// history needs two Returns. This list never takes the keyboard; the address bar keeps it
/// and drives the list with the arrow keys, Return and Escape.
final class AddressSuggestions: NSObject, NSTableViewDataSource, NSTableViewDelegate {
    /// Called with the address the reader clicked.
    var onPick: ((String) -> Void)?

    private static let rowHeight: CGFloat = 24

    private let panel = NSPanel(
        contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: true
    )
    private let table = ClickThroughTable()
    private var items: [Browser.VisitedPage] = []

    override init() {
        super.init()
        panel.hasShadow = true
        panel.isReleasedWhenClosed = false

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("address"))
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)
        table.headerView = nil
        table.rowHeight = Self.rowHeight
        table.style = .plain
        table.refusesFirstResponder = true
        table.dataSource = self
        table.delegate = self
        table.target = self
        table.action = #selector(clicked)

        let scroller = NSScrollView()
        scroller.documentView = table
        scroller.hasVerticalScroller = false
        scroller.drawsBackground = false
        panel.contentView = scroller
    }

    var isVisible: Bool { panel.isVisible }

    /// Show `pages` under `field`, nothing highlighted. No pages hides the list.
    func show(_ pages: [Browser.VisitedPage], under field: NSView) {
        guard !pages.isEmpty, let window = field.window else {
            hide()
            return
        }
        items = pages
        table.reloadData()
        table.deselectAll(nil)

        let anchor = window.convertToScreen(field.convert(field.bounds, to: nil))
        let height = CGFloat(pages.count) * (Self.rowHeight + table.intercellSpacing.height)
        panel.setFrame(
            NSRect(x: anchor.minX, y: anchor.minY - height - 2, width: anchor.width, height: height),
            display: true
        )
        if panel.parent == nil {
            window.addChildWindow(panel, ordered: .above)
        }
        panel.orderFront(nil)
    }

    func hide() {
        guard panel.isVisible else { return }
        panel.parent?.removeChildWindow(panel)
        panel.orderOut(nil)
        // With the table: it lays out from its own row count, which outlives a hidden panel.
        items = []
        table.reloadData()
    }

    /// Move the highlight `step` rows and return the address it is on. Moving up off the
    /// first row clears it and returns nil: back to what was typed.
    func move(by step: Int) -> String? {
        guard !items.isEmpty else { return nil }
        let row = min(max(table.selectedRow + step, -1), items.count - 1)
        if row < 0 {
            table.deselectAll(nil)
            return nil
        }
        table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        table.scrollRowToVisible(row)
        return items[row].url
    }

    func numberOfRows(in tableView: NSTableView) -> Int { items.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard row < items.count else { return nil }
        let cell = NSTableCellView()
        let label = NSTextField(labelWithString: items[row].url)
        label.lineBreakMode = .byTruncatingTail
        label.font = .systemFont(ofSize: 13)
        label.translatesAutoresizingMaskIntoConstraints = false
        cell.addSubview(label)
        cell.textField = label
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 6),
            label.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -6),
            label.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
        ])
        return cell
    }

    func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
        AlwaysEmphasizedRow()
    }

    @objc private func clicked() {
        let row = table.clickedRow
        guard row >= 0, row < items.count else { return }
        let url = items[row].url
        hide()
        onPick?(url)
    }
}

/// The panel is never key, and a view in a window that is not key ignores the first click
/// unless it says otherwise.
private final class ClickThroughTable: NSTableView {
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}

/// The highlight in the accent colour, not the grey of a window that is not key: the
/// keyboard is still in the address bar, so the list is the thing being used.
private final class AlwaysEmphasizedRow: NSTableRowView {
    override var isEmphasized: Bool {
        get { true }
        set {}
    }
}
