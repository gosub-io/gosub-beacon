//! The browser, minus the window.
//!
//! Tabs, navigation, session history mirroring, session save/restore, downloads,
//! bookmarks queries and address parsing belong here — everything a frontend needs to
//! decide *what* to show, with no opinion about *how*. Nothing in this crate may depend
//! on GTK, egui, winit or any other toolkit; that constraint is the whole point.

pub mod address_parser;
pub mod beacon;
pub mod cli;
pub mod command;
pub mod download;
pub mod engine;
pub mod event;
pub mod paths;
pub mod session;
pub mod source_page;
pub mod state;
pub mod tab;
