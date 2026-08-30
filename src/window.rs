use gtk4::glib;
use gtk4::glib::{clone, spawn_future_local};

mod config_page;
mod imp;
mod message;
mod page_context_menu;
mod source_page;
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
    pub fn new(app: &Application, private: bool) -> Self {
        let window: Self = glib::Object::builder().property("application", app).build();
        window.imp().private.set(private);
        if private {
            window.add_css_class("private-window");
            window.set_title(Some("Gosub Beacon (Private)"));
        }

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

            // The window is MAPPED here, but not yet ALLOCATED: GLib dispatches
            // G_PRIORITY_DEFAULT (0) sources ahead of the frame clock's layout phase
            // (GDK_PRIORITY_REDRAW = 120), so `content_stack` still measures 0x0. Opening the
            // tabs now gives every one of them the engine's fallback viewport instead of the
            // real one, and each then has to reflow the first time it is shown - visible as the
            // layout jumping sideways. Wait for a real allocation (idle runs at
            // G_PRIORITY_DEFAULT_IDLE = 200, i.e. after layout) so they are born correct.
            let wait_window = window.clone();
            // Bounded: never spin forever if an allocation never arrives (window mapped then
            // immediately hidden). Falling through just restores the previous behaviour.
            let tries = std::cell::Cell::new(0u32);
            glib::idle_add_local(move || {
                let unallocated = wait_window.imp().content_stack.width() <= 0 || wait_window.imp().content_stack.height() <= 0;
                if unallocated && tries.get() < 100 {
                    tries.set(tries.get() + 1);
                    return glib::ControlFlow::Continue;
                }
                let window_clone = wait_window.clone();
                let private = window_clone.imp().private.get();
                spawn_future_local(async move {
                    // URLs on the command line become the startup tabs; without any, the
                    // previous session is restored (falling back to a default set). A
                    // private window starts on the home page only.
                    let mut initial_urls: Vec<String> = crate::cli::Cli::global().urls.clone();
                    let mut session = Vec::new();
                    if private {
                        initial_urls = vec!["gosub://home".to_string()];
                    } else if initial_urls.is_empty() {
                        session = crate::session::load();
                        initial_urls = if session.is_empty() {
                            ["https://gosub.io", "https://adayinthelifeof.nl", "https://news.ycombinator.com"]
                                .map(String::from)
                                .to_vec()
                        } else {
                            session.iter().map(|t| t.url.clone()).collect()
                        };
                    }

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

                    // Re-apply the restored session's pin flags and active tab.
                    if !session.is_empty() {
                        let pinned = (0..session.len()).filter(|i| session[*i].pinned).collect();
                        let active = session.iter().position(|t| t.active);
                        window_clone
                            .imp()
                            .get_sender()
                            .send(Message::RestoreSession { pinned, active })
                            .await
                            .unwrap();
                    }
                });
                glib::ControlFlow::Break
            });
        });

        // Chrome (header bar, tab strip, nav toolbar, bookmarks bar) is hidden in fullscreen.
        // Driven off the window's own state rather than the action, so it cannot drift.
        window.connect_fullscreened_notify(|window| {
            let show_chrome = !window.is_fullscreen();
            let imp = window.imp();
            imp.headerbar.set_visible(show_chrome);
            imp.tab_row.set_visible(show_chrome);
            imp.nav_toolbar.set_visible(show_chrome);
            imp.bookmarks_bar.set_visible(show_chrome);
        });

        window
    }

    fn connect_accelerators(app: &Application, _window: &Self) {
        app.set_accels_for_action("app.open-new-tab", &["<Primary>T"]);
        app.set_accels_for_action("app.close-tab", &["<Primary>W"]);
        app.set_accels_for_action("app.reopen-closed-tab", &["<Primary><Shift>T"]);
        // Ctrl+L is the address bar in every mainstream browser; the log console moves
        // aside to Ctrl+Shift+L.
        app.set_accels_for_action("app.focus-address-bar", &["<Primary>L", "<Alt>D", "F6"]);
        app.set_accels_for_action("app.toggle-log", &["<Primary><Shift>L"]);
        app.set_accels_for_action("app.reload", &["F5", "<Primary>R"]);
        app.set_accels_for_action("app.navigate-back", &["<Alt>Left"]);
        app.set_accels_for_action("app.navigate-forward", &["<Alt>Right"]);
        app.set_accels_for_action("app.new-window", &["<Primary>N"]);
        app.set_accels_for_action("app.bookmark-page", &["<Primary>D"]);
        app.set_accels_for_action("app.zoom-in", &["<Primary>equal", "<Primary>plus", "<Primary>KP_Add"]);
        app.set_accels_for_action("app.zoom-out", &["<Primary>minus", "<Primary>KP_Subtract"]);
        app.set_accels_for_action("app.zoom-reset", &["<Primary>0", "<Primary>KP_0"]);
        app.set_accels_for_action("app.new-private-window", &["<Primary><Shift>P"]);
        app.set_accels_for_action("app.toggle-fullscreen", &["F11"]);
        // MRU tab cycling. Ctrl+Tab arrives as ISO_Left_Tab when Shift is held, which is a
        // separate keysym rather than a modifier on Tab -- both spellings are bound.
        app.set_accels_for_action("app.cycle-tab-next", &["<Primary>Tab"]);
        app.set_accels_for_action("app.cycle-tab-prev", &["<Primary><Shift>Tab", "<Primary><Shift>ISO_Left_Tab"]);
        for n in 1..=9i32 {
            app.set_accels_for_action(&format!("app.select-tab({n})"), &[&format!("<Primary>{n}")]);
        }
    }

    /// The window app actions should act on: the focused one (multi-window safe).
    fn action_target(app: &Application) -> Option<Self> {
        app.active_window().and_then(|w| w.downcast::<Self>().ok())
    }

    fn connect_actions(app: &Application, _window: &Self) {
        // Registered on every window construction; re-registration overwrites the
        // previous action, which is harmless since they all resolve the active window
        // at activation time.
        let logwindow_action = SimpleAction::new("toggle-log", None);
        logwindow_action.connect_activate({
            let app = app.clone();
            move |_, _| {
                let Some(window) = BrowserWindow::action_target(&app) else { return };
                window.imp().log_scroller.set_visible(!window.imp().log_scroller.get_visible());
            }
        });
        app.add_action(&logwindow_action);

        // Create new tab
        let new_tab_action = SimpleAction::new("open-new-tab", None);
        new_tab_action.connect_activate({
            let app = app.clone();
            move |_, _| {
                let Some(window) = BrowserWindow::action_target(&app) else { return };
                let sender = window.imp().sender.clone();
                runtime().spawn(clone!(
                    #[strong]
                    sender,
                    async move {
                        sender
                            .send(Message::OpenTab("gosub://home".into(), "New Tab".into()))
                            .await
                            .unwrap();
                    }
                ));
            }
        });
        app.add_action(&new_tab_action);

        // Close the active tab (Ctrl+W).
        let close_tab_action = SimpleAction::new("close-tab", None);
        close_tab_action.connect_activate({
            let app = app.clone();
            move |_, _| {
                let Some(window) = BrowserWindow::action_target(&app) else { return };
                window.imp().close_active_tab();
            }
        });
        app.add_action(&close_tab_action);

        // Reopen the most recently closed tab (Ctrl+Shift+T).
        let reopen_action = SimpleAction::new("reopen-closed-tab", None);
        reopen_action.connect_activate({
            let app = app.clone();
            move |_, _| {
                let Some(window) = BrowserWindow::action_target(&app) else { return };
                window.imp().reopen_closed_tab();
            }
        });
        app.add_action(&reopen_action);

        // New private window.
        let private_action = SimpleAction::new("new-private-window", None);
        private_action.connect_activate({
            let app = app.clone();
            move |_, _| {
                BrowserWindow::new(&app, true).present();
            }
        });
        app.add_action(&private_action);

        // Fullscreen (F11). Only flips the window state; the chrome hides itself off the
        // resulting `fullscreened` notify, so a change made by the window manager instead of
        // this action (titlebar double-click, compositor keybinding) stays in sync.
        let fullscreen_action = SimpleAction::new("toggle-fullscreen", None);
        fullscreen_action.connect_activate({
            let app = app.clone();
            move |_, _| {
                let Some(window) = BrowserWindow::action_target(&app) else { return };
                window.set_fullscreened(!window.is_fullscreen());
            }
        });
        app.add_action(&fullscreen_action);

        // Focus the address bar and select what is there, so typing replaces the URL.
        let focus_url_action = SimpleAction::new("focus-address-bar", None);
        focus_url_action.connect_activate({
            let app = app.clone();
            move |_, _| {
                let Some(window) = BrowserWindow::action_target(&app) else { return };
                let imp = window.imp();
                imp.searchbar.grab_focus();
                imp.searchbar.select_region(0, -1);
            }
        });
        app.add_action(&focus_url_action);

        // Reload (F5 / Ctrl+R). Reuses the toolbar button's helper, which doubles as stop
        // while a page is still loading.
        let reload_action = SimpleAction::new("reload", None);
        reload_action.connect_activate({
            let app = app.clone();
            move |_, _| {
                let Some(window) = BrowserWindow::action_target(&app) else { return };
                let imp = window.imp();
                if let Some(tab_id) = imp.active_tab_id() {
                    imp.reload_or_stop(tab_id);
                }
            }
        });
        app.add_action(&reload_action);

        let back_action = SimpleAction::new("navigate-back", None);
        back_action.connect_activate({
            let app = app.clone();
            move |_, _| {
                let Some(window) = BrowserWindow::action_target(&app) else { return };
                window.imp().navigate_back();
            }
        });
        app.add_action(&back_action);

        // Forward is anchored to the toolbar button: with several forward branches the
        // helper pops a menu asking which to follow, and it needs somewhere to appear.
        let forward_action = SimpleAction::new("navigate-forward", None);
        forward_action.connect_activate({
            let app = app.clone();
            move |_, _| {
                let Some(window) = BrowserWindow::action_target(&app) else { return };
                let imp = window.imp();
                let anchor = imp.btn_next.clone();
                imp.navigate_forward(&anchor);
            }
        });
        app.add_action(&forward_action);

        // Ctrl+D: same toggle as the toolbar star, so both add/remove identically. Internal
        // pages are not bookmarkable and the helper simply returns for them.
        let bookmark_action = SimpleAction::new("bookmark-page", None);
        bookmark_action.connect_activate({
            let app = app.clone();
            move |_, _| {
                let Some(window) = BrowserWindow::action_target(&app) else { return };
                window.imp().toggle_bookmark();
            }
        });
        app.add_action(&bookmark_action);

        let new_window_action = SimpleAction::new("new-window", None);
        new_window_action.connect_activate({
            let app = app.clone();
            move |_, _| {
                BrowserWindow::new(&app, false).present();
            }
        });
        app.add_action(&new_window_action);

        for (name, direction) in [("cycle-tab-next", 1), ("cycle-tab-prev", -1)] {
            let action = SimpleAction::new(name, None);
            action.connect_activate({
                let app = app.clone();
                move |_, _| {
                    let Some(window) = BrowserWindow::action_target(&app) else { return };
                    window.imp().cycle_tab(direction);
                }
            });
            app.add_action(&action);
        }

        // One parameterised action rather than nine. Parameter is i32, not u32: the accel is spelled `app.select-tab(3)`, and GVariant
        // parses a bare integer literal as int32. Declaring u32 here makes the detailed
        // action name fail to match and the accel silently never fires.
        let select_tab = SimpleAction::new("select-tab", Some(&i32::static_variant_type()));
        select_tab.connect_activate({
            let app = app.clone();
            move |_, param| {
                let Some(window) = BrowserWindow::action_target(&app) else { return };
                let Some(n) = param.and_then(|p| p.get::<i32>()) else { return };
                window.imp().select_tab_by_position(n.max(0) as usize);
            }
        });
        app.add_action(&select_tab);

        // Page zoom on the active tab.
        for (name, direction) in [("zoom-in", 1), ("zoom-out", -1)] {
            let action = SimpleAction::new(name, None);
            action.connect_activate({
                let app = app.clone();
                move |_, _| {
                    let Some(window) = BrowserWindow::action_target(&app) else { return };
                    let imp = window.imp();
                    if let Some(tab_id) = imp.active_tab_id() {
                        imp.zoom_step(tab_id, direction);
                    }
                }
            });
            app.add_action(&action);
        }
        let zoom_reset = SimpleAction::new("zoom-reset", None);
        zoom_reset.connect_activate({
            let app = app.clone();
            move |_, _| {
                let Some(window) = BrowserWindow::action_target(&app) else { return };
                let imp = window.imp();
                if let Some(tab_id) = imp.active_tab_id() {
                    imp.set_zoom(tab_id, 1.0);
                }
            }
        });
        app.add_action(&zoom_reset);

        // Tab switching (chip clicks) is handled by `imp::BrowserWindow::activate_tab`,
        // which also syncs the address bar and nav buttons.
    }
}
