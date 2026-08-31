//! The few things the browser needs from the desktop it is running on.
//!
//! Deliberately tiny. Beacon's whole platform surface today is a clipboard write and
//! "open this file in whatever the desktop uses for it" — two operations, at four call
//! sites. Notifications and a global menu bar will belong here when something actually
//! wants them; inventing the methods now would be guessing at signatures with no second
//! implementation to check them against.
//!
//! Choosing a file is *not* here. That already crosses the seam as
//! [`crate::event::BeaconEvent::DownloadOffered`]: core says a download was offered, the
//! frontend runs its own native dialog, and the chosen path comes back as a command. A
//! `save_file_as` on this trait would have to take a callback that re-enters core, which
//! is a worse shape than the event it would duplicate.

use std::path::Path;

/// What a frontend provides so the browser can reach the desktop.
///
/// Implemented once per frontend: GTK today, and whatever hosts Beacon next.
pub trait Platform {
    /// Put `text` on the system clipboard.
    fn copy_text(&self, text: &str);

    /// Open `path` in the desktop's default application for it.
    fn open_path(&self, path: &Path);
}

/// A [`Platform`] that does nothing, for tests and headless use.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullPlatform;

impl Platform for NullPlatform {
    fn copy_text(&self, _text: &str) {}
    fn open_path(&self, _path: &Path) {}
}
