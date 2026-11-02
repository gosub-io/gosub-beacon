using System.Runtime.InteropServices;
using System.Windows.Input;
using Gosub.Beacon.Windows.Interop;

namespace Gosub.Beacon.Windows;

/// <summary>
/// WPF keys as the web names them: KeyboardEvent.key and .code.
///
/// WPF's Key is a virtual key, which follows the layout, so `code` is an approximation of
/// the physical key, as on GTK; the engine reads only `key` today. What a press types is
/// the TextInput WPF raises after it; only a chord, which types nothing, is looked up here.
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

    /// <summary>The character a key types with only Shift and Caps Lock applied, for a chord:
    /// Ctrl+A types nothing, so its `key` comes from here. The layout decides: Ctrl+Shift+1 is
    /// "!" on a US board.</summary>
    public static string? Character(Key key, bool shift)
    {
        var vk = (uint)KeyInterop.VirtualKeyFromKey(key);
        if (vk == 0)
        {
            return null;
        }

        var state = new byte[256];
        if (shift)
        {
            state[VkShift] = 0x80;
        }

        if (Keyboard.IsKeyToggled(Key.CapsLock))
        {
            state[VkCapital] = 0x01;
        }

        var layout = GetKeyboardLayout(0);
        var buffer = new char[4];
        var count = ToUnicodeEx(vk, MapVirtualKeyEx(vk, MapVkToVsc, layout), state, buffer, buffer.Length,
            DontChangeKeyboardState, layout);
        // 0 is no character, below 0 a dead key; several is a ligature, not one key.
        if (count != 1 || buffer[0] < ' ' || buffer[0] == '\x7f')
        {
            return null;
        }

        return buffer[0].ToString();
    }

    private const int VkShift = 0x10;
    private const int VkCapital = 0x14;
    private const uint MapVkToVsc = 0;

    /// <summary>Leave the keyboard's dead-key state alone: this is a question, not a press.</summary>
    private const uint DontChangeKeyboardState = 4;

    [DllImport("user32.dll")]
    private static extern int ToUnicodeEx(uint virtKey, uint scanCode, byte[] keyState,
        [Out] char[] buffer, int bufferSize, uint flags, IntPtr layout);

    [DllImport("user32.dll")]
    private static extern uint MapVirtualKeyEx(uint code, uint mapType, IntPtr layout);

    [DllImport("user32.dll")]
    private static extern IntPtr GetKeyboardLayout(uint threadId);

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
