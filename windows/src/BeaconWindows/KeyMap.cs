using System.Windows.Input;
using Gosub.Beacon.Windows.Interop;

namespace Gosub.Beacon.Windows;

/// <summary>
/// WPF keys as the web names them: KeyboardEvent.key and .code.
///
/// WPF's Key is a virtual key, which follows the layout, so `code` is an approximation of
/// the physical key, as on GTK; the engine reads only `key` today. What a press types is
/// not worked out here: that is the TextInput WPF raises after it.
/// </summary>
internal static class KeyMap
{
    public static BeaconNative.Modifiers Modifiers(ModifierKeys keys)
    {
        var mods = BeaconNative.Modifiers.None;
        if ((keys & ModifierKeys.Shift) != 0)
        {
            mods |= BeaconNative.Modifiers.Shift;
        }

        if ((keys & ModifierKeys.Control) != 0)
        {
            mods |= BeaconNative.Modifiers.Control;
        }

        if ((keys & ModifierKeys.Alt) != 0)
        {
            mods |= BeaconNative.Modifiers.Alt;
        }

        if ((keys & ModifierKeys.Windows) != 0)
        {
            mods |= BeaconNative.Modifiers.Meta;
        }

        return mods;
    }

    /// <summary>The DOM name of a key that is not sent as the character it types.</summary>
    public static string? Named(Key key) => key switch
    {
        Key.Enter => "Enter",
        Key.Tab => "Tab",
        // The DOM calls the space key " ".
        Key.Space => " ",
        Key.Back => "Backspace",
        Key.Escape => "Escape",
        Key.Delete => "Delete",
        Key.Home => "Home",
        Key.End => "End",
        Key.PageUp => "PageUp",
        Key.PageDown => "PageDown",
        Key.Left => "ArrowLeft",
        Key.Right => "ArrowRight",
        Key.Up => "ArrowUp",
        Key.Down => "ArrowDown",
        >= Key.F1 and <= Key.F12 => $"F{key - Key.F1 + 1}",
        _ => null,
    };

    /// <summary>The character of a letter or digit key, for a chord: Ctrl+A types nothing,
    /// so its `key` comes from here.</summary>
    public static string? Character(Key key, bool shift) => key switch
    {
        >= Key.A and <= Key.Z => ((char)((shift ? 'A' : 'a') + (key - Key.A))).ToString(),
        >= Key.D0 and <= Key.D9 => ((char)('0' + (key - Key.D0))).ToString(),
        _ => null,
    };

    /// <summary>KeyboardEvent.code for the main block, or null.</summary>
    public static string? Code(Key key) => key switch
    {
        >= Key.A and <= Key.Z => $"Key{(char)('A' + (key - Key.A))}",
        >= Key.D0 and <= Key.D9 => $"Digit{key - Key.D0}",
        >= Key.NumPad0 and <= Key.NumPad9 => $"Numpad{key - Key.NumPad0}",
        Key.Space => "Space",
        Key.OemMinus => "Minus",
        Key.OemPlus => "Equal",
        Key.OemComma => "Comma",
        Key.OemPeriod => "Period",
        Key.Oem1 => "Semicolon",
        Key.Oem2 => "Slash",
        Key.Oem3 => "Backquote",
        Key.Oem4 => "BracketLeft",
        Key.Oem5 => "Backslash",
        Key.Oem6 => "BracketRight",
        Key.Oem7 => "Quote",
        _ => Named(key),
    };
}
