//! What a shell needs about the page in front of it, beyond drawing it: what is under the
//! pointer, the source behind it, where forward leads when history forks, and how to
//! restart a tab whose worker died.
//!
//! Everything here follows the boundary's two habits: an answer that takes time arrives as
//! an event and is then read by accessor, and anything that is a *rule* rather than a
//! rendering lives in `beacon_core` so the GTK shell and this one cannot disagree about it.

use std::ffi::c_char;

use beacon_core::tab::TabId;
use gosub_engine::events::{HitTestToken, TabCommand};

use crate::{to_c_string, BeaconBrowser};

// ── what is under the pointer ────────────────────────────────────────────────

/// Ask what is at (`x`, `y`) in the page, in CSS pixels. Returns a token, or 0 if the tab
/// is gone.
///
/// The answer arrives as a `BEACON_HIT_TEST` event carrying the same token in `number`;
/// read it with the accessors below. Asynchronous because the engine answers from the
/// layout tree on its own thread, and a shell that blocked for it would stall its own
/// event loop on every right-click.
///
/// A context menu is the reason this exists: "Open Link in New Tab" needs to know there is
/// a link, and the hover URL only covers the pointer sitting still on one.
///
/// # Safety
/// `browser` must be a live handle from [`crate::beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_hit_test(browser: *mut BeaconBrowser, tab: u64, x: f32, y: f32) -> u64 {
    let b = browser!(browser, 0);
    let Some(tab_id) = b.tab(tab) else { return 0 };

    let token = b.next_hit_token;
    b.next_hit_token += 1;
    b.hit_tabs.insert(token, tab_id);
    b.send(
        tab_id,
        TabCommand::QueryHitTest {
            x,
            y,
            token: HitTestToken(token),
        },
    );
    token
}

macro_rules! hit_string {
    ($name:ident, $doc:literal, |$hit:ident| $value:expr) => {
        #[doc = $doc]
        ///
        /// From the last `BEACON_HIT_TEST` answer. Free with [`crate::beacon_string_free`];
        /// NULL when there was nothing of the kind at the point.
        ///
        /// # Safety
        /// `browser` must be a live handle from [`crate::beacon_new`].
        #[no_mangle]
        pub unsafe extern "C" fn $name(browser: *mut BeaconBrowser) -> *mut c_char {
            let b = browser!(browser, std::ptr::null_mut());
            let Some($hit) = b.hit.as_ref() else {
                return std::ptr::null_mut();
            };
            match $value {
                Some(value) => to_c_string(value),
                None => std::ptr::null_mut(),
            }
        }
    };
}

hit_string!(beacon_hit_link, "The nearest enclosing link.", |hit| hit.link_url.as_deref());
hit_string!(beacon_hit_image, "The image at the point.", |hit| hit.image_url.as_deref());
hit_string!(
    beacon_hit_text,
    "The text node at the point, trimmed — what a `Copy` item on a context menu would copy.",
    |hit| hit.text.as_deref()
);
hit_string!(
    beacon_hit_selection,
    "The current selection, when the point lies inside it. Always NULL until text selection lands in the engine.",
    |hit| hit.selection.as_deref()
);

/// Whether the point is inside a text-editable control.
///
/// # Safety
/// `browser` must be a live handle from [`crate::beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_hit_is_editable(browser: *mut BeaconBrowser) -> bool {
    let b = browser!(browser, false);
    b.hit.as_ref().is_some_and(|hit| hit.is_editable)
}

// ── the page's source ────────────────────────────────────────────────────────

/// Open the source of `tab` in a new tab, highlighted unless `raw`. Returns the new tab's
/// handle, or 0.
///
/// Equivalent to opening `view-source:<url>` (or `raw:<url>`) yourself — those addresses
/// work from [`crate::beacon_open_tab`] and [`crate::beacon_navigate`] too, so a typed
/// address behaves the same as the menu item.
///
/// The bytes come from a one-shot fetch outside the engine, which carries no cookies and
/// shares no cache with it: the source of a page behind a login is the logged-out HTML.
/// That is a stopgap in `beacon_core::fetch` and not a property of this call.
///
/// # Safety
/// `browser` must be a live handle from [`crate::beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_view_source(browser: *mut BeaconBrowser, tab: u64, raw: bool) -> u64 {
    let b = browser!(browser, 0);
    let Some(tab_id) = b.tab(tab) else { return 0 };
    let url = {
        let tabs = b.tabs.lock().unwrap();
        match tabs.get_tab(tab_id) {
            Some(tab) => tab.url().to_string(),
            None => return 0,
        }
    };
    // Through the address, not around it: the prefix is what the tab's URL will read, and
    // reload has to be able to redo this from that alone.
    let prefix = if raw { "raw:" } else { "view-source:" };
    let position = b.strip_position(tab_id).map(|index| index + 1);
    b.open_tab_at(&format!("{prefix}{url}"), position)
}

// ── a tab whose worker died ──────────────────────────────────────────────────

/// Why the tab crashed, or NULL if it did not.
///
/// A crashed tab keeps its place in the strip, its title and its address — it is a tab
/// that cannot draw, not a tab that is gone — so a shell can show the reason over it and
/// offer to start it again.
///
/// Free with [`crate::beacon_string_free`].
///
/// # Safety
/// `browser` must be a live handle from [`crate::beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_tab_crash_reason(browser: *mut BeaconBrowser, tab: u64) -> *mut c_char {
    let b = browser!(browser, std::ptr::null_mut());
    let Some(tab_id) = b.tab(tab) else {
        return std::ptr::null_mut();
    };
    let tabs = b.tabs.lock().unwrap();
    match tabs.get_tab(tab_id).and_then(|t| t.crashed().map(str::to_string)) {
        Some(reason) => to_c_string(&reason),
        None => std::ptr::null_mut(),
    }
}

/// Give a crashed tab a new engine worker and reload what it was showing. False when the
/// tab is unknown or the engine would not make one.
///
/// # Safety
/// `browser` must be a live handle from [`crate::beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_revive_tab(browser: *mut BeaconBrowser, tab: u64) -> bool {
    let b = browser!(browser, false);
    let Some(tab_id) = b.tab(tab) else { return false };
    b.revive(tab_id)
}

// ── where forward leads ──────────────────────────────────────────────────────

/// Snapshot the entries forward of where `tab` is, and return how many.
///
/// Usually one — the page you just came back from. More than one means the history forked:
/// you went back and then somewhere else, and both branches are still there. That is what
/// a press-and-hold on a Forward button offers, and the reason this is a list rather than
/// a boolean.
///
/// # Safety
/// `browser` must be a live handle from [`crate::beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_forward_snapshot(browser: *mut BeaconBrowser, tab: u64) -> usize {
    let b = browser!(browser, 0);
    let Some(tab_id) = b.tab(tab) else { return 0 };
    let tabs = b.tabs.lock().unwrap();
    b.forward = tabs.get_tab(tab_id).map(|t| t.history().forward_children()).unwrap_or_default();
    b.forward.len()
}

/// The URL of forward entry `index`. Free with [`crate::beacon_string_free`].
///
/// # Safety
/// `browser` must be a live handle from [`crate::beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_forward_url(browser: *mut BeaconBrowser, index: usize) -> *mut c_char {
    let b = browser!(browser, std::ptr::null_mut());
    match b.forward.get(index) {
        Some((_, url)) => to_c_string(url.as_str()),
        None => std::ptr::null_mut(),
    }
}

/// Go forward to entry `index` of the last snapshot, rather than to whichever branch the
/// engine prefers.
///
/// # Safety
/// `browser` must be a live handle from [`crate::beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_forward_go(browser: *mut BeaconBrowser, index: usize) {
    let b = browser!(browser);
    let Some((entry, _)) = b.forward.get(index).cloned() else {
        return;
    };
    let events = b.beacon.apply(beacon_core::command::BeaconCommand::Forward(Some(entry)));
    b.queue(events);
}

// ── the previous session ─────────────────────────────────────────────────────

/// Read the tabs the last session had open, and return how many.
///
/// The file is written as the browser runs — a private session never writes — so there is
/// nothing to call on the way out. A shell restores this when it is started with no URL of
/// its own, exactly as the GTK one does.
///
/// # Safety
/// `browser` must be a live handle from [`crate::beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_session_snapshot(browser: *mut BeaconBrowser) -> usize {
    let b = browser!(browser, 0);
    b.session = beacon_core::session::load();
    b.session.len()
}

/// The URL of session tab `index`. Free with [`crate::beacon_string_free`].
///
/// # Safety
/// `browser` must be a live handle from [`crate::beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_session_url(browser: *mut BeaconBrowser, index: usize) -> *mut c_char {
    let b = browser!(browser, std::ptr::null_mut());
    match b.session.get(index) {
        Some(tab) => to_c_string(&tab.url),
        None => std::ptr::null_mut(),
    }
}

/// Whether session tab `index` was pinned.
///
/// # Safety
/// `browser` must be a live handle from [`crate::beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_session_pinned(browser: *mut BeaconBrowser, index: usize) -> bool {
    let b = browser!(browser, false);
    b.session.get(index).is_some_and(|tab| tab.pinned)
}

/// Whether session tab `index` was the one in front.
///
/// # Safety
/// `browser` must be a live handle from [`crate::beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_session_active(browser: *mut BeaconBrowser, index: usize) -> bool {
    let b = browser!(browser, false);
    b.session.get(index).is_some_and(|tab| tab.active)
}

impl BeaconBrowser {
    /// Where `tab_id` sits in the strip.
    pub(crate) fn strip_position(&self, tab_id: TabId) -> Option<usize> {
        self.tabs.lock().unwrap().order().iter().position(|id| *id == tab_id)
    }

    /// Give a crashed tab a new worker and reload it. The tab keeps its identity: its
    /// handle, its place in the strip and its address all survive, which is what makes
    /// this a restart rather than "close it and open another one".
    pub(crate) fn revive(&mut self, tab_id: TabId) -> bool {
        let url = {
            let tabs = self.tabs.lock().unwrap();
            match tabs.get_tab(tab_id) {
                Some(tab) => tab.url().to_string(),
                None => return false,
            }
        };
        let viewport = self
            .viewports
            .get(&tab_id)
            .map(|v| (v.logical_width.max(1), v.logical_height.max(1)));
        let handle = match self.engine.create_tab(crate::runtime(), "New Tab", viewport) {
            Ok(handle) => handle,
            Err(e) => {
                log::warn!("could not restart the tab's engine worker: {e}");
                return false;
            }
        };

        self.beacon.bind_engine_tab(handle.tab_id, tab_id);
        {
            let mut tabs = self.tabs.lock().unwrap();
            let Some(mut tab) = tabs.get_tab(tab_id) else {
                return false;
            };
            tab.set_tab_handle(handle);
            tab.set_crashed(None);
            tab.set_loading(true);
            tabs.update_tab(tab_id, &tab);
        }
        self.load(tab_id, &url);
        self.pending
            .push(crate::Outgoing::Core(beacon_core::event::BeaconEvent::TabsChanged));
        true
    }
}
