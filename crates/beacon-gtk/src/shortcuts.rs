//! Every keyboard shortcut the chrome binds, in one table.
//!
//! It exists because the shortcuts window used to be written out by hand beside the
//! bindings, and had drifted: it offered `Ctrl+D` for dark mode (which is `Ctrl+Shift+D`,
//! because `Ctrl+D` bookmarks the page), `Ctrl+L` for the developer pane (that focuses the
//! address bar), an `Ctrl+O` that was never bound, and none of the twenty accelerators the
//! window actually registers. Help that disagrees with the program is worse than no help.
//!
//! So the table below is the source: `Application` and `BrowserWindow` register from it,
//! and the shortcuts window is built from the same rows. A binding that is not here is not
//! bound, and one that is here cannot be missing from the help.

use gtk4::prelude::*;

/// Where a shortcut is registered — the application's own, or a window's.
#[derive(Clone, Copy, PartialEq)]
pub enum Scope {
    Application,
    Window,
}

pub struct Shortcut {
    /// The action it activates, as `set_accels_for_action` spells it.
    pub action: &'static str,
    pub title: &'static str,
    /// Which group of the shortcuts window it belongs to; the order here is the order shown.
    pub group: &'static str,
    /// Space-separated, as `ShortcutsShortcut` reads them: alternatives for the same thing.
    pub accels: &'static str,
    pub scope: Scope,
    /// Shown under the keys when the row stands for a family of them.
    pub subtitle: Option<&'static str>,
}

/// The nine `Ctrl+1`…`Ctrl+9` bindings are generated rather than listed: they activate one
/// parameterised action, so they cannot be spelled as a fixed action name. The table below
/// carries a single row for them, for the help.
pub const TAB_NUMBER_ACTION: &str = "app.select-tab";

pub const SHORTCUTS: &[Shortcut] = &[
    // ── Tabs ────────────────────────────────────────────────────────────
    Shortcut {
        action: "app.open-new-tab",
        title: "New tab",
        group: "Tabs",
        accels: "<Primary>T",
        scope: Scope::Window,
        subtitle: None,
    },
    Shortcut {
        action: "app.close-tab",
        title: "Close tab",
        group: "Tabs",
        accels: "<Primary>W",
        scope: Scope::Window,
        subtitle: None,
    },
    Shortcut {
        action: "app.reopen-closed-tab",
        title: "Reopen closed tab",
        group: "Tabs",
        accels: "<Primary><Shift>T",
        scope: Scope::Window,
        subtitle: None,
    },
    Shortcut {
        action: "app.cycle-tab-next",
        title: "Next tab",
        group: "Tabs",
        accels: "<Primary>Tab",
        scope: Scope::Window,
        subtitle: Some("Most recently used order"),
    },
    Shortcut {
        action: "app.cycle-tab-prev",
        title: "Previous tab",
        group: "Tabs",
        accels: "<Primary><Shift>Tab",
        scope: Scope::Window,
        subtitle: None,
    },
    // Generated, not registered from this row -- see TAB_NUMBER_ACTION.
    Shortcut {
        action: "",
        title: "Select tab by number",
        group: "Tabs",
        accels: "<Primary>1",
        scope: Scope::Window,
        subtitle: Some("Ctrl+1 to Ctrl+9"),
    },
    // ── Navigation ──────────────────────────────────────────────────────
    Shortcut {
        action: "app.navigate-back",
        title: "Back",
        group: "Navigation",
        accels: "<Alt>Left",
        scope: Scope::Window,
        subtitle: None,
    },
    Shortcut {
        action: "app.navigate-forward",
        title: "Forward",
        group: "Navigation",
        accels: "<Alt>Right",
        scope: Scope::Window,
        subtitle: None,
    },
    Shortcut {
        action: "app.reload",
        title: "Reload",
        group: "Navigation",
        accels: "F5 <Primary>R",
        scope: Scope::Window,
        subtitle: Some("Stops the page while it is still loading"),
    },
    Shortcut {
        action: "app.reload-ignoring-cache",
        title: "Reload ignoring cache",
        group: "Navigation",
        accels: "<Primary><Shift>R <Primary>F5",
        scope: Scope::Window,
        subtitle: None,
    },
    Shortcut {
        action: "app.focus-address-bar",
        title: "Focus the address bar",
        group: "Navigation",
        accels: "<Primary>L <Alt>D F6",
        scope: Scope::Window,
        subtitle: None,
    },
    // ── Page ────────────────────────────────────────────────────────────
    Shortcut {
        action: "app.bookmark-page",
        title: "Bookmark this page",
        group: "Page",
        accels: "<Primary>D",
        scope: Scope::Window,
        subtitle: None,
    },
    Shortcut {
        action: "app.zoom-in",
        title: "Zoom in",
        group: "Page",
        accels: "<Primary>equal <Primary>plus",
        scope: Scope::Window,
        subtitle: None,
    },
    Shortcut {
        action: "app.zoom-out",
        title: "Zoom out",
        group: "Page",
        accels: "<Primary>minus",
        scope: Scope::Window,
        subtitle: None,
    },
    Shortcut {
        action: "app.zoom-reset",
        title: "Actual size",
        group: "Page",
        accels: "<Primary>0",
        scope: Scope::Window,
        subtitle: None,
    },
    // ── Windows and chrome ──────────────────────────────────────────────
    Shortcut {
        action: "app.new-window",
        title: "New window",
        group: "Window",
        accels: "<Primary>N",
        scope: Scope::Window,
        subtitle: None,
    },
    Shortcut {
        action: "app.new-private-window",
        title: "New private window",
        group: "Window",
        accels: "<Primary><Shift>P",
        scope: Scope::Window,
        subtitle: None,
    },
    Shortcut {
        action: "app.toggle-fullscreen",
        title: "Fullscreen",
        group: "Window",
        accels: "F11",
        scope: Scope::Window,
        subtitle: None,
    },
    Shortcut {
        action: "app.toggle-bookmarks-bar",
        title: "Show the bookmarks bar",
        group: "Window",
        accels: "<Primary><Shift>B",
        scope: Scope::Window,
        subtitle: None,
    },
    Shortcut {
        action: "app.toggle-activity",
        title: "Show what the engine is doing",
        group: "Window",
        accels: "<Primary><Shift>A",
        scope: Scope::Window,
        subtitle: Some("Four status lines over the page with a running clock"),
    },
    Shortcut {
        action: "app.toggle-dark-mode",
        title: "Toggle dark mode",
        group: "Window",
        accels: "<Primary><Shift>D",
        scope: Scope::Application,
        subtitle: Some("Ctrl+D bookmarks the page, as in every other browser"),
    },
    Shortcut {
        action: "app.quit",
        title: "Quit",
        group: "Window",
        accels: "<Primary>Q",
        scope: Scope::Application,
        subtitle: None,
    },
    // ── Developer and help ──────────────────────────────────────────────
    Shortcut {
        action: "app.toggle-log",
        title: "Developer tools",
        group: "Developer",
        accels: "<Primary><Shift>I <Primary><Shift>L",
        scope: Scope::Window,
        subtitle: None,
    },
    Shortcut {
        action: "app.show-about",
        title: "About Gosub Beacon",
        group: "Developer",
        accels: "F1",
        scope: Scope::Application,
        subtitle: None,
    },
    Shortcut {
        action: "app.show-shortcuts",
        title: "Keyboard shortcuts",
        group: "Developer",
        accels: "F2",
        scope: Scope::Application,
        subtitle: None,
    },
];

/// Bind every shortcut of one scope. `Ctrl+Tab` arrives as `ISO_Left_Tab` when Shift is
/// held — a separate keysym rather than a modifier on Tab — so that one binding is widened
/// here rather than shown twice in the help.
pub fn register(app: &impl IsA<gtk4::Application>, scope: Scope) {
    let app = app.as_ref();
    for shortcut in SHORTCUTS.iter().filter(|s| s.scope == scope && !s.action.is_empty()) {
        let mut accels: Vec<&str> = shortcut.accels.split(' ').collect();
        if shortcut.action == "app.cycle-tab-prev" {
            accels.push("<Primary><Shift>ISO_Left_Tab");
        }
        app.set_accels_for_action(shortcut.action, &accels);
    }
    if scope == Scope::Window {
        // One parameterised action, nine keys: `app.select-tab(3)`.
        for n in 1..=9i32 {
            app.set_accels_for_action(&format!("{TAB_NUMBER_ACTION}({n})"), &[&format!("<Primary>{n}")]);
        }
    }
}
