//! What a frontend redraws from.
//!
//! [`BeaconEvent`] is the browser telling its host that something changed. It is
//! deliberately about *what happened*, never about what to draw: "the active tab changed"
//! rather than "set the stack's visible child". A GTK window and an egui frame should both
//! be able to act on every variant here without either of them being the obvious one.
//!
//! These are not the engine's events. [`crate::beacon::Beacon`] consumes
//! `gosub_engine::events::EngineEvent`, updates the tabs, and emits these — so the
//! frontend never sees an engine tab id or has to decide what a navigation means.

use crate::tab::TabId;
use url::Url;

/// The pointer shape the page wants, independent of any toolkit's cursor names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    Default,
    Pointer,
    Text,
}

/// Something the frontend should reflect.
#[derive(Debug, Clone, PartialEq)]
pub enum BeaconEvent {
    /// A new frame is ready; repaint the page areas.
    Redraw,

    /// The tab strip needs rebuilding (a tab appeared, closed, or changed).
    TabsChanged,
    /// `tab_id` is now the tab the user is looking at.
    ActiveTabChanged(TabId),
    /// The page reported a new title.
    TitleChanged(TabId, String),
    /// The tab's address changed — a navigation committed, or history moved.
    UrlChanged(TabId, Url),
    /// The tab started or stopped loading.
    LoadingChanged(TabId, bool),
    /// Load progress as a fraction, or `None` when no load is in flight and any progress
    /// indication should be cleared. A load whose total length the server never gave still
    /// reports a fraction — parked mid-way — rather than `None`.
    LoadProgress(TabId, Option<f64>),
    /// The tab's favicon bytes changed; read them off the tab.
    FaviconChanged(TabId),
    /// Back/forward availability changed for this tab.
    NavStateChanged(TabId),
    /// A navigation failed. The frontend decides what to show — Beacon renders an error
    /// page into the tab rather than replacing the chrome.
    NavigationFailed(TabId, Url, String),
    /// The tab's engine worker died; the tab is now marked crashed.
    TabCrashed(TabId, String),

    /// The pointer is over a link (or left one).
    HoverUrl(TabId, Option<String>),
    /// The pointer shape for what is under the cursor.
    CursorChanged(TabId, Cursor),

    /// A download was offered; the frontend should ask the user where to put it.
    DownloadOffered {
        tab_id: TabId,
        url: String,
        suggested_filename: String,
        total_bytes: Option<u64>,
    },
    /// A download's row changed (progress, finished or failed).
    DownloadChanged(u64),

    /// Something worth showing in the log pane.
    Log(String),
}
