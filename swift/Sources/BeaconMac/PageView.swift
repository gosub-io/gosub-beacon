import AppKit
import CBeacon

/// The view Beacon draws the page into, and the one that owns page input.
///
/// AppKit lays it out among the chrome like any other subview; what appears inside it is
/// rendered by Rust on the GPU, straight into this view's layer. Nothing is copied and this
/// class never touches a pixel.
final class PageView: NSView, NSTextInputClient {
    private let browser: Browser
    private var tab: BeaconTabId = 0
    private var attached = false
    private var tracking: NSTrackingArea?

    /// The link under the pointer. The label that shows it is the window's, not this
    /// view's: wgpu takes ownership of this view's layer to attach a CAMetalLayer, so
    /// nothing else should be trying to draw inside it.
    private var hoverURL: String = ""
    /// Called when the hovered link changes, so the window can show it.
    var onHoverChanged: ((String) -> Void)?

    /// A right-click, with the page point it landed on. The window asks the engine what is
    /// there and builds the menu from the answer — the hovered link alone would only cover
    /// a pointer that had come to rest on one.
    var onContextMenu: ((NSEvent, Float, Float) -> Void)?
    /// A gesture that means "open what is here in another tab": ⌘-click, or a middle click.
    /// The flag is whether to go there, which ⇧⌘-click asks for and the others do not.
    var onOpenInNewTab: ((Float, Float, Bool) -> Void)?
    /// A completed two-finger horizontal swipe: -1 back, +1 forward.
    var onSwipeNavigate: ((Int) -> Void)?
    var onZoomGesture: ((CGFloat) -> Void)?

    init(browser: Browser) {
        self.browser = browser
        super.init(frame: .zero)
        // wgpu attaches a CAMetalLayer to this view, which requires it to be layer-backed.
        wantsLayer = true
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    /// Web pages measure from the top-left; AppKit measures from the bottom-left unless
    /// told otherwise. Flipping here means every mouse coordinate below is already in the
    /// space the engine expects, rather than each call site remembering to subtract.
    override var isFlipped: Bool { true }

    // ── attachment ────────────────────────────────────────────────────────

    func show(tab newTab: BeaconTabId) {
        if attached, tab != 0 {
            browser.detach(tab)
            attached = false
        }
        tab = newTab
        setHover(nil)
        attachIfPossible()
    }

    var currentTab: BeaconTabId { tab }

    private func attachIfPossible() {
        guard !attached, tab != 0 else { return }
        guard window != nil else { return }
        let size = devicePixelSize
        guard size.width > 0, size.height > 0 else {
            // Silence here is how a zero-height page view looks like a broken renderer.
            NSLog("beacon: not attaching — page view is \(bounds.width)x\(bounds.height) points")
            return
        }

        // The view pointer is what Rust wraps as a surface. It must outlive the attachment,
        // which is why `detach` happens in viewWillMove(toWindow:) below.
        let pointer = Unmanaged.passUnretained(self).toOpaque()
        attached = browser.attach(tab, to: pointer, width: size.width, height: size.height)
        if attached {
            sendViewport()
        } else {
            NSLog("beacon: could not attach a view for tab \(tab) — see the beacon [WARN] line above")
        }
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        attachIfPossible()
        updateTracking()
        window?.makeFirstResponder(self)
    }

    override func viewWillMove(toWindow newWindow: NSWindow?) {
        // Beacon must stop drawing before the view goes away.
        if newWindow == nil, attached, tab != 0 {
            browser.detach(tab)
            attached = false
        }
        super.viewWillMove(toWindow: newWindow)
    }

    // ── sizing ────────────────────────────────────────────────────────────

    /// The engine wants CSS pixels for layout and device pixels for the surface, and on a
    /// Retina display those differ by the backing scale factor. Conflating them is the
    /// classic way to get a page rendered at half size or blurred.
    private var devicePixelSize: (width: UInt32, height: UInt32) {
        let backing = convertToBacking(bounds.size)
        return (UInt32(max(0, backing.width)), UInt32(max(0, backing.height)))
    }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        guard tab != 0 else { return }
        if !attached {
            attachIfPossible()
            return
        }
        let size = devicePixelSize
        browser.resizeView(tab, width: size.width, height: size.height)
        sendViewport()
        updateTracking()
    }

    func sendViewport() {
        guard tab != 0 else { return }
        // CSS pixels: the logical size, not the backing size. Zoom is applied on the Rust
        // side, so this stays the view's unzoomed size.
        let width = UInt32(max(0, bounds.width))
        let height = UInt32(max(0, bounds.height))
        guard width > 0, height > 0 else { return }
        // Retina displays are 2.0; an external 1x monitor is 1.0, and dragging the window
        // between them changes it, which is why this is sent on every resize.
        let scale = Float(window?.backingScaleFactor ?? 1.0)
        browser.setViewport(tab, width: width, height: height, scale: scale)
    }

    /// Repaint. Called when a redraw event arrives, not from `draw(_:)` — the page is not
    /// drawn with Core Graphics, so AppKit's own drawing cycle is not involved.
    func redraw() {
        guard attached, tab != 0 else { return }
        _ = browser.draw(tab)
    }

    // ── the hovered link ──────────────────────────────────────────────────

    func setHover(_ url: String?) {
        let value = url ?? ""
        guard value != hoverURL else { return }
        hoverURL = value
        onHoverChanged?(value)
    }

    // ── mouse ─────────────────────────────────────────────────────────────

    private func updateTracking() {
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(
            rect: bounds,
            options: [.activeInKeyWindow, .mouseMoved, .inVisibleRect],
            owner: self,
            userInfo: nil
        )
        addTrackingArea(area)
        tracking = area
    }

    /// Page coordinates. The engine works in unzoomed CSS pixels, so a zoomed page needs
    /// the pointer divided back down or clicks land somewhere other than where you aimed.
    private func pagePoint(_ event: NSEvent) -> (Float, Float) {
        let p = convert(event.locationInWindow, from: nil)
        let zoom = CGFloat(browser.zoom(of: tab))
        return (Float(p.x / zoom), Float(p.y / zoom))
    }

    override func mouseMoved(with event: NSEvent) {
        guard tab != 0 else { return }
        let (x, y) = pagePoint(event)
        browser.mouseMoved(tab, x: x, y: y)
    }

    override func mouseDown(with event: NSEvent) {
        guard tab != 0 else { return }
        window?.makeFirstResponder(self)
        let (x, y) = pagePoint(event)
        // ⌘-click opens a link in a background tab and ⇧⌘-click in front of you, as in
        // Safari and Chrome. The page never sees it: on a Mac this gesture belongs to the
        // browser, and forwarding it as well would follow the link twice.
        if event.modifierFlags.contains(.command) {
            onOpenInNewTab?(x, y, event.modifierFlags.contains(.shift))
            return
        }
        browser.mouseDown(tab, x: x, y: y)
    }

    override func mouseUp(with event: NSEvent) {
        guard tab != 0 else { return }
        let (x, y) = pagePoint(event)
        browser.mouseUp(tab, x: x, y: y)
    }

    override func otherMouseDown(with event: NSEvent) {
        guard tab != 0, event.buttonNumber == 2 else { return }
        // A middle click on a link is the same request as ⌘-click, so it is answered the
        // same way rather than being sent to the page as a third mouse button.
        let (x, y) = pagePoint(event)
        onOpenInNewTab?(x, y, false)
    }

    override func rightMouseDown(with event: NSEvent) {
        guard tab != 0 else { return }
        // Move first so the engine's own idea of what is under the pointer refers to where
        // the click actually landed, and not to wherever it was last.
        let (x, y) = pagePoint(event)
        browser.mouseMoved(tab, x: x, y: y)
        onContextMenu?(event, x, y)
    }

    // ── scrolling, swiping, pinching ──────────────────────────────────────

    /// Horizontal travel accumulated in the current scroll gesture, for swipe navigation.
    private var swipeAccumulator: CGFloat = 0
    private var swipeHandled = false

    override func scrollWheel(with event: NSEvent) {
        guard tab != 0 else { return }

        // A mostly-horizontal trackpad gesture is a navigation, not a scroll. This is
        // accumulated by hand rather than driven through trackSwipeEvent: there is no
        // rubber-band preview, but it behaves the same at the two ends and cannot get stuck
        // half-tracked, which matters more on a demo machine.
        if event.hasPreciseScrollingDeltas, abs(event.scrollingDeltaX) > abs(event.scrollingDeltaY) {
            // NSEvent.Phase is an OptionSet, so this tests membership rather than equality.
            if event.phase.contains(.began) {
                swipeAccumulator = 0
                swipeHandled = false
            } else if event.phase.contains(.changed) {
                swipeAccumulator += event.scrollingDeltaX
                if !swipeHandled, abs(swipeAccumulator) > 80 {
                    swipeHandled = true
                    // Content moves with the fingers: swiping right (positive) goes back.
                    onSwipeNavigate?(swipeAccumulator > 0 ? -1 : 1)
                }
            } else if event.phase.contains(.ended) || event.phase.contains(.cancelled) {
                swipeAccumulator = 0
            }
            if swipeHandled { return }
        }

        // AppKit reports a wheel notch as ±1 lines and a trackpad as precise deltas; the
        // engine scrolls in CSS pixels, so lines are scaled and precise deltas passed
        // through. Signs are inverted: scrolling down moves the page up.
        let step: Float = event.hasPreciseScrollingDeltas ? 1.0 : 40.0
        let zoom = Float(browser.zoom(of: tab))
        let dx = -Float(event.scrollingDeltaX) * step / zoom
        let dy = -Float(event.scrollingDeltaY) * step / zoom
        guard dx != 0 || dy != 0 else { return }
        browser.scroll(tab, dx: dx, dy: dy)
    }

    /// Pinch to zoom, which on a Mac is how people expect to resize a page.
    override func magnify(with event: NSEvent) {
        guard tab != 0 else { return }
        onZoomGesture?(event.magnification)
    }

    // ── keyboard ──────────────────────────────────────────────────────────

    override var acceptsFirstResponder: Bool { true }

    override func keyDown(with event: NSEvent) {
        guard tab != 0 else { return super.keyDown(with: event) }

        // Command belongs to the menu bar. AppKit has already offered it there by the time
        // this runs, so anything still arriving with Command is unclaimed and should not be
        // smuggled into the page as a keystroke.
        if event.modifierFlags.contains(.command) {
            super.keyDown(with: event)
            return
        }

        if let key = KeyMap.key(for: event) {
            browser.keyDown(tab, key: key, code: KeyMap.code(for: event), modifiers: KeyMap.modifiers(event.modifierFlags))
        }

        // Let the input method have the event too: it is what turns a dead key followed by
        // a vowel into "é", and what makes a CJK candidate window work at all. It calls
        // back into insertText below when it has committed text.
        interpretKeyEvents([event])
    }

    override func keyUp(with event: NSEvent) {
        guard tab != 0, !event.modifierFlags.contains(.command) else { return }
        if let key = KeyMap.key(for: event) {
            browser.keyUp(tab, key: key, code: KeyMap.code(for: event), modifiers: KeyMap.modifiers(event.modifierFlags))
        }
    }

    // ── NSTextInputClient ─────────────────────────────────────────────────
    //
    // Only enough of it to receive committed text. Marked text (an in-progress CJK
    // composition) is accepted and dropped rather than drawn inline: the engine has no API
    // for an inline composition yet, so the alternative is refusing the input entirely.

    func insertText(_ string: Any, replacementRange: NSRange) {
        guard tab != 0 else { return }
        let text: String
        if let attributed = string as? NSAttributedString {
            text = attributed.string
        } else if let plain = string as? String {
            text = plain
        } else {
            return
        }
        // Control characters arrive here as well as through keyDown; the page already got
        // those as named keys and does not want them twice.
        guard !text.isEmpty, text.unicodeScalars.allSatisfy({ $0.value >= 0x20 }) else { return }
        browser.textInput(tab, text: text)
    }

    func setMarkedText(_ string: Any, selectedRange: NSRange, replacementRange: NSRange) {}
    func unmarkText() {}
    func selectedRange() -> NSRange { NSRange(location: NSNotFound, length: 0) }
    func markedRange() -> NSRange { NSRange(location: NSNotFound, length: 0) }
    func hasMarkedText() -> Bool { false }
    func attributedSubstring(forProposedRange range: NSRange, actualRange: NSRangePointer?) -> NSAttributedString? { nil }
    func validAttributesForMarkedText() -> [NSAttributedString.Key] { [] }
    func firstRect(forCharacterRange range: NSRange, actualRange: NSRangePointer?) -> NSRect {
        // The caret's screen position, which an input-method candidate window needs. Without
        // a caret from the engine, the view's own origin is the honest answer.
        window?.convertToScreen(convert(CGRect(x: 0, y: bounds.height, width: 1, height: 16), to: nil)) ?? .zero
    }
    func characterIndex(for point: NSPoint) -> Int { NSNotFound }
    override func doCommand(by selector: Selector) {}
}
