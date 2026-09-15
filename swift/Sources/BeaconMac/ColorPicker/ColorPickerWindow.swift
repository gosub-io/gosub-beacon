import AppKit

/// The colour picker a page's `<input type=color>` opens: the Figma colour design's parts
/// (plane, hue and opacity strips, CSS-name and hex fields, RGB/HSL, quick swatches, the
/// named-colour list) laid out in the shared picker shell at a size that leaves room for
/// the page behind it.
///
/// Not `NSColorPanel`: that speaks in colour spaces and its own swatch drawer, while a web
/// form holds six hex digits in sRGB and is written against 148 names, so this picker is
/// built around those. Every change is reported through `onChange` as it happens, so the
/// page's swatch follows the drag; `onFinish` fires once, with the chosen colour on OK and
/// nil on Cancel.
final class ColorPickerWindowController: PickerShellWindowController, NSTableViewDataSource, NSTableViewDelegate, NSTextFieldDelegate {
    var onChange: ((CSSColor) -> Void)?
    var onFinish: ((CSSColor?) -> Void)?

    /// How much of the picker to show: the engine setting `useragent.colorpicker.details`.
    enum Layout: String {
        /// Plane, strips, fields, swatches and the list of named colours.
        case full
        /// The same without the list card.
        case nocss
        /// The plane and the hue strip alone, with a hex readout: no sidebar, no fields.
        case packed

        init(setting: String?) {
            self = setting.flatMap(Layout.init(rawValue:)) ?? .full
        }
    }

    let layout: Layout
    private let allowsAlpha: Bool
    private var color: CSSColor
    /// The hue the strip is on. Separate from the colour because a grey has none: dragging
    /// the plane into the black corner and back out must return to the same hue.
    private var hue: Double

    private let plane = SaturationValuePlane()
    private let hueStrip = ColorStrip(kind: .hue)
    private let alphaStrip = ColorStrip(kind: .alpha)
    private let nameField = PickerStyle.field(placeholder: "e.g. rebeccapurple, rgb(…)", size: 13)
    private let nameCaption = PickerStyle.label("", size: 11, weight: .regular, color: PickerStyle.inkSecondary)
    private let hexField = PickerStyle.field(placeholder: "#rrggbb", size: 13, monospaced: true)
    /// The packed layout's read-only hex, where the fields would be.
    private let hexReadout = PickerStyle.label("", size: 13, weight: .medium, color: PickerStyle.ink)
    /// R, G, B, H, S, L, tagged 1 through 6.
    private var channelFields: [NSTextField] = []
    private var swatchButtons: [SwatchButton] = []
    private var quickSwatches: [CSSColor] = Store.swatches()

    private let searchField = PickerStyle.field(placeholder: "Search", size: 13)
    private let clearSearch = NSButton(image: NSImage(systemSymbolName: "xmark.circle.fill", accessibilityDescription: "Clear")!, target: nil, action: nil)
    private var chips: [Chip] = []
    private let scroll = NSScrollView()
    private let table = NSTableView()

    fileprivate struct Row {
        let color: CSSColor
        let title: String
        let subtitle: String
    }

    private var rows: [Row] = []
    private var syncingTable = false

    init(initial: CSSColor, allowsAlpha: Bool, layout: Layout = .full) {
        self.layout = layout
        self.allowsAlpha = allowsAlpha
        self.color = initial
        self.hue = initial.hsv.h
        let nav = [NavItem(symbol: "paintpalette.fill", title: "Color")]
        switch layout {
        case .full:
            super.init(size: NSSize(width: 960, height: 600), title: "Select a color", nav: nav,
                       mainCard: NSRect(x: 154, y: 42, width: 520, height: 500), sideCard: NSRect(x: 686, y: 42, width: 260, height: 500))
        case .nocss:
            super.init(size: NSSize(width: 690, height: 600), title: "Select a color", nav: nav,
                       mainCard: NSRect(x: 154, y: 42, width: 520, height: 500), sideCard: nil)
        case .packed:
            super.init(size: NSSize(width: 362, height: 312), title: "Select a color", nav: nav,
                       mainCard: NSRect(x: 0, y: 0, width: 362, height: 262), sideCard: nil, packed: true)
        }
        if layout == .packed {
            buildPacked()
        } else {
            buildMain()
        }
        if layout == .full {
            buildSide()
        }
        apply(initial, from: .external)
        rebuildRows()
    }

    /// The packed layout: the plane, the hue strip, and the hex where the buttons are not.
    private func buildPacked() {
        let c = mainCard
        plane.frame = NSRect(x: 14, y: 14, width: 296, height: 240)
        plane.knobRadius = 10
        plane.onChange = { [weak self] s, v in
            guard let self else { return }
            self.apply(CSSColor(h: self.hue, s: s, v: v, a: self.color.a), from: .plane)
        }
        c.addSubview(plane)
        hueStrip.frame = NSRect(x: 322, y: 14, width: 26, height: 240)
        hueStrip.knobRadius = 11
        hueStrip.onChange = { [weak self] position in
            guard let self else { return }
            self.hue = position * 360
            let (_, s, v) = self.color.hsv
            self.apply(CSSColor(h: self.hue, s: s, v: v, a: self.color.a), from: .hue)
        }
        c.addSubview(hueStrip)
        hexReadout.font = .monospacedSystemFont(ofSize: 13, weight: .medium)
        hexReadout.frame = NSRect(x: 14, y: 272, width: 120, height: 24)
        root.addSubview(hexReadout)
        root.addSubview(eyedropper(frame: NSRect(x: 140, y: 268, width: 32, height: 30)))
    }

    /// The eyedropper: macOS's own screen sampler (the loupe), which needs no permission and
    /// hands back the colour under the pointer anywhere on any display.
    private func eyedropper(frame: NSRect) -> NSButton {
        let button = SymbolButton(symbol: "eyedropper", frame: frame)
        button.target = self
        button.action = #selector(sampleFromScreen)
        button.toolTip = "Pick a colour from the screen"
        return button
    }

    @objc private func sampleFromScreen() {
        NSColorSampler().show { [weak self] sampled in
            guard let self, let sampled, let css = CSSColor(nsColor: sampled) else { return }
            self.apply(css, from: .external)
        }
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    override func didFinish(ok: Bool) {
        onFinish?(ok ? color : nil)
    }

    override func detach() {
        onChange = nil
        onFinish = nil
        super.detach()
    }

    // ── the main card ─────────────────────────────────────────────────────

    private func buildMain() {
        let c = mainCard

        plane.frame = NSRect(x: 16, y: 16, width: 380, height: 260)
        plane.knobRadius = 11
        plane.onChange = { [weak self] s, v in
            guard let self else { return }
            self.apply(CSSColor(h: self.hue, s: s, v: v, a: self.color.a), from: .plane)
        }
        c.addSubview(plane)

        hueStrip.frame = NSRect(x: 408, y: 16, width: 22, height: 260)
        hueStrip.knobRadius = 12
        hueStrip.onChange = { [weak self] position in
            guard let self else { return }
            self.hue = position * 360
            let (_, s, v) = self.color.hsv
            self.apply(CSSColor(h: self.hue, s: s, v: v, a: self.color.a), from: .hue)
        }
        c.addSubview(hueStrip)

        alphaStrip.frame = NSRect(x: 442, y: 16, width: 22, height: 260)
        alphaStrip.knobRadius = 12
        alphaStrip.onChange = { [weak self] position in
            guard let self else { return }
            var next = self.color
            next.a = 1 - position
            self.apply(next, from: .alpha)
        }
        alphaStrip.toolTip = "Opacity. A form's colour input keeps only the opaque colour; the hex here shows both."
        c.addSubview(alphaStrip)

        c.addSubview(placed(PickerStyle.label("CSS name", size: 13, weight: .semibold, color: PickerStyle.ink), 16, 292, 200, 16))
        nameField.delegate = self
        c.addSubview(placed(BorderedField(nameField), 16, 310, 200, 30))
        c.addSubview(placed(nameCaption, 16, 344, 488, 16))

        c.addSubview(placed(PickerStyle.label("Hex", size: 13, weight: .semibold, color: PickerStyle.ink), 232, 292, 100, 16))
        hexField.delegate = self
        c.addSubview(placed(BorderedField(hexField), 232, 310, 130, 30))
        let copy = CopyButton(frame: NSRect(x: 370, y: 310, width: 32, height: 30))
        copy.target = self
        copy.action = #selector(copyHex)
        copy.toolTip = "Copy hex"
        c.addSubview(copy)
        c.addSubview(eyedropper(frame: NSRect(x: 410, y: 310, width: 32, height: 30)))

        let names = ["R", "G", "B", "H", "S", "L"]
        let xs: [CGFloat] = [16, 68, 120, 190, 242, 294]
        for (i, name) in names.enumerated() {
            c.addSubview(placed(PickerStyle.label(name, size: 11, weight: .semibold, color: PickerStyle.inkSecondary, align: .center), xs[i], 364, 48, 14))
            let field = PickerStyle.field(size: 13, centered: true)
            field.font = .monospacedDigitSystemFont(ofSize: 13, weight: .regular)
            field.tag = i + 1
            field.delegate = self
            let box = BorderedField(field)
            box.radius = 7
            c.addSubview(placed(box, xs[i], 380, 48, 30))
            channelFields.append(field)
        }
        c.addSubview(placed(PickerStyle.label("%", size: 12, weight: .medium, color: PickerStyle.inkSecondary), 346, 387, 20, 16))

        c.addSubview(placed(Panel(fill: PickerStyle.divider, radius: 0), 16, 424, 488, 1))
        c.addSubview(placed(PickerStyle.label("Quick swatches", size: 12, weight: .semibold, color: PickerStyle.ink), 16, 432, 150, 16))
        rebuildSwatchRow()
    }

    /// How many quick swatches are kept. The row shows them all; the "+" goes away when
    /// it is full, and a colour added past the cap replaces the oldest.
    static let swatchLimit = 10

    private func rebuildSwatchRow() {
        swatchButtons.forEach { $0.removeFromSuperview() }
        swatchButtons = []
        var cx: CGFloat = 33
        for (i, swatch) in quickSwatches.prefix(Self.swatchLimit).enumerated() {
            let button = SwatchButton(frame: NSRect(x: cx - 17, y: 451, width: 34, height: 34))
            button.color = swatch
            button.tag = i
            button.target = self
            button.action = #selector(swatchPressed(_:))
            button.toolTip = "\(swatch.cssName?.name ?? swatch.hex) · right-click to remove"
            let menu = NSMenu()
            let remove = NSMenuItem(title: "Remove Swatch", action: #selector(removeSwatch(_:)), keyEquivalent: "")
            remove.target = self
            remove.tag = i
            menu.addItem(remove)
            button.menu = menu
            mainCard.addSubview(button)
            swatchButtons.append(button)
            cx += 42
        }
        if quickSwatches.count < Self.swatchLimit {
            let adder = SwatchButton(frame: NSRect(x: cx - 17, y: 451, width: 34, height: 34))
            adder.isAdder = true
            adder.target = self
            adder.action = #selector(addSwatch)
            adder.toolTip = "Keep this colour as a quick swatch (\(quickSwatches.count) of \(Self.swatchLimit)); right-click one to remove it"
            mainCard.addSubview(adder)
            swatchButtons.append(adder)
        }
        refreshSwatchRings()
    }

    private func refreshSwatchRings() {
        for button in swatchButtons where !button.isAdder {
            button.isCurrent = button.color == color
        }
    }

    // ── the side card ─────────────────────────────────────────────────────

    private func buildSide() {
        let c = sideCard
        c.addSubview(placed(PickerStyle.label("CSS colors", size: 16, weight: .semibold, color: PickerStyle.ink), 12, 12, 200, 22))

        let search = Panel(fill: PickerStyle.fieldBackground, border: PickerStyle.fieldBorder, radius: 15)
        c.addSubview(placed(search, 12, 44, 236, 30))
        let glass = NSImageView(frame: NSRect(x: 10, y: 7, width: 16, height: 16))
        glass.image = NSImage(systemSymbolName: "magnifyingglass", accessibilityDescription: nil)
        glass.contentTintColor = PickerStyle.inkSecondary
        search.addSubview(glass)
        searchField.delegate = self
        searchField.frame = NSRect(x: 30, y: 6, width: 178, height: 18)
        search.addSubview(searchField)
        clearSearch.isBordered = false
        clearSearch.contentTintColor = PickerStyle.inkSecondary
        clearSearch.target = self
        clearSearch.action = #selector(clearSearchPressed)
        clearSearch.frame = NSRect(x: 212, y: 7, width: 16, height: 16)
        clearSearch.isHidden = true
        search.addSubview(clearSearch)

        // The design also has a "Page" chip -- the colours the page uses -- which needs an
        // engine API that does not exist yet, so it is not offered.
        for (i, (title, x, w)) in [("All", CGFloat(12), CGFloat(44)), ("Named", 62, 70), ("System", 138, 74)].enumerated() {
            let chip = Chip(title: title)
            chip.tag = i
            chip.target = self
            chip.action = #selector(chipPressed(_:))
            chip.isOn = i == 0
            c.addSubview(placed(chip, x, 84, w, 26))
            chips.append(chip)
        }

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("colour"))
        column.width = 242
        table.addTableColumn(column)
        table.headerView = nil
        table.rowHeight = 44
        table.intercellSpacing = .zero
        table.selectionHighlightStyle = .none
        table.backgroundColor = .clear
        table.dataSource = self
        table.delegate = self
        table.target = self
        table.doubleAction = #selector(okPressed)
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.autohidesScrollers = true
        c.addSubview(placed(scroll, 8, 120, 244, 368))
    }

    private func placed(_ view: NSView, _ x: CGFloat, _ y: CGFloat, _ w: CGFloat, _ h: CGFloat) -> NSView {
        view.frame = NSRect(x: x, y: y, width: w, height: h)
        return view
    }

    // ── the list ──────────────────────────────────────────────────────────

    @objc private func chipPressed(_ sender: Chip) {
        for chip in chips {
            chip.isOn = chip === sender
        }
        rebuildRows()
    }

    @objc private func clearSearchPressed() {
        searchField.stringValue = ""
        clearSearch.isHidden = true
        rebuildRows()
    }

    /// The rows for the current chip and search: names that start with the query, then
    /// names that contain it, then -- when what was typed is itself a colour ("purple",
    /// "#639") -- the names nearest to that colour, so a search for purple also turns up
    /// indigo and blueviolet.
    private func rebuildRows() {
        guard layout == .full else { return }
        let pool: [CSSColor.Named]
        switch chips.firstIndex(where: \.isOn) ?? 0 {
        case 1: pool = CSSColor.named
        case 2: pool = CSSColor.systemColors()
        default: pool = CSSColor.named + CSSColor.systemColors()
        }
        let query = searchField.stringValue.trimmingCharacters(in: .whitespaces).lowercased()
        var picked: [CSSColor.Named]
        if query.isEmpty {
            picked = pool
        } else {
            let starts = pool.filter { $0.name.lowercased().hasPrefix(query) }
            let contains = pool.filter { !$0.name.lowercased().hasPrefix(query) && $0.name.lowercased().contains(query) }
            picked = starts + contains
            if let probe = CSSColor.parse(query) {
                let listed = Set(picked.map(\.name))
                let near = pool.filter { !listed.contains($0.name) }
                    .map { ($0, probe.distance(to: $0.color)) }
                    .filter { $0.1 < 140 }
                    .sorted { $0.1 < $1.1 }
                    .prefix(12)
                    .map { $0.0 }
                picked += near
            }
        }
        rows = picked.map { Row(color: $0.color, title: $0.name, subtitle: $0.color.hex) }
        table.reloadData()
        syncTableSelection(scroll: true)
    }

    private func syncTableSelection(scroll: Bool) {
        guard layout == .full else { return }
        syncingTable = true
        defer { syncingTable = false }
        if let index = rows.firstIndex(where: { $0.color == color.opaque }) {
            table.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false)
            if scroll {
                table.scrollRowToVisible(index)
            }
        } else {
            table.deselectAll(nil)
        }
    }

    func numberOfRows(in tableView: NSTableView) -> Int { rows.count }

    func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
        let id = NSUserInterfaceItemIdentifier("ColourRowView")
        let view = (tableView.makeView(withIdentifier: id, owner: nil) as? ColorRowBackground) ?? ColorRowBackground()
        view.identifier = id
        return view
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let id = NSUserInterfaceItemIdentifier("ColourRow")
        let view = (tableView.makeView(withIdentifier: id, owner: nil) as? ColorRowView) ?? ColorRowView()
        view.identifier = id
        view.show(rows[row])
        return view
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        guard !syncingTable, table.selectedRow >= 0, table.selectedRow < rows.count else { return }
        apply(rows[table.selectedRow].color, from: .list)
    }

    // ── the colour, and who changed it ────────────────────────────────────

    private enum Source {
        case plane, hue, alpha, name, hex, channels, list, swatch, external
    }

    /// Set the colour and bring every control into line with it, except the one that just
    /// set it -- a field being typed into must not be rewritten under the caret.
    private func apply(_ proposed: CSSColor, from source: Source) {
        let next = allowsAlpha ? proposed : proposed.opaque
        color = next
        let hsv = next.hsv
        if source != .plane, source != .alpha, source != .hue, hsv.s > 0, hsv.v > 0 {
            hue = hsv.h
        }
        plane.hue = hue
        if source != .plane {
            plane.saturation = hsv.s
            plane.value = hsv.v
        }
        if source != .hue {
            hueStrip.position = hue / 360
        }
        alphaStrip.tint = next
        if source != .alpha {
            alphaStrip.position = 1 - next.a
        }
        let exact = next.cssName
        if source != .name {
            nameField.stringValue = exact?.name ?? ""
        }
        // Under the name field: whether what is in it is a CSS keyword a stylesheet can use
        // as-is, and if not, which keyword comes closest.
        if let exact {
            let alias = exact.aliases.isEmpty ? "" : " · also spelled \(exact.aliases.joined(separator: ", "))"
            nameCaption.stringValue = "CSS keyword\(alias)"
        } else {
            nameCaption.stringValue = "Nearest keyword: \(next.nearestName().named.name)"
        }
        if source != .hex {
            hexField.stringValue = next.hexWithAlpha
        }
        hexReadout.stringValue = next.hexWithAlpha
        if source != .channels {
            let hsl = next.hsl
            let values = [next.r8, next.g8, next.b8, Int(hsl.h.rounded()), Int((hsl.s * 100).rounded()), Int((hsl.l * 100).rounded())]
            for (field, value) in zip(channelFields, values) {
                field.stringValue = String(value)
            }
        }
        refreshSwatchRings()
        if source != .list {
            syncTableSelection(scroll: source != .plane && source != .hue && source != .alpha)
        }
        onChange?(next)
    }

    func controlTextDidChange(_ notification: Notification) {
        guard let field = notification.object as? NSTextField else { return }
        if field === searchField {
            clearSearch.isHidden = field.stringValue.isEmpty
            rebuildRows()
            return
        }
        if field === nameField || field === hexField {
            if let parsed = CSSColor.parse(field.stringValue) {
                apply(parsed, from: field === nameField ? .name : .hex)
            }
            return
        }
        guard field.tag >= 1, field.tag <= 6 else { return }
        let numbers = channelFields.map { Double($0.stringValue.trimmingCharacters(in: .whitespaces)) }
        if field.tag <= 3 {
            guard let r = numbers[0], let g = numbers[1], let b = numbers[2] else { return }
            apply(CSSColor(r: r / 255, g: g / 255, b: b / 255, a: color.a), from: .channels)
        } else {
            guard let h = numbers[3], let s = numbers[4], let l = numbers[5] else { return }
            apply(CSSColor(h: h, s: s / 100, l: l / 100, a: color.a), from: .channels)
        }
    }

    func controlTextDidEndEditing(_ notification: Notification) {
        guard let field = notification.object as? NSTextField, field !== searchField else { return }
        apply(color, from: .external)
    }

    @objc private func copyHex() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(color.hexWithAlpha, forType: .string)
    }

    @objc private func swatchPressed(_ sender: SwatchButton) {
        apply(sender.color, from: .swatch)
    }

    @objc private func addSwatch() {
        guard !quickSwatches.contains(color) else { return }
        quickSwatches.append(color)
        if quickSwatches.count > Self.swatchLimit {
            quickSwatches.removeFirst(quickSwatches.count - Self.swatchLimit)
        }
        Store.saveSwatches(quickSwatches)
        rebuildSwatchRow()
    }

    @objc private func removeSwatch(_ sender: NSMenuItem) {
        guard sender.tag < quickSwatches.count else { return }
        quickSwatches.remove(at: sender.tag)
        Store.saveSwatches(quickSwatches)
        rebuildSwatchRow()
    }

    /// Quick swatches, as hex strings in the defaults. The eight defaults are the design's.
    private enum Store {
        static let swatchesKey = "BeaconColorPickerSwatches"
        static let defaultSwatches = ["#663399", "#5a5fea", "#d63b9d", "#f06661", "#ff8a1a", "#ffbe2e", "#37ad72", "#18a8b6"]

        static func swatches() -> [CSSColor] {
            Array((UserDefaults.standard.stringArray(forKey: swatchesKey) ?? defaultSwatches).compactMap(CSSColor.parse).prefix(swatchLimit))
        }

        static func saveSwatches(_ colors: [CSSColor]) {
            UserDefaults.standard.set(colors.map(\.hexWithAlpha), forKey: swatchesKey)
        }
    }
}

// ── views ───────────────────────────────────────────────────────────────────

/// A filter chip: accent when on, grey when off.
private final class Chip: NSButton {
    var isOn = false { didSet { needsDisplay = true } }

    init(title: String) {
        super.init(frame: .zero)
        self.title = title
        isBordered = false
        setButtonType(.momentaryChange)
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    override func draw(_ dirtyRect: NSRect) {
        (isOn ? PickerStyle.accent : PickerStyle.chip).setFill()
        NSBezierPath(roundedRect: bounds, xRadius: 8, yRadius: 8).fill()
        let text = NSAttributedString(string: title, attributes: [
            .font: NSFont.systemFont(ofSize: 12, weight: .medium),
            .foregroundColor: isOn ? NSColor.white : PickerStyle.ink,
        ])
        let size = text.size()
        text.draw(at: NSPoint(x: (bounds.width - size.width) / 2, y: (bounds.height - size.height) / 2))
    }
}

/// A small bordered button with an SF Symbol in it, in the copy button's style.
private final class SymbolButton: NSButton {
    private let symbol: String

    init(symbol: String, frame: NSRect) {
        self.symbol = symbol
        super.init(frame: frame)
        isBordered = false
        title = ""
        setButtonType(.momentaryChange)
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    override func draw(_ dirtyRect: NSRect) {
        let path = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 7, yRadius: 7)
        (isHighlighted ? PickerStyle.chip : PickerStyle.buttonFace).setFill()
        path.fill()
        PickerStyle.fieldBorder.setStroke()
        path.stroke()
        guard let image = NSImage(systemSymbolName: symbol, accessibilityDescription: toolTip)?
            .withSymbolConfiguration(NSImage.SymbolConfiguration(pointSize: 13, weight: .medium)) else { return }
        let tint = PickerStyle.dynamic(PickerStyle.hex(0x59657A), PickerStyle.hex(0xB5BFCC))
        let tinted = NSImage(size: image.size, flipped: false) { r in
            image.draw(in: r)
            tint.set()
            r.fill(using: .sourceAtop)
            return true
        }
        let s = tinted.size
        tinted.draw(in: NSRect(x: (bounds.width - s.width) / 2, y: (bounds.height - s.height) / 2, width: s.width, height: s.height),
                    from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
    }
}

/// The copy-hex button: two overlapping page outlines, as the design draws them.
private final class CopyButton: NSButton {
    override func draw(_ dirtyRect: NSRect) {
        let path = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 7, yRadius: 7)
        (isHighlighted ? PickerStyle.chip : PickerStyle.buttonFace).setFill()
        path.fill()
        PickerStyle.fieldBorder.setStroke()
        path.stroke()
        PickerStyle.dynamic(PickerStyle.hex(0x59657A), PickerStyle.hex(0xB5BFCC)).setStroke()
        for offset in [CGPoint(x: 9.5, y: 7.5), CGPoint(x: 13.5, y: 11.5)] {
            let page = NSBezierPath(roundedRect: NSRect(x: offset.x, y: offset.y, width: 9, height: 11), xRadius: 1, yRadius: 1)
            page.lineWidth = 1.5
            page.stroke()
        }
    }
}

/// One row's box: a hairline card, or the accent outline when it is the current colour.
private final class ColorRowBackground: NSTableRowView {
    override var isFlipped: Bool { true }

    override func drawBackground(in dirtyRect: NSRect) {
        let box = NSRect(x: 0.5, y: 1.5, width: bounds.width - 1, height: 41)
        if isSelected {
            let path = NSBezierPath(roundedRect: box.insetBy(dx: 0.5, dy: 0.5), xRadius: 7, yRadius: 7)
            PickerStyle.dynamic(NSColor.white.withAlphaComponent(0.94), NSColor.white.withAlphaComponent(0.1)).setFill()
            path.fill()
            PickerStyle.accent.setStroke()
            path.lineWidth = 2
            path.stroke()
        } else {
            let path = NSBezierPath(roundedRect: box, xRadius: 7, yRadius: 7)
            PickerStyle.rowBackground.setFill()
            path.fill()
            PickerStyle.rowBorder.setStroke()
            path.stroke()
        }
    }

    override func drawSelection(in dirtyRect: NSRect) {}
}

/// One row of the list: a rounded swatch, the name, and the hex under it.
private final class ColorRowView: NSTableCellView {
    private let swatch = Panel(fill: .black, border: NSColor.black.withAlphaComponent(0.08), radius: 5)
    private let title = PickerStyle.label("", size: 13, weight: .medium, color: PickerStyle.ink)
    private let subtitle = PickerStyle.label("", size: 11, weight: .regular, color: PickerStyle.inkSecondary)

    init() {
        super.init(frame: .zero)
        subtitle.font = .monospacedDigitSystemFont(ofSize: 11, weight: .regular)
        for view in [swatch, title, subtitle] {
            addSubview(view)
        }
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    override var isFlipped: Bool { true }

    override func layout() {
        super.layout()
        swatch.frame = NSRect(x: 8, y: 8, width: 48, height: 28)
        title.frame = NSRect(x: 66, y: 6, width: bounds.width - 76, height: 17)
        subtitle.frame = NSRect(x: 66, y: 23, width: bounds.width - 76, height: 15)
    }

    func show(_ row: ColorPickerWindowController.Row) {
        swatch.fill = row.color.nsColor
        swatch.needsDisplay = true
        title.stringValue = row.title
        subtitle.stringValue = row.subtitle
    }
}
