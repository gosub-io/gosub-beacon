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
    NSTextFieldDelegate, NSMenuItemValidation
{
    let browser: Browser
    private let pageView: PageView
    private let tabStrip: TabStripView
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

    // ── construction ──────────────────────────────────────────────────────

    init(browser: Browser, startURL: String?) {
        self.browser = browser
        self.pageView = PageView(browser: browser)
        self.tabStrip = TabStripView(browser: browser)

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

        for view in [tabStrip, bookmarksBar, progressLine, pageView, hoverLabel] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            content.addSubview(view)
        }

        let barHeight = bookmarksBar.heightAnchor.constraint(equalToConstant: 0)
        bookmarksBarHeight = barHeight

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
            pageView.bottomAnchor.constraint(equalTo: content.bottomAnchor),

            hoverLabel.leadingAnchor.constraint(equalTo: pageView.leadingAnchor, constant: 4),
            hoverLabel.bottomAnchor.constraint(equalTo: pageView.bottomAnchor, constant: -4),
            hoverLabel.widthAnchor.constraint(lessThanOrEqualTo: pageView.widthAnchor, multiplier: 0.7),
        ])

        pageView.onHoverChanged = { [weak self] url in
            self?.hoverLabel.stringValue = url
            self?.hoverLabel.isHidden = url.isEmpty
        }

        pageView.onContextMenu = { [weak self] event, link in
            self?.showPageContextMenu(event, link: link)
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
    @discardableResult
    func openTab(_ url: String, activate: Bool) -> BeaconTabId {
        let tab = browser.openTab(url)
        guard tab != 0 else {
            NSLog("beacon: could not open \(url)")
            return 0
        }
        ownedTabs.append(tab)
        if activate {
            select(tab)
        } else {
            refreshChrome()
        }
        return tab
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
        openTab("gosub://home", activate: true)
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
        browser.navigate(currentTab, to: "gosub://home")
    }

    @objc func showEngineSettings(_ sender: Any?) {
        openTab("gosub://config", activate: true)
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

    private func showPageContextMenu(_ event: NSEvent, link: String) {
        let menu = NSMenu()
        if !link.isEmpty {
            let openInTab = NSMenuItem(title: "Open Link in New Tab", action: #selector(openLinkInNewTab(_:)), keyEquivalent: "")
            openInTab.target = self
            openInTab.representedObject = link
            menu.addItem(openInTab)

            let copyLink = NSMenuItem(title: "Copy Link", action: #selector(copyLink(_:)), keyEquivalent: "")
            copyLink.target = self
            copyLink.representedObject = link
            menu.addItem(copyLink)
            menu.addItem(.separator())
        }
        menu.addItem(withTitle: "Back", action: #selector(goBack), keyEquivalent: "")
        menu.addItem(withTitle: "Forward", action: #selector(goForward), keyEquivalent: "")
        menu.addItem(withTitle: "Reload", action: #selector(reloadOrStop), keyEquivalent: "")
        menu.addItem(.separator())
        let viewSource = NSMenuItem(title: "View Source", action: #selector(viewSource(_:)), keyEquivalent: "")
        viewSource.target = self
        menu.addItem(viewSource)
        menu.items.forEach { if $0.target == nil && $0.action != nil { $0.target = self } }
        NSMenu.popUpContextMenu(menu, with: event, for: pageView)
    }

    @objc private func openLinkInNewTab(_ sender: NSMenuItem) {
        guard let url = sender.representedObject as? String else { return }
        openTab(url, activate: false)
    }

    @objc private func copyLink(_ sender: NSMenuItem) {
        guard let url = sender.representedObject as? String else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(url, forType: .string)
    }

    @objc private func viewSource(_ sender: Any?) {
        guard currentTab != 0 else { return }
        openTab("view-source:" + browser.url(of: currentTab), activate: true)
    }

    private func showTabContextMenu(_ tab: BeaconTabId, event: NSEvent) {
        let menu = NSMenu()
        let pinned = browser.isPinned(tab)
        let pin = NSMenuItem(title: pinned ? "Unpin Tab" : "Pin Tab", action: #selector(togglePin(_:)), keyEquivalent: "")
        pin.target = self
        pin.representedObject = tab
        menu.addItem(pin)

        let duplicate = NSMenuItem(title: "Duplicate Tab", action: #selector(duplicateTab(_:)), keyEquivalent: "")
        duplicate.target = self
        duplicate.representedObject = tab
        menu.addItem(duplicate)

        menu.addItem(.separator())

        let closeItem = NSMenuItem(title: "Close Tab", action: #selector(closeTabFromMenu(_:)), keyEquivalent: "")
        closeItem.target = self
        closeItem.representedObject = tab
        menu.addItem(closeItem)

        let others = NSMenuItem(title: "Close Other Tabs", action: #selector(closeOtherTabs(_:)), keyEquivalent: "")
        others.target = self
        others.representedObject = tab
        menu.addItem(others)

        NSMenu.popUpContextMenu(menu, with: event, for: tabStrip)
    }

    @objc private func togglePin(_ sender: NSMenuItem) {
        guard let tab = sender.representedObject as? BeaconTabId else { return }
        browser.setPinned(tab, !browser.isPinned(tab))
        refreshChrome()
    }

    @objc private func duplicateTab(_ sender: NSMenuItem) {
        guard let tab = sender.representedObject as? BeaconTabId else { return }
        openTab(browser.url(of: tab), activate: true)
    }

    @objc private func closeTabFromMenu(_ sender: NSMenuItem) {
        guard let tab = sender.representedObject as? BeaconTabId else { return }
        close(tab)
    }

    @objc private func closeOtherTabs(_ sender: NSMenuItem) {
        guard let keep = sender.representedObject as? BeaconTabId else { return }
        for tab in ownedTabs where tab != keep {
            browser.closeTab(tab)
        }
        ownedTabs = [keep]
        select(keep)
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
