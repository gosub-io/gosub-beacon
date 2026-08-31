use crate::application::Application;
use gtk4::prelude::{BoxExt, GtkWindowExt};
use gtk4::{ShortcutsGroup, ShortcutsSection, ShortcutsShortcut, ShortcutsWindow};

pub struct ShortcutsDialog;

impl ShortcutsDialog {
    pub fn create_dialog(app: &Application) -> ShortcutsWindow {
        let shortcuts_window = ShortcutsWindow::builder().application(app).title("Keyboard Shortcuts").build();

        shortcuts_window.set_modal(true);

        let section = Self::general_section();
        shortcuts_window.add_section(&section);

        let section = Self::fkeys_section();
        shortcuts_window.add_section(&section);

        shortcuts_window
    }

    fn general_section() -> ShortcutsSection {
        let section = ShortcutsSection::builder().title("General").max_height(4).build();

        let group = Self::general_file_group();
        section.append(&group);
        let group = Self::general_developer_group("Developer");
        section.append(&group);

        section
    }

    fn general_file_group() -> ShortcutsGroup {
        let group = ShortcutsGroup::builder().title("File operations").build();

        let new_tab = ShortcutsShortcut::builder().title("New Tab").accelerator("<Ctrl>T").build();

        let open_shortcut = ShortcutsShortcut::builder().title("Open File").accelerator("<Ctrl>O").build();

        let toggle_darkmode = ShortcutsShortcut::builder()
            .title("Toggle dark mode")
            .accelerator("<Ctrl>D")
            .build();

        group.append(&new_tab);
        group.append(&open_shortcut);
        group.append(&toggle_darkmode);

        group
    }

    fn general_developer_group(title: &str) -> ShortcutsGroup {
        let group = ShortcutsGroup::builder().title(title).build();

        let toggle_log_window = ShortcutsShortcut::builder()
            .title("Toggle log window")
            .accelerator("<Ctrl>L")
            .build();
        group.append(&toggle_log_window);

        group
    }

    fn fkeys_section() -> ShortcutsSection {
        let section = ShortcutsSection::builder().title("Function Keys").max_height(4).build();

        let group = ShortcutsGroup::builder().title("Function Keys").build();

        let fkeys = ["Help Dialog", "Shortcut Dialog", "", "", "", "", "", "", "", "Developer Toolbar"];
        for (i, key) in fkeys.iter().enumerate() {
            let shortcut = ShortcutsShortcut::builder()
                .title(key.to_string())
                .accelerator(format!("F{}", i + 1))
                .build();
            group.append(&shortcut);
        }

        section.append(&group);

        section
    }
}
