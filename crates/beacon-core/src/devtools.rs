//! What a developer panel shows: the log, and the engine's timings.
//!
//! This lives in the core rather than in a frontend because there is nothing
//! toolkit-specific about it. A GTK pane, an AppKit table and a C shell all want the same
//! two lists, and the alternative — each frontend keeping its own ring buffer and its own
//! idea of what a timing row is — is exactly the duplication this crate exists to prevent.
//!
//! The log side is process-global because `log` allows one logger for the whole process,
//! and the engine's crates log from whatever thread they happen to be on. Frontends read
//! snapshots; nothing here hands out a reference into the live buffer.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// One captured record.
#[derive(Clone, Debug)]
pub struct LogLine {
    pub level: log::Level,
    /// The emitting crate or module — what a busy log is usefully filtered by.
    pub target: String,
    pub message: String,
    /// Milliseconds since the Unix epoch. Stored rather than formatted: what a timestamp
    /// should look like is the frontend's business, and its locale's.
    pub timestamp_ms: u64,
}

/// Enough to cover a page load and its network chatter; older lines fall off the front.
const CAPACITY: usize = 4000;

static BUFFER: Mutex<Vec<LogLine>> = Mutex::new(Vec::new());

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Add a line to the buffer without going through `log`.
///
/// For a frontend's own running commentary — "Visiting …", "Zoom: 150%" — which belongs in
/// the same pane as the engine's records rather than in a second one beside it.
pub fn record(level: log::Level, target: &str, message: &str) {
    push(LogLine {
        level,
        target: target.to_string(),
        message: message.to_string(),
        timestamp_ms: now_ms(),
    });
}

fn push(line: LogLine) {
    let Ok(mut buffer) = BUFFER.lock() else { return };
    if buffer.len() >= CAPACITY {
        // Drop the oldest quarter rather than one line at a time, so a busy load does not
        // memmove the whole buffer on every single record.
        buffer.drain(..CAPACITY / 4);
    }
    buffer.push(line);
}

/// The newest `max` records, oldest first.
pub fn log_snapshot(max: usize) -> Vec<LogLine> {
    let Ok(buffer) = BUFFER.lock() else {
        return Vec::new();
    };
    let start = buffer.len().saturating_sub(max);
    buffer[start..].to_vec()
}

/// How many records are held.
pub fn log_len() -> usize {
    BUFFER.lock().map(|b| b.len()).unwrap_or(0)
}

pub fn clear_logs() {
    if let Ok(mut buffer) = BUFFER.lock() {
        buffer.clear();
    }
}

/// A `log::Log` that copies everything into the panel buffer on its way to `inner`.
///
/// Composed rather than substituted because a frontend has already chosen how it wants
/// records printed — `colog` under GTK, a plain stderr line under the C ABI — and the panel
/// should not be the thing that takes that away.
struct Tee {
    inner: Box<dyn log::Log>,
}

impl log::Log for Tee {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.inner.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        // The inner logger's filter decides for both of us. Buffering records it rejects
        // would fill the panel with trace noise that never reaches the terminal, and the
        // two views of the log would disagree about what happened.
        if !self.inner.enabled(record.metadata()) {
            return;
        }
        push(LogLine {
            level: record.level(),
            target: record.target().to_string(),
            message: record.args().to_string(),
            timestamp_ms: now_ms(),
        });
        self.inner.log(record);
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

/// Install `inner` as the process logger, teeing every record it accepts into the panel.
///
/// `max_level` must be at least as permissive as `inner`'s own filtering, because `log`
/// drops anything above it before a logger is ever consulted. For an `env_logger`, that is
/// exactly what its `filter()` returns.
///
/// Does nothing if a logger is already installed — an embedder that set one up first keeps
/// it, and the panel simply stays empty rather than the process failing to start.
pub fn install_logger(inner: Box<dyn log::Log>, max_level: log::LevelFilter) {
    if log::set_boxed_logger(Box::new(Tee { inner })).is_ok() {
        log::set_max_level(max_level);
    }
}

/// The level asked for by `BEACON_LOG` or `RUST_LOG`, if either is set and parses.
///
/// `None` rather than a default, so a frontend that already has a considered set of
/// per-target levels can leave them alone unless the user actually asked for more.
pub fn env_level() -> Option<log::LevelFilter> {
    std::env::var("BEACON_LOG")
        .or_else(|_| std::env::var("RUST_LOG"))
        .ok()
        .and_then(|value| value.parse().ok())
}

/// The level a frontend with no opinion of its own should use.
///
/// Warnings only unless asked otherwise: a browser that prints a paragraph per page load is
/// one nobody reads the output of.
pub fn level_from_env() -> log::LevelFilter {
    env_level().unwrap_or(log::LevelFilter::Warn)
}

// ── network ──────────────────────────────────────────────────────────────────

/// Where a request got to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestState {
    Queued,
    Running,
    Finished,
    Failed,
    Cancelled,
}

impl RequestState {
    pub fn label(&self) -> &'static str {
        match self {
            RequestState::Queued => "queued",
            RequestState::Running => "loading",
            RequestState::Finished => "done",
            RequestState::Failed => "failed",
            RequestState::Cancelled => "cancelled",
        }
    }
}

/// How far an unfinished request got, so a slow load can be told from a stuck one.
///
/// "Loading" for thirty seconds says nothing. *Where* it has been loading for thirty
/// seconds says everything: a request stuck before the connection is a host problem, one
/// stuck after the request went out is a server that took it and answered nothing, and one
/// stuck part way through the body is a transfer that died mid-stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Not dispatched yet -- waiting behind other requests, or on the scheduler.
    Queued,
    /// Dispatched, and nothing has come back. Name resolution and connection setup both
    /// live here, and so does a request reusing a pooled connection, which reports no
    /// setup at all. Hence the vague name: this means "no news yet", not "connecting".
    Opening,
    /// Resolution finished and the connection has not. Stuck here means a host that
    /// resolves but will not accept a connection.
    Connecting,
    /// On the wire, with no response headers yet. Stuck here means a server that took the
    /// request and said nothing -- the case that looks most like a hung browser.
    Waiting,
    /// Headers arrived, body still coming. Stuck here means a stalled transfer, and the
    /// received byte count says whether anything is moving at all.
    Receiving,
    /// Nothing in flight: the request finished, failed or was cancelled.
    Done,
}

impl Phase {
    pub fn label(&self) -> &'static str {
        match self {
            Phase::Queued => "queued",
            Phase::Opening => "opening",
            Phase::Connecting => "connecting",
            Phase::Waiting => "waiting",
            Phase::Receiving => "receiving",
            Phase::Done => "done",
        }
    }

    /// What a developer looking at a stuck request should suspect.
    pub fn hint(&self) -> &'static str {
        match self {
            Phase::Queued => "not sent yet: waiting behind other requests",
            Phase::Opening => "no response from the connection attempt: DNS, or a host that never answers",
            Phase::Connecting => "the host resolves but is not accepting the connection",
            Phase::Waiting => "the server took the request and has not started replying",
            Phase::Receiving => "the response started but the body has not finished arriving",
            Phase::Done => "not in flight",
        }
    }
}

/// One request the engine made, folded together from the events it emitted about it.
///
/// The engine reports a request's life as half a dozen separate events -- queued, started,
/// headers, progress, finished. A panel wants one row that changes, so they are folded into
/// this on the way past and the panel reads rows.
#[derive(Clone, Debug)]
pub struct NetRequest {
    /// The engine's request id, and what later events are matched on.
    pub id: uuid::Uuid,
    /// Which tab asked. `None` before anything says.
    pub tab: Option<crate::tab::TabId>,
    pub url: String,
    /// `document`, `stylesheet`, `script`, `image`, `font`, ...
    pub kind: String,
    /// What caused it: `navigation`, `parser`, `script`, `css`.
    pub initiator: String,
    pub state: RequestState,
    pub status: Option<u16>,
    pub content_type: Option<String>,
    /// What the server said it would send, which is not always what arrived.
    pub content_length: Option<u64>,
    pub received_bytes: u64,
    /// Microseconds, not milliseconds: a local file or a cache hit finishes well inside a
    /// millisecond, and rounding those to `0ms` makes the column useless exactly where a
    /// developer is looking hardest.
    pub elapsed_us: Option<u64>,
    pub error: Option<String>,
    /// What kind of failure it was, when the engine could tell. A message alone leaves a
    /// panel guessing; this is what lets it say "certificate" rather than "error".
    pub failure: Option<gosub_engine::events::FailureKind>,
    /// The method actually sent. `None` until the request line is reported.
    pub method: Option<String>,
    /// The headers the net stack set on the way out.
    pub request_headers: Vec<(String, String)>,
    /// Every response header, for the detail view.
    pub headers: Vec<(String, String)>,
    /// The first bytes of the response body, when body capture was on. Raw: not decoded,
    /// and not necessarily valid UTF-8.
    pub body: Option<Vec<u8>>,
    /// Whether the body continued past the captured preview.
    pub body_truncated: bool,
    /// Whether a body was captured and then dropped to stay inside the total budget.
    /// Distinct from never having captured one, which is what a shell should say instead.
    pub body_evicted: bool,
    /// Redirect chain, as `status -> location` steps.
    pub redirects: Vec<(u16, String)>,
    /// When it was first seen, in milliseconds since the epoch. Rows are kept in this
    /// order, which is the order a page actually fetched things.
    pub started_ms: u64,
    /// How long name resolution took, when this request opened a connection. `None` for one
    /// served by a pooled connection, which resolved nothing.
    pub dns_us: Option<u64>,
    /// How long the connection took to establish. Encloses `dns_us` rather than following
    /// it: resolution happens inside the connector this times.
    pub connect_us: Option<u64>,
    /// When the response headers arrived, same clock. The split between waiting for the
    /// server and reading the body, which is the whole point of a waterfall: a row that is
    /// mostly wait is a slow server, one that is mostly body is a big file.
    pub headers_ms: Option<u64>,
}

impl NetRequest {
    /// Where this request has got to, derived only from what was reported.
    ///
    /// Note what is *not* claimed: with no connection timing there is no way to tell a
    /// name that will not resolve from a pooled connection being reused, so both are
    /// [`Phase::Opening`] rather than a guess dressed up as a fact.
    pub fn phase(&self) -> Phase {
        match self.state {
            RequestState::Queued => Phase::Queued,
            RequestState::Finished | RequestState::Failed | RequestState::Cancelled => Phase::Done,
            RequestState::Running => {
                if self.headers_ms.is_some() {
                    Phase::Receiving
                } else if self.connect_us.is_some() {
                    Phase::Waiting
                } else if self.dns_us.is_some() {
                    Phase::Connecting
                } else {
                    Phase::Opening
                }
            }
        }
    }

    /// A short name for why it failed, for a status column.
    pub fn failure_label(&self) -> Option<&'static str> {
        use gosub_engine::events::FailureKind;
        Some(match self.failure? {
            FailureKind::Blocked => "blocked",
            FailureKind::Tls => "TLS",
            FailureKind::Timeout => "timeout",
            FailureKind::Connect => "no connection",
            FailureKind::Transfer => "transfer broke",
            FailureKind::Redirect => "bad redirect",
            FailureKind::Cancelled => "cancelled",
            FailureKind::Other => "failed",
        })
    }

    /// What that failure means, in the words someone debugging a dead page needs.
    pub fn failure_hint(&self) -> Option<&'static str> {
        use gosub_engine::events::FailureKind;
        Some(match self.failure? {
            FailureKind::Blocked => "refused by policy before it was sent -- mixed content, or a URL the embedder disallows",
            FailureKind::Tls => "the TLS handshake failed: an expired, untrusted or mismatched certificate",
            FailureKind::Timeout => "no answer within the time limit",
            FailureKind::Connect => "no connection was established: the name did not resolve, or nothing accepted it",
            FailureKind::Transfer => "the connection worked and then broke part way through",
            FailureKind::Redirect => "a redirect could not be followed: too many hops, or an invalid target",
            FailureKind::Cancelled => "something gave up on the request",
            FailureKind::Other => "the network stack did not say what went wrong",
        })
    }
}

/// Plenty for a page load; oldest requests fall off the front.
const REQUEST_CAPACITY: usize = 1000;

/// How many bytes of captured bodies are kept in total.
///
/// The engine caps each response on its own, which stops one download eating memory, but
/// says nothing about a hundred of them. This is the ceiling across all of them: past it,
/// the oldest bodies are dropped while their rows stay, so the list keeps its history and
/// only loses the payloads nobody has looked at in a while.
const BODY_BUDGET: usize = 16 * 1024 * 1024;

static REQUESTS: Mutex<Vec<NetRequest>> = Mutex::new(Vec::new());

/// Fold one engine resource event into the request log.
///
/// Called from [`crate::beacon::Beacon::on_engine_event`], so every frontend gets the
/// network panel without doing anything: the events were already flowing past.
pub fn record_resource(tab: Option<crate::tab::TabId>, event: &gosub_engine::events::ResourceEvent) {
    use gosub_engine::events::ResourceEvent;

    let Ok(mut requests) = REQUESTS.lock() else { return };

    /// Find the row for a request id, or make one. Events do not always arrive in the
    /// order the engine emitted them, so any of them may be a row's first sight.
    fn row<'a>(requests: &'a mut Vec<NetRequest>, id: uuid::Uuid, url: &str, tab: Option<crate::tab::TabId>) -> &'a mut NetRequest {
        if let Some(index) = requests.iter().position(|r| r.id == id) {
            return &mut requests[index];
        }
        if requests.len() >= REQUEST_CAPACITY {
            requests.drain(..REQUEST_CAPACITY / 4);
        }
        requests.push(NetRequest {
            id,
            tab,
            url: url.to_string(),
            kind: "other".into(),
            initiator: "other".into(),
            state: RequestState::Queued,
            status: None,
            content_type: None,
            content_length: None,
            received_bytes: 0,
            elapsed_us: None,
            error: None,
            failure: None,
            method: None,
            request_headers: Vec::new(),
            headers: Vec::new(),
            body: None,
            body_truncated: false,
            body_evicted: false,
            redirects: Vec::new(),
            dns_us: None,
            connect_us: None,
            headers_ms: None,
            started_ms: now_ms(),
        });
        requests.last_mut().expect("just pushed")
    }

    match event {
        ResourceEvent::Queued {
            request_id,
            url,
            kind,
            initiator,
            ..
        } => {
            let entry = row(&mut requests, request_id.0, url, tab);
            entry.kind = kind_label(kind);
            entry.initiator = initiator_label(initiator);
            entry.state = RequestState::Queued;
        }
        ResourceEvent::Started {
            request_id,
            url,
            kind,
            initiator,
            ..
        } => {
            let entry = row(&mut requests, request_id.0, url, tab);
            entry.url = url.clone();
            entry.kind = kind_label(kind);
            entry.initiator = initiator_label(initiator);
            entry.state = RequestState::Running;
        }
        ResourceEvent::RequestSent {
            request_id,
            url,
            method,
            headers,
            ..
        } => {
            let entry = row(&mut requests, request_id.0, url, tab);
            entry.method = Some(method.clone());
            // A redirect sends a second request line for the same row; the last one is the
            // one whose response the row ends up describing.
            entry.request_headers = headers.clone();
        }
        ResourceEvent::BodyPreview {
            request_id,
            url,
            body,
            truncated,
            ..
        } => {
            let entry = row(&mut requests, request_id.0, url, tab);
            entry.body = Some(body.clone());
            entry.body_truncated = *truncated;
            evict_bodies_over_budget(&mut requests);
        }
        ResourceEvent::DnsResolved {
            request_id, elapsed_us, ..
        } => {
            let entry = row(&mut requests, request_id.0, "", tab);
            entry.dns_us = Some(*elapsed_us);
        }
        ResourceEvent::Connected {
            request_id, elapsed_us, ..
        } => {
            let entry = row(&mut requests, request_id.0, "", tab);
            entry.connect_us = Some(*elapsed_us);
        }
        ResourceEvent::Redirected {
            request_id, to, status, ..
        } => {
            let entry = row(&mut requests, request_id.0, to, tab);
            entry.redirects.push((*status, to.clone()));
            // The row follows the redirect, because what the user cares about is where the
            // bytes finally came from; the chain is kept for the detail view.
            entry.url = to.clone();
        }
        ResourceEvent::Headers {
            request_id,
            url,
            status,
            content_length,
            content_type,
            headers,
            ..
        } => {
            let entry = row(&mut requests, request_id.0, url, tab);
            entry.headers_ms.get_or_insert_with(now_ms);
            entry.status = Some(*status);
            entry.content_length = *content_length;
            entry.content_type = content_type.clone();
            entry.headers = headers.clone();
            entry.state = RequestState::Running;
        }
        ResourceEvent::Progress {
            request_id,
            received_bytes,
            ..
        } => {
            let entry = row(&mut requests, request_id.0, "", tab);
            entry.received_bytes = *received_bytes;
        }
        ResourceEvent::Finished {
            request_id,
            url,
            received_bytes,
            elapsed,
            ..
        } => {
            let entry = row(&mut requests, request_id.0, url.as_str(), tab);
            entry.received_bytes = *received_bytes;
            entry.elapsed_us = elapsed.map(|d| d.as_micros() as u64);
            entry.state = RequestState::Finished;
        }
        ResourceEvent::Failed {
            request_id,
            url,
            kind,
            error,
            ..
        } => {
            let entry = row(&mut requests, request_id.0, url, tab);
            entry.error = Some(error.to_string());
            entry.failure = Some(*kind);
            entry.state = RequestState::Failed;
        }
        ResourceEvent::Cancelled {
            request_id, url, reason, ..
        } => {
            let entry = row(&mut requests, request_id.0, url, tab);
            entry.error = Some(format!("{reason:?}"));
            entry.state = RequestState::Cancelled;
        }
    }
}

fn kind_label(kind: &gosub_engine::net::types::ResourceKind) -> String {
    use gosub_engine::net::types::ResourceKind;
    match kind {
        ResourceKind::Document => "document",
        ResourceKind::Stylesheet => "stylesheet",
        ResourceKind::Script { blocking: true } => "script (blocking)",
        ResourceKind::Script { .. } => "script",
        ResourceKind::Image => "image",
        ResourceKind::Font => "font",
        ResourceKind::Media => "media",
        ResourceKind::Xhr => "xhr",
        ResourceKind::Fetch => "fetch",
        ResourceKind::WebSocket => "websocket",
        ResourceKind::Other => "other",
    }
    .to_string()
}

fn initiator_label(initiator: &gosub_engine::net::types::Initiator) -> String {
    use gosub_engine::net::types::Initiator;
    match initiator {
        Initiator::Navigation => "navigation",
        Initiator::Parser => "parser",
        Initiator::Script => "script",
        Initiator::CSS => "css",
        Initiator::Other => "other",
    }
    .to_string()
}

/// Every request seen, oldest first — which is the order the page fetched them.
///
/// `tab` narrows it to one tab's requests; `None` returns all of them, including those the
/// engine never attributed to a tab.
pub fn requests(tab: Option<crate::tab::TabId>) -> Vec<NetRequest> {
    let Ok(requests) = REQUESTS.lock() else {
        return Vec::new();
    };
    match tab {
        Some(tab) => requests.iter().filter(|r| r.tab == Some(tab)).cloned().collect(),
        None => requests.clone(),
    }
}

/// Turn response-body capture on or off.
///
/// Off by default: a preview costs a copy of the net stack's peek window per request, which
/// is wasted on every page nobody is inspecting. A frontend switches it on when it opens a
/// developer panel and off when it closes one, and the net stack skips the copy in between.
pub fn set_capture_bodies(enabled: bool) {
    gosub_engine::set_capture_body_previews(enabled);
}

/// Render a captured body for display: text as text, anything else described rather than
/// spilled as mojibake.
pub fn format_body(bytes: &[u8], truncated: bool) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => {
            let mut out = text.to_string();
            if truncated {
                out.push_str("\n\n… truncated: only the start of the body is captured.\n");
            }
            out
        }
        // Binary, or text cut mid-codepoint by the preview window. Either way, printing it
        // would be noise; the size and a hex head say more.
        Err(_) => {
            let head: String = bytes.iter().take(64).map(|b| format!("{b:02x} ")).collect();
            format!(
                "Not valid UTF-8 -- binary, or text cut mid-character by the preview window.\n\n                 {} captured{}\n\nFirst bytes:\n{}\n",
                format_bytes(bytes.len() as u64),
                if truncated { ", and the body continues" } else { "" },
                head.trim_end()
            )
        }
    }
}

/// Drop the oldest captured bodies until the total is back under [`BODY_BUDGET`].
///
/// Rows are kept: a request whose body was evicted is still part of the page's history, and
/// losing the row would be a worse answer than losing the payload. The body becomes `None`,
/// which the panel reports as evicted rather than as never captured -- those are different
/// things and a developer needs to tell them apart.
fn evict_bodies_over_budget(requests: &mut [NetRequest]) {
    let mut total: usize = requests.iter().filter_map(|r| r.body.as_ref().map(Vec::len)).sum();
    if total <= BODY_BUDGET {
        return;
    }
    // Oldest first, which is the order the list is already in.
    for request in requests.iter_mut() {
        if total <= BODY_BUDGET {
            break;
        }
        if let Some(body) = request.body.take() {
            total -= body.len();
            request.body_evicted = true;
        }
    }
}

/// Bytes of captured bodies currently held.
pub fn captured_body_bytes() -> usize {
    REQUESTS
        .lock()
        .map(|r| r.iter().filter_map(|q| q.body.as_ref().map(Vec::len)).sum())
        .unwrap_or(0)
}

pub fn clear_requests() {
    if let Ok(mut requests) = REQUESTS.lock() {
        requests.clear();
    }
}

/// Bytes in whatever unit keeps the number short.
pub fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} kB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

// ── timings ──────────────────────────────────────────────────────────────────

pub use gosub_shared::timing::NamespaceStats;

/// The engine's timing table, slowest namespace first — a developer panel is opened to find
/// where the time went.
///
/// Empty when the engine was built without its `timing` feature: the whole subsystem
/// compiles out, so this is nothing to show rather than an error.
pub fn timings() -> Vec<NamespaceStats> {
    let mut stats = gosub_shared::timing::snapshot_stats();
    stats.sort_by_key(|row| std::cmp::Reverse(row.total_us));
    stats
}

/// Start measuring again from nothing, so the next navigation is timed on its own rather
/// than averaged in with every navigation since launch.
pub fn reset_timings() {
    gosub_shared::timing::reset_stats();
}

/// Microseconds in whatever unit keeps the number readable. A table of `4466000µs` is
/// technically right and useless.
pub fn format_duration(microseconds: u64) -> String {
    if microseconds >= 1_000_000 {
        format!("{:.2}s", microseconds as f64 / 1_000_000.0)
    } else if microseconds >= 1_000 {
        format!("{:.1}ms", microseconds as f64 / 1_000.0)
    } else {
        format!("{microseconds}µs")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_pick_a_readable_unit() {
        assert_eq!(format_duration(0), "0µs");
        assert_eq!(format_duration(999), "999µs");
        assert_eq!(format_duration(1_000), "1.0ms");
        assert_eq!(format_duration(4_466_000), "4.47s");
    }

    #[test]
    fn the_buffer_keeps_the_newest_lines() {
        clear_logs();
        for i in 0..10 {
            record(log::Level::Info, "test", &format!("line {i}"));
        }
        let snapshot = log_snapshot(3);
        assert_eq!(snapshot.len(), 3);
        // Oldest first within the window, so a pane can append rather than prepend.
        assert_eq!(snapshot[0].message, "line 7");
        assert_eq!(snapshot[2].message, "line 9");
        clear_logs();
    }

    /// Build `count` rows each carrying `size` bytes of body, as the fold would.
    fn rows_with_bodies(count: usize, size: usize) -> Vec<NetRequest> {
        (0..count)
            .map(|i| NetRequest {
                id: uuid::Uuid::new_v4(),
                tab: None,
                url: format!("https://example.test/{i}"),
                kind: "image".into(),
                initiator: "parser".into(),
                state: RequestState::Finished,
                status: Some(200),
                content_type: None,
                content_length: None,
                received_bytes: size as u64,
                elapsed_us: None,
                dns_us: None,
                connect_us: None,
                headers_ms: None,
                error: None,
                failure: None,
                method: None,
                request_headers: Vec::new(),
                headers: Vec::new(),
                body: Some(vec![0u8; size]),
                body_truncated: false,
                body_evicted: false,
                redirects: Vec::new(),
                started_ms: i as u64,
            })
            .collect()
    }

    #[test]
    fn bodies_inside_the_budget_are_left_alone() {
        let mut rows = rows_with_bodies(4, 1024);
        evict_bodies_over_budget(&mut rows);
        assert!(rows.iter().all(|r| r.body.is_some()));
        assert!(rows.iter().all(|r| !r.body_evicted));
    }

    #[test]
    fn going_over_the_budget_drops_the_oldest_bodies_first() {
        // Three over: the budget divides evenly, so the arithmetic is not the thing on trial.
        let size = BODY_BUDGET / 4;
        let mut rows = rows_with_bodies(7, size);
        evict_bodies_over_budget(&mut rows);

        let held: usize = rows.iter().filter_map(|r| r.body.as_ref().map(Vec::len)).sum();
        assert!(held <= BODY_BUDGET, "back under the budget ({held} bytes)");

        // The survivors are the newest, and the evicted ones say so rather than looking
        // like requests whose body was never captured.
        let evicted: Vec<usize> = rows.iter().enumerate().filter(|(_, r)| r.body.is_none()).map(|(i, _)| i).collect();
        assert_eq!(evicted, vec![0, 1, 2], "oldest first");
        assert!(rows[..3].iter().all(|r| r.body_evicted));
        assert!(rows[3..].iter().all(|r| !r.body_evicted));
    }

    #[test]
    fn eviction_keeps_the_rows_it_empties() {
        let mut rows = rows_with_bodies(6, BODY_BUDGET / 2);
        let before = rows.len();
        evict_bodies_over_budget(&mut rows);
        assert_eq!(rows.len(), before, "a request stays in the list without its body");
    }

    #[test]
    fn asking_for_more_than_there_is_yields_what_there_is() {
        clear_logs();
        record(log::Level::Warn, "test", "only one");
        assert_eq!(log_snapshot(1000).len(), 1);
        assert_eq!(log_len(), 1);
        clear_logs();
        assert!(log_snapshot(10).is_empty());
    }

    /// A row in flight, with whatever the network has reported about it so far.
    fn in_flight(dns_us: Option<u64>, connect_us: Option<u64>, headers_ms: Option<u64>) -> NetRequest {
        let mut row = rows_with_bodies(1, 0).remove(0);
        row.state = RequestState::Running;
        row.dns_us = dns_us;
        row.connect_us = connect_us;
        row.headers_ms = headers_ms;
        row
    }

    #[test]
    fn a_request_is_placed_by_the_last_thing_that_reported() {
        assert_eq!(in_flight(None, None, None).phase(), Phase::Opening);
        assert_eq!(in_flight(Some(900), None, None).phase(), Phase::Connecting);
        assert_eq!(in_flight(Some(900), Some(4_000), None).phase(), Phase::Waiting);
        assert_eq!(in_flight(Some(900), Some(4_000), Some(12)).phase(), Phase::Receiving);
    }

    /// The distinction the whole thing exists for: both have been running for ages, and
    /// they are different problems. One is a server that never replied, the other a body
    /// that stopped arriving part way through.
    #[test]
    fn a_silent_server_and_a_stalled_transfer_do_not_look_alike() {
        assert_ne!(
            in_flight(Some(900), Some(4_000), None).phase(),
            in_flight(Some(900), Some(4_000), Some(12)).phase()
        );
    }

    /// A pooled connection reports no setup at all, which looks exactly like a name that
    /// has not resolved yet. Neither is claimed: both are "opening".
    #[test]
    fn a_reused_connection_is_not_mistaken_for_a_stalled_lookup() {
        assert_eq!(in_flight(None, None, None).phase(), Phase::Opening);
    }

    #[test]
    fn a_finished_request_is_in_no_phase() {
        let mut row = in_flight(Some(900), Some(4_000), Some(12));
        row.state = RequestState::Finished;
        assert_eq!(row.phase(), Phase::Done);
    }

    #[test]
    fn a_failure_the_engine_did_not_classify_gets_no_label() {
        let mut row = in_flight(None, None, None);
        row.state = RequestState::Failed;
        assert_eq!(row.failure_label(), None);

        row.failure = Some(gosub_engine::events::FailureKind::Tls);
        assert_eq!(row.failure_label(), Some("TLS"));
        assert!(row.failure_hint().is_some());
    }
}
