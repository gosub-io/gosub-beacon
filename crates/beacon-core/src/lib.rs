//! The browser, minus the window.
//!
//! Tabs, navigation, session history mirroring, session save/restore, downloads,
//! bookmarks queries and address parsing belong here — everything a frontend needs to
//! decide *what* to show, with no opinion about *how*. Nothing in this crate may depend
//! on GTK, egui, winit or any other toolkit; that constraint is the whole point.
//!
//! Empty for now. It fills up as state moves out of `beacon-gtk`'s window widget,
//! module by module, each move keeping the GTK frontend working as it goes.
