// swift-tools-version:5.9
import PackageDescription

// A native macOS chrome over Beacon's C ABI. Nothing about the browser lives here: this
// package draws a window and forwards gestures, and asks beacon-core for everything else.
//
// Build the Rust side first, from the repository root:
//     cargo build -p beacon-ffi
// then, from this directory:
//     swift run BeaconMac https://example.com
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
            linkerSettings: [
                // Link the cdylib cargo just built, and record an rpath so the binary finds
                // it at runtime without DYLD_LIBRARY_PATH.
                .unsafeFlags([
                    "-L../target/debug",
                    "-lbeacon",
                    // SwiftPM puts the binary at .build/debug or .build/<triple>/debug
                    // depending on version, so record both depths rather than guess. If the
                    // library still is not found at runtime, DYLD_LIBRARY_PATH=../target/debug
                    // is the escape hatch.
                    "-Xlinker", "-rpath", "-Xlinker", "@executable_path/../../../target/debug",
                    "-Xlinker", "-rpath", "-Xlinker", "@executable_path/../../../../target/debug",
                ])
            ]
        ),
    ]
)
