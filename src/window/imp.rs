use crate::engine::{draw_frame, BrowserEngine, EngineTabId};
use crate::fetcher::address_parser::GosubAddressParser;
use crate::tab::{GosubTab, GosubTabManager, HistoryNodeId, TabCommand, TabId};
use crate::window::message::Message;
use crate::window::tab_context_menu::{build_context_menu, setup_context_menu_actions, TabInfo};
use crate::runtime;
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
    gdk, glib, Button, CompositeTemplate, DrawingArea, Entry, GestureClick, Image, Notebook, Popover, PopoverMenu, PopoverMenuFlags,
    ScrolledWindow, Settings, TemplateChild, TextView, ToggleButton, Widget,
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
#[template(resource = "/io/gosub/browser-gtk/ui/window.ui")]
pub struct BrowserWindow {
    #[template_child]
    pub searchbar: TemplateChild<Entry>,
    #[template_child]
    pub btn_prev: TemplateChild<Button>,
    #[template_child]
    pub btn_next: TemplateChild<Button>,
    #[template_child]
    pub tab_bar: TemplateChild<Notebook>,
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
            tab_bar: TemplateChild::default(),
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
    fn handle_new_tab(&self, _btn: &Button) {
        todo!("not yet implemented");
    }

    #[template_callback]
    fn handle_close_tab(&self, _btn: &Button) {
        todo!("not yet implemented");
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
    fn handle_toggle_darkmode(&self, btn: &ToggleButton) {
        self.log("Toggling dark mode");

        info!(target: "gtk", "Toggle dark mode action triggered");
        let settings = Settings::default().expect("Failed to get default GtkSettings");
        settings.set_property("gtk-application-prefer-dark-theme", btn.is_active());
    }

    #[template_callback]
    fn handle_refresh_clicked(&self, _btn: &Button) {
        self.log("Refreshing the current page");
    }

    #[template_callback]
    async fn handle_searchbar_clicked(&self, entry: &Entry) {
        let Some(page_num) = self.tab_bar.current_page() else {
            self.log("No active tab to load the URL");
            return;
        };

        match self.tab_bar.nth_page(Some(page_num)) {
            Some(page) => {
                self.log(format!("Visiting the URL {}", entry.text().as_str()).as_str());

                let tab_id = page.get_tab_id().unwrap();
                let url_str = entry.text().to_string();

                self.sender.send(Message::LoadUrl(tab_id, url_str)).await.unwrap();
            }
            None => {
                self.log("No active tab to load the URL");
            }
        }
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
                    let page_num = self.get_page_num_for_tab(tab_id);
                    self.tab_bar.set_current_page(page_num);
                }
                TabCommand::Insert(tab_id, position) => {
                    let manager = self.tab_manager.lock().unwrap();
                    let tab = manager.get_tab(tab_id).unwrap().clone();
                    drop(manager);

                    let label = self.create_tab_label(&tab);
                    let default_page = self.generate_default_page();

                    let notebook_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
                    notebook_box.append(&default_page);
                    notebook_box.set_tab_id(tab.id());
                    self.tab_bar.insert_page(&notebook_box, Some(&label), Some(position));

                    // We can reorder tab, unless it's pinned/pinned
                    if let Some(page) = self.tab_bar.nth_page(Some(position)) {
                        self.tab_bar.set_tab_reorderable(&page, !tab.is_pinned());
                    }
                }
                TabCommand::Close(tab_id) => {
                    let page_num = self.get_page_num_for_tab(tab_id);
                    self.tab_bar.remove_page(page_num);
                }
                TabCommand::CloseAll => {
                    for _ in 0..self.tab_bar.pages().n_items() {
                        self.tab_bar.remove_page(Some(0));
                    }
                }
                TabCommand::Move(tab_id, position) => {
                    let page_num = self.get_page_num_for_tab(tab_id);
                    let page = self.tab_bar.nth_page(page_num).unwrap();
                    self.tab_bar.reorder_child(&page, Some(position));
                }
                TabCommand::Update(tab_id) => {
                    let manager = self.tab_manager.lock().unwrap();
                    let tab = manager.get_tab(tab_id).unwrap().clone();
                    drop(manager);
                    let page_num = self.get_page_num_for_tab(tab_id).unwrap();

                    // Either an engine-backed render area (once the tab has an engine tab),
                    // or the default gosub splash page.
                    let child: Widget = if tab.has_engine_tab() {
                        self.build_render_area(&tab).upcast::<Widget>()
                    } else {
                        self.generate_default_page().upcast::<Widget>()
                    };

                    // Since a tab contains a box, we just update the child inside the box. This way
                    // we do not need to remove the actual page from the notebook, which results in all
                    // kind of issues.
                    let page = self.tab_bar.nth_page(Some(page_num)).unwrap();
                    let notebox_box = page.downcast_ref::<gtk4::Box>().unwrap();
                    notebox_box.remove(&notebox_box.first_child().unwrap());
                    notebox_box.append(&child);

                    // We update the tab label as well
                    let tab_label = self.create_tab_label(&tab);
                    self.tab_bar.set_tab_label(notebox_box, Some(&tab_label));

                    // self.tab_bar.set_current_page(Some(page_num));
                }
            }
        }
    }

    fn create_pinned_tab_label(&self, tab: &GosubTab) -> Widget {
        if let Some(favicon) = &tab.favicon() {
            let img = Image::from_paintable(Some(&favicon.clone()));
            img.set_margin_top(5);
            img.set_margin_bottom(5);
            return img.into();
        }

        // No favicon for this pinned tab, so use a default icon
        let img = Image::from_resource("/io/gosub/browser-gtk/assets/pin.svg");
        img.set_margin_top(5);
        img.set_margin_bottom(5);
        img.into()
    }

    fn create_normal_tab_label(&self, tab: &GosubTab) -> Widget {
        let label_vbox = gtk4::Box::new(gtk4::Orientation::Horizontal, 5);

        // When the tab is loading, we show a spinner
        if tab.is_loading() {
            let spinner = gtk4::Spinner::new();
            spinner.start();
            label_vbox.append(&spinner);
        } else if let Some(favicon) = &tab.favicon() {
            label_vbox.append(&Image::from_paintable(Some(&favicon.clone())));
        }

        let mut title = tab.title().to_string();
        title.truncate(20);
        let tab_label = gtk4::Label::new(Some(title.as_str()));
        label_vbox.append(&tab_label);

        let tab_close_button = Button::builder()
            .halign(gtk4::Align::End)
            .has_frame(false)
            .margin_bottom(0)
            .margin_end(0)
            .margin_start(0)
            .margin_top(0)
            .build();
        let img = Image::from_icon_name("window-close-symbolic");
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

    fn generate_default_page(&self) -> gtk4::Widget {
        let img = Image::from_resource("/io/gosub/browser-gtk/assets/submarine.svg");
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

    /// The tab id of the currently active notebook page, if any.
    fn active_tab_id(&self) -> Option<TabId> {
        let cur = self.tab_bar.current_page()?;
        self.tab_bar.nth_page(Some(cur))?.get_tab_id()
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
                for page_num in 0..self.tab_bar.pages().n_items() {
                    let page = self.tab_bar.nth_page(Some(page_num)).unwrap();
                    if page.get_tab_id().unwrap() == target_tab_id {
                        self.open_tab(Some(page_num as usize + 1), &url, &title);
                        return;
                    }
                }
            }

            Message::LoadUrl(tab_id, url_str) => {
                self.log(format!("Loading URL: {}", url_str).as_str());

                let Ok((_view_mode, url)) = GosubAddressParser::parse(url_str.as_str()) else {
                    self.log("Cannot parse URL");
                    return;
                };

                // Update information in the given tab with the new url
                let mut manager = self.tab_manager.lock().unwrap();
                let mut tab = manager.get_tab(tab_id).unwrap().clone();

                tab.set_favicon(None);
                tab.set_title(url.as_str());
                tab.set_url(url.clone());
                tab.set_loading(true);

                manager.update_tab(tab_id, &tab);
                drop(manager);

                self.refresh_tabs();

                // Hand navigation off to the engine (it fetches and renders).
                self.navigate_engine_tab(tab_id, url.as_str());
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
            Message::UnpinTab(tab_id) => {
                let mut manager = self.tab_manager.lock().unwrap();
                manager.unpin_tab(tab_id);
                drop(manager);

                // Update tab-bar
                self.refresh_tabs();
            }
        }
    }

    /// Retrieves the page number for the given TabID
    fn get_page_num_for_tab(&self, tab_id: TabId) -> Option<u32> {
        for i in 0..self.tab_bar.pages().n_items() {
            let page = self.tab_bar.nth_page(Some(i)).unwrap();
            if page.get_tab_id().unwrap() == tab_id {
                return Some(i);
            }
        }

        None
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
        tab.set_loading(true);

        // add tab to manager, and notify the tab has changed. This will update the
        // tab-bar during a refresh-tabs call.
        let mut manager = self.tab_manager.lock().unwrap();
        manager.add_tab(tab, position);
        manager.notify_tab_changed(tab_id);
        drop(manager);
        self.refresh_tabs();

        // Hand navigation off to the engine (it fetches and renders).
        self.navigate_engine_tab(tab_id, url.as_str());
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
            // Resize -> tell the engine the new viewport.
            let resize_handle = handle.clone();
            area.connect_resize(move |_area, w, h| {
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
                if let NavigationEvent::Finished { url, .. } = event {
                    let mut manager = self.tab_manager.lock().unwrap();
                    if let Some(mut tab) = manager.get_tab(our_id) {
                        tab.set_loading(false);
                        tab.set_title(url.as_str());

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
                }
            }
            EngineEvent::HoverUrl { tab_id, url } => {
                // Only surface hover for the active tab.
                let Some(our_id) = self.engine_tab_map.borrow().get(&tab_id).copied() else {
                    return;
                };
                let is_active = self
                    .tab_bar
                    .current_page()
                    .and_then(|cur| self.tab_bar.nth_page(Some(cur)))
                    .and_then(|page| page.get_tab_id())
                    == Some(our_id);
                if is_active {
                    self.statusbar.set_text(url.as_deref().unwrap_or(""));
                }
            }
            _ => {}
        }
    }
}
