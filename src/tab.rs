use crate::engine::EngineTabId;
use crate::fetcher::address_parser::GosubRenderMode;
use gosub_engine::tab::TabHandle;
use gtk4::gdk::Texture;
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
    render_mode: GosubRenderMode,
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
    /// Title of the tab
    title: String,
    /// Loaded favicon of the tab
    favicon: Option<Texture>,
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
            render_mode: GosubRenderMode::Rendered,
            loading: false,
            id: TabId::new(),
            pinned: false,
            private: false,
            url,
            history: History::default(),
            title: title.to_string(),
            favicon: None,
            content: String::new(),
            tab_handle: None,
        }
    }

    pub(crate) fn set_render_mode(&mut self, mode: GosubRenderMode) {
        self.render_mode = mode;
    }
    pub(crate) fn render_mode(&self) -> GosubRenderMode {
        self.render_mode.clone()
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

    pub(crate) fn favicon(&self) -> Option<Texture> {
        self.favicon.clone()
    }

    pub fn set_favicon(&mut self, favicon: Option<Texture>) {
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
    pub(crate) fn get_by_tab(&self, tab_id: TabId) -> Option<&GosubTab> {
        self.tabs.get(&tab_id)
    }

    pub(crate) fn commands(&mut self) -> Vec<TabCommand> {
        self.commands.drain(..).collect()
    }

    pub(crate) fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    /// Returns true when the given tab is the leftmost unpinned tab
    pub(crate) fn is_most_left_unpinned_tab(&self, tab_id: TabId) -> bool {
        self.unpinned_tab_order.front() == Some(&tab_id)
    }

    /// Returns true when the given tab is the rightmost tab
    pub(crate) fn is_most_right_tab(&self, tab_id: TabId) -> bool {
        self.unpinned_tab_order.back() == Some(&tab_id)
    }

    pub fn set_active(&mut self, tab_id: TabId) {
        self.commands.push(TabCommand::Activate(tab_id));
    }

    pub(crate) fn notify_tab_changed(&mut self, tab_id: TabId) {
        self.commands.push(TabCommand::Update(tab_id));
    }

    pub(crate) fn update_tab(&mut self, tab_id: TabId, tab: &GosubTab) {
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

    pub fn add_tab(&mut self, tab: GosubTab, position: Option<usize>) -> TabId {
        let mut real_position = position.unwrap_or(usize::MAX);

        if tab.is_pinned() {
            if real_position > self.pinned_tab_order.len() {
                self.pinned_tab_order.push_back(tab.id());
                real_position = self.pinned_tab_order.len() - 1;
            } else {
                self.pinned_tab_order.insert(real_position, tab.id());
            }
        } else if real_position > self.unpinned_tab_order.len() {
            self.unpinned_tab_order.push_back(tab.id());
            real_position = self.unpinned_tab_order.len() - 1;
        } else {
            self.unpinned_tab_order.insert(real_position, tab.id());
        }

        self.commands.push(TabCommand::Insert(tab.id(), real_position as u32));

        let tab_id = tab.id;
        self.tabs.insert(tab_id, tab);
        // self.set_active(tab_id);

        tab_id
    }

    pub fn remove_tab(&mut self, tab_id: TabId) {
        if let Some(index) = self.unpinned_tab_order.iter().position(|id| id == &tab_id) {
            self.unpinned_tab_order.remove(index);
            self.commands.push(TabCommand::Close(tab_id));

            // Set active tab to the last tab. Assumes there is always one tab
            if index == 0 {
                if let Some(new_active_tab) = self.unpinned_tab_order.front() {
                    self.set_active(*new_active_tab);
                }
            } else if let Some(new_active_tab) = self.unpinned_tab_order.get(index - 1) {
                self.set_active(*new_active_tab);
            }
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

    pub fn reorder(&mut self, tab_id: TabId, position: usize) {
        let tab = self.tabs.get(&tab_id).unwrap();

        if tab.is_pinned() {
            if let Some(index) = self.unpinned_tab_order.iter().position(|id| id == &tab_id) {
                match index.cmp(&position) {
                    std::cmp::Ordering::Equal => {}
                    std::cmp::Ordering::Less => {
                        self.unpinned_tab_order.remove(index);
                        self.pinned_tab_order.push_back(tab_id);
                    }
                    std::cmp::Ordering::Greater => {
                        self.unpinned_tab_order.remove(index);
                        self.pinned_tab_order.push_front(tab_id);
                    }
                }
                self.commands.push(TabCommand::Move(tab_id, position as u32));
            }
        } else if let Some(index) = self.pinned_tab_order.iter().position(|id| id == &tab_id) {
            match index.cmp(&position) {
                std::cmp::Ordering::Equal => {}
                std::cmp::Ordering::Less => {
                    self.pinned_tab_order.remove(index);
                    self.pinned_tab_order.insert(position, tab_id);
                }
                std::cmp::Ordering::Greater => {
                    self.pinned_tab_order.remove(index);
                    self.pinned_tab_order.insert(position, tab_id);
                }
            }
            self.commands.push(TabCommand::Move(tab_id, position as u32));
        }
    }
}

#[cfg(test)]
mod test {
    use super::{GosubTab, GosubTabManager, TabId};
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
}
