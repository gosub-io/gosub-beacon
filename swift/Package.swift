// swift-tools-version:5.9
import PackageDescription

// A native macOS chrome over Beacon's C ABI. Nothing about the browser lives here: this
// package draws a window and forwards gestures, and asks beacon-core for everything else.
//
// Build the Rust side first, from the repository root:
//     cargo build -p beacon-ffi
// then, from this directory:
//     swift run BeaconMac https://example.com
// (or `cargo build -p beacon-ffi --release` and `swift run -c release ...`).

// SwiftPM puts the binary at .build/<config> or .build/<triple>/<config> depending on
// version, so record both depths rather than guess. If the library still is not found at
// runtime, DYLD_LIBRARY_PATH=../target/<config> is the escape hatch.
func linkFlags(for config: String) -> [String] {
    [
        "-L../target/\(config)",
        "-lbeacon",
        "-Xlinker", "-rpath", "-Xlinker", "@executable_path/../../../target/\(config)",
        "-Xlinker", "-rpath", "-Xlinker", "@executable_path/../../../../target/\(config)",
    ]
}

let package = Package(
    name: "BeaconMac",
    platforms: [.macOS(.v13)],
    targets: [
        // The C ABI, exposed to Swift through a module map pointing at the real header --
        // no duplicated declarations to drift out of step.
        .systemLibrary(name: "CBeacon", path: "Sources/CBeacon"),
        .executableTarget(
            name: "BeaconMac",
            dependencies: ["CBeacon"],
            // The About artwork, shared in spirit with the GTK shell's GResource copy.
            // SwiftPM will not reach outside the target directory, so these are copies:
            // if the art changes, both places need it.
            resources: [.process("Resources")],
            // Link the cdylib cargo built for the same configuration -- target/debug for
            // `swift run`, target/release for `-c release` and package.sh -- and record an
            // rpath so the binary finds it at runtime without DYLD_LIBRARY_PATH.
            linkerSettings: [
                .unsafeFlags(linkFlags(for: "debug"), .when(configuration: .debug)),
                .unsafeFlags(linkFlags(for: "release"), .when(configuration: .release)),
            ]
        ),
    ]
)
