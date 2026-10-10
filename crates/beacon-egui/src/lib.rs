//! Beacon's egui frontend: a winit window, egui chrome, and the page rendered through
//! Vello on egui's own wgpu device.
//!
//! The second host for the same browser. Everything about what Beacon *does* is in
//! `beacon-core`; this crate draws it. Where it differs from `beacon-gtk` is instructive:
//! there is no widget tree to keep in step, so most `BeaconEvent`s need no handling at all
//! — the chrome is rebuilt from current state on every frame.

#[cfg(target_os = "android")]
pub mod android;
mod app;
mod chrome;
mod context;
mod platform;

use std::sync::OnceLock;
use tokio::runtime::Runtime;

fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| Runtime::new().expect("Setting up tokio runtime needs to succeed."))
}

/// Start Beacon under egui. Blocks until the window closes.
pub fn run() {
    // Must come first: the engine starts each component process by re-executing this
    // binary with a role argument, and a child must reach its role before any of Beacon's
    // startup runs. In Beacon itself this returns at once. `--isolated` gets this frontend
    // the network, vault and decoder processes; the renderer tier does not apply, because
    // Vello presents a GPU texture and isolated renderers produce CPU tiles.
    gosub_engine::child_process::dispatch_with::<app::EguiConfig>();
    // Then argv (the lockdown needs `--user-data-dir`) and the lockdown, in every mode,
    // before any thread, the logger or the engine exist.
    beacon_core::cli::Cli::init();
    beacon_core::isolation::lock_down_broker();

    // RUST_LOG wins when it is set, so a diagnostic run needs no rebuild:
    //   RUST_LOG=beacon_egui=debug ./gosub-beacon-egui https://example.org
    let mut builder = colog::basic_builder();
    builder
        .filter(None, log::LevelFilter::Error)
        .filter(Some("beacon_egui"), log::LevelFilter::Warn)
        .filter(Some("beacon_core"), log::LevelFilter::Warn);
    if let Ok(spec) = std::env::var("RUST_LOG") {
        builder.parse_filters(&spec);
    }
    builder.init();

    let cli = beacon_core::cli::Cli::init();
    let urls = cli.urls.clone();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Gosub Beacon")
            .with_inner_size([1024.0, 768.0]),
        // Vello needs wgpu; there is no software path here.
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    start(options, urls);
}

/// Run the event loop with `options` until the window closes, opening `urls` as startup
/// tabs. Shared by the desktop entry above and the Android one, which differ only in how
/// they get here.
fn start(options: eframe::NativeOptions, urls: Vec<String>) {
    let result = eframe::run_native(
        "Gosub Beacon",
        options,
        Box::new(move |cc| {
            app::BeaconApp::new(cc, runtime(), urls)
                .map(|app| Box::new(app) as Box<dyn eframe::App>)
                .map_err(|e| e.to_string().into())
        }),
    );
    if let Err(e) = result {
        log::error!("Beacon could not start: {e}");
    }
}
