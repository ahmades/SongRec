use crate::core::thread_messages::SongRecognizedMessage;
use adw::prelude::*;
use gettextrs::gettext;
use std::cell::Cell;
use std::rc::Rc;

pub struct ArtworkWindow {
    window: gtk::Window,
    artwork: gtk::Picture,
    artwork_placeholder: gtk::Label,
    title_label: gtk::Label,
    artist_label: gtk::Label,
    album_label: gtk::Label,
    details_label: gtk::Label,
}

impl ArtworkWindow {
    pub fn new(application: &adw::Application) -> Self {
        let window = gtk::Window::builder()
            .application(application)
            .title("SongRec")
            .default_width(720)
            .default_height(820)
            .resizable(true)
            .build();

        window.set_hide_on_close(true);

        let header = gtk::HeaderBar::new();
        header.set_title_widget(Some(&gtk::Label::new(Some(&gettext("Now playing")))));

        let fullscreen_button = gtk::Button::builder()
            .icon_name("view-fullscreen-symbolic")
            .tooltip_text(gettext("Toggle fullscreen"))
            .build();
        header.pack_end(&fullscreen_button);
        window.set_titlebar(Some(&header));

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(24)
            .margin_end(24)
            .build();

        let artwork_frame = gtk::AspectFrame::builder()
            .ratio(1.0)
            .obey_child(false)
            .hexpand(true)
            .vexpand(true)
            .build();

        let artwork = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .can_shrink(true)
            .hexpand(true)
            .vexpand(true)
            .build();

        let artwork_placeholder = gtk::Label::builder()
            .label(&gettext("No artwork available"))
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        let artwork_overlay = gtk::Overlay::new();
        artwork_overlay.set_child(Some(&artwork));
        artwork_overlay.add_overlay(&artwork_placeholder);
        artwork_frame.set_child(Some(&artwork_overlay));

        let title_label = gtk::Label::builder()
            .halign(gtk::Align::Center)
            .wrap(true)
            .css_classes(["now-playing-title"])
            .build();
        let artist_label = gtk::Label::builder()
            .halign(gtk::Align::Center)
            .wrap(true)
            .css_classes(["now-playing-artist"])
            .build();
        let album_label = gtk::Label::builder()
            .halign(gtk::Align::Center)
            .wrap(true)
            .css_classes(["now-playing-album"])
            .build();
        let details_label = gtk::Label::builder()
            .halign(gtk::Align::Center)
            .wrap(true)
            .css_classes(["now-playing-details"])
            .build();

        root.append(&artwork_frame);
        root.append(&title_label);
        root.append(&artist_label);
        root.append(&album_label);
        root.append(&details_label);
        window.set_child(Some(&root));

        // Use a dedicated CSS provider for the Now Playing labels. Their font sizes
        // are recalculated from the actual window size, so resizing and fullscreen
        // both use exactly the same sizing path.
        let text_css = gtk::CssProvider::new();
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &text_css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        let last_size = Rc::new(Cell::new((0, 0)));
        let text_css_for_resize = text_css.clone();
        let last_size_for_resize = last_size.clone();
        window.add_tick_callback(move |widget, _| {
            let size = (widget.width(), widget.height());
            if size != last_size_for_resize.get() {
                last_size_for_resize.set(size);

                let width_scale = size.0 as f64 / 720.0;
                let height_scale = size.1 as f64 / 820.0;
                let scale = width_scale.min(height_scale).clamp(0.60, 2.25);

                let title_size = (32.0 * scale).round() as i32;
                let artist_size = (24.0 * scale).round() as i32;
                let album_size = (18.0 * scale).round() as i32;
                let details_size = (18.0 * scale).round() as i32;

                let css = format!(
                    ".now-playing-title {{ font-size: {title_size}px; }}
                     .now-playing-artist {{ font-size: {artist_size}px; }}
                     .now-playing-album {{ font-size: {album_size}px; }}
                     .now-playing-details {{ font-size: {details_size}px; }}"
                );
                text_css_for_resize.load_from_string(&css);
            }

            glib::ControlFlow::Continue
        });

        let window_for_button = window.clone();
        fullscreen_button.connect_clicked(move |_| {
            if window_for_button.is_fullscreen() {
                window_for_button.unfullscreen();
            } else {
                window_for_button.fullscreen();
            }
        });

        let key_controller = gtk::EventControllerKey::new();
        let window_for_key = window.clone();
        key_controller.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                if window_for_key.is_fullscreen() {
                    window_for_key.unfullscreen();
                }
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        window.add_controller(key_controller);

        Self {
            window,
            artwork,
            artwork_placeholder,
            title_label,
            artist_label,
            album_label,
            details_label,
        }
    }

    pub fn update(&self, message: &SongRecognizedMessage) {
        self.title_label.set_label(&message.song_name);
        self.artist_label.set_label(&message.artist_name);
        self.album_label.set_label(
            message
                .album_name
                .as_deref()
                .filter(|value| !value.is_empty())
                .unwrap_or(""),
        );
        self.details_label.set_label(
            message
                .release_year
                .as_deref()
                .filter(|value| !value.is_empty())
                .unwrap_or(""),
        );

        if let Some(bytes) = message.cover_image.as_ref()
            && let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from(bytes))
        {
            self.artwork.set_paintable(Some(&texture));
            self.artwork.set_visible(true);
            self.artwork_placeholder.set_visible(false);
        } else {
            self.artwork.set_paintable(Option::<&gdk::Texture>::None);
            self.artwork.set_visible(false);
            self.artwork_placeholder.set_visible(true);
        }
    }

    pub fn present(&self) {
        self.window.present();
    }
}
