import AppKit

/// The thin loading line under the chrome.
///
/// Hand-drawn rather than an `NSProgressIndicator`: that control has an intrinsic height of
/// its own and fights any constraint that tries to make it two points tall, and it cannot
/// show determinate and indeterminate states without being swapped out. Two rectangles are
/// less code than working around either.
final class ProgressLine: NSView {
    /// 0...1 for a known length, nil for "loading, length unknown".
    var fraction: Double? {
        didSet { needsDisplay = true }
    }

    var isLoading = false {
        didSet {
            needsDisplay = true
            if isLoading { startPulse() } else { stopPulse() }
        }
    }

    private var pulse: Timer?
    private var pulsePhase: CGFloat = 0

    override var isFlipped: Bool { true }

    /// The indeterminate state slides a short bar along, which is the honest way to say
    /// "something is happening and nobody knows how much is left".
    private func startPulse() {
        guard pulse == nil else { return }
        let timer = Timer(timeInterval: 1.0 / 30.0, repeats: true) { [weak self] _ in
            guard let self else { return }
            self.pulsePhase += 0.02
            if self.pulsePhase > 1.3 { self.pulsePhase = -0.3 }
            if self.fraction == nil { self.needsDisplay = true }
        }
        RunLoop.main.add(timer, forMode: .common)
        pulse = timer
    }

    private func stopPulse() {
        pulse?.invalidate()
        pulse = nil
        pulsePhase = 0
    }

    deinit { pulse?.invalidate() }

    override func draw(_ dirtyRect: NSRect) {
        guard isLoading else { return }
        NSColor.controlAccentColor.setFill()
        if let fraction {
            let width = bounds.width * CGFloat(max(0, min(1, fraction)))
            CGRect(x: 0, y: 0, width: width, height: bounds.height).fill()
        } else {
            let width = bounds.width * 0.3
            CGRect(x: bounds.width * pulsePhase, y: 0, width: width, height: bounds.height).fill()
        }
    }
}
