//! The network panel, over the C ABI.
//!
//! Same shape as the log and timing snapshots next door: take a copy, then read it by
//! index. A panel refreshing several times a second must not be walking a live table, and
//! a request that vanished between two accessor calls would be worse than a stale row.
//!
//! The rows themselves are `beacon_core::devtools`'s, folded together from the engine's
//! resource events by [`beacon_core::beacon::Beacon::on_engine_event`] — so every shell
//! gets the same panel over the same data rather than each learning to read the engine.

use std::ffi::c_char;

use beacon_core::devtools;

use crate::{to_c_string, BeaconBrowser};

/// A field the engine never reported, as distinct from one that really is zero: a request
/// served from a pooled connection resolved nothing, which is not the same as resolving in
/// no time at all.
pub const BEACON_ABSENT: u64 = u64::MAX;

#[repr(u32)]
#[derive(Clone, Copy)]
pub enum BeaconRequestState {
    Queued = 0,
    Running = 1,
    Finished = 2,
    Failed = 3,
    Cancelled = 4,
}

/// How far an unfinished request got. "Loading" for thirty seconds says nothing; *where*
/// it has been loading for thirty seconds says everything.
#[repr(u32)]
#[derive(Clone, Copy)]
pub enum BeaconRequestPhase {
    Queued = 0,
    Opening = 1,
    Connecting = 2,
    Waiting = 3,
    Receiving = 4,
    Done = 5,
}

/// Everything about a request that is a number. The strings are read with the accessors
/// below, because a struct of owned pointers would need a matching free function and this
/// way the caller frees each string exactly as it frees every other string here.
#[repr(C)]
pub struct BeaconRequestRow {
    /// When it was first seen, milliseconds since the epoch. Rows are in this order, which
    /// is the order the page actually fetched things.
    pub started_ms: u64,
    pub received_bytes: u64,
    /// What the server said it would send, which is not always what arrived.
    pub content_length: u64,
    /// Total time, once it finished. `BEACON_ABSENT` while it is still in flight.
    pub elapsed_us: u64,
    /// Name resolution, connection setup, and the moment the response headers landed.
    /// `connect_us` encloses `dns_us` rather than following it: resolution happens inside
    /// the connector being timed.
    pub dns_us: u64,
    pub connect_us: u64,
    pub headers_ms: u64,
    /// 0 until a response line arrives.
    pub status: u32,
    pub state: BeaconRequestState,
    pub phase: BeaconRequestPhase,
    pub request_header_count: usize,
    pub response_header_count: usize,
    pub redirect_count: usize,
    pub has_body: bool,
    /// The response continued past the captured preview.
    pub body_truncated: bool,
    /// A body was captured and then dropped to stay inside the total budget — which is a
    /// different thing to say than "none was captured", and a panel should say it.
    pub body_evicted: bool,
}

fn absent(value: Option<u64>) -> u64 {
    value.unwrap_or(BEACON_ABSENT)
}

/// Copy the requests for `tab` — or every tab when it is 0 — and return how many are
/// readable. Newest last, in the order the page fetched them.
///
/// # Safety
/// `browser` must be a live handle from [`crate::beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_net_snapshot(browser: *mut BeaconBrowser, tab: u64) -> usize {
    let b = browser!(browser, 0);
    // 0 means "every tab" rather than "no tab": a window that has not settled on an active
    // tab yet should show the page's requests, not an empty panel.
    //
    // A tab handle that is not a tab is a different thing again, and must not fall back to
    // "everything": a panel asking about a tab that has gone would then quietly show every
    // other tab's requests as though they were its own.
    let filter = if tab == 0 {
        None
    } else {
        match b.tab(tab) {
            Some(tab_id) => Some(tab_id),
            None => {
                b.requests.clear();
                return 0;
            }
        }
    };
    b.requests = devtools::requests(filter);
    b.requests.len()
}

/// Forget every recorded request. Process-wide, like the log: there is one recorder.
///
/// # Safety
/// `browser` must be a live handle from [`crate::beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_net_clear(browser: *mut BeaconBrowser) {
    let b = browser!(browser);
    devtools::clear_requests();
    b.requests.clear();
}

/// Copy response bodies as they arrive, so the panel has something to show.
///
/// Off by default and meant to follow the panel's visibility: bodies are only worth
/// keeping while someone can look at them, and this is what keeps every page that is *not*
/// being inspected free of the cost.
///
/// # Safety
/// `browser` must be a live handle from [`crate::beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_net_set_capture_bodies(browser: *mut BeaconBrowser, enabled: bool) {
    let _ = browser!(browser);
    devtools::set_capture_bodies(enabled);
}

/// Show `cookie`, `authorization` and `proxy-authorization` in full rather than redacted.
///
/// Same window as body capture, and for the same reason: a request view that will not show
/// you your own `Cookie` cannot answer the question it was usually opened to answer, but a
/// panel nobody has opened has no business copying credentials into memory.
///
/// # Safety
/// `browser` must be a live handle from [`crate::beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_net_set_show_sensitive_headers(browser: *mut BeaconBrowser, enabled: bool) {
    let _ = browser!(browser);
    devtools::set_show_sensitive_headers(enabled);
}

/// How many bytes of captured bodies are being held, across every request.
///
/// # Safety
/// `browser` must be a live handle from [`crate::beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_net_captured_body_bytes(browser: *mut BeaconBrowser) -> usize {
    let _ = browser!(browser, 0);
    devtools::captured_body_bytes()
}

/// Read row `index` of the last snapshot. False when out of range, leaving `out` untouched.
///
/// # Safety
/// `browser` must be a live handle; `out` must point at a `BeaconRequestRow`.
#[no_mangle]
pub unsafe extern "C" fn beacon_net_at(browser: *mut BeaconBrowser, index: usize, out: *mut BeaconRequestRow) -> bool {
    let b = browser!(browser, false);
    if out.is_null() {
        return false;
    }
    let Some(request) = b.requests.get(index) else {
        return false;
    };

    let row = BeaconRequestRow {
        started_ms: request.started_ms,
        received_bytes: request.received_bytes,
        content_length: absent(request.content_length),
        elapsed_us: absent(request.elapsed_us),
        dns_us: absent(request.dns_us),
        connect_us: absent(request.connect_us),
        headers_ms: absent(request.headers_ms),
        status: request.status.unwrap_or(0) as u32,
        state: match request.state {
            devtools::RequestState::Queued => BeaconRequestState::Queued,
            devtools::RequestState::Running => BeaconRequestState::Running,
            devtools::RequestState::Finished => BeaconRequestState::Finished,
            devtools::RequestState::Failed => BeaconRequestState::Failed,
            devtools::RequestState::Cancelled => BeaconRequestState::Cancelled,
        },
        phase: match request.phase() {
            devtools::Phase::Queued => BeaconRequestPhase::Queued,
            devtools::Phase::Opening => BeaconRequestPhase::Opening,
            devtools::Phase::Connecting => BeaconRequestPhase::Connecting,
            devtools::Phase::Waiting => BeaconRequestPhase::Waiting,
            devtools::Phase::Receiving => BeaconRequestPhase::Receiving,
            devtools::Phase::Done => BeaconRequestPhase::Done,
        },
        request_header_count: request.request_headers.len(),
        response_header_count: request.headers.len(),
        redirect_count: request.redirects.len(),
        has_body: request.body.is_some(),
        body_truncated: request.body_truncated,
        body_evicted: request.body_evicted,
    };
    unsafe { std::ptr::write(out, row) };
    true
}

/// One string off a row. Every one of these frees with [`crate::beacon_string_free`], and
/// returns NULL rather than an empty string for "nothing to say" — a request that never
/// reached the wire has no method, which is not the same as an empty one.
macro_rules! row_string {
    ($name:ident, $doc:literal, |$request:ident| $value:expr) => {
        #[doc = $doc]
        ///
        /// Free with [`crate::beacon_string_free`]; NULL when there is none.
        ///
        /// # Safety
        /// `browser` must be a live handle from [`crate::beacon_new`].
        #[no_mangle]
        pub unsafe extern "C" fn $name(browser: *mut BeaconBrowser, index: usize) -> *mut c_char {
            let b = browser!(browser, std::ptr::null_mut());
            let Some($request) = b.requests.get(index) else {
                return std::ptr::null_mut();
            };
            match $value {
                Some(value) => to_c_string(&value),
                None => std::ptr::null_mut(),
            }
        }
    };
}

row_string!(beacon_net_url, "The URL requested.", |r| Some(r.url.clone()));
row_string!(beacon_net_kind, "`document`, `stylesheet`, `script`, `image`, `font`, …", |r| {
    Some(r.kind.clone())
});
row_string!(
    beacon_net_initiator,
    "What caused it: `navigation`, `parser`, `script`, `css`.",
    |r| Some(r.initiator.clone())
);
row_string!(
    beacon_net_method,
    "The method actually sent. NULL for a request that never reached the network — a file:// load, or one answered from cache.",
    |r| r.method.clone()
);
row_string!(beacon_net_content_type, "The response's declared content type.", |r| r
    .content_type
    .clone());
row_string!(beacon_net_error, "What went wrong, in the network stack's words.", |r| r
    .error
    .clone());
row_string!(
    beacon_net_state_label,
    "A word for the row's state: `queued`, `loading`, `done`, `failed`, `cancelled`.",
    |r| Some(r.state.label().to_string())
);
row_string!(
    beacon_net_phase_label,
    "Where an unfinished request has got to: `opening`, `connecting`, `waiting`, `receiving`.",
    |r| Some(r.phase().label().to_string())
);
row_string!(
    beacon_net_phase_hint,
    "What a developer looking at a request stuck in this phase should suspect.",
    |r| Some(r.phase().hint().to_string())
);
row_string!(
    beacon_net_failure_label,
    "A short name for why it failed — `TLS`, `timeout`, `no connection` — when the stack classified it. NULL otherwise.",
    |r| r.failure_label().map(str::to_string)
);
row_string!(
    beacon_net_failure_hint,
    "What that failure means, in the words someone debugging a dead page needs.",
    |r| r.failure_hint().map(str::to_string)
);
row_string!(
    beacon_net_body_text,
    "The captured response preview, decoded for display. NULL when no body was captured; check `body_evicted` to tell 'dropped' from 'never taken'.",
    |r| r
        .body
        .as_ref()
        .map(|bytes| devtools::format_body(bytes, r.body_truncated))
);

/// One header off a row, by index into `request_header_count` / `response_header_count`.
macro_rules! header_string {
    ($name:ident, $doc:literal, |$request:ident| $list:expr, $part:tt) => {
        #[doc = $doc]
        ///
        /// Free with [`crate::beacon_string_free`]; NULL when out of range.
        ///
        /// # Safety
        /// `browser` must be a live handle from [`crate::beacon_new`].
        #[no_mangle]
        pub unsafe extern "C" fn $name(browser: *mut BeaconBrowser, index: usize, header: usize) -> *mut c_char {
            let b = browser!(browser, std::ptr::null_mut());
            let Some($request) = b.requests.get(index) else {
                return std::ptr::null_mut();
            };
            match $list.get(header) {
                Some(pair) => to_c_string(&pair.$part),
                None => std::ptr::null_mut(),
            }
        }
    };
}

header_string!(
    beacon_net_request_header_name,
    "The name of request header `header`.",
    |r| r.request_headers,
    0
);
header_string!(
    beacon_net_request_header_value,
    "The value of request header `header`, redacted unless sensitive headers were asked for.",
    |r| r.request_headers,
    1
);
header_string!(
    beacon_net_response_header_name,
    "The name of response header `header`.",
    |r| r.headers,
    0
);
header_string!(
    beacon_net_response_header_value,
    "The value of response header `header`.",
    |r| r.headers,
    1
);
header_string!(beacon_net_redirect_url, "Where redirect hop `hop` pointed.", |r| r.redirects, 1);

/// The status code of redirect hop `hop`, or 0 when out of range.
///
/// # Safety
/// `browser` must be a live handle from [`crate::beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_net_redirect_status(browser: *mut BeaconBrowser, index: usize, hop: usize) -> u32 {
    let b = browser!(browser, 0);
    b.requests
        .get(index)
        .and_then(|r| r.redirects.get(hop))
        .map(|(status, _)| *status as u32)
        .unwrap_or(0)
}
