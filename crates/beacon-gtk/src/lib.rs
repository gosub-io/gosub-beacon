//! Beacon's GTK4 frontend: the application window, tab strip, toolbar and the
//! `GtkGLArea` the engine's frames are composited into.
//!
//! This is one possible host for the browser, not the browser itself. Everything that is
//! not a widget is on its way out to `beacon-core`; what stays here is GTK.

mod application;
mod dialog;
mod picker;
mod platform;
pub mod render;
mod shortcuts;
mod theme;
mod window;

use crate::application::Application;
use gtk4::gdk::Display;
use gtk4::prelude::{ActionGroupExt, ApplicationExt, ApplicationExtManual, ToVariant};
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
    // Must come first: the engine starts each component process by re-executing this
    // binary with a role argument, and a child must reach its role before any of Beacon's
    // startup runs. In Beacon itself this returns at once.
    gosub_engine::child_process::dispatch_with::<crate::render::GtkConfig>();

    // Parse argv next: `--help` / `--version` must answer cleanly, without a display, and
    // the lockdown below needs `--user-data-dir`. Everything downstream reads the parsed
    // result rather than re-scanning argv.
    let cli = beacon_core::cli::Cli::init();

    // The profile next: creating or migrating it writes outside the folders the lockdown
    // leaves open, and the lockdown needs to know which profile those are.
    // On failure the process only shows why and ends, so it never reaches the lockdown.
    if let Err(e) = beacon_core::paths::init() {
        dialog::startup::fail(&e.context("cannot open the profile"));
    }

    // Before any thread, the logger or the engine exist: confine this process to what it
    // writes. In every mode: single-process is where page content runs in this process.
    beacon_core::isolation::lock_down_broker();

    // `build()` rather than `init()`: the logger is handed to beacon-core, which installs
    // it wrapped so every record it accepts is also kept for the developer pane. env_logger still
    // decides what the terminal sees; the pane just gets a copy.
    let mut builder = env_logger::Builder::new();
    builder
        .filter(None, log::LevelFilter::Error)
        .filter(Some("gtk"), log::LevelFilter::Warn)
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
    // wrapped so every record it accepts is also kept for the developer pane. env_logger still
    // decides what the terminal sees; the pane gets a copy.
    let logger = builder.build();
    // `log` drops anything above the global max before a logger is consulted, so this must
    // be at least as permissive as env_logger's own filtering -- which is what `filter()` reports.
    let max_level = logger.filter();
    beacon_core::devtools::install_logger(Box::new(logger), max_level);

    if let Err(e) = gtk4::init() {
        eprintln!("gosub-beacon: cannot open a display: {e}");
        std::process::exit(1);
    }

    gio::resources_register_include!("gosub.gresource").expect("Failed to register resources.");

    let app = Application::new(&app_id());
    app.connect_startup(|_| {
        // Only the primary instance gets here; a second launch has already handed off. So
        // this is where a profile another process holds, or one without a keychain for its
        // key, is refused, before any window. It blocks `startup`, with a main loop of its
        // own, so `activate` only runs once the profile is open.
        if !dialog::startup::open_profile() {
            std::process::exit(1);
        }
        load_css();
        // Chrome follows the desktop light/dark preference; the manual toggle overrides it
        // until the desktop preference next changes.
        crate::theme::follow_desktop_color_scheme();
    });
    // Keep GTK away from argv: it is ours, already parsed, and plain `app.run()` would
    // reject the URLs and flags as unknown options.
    let argv0: Vec<String> = std::env::args().take(1).collect();
    // A second launch only presents the running instance, which never sees this argv: its
    // URLs, and whether it wants a private window, go over as an action first. Registering
    // is what tells the two apart, and `run` registers anyway; both D-Bus calls are flushed
    // before it returns.
    let handover = !cli.urls.is_empty() || cli.private;
    if handover && app.register(gio::Cancellable::NONE).is_ok() && app.is_remote() {
        app.activate_action(application::OPEN_URLS, Some(&(cli.private, cli.urls.clone()).to_variant()));
    }
    app.run_with_args(&argv0);
}

/// One GTK instance per profile. A second launch hands its URLs to the instance with the
/// same id, so a shared id would open `--profile work` as a window of whichever profile is
/// already running. The default profile keeps the plain id the desktop file and Flatpak use;
/// `beaconctl` refuses to move the default while either profile runs, so an id never goes
/// stale under a running instance.
fn app_id() -> String {
    if beacon_core::paths::is_default() {
        return APP_ID.to_string();
    }
    // D-Bus name elements may not start with a digit, hence the prefixes.
    let id = format!("{APP_ID}.Profile{}", beacon_core::paths::profile().id().simple());
    match &beacon_core::cli::Cli::global().user_data_dir {
        // A copied folder keeps its manifest, id and all: the folder itself tells two
        // copies apart, or the second would open as a window of the first.
        Some(dir) => {
            let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.clone());
            format!("{id}.Dir{:016x}", fnv1a(dir.as_os_str().as_encoded_bytes()))
        }
        None => id,
    }
}

/// FNV-1a, 64 bits: the same for a folder in every build, which `DefaultHasher` does not
/// promise.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, b| {
        (hash ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn load_css() {
    let provider = CssProvider::new();
    // One provider for both sheets: the pickers are part of the same chrome, and a second
    // provider would only add a priority to get wrong.
    provider.load_from_string(concat!(
        include_str!("../resources/style.css"),
        include_str!("../resources/picker.css")
    ));

    gtk4::style_context_add_provider_for_display(
        &Display::default().expect("Could not connect to a display"),
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

#[cfg(test)]
mod tests {
    use gtk4::prelude::ActionMapExt;

    /// GTK greys out a menu item whose action nothing registers, without a word.
    #[test]
    fn every_menu_action_is_registered() {
        let app = crate::application::Application::new("io.gosub.beacon.test");
        app.setup_actions();
        crate::window::BrowserWindow::connect_actions(&app);
        let menu = include_str!("../resources/main_menu.ui");
        let actions = menu
            .split("<attribute name=\"action\">app.")
            .skip(1)
            .filter_map(|s| s.split('<').next());
        for action in actions {
            assert!(app.lookup_action(action).is_some(), "menu action app.{action} is not registered");
        }
    }
}
