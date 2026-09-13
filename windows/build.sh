#!/usr/bin/env bash
# Build the Windows shell and the engine DLL it loads, from Linux.
#
# Both halves cross-compile, so only running needs Windows. The MSVC target is required
# twice over: it is the ABI .NET expects, and the one skia-safe publishes prebuilts for.
set -euo pipefail

cd "$(dirname "$0")/.."

PROFILE="${1:-debug}"
TARGET="x86_64-pc-windows-msvc"

# cargo-xwin supplies the MS CRT and Windows SDK; clang-cl and lld-link do the work.
# clang-cl is clang under another name (the driver reads argv[0]), so a symlink is enough
# and no Visual Studio is needed.
command -v cargo-xwin >/dev/null || { echo "need: cargo install cargo-xwin" >&2; exit 1; }
command -v clang-cl   >/dev/null || { echo "need: clang-cl on PATH (symlink it to clang)" >&2; exit 1; }
command -v lld-link   >/dev/null || { echo "need: lld-link on PATH (ships with lld)" >&2; exit 1; }

echo "==> beacon.dll ($PROFILE, $TARGET)"
if [ "$PROFILE" = "release" ]; then
    cargo xwin build -p beacon-ffi --target "$TARGET" --release
else
    cargo xwin build -p beacon-ffi --target "$TARGET"
fi

echo "==> GosubBeacon.exe"
# EnableWindowsTargeting lets the SDK build a WPF project on a non-Windows host. It lives in
# windows/Directory.Build.props, so a hand-run `dotnet build` picks it up too.
dotnet build windows/src/BeaconWindows/BeaconWindows.csproj \
    -c "$([ "$PROFILE" = release ] && echo Release || echo Debug)" \
    --nologo

echo
echo "Built. Copy the output directory to Windows and run GosubBeacon.exe:"
echo "  windows/src/BeaconWindows/bin/$([ "$PROFILE" = release ] && echo Release || echo Debug)/net10.0-windows/win-x64/"
