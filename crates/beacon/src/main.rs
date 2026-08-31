//! Beacon's entry point.
//!
//! Deliberately empty: choosing a frontend is all this crate does. When a second one
//! exists it becomes a cargo feature here, and nothing else in the tree has to care.

fn main() {
    beacon_gtk::run();
}
