use gosub_engine::tab::TabHandle;
use gosub_engine::tab::TabId as EngineTabId;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::fmt::Debug;
use std::str::FromStr;
use url::Url;
use uuid::Uuid;

pub use gosub_engine::tab::{HistoryEntryId, HistorySnapshot};

/// Session history lives in the engine (a tree: back = parent, forward = children); the shell
/// only mirrors the latest [`HistorySnapshot`] it was sent so the back/forward buttons and the
/// forward-branch menu can update without a round-trip. Empty until the first
/// `HistoryChanged` event arrives.
#[derive(Clone, Debug, Default)]
pub struct History {
    snapshot: Option<HistorySnapshot>,
}

impl History {
    pub fn update(&mut self, snapshot: HistorySnapshot) {
        self.snapshot = Some(snapshot);
    }

    pub fn can_go_back(&self) -> bool {
        self.snapshot.as_ref().is_some_and(|s| s.can_go_back)
    }

    pub fn can_go_forward(&self) -> bool {
        self.snapshot.as_ref().is_some_and(|s| !s.forward.is_empty())
    }

    /// Forward branches of the current entry (preferred first), as `(id, url)` pairs.
    pub fn forward_children(&self) -> Vec<(HistoryEntryId, Url)> {
        self.snapshot
            .as_ref()
            .map(|s| s.forward.iter().map(|e| (e.id, e.url.clone())).collect())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Copy)]
pub struct TabId(Uuid);

impl Default for TabId {
    fn default() -> Self {
        Self::new()
    }
}

impl TabId {
    pub fn new() -> Self {
        TabId(Uuid::new_v4())
    }

    pub fn from_uuid(uuid: Uuid) -> Self {
        TabId(uuid)
    }
}

impl FromStr for TabId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(TabId)
    }
}

// Optional: Implement `Display` for easier printing
impl fmt::Display for TabId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone)]
pub struct GosubTab {
    /// Tab is currently loading
    loading: bool,
    /// Id of the tab
    id: TabId,
    /// Tab is pinned and cannot be moved from the leftmost position
    pinned: bool,
    /// Tab content is private and not saved in history
    private: bool,
    /// URL that is loaded into the tab
    url: Url,
    /// Mirror of the engine's session history for this tab (back/forward state)
    history: History,
    /// Panic message when the engine worker for this tab crashed.
    crashed: Option<String>,
    /// Title of the tab
    title: String,
    /// The tab's favicon, exactly as the engine fetched it: an encoded image, usually
    /// PNG or ICO. Decoding needs a toolkit, so it is left to whichever frontend is
    /// drawing the tab strip.
    favicon: Option<Vec<u8>>,
    /// Actual content (HTML) of the tab
    content: String,
    /// Handle to the engine-side tab that drives navigation and rendering.
    tab_handle: Option<TabHandle>,
}

impl Debug for GosubTab {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GosubTab")
            .field("id", &self.id)
            .field("title", &self.title)
            .finish()
    }
}

impl GosubTab {
    pub fn new(url: Url, title: &str) -> Self {
        GosubTab {
            loading: false,
            id: TabId::new(),
            pinned: false,
            private: false,
            url,
            history: History::default(),
            crashed: None,
            title: title.to_string(),
            favicon: None,
            content: String::new(),
            tab_handle: None,
        }
    }

    /// Returns the engine tab handle, if one has been attached.
    pub fn tab_handle(&self) -> Option<TabHandle> {
        self.tab_handle.clone()
    }

    /// Returns the engine-side tab id, if a handle has been attached.
    pub fn engine_tab_id(&self) -> Option<EngineTabId> {
        self.tab_handle.as_ref().map(|h| h.tab_id)
    }

    /// Returns true once this tab is backed by an engine tab.
    pub fn has_engine_tab(&self) -> bool {
        self.tab_handle.is_some()
    }

    pub fn set_tab_handle(&mut self, handle: TabHandle) {
        self.tab_handle = Some(handle);
    }

    /// Panic message when the tab's engine worker crashed; `None` while healthy.
    /// A crashed tab keeps its (dead) handle until `set_crashed(None)` after a revive.
    pub fn crashed(&self) -> Option<&str> {
        self.crashed.as_deref()
    }

    pub fn set_crashed(&mut self, error: Option<String>) {
        self.crashed = error;
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
    }

    pub fn id(&self) -> TabId {
        self.id
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn set_pinned(&mut self, pinned: bool) {
        self.pinned = pinned;
    }

    pub fn is_pinned(&self) -> bool {
        self.pinned
    }

    pub fn set_private(&mut self, private: bool) {
        self.private = private;
    }

    pub fn set_content(&mut self, content: &str) {
        self.content = content.to_string();
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn set_url(&mut self, url: Url) {
        self.url = url
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    pub fn history_mut(&mut self) -> &mut History {
        &mut self.history
    }

    pub fn set_title(&mut self, title: &str) {
        self.title = title.to_string();
    }

    pub fn favicon(&self) -> Option<&[u8]> {
        self.favicon.as_deref()
    }

    pub fn set_favicon(&mut self, favicon: Option<Vec<u8>>) {
        self.favicon = favicon;
    }
}

#[derive(Debug)]
pub enum TabCommand {
    Close(TabId), // Close index
    #[allow(dead_code)]
    CloseAll, // Close all
    Move(TabId, u32), // tab has been moved to given position
    Update(TabId), // Update tab (tab + content)
    Insert(TabId, u32), // Insert new tab at given position
    Activate(TabId), // Set as active
}

pub struct GosubTabManager {
    // All known tabs in the system
    tabs: HashMap<TabId, GosubTab>,
    // Actual ordering of the pinned tabs in the notebook.
    pinned_tab_order: VecDeque<TabId>,
    // Actual ordering of the ubpinned tabs in the notebook.
    unpinned_tab_order: VecDeque<TabId>,
    // list of commands to execute on the next tab notebook update
    commands: Vec<TabCommand>,
}

impl Default for GosubTabManager {
    fn default() -> Self {
        Self::new()
    }
}

impl GosubTabManager {
    pub fn new() -> Self {
        GosubTabManager {
            tabs: HashMap::new(),
            unpinned_tab_order: VecDeque::new(),
            pinned_tab_order: VecDeque::new(),
            commands: Vec::new(),
        }
    }

    #[allow(dead_code)]
    pub fn get_by_tab(&self, tab_id: TabId) -> Option<&GosubTab> {
        self.tabs.get(&tab_id)
    }

    pub fn commands(&mut self) -> Vec<TabCommand> {
        std::mem::take(&mut self.commands)
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    /// Returns true when the given tab is the leftmost unpinned tab
    pub fn is_most_left_unpinned_tab(&self, tab_id: TabId) -> bool {
        self.unpinned_tab_order.front() == Some(&tab_id)
    }

    /// Returns true when the given tab is the rightmost tab
    pub fn is_most_right_tab(&self, tab_id: TabId) -> bool {
        self.unpinned_tab_order.back() == Some(&tab_id)
    }

    pub fn set_active(&mut self, tab_id: TabId) {
        self.commands.push(TabCommand::Activate(tab_id));
    }

    pub fn notify_tab_changed(&mut self, tab_id: TabId) {
        self.commands.push(TabCommand::Update(tab_id));
    }

    pub fn update_tab(&mut self, tab_id: TabId, tab: &GosubTab) {
        self.tabs.insert(tab_id, tab.clone());
        self.notify_tab_changed(tab_id);
    }

    pub fn pin_tab(&mut self, tab_id: TabId) {
        let tab = self.tabs.get_mut(&tab_id).unwrap();
        tab.set_pinned(true);

        self.unpinned_tab_order.retain(|id| id != &tab_id);
        self.pinned_tab_order.push_back(tab_id);

        // Tab has been moved to end of pinned tabs
        self.commands.push(TabCommand::Update(tab_id));
        self.commands
            .push(TabCommand::Move(tab_id, (self.pinned_tab_order.len() - 1) as u32));
    }

    pub fn unpin_tab(&mut self, tab_id: TabId) {
        let tab = self.tabs.get_mut(&tab_id).unwrap();
        tab.set_pinned(false);

        self.pinned_tab_order.retain(|id| id != &tab_id);
        self.unpinned_tab_order.push_front(tab_id);

        // Tab has been moved to begin of unpinned tabs
        self.commands.push(TabCommand::Update(tab_id));
        self.commands.push(TabCommand::Move(tab_id, self.pinned_tab_order.len() as u32));
    }

    /// Add `tab`, optionally at `position` — an index into the **visible strip** (see
    /// [`Self::order`]), which is what callers derive from the tab bar and what the view
    /// expects back on `TabCommand::Insert`. `None` appends.
    ///
    /// Pinned tabs occupy the front of the strip, so a pinned tab always lands among the
    /// pinned and an unpinned one among the unpinned; a strip index pointing into the other
    /// run is clamped to the nearest valid slot rather than crossing the boundary.
    ///
    /// The per-list index must not escape this function. It used to: the emitted position was
    /// an index into `unpinned_tab_order` while the view applied it to the whole strip, so with
    /// any pinned tab a new tab landed `pinned_len` slots too far left — and the incoming
    /// `position`, already a strip index, was inserted into the list as if it were a local one.
    pub fn add_tab(&mut self, tab: GosubTab, position: Option<usize>) -> TabId {
        let tab_id = tab.id();
        let pinned = tab.is_pinned();
        let pinned_len = self.pinned_tab_order.len();

        // Strip index -> index within the list this tab belongs to.
        let target = match position {
            None => usize::MAX, // append
            Some(strip) if pinned => strip.min(pinned_len),
            Some(strip) => strip.saturating_sub(pinned_len).min(self.unpinned_tab_order.len()),
        };

        let list = if pinned {
            &mut self.pinned_tab_order
        } else {
            &mut self.unpinned_tab_order
        };
        let local = if target > list.len() {
            list.push_back(tab_id);
            list.len() - 1
        } else {
            list.insert(target, tab_id);
            target
        };

        // Report the resting place as a strip index, matching what the view consumes.
        let global = if pinned { local } else { pinned_len + local };
        self.commands.push(TabCommand::Insert(tab_id, global as u32));

        self.tabs.insert(tab_id, tab);

        tab_id
    }

    /// The tab that should take focus once the tab at `index` has been removed from `order`:
    /// the one before it, or else whichever shifted into its place.
    fn neighbour_of(order: &VecDeque<TabId>, index: usize) -> Option<TabId> {
        if index == 0 {
            order.front().copied()
        } else {
            order.get(index - 1).copied()
        }
    }

    pub fn remove_tab(&mut self, tab_id: TabId) {
        // A tab lives in exactly one of the two order lists. Pinned tabs used to be skipped
        // here entirely - only `unpinned_tab_order` was consulted - so closing one dropped it
        // from `self.tabs` without ever emitting `TabCommand::Close`, leaving its chip and its
        // stack page orphaned in the UI.
        let from_pinned = self.pinned_tab_order.iter().position(|id| id == &tab_id);
        let from_unpinned = self.unpinned_tab_order.iter().position(|id| id == &tab_id);

        let next_active = match (from_pinned, from_unpinned) {
            (Some(index), _) => {
                self.pinned_tab_order.remove(index);
                Self::neighbour_of(&self.pinned_tab_order, index).or_else(|| self.unpinned_tab_order.front().copied())
            }
            (None, Some(index)) => {
                self.unpinned_tab_order.remove(index);
                Self::neighbour_of(&self.unpinned_tab_order, index).or_else(|| self.pinned_tab_order.back().copied())
            }
            // In neither order list: there is no chip or page to tear down.
            (None, None) => {
                self.tabs.remove(&tab_id);
                return;
            }
        };

        self.commands.push(TabCommand::Close(tab_id));
        if let Some(new_active_tab) = next_active {
            self.set_active(new_active_tab);
        }

        self.tabs.remove(&tab_id);
    }

    pub fn get_tab(&self, tab_id: TabId) -> Option<GosubTab> {
        if let Some(tab) = self.tabs.get(&tab_id) {
            return Some(tab.clone());
        }
        None
    }

    pub fn order(&self) -> Vec<TabId> {
        let mut order = Vec::with_capacity(self.pinned_tab_order.len() + self.unpinned_tab_order.len());
        order.extend_from_slice(&self.pinned_tab_order.iter().cloned().collect::<Vec<TabId>>());
        order.extend_from_slice(&self.unpinned_tab_order.iter().cloned().collect::<Vec<TabId>>());

        order
    }

    /// The tabs that "Close Tabs to Left" should close: every unpinned tab sitting before
    /// `tab_id` in the visible order. Pinned tabs are always leftmost (see `order`), so they
    /// are skipped rather than closed.
    pub fn closable_tabs_left_of(&self, tab_id: TabId) -> Vec<TabId> {
        let mut tabs_to_close = Vec::new();

        for id in self.order() {
            // Our tab is found, so everything after it is to the right.
            if id == tab_id {
                break;
            }
            // Pinned tab, we cannot close it.
            match self.tabs.get(&id) {
                Some(tab) if !tab.is_pinned() => tabs_to_close.push(id),
                _ => continue,
            }
        }

        tabs_to_close
    }

    /// Move `tab_id` to `position`, an index into the visible strip order (see [`Self::order`]).
    ///
    /// Reordering never moves a tab across the pinned boundary: pinned tabs reorder among the
    /// pinned, unpinned among the unpinned, and a target past either end clamps to the nearest
    /// slot in the tab's own list. List membership changes only via [`Self::pin_tab`] /
    /// [`Self::unpin_tab`], so `is_pinned` and membership can never disagree.
    pub fn reorder(&mut self, tab_id: TabId, position: usize) {
        // Unknown tab: nothing to reorder. This used to `.unwrap()`, which panics on a stale
        // id - the same failure that aborted the app from the Insert/Update handlers, since a
        // command batch can name a tab that an earlier command in the batch already closed.
        let pinned = match self.tabs.get(&tab_id) {
            Some(tab) => tab.is_pinned(),
            None => return,
        };
        let pinned_len = self.pinned_tab_order.len();

        let list = if pinned {
            &mut self.pinned_tab_order
        } else {
            &mut self.unpinned_tab_order
        };
        let Some(from) = list.iter().position(|id| id == &tab_id) else {
            // `is_pinned` disagrees with list membership. Only reachable if something set the
            // flag directly instead of going through pin_tab/unpin_tab.
            return;
        };

        // `position` indexes the concatenated strip (pinned first, then unpinned), which is what
        // the view resolves TabCommand::Move against. Translate it into this list, clamping
        // rather than letting a drag past the boundary change the tab's pinned state.
        let local_target = if pinned {
            position.min(list.len() - 1)
        } else {
            position.saturating_sub(pinned_len).min(list.len() - 1)
        };
        if local_target == from {
            return;
        }

        list.remove(from);
        list.insert(local_target, tab_id);

        // Report the resting place as a strip index, matching what the view expects.
        let global = if pinned { local_target } else { pinned_len + local_target };
        self.commands.push(TabCommand::Move(tab_id, global as u32));
    }
}

#[cfg(test)]
mod test {
    use super::{GosubTab, GosubTabManager, TabCommand, TabId};
    use url::Url;

    #[test]
    fn test_tab_id() {
        use std::str::FromStr;

        let id = TabId::new();
        let id_str = id.to_string();
        let id_parsed = TabId::from_str(&id_str).unwrap();

        assert_eq!(id, id_parsed);
    }

    #[test]
    fn test_tab_manager() {
        let mut manager = GosubTabManager::new();
        let tab = GosubTab::new(Url::parse("about:blank").unwrap(), "New tab");
        let tab_id = manager.add_tab(tab, None);

        assert_eq!(manager.tab_count(), 1);
        assert_eq!(manager.get_tab(tab_id).unwrap().url().as_str(), "about:blank");
        assert_eq!(manager.get_tab(tab_id).unwrap().title(), "New tab");

        manager.remove_tab(tab_id);
        assert_eq!(manager.tab_count(), 0);
    }

    #[test]
    fn test_tab_manager_remove() {
        let mut manager = GosubTabManager::new();
        let tab1 = GosubTab::new(Url::parse("about:blank").unwrap(), "New tab 1");
        let tab2 = GosubTab::new(Url::parse("about:blank").unwrap(), "New tab 2");
        let tab3 = GosubTab::new(Url::parse("about:blank").unwrap(), "New tab 3");

        let tab1_id = manager.add_tab(tab1, None);
        let tab2_id = manager.add_tab(tab2, None);
        let tab3_id = manager.add_tab(tab3, None);

        assert_eq!(manager.tab_count(), 3);

        manager.remove_tab(tab2_id);
        assert_eq!(manager.tab_count(), 2);
        assert_eq!(manager.order(), vec![tab1_id, tab3_id]);
    }

    /// Regression: closing a PINNED tab must emit `TabCommand::Close` and leave the order
    /// lists consistent. It used to be dropped from `tabs` with no command at all, so the UI
    /// kept its chip and its stack page forever.
    #[test]
    fn removing_a_pinned_tab_emits_close() {
        let mut manager = GosubTabManager::new();
        let mut pinned = GosubTab::new(Url::parse("about:blank").unwrap(), "pinned");
        pinned.set_pinned(true);
        let plain = GosubTab::new(Url::parse("about:blank").unwrap(), "plain");

        let pinned_id = manager.add_tab(pinned, None);
        let plain_id = manager.add_tab(plain, None);
        assert_eq!(manager.tab_count(), 2);

        let _ = manager.commands(); // drain setup commands
        manager.remove_tab(pinned_id);

        assert_eq!(manager.tab_count(), 1, "the pinned tab must be gone");
        assert!(
            manager
                .commands()
                .iter()
                .any(|c| matches!(c, TabCommand::Close(id) if *id == pinned_id)),
            "closing a pinned tab must emit TabCommand::Close so the UI tears down its chip and page"
        );
        assert_eq!(manager.order(), vec![plain_id]);
        assert!(manager.get_tab(pinned_id).is_none());
    }

    /// Regression: reordering an unknown/stale tab id must not panic.
    /// Fixture: strip order is [P1 P2 U1 U2 U3], pinned tabs first (see `order`).
    fn strip() -> (GosubTabManager, Vec<TabId>) {
        let mut m = GosubTabManager::new();
        let mut ids = Vec::new();
        for (n, pinned) in [("P1", true), ("P2", true), ("U1", false), ("U2", false), ("U3", false)] {
            let mut tab = GosubTab::new(Url::parse("about:blank").unwrap(), n);
            tab.set_pinned(pinned);
            ids.push(m.add_tab(tab, None));
        }
        (m, ids)
    }

    /// The emitted `Insert` position must be a STRIP index, not an index into the tab's own
    /// list. With pinned tabs present the two differ, and the view applies it to the strip.
    #[test]
    fn add_tab_emits_a_strip_index_not_a_list_index() {
        let (mut m, _ids) = strip(); // [P1 P2 U1 U2 U3]
        let _ = m.commands(); // drain setup commands

        let new = GosubTab::new(Url::parse("about:blank").unwrap(), "new");
        let new_id = m.add_tab(new, None);

        // Appended at the end of the strip: 2 pinned + 3 unpinned => index 5.
        assert_eq!(m.order().last(), Some(&new_id), "new tab should be last in the strip");
        let pos = m.commands().iter().find_map(|c| match c {
            TabCommand::Insert(id, p) if *id == new_id => Some(*p),
            _ => None,
        });
        assert_eq!(pos, Some(5), "emitted position must be the strip index, not the unpinned index (2)");
        assert_eq!(m.order().iter().position(|id| *id == new_id), Some(5));
    }

    /// A caller-supplied position is a strip index too (the shell derives it from the tab bar),
    /// so it must be translated into the target list rather than used as a local index.
    #[test]
    fn add_tab_accepts_a_strip_index_for_its_position() {
        let (mut m, ids) = strip(); // [P1 P2 U1 U2 U3]
        let (p1, p2, u1) = (ids[0], ids[1], ids[2]);

        // Strip index 3 is "just after U1" — i.e. the second unpinned slot.
        let new = GosubTab::new(Url::parse("about:blank").unwrap(), "new");
        let new_id = m.add_tab(new, Some(3));

        assert_eq!(m.order()[0], p1);
        assert_eq!(m.order()[1], p2);
        assert_eq!(m.order()[2], u1);
        assert_eq!(m.order()[3], new_id, "should land at the requested strip position");
        assert!(!m.get_tab(new_id).unwrap().is_pinned());
    }

    /// A strip index pointing into the pinned run clamps to the first unpinned slot rather
    /// than making the tab pinned — the same boundary rule `reorder` follows.
    #[test]
    fn add_tab_clamps_a_position_inside_the_pinned_run() {
        let (mut m, ids) = strip(); // [P1 P2 U1 U2 U3]
        let (p1, p2) = (ids[0], ids[1]);

        let new = GosubTab::new(Url::parse("about:blank").unwrap(), "new");
        let new_id = m.add_tab(new, Some(0)); // would be among the pinned tabs

        assert_eq!(m.order()[0], p1, "pinned tabs keep the front of the strip");
        assert_eq!(m.order()[1], p2);
        assert_eq!(m.order()[2], new_id, "clamped to the first unpinned slot");
        assert!(!m.get_tab(new_id).unwrap().is_pinned());
    }

    #[test]
    fn reorder_moves_within_the_unpinned_list() {
        let (mut m, ids) = strip();
        let (p1, p2, u1, u2, u3) = (ids[0], ids[1], ids[2], ids[3], ids[4]);
        assert_eq!(m.order(), vec![p1, p2, u1, u2, u3]);

        // Strip index 4 is U3's slot: move U1 to the end of the unpinned run.
        m.reorder(u1, 4);
        assert_eq!(m.order(), vec![p1, p2, u2, u3, u1]);
    }

    #[test]
    fn reorder_clamps_at_the_pinned_boundary_instead_of_pinning() {
        let (mut m, ids) = strip();
        let (p1, p2, u1, u2, u3) = (ids[0], ids[1], ids[2], ids[3], ids[4]);

        // Drag U3 to the very front of the strip. It must stop at the first unpinned slot,
        // not jump into the pinned run and not become pinned.
        m.reorder(u3, 0);
        assert_eq!(m.order(), vec![p1, p2, u3, u1, u2]);
        assert!(!m.get_tab(u3).unwrap().is_pinned());

        // Symmetrically, a pinned tab dragged past the end stays inside the pinned run.
        m.reorder(p1, 4);
        assert_eq!(m.order(), vec![p2, p1, u3, u1, u2]);
        assert!(m.get_tab(p1).unwrap().is_pinned());
    }

    #[test]
    fn reorder_to_the_current_slot_emits_nothing() {
        let (mut m, ids) = strip();
        let u2 = ids[3];
        let before = m.order();
        let _ = m.commands(); // drain the Insert commands from setup (`commands` takes the queue)
        m.reorder(u2, 3); // U2 is already at strip index 3
        assert_eq!(m.order(), before);
        assert!(m.commands().is_empty(), "a no-op reorder must not emit a Move");
    }

    #[test]
    fn reorder_of_an_unknown_tab_is_a_noop() {
        let mut manager = GosubTabManager::new();
        let tab = GosubTab::new(Url::parse("about:blank").unwrap(), "only");
        let tab_id = manager.add_tab(tab, None);

        // An id that was never added, and one that has just been removed.
        manager.reorder(TabId::new(), 0);
        manager.remove_tab(tab_id);
        manager.reorder(tab_id, 0);

        assert_eq!(manager.tab_count(), 0);
    }

    #[test]
    fn test_pinned_tabs() {
        let mut manager = GosubTabManager::new();
        let tab1 = GosubTab::new(Url::parse("about:blank").unwrap(), "New tab 1");
        let tab2 = GosubTab::new(Url::parse("about:blank").unwrap(), "New tab 2");
        let mut tab3 = GosubTab::new(Url::parse("about:blank").unwrap(), "New tab 3");
        tab3.set_pinned(true);
        let tab4 = GosubTab::new(Url::parse("about:blank").unwrap(), "New tab 4");
        let mut tab5 = GosubTab::new(Url::parse("about:blank").unwrap(), "New tab 5");
        tab5.set_pinned(true);
        let tab6 = GosubTab::new(Url::parse("about:blank").unwrap(), "New tab 6");

        let tab1_id = manager.add_tab(tab1, None);
        let tab2_id = manager.add_tab(tab2, None);
        let tab3_id = manager.add_tab(tab3, None);
        let tab4_id = manager.add_tab(tab4, None);
        let tab5_id = manager.add_tab(tab5, None);
        let tab6_id = manager.add_tab(tab6, None);

        // Since some tabs are pinned, this is the ordering:
        // [ 3 5 1 2 4 6 ]
        assert_eq!(manager.pinned_tab_order, vec![tab3_id, tab5_id]);
        assert_eq!(manager.unpinned_tab_order, vec![tab1_id, tab2_id, tab4_id, tab6_id]);

        assert!(manager.is_most_left_unpinned_tab(tab1_id));
        assert!(!manager.is_most_left_unpinned_tab(tab2_id));
        assert!(manager.is_most_right_tab(tab6_id));
        assert!(!manager.is_most_right_tab(tab5_id));
    }

    #[test]
    fn test_closable_tabs_left_of_skips_pinned_tabs() {
        let mut manager = GosubTabManager::new();
        let tab1 = GosubTab::new(Url::parse("about:blank").unwrap(), "New tab 1");
        let tab2 = GosubTab::new(Url::parse("about:blank").unwrap(), "New tab 2");
        let mut tab3 = GosubTab::new(Url::parse("about:blank").unwrap(), "New tab 3");
        tab3.set_pinned(true);
        let tab4 = GosubTab::new(Url::parse("about:blank").unwrap(), "New tab 4");
        let mut tab5 = GosubTab::new(Url::parse("about:blank").unwrap(), "New tab 5");
        tab5.set_pinned(true);
        let tab6 = GosubTab::new(Url::parse("about:blank").unwrap(), "New tab 6");

        let tab1_id = manager.add_tab(tab1, None);
        let tab2_id = manager.add_tab(tab2, None);
        let tab3_id = manager.add_tab(tab3, None);
        let tab4_id = manager.add_tab(tab4, None);
        let tab5_id = manager.add_tab(tab5, None);
        let tab6_id = manager.add_tab(tab6, None);

        // Visible ordering is [ 3 5 1 2 4 6 ], with 3 and 5 pinned.
        assert_eq!(manager.order(), vec![tab3_id, tab5_id, tab1_id, tab2_id, tab4_id, tab6_id]);

        // Only the unpinned tabs to the left are closed; the pinned ones are spared.
        assert_eq!(manager.closable_tabs_left_of(tab4_id), vec![tab1_id, tab2_id]);
        assert_eq!(manager.closable_tabs_left_of(tab6_id), vec![tab1_id, tab2_id, tab4_id]);

        // Leftmost unpinned tab has only pinned tabs before it, so nothing is closable.
        assert!(manager.closable_tabs_left_of(tab1_id).is_empty());

        // The tab itself is never included.
        assert!(!manager.closable_tabs_left_of(tab4_id).contains(&tab4_id));
    }
}
