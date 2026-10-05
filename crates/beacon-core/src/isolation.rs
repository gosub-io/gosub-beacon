//! The broker side of process isolation: confining Beacon's own process.
//!
//! The component processes (network, vault, decoders, renderers) are the engine's; see
//! `docs/process-isolation.md` there. What is Beacon's is the embedder contract's second
//! step: once `child_process::dispatch_with` has run, lock this process down to the
//! directories it legitimately writes.

use std::path::{Path, PathBuf};

/// Confine this process's filesystem writes to its profile, the downloads directory and the
/// temp dir, and drop the escalation syscalls. Call right after `dispatch_with`, before any
/// thread, the logger or the engine exist; everything the process writes to after this
/// point must be listed here or created under one of these.
///
/// `/dev/dri` is granted because the GTK frontend composites on GL, and EGL opens the render
/// node read-write; Landlock counts that as a file write.
pub fn lock_down_broker() {
    let mut writable: Vec<PathBuf> = vec![crate::paths::data_dir()];
    // Downloads go wherever the save dialog points; anywhere outside these fails under
    // the lockdown.
    if let Some(downloads) = dirs::download_dir() {
        writable.push(downloads);
    }
    if cfg!(target_os = "linux") {
        writable.push(PathBuf::from("/dev/dri"));
        // GTK reads its settings through dconf, which keeps a per-user cache under the
        // runtime dir and complains on every read when it cannot write it.
        if let Some(runtime) = dirs::runtime_dir() {
            writable.push(runtime.join("dconf"));
        }
    }
    let writable: Vec<&Path> = writable.iter().map(PathBuf::as_path).collect();
    gosub_engine::child_process::lock_down_broker(&writable);
}
