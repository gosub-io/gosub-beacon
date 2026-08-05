use gtk4::glib;
use gtk4::glib::{clone, spawn_future_local};

mod imp;
mod message;
mod tab_context_menu;

use crate::application::Application;
use crate::runtime;
use crate::window::message::Message;
use gtk4::gio;
use gtk4::gio::SimpleAction;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;

// This wrapper must be in a different module than the implementation, because both will define a
// `struct BrowserWindow` and they would clash. In this case, the browser window is a subclass of
// its implementation.
glib::wrapper! {
    pub struct BrowserWindow(ObjectSubclass<imp::BrowserWindow>)
        @extends gtk4::ApplicationWindow, gtk4::Window, gtk4::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget, gtk4::Native, gtk4::Root, gtk4::ShortcutManager;
}

impl BrowserWindow {
    pub fn new(app: &Application) -> Self {
        let window: Self = glib::Object::builder().property("application", app).build();

        window.set_resizable(true);
        window.set_decorated(true);
        window.set_default_size(1024, 768);

        let builder = gtk4::Builder::from_resource("/io/gosub/beacon/ui/main_menu.ui");
        let menubar = builder.object::<gio::MenuModel>("app-menu").expect("Could not find app-menu");

        // The menubar lives inside the headerbar (left of the tab strip) instead
        // of occupying its own row.
        window.set_show_menubar(false);
        let menu_bar = gtk4::PopoverMenuBar::from_model(Some(&menubar));
        menu_bar.add_css_class("header-menubar");
        window.imp().headerbar.pack_start(&menu_bar);

        Self::connect_actions(app, &window);
        Self::connect_accelerators(app, &window);

        // Start the engine and wire its redraw/event notifications before opening any tabs.
        window.imp().init_engine();

        // Spawn handler
        let window_clone = window.clone();
        spawn_future_local(async move {
            loop {
                match window_clone.imp().get_receiver().recv().await {
                    Ok(message) => {
                        window_clone.imp().handle_message(message).await;
                    }
                    Err(e) => {
                        log::error!("Error receiving message: {:?}", e);
                        return;
                    }
                }
            }
        });

        // Open the startup tabs only once the window is mapped, i.e. it has a real
        // allocated size. Creating tabs before then makes GTK measure the tab-label
        // icons against a zero-sized parent, producing `width 0, height -9`
        // allocation warnings.
        let opened = std::rc::Rc::new(std::cell::Cell::new(false));
        window.connect_map(move |window| {
            if opened.replace(true) {
                return;
            }

            let window_clone = window.clone();
            spawn_future_local(async move {
                let initial_urls = [
                    "https://gosub.io",
                    "https://adayinthelifeof.nl",
                    "https://news.ycombinator.com",
                ];

                for url in initial_urls.iter() {
                    window_clone
                        .imp()
                        .get_sender()
                        .send(Message::OpenTab(url.to_string(), "New Tab".to_string()))
                        .await
                        .unwrap();
                }

                // Refresh tabs on startup
                window_clone.imp().get_sender().send(Message::RefreshTabs()).await.unwrap();
            });
        });

        window
    }

    fn connect_accelerators(app: &Application, _window: &Self) {
        app.set_accels_for_action("app.open-new-tab", &["<Primary>T"]);
        app.set_accels_for_action("app.close-tab", &["<Primary>W"]);
        app.set_accels_for_action("app.toggle-log", &["<Primary>L"]);
    }

    fn connect_actions(app: &Application, window: &Self) {
        let logwindow_action = SimpleAction::new("toggle-log", None);
        logwindow_action.connect_activate({
            let window_clone = window.clone();
            move |_, _| {
                window_clone
                    .imp()
                    .log_scroller
                    .set_visible(!window_clone.imp().log_scroller.get_visible());
            }
        });
        app.add_action(&logwindow_action);

        // Create new tab
        let window_clone = window.clone();
        let new_tab_action = SimpleAction::new("open-new-tab", None);
        new_tab_action.connect_activate(move |_, _| {
            let sender = window_clone.imp().sender.clone();
            runtime().spawn(clone!(
                #[strong]
                sender,
                async move {
                    sender.send(Message::OpenTab("about:blank".into(), "New Tab".into())).await.unwrap();
                }
            ));
        });
        app.add_action(&new_tab_action);

        // Tab switching (chip clicks) is handled by `imp::BrowserWindow::activate_tab`,
        // which also syncs the address bar and nav buttons.
    }
}
