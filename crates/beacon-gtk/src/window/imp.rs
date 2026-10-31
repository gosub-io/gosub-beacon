use crate::render::{render_frame_gl, BrowserEngine};
use crate::runtime;
use crate::window::message::Message;
use crate::window::tab_context_menu::{build_context_menu, setup_context_menu_actions, TabInfo};
use async_channel::{Receiver, Sender};
use beacon_core::address_parser::{GosubAddressParser, GosubRenderMode};
use beacon_core::beacon::Beacon;
use beacon_core::command::BeaconCommand;
use beacon_core::download::DownloadState;
use beacon_core::event::{BeaconEvent, Cursor, PickerKind};
use beacon_core::tab::{GosubTab, GosubTabManager, HistoryEntryId, TabCommand, TabId};
use glib::subclass::InitializingObject;
use gosub_engine::events::{EngineEvent, NavigationEvent, TabCommand as EngineTabCommand};
use gosub_engine::tab::TabHandle;
use gtk4::gio::SimpleActionGroup;
use gtk4::glib::subclass::Signal;
use gtk4::glib::Quark;
use gtk4::graphene::Point;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{
    gdk, glib, Button, CompositeTemplate, Entry, GLArea, GestureClick, Image, Popover, PopoverMenu, PopoverMenuFlags, ScrolledWindow,
    SearchEntry, Settings, Stack, TemplateChild, TextView, ToggleButton, Widget,
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

/// Whether a render mode shows page source; `Some(true)` means unhighlighted (`raw:`).
fn source_mode(mode: &GosubRenderMode) -> Option<bool> {
    match mode {
        GosubRenderMode::Source => Some(false),
        GosubRenderMode::RawSource => Some(true),
        _ => None,
    }
}

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
    pub tab_scroller: TemplateChild<ScrolledWindow>,
    /// Row 2 (the tab strip's row) and row 3 (the nav toolbar). Held so fullscreen can
    /// hide the whole chrome, not just the header bar.
    #[template_child]
    pub tab_row: TemplateChild<gtk4::Box>,
    #[template_child]
    pub nav_toolbar: TemplateChild<gtk4::Box>,
    #[template_child]
    pub content_stack: TemplateChild<Stack>,
    #[template_child]
    pub content_paned: TemplateChild<gtk4::Paned>,
    #[template_child]
    pub devtools_pane: TemplateChild<gtk4::Box>,
    #[template_child]
    pub devtools_stack: TemplateChild<Stack>,
    #[template_child]
    pub devtools_action: TemplateChild<Button>,
    #[template_child]
    pub devtools_summary: TemplateChild<gtk4::Label>,
    #[template_child]
    pub devtools_filter: TemplateChild<SearchEntry>,
    #[template_child]
    pub log_scroller: TemplateChild<ScrolledWindow>,
    #[template_child]
    pub log_list: TemplateChild<gtk4::ColumnView>,
    #[template_child]
    pub timings_scroller: TemplateChild<ScrolledWindow>,
    #[template_child]
    pub timings_list: TemplateChild<gtk4::ColumnView>,
    #[template_child]
    pub history_list: TemplateChild<gtk4::ColumnView>,
    #[template_child]
    pub report_list: TemplateChild<gtk4::ColumnView>,
    #[template_child]
    pub network_scroller: TemplateChild<ScrolledWindow>,
    #[template_child]
    pub network_by_origin: TemplateChild<ToggleButton>,
    #[template_child]
    pub network_view: TemplateChild<Stack>,
    #[template_child]
    pub origins_list: TemplateChild<gtk4::ColumnView>,
    #[template_child]
    pub processes_list: TemplateChild<gtk4::ColumnView>,
    #[template_child]
    pub timeline_nav: TemplateChild<gtk4::DropDown>,
    #[template_child]
    pub timeline_area: TemplateChild<gtk4::DrawingArea>,
    #[template_child]
    pub network_list: TemplateChild<gtk4::ColumnView>,
    #[template_child]
    pub network_detail_stack: TemplateChild<Stack>,
    #[template_child]
    pub detail_raw: TemplateChild<gtk4::ToggleButton>,
    #[template_child]
    pub request_stack: TemplateChild<Stack>,
    #[template_child]
    pub response_stack: TemplateChild<Stack>,
    #[template_child]
    pub request_list: TemplateChild<gtk4::ListBox>,
    #[template_child]
    pub response_list: TemplateChild<gtk4::ListBox>,
    #[template_child]
    pub timing_list: TemplateChild<gtk4::ListBox>,
    #[template_child]
    pub sub_list: TemplateChild<gtk4::ListBox>,
    #[template_child]
    pub network_request_view: TemplateChild<TextView>,
    #[template_child]
    pub network_response_view: TemplateChild<TextView>,
    #[template_child]
    pub network_body_view: TemplateChild<TextView>,

    /// The request currently shown in the detail pane, so a refresh can reselect it.
    pub network_selected: RefCell<Option<uuid::Uuid>>,
    /// The timings table's rows, updated in place like the others'.
    pub timings_model: gtk4::gio::ListStore,
    /// The history tree's lines, updated in place like the others'.
    pub history_model: gtk4::gio::ListStore,
    /// The Page report's lines, updated in place like the others'.
    pub report_model: gtk4::gio::ListStore,
    /// The network table's By origin rows, updated in place like the others'.
    pub origins_model: gtk4::gio::ListStore,
    /// The Processes tab's rows, updated in place like the others'.
    pub processes_model: gtk4::gio::ListStore,
    /// The loads the Timeline's picker offers, oldest first, by navigation id.
    pub timeline_navs: RefCell<Vec<uuid::Uuid>>,
    /// The load the reader picked, or `None` to follow the newest.
    pub timeline_selected: RefCell<Option<uuid::Uuid>>,
    /// What the Timeline draws, laid out on the last refresh.
    pub timeline_drawn: Rc<RefCell<Option<beacon_core::timeline::Timeline>>>,
    /// The engine's telemetry while the pane is open: the Timeline's stage bars.
    pub devtools_rx: RefCell<Option<tokio::sync::broadcast::Receiver<Arc<gosub_engine::telemetry::Event>>>>,
    /// The rows the network table is showing, in order. Held rather than rebuilt: a refresh
    /// writes new values into the objects already here, and the view updates the labels it
    /// has bound to them. Rows are only added or removed when the request list really
    /// changes, which is what keeps selection, scroll position and clicks intact.
    /// The debug log's rows, updated in place like the network table's.
    pub log_model: gtk4::gio::ListStore,
    pub network_model: gtk4::gio::ListStore,
    /// The selection the view drives, kept so a refresh can put the highlight back on the
    /// row the user chose.
    pub network_selection: gtk4::SingleSelection,
    /// The same guard for each detail pane, keyed by pane name. Without it a pane is rebuilt
    /// four times a second and jumps back to the top on every rebuild, so anything below the
    /// fold cannot be read at all.
    pub detail_rendered: RefCell<std::collections::HashMap<&'static str, String>>,

    /// The repeating refresh, live only while the pane is open. A closed pane costs nothing.
    pub devtools_tick: RefCell<Option<glib::SourceId>>,
    /// How tall the pane was when it was last closed, so reopening it comes back the size
    /// the user left it rather than snapping to a default they already rejected.
    pub devtools_height: Cell<i32>,
    #[template_child]
    pub statusbar: TemplateChild<gtk4::Label>,
    /// The activity strip over the page (View > Show Activity), its model, the telemetry
    /// subscription that feeds it while it shows, and the tick that redraws it.
    #[template_child]
    pub activity_strip: TemplateChild<gtk4::Label>,
    pub activity: RefCell<beacon_core::activity::Activity>,
    pub activity_rx: RefCell<Option<tokio::sync::broadcast::Receiver<Arc<gosub_engine::telemetry::Event>>>>,
    pub activity_tick: RefCell<Option<glib::SourceId>>,
    #[template_child]
    pub btn_downloads: TemplateChild<gtk4::MenuButton>,
    #[template_child]
    pub btn_bookmark: TemplateChild<Button>,
    #[template_child]
    pub bookmark_icon: TemplateChild<Image>,
    #[template_child]
    pub bookmarks_bar: TemplateChild<gtk4::Box>,
    /// Dark-mode toggle. Held so its state can track `gtk-application-prefer-dark-theme`,
    /// which the desktop's colour-scheme also writes (see `crate::theme`).
    #[template_child]
    pub btn_darkmode: TemplateChild<ToggleButton>,

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
    /// Browser state that reacts to the engine: the engine-tab mapping, the download
    /// list, MRU order and the closed-tab stack. Shares `tab_manager` above.
    pub beacon: Rc<RefCell<Beacon>>,
    /// Clicks awaiting the engine's hit-test answer: token → (tab, window point, what to
    /// do with the answer). Right-click builds a context menu; Ctrl/middle-click opens the
    /// link it lands on in a background tab.
    pub pending_hit_tests: RefCell<HashMap<u64, (TabId, Point, HitIntent)>>,
    /// Source of hit-test tokens.
    pub next_hit_test_token: Cell<u64>,

    /// Fires when the cycle has settled; commits the landed tab to the front of the list.
    pub cycle_timer: RefCell<Option<glib::SourceId>>,
    /// List widget inside the downloads popover (built in `constructed`).
    downloads_list: RefCell<Option<gtk4::ListBox>>,
    /// URL-bar completion popover and its list (built in `constructed`).
    completion: RefCell<Option<(Popover, gtk4::ListBox)>>,
    /// Per-tab page zoom, shared with each render area's input/draw closures.
    tab_zoom: RefCell<HashMap<TabId, Rc<Cell<f64>>>>,
    /// Whether this is a private-browsing window (ephemeral engine, no history recording).
    pub private: Cell<bool>,
    /// URLs another launch handed over before the startup tabs were open, when the content
    /// area may not have its size yet; they open after the startup tabs. `None` once those
    /// are open.
    pub startup_urls: RefCell<Option<Vec<String>>>,
    /// Whether the user wants the bookmarks bar. Per window, as on the Mac: fullscreen hides
    /// the whole chrome, and this says whether the bar comes back with it.
    pub bookmarks_bar_wanted: Cell<bool>,
    /// The bookmark each tab was last sent to from the bookmarks bar, as the store spells
    /// it. The engine reports the URL it parsed (`https://gosub.io/` for a bookmark saved as
    /// `https://gosub.io`), so the page's icon finds its bookmark through this instead.
    bookmark_opened: RefCell<HashMap<TabId, String>>,
    /// The picker a form control asked for, while it is up. Held so a second request closes
    /// the first rather than stacking windows over the page.
    picker: RefCell<Option<crate::picker::PickerHandle>>,
}

/// Write a row property, but only when the value actually changed.
///
/// glib does not compare: writing a property the value it already holds still emits `notify`,
/// and every `notify` re-runs the bindings hanging off that property. For a tooltip binding
/// that is fatal — GTK reads a `tooltip-text` write as a change and restarts its hover timer,
/// so with this pane refreshing four times a second no tooltip in the devtools ever lived long
/// enough to be shown. Verified against a standalone GTK4 program: identical hover, the
/// tooltip appears with a static text and never appears with the text rewritten on a 250ms
/// tick. `rows::tests` pins the glib half of that.
macro_rules! set_changed {
    ($row:expr, $get:ident, $set:ident, $value:expr) => {{
        let value = $value;
        if $row.$get() != value {
            $row.$set(value);
        }
    }};
}

impl Default for BrowserWindow {
    fn default() -> Self {
        let (tx, rx) = async_channel::unbounded::<Message>();
        // Beacon shares this manager rather than owning it: the window reads it directly to
        // build the tab strip, and both must see the same tabs.
        let tab_manager = Arc::new(Mutex::new(GosubTabManager::new()));
        Self {
            searchbar: TemplateChild::default(),
            btn_prev: TemplateChild::default(),
            btn_next: TemplateChild::default(),
            btn_refresh: TemplateChild::default(),
            headerbar: TemplateChild::default(),
            tab_strip: TemplateChild::default(),
            tab_scroller: TemplateChild::default(),
            tab_row: TemplateChild::default(),
            nav_toolbar: TemplateChild::default(),
            content_stack: TemplateChild::default(),
            content_paned: TemplateChild::default(),
            devtools_pane: TemplateChild::default(),
            devtools_stack: TemplateChild::default(),
            devtools_action: TemplateChild::default(),
            devtools_summary: TemplateChild::default(),
            devtools_filter: TemplateChild::default(),
            log_scroller: TemplateChild::default(),
            timings_scroller: TemplateChild::default(),
            timings_list: TemplateChild::default(),
            history_list: TemplateChild::default(),
            report_list: TemplateChild::default(),
            network_scroller: TemplateChild::default(),
            network_by_origin: TemplateChild::default(),
            network_view: TemplateChild::default(),
            origins_list: TemplateChild::default(),
            processes_list: TemplateChild::default(),
            timeline_nav: TemplateChild::default(),
            timeline_area: TemplateChild::default(),
            network_list: TemplateChild::default(),
            network_detail_stack: TemplateChild::default(),
            detail_raw: TemplateChild::default(),
            request_stack: TemplateChild::default(),
            response_stack: TemplateChild::default(),
            request_list: TemplateChild::default(),
            response_list: TemplateChild::default(),
            timing_list: TemplateChild::default(),
            sub_list: TemplateChild::default(),
            network_request_view: TemplateChild::default(),
            network_response_view: TemplateChild::default(),
            network_body_view: TemplateChild::default(),
            network_selected: RefCell::new(None),
            log_model: gtk4::gio::ListStore::new::<super::rows::LogRow>(),
            network_model: gtk4::gio::ListStore::new::<super::rows::RequestRow>(),
            network_selection: gtk4::SingleSelection::builder().autoselect(false).can_unselect(true).build(),
            timings_model: gtk4::gio::ListStore::new::<super::rows::TimingRow>(),
            history_model: gtk4::gio::ListStore::new::<super::rows::HistoryRow>(),
            report_model: gtk4::gio::ListStore::new::<super::rows::ReportRow>(),
            origins_model: gtk4::gio::ListStore::new::<super::rows::OriginRow>(),
            processes_model: gtk4::gio::ListStore::new::<super::rows::ProcessRow>(),
            timeline_navs: RefCell::new(Vec::new()),
            timeline_selected: RefCell::new(None),
            timeline_drawn: Rc::new(RefCell::new(None)),
            devtools_rx: RefCell::new(None),
            detail_rendered: RefCell::new(std::collections::HashMap::new()),
            devtools_tick: RefCell::new(None),
            devtools_height: Cell::new(260),
            log_list: TemplateChild::default(),
            statusbar: TemplateChild::default(),
            activity_strip: TemplateChild::default(),
            activity: RefCell::new(beacon_core::activity::Activity::default()),
            activity_rx: RefCell::new(None),
            activity_tick: RefCell::new(None),
            btn_downloads: TemplateChild::default(),
            btn_bookmark: TemplateChild::default(),
            bookmark_icon: TemplateChild::default(),
            bookmarks_bar: TemplateChild::default(),
            btn_darkmode: TemplateChild::default(),

            tab_manager: tab_manager.clone(),
            sender: Arc::new(tx),
            receiver: Arc::new(rx),

            engine: Rc::new(RefCell::new(None)),
            render_areas: Rc::new(RefCell::new(HashMap::new())),
            last_viewport: Rc::new(Cell::new(None)),
            beacon: Rc::new(RefCell::new(Beacon::new(
                tab_manager.clone(),
                runtime().handle().clone(),
                Rc::new(beacon_core::platform::NullPlatform),
            ))),
            pending_hit_tests: RefCell::new(HashMap::new()),
            next_hit_test_token: Cell::new(1),
            cycle_timer: RefCell::new(None),
            downloads_list: RefCell::new(None),
            completion: RefCell::new(None),
            tab_zoom: RefCell::new(HashMap::new()),
            private: Cell::new(false),
            startup_urls: RefCell::new(Some(Vec::new())),
            bookmarks_bar_wanted: Cell::new(true),
            bookmark_opened: RefCell::new(HashMap::new()),
            picker: RefCell::new(None),
        }
    }
}

/// What to do with a `HitTestResult` once the engine answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitIntent {
    /// Right-click: build the page context menu from the hit.
    ContextMenu,
    /// Ctrl+click or middle-click: open `link_url` in a background tab, if there is one.
    OpenLinkInBackgroundTab,
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
        // The window exists now, so the clipboard and file launcher have something to hang
        // off; until this point Beacon has been on the no-op platform.
        self.beacon
            .borrow_mut()
            .set_platform(Rc::new(crate::platform::GtkPlatform::new(&self.obj())));
        self.setup_tab_strip_sizing();
        self.setup_downloads_popover();
        self.setup_bookmark_button();
        self.setup_devtools();
        self.setup_url_completion();
        self.sync_dark_mode_button();
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
        self.view_source();
    }

    /// Open the active tab's source in a new tab beside it.
    pub(crate) fn view_source(&self) {
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

        // Source renders in its own tab next to this one, browser-style. The engine
        // does not expose the fetched bytes, so the source is re-fetched.
        let sender = self.get_sender();
        runtime().spawn(async move {
            let _ = sender
                .send(Message::OpenTabRight(tab_id, format!("view-source:{url}"), "View source".into()))
                .await;
        });
    }

    /// Mirror `gtk-application-prefer-dark-theme` onto the toolbar toggle, so the button
    /// reflects reality when the *desktop* changes the theme rather than the button.
    /// One-way: the button's own handler still writes the property, and writing the value it
    /// already has emits no notify, so this cannot loop.
    fn sync_dark_mode_button(&self) {
        let Some(settings) = Settings::default() else { return };
        settings
            .bind_property("gtk-application-prefer-dark-theme", &*self.btn_darkmode, "active")
            .sync_create()
            .build();
    }

    #[template_callback]
    fn handle_toggle_darkmode(&self, btn: &ToggleButton) {
        self.log("Toggling dark mode");

        info!(target: "gtk", "Toggle dark mode action triggered");
        let settings = Settings::default().expect("Failed to get default GtkSettings");
        settings.set_property("gtk-application-prefer-dark-theme", btn.is_active());
    }

    /// Home button: send the active tab to the configured homepage. Goes through
    /// `Message::LoadUrl`, the same path the address bar uses, so scheme inference and
    /// internal-page routing behave identically to typing the URL.
    #[template_callback]
    fn handle_home_clicked(&self, _btn: &Button) {
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let url = self.homepage();
        self.log(&format!("Home: {url}"));
        _ = self.get_sender().send_blocking(Message::LoadUrl(tab_id, url));
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
        let (loading, url, typed, handle) = {
            let manager = self.tab_manager.lock().unwrap();
            match manager.get_tab(tab_id) {
                Some(tab) => (
                    tab.is_loading(),
                    tab.url().clone(),
                    tab.typed_address().map(str::to_string),
                    tab.tab_handle(),
                ),
                None => return,
            }
        };

        if loading {
            if let Some(handle) = handle {
                runtime().spawn(async move {
                    let _ = handle.send(EngineTabCommand::CancelNavigation).await;
                });
            }
            // The engine cancels without an event, so the Timeline is told here.
            beacon_core::timeline::stopped(tab_id, now_ms());
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

        // An address that did not parse has no page to refetch; going to it again brings
        // its error page back.
        if let Some(typed) = typed {
            let _ = self.get_sender().send_blocking(Message::LoadUrl(tab_id, typed));
            return;
        }

        // The shell-rendered config page has nothing to reload.
        if Self::is_shell_rendered(&url) {
            return;
        }

        // The engine can't refetch a view-source: page (the shell built it); re-run the
        // source flow instead.
        if matches!(url.scheme(), "view-source" | "raw") {
            let _ = self.get_sender().send_blocking(Message::LoadUrl(tab_id, url.to_string()));
            return;
        }

        self.send_reload(tab_id, handle, false);
    }

    /// Reload `tab_id` past the cache: every subresource refetched, as Ctrl+Shift+R does in
    /// every other browser. Unlike the button's reload this never stops a load in progress --
    /// asking for a fresh copy of a page is not the same as asking it to stop arriving.
    pub(crate) fn reload_ignoring_cache(&self, tab_id: TabId) {
        let (url, typed, handle) = {
            let manager = self.tab_manager.lock().unwrap();
            match manager.get_tab(tab_id) {
                Some(tab) => (tab.url().clone(), tab.typed_address().map(str::to_string), tab.tab_handle()),
                None => return,
            }
        };
        if let Some(typed) = typed {
            let _ = self.get_sender().send_blocking(Message::LoadUrl(tab_id, typed));
            return;
        }
        // The same two pages the ordinary reload cannot refetch: one the shell drew, one the
        // shell built.
        if Self::is_shell_rendered(&url) {
            return;
        }
        if matches!(url.scheme(), "view-source" | "raw") {
            let _ = self.get_sender().send_blocking(Message::LoadUrl(tab_id, url.to_string()));
            return;
        }
        self.send_reload(tab_id, handle, true);
    }

    /// Mark the tab loading and ask the engine for the page again.
    fn send_reload(&self, tab_id: TabId, handle: Option<TabHandle>, ignore_cache: bool) {
        let mut manager = self.tab_manager.lock().unwrap();
        if let Some(mut tab) = manager.get_tab(tab_id) {
            tab.set_loading(true);
            // A reload leaves the error page; failing again flags it again.
            tab.set_showing_error(false);
            manager.update_tab(tab_id, &tab);
        }
        drop(manager);
        self.refresh_tabs();

        if let Some(handle) = handle {
            runtime().spawn(async move {
                let _ = handle.send(EngineTabCommand::Reload { ignore_cache }).await;
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
        // Keys go to the page once the address is entered, as in other browsers, so typing
        // reaches a field the page autofocuses.
        if let Some(area) = self.render_areas.borrow().get(&tab_id) {
            area.grab_focus();
        }
        self.sender.send(Message::LoadUrl(tab_id, url_str)).await.unwrap();
    }
}

/// One row of the request list, worked out before any widget exists so the list can be
/// compared against what is already on screen and left alone when nothing has changed.
struct NetworkRow {
    id: uuid::Uuid,
    status: String,
    method: String,
    kind: String,
    size: String,
    time: String,
    url: String,
    /// The CSS classes this row's cells carry.
    css: &'static str,
    /// `(started, headers arrived, ended)` in epoch milliseconds, for the waterfall bar.
    span: (u64, Option<u64>, u64),
}

impl NetworkRow {
    /// The bar as fractions of the page's whole load: where it starts, how much of it was
    /// waiting for the server, and how much was the body arriving.
    fn bar(&self, window_start: u64, span: f64) -> (f64, f64, f64) {
        let (start, headers, end) = self.span;
        let wait_end = headers.unwrap_or(end).clamp(start, end);
        (
            start.saturating_sub(window_start) as f64 / span,
            wait_end.saturating_sub(start) as f64 / span,
            end.saturating_sub(wait_end) as f64 / span,
        )
    }
}

impl BrowserWindow {
    /// The window's own running commentary. It goes into the same buffer the engine's
    /// records do, so the console is one list rather than two views that disagree about
    /// what happened and in what order.
    pub fn log(&self, message: &str) {
        beacon_core::devtools::record(log::Level::Info, "beacon", message);
    }

    // ── developer pane ────────────────────────────────────────────────────

    /// Show or hide the pane, starting and stopping its refresh with it.
    pub(crate) fn toggle_devtools(&self) {
        let showing = !self.devtools_pane.get_visible();

        // Remember the height on the way out. Read before hiding: once the child is gone the
        // paned position no longer means anything.
        if !showing {
            let height = self.content_paned.height() - self.content_paned.position();
            if height > 0 {
                self.devtools_height.set(height);
            }
        }

        self.devtools_pane.set_visible(showing);

        // Bodies are only copied while someone can look at them. This is the switch that
        // keeps the network panel free for every page that is not being inspected.
        beacon_core::devtools::set_capture_bodies(showing);
        // Same window for the header values the engine redacts by default: a request view
        // that will not show you your own `Cookie` cannot answer the question it is usually
        // opened to answer.
        beacon_core::devtools::set_show_sensitive_headers(showing);

        if let Some(source) = self.devtools_tick.borrow_mut().take() {
            source.remove();
        }
        // Listening is what makes the engine announce its stages, so only while the pane is
        // open, like body capture.
        *self.devtools_rx.borrow_mut() = showing.then(gosub_engine::telemetry::subscribe);
        if !showing {
            return;
        }

        // The paned position is measured from the top, so the pane's height is whatever is
        // left below it. Deferred: at this moment the paned may not have been allocated yet
        // -- on the very first toggle its height is still 0, and a position computed from
        // that puts the divider at the top of the window.
        let paned = self.content_paned.get();
        let wanted = self.devtools_height.get();
        glib::idle_add_local_once(move || {
            let available = paned.height();
            if available > 0 {
                paned.set_position((available - wanted).max(0));
            }
        });

        self.refresh_devtools();
        let window = self.obj().clone();
        // Four times a second: fast enough to watch a page load, slow enough that the pane
        // is not the reason the page loads slowly.
        let source = glib::timeout_add_local(std::time::Duration::from_millis(250), move || {
            window.imp().refresh_devtools();
            glib::ControlFlow::Continue
        });
        *self.devtools_tick.borrow_mut() = Some(source);
    }

    /// Show or hide the activity strip, and answer whether it is now showing.
    ///
    /// Showing it subscribes to the engine's telemetry bus, which is what makes the
    /// engine announce its stages at all; hiding drops the subscription, so a page
    /// browsed without the strip costs nothing extra.
    pub(crate) fn toggle_activity(&self) -> bool {
        let showing = !self.activity_strip.get_visible();
        if let Some(source) = self.activity_tick.borrow_mut().take() {
            source.remove();
        }
        if !showing {
            self.activity_strip.set_visible(false);
            *self.activity_rx.borrow_mut() = None;
            *self.activity.borrow_mut() = beacon_core::activity::Activity::default();
            return false;
        }
        *self.activity_rx.borrow_mut() = Some(gosub_engine::telemetry::subscribe());
        self.activity_strip.set_text("");
        self.activity_strip.set_visible(true);
        self.refresh_activity();
        let window = self.obj().clone();
        // Ten times a second: the clocks read in tenths, and a stage shorter than that is
        // not what anyone is watching for.
        let source = glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            window.imp().refresh_activity();
            glib::ControlFlow::Continue
        });
        *self.activity_tick.borrow_mut() = Some(source);
        true
    }

    /// One tick of the activity strip: drain what the engine said since the last one,
    /// bring the network lines in step with the request log, and redraw.
    fn refresh_activity(&self) {
        let now = std::time::Instant::now();
        let mut activity = self.activity.borrow_mut();
        if let Some(rx) = self.activity_rx.borrow_mut().as_mut() {
            loop {
                match rx.try_recv() {
                    Ok(event) => activity.on_telemetry(&event.kind, &event.data, now),
                    Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        }
        let tab = self.beacon.borrow().active();
        activity.sync_requests(&beacon_core::devtools::requests(tab), now);
        self.activity_strip.set_text(&activity.lines(now).join("\n"));
    }

    /// Show or hide the bookmarks bar, and answer whether it is now showing.
    pub(crate) fn toggle_bookmarks_bar(&self) -> bool {
        let wanted = !self.bookmarks_bar_wanted.get();
        self.bookmarks_bar_wanted.set(wanted);
        // Fullscreen owns the chrome while it lasts; this only decides what returns with it.
        if !self.obj().is_fullscreen() {
            self.bookmarks_bar.set_visible(wanted);
        }
        wanted
    }

    /// Open the developer pane on a particular tab, for the menu items named after them.
    /// Shows the pane when it is closed: an item called "Network" should land on the network
    /// tab, never close the panel.
    pub(crate) fn show_devtools_page(&self, page: &str) {
        if !self.devtools_pane.get_visible() {
            self.toggle_devtools();
        }
        self.devtools_stack.set_visible_child_name(page);
    }

    /// Throw away the engine's timing statistics and redraw the table.
    pub(crate) fn reset_timings(&self) {
        beacon_core::devtools::reset_timings();
        self.refresh_devtools();
    }

    /// Wire the pane's own controls. Called once, at construction.
    pub(crate) fn setup_devtools(&self) {
        let window = self.obj().clone();
        self.devtools_action.connect_clicked(move |_| {
            let imp = window.imp();
            match imp.devtools_page().as_str() {
                "timings" => beacon_core::devtools::reset_timings(),
                "network" => {
                    beacon_core::devtools::clear_requests();
                    imp.network_selected.replace(None);
                }
                _ => beacon_core::devtools::clear_logs(),
            }
            imp.refresh_devtools();
        });

        // With the page area shrinkable there is nothing stopping a drag from closing it
        // entirely, and a browser whose page is zero pixels tall looks broken rather than
        // resized. Clamping here rather than through `shrink-start-child` keeps the floor a
        // number we chose. Re-setting the same position is a no-op, so this cannot recurse.
        self.content_paned.connect_position_notify(|paned| {
            const MIN_PAGE: i32 = 120;
            if paned.position() < MIN_PAGE && paned.height() > MIN_PAGE {
                paned.set_position(MIN_PAGE);
            }
        });

        // Raw or formatted, for both the request and the response at once: a developer
        // reading one wire dump wants the other in the same shape.
        let window = self.obj().clone();
        self.detail_raw.connect_toggled(move |button| {
            let imp = window.imp();
            let page = if button.is_active() { "raw" } else { "formatted" };
            imp.request_stack.set_visible_child_name(page);
            imp.response_stack.set_visible_child_name(page);
        });

        self.build_network_columns();
        self.build_log_columns();
        self.build_timings_columns();
        self.build_history_columns();
        self.build_report_columns();
        self.build_origins_columns();
        // The summary is cut to fit the header; the whole line is a hover away.
        self.devtools_summary
            .bind_property("label", &*self.devtools_summary, "tooltip-text")
            .sync_create()
            .build();
        self.build_processes_columns();
        self.setup_timeline();

        let window = self.obj().clone();
        self.network_by_origin.connect_toggled(move |toggle| {
            let imp = window.imp();
            imp.network_view
                .set_visible_child_name(if toggle.is_active() { "origins" } else { "requests" });
            imp.refresh_devtools();
        });

        // Selecting a request shows it in full. The row object carries the id, so a refresh
        // that reorders or trims the list cannot leave the detail pane on the wrong request.
        let window = self.obj().clone();
        self.network_selection.connect_selected_item_notify(move |selection| {
            let imp = window.imp();
            let id = selection
                .selected_item()
                .and_downcast::<super::rows::RequestRow>()
                .and_then(|row| row.request_id());
            imp.network_selected.replace(id);
            imp.show_request_detail();
        });

        let window = self.obj().clone();
        self.devtools_filter.connect_search_changed(move |_| {
            window.imp().refresh_devtools();
        });

        // The action button means different things on the two pages, so it has to follow
        // whichever is showing.
        let window = self.obj().clone();
        self.devtools_stack.connect_visible_child_notify(move |_| {
            let imp = window.imp();
            let (label, tooltip) = match imp.devtools_page().as_str() {
                "timings" => (
                    "Reset",
                    "Start timing again from nothing, so the next navigation is measured on its own",
                ),
                "network" => ("Clear", "Forget the requests recorded so far"),
                _ => ("Clear", "Discard the captured log records"),
            };
            imp.devtools_action.set_label(label);
            imp.devtools_action.set_tooltip_text(Some(tooltip));
            // Nothing to clear or filter on a page with nothing on it. History has content but
            // neither: the engine owns it, and a filter would cut the branches it is drawn from.
            // The Page report filters, but is the engine's account of the page, not a log to
            // clear: it is replaced by the next navigation's.
            let page = imp.devtools_page();
            let has_content = !matches!(page.as_str(), "console" | "history");
            // Nor is the process list, which is what is running, not what was recorded.
            // The Timeline keeps the last few loads on its own and has a picker instead.
            imp.devtools_action
                .set_visible(has_content && !matches!(page.as_str(), "report" | "processes" | "timeline"));
            imp.devtools_filter.set_visible(has_content);
            imp.network_by_origin.set_visible(page == "network");
            imp.timeline_nav.set_visible(page == "timeline");
            imp.refresh_devtools();
        });
    }

    fn devtools_page(&self) -> String {
        self.devtools_stack
            .visible_child_name()
            .map(|name| name.to_string())
            .unwrap_or_else(|| "log".to_string())
    }

    fn refresh_devtools(&self) {
        if !self.devtools_pane.get_visible() {
            return;
        }
        // Whatever page is showing: a load watched from the Network tab still wants its
        // stages on the Timeline afterwards.
        if let Some(rx) = self.devtools_rx.borrow_mut().as_mut() {
            loop {
                match rx.try_recv() {
                    Ok(event) => beacon_core::timeline::on_telemetry(&event.kind, &event.data, event.ts_us),
                    Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        }
        let needle = self.devtools_filter.text().to_lowercase();
        match self.devtools_page().as_str() {
            "timings" => self.refresh_timings(&needle),
            "network" if self.network_by_origin.is_active() => self.refresh_origins(&needle),
            "network" => self.refresh_network(&needle),
            "history" => self.refresh_history(),
            "processes" => self.refresh_processes(&needle),
            "timeline" => self.refresh_timeline(&needle),
            "report" => self.refresh_report(&needle),
            // The page's own console, which has nothing to say until scripts run. Its
            // summary line would otherwise keep whatever the last page put there.
            "console" => self.devtools_summary.set_text(""),
            _ => self.refresh_log(&needle),
        }
    }

    fn refresh_log(&self, needle: &str) {
        let lines = beacon_core::devtools::log_snapshot(2000);
        let total = lines.len();

        let rows: Vec<_> = lines
            .iter()
            .filter(|line| needle.is_empty() || line.message.to_lowercase().contains(needle) || line.target.to_lowercase().contains(needle))
            .collect();
        self.devtools_summary.set_text(&format!("{} of {total} records", rows.len()));

        // Following the tail is the reason to watch a log; reading one is the reason to stop
        // following. Decided before the rows change, so a record arriving mid-read does not
        // yank the view away.
        let adjustment = self.log_scroller.vadjustment();
        let at_bottom = adjustment.value() + adjustment.page_size() >= adjustment.upper() - 8.0;

        for (index, line) in rows.iter().enumerate() {
            let level = line.level.to_string();
            // Warnings and errors carry their own class, which is the whole reason a row
            // knows about CSS at all.
            let css = match line.level {
                log::Level::Error => "monospace error",
                log::Level::Warn => "monospace warn",
                _ => "monospace",
            };
            match self.log_model.item(index as u32).and_downcast::<super::rows::LogRow>() {
                Some(item) => {
                    set_changed!(item, time, set_time, format_clock(line.timestamp_ms));
                    set_changed!(item, level, set_level, level.as_str());
                    set_changed!(item, target, set_target, line.target.as_str());
                    set_changed!(item, message, set_message, line.message.as_str());
                    item.set_css(css);
                }
                None => {
                    let item = super::rows::LogRow::default();
                    item.set_time(format_clock(line.timestamp_ms));
                    item.set_level(level.as_str());
                    item.set_target(line.target.as_str());
                    item.set_message(line.message.as_str());
                    item.set_css(css);
                    self.log_model.append(&item);
                }
            }
        }
        // The buffer is a ring: once it is full the oldest records fall off the front, and
        // the rows shift under whatever is on screen.
        let wanted = rows.len() as u32;
        if self.log_model.n_items() > wanted {
            self.log_model
                .splice(wanted, self.log_model.n_items() - wanted, &[] as &[super::rows::LogRow]);
        }

        if at_bottom && wanted > 0 {
            self.log_list.scroll_to(wanted - 1, None, gtk4::ListScrollFlags::empty(), None);
        }
    }

    fn refresh_timings(&self, needle: &str) {
        use beacon_core::devtools::format_duration;

        let all = beacon_core::devtools::timings();
        let total: u64 = all.iter().map(|row| row.total_us).sum();
        self.devtools_summary
            .set_text(&format!("{} namespaces · {} total", all.len(), format_duration(total)));

        let rows: Vec<_> = all
            .iter()
            .filter(|row| needle.is_empty() || row.namespace.to_lowercase().contains(needle))
            .collect();

        for (index, row) in rows.iter().enumerate() {
            let item = match self.timings_model.item(index as u32).and_downcast::<super::rows::TimingRow>() {
                Some(item) => item,
                None => {
                    let item = super::rows::TimingRow::default();
                    self.timings_model.append(&item);
                    item
                }
            };
            set_changed!(item, namespace, set_namespace, row.namespace.as_str());
            // A namespace the engine knows explains itself; one it does not is at least
            // readable in full on hover.
            set_changed!(
                item,
                description,
                set_description,
                match row.timing {
                    Some(timing) => timing.describes().to_string(),
                    None => row.namespace.clone(),
                }
            );
            set_changed!(item, explained, set_explained, row.timing.is_some());
            set_changed!(item, count, set_count, row.count.to_string());
            set_changed!(item, total, set_total, format_duration(row.total_us));
            set_changed!(item, avg, set_avg, format_duration(row.avg_us));
            set_changed!(item, p50, set_p50, format_duration(row.p50_us));
            set_changed!(item, p95, set_p95, format_duration(row.p95_us));
            set_changed!(item, peak, set_peak, format_duration(row.max_us));
            set_changed!(item, css, set_css, "monospace");
        }
        let wanted = rows.len() as u32;
        if self.timings_model.n_items() > wanted {
            self.timings_model
                .splice(wanted, self.timings_model.n_items() - wanted, &[] as &[super::rows::TimingRow]);
        }
    }

    /// The timings table's columns. The namespace is the only one whose width is anyone's
    /// guess; the rest hold a duration.
    fn build_timings_columns(&self) {
        self.timings_list
            .set_model(Some(&gtk4::NoSelection::new(Some(self.timings_model.clone()))));
        for (title, property, width, xalign) in [
            ("Namespace", "namespace", None, 0.0),
            ("Count", "count", Some(64), 1.0),
            ("Total", "total", Some(88), 1.0),
            ("Avg", "avg", Some(88), 1.0),
            ("p50", "p50", Some(88), 1.0),
            ("p95", "p95", Some(88), 1.0),
            ("Max", "peak", Some(88), 1.0),
        ] {
            let column = match property {
                "namespace" => Self::namespace_column(title),
                _ => Self::text_column(title, property, width, xalign),
            };
            self.timings_list.append_column(&column);
        }
    }

    // ── page report ───────────────────────────────────────────────────────

    /// The Page report's columns: what kind of problem, what it was, where, how often.
    fn build_report_columns(&self) {
        self.report_list
            .set_model(Some(&gtk4::NoSelection::new(Some(self.report_model.clone()))));
        for (title, property, width, xalign) in [
            ("", "section", Some(56), 0.0),
            ("Kind", "kind", Some(170), 0.0),
            ("What", "subject", None, 0.0),
            ("Where", "location", Some(200), 0.0),
            ("Count", "count", Some(64), 1.0),
        ] {
            let column = Self::text_column(title, property, width, xalign);
            if property == "subject" {
                column.set_expand(true);
            }
            column.set_resizable(true);
            self.report_list.append_column(&column);
        }
    }

    /// The active tab's report, filtered by kind, subject or location.
    fn refresh_report(&self, needle: &str) {
        use beacon_core::page_report::{self, Report};

        let rows = match self.active_tab_id().and_then(page_report::report) {
            Some(Report::Ready(diagnostics)) => {
                self.devtools_summary.set_text(&page_report::summary(&diagnostics));
                page_report::rows(&diagnostics)
            }
            Some(Report::Waiting) => {
                self.devtools_summary.set_text("Waiting for the page's first frame");
                Vec::new()
            }
            None => {
                self.devtools_summary.set_text("No page loaded");
                Vec::new()
            }
        };
        let rows: Vec<_> = rows
            .iter()
            .filter(|row| {
                needle.is_empty()
                    || row.kind.to_lowercase().contains(needle)
                    || row.subject.to_lowercase().contains(needle)
                    || row.location.to_lowercase().contains(needle)
            })
            .collect();

        for (index, row) in rows.iter().enumerate() {
            let item = match self.report_model.item(index as u32).and_downcast::<super::rows::ReportRow>() {
                Some(item) => item,
                None => {
                    let item = super::rows::ReportRow::default();
                    self.report_model.append(&item);
                    item
                }
            };
            set_changed!(item, section, set_section, row.section.label());
            set_changed!(item, kind, set_kind, row.kind);
            set_changed!(item, subject, set_subject, row.subject.as_str());
            set_changed!(item, location, set_location, row.location.as_str());
            set_changed!(item, count, set_count, row.count.to_string());
            set_changed!(item, hint, set_hint, row.hint);
            set_changed!(item, css, set_css, "monospace");
        }
        let wanted = rows.len() as u32;
        if self.report_model.n_items() > wanted {
            self.report_model
                .splice(wanted, self.report_model.n_items() - wanted, &[] as &[super::rows::ReportRow]);
        }
    }

    // ── history ───────────────────────────────────────────────────────────

    /// The history page's columns, and the click that jumps.
    fn build_history_columns(&self) {
        self.history_list
            .set_model(Some(&gtk4::NoSelection::new(Some(self.history_model.clone()))));
        let page = Self::history_page_column("Page");
        page.set_fixed_width(320);
        page.set_resizable(true);
        self.history_list.append_column(&page);
        self.history_list.append_column(&Self::text_column("URL", "url", None, 0.0));

        // One click, not a double: a line here is a place to go, the way a bookmark is. The
        // row hands over the entry id, never its position, which a refresh may have moved.
        let window = self.obj().clone();
        self.history_list.connect_activate(move |_, position| {
            let imp = window.imp();
            let Some(row) = imp.history_model.item(position).and_downcast::<super::rows::HistoryRow>() else {
                return;
            };
            imp.go_to_history_entry(HistoryEntryId(row.entry() as usize));
        });
    }

    fn build_origins_columns(&self) {
        self.origins_list
            .set_model(Some(&gtk4::NoSelection::new(Some(self.origins_model.clone()))));
        for (title, property, width, xalign) in [
            ("Origin", "origin", Some(220), 0.0),
            ("Party", "party", Some(56), 0.0),
            ("Requests", "requests", Some(72), 1.0),
            ("Size", "size", Some(72), 1.0),
            ("Kinds", "kinds", None, 0.0),
        ] {
            self.origins_list.append_column(&Self::text_column(title, property, width, xalign));
        }

        // An origin opens into its requests: the request list, filtered to it. A tree that
        // expands in place would say the same with a great deal more machinery.
        let window = self.obj().clone();
        self.origins_list.connect_activate(move |_, position| {
            let imp = window.imp();
            let Some(row) = imp.origins_model.item(position).and_downcast::<super::rows::OriginRow>() else {
                return;
            };
            imp.devtools_filter.set_text(&row.origin());
            imp.network_by_origin.set_active(false);
        });
    }

    fn refresh_origins(&self, needle: &str) {
        use beacon_core::devtools::format_bytes;

        let requests = beacon_core::devtools::requests(self.active_tab_id());
        let all = beacon_core::origins::by_origin(&requests);
        let third = all.iter().filter(|row| !row.first_party).count();
        self.devtools_summary
            .set_text(&format!("{} origins, {third} third party · {} requests", all.len(), requests.len()));

        let rows: Vec<_> = all
            .iter()
            .filter(|row| needle.is_empty() || row.origin.to_lowercase().contains(needle))
            .collect();
        for (index, row) in rows.iter().enumerate() {
            let item = match self.origins_model.item(index as u32).and_downcast::<super::rows::OriginRow>() {
                Some(item) => item,
                None => {
                    let item = super::rows::OriginRow::default();
                    self.origins_model.append(&item);
                    item
                }
            };
            let requests = match row.failed {
                0 => row.requests.to_string(),
                failed => format!("{} ({failed} failed)", row.requests),
            };
            set_changed!(item, origin, set_origin, row.origin.as_str());
            set_changed!(item, party, set_party, if row.first_party { "first" } else { "third" });
            set_changed!(item, requests, set_requests, requests);
            set_changed!(item, size, set_size, format_bytes(row.bytes));
            set_changed!(item, kinds, set_kinds, beacon_core::origins::format_kinds(&row.kinds));
            set_changed!(item, css, set_css, "monospace");
        }
        let wanted = rows.len() as u32;
        if self.origins_model.n_items() > wanted {
            self.origins_model
                .splice(wanted, self.origins_model.n_items() - wanted, &[] as &[super::rows::OriginRow]);
        }
    }

    /// Height of one timeline lane, and of the axis above them.
    const TIMELINE_LANE: i32 = 16;
    /// Rows above the lanes: the axis, and two for the marks' labels, so two marks close
    /// together can both be read.
    const TIMELINE_HEADER: i32 = 3 * Self::TIMELINE_LANE;
    /// Width of the lane labels left of the bars.
    const TIMELINE_LABELS: f64 = 220.0;

    fn setup_timeline(&self) {
        self.timeline_nav.set_model(Some(&gtk4::StringList::new(&[])));
        let window = self.obj().clone();
        self.timeline_nav.connect_selected_notify(move |dropdown| {
            let imp = window.imp();
            let navs = imp.timeline_navs.borrow().clone();
            let index = dropdown.selected() as usize;
            // Picking the newest means following whatever is newest next.
            let picked = navs.get(index).copied().filter(|_| index + 1 < navs.len());
            if *imp.timeline_selected.borrow() != picked {
                imp.timeline_selected.replace(picked);
                imp.refresh_devtools();
            }
        });

        let drawn = self.timeline_drawn.clone();
        self.timeline_area.set_draw_func(move |area, cr, width, _| {
            if let Some(timeline) = drawn.borrow().as_ref() {
                Self::draw_timeline(area, cr, width, timeline);
            }
        });
        let drawn = self.timeline_drawn.clone();
        self.timeline_area.connect_query_tooltip(move |_, _, y, _, tooltip| {
            let drawn = drawn.borrow();
            let Some(timeline) = drawn.as_ref() else { return false };
            let lane = (y - Self::TIMELINE_HEADER) / Self::TIMELINE_LANE;
            let Some(lane) = (y >= Self::TIMELINE_HEADER).then(|| timeline.lanes.get(lane as usize)).flatten() else {
                return false;
            };
            tooltip.set_text(Some(&lane.detail));
            true
        });
    }

    fn refresh_timeline(&self, needle: &str) {
        use beacon_core::devtools::format_duration;

        let tab = self.active_tab_id();
        let navs = tab.map(beacon_core::timeline::loads).unwrap_or_default();

        // The picker's entries change only when the loads do, so a choice survives refreshes.
        let ids: Vec<uuid::Uuid> = navs.iter().map(|n| n.id).collect();
        if *self.timeline_navs.borrow() != ids {
            let labels: Vec<String> = navs
                .iter()
                .map(|n| format!("{} {}", format_clock(n.started_ms), truncate(&n.url, 40)))
                .collect();
            let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
            self.timeline_navs.replace(ids.clone());
            if !ids.contains(&self.timeline_selected.borrow().unwrap_or_default()) {
                self.timeline_selected.replace(None);
            }
            let selected = self.timeline_selected.borrow().and_then(|id| ids.iter().position(|n| *n == id));
            if let Some(model) = self.timeline_nav.model().and_downcast::<gtk4::StringList>() {
                model.splice(0, model.n_items(), &labels);
            }
            self.timeline_nav
                .set_selected(selected.unwrap_or(ids.len().saturating_sub(1)) as u32);
        }

        let index = self
            .timeline_selected
            .borrow()
            .and_then(|id| navs.iter().position(|n| n.id == id))
            .or(navs.len().checked_sub(1));
        let Some(index) = index else {
            self.devtools_summary.set_text("No page loads in this tab yet");
            self.timeline_drawn.replace(None);
            self.timeline_area.set_content_height(0);
            self.timeline_area.queue_draw();
            return;
        };
        // Only the load on screen is copied out with its stages.
        let Some(nav) = tab.and_then(|tab| beacon_core::timeline::navigation(tab, navs[index].id)) else {
            return;
        };
        let requests = beacon_core::devtools::requests(tab);
        let next = navs.get(index + 1).map(|n| n.started_ms);
        let mut timeline = beacon_core::timeline::layout(&nav, &requests, next, now_ms());
        if !needle.is_empty() {
            timeline
                .lanes
                .retain(|lane| lane.label.to_lowercase().contains(needle) || lane.detail.to_lowercase().contains(needle));
        }

        let stages = nav.stages.len();
        let mut summary = format!(
            "{} · {} requests, {stages} stages",
            format_duration(((timeline.end_ms - timeline.start_ms) * 1000.0) as u64),
            timeline
                .lanes
                .iter()
                .filter(|l| l.kind != beacon_core::timeline::LaneKind::Stage)
                .count(),
        );
        if let Some(outcome) = &nav.outcome {
            summary.push_str(&format!(" · {outcome}"));
        }
        if stages == 0 && nav.ended_ms.is_some() {
            // Out of process, the page's stages are timed in the renderer, which does not put
            // them on this process's bus.
            let isolated = self.engine.borrow().as_ref().is_some_and(|engine| !engine.renderers().is_empty());
            summary.push_str(if isolated {
                " · page stages run in the renderer process and are not reported here"
            } else {
                " · stages are recorded only while this pane is open"
            });
        }
        self.devtools_summary.set_text(&summary);

        let height = Self::TIMELINE_HEADER + timeline.lanes.len() as i32 * Self::TIMELINE_LANE + 4;
        self.timeline_drawn.replace(Some(timeline));
        self.timeline_area.set_content_height(height);
        self.timeline_area.queue_draw();
    }

    /// One load: an axis along the top, a lane per request and per stage, and each mark as a
    /// line through all of them.
    fn draw_timeline(area: &gtk4::DrawingArea, cr: &gtk4::cairo::Context, width: i32, timeline: &beacon_core::timeline::Timeline) {
        use beacon_core::timeline::LaneKind;

        let ink = area.color();
        let (r, g, b) = (ink.red() as f64, ink.green() as f64, ink.blue() as f64);
        let lane_h = Self::TIMELINE_LANE as f64;
        let left = Self::TIMELINE_LABELS;
        let span_w = (width as f64 - left - 8.0).max(1.0);
        let span = (timeline.end_ms - timeline.start_ms).max(1.0);
        let x_of = |ms: f64| left + (ms - timeline.start_ms) / span * span_w;
        let header = Self::TIMELINE_HEADER as f64;
        let bottom = header + lane_h * timeline.lanes.len() as f64;
        let now = now_ms() as f64;

        cr.select_font_face("monospace", gtk4::cairo::FontSlant::Normal, gtk4::cairo::FontWeight::Normal);
        cr.set_font_size(10.0);
        // Labels cut to the column, at the width of one monospace character at this size.
        let label_chars = ((left - 8.0) / 6.0) as usize;
        let text = |text: &str, x: f64, y: f64| {
            cr.move_to(x, y + 10.0);
            let _ = cr.show_text(text);
        };

        // The axis: a tick every round number of milliseconds that leaves room for its label.
        let step = [
            1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0, 30_000.0,
        ]
        .into_iter()
        .find(|step| span / step <= span_w / 70.0)
        .unwrap_or(60_000.0);
        let mut tick = 0.0;
        while tick <= span {
            let x = x_of(timeline.start_ms + tick);
            cr.set_source_rgba(r, g, b, 0.10);
            cr.rectangle(x, lane_h, 1.0, bottom - lane_h);
            let _ = cr.fill();
            cr.set_source_rgba(r, g, b, 0.6);
            text(
                &if tick >= 1000.0 {
                    format!("{:.1}s", tick / 1000.0)
                } else {
                    format!("{tick}ms")
                },
                x + 2.0,
                1.0,
            );
            tick += step;
        }

        for (index, lane) in timeline.lanes.iter().enumerate() {
            let y = header + lane_h * index as f64;
            cr.set_source_rgba(r, g, b, 0.85);
            text(&truncate(&lane.label, label_chars), 4.0, y + 1.0);

            let bar_h = lane_h - 6.0;
            for (start, end) in &lane.spans {
                let x = x_of(*start);
                let end_x = x_of(end.unwrap_or(now).min(timeline.end_ms));
                match lane.kind {
                    LaneKind::Request { headers_ms, failed } => {
                        // Waiting for the server in the ink colour, the body in blue: the
                        // network table's waterfall, on this clock.
                        let split = headers_ms.map(x_of).unwrap_or(end_x).clamp(x, end_x);
                        cr.set_source_rgba(r, g, b, 0.30);
                        cr.rectangle(x, y + 3.0, (split - x).max(0.0), bar_h);
                        let _ = cr.fill();
                        if failed {
                            cr.set_source_rgba(0.85, 0.25, 0.20, 0.85);
                        } else {
                            cr.set_source_rgba(0.20, 0.45, 0.85, 0.85);
                        }
                        cr.rectangle(split, y + 3.0, (end_x - split).max(1.0), bar_h);
                        let _ = cr.fill();
                    }
                    LaneKind::Stage => {
                        cr.set_source_rgba(0.85, 0.55, 0.15, 0.85);
                        cr.rectangle(x, y + 3.0, (end_x - x).max(1.0), bar_h);
                        let _ = cr.fill();
                    }
                }
            }
        }

        // Marks last, over everything, each labelled at the top of its line: on the first label
        // row, or the second when the first is taken there.
        let mut row_ends = [f64::MIN; 2];
        for mark in &timeline.marks {
            let x = x_of(mark.at_ms);
            let (mr, mg, mb) = match mark.label {
                "first paint" => (0.20, 0.65, 0.30),
                "finished" | "stopped" => (0.20, 0.45, 0.85),
                "first bytes" => (r, g, b),
                _ => (0.60, 0.35, 0.75),
            };
            cr.set_source_rgba(mr, mg, mb, 0.9);
            cr.rectangle(x, lane_h, 1.5, bottom - lane_h);
            let _ = cr.fill();
            // Left of its line when the right edge would cut it off.
            let label_w = cr.text_extents(mark.label).map_or(0.0, |extents| extents.x_advance());
            let label_x = if x + 3.0 + label_w > width as f64 {
                x - 3.0 - label_w
            } else {
                x + 3.0
            };
            // Two marks within a few pixels of each other are one line to the eye; a third
            // label there would only print over the other two.
            if let Some(row) = row_ends.iter().position(|end| label_x > end + 4.0) {
                row_ends[row] = label_x + label_w;
                text(mark.label, label_x, lane_h * (row as f64 + 1.0) + 1.0);
            }
        }
    }

    fn build_processes_columns(&self) {
        self.processes_list
            .set_model(Some(&gtk4::NoSelection::new(Some(self.processes_model.clone()))));
        for (title, property, width, xalign) in [
            ("Process", "name", Some(260), 0.0),
            ("PID", "pid", Some(72), 1.0),
            ("Memory", "memory", Some(88), 1.0),
            ("Uptime", "uptime", Some(80), 1.0),
            ("Serves", "detail", None, 0.0),
        ] {
            self.processes_list
                .append_column(&Self::text_column(title, property, width, xalign));
        }
    }

    fn refresh_processes(&self, needle: &str) {
        use beacon_core::processes::format_uptime;

        let (renderers, tier) = match self.engine.borrow().as_ref() {
            Some(engine) => (engine.renderers(), engine.renderer_tier()),
            None => (Vec::new(), None),
        };
        let all = beacon_core::processes::tree(&renderers, tier.as_deref());
        let total_kb: u64 = all.iter().filter_map(|p| p.rss_kb).sum();
        self.devtools_summary.set_text(&format!(
            "{} processes · {} resident",
            all.len(),
            beacon_core::devtools::format_bytes(total_kb * 1024)
        ));

        // A filter keeps a matching process's place in the tree, so its indent still says
        // what it hangs under.
        let rows: Vec<_> = all
            .iter()
            .filter(|p| needle.is_empty() || p.name.to_lowercase().contains(needle) || p.detail.to_lowercase().contains(needle))
            .collect();
        for (index, process) in rows.iter().enumerate() {
            let item = match self.processes_model.item(index as u32).and_downcast::<super::rows::ProcessRow>() {
                Some(item) => item,
                None => {
                    let item = super::rows::ProcessRow::default();
                    self.processes_model.append(&item);
                    item
                }
            };
            let name = match process.depth {
                0 => process.name.clone(),
                depth => format!("{}└ {}", "   ".repeat(depth - 1), process.name),
            };
            let memory = process
                .rss_kb
                .map(|kb| beacon_core::devtools::format_bytes(kb * 1024))
                .unwrap_or_else(|| "—".to_string());
            set_changed!(item, name, set_name, name);
            set_changed!(item, pid, set_pid, process.pid.to_string());
            set_changed!(item, memory, set_memory, memory);
            set_changed!(item, uptime, set_uptime, process.uptime_s.map(format_uptime).unwrap_or_default());
            set_changed!(item, detail, set_detail, process.detail.as_str());
            set_changed!(item, css, set_css, "monospace");
        }
        let wanted = rows.len() as u32;
        if self.processes_model.n_items() > wanted {
            self.processes_model
                .splice(wanted, self.processes_model.n_items() - wanted, &[] as &[super::rows::ProcessRow]);
        }
    }

    /// The tree column: indented by the row's branch depth, a mark for where the tab is.
    fn history_page_column(title: &str) -> gtk4::ColumnViewColumn {
        /// Pixels per level of side branch.
        const INDENT: u32 = 16;

        let factory = gtk4::SignalListItemFactory::new();
        factory.connect_setup(move |_, item| {
            let cell = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
            let marker = gtk4::Label::new(None);
            // Wide enough for either mark, so titles line up whether a row has one or not.
            marker.set_width_chars(1);
            let label = gtk4::Label::new(None);
            label.set_xalign(0.0);
            label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            label.set_hexpand(true);
            cell.append(&marker);
            cell.append(&label);
            Self::as_list_item(item).set_child(Some(&cell));
        });
        factory.connect_bind(move |_, item| {
            let item = Self::as_list_item(item);
            let (Some(row), Some(cell)) = (item.item(), item.child().and_downcast::<gtk4::Box>()) else {
                return;
            };
            let (Some(marker), Some(label)) = (
                cell.first_child().and_downcast::<gtk4::Label>(),
                cell.last_child().and_downcast::<gtk4::Label>(),
            ) else {
                return;
            };
            let classes = |_: &glib::Binding, classes: String| {
                let classes: glib::StrV = classes.split_whitespace().collect::<Vec<_>>().into();
                Some(classes.to_value())
            };
            let bindings = vec![
                row.bind_property("depth", &cell, "margin-start")
                    .transform_to(|_, depth: u32| Some((depth * INDENT) as i32))
                    .sync_create()
                    .build(),
                row.bind_property("marker", &marker, "label").sync_create().build(),
                row.bind_property("title", &label, "label").sync_create().build(),
                row.bind_property("url", &cell, "tooltip-text").sync_create().build(),
                row.bind_property("css", &label, "css-classes")
                    .transform_to(classes)
                    .sync_create()
                    .build(),
                row.bind_property("css", &marker, "css-classes")
                    .transform_to(classes)
                    .sync_create()
                    .build(),
            ];
            unsafe { item.set_data("bindings", bindings) };
        });
        factory.connect_unbind(move |_, item| {
            let item = Self::as_list_item(item);
            if let Some(bindings) = unsafe { item.steal_data::<Vec<glib::Binding>>("bindings") } {
                for binding in bindings {
                    binding.unbind();
                }
            }
        });
        gtk4::ColumnViewColumn::new(Some(title), Some(factory))
    }

    /// Redraw the active tab's history tree. The tree itself is `beacon-core`'s; this only
    /// writes its lines into the rows already on screen.
    fn refresh_history(&self) {
        let rows = match self.active_tab_id() {
            Some(id) => {
                let manager = self.tab_manager.lock().unwrap();
                manager.get_tab(id).map(|tab| tab.history().tree()).unwrap_or_default()
            }
            None => Vec::new(),
        };

        let branches = rows.iter().filter(|row| row.depth > 0).count();
        self.devtools_summary.set_text(&match (rows.len(), branches) {
            (1, _) => "1 entry".to_string(),
            (n, 0) => format!("{n} entries"),
            (n, b) => format!("{n} entries, {b} on side branches"),
        });

        for (index, line) in rows.iter().enumerate() {
            // The current entry stands out; what "back" would walk through reads normally;
            // everything off that path -- branches left behind -- is quieter.
            let css = match (line.current, line.on_current_path) {
                (true, _) => "history-current",
                (false, true) => "",
                (false, false) => "history-off-path",
            };
            let marker = if line.current {
                "\u{25CF}"
            } else if line.next {
                "\u{203A}"
            } else {
                ""
            };
            let title = line.title.as_deref().filter(|t| !t.trim().is_empty()).unwrap_or(line.url.as_str());
            let entry = line.id.0 as u64;
            let depth = line.depth as u32;
            let item = match self.history_model.item(index as u32).and_downcast::<super::rows::HistoryRow>() {
                Some(item) => item,
                None => {
                    let item = super::rows::HistoryRow::default();
                    self.history_model.append(&item);
                    item
                }
            };
            set_changed!(item, entry, set_entry, entry);
            set_changed!(item, depth, set_depth, depth);
            set_changed!(item, title, set_title, title);
            set_changed!(item, url, set_url, line.url.as_str());
            set_changed!(item, marker, set_marker, marker);
            set_changed!(item, css, set_css, css);
        }
        let wanted = rows.len() as u32;
        if self.history_model.n_items() > wanted {
            self.history_model
                .splice(wanted, self.history_model.n_items() - wanted, &[] as &[super::rows::HistoryRow]);
        }
    }

    // ── network ───────────────────────────────────────────────────────────

    /// The request list's columns, in pixels.
    ///
    /// Real columns rather than a single label padded with spaces. Padding only lines up
    /// while every row fits: once the pane is narrower than the text, the label clips and
    /// whole columns disappear -- which is how Status, Method and Type went missing from a
    /// narrowed pane while the header above them lost the same three words.
    /// The table's columns: title, the row property each one shows, its starting width
    /// (`None` for the column that takes the remaining space), and its alignment. Widths are
    /// a starting point only -- the header's dividers resize them from here.
    const NETWORK_COLUMNS: [(&'static str, &'static str, Option<i32>, f32); 6] = [
        ("Status", "status", Some(56), 0.0),
        ("Method", "method", Some(56), 0.0),
        ("Type", "kind", Some(96), 0.0),
        ("Size", "size", Some(72), 1.0),
        ("Time", "time", Some(88), 1.0),
        ("URL", "url", Some(200), 0.0),
    ];

    /// One cell: fixed width, its own alignment, no ellipsizing so it never eats a neighbour.
    fn cell(text: &str, width: i32, xalign: f32) -> gtk4::Label {
        let label = gtk4::Label::new(Some(text));
        label.set_xalign(xalign);
        label.set_width_chars(0);
        label.set_size_request(width, -1);
        label
    }

    /// Width of the waterfall column, in pixels.
    /// Narrow enough that a row's fixed columns still fit the pane at its default width.
    const WATERFALL_WIDTH: i32 = 200;

    /// One request's bar, drawn against the page's whole load.
    ///
    /// `window` is the span every bar is measured in: the earliest start and the latest
    /// finish across the list. Without a shared span each bar would be drawn to its own
    /// scale, and the picture -- what overlapped what, what the page waited on -- is the
    /// only reason to draw it at all.
    ///
    /// Two tones: pale up to the response headers, solid after. A row that is mostly pale
    /// was waiting on the server; one that is mostly solid was reading a large body.
    /// Draw one request's bar. The fractions are of the page's whole load, worked out where
    /// that span is known; this only has to paint them.
    fn draw_waterfall(widget: &gtk4::DrawingArea, cr: &gtk4::cairo::Context, width: i32, height: i32, offset: f64, wait: f64, body: f64) {
        let w = width as f64;
        let h = height as f64;
        let ink = widget.color();
        let bar_h = (h - 6.0).clamp(3.0, 8.0);
        let y = (h - bar_h) / 2.0;

        // A hairline track, so a bar's position reads against the whole load rather
        // than floating in space.
        cr.set_source_rgba(ink.red() as f64, ink.green() as f64, ink.blue() as f64, 0.10);
        cr.rectangle(0.0, h / 2.0 - 0.5, w, 1.0);
        let _ = cr.fill();

        let x = offset * w;
        // A request too fast to have any width still gets a mark: a bar that vanishes
        // reads as a request that never happened.
        let wait_w = (wait * w).max(if wait > 0.0 { 1.0 } else { 0.0 });
        let body_w = (body * w).max(1.0);

        if wait_w > 0.0 {
            cr.set_source_rgba(ink.red() as f64, ink.green() as f64, ink.blue() as f64, 0.30);
            cr.rectangle(x, y, wait_w, bar_h);
            let _ = cr.fill();
        }
        cr.set_source_rgba(0.20, 0.45, 0.85, 0.85);
        cr.rectangle(x + wait_w, y, body_w, bar_h);
        let _ = cr.fill();
    }

    /// Build the table's columns, once, at construction.
    ///
    /// Each column binds a property of the row object to the widget it makes, so a value
    /// that changes while a request runs reaches the label already on screen. Widths are the
    /// view's to manage from here on: every column is resizable by dragging the divider in
    /// the header, and the URL column takes whatever is left.
    fn build_network_columns(&self) {
        self.network_selection.set_model(Some(&self.network_model));
        self.network_list.set_model(Some(&self.network_selection));

        for (title, property, width, xalign) in Self::NETWORK_COLUMNS {
            self.network_list.append_column(&Self::text_column(title, property, width, xalign));
        }
        self.network_list.append_column(&self.waterfall_column());
        self.attach_request_menu();
    }

    /// A right click on any cell of a request offers to copy or export it.
    ///
    /// Hooked into every column's factory, after the column's own setup has made the cell's
    /// widget: a right click does not select a row in a column view, so the cell has to say
    /// which request it is showing.
    fn attach_request_menu(&self) {
        let columns = self.network_list.columns();
        for index in 0..columns.n_items() {
            let Some(factory) = columns
                .item(index)
                .and_downcast::<gtk4::ColumnViewColumn>()
                .and_then(|column| column.factory())
                .and_downcast::<gtk4::SignalListItemFactory>()
            else {
                continue;
            };
            let window = self.obj().downgrade();
            factory.connect_setup(move |_, item| {
                let item = Self::as_list_item(item);
                let Some(child) = item.child() else { return };
                let click = GestureClick::new();
                click.set_button(gdk::BUTTON_SECONDARY);
                let item = item.downgrade();
                let window = window.clone();
                click.connect_pressed(move |gesture, _, x, y| {
                    let (Some(window), Some(item)) = (window.upgrade(), item.upgrade()) else {
                        return;
                    };
                    let Some(id) = item
                        .item()
                        .and_downcast::<super::rows::RequestRow>()
                        .and_then(|row| row.request_id())
                    else {
                        return;
                    };
                    if let Some(widget) = gesture.widget() {
                        window.imp().show_request_menu(id, &widget, x, y);
                    }
                });
                child.add_controller(click);
            });
        }
    }

    fn show_request_menu(&self, id: uuid::Uuid, widget: &gtk4::Widget, x: f64, y: f64) {
        // One flat list: with a section separator the popover came up a row short and cut
        // its last item off.
        let menu = gtk4::gio::Menu::new();
        menu.append(Some("Copy as cURL"), Some("request.copy-curl"));
        menu.append(Some("Copy URL"), Some("request.copy-url"));
        menu.append(Some("Export HAR"), Some("request.export-har"));

        let actions = SimpleActionGroup::new();
        type CopyAs = fn(&beacon_core::devtools::NetRequest) -> String;
        let copies: [(&str, CopyAs); 2] = [
            ("copy-curl", beacon_core::export::curl),
            ("copy-url", |request| request.url.clone()),
        ];
        for (name, copy) in copies {
            let action = gtk4::gio::SimpleAction::new(name, None);
            let window = self.obj().downgrade();
            action.connect_activate(move |_, _| {
                let Some(window) = window.upgrade() else { return };
                let imp = window.imp();
                let Some(request) = beacon_core::devtools::requests(imp.active_tab_id())
                    .into_iter()
                    .find(|r| r.id == id)
                else {
                    return;
                };
                window.clipboard().set_text(&copy(&request));
            });
            actions.add_action(&action);
        }
        let action = gtk4::gio::SimpleAction::new("export-har", None);
        let window = self.obj().downgrade();
        action.connect_activate(move |_, _| {
            if let Some(window) = window.upgrade() {
                window.imp().export_har();
            }
        });
        actions.add_action(&action);

        let window = self.obj().clone();
        let popover = PopoverMenu::builder()
            .halign(gtk4::Align::Start)
            .has_arrow(false)
            .flags(PopoverMenuFlags::NESTED)
            .build();
        popover.insert_action_group("request", Some(&actions));
        // Parented to the window, not the cell: list cells are recycled, and a popover on one
        // would go with it.
        popover.set_parent(&window);
        popover.set_menu_model(Some(&menu));
        popover.connect_closed(|popover| {
            let popover = popover.clone();
            glib::idle_add_local_once(move || popover.unparent());
        });
        if let Some(point) = widget.compute_point(&window, &Point::new(x as f32, y as f32)) {
            popover.set_pointing_to(Some(&gdk::Rectangle::new(point.x() as i32, point.y() as i32, 0, 0)));
            popover.popup();
        }
    }

    /// Save the active tab's request log as a HAR file, asking where.
    pub(crate) fn export_har(&self) {
        let requests = beacon_core::devtools::requests(self.active_tab_id());
        if requests.is_empty() {
            self.log("Export HAR: this tab has made no requests yet");
            return;
        }
        let name = beacon_core::export::har_file_name(&requests, now_ms());
        let dialog = gtk4::FileDialog::builder().initial_name(name).build();
        let window = self.obj().clone();
        dialog.save(Some(&*self.obj()), gtk4::gio::Cancellable::NONE, move |result| {
            let file = match result {
                Ok(file) => file,
                Err(err) if err.matches(gtk4::DialogError::Dismissed) => return,
                Err(err) => {
                    window.imp().log(&format!("Export HAR: no file chooser: {err}"));
                    return;
                }
            };
            let Some(path) = file.path() else { return };
            // The log as it is now, not as it was when the dialog opened: the page may have
            // gone on loading while the name was typed.
            let requests = beacon_core::devtools::requests(window.imp().active_tab_id());
            let count = requests.len();
            // Serializing a log full of bodies and writing it out is not the main loop's work:
            // the window would stop drawing until the disk is done.
            glib::spawn_future_local(async move {
                let target = path.clone();
                let written = gtk4::gio::spawn_blocking(move || std::fs::write(&target, beacon_core::export::har_text(&requests))).await;
                match written {
                    Ok(Ok(())) => window.imp().log(&format!("Exported {count} requests to {}", path.display())),
                    Ok(Err(err)) => window.imp().log(&format!("Export HAR to {} failed: {err}", path.display())),
                    Err(_) => window
                        .imp()
                        .log(&format!("Export HAR to {} failed: the writer stopped", path.display())),
                }
            });
        });
    }

    /// The debug log's columns. Level and target are narrow and fixed; the message takes
    /// what is left, because it is the only part whose length is anyone's guess.
    fn build_log_columns(&self) {
        self.log_list.set_model(Some(&gtk4::NoSelection::new(Some(self.log_model.clone()))));
        for (title, property, width, xalign) in [
            ("Time", "time", Some(96), 0.0),
            ("Level", "level", Some(64), 0.0),
            ("Source", "target", Some(180), 0.0),
            ("Message", "message", None, 0.0),
        ] {
            self.log_list.append_column(&Self::text_column(title, property, width, xalign));
        }
    }

    /// The timings table's first column: the namespace, plus an info icon for the sentence
    /// saying what it measures. A bare tooltip is invisible until you happen to rest the
    /// pointer on the right cell, so the icon is what tells the reader there is more here.
    ///
    /// The tooltip sits on the cell rather than the icon, so it answers a hover anywhere in
    /// the column — the icon advertises it, it is not a target you have to hit.
    fn namespace_column(title: &str) -> gtk4::ColumnViewColumn {
        let factory = gtk4::SignalListItemFactory::new();
        factory.connect_setup(move |_, item| {
            let cell = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
            let label = gtk4::Label::new(None);
            label.set_xalign(0.0);
            label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            let info = gtk4::Image::from_icon_name("help-about-symbolic");
            info.set_pixel_size(12);
            info.add_css_class("info-icon");
            // The icon belongs to the name beside it, so it follows the text rather than
            // sitting at the far edge of a column that is mostly empty space. The filler
            // takes that space instead, and the label still ellipsizes when there is none.
            let filler = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
            filler.set_hexpand(true);
            cell.append(&label);
            cell.append(&info);
            cell.append(&filler);
            Self::as_list_item(item).set_child(Some(&cell));
        });
        factory.connect_bind(move |_, item| {
            let item = Self::as_list_item(item);
            let (Some(row), Some(cell)) = (item.item(), item.child().and_downcast::<gtk4::Box>()) else {
                return;
            };
            let (Some(label), Some(info)) = (
                cell.first_child().and_downcast::<gtk4::Label>(),
                cell.first_child()
                    .and_then(|label| label.next_sibling())
                    .and_downcast::<gtk4::Image>(),
            ) else {
                return;
            };
            let bindings = vec![
                row.bind_property("namespace", &label, "label").sync_create().build(),
                row.bind_property("description", &cell, "tooltip-text").sync_create().build(),
                // A namespace the engine cannot explain gets no icon: an icon promising an
                // explanation that turns out to be the row's own text is worse than none.
                row.bind_property("explained", &info, "visible").sync_create().build(),
                row.bind_property("css", &label, "css-classes")
                    .transform_to(|_, classes: String| {
                        let classes: glib::StrV = classes.split_whitespace().collect::<Vec<_>>().into();
                        Some(classes.to_value())
                    })
                    .sync_create()
                    .build(),
            ];
            unsafe { item.set_data("bindings", bindings) };
        });
        factory.connect_unbind(move |_, item| {
            let item = Self::as_list_item(item);
            if let Some(bindings) = unsafe { item.steal_data::<Vec<glib::Binding>>("bindings") } {
                for binding in bindings {
                    binding.unbind();
                }
            }
        });

        let column = gtk4::ColumnViewColumn::builder()
            .title(title)
            .factory(&factory)
            .resizable(true)
            .build();
        column.set_expand(true);
        column
    }

    /// One column of a devtools table.
    ///
    /// Every table here is a list of rows with a `css` property and one property per column,
    /// so they all want the same column: a label bound to a property, ellipsized, classed by
    /// the row. Resizable, because the widths that suit one page suit another badly, and the
    /// reader is better placed to decide than a constant in this file.
    fn text_column(title: &str, property: &'static str, width: Option<i32>, xalign: f32) -> gtk4::ColumnViewColumn {
        let factory = gtk4::SignalListItemFactory::new();
        factory.connect_setup(move |_, item| {
            let label = gtk4::Label::new(None);
            label.set_xalign(xalign);
            label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            Self::as_list_item(item).set_child(Some(&label));
        });
        factory.connect_bind(move |_, item| {
            let item = Self::as_list_item(item);
            let (Some(row), Some(label)) = (item.item(), item.child().and_downcast::<gtk4::Label>()) else {
                return;
            };
            // Bindings rather than one-off setters: the row object outlives this widget and
            // keeps changing, and the binding is what carries those changes across without
            // anything being rebuilt.
            let mut bindings = vec![row.bind_property(property, &label, "label").sync_create().build()];
            // Text wider than its column is still readable on hover. A column of numbers has
            // nothing to add, so only the ones that can overflow get a tooltip.
            if matches!(property, "url" | "message" | "target" | "subject" | "location") {
                bindings.push(row.bind_property(property, &label, "tooltip-text").sync_create().build());
            }
            // A Page report row's kind says what it means on hover. Network rows have a kind
            // too, but no hint, and binding a property the row lacks aborts the process.
            if property == "kind" && row.find_property("hint").is_some() {
                bindings.push(row.bind_property("hint", &label, "tooltip-text").sync_create().build());
            }
            bindings.push(
                row.bind_property("css", &label, "css-classes")
                    .transform_to(|_, classes: String| {
                        let classes: glib::StrV = classes.split_whitespace().collect::<Vec<_>>().into();
                        Some(classes.to_value())
                    })
                    .sync_create()
                    .build(),
            );
            unsafe { item.set_data("bindings", bindings) };
        });
        // Bindings are per-row, and list items are recycled: one left in place would go on
        // writing a departed row's values into a widget now showing something else.
        factory.connect_unbind(move |_, item| {
            let item = Self::as_list_item(item);
            if let Some(bindings) = unsafe { item.steal_data::<Vec<glib::Binding>>("bindings") } {
                for binding in bindings {
                    binding.unbind();
                }
            }
        });

        let column = gtk4::ColumnViewColumn::builder()
            .title(title)
            .factory(&factory)
            .resizable(true)
            .build();
        match width {
            Some(width) => column.set_fixed_width(width),
            None => column.set_expand(true),
        }
        column
    }

    /// The waterfall column: one bar per request, drawn against the page's whole load.
    fn waterfall_column(&self) -> gtk4::ColumnViewColumn {
        let factory = gtk4::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let area = gtk4::DrawingArea::new();
            area.set_content_width(Self::WATERFALL_WIDTH);
            Self::as_list_item(item).set_child(Some(&area));
        });
        factory.connect_bind(|_, item| {
            let item = Self::as_list_item(item);
            let (Some(row), Some(area)) = (
                item.item().and_downcast::<super::rows::RequestRow>(),
                item.child().and_downcast::<gtk4::DrawingArea>(),
            ) else {
                return;
            };
            let drawn = row.clone();
            area.set_draw_func(move |widget, cr, width, height| {
                Self::draw_waterfall(widget, cr, width, height, drawn.bar_offset(), drawn.bar_wait(), drawn.bar_body());
            });
            // The bar moves while a request runs, and a property change does not redraw a
            // canvas the way it repaints a label.
            let handler = row.connect_notify_local(None, {
                let area = area.clone();
                move |_, _| area.queue_draw()
            });
            unsafe { item.set_data("notify", (row, handler)) };
        });
        factory.connect_unbind(|_, item| {
            let item = Self::as_list_item(item);
            if let Some(data) = unsafe { item.steal_data::<(super::rows::RequestRow, glib::SignalHandlerId)>("notify") } {
                let (row, handler) = data;
                row.disconnect(handler);
            }
        });

        // The bar takes whatever is left rather than a width of its own: at a narrow pane it
        // is the column that can afford to be a stub, and the columns that carry words keep
        // their space. Widening the pane, or dragging any divider, gives it room again.
        gtk4::ColumnViewColumn::builder()
            .title("Waterfall")
            .factory(&factory)
            .resizable(true)
            .expand(true)
            .build()
    }

    /// The list item a factory callback is talking about.
    fn as_list_item(item: &glib::Object) -> gtk4::ListItem {
        #[allow(clippy::expect_used)] // PANIC-SAFE: a list factory is only ever passed ListItems
        item.clone().downcast::<gtk4::ListItem>().expect("list item")
    }

    /// The line above the request list. Mentions held bodies only when there are some, so
    /// the common case stays short.
    fn network_summary(&self, count: usize, transferred: u64) -> String {
        use beacon_core::devtools::format_bytes;
        let mut summary = format!("{count} requests · {} transferred", format_bytes(transferred));
        let held = beacon_core::devtools::captured_body_bytes();
        if held > 0 {
            summary.push_str(&format!(" · {} of bodies held", format_bytes(held as u64)));
        }

        // What the page is still waiting on, and for how long. The first question anyone
        // asks of an unresponsive page, answered without having to read down the list.
        let now = now_ms();
        let stuck: Vec<(u64, beacon_core::devtools::Phase)> = beacon_core::devtools::requests(self.active_tab_id())
            .iter()
            .filter(|r| {
                r.elapsed_us.is_none()
                    && matches!(
                        r.state,
                        beacon_core::devtools::RequestState::Running | beacon_core::devtools::RequestState::Queued
                    )
            })
            .map(|r| (now.saturating_sub(r.started_ms), r.phase()))
            .collect();
        if let Some((longest, phase)) = stuck.iter().max_by_key(|(age, _)| *age) {
            summary.push_str(&format!(
                " · {} in flight, longest {} {}",
                stuck.len(),
                beacon_core::devtools::format_duration(longest * 1000),
                phase.label(),
            ));
        }
        summary
    }

    fn refresh_network(&self, needle: &str) {
        use beacon_core::devtools::{format_bytes, format_duration};

        // Only this window's active tab: a request list mixing several tabs together is a
        // log, not a network panel.
        let requests = beacon_core::devtools::requests(self.active_tab_id());
        let total: u64 = requests.iter().map(|r| r.received_bytes).sum();
        self.devtools_summary.set_text(&self.network_summary(requests.len(), total));

        // The span every bar is drawn against: from the first request the page made to the
        // last byte of the last one. Computed once, so the bars share a scale and can be
        // read against each other.
        let window_start = requests.iter().map(|r| r.started_ms).min().unwrap_or(0);
        let window_end = requests
            .iter()
            .map(|r| r.started_ms + r.elapsed_us.unwrap_or(0) / 1000)
            .max()
            .unwrap_or(window_start + 1)
            .max(window_start + 1);

        // What each row will say, worked out before a single widget is touched. The list is
        // rebuilt wholesale when it changes -- a request list is short, and diffing widgets
        // would be a great deal of bookkeeping -- but rebuilding it when it has *not*
        // changed destroys and recreates every row four times a second, which is slow and
        // eats clicks, since the row under the pointer is gone before the button comes up.
        let mut rows = Vec::with_capacity(requests.len());
        for request in &requests {
            if !needle.is_empty() && !request.url.to_lowercase().contains(needle) && !request.kind.contains(needle) {
                continue;
            }

            // A failed request has no status code, and "err" says nothing a developer can
            // act on. The kind the engine derived does: "TLS" and "no connection" send you
            // to different places.
            //
            // A response the cache answered says so, the way other browsers mark it: a hit
            // sent nothing and has no status of its own, a revalidation got its 304.
            let cached = request.cache_answer().is_some();
            let status = match (request.status, request.state) {
                (Some(code), _) if cached => format!("{code} (cache)"),
                (Some(code), _) => code.to_string(),
                (None, beacon_core::devtools::RequestState::Failed) => request.failure_label().unwrap_or("err").to_string(),
                (None, _) if cached => "(cache)".to_string(),
                _ => "—".to_string(),
            };
            let size = if request.received_bytes > 0 {
                format_bytes(request.received_bytes)
            } else {
                request.content_length.map(format_bytes).unwrap_or_else(|| "—".to_string())
            };
            // A request still in flight shows how long it has been in flight, not the word
            // "loading". A page that is not responding is a page with a request sitting at
            // twelve seconds, and that is only visible if the number is on screen.
            let time = match request.elapsed_us {
                Some(us) => format_duration(us),
                None if matches!(
                    request.state,
                    beacon_core::devtools::RequestState::Running | beacon_core::devtools::RequestState::Queued
                ) =>
                {
                    // Which phase it has been sitting in, not just how long: twelve seconds
                    // waiting for a reply and twelve seconds part way through a body are
                    // different problems with different fixes.
                    format!(
                        "{} {}…",
                        request.phase().label(),
                        format_duration(now_ms().saturating_sub(request.started_ms) * 1000)
                    )
                }
                None => request.state.label().to_string(),
            };

            // A request that never reached the wire has no method to report. A cache hit is
            // one, and the status column says so from the engine's own report; with no cache
            // outcome, a missing method is a file:// load or a request that failed before
            // its request line went out.
            let method = request.method.as_deref().unwrap_or("—").to_string();
            rows.push(NetworkRow {
                id: request.id,
                status,
                method,
                kind: truncate(&request.kind, 18),
                size,
                time,
                url: request.url.clone(),
                css: if request.error.is_some() { "monospace error" } else { "monospace" },
                span: (
                    request.started_ms,
                    request.headers_ms,
                    request.started_ms + request.elapsed_us.unwrap_or(0) / 1000,
                ),
            });
        }

        // The model is updated, not replaced: a row already on screen keeps its widgets and
        // its place, and only the values that changed are written into it. That is what lets
        // a clock tick without the row underneath the pointer being destroyed, and what keeps
        // the selection and the scroll position where the reader left them.
        let span = window_end.saturating_sub(window_start).max(1) as f64;
        for (index, row) in rows.iter().enumerate() {
            let (offset, wait, body) = row.bar(window_start, span);
            let existing = self
                .network_model
                .item(index as u32)
                .and_downcast::<super::rows::RequestRow>()
                .filter(|item| item.id() == row.id.to_string());

            match existing {
                // Same request in the same place: write over what changed, and *only* what
                // changed — an identical write is not free, it is a `notify` like any other.
                Some(item) => {
                    set_changed!(item, status, set_status, row.status.as_str());
                    set_changed!(item, method, set_method, row.method.as_str());
                    set_changed!(item, kind, set_kind, row.kind.as_str());
                    set_changed!(item, size, set_size, row.size.as_str());
                    set_changed!(item, time, set_time, row.time.as_str());
                    set_changed!(item, url, set_url, row.url.as_str());
                    set_changed!(item, css, set_css, row.css);
                    set_changed!(item, bar_offset, set_bar_offset, offset);
                    set_changed!(item, bar_wait, set_bar_wait, wait);
                    set_changed!(item, bar_body, set_bar_body, body);
                }
                // A different request here, or nothing yet: the list has actually changed
                // shape, so from this point on it is rebuilt.
                None => {
                    let item = super::rows::RequestRow::default();
                    item.set_id(row.id.to_string());
                    item.set_status(row.status.as_str());
                    item.set_method(row.method.as_str());
                    item.set_kind(row.kind.as_str());
                    item.set_size(row.size.as_str());
                    item.set_time(row.time.as_str());
                    item.set_url(row.url.as_str());
                    item.set_css(row.css);
                    item.set_bar_offset(offset);
                    item.set_bar_wait(wait);
                    item.set_bar_body(body);

                    let index = index as u32;
                    if index < self.network_model.n_items() {
                        self.network_model.splice(index, 1, &[item]);
                    } else {
                        self.network_model.append(&item);
                    }
                }
            }
        }
        // Anything past the end is gone -- the log evicts its oldest rows once it is full.
        let wanted = rows.len() as u32;
        if self.network_model.n_items() > wanted {
            self.network_model
                .splice(wanted, self.network_model.n_items() - wanted, &[] as &[super::rows::RequestRow]);
        }

        // Put the highlight back on whatever the reader chose, wherever it now sits.
        let selected = *self.network_selected.borrow();
        let position = selected.and_then(|id| rows.iter().position(|row| row.id == id));
        match position {
            Some(position) if self.network_selection.selected() != position as u32 => {
                self.network_selection.set_selected(position as u32);
            }
            None if selected.is_none() => self.network_selection.set_selected(gtk4::INVALID_LIST_POSITION),
            _ => {}
        }

        self.show_request_detail();
    }

    /// What happened to the request on its way out, for the Request tab: upgrades and policy
    /// changes, CORS preflights, and retries. Each section only when there is something in it.
    fn push_security_rows(&self, request: &beacon_core::devtools::NetRequest, rows: &mut Vec<DetailRow>) {
        use beacon_core::devtools::format_duration;

        // The hop's URL only when it is not the one the row already shows, which is the
        // common case and would repeat the URL on every line.
        let on = |url: &str| {
            if url == request.url {
                String::new()
            } else {
                format!(" ({url})")
            }
        };

        if !request.upgrades.is_empty() || !request.changes.is_empty() {
            rows.push(DetailRow::Section("Security".into()));
            for upgrade in &request.upgrades {
                rows.push(DetailRow::Pair(
                    format!("Upgraded ({})", upgrade.reason),
                    format!("{} -> {}", upgrade.from, upgrade.to),
                ));
            }
            for (url, change) in &request.changes {
                rows.push(DetailRow::Pair("Changed".into(), format!("{change}{}", on(url))));
            }
        }

        if !request.preflights.is_empty() {
            rows.push(DetailRow::Section("CORS preflight".into()));
            for preflight in &request.preflights {
                let took = match preflight.elapsed_us {
                    Some(us) => format_duration(us),
                    None if request.phase() == beacon_core::devtools::Phase::Done => "no response".into(),
                    None => "waiting".into(),
                };
                rows.push(DetailRow::Pair("OPTIONS".into(), format!("{took}{}", on(&preflight.url))));
            }
        }

        if !request.retries.is_empty() {
            rows.push(DetailRow::Section("Retries".into()));
            for retry in &request.retries {
                rows.push(DetailRow::Pair(
                    format!("Retry {}", retry.attempt),
                    format!("after {}: {}", format_duration(retry.delay_us), retry.reason),
                ));
            }
            rows.push(DetailRow::Note("Timings and the response are from the latest attempt.".into()));
        }
    }

    /// What the cache did and which credentials were asked for, for the Response tab. Each
    /// section only when there is something in it.
    fn push_cache_and_auth_rows(&self, request: &beacon_core::devtools::NetRequest, rows: &mut Vec<DetailRow>) {
        use gosub_engine::events::AuthTarget;

        let on = |url: &str| {
            if url == request.url {
                String::new()
            } else {
                format!(" ({url})")
            }
        };

        if !request.cache.is_empty() {
            rows.push(DetailRow::Section("Cache".into()));
            for (url, outcome) in &request.cache {
                rows.push(DetailRow::Pair("Cache".into(), format!("{outcome}{}", on(url))));
            }
        }

        if !request.auth.is_empty() {
            rows.push(DetailRow::Section("Authentication".into()));
            for seen in &request.auth {
                let asked = match seen.target {
                    AuthTarget::Server => "Server (401)",
                    AuthTarget::Proxy => "Proxy (407)",
                };
                rows.push(DetailRow::Pair(
                    asked.into(),
                    format!("{}{}", seen.challenges_label(), on(&seen.url)),
                ));
                let answered = if seen.retried {
                    "yes, sent again with credentials"
                } else {
                    "no: this response is what the page got"
                };
                rows.push(DetailRow::Pair("Answered".into(), answered.into()));
            }
        }
    }

    /// Fill the detail tabs for the selected request.
    ///
    /// Split the way a developer thinks about a request rather than the way the events
    /// arrive: what was asked for, what came back, what the bytes were, and what the page
    /// went on to fetch because of it.
    fn show_request_detail(&self) {
        use beacon_core::devtools::{format_bytes, format_duration};

        let selected = *self.network_selected.borrow();
        let requests = beacon_core::devtools::requests(self.active_tab_id());
        let request = selected.and_then(|id| requests.iter().find(|r| r.id == id));

        let Some(request) = request else {
            let empty = if selected.is_some() {
                "That request is no longer recorded."
            } else {
                "Select a request."
            };
            let rows = [DetailRow::Note(empty.to_string())];
            self.fill_pairs("request", &self.request_list, &rows);
            self.fill_pairs("response", &self.response_list, &rows);
            self.fill_pairs("subresources", &self.sub_list, &rows);
            self.fill_pairs("timing", &self.timing_list, &rows);
            self.set_raw("request-raw", &self.network_request_view, empty);
            self.set_raw("response-raw", &self.network_response_view, empty);
            self.set_raw("body", &self.network_body_view, empty);
            return;
        };

        // ── Request ───────────────────────────────────────────────────────
        let mut rows = vec![
            DetailRow::Section("Request".into()),
            DetailRow::Pair("URL".into(), request.url.clone()),
            DetailRow::Pair("Method".into(), request.method.clone().unwrap_or_else(|| "—".into())),
            DetailRow::Pair("Kind".into(), request.kind.clone()),
            DetailRow::Pair("Initiated by".into(), request.initiator.clone()),
            DetailRow::Pair("State".into(), request.state.label().into()),
        ];
        if !request.redirects.is_empty() {
            rows.push(DetailRow::Section("Redirects".into()));
            for (status, to) in &request.redirects {
                rows.push(DetailRow::Pair(status.to_string(), to.clone()));
            }
        }
        self.push_security_rows(request, &mut rows);
        if request.request_headers.is_empty() {
            let note = if request.cache_answer() == Some(gosub_engine::events::CacheOutcome::Hit) {
                "No request line: the cache answered without contacting the server."
            } else {
                "No request line recorded: this never reached the network."
            };
            rows.push(DetailRow::Note(note.into()));
        } else {
            rows.push(DetailRow::Section("Request headers".into()));
            for (name, value) in &request.request_headers {
                rows.push(DetailRow::Pair(name.clone(), value.clone()));
            }
            // Everything the stack sends is now listed, so the note is down to the one
            // header that cannot be: `host` is added by the connection, and a developer
            // used to seeing it in another inspector would otherwise wonder where it went.
            //
            // Still derived rather than written out, because twice a hard-coded version
            // went on claiming a header was unavailable after the network stack had started
            // reporting it -- the pane showing the header and denying it in the same breath.
            let mut absent: Vec<&str> = vec!["host"];
            if !request
                .request_headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("accept-encoding"))
            {
                absent.push("accept-encoding");
            }
            rows.push(DetailRow::Note(format!(
                "Added below this layer and not visible here: {}.",
                absent.join(", ")
            )));
        }
        self.fill_pairs("request", &self.request_list, &rows);

        let mut raw = format!("{} {} HTTP/1.1\n", request.method.as_deref().unwrap_or("GET"), request.url);
        for (name, value) in &request.request_headers {
            raw.push_str(&format!("{name}: {value}\n"));
        }
        if request.request_headers.is_empty() {
            raw.push_str("\n(no request line recorded: this never reached the network)\n");
        }
        self.set_raw("request-raw", &self.network_request_view, &raw);

        // ── Response ──────────────────────────────────────────────────────
        let status = request
            .status
            .map(|s| s.to_string())
            .unwrap_or_else(|| request.state.label().to_string());
        let mut rows = vec![DetailRow::Section("Response".into()), DetailRow::Pair("Status".into(), status)];
        if let Some(content_type) = &request.content_type {
            rows.push(DetailRow::Pair("Content-Type".into(), content_type.clone()));
        }
        rows.push(DetailRow::Pair("Received".into(), format_bytes(request.received_bytes)));
        if let Some(length) = request.content_length {
            rows.push(DetailRow::Pair("Declared".into(), format_bytes(length)));
        }
        if let Some(us) = request.elapsed_us {
            rows.push(DetailRow::Pair("Took".into(), format_duration(us)));
        }
        if let Some(error) = &request.error {
            rows.push(DetailRow::Section("Failure".into()));
            if let Some(label) = request.failure_label() {
                rows.push(DetailRow::Pair("Cause".into(), label.into()));
            }
            rows.push(DetailRow::Pair("Error".into(), error.clone()));
            if let Some(host) = request.tls_host() {
                rows.push(DetailRow::Pair("Host".into(), host.into()));
            }
            if let Some(fingerprint) = request.tls_fingerprint() {
                rows.push(DetailRow::Pair("SHA-256".into(), fingerprint));
            }
            if let Some(hint) = request.failure_hint() {
                rows.push(DetailRow::Note(hint.into()));
            }
            if let Some(fix) = request.failure_fix() {
                rows.push(DetailRow::Pair("Fix".into(), fix.into()));
            }
            if let Some(spec) = request.failure_spec() {
                rows.push(DetailRow::Pair("Rule".into(), spec.into()));
            }
            // What the refusal was decided on, pulled out of the two header lists so the
            // reader does not have to find them there.
            let evidence = request.cors_evidence();
            if !evidence.is_empty() {
                rows.push(DetailRow::Section("CORS headers".into()));
                for (name, value) in evidence {
                    rows.push(DetailRow::Pair(name, value));
                }
            }
        }
        self.push_cache_and_auth_rows(request, &mut rows);
        if request.headers.is_empty() {
            rows.push(DetailRow::Note("No response headers recorded.".into()));
        } else {
            rows.push(DetailRow::Section("Response headers".into()));
            for (name, value) in &request.headers {
                rows.push(DetailRow::Pair(name.clone(), value.clone()));
            }
        }
        self.fill_pairs("response", &self.response_list, &rows);

        let mut raw = match request.status {
            Some(code) => format!("HTTP/1.1 {code}\n"),
            None => format!("(no response: {})\n", request.state.label()),
        };
        for (name, value) in &request.headers {
            raw.push_str(&format!("{name}: {value}\n"));
        }
        self.set_raw("response-raw", &self.network_response_view, &raw);

        // ── Body ──────────────────────────────────────────────────────────
        let text = match &request.body {
            Some(bytes) => beacon_core::devtools::format_body(bytes, request.body_truncated),
            None if request.body_evicted => "Captured, then dropped.\n\nThe panel keeps a fixed total of body bytes and this was among the\noldest when that ran out. Reload to capture it again.\n".to_string(),
            None if !self.devtools_pane.get_visible() => "No body captured.\n".to_string(),
            None => {
                let mut text = String::from("No body captured for this request.\n\n");
                text.push_str("Capture starts when this pane opens, so anything that finished\n");
                text.push_str("earlier has none -- reload the page to record it.\n\n");
                text.push_str("Video, audio and downloads are never captured, and neither is\n");
                text.push_str("a response that declares itself larger than the per-request cap.\n");
                text
            }
        };
        self.set_raw("body", &self.network_body_view, &text);

        // ── Timing ────────────────────────────────────────────────────────
        //
        // Laid out end to end against the request's own total rather than the page's, so
        // this answers "where did *this* request's time go" -- which is a different question
        // from the waterfall column's "when did it happen relative to everything else".
        let total_us = request.elapsed_us.unwrap_or(0);
        let wait_us = match (request.headers_ms, request.elapsed_us) {
            (Some(headers), Some(_)) => headers.saturating_sub(request.started_ms) * 1000,
            _ => 0,
        };
        let receive_us = total_us.saturating_sub(wait_us);

        let mut rows = Vec::new();
        // A request still running has no phase breakdown to show yet, but it does have the
        // one fact worth having: which phase it has been sitting in, and for how long. That
        // is the answer to "why is this page not responding", so it goes first.
        let phase = request.phase();
        if phase != beacon_core::devtools::Phase::Done {
            // No breakdown yet -- the totals it divides up only exist once the request
            // ends. What does exist is the one fact worth having: which phase it has been
            // sitting in, and for how long.
            rows.push(DetailRow::Section("Still running".into()));
            rows.push(DetailRow::Pair("Phase".into(), phase.label().into()));
            rows.push(DetailRow::Pair(
                "Running for".into(),
                format_duration(now_ms().saturating_sub(request.started_ms) * 1000),
            ));
            rows.push(DetailRow::Note(phase.hint().into()));
            if let Some(us) = request.dns_us {
                rows.push(DetailRow::Pair("DNS took".into(), format_duration(us)));
            }
            if let Some(us) = request.connect_us {
                rows.push(DetailRow::Pair("Connect took".into(), format_duration(us)));
            }
        } else if total_us == 0 && request.dns_us.is_none() {
            rows.push(DetailRow::Note(format!(
                "No timing recorded: this request is {}.",
                request.state.label()
            )));
        } else {
            let span = total_us.max(1) as f64;

            // Built as a list first, then laid out. A closure that both captured the running
            // offset and had to be reset around the DNS/connect pair could not borrow-check,
            // and the reset is the interesting part: `Connect` encloses `DNS` rather than
            // following it, so both are drawn from the same origin.
            let mut phases: Vec<(&str, u64, f64)> = Vec::new();
            match (request.dns_us, request.connect_us) {
                (None, None) => rows.push(DetailRow::Note(
                    "Connection reused: nothing was resolved or dialled for this request.".into(),
                )),
                (dns, connect) => {
                    if let Some(us) = dns {
                        phases.push(("DNS", us, 0.0));
                    }
                    if let Some(us) = connect {
                        phases.push(("Connect", us, 0.0));
                    }
                }
            }

            // The transfer runs end to end after the connection is up.
            let mut at = 0.0;
            for (name, us) in [("Waiting", wait_us), ("Receiving", receive_us)] {
                phases.push((name, us, at));
                at += us as f64 / span;
            }

            for (name, us, offset) in phases {
                rows.push(DetailRow::Phase {
                    name: name.to_string(),
                    offset,
                    fraction: us as f64 / span,
                    label: format_duration(us),
                });
            }
            rows.push(DetailRow::Pair("Total".into(), format_duration(total_us)));
            rows.push(DetailRow::Note(
                "Waiting is the time to the response headers; receiving is the body after \
                 them. Connect encloses DNS rather than following it. Blocked, TLS setup and \
                 send time are not reported by the network stack."
                    .into(),
            ));
        }
        self.fill_pairs("timing", &self.timing_list, &rows);

        // ── Subresources ──────────────────────────────────────────────────
        let mut rows = Vec::new();
        if request.kind == "document" {
            let children: Vec<_> = requests
                .iter()
                .filter(|r| r.id != request.id && r.initiator != "navigation")
                .collect();
            if children.is_empty() {
                rows.push(DetailRow::Note("This document pulled in no subresources.".into()));
            } else {
                rows.push(DetailRow::Section(format!("{} loaded for this document", children.len())));
                rows.push(DetailRow::Columns(("Type".into(), "Size".into(), "URL".into())));
                for child in children {
                    let size = if child.error.is_some() {
                        child.state.label().to_string()
                    } else {
                        format_bytes(child.received_bytes)
                    };
                    rows.push(DetailRow::Columns((child.kind.clone(), size, short_url(&child.url))));
                }
                rows.push(DetailRow::Note(
                    "Grouped by exclusion, not by parentage: the engine records no initiating \
                     request id, so this is everything the page fetched that a navigation did not."
                        .into(),
                ));
            }
        } else {
            rows.push(DetailRow::Note("Only a document has subresources.".into()));
        }
        self.fill_pairs("subresources", &self.sub_list, &rows);
    }

    /// Fill a striped list with rows, but only when something changed.
    ///
    /// The guard is what makes these panes readable: they refresh four times a second, and
    /// rebuilding a list resets its scroll, so anything past the first screenful could never
    /// be reached.
    fn fill_pairs(&self, key: &'static str, list: &gtk4::ListBox, rows: &[DetailRow]) {
        let signature: String = rows.iter().map(DetailRow::signature).collect();
        if self.detail_rendered.borrow().get(key) == Some(&signature) {
            return;
        }
        self.detail_rendered.borrow_mut().insert(key, signature);

        while let Some(child) = list.first_child() {
            list.remove(&child);
        }
        for entry in rows {
            let row = gtk4::ListBoxRow::new();
            row.set_activatable(false);
            match entry {
                // A heading, not data: it gets its own look and is skipped by the banding,
                // so a long header list reads as sections rather than one undifferentiated
                // run of rows.
                DetailRow::Section(title) => {
                    let label = gtk4::Label::new(Some(title));
                    label.set_xalign(0.0);
                    label.add_css_class("detail-section");
                    row.set_child(Some(&label));
                    row.add_css_class("section-row");
                }
                DetailRow::Pair(name, value) => {
                    // Two real columns, so every value starts at the same x whether its name
                    // is "URL" or "content-security-policy".
                    let line = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
                    let key_label = Self::cell(name, 150, 0.0);
                    key_label.add_css_class("detail-key");
                    line.append(&key_label);
                    let value_label = gtk4::Label::new(Some(value));
                    value_label.set_xalign(0.0);
                    value_label.set_hexpand(true);
                    value_label.set_selectable(true);
                    // Wrapped, not ellipsized. A URL's query string and a long
                    // content-security-policy are exactly the parts worth reading, and they
                    // live at the end -- cutting them off leaves three `load.php` rows that
                    // cannot be told apart. The pane scrolls; a taller row is the cheaper
                    // price.
                    value_label.set_wrap(true);
                    value_label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
                    line.append(&value_label);
                    line.add_css_class("monospace");
                    row.set_child(Some(&line));
                }
                DetailRow::Columns(cells) => {
                    let line = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
                    line.append(&Self::cell(&cells.0, 120, 0.0));
                    line.append(&Self::cell(&cells.1, 76, 1.0));
                    let tail = gtk4::Label::new(Some(&cells.2));
                    tail.set_xalign(0.0);
                    tail.set_hexpand(true);
                    tail.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                    tail.set_tooltip_text(Some(&cells.2));
                    line.append(&tail);
                    line.add_css_class("monospace");
                    row.set_child(Some(&line));
                }
                DetailRow::Phase {
                    name,
                    offset,
                    fraction,
                    label,
                } => {
                    let line = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
                    let name_label = Self::cell(name, 120, 0.0);
                    name_label.add_css_class("detail-key");
                    line.append(&name_label);

                    // The bar takes the room between the name and the number, so every
                    // phase is drawn against the same width and can be compared by eye.
                    let (offset, fraction) = (*offset, *fraction);
                    let bar = gtk4::DrawingArea::new();
                    bar.set_hexpand(true);
                    bar.set_content_height(12);
                    bar.set_draw_func(move |widget, cr, width, height| {
                        let w = width as f64;
                        let h = height as f64;
                        let ink = widget.color();
                        cr.set_source_rgba(ink.red() as f64, ink.green() as f64, ink.blue() as f64, 0.10);
                        cr.rectangle(0.0, h / 2.0 - 0.5, w, 1.0);
                        let _ = cr.fill();

                        // A phase too short to have width still shows a tick: zero is a
                        // measurement, and a blank row looks like missing data.
                        let bar_w = (fraction * w).max(1.5);
                        cr.set_source_rgba(0.20, 0.45, 0.85, 0.85);
                        cr.rectangle(offset * w, 2.0, bar_w, h - 4.0);
                        let _ = cr.fill();
                    });
                    line.append(&bar);

                    let value = Self::cell(label, 76, 1.0);
                    value.add_css_class("monospace");
                    line.append(&value);
                    row.set_child(Some(&line));
                }
                DetailRow::Note(text) => {
                    let label = gtk4::Label::new(Some(text));
                    label.set_xalign(0.0);
                    label.set_wrap(true);
                    label.add_css_class("dim-label");
                    row.set_child(Some(&label));
                    row.add_css_class("section-row");
                }
            }
            list.append(&row);
        }
    }

    /// Same guard for the plain-text panes.
    fn set_raw(&self, key: &'static str, view: &TextView, text: &str) {
        if self.detail_rendered.borrow().get(key).map(String::as_str) == Some(text) {
            return;
        }
        self.detail_rendered.borrow_mut().insert(key, text.to_string());
        view.buffer().set_text(text);
    }

    pub(crate) fn close_tab(&self, tab_id: TabId) {
        // Remember the tab for "Reopen Closed Tab" (url + strip position).
        let position = self.chips().iter().position(|c| c.get_tab_id() == Some(tab_id));

        let mut manager = self.tab_manager.lock().unwrap();
        if manager.tab_count() == 1 {
            drop(manager);
            self.log("Cannot close the last tab");
            return;
        }
        if let Some(tab) = manager.get_tab(tab_id) {
            self.beacon.borrow_mut().closed_mut().push(tab.url().to_string(), position);
        }
        let handle = manager.get_tab(tab_id).and_then(|t| t.tab_handle());
        manager.remove_tab(tab_id);
        drop(manager);

        // Shut down the engine worker behind the tab and drop our references.
        if let Some(handle) = handle {
            self.beacon.borrow_mut().unbind_engine_tab(handle.tab_id);
            runtime().spawn(async move {
                let _ = handle.send(EngineTabCommand::CloseTab).await;
            });
        }
        self.tab_zoom.borrow_mut().remove(&tab_id);
        self.bookmark_opened.borrow_mut().remove(&tab_id);
        self.forget_mru(tab_id);
        self.refresh_tabs();
    }

    /// Close the currently active tab (Ctrl+W / app.close-tab).
    pub(crate) fn close_active_tab(&self) {
        if let Some(tab_id) = self.active_tab_id() {
            self.close_tab(tab_id);
        }
    }

    /// Whether "Reopen Closed Tab" has anything to reopen.
    pub(crate) fn has_closed_tabs(&self) -> bool {
        !self.beacon.borrow().closed().is_empty()
    }

    /// Reopen the most recently closed tab at its old position (Ctrl+Shift+T).
    pub(crate) fn reopen_closed_tab(&self) {
        let Some(closed) = self.beacon.borrow_mut().closed_mut().pop() else {
            return;
        };
        if let Some(tab_id) = self.open_tab(closed.position, &closed.url, "New Tab") {
            self.activate_tab(tab_id);
        }
    }

    /// Show the source of `inner` in the tab: fetch it, build the line-numbered page
    /// and push it via `LoadHtml`. `raw` skips highlighting (the `raw:` prefix).
    fn load_view_source(&self, tab_id: TabId, inner: &url::Url, raw: bool) {
        let prefix = if raw { "raw:" } else { "view-source:" };
        let display = url::Url::parse(&format!("{prefix}{inner}")).unwrap_or_else(|_| inner.clone());

        let mut manager = self.tab_manager.lock().unwrap();
        let Some(mut tab) = manager.get_tab(tab_id) else {
            return;
        };
        tab.set_favicon(None);
        tab.set_title(display.as_str());
        tab.set_url(display.clone());
        tab.set_loading(false);
        manager.update_tab(tab_id, &tab);
        let handle = tab.tab_handle();
        drop(manager);
        self.refresh_tabs();

        let Some(handle) = handle else {
            return;
        };
        let sender = self.get_sender();
        let inner = inner.clone();
        // Read the UA off the engine here, on the main thread: the settings store is not
        // reachable from the spawned task.
        let user_agent = self
            .engine
            .borrow()
            .as_ref()
            .map(|e| e.settings().get_string("net.user_agent"))
            .unwrap_or_default();
        runtime().spawn(async move {
            match beacon_core::source_page::load(&inner, !raw, user_agent).await {
                Ok(html) => {
                    let _ = handle
                        .send(EngineTabCommand::LoadHtml {
                            html,
                            base_url: display.to_string(),
                        })
                        .await;
                    let _ = handle.send(EngineTabCommand::ResumeDrawing { fps: 30 }).await;
                }
                Err(e) => {
                    let _ = sender.send(Message::Log(format!("View source failed: {e}"))).await;
                }
            }
        });
    }

    /// Persist which tabs are open, so the next start can restore them. Called after
    /// every tab mutation; private windows leave no trace.
    pub(crate) fn save_session(&self) {
        if self.private.get() {
            return;
        }
        let active = self.active_tab_id();
        let manager = self.tab_manager.lock().unwrap();
        let tabs: Vec<beacon_core::session::SessionTab> = manager
            .order()
            .iter()
            .filter_map(|id| {
                manager.get_tab(*id).map(|t| beacon_core::session::SessionTab {
                    url: t.url().to_string(),
                    pinned: t.is_pinned(),
                    active: Some(*id) == active,
                })
            })
            .collect();
        drop(manager);
        if !tabs.is_empty() {
            beacon_core::session::save(&tabs);
        }
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
                        self.build_shell_page(tab.url())
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

        // Chip count may have changed, so the per-tab width budget has too.
        self.relayout_tab_strip();

        // Loading state may have changed for the active tab.
        self.update_reload_button();
        self.save_session();
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
        if beacon_core::error_page::is_cancellation(error) {
            return;
        }

        let mut manager = self.tab_manager.lock().unwrap();
        if let Some(mut tab) = manager.get_tab(our_id) {
            tab.set_loading(false);
            manager.update_tab(our_id, &tab);
        }
        drop(manager);
        self.refresh_tabs();
        self.load_error_page(our_id, url.as_str(), url.as_str(), error);
    }

    /// A typed address that is no URL at all. There is nothing to navigate to, so the error
    /// page loads as `about:blank` and names what was typed. The tab takes that URL too:
    /// left on the previous one, a tab that was on a shell page would stay on it. What was
    /// typed stays in the address bar, and a reload tries it again.
    fn show_invalid_address(&self, tab_id: TabId, address: &str, error: &str) {
        let blank = url::Url::parse("about:blank").expect("about:blank parses");
        let mut manager = self.tab_manager.lock().unwrap();
        let Some(mut tab) = manager.get_tab(tab_id) else {
            return;
        };
        tab.set_favicon(None);
        tab.set_url(blank.clone());
        tab.set_typed_address(Some(address.to_string()));
        tab.set_loading(false);
        tab.set_showing_error(true);
        manager.update_tab(tab_id, &tab);
        drop(manager);
        self.refresh_tabs();
        self.load_error_page(tab_id, address, blank.as_str(), error);
    }

    /// A page's server showed another certificate than the one first seen there, and the
    /// load was refused. Asked about in the window when it is the active tab's: trusting
    /// stores exactly the certificate shown here and loads the page again. A background
    /// tab keeps its error page, and asks when it is reloaded in front.
    fn on_certificate_changed(&self, tab_id: TabId, url: url::Url, fingerprint: [u8; 32]) {
        if self.active_tab_id() != Some(tab_id) {
            return;
        }
        let server = match url.port() {
            Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
            None => url.host_str().unwrap_or_default().to_string(),
        };
        let sha256 = fingerprint.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":");
        let dialog = gtk4::AlertDialog::builder()
            .modal(true)
            .message(format!("The certificate of {server} changed"))
            .detail(format!(
                "{server} showed another certificate than on your first visit. A server that \
                 renewed its certificate does this, and so does someone intercepting the \
                 connection.\n\nThe new certificate is SHA-256\n{sha256}\n\nTrust it only if you \
                 expected the change, or can check the fingerprint with the server's owner."
            ))
            .buttons(["Cancel", "Trust and Reload"])
            .cancel_button(0)
            .default_button(0)
            .build();
        let window = self.obj().clone();
        dialog.choose(Some(&*self.obj()), gtk4::gio::Cancellable::NONE, move |answer| {
            if answer.ok() != Some(1) {
                return;
            }
            let imp = window.imp();
            let trusted = imp
                .engine
                .borrow()
                .as_ref()
                .is_some_and(|engine| engine.trust_certificate(&url, &fingerprint));
            if trusted {
                imp.log(&format!("Trusted the new certificate of {server}"));
                let _ = imp.get_sender().send_blocking(Message::LoadUrl(tab_id, url.to_string()));
            }
        });
    }

    /// A page's server asked for a client certificate, and the load failed. Asked about in
    /// the window when it is the active tab's, like a changed certificate. With none
    /// presented the user may make one; with one presented the server refused it, and the
    /// user may forget it or make another in its place. Nothing is made without asking: the
    /// server learns who the user is from it.
    fn on_identity_asked(&self, tab_id: TabId, url: url::Url, status: u8, message: String, presented: bool) {
        if self.active_tab_id() != Some(tab_id) {
            return;
        }
        let server = match url.port() {
            Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
            None => url.host_str().unwrap_or_default().to_string(),
        };
        if !presented {
            self.ask_identity_name(tab_id, url, server, message);
            return;
        }
        let refusal = if status == 62 {
            "says the identity you gave it is not valid"
        } else {
            "does not accept the identity you gave it"
        };
        let dialog = gtk4::AlertDialog::builder()
            .modal(true)
            .message(format!("{server} {refusal}"))
            .detail(format!(
                "The server says: {message}\n\nYou can keep the identity, forget it so this \
                 place no longer gets it, or make a new one in its place."
            ))
            .buttons(["Keep", "Forget", "Make a New One"])
            .cancel_button(0)
            .default_button(0)
            .build();
        let window = self.obj().clone();
        dialog.choose(Some(&*self.obj()), gtk4::gio::Cancellable::NONE, move |answer| {
            let imp = window.imp();
            match answer.ok() {
                Some(1) => {
                    let forgot = imp.engine.borrow().as_ref().is_some_and(|engine| engine.forget_identity(&url));
                    if forgot {
                        imp.log(&format!("Forgot the identity for {server}"));
                    }
                }
                Some(2) => imp.ask_identity_name(tab_id, url, server, message),
                _ => {}
            }
        });
    }

    /// Ask for a name for a new identity for the place `url` is in, make it, and load `url`
    /// again with it. The name goes into the certificate, where the server reads it.
    fn ask_identity_name(&self, tab_id: TabId, url: url::Url, server: String, message: String) {
        let place = format!("{server}{}", url.path());
        let dialog = gtk4::Window::builder()
            .title(format!("Identity for {server}"))
            .modal(true)
            .resizable(false)
            .transient_for(&*self.obj())
            .build();
        let content = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
        content.set_margin_top(18);
        content.set_margin_bottom(18);
        content.set_margin_start(18);
        content.set_margin_end(18);
        let heading = gtk4::Label::new(Some(&format!("{server} asks who you are")));
        heading.add_css_class("title-4");
        heading.set_xalign(0.0);
        let said = gtk4::Label::new(Some(&format!("The server says: {message}")));
        said.set_wrap(true);
        said.set_xalign(0.0);
        let explained = gtk4::Label::new(Some(&format!(
            "Beacon can make a certificate for you to give it. It goes to {place} and the pages \
             under it, and to no other server. The server can recognise you by it on every visit."
        )));
        explained.set_wrap(true);
        explained.set_max_width_chars(48);
        explained.set_xalign(0.0);
        let name_label = gtk4::Label::new(Some("Name the server sees"));
        name_label.set_xalign(0.0);
        let name = gtk4::Entry::builder()
            .text(url.host_str().unwrap_or_default())
            .activates_default(true)
            .build();
        let buttons = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        buttons.set_halign(gtk4::Align::End);
        let cancel = gtk4::Button::with_label("Cancel");
        let make = gtk4::Button::with_label("Make Identity and Reload");
        make.add_css_class("suggested-action");
        buttons.append(&cancel);
        buttons.append(&make);
        for widget in [
            heading.upcast_ref::<gtk4::Widget>(),
            said.upcast_ref(),
            explained.upcast_ref(),
            name_label.upcast_ref(),
            name.upcast_ref(),
            buttons.upcast_ref(),
        ] {
            content.append(widget);
        }
        dialog.set_child(Some(&content));
        dialog.set_default_widget(Some(&make));

        let weak_dialog = dialog.downgrade();
        cancel.connect_clicked(move |_| {
            if let Some(dialog) = weak_dialog.upgrade() {
                dialog.close();
            }
        });
        let window = self.obj().clone();
        let (weak_dialog, weak_name) = (dialog.downgrade(), name.downgrade());
        make.connect_clicked(move |_| {
            let (Some(dialog), Some(name)) = (weak_dialog.upgrade(), weak_name.upgrade()) else {
                return;
            };
            let chosen = name.text().trim().to_string();
            if chosen.is_empty() {
                name.grab_focus();
                return;
            }
            let imp = window.imp();
            let made = imp
                .engine
                .borrow()
                .as_ref()
                .is_some_and(|engine| engine.make_identity(&url, &chosen));
            dialog.close();
            if made {
                imp.log(&format!("Made an identity for {place}"));
                let _ = imp.get_sender().send_blocking(Message::LoadUrl(tab_id, url.to_string()));
            } else {
                imp.log(&format!("Could not make an identity for {place}"));
            }
        });
        dialog.present();
    }

    /// Whether `tab_id` shows the error page (see `GosubTab::showing_error`).
    fn tab_shows_error(&self, tab_id: TabId) -> bool {
        let manager = self.tab_manager.lock().unwrap();
        manager.get_tab(tab_id).is_some_and(|tab| tab.showing_error())
    }

    /// Push the branded error page into a tab whose navigation failed.
    ///
    /// The page itself is `beacon_core::error_page`'s, shared with every other frontend;
    /// what is left here is getting it into this tab.
    /// `address` is what the page names; `base_url` is what the tab loads it as, and has to
    /// parse.
    fn load_error_page(&self, tab_id: TabId, address: &str, base_url: &str, error: &str) {
        let html = beacon_core::error_page::build(address, error);

        let manager = self.tab_manager.lock().unwrap();
        let Some(handle) = manager.get_tab(tab_id).and_then(|t| t.tab_handle()) else {
            return;
        };
        drop(manager);

        let base_url = base_url.to_string();
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
    /// Narrowest a tab title may get before the strip scrolls instead of shrinking.
    const TAB_LABEL_MIN_CHARS: i32 = 5;
    /// Widest a tab title gets, however few tabs are open.
    const TAB_LABEL_MAX_CHARS: i32 = 16;

    /// Re-share the strip width whenever the viewport resizes. The horizontal adjustment's
    /// page size *is* the viewport width, and unlike the widget there is a signal for it.
    ///
    /// Also makes the wheel scroll the strip. A `ScrolledWindow` whose vertical policy is
    /// `never` does NOT fold a vertical wheel delta into horizontal motion by itself, so
    /// without this controller the wheel over the tab strip does nothing at all and the only
    /// thing that can move the strip is `scroll_active_chip_into_view`.
    fn setup_tab_strip_sizing(&self) {
        let window = self.obj().clone();
        self.tab_scroller.hadjustment().connect_page_size_notify(move |_| {
            window.imp().relayout_tab_strip();
        });

        let scroll =
            gtk4::EventControllerScroll::new(gtk4::EventControllerScrollFlags::BOTH_AXES | gtk4::EventControllerScrollFlags::DISCRETE);
        let scroller = self.tab_scroller.get();
        scroll.connect_scroll(move |_, dx, dy| {
            let adj = scroller.hadjustment();
            // A plain wheel reports on the vertical axis and a tilt wheel on the horizontal
            // one; the strip only moves sideways, so take whichever axis actually moved.
            let delta = if dx.abs() > dy.abs() { dx } else { dy };
            if delta == 0.0 {
                return glib::Propagation::Proceed;
            }
            let step = adj.step_increment().max(48.0);
            let max = (adj.upper() - adj.page_size()).max(adj.lower());
            adj.set_value((adj.value() + delta * step).clamp(adj.lower(), max));
            glib::Propagation::Stop
        });
        self.tab_scroller.add_controller(scroll);
    }

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

    /// The title label inside a chip. Pinned chips are icon-only and have none.
    fn chip_label(chip: &gtk4::Box) -> Option<gtk4::Label> {
        let content = Self::chip_toggle(chip)?.child()?;
        let mut child = content.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            if let Ok(label) = widget.downcast::<gtk4::Label>() {
                return Some(label);
            }
        }
        None
    }

    /// Share the strip's width between the chips that have a title, narrowing them as tabs
    /// multiply and widening them again as tabs close.
    ///
    /// This has to be driven from code. The strip lives in a `ScrolledWindow`, whose viewport
    /// always allocates the child at least its natural width, so chips can never be squeezed
    /// by the layout itself - their natural width has to come down, and for an ellipsizing
    /// label that means lowering its char bounds. Below [`Self::TAB_LABEL_MIN_CHARS`] the
    /// width stops shrinking and the strip scrolls instead, which is where
    /// `scroll_active_chip_into_view` takes over.
    fn relayout_tab_strip(&self) {
        let avail = self.tab_scroller.width();
        if avail <= 0 {
            return; // not allocated yet; the resize hook will call us again
        }

        // Pinned chips are icon-only and keep their natural size: only titled chips flex.
        // The + button is not in here -- it sits outside the scroller so it stays reachable.
        let mut fixed = 0;
        let mut children = 0;
        let mut flexible: Vec<(gtk4::Box, gtk4::Label)> = Vec::new();
        let mut child = self.tab_strip.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            children += 1;
            match widget.downcast::<gtk4::Box>() {
                Ok(chip) => match Self::chip_label(&chip) {
                    Some(label) => flexible.push((chip, label)),
                    None => fixed += chip.measure(gtk4::Orientation::Horizontal, -1).1,
                },
                Err(other) => fixed += other.measure(gtk4::Orientation::Horizontal, -1).1,
            }
        }
        if flexible.is_empty() {
            return;
        }

        // Everything in a chip that is not the title: favicon, padding, close button. Measured
        // rather than assumed, so it survives theme and icon-size changes.
        let overhead = flexible
            .iter()
            .map(|(chip, label)| chip.measure(gtk4::Orientation::Horizontal, -1).1 - label.measure(gtk4::Orientation::Horizontal, -1).1)
            .max()
            .unwrap_or(0);

        let spacing = self.tab_strip.spacing() * (children - 1).max(0);
        let per_chip = (avail - fixed - spacing) / flexible.len() as i32;

        // One width for all of them, so tabs stay uniform as they narrow. The char count is
        // only an estimate - `approximate_char_width` is an average, and chip chrome does not
        // scale with it - so measure the result and step down until it genuinely fits. Without
        // the check a nine-tab strip overflowed by a few pixels and scrolled needlessly.
        let char_width = (flexible[0].1.pango_context().metrics(None, None).approximate_char_width() / gtk4::pango::SCALE).max(1);
        let mut chars = ((per_chip - overhead) / char_width).clamp(Self::TAB_LABEL_MIN_CHARS, Self::TAB_LABEL_MAX_CHARS);
        loop {
            for (_, label) in &flexible {
                // Only touch labels that actually change: assigning here triggers a resize,
                // and the resize hook calls back into this function.
                if label.max_width_chars() != chars {
                    label.set_width_chars(chars);
                    label.set_max_width_chars(chars);
                }
            }
            // At the floor we stop shrinking and let the strip scroll instead.
            if chars <= Self::TAB_LABEL_MIN_CHARS || self.tab_strip.measure(gtk4::Orientation::Horizontal, -1).1 <= avail {
                return;
            }
            chars -= 1;
        }
    }

    /// Keep the strip scrolled so the active chip is visible. Without this, activating a tab
    /// that has overflowed the strip leaves no chip highlighted at all.
    fn scroll_active_chip_into_view(&self) {
        let Some(chip) = self.active_tab_id().and_then(|id| self.chip_for_tab(id)) else {
            return;
        };
        let scroller = self.tab_scroller.get();

        // A chip inserted this frame has no allocation yet, so the bounds below would be
        // meaningless. Wait until the layout has run.
        glib::idle_add_local_once(move || {
            let Some(bounds) = chip.compute_bounds(&scroller) else { return };
            let adj = scroller.hadjustment();
            // Bounds are relative to the visible viewport, so they are already scroll-adjusted.
            let (left, right) = (bounds.x() as f64, (bounds.x() + bounds.width()) as f64);
            if left < 0.0 {
                adj.set_value(adj.value() + left);
            } else if right > adj.page_size() {
                adj.set_value(adj.value() + right - adj.page_size());
            }
        });
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

    /// Move `tab_id` to the front of the MRU list (inserting it if new).
    pub(crate) fn touch_mru(&self, tab_id: TabId) {
        self.beacon.borrow_mut().mru_mut().touch(tab_id);
    }

    /// Drop a closed tab from the MRU list, abandoning the cycle if too little is left.
    pub(crate) fn forget_mru(&self, tab_id: TabId) {
        if self.beacon.borrow_mut().mru_mut().forget(tab_id) {
            self.clear_cycle_timer();
        }
    }

    /// Ctrl+Tab (`+1`) / Ctrl+Shift+Tab (`-1`): step through the MRU list.
    ///
    /// The list is deliberately not reordered while stepping, so repeated presses reach the
    /// third tab and beyond rather than bouncing between two. A short idle timer ends the
    /// cycle and commits wherever it landed.
    pub(crate) fn cycle_tab(&self, direction: i32) {
        let Some(target) = self.beacon.borrow_mut().mru_mut().step(direction) else {
            return;
        };
        self.activate_tab(target);
        self.restart_cycle_timer();
    }

    /// Restart the settle timer; when it fires the cycle is over.
    fn restart_cycle_timer(&self) {
        if let Some(id) = self.cycle_timer.borrow_mut().take() {
            id.remove();
        }
        let window = self.obj().downgrade();
        let id = glib::timeout_add_local_once(std::time::Duration::from_millis(1200), move || {
            if let Some(window) = window.upgrade() {
                window.imp().commit_cycle();
            }
        });
        *self.cycle_timer.borrow_mut() = Some(id);
    }

    /// End the cycle, promoting the tab it landed on to the front of the MRU list.
    fn commit_cycle(&self) {
        self.cycle_timer.borrow_mut().take();
        let active = self.active_tab_id();
        self.beacon.borrow_mut().mru_mut().commit(active);
    }

    /// Drop the settle timer. The list has already ended the cycle on its own side.
    fn clear_cycle_timer(&self) {
        if let Some(id) = self.cycle_timer.borrow_mut().take() {
            id.remove();
        }
    }

    /// Ctrl+1..8 select that position in the visible strip; Ctrl+9 is always the last tab,
    /// matching Chrome and Firefox.
    pub(crate) fn select_tab_by_position(&self, n: usize) {
        let order = self.tab_manager.lock().unwrap().order();
        if order.is_empty() {
            return;
        }
        let idx = if n >= 9 { order.len() - 1 } else { n.saturating_sub(1) };
        if let Some(tab_id) = order.get(idx).copied() {
            self.activate_tab(tab_id);
        }
    }

    /// Ask the engine what sits under `(x, y)` in `tab_id`'s render area, recording what the
    /// answer is for. Shared by right-click (context menu) and Ctrl/middle-click (open link
    /// in a background tab) so the token bookkeeping exists in exactly one place.
    pub(crate) fn query_hit_test(&self, tab_id: TabId, gesture: &gtk4::GestureClick, x: f64, y: f64, intent: HitIntent) {
        let handle = {
            let manager = self.tab_manager.lock().unwrap();
            manager.get_tab(tab_id).and_then(|t| t.tab_handle())
        };
        let Some(handle) = handle else { return };
        let window = self.obj().clone();
        let Some(widget) = gesture.widget() else { return };
        // Anchor point in window coordinates, for whatever the answer wants to position.
        let Some(p) = widget.compute_point(&window, &Point::new(x as f32, y as f32)) else {
            return;
        };
        let token = self.next_hit_test_token.get();
        self.next_hit_test_token.set(token + 1);
        self.pending_hit_tests.borrow_mut().insert(token, (tab_id, p, intent));
        // The engine works in CSS px; the click arrives in zoomed widget px.
        let z = self.tab_zoom.borrow().get(&tab_id).map(|c| c.get()).unwrap_or(1.0);
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
    }

    pub(crate) fn activate_tab(&self, tab_id: TabId) {
        // A background tab that keeps drawing at 30 fps is a laptop-fan bug, and with one
        // shared GPU context it competes with the tab the user is actually looking at. The
        // tab keeps loading either way: suspending stops its rendering, nothing else.
        //
        // Read before `mark_active` below, which is what makes the new tab the active one.
        let outgoing = self.active_tab_id().filter(|previous| *previous != tab_id);
        if let Some(previous) = outgoing {
            let handle = {
                let manager = self.tab_manager.lock().unwrap();
                manager.get_tab(previous).and_then(|tab| tab.tab_handle())
            };
            if let Some(handle) = handle {
                runtime().spawn(async move {
                    let _ = handle.send(EngineTabCommand::SuspendDrawing).await;
                });
            }
        }

        // A cycle in progress is *previewing* tabs; reordering now would collapse the walk
        // into a two-tab ping-pong. `commit_cycle` promotes the landing tab instead.
        self.touch_mru(tab_id);
        for chip in self.chips() {
            Self::set_chip_active(&chip, chip.get_tab_id() == Some(tab_id));
        }
        // Correct the engine's viewport for this tab BEFORE it is shown, so its GLArea's
        // first-ever resize does not land as a change and trigger a full re-layout.
        self.sync_viewport_for(tab_id);
        if let Some(page) = self.page_for_tab(tab_id) {
            self.content_stack.set_visible_child(&page);
        }
        // ...and only now start it drawing again, with the viewport it is about to be shown
        // at rather than the one the last active tab left behind.
        if outgoing.is_some() {
            let handle = {
                let manager = self.tab_manager.lock().unwrap();
                manager.get_tab(tab_id).and_then(|tab| tab.tab_handle())
            };
            if let Some(handle) = handle {
                runtime().spawn(async move {
                    let _ = handle.send(EngineTabCommand::ResumeDrawing { fps: 30 }).await;
                });
            }
        }

        let mut manager = self.tab_manager.lock().unwrap();
        // The tab is on screen now, so the manager -- which `active_tab_id` reads -- has to
        // agree. `mark_active`, not `set_active`: the latter queues an Activate command that
        // refresh_tabs would replay straight back into here.
        manager.mark_active(tab_id);
        if let Some(tab) = manager.get_tab(tab_id) {
            // New-tab pages (blank, home) get an empty address bar, ready to type into.
            let page = gosub_engine::internal_pages::InternalPages::page_name(tab.url());
            if tab.typed_address().is_none() && matches!(tab.url().scheme(), "gosub" | "about") && matches!(page, "blank" | "home") {
                self.show_address(None);
            } else {
                self.show_tab_address(&tab);
            }
            let suffix = if self.private.get() { " (Private)" } else { "" };
            self.obj().set_title(Some(&format!("{} — Gosub Beacon{suffix}", tab.title())));
        }
        drop(manager);
        // The raster DPR is a process-wide atomic: re-store it for this tab's zoom.
        {
            let zoom_level = self.tab_zoom.borrow().get(&tab_id).map(|z| z.get()).unwrap_or(1.0);
            if let Some(area) = self.render_areas.borrow().get(&tab_id) {
                use gosub_render_pipeline::render::DEVICE_PIXEL_RATIO;
                let raster_dpr = ((crate::render::render_dpr(area) as f64 * zoom_level).ceil() as u32).clamp(1, 4);
                DEVICE_PIXEL_RATIO.store(raster_dpr, std::sync::atomic::Ordering::Relaxed);
            }
        }
        self.scroll_active_chip_into_view();
        self.searchbar.set_progress_fraction(0.0);
        self.update_nav_buttons();
        self.update_reload_button();
        self.update_bookmark_button();
        self.save_session();
    }

    /// A tab chip: a `Box` holding the label toggle and, beside it, the close button.
    ///
    /// The close button MUST be a sibling of the toggle, never a descendant - see [`Self::chips`].
    fn create_tab_chip(&self, tab: &GosubTab) -> gtk4::Box {
        let chip = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        chip.add_css_class("tab-chip");
        chip.set_tab_id(tab.id());

        // Middle-click closes the tab. On the chip itself rather than its toggle: the close
        // button is a sibling, and a gesture on the toggle would miss clicks landing on it.
        let middle = gtk4::GestureClick::new();
        middle.set_button(gdk::BUTTON_MIDDLE);
        let window = self.obj().clone();
        let tab_id = tab.id();
        middle.connect_pressed(move |_g, _n, _x, _y| {
            let imp = window.imp();
            imp.close_tab(tab_id);
            _ = imp.get_sender().send_blocking(Message::RefreshTabs());
        });
        chip.add_controller(middle);

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
            let close = self.create_tab_close_button(tab);
            // The toggle is already packed, so prepend puts the button before the label.
            if self.close_button_on_left() {
                close.set_halign(gtk4::Align::Start);
                chip.prepend(&close);
            } else {
                chip.append(&close);
            }
        }

        Self::set_chip_active(chip, was_active);
    }

    /// The chip's X button. Lives beside the label toggle, so its clicks actually arrive.
    /// The configured homepage (`useragent.general.homepage`, registered by beacon in
    /// `BrowserEngine::new`). Falls back to the branded start page if the engine is not up
    /// yet or the value was cleared.
    fn homepage(&self) -> String {
        self.engine
            .borrow()
            .as_ref()
            .map(|e| e.settings().get_string("useragent.general.homepage"))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "gosub://home".to_string())
    }

    /// Whether the tab close button goes on the left of the label, from
    /// `useragent.tab.close_button` in the engine settings store (`left` | `right`).
    /// Falls back to the right when the engine is not up yet, which is where the button
    /// sat before the setting was honoured.
    ///
    /// The setting is a map (`m:right` by default), so read it with `get_map`: `get_string`
    /// works but logs "setting is not a string" on every chip it builds.
    fn close_button_on_left(&self) -> bool {
        self.engine.borrow().as_ref().is_some_and(|e| {
            e.settings()
                .get_map("useragent.tab.close_button")
                .first()
                .is_some_and(|side| side == "left")
        })
    }

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
        if let Some(texture) = tab.favicon().and_then(decode_favicon) {
            let img = Image::from_paintable(Some(&texture));
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
        } else if let Some(texture) = tab.favicon().and_then(decode_favicon) {
            let img = Image::from_paintable(Some(&texture));
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
    /// other navigation. Two exceptions: `gosub://config`, whose engine version is a
    /// read-only dump (it cannot do forms yet), so Beacon renders its own editable GTK
    /// page for it; and `gosub://dive`, the game, which is pixels the shell shows
    /// (see `beacon_core::dive`). Everything else - home, help, blank, version, history,
    /// unknown pages - goes to the engine.
    fn is_shell_rendered(url: &url::Url) -> bool {
        beacon_core::settings::is_editor_url(url) || beacon_core::dive::is_dive(url)
    }

    /// What a shell-rendered page's tab is titled.
    fn shell_page_title(url: &url::Url) -> &'static str {
        if beacon_core::dive::is_dive(url) {
            beacon_core::dive::TITLE
        } else {
            "Engine settings"
        }
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

        self.beacon.borrow_mut().bind_engine_tab(handle.tab_id, tab_id);
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

    /// The shell-rendered pages (see `is_shell_rendered`): the game, or the
    /// `gosub://config` editor.
    fn build_shell_page(&self, url: &url::Url) -> Widget {
        if beacon_core::dive::is_dive(url) {
            return super::dive_page::build();
        }
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
        // The tab manager is the truth, not the stack's visible child: core has to be able
        // to answer this too, and two sources would drift.
        self.tab_manager.lock().unwrap().active()
    }

    /// Back button: the engine owns session history, so just ask it to go back. It answers
    /// with `HistoryChanged` (cursor moved) and the usual navigation events for the reload.
    pub(crate) fn navigate_back(&self) {
        self.dispatch(BeaconCommand::Back);
    }

    /// Forward button: with a single forward branch go straight there; with several, pop up a
    /// menu (anchored to `anchor`) asking which branch to follow.
    pub(crate) fn navigate_forward(&self, anchor: &Button) {
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
            [_] => self.dispatch(BeaconCommand::Forward(None)),
            _ => self.show_forward_menu(anchor, children),
        }
    }

    /// Navigate the active tab to any entry in its history tree: a forward branch from the
    /// forward menu, or any line of the history page.
    fn go_to_history_entry(&self, entry: HistoryEntryId) {
        self.dispatch(BeaconCommand::GoToHistoryEntry(entry));
    }

    /// The zone's places store (bookmarks + history), once the engine is up.
    fn places(&self) -> Option<gosub_engine::places::PlacesHandle> {
        self.engine.borrow().as_ref().map(|e| e.places())
    }

    /// Star button: toggles a bookmark for the active tab.
    fn setup_bookmark_button(&self) {
        let window = self.obj().clone();
        self.btn_bookmark.connect_clicked(move |_| {
            window.imp().toggle_bookmark();
        });
    }

    /// Put `url` in the address bar, with what the engine says about its connection: a
    /// padlock when encrypted, "not secure" when cleartext, nothing for local and internal
    /// pages. `None` empties the bar for typing.
    fn show_address(&self, url: Option<&url::Url>) {
        use gosub_engine::net::protocols::{transport, Transport};
        self.searchbar.set_text(url.map_or("", |u| u.as_str()));
        let (icon, tooltip) = match url.and_then(transport) {
            Some(Transport::Encrypted) => (Some("channel-secure-symbolic"), Some("Secure connection")),
            Some(Transport::Cleartext) => (Some("channel-insecure-symbolic"), Some("Not secure")),
            None => (None, None),
        };
        self.searchbar.set_icon_from_icon_name(gtk4::EntryIconPosition::Primary, icon);
        self.searchbar.set_icon_tooltip_text(gtk4::EntryIconPosition::Primary, tooltip);
    }

    /// The address bar for `tab`: what was typed while it shows the error page for an
    /// address that does not parse, its URL otherwise.
    fn show_tab_address(&self, tab: &GosubTab) {
        match tab.typed_address() {
            Some(typed) => {
                self.show_address(None);
                self.searchbar.set_text(typed);
            }
            None => self.show_address(Some(tab.url())),
        }
    }

    /// Add or remove the active tab's page from the bookmarks. Shared by the toolbar star and
    /// the `app.bookmark-page` action (Ctrl+D), so both behave identically.
    pub(crate) fn toggle_bookmark(&self) {
        let Some(places) = self.places() else { return };
        let Some(tab_id) = self.active_tab_id() else { return };
        let (url, title) = {
            let manager = self.tab_manager.lock().unwrap();
            match manager.get_tab(tab_id) {
                Some(tab) => {
                    // Pages from the network are bookmarkable, over whichever protocol the
                    // engine speaks; internal pages and local files are not.
                    if !gosub_engine::net::protocols::is_network_place(tab.url()) {
                        return;
                    }
                    (tab.url().to_string(), tab.title().to_string())
                }
                None => return,
            }
        };
        if places.is_bookmarked(&url) {
            places.remove_bookmark(&url);
        } else {
            places.add_bookmark(&url, if title.is_empty() { &url } else { &title });
        }
        self.update_bookmark_button();
        self.rebuild_bookmarks_bar();
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
        let scale = crate::render::render_dpr(&area) as f64;
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
            let content = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
            if let Some(texture) = bookmark.favicon.as_deref().and_then(decode_favicon) {
                let img = Image::from_paintable(Some(&texture));
                img.set_pixel_size(16);
                content.append(&img);
            }
            content.append(&gtk4::Label::new(Some(if bookmark.title.is_empty() {
                &bookmark.url
            } else {
                &bookmark.title
            })));
            let button = Button::builder()
                .child(&content)
                .has_frame(false)
                .tooltip_text(&bookmark.url)
                .build();
            let window = self.obj().clone();
            let url = bookmark.url.clone();
            button.connect_clicked(move |_| {
                let imp = window.imp();
                let Some(tab_id) = imp.active_tab_id() else { return };
                imp.bookmark_opened.borrow_mut().insert(tab_id, url.clone());
                let _ = imp.get_sender().send_blocking(Message::LoadUrl(tab_id, url.clone()));
            });
            self.bookmarks_bar.append(&button);
        }
    }

    /// Give the page's new icon to its bookmark, if it has one, and redraw the bar when that
    /// changed it. The bookmark is the one this tab was opened from, for as long as the tab
    /// stays on that site, or else one that names the tab's own URL however it is spelled.
    fn store_bookmark_favicon(&self, tab_id: TabId) {
        let Some(places) = self.places() else { return };
        let Some((url, favicon)) = self
            .tab_manager
            .lock()
            .unwrap()
            .get_tab(tab_id)
            .and_then(|tab| Some((tab.url().clone(), tab.favicon()?.to_vec())))
        else {
            return;
        };
        // The engine checked that it is an image; only one that draws is worth keeping.
        if decode_favicon(&favicon).is_none() {
            return;
        }
        // A page can report a new icon after its first, so the link to the bookmark it was
        // opened from holds until the tab leaves that site.
        let opened = {
            let mut opened = self.bookmark_opened.borrow_mut();
            let on_site = opened
                .get(&tab_id)
                .is_some_and(|bookmark| url::Url::parse(bookmark).is_ok_and(|bookmark| same_site(&bookmark, &url)));
            if on_site {
                opened.get(&tab_id).cloned()
            } else {
                opened.remove(&tab_id);
                None
            }
        };
        let bookmark = opened.or_else(|| {
            places
                .bookmarks()
                .into_iter()
                .find(|bookmark| url::Url::parse(&bookmark.url).is_ok_and(|bookmark| bookmark == url))
                .map(|bookmark| bookmark.url)
        });
        if bookmark.is_some_and(|bookmark| places.set_bookmark_favicon(&bookmark, &favicon)) {
            self.rebuild_bookmarks_bar();
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

        let beacon = self.beacon.borrow();
        let downloads = beacon.downloads().entries();
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
                let download_id = entry.id;
                let window = self.obj().clone();
                open.connect_clicked(move |_| {
                    window.imp().dispatch(BeaconCommand::OpenDownload(download_id));
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
                DownloadState::Cancelled => {
                    let status = gtk4::Label::new(Some("Cancelled"));
                    status.set_halign(gtk4::Align::Start);
                    status.add_css_class("download-status");
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
        self.beacon.borrow_mut().downloads_mut().start(id, filename, path.to_path_buf());
        self.refresh_downloads();
    }

    /// Ask the user where to save `url` (native save dialog, prefilled with
    /// `suggested_name`), then start the engine download on `tab_id`'s handle.
    /// Open the picker a form control asked for, and report what is done with it.
    ///
    /// The control previews live, so every intermediate value goes straight back as
    /// `PickerChanged`; a cancel answers with the value the control came in with, and
    /// `PickerClosed` ends the exchange either way. A kind the shell cannot draw yet is
    /// logged and left alone -- the engine draws no picker of its own, so nothing opens.
    fn open_picker(&self, tab_id: TabId, kind: PickerKind, value: &str, min: Option<&str>, max: Option<&str>, step: Option<&str>) {
        let manager = self.tab_manager.lock().unwrap();
        let handle = manager.get_tab(tab_id).and_then(|t| t.tab_handle());
        drop(manager);
        let Some(handle) = handle else {
            return;
        };

        // A control that asks while another picker is up replaces it, and the old one is
        // detached so it cannot answer for a control that has moved on.
        if let Some(previous) = self.picker.borrow_mut().take() {
            previous.close();
        }

        let send = move |command: EngineTabCommand| {
            let handle = handle.clone();
            runtime().spawn(async move {
                let _ = handle.send(command).await;
            });
        };
        let on_change = send.clone();
        let on_finish = send;
        let opened = crate::picker::open(
            &*self.obj(),
            kind,
            value,
            min,
            max,
            step,
            move |value| on_change(EngineTabCommand::PickerChanged { value }),
            move |value| {
                on_finish(EngineTabCommand::PickerChanged { value });
                on_finish(EngineTabCommand::PickerClosed);
            },
        );
        match opened {
            Some(picker) => *self.picker.borrow_mut() = Some(picker),
            None => self.log(&format!(
                "{kind:?} picker requested (current {value:?}); not available in the GTK shell yet"
            )),
        }
    }

    /// Ask where to save a download. With `offer` set (the engine offered it) the body it
    /// already spooled is placed; without one (save link as) the URL is fetched.
    pub(crate) fn save_download_as(
        &self,
        tab_id: TabId,
        offer: Option<gosub_engine::events::DownloadOfferId>,
        url: String,
        suggested_name: &str,
    ) {
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
            let id = window.imp().beacon.borrow_mut().downloads_mut().next_id();
            window.imp().log(&format!("Download #{id}: {url} → {}", path.display()));
            window.imp().add_download(id, &path);
            let handle = handle.clone();
            runtime().spawn(async move {
                let _ = handle
                    .send(EngineTabCommand::StartDownload {
                        id: gosub_engine::events::DownloadId(id),
                        url,
                        target_path: path,
                        offer,
                    })
                    .await;
            });
        });
    }

    /// Send a history traversal command to the active tab's engine tab and mark it loading.
    /// Hand a browser gesture to `beacon-core` and draw whatever it says changed.
    pub(crate) fn dispatch(&self, command: BeaconCommand) {
        let events = self.beacon.borrow_mut().apply(command);
        for event in events {
            self.apply_beacon_event(event);
        }
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
            Message::RestoreSession { pinned, active } => {
                // Snapshot insertion order before pinning: pin_tab reorders, but the
                // saved indices refer to the order the tabs were opened in.
                let order = self.tab_manager.lock().unwrap().order();
                {
                    let mut manager = self.tab_manager.lock().unwrap();
                    for index in pinned {
                        if let Some(id) = order.get(index) {
                            manager.pin_tab(*id);
                        }
                    }
                }
                self.refresh_tabs();
                if let Some(id) = active.and_then(|i| order.get(i)) {
                    self.activate_tab(*id);
                }
            }
            Message::OpenTab(url, title) => {
                self.open_tab(None, &url, &title);
            }
            Message::OpenTabForeground(url, title) => {
                // The explicit new-tab gesture switches to what it just made. `open_tab` alone
                // does not activate, which is why Ctrl+T used to leave you on the old tab.
                if let Some(tab_id) = self.open_tab(None, &url, &title) {
                    self.activate_tab(tab_id);
                }
            }
            Message::OpenTabRight(target_tab_id, url, title) => {
                if let Some(pos) = self.get_page_num_for_tab(target_tab_id) {
                    self.open_tab(Some(pos as usize + 1), &url, &title);
                }
            }

            Message::DuplicateTab(tab_id) => {
                // The copy belongs beside its original and takes the focus -- what every
                // browser's Duplicate Tab does, and what the Mac's `duplicateTab` does. It
                // carries the address over, not the session history, so the duplicate opens
                // with a single entry and a dead Back button.
                //
                // Taking the title too means the new tab reads as its original while it
                // loads instead of flashing "New Tab"; `open_tab` re-parses the address, so
                // a `view-source:` tab duplicates as another view-source tab and a
                // shell-rendered one gets its own title back.
                let origin = {
                    let manager = self.tab_manager.lock().unwrap();
                    manager.get_tab(tab_id).map(|tab| (tab.url().to_string(), tab.title().to_string()))
                };
                let (Some((url, title)), Some(pos)) = (origin, self.get_page_num_for_tab(tab_id)) else {
                    return;
                };
                if let Some(new_tab_id) = self.open_tab(Some(pos as usize + 1), &url, &title) {
                    self.activate_tab(new_tab_id);
                }
            }

            Message::LoadUrl(tab_id, url_str) => {
                self.log(format!("Loading URL: {}", url_str).as_str());

                let (view_mode, url) = match GosubAddressParser::parse(url_str.as_str()) {
                    Ok(parsed) => parsed,
                    Err(e) => {
                        self.log("Cannot parse URL");
                        self.show_invalid_address(tab_id, &url_str, &e.to_string());
                        return;
                    }
                };

                // `view-source:`/`raw:` addresses render the target's source in-tab.
                if let Some(raw) = source_mode(&view_mode) {
                    self.load_view_source(tab_id, &url, raw);
                    return;
                }

                // Update information in the given tab with the new url
                let shell = Self::is_shell_rendered(&url);
                let mut manager = self.tab_manager.lock().unwrap();
                // The tab may have closed since this was queued.
                let Some(mut tab) = manager.get_tab(tab_id) else {
                    return;
                };

                tab.set_favicon(None);
                tab.set_title(if shell { Self::shell_page_title(&url) } else { url.as_str() });
                tab.set_url(url.clone());
                // Typing about:blank is no move to set_url, but it is a new address.
                tab.set_typed_address(None);
                tab.set_loading(!shell);
                tab.set_showing_error(false);

                manager.update_tab(tab_id, &tab);
                drop(manager);

                self.refresh_tabs();

                // Everything but the shell-rendered pages is an engine navigation
                // (gosub:// pages included - the engine serves them from its registry).
                // A shell page raises no navigation event, so the address bar is told here.
                if shell {
                    if self.active_tab_id() == Some(tab_id) {
                        self.show_address(Some(&url));
                    }
                } else {
                    self.navigate_engine_tab(tab_id, url.as_str());
                }
            }
            Message::Log(msg) => {
                self.log(msg.as_str());
            }
            Message::PinTab(tab_id) => {
                let mut manager = self.tab_manager.lock().unwrap();
                manager.pin_tab(tab_id);
                drop(manager);

                // Update tab-bar
                self.refresh_tabs();
            }
            Message::FaviconLoaded(tab_id, bytes) => {
                // Decode once here purely to reject what we cannot draw; the tab keeps the
                // encoded bytes, and the chip decodes them again when it is built.
                if decode_favicon(&bytes).is_none() {
                    self.log("Could not decode favicon");
                    return;
                }

                let mut manager = self.tab_manager.lock().unwrap();
                if let Some(mut tab) = manager.get_tab(tab_id) {
                    tab.set_favicon(Some(bytes));
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
    fn open_tab(&self, position: Option<usize>, url_str: &str, title: &str) -> Option<TabId> {
        let Ok((render_mode, url)) = GosubAddressParser::parse(url_str) else {
            self.log("Cannot parse URL");
            return None;
        };

        let mut tab = GosubTab::new(url.clone(), title);
        let tab_id = tab.id();

        // Create the matching engine-side tab and remember the id mapping.
        let handle = {
            let mut eng = self.engine.borrow_mut();
            let Some(eng) = eng.as_mut() else {
                self.log("Engine not ready");
                return None;
            };
            let viewport = self.viewport_for_new_tab();
            match eng.create_tab(runtime(), title, viewport) {
                Ok(h) => h,
                Err(e) => {
                    self.log(format!("Failed to create engine tab: {e}").as_str());
                    return None;
                }
            }
        };
        self.beacon.borrow_mut().bind_engine_tab(handle.tab_id, tab_id);
        tab.set_tab_handle(handle);

        let shell = Self::is_shell_rendered(&url);
        if shell {
            tab.set_title(Self::shell_page_title(&url));
        }
        tab.set_loading(!shell);

        // add tab to manager, and notify the tab has changed. This will update the
        // tab-bar during a refresh-tabs call.
        let mut manager = self.tab_manager.lock().unwrap();
        manager.add_tab(tab, position);
        manager.notify_tab_changed(tab_id);
        drop(manager);
        // Enter the MRU list at the *back*: a freshly opened tab is cycleable, but it has
        // not been used yet. Without this only activated tabs were ever in the list, so
        // opening several tabs and never switching left it with one entry and Ctrl+Tab
        // silently did nothing.
        self.beacon.borrow_mut().mru_mut().insert_unused(tab_id);
        self.refresh_tabs();

        if let Some(raw) = source_mode(&render_mode) {
            self.load_view_source(tab_id, &url, raw);
        } else if !shell {
            self.navigate_engine_tab(tab_id, url.as_str());
        }
        Some(tab_id)
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
                let scale = crate::render::render_dpr(area) as i32;
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
                let raster_dpr = ((crate::render::render_dpr(area) as f64 * z).ceil() as u32).clamp(1, 4);
                DEVICE_PIXEL_RATIO.store(raster_dpr, std::sync::atomic::Ordering::Relaxed);
                // GtkGLArea's resize reports PHYSICAL pixels; the engine lays out in CSS
                // px, which shrink as the zoom grows.
                let eff = crate::render::render_dpr(area) as f64 * z;
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
                // A touchpad reports surface pixels, already smooth, which the engine applies
                // as they come; a wheel reports notches, which are scaled and animated.
                let precise = c.unit() == gdk::ScrollUnit::Surface;
                let step = if precise { 1.0 } else { 40.0 };
                // The engine scrolls in CSS px, which cover more screen when zoomed in.
                let z = scroll_zoom.get() as f32;
                let delta_x = dx as f32 * step / z;
                let delta_y = dy as f32 * step / z;
                runtime().spawn(async move {
                    let _ = handle.send(EngineTabCommand::MouseScroll { delta_x, delta_y, precise }).await;
                });
                glib::Propagation::Stop
            });
            area.add_controller(scroll);

            // Mouse move -> hover. The engine resolves the link under the cursor and emits
            // a `HoverUrl` event back to us.
            let motion = gtk4::EventControllerMotion::new();
            let motion_handle = handle.clone();
            let motion_zoom = zoom.clone();
            // GTK reports motion when the widget under the pointer changes, not only when
            // the pointer does -- so a page that keeps painting produces a stream of events
            // at a position that has not moved. Each one costs a hit test, and none of them
            // can change what is hovered.
            let last_position = std::cell::Cell::new((f64::NAN, f64::NAN));
            motion.connect_motion(move |_c, x, y| {
                let handle = motion_handle.clone();
                let z = motion_zoom.get();
                let (x, y) = (x / z, y / z);
                if last_position.replace((x, y)) == (x, y) {
                    return;
                }
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
            let click_window = self.obj().clone();
            let click_tab_id = tab.id();
            click.connect_pressed(move |g, _n, x, y| {
                // Keys should go to the page after a click on it.
                if let Some(widget) = g.widget() {
                    widget.grab_focus();
                }
                // Ctrl+click opens a link in a background tab instead of following it. Ask
                // the engine what is under the pointer; the answer decides (see HitIntent).
                if g.current_event_state().contains(gdk::ModifierType::CONTROL_MASK) {
                    click_window
                        .imp()
                        .query_hit_test(click_tab_id, g, x, y, HitIntent::OpenLinkInBackgroundTab);
                    return;
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

            // ...and the release, which is what ends a drag. The engine answers `MouseUp`
            // by clearing the pointer's grip on a range, a resize handle, a scrollbar thumb
            // or a selection -- so a shell that only ever presses leaves every one of those
            // grabbed for good.
            //
            // Sent unconditionally, including after a Ctrl+click that went to the hit test
            // instead of the engine: an unpaired release only clears state that was never
            // set, while a missing one is the bug this exists to avoid. The modifier can
            // also be let go before the button is, so the press and the release cannot be
            // relied on to agree about it.
            let release_handle = handle.clone();
            let release_zoom = zoom.clone();
            click.connect_released(move |_g, _n, x, y| {
                let handle = release_handle.clone();
                let z = release_zoom.get();
                let (x, y) = (x / z, y / z);
                runtime().spawn(async move {
                    let _ = handle
                        .send(EngineTabCommand::MouseUp {
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

            // What the page receives for a press is one `KeyDown` carrying the character
            // the input method made of it, with `TextInput` kept for commits that are not a
            // single press -- a CJK composition, or a paste. That is the engine's contract
            // (`edit_key` inserts from the key), and the Mac shell's `PageView.keyDown`
            // follows the same one.
            //
            // Only the input method knows what a press produced: Compose then e is one "é",
            // not an "e" and an "é", and a composition may commit several characters at once
            // or none yet. So the press goes to it first and the named key -- Backspace,
            // ArrowLeft, Enter -- is sent only when it made nothing of it.
            let im = gtk4::IMMulticontext::new();
            im.set_client_widget(Some(&area));

            /// The press being interpreted: its `code` and modifiers, for the commit to
            /// answer as.
            type PressInFlight = Rc<RefCell<Option<(String, gosub_engine::events::Modifiers)>>>;
            let press_in_flight: PressInFlight = Rc::new(RefCell::new(None));
            let committed = Rc::new(Cell::new(false));

            let im_handle = handle.clone();
            let commit_press = press_in_flight.clone();
            let commit_flag = committed.clone();
            im.connect_commit(move |_, text| {
                // Control characters arrive here too (Ctrl+A is U+0001). They are not text;
                // the named-key path below is the one that reports those.
                if text.is_empty() || text.chars().any(|c| (c as u32) < 0x20) {
                    return;
                }
                let handle = im_handle.clone();
                let press = commit_press.borrow().clone();
                commit_flag.set(true);
                match press {
                    // A single character is the press itself, as the input method resolved
                    // it: "a", " ", "é". Sent as the key so the engine can treat it as one --
                    // typing into a field, but also space scrolling a page with nothing
                    // focused -- and never as text on top.
                    Some((code, modifiers)) if text.chars().count() == 1 => {
                        let key = text.to_string();
                        runtime().spawn(async move {
                            let _ = handle.send(EngineTabCommand::KeyDown { key, code, modifiers }).await;
                        });
                    }
                    // Several at once is a composition being committed, or a commit that
                    // arrived on its own. Text, not a key.
                    _ => {
                        let text = text.to_string();
                        runtime().spawn(async move {
                            let _ = handle.send(EngineTabCommand::TextInput { text }).await;
                        });
                    }
                }
            });

            // An input method only composes for the focused widget, and has to be told.
            let focus = gtk4::EventControllerFocus::new();
            focus.connect_enter({
                let im = im.clone();
                move |_| im.focus_in()
            });
            focus.connect_leave({
                let im = im.clone();
                move |_| im.focus_out()
            });
            area.add_controller(focus);

            let key_handle = handle.clone();
            let press_im = im.clone();
            let error_window = self.obj().clone();
            let error_tab_id = tab.id();
            keys.connect_key_pressed(move |c, keyval, _keycode, state| {
                if state.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK | gdk::ModifierType::SUPER_MASK) {
                    return glib::Propagation::Proceed;
                }
                // Space on the page a failed navigation shows starts the game
                // (the browser's dinosaur); the page itself has nothing to take it.
                if keyval == gdk::Key::space && error_window.imp().tab_shows_error(error_tab_id) {
                    let _ = error_window
                        .imp()
                        .get_sender()
                        .send_blocking(Message::LoadUrl(error_tab_id, beacon_core::dive::URL.to_string()));
                    return glib::Propagation::Stop;
                }
                let modifiers = engine_modifiers(state);
                // Web `code` (physical key) is approximated with the logical name until a
                // scancode map exists; the engine only reads `key` today.
                let named = web_key_name(keyval);
                *press_in_flight.borrow_mut() = Some((named.clone().unwrap_or_default(), modifiers));
                committed.set(false);
                if let Some(event) = c.current_event() {
                    press_im.filter_keypress(&event);
                }
                let handled = committed.get();
                *press_in_flight.borrow_mut() = None;
                if !handled {
                    if let Some(key) = named {
                        let handle = key_handle.clone();
                        let code = key.clone();
                        runtime().spawn(async move {
                            let _ = handle.send(EngineTabCommand::KeyDown { key, code, modifiers }).await;
                        });
                    }
                }
                glib::Propagation::Stop
            });

            // The release. Not offered to the input method: those compose on press, and
            // `filter_keypress` reads any event it is handed as one.
            let release_handle = handle.clone();
            keys.connect_key_released(move |_c, keyval, _keycode, state| {
                let Some(key) = web_key_name(keyval) else { return };
                let modifiers = engine_modifiers(state);
                let handle = release_handle.clone();
                let code = key.clone();
                runtime().spawn(async move {
                    let _ = handle.send(EngineTabCommand::KeyUp { key, code, modifiers }).await;
                });
            });
            area.add_controller(keys);

            // Middle click on the page: same link-in-background-tab behaviour as Ctrl+click.
            let mid = gtk4::GestureClick::new();
            mid.set_button(gdk::BUTTON_MIDDLE);
            let mid_window = self.obj().clone();
            let mid_tab_id = tab.id();
            mid.connect_pressed(move |g, _n, x, y| {
                mid_window
                    .imp()
                    .query_hit_test(mid_tab_id, g, x, y, HitIntent::OpenLinkInBackgroundTab);
            });
            area.add_controller(mid);

            // Secondary click -> ask the engine what is under the pointer; the context menu
            // is built from its HitTestResult (see handle_engine_event).
            let right = gtk4::GestureClick::new();
            right.set_button(gdk::BUTTON_SECONDARY);
            let right_window = self.obj().clone();
            let right_tab_id = tab.id();
            right.connect_pressed(move |gesture, _n, x, y| {
                right_window
                    .imp()
                    .query_hit_test(right_tab_id, gesture, x, y, HitIntent::ContextMenu);
            });
            area.add_controller(right);
        }

        self.render_areas.borrow_mut().insert(tab.id(), area.clone());
        area
    }

    /// Stop the engine and wait for it, or its workers race SQLCipher's exit-time cleanup
    /// (see `BrowserEngine::shutdown`). Later calls find nothing to stop.
    pub fn shutdown_engine(&self) {
        let engine = self.engine.borrow_mut().take();
        if let Some(engine) = engine {
            engine.shutdown(runtime());
        }
    }

    /// Start the engine and wire its redraw/event notifications into the GTK main loop.
    pub fn init_engine(&self) {
        let mut engine = match BrowserEngine::new(runtime(), self.private.get(), crate::render::backend()) {
            Ok(e) => e,
            Err(e) => {
                self.log(format!("Failed to start engine: {e}").as_str());
                log::error!("engine init failed: {e:?}");
                return;
            }
        };

        let redraw_rx = engine.take_redraw_rx();
        let event_rx = engine.take_event_rx();
        let resource_rx = engine.take_resource_rx();
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

        // Per-resource fetch detail feeds the developer panel's request log and nothing
        // else, so it is drained straight into core without a window round trip.
        if let Some(mut resource_rx) = resource_rx {
            let weak = self.obj().downgrade();
            glib::spawn_future_local(async move {
                loop {
                    match resource_rx.recv().await {
                        Ok(update) => {
                            if let Some(win) = weak.upgrade() {
                                win.imp().beacon.borrow_mut().on_resource_update(update);
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
    /// Route one engine event: the two cases this frontend must answer itself, then
    /// `beacon-core`'s translation, then drawing whatever it says changed.
    fn handle_engine_event(&self, evt: EngineEvent) {
        if self.handle_gtk_only_event(&evt) {
            return;
        }
        let events = self.beacon.borrow_mut().on_engine_event(evt);
        for event in events {
            self.apply_beacon_event(event);
        }
    }

    /// The engine events this frontend answers on its own, before core sees them.
    /// Returns whether the event was consumed.
    ///
    /// Both are here because they are about *this* shell, not about the browser: one page
    /// that GTK draws with widgets instead of letting the engine render it, and a hit-test
    /// round trip whose pending-click bookkeeping is tied to GTK gestures.
    fn handle_gtk_only_event(&self, evt: &EngineEvent) -> bool {
        match evt {
            // The engine serves gosub:// pages itself, except the one Beacon renders as
            // GTK widgets (gosub://config). A link to it clicked inside an engine page
            // starts an engine navigation; cancel that and swap in the shell page instead.
            // (LoadUrl already set the tab's URL for our own navigations, so the equality
            // check keeps this from looping.)
            EngineEvent::Navigation {
                tab_id,
                event: NavigationEvent::Started { url, .. },
            } => {
                if !Self::is_shell_rendered(url) {
                    return false;
                }
                let Some(our_id) = self.beacon.borrow().tab_for_engine(*tab_id) else {
                    return true;
                };
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
                true
            }

            // Answer to a click's QueryHitTest; what to do with it was decided when the
            // gesture fired and parked in `pending_hit_tests`.
            EngineEvent::HitTestResult { token, hit, .. } => {
                let Some((tab_id, point, intent)) = self.pending_hit_tests.borrow_mut().remove(&token.0) else {
                    return true;
                };
                match intent {
                    HitIntent::ContextMenu => super::page_context_menu::show(&self.obj(), tab_id, point, hit.clone()),
                    HitIntent::OpenLinkInBackgroundTab => {
                        // Not on a link: a Ctrl/middle click on empty page area does nothing.
                        let Some(link) = hit.link_url.clone() else { return true };
                        let sender = self.get_sender();
                        runtime().spawn(async move {
                            let _ = sender.send(Message::OpenTabRight(tab_id, link, "New Tab".into())).await;
                        });
                    }
                }
                true
            }

            // Focus moved inside the page. Nothing to do yet - this becomes the IME /
            // on-screen-keyboard trigger once text editing lands.
            EngineEvent::FocusChanged { focused, editable, .. } => {
                log::debug!("page focus changed: focused={focused} editable={editable}");
                true
            }

            _ => false,
        }
    }

    /// Draw one `BeaconEvent`. Everything below is widget work; the decisions behind it
    /// were made in `beacon-core`.
    fn apply_beacon_event(&self, event: BeaconEvent) {
        match event {
            BeaconEvent::Redraw => {
                for area in self.render_areas.borrow().values() {
                    area.queue_render();
                }
            }
            BeaconEvent::TabsChanged => self.refresh_tabs(),
            BeaconEvent::ActiveTabChanged(tab_id) => self.activate_tab(tab_id),
            BeaconEvent::NavStateChanged(_) => {
                self.update_nav_buttons();
                self.update_bookmark_button();
            }
            BeaconEvent::TitleChanged(tab_id, title) => {
                if self.active_tab_id() == Some(tab_id) {
                    self.obj().set_title(Some(&format!("{title} — Gosub Beacon")));
                }
            }
            BeaconEvent::UrlChanged(tab_id, url) => {
                if self.active_tab_id() == Some(tab_id) {
                    let tab = self.tab_manager.lock().unwrap().get_tab(tab_id);
                    match tab {
                        Some(tab) => self.show_tab_address(&tab),
                        None => self.show_address(Some(&url)),
                    }
                }
            }
            BeaconEvent::LoadingChanged(..) => {}
            // The address bar doubles as the progress bar (GtkEntry's fill).
            BeaconEvent::LoadProgress(tab_id, fraction) => {
                if self.active_tab_id() == Some(tab_id) {
                    self.searchbar.set_progress_fraction(fraction.unwrap_or(0.0));
                }
            }
            // The bytes are on the tab; the chip decodes them when it is rebuilt, which
            // `TabsChanged` asks for. A bookmarked page's icon also goes to its bookmark.
            BeaconEvent::FaviconChanged(tab_id) => self.store_bookmark_favicon(tab_id),
            BeaconEvent::NavigationFailed(tab_id, url, error) => self.on_navigation_failed(tab_id, &url, &error),
            BeaconEvent::CertificateChanged(tab_id, url, fingerprint) => self.on_certificate_changed(tab_id, url, fingerprint),
            BeaconEvent::IdentityAsked {
                tab_id,
                url,
                status,
                message,
                presented,
            } => self.on_identity_asked(tab_id, url, status, message, presented),
            BeaconEvent::TabCrashed(tab_id, _) => {
                self.render_areas.borrow_mut().remove(&tab_id);
            }
            BeaconEvent::HoverUrl(tab_id, url) => {
                if self.active_tab_id() == Some(tab_id) {
                    let text = url.as_deref().unwrap_or("");
                    self.statusbar.set_text(text);
                    self.statusbar.set_visible(!text.is_empty());
                }
            }
            BeaconEvent::CursorChanged(tab_id, cursor) => {
                let name = match cursor {
                    Cursor::Pointer => "pointer",
                    Cursor::Text => "text",
                    Cursor::Resize => "nwse-resize",
                    Cursor::Default => "default",
                };
                if let Some(area) = self.render_areas.borrow().get(&tab_id) {
                    area.set_cursor_from_name(Some(name));
                }
            }
            BeaconEvent::DownloadOffered {
                tab_id,
                offer,
                url,
                suggested_filename,
                ..
            } => self.save_download_as(tab_id, Some(offer), url, &suggested_filename),
            BeaconEvent::DownloadChanged(_) => self.refresh_downloads(),
            BeaconEvent::PickerRequested {
                tab_id,
                kind,
                value,
                min,
                max,
                step,
                ..
            } => self.open_picker(tab_id, kind, &value, min.as_deref(), max.as_deref(), step.as_deref()),
            BeaconEvent::Log(message) => self.log(&message),
        }
    }
}

/// Whether two URLs are on the same site for a bookmark's icon: one host and one port, so
/// `localhost:3000` and `localhost:4000` are two services. A default port counts as none, so the
/// `http://a` a bookmark names and the `https://a` it redirects to stay one site.
fn same_site(a: &url::Url, b: &url::Url) -> bool {
    a.host_str() == b.host_str() && a.port() == b.port()
}

/// Map a GDK keyval to the web [`KeyboardEvent.key`] name the engine expects.
/// `None` for keys that have neither a named mapping nor a printable character.
/// Decode an encoded favicon into something GTK can paint. `PixbufLoader` handles ICO
/// -- the common favicon format -- which `gdk::Texture::from_bytes` does not reliably
/// decode. Returns `None` for anything that does not decode.
fn decode_favicon(bytes: &[u8]) -> Option<gdk::Texture> {
    let loader = gtk4::gdk_pixbuf::PixbufLoader::new();
    loader
        .write(bytes)
        .and_then(|_| loader.close())
        .ok()
        .and_then(|_| loader.pixbuf())
        .map(|pixbuf| gdk::Texture::for_pixbuf(&pixbuf))
}

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

/// A wall-clock time from a Unix millisecond stamp. The core stores the number and leaves
/// the formatting to whoever is showing it.
fn format_clock(timestamp_ms: u64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_millis_opt(timestamp_ms as i64) {
        chrono::LocalResult::Single(time) => time.format("%H:%M:%S%.3f").to_string(),
        _ => "--:--:--".to_string(),
    }
}

/// A URL short enough for a list row: host plus the tail of the path, which is the part
/// that tells one request from another.
fn short_url(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(parsed) => {
            let host = parsed.host_str().unwrap_or("");
            let path = parsed.path();
            let tail = path.rsplit('/').next().unwrap_or(path);
            // The query comes along: on a site that serves everything through one endpoint
            // it is the only thing that distinguishes one request from the next, and a list
            // of identical `load.php` rows is no use to anyone.
            let query = parsed.query().map(|q| format!("?{q}")).unwrap_or_default();
            if tail.is_empty() {
                format!("{host}{path}{query}")
            } else {
                format!("{host}/…/{tail}{query}")
            }
        }
        Err(_) => url.to_string(),
    }
    // No length cap here. The cell this lands in ellipsizes at the end, so it shortens
    // itself to whatever room there is -- and it cuts the query first, which is the part
    // worth losing, leaving the host and file name visible. Trimming here as well used to
    // eat the start of the URL before the label had a chance.
}

/// Clip to `width` characters, marking that something was dropped. Counts characters, not
/// bytes -- slicing a UTF-8 string by byte index panics mid-codepoint.
fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }
    let mut out: String = value.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// A line in one of the request-detail panes.
///
/// Typed rather than pre-formatted strings so the pane can lay each kind out properly: a
/// heading is not a value, and a value column that starts wherever the previous name happened
/// to end is what made these read as a wall of text.
pub(crate) enum DetailRow {
    /// A heading above the rows that follow.
    Section(String),
    /// A name and its value, in two aligned columns.
    Pair(String, String),
    /// Three columns: kind, size, URL. Used by the subresource list.
    Columns((String, String, String)),
    /// One phase of a request's life: a name, a bar drawn to `fraction` of the row's width
    /// starting at `offset`, and the duration written out.
    Phase {
        /// What this phase is called
        name: String,
        /// Where the bar starts, 0.0-1.0 of the total
        offset: f64,
        /// How much of the total it spans
        fraction: f64,
        /// The duration, already formatted
        label: String,
    },
    /// Explanatory prose, wrapped.
    Note(String),
}

impl DetailRow {
    /// What this row would render as, for the "has anything changed" check that stops these
    /// panes rebuilding -- and losing their scroll position -- four times a second.
    fn signature(&self) -> String {
        match self {
            DetailRow::Section(t) => format!("S\u{1}{t}\u{2}"),
            DetailRow::Pair(k, v) => format!("P\u{1}{k}\u{1}{v}\u{2}"),
            DetailRow::Columns((a, b, c)) => format!("C\u{1}{a}\u{1}{b}\u{1}{c}\u{2}"),
            DetailRow::Phase {
                name,
                offset,
                fraction,
                label,
            } => format!("T\u{1}{name}\u{1}{offset:.4}\u{1}{fraction:.4}\u{1}{label}\u{2}"),
            DetailRow::Note(t) => format!("N\u{1}{t}\u{2}"),
        }
    }
}

/// Milliseconds since the Unix epoch, for measuring how long a request has been in flight.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
