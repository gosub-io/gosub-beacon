//! Follow the desktop's light/dark preference.
//!
//! Beacon is plain GTK4 with no libadwaita, and GTK does not itself watch the desktop's
//! colour-scheme preference — it only exposes `gtk-application-prefer-dark-theme` for the
//! application to set. So read the preference from the XDG desktop portal and mirror it into
//! that property, then keep mirroring it as the user changes it.
//!
//! The manual toggle (Ctrl+D / the toolbar button) writes the same property, so it acts as an
//! override that lasts until the desktop preference next changes.

use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::Settings;
use log::{info, warn};

const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const PORTAL_IFACE: &str = "org.freedesktop.portal.Settings";
const NAMESPACE: &str = "org.freedesktop.appearance";
const KEY: &str = "color-scheme";

/// Portal values: 0 = no preference, 1 = prefer dark, 2 = prefer light.
fn apply(scheme: u32) {
    let Some(settings) = Settings::default() else { return };
    // "No preference" is treated as light: that is what the property's `false` means, and it
    // matches what every GTK app without an explicit preference already renders.
    let dark = scheme == 1;
    if settings.property::<bool>("gtk-application-prefer-dark-theme") != dark {
        info!(target: "gtk", "desktop colour-scheme = {scheme}; prefer-dark -> {dark}");
        settings.set_property("gtk-application-prefer-dark-theme", dark);
    }
}

/// Pull a `u32` out of the portal's doubly-wrapped reply / signal payload.
fn scheme_from(value: &glib::Variant) -> Option<u32> {
    // ReadOne answers `(v)`; the inner variant may itself wrap another variant.
    let inner = value.child_value(0);
    inner.get::<u32>().or_else(|| inner.as_variant().and_then(|v| v.get::<u32>()))
}

/// Mirror the desktop colour-scheme into GTK, now and whenever it changes.
///
/// A missing portal is not an error: the chrome simply keeps whatever theme GTK picked, and
/// the manual toggle still works.
pub fn follow_desktop_color_scheme() {
    let proxy = match gio::DBusProxy::for_bus_sync(
        gio::BusType::Session,
        gio::DBusProxyFlags::NONE,
        None,
        PORTAL_BUS,
        PORTAL_PATH,
        PORTAL_IFACE,
        gio::Cancellable::NONE,
    ) {
        Ok(proxy) => proxy,
        Err(e) => {
            warn!(target: "gtk", "no desktop portal ({e}); chrome will not follow the OS theme");
            return;
        }
    };

    // Initial value. `ReadOne` is the current call; older portals only have `Read`, which
    // returns the value wrapped one level deeper.
    let args = (NAMESPACE, KEY).to_variant();
    let reply = proxy
        .call_sync("ReadOne", Some(&args), gio::DBusCallFlags::NONE, 1000, gio::Cancellable::NONE)
        .or_else(|_| proxy.call_sync("Read", Some(&args), gio::DBusCallFlags::NONE, 1000, gio::Cancellable::NONE));
    match reply {
        Ok(v) => {
            if let Some(scheme) = scheme_from(&v) {
                apply(scheme);
            }
        }
        Err(e) => warn!(target: "gtk", "portal did not answer colour-scheme ({e})"),
    }

    // Live updates: SettingChanged carries (namespace, key, value).
    proxy.connect_local("g-signal", false, move |args| {
        let signal = args[2].get::<String>().unwrap_or_default();
        if signal != "SettingChanged" {
            return None;
        }
        let Ok(params) = args[3].get::<glib::Variant>() else { return None };
        if params.child_value(0).str() != Some(NAMESPACE) || params.child_value(1).str() != Some(KEY) {
            return None;
        }
        let value = params.child_value(2);
        if let Some(scheme) = value.get::<u32>().or_else(|| value.as_variant().and_then(|v| v.get::<u32>())) {
            apply(scheme);
        }
        None
    });

    // The proxy must outlive this call or the signal subscription dies with it.
    std::mem::forget(proxy);
}
