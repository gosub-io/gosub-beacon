import AppKit

/// An alert for something that did not work: the reason as its message, and the hint and
/// the error underneath as selectable text, since a hint may be a command to copy.
enum FailureAlert {
    /// Run modally. True for "Try Again", which is only offered when `retry` is.
    @discardableResult
    static func run(_ failure: Browser.Failure, retry: Bool = false) -> Bool {
        let alert = make(failure)
        if retry {
            alert.addButton(withTitle: "Try Again")
            alert.addButton(withTitle: "Quit")
        } else {
            alert.addButton(withTitle: "OK")
        }
        NSApp.activate(ignoringOtherApps: true)
        let answer = alert.runModal()
        return retry && answer == .alertFirstButtonReturn
    }

    /// As a sheet on `window`, or modally when there is none.
    static func show(_ failure: Browser.Failure, on window: NSWindow?) {
        guard let window else {
            run(failure)
            return
        }
        make(failure).beginSheetModal(for: window)
    }

    private static func make(_ failure: Browser.Failure) -> NSAlert {
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = failure.reason
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 8
        if let hint = failure.hint {
            stack.addArrangedSubview(label(hint, size: NSFont.systemFontSize))
        }
        if !failure.detail.isEmpty {
            let detail = label(failure.detail, size: NSFont.smallSystemFontSize)
            detail.textColor = .secondaryLabelColor
            stack.addArrangedSubview(detail)
        }
        if !stack.arrangedSubviews.isEmpty {
            // NSAlert sizes the accessory by its frame, so wrap at a fixed width first.
            stack.widthAnchor.constraint(equalToConstant: 360).isActive = true
            stack.frame.size = NSSize(width: 360, height: stack.fittingSize.height)
            alert.accessoryView = stack
        }
        return alert
    }

    private static func label(_ text: String, size: CGFloat) -> NSTextField {
        let label = NSTextField(wrappingLabelWithString: text)
        label.isSelectable = true
        label.font = .systemFont(ofSize: size)
        label.preferredMaxLayoutWidth = 360
        return label
    }
}
