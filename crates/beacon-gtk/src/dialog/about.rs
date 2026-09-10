use gtk4::gdk::{Key, Texture};
use gtk4::gdk_pixbuf::Pixbuf;
use gtk4::glib::Propagation;
use gtk4::prelude::*;
use gtk4::{
    ActionBar, Align, Box as GtkBox, Button, ContentFit, EventControllerKey, Label, LinkButton, Orientation, Overlay, Picture,
    ScrolledWindow, Stack, StackTransitionType, Window,
};

// Keep the artwork's 1672x941 aspect so ContentFit::Cover never crops.
const ART_WIDTH: i32 = 660;
// 16:9, the aspect of the artwork in resources/. Both pages share it, so the dialog does
// not change size when you flip between them.
const ART_HEIGHT: i32 = 371;

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
        bar.set_center_widget(Some(&Self::build_info_bar()));
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

    /// Only the picture. These finals are a finished composition -- wordmark, tagline and
    /// submarine reach the bottom of the panel -- so the version block cannot sit on them
    /// without landing on the artwork; it lives in the action bar instead.
    fn build_art_page() -> Overlay {
        let overlay = Overlay::new();
        overlay.set_child(Some(&Self::scaled_art("/io/gosub/beacon/assets/about.png")));
        overlay
    }

    /// The version block, for the action bar's centre.
    fn build_info_bar() -> GtkBox {
        let info = GtkBox::new(Orientation::Horizontal, 8);
        let label = Label::new(Some(concat!(
            "Gosub Beacon ",
            env!("CARGO_PKG_VERSION"),
            " · Powered by the Gosub Engine · © 2026 Gosub Project"
        )));
        label.add_css_class("about-info-line");
        info.append(&label);
        let link = LinkButton::with_label("https://gosub.io", "https://gosub.io");
        link.add_css_class("about-info-link");
        info.append(&link);
        info
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

        // Spanning the right of the picture: the names sit over the open water, clear of the
        // gradient, and the scrollbar lands at the edge of the artwork rather than down the
        // middle of it.
        scroller.set_vexpand(true);
        scroller.set_hexpand(true);
        scroller.set_halign(Align::Fill);
        scroller.set_margin_start(ART_WIDTH * 56 / 100);
        scroller.set_margin_end(20);
        scroller.set_margin_top(22);
        scroller.set_margin_bottom(22);

        let overlay = Overlay::new();
        overlay.set_child(Some(&picture));
        overlay.add_overlay(&scroller);
        overlay
    }
}
