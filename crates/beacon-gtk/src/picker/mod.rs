//! The pickers the engine asks for when a form control that opens one is activated.
//!
//! The chrome draws them, not the engine: a `PickerRequested` event arrives with the
//! control's border box and its current value, and the picker answers with `PickerChanged`
//! as the choice moves — the control previews live — and `PickerClosed` when it is done.
//! A cancel is a `PickerChanged` back to the value the control started with.
//!
//! Ported from the Mac shell's `swift/Sources/BeaconMac/ColorPicker/`, to the same Figma
//! design, so the two chromes look and behave like one product. Colour is done; the date,
//! time, month and week pickers still fall through to the caller's log.

pub mod color;
pub mod color_picker;
pub mod eyedropper;
pub mod shell;
pub mod widgets;

use beacon_core::event::PickerKind;
use gtk4::prelude::*;
use gtk4::Window;
use std::rc::Rc;

use color::CssColor;
use color_picker::ColorPicker;

/// A picker that is up. Held by the window so a second request closes the first, and so the
/// one being replaced does not answer for a control that has moved on.
pub struct PickerHandle {
    picker: Rc<ColorPicker>,
}

impl PickerHandle {
    pub fn close(&self) {
        self.picker.detach();
    }
}

/// Open the picker `kind` asks for over `parent`.
///
/// `on_change` is called with every intermediate value, `on_finish` once, with the value to
/// keep — which for a cancel is the one the control came in with. Returns `None` for a kind
/// that has no picker yet, so the caller can say so.
pub fn open(
    parent: &impl IsA<Window>,
    kind: PickerKind,
    value: &str,
    on_change: impl Fn(String) + 'static,
    on_finish: impl Fn(String) + 'static,
) -> Option<PickerHandle> {
    match kind {
        PickerKind::Color => {
            // An unparsable value is black, which is what the engine sanitises an empty or
            // broken `<input type=color>` to.
            let initial = CssColor::parse(value).unwrap_or(CssColor::BLACK);
            // A form's colour input holds six digits, so the alpha strip is drawn — the
            // design has it — but not offered.
            let picker = ColorPicker::new(Some(parent), initial, false);
            picker.connect_change(move |color| on_change(color.hex()));
            picker.connect_finish(move |chosen| {
                on_finish(chosen.unwrap_or(initial).hex());
            });
            picker.present();
            Some(PickerHandle { picker })
        }
        _ => None,
    }
}
