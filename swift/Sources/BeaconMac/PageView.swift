import AppKit
import CBeacon

/// The view Beacon draws the page into.
///
/// AppKit lays it out among the chrome like any other subview; what appears inside it is
/// rendered by Rust on the GPU, straight into this view's layer. Nothing is copied and this
/// class never touches a pixel.
final class PageView: NSView {
    private let browser: Browser
    private var tab: BeaconTabId = 0
    private var attached = false
    private var tracking: NSTrackingArea?

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
        attachIfPossible()
    }

    private func attachIfPossible() {
        guard !attached, tab != 0, window != nil else { return }
        let size = devicePixelSize
        guard size.width > 0, size.height > 0 else { return }

        // The view pointer is what Rust wraps as a surface. It must outlive the attachment,
        // which is why `detach` happens in viewWillMove(toWindow:) below.
        let pointer = Unmanaged.passUnretained(self).toOpaque()
        attached = browser.attach(tab, to: pointer, width: size.width, height: size.height)
        if attached {
            sendViewport()
        } else {
            NSLog("beacon: could not attach a view for tab \(tab)")
        }
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        attachIfPossible()
        updateTracking()
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

    private func sendViewport() {
        guard tab != 0 else { return }
        // CSS pixels: the logical size, not the backing size.
        let width = UInt32(max(0, bounds.width))
        let height = UInt32(max(0, bounds.height))
        guard width > 0, height > 0 else { return }
        browser.setViewport(tab, width: width, height: height)
    }

    /// Repaint. Called when a redraw event arrives, not from `draw(_:)` — the page is not
    /// drawn with Core Graphics, so AppKit's own drawing cycle is not involved.
    func redraw() {
        guard attached, tab != 0 else { return }
        browser.draw(tab)
    }

    // ── input ─────────────────────────────────────────────────────────────

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

    private func pagePoint(_ event: NSEvent) -> (Float, Float) {
        let p = convert(event.locationInWindow, from: nil)
        return (Float(p.x), Float(p.y))
    }

    override func mouseMoved(with event: NSEvent) {
        guard tab != 0 else { return }
        let (x, y) = pagePoint(event)
        browser.mouseMoved(tab, x: x, y: y)
    }

    override func mouseDown(with event: NSEvent) {
        guard tab != 0 else { return }
        let (x, y) = pagePoint(event)
        browser.mouseDown(tab, x: x, y: y)
    }

    override func scrollWheel(with event: NSEvent) {
        guard tab != 0 else { return }
        // AppKit reports a wheel notch as ±1 lines and a trackpad as precise deltas; the
        // engine scrolls in CSS pixels, so lines are scaled and precise deltas passed
        // through. Signs are inverted: scrolling down moves the page up.
        let scale: Float = event.hasPreciseScrollingDeltas ? 1.0 : 40.0
        let dx = -Float(event.scrollingDeltaX) * scale
        let dy = -Float(event.scrollingDeltaY) * scale
        guard dx != 0 || dy != 0 else { return }
        browser.scroll(tab, dx: dx, dy: dy)
    }

    override var acceptsFirstResponder: Bool { true }
}
