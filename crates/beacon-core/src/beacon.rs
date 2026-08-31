//! The browser's reaction to the engine.
//!
//! [`Beacon`] owns the mapping between engine tabs and ours, the download list, and the
//! MRU / closed-tab state, and shares the tab manager with the frontend. Its job is
//! [`Beacon::on_engine_event`]: take one `EngineEvent`, update the tabs, and say what
//! changed in terms a frontend can draw — see [`crate::event::BeaconEvent`].
//!
//! Everything here used to live in the GTK window's `handle_engine_event`, interleaved
//! with widget calls, which meant none of it could be exercised without a display.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gosub_engine::events::{CursorShape, EngineEvent, NavigationEvent, TabCommand as EngineTabCommand};

use crate::command::BeaconCommand;
use crate::download::{DownloadState, Downloads};
use crate::event::{BeaconEvent, Cursor};
use crate::state::{ClosedTabs, MruList};
use crate::tab::{GosubTabManager, TabId};

/// The engine's own tab identifier.
pub type EngineTabId = gosub_engine::tab::TabId;

/// Browser state that reacts to the engine.
pub struct Beacon {
    /// Shared with the frontend, which also reads it to build the tab strip.
    tabs: Arc<Mutex<GosubTabManager>>,
    /// Engine tab id → our tab id, for routing engine events.
    engine_tabs: HashMap<EngineTabId, TabId>,
    downloads: Downloads,
    mru: MruList,
    closed: ClosedTabs,
    /// Talking to the engine is async, so applying a command needs somewhere to run the
    /// send. The frontend supplies its runtime rather than this crate owning one.
    rt: tokio::runtime::Handle,
}

impl Beacon {
    pub fn new(tabs: Arc<Mutex<GosubTabManager>>, rt: tokio::runtime::Handle) -> Self {
        Self {
            tabs,
            engine_tabs: HashMap::new(),
            downloads: Downloads::new(),
            mru: MruList::new(),
            closed: ClosedTabs::new(),
            rt,
        }
    }

    /// Carry out a frontend gesture.
    pub fn apply(&mut self, command: BeaconCommand) -> Vec<BeaconEvent> {
        match command {
            BeaconCommand::Back => self.send_to_active(EngineTabCommand::GoBack),
            BeaconCommand::Forward(entry) => self.send_to_active(EngineTabCommand::GoForward { entry }),
            BeaconCommand::GoToHistoryEntry(entry) => self.send_to_active(EngineTabCommand::GoForward { entry: Some(entry) }),
            BeaconCommand::Reload { ignore_cache } => self.send_to_active(EngineTabCommand::Reload { ignore_cache }),
            BeaconCommand::Stop => self.send_to_active(EngineTabCommand::CancelNavigation),

            BeaconCommand::PinTab(tab_id) => {
                self.tabs.lock().unwrap().pin_tab(tab_id);
                vec![BeaconEvent::TabsChanged]
            }
            BeaconCommand::UnpinTab(tab_id) => {
                self.tabs.lock().unwrap().unpin_tab(tab_id);
                vec![BeaconEvent::TabsChanged]
            }
        }
    }

    /// Send an engine command to the active tab, marking it loading on the way.
    ///
    /// The engine owns session history, so back/forward/reload are just asks: it answers
    /// with `HistoryChanged` and the usual navigation events, which come back through
    /// [`Self::on_engine_event`].
    fn send_to_active(&mut self, command: EngineTabCommand) -> Vec<BeaconEvent> {
        let Some(tab_id) = self.active() else {
            return Vec::new();
        };
        let handle = {
            let manager = self.tabs.lock().unwrap();
            let Some(tab) = manager.get_tab(tab_id) else {
                return Vec::new();
            };
            tab.tab_handle()
        };
        // Check for the handle BEFORE marking the tab loading. The other order -- which this
        // carried over from the GTK original -- left a tab that had no engine handle yet
        // spinning forever, because nothing would ever arrive to clear it.
        let Some(handle) = handle else {
            return vec![BeaconEvent::Log("Tab has no engine handle yet".into())];
        };
        {
            let mut manager = self.tabs.lock().unwrap();
            if let Some(mut tab) = manager.get_tab(tab_id) {
                tab.set_loading(true);
                manager.update_tab(tab_id, &tab);
            }
        }
        self.rt.spawn(async move {
            let _ = handle.send(command).await;
        });
        vec![BeaconEvent::LoadingChanged(tab_id, true), BeaconEvent::TabsChanged]
    }

    pub fn tabs(&self) -> &Arc<Mutex<GosubTabManager>> {
        &self.tabs
    }

    pub fn downloads(&self) -> &Downloads {
        &self.downloads
    }

    pub fn downloads_mut(&mut self) -> &mut Downloads {
        &mut self.downloads
    }

    pub fn mru(&self) -> &MruList {
        &self.mru
    }

    pub fn mru_mut(&mut self) -> &mut MruList {
        &mut self.mru
    }

    pub fn closed(&self) -> &ClosedTabs {
        &self.closed
    }

    pub fn closed_mut(&mut self) -> &mut ClosedTabs {
        &mut self.closed
    }

    /// Remember that `engine_id` backs `tab_id`.
    pub fn bind_engine_tab(&mut self, engine_id: EngineTabId, tab_id: TabId) {
        self.engine_tabs.insert(engine_id, tab_id);
    }

    pub fn unbind_engine_tab(&mut self, engine_id: EngineTabId) {
        self.engine_tabs.remove(&engine_id);
    }

    /// Our tab id for an engine tab, if we still have one. Events for tabs we have already
    /// forgotten are dropped rather than treated as errors: the engine and the shell close
    /// tabs independently, so in-flight events for a closed tab are normal.
    pub fn tab_for_engine(&self, engine_id: EngineTabId) -> Option<TabId> {
        self.engine_tabs.get(&engine_id).copied()
    }

    /// The tab the user is looking at.
    pub fn active(&self) -> Option<TabId> {
        self.tabs.lock().unwrap().active()
    }

    /// Translate one engine event into what the frontend should reflect, applying any tab
    /// state changes on the way.
    pub fn on_engine_event(&mut self, evt: EngineEvent) -> Vec<BeaconEvent> {
        match evt {
            EngineEvent::Redraw { .. } => vec![BeaconEvent::Redraw],

            EngineEvent::Navigation { tab_id, event } => match self.tab_for_engine(tab_id) {
                Some(our_id) => self.on_navigation(our_id, event),
                None => Vec::new(),
            },

            EngineEvent::FavIconChanged { tab_id, favicon } => {
                let Some(our_id) = self.tab_for_engine(tab_id) else {
                    return Vec::new();
                };
                self.with_tab(our_id, |tab| tab.set_favicon(Some(favicon)));
                vec![BeaconEvent::FaviconChanged(our_id), BeaconEvent::TabsChanged]
            }

            EngineEvent::TitleChanged { tab_id, title } => {
                let Some(our_id) = self.tab_for_engine(tab_id) else {
                    return Vec::new();
                };
                self.with_tab(our_id, |tab| tab.set_title(&title));
                vec![BeaconEvent::TitleChanged(our_id, title), BeaconEvent::TabsChanged]
            }

            EngineEvent::HoverUrl { tab_id, url } => match self.tab_for_engine(tab_id) {
                Some(our_id) => vec![BeaconEvent::HoverUrl(our_id, url)],
                None => Vec::new(),
            },

            EngineEvent::CursorChanged { tab_id, cursor } => {
                let Some(our_id) = self.tab_for_engine(tab_id) else {
                    return Vec::new();
                };
                let cursor = match cursor {
                    CursorShape::Pointer => Cursor::Pointer,
                    CursorShape::Text => Cursor::Text,
                    CursorShape::Default => Cursor::Default,
                };
                vec![BeaconEvent::CursorChanged(our_id, cursor)]
            }

            // The tab's engine worker died. The tab stays in the strip, marked crashed, so
            // the frontend can offer to reload it rather than taking the browser down.
            EngineEvent::TabCrashed { tab_id, error, .. } => {
                let Some(our_id) = self.tab_for_engine(tab_id) else {
                    return Vec::new();
                };
                self.unbind_engine_tab(tab_id);
                self.with_tab(our_id, |tab| {
                    tab.set_loading(false);
                    tab.set_crashed(Some(error.clone()));
                });
                vec![
                    BeaconEvent::Log(format!("Tab crashed: {error}")),
                    BeaconEvent::TabCrashed(our_id, error),
                    BeaconEvent::TabsChanged,
                    BeaconEvent::NavStateChanged(our_id),
                ]
            }

            EngineEvent::DownloadRequested {
                tab_id,
                url,
                suggested_filename,
                total_bytes,
                ..
            } => {
                let Some(our_id) = self.tab_for_engine(tab_id) else {
                    return Vec::new();
                };
                let size = total_bytes.map(|b| format!(" ({b} bytes)")).unwrap_or_default();
                vec![
                    BeaconEvent::Log(format!("Download offered: {suggested_filename}{size}")),
                    BeaconEvent::DownloadOffered {
                        tab_id: our_id,
                        url: url.to_string(),
                        suggested_filename,
                        total_bytes,
                    },
                ]
            }

            EngineEvent::DownloadProgress {
                id,
                received_bytes,
                total_bytes,
                ..
            } => {
                self.downloads.update(id.0, |e| {
                    e.received = received_bytes;
                    e.total = total_bytes;
                });
                vec![BeaconEvent::DownloadChanged(id.0)]
            }

            EngineEvent::DownloadFinished {
                id, path, received_bytes, ..
            } => {
                self.downloads.update(id.0, |e| {
                    e.received = received_bytes;
                    e.state = DownloadState::Finished;
                });
                vec![
                    BeaconEvent::Log(format!("Download #{} finished: {} ({received_bytes} bytes)", id.0, path.display())),
                    BeaconEvent::DownloadChanged(id.0),
                ]
            }

            EngineEvent::DownloadFailed { id, error, .. } => {
                self.downloads.update(id.0, |e| e.state = DownloadState::Failed(error.clone()));
                vec![
                    BeaconEvent::Log(format!("Download #{} FAILED: {error}", id.0)),
                    BeaconEvent::DownloadChanged(id.0),
                ]
            }

            _ => Vec::new(),
        }
    }

    fn on_navigation(&mut self, our_id: TabId, event: NavigationEvent) -> Vec<BeaconEvent> {
        let is_active = self.active() == Some(our_id);

        match event {
            NavigationEvent::Started { .. } => {
                if is_active {
                    // A small non-zero fraction, so the bar shows *something* the moment a
                    // load begins rather than staying empty until the first byte lands.
                    vec![BeaconEvent::LoadProgress(our_id, Some(0.05))]
                } else {
                    Vec::new()
                }
            }

            NavigationEvent::Progress {
                received_bytes,
                expected_length,
                ..
            } => {
                if !is_active {
                    return Vec::new();
                }
                let fraction = match expected_length {
                    Some(total) if total > 0 => (received_bytes as f64 / total as f64).clamp(0.05, 0.98),
                    // Unknown length: park mid-way rather than pretending precision.
                    _ => 0.5,
                };
                vec![BeaconEvent::LoadProgress(our_id, Some(fraction))]
            }

            NavigationEvent::HistoryChanged { history } => {
                // The engine also moves the address bar target: on a back/forward traversal
                // the tab's URL is the entry we moved to, even while it is still loading.
                let current_url = history.current.and_then(|id| history.entries.get(id.0)).map(|e| e.url.clone());
                let url_for_event = current_url.clone();
                self.with_tab(our_id, |tab| {
                    tab.history_mut().update(history);
                    if let Some(url) = &current_url {
                        tab.set_url(url.clone());
                    }
                });
                let mut out = Vec::new();
                if let Some(url) = url_for_event {
                    out.push(BeaconEvent::UrlChanged(our_id, url));
                }
                out.push(BeaconEvent::NavStateChanged(our_id));
                out
            }

            // Load ended without a page change (stop button, download offer).
            NavigationEvent::Cancelled { .. } => {
                if is_active {
                    vec![BeaconEvent::LoadProgress(our_id, None)]
                } else {
                    Vec::new()
                }
            }

            NavigationEvent::Failed { url, error, .. } => {
                let mut out = Vec::new();
                if is_active {
                    out.push(BeaconEvent::LoadProgress(our_id, None));
                }
                out.push(BeaconEvent::NavigationFailed(our_id, url, error.to_string()));
                out
            }

            NavigationEvent::FailedUrl { url, error, .. } => {
                vec![BeaconEvent::Log(format!("Cannot load {url}: {error}"))]
            }

            NavigationEvent::Finished { url, .. } => {
                self.with_tab(our_id, |tab| {
                    tab.set_loading(false);
                    // Session history is recorded by the engine; it follows up with a
                    // HistoryChanged event that refreshes the back/forward state, and the
                    // favicon arrives as its own event.
                    tab.set_title(url.as_str());
                });
                let mut out = Vec::new();
                if is_active {
                    out.push(BeaconEvent::LoadProgress(our_id, None));
                }
                out.push(BeaconEvent::UrlChanged(our_id, url));
                out.push(BeaconEvent::LoadingChanged(our_id, false));
                out.push(BeaconEvent::TabsChanged);
                out.push(BeaconEvent::NavStateChanged(our_id));
                out
            }

            _ => Vec::new(),
        }
    }

    /// Read-modify-write one tab through the manager, which records the change so the
    /// frontend's next refresh picks it up. A no-op if the tab is already gone.
    fn with_tab(&mut self, tab_id: TabId, apply: impl FnOnce(&mut crate::tab::GosubTab)) {
        let mut manager = self.tabs.lock().unwrap();
        if let Some(mut tab) = manager.get_tab(tab_id) {
            apply(&mut tab);
            manager.update_tab(tab_id, &tab);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tab::GosubTab;
    use gosub_engine::NavigationId;
    use url::Url;

    /// A runtime for the tests to hand Beacon. Applying a command spawns the engine send
    /// on it; nothing in these tests has a real engine to reach, so it just has to exist.
    fn test_runtime() -> tokio::runtime::Handle {
        use std::sync::OnceLock;
        static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
        RT.get_or_init(|| tokio::runtime::Runtime::new().unwrap()).handle().clone()
    }

    /// A Beacon with one tab, bound to an engine tab and active.
    fn beacon_with_tab() -> (Beacon, TabId, EngineTabId) {
        let manager = Arc::new(Mutex::new(GosubTabManager::new()));
        let tab = GosubTab::new(Url::parse("https://example.com/").unwrap(), "Example");
        let tab_id = tab.id();
        {
            let mut m = manager.lock().unwrap();
            m.add_tab(tab, None);
            m.set_active(tab_id);
        }
        let mut beacon = Beacon::new(manager, test_runtime());
        let engine_id = EngineTabId::new();
        beacon.bind_engine_tab(engine_id, tab_id);
        (beacon, tab_id, engine_id)
    }

    fn nav(tab_id: EngineTabId, event: NavigationEvent) -> EngineEvent {
        EngineEvent::Navigation { tab_id, event }
    }

    #[test]
    fn events_for_an_unknown_engine_tab_are_dropped() {
        let (mut beacon, _, _) = beacon_with_tab();
        let stranger = EngineTabId::new();
        let out = beacon.on_engine_event(EngineEvent::TitleChanged {
            tab_id: stranger,
            title: "nope".into(),
        });
        assert!(out.is_empty());
    }

    #[test]
    fn a_title_change_reaches_the_tab_and_the_strip() {
        let (mut beacon, tab_id, engine_id) = beacon_with_tab();
        let out = beacon.on_engine_event(EngineEvent::TitleChanged {
            tab_id: engine_id,
            title: "Hello".into(),
        });
        assert!(out.contains(&BeaconEvent::TitleChanged(tab_id, "Hello".into())));
        assert!(out.contains(&BeaconEvent::TabsChanged));
        let title = beacon.tabs().lock().unwrap().get_tab(tab_id).unwrap().title().to_string();
        assert_eq!(title, "Hello");
    }

    #[test]
    fn progress_with_a_known_length_reports_that_fraction() {
        let (mut beacon, tab_id, engine_id) = beacon_with_tab();
        let out = beacon.on_engine_event(nav(
            engine_id,
            NavigationEvent::Progress {
                nav_id: NavigationId::new(),
                received_bytes: 500,
                expected_length: Some(1000),
                elapsed: std::time::Duration::ZERO,
            },
        ));
        assert_eq!(out, vec![BeaconEvent::LoadProgress(tab_id, Some(0.5))]);
    }

    #[test]
    fn progress_without_a_length_parks_midway_rather_than_reporting_nothing() {
        let (mut beacon, tab_id, engine_id) = beacon_with_tab();
        let out = beacon.on_engine_event(nav(
            engine_id,
            NavigationEvent::Progress {
                nav_id: NavigationId::new(),
                received_bytes: 500,
                expected_length: None,
                elapsed: std::time::Duration::ZERO,
            },
        ));
        assert_eq!(out, vec![BeaconEvent::LoadProgress(tab_id, Some(0.5))]);
    }

    #[test]
    fn a_zero_length_response_does_not_divide_by_zero() {
        let (mut beacon, tab_id, engine_id) = beacon_with_tab();
        let out = beacon.on_engine_event(nav(
            engine_id,
            NavigationEvent::Progress {
                nav_id: NavigationId::new(),
                received_bytes: 0,
                expected_length: Some(0),
                elapsed: std::time::Duration::ZERO,
            },
        ));
        assert_eq!(out, vec![BeaconEvent::LoadProgress(tab_id, Some(0.5))]);
    }

    #[test]
    fn progress_for_a_background_tab_is_not_reported() {
        let (mut beacon, _, engine_id) = beacon_with_tab();
        // A second tab takes over as active, so the first is now in the background.
        let other = GosubTab::new(Url::parse("https://other.example/").unwrap(), "Other");
        let other_id = other.id();
        {
            let mut m = beacon.tabs().lock().unwrap();
            m.add_tab(other, None);
            m.set_active(other_id);
        }
        let out = beacon.on_engine_event(nav(
            engine_id,
            NavigationEvent::Progress {
                nav_id: NavigationId::new(),
                received_bytes: 1,
                expected_length: Some(2),
                elapsed: std::time::Duration::ZERO,
            },
        ));
        assert!(out.is_empty(), "a background tab must not move the address bar: {out:?}");
    }

    #[test]
    fn finishing_a_load_clears_progress_and_stops_the_spinner() {
        let (mut beacon, tab_id, engine_id) = beacon_with_tab();
        beacon.tabs().lock().unwrap().get_tab(tab_id).unwrap();
        let url = Url::parse("https://example.com/page").unwrap();
        let out = beacon.on_engine_event(nav(
            engine_id,
            NavigationEvent::Finished {
                nav_id: NavigationId::new(),
                url: url.clone(),
            },
        ));
        assert!(out.contains(&BeaconEvent::LoadProgress(tab_id, None)));
        assert!(out.contains(&BeaconEvent::UrlChanged(tab_id, url)));
        assert!(out.contains(&BeaconEvent::LoadingChanged(tab_id, false)));
        assert!(!beacon.tabs().lock().unwrap().get_tab(tab_id).unwrap().is_loading());
    }

    #[test]
    fn a_favicon_lands_on_the_tab_as_bytes() {
        let (mut beacon, tab_id, engine_id) = beacon_with_tab();
        let out = beacon.on_engine_event(EngineEvent::FavIconChanged {
            tab_id: engine_id,
            favicon: vec![1, 2, 3],
        });
        assert!(out.contains(&BeaconEvent::FaviconChanged(tab_id)));
        let tabs = beacon.tabs().lock().unwrap();
        assert_eq!(tabs.get_tab(tab_id).unwrap().favicon(), Some(&[1u8, 2, 3][..]));
    }

    #[test]
    fn a_crash_marks_the_tab_and_forgets_its_engine_id() {
        let (mut beacon, tab_id, engine_id) = beacon_with_tab();
        let out = beacon.on_engine_event(EngineEvent::TabCrashed {
            tab_id: engine_id,
            zone_id: gosub_engine::zone::ZoneId::default(),
            error: "boom".into(),
        });
        assert!(out.contains(&BeaconEvent::TabCrashed(tab_id, "boom".into())));
        assert!(beacon.tab_for_engine(engine_id).is_none());
        let tabs = beacon.tabs().lock().unwrap();
        assert_eq!(tabs.get_tab(tab_id).unwrap().crashed(), Some("boom"));
    }

    #[test]
    fn a_crashed_tabs_later_events_are_dropped_rather_than_misrouted() {
        let (mut beacon, _, engine_id) = beacon_with_tab();
        beacon.on_engine_event(EngineEvent::TabCrashed {
            tab_id: engine_id,
            zone_id: gosub_engine::zone::ZoneId::default(),
            error: "boom".into(),
        });
        let out = beacon.on_engine_event(EngineEvent::TitleChanged {
            tab_id: engine_id,
            title: "ghost".into(),
        });
        assert!(out.is_empty());
    }

    #[test]
    fn hover_is_reported_for_the_active_tab() {
        let (mut beacon, tab_id, engine_id) = beacon_with_tab();
        let out = beacon.on_engine_event(EngineEvent::HoverUrl {
            tab_id: engine_id,
            url: Some("https://link.example/".into()),
        });
        assert_eq!(out, vec![BeaconEvent::HoverUrl(tab_id, Some("https://link.example/".into()))]);
    }

    #[test]
    fn a_cursor_shape_crosses_the_seam_without_a_toolkit_name() {
        let (mut beacon, tab_id, engine_id) = beacon_with_tab();
        let out = beacon.on_engine_event(EngineEvent::CursorChanged {
            tab_id: engine_id,
            cursor: CursorShape::Pointer,
        });
        assert_eq!(out, vec![BeaconEvent::CursorChanged(tab_id, Cursor::Pointer)]);
    }

    #[test]
    fn pinning_a_tab_goes_through_the_manager_and_redraws_the_strip() {
        let (mut beacon, tab_id, _) = beacon_with_tab();
        let out = beacon.apply(BeaconCommand::PinTab(tab_id));
        assert_eq!(out, vec![BeaconEvent::TabsChanged]);
        assert!(beacon.tabs().lock().unwrap().get_tab(tab_id).unwrap().is_pinned());

        let out = beacon.apply(BeaconCommand::UnpinTab(tab_id));
        assert_eq!(out, vec![BeaconEvent::TabsChanged]);
        assert!(!beacon.tabs().lock().unwrap().get_tab(tab_id).unwrap().is_pinned());
    }

    #[test]
    fn a_navigation_command_with_no_active_tab_does_nothing() {
        let manager = Arc::new(Mutex::new(GosubTabManager::new()));
        let mut beacon = Beacon::new(manager, test_runtime());
        assert!(beacon.apply(BeaconCommand::Back).is_empty());
        assert!(beacon.apply(BeaconCommand::Reload { ignore_cache: false }).is_empty());
    }

    #[test]
    fn a_tab_without_an_engine_handle_reports_rather_than_pretending_to_navigate() {
        // The tab in this fixture was never given a TabHandle, which is the state a tab is
        // in between being created and the engine catching up.
        let (mut beacon, tab_id, _) = beacon_with_tab();
        let out = beacon.apply(BeaconCommand::Back);
        assert!(matches!(out.as_slice(), [BeaconEvent::Log(_)]), "expected a log line, got {out:?}");
        // ...and it must not be left looking like a load is under way, or the tab spins
        // forever: nothing is coming back to clear it.
        assert!(!beacon.tabs().lock().unwrap().get_tab(tab_id).unwrap().is_loading());
    }
}
