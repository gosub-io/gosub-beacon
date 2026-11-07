import AppKit

/// The address bar.
///
/// Two departures from a plain `NSTextField`, both because this is a browser:
///
/// - `Ctrl+A` selects all. macOS gives text fields emacs bindings in which it means *move
///   to the beginning of the line*; in a browser address bar every other platform selects,
///   and that is what people reach for. `⌘A` keeps working through the Edit menu.
/// - Clicking an unfocused field selects the whole address, so typing replaces it — Safari
///   and Chrome both do this, and it is the difference between "type a new address" being
///   one gesture or three.
final class AddressField: NSTextField {
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        // performKeyEquivalent is offered to every view in the window, not just the focused
        // one, so this must not fire when the address bar is merely present.
        guard let editor = currentEditor(), window?.firstResponder === editor else {
            return super.performKeyEquivalent(with: event)
        }

        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        // Control alone: Ctrl+Shift+A and Ctrl+Alt+A are left to whatever claims them.
        if flags == .control, event.charactersIgnoringModifiers?.lowercased() == "a" {
            editor.selectAll(nil)
            return true
        }

        return super.performKeyEquivalent(with: event)
    }

    override func mouseDown(with event: NSEvent) {
        let wasFocused = currentEditor() != nil
        super.mouseDown(with: event)
        // super.mouseDown runs the field's own tracking loop and returns once the click is
        // finished, so by here the editor exists and a drag-selection has already happened.
        // Only select everything when the click is what gave the field focus; a second click
        // is someone placing the caret.
        if !wasFocused, let editor = currentEditor(), editor.selectedRange.length == 0 {
            editor.selectAll(nil)
        }
    }
}
