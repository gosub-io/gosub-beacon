import AppKit

/// The colours and small drawings the picker windows share, transcribed from the Figma
/// export (`Picker – Full Design.svg`, 1396 × 910 pt). The design was drawn for the light
/// appearance; each colour carries a dark counterpart so the windows are not blinding at
/// night, chosen to keep the same contrast rather than to match anything.
enum PickerStyle {
    static func dynamic(_ light: NSColor, _ dark: NSColor) -> NSColor {
        NSColor(name: nil) { appearance in
            appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? dark : light
        }
    }

    static func hex(_ rgb: UInt32, alpha: CGFloat = 1) -> NSColor {
        NSColor(
            srgbRed: CGFloat((rgb >> 16) & 0xff) / 255,
            green: CGFloat((rgb >> 8) & 0xff) / 255,
            blue: CGFloat(rgb & 0xff) / 255,
            alpha: alpha
        )
    }

    /// The window: #EAF0F7 with a #D8E0EA hairline.
    static let windowBackground = dynamic(hex(0xEAF0F7), hex(0x1E2229))
    static let windowBorder = dynamic(hex(0xD8E0EA), hex(0x2E333C))
    /// The sidebar: #E8EEF6.
    static let sidebar = dynamic(hex(0xE8EEF6), hex(0x232830))
    /// A card: white at 72% over the window.
    static let card = dynamic(NSColor.white.withAlphaComponent(0.72), NSColor.white.withAlphaComponent(0.06))
    /// The selected sidebar item: #DCE7F8.
    static let sidebarSelection = dynamic(hex(0xDCE7F8), hex(0x2C3A55))
    /// The accent the design uses for selection, chips and the Select button.
    static let accent = dynamic(hex(0x1B6DFF), hex(0x4C8DFF))
    static let selectButton = dynamic(hex(0x0D6FFF), hex(0x2F7BFF))
    static let ink = dynamic(hex(0x1A2233), hex(0xE6EAF0))
    static let inkSecondary = dynamic(hex(0x6B7A90), hex(0x9AA5B5))
    static let brand = dynamic(hex(0x243A68), hex(0xC9D6F0))
    static let fieldBorder = dynamic(hex(0xC9D1DD), hex(0x3A414C))
    static let fieldBackground = dynamic(NSColor.white, hex(0x2A2F38))
    static let chip = dynamic(hex(0xEEF1F6), hex(0x2E343E))
    static let rowBorder = dynamic(hex(0xDCE1E9), hex(0x363C47))
    static let rowBackground = dynamic(NSColor.white.withAlphaComponent(0.55), NSColor.white.withAlphaComponent(0.04))
    static let divider = dynamic(hex(0xD5DCE6), hex(0x343A44))
    static let buttonFace = dynamic(hex(0xF5F7FA), hex(0x2E343E))
    static let cancelFace = dynamic(hex(0xE4E9F1), hex(0x343A45))
    /// The shared shell (from the time-picker design): window #EFF2F8, sidebar #E4E9F3,
    /// cards a soft white, quick-select rows a shade under the card.
    static let shellBackground = dynamic(hex(0xEFF2F8), hex(0x1E2229))
    static let shellSidebar = dynamic(hex(0xE4E9F3), hex(0x232830))
    static let shellCard = dynamic(NSColor.white.withAlphaComponent(0.6), NSColor.white.withAlphaComponent(0.06))
    static let shellCardBorder = dynamic(hex(0xE1E6EE), hex(0x2E333C))
    static let quickRow = dynamic(hex(0xEEF1F7), hex(0x2A2F38))

    /// The lighthouse for the foot of a sidebar, cut from the design (198 × 260).
    static let lighthouse: NSImage? = {
        guard let url = AboutWindowController.resources.url(forResource: "picker-sidebar", withExtension: "png") else {
            NSLog("beacon: picker artwork 'picker-sidebar.png' is not in the bundle")
            return nil
        }
        return NSImage(contentsOf: url)
    }()

    /// The gosub submarine, as the design draws it: hull, tower, periscope, three portholes.
    /// Drawn with `origin` at the hull's top-left in a flipped view, at the design's size
    /// (48 × 30 including the periscope).
    static func drawSubmarine(at origin: NSPoint, scale k: CGFloat = 1, color: NSColor, portholes: NSColor) {
        func r(_ x: CGFloat, _ y: CGFloat, _ w: CGFloat, _ h: CGFloat) -> NSRect {
            NSRect(x: origin.x + x * k, y: origin.y + y * k, width: w * k, height: h * k)
        }
        color.setFill()
        NSBezierPath(roundedRect: r(0, 12, 48, 18), xRadius: 9 * k, yRadius: 9 * k).fill()
        NSBezierPath(roundedRect: r(19, 6, 14, 8), xRadius: 4 * k, yRadius: 4 * k).fill()
        NSBezierPath(roundedRect: r(25, 0, 4, 8), xRadius: 2 * k, yRadius: 2 * k).fill()
        portholes.setFill()
        for x in [14.5, 23.5, 32.5] as [CGFloat] {
            NSBezierPath(ovalIn: r(x - 2.5, 17, 5, 5)).fill()
        }
    }

    /// The rainbow disc the sidebar's Picker item wears: a conic sweep with a white core.
    static func drawHueDisc(center: NSPoint, radius: CGFloat, in ctx: CGContext) {
        let steps = 36
        for i in 0..<steps {
            let start = CGFloat(i) / CGFloat(steps) * 2 * .pi
            let end = CGFloat(i + 1) / CGFloat(steps) * 2 * .pi + 0.02
            ctx.setFillColor(CSSColor(h: Double(i) / Double(steps) * 360, s: 0.85, v: 1).nsColor.cgColor)
            ctx.move(to: center)
            ctx.addArc(center: center, radius: radius, startAngle: start, endAngle: end, clockwise: false)
            ctx.closePath()
            ctx.fillPath()
        }
        ctx.setFillColor(NSColor.white.cgColor)
        ctx.fillEllipse(in: NSRect(x: center.x - radius * 0.39, y: center.y - radius * 0.39, width: radius * 0.78, height: radius * 0.78))
    }

    static func label(_ text: String, size: CGFloat, weight: NSFont.Weight, color: NSColor, align: NSTextAlignment = .left) -> NSTextField {
        let field = NSTextField(labelWithString: text)
        field.font = .systemFont(ofSize: size, weight: weight)
        field.textColor = color
        field.alignment = align
        field.lineBreakMode = .byTruncatingTail
        return field
    }

    /// A bordered text field in the design's style: white, #C9D1DD hairline, 8.5 radius.
    static func field(placeholder: String = "", size: CGFloat = 16, monospaced: Bool = false, centered: Bool = false) -> NSTextField {
        let field = NSTextField()
        field.placeholderString = placeholder
        field.isBezeled = false
        field.drawsBackground = false
        field.focusRingType = .none
        field.font = monospaced ? .monospacedSystemFont(ofSize: size, weight: .regular) : .systemFont(ofSize: size)
        field.textColor = ink
        field.alignment = centered ? .center : .left
        field.lineBreakMode = .byClipping
        return field
    }
}

/// Top-left origin, so frames read like the design they were transcribed from.
final class FlippedRoot: NSView {
    override var isFlipped: Bool { true }
}

/// A rounded, filled, optionally bordered rectangle whose colours follow the appearance.
class Panel: NSView {
    var fill: NSColor
    var border: NSColor?
    var borderWidth: CGFloat = 1
    var radius: CGFloat
    /// Settable, unlike a plain view's, so panels can be found and removed by tag.
    private var storedTag = 0
    override var tag: Int {
        get { storedTag }
        set { storedTag = newValue }
    }

    init(fill: NSColor, border: NSColor? = nil, radius: CGFloat) {
        self.fill = fill
        self.border = border
        self.radius = radius
        super.init(frame: .zero)
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    override var isFlipped: Bool { true }
    /// A press on a card, or on anything drawn in one, must not drag the window: the plane,
    /// the strips and the clock all take drags of their own.
    override var mouseDownCanMoveWindow: Bool { false }

    override func draw(_ dirtyRect: NSRect) {
        let inset = border == nil ? 0 : borderWidth / 2
        let path = NSBezierPath(roundedRect: bounds.insetBy(dx: inset, dy: inset), xRadius: radius, yRadius: radius)
        fill.setFill()
        path.fill()
        if let border {
            border.setStroke()
            path.lineWidth = borderWidth
            path.stroke()
        }
    }
}

/// A text field drawn inside a `Panel` border, since the design's fields are hairline boxes
/// rather than AppKit's bezels.
final class BorderedField: Panel {
    let field: NSTextField

    init(_ field: NSTextField) {
        self.field = field
        super.init(fill: PickerStyle.fieldBackground, border: PickerStyle.fieldBorder, radius: 8.5)
        addSubview(field)
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    override func layout() {
        super.layout()
        let height = field.font.map { ceil($0.ascender - $0.descender) + 2 } ?? 20
        // Narrow boxes (the RGB/HSL cells) keep their padding small so three digits fit.
        let inset: CGFloat = bounds.width < 70 ? 4 : 12
        field.frame = NSRect(x: inset, y: (bounds.height - height) / 2, width: bounds.width - inset * 2, height: height)
    }
}

/// A flat button drawn to the design: a rounded fill, a hairline, a title.
final class DesignButton: NSButton {
    var fill: NSColor = PickerStyle.buttonFace
    var border: NSColor? = PickerStyle.fieldBorder
    var titleColor: NSColor = PickerStyle.ink
    var titleSize: CGFloat = 15
    var titleWeight: NSFont.Weight = .medium
    var radius: CGFloat = 8.5
    /// A chevron at the right edge, as on "Show all 148 CSS colors".
    var showsChevron = false

    override func draw(_ dirtyRect: NSRect) {
        let inset: CGFloat = border == nil ? 0 : 0.5
        let path = NSBezierPath(roundedRect: bounds.insetBy(dx: inset, dy: inset), xRadius: radius, yRadius: radius)
        (isHighlighted ? fill.blended(withFraction: 0.12, of: .black) ?? fill : fill).setFill()
        path.fill()
        if let border {
            border.setStroke()
            path.stroke()
        }
        let attributes: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: titleSize, weight: titleWeight),
            .foregroundColor: titleColor,
        ]
        let text = NSAttributedString(string: title, attributes: attributes)
        let size = text.size()
        let x = showsChevron ? 18 : (bounds.width - size.width) / 2
        text.draw(at: NSPoint(x: x, y: (bounds.height - size.height) / 2))
        if showsChevron {
            let chevron = NSAttributedString(string: "›", attributes: [
                .font: NSFont.systemFont(ofSize: 20, weight: .regular), .foregroundColor: PickerStyle.inkSecondary,
            ])
            let cs = chevron.size()
            chevron.draw(at: NSPoint(x: bounds.width - 18 - cs.width, y: (bounds.height - cs.height) / 2 - 1))
        }
    }
}
