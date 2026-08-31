//! Right-click menu over the page. The engine answers "what is at (x,y)" (link, image, text,
//! editable) via `HitTestResult`; this module turns that answer into a native GTK popover
//! menu. Items appear only when relevant: link items for a link, image items for an image,
//! Copy for text, and the always-present navigation trio.

use crate::runtime;
use crate::window::message::Message;
use beacon_core::tab::TabId;
use gosub_engine::events::{HitTestResponse, TabCommand as EngineTabCommand};
use gtk4::gio::{Menu, SimpleAction, SimpleActionGroup};
use gtk4::graphene::Point;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gdk, PopoverMenu, PopoverMenuFlags};

/// Build and pop up the page context menu at `point` (window coordinates) for `tab_id`.
pub(crate) fn show(window: &super::BrowserWindow, tab_id: TabId, point: Point, hit: HitTestResponse) {
    let menu = Menu::new();
    let actions = SimpleActionGroup::new();

    // Navigation - always present.
    let nav = Menu::new();
    nav.append(Some("Back"), Some("page.back"));
    nav.append(Some("Forward"), Some("page.forward"));
    nav.append(Some("Reload"), Some("page.reload"));
    menu.append_section(None, &nav);
    add_action(&actions, "back", {
        let window = window.clone();
        move || window.imp().navigate_back()
    });
    add_action(&actions, "forward", {
        let window = window.clone();
        move || window.imp().send_history_command(EngineTabCommand::GoForward { entry: None })
    });
    add_action(&actions, "reload", {
        let window = window.clone();
        move || window.imp().reload_or_stop(tab_id)
    });

    if let Some(link) = hit.link_url.clone() {
        let section = Menu::new();
        section.append(Some("Open Link in New Tab"), Some("page.open-link-new-tab"));
        section.append(Some("Save Link As…"), Some("page.save-link-as"));
        section.append(Some("Copy Link Address"), Some("page.copy-link"));
        menu.append_section(None, &section);
        add_action(&actions, "save-link-as", {
            let window = window.clone();
            let link = link.clone();
            move || {
                let name = link.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("download");
                window.imp().save_download_as(tab_id, link.clone(), name);
            }
        });
        add_action(&actions, "open-link-new-tab", {
            let window = window.clone();
            let link = link.clone();
            move || {
                let sender = window.imp().get_sender();
                let link = link.clone();
                runtime().spawn(async move {
                    let _ = sender.send(Message::OpenTabRight(tab_id, link, "New Tab".into())).await;
                });
            }
        });
        add_action(&actions, "copy-link", {
            let window = window.clone();
            move || copy_text(&window, &link)
        });
    }

    if let Some(image) = hit.image_url.clone() {
        let section = Menu::new();
        section.append(Some("Open Image in New Tab"), Some("page.open-image-new-tab"));
        section.append(Some("Copy Image Address"), Some("page.copy-image"));
        menu.append_section(None, &section);
        add_action(&actions, "open-image-new-tab", {
            let window = window.clone();
            let image = image.clone();
            move || {
                let sender = window.imp().get_sender();
                let image = image.clone();
                runtime().spawn(async move {
                    let _ = sender.send(Message::OpenTabRight(tab_id, image, "Image".into())).await;
                });
            }
        });
        add_action(&actions, "copy-image", {
            let window = window.clone();
            move || copy_text(&window, &image)
        });
    }

    // Until the engine has selection, "Copy" copies the text node under the pointer.
    if let Some(text) = hit.selection.clone().or_else(|| hit.text.clone()) {
        let section = Menu::new();
        section.append(Some("Copy"), Some("page.copy"));
        menu.append_section(None, &section);
        add_action(&actions, "copy", {
            let window = window.clone();
            move || copy_text(&window, &text)
        });
    }

    let popover = PopoverMenu::builder()
        .menu_model(&menu)
        .halign(gtk4::Align::Start)
        .has_arrow(false)
        .flags(PopoverMenuFlags::NESTED)
        .build();
    popover.insert_action_group("page", Some(&actions));
    popover.set_parent(window);
    popover.connect_closed(|p| p.unparent());
    popover.set_pointing_to(Some(&gdk::Rectangle::new(point.x() as i32, point.y() as i32, 0, 0)));
    popover.popup();
}

fn add_action(group: &SimpleActionGroup, name: &str, f: impl Fn() + 'static) {
    let action = SimpleAction::new(name, None);
    action.connect_activate(move |_, _| f());
    group.add_action(&action);
}

fn copy_text(window: &super::BrowserWindow, text: &str) {
    window.clipboard().set_text(text);
}
