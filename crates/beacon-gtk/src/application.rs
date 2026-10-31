use crate::dialog::about::About;
use crate::dialog::shortcuts::ShortcutsDialog;
use crate::window::BrowserWindow;
use gtk4::glib::clone;
use gtk4::subclass::prelude::GtkApplicationImpl;
use gtk4::{gio, glib, prelude::*, subclass::prelude::*, Settings};
use log::info;

/// The app action a second launch hands its URLs to the running instance with: whether it
/// was started `--private`, and a string array, each opened as a tab.
pub(crate) const OPEN_URLS: &str = "open-urls";

mod imp {
    use super::*;
    use crate::window::BrowserWindow;

    #[derive(Default)]
    pub struct Application {}

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

            let obj = self.obj();

            if let Some(window) = obj.windows().first() {
                window.present();
                return;
            }

            let private = beacon_core::cli::Cli::global().private;
            let window = BrowserWindow::new(&obj, private);
            window.present();
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
            for window in self.obj().windows() {
                if let Ok(window) = window.downcast::<BrowserWindow>() {
                    window.imp().shutdown_engine();
                }
            }
            self.parent_shutdown();
        }
    }

    impl GtkApplicationImpl for Application {
        // A closed window, the last one included, stops its engine as it goes.
        fn window_removed(&self, window: &gtk4::Window) {
            if let Some(window) = window.downcast_ref::<BrowserWindow>() {
                window.imp().shutdown_engine();
            }
            self.parent_window_removed(window);
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

    pub(crate) fn setup_actions(&self) {
        let open_urls = gio::SimpleAction::new(OPEN_URLS, Some(&glib::VariantType::new("(bas)").unwrap()));
        open_urls.connect_activate(clone!(
            #[weak(rename_to=app)]
            self,
            move |_, param| {
                let Some((private, urls)) = param.and_then(|p| p.get::<(bool, Vec<String>)>()) else {
                    return;
                };
                info!(target: "gtk", "Opening {} URL(s) from another launch", urls.len());
                if private {
                    // Never in a regular window: that would load them with its cookies and
                    // keep them in its session.
                    let window = BrowserWindow::new(&app, true);
                    window.open_urls(urls);
                    window.present();
                } else {
                    app.window().open_urls(urls);
                }
            }
        ));
        self.add_action(&open_urls);

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
