import AppKit
import CBeacon

/// The engine's settings, as a Settings window.
///
/// The GTK shell draws these as a `gosub://config` page because GTK can put widgets in a
/// tab. On macOS a page would be wrong twice over: settings live behind ⌘, in every Mac
/// application, and a browser page that is really a preferences panel is a thing people
/// have learned to distrust. Same store, same rules — `beacon_setting_set` decides what a
/// write means, here as there — drawn where a Mac user looks for it.
///
/// Each row gets the editor its type asks for: a switch for a boolean, a popup for a
/// setting restricted to named values, a number field for a bounded number, text for the
/// rest. Nothing is validated here; the store is asked, and a refusal puts the editor back.
final class SettingsWindowController: NSWindowController, NSTableViewDataSource, NSTableViewDelegate {
    private let browser: Browser
    private let table = NSTableView()
    private let searchField = NSSearchField()
    private let notice = NSTextField(labelWithString: "")
    private var rows: [Browser.SettingRow] = []

    init(browser: Browser) {
        self.browser = browser
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 720, height: 520),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Settings"
        window.setFrameAutosaveName("BeaconSettingsWindow")
        super.init(window: window)
        build()
        reload()
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    // ── layout ────────────────────────────────────────────────────────────

    private func build() {
        guard let window else { return }
        let content = NSView()
        window.contentView = content

        searchField.placeholderString = "Search settings"
        searchField.target = self
        searchField.action = #selector(searchChanged)
        searchField.sendsSearchStringImmediately = true
        searchField.sendsWholeSearchString = false

        // Said once, at the top, rather than on the rows it applies to: which settings are
        // read at startup is the engine's business and changes, and a note that goes stale
        // is worse than one that is general.
        notice.stringValue =
            "Changes apply to the running engine. Settings read once at startup (net.*) take effect after a restart."
        notice.font = .systemFont(ofSize: 11)
        notice.textColor = .secondaryLabelColor
        notice.lineBreakMode = .byWordWrapping
        notice.maximumNumberOfLines = 2

        table.dataSource = self
        table.delegate = self
        table.headerView = nil
        table.usesAlternatingRowBackgroundColors = true
        table.rowHeight = 54
        table.style = .inset
        table.selectionHighlightStyle = .none
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("setting"))
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)

        let scroller = NSScrollView()
        scroller.documentView = table
        scroller.hasVerticalScroller = true
        scroller.autohidesScrollers = true
        scroller.borderType = .noBorder

        for view in [searchField, notice, scroller] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            content.addSubview(view)
        }

        NSLayoutConstraint.activate([
            searchField.topAnchor.constraint(equalTo: content.topAnchor, constant: 12),
            searchField.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 16),
            searchField.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -16),

            notice.topAnchor.constraint(equalTo: searchField.bottomAnchor, constant: 8),
            notice.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 16),
            notice.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -16),

            scroller.topAnchor.constraint(equalTo: notice.bottomAnchor, constant: 8),
            scroller.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            scroller.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            scroller.bottomAnchor.constraint(equalTo: content.bottomAnchor),
        ])
    }

    // ── data ──────────────────────────────────────────────────────────────

    private func reload() {
        rows = browser.settings(matching: searchField.stringValue)
        table.reloadData()
    }

    func numberOfRows(in tableView: NSTableView) -> Int { rows.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard row < rows.count else { return nil }
        let setting = rows[row]
        // Built fresh rather than reused: a row's editor depends on its type, and a recycled
        // switch turning up in a text setting's row is the classic table-view bug.
        let view = SettingRowView(setting: setting)
        view.onCommit = { [weak self] value in
            guard let self else { return }
            if !self.browser.setSetting(setting.key, to: value) {
                // Refused: the store kept what it had, so the editor must go back to it
                // rather than sit there showing a value that is not in force.
                NSSound.beep()
            }
            self.reloadPreservingScroll()
        }
        view.onReset = { [weak self] in
            guard let self else { return }
            self.browser.resetSetting(setting.key)
            self.reloadPreservingScroll()
        }
        return view
    }

    /// Rewrite the rows without throwing away where the reader was.
    ///
    /// Deferred to the next turn of the run loop on purpose: this is called from an
    /// editor's own action, and rebuilding the row that owns the control currently
    /// delivering that action — replacing a switch mid-click, or a text field while its
    /// field editor is still resigning — is how table views come apart.
    private func reloadPreservingScroll() {
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            let visible = self.table.visibleRect
            self.reload()
            self.table.scrollToVisible(visible)
        }
    }

    @objc private func searchChanged() {
        reload()
    }
}

/// One setting: its key, what it does, and the editor its type asks for.
private final class SettingRowView: NSView {
    var onCommit: ((String) -> Void)?
    var onReset: (() -> Void)?

    private let setting: Browser.SettingRow
    private var editor: NSView?

    init(setting: Browser.SettingRow) {
        self.setting = setting
        super.init(frame: .zero)
        build()
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    private func build() {
        let key = NSTextField(labelWithString: setting.key)
        // Bold marks a setting that has been changed from its default — the one thing worth
        // being able to find by eye in a list of a hundred.
        key.font = .systemFont(ofSize: 12, weight: setting.isModified ? .semibold : .regular)

        var explanation = setting.description
        if let constraint = constraintText() {
            explanation += "  ·  \(constraint)"
        }
        explanation += "  ·  default \(setting.defaultValue.isEmpty ? "empty" : setting.defaultValue)"
        let description = NSTextField(labelWithString: explanation)
        description.font = .systemFont(ofSize: 10)
        description.textColor = .secondaryLabelColor
        description.lineBreakMode = .byTruncatingTail

        let control = makeEditor()
        editor = control

        let reset = NSButton(
            image: NSImage(systemSymbolName: "arrow.uturn.backward", accessibilityDescription: "Reset") ?? NSImage(),
            target: self,
            action: #selector(resetClicked)
        )
        reset.isBordered = false
        reset.toolTip = "Reset to the default"
        // Nothing to undo on a setting that is already at its default.
        reset.isEnabled = setting.isModified

        for view in [key, description, control, reset] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            addSubview(view)
        }

        NSLayoutConstraint.activate([
            key.topAnchor.constraint(equalTo: topAnchor, constant: 8),
            key.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 8),
            key.trailingAnchor.constraint(lessThanOrEqualTo: control.leadingAnchor, constant: -12),

            description.topAnchor.constraint(equalTo: key.bottomAnchor, constant: 2),
            description.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 8),
            description.trailingAnchor.constraint(lessThanOrEqualTo: control.leadingAnchor, constant: -12),

            control.centerYAnchor.constraint(equalTo: centerYAnchor),
            control.trailingAnchor.constraint(equalTo: reset.leadingAnchor, constant: -8),
            control.widthAnchor.constraint(lessThanOrEqualToConstant: 220),

            reset.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -10),
            reset.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }

    /// The accepted values, in a form that fits on the description line.
    private func constraintText() -> String? {
        guard setting.choices.isEmpty else { return nil } // already shown by the popup
        if let (low, high) = setting.bounds {
            return "\(low)–\(high)"
        }
        return nil
    }

    private func makeEditor() -> NSView {
        // A switch is the Mac control for a boolean, and it commits the moment it moves —
        // there is nothing to confirm about a two-state value.
        if setting.type == BEACON_SETTING_BOOL {
            let toggle = NSSwitch()
            toggle.state = ["true", "1", "yes", "on"].contains(setting.value.lowercased()) ? .on : .off
            toggle.target = self
            toggle.action = #selector(switchChanged(_:))
            return toggle
        }

        // A setting restricted to named values is a popup: typing "lft" into a text field
        // and being refused is a worse way to learn what it accepts.
        if !setting.choices.isEmpty {
            let popup = NSPopUpButton()
            popup.addItems(withTitles: setting.choices)
            popup.selectItem(withTitle: setting.value)
            popup.target = self
            popup.action = #selector(popupChanged(_:))
            return popup
        }

        let field = NSTextField(string: setting.value)
        field.font = .systemFont(ofSize: 11)
        field.alignment = isNumeric ? .right : .left
        field.target = self
        field.action = #selector(fieldCommitted(_:))
        field.delegate = self
        return field
    }

    private var isNumeric: Bool {
        setting.type == BEACON_SETTING_INT || setting.type == BEACON_SETTING_UINT || setting.type == BEACON_SETTING_FLOAT
    }

    @objc private func switchChanged(_ sender: NSSwitch) {
        onCommit?(sender.state == .on ? "true" : "false")
    }

    @objc private func popupChanged(_ sender: NSPopUpButton) {
        onCommit?(sender.titleOfSelectedItem ?? setting.value)
    }

    @objc private func fieldCommitted(_ sender: NSTextField) {
        guard sender.stringValue != setting.value else { return }
        onCommit?(sender.stringValue)
    }

    @objc private func resetClicked() {
        onReset?()
    }
}

extension SettingRowView: NSTextFieldDelegate {
    /// Committing on the way out as well as on Return: leaving a field with a value that
    /// was never stored is the commonest way a settings panel lies to someone.
    func controlTextDidEndEditing(_ notification: Notification) {
        guard let field = notification.object as? NSTextField else { return }
        fieldCommitted(field)
    }
}
