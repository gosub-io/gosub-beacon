mod application;
mod cookies;
mod dialog;
pub mod engine;
#[allow(dead_code)]
mod fetcher;
mod tab;
mod window;

use crate::application::Application;
use crate::fetcher::Fetcher;
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

fn main() {
    colog::basic_builder()
        .format_file(true)
        .format_indent(Some(2))
        .format_level(true)
        .format_suffix(" ")
        .format_module_path(true)
        .format_source_path(true)
        .format_target(true)
        .filter(None, log::LevelFilter::Error)
        .filter(Some("fetcher"), log::LevelFilter::Trace)
        .filter(Some("gtk"), log::LevelFilter::Info)
        // Our own warnings must not be swallowed by the global Error filter.
        .filter(Some("gosub_beacon"), log::LevelFilter::Warn)
        .init();

    Fetcher::protocols_implemented().iter().for_each(|protocol| {
        println!("Protocol: {}", protocol);
    });

    gtk4::init().unwrap();

    gio::resources_register_include!("gosub.gresource").expect("Failed to register resources.");

    let app = Application::new();
    app.connect_startup(|_| load_css());
    // Keep GTK away from argv: URLs passed on the command line become the startup
    // tabs (see BrowserWindow::new), which plain `app.run()` would reject as unknown
    // options.
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
