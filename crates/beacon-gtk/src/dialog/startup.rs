//! The profile opens before any window. When it cannot, a dialog says why and offers to try
//! again; when the keychain is slow, a small window says Beacon is waiting for it. Both run
//! a main loop of their own, so they work before the application has a window.

use beacon_core::startup::StartupError;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Expander, Label, Orientation, Spinner, Window};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::TryRecvError;

/// Set once the profile is open. Until then a launch handed over from elsewhere opens no
/// window: its engine would try to take the profile a second time.
static OPEN: AtomicBool = AtomicBool::new(false);

pub fn profile_open() -> bool {
    OPEN.load(Ordering::Acquire)
}

/// Lock and unlock the profile, saying so when the keychain is slow and why when it fails.
/// False when the user gave up.
pub fn open_profile() -> bool {
    loop {
        match wait_for(beacon_core::startup::open_profile()) {
            Ok(()) => {
                OPEN.store(true, Ordering::Release);
                return true;
            }
            Err(e) => {
                eprintln!("gosub-beacon: {e:#}");
                if !ask(&StartupError::from_error(&e), true) {
                    return false;
                }
            }
        }
    }
}

/// Say why Beacon cannot start, before GTK is otherwise up, and end the process.
pub fn fail(e: &anyhow::Error) -> ! {
    eprintln!("gosub-beacon: {e:#}");
    if gtk4::init().is_ok() {
        ask(&StartupError::from_error(e), false);
    }
    std::process::exit(1);
}

/// Run the main loop until `rx` answers, showing the waiting window once that takes longer
/// than [`beacon_core::startup::PATIENCE`].
fn wait_for(rx: std::sync::mpsc::Receiver<anyhow::Result<()>>) -> anyhow::Result<()> {
    let main_loop = glib::MainLoop::new(None, false);
    let result = Rc::new(Cell::new(None));
    let started = std::time::Instant::now();
    let mut waiting: Option<Window> = None;
    glib::timeout_add_local(std::time::Duration::from_millis(50), {
        let main_loop = main_loop.clone();
        let result = result.clone();
        move || match rx.try_recv() {
            Err(TryRecvError::Empty) => {
                if waiting.is_none() && started.elapsed() >= beacon_core::startup::PATIENCE {
                    waiting = Some(waiting_window());
                }
                glib::ControlFlow::Continue
            }
            done => {
                result.set(Some(
                    done.unwrap_or_else(|_| Err(anyhow::anyhow!("the profile thread ended without an answer"))),
                ));
                if let Some(window) = waiting.take() {
                    window.destroy();
                }
                main_loop.quit();
                glib::ControlFlow::Break
            }
        }
    });
    main_loop.run();
    result.take().expect("the loop ends with a result")
}

/// "Waiting for the keychain", with a way out: the call it waits on cannot be cut short.
fn waiting_window() -> Window {
    let window = Window::builder().title("Gosub Beacon").resizable(false).build();
    let content = GtkBox::new(Orientation::Vertical, 12);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(24);
    content.set_margin_end(24);

    let row = GtkBox::new(Orientation::Horizontal, 12);
    let spinner = Spinner::new();
    spinner.start();
    row.append(&spinner);
    row.append(&heading("Waiting for the keychain..."));
    content.append(&row);
    let about = text(
        "Beacon keeps each profile's key in the keychain. A locked keyring waits for its password; \
         a keychain service that does not answer gives up after two minutes.",
    );
    about.set_selectable(false);
    content.append(&about);

    let quit = Button::with_label("Quit");
    quit.set_halign(Align::End);
    quit.connect_clicked(|_| std::process::exit(1));
    content.append(&quit);

    window.set_child(Some(&content));
    window.connect_close_request(|_| std::process::exit(1));
    GtkWindowExt::set_focus(&window, Some(&quit));
    window.present();
    window
}

/// Show `error` until the user answers. True for "Try Again", which is only offered when
/// `retry` is.
fn ask(error: &StartupError, retry: bool) -> bool {
    let window = Window::builder().title("Gosub Beacon").resizable(false).default_width(480).build();
    let content = GtkBox::new(Orientation::Vertical, 12);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(24);
    content.set_margin_end(24);

    content.append(&heading(&error.reason));
    if let Some(hint) = &error.hint {
        // Selectable: a hint may be a command to copy into a terminal.
        content.append(&text(hint));
    }
    let details = Expander::new(Some("Details"));
    details.set_child(Some(&text(&error.detail)));
    // With no hint, the detail is all there is to go on.
    details.set_expanded(error.hint.is_none());
    content.append(&details);

    let buttons = GtkBox::new(Orientation::Horizontal, 8);
    buttons.set_halign(Align::End);
    let quit = Button::with_label("Quit");
    buttons.append(&quit);
    let again = retry.then(|| {
        let again = Button::with_label("Try Again");
        again.add_css_class("suggested-action");
        buttons.append(&again);
        again
    });
    content.append(&buttons);
    window.set_child(Some(&content));

    let main_loop = glib::MainLoop::new(None, false);
    let answer = Rc::new(Cell::new(false));
    let answered = {
        let main_loop = main_loop.clone();
        let answer = answer.clone();
        move |retry: bool| {
            answer.set(retry);
            main_loop.quit();
        }
    };
    quit.connect_clicked({
        let answered = answered.clone();
        move |_| answered(false)
    });
    if let Some(again) = &again {
        let answered = answered.clone();
        again.connect_clicked(move |_| answered(true));
        window.set_default_widget(Some(again));
    }
    window.connect_close_request({
        let answered = answered.clone();
        move |_| {
            answered(false);
            glib::Propagation::Proceed
        }
    });
    // Focus on a button before the window shows: the first focusable widget would be the
    // selectable text, which takes focus by selecting all of it.
    GtkWindowExt::set_focus(&window, Some(again.as_ref().unwrap_or(&quit)));
    window.present();
    main_loop.run();
    window.destroy();
    answer.get()
}

fn heading(s: &str) -> Label {
    let label = Label::new(None);
    label.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(s)));
    label.set_wrap(true);
    label.set_max_width_chars(60);
    label.set_xalign(0.0);
    label
}

fn text(s: &str) -> Label {
    let label = Label::new(Some(s));
    label.set_wrap(true);
    label.set_max_width_chars(60);
    label.set_xalign(0.0);
    label.set_selectable(true);
    label
}
