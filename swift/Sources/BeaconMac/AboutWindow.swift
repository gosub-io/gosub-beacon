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
    private static let artHeight: CGFloat = 405
    private static let barHeight: CGFloat = 44

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

        for view in [artPage!, creditsPage!, toggleButton, closeButton] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            content.addSubview(view)
        }

        var constraints: [NSLayoutConstraint] = [
            toggleButton.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 12),
            toggleButton.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -10),
            closeButton.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -12),
            closeButton.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -10),
        ]
        // Both pages occupy the same rectangle; only alpha tells them apart.
        for page in [artPage!, creditsPage!] {
            constraints += [
                page.topAnchor.constraint(equalTo: content.topAnchor),
                page.leadingAnchor.constraint(equalTo: content.leadingAnchor),
                page.trailingAnchor.constraint(equalTo: content.trailingAnchor),
                page.heightAnchor.constraint(equalToConstant: Self.artHeight),
            ]
        }
        NSLayoutConstraint.activate(constraints)
    }

    /// The artwork already carries the logo, the wordmark and the tagline, and leaves the
    /// bottom-left deliberately empty. Only the version block is laid over it.
    private func buildArtPage() -> NSView {
        let page = NSView()
        let image = artwork(named: "about")
        page.addSubview(image)

        let lines = [
            "Gosub Beacon \(Bundle.main.shortVersion)",
            "Powered by the Gosub Engine",
            "Copyright © 2026 Gosub Project",
            "All rights reserved.",
        ]
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 2
        for line in lines {
            let label = NSTextField(labelWithString: line)
            label.font = .systemFont(ofSize: 11)
            label.textColor = Self.inkMuted
            stack.addArrangedSubview(label)
        }

        let link = NSButton(title: "https://gosub.io", target: self, action: #selector(openWebsite))
        link.isBordered = false
        // attributedTitle rather than contentTintColor: that tints template images, and a
        // borderless button's text goes on being the system's label colour.
        link.attributedTitle = NSAttributedString(
            string: "https://gosub.io",
            attributes: [
                .font: NSFont.systemFont(ofSize: 11, weight: .medium),
                .foregroundColor: Self.ink,
                .underlineStyle: NSUnderlineStyle.single.rawValue,
            ]
        )
        // Without this the button pads itself away from the labels above it.
        link.setContentHuggingPriority(.defaultHigh, for: .horizontal)
        stack.addArrangedSubview(link)

        stack.translatesAutoresizingMaskIntoConstraints = false
        page.addSubview(stack)
        NSLayoutConstraint.activate([
            image.topAnchor.constraint(equalTo: page.topAnchor),
            image.leadingAnchor.constraint(equalTo: page.leadingAnchor),
            image.trailingAnchor.constraint(equalTo: page.trailingAnchor),
            image.bottomAnchor.constraint(equalTo: page.bottomAnchor),
            // Matching the GTK dialog's margins, so the block lands in the same empty space.
            stack.leadingAnchor.constraint(equalTo: page.leadingAnchor, constant: 42),
            stack.bottomAnchor.constraint(equalTo: page.bottomAnchor, constant: -24),
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
            heading.textColor = Self.ink
            list.addArrangedSubview(heading)
            list.setCustomSpacing(4, after: heading)
            for name in names {
                let label = NSTextField(labelWithString: "    " + name)
                label.font = .systemFont(ofSize: 11)
                label.textColor = Self.inkMuted
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
        scroller.documentView = list

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

            // Confined to the artwork's clear left half.
            scroller.leadingAnchor.constraint(equalTo: page.leadingAnchor, constant: 28),
            scroller.topAnchor.constraint(equalTo: page.topAnchor, constant: 20),
            scroller.bottomAnchor.constraint(equalTo: page.bottomAnchor, constant: -20),
            scroller.widthAnchor.constraint(equalToConstant: Self.artWidth * 0.4),

            list.leadingAnchor.constraint(equalTo: scroller.contentView.leadingAnchor),
            list.topAnchor.constraint(equalTo: scroller.contentView.topAnchor),
            list.widthAnchor.constraint(equalTo: scroller.widthAnchor, constant: -16),
        ])
        return page
    }

    /// Load a bundled PNG. `Bundle.module` is generated by SwiftPM for the resources
    /// declared in Package.swift; a missing file yields an empty view rather than a crash,
    /// so a packaging mistake costs the artwork and not the dialog.
    private func artwork(named name: String) -> NSImageView {
        let view = NSImageView()
        view.imageScaling = .scaleProportionallyUpOrDown
        view.translatesAutoresizingMaskIntoConstraints = false
        if let url = Bundle.module.url(forResource: name, withExtension: "png"),
            let image = NSImage(contentsOf: url)
        {
            view.image = image
        } else {
            NSLog("beacon: About artwork '\(name).png' is not in the bundle")
        }
        return view
    }

    // ── actions ───────────────────────────────────────────────────────────

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

    @objc private func openWebsite() {
        if let url = URL(string: "https://gosub.io") {
            NSWorkspace.shared.open(url)
        }
    }
}

extension Bundle {
    /// The version to show. A `swift run` binary has no Info.plist, so this falls back
    /// rather than showing an empty string in the one place people look for a version.
    var shortVersion: String {
        (object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String) ?? "0.1.0"
    }
}
