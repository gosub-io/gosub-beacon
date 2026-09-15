import AppKit

/// A colour the way CSS spells it: sRGB channels in 0...1, plus alpha.
///
/// The picker's own model, kept apart from `NSColor` because an `NSColor` carries a colour
/// space and the page does not: an `<input type=color>` holds six hex digits in sRGB and
/// nothing else, and every conversion here (HSV for the plane, HSL for the fields, names
/// for the list) is defined on those digits.
struct CSSColor: Equatable {
    var r: Double
    var g: Double
    var b: Double
    var a: Double = 1

    static let black = CSSColor(r: 0, g: 0, b: 0)
    static let white = CSSColor(r: 1, g: 1, b: 1)

    init(r: Double, g: Double, b: Double, a: Double = 1) {
        self.r = min(max(r, 0), 1)
        self.g = min(max(g, 0), 1)
        self.b = min(max(b, 0), 1)
        self.a = min(max(a, 0), 1)
    }

    init(r8: Int, g8: Int, b8: Int, a: Double = 1) {
        self.init(r: Double(r8) / 255, g: Double(g8) / 255, b: Double(b8) / 255, a: a)
    }

    private init(rgb: UInt32) {
        self.init(r8: Int((rgb >> 16) & 0xff), g8: Int((rgb >> 8) & 0xff), b8: Int(rgb & 0xff))
    }

    // ── channels ──────────────────────────────────────────────────────────

    var r8: Int { Int((r * 255).rounded()) }
    var g8: Int { Int((g * 255).rounded()) }
    var b8: Int { Int((b * 255).rounded()) }
    var a8: Int { Int((a * 255).rounded()) }

    /// `#rrggbb`, which is what the page's control holds. Alpha is not part of it.
    var hex: String { String(format: "#%02x%02x%02x", r8, g8, b8) }

    /// `#rrggbb`, or `#rrggbbaa` when the colour is not opaque.
    var hexWithAlpha: String {
        a < 1 ? String(format: "#%02x%02x%02x%02x", r8, g8, b8, a8) : hex
    }

    var opaque: CSSColor { CSSColor(r: r, g: g, b: b) }

    var nsColor: NSColor {
        NSColor(srgbRed: CGFloat(r), green: CGFloat(g), blue: CGFloat(b), alpha: CGFloat(a))
    }

    init?(nsColor: NSColor) {
        guard let c = nsColor.usingColorSpace(.sRGB) else { return nil }
        self.init(r: Double(c.redComponent), g: Double(c.greenComponent), b: Double(c.blueComponent), a: Double(c.alphaComponent))
    }

    /// Perceived lightness, for deciding whether a label over the colour should be dark or light.
    var isLight: Bool { 0.299 * r + 0.587 * g + 0.114 * b > 0.6 }

    // ── HSV, for the plane and the hue strip ──────────────────────────────

    /// Hue in degrees (0..<360), saturation and value in 0...1. A grey has no hue of its own
    /// and reports 0; callers that need to keep the strip where it was hold their own hue.
    var hsv: (h: Double, s: Double, v: Double) {
        let maxC = max(r, g, b)
        let minC = min(r, g, b)
        let delta = maxC - minC
        var h = 0.0
        if delta > 0 {
            if maxC == r {
                h = 60 * ((g - b) / delta).truncatingRemainder(dividingBy: 6)
            } else if maxC == g {
                h = 60 * ((b - r) / delta + 2)
            } else {
                h = 60 * ((r - g) / delta + 4)
            }
            if h < 0 { h += 360 }
        }
        let s = maxC == 0 ? 0 : delta / maxC
        return (h, s, maxC)
    }

    init(h: Double, s: Double, v: Double, a: Double = 1) {
        let h = ((h.truncatingRemainder(dividingBy: 360)) + 360).truncatingRemainder(dividingBy: 360)
        let s = min(max(s, 0), 1)
        let v = min(max(v, 0), 1)
        let c = v * s
        let x = c * (1 - abs((h / 60).truncatingRemainder(dividingBy: 2) - 1))
        let m = v - c
        let (r1, g1, b1): (Double, Double, Double)
        switch h {
        case ..<60: (r1, g1, b1) = (c, x, 0)
        case ..<120: (r1, g1, b1) = (x, c, 0)
        case ..<180: (r1, g1, b1) = (0, c, x)
        case ..<240: (r1, g1, b1) = (0, x, c)
        case ..<300: (r1, g1, b1) = (x, 0, c)
        default: (r1, g1, b1) = (c, 0, x)
        }
        self.init(r: r1 + m, g: g1 + m, b: b1 + m, a: a)
    }

    // ── HSL, for the fields ───────────────────────────────────────────────

    /// Hue in degrees, saturation and lightness in 0...1 -- what `hsl()` in a stylesheet takes.
    var hsl: (h: Double, s: Double, l: Double) {
        let (h, sv, v) = hsv
        let l = v * (1 - sv / 2)
        let s = (l == 0 || l == 1) ? 0 : (v - l) / min(l, 1 - l)
        return (h, s, l)
    }

    init(h: Double, s: Double, l: Double, a: Double = 1) {
        let s = min(max(s, 0), 1)
        let l = min(max(l, 0), 1)
        let v = l + s * min(l, 1 - l)
        let sv = v == 0 ? 0 : 2 * (1 - l / v)
        self.init(h: h, s: sv, v: v, a: a)
    }

    // ── parsing ───────────────────────────────────────────────────────────

    /// Anything CSS would accept as a colour: `#639`, `#663399`, `#663399cc`, a name,
    /// `transparent`, `rgb()`/`rgba()` and `hsl()`/`hsla()` in either the comma or the
    /// space-separated syntax. Bare hex digits without the `#` are taken too, as a kindness
    /// to someone typing into the hex field.
    static func parse(_ text: String) -> CSSColor? {
        let s = text.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !s.isEmpty else { return nil }
        if s.hasPrefix("#") {
            return parseHex(String(s.dropFirst()))
        }
        if s == "transparent" {
            return CSSColor(r: 0, g: 0, b: 0, a: 0)
        }
        if let named = namedByName[s] {
            return named
        }
        if let call = parseFunction(s) {
            return call
        }
        return parseHex(s)
    }

    private static func parseHex(_ digits: String) -> CSSColor? {
        guard [3, 4, 6, 8].contains(digits.count), digits.allSatisfy(\.isHexDigit) else { return nil }
        let expanded: String
        if digits.count <= 4 {
            expanded = digits.map { "\($0)\($0)" }.joined()
        } else {
            expanded = digits
        }
        let chars = Array(expanded)
        func byte(_ i: Int) -> Int { Int(String(chars[i..<i + 2]), radix: 16) ?? 0 }
        let a = chars.count == 8 ? Double(byte(6)) / 255 : 1
        return CSSColor(r8: byte(0), g8: byte(2), b8: byte(4), a: a)
    }

    private static func parseFunction(_ s: String) -> CSSColor? {
        guard let open = s.firstIndex(of: "("), s.hasSuffix(")") else { return nil }
        let name = s[..<open].trimmingCharacters(in: .whitespaces)
        let inner = s[s.index(after: open)..<s.index(before: s.endIndex)]
        // `rgb(1 2 3 / 0.5)` and `rgb(1, 2, 3, 0.5)` are the same call.
        let parts = inner.replacingOccurrences(of: "/", with: " ")
            .split(whereSeparator: { $0 == "," || $0.isWhitespace })
            .map(String.init)
        guard parts.count == 3 || parts.count == 4 else { return nil }

        /// A number, or a percentage as a fraction of 1.
        func fraction(_ p: String, of scale: Double) -> Double? {
            if p.hasSuffix("%") {
                return Double(p.dropLast()).map { $0 / 100 }
            }
            return Double(p).map { $0 / scale }
        }
        let alpha = parts.count == 4 ? fraction(parts[3], of: 1) : 1
        guard let alpha else { return nil }

        switch name {
        case "rgb", "rgba":
            guard let r = fraction(parts[0], of: 255), let g = fraction(parts[1], of: 255), let b = fraction(parts[2], of: 255)
            else { return nil }
            return CSSColor(r: r, g: g, b: b, a: alpha)
        case "hsl", "hsla":
            let hueText = parts[0].hasSuffix("deg") ? String(parts[0].dropLast(3)) : parts[0]
            guard let h = Double(hueText), let sat = fraction(parts[1], of: 100), let l = fraction(parts[2], of: 100)
            else { return nil }
            return CSSColor(h: h, s: sat, l: l, a: alpha)
        default:
            return nil
        }
    }

    // ── names ─────────────────────────────────────────────────────────────

    /// A CSS named colour.
    struct Named: Equatable {
        let name: String
        let color: CSSColor
        /// Other names for the same six digits: `grey` for `gray`, `cyan` for `aqua`.
        let aliases: [String]
    }

    /// The 148 named colours of CSS Color 4, alphabetical, the way a stylesheet spells them.
    /// Aliases (`gray`/`grey`, `aqua`/`cyan`, `fuchsia`/`magenta`) are separate entries, as
    /// they are in the spec's table, and each knows the others.
    static let named: [Named] = {
        var byHex: [String: [String]] = [:]
        for (name, rgb) in namedTable {
            byHex[CSSColor(rgb: rgb).hex, default: []].append(name)
        }
        return namedTable.map { name, rgb in
            let color = CSSColor(rgb: rgb)
            return Named(name: name, color: color, aliases: (byHex[color.hex] ?? []).filter { $0 != name })
        }
    }()

    private static let namedByName: [String: CSSColor] = {
        Dictionary(uniqueKeysWithValues: namedTable.map { ($0.0, CSSColor(rgb: $0.1)) })
    }()

    /// The entry whose six digits these are, when there is one. The first of a pair of
    /// aliases, so `#808080` is `gray` and `grey` is its alias.
    var cssName: Named? {
        guard a == 1 else { return nil }
        let h = hex
        return Self.named.first { $0.color.hex == h }
    }

    /// The named colour nearest to this one, with how far off it is: 0 is exact, and a few
    /// units is what most people would still call by that name.
    func nearestName() -> (named: Named, distance: Double) {
        var best = (Self.named[0], Double.infinity)
        for entry in Self.named {
            let d = distance(to: entry.color)
            if d < best.1 {
                best = (entry, d)
            }
        }
        return best
    }

    /// Perceptual-ish distance ("redmean"): weighted RGB that agrees with the eye far better
    /// than a plain Euclidean distance does, for a fraction of the cost of going through Lab.
    func distance(to other: CSSColor) -> Double {
        let rMean = (r + other.r) / 2
        let dr = (r - other.r) * 255
        let dg = (g - other.g) * 255
        let db = (b - other.b) * 255
        return ((2 + rMean) * dr * dr + 4 * dg * dg + (2 + (1 - rMean)) * db * db).squareRoot()
    }

    /// The CSS system colours (`Canvas`, `Highlight`, `AccentColor`, …), resolved against the
    /// running desktop. Asked for each time rather than stored: the accent colour and the
    /// appearance can both change while the picker is up.
    static func systemColors() -> [Named] {
        let table: [(String, NSColor)] = [
            ("AccentColor", .controlAccentColor),
            ("AccentColorText", .white),
            ("ActiveText", .systemRed),
            ("ButtonBorder", .separatorColor),
            ("ButtonFace", .controlColor),
            ("ButtonText", .controlTextColor),
            ("Canvas", .textBackgroundColor),
            ("CanvasText", .textColor),
            ("Field", .textBackgroundColor),
            ("FieldText", .textColor),
            ("GrayText", .disabledControlTextColor),
            ("Highlight", .selectedContentBackgroundColor),
            ("HighlightText", .alternateSelectedControlTextColor),
            ("LinkText", .linkColor),
            ("Mark", .systemYellow),
            ("MarkText", .black),
            ("SelectedItem", .selectedControlColor),
            ("SelectedItemText", .selectedControlTextColor),
            ("VisitedText", .systemPurple),
        ]
        return table.compactMap { name, ns in
            CSSColor(nsColor: ns).map { Named(name: name, color: $0.opaque, aliases: []) }
        }
    }

    // swiftlint:disable line_length
    private static let namedTable: [(String, UInt32)] = [
        ("aliceblue", 0xf0f8ff), ("antiquewhite", 0xfaebd7), ("aqua", 0x00ffff), ("aquamarine", 0x7fffd4),
        ("azure", 0xf0ffff), ("beige", 0xf5f5dc), ("bisque", 0xffe4c4), ("black", 0x000000),
        ("blanchedalmond", 0xffebcd), ("blue", 0x0000ff), ("blueviolet", 0x8a2be2), ("brown", 0xa52a2a),
        ("burlywood", 0xdeb887), ("cadetblue", 0x5f9ea0), ("chartreuse", 0x7fff00), ("chocolate", 0xd2691e),
        ("coral", 0xff7f50), ("cornflowerblue", 0x6495ed), ("cornsilk", 0xfff8dc), ("crimson", 0xdc143c),
        ("cyan", 0x00ffff), ("darkblue", 0x00008b), ("darkcyan", 0x008b8b), ("darkgoldenrod", 0xb8860b),
        ("darkgray", 0xa9a9a9), ("darkgreen", 0x006400), ("darkgrey", 0xa9a9a9), ("darkkhaki", 0xbdb76b),
        ("darkmagenta", 0x8b008b), ("darkolivegreen", 0x556b2f), ("darkorange", 0xff8c00), ("darkorchid", 0x9932cc),
        ("darkred", 0x8b0000), ("darksalmon", 0xe9967a), ("darkseagreen", 0x8fbc8f), ("darkslateblue", 0x483d8b),
        ("darkslategray", 0x2f4f4f), ("darkslategrey", 0x2f4f4f), ("darkturquoise", 0x00ced1), ("darkviolet", 0x9400d3),
        ("deeppink", 0xff1493), ("deepskyblue", 0x00bfff), ("dimgray", 0x696969), ("dimgrey", 0x696969),
        ("dodgerblue", 0x1e90ff), ("firebrick", 0xb22222), ("floralwhite", 0xfffaf0), ("forestgreen", 0x228b22),
        ("fuchsia", 0xff00ff), ("gainsboro", 0xdcdcdc), ("ghostwhite", 0xf8f8ff), ("gold", 0xffd700),
        ("goldenrod", 0xdaa520), ("gray", 0x808080), ("green", 0x008000), ("greenyellow", 0xadff2f),
        ("grey", 0x808080), ("honeydew", 0xf0fff0), ("hotpink", 0xff69b4), ("indianred", 0xcd5c5c),
        ("indigo", 0x4b0082), ("ivory", 0xfffff0), ("khaki", 0xf0e68c), ("lavender", 0xe6e6fa),
        ("lavenderblush", 0xfff0f5), ("lawngreen", 0x7cfc00), ("lemonchiffon", 0xfffacd), ("lightblue", 0xadd8e6),
        ("lightcoral", 0xf08080), ("lightcyan", 0xe0ffff), ("lightgoldenrodyellow", 0xfafad2), ("lightgray", 0xd3d3d3),
        ("lightgreen", 0x90ee90), ("lightgrey", 0xd3d3d3), ("lightpink", 0xffb6c1), ("lightsalmon", 0xffa07a),
        ("lightseagreen", 0x20b2aa), ("lightskyblue", 0x87cefa), ("lightslategray", 0x778899), ("lightslategrey", 0x778899),
        ("lightsteelblue", 0xb0c4de), ("lightyellow", 0xffffe0), ("lime", 0x00ff00), ("limegreen", 0x32cd32),
        ("linen", 0xfaf0e6), ("magenta", 0xff00ff), ("maroon", 0x800000), ("mediumaquamarine", 0x66cdaa),
        ("mediumblue", 0x0000cd), ("mediumorchid", 0xba55d3), ("mediumpurple", 0x9370db), ("mediumseagreen", 0x3cb371),
        ("mediumslateblue", 0x7b68ee), ("mediumspringgreen", 0x00fa9a), ("mediumturquoise", 0x48d1cc), ("mediumvioletred", 0xc71585),
        ("midnightblue", 0x191970), ("mintcream", 0xf5fffa), ("mistyrose", 0xffe4e1), ("moccasin", 0xffe4b5),
        ("navajowhite", 0xffdead), ("navy", 0x000080), ("oldlace", 0xfdf5e6), ("olive", 0x808000),
        ("olivedrab", 0x6b8e23), ("orange", 0xffa500), ("orangered", 0xff4500), ("orchid", 0xda70d6),
        ("palegoldenrod", 0xeee8aa), ("palegreen", 0x98fb98), ("paleturquoise", 0xafeeee), ("palevioletred", 0xdb7093),
        ("papayawhip", 0xffefd5), ("peachpuff", 0xffdab9), ("peru", 0xcd853f), ("pink", 0xffc0cb),
        ("plum", 0xdda0dd), ("powderblue", 0xb0e0e6), ("purple", 0x800080), ("rebeccapurple", 0x663399),
        ("red", 0xff0000), ("rosybrown", 0xbc8f8f), ("royalblue", 0x4169e1), ("saddlebrown", 0x8b4513),
        ("salmon", 0xfa8072), ("sandybrown", 0xf4a460), ("seagreen", 0x2e8b57), ("seashell", 0xfff5ee),
        ("sienna", 0xa0522d), ("silver", 0xc0c0c0), ("skyblue", 0x87ceeb), ("slateblue", 0x6a5acd),
        ("slategray", 0x708090), ("slategrey", 0x708090), ("snow", 0xfffafa), ("springgreen", 0x00ff7f),
        ("steelblue", 0x4682b4), ("tan", 0xd2b48c), ("teal", 0x008080), ("thistle", 0xd8bfd8),
        ("tomato", 0xff6347), ("turquoise", 0x40e0d0), ("violet", 0xee82ee), ("wheat", 0xf5deb3),
        ("white", 0xffffff), ("whitesmoke", 0xf5f5f5), ("yellow", 0xffff00), ("yellowgreen", 0x9acd32),
    ]
    // swiftlint:enable line_length
}
