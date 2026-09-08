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
