//! egui's answer to [`beacon_core::platform::Platform`].

use std::path::Path;

use beacon_core::platform::Platform;

pub struct EguiPlatform {
    ctx: egui::Context,
}

impl EguiPlatform {
    pub fn new(ctx: egui::Context) -> Self {
        Self { ctx }
    }
}

impl Platform for EguiPlatform {
    fn copy_text(&self, text: &str) {
        self.ctx.copy_text(text.to_string());
    }

    /// Hand the file to the desktop. There is no portable API for this in egui or winit, and
    /// pulling in a crate for one call is not worth it, so this shells out to the platform's
    /// opener. Failure is logged rather than surfaced: the user clicked Open on a file that
    /// is already on disk, and there is nothing useful for them to do about a missing
    /// handler from inside the browser.
    fn open_path(&self, path: &Path) {
        #[cfg(target_os = "macos")]
        let program = "open";
        #[cfg(target_os = "windows")]
        let program = "explorer";
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let program = "xdg-open";

        match std::process::Command::new(program).arg(path).spawn() {
            Ok(_) => {}
            Err(e) => log::warn!("could not open {}: {e}", path.display()),
        }
    }
}
