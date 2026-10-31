using System.Reflection;

namespace Gosub.Beacon.Windows;

/// The version from the csproj and the name the windows carry.
internal static class BuildInfo
{
    /// "0.1.1-preview.1". The informational version keeps the suffix the numeric
    /// assembly version drops; the SDK may append "+commit", which is cut off.
    public static string Version { get; } =
        typeof(BuildInfo).Assembly.GetCustomAttribute<AssemblyInformationalVersionAttribute>()
            ?.InformationalVersion.Split('+')[0] ?? "0.1.0";

    /// "Gosub Beacon Preview" for a preview version.
    public static string Product { get; } =
        Version.Contains("-preview") ? "Gosub Beacon Preview" : "Gosub Beacon";
}
