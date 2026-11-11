//! Session persistence: which tabs are open, saved as the browser runs and restored on
//! the next start (when no URLs are given on the command line).
//!
//! The file is one tab per line in strip order, `[*][!]<url>` - `*` marks the active
//! tab, `!` a pinned one - with a blank line between windows. Sealed with the profile key
//! like the rest of the profile (it names every open page). Private windows never write.
//! A v1 file is a session of one window, and a build that reads v1 takes every window's
//! tabs as one.

/// One open tab, as remembered across restarts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTab {
    pub url: String,
    pub pinned: bool,
    pub active: bool,
}

const FILE: &str = "session.tabs";

/// The previous session's tabs, every window's in one list with the first window's active
/// tab, for a shell with one window. Empty when there is none (or it cannot be read).
pub fn load() -> Vec<SessionTab> {
    merge(load_windows())
}

fn merge(windows: Vec<Vec<SessionTab>>) -> Vec<SessionTab> {
    let mut windows = windows.into_iter();
    let mut tabs = windows.next().unwrap_or_default();
    tabs.extend(windows.flatten().map(|t| SessionTab { active: false, ..t }));
    tabs
}

/// The previous session's windows, each with at least one tab; empty when there is none
/// (or it cannot be read).
pub fn load_windows() -> Vec<Vec<SessionTab>> {
    match crate::paths::read_profile_file(FILE) {
        Ok(Some(content)) => parse(&String::from_utf8_lossy(&content)),
        Ok(None) => Vec::new(),
        Err(e) => {
            log::warn!("cannot read the previous session: {e:#}");
            Vec::new()
        }
    }
}

/// Persist the tabs of a shell with one window.
pub fn save(tabs: &[SessionTab]) {
    write(&serialize(&[tabs]));
}

/// Persist every window's tabs.
pub fn save_windows(windows: &[Vec<SessionTab>]) {
    write(&serialize(windows));
}

/// Written via a temp file so a crash mid-write cannot truncate the previous session.
fn write(content: &str) {
    if let Err(e) = crate::paths::write_profile_file(FILE, content.as_bytes()) {
        log::warn!("cannot save the session: {e:#}");
    }
}

fn serialize<W: AsRef<[SessionTab]>>(windows: &[W]) -> String {
    let mut out = String::from("# gosub-beacon session v2\n");
    for (i, window) in windows.iter().map(AsRef::as_ref).filter(|w| !w.is_empty()).enumerate() {
        if i > 0 {
            out.push('\n');
        }
        for tab in window {
            if tab.active {
                out.push('*');
            }
            if tab.pinned {
                out.push('!');
            }
            out.push_str(&tab.url);
            out.push('\n');
        }
    }
    out
}

fn parse(content: &str) -> Vec<Vec<SessionTab>> {
    let mut windows = vec![Vec::new()];
    for mut line in content.lines() {
        if line.is_empty() {
            if windows.last().is_some_and(|w| !w.is_empty()) {
                windows.push(Vec::new());
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let active = line.starts_with('*');
        if active {
            line = &line[1..];
        }
        let pinned = line.starts_with('!');
        if pinned {
            line = &line[1..];
        }
        if !line.is_empty() {
            windows.last_mut().unwrap().push(SessionTab {
                url: line.to_string(),
                pinned,
                active,
            });
        }
    }
    windows.retain(|w| !w.is_empty());
    windows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(url: &str, pinned: bool, active: bool) -> SessionTab {
        SessionTab {
            url: url.into(),
            pinned,
            active,
        }
    }

    #[test]
    fn roundtrip_preserves_windows_order_and_flags() {
        let windows = vec![
            vec![tab("https://a.example/", true, false), tab("https://b.example/x?y=1", false, true)],
            vec![tab("gosub://home", false, true)],
        ];
        assert_eq!(parse(&serialize(&windows)), windows);
    }

    #[test]
    fn a_window_without_tabs_is_not_written() {
        let windows = vec![vec![], vec![tab("https://a.example/", false, true)], vec![]];
        assert_eq!(serialize(&windows), "# gosub-beacon session v2\n*https://a.example/\n");
    }

    #[test]
    fn a_v1_file_is_one_window() {
        let v1 = "# gosub-beacon session v1\n!https://a.example/\n*https://b.example/\n";
        assert_eq!(
            parse(v1),
            vec![vec![tab("https://a.example/", true, false), tab("https://b.example/", false, true)]]
        );
    }

    #[test]
    fn parse_skips_comments_and_extra_blank_lines() {
        let windows = parse("# header\n\n*!https://a.example/\n\n\n# between\nhttps://b.example/\n\n");
        assert_eq!(
            windows,
            vec![
                vec![tab("https://a.example/", true, true)],
                vec![tab("https://b.example/", false, false)]
            ]
        );
    }

    #[test]
    fn one_window_takes_every_window_with_the_first_active_tab() {
        let windows = vec![
            vec![tab("https://a.example/", false, true)],
            vec![tab("https://b.example/", true, true)],
        ];
        assert_eq!(
            merge(windows),
            vec![tab("https://a.example/", false, true), tab("https://b.example/", true, false)]
        );
    }

    #[test]
    fn empty_content_is_an_empty_session() {
        assert!(parse("").is_empty());
        assert!(parse("# only a comment\n").is_empty());
        assert!(merge(Vec::new()).is_empty());
    }
}
