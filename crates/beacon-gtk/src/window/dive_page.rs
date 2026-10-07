//! `gosub://dive` as GTK shows it: a drawing area the game paints into at
//! the frame clock's pace, the space bar (or a click) to flap.
//!
//! The game itself is `beacon_core::dive`, shared with every other shell;
//! this file only moves pixels onto a Cairo surface and keys into the game.

use beacon_core::dive::paint::PixelOrder;
use beacon_core::dive::{Dive, FRAME};
use gtk4::prelude::*;
use gtk4::{cairo, gdk, glib, DrawingArea, Widget};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Build the game view for one tab.
pub fn build() -> Widget {
    let area = DrawingArea::new();
    area.set_hexpand(true);
    area.set_vexpand(true);
    area.set_focusable(true);
    area.set_can_focus(true);
    area.add_css_class("dive-page");

    let dive: Rc<RefCell<Option<Dive>>> = Rc::new(RefCell::new(None));

    // The shell's colour scheme, as the toolbar's moon button sets it; followed
    // live, so toggling it mid-game repaints the water.
    let prefers_dark = || gtk4::Settings::default().is_some_and(|s| s.is_gtk_application_prefer_dark_theme());
    let dark = Rc::new(Cell::new(prefers_dark()));
    if let Some(settings) = gtk4::Settings::default() {
        let dark_dive = Rc::clone(&dive);
        let dark_flag = Rc::clone(&dark);
        let dark_area = area.clone();
        settings.connect_gtk_application_prefer_dark_theme_notify(move |settings| {
            let now = settings.is_gtk_application_prefer_dark_theme();
            dark_flag.set(now);
            if let Some(game) = dark_dive.borrow_mut().as_mut() {
                game.set_dark(now);
            }
            dark_area.queue_draw();
        });
    }

    // Paint: the game paints in Cairo's byte order already (BGRA, opaque), so
    // the frame goes onto a surface as it is, placed at the device scale so a
    // logical pixel of the game is one of the widget's.
    let draw_dive = Rc::clone(&dive);
    area.set_draw_func(move |area, cr, width, height| {
        let scale = area.scale_factor().max(1);
        let (w, h) = ((width * scale).max(1) as u32, (height * scale).max(1) as u32);
        // A frame is painted on the CPU and copied to the surface every tick:
        // past a few million pixels (a maximised window on a wide display)
        // that no longer fits in a frame, so the game is painted at half
        // resolution and Cairo doubles it. Pixel art doubles cleanly.
        let coarse = if w * h > 3_000_000 { 2 } else { 1 };
        let (pw, ph) = ((w / coarse).max(1), (h / coarse).max(1));
        let painted_scale = scale as f32 / coarse as f32;
        let mut slot = draw_dive.borrow_mut();
        let game = slot.get_or_insert_with(|| {
            let mut game = Dive::new(pw, ph, painted_scale, PixelOrder::Bgra);
            game.set_dark(dark.get());
            game
        });
        game.resize(pw, ph, painted_scale);
        let stride = (pw * 4) as i32;
        if let Ok(surface) = cairo::ImageSurface::create_for_data(game.frame().to_vec(), cairo::Format::Rgb24, pw as i32, ph as i32, stride)
        {
            surface.set_device_scale(painted_scale as f64, painted_scale as f64);
            // Nearest, so doubled pixels stay crisp.
            let pattern = cairo::SurfacePattern::create(&surface);
            pattern.set_filter(cairo::Filter::Nearest);
            let _ = cr.set_source(&pattern);
            let _ = cr.paint();
        }
    });

    // Tick at the frame clock, in fixed steps of one game frame, so the game
    // runs at its own speed whatever the monitor's rate. Only while on screen:
    // a game in a background tab neither plays nor paints, and does not sink
    // while nobody watches.
    let tick_dive = Rc::clone(&dive);
    let last = Rc::new(Cell::new(0i64));
    let owed = Rc::new(Cell::new(0i64));
    let owed_set = Rc::clone(&owed);
    area.add_tick_callback(move |area, clock| {
        if !area.is_mapped() {
            last.set(0);
            return glib::ControlFlow::Continue;
        }
        let now = clock.frame_time();
        let previous = last.replace(now);
        if previous == 0 {
            return glib::ControlFlow::Continue;
        }
        // Time owed to the game, carried from tick to tick: dropping the
        // remainder would round a 16.6 ms monitor frame down to no step at all
        // every so often, and the game would judder. At most a handful of
        // steps at once: a stall is not a fast-forward.
        let frame = FRAME.as_micros() as i64;
        let owed = (owed.get() + (now - previous).max(0)).min(frame * 4);
        let steps = owed / frame;
        owed_set.set(owed - steps * frame);
        if steps > 0 {
            if let Some(game) = tick_dive.borrow_mut().as_mut() {
                for _ in 0..steps {
                    game.tick();
                }
            }
            area.queue_draw();
        }
        glib::ControlFlow::Continue
    });

    // Space, Up or Enter flaps; Ctrl, Alt and Super chords stay the shell's.
    let keys = gtk4::EventControllerKey::new();
    let key_dive = Rc::clone(&dive);
    keys.connect_key_pressed(move |_c, keyval, _code, state| {
        if state.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK | gdk::ModifierType::SUPER_MASK) {
            return glib::Propagation::Proceed;
        }
        match keyval {
            gdk::Key::space | gdk::Key::Up | gdk::Key::Return | gdk::Key::KP_Enter => {
                if let Some(game) = key_dive.borrow_mut().as_mut() {
                    game.flap();
                }
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    });
    area.add_controller(keys);

    // A click flaps too, and takes the keyboard focus so space works after it.
    let click = gtk4::GestureClick::new();
    click.set_button(gdk::BUTTON_PRIMARY);
    let click_dive = Rc::clone(&dive);
    click.connect_pressed(move |g, _n, _x, _y| {
        if let Some(widget) = g.widget() {
            widget.grab_focus();
        }
        if let Some(game) = click_dive.borrow_mut().as_mut() {
            game.flap();
        }
    });
    area.add_controller(click);

    // Focus on arrival, so the first space press plays.
    area.connect_map(|area| {
        area.grab_focus();
    });

    area.upcast::<Widget>()
}
