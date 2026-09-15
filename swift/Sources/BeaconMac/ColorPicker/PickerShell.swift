import AppKit

/// The window every picker shares: a sidebar with the gosub mark, the sections and the
/// lighthouse; a title; a main card; an optional side card; Cancel and OK. Subclasses fill
/// the cards and decide what OK means.
///
/// Two sizes of it exist, both from the designs: the *large* one (983 × 910, from the time
/// picker screens: a 275-wide sidebar with the tagline, 30 pt title, a help button) and the
/// *small* one (from the earlier time-picker screenshot: a 143-wide sidebar, 20 pt title),
/// which the colour picker uses at 960 × 600.
class PickerShellWindowController: NSWindowController, NSWindowDelegate {
    struct NavItem {
        let symbol: String
        let title: String
    }

    /// The shell's fixed measurements, for each of its two sizes.
    struct Metrics {
        let sidebarWidth: CGFloat
        let logoOrigin: NSPoint
        let logoScale: CGFloat
        let wordmarkOrigin: NSPoint
        let wordmarkSize: CGFloat
        let tagline: NSPoint?
        let navOrigin: NSPoint
        let navSize: NSSize
        let navPitch: CGFloat
        let navFontSize: CGFloat
        let navSymbolSize: CGFloat
        let titleOrigin: NSPoint
        let titleSize: CGFloat
        let buttonHeight: CGFloat
        let buttonFontSize: CGFloat
        let okWidth: CGFloat
        let cancelWidth: CGFloat
        let buttonBottomInset: CGFloat
        let helpButton: Bool
        let windowRadius: CGFloat

        static let small = Metrics(
            sidebarWidth: 143, logoOrigin: NSPoint(x: 17, y: 41), logoScale: 0.6, wordmarkOrigin: NSPoint(x: 51, y: 50), wordmarkSize: 20,
            tagline: nil, navOrigin: NSPoint(x: 11, y: 78), navSize: NSSize(width: 124, height: 35), navPitch: 40, navFontSize: 15,
            navSymbolSize: 15, titleOrigin: NSPoint(x: 163, y: 14), titleSize: 20, buttonHeight: 35, buttonFontSize: 15, okWidth: 102,
            cancelWidth: 100, buttonBottomInset: 46, helpButton: false, windowRadius: 12
        )
        static let large = Metrics(
            sidebarWidth: 275, logoOrigin: NSPoint(x: 34, y: 70), logoScale: 0.95, wordmarkOrigin: NSPoint(x: 92, y: 85), wordmarkSize: 30,
            tagline: NSPoint(x: 93, y: 113), navOrigin: NSPoint(x: 17, y: 167), navSize: NSSize(width: 244, height: 58), navPitch: 68,
            navFontSize: 20, navSymbolSize: 24, titleOrigin: NSPoint(x: 301, y: 42), titleSize: 30, buttonHeight: 50, buttonFontSize: 18,
            okWidth: 150, cancelWidth: 141, buttonBottomInset: 69, helpButton: true, windowRadius: 12
        )
    }

    let metrics: Metrics
    let root = FlippedRoot()

    /// How large the large shell is drawn relative to its 983 × 910 design: 0.7 (688 × 637)
    /// unless `BeaconPickerScale` in the defaults says otherwise (0.5 to 1). The layout is
    /// the design's either way: the whole window is scaled, not re-laid-out.
    static var largeScale: CGFloat {
        let stored = UserDefaults.standard.double(forKey: "BeaconPickerScale")
        return stored > 0 ? CGFloat(min(max(stored, 0.5), 1)) : 0.7
    }
    let mainCard = Panel(fill: PickerStyle.shellCard, border: PickerStyle.shellCardBorder, radius: 12)
    let sideCard = Panel(fill: PickerStyle.shellCard, border: PickerStyle.shellCardBorder, radius: 10)
    let okButton = DesignButton(title: "OK", target: nil, action: nil)
    let cancelButton = DesignButton(title: "Cancel", target: nil, action: nil)
    private let titleLabel = PickerStyle.label("", size: 20, weight: .bold, color: PickerStyle.brand)
    private(set) var navButtons: [ShellNavButton] = []
    /// A sidebar section was chosen.
    var onNav: (() -> Void)?
    private(set) var selectedNav = 0
    private(set) var finished = false
    /// Where the help button leads, when the shell has one.
    var helpURL: URL?

    init(size: NSSize, title: String, nav: [NavItem], mainCard mainFrame: NSRect, sideCard sideFrame: NSRect?, metrics: Metrics = .small, packed: Bool = false) {
        self.metrics = metrics
        let window = NSWindow(
            contentRect: NSRect(origin: .zero, size: size),
            styleMask: [.titled, .closable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        window.title = title
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.isMovableByWindowBackground = true
        window.level = .floating
        window.isReleasedWhenClosed = false
        window.backgroundColor = .clear
        window.isOpaque = false
        super.init(window: window)
        window.delegate = self

        root.frame = NSRect(origin: .zero, size: size)
        window.contentView = root
        let background = Panel(fill: PickerStyle.shellBackground, border: PickerStyle.windowBorder, radius: metrics.windowRadius)
        background.frame = root.bounds
        root.addSubview(background)

        let sidebar = ShellSidebar(metrics: metrics, frame: NSRect(x: 0, y: 0, width: metrics.sidebarWidth, height: size.height))
        sidebar.isHidden = packed
        root.addSubview(sidebar)
        for (i, item) in nav.enumerated() {
            let button = ShellNavButton(symbol: item.symbol, title: item.title, fontSize: metrics.navFontSize, symbolSize: metrics.navSymbolSize)
            button.tag = i
            button.target = self
            button.action = #selector(navPressed(_:))
            button.frame = NSRect(x: metrics.navOrigin.x, y: metrics.navOrigin.y + CGFloat(i) * metrics.navPitch, width: metrics.navSize.width, height: metrics.navSize.height)
            sidebar.addSubview(button)
            navButtons.append(button)
        }
        selectNav(0)

        titleLabel.stringValue = title
        titleLabel.font = .systemFont(ofSize: metrics.titleSize, weight: .bold)
        titleLabel.frame = NSRect(x: metrics.titleOrigin.x, y: metrics.titleOrigin.y, width: size.width - metrics.titleOrigin.x - 160, height: metrics.titleSize + 10)
        titleLabel.isHidden = packed
        root.addSubview(titleLabel)

        mainCard.frame = mainFrame
        if packed {
            mainCard.fill = .clear
            mainCard.border = nil
        }
        root.addSubview(mainCard)
        if let sideFrame {
            sideCard.frame = sideFrame
            root.addSubview(sideCard)
        }

        if metrics.helpButton, !packed {
            let help = DesignButton(title: "?", target: self, action: #selector(helpPressed))
            help.fill = PickerStyle.fieldBackground
            help.border = PickerStyle.dynamic(PickerStyle.hex(0xC7CFDB), PickerStyle.hex(0x3A414C))
            help.radius = 17
            help.titleSize = 16
            help.titleWeight = .semibold
            help.frame = NSRect(x: metrics.sidebarWidth + 5, y: size.height - 61, width: 34, height: 34)
            root.addSubview(help)
        }

        let buttonY = size.height - metrics.buttonBottomInset
        okButton.fill = PickerStyle.selectButton
        okButton.border = nil
        okButton.titleColor = .white
        okButton.titleSize = metrics.buttonFontSize
        okButton.radius = metrics.buttonHeight > 40 ? 10 : 8
        okButton.keyEquivalent = "\r"
        okButton.target = self
        okButton.action = #selector(okPressed)
        okButton.frame = NSRect(x: size.width - 18 - metrics.okWidth, y: buttonY, width: metrics.okWidth, height: metrics.buttonHeight)
        root.addSubview(okButton)

        cancelButton.fill = PickerStyle.cancelFace
        cancelButton.border = metrics.helpButton ? nil : PickerStyle.dynamic(PickerStyle.hex(0xD0D7E2), PickerStyle.hex(0x3A414C))
        cancelButton.titleSize = metrics.buttonFontSize
        cancelButton.radius = okButton.radius
        cancelButton.keyEquivalent = "\u{1b}"
        cancelButton.target = self
        cancelButton.action = #selector(cancelPressed)
        cancelButton.frame = NSRect(x: okButton.frame.minX - 15 - metrics.cancelWidth, y: buttonY, width: metrics.cancelWidth, height: metrics.buttonHeight)
        root.addSubview(cancelButton)
        if packed {
            okButton.titleSize = 13
            cancelButton.titleSize = 13
            okButton.frame = NSRect(x: size.width - 14 - 72, y: size.height - 44, width: 72, height: 30)
            cancelButton.frame = NSRect(x: size.width - 14 - 72 - 10 - 76, y: size.height - 44, width: 76, height: 30)
        }

        // The large design is most of a laptop screen: draw it smaller. Everything above and
        // whatever a subclass adds is in design coordinates; the root's bounds map them.
        if metrics.helpButton {
            let scale = Self.largeScale
            let scaled = NSSize(width: size.width * scale, height: size.height * scale)
            window.setContentSize(scaled)
            root.frame = NSRect(origin: .zero, size: scaled)
            root.bounds = NSRect(origin: .zero, size: size)
        }
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    func setTitle(_ title: String) {
        titleLabel.stringValue = title
    }

    func selectNav(_ index: Int) {
        selectedNav = index
        for button in navButtons {
            button.isSelected = button.tag == index
        }
    }

    @objc private func navPressed(_ sender: ShellNavButton) {
        selectNav(sender.tag)
        onNav?()
    }

    @objc private func helpPressed() {
        if let helpURL {
            NSWorkspace.shared.open(helpURL)
        }
    }

    /// Show the picker as a sheet of `parent`: modal to that window, so the page behind it
    /// cannot be clicked or closed until the picker is answered, and ⌘W dismisses the picker
    /// rather than the browser. Without a parent (a demo launch with no window yet) it is a
    /// floating window beside `anchor`, the control that asked.
    func present(near anchor: NSRect?, of parent: NSWindow?) {
        guard let window else { return }
        if let parent {
            window.level = .normal
            parent.beginSheet(window) { _ in }
            NSLog("beacon: picker sheet on \(parent.title), \(window.frame.size), visible \(window.isVisible)")
            return
        }
        let size = window.frame.size
        let screen = (parent?.screen ?? NSScreen.main)?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
        var origin: NSPoint
        if let anchor {
            origin = NSPoint(x: anchor.minX, y: anchor.minY - size.height - 8)
        } else {
            origin = NSPoint(x: screen.midX - size.width / 2, y: screen.midY - size.height / 2)
        }
        origin.x = min(max(origin.x, screen.minX), screen.maxX - size.width)
        origin.y = min(max(origin.y, screen.minY), screen.maxY - size.height)
        window.setFrameOrigin(origin)
        showWindow(nil)
        window.makeKeyAndOrderFront(nil)
        NSLog("beacon: picker window at \(window.frame), visible \(window.isVisible), key \(window.isKeyWindow)")
    }

    /// Forget the caller: a picker being replaced must not answer for the control that has
    /// moved on.
    func detach() {
        finished = true
    }

    /// A sheet is ended, not closed: ending it is what slides it away and re-enables the
    /// window under it.
    override func close() {
        if let window, let parent = window.sheetParent {
            parent.endSheet(window)
        } else {
            super.close()
        }
    }

    /// OK, Cancel, the red button or Escape, once. Subclasses report the result.
    func didFinish(ok: Bool) {}

    private func finishOnce(ok: Bool) {
        guard !finished else { return }
        finished = true
        didFinish(ok: ok)
    }

    @objc func okPressed() {
        finishOnce(ok: true)
        close()
    }

    @objc func cancelPressed() {
        finishOnce(ok: false)
        close()
    }

    func windowWillClose(_ notification: Notification) {
        finishOnce(ok: false)
    }
}

/// The sidebar: its tint, the submarine and wordmark (and tagline, in the large shell), the
/// lighthouse fading in at the foot.
private final class ShellSidebar: NSView {
    private let metrics: PickerShellWindowController.Metrics

    init(metrics: PickerShellWindowController.Metrics, frame: NSRect) {
        self.metrics = metrics
        super.init(frame: frame)
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    override var isFlipped: Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        let sidebar = PickerStyle.shellSidebar
        let r = metrics.windowRadius
        let path = NSBezierPath()
        path.move(to: NSPoint(x: bounds.maxX, y: 0))
        path.line(to: NSPoint(x: r, y: 0))
        path.appendArc(withCenter: NSPoint(x: r, y: r), radius: r, startAngle: 270, endAngle: 180, clockwise: true)
        path.line(to: NSPoint(x: 0, y: bounds.maxY - r))
        path.appendArc(withCenter: NSPoint(x: r, y: bounds.maxY - r), radius: r, startAngle: 180, endAngle: 90, clockwise: true)
        path.line(to: NSPoint(x: bounds.maxX, y: bounds.maxY))
        path.close()
        sidebar.setFill()
        path.fill()

        if let art = PickerStyle.lighthouse {
            let height = bounds.width * art.size.height / art.size.width
            let artRect = NSRect(x: 0, y: bounds.height - height, width: bounds.width, height: height)
            NSGraphicsContext.saveGraphicsState()
            path.addClip()
            art.draw(in: artRect, from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
            PickerStyle.dynamic(PickerStyle.hex(0xC8DAEE, alpha: 0.12), NSColor.black.withAlphaComponent(0.25)).setFill()
            artRect.fill()
            let clear = sidebar.withAlphaComponent(0)
            NSGradient(starting: sidebar, ending: clear)?.draw(in: NSRect(x: 0, y: artRect.minY - 10, width: bounds.width, height: height * 0.4), angle: 90)
            NSGradient(starting: clear, ending: sidebar)?.draw(in: NSRect(x: bounds.width * 0.7, y: artRect.minY, width: bounds.width * 0.3, height: height), angle: 0)
            NSGraphicsContext.restoreGraphicsState()
        }

        PickerStyle.drawSubmarine(at: metrics.logoOrigin, scale: metrics.logoScale, color: PickerStyle.brand, portholes: sidebar)
        let mark = NSAttributedString(string: "gosub", attributes: [
            .font: NSFont.systemFont(ofSize: metrics.wordmarkSize, weight: .bold), .foregroundColor: PickerStyle.brand,
        ])
        mark.draw(at: NSPoint(x: metrics.wordmarkOrigin.x, y: metrics.wordmarkOrigin.y - mark.size().height / 2))
        if let tagline = metrics.tagline {
            let text = NSAttributedString(string: "For a more open web", attributes: [
                .font: NSFont.systemFont(ofSize: 13, weight: .regular), .foregroundColor: PickerStyle.inkSecondary,
            ])
            text.draw(at: NSPoint(x: tagline.x, y: tagline.y - text.size().height / 2))
        }
    }
}

/// A sidebar section: a symbol and a title, in the accent tint when selected.
final class ShellNavButton: NSButton {
    var isSelected = false { didSet { needsDisplay = true } }
    private let symbol: String
    private let fontSize: CGFloat
    private let symbolSize: CGFloat

    init(symbol: String, title: String, fontSize: CGFloat, symbolSize: CGFloat) {
        self.symbol = symbol
        self.fontSize = fontSize
        self.symbolSize = symbolSize
        super.init(frame: .zero)
        self.title = title
        isBordered = false
        setButtonType(.momentaryChange)
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    override func draw(_ dirtyRect: NSRect) {
        if isSelected {
            PickerStyle.sidebarSelection.setFill()
            NSBezierPath(roundedRect: bounds, xRadius: bounds.height > 40 ? 14 : 8, yRadius: bounds.height > 40 ? 14 : 8).fill()
        }
        let tint: NSColor = isSelected ? PickerStyle.accent : PickerStyle.brand
        let iconX = bounds.height > 40 ? 34 : 18
        let textX = bounds.height > 40 ? 76 : 42
        if let image = NSImage(systemSymbolName: symbol, accessibilityDescription: title)?
            .withSymbolConfiguration(NSImage.SymbolConfiguration(pointSize: symbolSize, weight: .medium))
        {
            let tinted = NSImage(size: image.size, flipped: false) { rect in
                image.draw(in: rect)
                tint.set()
                rect.fill(using: .sourceAtop)
                return true
            }
            let size = tinted.size
            tinted.draw(in: NSRect(x: CGFloat(iconX) - size.width / 2, y: (bounds.height - size.height) / 2, width: size.width, height: size.height),
                        from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
        }
        let text = NSAttributedString(string: title, attributes: [
            .font: NSFont.systemFont(ofSize: fontSize, weight: .medium),
            .foregroundColor: isSelected ? PickerStyle.accent : PickerStyle.brand,
        ])
        text.draw(at: NSPoint(x: CGFloat(textX), y: (bounds.height - text.size().height) / 2))
    }
}
