import AppKit
import CBeacon

/// Turning an `NSEvent` into what the web calls a key.
///
/// This mapping lives in the shell on purpose. `KeyboardEvent.code` names a *physical* key
/// and `KeyboardEvent.key` names what that key produces under the current layout — on a
/// French AZERTY board the key labelled A is `KeyQ` — and only the window server knows the
/// layout. The engine is handed both and never has to guess.
enum KeyMap {
    /// Modifier bits in the ABI's terms. `META` is Command, which is what the web calls
    /// `metaKey` and what macOS users reach for where Linux users reach for Control.
    static func modifiers(_ flags: NSEvent.ModifierFlags) -> UInt32 {
        // Wrapped in UInt32(): the header defines these as C macros, and whether the
        // importer types them Int32 or UInt32 is not worth depending on.
        var bits: UInt32 = 0
        if flags.contains(.shift) { bits |= UInt32(BEACON_MOD_SHIFT) }
        if flags.contains(.control) { bits |= UInt32(BEACON_MOD_CONTROL) }
        if flags.contains(.option) { bits |= UInt32(BEACON_MOD_ALT) }
        if flags.contains(.command) { bits |= UInt32(BEACON_MOD_META) }
        return bits
    }

    /// `KeyboardEvent.key`: what the key means. Printable characters are themselves;
    /// everything else gets its DOM name.
    static func key(for event: NSEvent) -> String? {
        if let named = namedKey(event.keyCode) {
            return named
        }
        // charactersIgnoringModifiers keeps Shift (so Shift+a is "A") but drops Command and
        // Option, which is what the DOM wants: Cmd+A is still key "a".
        guard let characters = event.charactersIgnoringModifiers, let first = characters.unicodeScalars.first else {
            return nil
        }
        // Control-modified keys arrive as C0 control characters (Ctrl+A is U+0001); the DOM
        // reports the letter, not the control code.
        if first.value < 0x20 || first.value == 0x7F {
            return nil
        }
        return String(characters.prefix(1))
    }

    /// `KeyboardEvent.code`: which physical key, independent of layout. Virtual key codes
    /// are positional on macOS, which is exactly what `code` means.
    static func code(for event: NSEvent) -> String? {
        physicalCode(event.keyCode)
    }

    /// Keys with no printable character, by macOS virtual key code.
    private static func namedKey(_ keyCode: UInt16) -> String? {
        switch keyCode {
        case 36, 76: return "Enter"          // Return, keypad Enter
        case 48: return "Tab"
        case 49: return " "                  // the DOM calls the space key " "
        case 51: return "Backspace"
        case 53: return "Escape"
        case 117: return "Delete"            // forward delete
        case 115: return "Home"
        case 119: return "End"
        case 116: return "PageUp"
        case 121: return "PageDown"
        case 123: return "ArrowLeft"
        case 124: return "ArrowRight"
        case 125: return "ArrowDown"
        case 126: return "ArrowUp"
        case 122: return "F1"
        case 120: return "F2"
        case 99: return "F3"
        case 118: return "F4"
        case 96: return "F5"
        case 97: return "F6"
        case 98: return "F7"
        case 100: return "F8"
        case 101: return "F9"
        case 109: return "F10"
        case 103: return "F11"
        case 111: return "F12"
        default: return nil
        }
    }

    /// macOS virtual key code to the DOM's physical-key name. Covers the main block; keys
    /// outside it fall back to nil and the engine reads `key` alone.
    private static func physicalCode(_ keyCode: UInt16) -> String? {
        switch keyCode {
        case 0: return "KeyA"
        case 1: return "KeyS"
        case 2: return "KeyD"
        case 3: return "KeyF"
        case 4: return "KeyH"
        case 5: return "KeyG"
        case 6: return "KeyZ"
        case 7: return "KeyX"
        case 8: return "KeyC"
        case 9: return "KeyV"
        case 11: return "KeyB"
        case 12: return "KeyQ"
        case 13: return "KeyW"
        case 14: return "KeyE"
        case 15: return "KeyR"
        case 16: return "KeyY"
        case 17: return "KeyT"
        case 31: return "KeyO"
        case 32: return "KeyU"
        case 34: return "KeyI"
        case 35: return "KeyP"
        case 37: return "KeyL"
        case 38: return "KeyJ"
        case 40: return "KeyK"
        case 45: return "KeyN"
        case 46: return "KeyM"
        case 18: return "Digit1"
        case 19: return "Digit2"
        case 20: return "Digit3"
        case 21: return "Digit4"
        case 22: return "Digit6"
        case 23: return "Digit5"
        case 25: return "Digit9"
        case 26: return "Digit7"
        case 28: return "Digit8"
        case 29: return "Digit0"
        case 24: return "Equal"
        case 27: return "Minus"
        case 33: return "BracketLeft"
        case 30: return "BracketRight"
        case 39: return "Quote"
        case 41: return "Semicolon"
        case 42: return "Backslash"
        case 43: return "Comma"
        case 44: return "Slash"
        case 47: return "Period"
        case 50: return "Backquote"
        case 36: return "Enter"
        case 48: return "Tab"
        case 49: return "Space"
        case 51: return "Backspace"
        case 53: return "Escape"
        case 117: return "Delete"
        case 123: return "ArrowLeft"
        case 124: return "ArrowRight"
        case 125: return "ArrowDown"
        case 126: return "ArrowUp"
        default: return nil
        }
    }
}
