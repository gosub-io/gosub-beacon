//! Session persistence: which tabs are open, saved as the browser runs and restored on
//! the next start (when no URLs are given on the command line).
//!
//! The file is one tab per line in strip order, `[*][!]<url>` — `*` marks the active
//! tab, `!` a pinned one, sealed with the profile key like the rest of the profile (it
//! names every open page). Private windows never write. One session covers one window;
//! with several normal windows open the last writer wins.

/// One open tab, as remembered across restarts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTab {
    pub url: String,
    pub pinned: bool,
    pub active: bool,
}

const FILE: &str = "session.tabs";

/// The previous session's tabs; empty when there is none (or it cannot be read).
pub fn load() -> Vec<SessionTab> {
    match crate::paths::read_profile_file(FILE) {
        Ok(Some(content)) => parse(&String::from_utf8_lossy(&content)),
        Ok(None) => Vec::new(),
        Err(e) => {
            log::warn!("cannot read the previous session: {e:#}");
            Vec::new()
        }
    }
}

/// Persist the current tabs. Written via a temp file so a crash mid-write cannot
/// truncate the previous session.
pub fn save(tabs: &[SessionTab]) {
    if let Err(e) = crate::paths::write_profile_file(FILE, serialize(tabs).as_bytes()) {
        log::warn!("cannot save the session: {e:#}");
    }
}

fn serialize(tabs: &[SessionTab]) -> String {
    let mut out = String::from("# gosub-beacon session v1\n");
    for tab in tabs {
        if tab.active {
            out.push('*');
        }
        if tab.pinned {
            out.push('!');
        }
        out.push_str(&tab.url);
        out.push('\n');
    }
    out
}

fn parse(content: &str) -> Vec<SessionTab> {
    content
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|mut line| {
            let active = line.starts_with('*');
            if active {
                line = &line[1..];
            }
            let pinned = line.starts_with('!');
            if pinned {
                line = &line[1..];
            }
            SessionTab {
                url: line.to_string(),
                pinned,
                active,
            }
        })
        .filter(|t| !t.url.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_preserves_order_and_flags() {
        let tabs = vec![
            SessionTab {
                url: "https://a.example/".into(),
                pinned: true,
                active: false,
            },
            SessionTab {
                url: "https://b.example/x?y=1".into(),
                pinned: false,
                active: true,
            },
            SessionTab {
                url: "gosub://home".into(),
                pinned: false,
                active: false,
            },
        ];
        assert_eq!(parse(&serialize(&tabs)), tabs);
    }

    #[test]
    fn parse_skips_comments_and_blank_lines() {
        let tabs = parse("# header\n\n*!https://a.example/\n\n# trailing\n");
        assert_eq!(
            tabs,
            vec![SessionTab {
                url: "https://a.example/".into(),
                pinned: true,
                active: true,
            }]
        );
    }

    #[test]
    fn empty_content_is_an_empty_session() {
        assert!(parse("").is_empty());
        assert!(parse("# only a comment\n").is_empty());
    }
}
