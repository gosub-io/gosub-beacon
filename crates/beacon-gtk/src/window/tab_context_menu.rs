use crate::runtime;
use crate::window::message::Message;
use beacon_core::tab::TabId;
use gtk4::gio::{Menu, SimpleAction, SimpleActionGroup};
use gtk4::glib::clone;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

/// Simple structure to keep track of tab information. This info is needed in order to enable/disable certain context menu
/// actions.
#[derive(Debug, Clone)]
pub(crate) struct TabInfo {
    /// ID of the tab
    pub(crate) id: TabId,
    /// Tab is pinned or not
    pub(crate) is_pinned: bool,
    /// Tab is at the most left side of the non-pinned tabs
    pub(crate) is_left: bool,
    /// Tab is at the most right side of the non-pinned tabs
    pub(crate) is_right: bool,
    /// Number of total tabs
    pub(crate) tab_count: usize,
}

pub(crate) fn setup_context_menu_actions(action_group: &SimpleActionGroup, window: &super::BrowserWindow, info: TabInfo) {
    let window_clone = window.clone();

    // New Tab to Right
    let new_tab_right = SimpleAction::new("new-tab-right", None);
    new_tab_right.connect_activate(move |_, _| {
        let sender = window_clone.imp().sender.clone();
        runtime().spawn(clone!(
            #[strong]
            sender,
            async move {
                sender
                    .send(Message::OpenTabRight(info.id, "gosub://home".into(), "New Tab".into()))
                    .await
                    .unwrap();
            }
        ));
    });
    action_group.add_action(&new_tab_right);

    // Reload Tab
    let window_clone = window.clone();
    let reload_tab = SimpleAction::new("reload", None);
    reload_tab.connect_activate(move |_, _| {
        let manager = window_clone.imp().tab_manager.lock().unwrap();
        if let Some(tab) = manager.get_tab(info.id) {
            let sender = window_clone.imp().sender.clone();
            runtime().spawn(clone!(
                #[strong]
                sender,
                async move {
                    sender.send(Message::LoadUrl(info.id, tab.url().to_string())).await.unwrap();
                }
            ));
        }
    });
    action_group.add_action(&reload_tab);

    // Mute Tab
    let window_clone = window.clone();
    let mute_tab = SimpleAction::new("mute", None);
    mute_tab.connect_activate(move |_, _| {
        // @todo: implement mute tab
        let sender = window_clone.imp().sender.clone();
        runtime().spawn(clone!(
            #[strong]
            sender,
            async move {
                sender.send(Message::Log("Tab should be muted".into())).await.unwrap();
            }
        ));
    });
    action_group.add_action(&mute_tab);

    // Pin Tab
    let pin_tab = SimpleAction::new("pin", None);
    if info.is_pinned {
        pin_tab.set_enabled(false);
    }
    let window_clone = window.clone();
    pin_tab.connect_activate(move |_, _| {
        let sender = window_clone.imp().sender.clone();
        runtime().spawn(clone!(
            #[strong]
            sender,
            async move {
                sender.send(Message::PinTab(info.id)).await.unwrap();
            }
        ));
    });
    action_group.add_action(&pin_tab);

    // Unpin Tab
    let unpin_tab = SimpleAction::new("unpin", None);
    if !info.is_pinned {
        unpin_tab.set_enabled(false);
    }
    let window_clone = window.clone();
    unpin_tab.connect_activate(move |_, _| {
        let sender = window_clone.imp().sender.clone();
        runtime().spawn(clone!(
            #[strong]
            sender,
            async move {
                sender.send(Message::UnpinTab(info.id)).await.unwrap();
            }
        ));
    });
    action_group.add_action(&unpin_tab);

    // Duplicate Tab
    let window_clone = window.clone();
    let duplicate_tab = SimpleAction::new("duplicate", None);
    duplicate_tab.connect_activate(move |_, _| {
        // @todo: implement duplicate tab
        let sender = window_clone.imp().sender.clone();
        runtime().spawn(clone!(
            #[strong]
            sender,
            async move {
                sender.send(Message::Log("Tab should be duplicated".into())).await.unwrap();
            }
        ));
    });
    action_group.add_action(&duplicate_tab);

    // Close Tab
    let close_tab = SimpleAction::new("close", None);
    let window_clone = window.clone();
    close_tab.connect_activate(move |_, _| {
        window_clone.imp().close_tab(info.id);
        _ = window_clone.imp().get_sender().send_blocking(Message::RefreshTabs());
    });
    action_group.add_action(&close_tab);

    // Reopen Closed Tab
    let window_clone = window.clone();
    let reopen_closed_tab = SimpleAction::new("reopen", None);
    if !window.imp().has_closed_tabs() {
        reopen_closed_tab.set_enabled(false);
    }
    reopen_closed_tab.connect_activate(move |_, _| {
        window_clone.imp().reopen_closed_tab();
    });
    action_group.add_action(&reopen_closed_tab);

    // Close Tabs to Left
    let window_clone = window.clone();
    let close_tabs_left = SimpleAction::new("close-left", None);
    if info.is_pinned || info.is_left {
        close_tabs_left.set_enabled(false);
    }
    close_tabs_left.connect_activate(move |_, _| {
        let manager = window_clone.imp().tab_manager.lock().unwrap();
        let tabs_to_close = manager.closable_tabs_left_of(info.id);
        drop(manager);

        // close all the tabs we need to close
        for tab_id in tabs_to_close {
            window_clone.imp().close_tab(tab_id);
        }
        _ = window_clone.imp().get_sender().send_blocking(Message::RefreshTabs());
    });
    action_group.add_action(&close_tabs_left);

    // Close Tabs to Right
    let window_clone = window.clone();
    let close_tabs_right = SimpleAction::new("close-right", None);
    if info.is_pinned || info.is_right {
        close_tabs_right.set_enabled(false);
    }
    close_tabs_right.connect_activate(move |_, _| {
        let mut own_tab_found = false;
        let mut tabs_to_close = vec![];

        let manager = window_clone.imp().tab_manager.lock().unwrap();
        for tab_id in manager.order() {
            if tab_id == info.id {
                own_tab_found = true;
                continue;
            }

            if own_tab_found {
                tabs_to_close.push(tab_id);
            }
        }
        drop(manager);

        // close all the tabs we need to close
        for tab_id in tabs_to_close {
            window_clone.imp().close_tab(tab_id);
        }
        _ = window_clone.imp().get_sender().send_blocking(Message::RefreshTabs());
    });
    action_group.add_action(&close_tabs_right);

    // Close Other Tabs
    let window_clone = window.clone();
    let close_other_tabs = SimpleAction::new("close-others", None);
    if info.tab_count == 1 {
        close_other_tabs.set_enabled(false);
    }
    close_other_tabs.connect_activate(move |_, _| {
        let manager = window_clone.imp().tab_manager.lock().unwrap();
        // Collect before closing: `close_tab` takes the same lock, so it must not be held here.
        let tabs_to_close = manager
            .order()
            .into_iter()
            .filter(|tab_id| *tab_id != info.id)
            .filter(|tab_id| manager.get_tab(*tab_id).is_some_and(|tab| !tab.is_pinned()))
            .collect::<Vec<_>>();
        drop(manager);

        for tab_id in tabs_to_close {
            window_clone.imp().close_tab(tab_id);
        }
        _ = window_clone.imp().get_sender().send_blocking(Message::RefreshTabs());
    });
    action_group.add_action(&close_other_tabs);
}

pub(crate) fn build_context_menu(tab_info: TabInfo) -> Menu {
    let menu = Menu::new();

    let section = Menu::new();
    section.append(Some("New Tab to Right"), Some("tab.new-tab-right"));
    menu.append_section(None, &section);

    let section = Menu::new();
    section.append(Some("Reload Tab"), Some("tab.reload"));
    section.append(Some("Mute Tab"), Some("tab.mute"));
    if tab_info.is_pinned {
        section.append(Some("Unpin Tab"), Some("tab.unpin"));
    } else {
        section.append(Some("Pin Tab"), Some("tab.pin"));
    }
    section.append(Some("Duplicate Tab"), Some("tab.duplicate"));
    menu.append_section(None, &section);

    let section = Menu::new();
    section.append(Some("Close Tab"), Some("tab.close"));

    let submenu = Menu::new();
    submenu.append(Some("Close Tabs to Left"), Some("tab.close-left"));
    submenu.append(Some("Close Tabs to Right"), Some("tab.close-right"));
    submenu.append(Some("Close Other Tabs"), Some("tab.close-others"));
    section.append_submenu(Some("Close Other Tabs"), &submenu);

    section.append(Some("Reopen Closed Tab"), Some("tab.reopen"));
    menu.append_section(None, &section);

    menu
}
