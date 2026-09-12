import AppKit
import CBeacon

/// A browser window: toolbar, tab strip, page, and the loop that pumps Beacon's events.
///
/// Everything shown here is read back from the browser when it is drawn. The tab strip is
/// rebuilt from the browser's own answers rather than from a list kept alongside them, and
/// the address field is filled from `browser.url(of:)`. That is not ceremony — it is what
/// stops this shell slowly becoming a second, disagreeing browser.
///
/// The one thing the window does own is *which* tabs it shows. The browser has no concept
/// of windows, so tab-to-window membership can only live here.
final class BrowserWindowController: NSWindowController, NSWindowDelegate, NSToolbarDelegate,
    NSTextFieldDelegate, NSMenuItemValidation, NSMenuDelegate
{
    let browser: Browser
    private let pageView: PageView
    private let tabStrip: TabStripView
    private let devPanel: DeveloperPanel
    private var devPanelHeight: NSLayoutConstraint?
    private let bookmarksBar = NSStackView()
    private let addressField = AddressField()
    private let progressLine = ProgressLine()
    /// Shows the hovered link over the bottom-left of the page. It lives in the window's
    /// content view, not in the page view: wgpu owns that view's layer.
    private let hoverLabel = NSTextField(labelWithString: "")

    private var navButtons = NSSegmentedControl()
    private var reloadButton = NSButton()
    private var bookmarkButton = NSButton()
    private var downloadsButton = NSButton()

    /// The tabs this window shows, in the order it shows them.
    private var ownedTabs: [BeaconTabId] = []
    private var currentTab: BeaconTabId = 0

    /// Lent to the address field so completion replaces the whole address rather than the
    /// last word. One instance, reused: AppKit expects a window to hand back the same
    /// editor each time it asks.
    private lazy var addressEditor: AddressFieldEditor = {
        let editor = AddressFieldEditor()
        editor.isFieldEditor = true
        return editor
    }()

    /// True while the last edit was a deletion, so backspacing does not fight a suggestion
    /// popup that reappears on every keystroke.
    private var isDeleting = false

    private var pump: Timer?
    /// True while the user is editing the address, so engine updates do not fight the caret.
    private var editingAddress = false
    private var bookmarksBarVisible = false

    private var bookmarksBarHeight: NSLayoutConstraint?

    /// Shown over the page when the tab's engine worker has died. AppKit chrome rather than
    /// a page pushed into the tab, because a crashed tab has no worker left to draw one.
    private let crashOverlay = CrashOverlay()

    /// Hit tests this window has asked for and what it means to do with the answer. Keyed
    /// by the token the ABI handed back, because a second right-click can land before the
    /// first answer does and a menu built from the wrong one is worse than a slow menu.
    private var pendingHits: [UInt64: HitIntent] = [:]

    /// What to do with a hit-test answer once it arrives.
    private enum HitIntent {
        /// Show the page context menu at this event's location.
        case contextMenu(NSEvent)
        /// Open the link there in another tab; the flag is whether to go to it.
        case openInNewTab(foreground: Bool)
    }

    // ── construction ──────────────────────────────────────────────────────

    init(browser: Browser, startURL: String?) {
        self.browser = browser
        self.pageView = PageView(browser: browser)
        self.tabStrip = TabStripView(browser: browser)
        self.devPanel = DeveloperPanel(browser: browser)

        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1200, height: 820),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        super.init(window: window)

        window.title = browser.isPrivate ? "Private Browsing" : "Gosub Beacon"
        window.delegate = self
        if browser.isPrivate {
            // Dark chrome for a private window, which is the signal Chrome and Firefox both
            // use and the one people recognise without reading anything. Forced rather than
            // following the system: the whole point is that it differs from a normal window.
            window.appearance = NSAppearance(named: .darkAqua)
            // Its own autosave name, or a private window inherits the normal one's frame and
            // then overwrites it on close.
            window.setFrameAutosaveName("BeaconPrivateWindow")
        }
        window.tabbingMode = .disallowed // this browser draws its own tabs
        if !browser.isPrivate {
            window.setFrameAutosaveName("BeaconBrowserWindow")
        }

        buildToolbar()
        buildContent()
        wireTabStrip()

        if let startURL {
            openTab(startURL, activate: true)
        }

        window.center()
        window.makeKeyAndOrderFront(nil)
        startPump()
        refreshChrome()

    }

    required init?(coder: NSCoder) { fatalError("not used") }

    deinit { pump?.invalidate() }

    // ── toolbar ───────────────────────────────────────────────────────────

    private enum ToolbarID {
        static let navigation = NSToolbarItem.Identifier("beacon.navigation")
        static let reload = NSToolbarItem.Identifier("beacon.reload")
        static let address = NSToolbarItem.Identifier("beacon.address")
        static let bookmark = NSToolbarItem.Identifier("beacon.bookmark")
        static let downloads = NSToolbarItem.Identifier("beacon.downloads")
        static let newTab = NSToolbarItem.Identifier("beacon.newTab")
    }

    private func buildToolbar() {
        navButtons = NSSegmentedControl(
            images: [
                NSImage(systemSymbolName: "chevron.left", accessibilityDescription: "Back") ?? NSImage(),
                NSImage(systemSymbolName: "chevron.right", accessibilityDescription: "Forward") ?? NSImage(),
            ],
            trackingMode: .momentary,
            target: self,
            action: #selector(navSegmentClicked)
        )
        navButtons.segmentStyle = .separated
        // Press and hold on Forward offers the branches, as it does in Safari. The menu is
        // filled in when it is about to open (see menuNeedsUpdate): what is ahead changes
        // with every navigation, and a menu built once would be a menu built wrong.
        let forwardMenu = NSMenu()
        forwardMenu.delegate = self
        navButtons.setMenu(forwardMenu, forSegment: 1)

        reloadButton = NSButton(
            image: NSImage(systemSymbolName: "arrow.clockwise", accessibilityDescription: "Reload") ?? NSImage(),
            target: self,
            action: #selector(reloadOrStop)
        )
        reloadButton.bezelStyle = .texturedRounded
        reloadButton.isBordered = false

        bookmarkButton = NSButton(
            image: NSImage(systemSymbolName: "star", accessibilityDescription: "Bookmark") ?? NSImage(),
            target: self,
            action: #selector(toggleBookmark)
        )
        bookmarkButton.bezelStyle = .texturedRounded
        bookmarkButton.isBordered = false

        downloadsButton = NSButton(
            image: NSImage(systemSymbolName: "arrow.down.circle", accessibilityDescription: "Downloads") ?? NSImage(),
            target: self,
            action: #selector(showDownloads)
        )
        downloadsButton.bezelStyle = .texturedRounded
        downloadsButton.isBordered = false
        downloadsButton.isHidden = true // appears once there is a download to show

        addressField.delegate = self
        addressField.target = self
        addressField.action = #selector(addressEntered)
        addressField.placeholderString = "Search or enter address"
        addressField.font = .systemFont(ofSize: 13)
        addressField.bezelStyle = .roundedBezel
        addressField.focusRingType = .none

        let toolbar = NSToolbar(identifier: "beacon.toolbar")
        toolbar.delegate = self
        toolbar.displayMode = .iconOnly
        toolbar.allowsUserCustomization = false
        window?.toolbar = toolbar
        window?.toolbarStyle = .unified
    }

    func toolbarAllowedItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        toolbarDefaultItemIdentifiers(toolbar)
    }

    func toolbarDefaultItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        [
            ToolbarID.navigation,
            ToolbarID.reload,
            .flexibleSpace,
            ToolbarID.address,
            ToolbarID.bookmark,
            .flexibleSpace,
            ToolbarID.downloads,
            ToolbarID.newTab,
        ]
    }

    func toolbar(
        _ toolbar: NSToolbar,
        itemForItemIdentifier identifier: NSToolbarItem.Identifier,
        willBeInsertedIntoToolbar flag: Bool
    ) -> NSToolbarItem? {
        let item = NSToolbarItem(itemIdentifier: identifier)
        switch identifier {
        case ToolbarID.navigation:
            item.view = navButtons
            item.label = "Back/Forward"
        case ToolbarID.reload:
            item.view = reloadButton
            item.label = "Reload"
        case ToolbarID.address:
            item.view = addressField
            item.minSize = NSSize(width: 260, height: 24)
            item.maxSize = NSSize(width: 900, height: 24)
            item.label = "Address"
        case ToolbarID.bookmark:
            item.view = bookmarkButton
            item.label = "Bookmark"
        case ToolbarID.downloads:
            item.view = downloadsButton
            item.label = "Downloads"
        case ToolbarID.newTab:
            let button = NSButton(
                image: NSImage(systemSymbolName: "plus", accessibilityDescription: "New Tab") ?? NSImage(),
                target: self,
                action: #selector(newTab)
            )
            button.bezelStyle = .texturedRounded
            button.isBordered = false
            item.view = button
            item.label = "New Tab"
        default:
            return nil
        }
        return item
    }

    // ── content layout ────────────────────────────────────────────────────

    private func buildContent() {
        guard let window else { return }
        let content = NSView()
        window.contentView = content

        bookmarksBar.orientation = .horizontal
        bookmarksBar.spacing = 2
        bookmarksBar.alignment = .centerY
        bookmarksBar.edgeInsets = NSEdgeInsets(top: 2, left: 8, bottom: 2, right: 8)
        bookmarksBar.isHidden = true

        // Explicit constraints rather than an outer NSStackView. A plain NSView has no
        // intrinsic content size, so inside a stack the page collapses to zero height —
        // which looks exactly like "the renderer is broken" and is not.
        hoverLabel.font = .systemFont(ofSize: 11)
        hoverLabel.textColor = .secondaryLabelColor
        hoverLabel.backgroundColor = .windowBackgroundColor
        hoverLabel.drawsBackground = true
        hoverLabel.isHidden = true
        hoverLabel.lineBreakMode = .byTruncatingMiddle
        hoverLabel.wantsLayer = true
        hoverLabel.layer?.cornerRadius = 3

        devPanel.isHidden = true
        devPanel.onClose = { [weak self] in self?.toggleDeveloperTools(nil) }
        devPanel.onResize = { [weak self] wanted in self?.setDeveloperPanelHeight(wanted) }

        crashOverlay.isHidden = true
        crashOverlay.onReload = { [weak self] in
            guard let self, self.currentTab != 0 else { return }
            self.browser.reviveTab(self.currentTab)
            self.refreshChrome()
        }

        for view in [tabStrip, bookmarksBar, progressLine, pageView, devPanel, crashOverlay, hoverLabel] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            content.addSubview(view)
        }

        let barHeight = bookmarksBar.heightAnchor.constraint(equalToConstant: 0)
        bookmarksBarHeight = barHeight
        let panelHeight = devPanel.heightAnchor.constraint(equalToConstant: 0)
        devPanelHeight = panelHeight

        NSLayoutConstraint.activate([
            tabStrip.topAnchor.constraint(equalTo: content.topAnchor),
            tabStrip.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            tabStrip.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            tabStrip.heightAnchor.constraint(equalToConstant: TabStripView.height),

            bookmarksBar.topAnchor.constraint(equalTo: tabStrip.bottomAnchor),
            bookmarksBar.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            bookmarksBar.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            barHeight,

            progressLine.topAnchor.constraint(equalTo: bookmarksBar.bottomAnchor),
            progressLine.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            progressLine.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            progressLine.heightAnchor.constraint(equalToConstant: 2),

            // The page takes everything left over, and is where the window's size goes.
            pageView.topAnchor.constraint(equalTo: progressLine.bottomAnchor),
            pageView.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            pageView.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            pageView.bottomAnchor.constraint(equalTo: devPanel.topAnchor),

            devPanel.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            devPanel.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            devPanel.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            panelHeight,

            // Exactly over the page: the tab is still there, it is the page that is gone.
            crashOverlay.topAnchor.constraint(equalTo: pageView.topAnchor),
            crashOverlay.leadingAnchor.constraint(equalTo: pageView.leadingAnchor),
            crashOverlay.trailingAnchor.constraint(equalTo: pageView.trailingAnchor),
            crashOverlay.bottomAnchor.constraint(equalTo: pageView.bottomAnchor),

            hoverLabel.leadingAnchor.constraint(equalTo: pageView.leadingAnchor, constant: 4),
            hoverLabel.bottomAnchor.constraint(equalTo: pageView.bottomAnchor, constant: -4),
            hoverLabel.widthAnchor.constraint(lessThanOrEqualTo: pageView.widthAnchor, multiplier: 0.7),
        ])

        pageView.onHoverChanged = { [weak self] url in
            self?.hoverLabel.stringValue = url
            self?.hoverLabel.isHidden = url.isEmpty
        }

        pageView.onContextMenu = { [weak self] event, x, y in
            self?.askWhatIsThere(x: x, y: y, intent: .contextMenu(event))
        }
        pageView.onOpenInNewTab = { [weak self] x, y, foreground in
            self?.askWhatIsThere(x: x, y: y, intent: .openInNewTab(foreground: foreground))
        }
        pageView.onSwipeNavigate = { [weak self] direction in
            direction < 0 ? self?.goBack() : self?.goForward()
        }
        pageView.onZoomGesture = { [weak self] magnification in
            guard let self, self.currentTab != 0 else { return }
            let current = self.browser.zoom(of: self.currentTab)
            self.browser.setZoom(self.currentTab, current * Float(1 + magnification))
        }
    }

    private func wireTabStrip() {
        tabStrip.onSelect = { [weak self] tab in self?.select(tab) }
        tabStrip.onClose = { [weak self] tab in self?.close(tab) }
        tabStrip.onNewTab = { [weak self] in self?.newTab(nil) }
        tabStrip.onContextMenu = { [weak self] tab, event in self?.showTabContextMenu(tab, event: event) }
        tabStrip.onReordered = { [weak self] in self?.refreshChrome() }
    }

    // ── tab ownership ─────────────────────────────────────────────────────

    /// Open a tab and claim it for this window.
    ///
    /// `after` puts it beside the tab it came from rather than at the end of the strip,
    /// which is what opening a link from a page means.
    @discardableResult
    func openTab(_ url: String, activate: Bool, after: BeaconTabId = 0) -> BeaconTabId {
        let tab = after == 0 ? browser.openTab(url) : browser.openTab(url, after: after)
        guard tab != 0 else {
            NSLog("beacon: could not open \(url)")
            return 0
        }
        if let index = ownedTabs.firstIndex(of: after) {
            ownedTabs.insert(tab, at: index + 1)
        } else {
            ownedTabs.append(tab)
        }
        if activate {
            select(tab)
        } else {
            refreshChrome()
        }
        return tab
    }

    /// Reopen the tabs a previous run had open, in the order it had them.
    ///
    /// Pinned tabs stay pinned, and whatever was in front comes back in front. Nothing here
    /// decides what a session *is* — Beacon writes the file as it runs, so this only puts
    /// back what it recorded.
    func restore(_ session: [Browser.SessionTab]) {
        var front: BeaconTabId = 0
        for saved in session {
            let tab = openTab(saved.url, activate: false)
            guard tab != 0 else { continue }
            if saved.pinned {
                browser.setPinned(tab, true)
            }
            if saved.active {
                front = tab
            }
        }
        // Something has to be in front. The saved active tab, or the first that opened.
        select(front != 0 ? front : (ownedTabs.first ?? 0))
    }

    private func select(_ tab: BeaconTabId) {
        guard tab != 0, ownedTabs.contains(tab) else { return }
        currentTab = tab
        browser.activateTab(tab)
        pageView.show(tab: tab)
        if !editingAddress {
            addressField.stringValue = browser.url(of: tab)
        }
        refreshChrome()
    }

    private func close(_ tab: BeaconTabId) {
        guard let index = ownedTabs.firstIndex(of: tab) else { return }
        // The browser refuses to close the very last tab; if this window has only one but
        // the browser has more, closing the window is what the user means.
        if ownedTabs.count == 1 {
            window?.performClose(nil)
            return
        }
        browser.closeTab(tab)
        ownedTabs.remove(at: index)
        if currentTab == tab {
            select(ownedTabs[min(index, ownedTabs.count - 1)])
        } else {
            refreshChrome()
        }
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
            // Events for tabs another window owns are that window's business. Both windows
            // poll the same queue, so each takes what is its own and ignores the rest.
            if event.tab != 0, !ownedTabs.contains(event.tab), event.kind != BEACON_REDRAW {
                continue
            }
            switch event.kind {
            case BEACON_REDRAW:
                pageView.redraw()
            case BEACON_URL_CHANGED:
                if event.tab == currentTab, !editingAddress, let text = event.text {
                    addressField.stringValue = text
                }
                needsChromeRefresh = true
            case BEACON_HOVER_URL:
                if event.tab == currentTab {
                    pageView.setHover(event.text)
                }
            case BEACON_TITLE_CHANGED:
                if event.tab == currentTab, let text = event.text {
                    setWindowTitle(text)
                }
                needsChromeRefresh = true
            case BEACON_PROGRESS:
                if event.tab == currentTab {
                    updateProgress()
                }
            case BEACON_CURSOR_CHANGED:
                applyCursor(event.number)
            case BEACON_DOWNLOAD_OFFERED:
                offerDownload(offer: UInt64(event.number), suggested: event.text ?? "download")
            case BEACON_DOWNLOAD_CHANGED:
                refreshDownloads()
            case BEACON_TABS_CHANGED, BEACON_NAV_STATE_CHANGED, BEACON_LOADING_CHANGED,
                BEACON_ACTIVE_TAB_CHANGED, BEACON_FAVICON_CHANGED:
                needsChromeRefresh = true
            case BEACON_TAB_CRASHED:
                NSLog("beacon: tab crashed: \(event.text ?? "")")
                needsChromeRefresh = true
            case BEACON_NAVIGATION_FAILED:
                // The tab already holds the error page — Beacon put it there — so this is
                // only worth a line in the log a developer might be reading.
                NSLog("beacon: navigation failed: \(event.text ?? "")")
                needsChromeRefresh = true
            case BEACON_HIT_TEST:
                // The token is ours, handed back — but it arrives as a double, and a
                // conversion that traps would turn a garbled event into a crash.
                if event.number > 0, event.number < Double(UInt64.max) {
                    answerHitTest(token: UInt64(event.number))
                }
            case BEACON_LOG:
                NSLog("beacon: \(event.text ?? "")")
            default:
                break
            }
        }
        if needsChromeRefresh { refreshChrome() }
    }

    /// Rebuild everything from what the browser currently says.
    func refreshChrome() {
        // Drop tabs the browser no longer has — a crash or an engine-side close.
        let live = Set(browser.tabs)
        ownedTabs.removeAll { !live.contains($0) }
        if !ownedTabs.contains(currentTab) {
            currentTab = ownedTabs.first ?? 0
            if currentTab != 0 { pageView.show(tab: currentTab) }
        }
        // Keep this window's order in step with the browser's, so a drag survives a rebuild.
        let browserOrder = browser.tabs
        ownedTabs.sort { a, b in
            (browserOrder.firstIndex(of: a) ?? 0) < (browserOrder.firstIndex(of: b) ?? 0)
        }

        navButtons.setEnabled(currentTab != 0 && browser.canGoBack(currentTab), forSegment: 0)
        navButtons.setEnabled(currentTab != 0 && browser.canGoForward(currentTab), forSegment: 1)

        let loading = currentTab != 0 && browser.isLoading(currentTab)
        reloadButton.image = NSImage(
            systemSymbolName: loading ? "xmark" : "arrow.clockwise",
            accessibilityDescription: loading ? "Stop" : "Reload"
        )
        reloadButton.toolTip = loading ? "Stop (⌘.)" : "Reload (⌘R)"

        let bookmarked = currentTab != 0 && browser.isBookmarked(currentTab)
        bookmarkButton.image = NSImage(
            systemSymbolName: bookmarked ? "star.fill" : "star",
            accessibilityDescription: "Bookmark"
        )

        tabStrip.tabs = ownedTabs
        tabStrip.activeTab = currentTab
        tabStrip.refresh()

        // A dead tab keeps its place in the strip and its address; what it cannot do is
        // draw, so the shell says why and offers to start it again.
        let crash = currentTab == 0 ? nil : browser.crashReason(currentTab)
        crashOverlay.reason = crash
        crashOverlay.isHidden = crash == nil

        // The network panel follows the tab in front of it. A request list mixing several
        // tabs together is a log, not a panel.
        devPanel.tab = currentTab

        updateProgress()
        rebuildBookmarksBar()
        refreshDownloads()
    }

    /// A private window keeps saying so in its title, because the page title is the one
    /// piece of chrome someone reads without looking.
    private func setWindowTitle(_ pageTitle: String) {
        let base = pageTitle.isEmpty ? "Gosub Beacon" : pageTitle
        window?.title = browser.isPrivate ? "\(base) — Private" : base
    }

    /// Whether this window's session is private. The browser is asked, not remembered.
    var isPrivate: Bool { browser.isPrivate }

    private func updateProgress() {
        guard currentTab != 0, browser.isLoading(currentTab) else {
            progressLine.isLoading = false
            return
        }
        // nil means the server sent no length: a sliding bar is honest, a full one is a lie.
        progressLine.fraction = browser.progress(of: currentTab)
        progressLine.isLoading = true
    }

    private func applyCursor(_ code: Double) {
        switch code {
        case 1: NSCursor.pointingHand.set()
        case 2: NSCursor.iBeam.set()
        default: NSCursor.arrow.set()
        }
    }

    // ── actions ───────────────────────────────────────────────────────────

    @objc private func navSegmentClicked() {
        navButtons.selectedSegment == 0 ? goBack() : goForward()
    }

    @objc func goBack() { browser.back() }
    @objc func goForward() { browser.forward() }

    @objc func reloadOrStop() {
        guard currentTab != 0 else { return }
        browser.isLoading(currentTab) ? browser.stop() : browser.reload()
    }

    @objc func reloadIgnoringCache() { browser.reload(ignoringCache: true) }
    @objc func stopLoading() { browser.stop() }

    @objc func newTab(_ sender: Any?) {
        openTab(browser.homepage, activate: true)
        focusAddressBar(nil)
    }

    @objc func closeTab(_ sender: Any?) {
        guard currentTab != 0 else { return }
        close(currentTab)
    }

    @objc func reopenClosedTab(_ sender: Any?) {
        let tab = browser.reopenClosedTab()
        guard tab != 0 else { return }
        ownedTabs.append(tab)
        select(tab)
    }

    @objc func focusAddressBar(_ sender: Any?) {
        window?.makeFirstResponder(addressField)
        addressField.currentEditor()?.selectAll(nil)
    }

    @objc private func addressEntered() {
        let text = addressField.stringValue.trimmingCharacters(in: .whitespaces)
        guard !text.isEmpty, currentTab != 0 else { return }
        browser.navigate(currentTab, to: text)
        window?.makeFirstResponder(pageView)
    }

    @objc func toggleBookmark() {
        guard currentTab != 0 else { return }
        browser.toggleBookmark(currentTab)
        refreshChrome()
    }

    @objc func zoomIn(_ sender: Any?) {
        guard currentTab != 0 else { return }
        browser.stepZoom(currentTab, by: 1)
    }

    @objc func zoomOut(_ sender: Any?) {
        guard currentTab != 0 else { return }
        browser.stepZoom(currentTab, by: -1)
    }

    @objc func zoomReset(_ sender: Any?) {
        guard currentTab != 0 else { return }
        browser.setZoom(currentTab, 1.0)
    }

    @objc func toggleBookmarksBar(_ sender: Any?) {
        bookmarksBarVisible.toggle()
        bookmarksBar.isHidden = !bookmarksBarVisible
        bookmarksBarHeight?.constant = bookmarksBarVisible ? 28 : 0
        if bookmarksBarVisible {
            rebuildBookmarksBar()
        } else {
            // A hidden stack still lays out its arranged subviews, and they would argue with
            // the zero-height constraint. Emptying it is cheaper than a priority fight.
            bookmarksBar.arrangedSubviews.forEach { $0.removeFromSuperview() }
        }
    }

    @objc func selectNextTab(_ sender: Any?) {
        guard let index = ownedTabs.firstIndex(of: currentTab), !ownedTabs.isEmpty else { return }
        select(ownedTabs[(index + 1) % ownedTabs.count])
    }

    @objc func selectPreviousTab(_ sender: Any?) {
        guard let index = ownedTabs.firstIndex(of: currentTab), !ownedTabs.isEmpty else { return }
        select(ownedTabs[(index - 1 + ownedTabs.count) % ownedTabs.count])
    }

    /// Cmd+1..8 pick a tab; Cmd+9 is the last one, as in Safari and Chrome.
    func selectTab(number: Int) {
        guard !ownedTabs.isEmpty else { return }
        let index = number == 9 ? ownedTabs.count - 1 : number - 1
        guard index >= 0, index < ownedTabs.count else { return }
        select(ownedTabs[index])
    }

    @objc func openHomePage(_ sender: Any?) {
        guard currentTab != 0 else { return }
        // The homepage is a setting, and the browser is the one that knows it.
        browser.navigate(currentTab, to: browser.homepage)
    }

    // ── developer panel ───────────────────────────────────────────────────

    var isDeveloperPanelOpen: Bool { !devPanel.isHidden }

    @objc func toggleDeveloperTools(_ sender: Any?) {
        setDeveloperPanel(open: devPanel.isHidden)
    }

    @objc func showLog(_ sender: Any?) {
        setDeveloperPanel(open: true)
        devPanel.show(.log)
    }

    @objc func showConsole(_ sender: Any?) {
        setDeveloperPanel(open: true)
        devPanel.show(.console)
    }

    @objc func showTimings(_ sender: Any?) {
        setDeveloperPanel(open: true)
        devPanel.show(.timings)
    }

    @objc func showNetwork(_ sender: Any?) {
        setDeveloperPanel(open: true)
        devPanel.show(.network)
    }

    @objc func resetTimings(_ sender: Any?) {
        browser.resetTimings()
    }

    /// Resize the panel from a drag on its top edge.
    ///
    /// Clamped here rather than in the panel: how much room there is belongs to the window,
    /// and a panel that could be dragged over the whole page would leave a browser showing
    /// no page at all.
    private func setDeveloperPanelHeight(_ wanted: CGFloat) {
        guard let content = window?.contentView else { return }
        let ceiling = max(120, content.bounds.height - 180)
        devPanelHeight?.constant = min(max(wanted, 90), ceiling)
        content.layoutSubtreeIfNeeded()
    }

    private func setDeveloperPanel(open: Bool) {
        devPanel.isHidden = !open
        devPanelHeight?.constant = open ? DeveloperPanel.defaultHeight : 0
        // Polling stops with the panel: a closed panel should cost nothing at all.
        devPanel.setActive(open)
        // The page view is resized by the constraint change, and its own setFrameSize sends
        // the new viewport — but only once AppKit has actually laid out.
        window?.contentView?.layoutSubtreeIfNeeded()
    }

    // ── bookmarks bar ─────────────────────────────────────────────────────

    private func rebuildBookmarksBar() {
        guard bookmarksBarVisible else { return }
        bookmarksBar.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for bookmark in browser.bookmarks.prefix(20) {
            let label = bookmark.title.isEmpty ? bookmark.url : bookmark.title
            let button = NSButton(title: String(label.prefix(24)), target: self, action: #selector(bookmarkClicked(_:)))
            button.bezelStyle = .inline
            button.isBordered = false
            button.font = .systemFont(ofSize: 11)
            button.toolTip = bookmark.url
            bookmarksBar.addArrangedSubview(button)
        }
    }

    @objc private func bookmarkClicked(_ sender: NSButton) {
        guard let url = sender.toolTip, currentTab != 0 else { return }
        browser.navigate(currentTab, to: url)
    }

    // ── downloads ─────────────────────────────────────────────────────────

    private func offerDownload(offer: UInt64, suggested: String) {
        guard let window else {
            browser.rejectDownload(offer)
            return
        }
        let panel = NSSavePanel()
        panel.nameFieldStringValue = suggested
        panel.canCreateDirectories = true
        panel.beginSheetModal(for: window) { [weak self] response in
            guard let self else { return }
            guard response == .OK, let url = panel.url else {
                // Dismissing the panel must decline the offer, or it is leaked on the Rust
                // side waiting for an answer that never comes.
                self.browser.rejectDownload(offer)
                return
            }
            self.browser.acceptDownload(offer, to: url.path)
            self.refreshDownloads()
        }
    }

    private func refreshDownloads() {
        downloadsButton.isHidden = browser.downloads.isEmpty
    }

    @objc private func showDownloads() {
        let menu = NSMenu()
        let downloads = browser.downloads
        if downloads.isEmpty {
            menu.addItem(withTitle: "No Downloads", action: nil, keyEquivalent: "")
        }
        for download in downloads.reversed() {
            let detail: String
            switch download.state {
            case BEACON_DOWNLOAD_FINISHED: detail = "Finished"
            case BEACON_DOWNLOAD_FAILED: detail = "Failed"
            default:
                if let fraction = download.progress {
                    detail = "\(Int(fraction * 100))%"
                } else {
                    detail = ByteCountFormatter.string(fromByteCount: Int64(download.received), countStyle: .file)
                }
            }
            let item = NSMenuItem(
                title: "\(download.filename) — \(detail)",
                action: #selector(openDownload(_:)),
                keyEquivalent: ""
            )
            item.target = self
            item.representedObject = download.id
            menu.addItem(item)
        }
        menu.popUp(positioning: nil, at: NSPoint(x: 0, y: downloadsButton.bounds.height), in: downloadsButton)
    }

    @objc private func openDownload(_ sender: NSMenuItem) {
        guard let id = sender.representedObject as? UInt64 else { return }
        browser.openDownload(id)
    }

    // ── context menus ─────────────────────────────────────────────────────

    /// Ask the engine what is at a page point, and remember why we asked.
    ///
    /// The answer comes back as an event rather than a return value: the engine reads its
    /// layout tree on its own thread, and a shell that blocked for it would stall its own
    /// run loop on every right-click.
    private func askWhatIsThere(x: Float, y: Float, intent: HitIntent) {
        guard currentTab != 0 else { return }
        let token = browser.hitTest(currentTab, x: x, y: y)
        guard token != 0 else { return }
        pendingHits[token] = intent
    }

    private func answerHitTest(token: UInt64) {
        guard let intent = pendingHits.removeValue(forKey: token) else { return }
        let hit = browser.lastHit
        switch intent {
        case .contextMenu(let event):
            showPageContextMenu(event, hit: hit)
        case .openInNewTab(let foreground):
            // Nothing there: a ⌘-click on empty page area does nothing, rather than opening
            // a tab on whatever the pointer happened to pass over earlier.
            guard let link = hit.link else { return }
            openTab(link, activate: foreground, after: currentTab)
        }
    }

    private func showPageContextMenu(_ event: NSEvent, hit: Browser.Hit) {
        let menu = NSMenu()

        if let link = hit.link {
            add(to: menu, "Open Link in New Tab", #selector(openLinkInNewTab(_:)), link)
            add(to: menu, "Download Linked File…", #selector(saveLinkAs(_:)), link)
            add(to: menu, "Copy Link", #selector(copyString(_:)), link)
            menu.addItem(.separator())
        }
        if let image = hit.image {
            add(to: menu, "Open Image in New Tab", #selector(openLinkInNewTab(_:)), image)
            add(to: menu, "Copy Image Address", #selector(copyString(_:)), image)
            menu.addItem(.separator())
        }
        // Until the engine has text selection, Copy copies the text node under the pointer.
        // Offered rather than left out: it is the answer people want often enough, and a
        // greyed-out Copy would say less about why.
        if let text = hit.selection ?? hit.text {
            add(to: menu, "Copy", #selector(copyString(_:)), text)
            menu.addItem(.separator())
        }

        add(to: menu, "Back", #selector(goBack), nil, enabled: canGoBack)
        add(to: menu, "Forward", #selector(goForward), nil, enabled: canGoForward)
        add(to: menu, "Reload", #selector(reloadOrStop), nil)
        menu.addItem(.separator())
        add(to: menu, "View Source", #selector(viewSource(_:)), nil)

        NSMenu.popUpContextMenu(menu, with: event, for: pageView)
    }

    /// A menu item wired to this window, carrying whatever the action needs.
    @discardableResult
    private func add(
        to menu: NSMenu,
        _ title: String,
        _ action: Selector,
        _ object: Any?,
        enabled: Bool = true
    ) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: action, keyEquivalent: "")
        item.target = self
        item.representedObject = object
        item.isEnabled = enabled
        menu.addItem(item)
        return item
    }

    @objc private func openLinkInNewTab(_ sender: NSMenuItem) {
        guard let url = sender.representedObject as? String else { return }
        openTab(url, activate: false, after: currentTab)
    }

    @objc private func copyString(_ sender: NSMenuItem) {
        guard let text = sender.representedObject as? String else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    /// Save a link's target rather than following it. The engine has no "fetch to disk"
    /// command, so this navigates to it: anything it will not render arrives as a download
    /// offer, which is the panel the user was asking for.
    @objc private func saveLinkAs(_ sender: NSMenuItem) {
        guard let url = sender.representedObject as? String, currentTab != 0 else { return }
        browser.navigate(currentTab, to: url)
    }

    @objc func viewSource(_ sender: Any?) {
        guard currentTab != 0 else { return }
        // Through the browser, which fetches the bytes and marks them up: a tab opened on
        // "view-source:…" by hand would do exactly the same thing.
        let tab = browser.viewSource(of: currentTab)
        guard tab != 0 else { return }
        ownedTabs.append(tab)
        select(tab)
    }

    // ── where forward leads ───────────────────────────────────────────────

    /// Fill the Forward button's press-and-hold menu with what is actually ahead.
    ///
    /// Usually one page — the one you just came back from. More than one means the history
    /// forked: you went back and then somewhere else, and both branches are still there.
    /// That fork is the only reason this menu exists, so with nothing ahead it stays empty
    /// and the press falls through to the button's own click.
    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()
        guard currentTab != 0 else { return }
        for (index, url) in browser.forwardEntries(currentTab).enumerated() {
            let item = NSMenuItem(title: Self.shorten(url), action: #selector(goForwardTo(_:)), keyEquivalent: "")
            item.target = self
            item.tag = index
            item.toolTip = url
            menu.addItem(item)
        }
    }

    /// A URL that fits in a menu. The host and the last path segment are what identify a
    /// page to someone choosing between two of them; the middle rarely is.
    private static func shorten(_ url: String) -> String {
        guard url.count > 72, let parsed = URL(string: url), let host = parsed.host else {
            return url
        }
        let last = parsed.lastPathComponent
        return last.isEmpty ? host : "\(host)/…/\(last)"
    }

    @objc private func goForwardTo(_ sender: NSMenuItem) {
        browser.goForward(to: sender.tag)
    }

    private func showTabContextMenu(_ tab: BeaconTabId, event: NSEvent) {
        let menu = NSMenu()
        let pinned = browser.isPinned(tab)
        let pin = NSMenuItem(title: pinned ? "Unpin Tab" : "Pin Tab", action: #selector(togglePin(_:)), keyEquivalent: "")
        pin.target = self
        pin.representedObject = tab
        menu.addItem(pin)

        let newRight = NSMenuItem(title: "New Tab to the Right", action: #selector(newTabToTheRight(_:)), keyEquivalent: "")
        newRight.target = self
        newRight.representedObject = tab
        menu.addItem(newRight)

        let duplicate = NSMenuItem(title: "Duplicate Tab", action: #selector(duplicateTab(_:)), keyEquivalent: "")
        duplicate.target = self
        duplicate.representedObject = tab
        menu.addItem(duplicate)

        let reload = NSMenuItem(title: "Reload Tab", action: #selector(reloadTabFromMenu(_:)), keyEquivalent: "")
        reload.target = self
        reload.representedObject = tab
        menu.addItem(reload)

        menu.addItem(.separator())

        let closeItem = NSMenuItem(title: "Close Tab", action: #selector(closeTabFromMenu(_:)), keyEquivalent: "")
        closeItem.target = self
        closeItem.representedObject = tab
        menu.addItem(closeItem)

        // The three "close a lot of tabs" items sit in a submenu, where a mis-click costs
        // nothing: they are the only items here that throw work away.
        let closeMany = NSMenu()
        for (title, action) in [
            ("Close Tabs to the Left", #selector(closeTabsLeft(_:))),
            ("Close Tabs to the Right", #selector(closeTabsRight(_:))),
            ("Close Other Tabs", #selector(closeOtherTabs(_:))),
        ] {
            let item = NSMenuItem(title: title, action: action, keyEquivalent: "")
            item.target = self
            item.representedObject = tab
            closeMany.addItem(item)
        }
        let closeManyItem = NSMenuItem(title: "Close Multiple Tabs", action: nil, keyEquivalent: "")
        closeManyItem.submenu = closeMany
        menu.addItem(closeManyItem)

        NSMenu.popUpContextMenu(menu, with: event, for: tabStrip)
    }

    @objc private func togglePin(_ sender: NSMenuItem) {
        guard let tab = sender.representedObject as? BeaconTabId else { return }
        browser.setPinned(tab, !browser.isPinned(tab))
        refreshChrome()
    }

    @objc private func duplicateTab(_ sender: NSMenuItem) {
        guard let tab = sender.representedObject as? BeaconTabId else { return }
        openTab(browser.url(of: tab), activate: true, after: tab)
    }

    @objc private func newTabToTheRight(_ sender: NSMenuItem) {
        guard let tab = sender.representedObject as? BeaconTabId else { return }
        openTab(browser.homepage, activate: true, after: tab)
        focusAddressBar(nil)
    }

    @objc private func closeTabFromMenu(_ sender: NSMenuItem) {
        guard let tab = sender.representedObject as? BeaconTabId else { return }
        close(tab)
    }

    @objc private func closeOtherTabs(_ sender: NSMenuItem) {
        guard let keep = sender.representedObject as? BeaconTabId else { return }
        close(ownedTabs.filter { $0 != keep })
    }

    @objc private func closeTabsLeft(_ sender: NSMenuItem) {
        guard let pivot = sender.representedObject as? BeaconTabId,
            let index = ownedTabs.firstIndex(of: pivot)
        else { return }
        close(Array(ownedTabs.prefix(index)))
    }

    @objc private func closeTabsRight(_ sender: NSMenuItem) {
        guard let pivot = sender.representedObject as? BeaconTabId,
            let index = ownedTabs.firstIndex(of: pivot)
        else { return }
        close(Array(ownedTabs.suffix(from: index + 1)))
    }

    @objc private func reloadTabFromMenu(_ sender: NSMenuItem) {
        guard let tab = sender.representedObject as? BeaconTabId else { return }
        // Reload acts on the active tab, so make it that first — the browser owns which tab
        // is active, and this is the shell asking it to change its mind, not working around it.
        select(tab)
        browser.reload()
    }

    /// Close several tabs at once, leaving the window on something sensible.
    ///
    /// The browser refuses to close a pinned tab and the very last one, so this asks for
    /// each and then believes what the browser says about what is left, rather than
    /// assuming every close landed.
    private func close(_ tabs: [BeaconTabId]) {
        for tab in tabs {
            browser.closeTab(tab)
        }
        refreshChrome()
        if !ownedTabs.contains(currentTab), let first = ownedTabs.first {
            select(first)
        }
    }

    /// Asked by the menu bar, which validates against the application delegate.
    var canGoBack: Bool { currentTab != 0 && browser.canGoBack(currentTab) }
    var canGoForward: Bool { currentTab != 0 && browser.canGoForward(currentTab) }

    // ── menu validation ───────────────────────────────────────────────────

    func validateMenuItem(_ item: NSMenuItem) -> Bool {
        switch item.action {
        case #selector(goBack):
            return currentTab != 0 && browser.canGoBack(currentTab)
        case #selector(goForward):
            return currentTab != 0 && browser.canGoForward(currentTab)
        case #selector(closeTab(_:)):
            return currentTab != 0
        default:
            return true
        }
    }

    // ── window delegate ───────────────────────────────────────────────────

    func controlTextDidBeginEditing(_ obj: Notification) { editingAddress = true }
    func controlTextDidEndEditing(_ obj: Notification) { editingAddress = false }

    /// Re-offer suggestions on every keystroke. AppKit only completes on demand, so this is
    /// what turns "press F5 for a list" into an address bar that behaves like a browser's.
    func controlTextDidChange(_ obj: Notification) {
        guard (obj.object as AnyObject?) === addressField else { return }
        // Completing while deleting fights the user: they remove a character, a suggestion
        // reappears, and the field looks stuck.
        guard !isDeleting else { return }
        guard addressField.stringValue.count >= 2 else { return }
        addressField.currentEditor()?.complete(nil)
    }

    func control(
        _ control: NSControl,
        textView: NSTextView,
        completions words: [String],
        forPartialWordRange charRange: NSRange,
        indexOfSelectedItem index: UnsafeMutablePointer<Int>
    ) -> [String] {
        // -1 leaves nothing preselected, so the list is offered rather than typed into the
        // field. Preselecting would overwrite what the user is still typing.
        index.pointee = -1
        let typed = textView.string.trimmingCharacters(in: .whitespaces)
        guard typed.count >= 2 else { return [] }
        return browser.searchHistory(typed, limit: 8).map(\.url)
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy commandSelector: Selector) -> Bool {
        guard control === addressField else { return false }

        switch commandSelector {
        case #selector(NSResponder.deleteBackward(_:)), #selector(NSResponder.deleteForward(_:)):
            isDeleting = true
            return false

        case #selector(NSResponder.cancelOperation(_:)):
            // Escape puts the address back to the page's own and hands focus to the page —
            // Safari's behaviour, and the only way out of a half-typed address that does not
            // involve selecting and retyping it.
            addressField.stringValue = currentTab != 0 ? browser.url(of: currentTab) : ""
            window?.makeFirstResponder(pageView)
            return true

        default:
            isDeleting = false
            return false
        }
    }

    /// AppKit asks the window for a field editor. Handing the address bar its own is what
    /// makes completion replace the whole address.
    func windowWillReturnFieldEditor(_ sender: NSWindow, to client: Any?) -> Any? {
        client is AddressField ? addressEditor : nil
    }

    /// Two windows share one browser, and the ABI has a single notion of the active tab.
    /// Re-asserting it on focus is what keeps "back" meaning the front window's back.
    func windowDidBecomeKey(_ notification: Notification) {
        if currentTab != 0 {
            browser.activateTab(currentTab)
        }
    }

    func windowWillClose(_ notification: Notification) {
        pump?.invalidate()
        pump = nil
        for tab in ownedTabs {
            browser.closeTab(tab)
        }
        // The registry is what keeps this controller alive, so dropping it here would free
        // the object in the middle of its own delegate callback.
        DispatchQueue.main.async { WindowRegistry.shared.remove(self) }
    }
}
