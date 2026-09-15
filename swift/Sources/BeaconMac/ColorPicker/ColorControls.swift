import AppKit

/// The saturation/value plane: hue across the top, white at the left, black at the bottom.
///
/// Drawn with two Core Graphics gradients over a flat hue fill rather than a bitmap of
/// every pixel, so a drag redraws at the display's rate for the cost of three fills. The
/// knob is a ring, so the colour under it stays visible.
final class SaturationValuePlane: NSView {
    /// Degrees, 0..<360. Kept here rather than derived from the colour, because a grey has
    /// no hue and the plane must not jump to red when the value hits black.
    var hue: Double = 0 { didSet { needsDisplay = true } }
    var saturation: Double = 0 { didSet { needsDisplay = true } }
    var value: Double = 1 { didSet { needsDisplay = true } }
    var onChange: ((_ saturation: Double, _ value: Double) -> Void)?
    var knobRadius: CGFloat = 14

    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }
    /// A press here is a pick, not a grab of the window (which is movable by its background).
    override var mouseDownCanMoveWindow: Bool { false }

    private static let radius: CGFloat = 10

    override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        let path = NSBezierPath(roundedRect: bounds, xRadius: Self.radius, yRadius: Self.radius)
        ctx.saveGState()
        path.addClip()

        CSSColor(h: hue, s: 1, v: 1).nsColor.setFill()
        bounds.fill()

        let space = CGColorSpace(name: CGColorSpace.sRGB)!
        if let toWhite = CGGradient(
            colorsSpace: space,
            colors: [NSColor.white.cgColor, NSColor.white.withAlphaComponent(0).cgColor] as CFArray,
            locations: [0, 1]
        ) {
            ctx.drawLinearGradient(toWhite, start: CGPoint(x: 0, y: 0), end: CGPoint(x: bounds.width, y: 0), options: [])
        }
        if let toBlack = CGGradient(
            colorsSpace: space,
            colors: [NSColor.black.withAlphaComponent(0).cgColor, NSColor.black.cgColor] as CFArray,
            locations: [0, 1]
        ) {
            ctx.drawLinearGradient(toBlack, start: CGPoint(x: 0, y: 0), end: CGPoint(x: 0, y: bounds.height), options: [])
        }
        ctx.restoreGState()

        ColorKnob.draw(at: knobPoint, radius: knobRadius, in: ctx)
    }

    private var knobPoint: CGPoint {
        CGPoint(x: CGFloat(saturation) * bounds.width, y: (1 - CGFloat(value)) * bounds.height)
    }

    private func pick(_ event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        let s = min(max(Double(p.x / bounds.width), 0), 1)
        let v = min(max(1 - Double(p.y / bounds.height), 0), 1)
        saturation = s
        value = v
        onChange?(s, v)
    }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        pick(event)
    }
    override func mouseDragged(with event: NSEvent) { pick(event) }

    /// Arrow keys nudge by 1%, Shift by 10%: a mouse can land near a colour, a keyboard can
    /// land on it.
    override func keyDown(with event: NSEvent) {
        let step = event.modifierFlags.contains(.shift) ? 0.1 : 0.01
        switch event.specialKey {
        case .leftArrow?: saturation = max(0, saturation - step)
        case .rightArrow?: saturation = min(1, saturation + step)
        case .upArrow?: value = min(1, value + step)
        case .downArrow?: value = max(0, value - step)
        default: return super.keyDown(with: event)
        }
        onChange?(saturation, value)
    }
}

/// A vertical strip with a draggable knob: the hue spectrum, or the alpha ramp. One class,
/// because the only difference between them is what the gradient is made of.
final class ColorStrip: NSView {
    enum Kind {
        case hue
        case alpha
    }

    let kind: Kind
    /// 0...1 from the top: hue/360, or 1 - alpha (opaque at the top, as in every picker).
    var position: Double = 0 { didSet { needsDisplay = true } }
    /// The colour the alpha ramp fades; ignored by the hue strip.
    var tint: CSSColor = .black { didSet { needsDisplay = true } }
    var onChange: ((Double) -> Void)?
    /// False draws the strip but ignores the pointer: shown for the design's sake on a
    /// control that has no opacity.
    var isEnabled = true
    var knobRadius: CGFloat = 16

    init(kind: Kind) {
        self.kind = kind
        super.init(frame: .zero)
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }
    override var mouseDownCanMoveWindow: Bool { false }

    override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        let path = NSBezierPath(roundedRect: bounds, xRadius: 10, yRadius: 10)
        ctx.saveGState()
        path.addClip()

        let space = CGColorSpace(name: CGColorSpace.sRGB)!
        let colors: [CGColor]
        switch kind {
        case .hue:
            colors = stride(from: 0.0, through: 360.0, by: 60.0).map { CSSColor(h: $0, s: 1, v: 1).nsColor.cgColor }
        case .alpha:
            ColorKnob.drawCheckerboard(in: bounds, ctx: ctx, cell: 20)
            colors = [tint.opaque.nsColor.cgColor, tint.opaque.nsColor.withAlphaComponent(0).cgColor]
        }
        let locations = (0..<colors.count).map { CGFloat($0) / CGFloat(colors.count - 1) }
        if let gradient = CGGradient(colorsSpace: space, colors: colors as CFArray, locations: locations) {
            ctx.drawLinearGradient(gradient, start: CGPoint(x: 0, y: 0), end: CGPoint(x: 0, y: bounds.height), options: [])
        }
        ctx.restoreGState()

        ColorKnob.draw(at: CGPoint(x: bounds.midX, y: CGFloat(position) * bounds.height), radius: knobRadius, in: ctx)
    }

    private func pick(_ event: NSEvent) {
        guard isEnabled else { return }
        let p = convert(event.locationInWindow, from: nil)
        position = min(max(Double(p.y / bounds.height), 0), 1)
        onChange?(position)
    }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        pick(event)
    }
    override func mouseDragged(with event: NSEvent) { pick(event) }

    override func keyDown(with event: NSEvent) {
        let step = event.modifierFlags.contains(.shift) ? 0.1 : 0.01
        switch event.specialKey {
        case .upArrow?: position = max(0, position - step)
        case .downArrow?: position = min(1, position + step)
        default: return super.keyDown(with: event)
        }
        onChange?(position)
    }
}

/// The ring the plane and the strips share, and the checkerboard alpha is shown against.
enum ColorKnob {
    /// A white ring of the given radius with a soft shadow, as the design draws every knob.
    static func draw(at point: CGPoint, radius: CGFloat, in ctx: CGContext) {
        let outer = CGRect(x: point.x - radius, y: point.y - radius, width: radius * 2, height: radius * 2)
        ctx.saveGState()
        ctx.setShadow(offset: CGSize(width: 0, height: 2), blur: 6, color: NSColor.black.withAlphaComponent(0.35).cgColor)
        ctx.setStrokeColor(NSColor.white.cgColor)
        ctx.setLineWidth(4)
        ctx.strokeEllipse(in: outer)
        ctx.restoreGState()
    }

    static func drawCheckerboard(in rect: CGRect, ctx: CGContext, cell: CGFloat = 6) {
        ctx.setFillColor(NSColor(srgbRed: 0xF0 / 255, green: 0xF0 / 255, blue: 0xF2 / 255, alpha: 1).cgColor)
        ctx.fill(rect)
        ctx.setFillColor(NSColor(srgbRed: 0xB7 / 255, green: 0xB9 / 255, blue: 0xC8 / 255, alpha: 1).cgColor)
        var y = rect.minY
        var row = 0
        while y < rect.maxY {
            var x = rect.minX + (row % 2 == 0 ? 0 : cell)
            while x < rect.maxX {
                ctx.fill(CGRect(x: x, y: y, width: cell, height: cell))
                x += cell * 2
            }
            y += cell
            row += 1
        }
    }
}

/// A round swatch: filled with its colour, ringed when it is the current one. Used by the
/// quick-swatch row and the recent list.
final class SwatchButton: NSButton {
    var color: CSSColor = .black { didSet { needsDisplay = true } }
    var isCurrent = false { didSet { needsDisplay = true } }
    /// Drawn as a "+" outline rather than a colour.
    var isAdder = false { didSet { needsDisplay = true } }

    override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        let circle = bounds.insetBy(dx: 2.5, dy: 2.5)
        if isAdder {
            ctx.setFillColor(PickerStyle.dynamic(PickerStyle.hex(0xEEF1F5), PickerStyle.hex(0x2E343E)).cgColor)
            ctx.fillEllipse(in: circle)
            ctx.setStrokeColor(PickerStyle.dynamic(PickerStyle.hex(0xD1D7E0), PickerStyle.hex(0x3A414C)).cgColor)
            ctx.setLineWidth(1)
            ctx.strokeEllipse(in: circle)
            ctx.setStrokeColor(PickerStyle.inkSecondary.cgColor)
            ctx.setLineWidth(1.5)
            ctx.move(to: CGPoint(x: circle.midX - 6, y: circle.midY))
            ctx.addLine(to: CGPoint(x: circle.midX + 6, y: circle.midY))
            ctx.move(to: CGPoint(x: circle.midX, y: circle.midY - 6))
            ctx.addLine(to: CGPoint(x: circle.midX, y: circle.midY + 6))
            ctx.strokePath()
            return
        }
        if color.a < 1 {
            ctx.saveGState()
            ctx.addEllipse(in: circle)
            ctx.clip()
            ColorKnob.drawCheckerboard(in: circle, ctx: ctx)
            ctx.restoreGState()
        }
        ctx.setFillColor(color.nsColor.cgColor)
        ctx.fillEllipse(in: circle)
        ctx.setStrokeColor(NSColor.black.withAlphaComponent(0.12).cgColor)
        ctx.setLineWidth(1)
        ctx.strokeEllipse(in: circle)
        if isCurrent {
            ctx.setStrokeColor(PickerStyle.dynamic(PickerStyle.hex(0x236CFF), PickerStyle.hex(0x4C8DFF)).cgColor)
            ctx.setLineWidth(3)
            ctx.strokeEllipse(in: bounds.insetBy(dx: 1.5, dy: 1.5))
        }
    }
}
