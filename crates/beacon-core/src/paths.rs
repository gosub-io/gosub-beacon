//! Where Beacon keeps a profile on disk.

use std::path::PathBuf;

/// Per-user data directory holding the profile databases (cookies, local storage,
/// settings, session): `$XDG_DATA_HOME/gosub-beacon`, i.e. `~/.local/share/gosub-beacon`
/// on Linux, `~/Library/Application Support/gosub-beacon` on macOS. Created on first use.
/// Falls back to the working directory if it cannot be created, so a locked-down
/// environment still gets a working (if non-standard) profile.
pub fn data_dir() -> PathBuf {
    // `--user-data-dir` wins over the platform location, so a scratch profile can be used
    // without touching the real one.
    let dir = match &crate::cli::Cli::global().user_data_dir {
        Some(dir) => dir.clone(),
        // Was `glib::user_data_dir()`, which is XDG on every platform. `dirs` follows the
        // platform convention instead, which is what a macOS build will want.
        None => match dirs::data_dir() {
            Some(base) => base.join("gosub-beacon"),
            None => PathBuf::from("."),
        },
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("cannot create data dir {}: {e}; using the working directory", dir.display());
        return PathBuf::from(".");
    }
    dir
}
