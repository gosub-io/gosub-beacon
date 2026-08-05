use crate::tab::TabId;
use std::fmt;
use std::fmt::{Debug, Formatter};

pub enum Message {
    /// Open a new tab, and load a URL
    OpenTab(String, String),
    /// Opens a new tab on the right side of the given TabID
    OpenTabRight(TabId, String, String),
    /// Sent when we need to load a new url into a tab
    LoadUrl(TabId, String),

    /// Refresh tabs
    RefreshTabs(),

    /// Pins a tab
    PinTab(TabId),
    /// Unpins a tab
    UnpinTab(TabId),

    /// Single message to print in the log
    Log(String),

    /// Show the fetched page source (url, source) in a viewer window
    ShowSource(String, String),

    /// Raw favicon bytes fetched for a tab (decoded on the GTK thread)
    FaviconLoaded(TabId, Vec<u8>),
}

impl Debug for Message {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Message::OpenTab(url, title) => write!(f, "OpenTab({} {})", url, title),
            Message::OpenTabRight(tab_id, url, title) => write!(f, "OpenTabRight({:?}, {} {})", tab_id, url, title),
            Message::LoadUrl(tab_id, url) => write!(f, "LoadUrl({:?}, {})", tab_id, url),
            Message::RefreshTabs() => write!(f, "RefreshTabs()"),
            Message::Log(msg) => write!(f, "Log({})", msg),
            Message::PinTab(tab_id) => write!(f, "PinTab({:?})", tab_id),
            Message::UnpinTab(tab_id) => write!(f, "UnpinTab({:?})", tab_id),
            Message::ShowSource(url, content) => write!(f, "ShowSource({}, {} bytes)", url, content.len()),
            Message::FaviconLoaded(tab_id, bytes) => write!(f, "FaviconLoaded({:?}, {} bytes)", tab_id, bytes.len()),
        }
    }
}
