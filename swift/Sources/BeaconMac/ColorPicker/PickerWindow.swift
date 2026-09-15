import AppKit

/// The picker a page's date, time, `datetime-local`, month or week input opens, built to
/// the time-picker screens (`Picker – Time Select 12h/24h.png`, `Picker - Quick Select.png`,
/// 983 × 910 pt): the large shell, with the sidebar offering the input's own section (Time,
/// Date, Month or Week) and Quick select, a 12 h / 24 h toggle for the time kinds, and one
/// main card. A `datetime-local` input gets Date and Time sections.
///
/// Values cross the seam in the HTML forms' ISO shapes (`2026-09-15`, `10:35`,
/// `2026-09-15T10:35`, `2026-09`, `2026-W38`); every change is reported live through
/// `onChange`, and `onFinish` fires once with the final value, or nil on cancel.
final class PickerWindowController: PickerShellWindowController {
    var onChange: ((String) -> Void)?
    var onFinish: ((String?) -> Void)?

    private enum Section {
        case date, time, quick
    }

    private let kind: Browser.PickerKind
    private var value: PickerValue
    private let limits: PickerBounds
    private let sections: [Section]
    private let baseTitle: String

    private lazy var datePage = DatePage(weeks: kind == .week)
    private let monthYearPage = MonthYearPage()
    /// The Date section drilled down into the month & year view, and the date it had when
    /// it did, for Cancel to put back.
    private var showingMonthYear = false
    private var beforeDrillDown: PickerValue?
    private let timePage = FlippedRoot()
    private let quickPage = QuickSelectPage()
    private let clock = ClockFaceView()
    private let meridiem = MeridiemToggle()
    private let hourField = StepperField()
    private let minuteField = StepperField()
    private let secondField = StepperField()
    private let captionIcon = NSImageView()
    private let captionTime = PickerStyle.label("", size: 18, weight: .regular, color: PickerStyle.inkSecondary)
    private let captionPart = PickerStyle.label("", size: 18, weight: .regular, color: PickerStyle.inkSecondary)
    private let formatToggle = ClockFormatToggle()
    private var showsSeconds = false

    init(kind: Browser.PickerKind, value: String, min: String?, max: String?, step: String?) {
        self.kind = kind
        self.limits = PickerBounds(kind: kind, min: min, max: max, step: step)
        self.value = PickerValue.parse(value, kind: kind) ?? PickerValue.now(kind: kind, step: limits.step)
        let nav: [NavItem]
        switch kind {
        case .time:
            sections = [.time, .quick]
            nav = [NavItem(symbol: "clock", title: "Time"), NavItem(symbol: "bolt.fill", title: "Quick select")]
            baseTitle = "Select a time"
        case .dateTimeLocal:
            sections = [.date, .time, .quick]
            nav = [NavItem(symbol: "calendar", title: "Date"), NavItem(symbol: "clock", title: "Time"), NavItem(symbol: "bolt.fill", title: "Quick select")]
            baseTitle = "Select a date and time"
        case .month:
            sections = [.date, .quick]
            nav = [NavItem(symbol: "calendar", title: "Month"), NavItem(symbol: "bolt.fill", title: "Quick select")]
            baseTitle = "Select a month"
        case .week:
            sections = [.date, .quick]
            nav = [NavItem(symbol: "calendar", title: "Week"), NavItem(symbol: "bolt.fill", title: "Quick select")]
            baseTitle = "Select a week"
        default:
            sections = [.date, .quick]
            nav = [NavItem(symbol: "calendar", title: "Date"), NavItem(symbol: "bolt.fill", title: "Quick select")]
            baseTitle = "Select a date"
        }
        super.init(
            size: NSSize(width: 983, height: 910),
            title: baseTitle,
            nav: nav,
            mainCard: NSRect(x: 275, y: 18, width: 690, height: 807),
            sideCard: nil,
            metrics: .large
        )
        helpURL = URL(string: kind == .time ? "https://developer.mozilla.org/en-US/docs/Web/HTML/Element/input/time" : "https://developer.mozilla.org/en-US/docs/Web/HTML/Element/input/date")
        showingMonthYear = kind == .month
        build()
        show()
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    /// In the month & year drill-down the foot buttons belong to that view: OK keeps the
    /// month and year and returns to the calendar, Cancel returns with them as they were.
    /// The picker itself is committed from the calendar only.
    private var inDrillDown: Bool { section == .date && showingMonthYear && kind != .month }

    override func okPressed() {
        guard inDrillDown else { return super.okPressed() }
        showingMonthYear = false
        show()
    }

    override func cancelPressed() {
        guard inDrillDown else { return super.cancelPressed() }
        if let before = beforeDrillDown {
            value = before
            changed()
        }
        showingMonthYear = false
        show()
    }

    override func didFinish(ok: Bool) {
        onFinish?(ok ? value.iso(for: kind) : nil)
    }

    override func detach() {
        onChange = nil
        onFinish = nil
        super.detach()
    }

    private var section: Section { sections[selectedNav] }
    private var hasTime: Bool { kind == .time || kind == .dateTimeLocal }

    // ── building ──────────────────────────────────────────────────────────

    private func build() {
        onNav = { [weak self] in
            guard let self else { return }
            if self.kind != .month { self.showingMonthYear = false }
            self.show()
        }

        // The 12 h / 24 h toggle, top right of the card on every section of a time kind.
        formatToggle.frame = NSRect(x: 568, y: 30, width: 106, height: 27)
        formatToggle.onChange = { [weak self] in
            guard let self else { return }
            self.refreshTime()
            self.quickPage.reload()
        }

        // Date section: the calendar with its steppers and caption, and the month & year
        // view it drills down into.
        datePage.frame = mainCard.bounds
        datePage.limits = limits
        datePage.onChange = { [weak self] picked in
            guard let self else { return }
            (self.value.year, self.value.month, self.value.day) = (picked.year, picked.month, picked.day)
            self.datePage.value = self.value
            self.changed()
        }
        datePage.onOpenMonthYear = { [weak self] in
            guard let self else { return }
            self.beforeDrillDown = self.value
            self.showingMonthYear = true
            self.show()
        }
        monthYearPage.frame = mainCard.bounds
        monthYearPage.limits = limits
        monthYearPage.onChange = { [weak self] year, month, _ in
            guard let self else { return }
            (self.value.year, self.value.month) = (year, month)
            let days = PickerValue.daysIn(year: year, month: month)
            if self.value.day > days { self.value.day = days }
            if self.kind == .month { self.value.day = 1 }
            self.changed()
            self.monthYearPage.caption = self.kind == .month ? self.monthCaption : ""
            // The view stays up so month and year can both be chosen; OK and Cancel at the
            // foot return to the calendar, applying or discarding them.
            self.show()
        }

        // Time section, as the screens draw it.
        timePage.frame = mainCard.bounds
        clock.frame = NSRect(x: 71, y: 127, width: 448, height: 448)
        clock.onChange = { [weak self] hour, minute in
            guard let self else { return }
            (self.value.hour, self.value.minute) = (hour, minute)
            self.timeChanged()
        }
        timePage.addSubview(clock)
        meridiem.frame = NSRect(x: 549, y: 198, width: 81, height: 258)
        meridiem.onChange = { [weak self] top in
            guard let self else { return }
            // 12 h: the top half is PM; 24 h: the top half is 00-11.
            let pm = PickerValue.uses12Hour ? top : !top
            self.value.hour = (self.value.hour % 12) + (pm ? 12 : 0)
            self.timeChanged()
        }
        timePage.addSubview(meridiem)

        let labelColor = PickerStyle.brand
        let fields: [(StepperField, String, CGFloat, CGFloat)] = [(hourField, "Hour", 109, 181), (minuteField, "Minute", 329, 204), (secondField, "Second", 549, 0)]
        for (field, name, x, w) in fields {
            let label = PickerStyle.label(name, size: 16, weight: .semibold, color: labelColor)
            label.frame = NSRect(x: x, y: 615, width: 120, height: 20)
            label.tag = 200
            timePage.addSubview(label)
            field.frame = NSRect(x: x, y: 647, width: w, height: 60)
            field.onChange = { [weak self] _ in self?.fieldChanged() }
            timePage.addSubview(field)
            if w == 0 {
                label.isHidden = true
                field.isHidden = true
            }
        }
        let colon = PickerStyle.label(":", size: 26, weight: .regular, color: PickerStyle.inkSecondary, align: .center)
        colon.frame = NSRect(x: 292, y: 660, width: 26, height: 34)
        timePage.addSubview(colon)

        showsSeconds = limits.step.map { $0 < 60 } ?? (value.second != nil)
        if showsSeconds {
            // Three fields share the row; the labels above them follow.
            let xs: [CGFloat] = [109, 279, 449]
            for (i, field) in [hourField, minuteField, secondField].enumerated() {
                field.frame = NSRect(x: xs[i], y: 647, width: 140, height: 60)
                field.isHidden = false
            }
            for (i, label) in timePage.subviews.compactMap({ $0 as? NSTextField }).filter({ $0.tag == 200 }).enumerated() where i < 3 {
                label.frame.origin.x = xs[i]
                label.isHidden = false
            }
            colon.isHidden = true
            if value.second == nil { value.second = 0 }
        }
        hourField.wraps = true
        minuteField.range = 0...59
        minuteField.wraps = true
        minuteField.step = limits.step.map { $0 >= 60 ? $0 / 60 : 1 } ?? 1
        secondField.range = 0...59
        secondField.wraps = true
        clock.minuteStep = minuteField.step

        captionIcon.frame = NSRect(x: 42, y: 728, width: 40, height: 40)
        timePage.addSubview(captionIcon)
        captionTime.frame = NSRect(x: 108, y: 728, width: 500, height: 24)
        timePage.addSubview(captionTime)
        captionPart.frame = NSRect(x: 108, y: 754, width: 500, height: 24)
        timePage.addSubview(captionPart)

        // Quick select page.
        quickPage.frame = mainCard.bounds
        quickPage.onPick = { [weak self] pick in
            guard let self else { return }
            pick.apply(&self.value)
            self.changed()
            if self.kind == .dateTimeLocal {
                // Both a day and a time are picked here, so the page stays; its caption
                // shows what they add up to.
                self.quickPage.caption = self.combinedCaption
                return
            }
            // Back to the section the pick belongs to, with the choice showing; OK commits.
            self.selectNav(0)
            self.show()
        }
    }

    private func show() {
        mainCard.subviews.forEach { $0.removeFromSuperview() }
        formatToggle.removeFromSuperview()
        switch section {
        case .date:
            if showingMonthYear {
                setTitle(kind == .month ? baseTitle : "Select month and year")
                monthYearPage.set(year: value.year, month: value.month)
                monthYearPage.caption = kind == .month ? monthCaption : ""
                mainCard.addSubview(monthYearPage)
            } else {
                setTitle(kind == .dateTimeLocal ? "Select a date" : baseTitle)
                datePage.value = value
                mainCard.addSubview(datePage)
            }
        case .time:
            setTitle(kind == .dateTimeLocal ? "Select a time" : baseTitle)
            mainCard.addSubview(timePage)
            refreshTime()
        case .quick:
            setTitle("Quick select")
            quickPage.groups = QuickPick.groups(for: kind, timeSection: kind == .time)
            quickPage.subtitle = kind == .time ? "Choose a useful time without setting the clock manually." : ""
            quickPage.caption = kind == .dateTimeLocal ? combinedCaption : ""
            mainCard.addSubview(quickPage)
        }
        if kind == .dateTimeLocal {
            datePage.captionSuffix = " at \(PickerValue.clockString(hour: value.hour, minute: value.minute))"
        }
        if hasTime, section != .date {
            mainCard.addSubview(formatToggle)
        }
    }

    /// "Selected month: September 2026", for a month picker.
    private var monthCaption: String {
        let f = DateFormatter()
        f.calendar = PickerValue.gregorian
        f.setLocalizedDateFormatFromTemplate("MMMM yyyy")
        return "Selected month: " + (value.date.map { f.string(from: $0) } ?? "")
    }

    /// "Tuesday, 15 September 2026 at 10:30 PM", for the pages of a datetime-local picker.
    private var combinedCaption: String {
        let f = DateFormatter()
        f.calendar = PickerValue.gregorian
        f.setLocalizedDateFormatFromTemplate("EEEE d MMMM yyyy")
        let day = value.date.map { f.string(from: $0) } ?? ""
        return "Selected: \(day) at \(PickerValue.clockString(hour: value.hour, minute: value.minute))"
    }

    private func changed() {
        onChange?(value.iso(for: kind))
    }

    private func timeChanged() {
        refreshTime()
        changed()
    }

    private func fieldChanged() {
        let h = hourField.value
        value.hour = PickerValue.uses12Hour ? (h % 12) + (value.hour >= 12 ? 12 : 0) : h
        value.minute = minuteField.value
        if showsSeconds { value.second = secondField.value }
        timeChanged()
    }

    private func refreshTime() {
        let twelve = PickerValue.uses12Hour
        clock.twentyFourHour = !twelve
        clock.set(hour: value.hour, minute: value.minute)
        meridiem.twelveHour = twelve
        meridiem.isTop = twelve ? value.hour >= 12 : value.hour < 12
        hourField.range = twelve ? 1...12 : 0...23
        hourField.value = twelve ? ((value.hour % 12 == 0) ? 12 : value.hour % 12) : value.hour
        minuteField.value = value.minute
        secondField.value = value.second ?? 0
        captionTime.stringValue = kind == .dateTimeLocal
            ? combinedCaption
            : "Selected time: \(PickerValue.clockString(hour: value.hour, minute: value.minute))"
        let part = Daypart.of(hour: value.hour)
        captionPart.stringValue = "\(part.name) (\(part.range))"
        captionIcon.image = part.image(pointSize: 30)
    }
}

// ── the value ───────────────────────────────────────────────────────────────

/// What the picker edits: enough calendar fields for every kind, formatted per kind.
struct PickerValue {
    var year: Int
    var month: Int
    var day: Int
    var hour: Int
    var minute: Int
    var second: Int?

    static let gregorian: Calendar = {
        var c = Calendar(identifier: .gregorian)
        c.timeZone = .current
        return c
    }()

    static let iso: Calendar = {
        var c = Calendar(identifier: .iso8601)
        c.timeZone = .current
        return c
    }()

    static func daysIn(year: Int, month: Int) -> Int {
        var comps = DateComponents()
        (comps.year, comps.month, comps.day) = (year, month, 1)
        guard let first = gregorian.date(from: comps), let range = gregorian.range(of: .day, in: .month, for: first) else { return 31 }
        return range.count
    }

    /// The picker's 12 h / 24 h toggle, remembered across pickers; nil follows the locale.
    static var clockFormat: Int? {
        get {
            let stored = UserDefaults.standard.integer(forKey: "BeaconPickerClockFormat")
            return stored == 12 || stored == 24 ? stored : nil
        }
        set { UserDefaults.standard.set(newValue ?? 0, forKey: "BeaconPickerClockFormat") }
    }

    /// Whether times are written with AM/PM: the toggle's choice, else the locale's (a "j"
    /// skeleton comes back with "h" for 12-hour locales and "H" for 24-hour ones).
    static var uses12Hour: Bool {
        if let clockFormat { return clockFormat == 12 }
        return DateFormatter.dateFormat(fromTemplate: "j", options: 0, locale: .current)?.contains("h") ?? false
    }

    /// "10:30 PM" or "22:30".
    static func clockString(hour: Int, minute: Int) -> String {
        if uses12Hour {
            return String(format: "%d:%02d %@", (hour % 12 == 0) ? 12 : hour % 12, minute, hour < 12 ? "AM" : "PM")
        }
        return String(format: "%02d:%02d", hour, minute)
    }

    static func now(kind: Browser.PickerKind, step: Int?) -> PickerValue {
        let c = gregorian.dateComponents([.year, .month, .day, .hour, .minute], from: Date())
        var v = PickerValue(year: c.year ?? 2026, month: c.month ?? 1, day: c.day ?? 1, hour: c.hour ?? 0, minute: c.minute ?? 0, second: nil)
        if let step, step >= 60 {
            let m = step / 60
            v.minute = (v.minute / m) * m
        }
        return v
    }

    /// From the control's sanitised ISO value. Missing parts (a time has no date) come from
    /// today, so switching a datetime-local picker to its Date section shows a month.
    static func parse(_ text: String, kind: Browser.PickerKind) -> PickerValue? {
        guard !text.isEmpty else { return nil }
        var v = now(kind: kind, step: nil)
        func ints(_ s: Substring, _ sep: Character) -> [Int] { s.split(separator: sep).compactMap { Int($0) } }
        switch kind {
        case .date:
            let p = ints(text[...], "-")
            guard p.count == 3 else { return nil }
            (v.year, v.month, v.day) = (p[0], p[1], p[2])
        case .time:
            let p = ints(text[...], ":")
            guard p.count >= 2 else { return nil }
            (v.hour, v.minute) = (p[0], p[1])
            v.second = p.count > 2 ? p[2] : nil
        case .dateTimeLocal:
            guard let t = text.firstIndex(of: "T") else { return nil }
            let d = ints(text[..<t], "-")
            let p = ints(text[text.index(after: t)...], ":")
            guard d.count == 3, p.count >= 2 else { return nil }
            (v.year, v.month, v.day, v.hour, v.minute) = (d[0], d[1], d[2], p[0], p[1])
            v.second = p.count > 2 ? p[2] : nil
        case .month:
            let p = ints(text[...], "-")
            guard p.count == 2 else { return nil }
            (v.year, v.month, v.day) = (p[0], p[1], 1)
        case .week:
            guard let r = text.range(of: "-W"), let year = Int(text[..<r.lowerBound]), let week = Int(text[r.upperBound...]) else { return nil }
            var comps = DateComponents()
            comps.yearForWeekOfYear = year
            comps.weekOfYear = week
            comps.weekday = 2
            guard let monday = iso.date(from: comps) else { return nil }
            let c = gregorian.dateComponents([.year, .month, .day], from: monday)
            (v.year, v.month, v.day) = (c.year ?? year, c.month ?? 1, c.day ?? 1)
        case .color:
            return nil
        }
        return v
    }

    var date: Date? {
        var comps = DateComponents()
        (comps.year, comps.month, comps.day, comps.hour, comps.minute, comps.second) = (year, month, day, hour, minute, second ?? 0)
        return Self.gregorian.date(from: comps)
    }

    func iso(for kind: Browser.PickerKind) -> String {
        let d = String(format: "%04d-%02d-%02d", year, month, day)
        let t = second.map { String(format: "%02d:%02d:%02d", hour, minute, $0) } ?? String(format: "%02d:%02d", hour, minute)
        switch kind {
        case .date: return d
        case .time: return t
        case .dateTimeLocal: return "\(d)T\(t)"
        case .month: return String(format: "%04d-%02d", year, month)
        case .week:
            guard let date else { return "" }
            let c = Self.iso.dateComponents([.yearForWeekOfYear, .weekOfYear], from: date)
            return String(format: "%04d-W%02d", c.yearForWeekOfYear ?? year, c.weekOfYear ?? 1)
        case .color: return ""
        }
    }
}

/// The control's `min`, `max` and `step`, parsed for its kind. `step` is in seconds for the
/// time kinds and days for the date kinds, as the HTML spec scales them.
struct PickerBounds {
    let min: PickerValue?
    let max: PickerValue?
    let step: Int?
    let kind: Browser.PickerKind

    init(kind: Browser.PickerKind, min: String?, max: String?, step: String?) {
        self.kind = kind
        self.min = min.flatMap { PickerValue.parse($0, kind: kind) }
        self.max = max.flatMap { PickerValue.parse($0, kind: kind) }
        self.step = step.flatMap { Int($0.trimmingCharacters(in: .whitespaces)) }.flatMap { $0 > 0 ? $0 : nil }
    }

    /// Whether a day is one the control would accept: inside min...max, and on the step
    /// grid counted from min (or from 1970-01-01, as the spec's default base).
    func allows(_ date: Date) -> Bool {
        let cal = PickerValue.gregorian
        if let min = min?.date, cal.compare(date, to: min, toGranularity: .day) == .orderedAscending { return false }
        if let max = max?.date, cal.compare(date, to: max, toGranularity: .day) == .orderedDescending { return false }
        if kind == .date, let step, step > 1 {
            let base = min?.date ?? PickerValue(year: 1970, month: 1, day: 1, hour: 0, minute: 0, second: nil).date!
            let days = cal.dateComponents([.day], from: cal.startOfDay(for: base), to: cal.startOfDay(for: date)).day ?? 0
            if days % step != 0 { return false }
        }
        return true
    }
}

/// The four parts of the day the caption names, with the icon the screens give each.
enum Daypart {
    case night, morning, afternoon, evening

    static func of(hour: Int) -> Daypart {
        switch hour {
        case 0..<6: return .night
        case 6..<12: return .morning
        case 12..<18: return .afternoon
        default: return .evening
        }
    }

    var name: String {
        switch self {
        case .night: return "Night"
        case .morning: return "Morning"
        case .afternoon: return "Afternoon"
        case .evening: return "Evening"
        }
    }

    var range: String {
        switch self {
        case .night: return "00:00 - 06:00"
        case .morning: return "06:00 - 12:00"
        case .afternoon: return "12:00 - 18:00"
        case .evening: return "18:00 - 24:00"
        }
    }

    var symbol: String {
        switch self {
        case .night: return "moon.fill"
        case .morning: return "sunrise.fill"
        case .afternoon: return "sun.max.fill"
        case .evening: return "sunset.fill"
        }
    }

    var tint: NSColor {
        switch self {
        case .night, .evening: return QuickPick.moon
        case .morning: return QuickPick.sunset
        case .afternoon: return QuickPick.sun
        }
    }

    func image(pointSize: CGFloat) -> NSImage? {
        QuickPick.tinted(symbol: symbol, tint: tint, pointSize: pointSize)
    }
}

/// A quick-select row: a preset the user can jump to.
struct QuickPick {
    let symbol: String
    let tint: NSColor
    /// For the relative times: a pie filled this far instead of a symbol.
    let pie: Double?
    let title: String
    /// What it resolves to, shown at the right: "10:45 PM", "Tue 16 Sep".
    let value: String
    let isTime: Bool
    let apply: (inout PickerValue) -> Void

    static let sun = PickerStyle.dynamic(PickerStyle.hex(0xF6B41E), PickerStyle.hex(0xF6C453))
    static let sunset = PickerStyle.dynamic(PickerStyle.hex(0xF08A1A), PickerStyle.hex(0xF6964A))
    static let moon = PickerStyle.dynamic(PickerStyle.hex(0x6C7FC4), PickerStyle.hex(0x9DB0D6))
    static let clock = PickerStyle.dynamic(PickerStyle.hex(0x6B7A90), PickerStyle.hex(0x9AA5B5))

    static func tinted(symbol: String, tint: NSColor, pointSize: CGFloat) -> NSImage? {
        guard let image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)?
            .withSymbolConfiguration(NSImage.SymbolConfiguration(pointSize: pointSize, weight: .medium)) else { return nil }
        return NSImage(size: image.size, flipped: false) { rect in
            image.draw(in: rect)
            tint.set()
            rect.fill(using: .sourceAtop)
            return true
        }
    }

    /// The Quick select page's groups for a kind: relative times and dayparts, or relative
    /// days and jumps.
    static func groups(for kind: Browser.PickerKind, timeSection: Bool) -> [(title: String, picks: [QuickPick])] {
        let cal = PickerValue.gregorian
        if kind == .month {
            let f = DateFormatter()
            f.setLocalizedDateFormatFromTemplate("MMMM yyyy")
            let thisMonth = cal.date(from: cal.dateComponents([.year, .month], from: Date())) ?? Date()
            func month(_ date: Date, _ symbol: String, _ title: String) -> QuickPick {
                let c = cal.dateComponents([.year, .month], from: date)
                return QuickPick(symbol: symbol, tint: moon, pie: nil, title: title, value: f.string(from: date), isTime: false) { v in
                    (v.year, v.month, v.day) = (c.year ?? v.year, c.month ?? v.month, 1)
                }
            }
            func plus(_ n: Int) -> Date { cal.date(byAdding: .month, value: n, to: thisMonth) ?? thisMonth }
            let january = cal.date(from: DateComponents(year: (cal.component(.year, from: Date())) + 1, month: 1)) ?? thisMonth
            return [
                ("", [month(thisMonth, "calendar", "This month"), month(plus(1), "arrow.right", "Next month"), month(plus(3), "clock", "In 3 months"), month(plus(6), "clock", "In 6 months")]),
                ("", [month(january, "calendar", "January next year"), month(plus(12), "calendar", "Same month next year")]),
            ]
        }
        if kind == .dateTimeLocal {
            // A day group and a time group, so both halves can be picked without leaving
            // the page; "Now" sets both.
            let days = groups(for: .date, timeSection: false).flatMap(\.picks)
            let times = groups(for: .time, timeSection: true)
            let relative = times[0].picks
            let parts = times[1].picks
            let now = QuickPick(symbol: "clock", tint: clock, pie: 0, title: "Now", value: relative[0].value, isTime: true) { v in
                let c = cal.dateComponents([.year, .month, .day], from: Date())
                (v.year, v.month, v.day) = (c.year ?? v.year, c.month ?? v.month, c.day ?? v.day)
                relative[0].apply(&v)
            }
            return [
                ("Day", [days[0], days[1], days[3]]),
                ("Time", [now, parts[0], parts[1], parts[2]]),
            ]
        }
        if timeSection {
            let now = Date()
            func relative(_ minutes: Int, _ pie: Double, _ title: String) -> QuickPick {
                let date = cal.date(byAdding: .minute, value: minutes, to: now) ?? now
                let c = cal.dateComponents([.hour, .minute], from: date)
                return QuickPick(symbol: "clock", tint: clock, pie: pie, title: title, value: PickerValue.clockString(hour: c.hour ?? 0, minute: c.minute ?? 0), isTime: true) { v in
                    (v.hour, v.minute) = (c.hour ?? v.hour, c.minute ?? v.minute)
                    if v.second != nil { v.second = 0 }
                }
            }
            func fixed(_ hour: Int, _ part: Daypart, _ title: String) -> QuickPick {
                QuickPick(symbol: part.symbol, tint: part.tint, pie: nil, title: title, value: PickerValue.clockString(hour: hour, minute: 0), isTime: true) { v in
                    (v.hour, v.minute) = (hour, 0)
                    if v.second != nil { v.second = 0 }
                }
            }
            return [
                ("Relative", [relative(0, 0, "Now"), relative(15, 0.25, "In 15 minutes"), relative(30, 0.5, "In 30 minutes"), relative(60, 1, "In 1 hour")]),
                ("Dayparts", [fixed(6, .morning, "Morning"), fixed(12, .afternoon, "Noon"), fixed(18, .evening, "Evening"), fixed(0, .night, "Night")]),
            ]
        }
        let f = DateFormatter()
        f.setLocalizedDateFormatFromTemplate("EEE d MMM yyyy")
        func day(_ date: Date, _ symbol: String, _ tint: NSColor, _ title: String) -> QuickPick {
            let c = cal.dateComponents([.year, .month, .day], from: date)
            return QuickPick(symbol: symbol, tint: tint, pie: nil, title: title, value: f.string(from: date), isTime: false) { v in
                (v.year, v.month, v.day) = (c.year ?? v.year, c.month ?? v.month, c.day ?? v.day)
            }
        }
        let today = cal.startOfDay(for: Date())
        func plus(_ n: Int, _ unit: Calendar.Component) -> Date { cal.date(byAdding: unit, value: n, to: today) ?? today }
        func nextWeekday(_ weekday: Int) -> Date {
            let current = cal.component(.weekday, from: today)
            var ahead = (weekday - current + 7) % 7
            if ahead == 0 { ahead = 7 }
            return plus(ahead, .day)
        }
        // Headerless groups, separated by rules, as the date screen lists them.
        return [
            ("", [day(today, "calendar", moon, "Today"), day(plus(1, .day), "calendar", moon, "Tomorrow"), day(nextWeekday(7), "calendar", moon, "This weekend")]),
            ("", [day(nextWeekday(2), "arrow.right", PickerStyle.brand, "Next Monday"), day(nextWeekday(6), "arrow.right", PickerStyle.brand, "Next Friday")]),
            ("", [day(plus(7, .day), "clock", moon, "In 1 week"), day(plus(14, .day), "clock", moon, "In 2 weeks"), day(plus(1, .month), "clock", moon, "In 1 month")]),
        ]
    }
}

// ── views ───────────────────────────────────────────────────────────────────

/// The Quick select screen: a subtitle, then groups of rows, each a glyph (or a pie), a
/// title and the value it lands on.
final class QuickSelectPage: NSView {
    var groups: [(title: String, picks: [QuickPick])] = [] { didSet { reload() } }
    var subtitle = "" { didSet { subtitleLabel.stringValue = subtitle } }
    /// What the picks add up to, at the foot of the page (the datetime-local picker).
    var caption = "" { didSet { captionLabel.stringValue = caption } }
    var onPick: ((QuickPick) -> Void)?

    private let subtitleLabel = PickerStyle.label("", size: 16, weight: .regular, color: PickerStyle.inkSecondary)
    private let captionLabel = PickerStyle.label("", size: 18, weight: .semibold, color: PickerStyle.brand)
    private var rows: [QuickRowButton] = []

    override var isFlipped: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        subtitleLabel.frame = NSRect(x: 26, y: 66, width: 640, height: 22)
        addSubview(subtitleLabel)
        captionLabel.frame = NSRect(x: 27, y: 752, width: 640, height: 26)
        addSubview(captionLabel)
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    /// Rebuild the rows; the values (relative times) are recomputed on the way.
    func reload() {
        rows.forEach { $0.removeFromSuperview() }
        subviews.filter { $0.tag == 300 }.forEach { $0.removeFromSuperview() }
        rows = []
        let headed = groups.contains { !$0.title.isEmpty }
        var y: CGFloat = headed ? 114 : 119
        for (i, group) in groups.enumerated() {
            if headed {
                let header = PickerStyle.label(group.title, size: 15, weight: .semibold, color: PickerStyle.inkSecondary)
                header.frame = NSRect(x: 27, y: y, width: 300, height: 20)
                header.tag = 300
                addSubview(header)
                y += 32
            } else if i > 0 {
                let rule = Panel(fill: PickerStyle.divider, radius: 0)
                rule.frame = NSRect(x: 45, y: y + 12, width: 600, height: 1)
                rule.tag = 300
                addSubview(rule)
                y += 26
            }
            for pick in group.picks {
                let row = QuickRowButton(pick: pick)
                row.target = self
                row.action = #selector(rowPressed(_:))
                row.frame = headed ? NSRect(x: 27, y: y, width: 620, height: 59) : NSRect(x: 45, y: y, width: 600, height: 60)
                addSubview(row)
                rows.append(row)
                y += headed ? 70 : 60
            }
            y += headed ? 8 : 0
        }
    }

    @objc private func rowPressed(_ sender: QuickRowButton) {
        onPick?(sender.pick)
    }
}

private final class QuickRowButton: NSButton {
    let pick: QuickPick

    init(pick: QuickPick) {
        self.pick = pick
        super.init(frame: .zero)
        isBordered = false
        title = ""
        setButtonType(.momentaryChange)
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    override var isFlipped: Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        (isHighlighted ? PickerStyle.sidebarSelection : PickerStyle.quickRow).setFill()
        NSBezierPath(roundedRect: bounds, xRadius: 12, yRadius: 12).fill()
        let iconCenter = NSPoint(x: 38, y: bounds.midY)
        if let pie = pick.pie {
            // A clock-like pie: an outline, filled from twelve o'clock this far round.
            let r: CGFloat = 15
            let circle = NSBezierPath(ovalIn: NSRect(x: iconCenter.x - r, y: iconCenter.y - r, width: r * 2, height: r * 2))
            PickerStyle.fieldBackground.setFill()
            circle.fill()
            if pie > 0 {
                let wedge = NSBezierPath()
                wedge.move(to: iconCenter)
                // Flipped view: angles run clockwise from three o'clock, so twelve is -90.
                wedge.appendArc(withCenter: iconCenter, radius: r, startAngle: -90, endAngle: -90 + CGFloat(pie) * 360, clockwise: false)
                wedge.close()
                PickerStyle.accent.setFill()
                wedge.fill()
            }
            QuickPick.moon.setStroke()
            circle.lineWidth = 2
            circle.stroke()
        } else if let image = QuickPick.tinted(symbol: pick.symbol, tint: pick.tint, pointSize: 24) {
            let s = image.size
            image.draw(in: NSRect(x: iconCenter.x - s.width / 2, y: iconCenter.y - s.height / 2, width: s.width, height: s.height),
                       from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
        }
        let title = NSAttributedString(string: pick.title, attributes: [
            .font: NSFont.systemFont(ofSize: 17, weight: .semibold), .foregroundColor: PickerStyle.brand,
        ])
        title.draw(at: NSPoint(x: 75, y: (bounds.height - title.size().height) / 2))
        let value = NSAttributedString(string: pick.value, attributes: [
            .font: NSFont.systemFont(ofSize: 17, weight: .regular), .foregroundColor: PickerStyle.inkSecondary,
        ])
        value.draw(at: NSPoint(x: bounds.width - 24 - value.size().width, y: (bounds.height - value.size().height) / 2))
    }
}

/// The "12 h | 24 h" control in the card's corner.
final class ClockFormatToggle: NSView {
    var onChange: (() -> Void)?

    override var isFlipped: Bool { true }
    override var mouseDownCanMoveWindow: Bool { false }

    override func draw(_ dirtyRect: NSRect) {
        PickerStyle.dynamic(PickerStyle.hex(0xE8EDF5), PickerStyle.hex(0x2E343E)).setFill()
        NSBezierPath(roundedRect: bounds, xRadius: 8, yRadius: 8).fill()
        let twelve = PickerValue.uses12Hour
        for (i, label) in ["12 h", "24 h"].enumerated() {
            let rect = NSRect(x: 3 + CGFloat(i) * (bounds.width / 2 - 3), y: 3, width: bounds.width / 2 - 3, height: bounds.height - 6)
            let on = (i == 0) == twelve
            if on {
                NSGraphicsContext.saveGraphicsState()
                let shadow = NSShadow()
                shadow.shadowColor = NSColor.black.withAlphaComponent(0.12)
                shadow.shadowBlurRadius = 3
                shadow.shadowOffset = NSSize(width: 0, height: -1)
                shadow.set()
                PickerStyle.fieldBackground.setFill()
                NSBezierPath(roundedRect: rect, xRadius: 6, yRadius: 6).fill()
                NSGraphicsContext.restoreGraphicsState()
            }
            let text = NSAttributedString(string: label, attributes: [
                .font: NSFont.systemFont(ofSize: 12, weight: .medium),
                .foregroundColor: on ? PickerStyle.brand : PickerStyle.inkSecondary,
            ])
            text.draw(at: NSPoint(x: rect.midX - text.size().width / 2, y: rect.midY - text.size().height / 2))
        }
    }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        let chosen = p.x < bounds.midX ? 12 : 24
        guard (chosen == 12) != PickerValue.uses12Hour else { return }
        PickerValue.clockFormat = chosen
        needsDisplay = true
        onChange?()
    }
}

/// The Date screen: Today at the top right, the month title that opens the month & year
/// view, a 7-column grid with the neighbouring months' days greyed and the chosen day as a
/// filled disc, Day/Month/Year steppers, and the chosen date spelled out. For a week input
/// the chosen week's row is tinted and any day in it picks the week.
final class DatePage: NSView {
    var value = PickerValue.now(kind: .date, step: nil) {
        didSet {
            // Turn to the value's month, unless it is a week already in view: a week is a
            // row here, and picking a visible row must not move the page under the pointer.
            if !(weeks && isShown(value)) {
                shownYear = value.year
                shownMonth = value.month
            }
            refreshFields()
            needsDisplay = true
        }
    }
    var limits = PickerBounds(kind: .date, min: nil, max: nil, step: nil)
    var onChange: ((PickerValue) -> Void)?
    var onOpenMonthYear: (() -> Void)?
    /// Appended to the spelled-out date: " at 10:30 PM" for a datetime-local picker.
    var captionSuffix = "" { didSet { refreshFields() } }

    private let weeks: Bool

    private func isShown(_ v: PickerValue) -> Bool {
        guard let date = v.date else { return false }
        return cells().contains { cal.isDate($0.date, inSameDayAs: date) }
    }
    private var shownYear = 2026
    private var shownMonth = 1
    private let today = PickerValue.now(kind: .date, step: nil)
    private let todayButton = DesignButton(title: "Today", target: nil, action: nil)
    private let prev = NSButton(title: "‹", target: nil, action: nil)
    private let next = NSButton(title: "›", target: nil, action: nil)
    private let titleButton = NSButton(title: "", target: nil, action: nil)
    private let dayField = StepperField()
    private let monthField = StepperField()
    private let yearField = StepperField()
    private let captionIcon = NSImageView()
    private let captionDate = PickerStyle.label("", size: 20, weight: .bold, color: PickerStyle.brand)

    private static let cellPitch: CGFloat = 78
    private static let rowPitch: CGFloat = 58
    /// The week picker shifts the grid right to make room for the week numbers.
    private var gridLeft: CGFloat { weeks ? 112 : 77 }
    private static let weekdayY: CGFloat = 158
    private static let firstRowCenter: CGFloat = 212
    /// The week picker's rows are ISO weeks, Monday to Sunday, whatever the locale starts on.
    private var firstWeekday: Int { weeks ? 2 : cal.firstWeekday }

    init(weeks: Bool) {
        self.weeks = weeks
        super.init(frame: .zero)
        todayButton.titleSize = 17
        todayButton.fill = PickerStyle.dynamic(PickerStyle.hex(0xF3F7FE), PickerStyle.hex(0x2A2F38))
        todayButton.border = PickerStyle.dynamic(PickerStyle.hex(0xBFD4F5), PickerStyle.hex(0x3A414C))
        todayButton.titleColor = PickerStyle.accent
        todayButton.radius = 10
        todayButton.target = self
        todayButton.action = #selector(pickToday)
        todayButton.frame = NSRect(x: 560, y: 22, width: 110, height: 37)
        addSubview(todayButton)

        for (button, x) in [(prev, CGFloat(33)), (next, CGFloat(608))] {
            button.isBordered = false
            button.font = .systemFont(ofSize: 30, weight: .medium)
            button.contentTintColor = PickerStyle.brand
            button.frame = NSRect(x: x, y: 92, width: 40, height: 40)
            button.target = self
            addSubview(button)
        }
        prev.action = #selector(showPrevious)
        next.action = #selector(showNext)
        titleButton.isBordered = false
        titleButton.target = self
        titleButton.action = #selector(openMonthYear)
        titleButton.frame = NSRect(x: 100, y: 92, width: 300, height: 40)
        titleButton.alignment = .left
        addSubview(titleButton)

        let months = DateFormatter().monthSymbols ?? []
        // A week input steps weeks and years; a date input days, months and years.
        let layout: [(StepperField, String, CGFloat, CGFloat)] = weeks
            ? [(dayField, "Week", 25, 124), (yearField, "Year", 175, 179)]
            : [(dayField, "Day", 25, 124), (monthField, "Month", 175, 234), (yearField, "Year", 435, 179)]
        for (field, label, x, w) in layout {
            let title = PickerStyle.label(label, size: 16, weight: .regular, color: PickerStyle.brand)
            title.frame = NSRect(x: x, y: 552, width: 120, height: 20)
            addSubview(title)
            field.frame = NSRect(x: x, y: 578, width: w, height: 56)
            field.onChange = { [weak self] _ in self?.fieldsChanged() }
            addSubview(field)
        }
        dayField.range = 1...31
        dayField.wraps = true
        if weeks {
            // Week 52 up is week 1 of the next year, week 1 down is the last of the one before.
            dayField.onStep = { [weak self] direction in self?.shift(days: 7 * direction) }
        }
        monthField.range = 1...12
        monthField.wraps = true
        monthField.format = { months.indices.contains($0 - 1) ? months[$0 - 1] : String($0) }
        monthField.parse = { text in
            let t = text.trimmingCharacters(in: .whitespaces).lowercased()
            if let n = Int(t) { return n }
            return months.firstIndex { $0.lowercased().hasPrefix(t) && !t.isEmpty }.map { $0 + 1 }
        }
        yearField.range = 1...9999
        yearField.format = { String($0) }

        let rule = Panel(fill: PickerStyle.divider, radius: 0)
        rule.frame = NSRect(x: 25, y: 697, width: 620, height: 1)
        addSubview(rule)
        captionIcon.image = QuickPick.tinted(symbol: "calendar", tint: PickerStyle.brand, pointSize: 26)
        captionIcon.frame = NSRect(x: 30, y: 726, width: 36, height: 36)
        addSubview(captionIcon)
        let captionTitle = PickerStyle.label("Selected date:", size: 18, weight: .regular, color: PickerStyle.inkSecondary)
        captionTitle.frame = NSRect(x: 75, y: 722, width: 400, height: 24)
        addSubview(captionTitle)
        captionDate.frame = NSRect(x: 75, y: 749, width: 560, height: 28)
        addSubview(captionDate)
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    override var isFlipped: Bool { true }
    override var mouseDownCanMoveWindow: Bool { false }

    private var cal: Calendar { PickerValue.gregorian }

    /// The ISO week the value is in: its week number, week-year, and the Monday and Sunday.
    private var isoWeek: (week: Int, year: Int, monday: Date, sunday: Date)? {
        guard let date = value.date else { return nil }
        let c = PickerValue.iso.dateComponents([.weekOfYear, .yearForWeekOfYear], from: date)
        var start = DateComponents()
        (start.yearForWeekOfYear, start.weekOfYear, start.weekday) = (c.yearForWeekOfYear, c.weekOfYear, 2)
        guard let monday = PickerValue.iso.date(from: start), let sunday = cal.date(byAdding: .day, value: 6, to: monday),
              let week = c.weekOfYear, let year = c.yearForWeekOfYear else { return nil }
        return (week, year, monday, sunday)
    }

    private func refreshFields() {
        if weeks, let w = isoWeek {
            let weeksInYear = PickerValue.iso.range(of: .weekOfYear, in: .yearForWeekOfYear, for: w.monday)?.count ?? 52
            dayField.range = 1...weeksInYear
            dayField.value = w.week
            yearField.value = w.year
            let f = DateFormatter()
            f.calendar = cal
            f.setLocalizedDateFormatFromTemplate("d MMM")
            captionDate.stringValue = "Week \(w.week), \(w.year) · \(f.string(from: w.monday)) – \(f.string(from: w.sunday))"
        } else {
            dayField.range = 1...PickerValue.daysIn(year: value.year, month: value.month)
            dayField.value = value.day
            monthField.value = value.month
            yearField.value = value.year
            let f = DateFormatter()
            f.calendar = cal
            f.setLocalizedDateFormatFromTemplate("EEEE d MMMM yyyy")
            captionDate.stringValue = (value.date.map { f.string(from: $0) } ?? "") + captionSuffix
        }
        let title = DateFormatter()
        title.calendar = cal
        title.setLocalizedDateFormatFromTemplate("MMMM yyyy")
        var comps = DateComponents()
        (comps.year, comps.month, comps.day) = (shownYear, shownMonth, 1)
        let shown = cal.date(from: comps).map { title.string(from: $0) } ?? ""
        titleButton.attributedTitle = NSAttributedString(string: "\(shown) ⌄", attributes: [
            .font: NSFont.systemFont(ofSize: 22, weight: .semibold), .foregroundColor: PickerStyle.accent,
        ])
    }

    private func fieldsChanged() {
        var v = value
        if weeks {
            // Week n of the year: its Monday.
            var comps = DateComponents()
            (comps.yearForWeekOfYear, comps.weekOfYear, comps.weekday) = (max(1, yearField.value), dayField.value, 2)
            guard let monday = PickerValue.iso.date(from: comps) else { return }
            let c = cal.dateComponents([.year, .month, .day], from: monday)
            (v.year, v.month, v.day) = (c.year ?? v.year, c.month ?? v.month, c.day ?? v.day)
        } else {
            v.year = max(1, yearField.value)
            v.month = monthField.value
            v.day = min(dayField.value, PickerValue.daysIn(year: v.year, month: v.month))
        }
        onChange?(v)
    }

    @objc private func showPrevious() { step(-1) }
    @objc private func showNext() { step(1) }

    /// Move the chosen date by whole days, across month and year ends.
    private func shift(days: Int) {
        guard let current = value.date, let moved = cal.date(byAdding: .day, value: days, to: current), limits.allows(moved) else { return }
        let c = cal.dateComponents([.year, .month, .day], from: moved)
        var v = value
        (v.year, v.month, v.day) = (c.year ?? v.year, c.month ?? v.month, c.day ?? v.day)
        onChange?(v)
    }

    /// The wheel: a week picker walks the weeks (the page follows when the chosen week
    /// leaves it), a date picker turns the months without touching the choice.
    private var wheelAccumulator: CGFloat = 0

    override func scrollWheel(with event: NSEvent) {
        wheelAccumulator += event.scrollingDeltaY * (event.hasPreciseScrollingDeltas ? 1 : 8)
        let notch: CGFloat = 40
        while abs(wheelAccumulator) >= notch {
            // Scrolling down (content moves up) goes forward in time.
            let forward = wheelAccumulator < 0
            wheelAccumulator += forward ? notch : -notch
            if weeks {
                shift(days: forward ? 7 : -7)
            } else {
                step(forward ? 1 : -1)
            }
        }
    }

    private func step(_ months: Int) {
        var m = shownMonth + months
        var y = shownYear
        while m < 1 {
            m += 12
            y -= 1
        }
        while m > 12 {
            m -= 12
            y += 1
        }
        shownMonth = m
        shownYear = y
        refreshFields()
        needsDisplay = true
    }

    @objc private func openMonthYear() {
        onOpenMonthYear?()
    }

    @objc private func pickToday() {
        onChange?(today)
    }

    /// Six rows of seven: the shown month's days plus the neighbours' that fill the grid.
    private func cells() -> [(date: Date, day: Int, inMonth: Bool, col: Int, row: Int)] {
        var comps = DateComponents()
        (comps.year, comps.month, comps.day) = (shownYear, shownMonth, 1)
        guard let first = cal.date(from: comps) else { return [] }
        let weekday = cal.component(.weekday, from: first)
        let lead = (weekday - firstWeekday + 7) % 7
        guard let start = cal.date(byAdding: .day, value: -lead, to: first) else { return [] }
        return (0..<42).compactMap { i in
            guard let d = cal.date(byAdding: .day, value: i, to: start) else { return nil }
            let c = cal.dateComponents([.month, .day], from: d)
            return (d, c.day ?? 1, c.month == shownMonth, i % 7, i / 7)
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        let ink = PickerStyle.brand
        let muted = PickerStyle.inkSecondary
        let accent = PickerStyle.accent
        let f = DateFormatter()
        f.calendar = cal
        // Two-letter weekday initials, from the locale's first weekday.
        let symbols = f.shortWeekdaySymbols ?? ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]
        for col in 0..<7 {
            let symbol = String(symbols[(firstWeekday - 1 + col) % 7].prefix(2))
            draw(symbol, at: NSPoint(x: gridLeft + CGFloat(col) * Self.cellPitch, y: Self.weekdayY), size: 16, weight: .regular, color: muted)
        }
        let grid = cells()
        if weeks {
            // A gutter of week numbers: its own tinted band and a rule between it and the
            // days, so it reads as an index to the rows rather than an eighth column.
            let chosen = value.date.map { PickerValue.iso.dateComponents([.yearForWeekOfYear, .weekOfYear], from: $0) }
            let top = Self.weekdayY - 18
            let bottom = Self.firstRowCenter + 5 * Self.rowPitch + 26
            PickerStyle.quickRow.setFill()
            NSBezierPath(roundedRect: NSRect(x: 22, y: top, width: 44, height: bottom - top), xRadius: 10, yRadius: 10).fill()
            PickerStyle.divider.setFill()
            NSRect(x: 72, y: top + 6, width: 1, height: bottom - top - 12).fill()
            draw("WK", at: NSPoint(x: 44, y: Self.weekdayY), size: 12, weight: .semibold, color: muted)
            for row in 0..<6 {
                guard let firstInRow = grid.first(where: { $0.row == row }) else { continue }
                let week = PickerValue.iso.dateComponents([.yearForWeekOfYear, .weekOfYear], from: firstInRow.date)
                let y = Self.firstRowCenter + CGFloat(row) * Self.rowPitch
                let isChosen = week == chosen
                if isChosen {
                    accent.withAlphaComponent(0.15).setFill()
                    NSBezierPath(roundedRect: NSRect(x: gridLeft - 30, y: y - 24, width: 6 * Self.cellPitch + 60, height: 48), xRadius: 24, yRadius: 24).fill()
                    accent.setFill()
                    NSBezierPath(roundedRect: NSRect(x: 27, y: y - 14, width: 34, height: 28), xRadius: 8, yRadius: 8).fill()
                }
                draw(String(week.weekOfYear ?? 0), at: NSPoint(x: 44, y: y), size: 15, weight: .semibold, color: isChosen ? .white : PickerStyle.brand)
            }
        }
        for cell in grid {
            let cx = gridLeft + CGFloat(cell.col) * Self.cellPitch
            let cy = Self.firstRowCenter + CGFloat(cell.row) * Self.rowPitch
            let c = cal.dateComponents([.year, .month, .day], from: cell.date)
            let isChosen = !weeks && c.year == value.year && c.month == value.month && c.day == value.day
            let isToday = c.year == today.year && c.month == today.month && c.day == today.day
            let allowed = limits.allows(cell.date)
            if isChosen {
                accent.setFill()
                NSBezierPath(ovalIn: NSRect(x: cx - 20, y: cy - 20, width: 40, height: 40)).fill()
            } else if isToday {
                accent.withAlphaComponent(0.5).setStroke()
                let ring = NSBezierPath(ovalIn: NSRect(x: cx - 19.5, y: cy - 19.5, width: 39, height: 39))
                ring.lineWidth = 1.5
                ring.stroke()
            }
            let color: NSColor = isChosen ? .white : (!cell.inMonth || !allowed ? muted.withAlphaComponent(0.55) : ink)
            draw(String(cell.day), at: NSPoint(x: cx, y: cy), size: 20, weight: isChosen ? .bold : .regular, color: color)
        }
    }

    private func draw(_ text: String, at center: NSPoint, size: CGFloat, weight: NSFont.Weight, color: NSColor) {
        let attributed = NSAttributedString(string: text, attributes: [.font: NSFont.systemFont(ofSize: size, weight: weight), .foregroundColor: color])
        let s = attributed.size()
        attributed.draw(at: NSPoint(x: center.x - s.width / 2, y: center.y - s.height / 2))
    }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        let grid = cells()
        if weeks {
            // Anywhere along a row, week number included, picks that week (by its Monday).
            for row in 0..<6 {
                let cy = Self.firstRowCenter + CGFloat(row) * Self.rowPitch
                guard abs(p.y - cy) <= 26, p.x >= 25, p.x <= gridLeft + 6 * Self.cellPitch + 30,
                      let monday = grid.first(where: { $0.row == row }) else { continue }
                guard limits.allows(monday.date) else { return }
                let c = cal.dateComponents([.year, .month, .day], from: monday.date)
                var v = value
                (v.year, v.month, v.day) = (c.year ?? v.year, c.month ?? v.month, c.day ?? v.day)
                onChange?(v)
                return
            }
            return
        }
        for cell in grid {
            let cx = gridLeft + CGFloat(cell.col) * Self.cellPitch
            let cy = Self.firstRowCenter + CGFloat(cell.row) * Self.rowPitch
            if abs(p.x - cx) <= 26, abs(p.y - cy) <= 24 {
                guard limits.allows(cell.date) else { return }
                let c = cal.dateComponents([.year, .month, .day], from: cell.date)
                var v = value
                (v.year, v.month, v.day) = (c.year ?? v.year, c.month ?? v.month, c.day ?? v.day)
                onChange?(v)
                return
            }
        }
    }

    override func keyDown(with event: NSEvent) {
        // Arrow keys walk the days; the steppers below handle their own.
        let delta: Int
        switch event.specialKey {
        case .leftArrow?: delta = -1
        case .rightArrow?: delta = 1
        case .upArrow?: delta = -7
        case .downArrow?: delta = 7
        default: return super.keyDown(with: event)
        }
        shift(days: weeks && abs(delta) == 1 ? delta * 7 : delta)
    }
}

/// The month & year screen: a 4 × 3 grid of months, a year stepper, and a 3 × 3 grid of
/// years around the shown one. The arrows step the year.
final class MonthYearPage: NSView {
    var limits = PickerBounds(kind: .date, min: nil, max: nil, step: nil)
    /// `done` is true for a month, false for a year; the view stays up either way.
    var onChange: ((_ year: Int, _ month: Int, _ done: Bool) -> Void)?
    /// The chosen month spelled out at the foot, for a month input.
    var caption = "" { didSet { captionLabel.stringValue = caption } }
    private let captionLabel = PickerStyle.label("", size: 18, weight: .semibold, color: PickerStyle.brand)

    private var year = 2026
    private var month = 1
    /// The first of the nine years on show; the grid scrolls by rows of three.
    private var windowStart = 2023
    private var wheelAccumulator: CGFloat = 0
    private let prev = NSButton(title: "‹", target: nil, action: nil)
    private let next = NSButton(title: "›", target: nil, action: nil)
    private let yearField = StepperField()

    override init(frame: NSRect) {
        super.init(frame: frame)
        for (button, x) in [(prev, CGFloat(33)), (next, CGFloat(608))] {
            button.isBordered = false
            button.font = .systemFont(ofSize: 30, weight: .medium)
            button.contentTintColor = PickerStyle.brand
            button.frame = NSRect(x: x, y: 92, width: 40, height: 40)
            button.target = self
            addSubview(button)
        }
        prev.action = #selector(previousYear)
        next.action = #selector(nextYear)
        for (text, y, size) in [("Month", CGFloat(127), CGFloat(16)), ("Year", 400, 20)] {
            let label = PickerStyle.label(text, size: size, weight: .semibold, color: PickerStyle.brand)
            label.frame = NSRect(x: 25, y: y, width: 200, height: 26)
            addSubview(label)
        }
        captionLabel.frame = NSRect(x: 25, y: 752, width: 640, height: 26)
        addSubview(captionLabel)
        yearField.range = 1...9999
        yearField.format = { String($0) }
        yearField.frame = NSRect(x: 200, y: 399, width: 209, height: 45)
        yearField.onChange = { [weak self] y in
            guard let self, y >= 1 else { return }
            self.year = y
            self.needsDisplay = true
            self.onChange?(self.year, self.month, false)
        }
        addSubview(yearField)
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    override var isFlipped: Bool { true }
    override var mouseDownCanMoveWindow: Bool { false }

    func set(year: Int, month: Int) {
        self.year = year
        self.month = month
        yearField.value = year
        // Keep the chosen year on show; recentre only when it has scrolled out of view.
        if !(windowStart...windowStart + 8).contains(year) {
            windowStart = year - 3
        }
        needsDisplay = true
    }

    /// The wheel over the years scrolls the window a row at a time; the choice stays.
    override func scrollWheel(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        guard p.y >= 460 else { return super.scrollWheel(with: event) }
        wheelAccumulator += event.scrollingDeltaY * (event.hasPreciseScrollingDeltas ? 1 : 8)
        let notch: CGFloat = 40
        while abs(wheelAccumulator) >= notch {
            let forward = wheelAccumulator < 0
            wheelAccumulator += forward ? notch : -notch
            windowStart = max(1, windowStart + (forward ? 3 : -3))
        }
        needsDisplay = true
    }

    @objc private func previousYear() { onChange?(max(1, year - 1), month, false) }
    @objc private func nextYear() { onChange?(year + 1, month, false) }

    private func monthRect(_ m: Int) -> NSRect {
        NSRect(x: 25 + CGFloat((m - 1) % 4) * 150, y: 158 + CGFloat((m - 1) / 4) * 70, width: 139, height: 57)
    }

    private var yearsShown: [Int] { (windowStart...windowStart + 8).map { $0 } }

    private func yearRect(_ index: Int) -> NSRect {
        NSRect(x: 25 + CGFloat(index % 3) * 202, y: 470 + CGFloat(index / 3) * 64, width: 190, height: 50)
    }

    override func draw(_ dirtyRect: NSRect) {
        let names = DateFormatter().shortMonthSymbols ?? []
        for m in 1...12 {
            box(monthRect(m), text: names[m - 1], on: m == month, size: 20)
        }
        for (i, y) in yearsShown.enumerated() {
            box(yearRect(i), text: String(y), on: y == year, size: 20)
        }
    }

    private func box(_ rect: NSRect, text: String, on: Bool, size: CGFloat) {
        let path = NSBezierPath(roundedRect: rect.insetBy(dx: 0.5, dy: 0.5), xRadius: 10, yRadius: 10)
        (on ? PickerStyle.accent : PickerStyle.fieldBackground).setFill()
        path.fill()
        if !on {
            PickerStyle.dynamic(PickerStyle.hex(0xC7D4E8), PickerStyle.hex(0x3A414C)).setStroke()
            path.stroke()
        }
        let attributed = NSAttributedString(string: text, attributes: [
            .font: NSFont.systemFont(ofSize: size, weight: on ? .semibold : .regular),
            .foregroundColor: on ? NSColor.white : PickerStyle.brand,
        ])
        let s = attributed.size()
        attributed.draw(at: NSPoint(x: rect.midX - s.width / 2, y: rect.midY - s.height / 2))
    }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        for m in 1...12 where monthRect(m).contains(p) {
            onChange?(year, m, true)
            return
        }
        for (i, y) in yearsShown.enumerated() where yearRect(i).contains(p) {
            onChange?(y, month, false)
            return
        }
    }
}

/// The clock face from the screens: hour numerals on the outer ring and minute numerals on
/// the inner one, as designed, with the hands the conventional way round -- the short hand
/// is the hour and reaches the inner ring, the long hand is the minute and reaches the
/// outer ring. Each hand ends in a knob that shows its number and can be dragged.
final class ClockFaceView: NSView {
    var onChange: ((_ hour: Int, _ minute: Int) -> Void)?
    var minuteStep = 1
    /// In 24 h mode the ring shows the chosen half's hours: 00-11, or 12-23.
    var twentyFourHour = false { didSet { needsDisplay = true } }

    private var hour = 10
    private var minute = 30
    private var dragging: Hand?

    private enum Hand {
        case hour, minute
    }

    override var isFlipped: Bool { true }
    override var mouseDownCanMoveWindow: Bool { false }

    func set(hour: Int, minute: Int) {
        self.hour = hour
        self.minute = minute
        needsDisplay = true
    }

    private var center: NSPoint { NSPoint(x: bounds.midX, y: bounds.midY) }
    private var radius: CGFloat { bounds.width / 2 }
    /// Where the numerals sit: hours outside, minutes inside.
    private var hourNumerals: CGFloat { radius - 46 }
    private var minuteNumerals: CGFloat { radius - 95 }
    /// How far the hands reach: the hour hand is the short one.
    private var hourRing: CGFloat { radius - 95 }
    private var minuteRing: CGFloat { radius - 46 }
    /// The hour hand sits on its hour, not part-way to the next as a real clock's would: it
    /// is a control, and dragging the minutes must not move it.
    private var hourAngle: CGFloat { CGFloat(hour % 12) / 12 * 2 * .pi - .pi / 2 }
    private var minuteAngle: CGFloat { CGFloat(minute) / 60 * 2 * .pi - .pi / 2 }
    private var hourKnob: NSPoint { point(at: hourAngle, radius: hourRing) }
    private var minuteKnob: NSPoint { point(at: minuteAngle, radius: minuteRing) }

    private func point(at angle: CGFloat, radius r: CGFloat) -> NSPoint {
        NSPoint(x: center.x + cos(angle) * r, y: center.y + sin(angle) * r)
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        let navy = PickerStyle.dynamic(PickerStyle.hex(0x152A54), PickerStyle.hex(0xD5DBE5))
        let muted = PickerStyle.inkSecondary
        let blue = PickerStyle.dynamic(PickerStyle.hex(0x0A73FA), PickerStyle.hex(0x4C8DFF))

        let face = NSRect(x: center.x - radius, y: center.y - radius, width: radius * 2, height: radius * 2)
        ctx.saveGState()
        ctx.setShadow(offset: CGSize(width: 0, height: -3), blur: 14, color: NSColor.black.withAlphaComponent(0.06).cgColor)
        ctx.setFillColor(PickerStyle.dynamic(PickerStyle.hex(0xEEF3FB), PickerStyle.hex(0x2A2F38)).cgColor)
        ctx.fillEllipse(in: face)
        ctx.restoreGState()
        NSGraphicsContext.saveGraphicsState()
        NSBezierPath(ovalIn: face).addClip()
        NSGradient(starting: PickerStyle.dynamic(.white, PickerStyle.hex(0x30363F)), ending: PickerStyle.dynamic(PickerStyle.hex(0xEEF3FB), PickerStyle.hex(0x2A2F38)))?
            .draw(fromCenter: center, radius: 0, toCenter: center, radius: radius, options: [])
        NSGraphicsContext.restoreGraphicsState()
        ctx.setStrokeColor(PickerStyle.dynamic(PickerStyle.hex(0xDCE4F0), PickerStyle.hex(0x3A414C)).cgColor)
        ctx.setLineWidth(1)
        ctx.strokeEllipse(in: face)

        ctx.setStrokeColor(navy.cgColor)
        ctx.setLineCap(.round)
        for i in 0..<60 {
            let a = CGFloat(i) / 60 * 2 * .pi - .pi / 2
            let major = i % 5 == 0
            ctx.setLineWidth(major ? 2.5 : 1.2)
            ctx.move(to: point(at: a, radius: radius - (major ? 20 : 14)))
            ctx.addLine(to: point(at: a, radius: radius - 6))
            ctx.strokePath()
        }
        for h in 1...12 {
            let a = CGFloat(h) / 12 * 2 * .pi - .pi / 2
            // 24 h: the chosen half's own hours, 00-11 or 12-23, with the top numeral 00 or 12.
            let label = twentyFourHour ? String(format: "%02d", (h % 12) + (hour >= 12 ? 12 : 0)) : String(h)
            draw(label, at: point(at: a, radius: hourNumerals), size: 22, weight: .bold, color: navy)
        }
        for m in stride(from: 0, to: 60, by: 5) {
            let a = CGFloat(m) / 60 * 2 * .pi - .pi / 2
            draw(String(format: "%02d", m), at: point(at: a, radius: minuteNumerals), size: 14, weight: .regular, color: muted)
        }

        ctx.setStrokeColor(blue.cgColor)
        ctx.setLineWidth(5)
        ctx.move(to: center)
        ctx.addLine(to: hourKnob)
        ctx.strokePath()
        ctx.move(to: center)
        ctx.addLine(to: minuteKnob)
        ctx.strokePath()
        let hourText = twentyFourHour ? String(format: "%02d", hour) : String((hour % 12 == 0) ? 12 : hour % 12)
        knob(at: hourKnob, text: hourText, ctx: ctx, fill: blue)
        knob(at: minuteKnob, text: String(format: "%02d", minute), ctx: ctx, fill: blue)
        ctx.setFillColor(navy.cgColor)
        ctx.fillEllipse(in: NSRect(x: center.x - 12, y: center.y - 12, width: 24, height: 24))
    }

    private func knob(at p: NSPoint, text: String, ctx: CGContext, fill: NSColor) {
        ctx.saveGState()
        ctx.setShadow(offset: CGSize(width: 0, height: -2), blur: 6, color: NSColor.black.withAlphaComponent(0.2).cgColor)
        ctx.setFillColor(fill.cgColor)
        ctx.fillEllipse(in: NSRect(x: p.x - 21, y: p.y - 21, width: 42, height: 42))
        ctx.restoreGState()
        draw(text, at: p, size: 20, weight: .semibold, color: .white)
    }

    private func draw(_ text: String, at point: NSPoint, size: CGFloat, weight: NSFont.Weight, color: NSColor) {
        let attributed = NSAttributedString(string: text, attributes: [.font: NSFont.systemFont(ofSize: size, weight: weight), .foregroundColor: color])
        let s = attributed.size()
        attributed.draw(at: NSPoint(x: point.x - s.width / 2, y: point.y - s.height / 2))
    }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        func distance(_ a: NSPoint, _ b: NSPoint) -> CGFloat { ((a.x - b.x) * (a.x - b.x) + (a.y - b.y) * (a.y - b.y)).squareRoot() }
        let fromCenter = distance(p, center)
        guard fromCenter <= radius + 4 else { return }
        // The knob under the pointer takes the drag; a press elsewhere goes to the hand
        // whose reach it landed on -- the long minute hand outside, the short hour hand inside.
        let toHour = distance(p, hourKnob)
        let toMinute = distance(p, minuteKnob)
        if min(toHour, toMinute) < 30 {
            dragging = toHour < toMinute ? .hour : .minute
        } else {
            dragging = fromCenter > (hourRing + minuteRing) / 2 ? .minute : .hour
        }
        drag(to: p)
    }

    override func mouseDragged(with event: NSEvent) {
        guard dragging != nil else { return }
        drag(to: convert(event.locationInWindow, from: nil))
    }

    override func mouseUp(with event: NSEvent) {
        dragging = nil
    }

    private func drag(to p: NSPoint) {
        let angle = atan2(p.y - center.y, p.x - center.x) + .pi / 2
        let turn = (angle < 0 ? angle + 2 * .pi : angle) / (2 * .pi)
        switch dragging {
        case .hour?:
            hour = (hour >= 12 ? 12 : 0) + Int((turn * 12).rounded()) % 12
        case .minute?:
            let m = Int((turn * 60).rounded()) % 60
            minute = (m / max(minuteStep, 1)) * max(minuteStep, 1)
        case nil:
            return
        }
        needsDisplay = true
        onChange?(hour, minute)
    }
}

/// The column beside the face: two halves in one rounded border. In 12 h mode PM (sun) sits
/// above AM (moon), as the screen has them; in 24 h mode the halves are 00-11 (moon) and
/// 12-23 (sun). The chosen half is filled with the accent.
final class MeridiemToggle: NSView {
    var isTop = false { didSet { needsDisplay = true } }
    var twelveHour = true { didSet { needsDisplay = true } }
    var onChange: ((_ top: Bool) -> Void)?

    override var isFlipped: Bool { true }
    override var mouseDownCanMoveWindow: Bool { false }

    override func draw(_ dirtyRect: NSRect) {
        let outer = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 14, yRadius: 14)
        PickerStyle.fieldBackground.setFill()
        outer.fill()
        PickerStyle.dynamic(PickerStyle.hex(0xC7D4E8), PickerStyle.hex(0x3A414C)).setStroke()
        outer.stroke()
        let halves: [(NSRect, Bool)] = [
            (NSRect(x: 0, y: 0, width: bounds.width, height: 126), true),
            (NSRect(x: 0, y: 128, width: bounds.width, height: bounds.height - 128), false),
        ]
        for (rect, top) in halves {
            let on = top == isTop
            let label: String
            let symbol: String
            let tint: NSColor
            if twelveHour {
                label = top ? "PM" : "AM"
                symbol = top ? "sun.max.fill" : "moon.fill"
                tint = top ? QuickPick.sun : QuickPick.moon
            } else {
                label = top ? "00-11" : "12-23"
                symbol = top ? "moon.fill" : "sun.max.fill"
                tint = top ? QuickPick.sun : QuickPick.sun
            }
            if on {
                PickerStyle.dynamic(PickerStyle.hex(0x0A73FA), PickerStyle.hex(0x2F7BFF)).setFill()
                NSBezierPath(roundedRect: rect, xRadius: 14, yRadius: 14).fill()
            }
            if let image = QuickPick.tinted(symbol: symbol, tint: on ? (twelveHour && !top ? NSColor.white : tint) : (twelveHour && !top ? QuickPick.moon : tint), pointSize: 30) {
                let s = image.size
                image.draw(in: NSRect(x: rect.midX - s.width / 2, y: rect.minY + 30, width: s.width, height: s.height),
                           from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
            }
            let text = NSAttributedString(string: label, attributes: [
                .font: NSFont.systemFont(ofSize: twelveHour ? 24 : 20, weight: .semibold),
                .foregroundColor: on ? NSColor.white : PickerStyle.brand,
            ])
            text.draw(at: NSPoint(x: rect.midX - text.size().width / 2, y: rect.maxY - 48))
        }
    }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        let top = p.y < 127
        guard top != isTop else { return }
        isTop = top
        onChange?(top)
    }
}

/// A number field with up/down arrows, as the screens draw Hour and Minute: a bordered box,
/// the number large at the left, the stepper at the right.
final class StepperField: Panel, NSTextFieldDelegate {
    var range: ClosedRange<Int> = 0...59
    var step = 1
    var wraps = false
    var onChange: ((Int) -> Void)?
    /// How a value is shown ("09", "September") and read back ("sep", "9").
    var format: ((Int) -> String)?
    var parse: ((String) -> Int?)?
    /// When set, the arrows and arrow keys call this with +1/-1 instead of stepping within
    /// `range` -- for a week stepper that must roll into the next year.
    var onStep: ((Int) -> Void)?
    private var current = 0
    var value: Int {
        get { current }
        set {
            current = newValue
            field.stringValue = format?(newValue) ?? (range.lowerBound == 0 ? String(format: "%02d", newValue) : String(newValue))
        }
    }

    private let field = PickerStyle.field(size: 30)
    private let stepper = NSStepper()

    init() {
        super.init(fill: PickerStyle.fieldBackground, border: PickerStyle.dynamic(PickerStyle.hex(0xC7D4E8), PickerStyle.hex(0x3A414C)), radius: 12)
        field.font = .systemFont(ofSize: 30, weight: .medium)
        field.textColor = PickerStyle.brand
        field.delegate = self
        addSubview(field)
        stepper.target = self
        stepper.action = #selector(stepped)
        stepper.valueWraps = true
        stepper.minValue = 0
        stepper.maxValue = 100
        stepper.integerValue = 50
        addSubview(stepper)
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    override func layout() {
        super.layout()
        field.frame = NSRect(x: 18, y: (bounds.height - 38) / 2, width: bounds.width - 70, height: 38)
        stepper.frame = NSRect(x: bounds.width - 40, y: (bounds.height - 27) / 2, width: 20, height: 27)
    }

    @objc private func stepped() {
        // The stepper lives in a flipped view, so its arrows draw and hit the other way up:
        // AppKit's "up" is the arrow the user sees at the bottom.
        let direction = (stepper.integerValue >= 50 ? 1 : -1) * (isFlipped ? -1 : 1)
        stepper.integerValue = 50
        move(direction)
    }

    private func move(_ direction: Int) {
        if let onStep {
            onStep(direction)
            return
        }
        var next = value + direction * step
        if next > range.upperBound { next = wraps ? range.lowerBound : range.upperBound }
        if next < range.lowerBound { next = wraps ? range.upperBound : range.lowerBound }
        value = next
        onChange?(next)
    }

    func controlTextDidChange(_ notification: Notification) {
        let text = field.stringValue
        guard let n = parse?(text) ?? Int(text.trimmingCharacters(in: .whitespaces)), range.contains(n) else { return }
        current = n
        onChange?(n)
    }

    /// Up and down arrows in the field step it, as the design's keyboard note asks.
    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        switch selector {
        case #selector(NSResponder.moveUp(_:)):
            move(1)
            return true
        case #selector(NSResponder.moveDown(_:)):
            move(-1)
            return true
        default:
            return false
        }
    }
}
