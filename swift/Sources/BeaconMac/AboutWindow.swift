import AppKit

/// The About window: the branded artwork, with a version block and a credits page.
///
/// A port of the GTK shell's dialog rather than a re-imagining — same artwork, same two
/// pages, same crossfade — because it is the app's face and the two shells should not
/// disagree about it. What changes is the framing: this is a Mac panel opened from the
/// application menu, not an F1 dialog.
///
/// The artwork is a fixed light image, so the text drawn over it uses explicit colours
/// rather than semantic ones. A label that turned white in dark mode would vanish into the
/// picture's pale left half.
final class AboutWindowController: NSWindowController {
    /// The artwork is 1403x861; this keeps that ratio so nothing is cropped or stretched.
    private static let artWidth: CGFloat = 660
    // 16:9, the aspect of the artwork in Resources/. Both pages share it, so the window
    // never changes size when you flip between them.
    private static let artHeight: CGFloat = 371
    private static let barHeight: CGFloat = 44

    /// White on a photograph needs help where the lighthouse beam crosses behind it.
    private static let legibilityShadow: NSShadow = {
        let shadow = NSShadow()
        shadow.shadowColor = NSColor.black.withAlphaComponent(0.85)
        shadow.shadowBlurRadius = 3
        shadow.shadowOffset = NSSize(width: 0, height: -1)
        return shadow
    }()

    private static let credits: [(String, [String])] = [
        ("Gosub Beacon", ["Gosub Team", "Joshua Thijssen", "SharkTheOne"]),
        ("Networking", ["Gosub Team"]),
        ("HTML5 parser", ["Gosub Team"]),
        ("CSS3 parser", ["Gosub Team"]),
        ("Renderer", ["Gosub Team"]),
        ("Javascript engine", ["Gosub Team"]),
        ("UI", ["Gosub Team"]),
        ("AppKit integration", ["Gosub Team"]),
        ("Rust integration", ["Gosub Team"]),
        ("Translations", ["Gosub Team"]),
    ]

    /// Ink that reads on the artwork's pale left half, in either system appearance.
    private static let ink = NSColor(calibratedRed: 0.08, green: 0.13, blue: 0.29, alpha: 1)
    private static let inkMuted = NSColor(calibratedRed: 0.08, green: 0.13, blue: 0.29, alpha: 0.75)

    private var artPage: NSView!
    private var creditsPage: NSView!
    private let toggleButton = NSButton()
    private var showingCredits = false

    init() {
        let size = NSSize(width: Self.artWidth, height: Self.artHeight + Self.barHeight)
        let window = NSWindow(
            contentRect: NSRect(origin: .zero, size: size),
            // No .resizable: the artwork has one size and letterboxing it would look worse
            // than not offering the gesture.
            styleMask: [.titled, .closable],
            backing: .buffered,
            defer: false
        )
        super.init(window: window)

        window.title = "About Gosub Beacon"
        window.isReleasedWhenClosed = false
        build()
        window.center()
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    // ── layout ────────────────────────────────────────────────────────────

    private func build() {
        guard let window, let content = window.contentView else { return }

        artPage = buildArtPage()
        creditsPage = buildCreditsPage()
        creditsPage.alphaValue = 0

        toggleButton.title = "Credits"
        toggleButton.bezelStyle = .rounded
        toggleButton.target = self
        toggleButton.action = #selector(togglePage)

        let closeButton = NSButton(title: "Close", target: self, action: #selector(closeWindow))
        closeButton.bezelStyle = .rounded
        // Escape closes, which is what the GTK dialog binds and what a Mac panel should do
        // anyway. AppKit routes Escape to whichever button claims it as its key equivalent.
        closeButton.keyEquivalent = "\u{1b}"

        let info = NSTextField(
            labelWithString: "Gosub Beacon \(Bundle.main.shortVersion) · Powered by the Gosub Engine · © 2026 Gosub Project"
        )
        info.font = .systemFont(ofSize: 10)
        info.textColor = .secondaryLabelColor

        let link = NSButton(title: "https://gosub.io", target: self, action: #selector(openWebsite))
        link.isBordered = false
        link.attributedTitle = NSAttributedString(
            string: "https://gosub.io",
            attributes: [
                .font: NSFont.systemFont(ofSize: 10, weight: .medium),
                .foregroundColor: NSColor.linkColor,
                .underlineStyle: NSUnderlineStyle.single.rawValue,
            ]
        )

        let bar = NSStackView(views: [info, link])
        bar.orientation = .horizontal
        bar.spacing = 8

        for view in [artPage!, creditsPage!, toggleButton, closeButton, bar] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            content.addSubview(view)
        }

        var constraints: [NSLayoutConstraint] = [
            toggleButton.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 12),
            toggleButton.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -10),
            closeButton.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -12),
            closeButton.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -10),

            bar.centerXAnchor.constraint(equalTo: content.centerXAnchor),
            bar.centerYAnchor.constraint(equalTo: closeButton.centerYAnchor),
            bar.leadingAnchor.constraint(greaterThanOrEqualTo: toggleButton.trailingAnchor, constant: 10),
            bar.trailingAnchor.constraint(lessThanOrEqualTo: closeButton.leadingAnchor, constant: -10),
        ]
        // Both pages occupy the same rectangle; only alpha tells them apart.
        for page in [artPage!, creditsPage!] {
            constraints += [
                page.topAnchor.constraint(equalTo: content.topAnchor),
                page.leadingAnchor.constraint(equalTo: content.leadingAnchor),
                page.trailingAnchor.constraint(equalTo: content.trailingAnchor),
                page.widthAnchor.constraint(equalToConstant: Self.artWidth),
                page.heightAnchor.constraint(equalToConstant: Self.artHeight),
            ]
        }
        NSLayoutConstraint.activate(constraints)
    }

    /// Only the picture. These finals are a finished composition -- wordmark, tagline and
    /// submarine reach the bottom of the panel -- so the version block cannot sit on them
    /// without landing on the artwork; it lives in the button bar instead.
    private func buildArtPage() -> NSView {
        let page = NSView()
        let image = artwork(named: "about")
        page.addSubview(image)
        NSLayoutConstraint.activate([
            image.topAnchor.constraint(equalTo: page.topAnchor),
            image.leadingAnchor.constraint(equalTo: page.leadingAnchor),
            image.trailingAnchor.constraint(equalTo: page.trailingAnchor),
            image.bottomAnchor.constraint(equalTo: page.bottomAnchor),
        ])
        return page
    }

    /// The credits artwork keeps its whole left half clear; the scrolling column sits there.
    private func buildCreditsPage() -> NSView {
        let page = NSView()
        let image = artwork(named: "about-credits")
        page.addSubview(image)

        let list = NSStackView()
        list.orientation = .vertical
        list.alignment = .leading
        list.spacing = 2
        for (section, names) in Self.credits {
            let heading = NSTextField(labelWithString: section)
            heading.font = .systemFont(ofSize: 11, weight: .semibold)
            heading.textColor = .white
            heading.shadow = Self.legibilityShadow
            list.addArrangedSubview(heading)
            list.setCustomSpacing(4, after: heading)
            for name in names {
                let label = NSTextField(labelWithString: "    " + name)
                label.font = .systemFont(ofSize: 11)
                label.textColor = NSColor.white.withAlphaComponent(0.88)
                label.shadow = Self.legibilityShadow
                list.addArrangedSubview(label)
            }
            if let last = list.arrangedSubviews.last {
                list.setCustomSpacing(12, after: last)
            }
        }

        let scroller = NSScrollView()
        scroller.drawsBackground = false
        scroller.hasVerticalScroller = true
        scroller.hasHorizontalScroller = false
        scroller.autohidesScrollers = true
        // Overlay rather than the legacy track, whatever the system preference says: a solid
        // grey bar down the middle of a photograph reads as a mistake. Light knob, because
        // what is behind it is the night sky.
        scroller.scrollerStyle = .overlay
        scroller.scrollerKnobStyle = .light
        // A flipped container, or the stack lays out from the bottom and the scroller opens
        // on the *end* of the list with the first section clipped off the top.
        let document = FlippedView()
        document.translatesAutoresizingMaskIntoConstraints = false
        document.addSubview(list)
        scroller.documentView = document

        // The document view is sized by its own content; only its width is pinned, or the
        // stack lays out at zero and the column comes out empty.
        list.translatesAutoresizingMaskIntoConstraints = false
        scroller.translatesAutoresizingMaskIntoConstraints = false
        page.addSubview(scroller)

        NSLayoutConstraint.activate([
            image.topAnchor.constraint(equalTo: page.topAnchor),
            image.leadingAnchor.constraint(equalTo: page.leadingAnchor),
            image.trailingAnchor.constraint(equalTo: page.trailingAnchor),
            image.bottomAnchor.constraint(equalTo: page.bottomAnchor),

            // Spanning the right of the picture: the names sit over the open water clear of
            // the gradient, and the scrollbar lands at the edge of the artwork rather than
            // down the middle of it.
            scroller.leadingAnchor.constraint(equalTo: page.leadingAnchor, constant: Self.artWidth * 0.56),
            scroller.trailingAnchor.constraint(equalTo: page.trailingAnchor, constant: -20),
            scroller.topAnchor.constraint(equalTo: page.topAnchor, constant: 22),
            scroller.bottomAnchor.constraint(equalTo: page.bottomAnchor, constant: -22),

            document.leadingAnchor.constraint(equalTo: scroller.contentView.leadingAnchor),
            document.topAnchor.constraint(equalTo: scroller.contentView.topAnchor),
            document.widthAnchor.constraint(equalTo: scroller.widthAnchor, constant: -16),

            list.leadingAnchor.constraint(equalTo: document.leadingAnchor),
            list.trailingAnchor.constraint(equalTo: document.trailingAnchor),
            list.topAnchor.constraint(equalTo: document.topAnchor),
            list.bottomAnchor.constraint(equalTo: document.bottomAnchor),
        ])
        return page
    }

    /// Where the resources SwiftPM bundled actually are at runtime.
    ///
    /// Not `Bundle.module`. That accessor is generated with exactly two paths in it: beside
    /// the main bundle, and the absolute path of the build directory it was compiled in.
    /// A packaged .app keeps its resources in Contents/Resources, which is neither, so the
    /// only reason the app works on the machine that built it is the second path still
    /// existing there. Anywhere else `Bundle.module` finds nothing -- and it does not return
    /// nil, it traps. Opening About crashed the first DMG that left this machine.
    ///
    /// `Bundle.main` covers both shapes: Contents/Resources in a packaged app, and the
    /// directory the executable sits in when it is run straight out of .build.
    private static let resources: Bundle = {
        if let url = Bundle.main.url(forResource: "BeaconMac_BeaconMac", withExtension: "bundle"),
            let bundle = Bundle(url: url)
        {
            return bundle
        }
        // Whatever is next to the executable. A missing picture is then a missing picture,
        // logged below, rather than the dialog taking the app down with it.
        return Bundle.main
    }()

    /// Load a bundled PNG. A missing file yields an empty view rather than a crash, so a
    /// packaging mistake costs the artwork and not the dialog.
    private func artwork(named name: String) -> NSImageView {
        let view = NSImageView()
        view.imageScaling = .scaleProportionallyUpOrDown
        view.translatesAutoresizingMaskIntoConstraints = false
        // An image view reports the picture's own size as its intrinsic one, and this window
        // is not resizable, so AppKit sizes the window to fit the artwork: 1672 points wide
        // for a 660 point panel, with the art floating in the middle of it. The layout
        // decides how large the art is drawn, not the file.
        for axis in [NSLayoutConstraint.Orientation.horizontal, .vertical] {
            view.setContentCompressionResistancePriority(.defaultLow, for: axis)
            view.setContentHuggingPriority(.defaultLow, for: axis)
        }
        if let url = Self.resources.url(forResource: name, withExtension: "png"),
            let image = NSImage(contentsOf: url)
        {
            view.image = image
        } else {
            NSLog("beacon: About artwork '\(name).png' is not in the bundle")
        }
        return view
    }

    // ── actions ───────────────────────────────────────────────────────────

    @objc private func openWebsite() {
        if let url = URL(string: "https://gosub.io") {
            NSWorkspace.shared.open(url)
        }
    }

    @objc private func togglePage() {
        showingCredits.toggle()
        toggleButton.title = showingCredits ? "About" : "Credits"
        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.25
            artPage.animator().alphaValue = showingCredits ? 0 : 1
            creditsPage.animator().alphaValue = showingCredits ? 1 : 0
        }
    }

    @objc private func closeWindow() {
        window?.close()
    }

}

/// A view that measures from the top, which is what a scrolling column of text wants: the
/// AppKit default puts the origin at the bottom, so a stack inside a scroll view fills
/// upwards and opens scrolled to its end.
private final class FlippedView: NSView {
    override var isFlipped: Bool { true }
}

extension Bundle {
    /// The version to show. A `swift run` binary has no Info.plist, so this falls back
    /// rather than showing an empty string in the one place people look for a version.
    var shortVersion: String {
        (object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String) ?? "0.1.0"
    }
}
