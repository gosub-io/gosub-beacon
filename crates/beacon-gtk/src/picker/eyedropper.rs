//! Sampling a colour from the screen, through the desktop portal.
//!
//! A browser cannot read the screen itself — under Wayland nothing can — so the eyedropper
//! is `org.freedesktop.portal.Screenshot.PickColor`, which is what the toolkit's own colour
//! chooser uses. The compositor draws the loupe, the user clicks a pixel, and the portal
//! answers with sRGB in 0..=1. It works the same inside a flatpak, where reading the screen
//! directly would be refused outright.

use gtk4::gio;
use gtk4::glib::{self, VariantDict, VariantTy};
use gtk4::prelude::*;
use log::warn;
use std::cell::RefCell;
use std::rc::Rc;

use super::color::CssColor;

const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SCREENSHOT_IFACE: &str = "org.freedesktop.portal.Screenshot";
const REQUEST_IFACE: &str = "org.freedesktop.portal.Request";

/// Whether the desktop offers a colour picker. `PickColor` arrived in version 2 of the
/// interface, so a portal older than that — or no portal at all — means no eyedropper.
/// Asked once, when a picker is built, rather than on the click that would fail.
pub fn is_available() -> bool {
    let Ok(connection) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else {
        return false;
    };
    let args = (SCREENSHOT_IFACE, "version").to_variant();
    let reply = connection.call_sync(
        Some(PORTAL_BUS),
        PORTAL_PATH,
        "org.freedesktop.DBus.Properties",
        "Get",
        Some(&args),
        Some(VariantTy::new("(v)").unwrap()),
        gio::DBusCallFlags::NONE,
        1000,
        gio::Cancellable::NONE,
    );
    match reply {
        Ok(reply) => reply.child_value(0).as_variant().and_then(|v| v.get::<u32>()).unwrap_or(0) >= 2,
        Err(error) => {
            warn!(target: "gtk", "no screenshot portal ({error}); the eyedropper is not offered");
            false
        }
    }
}

/// Ask the compositor for a pixel. `on_picked` runs once, on the main thread, if and when
/// the user clicks one; a cancelled pick calls nothing.
pub fn pick(on_picked: impl Fn(CssColor) + 'static) {
    let Ok(connection) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else {
        warn!(target: "gtk", "no session bus; cannot sample a colour from the screen");
        return;
    };

    // The portal answers on a `Request` object whose path it derives from our bus name and
    // the token we hand it, so it can be subscribed to *before* the call goes out. Waiting
    // for the call to return the path first would be a race the portal is allowed to win.
    let Some(unique) = connection.unique_name() else {
        warn!(target: "gtk", "session bus has no unique name; cannot sample a colour");
        return;
    };
    let sender = unique.trim_start_matches(':').replace('.', "_");
    let token = format!("beacon_{}", glib::random_int());
    let request_path = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");

    // The subscription is an RAII guard: dropping it unsubscribes, and it also keeps the
    // connection alive for as long as the pick is outstanding.
    let subscription: Rc<RefCell<Option<gio::SignalSubscription>>> = Rc::new(RefCell::new(None));
    let handle = connection.subscribe_to_signal(
        Some(PORTAL_BUS),
        Some(REQUEST_IFACE),
        Some("Response"),
        Some(&request_path),
        None,
        gio::DBusSignalFlags::NONE,
        {
            let subscription = subscription.clone();
            move |signal| {
                // One answer per request: without dropping the subscription, a second pick
                // would run this handler twice, a third three times. The drop is deferred to
                // the next turn of the loop rather than done here, because the subscription
                // owns *this* closure -- glib frees a subscriber's data as soon as it is
                // unsubscribed from the thread that owns the context, which would pull the
                // ground out from under the code still running below.
                if let Some(handle) = subscription.borrow_mut().take() {
                    glib::idle_add_local_once(move || drop(handle));
                }
                let response: u32 = signal.parameters.child_value(0).get().unwrap_or(2);
                match response {
                    // 1 is the user pressing Escape on the loupe, which is not a failure.
                    1 => return,
                    0 => {}
                    other => {
                        warn!(target: "gtk", "the screen colour pick failed (response {other})");
                        return;
                    }
                }
                let results = signal.parameters.child_value(1);
                let color = VariantDict::new(Some(&results))
                    .lookup_value("color", Some(VariantTy::new("(ddd)").unwrap()))
                    .and_then(|value| value.get::<(f64, f64, f64)>());
                match color {
                    Some((r, g, b)) => on_picked(CssColor::new(r, g, b, 1.0)),
                    None => warn!(target: "gtk", "the portal answered a colour pick without a colour"),
                }
            }
        },
    );
    *subscription.borrow_mut() = Some(handle);

    // An empty parent window: the loupe is the compositor's own overlay and is not parented
    // to anything, and a real handle would mean pulling in the Wayland and X11 gdk backends
    // for an async round-trip that buys nothing here.
    let options = VariantDict::new(None);
    options.insert("handle_token", token);
    let args = glib::Variant::tuple_from_iter([("").to_variant(), options.end()]);
    connection.call(
        Some(PORTAL_BUS),
        PORTAL_PATH,
        SCREENSHOT_IFACE,
        "PickColor",
        Some(&args),
        Some(VariantTy::new("(o)").unwrap()),
        gio::DBusCallFlags::NONE,
        5000,
        gio::Cancellable::NONE,
        {
            let subscription = subscription.clone();
            move |result| {
                if let Err(error) = result {
                    warn!(target: "gtk", "the portal refused a colour pick ({error})");
                    drop(subscription.borrow_mut().take());
                }
            }
        },
    );
}
