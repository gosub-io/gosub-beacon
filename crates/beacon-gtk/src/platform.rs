//! GTK's answer to [`beacon_core::platform::Platform`].
//!
//! Both operations need a widget to hang off — the clipboard belongs to a display, and
//! `FileLauncher` wants a parent window for its portal prompt — so this holds a weak
//! reference to the window. Weak, because the window owns the `Beacon` that owns this:
//! a strong reference here would be a cycle, and the window would never be dropped.

use std::path::Path;

use beacon_core::platform::Platform;
use gtk4::glib;
use gtk4::prelude::*;

use crate::window::BrowserWindow;

pub struct GtkPlatform {
    window: glib::WeakRef<BrowserWindow>,
}

impl GtkPlatform {
    pub fn new(window: &BrowserWindow) -> Self {
        Self {
            window: window.downgrade(),
        }
    }
}

impl Platform for GtkPlatform {
    fn copy_text(&self, text: &str) {
        let Some(window) = self.window.upgrade() else {
            log::warn!("clipboard write dropped: the window is gone");
            return;
        };
        window.clipboard().set_text(text);
    }

    fn open_path(&self, path: &Path) {
        let Some(window) = self.window.upgrade() else { return };
        let launcher = gtk4::FileLauncher::new(Some(&gtk4::gio::File::for_path(path)));
        launcher.launch(Some(&window), gtk4::gio::Cancellable::NONE, |result| {
            if let Err(e) = result {
                log::warn!("open download failed: {e}");
            }
        });
    }
}
