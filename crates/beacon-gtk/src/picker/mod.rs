//! The pickers the engine asks for when a form control that opens one is activated.
//!
//! The chrome draws them, not the engine: a `PickerRequested` event arrives with the
//! control's border box and its current value, and the picker answers with `PickerChanged`
//! as the choice moves — the control previews live — and `PickerClosed` when it is done.
//! A cancel is a `PickerChanged` back to the value the control started with.
//!
//! Ported from the Mac shell's `swift/Sources/BeaconMac/ColorPicker/`, to the same Figma
//! design, so the two chromes look and behave like one product.

pub mod color;
pub mod color_picker;
pub mod date_page;
pub mod datetime;
pub mod datetime_picker;
pub mod eyedropper;
pub mod locale;
pub mod month_year_page;
pub mod quick_select;
pub mod shell;
pub mod stepper;
pub mod time_page;
pub mod widgets;

use beacon_core::event::PickerKind;
use gtk4::prelude::*;
use gtk4::Window;
use std::rc::Rc;

use color::CssColor;
use color_picker::ColorPicker;
use datetime_picker::DateTimePicker;

/// A picker that is up. Held by the window so a second request closes the first, and so the
/// one being replaced does not answer for a control that has moved on.
pub enum PickerHandle {
    Color(Rc<ColorPicker>),
    DateTime(Rc<DateTimePicker>),
}

impl PickerHandle {
    pub fn close(&self) {
        match self {
            Self::Color(picker) => picker.detach(),
            Self::DateTime(picker) => picker.detach(),
        }
    }
}

/// Open the picker `kind` asks for over `parent`.
///
/// `on_change` is called with every intermediate value, `on_finish` once, with the value to
/// keep — which for a cancel is the one the control came in with.
#[allow(clippy::too_many_arguments)]
pub fn open(
    parent: &impl IsA<Window>,
    kind: PickerKind,
    value: &str,
    min: Option<&str>,
    max: Option<&str>,
    step: Option<&str>,
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
            Some(PickerHandle::Color(picker))
        }
        // Every other kind is a date or a time, and they share one picker.
        _ => {
            let picker = DateTimePicker::new(Some(parent), kind, value, min, max, step);
            let original = value.to_string();
            picker.connect_change(on_change);
            picker.connect_finish(move |chosen| on_finish(chosen.unwrap_or_else(|| original.clone())));
            picker.present();
            Some(PickerHandle::DateTime(picker))
        }
    }
}
