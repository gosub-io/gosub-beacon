using Gosub.Beacon.Windows.Interop;

namespace Gosub.Beacon.Windows;

/// The shell's own lines, into the engine's log and so into beacon.log in the profile.
/// Console.Error goes nowhere in a WinExe.
internal static class Log
{
    public static void Warn(string message) => BeaconNative.LogWrite(BeaconNative.LogLevel.Warn, message);

    public static void Debug(string message) => BeaconNative.LogWrite(BeaconNative.LogLevel.Debug, message);
}
