//! What a frontend asks the browser to do.
//!
//! The counterpart to [`crate::event::BeaconEvent`]: a gesture arrives, the frontend turns
//! it into a [`BeaconCommand`], and [`crate::beacon::Beacon::apply`] decides what that
//! means. A toolbar button, a menu item and a keyboard accelerator that all mean "go back"
//! produce the same command, and the rule for what "back" does lives in one place.
//!
//! These are not the engine's commands. `gosub_engine::events::TabCommand` already covers
//! navigation at the engine's level, and a `BeaconCommand` mostly forwards to one — after
//! deciding *which* tab, and updating the tab state that the engine knows nothing about.
//! Anything the engine already has an opinion about belongs there, not here.

use crate::tab::{HistoryEntryId, TabId};

/// A browser action, independent of what triggered it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeaconCommand {
    /// Go back in the active tab's session history.
    Back,
    /// Go forward. `None` follows the preferred branch; a specific entry picks one when
    /// the history tree forks.
    Forward(Option<HistoryEntryId>),
    /// Jump to a specific entry in the active tab's history.
    GoToHistoryEntry(HistoryEntryId),
    /// Reload the active tab.
    Reload { ignore_cache: bool },
    /// Stop the active tab's in-flight load.
    Stop,

    /// Put text on the system clipboard (a context-menu "Copy …").
    CopyText(String),
    /// Open a finished download in the desktop's default application.
    OpenDownload(u64),

    /// Pin a tab to the left of the strip.
    PinTab(TabId),
    /// Unpin a tab.
    UnpinTab(TabId),
}
