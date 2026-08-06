use crate::engine::{draw_frame, BrowserEngine, EngineTabId};
use crate::fetcher::address_parser::GosubAddressParser;
use crate::tab::{GosubTab, GosubTabManager, HistoryNodeId, TabCommand, TabId};
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
    gdk, glib, Button, CompositeTemplate, DrawingArea, Entry, GestureClick, Image, Popover, PopoverMenu, PopoverMenuFlags,
    ScrolledWindow, Settings, Stack, TemplateChild, TextView, ToggleButton, Widget,
};
use log::info;
use once_cell::sync::Lazy;
use std::cell::RefCell;
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

    // Other stuff that are non-widgets
    pub tab_manager: Arc<Mutex<GosubTabManager>>,
    pub sender: Arc<Sender<Message>>,
    pub receiver: Arc<Receiver<Message>>,

    /// The running engine (created in `init_engine`). Main-thread only.
    pub engine: Rc<RefCell<Option<BrowserEngine>>>,
    /// Per-tab drawing areas, so the redraw loop can request repaints.
    pub render_areas: Rc<RefCell<HashMap<TabId, DrawingArea>>>,
    /// Maps engine tab ids back to our tab ids (for routing engine events).
    pub engine_tab_map: Rc<RefCell<HashMap<EngineTabId, TabId>>>,
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

            tab_manager: Arc::new(Mutex::new(GosubTabManager::new())),
            sender: Arc::new(tx),
            receiver: Arc::new(rx),

            engine: Rc::new(RefCell::new(None)),
            render_areas: Rc::new(RefCell::new(HashMap::new())),
            engine_tab_map: Rc::new(RefCell::new(HashMap::new())),
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
        self.log("Browser created...");
    }
}

impl WidgetImpl for BrowserWindow {}
impl WindowImpl for BrowserWindow {}
impl ApplicationWindowImpl for BrowserWindow {}

#[gtk4::template_callbacks]
impl BrowserWindow {
    #[template_callback]
    fn handle_sidebar_home(&self, _btn: &Button) {
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let sender = self.get_sender();
        let _ = sender.send_blocking(Message::LoadUrl(tab_id, "https://gosub.io".into()));
    }

    #[template_callback]
    fn handle_sidebar_todo(&self, _btn: &Button) {
        self.log("Not implemented yet");
    }

    /// Bookmarks-bar buttons carry their URL in the widget `name` property.
    #[template_callback]
    fn handle_bookmark_clicked(&self, btn: &Button) {
        let url = btn.widget_name();
        if !url.starts_with("http") {
            return;
        }
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let _ = self.get_sender().send_blocking(Message::LoadUrl(tab_id, url.to_string()));
    }

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

        // Shell-rendered internal pages have nothing to reload; engine-rendered
        // ones get their HTML pushed again.
        if Self::is_internal_url(&url) {
            if Self::engine_rendered_internal(Self::internal_page_name(&url)) {
                self.load_internal_html(tab_id, &url);
            }
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
                    let manager = self.tab_manager.lock().unwrap();
                    let tab = manager.get_tab(tab_id).unwrap().clone();
                    drop(manager);

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
                        chip.set_active(true);
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
                    while let Some(chip) = self.tab_strip.first_child() {
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
                    let manager = self.tab_manager.lock().unwrap();
                    let tab = manager.get_tab(tab_id).unwrap().clone();
                    drop(manager);

                    // Shell-rendered internal pages get a GTK widget; everything else
                    // (real URLs and engine-rendered internal pages like gosub://help)
                    // gets an engine-backed render area, or the splash page.
                    let shell_internal = Self::is_internal_url(tab.url())
                        && !Self::engine_rendered_internal(Self::internal_page_name(tab.url()));
                    let child: Widget = if shell_internal {
                        self.build_internal_page(tab.url())
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
                        chip.set_child(Some(&self.create_tab_label(&tab)));
                        if tab.is_pinned() {
                            chip.add_css_class("pinned");
                        } else {
                            chip.remove_css_class("pinned");
                        }
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
        // Cancellations (stop button, gosub:// interception) are not errors.
        if error.to_lowercase().contains("cancel") {
            return;
        }
        if Self::is_internal_url(url) {
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
    fn chips(&self) -> Vec<ToggleButton> {
        let mut out = Vec::new();
        let mut child = self.tab_strip.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            if let Ok(chip) = widget.downcast::<ToggleButton>() {
                out.push(chip);
            }
        }
        out
    }

    fn chip_for_tab(&self, tab_id: TabId) -> Option<ToggleButton> {
        self.chips().into_iter().find(|c| c.get_tab_id() == Some(tab_id))
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
    pub(crate) fn activate_tab(&self, tab_id: TabId) {
        for chip in self.chips() {
            chip.set_active(chip.get_tab_id() == Some(tab_id));
        }
        if let Some(page) = self.page_for_tab(tab_id) {
            self.content_stack.set_visible_child(&page);
        }

        let manager = self.tab_manager.lock().unwrap();
        if let Some(tab) = manager.get_tab(tab_id) {
            // A blank tab gets an empty address bar, ready to type into.
            if Self::is_internal_url(tab.url()) && Self::internal_page_name(tab.url()) == "blank" {
                self.searchbar.set_text("");
            } else {
                self.searchbar.set_text(tab.url().as_str());
            }
            self.obj().set_title(Some(&format!("{} — Gosub Beacon", tab.title())));
        }
        drop(manager);
        self.update_nav_buttons();
        self.update_reload_button();
    }

    /// A tab chip: toggle button in the strip whose child is the tab label.
    fn create_tab_chip(&self, tab: &GosubTab) -> ToggleButton {
        let chip = ToggleButton::new();
        chip.set_has_frame(false);
        chip.add_css_class("tab-chip");
        if tab.is_pinned() {
            chip.add_css_class("pinned");
        }
        chip.set_child(Some(&self.create_tab_label(tab)));
        chip.set_tab_id(tab.id());

        let window_clone = self.obj().clone();
        let tab_id = tab.id();
        chip.connect_clicked(move |_| {
            window_clone.imp().activate_tab(tab_id);
        });
        chip
    }

    fn create_pinned_tab_label(&self, tab: &GosubTab) -> Widget {
        if let Some(favicon) = &tab.favicon() {
            let img = Image::from_paintable(Some(&favicon.clone()));
            img.set_margin_top(5);
            img.set_margin_bottom(5);
            return img.into();
        }

        // No favicon for this pinned tab, so use a default icon
        let img = Image::from_resource("/io/gosub/beacon/assets/pin.svg");
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

        let tab_close_button = Button::builder()
            .halign(gtk4::Align::End)
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
        label_vbox.append(&tab_close_button);

        let window_clone = self.obj().clone();
        let tab_id = tab.id();
        tab_close_button.connect_clicked(move |_| {
            info!(target: "gtk", "Clicked close button for tab {}", tab_id);
            window_clone.imp().close_tab(tab_id);
            _ = window_clone.imp().get_sender().send_blocking(Message::RefreshTabs());
        });

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

    /// Internal pages (`gosub://…`, plus `about:` aliases) are rendered by the
    /// shell and never sent to the engine.
    fn is_internal_url(url: &url::Url) -> bool {
        matches!(url.scheme(), "gosub" | "about")
    }

    /// Page name of an internal URL: `gosub://blank` → "blank" (host form),
    /// `about:blank` → "blank" (opaque-path form).
    fn internal_page_name(url: &url::Url) -> &str {
        url.host_str().unwrap_or_else(|| url.path())
    }

    fn internal_title(url: &url::Url) -> &str {
        match Self::internal_page_name(url) {
            "home" => "Home",
            "help" => "Help",
            _ => "New Tab",
        }
    }

    /// Internal pages that go through the engine (via `LoadHtml`) instead of a
    /// shell-built GTK widget.
    fn engine_rendered_internal(name: &str) -> bool {
        matches!(name, "help")
    }

    /// Send the bundled HTML for an engine-rendered internal page to the tab.
    fn load_internal_html(&self, tab_id: TabId, url: &url::Url) {
        let html = match Self::internal_page_name(url) {
            "help" => include_str!("../../resources/help.html").to_string(),
            _ => return,
        };

        let manager = self.tab_manager.lock().unwrap();
        let Some(handle) = manager.get_tab(tab_id).and_then(|t| t.tab_handle()) else {
            drop(manager);
            self.log("Tab has no engine handle yet");
            return;
        };
        drop(manager);

        let base_url = url.to_string();
        runtime().spawn(async move {
            if let Err(e) = handle.send(EngineTabCommand::LoadHtml { html, base_url }).await {
                log::error!("load_html failed: {e:?}");
            }
            let _ = handle.send(EngineTabCommand::ResumeDrawing { fps: 30 }).await;
        });
    }

    /// Shell-rendered stand-ins until the engine serves gosub:// pages itself.
    fn build_internal_page(&self, url: &url::Url) -> Widget {
        match Self::internal_page_name(url) {
            // Placeholder for the branded gosub://home page: the splash art.
            "home" => self.generate_default_page(),
            // gosub://blank (and about:blank, and unknown pages): plain white,
            // like every browser's blank page regardless of theme.
            _ => {
                let page = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
                page.set_hexpand(true);
                page.set_vexpand(true);
                page.add_css_class("blank-page");
                page.upcast::<Widget>()
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
    fn active_tab_id(&self) -> Option<TabId> {
        self.content_stack.visible_child()?.get_tab_id()
    }

    /// Navigate the active tab to its parent history node (the Back button).
    fn navigate_back(&self) {
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let url = {
            let mut manager = self.tab_manager.lock().unwrap();
            let Some(mut tab) = manager.get_tab(tab_id) else {
                return;
            };
            let Some(url) = tab.history_mut().go_back() else {
                return;
            };
            self.stage_history_nav(&mut manager, tab_id, &mut tab, url.clone());
            url
        };
        self.finish_history_nav(tab_id, url);
    }

    /// Handle the Forward button: with a single forward branch go straight there; with several,
    /// pop up a menu (anchored to `anchor`) asking which branch to follow.
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
            [(id, _url)] => self.go_to_history_node(*id),
            _ => self.show_forward_menu(anchor, children),
        }
    }

    /// Navigate the active tab to a specific (forward) history node.
    fn go_to_history_node(&self, node_id: HistoryNodeId) {
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let url = {
            let mut manager = self.tab_manager.lock().unwrap();
            let Some(mut tab) = manager.get_tab(tab_id) else {
                return;
            };
            let Some(url) = tab.history_mut().go_to(node_id) else {
                return;
            };
            self.stage_history_nav(&mut manager, tab_id, &mut tab, url.clone());
            url
        };
        self.finish_history_nav(tab_id, url);
    }

    /// Mark a navigation as history-driven (so its `Finished` event won't push a new node) and
    /// store the updated tab. Runs while the manager lock is held by the caller.
    fn stage_history_nav(&self, manager: &mut GosubTabManager, tab_id: TabId, tab: &mut GosubTab, url: url::Url) {
        tab.set_suppress_history_push(true);
        tab.set_url(url);
        tab.set_loading(true);
        manager.update_tab(tab_id, tab);
    }

    /// Shared tail of a history navigation, run after the manager lock has been released.
    fn finish_history_nav(&self, tab_id: TabId, url: url::Url) {
        self.refresh_tabs();
        self.navigate_engine_tab(tab_id, url.as_str());
        self.update_nav_buttons();
    }

    /// Build and show a popover listing the forward branches of the active tab; picking one
    /// navigates to it.
    fn show_forward_menu(&self, anchor: &Button, children: Vec<(HistoryNodeId, url::Url)>) {
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
                window.imp().go_to_history_node(id);
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
                let internal = Self::is_internal_url(&url);
                let engine_internal = internal && Self::engine_rendered_internal(Self::internal_page_name(&url));
                let mut manager = self.tab_manager.lock().unwrap();
                let mut tab = manager.get_tab(tab_id).unwrap().clone();

                tab.set_favicon(None);
                tab.set_title(if internal { Self::internal_title(&url) } else { url.as_str() });
                tab.set_url(url.clone());
                tab.set_loading(!internal || engine_internal);

                manager.update_tab(tab_id, &tab);
                drop(manager);

                self.refresh_tabs();

                // Real URLs go to the engine as navigations; engine-rendered
                // internal pages are pushed as HTML; the rest is pure shell.
                if !internal {
                    self.navigate_engine_tab(tab_id, url.as_str());
                } else if engine_internal {
                    self.load_internal_html(tab_id, &url);
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
            match eng.create_tab(runtime(), title) {
                Ok(h) => h,
                Err(e) => {
                    self.log(format!("Failed to create engine tab: {e}").as_str());
                    return;
                }
            }
        };
        self.engine_tab_map.borrow_mut().insert(handle.tab_id, tab_id);
        tab.set_tab_handle(handle);

        let internal = Self::is_internal_url(&url);
        let engine_internal = internal && Self::engine_rendered_internal(Self::internal_page_name(&url));
        if internal {
            tab.set_title(Self::internal_title(&url));
        }
        tab.set_loading(!internal || engine_internal);

        // add tab to manager, and notify the tab has changed. This will update the
        // tab-bar during a refresh-tabs call.
        let mut manager = self.tab_manager.lock().unwrap();
        manager.add_tab(tab, position);
        manager.notify_tab_changed(tab_id);
        drop(manager);
        self.refresh_tabs();

        if !internal {
            self.navigate_engine_tab(tab_id, url.as_str());
        } else if engine_internal {
            self.load_internal_html(tab_id, &url);
        }
    }

    /// Build a drawing area that blits the engine's composited frames for `tab`, and forwards
    /// resize/scroll input to the engine tab.
    fn build_render_area(&self, tab: &GosubTab) -> DrawingArea {
        let area = DrawingArea::default();
        area.set_vexpand(true);
        area.set_hexpand(true);
        area.set_focusable(true);

        let engine_id = tab.engine_tab_id().expect("engine tab id");
        let compositor = self
            .engine
            .borrow()
            .as_ref()
            .expect("engine initialised")
            .compositor
            .clone();

        area.set_draw_func(move |_area, cr, w, h| {
            draw_frame(&compositor, engine_id, cr, w, h);
        });

        if let Some(handle) = tab.tab_handle() {
            // Resize -> tell the engine the new viewport. The DPR must be stored
            // before the viewport lands so the rasterizer renders at physical
            // resolution — otherwise HiDPI/fractional-scale displays get a 1x
            // buffer upscaled by the compositor (blurry text).
            let resize_handle = handle.clone();
            area.connect_resize(move |area, w, h| {
                use gosub_render_pipeline::render::DEVICE_PIXEL_RATIO;
                DEVICE_PIXEL_RATIO.store(crate::engine::render_dpr(area), std::sync::atomic::Ordering::Relaxed);
                let handle = resize_handle.clone();
                runtime().spawn(async move {
                    let _ = handle
                        .send(EngineTabCommand::SetViewport {
                            x: 0,
                            y: 0,
                            width: w as u32,
                            height: h as u32,
                        })
                        .await;
                });
            });

            // Scroll -> forward to the engine; it re-renders and notifies us to repaint.
            let scroll = gtk4::EventControllerScroll::new(gtk4::EventControllerScrollFlags::BOTH_AXES);
            let scroll_handle = handle.clone();
            scroll.connect_scroll(move |_c, dx, dy| {
                let handle = scroll_handle.clone();
                let delta_x = dx as f32 * 40.0;
                let delta_y = dy as f32 * 40.0;
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
            motion.connect_motion(move |_c, x, y| {
                let handle = motion_handle.clone();
                runtime().spawn(async move {
                    let _ = handle
                        .send(EngineTabCommand::MouseMove {
                            x: x as f32,
                            y: y as f32,
                        })
                        .await;
                });
            });
            area.add_controller(motion);

            // Primary click -> mouse down (lets the engine follow links).
            let click = gtk4::GestureClick::new();
            click.set_button(gdk::BUTTON_PRIMARY);
            let click_handle = handle.clone();
            click.connect_pressed(move |_g, _n, x, y| {
                let handle = click_handle.clone();
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
                        area.queue_draw();
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
    fn handle_engine_event(&self, evt: EngineEvent) {
        match evt {
            EngineEvent::Redraw { .. } => {
                for area in self.render_areas.borrow().values() {
                    area.queue_draw();
                }
            }
            EngineEvent::Navigation { tab_id, event } => {
                let Some(our_id) = self.engine_tab_map.borrow().get(&tab_id).copied() else {
                    return;
                };

                // The engine cannot serve internal pages: when a link click makes it
                // start a navigation to gosub://…, cancel the doomed fetch and route
                // the URL through the shell's internal-page handling instead. Our own
                // LoadHtml pushes also emit Started, but for those the tab's URL was
                // already updated, so the equality check keeps them from looping.
                if let NavigationEvent::Started { url, .. } = &event {
                    if Self::is_internal_url(url) {
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

                if let NavigationEvent::Failed { url, error, .. } = &event {
                    self.on_navigation_failed(our_id, url, &error.to_string());
                    return;
                }
                if let NavigationEvent::FailedUrl { url, error, .. } = &event {
                    self.log(&format!("Cannot load {url}: {error}"));
                    return;
                }

                if let NavigationEvent::Finished { url, .. } = event {
                    let mut need_favicon = false;
                    let mut manager = self.tab_manager.lock().unwrap();
                    if let Some(mut tab) = manager.get_tab(our_id) {
                        tab.set_loading(false);
                        tab.set_title(url.as_str());
                        need_favicon = tab.favicon().is_none();

                        // Record history: a history-driven (back/forward) navigation only moved
                        // the cursor, so don't push; any other navigation appends a new entry
                        // (using the final URL, so server redirects collapse into one entry).
                        if tab.suppress_history_push() {
                            tab.set_suppress_history_push(false);
                        } else {
                            tab.history_mut().push(url.clone());
                        }

                        manager.update_tab(our_id, &tab);
                    }
                    drop(manager);

                    // Update the address bar if this is the active tab.
                    if self.active_tab_id() == Some(our_id) {
                        self.searchbar.set_text(url.as_str());
                    }
                    self.refresh_tabs();
                    self.update_nav_buttons();

                    // Fetch the site's favicon off-thread; bytes come back as a
                    // FaviconLoaded message and are decoded on the GTK side.
                    // Internal schemes (gosub://) have no favicon endpoint.
                    if need_favicon && url.scheme().starts_with("http") {
                        let sender = self.get_sender();
                        let page_url = url.to_string();
                        runtime().spawn(async move {
                            let bytes = fetcher::fetch_favicon(&page_url).await;
                            if !bytes.is_empty() {
                                let _ = sender.send(Message::FaviconLoaded(our_id, bytes)).await;
                            }
                        });
                    }
                }
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
