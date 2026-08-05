use gtk4::gdk::{Key, Texture};
use gtk4::gdk_pixbuf::Pixbuf;
use gtk4::glib::Propagation;
use gtk4::prelude::*;
use gtk4::{
    ActionBar, Align, Box as GtkBox, Button, ContentFit, EventControllerKey, Label, LinkButton, Orientation, Overlay, Picture,
    ScrolledWindow, Stack, StackTransitionType, Window,
};

// Keep the artwork's 1403x861 aspect so ContentFit::Cover never crops.
const ART_WIDTH: i32 = 660;
const ART_HEIGHT: i32 = 405;

const CREDITS: &[(&str, &[&str])] = &[
    ("Gosub Beacon", &["Gosub Team", "Joshua Thijssen", "SharkTheOne"]),
    ("Networking", &["Gosub Team"]),
    ("HTML5 parser", &["Gosub Team"]),
    ("CSS3 parser", &["Gosub Team"]),
    ("Renderer", &["Gosub Team"]),
    ("Javascript engine", &["Gosub Team"]),
    ("UI", &["Gosub Team"]),
    ("GTK integration", &["Gosub Team"]),
    ("Rust integration", &["Gosub Team"]),
    ("Translations", &["Gosub Team"]),
];

pub struct About;

impl About {
    pub fn create_dialog() -> Window {
        let window = Window::builder().title("About Gosub Beacon").modal(true).resizable(false).build();

        let stack = Stack::new();
        stack.set_transition_type(StackTransitionType::Crossfade);
        stack.add_named(&Self::build_art_page(), Some("about"));
        stack.add_named(&Self::build_credits_page(), Some("credits"));

        let credits_button = Button::with_label("Credits");
        credits_button.connect_clicked({
            let stack = stack.clone();
            move |button| {
                if stack.visible_child_name().as_deref() == Some("about") {
                    stack.set_visible_child_name("credits");
                    button.set_label("About");
                } else {
                    stack.set_visible_child_name("about");
                    button.set_label("Credits");
                }
            }
        });

        let close_button = Button::with_label("Close");
        close_button.connect_clicked({
            let window = window.clone();
            move |_| window.close()
        });

        let bar = ActionBar::new();
        bar.pack_start(&credits_button);
        bar.pack_end(&close_button);

        let content = GtkBox::new(Orientation::Vertical, 0);
        content.append(&stack);
        content.append(&bar);
        window.set_child(Some(&content));
        window.set_default_widget(Some(&close_button));

        let keys = EventControllerKey::new();
        keys.connect_key_pressed({
            let window = window.clone();
            move |_, key, _, _| {
                if key == Key::Escape {
                    window.close();
                    Propagation::Stop
                } else {
                    Propagation::Proceed
                }
            }
        });
        window.add_controller(keys);

        window
    }

    /// A GtkPicture's *natural* size is the paintable's full resolution and a
    /// non-resizable window allocates at natural size, so a size request alone
    /// cannot shrink the dialog — scale the pixbuf itself to the target size.
    fn scaled_art(resource: &str) -> Picture {
        let picture = match Pixbuf::from_resource_at_scale(resource, ART_WIDTH, ART_HEIGHT, true) {
            Ok(pixbuf) => Picture::for_paintable(&Texture::for_pixbuf(&pixbuf)),
            Err(_) => Picture::for_resource(resource),
        };
        picture.set_content_fit(ContentFit::ScaleDown);
        picture.set_size_request(ART_WIDTH, ART_HEIGHT);
        picture
    }

    /// The branded artwork already contains the logo, tagline and an empty
    /// bottom-left region; only the version block is overlaid as real widgets.
    fn build_art_page() -> Overlay {
        let picture = Self::scaled_art("/io/gosub/beacon/assets/about.png");

        let info = GtkBox::new(Orientation::Vertical, 2);
        info.set_halign(Align::Start);
        info.set_valign(Align::End);
        info.set_margin_start(42);
        info.set_margin_bottom(28);
        for line in [
            concat!("Gosub Beacon ", env!("CARGO_PKG_VERSION")),
            "Powered by the Gosub Engine",
            "Copyright © 2026 Gosub Project",
            "All rights reserved.",
        ] {
            let label = Label::new(Some(line));
            label.set_halign(Align::Start);
            label.add_css_class("about-info-line");
            info.append(&label);
        }
        let link = LinkButton::with_label("https://gosub.io", "https://gosub.io");
        link.set_halign(Align::Start);
        link.add_css_class("about-info-link");
        info.append(&link);

        let overlay = Overlay::new();
        overlay.set_child(Some(&picture));
        overlay.add_overlay(&info);
        overlay
    }

    /// Credits artwork keeps the whole left half white; the scrolling credits
    /// column is overlaid there.
    fn build_credits_page() -> Overlay {
        let picture = Self::scaled_art("/io/gosub/beacon/assets/about-credits.png");

        let list = GtkBox::new(Orientation::Vertical, 4);
        list.set_margin_end(12);
        for (section, names) in CREDITS {
            let heading = Label::new(Some(section));
            heading.set_halign(Align::Start);
            heading.add_css_class("about-credits-heading");
            heading.set_margin_top(8);
            list.append(&heading);
            for name in *names {
                let label = Label::new(Some(name));
                label.set_halign(Align::Start);
                label.set_margin_start(12);
                label.add_css_class("about-credits-name");
                list.append(&label);
            }
        }

        let scroller = ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .build();
        scroller.add_css_class("about-credits-scroller");
        // Confine the column to the artwork's white left half.
        scroller.set_size_request(ART_WIDTH * 2 / 5, -1);
        scroller.set_halign(Align::Start);
        scroller.set_margin_start(28);
        scroller.set_margin_top(20);
        scroller.set_margin_bottom(20);

        let overlay = Overlay::new();
        overlay.set_child(Some(&picture));
        overlay.add_overlay(&scroller);
        overlay
    }
}
