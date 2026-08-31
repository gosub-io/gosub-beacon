use beacon_core::tab::TabId;
use std::fmt;
use std::fmt::{Debug, Formatter};

pub enum Message {
    /// Open a new tab in the background, and load a URL. Used for startup tabs, where the
    /// first URL should end up active rather than the last.
    OpenTab(String, String),
    /// Open a new tab and switch to it. The explicit new-tab gesture (Ctrl+T, the + button)
    /// only; link-opening stays on `OpenTabRight`, which must not steal focus.
    OpenTabForeground(String, String),
    /// Opens a new tab on the right side of the given TabID
    OpenTabRight(TabId, String, String),
    /// Sent when we need to load a new url into a tab
    LoadUrl(TabId, String),

    /// Refresh tabs
    RefreshTabs(),

    /// Apply session flags after the restored tabs opened: which insertion-order
    /// indices are pinned, and which one becomes active.
    RestoreSession { pinned: Vec<usize>, active: Option<usize> },

    /// Pins a tab
    PinTab(TabId),
    /// Unpins a tab
    UnpinTab(TabId),

    /// Single message to print in the log
    Log(String),

    /// Raw favicon bytes fetched for a tab (decoded on the GTK thread)
    FaviconLoaded(TabId, Vec<u8>),
}

impl Debug for Message {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Message::OpenTab(url, title) => write!(f, "OpenTab({} {})", url, title),
            Message::OpenTabForeground(url, title) => write!(f, "OpenTabForeground({} {})", url, title),
            Message::OpenTabRight(tab_id, url, title) => write!(f, "OpenTabRight({:?}, {} {})", tab_id, url, title),
            Message::LoadUrl(tab_id, url) => write!(f, "LoadUrl({:?}, {})", tab_id, url),
            Message::RefreshTabs() => write!(f, "RefreshTabs()"),
            Message::RestoreSession { pinned, active } => {
                write!(f, "RestoreSession(pinned: {:?}, active: {:?})", pinned, active)
            }
            Message::Log(msg) => write!(f, "Log({})", msg),
            Message::PinTab(tab_id) => write!(f, "PinTab({:?})", tab_id),
            Message::UnpinTab(tab_id) => write!(f, "UnpinTab({:?})", tab_id),
            Message::FaviconLoaded(tab_id, bytes) => write!(f, "FaviconLoaded({:?}, {} bytes)", tab_id, bytes.len()),
        }
    }
}
