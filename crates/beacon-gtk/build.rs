/// This will be run prior to compiling the project and will compile the resources
fn main() {
    // The GTK frontend does not build on macOS, and Cargo.toml leaves GTK and Skia out of a
    // macOS build, so say what to build instead rather than fail on a missing `gtk4`.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        eprintln!(
            "\n\
             error: the GTK frontend (beacon-gtk, gosub-beacon-gtk) does not build on macOS.\n\
             \n\
             On a Mac, build the Swift app instead:\n\
             \n    \
                 cargo build -p beacon-ffi\n    \
                 cd swift && swift run BeaconMac\n\
             \n\
             See swift/README.md. The egui frontend does build here:\n\
             \n    \
                 cargo build -p gosub-beacon --no-default-features --features egui\n"
        );
        std::process::exit(1);
    }
    // `cfg` in a build script is the host; see the build-dependency in Cargo.toml.
    #[cfg(not(target_os = "macos"))]
    glib_build_tools::compile_resources(&["./resources"], "./resources/resources.gresource.xml", "gosub.gresource");
}
