# Build the Windows release zip, on Windows: beacon.dll in release, the shell published
# self-contained, and the VC++ runtime DLLs beacon.dll needs copied next to it (Microsoft's
# app-local deployment), so neither .NET nor the VC++ redistributable has to be installed.
# CI uploads what this makes; test that zip, not a local build.
#
#   windows\package.ps1            -> target\windows\GosubBeacon-<version>-<build>-<commit>-engine-<engine>-win-x64.zip
$ErrorActionPreference = "Stop"

$root = Resolve-Path "$PSScriptRoot\.."
Set-Location $root

# The toolchain releases are built with; RUSTUP_TOOLCHAIN overrides it.
if (-not $env:RUSTUP_TOOLCHAIN) {
    $env:RUSTUP_TOOLCHAIN = (Get-Content scripts\release-toolchain).Trim()
}

$target = "x86_64-pc-windows-msvc"
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches.Groups[1].Value
$out = Join-Path $root "target\windows"
$stage = Join-Path $out "GosubBeacon"

# Same identity rules as swift/package.sh: the short SHA (10), "-dirty" when tracked files
# changed, "unknown" outside a git checkout. The engine is the ..\gosub-engine checkout.
# try: under "Stop", Windows PowerShell turns a native command's stderr into an exception.
function GitOutput {
    try { $out = git @args 2>$null } catch { return $null }
    if ($LASTEXITCODE) { return $null }
    return $out
}

function Identity($dir) {
    $sha = GitOutput -C $dir rev-parse --short=10 HEAD
    if (-not $sha) { return "unknown" }
    if (GitOutput -C $dir status --porcelain --untracked-files=no) { return "$sha-dirty" }
    return $sha
}

$build = GitOutput rev-list --count HEAD
if (-not $build) { $build = "0" }
$commit = Identity $root
$engine = Identity (Join-Path $root "..\gosub-engine")
$zip = Join-Path $out "GosubBeacon-$version-$build-$commit-engine-$engine-win-x64.zip"

Write-Host "==> beacon.dll (release, $target)"
cargo build -p beacon-ffi --release --target $target
if ($LASTEXITCODE) { exit $LASTEXITCODE }

# Not +crt-static: skia-binaries publishes no static-CRT build of our feature set, and
# compiling Skia instead needs LLVM and a long build. The runtime DLLs ship app-local.
$vs = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -property installationPath
$crt = Get-ChildItem "$vs\VC\Redist\MSVC\*\x64\Microsoft.VC14*.CRT" -Directory |
    Sort-Object FullName | Select-Object -Last 1
if (-not $crt) { throw "no VC++ redistributable folder under $vs" }

Write-Host "==> GosubBeacon.exe (self-contained)"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
dotnet publish windows\src\BeaconWindows\BeaconWindows.csproj -c Release --self-contained true -o $stage --nologo
if ($LASTEXITCODE) { exit $LASTEXITCODE }
if (-not (Test-Path (Join-Path $stage "beacon.dll"))) {
    throw "beacon.dll is not in the published folder"
}

foreach ($runtime in "msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll") {
    Copy-Item (Join-Path $crt.FullName $runtime) $stage
}
Copy-Item windows\README-zip.txt (Join-Path $stage "README.txt")

Write-Host "==> $zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path $stage -DestinationPath $zip
Write-Host "Built $zip"
