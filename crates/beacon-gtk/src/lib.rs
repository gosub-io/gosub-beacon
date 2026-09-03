//! Beacon's GTK4 frontend: the application window, tab strip, toolbar and the
//! `GtkGLArea` the engine's frames are composited into.
//!
//! This is one possible host for the browser, not the browser itself. Everything that is
//! not a widget is on its way out to `beacon-core`; what stays here is GTK.

mod application;
mod dialog;
mod fetch;
mod platform;
pub mod render;
mod theme;
mod window;

use crate::application::Application;
use gtk4::gdk::Display;
use gtk4::prelude::{ApplicationExt, ApplicationExtManual};
use gtk4::{gio, CssProvider};
use std::sync::OnceLock;
use tokio::runtime::Runtime;

const APP_ID: &str = "io.gosub.beacon";

fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| Runtime::new().expect("Setting up tokio runtime needs to succeed."))
}

/// Start Beacon under GTK. Blocks until the last window closes.
pub fn run() {
    // `build()` rather than `init()`: the logger is handed to beacon-core, which installs
    // it wrapped so every record it accepts is also kept for the developer pane. colog still
    // decides what the terminal sees; the pane just gets a copy.
    let mut builder = colog::basic_builder();
    builder
        .format_file(true)
        .format_indent(Some(2))
        .format_level(true)
        .format_suffix(" ")
        .format_module_path(true)
        .format_source_path(true)
        .format_target(true)
        .filter(None, log::LevelFilter::Error)
        .filter(Some("gtk"), log::LevelFilter::Info)
        // Our own warnings must not be swallowed by the global Error filter. These match
        // on module path, which starts with the crate name -- so they have to be kept in
        // step with the crate names, or our logging silently goes quiet.
        .filter(Some("beacon_gtk"), log::LevelFilter::Warn)
        .filter(Some("beacon_core"), log::LevelFilter::Warn);

    // Only when actually asked: BEACON_LOG / RUST_LOG raises the global default, which is
    // how the developer pane is made to show more than warnings. Applying it unconditionally
    // would quietly make every run noisier than the filters above intend.
    if let Some(level) = beacon_core::devtools::env_level() {
        builder.filter(None, level);
    }

    // `build()` rather than `init()`: the logger goes to beacon-core, which installs it
    // wrapped so every record it accepts is also kept for the developer pane. colog still
    // decides what the terminal sees; the pane gets a copy.
    let logger = builder.build();
    // `log` drops anything above the global max before a logger is consulted, so this must
    // be at least as permissive as colog's own filtering -- which is what `filter()` reports.
    let max_level = logger.filter();
    beacon_core::devtools::install_logger(Box::new(logger), max_level);

    // Parse argv first: `--help` / `--version` must answer cleanly, without a display.
    // Everything downstream reads the parsed result rather than re-scanning argv.
    let cli = beacon_core::cli::Cli::init();

    gtk4::init().unwrap();

    gio::resources_register_include!("gosub.gresource").expect("Failed to register resources.");

    let app = Application::new();
    app.connect_startup(|_| {
        load_css();
        // Chrome follows the desktop light/dark preference; the manual toggle overrides it
        // until the desktop preference next changes.
        crate::theme::follow_desktop_color_scheme();
    });
    // Keep GTK away from argv: it is ours, already parsed, and plain `app.run()` would
    // reject the URLs and flags as unknown options.
    let _ = cli;
    let argv0: Vec<String> = std::env::args().take(1).collect();
    app.run_with_args(&argv0);
}

fn load_css() {
    let provider = CssProvider::new();
    provider.load_from_string(include_str!("../resources/style.css"));

    gtk4::style_context_add_provider_for_display(
        &Display::default().expect("Could not connect to a display"),
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
