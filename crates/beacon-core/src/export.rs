//! The request log, written out for other tools: a HAR file for a whole page, a `curl`
//! command line for one request.
//!
//! Both are built from the rows [`devtools`](crate::devtools) already keeps, so they say
//! exactly what the Network panel says and nothing it does not: no request bodies (the
//! engine does not report them), no HTTP version, and only the response bodies that were
//! captured while the panel was open.

use crate::devtools::{NetRequest, RequestState};
use serde_json::{json, Value};

/// Request and response headers a HAR file never carries in the clear.
///
/// A HAR file is made to be passed around -- attached to a bug, mailed to a colleague -- and
/// the panel shows these values while it is open, so an export taken straight from the rows
/// would hand out a live session. Other browsers settled on the same default.
const SECRET_HEADERS: &[&str] = &["cookie", "set-cookie", "authorization", "proxy-authorization"];

fn is_secret(name: &str) -> bool {
    SECRET_HEADERS.iter().any(|secret| name.eq_ignore_ascii_case(secret))
}

fn headers(headers: &[(String, String)]) -> Value {
    headers
        .iter()
        .map(|(name, value)| {
            let value = if is_secret(name) { gosub_engine::REDACTED } else { value.as_str() };
            json!({ "name": name, "value": value })
        })
        .collect()
}

fn query_string(url: &str) -> Value {
    let Ok(url) = url::Url::parse(url) else {
        return json!([]);
    };
    url.query_pairs()
        .map(|(name, value)| json!({ "name": name, "value": value }))
        .collect()
}

/// Milliseconds since the epoch as the ISO 8601 a HAR file wants.
fn iso_time(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// The HAR `timings` block, and the entry's total time, which the format defines as their sum.
///
/// The log has no send or TLS split, so `send` is 0 and `ssl` unknown. Connection setup
/// encloses name resolution here, while HAR counts them apart, hence the subtraction.
fn timings(request: &NetRequest) -> (f64, Value) {
    let ms = |us: u64| us as f64 / 1000.0;
    let total = request.elapsed_us.map(ms).unwrap_or(0.0);
    let dns = request.dns_us.map(ms);
    let connect = request.connect_us.map(|us| ms(us.saturating_sub(request.dns_us.unwrap_or(0))));
    let setup = dns.unwrap_or(0.0) + connect.unwrap_or(0.0);
    let (wait, receive) = match request.headers_ms {
        Some(headers) => {
            let to_headers = headers.saturating_sub(request.started_ms) as f64;
            ((to_headers - setup).max(0.0), (total - to_headers).max(0.0))
        }
        None => ((total - setup).max(0.0), 0.0),
    };
    let time = setup + wait + receive;
    (
        time,
        json!({
            "blocked": -1,
            "dns": dns.unwrap_or(-1.0),
            "connect": connect.unwrap_or(-1.0),
            "ssl": -1,
            "send": 0,
            "wait": wait,
            "receive": receive,
        }),
    )
}

fn content(request: &NetRequest) -> Value {
    let mime = request.content_type.clone().unwrap_or_default();
    let mut content = json!({ "size": request.received_bytes, "mimeType": mime });
    // Only text: a binary preview is the first few kilobytes of a file, which no HAR reader
    // could do anything with.
    if let Some(text) = request.body.as_deref().and_then(|body| std::str::from_utf8(body).ok()) {
        content["text"] = json!(text);
        if request.body_truncated {
            content["comment"] = json!("only the start of the body was captured");
        }
    }
    content
}

fn entry(request: &NetRequest, page: Option<usize>) -> Value {
    let (time, timings) = timings(request);
    let redirect = request.redirects.last().map(|(_, location)| location.as_str()).unwrap_or("");
    let mut entry = json!({
        "startedDateTime": iso_time(request.started_ms),
        "time": time,
        "request": {
            "method": request.method.as_deref().unwrap_or("GET"),
            "url": request.url,
            "httpVersion": "",
            "cookies": [],
            "headers": headers(&request.request_headers),
            "queryString": query_string(&request.url),
            "headersSize": -1,
            "bodySize": -1,
        },
        "response": {
            // HAR has no "no response": 0 is what other browsers write for a request that
            // failed or never finished.
            "status": request.status.unwrap_or(0),
            "statusText": "",
            "httpVersion": "",
            "cookies": [],
            "headers": headers(&request.headers),
            "content": content(request),
            "redirectURL": redirect,
            "headersSize": -1,
            "bodySize": if request.state == RequestState::Finished { request.received_bytes as i64 } else { -1 },
        },
        "cache": {},
        "timings": timings,
    });
    if let Some(page) = page {
        entry["pageref"] = json!(format!("page_{page}"));
    }
    if let Some(error) = &request.error {
        entry["comment"] = json!(error);
    }
    entry
}

/// Whether `request` is a page load, which opens a new HAR page.
fn starts_page(request: &NetRequest) -> bool {
    request.kind == "document" && request.initiator == "navigation"
}

/// `requests` as a HAR 1.2 log, oldest first.
///
/// Every navigation's document request opens a page, and what follows it belongs to that
/// page until the next one. Cookie and authorization headers are redacted; see
/// [`SECRET_HEADERS`].
pub fn har(requests: &[NetRequest]) -> Value {
    let mut pages = Vec::new();
    let mut entries = Vec::with_capacity(requests.len());
    for request in requests {
        if starts_page(request) {
            pages.push(json!({
                "startedDateTime": iso_time(request.started_ms),
                "id": format!("page_{}", pages.len() + 1),
                "title": request.url,
                "pageTimings": {},
            }));
        }
        let page = (!pages.is_empty()).then_some(pages.len());
        entries.push(entry(request, page));
    }
    json!({
        "log": {
            "version": "1.2",
            "creator": { "name": "Beacon", "version": env!("CARGO_PKG_VERSION") },
            "pages": pages,
            "entries": entries,
        }
    })
}

/// [`har`] as the indented JSON text a `.har` file holds.
pub fn har_text(requests: &[NetRequest]) -> String {
    // A `Value` always serializes; the fallback is never taken.
    serde_json::to_string_pretty(&har(requests)).unwrap_or_default()
}

/// A name for a saved HAR file: the page's host and the time, so two exports do not collide.
pub fn har_file_name(requests: &[NetRequest], now_ms: u64) -> String {
    let host = requests
        .iter()
        .find(|r| starts_page(r))
        .or(requests.first())
        .and_then(|r| url::Url::parse(&r.url).ok())
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_else(|| "beacon".to_string());
    let stamp = chrono::DateTime::from_timestamp_millis(now_ms as i64)
        .unwrap_or_default()
        .format("%Y%m%d-%H%M%S");
    format!("{host}-{stamp}.har")
}

/// Quote `value` for a POSIX shell: single quotes, with each single quote inside closed,
/// escaped and reopened.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Headers curl works out on its own; sending the stack's copies would only disagree with it.
const CURL_COMPUTES: &[&str] = &["host", "content-length", "connection"];

/// `request` as a `curl` command line that sends the same request again.
///
/// Headers go out as the stack sent them, cookies included: reproducing the request is the
/// reason to copy it. A request logged while the panel was closed had its cookie redacted
/// by the engine, and that header is left out rather than sent as the marker. There is no
/// body, because the engine does not report request bodies.
pub fn curl(request: &NetRequest) -> String {
    let mut parts = vec![format!("curl {}", shell_quote(&request.url))];
    let method = request.method.as_deref().unwrap_or("GET");
    if method != "GET" {
        parts.push(format!("-X {}", shell_quote(method)));
    }
    let mut compressed = false;
    for (name, value) in &request.request_headers {
        if CURL_COMPUTES.iter().any(|skip| name.eq_ignore_ascii_case(skip)) || value == gosub_engine::REDACTED {
            continue;
        }
        // curl only decodes a response it asked to have compressed.
        if name.eq_ignore_ascii_case("accept-encoding") {
            compressed = true;
        }
        parts.push(format!("-H {}", shell_quote(&format!("{name}: {value}"))));
    }
    if compressed {
        parts.push("--compressed".to_string());
    }
    parts.join(" \\\n  ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(url: &str, kind: &str, initiator: &str, started_ms: u64) -> NetRequest {
        let mut r = NetRequest::new(uuid::Uuid::new_v4(), None, url, started_ms);
        r.kind = kind.to_string();
        r.initiator = initiator.to_string();
        r.state = RequestState::Finished;
        r.status = Some(200);
        r.content_type = Some("text/html".to_string());
        r.received_bytes = 1200;
        r.elapsed_us = Some(50_000);
        r.method = Some("GET".to_string());
        r
    }

    #[test]
    fn a_page_load_opens_a_page_and_its_subresources_belong_to_it() {
        let log = har(&[
            request("https://a.test/", "document", "navigation", 1_000),
            request("https://a.test/s.css", "stylesheet", "parser", 1_010),
            request("https://b.test/", "document", "navigation", 2_000),
        ]);
        let pages = log["log"]["pages"].as_array().unwrap();
        assert_eq!(pages.len(), 2);
        let refs: Vec<_> = log["log"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["pageref"].as_str().unwrap())
            .collect();
        assert_eq!(refs, ["page_1", "page_1", "page_2"]);
        assert_eq!(log["log"]["version"], "1.2");
        assert_eq!(log["log"]["entries"][0]["startedDateTime"], "1970-01-01T00:00:01.000Z");
    }

    #[test]
    fn timings_add_up_to_the_entry_time_and_split_dns_out_of_connect() {
        let mut r = request("https://a.test/", "document", "navigation", 10_000);
        r.dns_us = Some(5_000);
        r.connect_us = Some(15_000);
        r.headers_ms = Some(10_030);
        r.elapsed_us = Some(50_000);
        let entry = &har(&[r])["log"]["entries"][0];
        let t = &entry["timings"];
        assert_eq!(t["dns"], 5.0);
        assert_eq!(t["connect"], 10.0);
        assert_eq!(t["wait"], 15.0);
        assert_eq!(t["receive"], 20.0);
        assert_eq!(entry["time"], 50.0);
    }

    #[test]
    fn secrets_are_redacted_in_a_har_but_other_headers_are_not() {
        let mut r = request("https://a.test/?q=1&x=y", "document", "navigation", 0);
        r.request_headers = vec![
            ("Cookie".to_string(), "session=abc".to_string()),
            ("Accept".to_string(), "text/html".to_string()),
        ];
        r.headers = vec![("Set-Cookie".to_string(), "session=abc".to_string())];
        let entry = &har(&[r])["log"]["entries"][0];
        let text = entry.to_string();
        assert!(!text.contains("session=abc"), "{text}");
        assert_eq!(entry["request"]["headers"][1]["value"], "text/html");
        assert_eq!(entry["request"]["queryString"][1]["name"], "x");
    }

    #[test]
    fn only_a_text_body_goes_into_the_har() {
        let mut text = request("https://a.test/", "document", "navigation", 0);
        text.body = Some(b"<p>hi</p>".to_vec());
        let mut binary = request("https://a.test/i.png", "image", "parser", 0);
        binary.body = Some(vec![0x89, 0x50, 0xff, 0xfe]);
        let log = har(&[text, binary]);
        assert_eq!(log["log"]["entries"][0]["response"]["content"]["text"], "<p>hi</p>");
        assert!(log["log"]["entries"][1]["response"]["content"].get("text").is_none());
    }

    #[test]
    fn curl_quotes_what_a_shell_would_break_on() {
        let mut r = request("https://a.test/it's", "document", "navigation", 0);
        r.method = Some("POST".to_string());
        r.request_headers = vec![
            ("Host".to_string(), "a.test".to_string()),
            ("X-Note".to_string(), "don't".to_string()),
            ("Accept-Encoding".to_string(), "gzip".to_string()),
        ];
        assert_eq!(
            curl(&r),
            "curl 'https://a.test/it'\\''s' \\\n  -X 'POST' \\\n  -H 'X-Note: don'\\''t' \\\n  -H 'Accept-Encoding: gzip' \\\n  --compressed"
        );
    }

    #[test]
    fn curl_leaves_out_a_header_the_engine_redacted() {
        let mut r = request("https://a.test/", "document", "navigation", 0);
        r.request_headers = vec![("Cookie".to_string(), gosub_engine::REDACTED.to_string())];
        assert_eq!(curl(&r), "curl 'https://a.test/'");
    }

    #[test]
    fn a_har_file_is_named_after_the_page() {
        let requests = [request("https://www.a.test/x", "document", "navigation", 0)];
        assert_eq!(har_file_name(&requests, 0), "www.a.test-19700101-000000.har");
    }
}
