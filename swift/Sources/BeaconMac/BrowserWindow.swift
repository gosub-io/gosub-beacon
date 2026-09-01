import AppKit
import CBeacon

/// The chrome: a window, a toolbar, a tab strip, and the loop that pumps Beacon's events.
///
/// Everything shown here is read back from the browser when it is drawn. The tab strip is
/// rebuilt from `browser.tabs` rather than from a list kept alongside it, and the address
/// field is filled from `browser.url(of:)`. That is not ceremony — it is what stops this
/// shell slowly becoming a second, disagreeing browser.
final class BrowserWindow: NSObject, NSWindowDelegate, NSTextFieldDelegate {
    private let browser: Browser
    private let window: NSWindow
    private let pageView: PageView

    private let backButton = NSButton(title: "◀", target: nil, action: nil)
    private let forwardButton = NSButton(title: "▶", target: nil, action: nil)
    private let reloadButton = NSButton(title: "⟳", target: nil, action: nil)
    private let addressField = NSTextField()
    private let statusLabel = NSTextField(labelWithString: "")
    private let tabStrip = NSStackView()

    private var pump: Timer?
    /// True while the user is editing the address, so engine updates do not fight the caret.
    private var editingAddress = false

    init(browser: Browser, startURL: String) {
        self.browser = browser
        self.pageView = PageView(browser: browser)
        window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1100, height: 800),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        super.init()

        window.title = "Gosub Beacon"
        window.delegate = self
        buildChrome()

        let tab = browser.openTab(startURL)
        guard tab != 0 else {
            NSLog("beacon: could not open \(startURL)")
            return
        }
        browser.activateTab(tab)
        pageView.show(tab: tab)
        addressField.stringValue = startURL

        window.center()
        window.makeKeyAndOrderFront(nil)
        startPump()
        refreshChrome()
    }

    // ── layout ────────────────────────────────────────────────────────────

    private func buildChrome() {
        let content = NSView()
        window.contentView = content

        for (button, action) in [
            (backButton, #selector(goBack)), (forwardButton, #selector(goForward)), (reloadButton, #selector(reloadPage)),
        ] {
            button.target = self
            button.action = action
            button.bezelStyle = .rounded
            button.setButtonType(.momentaryPushIn)
        }

        addressField.delegate = self
        addressField.target = self
        addressField.action = #selector(addressEntered)
        addressField.placeholderString = "Search or enter address"
        addressField.font = .systemFont(ofSize: 13)

        let toolbar = NSStackView(views: [backButton, forwardButton, reloadButton, addressField])
        toolbar.orientation = .horizontal
        toolbar.spacing = 6
        toolbar.edgeInsets = NSEdgeInsets(top: 8, left: 10, bottom: 8, right: 10)
        toolbar.setHuggingPriority(.defaultLow, for: .horizontal)
        addressField.setContentHuggingPriority(.defaultLow, for: .horizontal)

        tabStrip.orientation = .horizontal
        tabStrip.spacing = 4
        tabStrip.edgeInsets = NSEdgeInsets(top: 4, left: 10, bottom: 0, right: 10)
        tabStrip.alignment = .centerY

        statusLabel.font = .systemFont(ofSize: 11)
        statusLabel.textColor = .secondaryLabelColor
        statusLabel.isHidden = true

        let stack = NSStackView(views: [tabStrip, toolbar, pageView, statusLabel])
        stack.orientation = .vertical
        stack.spacing = 0
        stack.alignment = .leading
        stack.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(stack)

        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: content.topAnchor),
            stack.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            stack.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            toolbar.widthAnchor.constraint(equalTo: stack.widthAnchor),
            tabStrip.widthAnchor.constraint(equalTo: stack.widthAnchor),
            pageView.widthAnchor.constraint(equalTo: stack.widthAnchor),
        ])
        // The page takes whatever the chrome does not.
        pageView.setContentHuggingPriority(.defaultLow, for: .vertical)
        pageView.setContentCompressionResistancePriority(.defaultLow, for: .vertical)
    }

    // ── the pump ──────────────────────────────────────────────────────────

    /// Beacon's events are pulled, not pushed, so this is where they arrive. A timer on the
    /// main run loop keeps every ABI call on the thread AppKit requires.
    private func startPump() {
        let timer = Timer(timeInterval: 1.0 / 60.0, repeats: true) { [weak self] _ in
            self?.tick()
        }
        RunLoop.main.add(timer, forMode: .common)
        pump = timer
    }

    private func tick() {
        var needsChromeRefresh = false
        for event in browser.pollEvents() {
            switch event.kind {
            case BEACON_REDRAW:
                pageView.redraw()
            case BEACON_URL_CHANGED:
                if event.tab == browser.activeTab, !editingAddress, let text = event.text {
                    addressField.stringValue = text
                }
                needsChromeRefresh = true
            case BEACON_HOVER_URL:
                let text = event.text ?? ""
                statusLabel.stringValue = text
                statusLabel.isHidden = text.isEmpty
            case BEACON_TITLE_CHANGED:
                if event.tab == browser.activeTab, let text = event.text {
                    window.title = "\(text) — Gosub Beacon"
                }
                needsChromeRefresh = true
            case BEACON_TABS_CHANGED, BEACON_NAV_STATE_CHANGED, BEACON_LOADING_CHANGED,
                 BEACON_ACTIVE_TAB_CHANGED, BEACON_FAVICON_CHANGED:
                needsChromeRefresh = true
            case BEACON_TAB_CRASHED:
                NSLog("beacon: tab crashed: \(event.text ?? "")")
                needsChromeRefresh = true
            case BEACON_LOG:
                NSLog("beacon: \(event.text ?? "")")
            default:
                break
            }
        }
        if needsChromeRefresh { refreshChrome() }
    }

    /// Rebuild everything from what the browser currently says.
    private func refreshChrome() {
        let active = browser.activeTab
        backButton.isEnabled = browser.canGoBack(active)
        forwardButton.isEnabled = browser.canGoForward(active)
        reloadButton.title = browser.isLoading(active) ? "✕" : "⟳"

        tabStrip.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for tab in browser.tabs {
            let title = browser.title(of: tab)
            let label = title.isEmpty ? browser.url(of: tab) : title
            let button = NSButton(title: String(label.prefix(28)), target: self, action: #selector(tabClicked(_:)))
            button.tag = Int(tab)
            button.bezelStyle = .recessed
            button.setButtonType(.pushOnPushOff)
            button.state = (tab == active) ? .on : .off
            tabStrip.addArrangedSubview(button)
        }
        let newTab = NSButton(title: "+", target: self, action: #selector(newTabClicked))
        newTab.bezelStyle = .recessed
        tabStrip.addArrangedSubview(newTab)
    }

    // ── actions ───────────────────────────────────────────────────────────

    @objc private func goBack() { browser.back() }
    @objc private func goForward() { browser.forward() }

    @objc private func reloadPage() {
        if browser.isLoading(browser.activeTab) { browser.stop() } else { browser.reload() }
    }

    @objc private func addressEntered() {
        let text = addressField.stringValue.trimmingCharacters(in: .whitespaces)
        guard !text.isEmpty else { return }
        browser.navigate(browser.activeTab, to: text)
        window.makeFirstResponder(pageView)
    }

    @objc private func tabClicked(_ sender: NSButton) {
        let tab = BeaconTabId(sender.tag)
        browser.activateTab(tab)
        pageView.show(tab: tab)
        addressField.stringValue = browser.url(of: tab)
        refreshChrome()
    }

    @objc private func newTabClicked() {
        let tab = browser.openTab("gosub://home")
        guard tab != 0 else { return }
        browser.activateTab(tab)
        pageView.show(tab: tab)
        refreshChrome()
    }

    func controlTextDidBeginEditing(_ obj: Notification) { editingAddress = true }
    func controlTextDidEndEditing(_ obj: Notification) { editingAddress = false }

    func windowWillClose(_ notification: Notification) {
        pump?.invalidate()
        NSApp.terminate(nil)
    }
}
