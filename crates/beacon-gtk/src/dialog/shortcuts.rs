//! The keyboard shortcuts window, built from the table the bindings come from.
//!
//! Nothing here is written by hand: the rows are `crate::shortcuts::SHORTCUTS`, grouped in
//! the order they appear there. A shortcut cannot be listed here without being bound, and
//! cannot be bound without appearing here.

use crate::application::Application;
use crate::shortcuts::{Shortcut, SHORTCUTS};
use gtk4::prelude::BoxExt;
use gtk4::{ShortcutsGroup, ShortcutsSection, ShortcutsShortcut, ShortcutsWindow};

pub struct ShortcutsDialog;

impl ShortcutsDialog {
    pub fn create_dialog(app: &Application) -> ShortcutsWindow {
        let window = ShortcutsWindow::builder()
            .application(app)
            .title("Keyboard Shortcuts")
            .modal(true)
            .build();

        let section = ShortcutsSection::builder().title("Shortcuts").max_height(8).build();
        for (title, rows) in groups() {
            let group = ShortcutsGroup::builder().title(title).build();
            for row in rows {
                let mut builder = ShortcutsShortcut::builder().title(row.title).accelerator(row.accels);
                if let Some(subtitle) = row.subtitle {
                    builder = builder.subtitle(subtitle);
                }
                group.append(&builder.build());
            }
            section.append(&group);
        }
        window.add_section(&section);
        window
    }
}

/// The table's rows, gathered by group, keeping the table's order.
fn groups() -> Vec<(&'static str, Vec<&'static Shortcut>)> {
    let mut groups: Vec<(&'static str, Vec<&'static Shortcut>)> = Vec::new();
    for shortcut in SHORTCUTS {
        match groups.iter_mut().find(|(title, _)| *title == shortcut.group) {
            Some((_, rows)) => rows.push(shortcut),
            None => groups.push((shortcut.group, vec![shortcut])),
        }
    }
    groups
}
