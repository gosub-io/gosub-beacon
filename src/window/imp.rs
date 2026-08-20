use crate::engine::{render_frame_gl, BrowserEngine, EngineTabId};
use crate::fetcher::address_parser::GosubAddressParser;
use crate::tab::{GosubTab, GosubTabManager, HistoryEntryId, TabCommand, TabId};
use crate::window::message::Message;
use crate::window::tab_context_menu::{build_context_menu, setup_context_menu_actions, TabInfo};
use crate::{fetcher, runtime};
use async_channel::{Receiver, Sender};
use glib::subclass::InitializingObject;
use gosub_engine::events::{EngineEvent, NavigationEvent, TabCommand as EngineTabCommand};
use gtk4::gio::SimpleActionGroup;
use gtk4::glib::subclass::Signal;
use gtk4::glib::Quark;
use gtk4::graphene::Point;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{
    gdk, glib, Button, CompositeTemplate, Entry, GLArea, GestureClick, Image, Popover, PopoverMenu, PopoverMenuFlags, ScrolledWindow,
    Settings, Stack, TemplateChild, TextView, ToggleButton, Widget,
};
use log::info;
use once_cell::sync::Lazy;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;

// Create a static Quark as a unique key
static TAB_ID_QUARK: Lazy<Quark> = Lazy::new(|| Quark::from_str("tab_id"));

pub trait WidgetExtTabId {
    fn set_tab_id(&self, tab_id: TabId);
    fn get_tab_id(&self) -> Option<TabId>;
}

impl<T: IsA<Widget>> WidgetExtTabId for T {
    fn set_tab_id(&self, tab_id: TabId) {
        unsafe {
            // - 'tab_id' is of type 'TabId', which is 'Copy' and 'static'.
            // - We ensure that the same type is used when retrieving the data.
            self.set_qdata(*TAB_ID_QUARK, tab_id);
        }
    }

    fn get_tab_id(&self) -> Option<TabId> {
        unsafe { self.qdata::<TabId>(*TAB_ID_QUARK).map(|ptr| *ptr.as_ref()) }
    }
}

/// One download this session, as shown in the downloads popover.
pub struct DownloadEntry {
    pub id: u64,
    pub filename: String,
    pub path: std::path::PathBuf,
    pub received: u64,
    pub total: Option<u64>,
    pub state: DownloadState,
}

#[derive(PartialEq)]
pub enum DownloadState {
    Running,
    Finished,
    Failed(String),
}

#[derive(CompositeTemplate)]
#[template(resource = "/io/gosub/beacon/ui/window.ui")]
pub struct BrowserWindow {
    #[template_child]
    pub searchbar: TemplateChild<Entry>,
    #[template_child]
    pub btn_prev: TemplateChild<Button>,
    #[template_child]
    pub btn_next: TemplateChild<Button>,
    #[template_child]
    pub btn_refresh: TemplateChild<Button>,
    #[template_child]
    pub headerbar: TemplateChild<gtk4::HeaderBar>,
    #[template_child]
    pub tab_strip: TemplateChild<gtk4::Box>,
    #[template_child]
    pub content_stack: TemplateChild<Stack>,
    #[template_child]
    pub log_scroller: TemplateChild<ScrolledWindow>,
    #[template_child]
    pub log: TemplateChild<TextView>,
    #[template_child]
    pub statusbar: TemplateChild<gtk4::Label>,
    #[template_child]
    pub btn_downloads: TemplateChild<gtk4::MenuButton>,
    #[template_child]
    pub btn_bookmark: TemplateChild<Button>,
    #[template_child]
    pub bookmark_icon: TemplateChild<Image>,
    #[template_child]
    pub bookmarks_bar: TemplateChild<gtk4::Box>,

    // Other stuff that are non-widgets
    pub tab_manager: Arc<Mutex<GosubTabManager>>,
    pub sender: Arc<Sender<Message>>,
    pub receiver: Arc<Receiver<Message>>,

    /// The running engine (created in `init_engine`). Main-thread only.
    pub engine: Rc<RefCell<Option<BrowserEngine>>>,
    /// Per-tab GL areas, so the redraw loop can request repaints.
    pub render_areas: Rc<RefCell<HashMap<TabId, GLArea>>>,
    /// Last viewport (CSS px) a GLArea resize computed, in exactly the form the engine expects.
    /// A hidden `GtkStack` page is never allocated, so a background tab's own GLArea never
    /// emits `::resize`; without this, such a tab would be sized by a second, different
    /// formula (`content_stack` logical px) and every switch to it would land a differing
    /// `SetViewport`, which drops the whole tile cache and re-lays-out the page.
    pub last_viewport: Rc<Cell<Option<(u32, u32)>>>,
    /// Maps engine tab ids back to our tab ids (for routing engine events).
    pub engine_tab_map: Rc<RefCell<HashMap<EngineTabId, TabId>>>,
    /// Right-clicks awaiting the engine's hit-test answer: token → (tab, window point).
    pub pending_hit_tests: RefCell<HashMap<u64, (TabId, Point)>>,
    /// Source of hit-test tokens.
    pub next_hit_test_token: Cell<u64>,
    /// Source of download ids.
    pub next_download_id: Cell<u64>,
    /// Session downloads, newest last; rendered into the downloads popover.
    pub downloads: RefCell<Vec<DownloadEntry>>,
    /// List widget inside the downloads popover (built in `constructed`).
    downloads_list: RefCell<Option<gtk4::ListBox>>,
    /// URL-bar completion popover and its list (built in `constructed`).
    completion: RefCell<Option<(Popover, gtk4::ListBox)>>,
    /// Per-tab page zoom, shared with each render area's input/draw closures.
    tab_zoom: RefCell<HashMap<TabId, Rc<Cell<f64>>>>,
}

impl Default for BrowserWindow {
    fn default() -> Self {
        let (tx, rx) = async_channel::unbounded::<Message>();
        Self {
            searchbar: TemplateChild::default(),
            btn_prev: TemplateChild::default(),
            btn_next: TemplateChild::default(),
            btn_refresh: TemplateChild::default(),
            headerbar: TemplateChild::default(),
            tab_strip: TemplateChild::default(),
            content_stack: TemplateChild::default(),
            log_scroller: TemplateChild::default(),
            log: TemplateChild::default(),
            statusbar: TemplateChild::default(),
            btn_downloads: TemplateChild::default(),
            btn_bookmark: TemplateChild::default(),
            bookmark_icon: TemplateChild::default(),
            bookmarks_bar: TemplateChild::default(),

            tab_manager: Arc::new(Mutex::new(GosubTabManager::new())),
            sender: Arc::new(tx),
            receiver: Arc::new(rx),

            engine: Rc::new(RefCell::new(None)),
            render_areas: Rc::new(RefCell::new(HashMap::new())),
            last_viewport: Rc::new(Cell::new(None)),
            engine_tab_map: Rc::new(RefCell::new(HashMap::new())),
            pending_hit_tests: RefCell::new(HashMap::new()),
            next_hit_test_token: Cell::new(1),
            next_download_id: Cell::new(1),
            downloads: RefCell::new(Vec::new()),
            downloads_list: RefCell::new(None),
            completion: RefCell::new(None),
            tab_zoom: RefCell::new(HashMap::new()),
        }
    }
}

impl BrowserWindow {
    pub(crate) fn get_sender(&self) -> Arc<Sender<Message>> {
        self.sender.clone()
    }

    pub(crate) fn get_receiver(&self) -> Arc<Receiver<Message>> {
        self.receiver.clone()
    }
}

#[glib::object_subclass]
impl ObjectSubclass for BrowserWindow {
    const NAME: &'static str = "BrowserWindow";
    type Type = super::BrowserWindow;
    type ParentType = gtk4::ApplicationWindow;

    fn class_init(klass: &mut Self::Class) {
        klass.bind_template();
        klass.bind_template_callbacks();
    }

    fn instance_init(obj: &InitializingObject<Self>) {
        obj.init_template();
    }
}

impl ObjectImpl for BrowserWindow {
    fn signals() -> &'static [Signal] {
        static SIGNALS: Lazy<Vec<Signal>> = Lazy::new(|| vec![Signal::builder("update-tabs").build()]);

        SIGNALS.as_ref()
    }

    fn constructed(&self) {
        self.parent_constructed();
        self.setup_downloads_popover();
        self.setup_bookmark_button();
        self.setup_url_completion();
        self.log("Browser created...");
    }
}

impl WidgetImpl for BrowserWindow {}
impl WindowImpl for BrowserWindow {}
impl ApplicationWindowImpl for BrowserWindow {}

#[gtk4::template_callbacks]
impl BrowserWindow {
    #[template_callback]
    fn handle_prev_clicked(&self, _btn: &Button) {
        self.navigate_back();
    }

    #[template_callback]
    fn handle_next_clicked(&self, btn: &Button) {
        self.navigate_forward(btn);
    }

    #[template_callback]
    fn handle_view_source_clicked(&self, _btn: &Button) {
        let Some(tab_id) = self.active_tab_id() else {
            self.log("No active tab to view source for");
            return;
        };
        let url = {
            let manager = self.tab_manager.lock().unwrap();
            match manager.get_tab(tab_id) {
                Some(tab) => tab.url().clone(),
                None => return,
            }
        };

        // The engine does not expose the page source, so re-fetch the URL.
        let sender = self.get_sender();
        runtime().spawn(async move {
            match fetcher::fetch_url_body(url.clone()).await {
                Ok(bytes) => {
                    let content = String::from_utf8_lossy(&bytes).to_string();
                    let _ = sender.send(Message::ShowSource(url.to_string(), content)).await;
                }
                Err(e) => {
                    let _ = sender.send(Message::Log(format!("View source failed: {e}"))).await;
                }
            }
        });
    }

    #[template_callback]
    fn handle_toggle_darkmode(&self, btn: &ToggleButton) {
        self.log("Toggling dark mode");

        info!(target: "gtk", "Toggle dark mode action triggered");
        let settings = Settings::default().expect("Failed to get default GtkSettings");
        settings.set_property("gtk-application-prefer-dark-theme", btn.is_active());
    }

    /// Reload the active tab — or, while it is loading, stop it (the button
    /// doubles as a stop button; see `update_reload_button`).
    #[template_callback]
    fn handle_refresh_clicked(&self, _btn: &Button) {
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        self.reload_or_stop(tab_id);
    }

    /// Reload `tab_id` (a true `TabCommand::Reload`: keeps the history entry, restores scroll),
    /// or stop it if it is still loading. Shell-rendered internal pages have nothing to
    /// reload; engine-rendered ones get their HTML pushed again.
    pub(crate) fn reload_or_stop(&self, tab_id: TabId) {
        let (loading, url, handle) = {
            let manager = self.tab_manager.lock().unwrap();
            match manager.get_tab(tab_id) {
                Some(tab) => (tab.is_loading(), tab.url().clone(), tab.tab_handle()),
                None => return,
            }
        };

        if loading {
            if let Some(handle) = handle {
                runtime().spawn(async move {
                    let _ = handle.send(EngineTabCommand::CancelNavigation).await;
                });
            }
            let mut manager = self.tab_manager.lock().unwrap();
            if let Some(mut tab) = manager.get_tab(tab_id) {
                tab.set_loading(false);
                manager.update_tab(tab_id, &tab);
            }
            drop(manager);
            self.refresh_tabs();
            self.log("Stopped loading");
            return;
        }

        // The shell-rendered config page has nothing to reload.
        if Self::is_shell_rendered(&url) {
            return;
        }

        let mut manager = self.tab_manager.lock().unwrap();
        if let Some(mut tab) = manager.get_tab(tab_id) {
            tab.set_loading(true);
            manager.update_tab(tab_id, &tab);
        }
        drop(manager);
        self.refresh_tabs();

        if let Some(handle) = handle {
            runtime().spawn(async move {
                let _ = handle.send(EngineTabCommand::Reload { ignore_cache: false }).await;
                let _ = handle.send(EngineTabCommand::ResumeDrawing { fps: 30 }).await;
            });
        }
    }

    #[template_callback]
    async fn handle_searchbar_clicked(&self, entry: &Entry) {
        let Some(tab_id) = self.active_tab_id() else {
            self.log("No active tab to load the URL");
            return;
        };
        self.log(format!("Visiting the URL {}", entry.text().as_str()).as_str());
        let url_str = entry.text().to_string();
        self.sender.send(Message::LoadUrl(tab_id, url_str)).await.unwrap();
    }
}

impl BrowserWindow {
    pub fn log(&self, message: &str) {
        let s = format!("[{}] {}\n", chrono::Local::now().format("%X"), message);
        info!(target: "ftk", "Logmessage: {}", s.as_str());

        let buf = self.log.buffer();
        let mut iter = buf.end_iter();
        buf.insert(&mut iter, s.as_str());

        let mark = buf.create_mark(None, &iter, false);
        self.log.scroll_to_mark(&mark, 0.0, true, 0.0, 1.0);
    }

    pub(crate) fn close_tab(&self, tab_id: TabId) {
        let mut manager = self.tab_manager.lock().unwrap();
        if manager.tab_count() == 1 {
            self.log("Cannot close the last tab");
            return;
        }
        manager.remove_tab(tab_id);
    }

    pub(crate) fn refresh_tabs(&self) {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

        rt.block_on(self.refresh_tabs_async())
    }

    /// Refresh tabs will asynchronously update the tab bar based on the current state of the tab
    /// manager. Any mutations that are done on tabs in the manager, are recorded as commands and
    /// played back here.
    async fn refresh_tabs_async(&self) {
        let mut manager = self.tab_manager.lock().unwrap();
        let commands = manager.commands();
        drop(manager);

        for cmd in commands {
            match cmd {
                TabCommand::Activate(tab_id) => {
                    self.activate_tab(tab_id);
                }
                TabCommand::Insert(tab_id, position) => {
                    // Commands are drained in one batch, so this can name a tab that a later
                    // command in the SAME batch already closed (`remove_tab` drops it from
                    // `tabs` immediately). Unwrapping here panicked while holding the lock,
                    // which poisoned it and turned every later click into an abort.
                    let manager = self.tab_manager.lock().unwrap();
                    let tab = manager.get_tab(tab_id);
                    drop(manager);
                    let Some(tab) = tab else {
                        continue;
                    };

                    let chip = self.create_tab_chip(&tab);
                    let sibling = if position == 0 {
                        None
                    } else {
                        self.chips().get(position as usize - 1).cloned()
                    };
                    self.tab_strip.insert_child_after(&chip, sibling.as_ref());

                    let page = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
                    page.append(&self.generate_default_page());
                    page.set_tab_id(tab.id());
                    self.content_stack.add_child(&page);

                    // The stack shows its first child automatically; mirror that on the chip.
                    if self.content_stack.visible_child().as_ref() == Some(page.upcast_ref()) {
                        Self::set_chip_active(&chip, true);
                    }
                }
                TabCommand::Close(tab_id) => {
                    if let Some(chip) = self.chip_for_tab(tab_id) {
                        self.tab_strip.remove(&chip);
                    }
                    if let Some(page) = self.page_for_tab(tab_id) {
                        self.content_stack.remove(&page);
                    }
                    self.render_areas.borrow_mut().remove(&tab_id);
                }
                TabCommand::CloseAll => {
                    // Only the chips: the new-tab button is also a child of the strip.
                    for chip in self.chips() {
                        self.tab_strip.remove(&chip);
                    }
                    while let Some(page) = self.content_stack.first_child() {
                        self.content_stack.remove(&page);
                    }
                    self.render_areas.borrow_mut().clear();
                }
                TabCommand::Move(tab_id, position) => {
                    if let Some(chip) = self.chip_for_tab(tab_id) {
                        let sibling = if position == 0 {
                            None
                        } else {
                            self.chips().into_iter().filter(|c| *c != chip).nth(position as usize - 1)
                        };
                        self.tab_strip.reorder_child_after(&chip, sibling.as_ref());
                    }
                }
                TabCommand::Update(tab_id) => {
                    // Same as Insert: the tab may already be gone by the time we get here.
                    let manager = self.tab_manager.lock().unwrap();
                    let tab = manager.get_tab(tab_id);
                    drop(manager);
                    let Some(tab) = tab else {
                        continue;
                    };

                    // The shell-rendered config page gets a GTK widget; everything else
                    // (real URLs and engine-served gosub:// pages) gets an engine-backed
                    // render area, or the splash page.
                    let child: Widget = if tab.crashed().is_some() {
                        self.build_crashed_page(&tab)
                    } else if Self::is_shell_rendered(tab.url()) {
                        self.build_shell_page()
                    } else if tab.has_engine_tab() {
                        self.build_render_area(&tab).upcast::<Widget>()
                    } else {
                        self.generate_default_page().upcast::<Widget>()
                    };

                    // The stack page is a box wrapper; swap only its inner child so the
                    // page itself (and the visible-child state) stays put.
                    if let Some(page) = self.page_for_tab(tab_id) {
                        let page_box = page.downcast_ref::<gtk4::Box>().unwrap();
                        if let Some(old) = page_box.first_child() {
                            page_box.remove(&old);
                        }
                        page_box.append(&child);
                    }

                    if let Some(chip) = self.chip_for_tab(tab_id) {
                        self.populate_chip(&chip, &tab);
                    }
                }
            }
        }

        // Loading state may have changed for the active tab.
        self.update_reload_button();
    }

    /// Swap the reload button between reload and stop based on the active
    /// tab's loading state.
    pub(crate) fn update_reload_button(&self) {
        let loading = self
            .active_tab_id()
            .and_then(|id| self.tab_manager.lock().unwrap().get_tab(id).map(|t| t.is_loading()))
            .unwrap_or(false);
        if loading {
            self.btn_refresh.set_icon_name("process-stop-symbolic");
            self.btn_refresh.set_tooltip_text(Some("Stop loading"));
        } else {
            self.btn_refresh.set_icon_name("view-refresh-symbolic");
            self.btn_refresh.set_tooltip_text(Some("Reload"));
        }
    }

    /// A navigation failed: clear the loading state and show the error page.
    fn on_navigation_failed(&self, our_id: TabId, url: &url::Url, error: &str) {
        // Cancellations (stop button) are not errors.
        if error.to_lowercase().contains("cancel") {
            return;
        }

        let mut manager = self.tab_manager.lock().unwrap();
        if let Some(mut tab) = manager.get_tab(our_id) {
            tab.set_loading(false);
            manager.update_tab(our_id, &tab);
        }
        drop(manager);
        self.refresh_tabs();
        self.load_error_page(our_id, url, error);
    }

    /// Push the branded error page into a tab whose navigation failed.
    fn load_error_page(&self, tab_id: TabId, url: &url::Url, error: &str) {
        fn esc(s: &str) -> String {
            s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
        }
        let html = include_str!("../../resources/error.html")
            .replace("{{URL}}", &esc(url.as_str()))
            .replace("{{ERROR}}", &esc(error));

        let manager = self.tab_manager.lock().unwrap();
        let Some(handle) = manager.get_tab(tab_id).and_then(|t| t.tab_handle()) else {
            return;
        };
        drop(manager);

        let base_url = url.to_string();
        runtime().spawn(async move {
            let _ = handle.send(EngineTabCommand::LoadHtml { html, base_url }).await;
            let _ = handle.send(EngineTabCommand::ResumeDrawing { fps: 30 }).await;
        });
    }

    /// All tab chips in strip order.
    ///
    /// A chip is a `Box`, not a button: the close button has to be a SIBLING of the label
    /// toggle rather than nested inside it. GTK4 does not deliver clicks to a button nested
    /// in another button - the outer one takes them - which is why the close button did
    /// nothing while the chip was itself a `ToggleButton`.
    fn chips(&self) -> Vec<gtk4::Box> {
        let mut out = Vec::new();
        let mut child = self.tab_strip.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            if let Ok(chip) = widget.downcast::<gtk4::Box>() {
                out.push(chip);
            }
        }
        out
    }

    fn chip_for_tab(&self, tab_id: TabId) -> Option<gtk4::Box> {
        self.chips().into_iter().find(|c| c.get_tab_id() == Some(tab_id))
    }

    /// The label toggle inside a chip (its first `ToggleButton` child).
    fn chip_toggle(chip: &gtk4::Box) -> Option<ToggleButton> {
        let mut child = chip.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            if let Ok(toggle) = widget.downcast::<ToggleButton>() {
                return Some(toggle);
            }
        }
        None
    }

    /// Mark a chip selected. The `Box` has no `:checked` state of its own, so the selected
    /// look rides on an `active` class while the inner toggle keeps the real toggle state.
    fn set_chip_active(chip: &gtk4::Box, active: bool) {
        if let Some(toggle) = Self::chip_toggle(chip) {
            toggle.set_active(active);
        }
        if active {
            chip.add_css_class("active");
        } else {
            chip.remove_css_class("active");
        }
    }

    fn page_for_tab(&self, tab_id: TabId) -> Option<Widget> {
        let mut child = self.content_stack.first_child();
        while let Some(widget) = child {
            if widget.get_tab_id() == Some(tab_id) {
                return Some(widget);
            }
            child = widget.next_sibling();
        }
        None
    }

    /// Make `tab_id` the visible tab: check its chip, show its page, and sync
    /// the address bar and nav buttons (the old notebook switch-page handler).
    /// Viewport (CSS px) for a tab whose own GLArea has not been allocated yet.
    ///
    /// Prefers the value a real GLArea `::resize` produced, so every tab is sized by the one
    /// formula the engine will later be told; falls back to the content area's logical size,
    /// and finally to `None`, which lets the engine apply its own non-zero fallback. It must
    /// never report a zero size: that used to reach the engine as a 0x0 viewport.
    fn viewport_for_new_tab(&self) -> Option<(u32, u32)> {
        if let Some(vp) = self.last_viewport.get() {
            return Some(vp);
        }
        let (w, h) = (self.content_stack.width(), self.content_stack.height());
        (w > 0 && h > 0).then_some((w as u32, h as u32))
    }

    /// Push the current viewport to `tab_id` before it is shown.
    ///
    /// A hidden `GtkStack` page is never allocated, so this tab's GLArea has never emitted
    /// `::resize` and the engine may still hold a stale size for it. Sending the size now means
    /// the GLArea's first resize is a no-op in the engine (`vp == desired_viewport`) instead of
    /// a change that drops the tile cache and re-lays-out the whole page mid-switch.
    fn sync_viewport_for(&self, tab_id: TabId) {
        let Some((vw, vh)) = self.viewport_for_new_tab() else {
            return;
        };
        let manager = self.tab_manager.lock().unwrap();
        let handle = manager.get_tab(tab_id).and_then(|t| t.tab_handle());
        drop(manager);
        let Some(handle) = handle else {
            return;
        };
        runtime().spawn(async move {
            let _ = handle
                .send(EngineTabCommand::SetViewport {
                    x: 0,
                    y: 0,
                    width: vw,
                    height: vh,
                })
                .await;
        });
    }

    pub(crate) fn activate_tab(&self, tab_id: TabId) {
        for chip in self.chips() {
            Self::set_chip_active(&chip, chip.get_tab_id() == Some(tab_id));
        }
        // Correct the engine's viewport for this tab BEFORE it is shown, so its GLArea's
        // first-ever resize does not land as a change and trigger a full re-layout.
        self.sync_viewport_for(tab_id);
        if let Some(page) = self.page_for_tab(tab_id) {
            self.content_stack.set_visible_child(&page);
        }

        let manager = self.tab_manager.lock().unwrap();
        if let Some(tab) = manager.get_tab(tab_id) {
            // New-tab pages (blank, home) get an empty address bar, ready to type into.
            let page = gosub_engine::internal_pages::InternalPages::page_name(tab.url());
            if matches!(tab.url().scheme(), "gosub" | "about") && matches!(page, "blank" | "home") {
                self.searchbar.set_text("");
            } else {
                self.searchbar.set_text(tab.url().as_str());
            }
            self.obj().set_title(Some(&format!("{} — Gosub Beacon", tab.title())));
        }
        drop(manager);
        // The raster DPR is a process-wide atomic: re-store it for this tab's zoom.
        {
            let zoom_level = self.tab_zoom.borrow().get(&tab_id).map(|z| z.get()).unwrap_or(1.0);
            if let Some(area) = self.render_areas.borrow().get(&tab_id) {
                use gosub_render_pipeline::render::DEVICE_PIXEL_RATIO;
                let raster_dpr = ((crate::engine::render_dpr(area) as f64 * zoom_level).ceil() as u32).clamp(1, 4);
                DEVICE_PIXEL_RATIO.store(raster_dpr, std::sync::atomic::Ordering::Relaxed);
            }
        }
        self.searchbar.set_progress_fraction(0.0);
        self.update_nav_buttons();
        self.update_reload_button();
        self.update_bookmark_button();
    }

    /// A tab chip: a `Box` holding the label toggle and, beside it, the close button.
    ///
    /// The close button MUST be a sibling of the toggle, never a descendant - see [`Self::chips`].
    fn create_tab_chip(&self, tab: &GosubTab) -> gtk4::Box {
        let chip = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        chip.add_css_class("tab-chip");
        chip.set_tab_id(tab.id());
        self.populate_chip(&chip, tab);
        chip
    }

    /// Fill (or refill) a chip's contents for `tab`, preserving which chip is selected.
    /// Rebuilding wholesale keeps pinned/unpinned transitions correct: pinned tabs have no
    /// close button.
    fn populate_chip(&self, chip: &gtk4::Box, tab: &GosubTab) {
        let was_active = self.active_tab_id() == Some(tab.id());

        while let Some(child) = chip.first_child() {
            chip.remove(&child);
        }

        if tab.is_pinned() {
            chip.add_css_class("pinned");
        } else {
            chip.remove_css_class("pinned");
        }

        let toggle = ToggleButton::new();
        toggle.set_has_frame(false);
        toggle.add_css_class("tab-chip-main");
        toggle.set_child(Some(&self.create_tab_label(tab)));
        toggle.set_tab_id(tab.id());
        let window_clone = self.obj().clone();
        let tab_id = tab.id();
        toggle.connect_clicked(move |_| {
            window_clone.imp().activate_tab(tab_id);
        });
        chip.append(&toggle);

        // Pinned tabs are icon-only and have never had a close button.
        if !tab.is_pinned() {
            chip.append(&self.create_tab_close_button(tab));
        }

        Self::set_chip_active(chip, was_active);
    }

    /// The chip's X button. Lives beside the label toggle, so its clicks actually arrive.
    fn create_tab_close_button(&self, tab: &GosubTab) -> Button {
        let tab_close_button = Button::builder()
            .halign(gtk4::Align::End)
            .valign(gtk4::Align::Center)
            .has_frame(false)
            .margin_bottom(0)
            .margin_end(0)
            .margin_start(0)
            .margin_top(0)
            .build();
        tab_close_button.add_css_class("tab-close");
        let img = Image::from_icon_name("window-close-symbolic");
        img.set_pixel_size(14);
        tab_close_button.set_child(Some(&img));

        let window_clone = self.obj().clone();
        let tab_id = tab.id();
        tab_close_button.connect_clicked(move |_| {
            info!(target: "gtk", "Clicked close button for tab {}", tab_id);
            window_clone.imp().close_tab(tab_id);
            _ = window_clone.imp().get_sender().send_blocking(Message::RefreshTabs());
        });
        tab_close_button
    }

    fn create_pinned_tab_label(&self, tab: &GosubTab) -> Widget {
        if let Some(favicon) = &tab.favicon() {
            let img = Image::from_paintable(Some(&favicon.clone()));
            img.set_margin_top(5);
            img.set_margin_bottom(5);
            return img.into();
        }

        // No favicon for this pinned tab, so fall back to a themed pin. This used to load
        // `/io/gosub/beacon/assets/pin.svg`, which does not exist -- it was never added to
        // resources.gresource.xml and there is no such file -- so pinned tabs without a
        // favicon rendered a broken-image placeholder. A stock symbolic icon needs no asset
        // and recolours with the theme, matching the favicon placeholder above.
        let img = Image::from_icon_name("view-pin-symbolic");
        img.set_pixel_size(16);
        img.add_css_class("dim-label");
        img.set_margin_top(5);
        img.set_margin_bottom(5);
        img.into()
    }

    fn create_normal_tab_label(&self, tab: &GosubTab) -> Widget {
        let label_vbox = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);

        // When the tab is loading, we show a spinner
        if tab.is_loading() {
            let spinner = gtk4::Spinner::new();
            spinner.start();
            label_vbox.append(&spinner);
        } else if let Some(favicon) = &tab.favicon() {
            let img = Image::from_paintable(Some(&favicon.clone()));
            img.set_pixel_size(16);
            label_vbox.append(&img);
        } else {
            // No favicon (yet): a globe placeholder keeps the slot occupied so
            // tabs don't jump when the real icon arrives.
            let img = Image::from_icon_name("web-browser-symbolic");
            img.set_pixel_size(16);
            img.add_css_class("dim-label");
            label_vbox.append(&img);
        }

        // Until the engine reports real page titles, tab titles are URLs; show
        // just the host so tabs read like a browser, not a log file. Ellipsize
        // instead of truncating (byte-truncation panics on multi-byte titles)
        // and keep fixed char bounds so all tabs are the same width. No hexpand:
        // chips must stay content-sized, not divide the strip between them.
        let display_title = url::Url::parse(tab.title())
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_else(|| tab.title().to_string());
        let tab_label = gtk4::Label::new(Some(&display_title));
        tab_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        tab_label.set_width_chars(12);
        tab_label.set_max_width_chars(16);
        tab_label.set_xalign(0.0);
        label_vbox.append(&tab_label);

        // The close button is NOT part of the label: it is appended beside the label toggle
        // by `populate_chip`, because a button nested in a button never receives clicks.
        label_vbox.into()
    }

    /// generates a tab label based on the tab info
    fn create_tab_label(&self, tab: &GosubTab) -> gtk4::Widget {
        let tab_label = match tab.is_pinned() {
            true => self.create_pinned_tab_label(tab),
            false => self.create_normal_tab_label(tab),
        };

        let gesture = GestureClick::builder()
            .button(0) // 0 means all buttons
            .build();

        let window_clone = self.obj().clone();
        let tab_id = tab.id();
        let tab_is_pinned = tab.is_pinned();

        gesture.connect_pressed(move |gesture, _n_press, x, y| {
            if gesture.current_button() == gdk::BUTTON_SECONDARY {
                // Refresh the tab info based on the current state
                let tab_manager = window_clone.imp().tab_manager.lock().unwrap();
                let tab_count = tab_manager.tab_count();
                let tab_info = TabInfo {
                    id: tab_id,
                    is_pinned: tab_is_pinned,
                    is_left: tab_manager.is_most_left_unpinned_tab(tab_id),
                    is_right: tab_manager.is_most_right_tab(tab_id),
                    tab_count,
                };
                drop(tab_manager);

                let menu_model = build_context_menu(tab_info.clone());
                let popover = PopoverMenu::builder()
                    .menu_model(&menu_model)
                    .halign(gtk4::Align::Start)
                    .has_arrow(false)
                    .flags(PopoverMenuFlags::NESTED)
                    .build();

                let action_group = SimpleActionGroup::new();
                setup_context_menu_actions(&action_group, &window_clone, tab_info.clone());
                popover.insert_action_group("tab", Some(&action_group));

                if let Some(widget) = gesture.widget() {
                    // We need to use the window as a parent, not the parent widget. Since X/Y coordinates
                    // are relative from the widget, we need to convert them X/Y positions based on the window.
                    popover.set_parent(&window_clone);
                    if let Some(p) = widget.compute_point(&window_clone, &Point::new(x as f32, y as f32)) {
                        popover.set_pointing_to(Some(&gdk::Rectangle::new(p.x() as i32, p.y() as i32, 0, 0)));
                        popover.popup();
                    }
                }
            }
        });
        tab_label.add_controller(gesture);

        tab_label
    }

    /// `gosub://` (and `about:`) pages are served by the engine's page registry like any
    /// other navigation. The one exception is `gosub://config`: the engine's version is a
    /// read-only dump (it cannot do forms yet), so Beacon renders its own editable GTK page
    /// for it. Everything else - home, help, blank, version, history, unknown pages - goes
    /// to the engine.
    fn is_shell_rendered(url: &url::Url) -> bool {
        matches!(url.scheme(), "gosub" | "about") && gosub_engine::internal_pages::InternalPages::page_name(url) == "config"
    }

    /// Sad-tab page for a crashed engine worker, with a Reload that recreates the tab.
    fn build_crashed_page(&self, tab: &GosubTab) -> Widget {
        let page = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
        page.set_hexpand(true);
        page.set_vexpand(true);
        page.set_halign(gtk4::Align::Center);
        page.set_valign(gtk4::Align::Center);
        page.add_css_class("crashed-page");

        let title = gtk4::Label::new(Some("This tab crashed"));
        title.add_css_class("crashed-title");
        page.append(&title);

        if let Some(error) = tab.crashed() {
            let detail = gtk4::Label::new(Some(error));
            detail.set_wrap(true);
            detail.set_max_width_chars(60);
            detail.add_css_class("crashed-detail");
            page.append(&detail);
        }

        let reload = Button::with_label("Reload Tab");
        reload.set_halign(gtk4::Align::Center);
        reload.add_css_class("suggested-action");
        let window = self.obj().clone();
        let tab_id = tab.id();
        reload.connect_clicked(move |_| {
            window.imp().revive_tab(tab_id);
        });
        page.append(&reload);

        page.upcast::<Widget>()
    }

    /// Recreate the engine tab behind a crashed shell tab and reload its URL.
    fn revive_tab(&self, tab_id: TabId) {
        let url = {
            let manager = self.tab_manager.lock().unwrap();
            match manager.get_tab(tab_id) {
                Some(tab) => tab.url().to_string(),
                None => return,
            }
        };

        let handle = {
            let mut eng = self.engine.borrow_mut();
            let Some(eng) = eng.as_mut() else {
                self.log("Engine not ready");
                return;
            };
            let viewport = self.viewport_for_new_tab();
            match eng.create_tab(runtime(), "New Tab", viewport) {
                Ok(h) => h,
                Err(e) => {
                    self.log(&format!("Failed to recreate engine tab: {e}"));
                    return;
                }
            }
        };

        self.engine_tab_map.borrow_mut().insert(handle.tab_id, tab_id);
        let mut manager = self.tab_manager.lock().unwrap();
        if let Some(mut tab) = manager.get_tab(tab_id) {
            tab.set_tab_handle(handle);
            tab.set_crashed(None);
            manager.update_tab(tab_id, &tab);
        }
        drop(manager);
        self.refresh_tabs();
        let _ = self.get_sender().send_blocking(Message::LoadUrl(tab_id, url));
    }

    /// The shell-rendered `gosub://config` editor (see `is_shell_rendered`).
    fn build_shell_page(&self) -> Widget {
        match self.engine.borrow().as_ref() {
            Some(engine) => super::config_page::build(engine.settings().clone()),
            None => {
                let label = gtk4::Label::new(Some("Engine not running"));
                label.set_hexpand(true);
                label.set_vexpand(true);
                label.upcast::<Widget>()
            }
        }
    }

    fn generate_default_page(&self) -> gtk4::Widget {
        let img = Image::from_resource("/io/gosub/beacon/assets/submarine.svg");
        img.set_visible(true);
        img.set_focusable(false);
        img.set_valign(gtk4::Align::Center);
        img.set_pixel_size(500);
        img.set_hexpand(true);
        img.set_vexpand(true);

        let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        vbox.set_visible(true);
        vbox.set_can_focus(false);
        vbox.set_halign(gtk4::Align::Center);
        vbox.set_vexpand(true);
        vbox.set_hexpand(true);

        vbox.append(&img);

        // Wrap in a ScrolledWindow so the oversized (500px) logo is absorbed gracefully
        // instead of producing negative allocations during the initial, tiny layout pass.
        let scrolled = ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .build();
        scrolled.set_child(Some(&vbox));
        scrolled.upcast::<Widget>()
    }

    /// Drive navigation through the engine: the engine owns fetching, parsing and rendering.
    fn navigate_engine_tab(&self, tab_id: TabId, url: &str) {
        let manager = self.tab_manager.lock().unwrap();
        let Some(tab) = manager.get_tab(tab_id) else {
            return;
        };
        let Some(handle) = tab.tab_handle() else {
            drop(manager);
            self.log("Tab has no engine handle yet");
            return;
        };
        drop(manager);

        let url = url.to_string();
        runtime().spawn(async move {
            if let Err(e) = handle.send(EngineTabCommand::Navigate { url: url.clone() }).await {
                log::error!("navigate failed: {e:?}");
            }
            let _ = handle.send(EngineTabCommand::ResumeDrawing { fps: 30 }).await;
        });
    }

    /// The tab id of the currently visible stack page, if any.
    pub(crate) fn active_tab_id(&self) -> Option<TabId> {
        self.content_stack.visible_child()?.get_tab_id()
    }

    /// Back button: the engine owns session history, so just ask it to go back. It answers
    /// with `HistoryChanged` (cursor moved) and the usual navigation events for the reload.
    pub(crate) fn navigate_back(&self) {
        self.send_history_command(EngineTabCommand::GoBack);
    }

    /// Forward button: with a single forward branch go straight there; with several, pop up a
    /// menu (anchored to `anchor`) asking which branch to follow.
    fn navigate_forward(&self, anchor: &Button) {
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let children = {
            let manager = self.tab_manager.lock().unwrap();
            match manager.get_tab(tab_id) {
                Some(tab) => tab.history().forward_children(),
                None => return,
            }
        };
        match children.as_slice() {
            [] => {}
            [_] => self.send_history_command(EngineTabCommand::GoForward { entry: None }),
            _ => self.show_forward_menu(anchor, children),
        }
    }

    /// Navigate the active tab to a specific (forward) history entry.
    fn go_to_history_entry(&self, entry: HistoryEntryId) {
        self.send_history_command(EngineTabCommand::GoForward { entry: Some(entry) });
    }

    /// The zone's places store (bookmarks + history), once the engine is up.
    fn places(&self) -> Option<gosub_engine::places::PlacesHandle> {
        self.engine.borrow().as_ref().map(|e| e.places())
    }

    /// Star button: toggles a bookmark for the active tab.
    fn setup_bookmark_button(&self) {
        let window = self.obj().clone();
        self.btn_bookmark.connect_clicked(move |_| {
            let imp = window.imp();
            let Some(places) = imp.places() else { return };
            let Some(tab_id) = imp.active_tab_id() else { return };
            let (url, title) = {
                let manager = imp.tab_manager.lock().unwrap();
                match manager.get_tab(tab_id) {
                    Some(tab) => (tab.url().to_string(), tab.title().to_string()),
                    None => return,
                }
            };
            if !url.starts_with("http") {
                return; // internal pages are not bookmarkable
            }
            if places.is_bookmarked(&url) {
                places.remove_bookmark(&url);
            } else {
                places.add_bookmark(&url, if title.is_empty() { &url } else { &title });
            }
            imp.update_bookmark_button();
            imp.rebuild_bookmarks_bar();
        });
    }

    /// Standard zoom ladder, matching mainstream browsers.
    const ZOOM_LEVELS: &'static [f64] = &[0.25, 0.33, 0.5, 0.67, 0.75, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0];

    /// Step the tab's zoom up (`+1`) or down (`-1`) along the ladder.
    pub(crate) fn zoom_step(&self, tab_id: TabId, direction: i32) {
        let current = self.tab_zoom.borrow().get(&tab_id).map(|z| z.get()).unwrap_or(1.0);
        let pos = Self::ZOOM_LEVELS.iter().position(|z| (*z - current).abs() < 1e-3).unwrap_or(6); // 1.0
        let next = pos.saturating_add_signed(direction as isize).min(Self::ZOOM_LEVELS.len() - 1);
        self.set_zoom(tab_id, Self::ZOOM_LEVELS[next]);
    }

    /// Apply `zoom` to the tab: re-store the raster DPR, resend the (shrunken) CSS
    /// viewport, and repaint. The GL composite corrects stale tiles in the meantime.
    pub(crate) fn set_zoom(&self, tab_id: TabId, zoom_level: f64) {
        let zoom_level = zoom_level.clamp(0.25, 5.0);
        let Some(cell) = self.tab_zoom.borrow().get(&tab_id).cloned() else {
            return;
        };
        if (cell.get() - zoom_level).abs() < 1e-3 {
            return;
        }
        cell.set(zoom_level);
        self.log(&format!("Zoom: {:.0}%", zoom_level * 100.0));

        let (area, handle) = {
            let areas = self.render_areas.borrow();
            let manager = self.tab_manager.lock().unwrap();
            (areas.get(&tab_id).cloned(), manager.get_tab(tab_id).and_then(|t| t.tab_handle()))
        };
        let (Some(area), Some(handle)) = (area, handle) else { return };

        use gosub_render_pipeline::render::DEVICE_PIXEL_RATIO;
        let scale = crate::engine::render_dpr(&area) as f64;
        let raster_dpr = ((scale * zoom_level).ceil() as u32).clamp(1, 4);
        DEVICE_PIXEL_RATIO.store(raster_dpr, std::sync::atomic::Ordering::Relaxed);
        let eff = scale * zoom_level;
        let (vw, vh) = (
            (area.width() as f64 * scale / eff) as u32,
            (area.height() as f64 * scale / eff) as u32,
        );
        area.queue_render();
        runtime().spawn(async move {
            let _ = handle
                .send(EngineTabCommand::SetViewport {
                    x: 0,
                    y: 0,
                    width: vw,
                    height: vh,
                })
                .await;
            let _ = handle.send(EngineTabCommand::ResumeDrawing { fps: 30 }).await;
        });
    }

    /// Reflect the active tab's bookmark state in the star icon.
    pub(crate) fn update_bookmark_button(&self) {
        let bookmarked = self
            .places()
            .zip(self.active_tab_id())
            .and_then(|(places, tab_id)| {
                let manager = self.tab_manager.lock().unwrap();
                manager.get_tab(tab_id).map(|tab| places.is_bookmarked(tab.url().as_str()))
            })
            .unwrap_or(false);
        self.bookmark_icon
            .set_icon_name(Some(if bookmarked { "starred-symbolic" } else { "non-starred-symbolic" }));
    }

    /// Rebuild the bookmarks bar from the store.
    pub(crate) fn rebuild_bookmarks_bar(&self) {
        let Some(places) = self.places() else { return };
        while let Some(child) = self.bookmarks_bar.first_child() {
            self.bookmarks_bar.remove(&child);
        }
        for bookmark in places.bookmarks() {
            let button = Button::builder()
                .label(if bookmark.title.is_empty() {
                    &bookmark.url
                } else {
                    &bookmark.title
                })
                .has_frame(false)
                .tooltip_text(&bookmark.url)
                .build();
            let window = self.obj().clone();
            let url = bookmark.url.clone();
            button.connect_clicked(move |_| {
                let imp = window.imp();
                let Some(tab_id) = imp.active_tab_id() else { return };
                let _ = imp.get_sender().send_blocking(Message::LoadUrl(tab_id, url.clone()));
            });
            self.bookmarks_bar.append(&button);
        }
    }

    /// URL-bar completion: a popover of visited pages matching what is being typed.
    fn setup_url_completion(&self) {
        let list = gtk4::ListBox::new();
        list.set_selection_mode(gtk4::SelectionMode::None);
        list.add_css_class("completion-list");
        let scroller = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .propagate_natural_height(true)
            .max_content_height(320)
            .min_content_width(500)
            .child(&list)
            .build();
        let popover = Popover::builder().child(&scroller).autohide(false).has_arrow(false).build();
        popover.set_parent(&*self.searchbar);
        *self.completion.borrow_mut() = Some((popover, list));

        let window = self.obj().clone();
        self.searchbar.connect_changed(move |entry| {
            let imp = window.imp();
            // Only complete while the user is typing, not on programmatic updates.
            if !entry.has_focus() {
                imp.hide_completion();
                return;
            }
            imp.show_completion(entry.text().as_str());
        });

        // Leaving the address bar dismisses the suggestions.
        let focus = gtk4::EventControllerFocus::new();
        let window = self.obj().clone();
        focus.connect_leave(move |_| window.imp().hide_completion());
        self.searchbar.add_controller(focus);
    }

    fn hide_completion(&self) {
        if let Some((popover, _)) = self.completion.borrow().as_ref() {
            popover.popdown();
        }
    }

    /// Populate and show the completion popover for `query`, or hide it when nothing matches.
    fn show_completion(&self, query: &str) {
        let query = query.trim();
        let hits = match self.places() {
            Some(places) if query.len() >= 2 => places.query_visited(query, 8),
            _ => Vec::new(),
        };
        let completion = self.completion.borrow();
        let Some((popover, list)) = completion.as_ref() else { return };
        if hits.is_empty() {
            popover.popdown();
            return;
        }

        while let Some(row) = list.first_child() {
            list.remove(&row);
        }
        for hit in hits {
            let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
            row.add_css_class("completion-row");
            let title = gtk4::Label::new(Some(if hit.title.is_empty() { &hit.url } else { &hit.title }));
            title.set_halign(gtk4::Align::Start);
            title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            title.add_css_class("completion-title");
            let url_label = gtk4::Label::new(Some(&hit.url));
            url_label.set_halign(gtk4::Align::Start);
            url_label.set_hexpand(true);
            url_label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
            url_label.add_css_class("completion-url");
            row.append(&title);
            row.append(&url_label);

            let click = gtk4::GestureClick::new();
            let window = self.obj().clone();
            let url = hit.url.clone();
            click.connect_released(move |_, _, _, _| {
                let imp = window.imp();
                imp.hide_completion();
                imp.searchbar.set_text(&url);
                if let Some(tab_id) = imp.active_tab_id() {
                    let _ = imp.get_sender().send_blocking(Message::LoadUrl(tab_id, url.clone()));
                }
            });
            row.add_controller(click);
            list.append(&row);
        }
        popover.popup();
    }

    /// Build the downloads popover once (a list inside a scroller on the toolbar button).
    fn setup_downloads_popover(&self) {
        let list = gtk4::ListBox::new();
        list.set_selection_mode(gtk4::SelectionMode::None);
        list.add_css_class("downloads-list");
        let scroller = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .propagate_natural_height(true)
            .max_content_height(400)
            .min_content_width(340)
            .child(&list)
            .build();
        let popover = Popover::builder().child(&scroller).build();
        self.btn_downloads.set_popover(Some(&popover));
        *self.downloads_list.borrow_mut() = Some(list);
        self.refresh_downloads();
    }

    /// Re-render the downloads popover from `self.downloads` (newest first).
    pub(crate) fn refresh_downloads(&self) {
        let list_ref = self.downloads_list.borrow();
        let Some(list) = list_ref.as_ref() else { return };
        while let Some(row) = list.first_child() {
            list.remove(&row);
        }

        let downloads = self.downloads.borrow();
        if downloads.is_empty() {
            let empty = gtk4::Label::new(Some("No downloads yet"));
            empty.add_css_class("downloads-empty");
            list.append(&empty);
            return;
        }

        for entry in downloads.iter().rev() {
            let row = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
            row.add_css_class("download-row");

            let top = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
            let name = gtk4::Label::new(Some(&entry.filename));
            name.set_halign(gtk4::Align::Start);
            name.set_hexpand(true);
            name.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
            name.add_css_class("download-name");
            top.append(&name);
            if entry.state == DownloadState::Finished {
                let open = Button::with_label("Open");
                open.set_has_frame(false);
                open.add_css_class("download-open");
                let path = entry.path.clone();
                let window = self.obj().clone();
                open.connect_clicked(move |_| {
                    let launcher = gtk4::FileLauncher::new(Some(&gtk4::gio::File::for_path(&path)));
                    launcher.launch(Some(&window), gtk4::gio::Cancellable::NONE, |result| {
                        if let Err(e) = result {
                            log::warn!("open download failed: {e}");
                        }
                    });
                });
                top.append(&open);
            }
            row.append(&top);

            match &entry.state {
                DownloadState::Running => {
                    let bar = gtk4::ProgressBar::new();
                    if let Some(total) = entry.total.filter(|t| *t > 0) {
                        bar.set_fraction(entry.received as f64 / total as f64);
                    }
                    row.append(&bar);
                    let status = gtk4::Label::new(Some(&match entry.total {
                        Some(total) => format!("{} of {}", human_bytes(entry.received), human_bytes(total)),
                        None => format!("{} so far…", human_bytes(entry.received)),
                    }));
                    status.set_halign(gtk4::Align::Start);
                    status.add_css_class("download-status");
                    row.append(&status);
                }
                DownloadState::Finished => {
                    let status = gtk4::Label::new(Some(&format!("{} — {}", human_bytes(entry.received), entry.path.display())));
                    status.set_halign(gtk4::Align::Start);
                    status.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
                    status.add_css_class("download-status");
                    row.append(&status);
                }
                DownloadState::Failed(error) => {
                    let status = gtk4::Label::new(Some(&format!("Failed: {error}")));
                    status.set_halign(gtk4::Align::Start);
                    status.set_wrap(true);
                    status.add_css_class("download-failed");
                    row.append(&status);
                }
            }
            list.append(&row);
        }
    }

    /// Record a new running download and update the popover.
    pub(crate) fn add_download(&self, id: u64, path: &std::path::Path) {
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "download".into());
        self.downloads.borrow_mut().push(DownloadEntry {
            id,
            filename,
            path: path.to_path_buf(),
            received: 0,
            total: None,
            state: DownloadState::Running,
        });
        self.refresh_downloads();
    }

    /// Apply an engine download event to the matching entry and update the popover.
    fn update_download(&self, id: u64, apply: impl FnOnce(&mut DownloadEntry)) {
        {
            let mut downloads = self.downloads.borrow_mut();
            let Some(entry) = downloads.iter_mut().find(|e| e.id == id) else {
                return;
            };
            apply(entry);
        }
        self.refresh_downloads();
    }

    /// Ask the user where to save `url` (native save dialog, prefilled with
    /// `suggested_name`), then start the engine download on `tab_id`'s handle.
    pub(crate) fn save_download_as(&self, tab_id: TabId, url: String, suggested_name: &str) {
        let handle = {
            let manager = self.tab_manager.lock().unwrap();
            manager.get_tab(tab_id).and_then(|t| t.tab_handle())
        };
        let Some(handle) = handle else {
            self.log("Tab has no engine handle for the download");
            return;
        };

        let dialog = gtk4::FileDialog::builder().initial_name(suggested_name).build();
        let window = self.obj().clone();
        dialog.save(Some(&*self.obj()), gtk4::gio::Cancellable::NONE, move |result| {
            // Cancelling the dialog just drops the offer.
            let Ok(file) = result else { return };
            let Some(path) = file.path() else { return };
            let id = window.imp().next_download_id.get();
            window.imp().next_download_id.set(id + 1);
            window.imp().log(&format!("Download #{id}: {url} → {}", path.display()));
            window.imp().add_download(id, &path);
            let handle = handle.clone();
            runtime().spawn(async move {
                let _ = handle
                    .send(EngineTabCommand::StartDownload {
                        id: gosub_engine::events::DownloadId(id),
                        url,
                        target_path: path,
                    })
                    .await;
            });
        });
    }

    /// Send a history traversal command to the active tab's engine tab and mark it loading.
    pub(crate) fn send_history_command(&self, cmd: EngineTabCommand) {
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let handle = {
            let mut manager = self.tab_manager.lock().unwrap();
            let Some(mut tab) = manager.get_tab(tab_id) else {
                return;
            };
            let handle = tab.tab_handle();
            tab.set_loading(true);
            manager.update_tab(tab_id, &tab);
            handle
        };
        let Some(handle) = handle else {
            self.log("Tab has no engine handle yet");
            return;
        };
        self.refresh_tabs();
        runtime().spawn(async move {
            if let Err(e) = handle.send(cmd).await {
                log::error!("history command failed: {e:?}");
            }
            let _ = handle.send(EngineTabCommand::ResumeDrawing { fps: 30 }).await;
        });
    }

    /// Build and show a popover listing the forward branches of the active tab; picking one
    /// navigates to it.
    fn show_forward_menu(&self, anchor: &Button, children: Vec<(HistoryEntryId, url::Url)>) {
        let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let popover = Popover::builder().build();
        popover.set_parent(anchor);
        popover.connect_closed(|p| p.unparent());

        for (id, url) in children {
            let item = Button::builder().label(url.as_str()).has_frame(false).build();
            if let Some(label) = item.child().and_downcast::<gtk4::Label>() {
                label.set_xalign(0.0);
                label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            }
            let window = self.obj().clone();
            let popover_clone = popover.clone();
            item.connect_clicked(move |_| {
                popover_clone.popdown();
                window.imp().go_to_history_entry(id);
            });
            vbox.append(&item);
        }

        popover.set_child(Some(&vbox));
        popover.popup();
    }

    /// Open a read-only monospace window showing the fetched page source.
    fn show_source_window(&self, url: &str, content: &str) {
        let buffer = gtk4::TextBuffer::builder().text(content).build();
        let view = TextView::builder()
            .buffer(&buffer)
            .editable(false)
            .cursor_visible(false)
            .monospace(true)
            .wrap_mode(gtk4::WrapMode::None)
            .build();

        let scrolled = ScrolledWindow::builder().hexpand(true).vexpand(true).child(&view).build();

        let parent = self.obj();
        let window = gtk4::Window::builder()
            .transient_for(&*parent)
            .title(format!("Source: {url}"))
            .default_width(800)
            .default_height(600)
            .child(&scrolled)
            .build();
        window.present();
    }

    /// Enable/disable the back and forward buttons based on the active tab's history.
    pub(crate) fn update_nav_buttons(&self) {
        let (back, forward) = match self.active_tab_id() {
            Some(id) => {
                let manager = self.tab_manager.lock().unwrap();
                match manager.get_tab(id) {
                    Some(tab) => (tab.history().can_go_back(), tab.history().can_go_forward()),
                    None => (false, false),
                }
            }
            None => (false, false),
        };
        self.btn_prev.set_sensitive(back);
        self.btn_next.set_sensitive(forward);
    }

    /// Handles all message coming from the async (tokio) tasks
    pub async fn handle_message(&self, message: Message) {
        info!(target: "gtk", "Received a message: {:?}", message);

        match message {
            Message::RefreshTabs() => {
                self.refresh_tabs();
            }
            Message::OpenTab(url, title) => {
                self.open_tab(None, &url, &title);
            }
            Message::OpenTabRight(target_tab_id, url, title) => {
                if let Some(pos) = self.get_page_num_for_tab(target_tab_id) {
                    self.open_tab(Some(pos as usize + 1), &url, &title);
                }
            }

            Message::LoadUrl(tab_id, url_str) => {
                self.log(format!("Loading URL: {}", url_str).as_str());

                let Ok((_view_mode, url)) = GosubAddressParser::parse(url_str.as_str()) else {
                    self.log("Cannot parse URL");
                    return;
                };

                // Update information in the given tab with the new url
                let shell = Self::is_shell_rendered(&url);
                let mut manager = self.tab_manager.lock().unwrap();
                let mut tab = manager.get_tab(tab_id).unwrap().clone();

                tab.set_favicon(None);
                tab.set_title(if shell { "Engine settings" } else { url.as_str() });
                tab.set_url(url.clone());
                tab.set_loading(!shell);

                manager.update_tab(tab_id, &tab);
                drop(manager);

                self.refresh_tabs();

                // Everything but the shell-rendered config page is an engine navigation
                // (gosub:// pages included - the engine serves them from its registry).
                if !shell {
                    self.navigate_engine_tab(tab_id, url.as_str());
                }
            }
            Message::Log(msg) => {
                self.log(msg.as_str());
            }
            Message::ShowSource(url, content) => {
                self.show_source_window(&url, &content);
            }
            Message::PinTab(tab_id) => {
                let mut manager = self.tab_manager.lock().unwrap();
                manager.pin_tab(tab_id);
                drop(manager);

                // Update tab-bar
                self.refresh_tabs();
            }
            Message::FaviconLoaded(tab_id, bytes) => {
                // PixbufLoader handles ICO (the common favicon format), which
                // gdk::Texture::from_bytes does not reliably decode.
                let loader = gtk4::gdk_pixbuf::PixbufLoader::new();
                let texture = loader
                    .write(&bytes)
                    .and_then(|_| loader.close())
                    .ok()
                    .and_then(|_| loader.pixbuf())
                    .map(|pixbuf| gdk::Texture::for_pixbuf(&pixbuf));
                let Some(texture) = texture else {
                    self.log("Could not decode favicon");
                    return;
                };

                let mut manager = self.tab_manager.lock().unwrap();
                if let Some(mut tab) = manager.get_tab(tab_id) {
                    tab.set_favicon(Some(texture));
                    manager.update_tab(tab_id, &tab);
                }
                drop(manager);
                self.refresh_tabs();
            }
            Message::UnpinTab(tab_id) => {
                let mut manager = self.tab_manager.lock().unwrap();
                manager.unpin_tab(tab_id);
                drop(manager);

                // Update tab-bar
                self.refresh_tabs();
            }
        }
    }

    /// Retrieves the strip position for the given TabID
    fn get_page_num_for_tab(&self, tab_id: TabId) -> Option<u32> {
        self.chips().iter().position(|c| c.get_tab_id() == Some(tab_id)).map(|i| i as u32)
    }

    /// Opens a new tab at the given position, with the given URL and title. If the position is None,
    /// the tab will be added at the end of the tab-bar.
    fn open_tab(&self, position: Option<usize>, url_str: &str, title: &str) {
        let Ok((_render_mode, url)) = GosubAddressParser::parse(url_str) else {
            self.log("Cannot parse URL");
            return;
        };

        let mut tab = GosubTab::new(url.clone(), title);
        let tab_id = tab.id();

        // Create the matching engine-side tab and remember the id mapping.
        let handle = {
            let mut eng = self.engine.borrow_mut();
            let Some(eng) = eng.as_mut() else {
                self.log("Engine not ready");
                return;
            };
            let viewport = self.viewport_for_new_tab();
            match eng.create_tab(runtime(), title, viewport) {
                Ok(h) => h,
                Err(e) => {
                    self.log(format!("Failed to create engine tab: {e}").as_str());
                    return;
                }
            }
        };
        self.engine_tab_map.borrow_mut().insert(handle.tab_id, tab_id);
        tab.set_tab_handle(handle);

        let shell = Self::is_shell_rendered(&url);
        if shell {
            tab.set_title("Engine settings");
        }
        tab.set_loading(!shell);

        // add tab to manager, and notify the tab has changed. This will update the
        // tab-bar during a refresh-tabs call.
        let mut manager = self.tab_manager.lock().unwrap();
        manager.add_tab(tab, position);
        manager.notify_tab_changed(tab_id);
        drop(manager);
        self.refresh_tabs();

        if !shell {
            self.navigate_engine_tab(tab_id, url.as_str());
        }
    }

    /// Build a GL area that composites the engine's tile frames for `tab` on the GPU,
    /// and forwards resize/scroll input to the engine tab.
    fn build_render_area(&self, tab: &GosubTab) -> GLArea {
        let area = GLArea::new();
        area.set_has_depth_buffer(false);
        area.set_has_stencil_buffer(true);
        area.set_vexpand(true);
        area.set_hexpand(true);
        area.set_focusable(true);

        let engine_id = tab.engine_tab_id().expect("engine tab id");
        let compositor = self.engine.borrow().as_ref().expect("engine initialised").compositor.clone();

        // Page zoom for this tab; survives render-area rebuilds via the map.
        let zoom = self
            .tab_zoom
            .borrow_mut()
            .entry(tab.id())
            .or_insert_with(|| Rc::new(Cell::new(1.0)))
            .clone();

        // Skia's GL context wrapper: created once the area is realized (its GdkGLContext
        // exists from then on), dropped again on unrealize so it can't outlive the context.
        let dc_holder: Rc<RefCell<Option<skia_safe::gpu::DirectContext>>> = Rc::new(RefCell::new(None));
        area.connect_realize({
            let dc_holder = dc_holder.clone();
            move |area| {
                area.make_current();
                if let Some(err) = area.error() {
                    log::error!("GLArea realize error: {err:?}");
                    return;
                }
                let Some(interface) = skia_safe::gpu::gl::Interface::new_native() else {
                    log::error!("Skia GL interface creation failed");
                    return;
                };
                *dc_holder.borrow_mut() = skia_safe::gpu::direct_contexts::make_gl(interface, None);
            }
        });
        area.connect_unrealize({
            let dc_holder = dc_holder.clone();
            move |_| {
                dc_holder.borrow_mut().take();
            }
        });

        area.connect_render({
            let dc_holder = dc_holder.clone();
            let zoom = zoom.clone();
            move |area, _ctx| {
                let mut dc_ref = dc_holder.borrow_mut();
                let Some(dc) = dc_ref.as_mut() else {
                    return glib::Propagation::Stop;
                };
                let scale = crate::engine::render_dpr(area) as i32;
                let target_scale = scale as f64 * zoom.get();
                render_frame_gl(
                    &compositor,
                    engine_id,
                    dc,
                    area.width() * scale,
                    area.height() * scale,
                    target_scale,
                );
                glib::Propagation::Stop
            }
        });

        if let Some(handle) = tab.tab_handle() {
            // Resize -> tell the engine the new viewport. The DPR must be stored
            // before the viewport lands so the rasterizer renders at physical
            // resolution — otherwise HiDPI/fractional-scale displays get a 1x
            // buffer upscaled by the compositor (blurry text).
            let resize_handle = handle.clone();
            let resize_zoom = zoom.clone();
            let resize_last_viewport = self.last_viewport.clone();
            area.connect_resize(move |area, w, h| {
                use gosub_render_pipeline::render::DEVICE_PIXEL_RATIO;
                let z = resize_zoom.get();
                // Rasterize at ceil(display scale × zoom) so zoomed-in pages stay sharp;
                // capped because tile memory grows with its square.
                let raster_dpr = ((crate::engine::render_dpr(area) as f64 * z).ceil() as u32).clamp(1, 4);
                DEVICE_PIXEL_RATIO.store(raster_dpr, std::sync::atomic::Ordering::Relaxed);
                // GtkGLArea's resize reports PHYSICAL pixels; the engine lays out in CSS
                // px, which shrink as the zoom grows.
                let eff = crate::engine::render_dpr(area) as f64 * z;
                let (vw, vh) = ((w as f64 / eff) as u32, (h as f64 / eff) as u32);
                if vw == 0 || vh == 0 {
                    return;
                }
                // Record it so tabs whose own GLArea has never been allocated are sized by
                // this exact formula rather than a second, subtly different one.
                resize_last_viewport.set(Some((vw, vh)));
                let handle = resize_handle.clone();
                runtime().spawn(async move {
                    let _ = handle
                        .send(EngineTabCommand::SetViewport {
                            x: 0,
                            y: 0,
                            width: vw,
                            height: vh,
                        })
                        .await;
                });
            });

            // Scroll -> forward to the engine; it re-renders and notifies us to repaint.
            let scroll = gtk4::EventControllerScroll::new(gtk4::EventControllerScrollFlags::BOTH_AXES);
            let scroll_handle = handle.clone();
            let scroll_zoom = zoom.clone();
            let scroll_window = self.obj().clone();
            let scroll_tab_id = tab.id();
            scroll.connect_scroll(move |c, dx, dy| {
                // Ctrl+wheel zooms instead of scrolling, like every browser.
                if c.current_event_state().contains(gdk::ModifierType::CONTROL_MASK) {
                    scroll_window.imp().zoom_step(scroll_tab_id, if dy < 0.0 { 1 } else { -1 });
                    return glib::Propagation::Stop;
                }
                let handle = scroll_handle.clone();
                // The engine scrolls in CSS px, which cover more screen when zoomed in.
                let z = scroll_zoom.get() as f32;
                let delta_x = dx as f32 * 40.0 / z;
                let delta_y = dy as f32 * 40.0 / z;
                runtime().spawn(async move {
                    let _ = handle.send(EngineTabCommand::MouseScroll { delta_x, delta_y }).await;
                });
                glib::Propagation::Stop
            });
            area.add_controller(scroll);

            // Mouse move -> hover. The engine resolves the link under the cursor and emits
            // a `HoverUrl` event back to us.
            let motion = gtk4::EventControllerMotion::new();
            let motion_handle = handle.clone();
            let motion_zoom = zoom.clone();
            motion.connect_motion(move |_c, x, y| {
                let handle = motion_handle.clone();
                let z = motion_zoom.get();
                let (x, y) = (x / z, y / z);
                runtime().spawn(async move {
                    let _ = handle.send(EngineTabCommand::MouseMove { x: x as f32, y: y as f32 }).await;
                });
            });
            area.add_controller(motion);

            // Primary click -> mouse down (lets the engine follow links).
            let click = gtk4::GestureClick::new();
            click.set_button(gdk::BUTTON_PRIMARY);
            let click_handle = handle.clone();
            let click_zoom = zoom.clone();
            click.connect_pressed(move |g, _n, x, y| {
                // Keys should go to the page after a click on it.
                if let Some(widget) = g.widget() {
                    widget.grab_focus();
                }
                let handle = click_handle.clone();
                let z = click_zoom.get();
                let (x, y) = (x / z, y / z);
                runtime().spawn(async move {
                    let _ = handle
                        .send(EngineTabCommand::MouseDown {
                            x: x as f32,
                            y: y as f32,
                            button: gosub_engine::events::MouseButton::Left,
                        })
                        .await;
                });
            });
            area.add_controller(click);

            // Keyboard -> engine. Shortcuts with Control/Alt/Super are the shell's (Ctrl+T
            // and friends) and propagate to GTK; everything else is the page's: focus
            // traversal (Tab), link activation (Enter), and scrolling keys.
            let keys = gtk4::EventControllerKey::new();
            let key_handle = handle.clone();
            keys.connect_key_pressed(move |_c, keyval, _keycode, state| {
                if state.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK | gdk::ModifierType::SUPER_MASK) {
                    return glib::Propagation::Proceed;
                }
                let Some(key) = web_key_name(keyval) else {
                    return glib::Propagation::Proceed;
                };
                let modifiers = engine_modifiers(state);
                let handle = key_handle.clone();
                // Web `code` (physical key) is approximated with the logical name until a
                // scancode map exists; the engine only reads `key` today.
                let code = key.clone();
                runtime().spawn(async move {
                    let _ = handle.send(EngineTabCommand::KeyDown { key, code, modifiers }).await;
                });
                glib::Propagation::Stop
            });
            area.add_controller(keys);

            // Secondary click -> ask the engine what is under the pointer; the context menu
            // is built from its HitTestResult (see handle_engine_event).
            let right = gtk4::GestureClick::new();
            right.set_button(gdk::BUTTON_SECONDARY);
            let right_handle = handle.clone();
            let right_zoom = zoom.clone();
            let window = self.obj().clone();
            let our_tab_id = tab.id();
            right.connect_pressed(move |gesture, _n, x, y| {
                let imp = window.imp();
                let token = imp.next_hit_test_token.get();
                imp.next_hit_test_token.set(token + 1);
                // Remember where to anchor the menu, in window coordinates.
                let Some(widget) = gesture.widget() else { return };
                let Some(p) = widget.compute_point(&window, &Point::new(x as f32, y as f32)) else {
                    return;
                };
                imp.pending_hit_tests.borrow_mut().insert(token, (our_tab_id, p));
                let handle = right_handle.clone();
                let z = right_zoom.get();
                let (qx, qy) = (x / z, y / z);
                runtime().spawn(async move {
                    let _ = handle
                        .send(EngineTabCommand::QueryHitTest {
                            x: qx as f32,
                            y: qy as f32,
                            token: gosub_engine::events::HitTestToken(token),
                        })
                        .await;
                });
            });
            area.add_controller(right);
        }

        self.render_areas.borrow_mut().insert(tab.id(), area.clone());
        area
    }

    /// Start the engine and wire its redraw/event notifications into the GTK main loop.
    pub fn init_engine(&self) {
        let mut engine = match BrowserEngine::new(runtime()) {
            Ok(e) => e,
            Err(e) => {
                self.log(format!("Failed to start engine: {e}").as_str());
                log::error!("engine init failed: {e:?}");
                return;
            }
        };

        let redraw_rx = engine.take_redraw_rx();
        let event_rx = engine.take_event_rx();
        *self.engine.borrow_mut() = Some(engine);

        // Repaint all render areas whenever a new frame is composited.
        if let Some(mut redraw_rx) = redraw_rx {
            let render_areas = self.render_areas.clone();
            glib::spawn_future_local(async move {
                while redraw_rx.recv().await.is_some() {
                    for area in render_areas.borrow().values() {
                        area.queue_render();
                    }
                }
            });
        }

        // The bookmarks bar renders from the store as soon as the engine exists.
        self.rebuild_bookmarks_bar();

        // Route engine events (navigation, redraw, …) to the window.
        if let Some(mut event_rx) = event_rx {
            let weak = self.obj().downgrade();
            glib::spawn_future_local(async move {
                loop {
                    match event_rx.recv().await {
                        Ok(evt) => {
                            if let Some(win) = weak.upgrade() {
                                win.imp().handle_engine_event(evt);
                            } else {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        }
    }

    /// Handle a single engine event on the GTK main thread.
    fn handle_engine_event(&self, evt: EngineEvent) {
        match evt {
            EngineEvent::Redraw { .. } => {
                for area in self.render_areas.borrow().values() {
                    area.queue_render();
                }
            }
            EngineEvent::Navigation { tab_id, event } => {
                let Some(our_id) = self.engine_tab_map.borrow().get(&tab_id).copied() else {
                    return;
                };

                // The engine serves gosub:// pages itself, except the one page Beacon
                // renders as a GTK widget (gosub://config). A link to it clicked inside an
                // engine page starts an engine navigation; cancel that and swap in the
                // shell page instead. (LoadUrl already set the tab's URL for our own
                // navigations, so the equality check keeps this from looping.)
                if let NavigationEvent::Started { url, .. } = &event {
                    if self.active_tab_id() == Some(our_id) && !Self::is_shell_rendered(url) {
                        self.searchbar.set_progress_fraction(0.05);
                    }
                    if Self::is_shell_rendered(url) {
                        let (differs, handle) = {
                            let manager = self.tab_manager.lock().unwrap();
                            match manager.get_tab(our_id) {
                                Some(tab) => (tab.url() != url, tab.tab_handle()),
                                None => (false, None),
                            }
                        };
                        if differs {
                            if let Some(handle) = handle {
                                runtime().spawn(async move {
                                    let _ = handle.send(EngineTabCommand::CancelNavigation).await;
                                });
                            }
                            let _ = self.get_sender().send_blocking(Message::LoadUrl(our_id, url.to_string()));
                        }
                        return;
                    }
                }

                // Load progress for the active tab, drawn as the address bar's fill
                // (GtkEntry's built-in progress underline).
                if let NavigationEvent::Progress {
                    received_bytes,
                    expected_length,
                    ..
                } = &event
                {
                    if self.active_tab_id() == Some(our_id) {
                        let fraction = match expected_length {
                            Some(total) if *total > 0 => (*received_bytes as f64 / *total as f64).clamp(0.05, 0.98),
                            // Unknown length: park mid-way rather than pretending precision.
                            _ => 0.5,
                        };
                        self.searchbar.set_progress_fraction(fraction);
                    }
                    return;
                }

                if let NavigationEvent::HistoryChanged { history } = event {
                    // The engine also updates the address bar target: on a back/forward
                    // traversal the tab's URL is the entry we moved to, even while it loads.
                    let current_url = history.current.and_then(|id| history.entries.get(id.0)).map(|e| e.url.clone());
                    let mut manager = self.tab_manager.lock().unwrap();
                    if let Some(mut tab) = manager.get_tab(our_id) {
                        tab.history_mut().update(history);
                        if let Some(url) = &current_url {
                            tab.set_url(url.clone());
                        }
                        manager.update_tab(our_id, &tab);
                    }
                    drop(manager);
                    if self.active_tab_id() == Some(our_id) {
                        if let Some(url) = &current_url {
                            self.searchbar.set_text(url.as_str());
                        }
                        self.update_nav_buttons();
                    }
                    return;
                }

                // Load ended without a page change (stop button, download offer):
                // clear the progress fill.
                if let NavigationEvent::Cancelled { .. } = &event {
                    if self.active_tab_id() == Some(our_id) {
                        self.searchbar.set_progress_fraction(0.0);
                    }
                    return;
                }

                if let NavigationEvent::Failed { url, error, .. } = &event {
                    if self.active_tab_id() == Some(our_id) {
                        self.searchbar.set_progress_fraction(0.0);
                    }
                    self.on_navigation_failed(our_id, url, &error.to_string());
                    return;
                }
                if let NavigationEvent::FailedUrl { url, error, .. } = &event {
                    self.log(&format!("Cannot load {url}: {error}"));
                    return;
                }

                if let NavigationEvent::Finished { url, .. } = event {
                    if self.active_tab_id() == Some(our_id) {
                        self.searchbar.set_progress_fraction(0.0);
                    }
                    let mut manager = self.tab_manager.lock().unwrap();
                    if let Some(mut tab) = manager.get_tab(our_id) {
                        tab.set_loading(false);
                        tab.set_title(url.as_str());
                        // Session history is recorded by the engine; it follows up with a
                        // HistoryChanged event that refreshes the back/forward state. The
                        // favicon likewise arrives as a FavIconChanged event.
                        manager.update_tab(our_id, &tab);
                    }
                    drop(manager);

                    // Update the address bar if this is the active tab.
                    if self.active_tab_id() == Some(our_id) {
                        self.searchbar.set_text(url.as_str());
                    }
                    self.refresh_tabs();
                    self.update_nav_buttons();
                    self.update_bookmark_button();
                }
            }
            // The engine fetched the page's icon (through its own fetcher, so with the
            // page's cookies and UA); decode it on the GTK side like before.
            EngineEvent::FavIconChanged { tab_id, favicon } => {
                let Some(our_id) = self.engine_tab_map.borrow().get(&tab_id).copied() else {
                    return;
                };
                let _ = self.get_sender().send_blocking(Message::FaviconLoaded(our_id, favicon));
            }
            // Answer to a right-click's QueryHitTest: build the page context menu from it.
            // A navigation turned out to be a download: ask where to save it, then hand
            // the engine the chosen path.
            EngineEvent::DownloadRequested {
                tab_id,
                url,
                suggested_filename,
                total_bytes,
                ..
            } => {
                let Some(our_id) = self.engine_tab_map.borrow().get(&tab_id).copied() else {
                    return;
                };
                let size = total_bytes.map(|b| format!(" ({b} bytes)")).unwrap_or_default();
                self.log(&format!("Download offered: {suggested_filename}{size}"));
                self.save_download_as(our_id, url.to_string(), &suggested_filename);
            }
            EngineEvent::DownloadProgress {
                id,
                received_bytes,
                total_bytes,
                ..
            } => {
                self.update_download(id.0, |e| {
                    e.received = received_bytes;
                    e.total = total_bytes;
                });
            }
            EngineEvent::DownloadFinished {
                id, path, received_bytes, ..
            } => {
                self.log(&format!("Download #{} finished: {} ({received_bytes} bytes)", id.0, path.display()));
                self.update_download(id.0, |e| {
                    e.received = received_bytes;
                    e.state = DownloadState::Finished;
                });
            }
            EngineEvent::DownloadFailed { id, error, .. } => {
                self.log(&format!("Download #{} FAILED: {error}", id.0));
                self.update_download(id.0, |e| e.state = DownloadState::Failed(error.clone()));
            }
            EngineEvent::HitTestResult { token, hit, .. } => {
                let Some((tab_id, point)) = self.pending_hit_tests.borrow_mut().remove(&token.0) else {
                    return;
                };
                super::page_context_menu::show(&self.obj(), tab_id, point, hit);
            }
            // Cursor shape for what is under the pointer; only the active tab's area is under
            // the pointer, but setting it on the tab's own area is always correct.
            EngineEvent::CursorChanged { tab_id, cursor } => {
                let Some(our_id) = self.engine_tab_map.borrow().get(&tab_id).copied() else {
                    return;
                };
                let name = match cursor {
                    gosub_engine::events::CursorShape::Pointer => "pointer",
                    gosub_engine::events::CursorShape::Text => "text",
                    gosub_engine::events::CursorShape::Default => "default",
                };
                if let Some(area) = self.render_areas.borrow().get(&our_id) {
                    area.set_cursor_from_name(Some(name));
                }
            }
            // The tab's engine worker died. Mark the tab crashed: its page becomes the
            // sad-tab widget with a Reload button that recreates the engine tab.
            EngineEvent::TabCrashed { tab_id, error, .. } => {
                let Some(our_id) = self.engine_tab_map.borrow_mut().remove(&tab_id) else {
                    return;
                };
                self.log(&format!("Tab crashed: {error}"));
                self.render_areas.borrow_mut().remove(&our_id);
                let mut manager = self.tab_manager.lock().unwrap();
                if let Some(mut tab) = manager.get_tab(our_id) {
                    tab.set_loading(false);
                    tab.set_crashed(Some(error));
                    manager.update_tab(our_id, &tab);
                }
                drop(manager);
                self.refresh_tabs();
                self.update_nav_buttons();
            }
            // Focus moved inside the page. Nothing to do yet - this becomes the IME /
            // on-screen-keyboard trigger once text editing lands.
            EngineEvent::FocusChanged { focused, editable, .. } => {
                log::debug!("page focus changed: focused={focused} editable={editable}");
            }
            EngineEvent::TitleChanged { tab_id, title } => {
                let Some(our_id) = self.engine_tab_map.borrow().get(&tab_id).copied() else {
                    return;
                };
                let mut manager = self.tab_manager.lock().unwrap();
                if let Some(mut tab) = manager.get_tab(our_id) {
                    tab.set_title(&title);
                    manager.update_tab(our_id, &tab);
                }
                drop(manager);
                self.refresh_tabs();

                if self.active_tab_id() == Some(our_id) {
                    self.obj().set_title(Some(&format!("{title} — Gosub Beacon")));
                }
            }
            EngineEvent::HoverUrl { tab_id, url } => {
                // Only surface hover for the active tab.
                let Some(our_id) = self.engine_tab_map.borrow().get(&tab_id).copied() else {
                    return;
                };
                if self.active_tab_id() == Some(our_id) {
                    let text = url.as_deref().unwrap_or("");
                    self.statusbar.set_text(text);
                    self.statusbar.set_visible(!text.is_empty());
                }
            }
            _ => {}
        }
    }
}

/// Map a GDK keyval to the web [`KeyboardEvent.key`] name the engine expects.
/// `None` for keys that have neither a named mapping nor a printable character.
fn web_key_name(key: gdk::Key) -> Option<String> {
    use gdk::Key;
    let named = match key {
        Key::Return | Key::KP_Enter => "Enter",
        // Shift+Tab arrives as ISO_Left_Tab; the SHIFT modifier carries the direction.
        Key::Tab | Key::ISO_Left_Tab => "Tab",
        Key::Escape => "Escape",
        Key::BackSpace => "Backspace",
        Key::Delete => "Delete",
        Key::Up => "ArrowUp",
        Key::Down => "ArrowDown",
        Key::Left => "ArrowLeft",
        Key::Right => "ArrowRight",
        Key::Page_Up => "PageUp",
        Key::Page_Down => "PageDown",
        Key::Home => "Home",
        Key::End => "End",
        Key::space => " ",
        _ => return key.to_unicode().filter(|c| !c.is_control()).map(|c| c.to_string()),
    };
    Some(named.to_string())
}

/// Map GDK modifier state to the engine's [`Modifiers`] flags.
fn engine_modifiers(state: gdk::ModifierType) -> gosub_engine::events::Modifiers {
    use gosub_engine::events::Modifiers;
    let mut m = Modifiers::empty();
    if state.contains(gdk::ModifierType::SHIFT_MASK) {
        m |= Modifiers::SHIFT;
    }
    if state.contains(gdk::ModifierType::CONTROL_MASK) {
        m |= Modifiers::CONTROL;
    }
    if state.contains(gdk::ModifierType::ALT_MASK) {
        m |= Modifiers::ALT;
    }
    if state.contains(gdk::ModifierType::SUPER_MASK) {
        m |= Modifiers::META;
    }
    m
}

/// Compact human byte count for the downloads popover (e.g. "3.4 MB").
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
