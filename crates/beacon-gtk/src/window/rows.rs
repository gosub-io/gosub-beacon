//! The row objects the devtools tables bind to.
//!
//! Each table is a `GtkColumnView` over a `ListStore` of one of these. Binding to properties
//! rather than building labels means a refresh writes new values into the rows already on
//! screen: the widgets, the selection and the scroll position all stay put, and a column's
//! width is the view's business rather than a constant in this file.
//!
//! Every row carries a `css` property because the tables share one column factory, and the
//! classes are the only part of a cell that varies by row -- a failed request, a warning in
//! the log.

use gtk4::glib;

mod imp_request {
    use gtk4::glib;
    use gtk4::glib::Properties;
    use gtk4::prelude::*;
    use gtk4::subclass::prelude::*;
    use std::cell::{Cell, RefCell};

    #[derive(Properties, Default)]
    #[properties(wrapper_type = super::RequestRow)]
    pub struct RequestRow {
        /// The engine's request id, as text: it identifies the row across a refresh, and the
        /// detail pane is looked up by it.
        #[property(get, set)]
        pub id: RefCell<String>,
        #[property(get, set)]
        pub status: RefCell<String>,
        #[property(get, set)]
        pub method: RefCell<String>,
        #[property(get, set)]
        pub kind: RefCell<String>,
        #[property(get, set)]
        pub size: RefCell<String>,
        #[property(get, set)]
        pub time: RefCell<String>,
        /// The URL in full. The column shows what it has room for and ellipsizes the rest,
        /// which the reader can widen -- so nothing is thrown away before it gets there.
        #[property(get, set)]
        pub url: RefCell<String>,
        /// Space-separated CSS classes for this row's cells.
        #[property(get, set)]
        pub css: RefCell<String>,
        /// The waterfall bar, as fractions of the page's whole load: where it starts, how
        /// much of it was waiting, and how much was the body arriving. Worked out where the
        /// span across every request is known, so the bar itself only has to draw.
        #[property(get, set)]
        pub bar_offset: Cell<f64>,
        #[property(get, set)]
        pub bar_wait: Cell<f64>,
        #[property(get, set)]
        pub bar_body: Cell<f64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RequestRow {
        const NAME: &'static str = "BeaconRequestRow";
        type Type = super::RequestRow;
    }

    #[glib::derived_properties]
    impl ObjectImpl for RequestRow {}
}

mod imp_log {
    use gtk4::glib;
    use gtk4::glib::Properties;
    use gtk4::prelude::*;
    use gtk4::subclass::prelude::*;
    use std::cell::RefCell;

    #[derive(Properties, Default)]
    #[properties(wrapper_type = super::LogRow)]
    pub struct LogRow {
        #[property(get, set)]
        pub time: RefCell<String>,
        #[property(get, set)]
        pub level: RefCell<String>,
        #[property(get, set)]
        pub target: RefCell<String>,
        #[property(get, set)]
        pub message: RefCell<String>,
        #[property(get, set)]
        pub css: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LogRow {
        const NAME: &'static str = "BeaconLogRow";
        type Type = super::LogRow;
    }

    #[glib::derived_properties]
    impl ObjectImpl for LogRow {}
}

mod imp_timing {
    use gtk4::glib;
    use gtk4::glib::Properties;
    use gtk4::prelude::*;
    use gtk4::subclass::prelude::*;
    use std::cell::{Cell, RefCell};

    #[derive(Properties, Default)]
    #[properties(wrapper_type = super::TimingRow)]
    pub struct TimingRow {
        #[property(get, set)]
        pub namespace: RefCell<String>,
        /// What the namespace measures, in a sentence. Shown on hover.
        #[property(get, set)]
        pub description: RefCell<String>,
        /// Whether `description` is a real explanation rather than the namespace repeated
        /// back. Only an explained row earns the info icon.
        #[property(get, set)]
        pub explained: Cell<bool>,
        #[property(get, set)]
        pub count: RefCell<String>,
        #[property(get, set)]
        pub total: RefCell<String>,
        #[property(get, set)]
        pub avg: RefCell<String>,
        #[property(get, set)]
        pub p50: RefCell<String>,
        #[property(get, set)]
        pub p95: RefCell<String>,
        /// The slowest single sample. Named `peak` rather than `max`, whose getter would
        /// collide with `Ord::max`.
        #[property(get, set)]
        pub peak: RefCell<String>,
        #[property(get, set)]
        pub css: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TimingRow {
        const NAME: &'static str = "BeaconTimingRow";
        type Type = super::TimingRow;
    }

    #[glib::derived_properties]
    impl ObjectImpl for TimingRow {}
}

mod imp_report {
    use gtk4::glib;
    use gtk4::glib::Properties;
    use gtk4::prelude::*;
    use gtk4::subclass::prelude::*;
    use std::cell::RefCell;

    #[derive(Properties, Default)]
    #[properties(wrapper_type = super::ReportRow)]
    pub struct ReportRow {
        /// HTML or CSS.
        #[property(get, set)]
        pub section: RefCell<String>,
        #[property(get, set)]
        pub kind: RefCell<String>,
        /// What was dropped, or the parser's error name.
        #[property(get, set)]
        pub subject: RefCell<String>,
        /// Where, as `line:column` or `stylesheet:line`.
        #[property(get, set)]
        pub location: RefCell<String>,
        #[property(get, set)]
        pub count: RefCell<String>,
        /// What the row means, shown on hover.
        #[property(get, set)]
        pub hint: RefCell<String>,
        #[property(get, set)]
        pub css: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ReportRow {
        const NAME: &'static str = "BeaconReportRow";
        type Type = super::ReportRow;
    }

    #[glib::derived_properties]
    impl ObjectImpl for ReportRow {}
}

mod imp_history {
    use gtk4::glib;
    use gtk4::glib::Properties;
    use gtk4::prelude::*;
    use gtk4::subclass::prelude::*;
    use std::cell::{Cell, RefCell};

    #[derive(Properties, Default)]
    #[properties(wrapper_type = super::HistoryRow)]
    pub struct HistoryRow {
        /// The engine's history entry id. A click is resolved through it rather than the row's
        /// position, so a refresh that inserts a branch above cannot send the jump elsewhere.
        #[property(get, set)]
        pub entry: Cell<u64>,
        /// Side-branch depth; the tree column indents by it.
        #[property(get, set)]
        pub depth: Cell<u32>,
        /// The title, or the URL while the page has none -- what the tree column shows.
        #[property(get, set)]
        pub title: RefCell<String>,
        #[property(get, set)]
        pub url: RefCell<String>,
        /// A one-character mark before the title: where the tab is, and where forward goes.
        #[property(get, set)]
        pub marker: RefCell<String>,
        #[property(get, set)]
        pub css: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for HistoryRow {
        const NAME: &'static str = "BeaconHistoryRow";
        type Type = super::HistoryRow;
    }

    #[glib::derived_properties]
    impl ObjectImpl for HistoryRow {}
}

mod imp_origin {
    use gtk4::glib;
    use gtk4::glib::Properties;
    use gtk4::prelude::*;
    use gtk4::subclass::prelude::*;
    use std::cell::RefCell;

    #[derive(Properties, Default)]
    #[properties(wrapper_type = super::OriginRow)]
    pub struct OriginRow {
        #[property(get, set)]
        pub origin: RefCell<String>,
        /// "first" or "third".
        #[property(get, set)]
        pub party: RefCell<String>,
        #[property(get, set)]
        pub requests: RefCell<String>,
        #[property(get, set)]
        pub size: RefCell<String>,
        #[property(get, set)]
        pub kinds: RefCell<String>,
        #[property(get, set)]
        pub css: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for OriginRow {
        const NAME: &'static str = "BeaconOriginRow";
        type Type = super::OriginRow;
    }

    #[glib::derived_properties]
    impl ObjectImpl for OriginRow {}
}

mod imp_process {
    use gtk4::glib;
    use gtk4::glib::Properties;
    use gtk4::prelude::*;
    use gtk4::subclass::prelude::*;
    use std::cell::RefCell;

    #[derive(Properties, Default)]
    #[properties(wrapper_type = super::ProcessRow)]
    pub struct ProcessRow {
        /// The name, indented by its depth in the tree.
        #[property(get, set)]
        pub name: RefCell<String>,
        #[property(get, set)]
        pub pid: RefCell<String>,
        #[property(get, set)]
        pub memory: RefCell<String>,
        #[property(get, set)]
        pub uptime: RefCell<String>,
        #[property(get, set)]
        pub detail: RefCell<String>,
        #[property(get, set)]
        pub css: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ProcessRow {
        const NAME: &'static str = "BeaconProcessRow";
        type Type = super::ProcessRow;
    }

    #[glib::derived_properties]
    impl ObjectImpl for ProcessRow {}
}

glib::wrapper! {
    /// One process in the Processes tab.
    pub struct ProcessRow(ObjectSubclass<imp_process::ProcessRow>);
}

impl Default for ProcessRow {
    fn default() -> Self {
        glib::Object::new()
    }
}

glib::wrapper! {
    /// Every request one origin served, in the network table's By origin view.
    pub struct OriginRow(ObjectSubclass<imp_origin::OriginRow>);
}

impl Default for OriginRow {
    fn default() -> Self {
        glib::Object::new()
    }
}

glib::wrapper! {
    /// One request in the network table.
    pub struct RequestRow(ObjectSubclass<imp_request::RequestRow>);
}

glib::wrapper! {
    /// One record in the debug log.
    pub struct LogRow(ObjectSubclass<imp_log::LogRow>);
}

glib::wrapper! {
    /// One namespace in the timings table.
    pub struct TimingRow(ObjectSubclass<imp_timing::TimingRow>);
}

glib::wrapper! {
    /// One line of the Page report.
    pub struct ReportRow(ObjectSubclass<imp_report::ReportRow>);
}

impl Default for ReportRow {
    fn default() -> Self {
        glib::Object::new()
    }
}

glib::wrapper! {
    /// One entry in the history tree.
    pub struct HistoryRow(ObjectSubclass<imp_history::HistoryRow>);
}

impl RequestRow {
    /// The request id this row stands for, or `None` if it is not a well-formed id.
    pub fn request_id(&self) -> Option<uuid::Uuid> {
        uuid::Uuid::parse_str(&self.id()).ok()
    }
}

impl Default for RequestRow {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl Default for LogRow {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl Default for TimingRow {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl Default for HistoryRow {
    fn default() -> Self {
        glib::Object::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    /// Whether writing a property its own value again still emits `notify` decides whether the
    /// devtools' 250ms refresh is free or a storm — every `notify` re-runs the bindings hanging
    /// off it, and a tooltip binding rewriting `tooltip-text` restarts GTK's hover timer.
    #[test]
    fn writing_a_property_its_own_value_still_notifies() {
        let row = TimingRow::default();
        row.set_description("Turning one stylesheet into rules.");

        let seen = Rc::new(Cell::new(0));
        let counter = seen.clone();
        row.connect_description_notify(move |_| counter.set(counter.get() + 1));

        row.set_description("Turning one stylesheet into rules.");
        assert_eq!(seen.get(), 1, "glib does not compare: an identical write notifies all the same");
    }
}
