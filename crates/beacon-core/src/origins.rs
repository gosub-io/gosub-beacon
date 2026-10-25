//! The request log grouped by who served it: the Network panel's "By origin" view.
//!
//! A page's own origin is rarely the one that sends it the most bytes. Grouping the rows by
//! origin, with the page's own site on top, is what answers "who else does this page talk
//! to, and how much".

use crate::devtools::{NetRequest, RequestState};
use gosub_engine::fork_server::site::site_of;

/// Every request one origin served.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OriginRow {
    /// `scheme://host[:port]`, or the scheme alone for a URL without a host (`data:`).
    pub origin: String,
    /// Whether the origin is on the page's own site (scheme plus registrable domain), which is
    /// what makes `cdn.example.com` first party on `www.example.com`.
    pub first_party: bool,
    pub requests: usize,
    pub failed: usize,
    pub bytes: u64,
    /// The kinds it served, most requested first: `script ×4, image ×2`.
    pub kinds: Vec<(String, usize)>,
}

fn origin_of(url: &url::Url) -> String {
    match url.origin() {
        url::Origin::Tuple(..) => url.origin().ascii_serialization(),
        // Opaque: `data:`, `blob:`, `about:`. There is no host to group by, so the scheme is
        // the group.
        url::Origin::Opaque(_) => format!("{}:", url.scheme()),
    }
}

/// The URL the page in `requests` was loaded from: the last navigation's document request.
pub fn page_url(requests: &[NetRequest]) -> Option<url::Url> {
    requests
        .iter()
        .rev()
        .find(|r| r.kind == "document" && r.initiator == "navigation")
        .and_then(|r| url::Url::parse(&r.url).ok())
}

/// `requests` grouped by origin: first party first, then by bytes, largest first.
pub fn by_origin(requests: &[NetRequest]) -> Vec<OriginRow> {
    let page_site = page_url(requests).map(|url| site_of(&url));
    let mut rows: Vec<OriginRow> = Vec::new();
    for request in requests {
        let Ok(url) = url::Url::parse(&request.url) else { continue };
        let origin = origin_of(&url);
        let index = match rows.iter().position(|row| row.origin == origin) {
            Some(index) => index,
            None => {
                rows.push(OriginRow {
                    origin,
                    first_party: page_site.as_deref() == Some(site_of(&url).as_str()),
                    requests: 0,
                    failed: 0,
                    bytes: 0,
                    kinds: Vec::new(),
                });
                rows.len() - 1
            }
        };
        let row = &mut rows[index];
        row.requests += 1;
        row.bytes += request.received_bytes;
        if request.state == RequestState::Failed {
            row.failed += 1;
        }
        match row.kinds.iter_mut().find(|(kind, _)| *kind == request.kind) {
            Some((_, count)) => *count += 1,
            None => row.kinds.push((request.kind.clone(), 1)),
        }
    }
    for row in &mut rows {
        row.kinds.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    }
    rows.sort_by(|a, b| {
        b.first_party
            .cmp(&a.first_party)
            .then_with(|| b.bytes.cmp(&a.bytes))
            .then_with(|| a.origin.cmp(&b.origin))
    });
    rows
}

/// [`OriginRow::kinds`] as one line: `script ×4, image ×2`.
pub fn format_kinds(kinds: &[(String, usize)]) -> String {
    kinds
        .iter()
        .map(|(kind, count)| if *count == 1 { kind.clone() } else { format!("{kind} ×{count}") })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(url: &str, kind: &str, bytes: u64) -> NetRequest {
        let mut r = NetRequest::new(uuid::Uuid::new_v4(), None, url, 0);
        r.state = RequestState::Finished;
        r.kind = kind.to_string();
        r.initiator = if kind == "document" { "navigation" } else { "parser" }.to_string();
        r.received_bytes = bytes;
        r
    }

    #[test]
    fn the_pages_own_site_comes_first_even_when_others_sent_more() {
        let rows = by_origin(&[
            request("https://www.example.com/", "document", 100),
            request("https://cdn.example.com/a.js", "script", 50),
            request("https://tracker.test/t.js", "script", 9_000),
            request("https://tracker.test/p.gif", "image", 10),
            request("http://www.example.com/x", "image", 1),
        ]);
        let order: Vec<_> = rows.iter().map(|r| (r.origin.as_str(), r.first_party)).collect();
        assert_eq!(
            order,
            [
                ("https://www.example.com", true),
                ("https://cdn.example.com", true),
                ("https://tracker.test", false),
                // Another scheme is another site.
                ("http://www.example.com", false),
            ]
        );
        assert_eq!(rows[2].requests, 2);
        assert_eq!(rows[2].bytes, 9_010);
        assert_eq!(format_kinds(&rows[2].kinds), "image, script");
    }

    #[test]
    fn a_url_without_a_host_groups_by_scheme() {
        let rows = by_origin(&[
            request("https://a.test/", "document", 0),
            request("data:image/png;base64,AAAA", "image", 4),
            request("data:text/css,p{}", "stylesheet", 4),
        ]);
        let data = rows.iter().find(|r| r.origin == "data:").unwrap();
        assert_eq!(data.requests, 2);
        assert!(!data.first_party);
    }

    #[test]
    fn kinds_are_counted_most_first() {
        let rows = by_origin(&[
            request("https://a.test/1.png", "image", 0),
            request("https://a.test/2.png", "image", 0),
            request("https://a.test/s.js", "script", 0),
        ]);
        assert_eq!(format_kinds(&rows[0].kinds), "image ×2, script");
    }
}
