use crate::dialog::about::About;
use crate::dialog::shortcuts::ShortcutsDialog;
use crate::window::BrowserWindow;
use crate::APP_ID;
use gtk4::glib::clone;
use gtk4::subclass::prelude::GtkApplicationImpl;
use gtk4::{gio, glib, prelude::*, subclass::prelude::*, Settings};
use gtk_macros::action;
use log::info;

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

            let private = crate::cli::Cli::global().private;
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
    }

    impl GtkApplicationImpl for Application {}
}

glib::wrapper! {
    pub struct Application(ObjectSubclass<imp::Application>)
        @extends gio::Application, gtk4::Application,
        @implements gio::ActionMap, gio::ActionGroup;
}

impl Application {
    pub fn new() -> Self {
        glib::Object::builder()
            .property("application-id", APP_ID)
            .property("resource-base-path", Some("/io/gosub/beacon"))
            .build()
    }

    pub fn window(&self) -> BrowserWindow {
        self.active_window()
            .map_or_else(|| BrowserWindow::new(self, false), |w| w.downcast().unwrap())
    }

    fn setup_actions(&self) {
        action!(
            self,
            "quit",
            clone!(
                #[weak(rename_to=app)]
                self,
                move |_, _| {
                    app.quit();
                }
            )
        );

        action!(
            self,
            "toggle-dark-mode",
            clone!(
                #[weak(rename_to=_app)]
                self,
                move |_, _| {
                    info!(target: "gtk", "Toggle dark mode action triggered");
                    let settings = Settings::default().expect("Failed to get default GtkSettings");
                    let mode: bool = settings.property("gtk-application-prefer-dark-theme");
                    settings.set_property("gtk-application-prefer-dark-theme", !mode);
                }
            )
        );

        action!(
            self,
            "show-about",
            clone!(
                #[weak(rename_to=app)]
                self,
                move |_, _| {
                    info!(target: "gtk", "Show about dialog action triggered");
                    let about = About::create_dialog();
                    about.set_transient_for(app.active_window().as_ref());
                    about.present();
                }
            )
        );

        action!(
            self,
            "show-shortcuts",
            clone!(
                #[weak(rename_to=app)]
                self,
                move |_, _| {
                    info!(target: "gtk", "Show about dialog action triggered");
                    let about = ShortcutsDialog::create_dialog(&app);
                    about.present();
                }
            )
        );
    }

    fn setup_accelerators(&self) {
        // Global application accelerators
        self.set_accels_for_action("app.quit", &["<Primary>Q"]);
        self.set_accels_for_action("app.toggle-dark-mode", &["<Primary>D"]);
        self.set_accels_for_action("app.show-about", &["F1"]);
        self.set_accels_for_action("app.show-shortcuts", &["F2"]);
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
