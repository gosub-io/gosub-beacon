//! The Android entry: the same frontend, started from a NativeActivity instead of `main`.
//!
//! What differs from the desktop is only how Beacon gets going. There is no argv, so the
//! command line is built here, with the profile pointed at the app's private storage
//! (`dirs` has no data dir on Android, and the fallback, the working directory, is `/`).
//! There is no stderr either, so the log goes to logcat. And there are no component
//! processes: the engine runs in-process, as it does whenever isolation is off.

use std::sync::OnceLock;

use beacon_core::cli::Cli;
use winit::platform::android::activity::AndroidApp;

static APP: OnceLock<AndroidApp> = OnceLock::new();

/// Start Beacon in the activity `app`. Called from the cdylib's `android_main`; returns
/// when the activity is destroyed.
pub fn run(app: AndroidApp) {
    // RUST_LOG cannot be set for an app, so the levels are fixed here; read them with
    //   adb logcat -s beacon RustStdoutStderr
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("beacon")
            .with_max_level(log::LevelFilter::Info)
            .with_filter(
                android_logger::FilterBuilder::new()
                    .parse("warn,beacon_egui=info,beacon_core=info")
                    .build(),
            ),
    );

    Cli::set(Cli {
        user_data_dir: app.internal_data_path(),
        ..Cli::default()
    });
    let _ = APP.set(app.clone());
    init_certificate_verifier(&app);

    let options = eframe::NativeOptions {
        android_app: Some(app),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    crate::start(options, Vec::new());
}

/// Give rustls-platform-verifier the JVM and the activity, so HTTPS fetches can check
/// certificates against Android's trust store. Without it the first fetch panics. The Java
/// half comes from the verifier's own `.aar`, which `scripts/android-apk.sh` dexes into the
/// APK; the activity's class loader is what finds it.
fn init_certificate_verifier(app: &AndroidApp) {
    // SAFETY: both pointers belong to the running NativeActivity and outlive this call: the
    // JVM for the process, the activity object (a global reference) for the activity.
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    let result = vm.attach_current_thread(|env| -> Result<(), jni::errors::Error> {
        let activity = unsafe { jni::objects::JObject::from_raw(env, app.activity_as_ptr().cast()) };
        rustls_platform_verifier::android::init_with_env(env, activity)
    });
    if let Err(e) = result {
        log::error!("cannot set up certificate verification, HTTPS will fail: {e}");
    }
}

/// Raise or drop the on-screen keyboard. winit does not do this for a NativeActivity, so
/// the frontend asks when a text field gains or loses focus.
pub fn show_keyboard(show: bool) {
    if let Some(app) = APP.get() {
        if show {
            app.show_soft_input(false);
        } else {
            app.hide_soft_input(false);
        }
    }
}

/// Put Beacon in the background, as Back does once there is nothing left to go back to.
/// winit reports every key it forwards as handled, so Android's own Back (finish the
/// activity) never runs; Beacon goes to the background with its tabs kept, as other
/// browsers do.
pub fn move_to_background() {
    let Some(app) = APP.get() else { return };
    // SAFETY: as in `init_certificate_verifier`.
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    let result = vm.attach_current_thread(|env| -> Result<(), jni::errors::Error> {
        let activity = unsafe { jni::objects::JObject::from_raw(env, app.activity_as_ptr().cast()) };
        env.call_method(
            &activity,
            jni::jni_str!("moveTaskToBack"),
            jni::jni_sig!("(Z)Z"),
            &[jni::JValue::Bool(true)],
        )?;
        Ok(())
    });
    if let Err(e) = result {
        log::warn!("cannot move Beacon to the background: {e}");
    }
}

/// The part of the screen the app may draw in, in physical pixels: the window minus the
/// status and navigation bars. `None` before the activity has reported it.
pub fn content_rect() -> Option<(i32, i32, i32, i32)> {
    let rect = APP.get()?.content_rect();
    Some((rect.left, rect.top, rect.right, rect.bottom))
}

/// Size the chrome for fingers: egui's defaults are for a mouse, and at a phone's density
/// they leave the address bar text and the buttons too small to read or hit. Only the
/// chrome changes; the page keeps its own CSS sizes.
pub fn touch_style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        use egui::{FontFamily, FontId, TextStyle};
        style
            .text_styles
            .insert(TextStyle::Body, FontId::new(18.0, FontFamily::Proportional));
        style
            .text_styles
            .insert(TextStyle::Button, FontId::new(18.0, FontFamily::Proportional));
        style
            .text_styles
            .insert(TextStyle::Small, FontId::new(14.0, FontFamily::Proportional));
        style.spacing.interact_size.y = 36.0;
        style.spacing.button_padding = egui::vec2(8.0, 6.0);
    });
}
