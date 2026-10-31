//! What this build is: version, product name and the commits it was built from (build.rs).

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Beacon's short SHA, `-dirty` for uncommitted changes, `unknown` outside git.
pub const COMMIT: &str = env!("BEACON_COMMIT");
/// The gosub-engine commit. A path dependency, so not implied by [`COMMIT`].
pub const ENGINE_COMMIT: &str = env!("BEACON_ENGINE_COMMIT");

/// "Gosub Beacon Preview" for a preview version, "Gosub Beacon" otherwise. Window titles and
/// About use it.
pub fn product() -> &'static str {
    if is_preview(VERSION) {
        "Gosub Beacon Preview"
    } else {
        "Gosub Beacon"
    }
}

/// "0.1.1-preview.1 (5df7700abc, engine d9045adb12)".
pub fn version_label() -> String {
    format!("{VERSION} ({COMMIT}, engine {ENGINE_COMMIT})")
}

fn is_preview(version: &str) -> bool {
    version.contains("-preview")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_preview_version_is_called_preview() {
        assert!(is_preview("0.1.1-preview.1"));
        assert!(!is_preview("0.1.1"));
        assert!(!is_preview("0.2.0-rc.1"));
    }

    #[test]
    fn the_label_names_both_commits() {
        let label = version_label();
        assert!(label.starts_with(VERSION));
        assert!(label.contains(COMMIT) && label.contains(ENGINE_COMMIT));
    }
}
