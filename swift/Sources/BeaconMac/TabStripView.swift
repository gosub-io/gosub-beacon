import AppKit
import CBeacon

/// One tab's chip: favicon, title, close button.
///
/// Drawn rather than assembled from controls. A browser rebuilds this strip on every title,
/// favicon and loading change — several times a second on a busy page — and tearing down a
/// stack of NSViews that often is both slower and flickerier than filling a few rects.
final class TabChip: NSView {
    let tab: BeaconTabId
    var title: String = ""
    var favicon: NSImage?
    var isActive = false
    var isLoading = false
    var isPinned = false

    /// Called with this chip's tab. The strip owns the behaviour; the chip only reports.
    var onSelect: ((BeaconTabId) -> Void)?
    var onClose: ((BeaconTabId) -> Void)?
    var onContextMenu: ((BeaconTabId, NSEvent) -> Void)?
    /// Dragging is the strip's business — it is the only thing that knows the other chips.
    var onDrag: ((TabChip, NSEvent) -> Void)?
    var onDragEnd: ((TabChip) -> Void)?

    /// Lifted while being dragged, so it reads as picked up rather than merely misplaced.
    var isDragging = false {
        didSet { needsDisplay = true }
    }

    private var closeHovered = false
    private var hovered = false
    private var tracking: NSTrackingArea?

    /// Pinned tabs shrink to the favicon, as they do in every browser that has them.
    static let pinnedWidth: CGFloat = 40
    static let maxWidth: CGFloat = 220
    static let minWidth: CGFloat = 70

    init(tab: BeaconTabId) {
        self.tab = tab
        super.init(frame: .zero)
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    override var isFlipped: Bool { true }

    private var closeRect: CGRect {
        CGRect(x: bounds.maxX - 22, y: (bounds.height - 14) / 2, width: 14, height: 14)
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(
            rect: bounds,
            options: [.activeInKeyWindow, .mouseEnteredAndExited, .mouseMoved, .inVisibleRect],
            owner: self,
            userInfo: nil
        )
        addTrackingArea(area)
        tracking = area
    }

    override func mouseEntered(with event: NSEvent) {
        hovered = true
        needsDisplay = true
    }

    override func mouseExited(with event: NSEvent) {
        hovered = false
        closeHovered = false
        needsDisplay = true
    }

    override func mouseMoved(with event: NSEvent) {
        let inside = closeRect.contains(convert(event.locationInWindow, from: nil))
        if inside != closeHovered {
            closeHovered = inside
            needsDisplay = true
        }
    }

    /// Where the press started, so a drag is told apart from a click by distance rather
    /// than by timing — a slow, deliberate click should still select.
    private var pressOrigin: NSPoint?

    override func mouseDown(with event: NSEvent) {
        if !isPinned, closeRect.contains(convert(event.locationInWindow, from: nil)) {
            onClose?(tab)
            return
        }
        pressOrigin = event.locationInWindow
        // Selecting on press, not release, is what every browser does: the tab switches
        // immediately and the drag continues from there.
        onSelect?(tab)
    }

    override func mouseDragged(with event: NSEvent) {
        guard let origin = pressOrigin else { return }
        // A few points of slop, or a click with a shaky hand reorders the strip.
        if !isDragging, abs(event.locationInWindow.x - origin.x) < 4 {
            return
        }
        onDrag?(self, event)
    }

    override func mouseUp(with event: NSEvent) {
        pressOrigin = nil
        if isDragging {
            onDragEnd?(self)
        }
    }

    /// Middle-click closes, the way it does in every other browser.
    override func otherMouseDown(with event: NSEvent) {
        if event.buttonNumber == 2 {
            onClose?(tab)
        }
    }

    override func rightMouseDown(with event: NSEvent) {
        onContextMenu?(tab, event)
    }

    override func draw(_ dirtyRect: NSRect) {
        let radius: CGFloat = 6
        // The active tab sits on the content's own background so it reads as continuous
        // with the page below it; the rest recede.
        let background: NSColor
        if isActive {
            background = .controlBackgroundColor
        } else if hovered {
            background = NSColor.controlBackgroundColor.withAlphaComponent(0.5)
        } else {
            background = .clear
        }

        let body = bounds.insetBy(dx: 1, dy: 3)
        let path = NSBezierPath(roundedRect: body, xRadius: radius, yRadius: radius)
        if isDragging {
            // A shadow under the dragged chip, so it reads as being above the strip.
            NSGraphicsContext.saveGraphicsState()
            let shadow = NSShadow()
            shadow.shadowColor = NSColor.shadowColor.withAlphaComponent(0.35)
            shadow.shadowBlurRadius = 6
            shadow.shadowOffset = NSSize(width: 0, height: -1)
            shadow.set()
            NSColor.controlBackgroundColor.setFill()
            path.fill()
            NSGraphicsContext.restoreGraphicsState()
        } else {
            background.setFill()
            path.fill()
        }

        var x = body.minX + 8

        // Favicon, or a spinner-ish dot while loading and nothing has arrived yet.
        let iconSize: CGFloat = 16
        let iconRect = CGRect(x: x, y: body.midY - iconSize / 2, width: iconSize, height: iconSize)
        if let favicon {
            favicon.draw(in: iconRect)
        } else if isLoading {
            NSColor.secondaryLabelColor.withAlphaComponent(0.6).setFill()
            NSBezierPath(ovalIn: iconRect.insetBy(dx: 5, dy: 5)).fill()
        } else {
            NSColor.tertiaryLabelColor.setFill()
            NSBezierPath(roundedRect: iconRect.insetBy(dx: 3, dy: 3), xRadius: 2, yRadius: 2).fill()
        }
        x += iconSize + 6

        guard !isPinned else { return }

        // Leave room for the close button so a long title never runs under it.
        let textRight = body.maxX - 24
        guard textRight > x else { return }

        let attributes: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: 12, weight: isActive ? .medium : .regular),
            .foregroundColor: isActive ? NSColor.labelColor : NSColor.secondaryLabelColor,
        ]
        let text = title.isEmpty ? "Untitled" : title
        let paragraph = NSMutableParagraphStyle()
        paragraph.lineBreakMode = .byTruncatingTail
        var withParagraph = attributes
        withParagraph[.paragraphStyle] = paragraph

        let textRect = CGRect(x: x, y: body.midY - 8, width: textRight - x, height: 16)
        (text as NSString).draw(in: textRect, withAttributes: withParagraph)

        // The close button appears on hover or on the active tab, as in Safari.
        if hovered || isActive {
            let rect = closeRect
            if closeHovered {
                NSColor.secondaryLabelColor.withAlphaComponent(0.25).setFill()
                NSBezierPath(ovalIn: rect.insetBy(dx: -2, dy: -2)).fill()
            }
            let cross = NSBezierPath()
            let inset: CGFloat = 4
            cross.move(to: CGPoint(x: rect.minX + inset, y: rect.minY + inset))
            cross.line(to: CGPoint(x: rect.maxX - inset, y: rect.maxY - inset))
            cross.move(to: CGPoint(x: rect.maxX - inset, y: rect.minY + inset))
            cross.line(to: CGPoint(x: rect.minX + inset, y: rect.maxY - inset))
            cross.lineWidth = 1.5
            cross.lineCapStyle = .round
            NSColor.secondaryLabelColor.setStroke()
            cross.stroke()
        }
    }
}

/// The tab strip: chips in a horizontal scroller, with a `+` pinned outside it.
///
/// The `+` deliberately does not scroll with the chips. The GTK shell shipped it inside the
/// scroller and past about nine tabs it slid off the right edge, leaving no pointer-only way
/// to open a tab at all.
final class TabStripView: NSView {
    private let browser: Browser
    private let scroller = NSScrollView()
    private let content = NSView()
    private let newTabButton = NSButton()
    private let privateLabel = NSTextField(labelWithString: "Private")
    private var chips: [BeaconTabId: TabChip] = [:]

    /// Which tabs this window shows, in strip order. A window owns a subset of the
    /// browser's tabs; the browser itself has no concept of windows.
    var tabs: [BeaconTabId] = []
    var activeTab: BeaconTabId = 0

    var onSelect: ((BeaconTabId) -> Void)?
    var onClose: ((BeaconTabId) -> Void)?
    var onNewTab: (() -> Void)?
    var onContextMenu: ((BeaconTabId, NSEvent) -> Void)?
    /// The strip reordered itself; the window should re-read the browser's tab order.
    var onReordered: (() -> Void)?

    /// The chip under the pointer during a drag, and where inside it the grab happened.
    private var draggingChip: TabChip?
    private var dragGrabOffset: CGFloat = 0

    static let height: CGFloat = 30

    init(browser: Browser) {
        self.browser = browser
        super.init(frame: .zero)

        scroller.hasHorizontalScroller = false
        scroller.hasVerticalScroller = false
        scroller.drawsBackground = false
        scroller.documentView = content
        // Framed by hand in layoutChips, so it keeps the autoresizing path rather than
        // waiting for constraints that are never added.
        content.translatesAutoresizingMaskIntoConstraints = true

        newTabButton.title = "+"
        newTabButton.bezelStyle = .texturedRounded
        newTabButton.isBordered = false
        newTabButton.font = .systemFont(ofSize: 15)
        newTabButton.target = self
        newTabButton.action = #selector(newTabClicked)
        newTabButton.toolTip = "New Tab (⌘T)"

        // A private window says so in the chrome as well as looking different. Colour alone
        // is not a label, and someone glancing at a screen should not have to know the
        // convention to read it.
        privateLabel.font = .systemFont(ofSize: 10, weight: .semibold)
        privateLabel.textColor = .secondaryLabelColor
        privateLabel.isHidden = !browser.isPrivate

        for view in [scroller, newTabButton, privateLabel] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            addSubview(view)
        }

        NSLayoutConstraint.activate([
            privateLabel.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 8),
            privateLabel.centerYAnchor.constraint(equalTo: centerYAnchor),

            scroller.leadingAnchor.constraint(
                equalTo: browser.isPrivate ? privateLabel.trailingAnchor : leadingAnchor,
                constant: 6
            ),
            scroller.topAnchor.constraint(equalTo: topAnchor),
            scroller.bottomAnchor.constraint(equalTo: bottomAnchor),
            scroller.trailingAnchor.constraint(equalTo: newTabButton.leadingAnchor, constant: -2),

            newTabButton.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -8),
            newTabButton.centerYAnchor.constraint(equalTo: centerYAnchor),
            newTabButton.widthAnchor.constraint(equalToConstant: 26),
            newTabButton.heightAnchor.constraint(equalToConstant: 22),
        ])
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    override var isFlipped: Bool { true }

    @objc private func newTabClicked() { onNewTab?() }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.windowBackgroundColor.setFill()
        dirtyRect.fill()
        // A hairline under the strip, separating chrome from page.
        NSColor.separatorColor.setStroke()
        let line = NSBezierPath()
        line.move(to: CGPoint(x: bounds.minX, y: bounds.maxY - 0.5))
        line.line(to: CGPoint(x: bounds.maxX, y: bounds.maxY - 0.5))
        line.lineWidth = 1
        line.stroke()
    }

    /// Rebuild from what the browser currently says. Chips are reused across rebuilds so a
    /// hover or a half-finished drag survives a title change.
    func refresh() {
        var seen = Set<BeaconTabId>()
        for tab in tabs {
            seen.insert(tab)
            let chip = chips[tab] ?? makeChip(tab)
            chip.title = browser.title(of: tab).isEmpty ? browser.url(of: tab) : browser.title(of: tab)
            chip.favicon = browser.favicon(of: tab)
            chip.isActive = (tab == activeTab)
            chip.isLoading = browser.isLoading(tab)
            chip.isPinned = browser.isPinned(tab)
            chip.needsDisplay = true
        }
        for (tab, chip) in chips where !seen.contains(tab) {
            chip.removeFromSuperview()
            chips.removeValue(forKey: tab)
        }
        layoutChips()
    }

    private func makeChip(_ tab: BeaconTabId) -> TabChip {
        let chip = TabChip(tab: tab)
        chip.onSelect = { [weak self] in self?.onSelect?($0) }
        chip.onClose = { [weak self] in self?.onClose?($0) }
        chip.onContextMenu = { [weak self] tab, event in self?.onContextMenu?(tab, event) }
        chip.onDrag = { [weak self] chip, event in self?.dragChip(chip, event: event) }
        chip.onDragEnd = { [weak self] chip in self?.endDrag(chip) }
        content.addSubview(chip)
        chips[tab] = chip
        return chip
    }

    // ── dragging ──────────────────────────────────────────────────────────

    /// Follow the pointer, and commit each crossing as it happens.
    ///
    /// Reordering live rather than on drop is what makes the other chips slide out of the
    /// way under the cursor: the browser is the model, so moving the tab there *is* the
    /// animation, and the next layout puts everyone else in their new place.
    private func dragChip(_ chip: TabChip, event: NSEvent) {
        let point = content.convert(event.locationInWindow, from: nil)

        if draggingChip !== chip {
            draggingChip = chip
            dragGrabOffset = point.x - chip.frame.minX
            chip.isDragging = true
            // Above its neighbours, or it slides under the chip it is passing.
            content.addSubview(chip, positioned: .above, relativeTo: nil)
        }

        var frame = chip.frame
        frame.origin.x = min(max(point.x - dragGrabOffset, 0), max(content.bounds.width - frame.width, 0))
        chip.frame = frame

        let target = dropIndex(forCentre: frame.midX, of: chip)
        if let current = tabs.firstIndex(of: chip.tab), current != target {
            browser.moveTab(chip.tab, to: target)
            onReordered?()
        }
    }

    /// Which strip position the dragged chip's centre now falls in.
    ///
    /// Measured against the *settled* chips, not the dragged one — comparing it with its own
    /// old slot is what makes a drag oscillate between two positions.
    private func dropIndex(forCentre centre: CGFloat, of chip: TabChip) -> Int {
        var index = 0
        var x: CGFloat = 0
        for tab in tabs {
            guard tab != chip.tab, let other = chips[tab] else { continue }
            if centre < x + other.frame.width / 2 {
                return index
            }
            x += other.frame.width
            index += 1
        }
        return index
    }

    private func endDrag(_ chip: TabChip) {
        chip.isDragging = false
        draggingChip = nil
        layoutChips()
    }

    /// Chips share the available width and shrink to a floor, then the strip scrolls. That
    /// floor is what keeps a twentieth tab reachable instead of two pixels wide.
    private func layoutChips() {
        let available = scroller.bounds.width
        let pinnedCount = tabs.filter { browser.isPinned($0) }.count
        let normalCount = max(tabs.count - pinnedCount, 0)
        let pinnedTotal = CGFloat(pinnedCount) * TabChip.pinnedWidth

        var width = TabChip.maxWidth
        if normalCount > 0 {
            width = (available - pinnedTotal) / CGFloat(normalCount)
            width = min(max(width, TabChip.minWidth), TabChip.maxWidth)
        }

        var x: CGFloat = 0
        for tab in tabs {
            guard let chip = chips[tab] else { continue }
            let chipWidth = browser.isPinned(tab) ? TabChip.pinnedWidth : width
            // The dragged chip follows the pointer; it still takes up its slot so the others
            // lay out around the gap it will drop into.
            if chip !== draggingChip {
                chip.frame = CGRect(x: x, y: 0, width: chipWidth, height: bounds.height)
            }
            x += chipWidth
        }
        content.frame = CGRect(x: 0, y: 0, width: max(x, available), height: bounds.height)

        // Keep the active tab on screen — activating an off-screen tab and seeing no
        // highlight anywhere is how the GTK strip looked broken. Not while dragging, or the
        // strip scrolls out from under the pointer.
        if draggingChip == nil, let chip = chips[activeTab] {
            content.scrollToVisible(chip.frame)
        }
    }

    override func layout() {
        super.layout()
        layoutChips()
    }
}
