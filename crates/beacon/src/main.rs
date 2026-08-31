//! Beacon's entry point.
//!
//! Choosing a frontend is all this crate does. Both host the same browser; what differs
//! is the window around it.

#[cfg(all(feature = "gtk", feature = "egui"))]
compile_error!("Pick one frontend: features `gtk` and `egui` are mutually exclusive.");

#[cfg(not(any(feature = "gtk", feature = "egui")))]
compile_error!("Pick a frontend: enable either the `gtk` or the `egui` feature.");

fn main() {
    #[cfg(feature = "gtk")]
    beacon_gtk::run();

    #[cfg(feature = "egui")]
    beacon_egui::run();
}
