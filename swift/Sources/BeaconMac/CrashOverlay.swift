import AppKit

/// What a tab shows when its engine worker has died.
///
/// Drawn by the shell rather than pushed into the tab as HTML, which is what a failed
/// *navigation* gets: there is no worker left to render a page with. The tab keeps
/// everything else — its place in the strip, its title, its address — because what died is
/// the process behind one tab, not the browser.
final class CrashOverlay: NSView {
    /// Called when the reader asks for the tab to be started again.
    var onReload: (() -> Void)?

    /// Why it died, as the engine reported it. nil while the tab is healthy.
    var reason: String? {
        didSet {
            guard reason != oldValue else { return }
            detail.stringValue = reason ?? ""
            detail.isHidden = (reason ?? "").isEmpty
        }
    }

    private let title = NSTextField(labelWithString: "This tab has stopped")
    private let detail = NSTextField(labelWithString: "")

    init() {
        super.init(frame: .zero)
        build()
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    private func build() {
        wantsLayer = true
        clipsToBounds = true

        let icon = NSImageView()
        icon.image = NSImage(systemSymbolName: "exclamationmark.triangle", accessibilityDescription: nil)
        icon.symbolConfiguration = NSImage.SymbolConfiguration(pointSize: 30, weight: .regular)
        icon.contentTintColor = .secondaryLabelColor

        title.font = .systemFont(ofSize: 17, weight: .semibold)
        title.alignment = .center

        detail.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
        detail.textColor = .secondaryLabelColor
        detail.alignment = .center
        detail.lineBreakMode = .byWordWrapping
        detail.maximumNumberOfLines = 4

        let hint = NSTextField(
            labelWithString: "Reloading gives it a new engine worker and loads the page again."
        )
        hint.font = .systemFont(ofSize: 11)
        hint.textColor = .tertiaryLabelColor
        hint.alignment = .center

        let button = NSButton(title: "Reload", target: self, action: #selector(reloadClicked))
        button.bezelStyle = .rounded
        button.keyEquivalent = "\r"

        let stack = NSStackView(views: [icon, title, detail, hint, button])
        stack.orientation = .vertical
        stack.alignment = .centerX
        stack.spacing = 10
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)

        NSLayoutConstraint.activate([
            stack.centerXAnchor.constraint(equalTo: centerXAnchor),
            stack.centerYAnchor.constraint(equalTo: centerYAnchor),
            stack.widthAnchor.constraint(lessThanOrEqualTo: widthAnchor, multiplier: 0.8),
        ])
    }

    override func draw(_ dirtyRect: NSRect) {
        // Opaque: the last frame the dead worker drew is still in the view underneath, and
        // a half-visible page behind an error reads as a rendering fault rather than a
        // stopped tab.
        NSColor.windowBackgroundColor.setFill()
        // Clipped to the view: a dirty rect can be larger than the view it belongs to.
        dirtyRect.intersection(bounds).fill()
    }

    /// The overlay swallows clicks meant for a page that is no longer being drawn.
    override func hitTest(_ point: NSPoint) -> NSView? {
        isHidden ? nil : super.hitTest(point)
    }

    @objc private func reloadClicked() {
        onReload?()
    }
}
