//! Gosub Beacon on Android. Everything is in `beacon-egui`; this crate only exists to be
//! the shared library a NativeActivity loads, whose one export is `android_main`.
//!
//! Empty on every other target, so a desktop `cargo build --workspace` still works.

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: winit::platform::android::activity::AndroidApp) {
    beacon_egui::android::run(app);
}
