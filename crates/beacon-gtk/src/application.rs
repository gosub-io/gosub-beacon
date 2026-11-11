use crate::dialog::about::About;
use crate::dialog::shortcuts::ShortcutsDialog;
use crate::window::BrowserWindow;
use gtk4::glib::clone;
use gtk4::subclass::prelude::GtkApplicationImpl;
use gtk4::{gio, glib, prelude::*, subclass::prelude::*, Settings};
use log::info;
use std::cell::{Cell, RefCell};

/// The app action a second launch hands its URLs to the running instance with: whether it
/// was started `--private`, and a string array, each opened as a tab.
pub(crate) const OPEN_URLS: &str = "open-urls";

/// The app action a second launch started `--new-window` asks for a new regular window with:
/// a string array, each opened as a tab; the home page when it is empty.
pub(crate) const OPEN_WINDOW: &str = "open-window";

/// What a second launch handed over.
pub(crate) enum Handoff {
    /// [`OPEN_URLS`]: tabs in the current window, or a new private one.
    Tabs { private: bool, urls: Vec<String> },
    /// [`OPEN_WINDOW`]: a new regular window.
    Window(Vec<String>),
}

impl Handoff {
    fn urls(&self) -> &[String] {
        match self {
            Handoff::Tabs { urls, .. } | Handoff::Window(urls) => urls,
        }
    }
}

mod imp {
    use super::*;
    use crate::window::BrowserWindow;

    #[derive(Default)]
    pub struct Application {
        /// What other launches handed over while the profile was still opening; opened
        /// after the first window.
        pub(crate) pending: RefCell<Vec<Handoff>>,
        /// Set once the app shuts down: windows going away then leave the session as it was.
        pub(crate) quitting: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Application {
        const NAME: &'static str = "Application";
        type Type = super::Application;
        type ParentType = gtk4::Application;
    }

    impl ObjectImpl for Application {}

    impl ApplicationImpl for Application {
        fn activate(&self) {
            info!(target: "gtk", "GtkApplication<Application>::activate");
            self.parent_activate();
            // A second launch while the profile is still opening: the first window comes
            // once it is open.
            if !crate::dialog::startup::profile_open() {
                return;
            }

            let obj = self.obj();

            if let Some(window) = obj.windows().first() {
                window.present();
                return;
            }

            // Without URLs on the command line, every window of the previous session comes
            // back. The first one in it was focused last: it is made and presented last, which
            // also puts it first in `windows()` again when nothing gets focus.
            let cli = beacon_core::cli::Cli::global();
            let mut session = Vec::new();
            if !cli.private && cli.urls.is_empty() {
                session = beacon_core::session::load_windows();
            }
            let restore = (!session.is_empty()).then(|| session.remove(0));
            for tabs in session.into_iter().rev() {
                let other = BrowserWindow::new(&obj, false);
                *other.imp().restore.borrow_mut() = Some(tabs);
                other.present();
            }
            let window = BrowserWindow::new(&obj, cli.private);
            window.imp().first_window.set(true);
            *window.imp().restore.borrow_mut() = restore;
            window.present();

            for handoff in self.pending.take() {
                obj.open_handoff(handoff);
            }
        }

        fn startup(&self) {
            info!(target: "gtk", "GtkApplication<Application>::startup");
            self.parent_startup();

            let obj = self.obj();
            obj.setup_actions();
            obj.setup_accelerators();
        }

        // Quitting leaves the windows open; their engines stop before `run` returns.
        fn shutdown(&self) {
            self.quitting.set(true);
            for window in self.obj().windows() {
                if let Ok(window) = window.downcast::<BrowserWindow>() {
                    window.imp().shutdown_engine();
                }
            }
            self.parent_shutdown();
        }
    }

    impl GtkApplicationImpl for Application {
        // A closed window, the last one included, stops its engine as it goes. Closing one
        // of several regular windows takes it out of the session; closing the last one
        // leaves the session for the next start.
        fn window_removed(&self, window: &gtk4::Window) {
            let regular = window.downcast_ref::<BrowserWindow>().is_some_and(|w| !w.imp().private.get());
            if let Some(window) = window.downcast_ref::<BrowserWindow>() {
                window.imp().shutdown_engine();
            }
            self.parent_window_removed(window);
            if regular && !self.quitting.get() {
                self.obj().save_session();
            }
        }
    }
}

glib::wrapper! {
    pub struct Application(ObjectSubclass<imp::Application>)
        @extends gio::Application, gtk4::Application,
        @implements gio::ActionMap, gio::ActionGroup;
}

impl Application {
    pub fn new(app_id: &str) -> Self {
        glib::Object::builder()
            .property("application-id", app_id)
            .property("resource-base-path", Some("/io/gosub/beacon"))
            .build()
    }

    pub fn window(&self) -> BrowserWindow {
        self.active_window()
            .map_or_else(|| BrowserWindow::new(self, false), |w| w.downcast().unwrap())
    }

    /// Persist every regular window's tabs as the session, the most recently focused window
    /// first. Nothing while none is open: closing the last one keeps the session it had.
    pub(crate) fn save_session(&self) {
        let windows: Vec<_> = self
            .windows()
            .into_iter()
            .filter_map(|w| w.downcast::<BrowserWindow>().ok())
            .filter(|w| !w.imp().private.get())
            .map(|w| w.imp().session_tabs())
            .filter(|tabs| !tabs.is_empty())
            .collect();
        if !windows.is_empty() {
            beacon_core::session::save_windows(&windows);
        }
    }

    /// A new window for another launch: it starts on `urls` alone, or the home page.
    fn handed_off_window(&self, private: bool, urls: Vec<String>) {
        let window = BrowserWindow::new(self, private);
        window.open_urls(urls);
        window.present();
    }

    /// Open what another launch handed over, or keep it for after the first window while the
    /// profile is still opening: a window now would take the profile a second time.
    fn handoff(&self, handoff: Handoff) {
        if !crate::dialog::startup::profile_open() {
            info!(target: "gtk", "Keeping {} URL(s) from another launch until the profile is open", handoff.urls().len());
            self.imp().pending.borrow_mut().push(handoff);
            return;
        }
        self.open_handoff(handoff);
    }

    fn open_handoff(&self, handoff: Handoff) {
        info!(target: "gtk", "Opening {} URL(s) from another launch", handoff.urls().len());
        match handoff {
            // Never in a regular window: that would load them with its cookies and keep them
            // in its session.
            Handoff::Tabs { private: true, urls } => self.handed_off_window(true, urls),
            // Nor the other way round: the last focused window may be a private one.
            Handoff::Tabs { private: false, urls } => match self.regular_window() {
                Some(window) => {
                    window.open_urls(urls);
                    window.present();
                }
                None => self.handed_off_window(false, urls),
            },
            Handoff::Window(urls) => self.handed_off_window(false, urls),
        }
    }

    /// The most recently focused window that is not private.
    fn regular_window(&self) -> Option<BrowserWindow> {
        self.windows()
            .into_iter()
            .filter_map(|w| w.downcast::<BrowserWindow>().ok())
            .find(|w| !w.imp().private.get())
    }

    pub(crate) fn setup_actions(&self) {
        let open_urls = gio::SimpleAction::new(OPEN_URLS, Some(&glib::VariantType::new("(bas)").unwrap()));
        open_urls.connect_activate(clone!(
            #[weak(rename_to=app)]
            self,
            move |_, param| {
                if let Some((private, urls)) = param.and_then(|p| p.get::<(bool, Vec<String>)>()) {
                    app.handoff(Handoff::Tabs { private, urls });
                }
            }
        ));
        self.add_action(&open_urls);

        let open_window = gio::SimpleAction::new(OPEN_WINDOW, Some(&glib::VariantType::new("as").unwrap()));
        open_window.connect_activate(clone!(
            #[weak(rename_to=app)]
            self,
            move |_, param| {
                if let Some(urls) = param.and_then(|p| p.get::<Vec<String>>()) {
                    app.handoff(Handoff::Window(urls));
                }
            }
        ));
        self.add_action(&open_window);

        let quit = gio::ActionEntry::builder("quit").activate(|app: &Self, _, _| app.quit()).build();
        let toggle_dark_mode = gio::ActionEntry::builder("toggle-dark-mode")
            .activate(|_: &Self, _, _| {
                info!(target: "gtk", "Toggle dark mode action triggered");
                let settings = Settings::default().expect("Failed to get default GtkSettings");
                let mode: bool = settings.property("gtk-application-prefer-dark-theme");
                settings.set_property("gtk-application-prefer-dark-theme", !mode);
            })
            .build();
        let show_about = gio::ActionEntry::builder("show-about")
            .activate(|app: &Self, _, _| {
                info!(target: "gtk", "Show about dialog action triggered");
                let about = About::create_dialog();
                about.set_transient_for(app.active_window().as_ref());
                about.present();
            })
            .build();
        let show_shortcuts = gio::ActionEntry::builder("show-shortcuts")
            .activate(|app: &Self, _, _| {
                info!(target: "gtk", "Show shortcuts dialog action triggered");
                let about = ShortcutsDialog::create_dialog(app);
                about.present();
            })
            .build();
        self.add_action_entries([quit, toggle_dark_mode, show_about, show_shortcuts]);
    }

    fn setup_accelerators(&self) {
        crate::shortcuts::register(self, crate::shortcuts::Scope::Application);
    }

    pub fn run(&self) {
        info!("Application started");
        ApplicationExtManual::run(self);
    }
}

impl Default for Application {
    fn default() -> Self {
        gio::Application::default().unwrap().downcast::<Application>().unwrap()
    }
}
