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
    background_css: gtk::CssProvider,
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
        window.set_titlebar(Some(&header));

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .build();
        root.add_css_class("now-playing-background");

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

        // The Now Playing window deliberately does not follow the system theme.
        // Its background is derived from the current cover art, while the text is
        // forced to a light foreground color with enough contrast against it.
        let background_css = gtk::CssProvider::new();
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &background_css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        background_css.load_from_string(
            ".now-playing-background { background-color: #181818; color: #ffffff; padding: 24px; }
             .now-playing-background > label { color: #ffffff; }",
        );

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

        let key_controller = gtk::EventControllerKey::new();
        let window_for_key = window.clone();
        key_controller.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::F11 {
                if window_for_key.is_fullscreen() {
                    window_for_key.unfullscreen();
                } else {
                    window_for_key.fullscreen();
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
            background_css,
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
            self.set_background_from_cover(bytes);
        } else {
            self.artwork.set_paintable(Option::<&gdk::Texture>::None);
            self.artwork.set_visible(false);
            self.artwork_placeholder.set_visible(true);
            self.set_background_color((24, 24, 24));
        }
    }

    fn set_background_from_cover(&self, bytes: &[u8]) {
        let color = image::load_from_memory(bytes)
            .ok()
            .map(|image| dominant_background_color(&image))
            .unwrap_or((24, 24, 24));

        self.set_background_color(color);
    }

    fn set_background_color(&self, (red, green, blue): (u8, u8, u8)) {
        let css = format!(
            ".now-playing-background {{ background-color: rgb({red}, {green}, {blue}); color: #ffffff; padding: 24px; }}
             .now-playing-background > label {{ color: #ffffff; }}
             .now-playing-background .now-playing-title,
             .now-playing-background .now-playing-artist,
             .now-playing-background .now-playing-album,
             .now-playing-background .now-playing-details {{ color: #ffffff; }}"
        );
        self.background_css.load_from_string(&css);
    }

    pub fn present(&self) {
        self.window.present();
    }
}

fn dominant_background_color(image: &image::DynamicImage) -> (u8, u8, u8) {
    let small = image.thumbnail(48, 48).to_rgb8();
    let mut histogram = std::collections::HashMap::<u32, f32>::new();

    for pixel in small.pixels() {
        let [red, green, blue] = pixel.0;
        let max = red.max(green).max(blue) as f32 / 255.0;
        let min = red.min(green).min(blue) as f32 / 255.0;
        let saturation = if max == 0.0 { 0.0 } else { (max - min) / max };

        // Very bright pixels make poor backgrounds and neutral pixels carry
        // little visual information, so give them much less influence.
        let luminance = 0.2126 * red as f32 / 255.0
            + 0.7152 * green as f32 / 255.0
            + 0.0722 * blue as f32 / 255.0;
        if luminance > 0.92 {
            continue;
        }

        let quantize = |value: u8| -> u8 { (value / 32) * 32 + 16 };
        let qr = quantize(red) as u32;
        let qg = quantize(green) as u32;
        let qb = quantize(blue) as u32;
        let key = (qr << 16) | (qg << 8) | qb;
        let weight = 1.0 + saturation * 2.5;
        *histogram.entry(key).or_default() += weight;
    }

    let (key, _) = histogram
        .into_iter()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .unwrap_or(((24u32 << 16) | (24u32 << 8) | 24u32, 1.0));

    let mut red = ((key >> 16) & 0xff) as f32 / 255.0;
    let mut green = ((key >> 8) & 0xff) as f32 / 255.0;
    let mut blue = (key & 0xff) as f32 / 255.0;

    // Convert the chosen color to HSL so we can keep its hue while making it
    // dark enough for white UI text.
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let lightness = (max + min) / 2.0;
    let delta = max - min;

    let saturation = if delta == 0.0 {
        0.0
    } else {
        delta / (1.0 - (2.0 * lightness - 1.0).abs())
    };

    let hue = if delta == 0.0 {
        0.0
    } else if max == red {
        ((green - blue) / delta).rem_euclid(6.0) / 6.0
    } else if max == green {
        (((blue - red) / delta) + 2.0) / 6.0
    } else {
        (((red - green) / delta) + 4.0) / 6.0
    };

    let target_lightness = if saturation < 0.08 { 0.13 } else { 0.17 };
    let target_saturation = saturation.clamp(0.18, 0.78);

    let chroma: f32 = (1.0f32 - (2.0f32 * target_lightness - 1.0f32).abs()) * target_saturation;
    let x = chroma * (1.0 - ((hue * 6.0).rem_euclid(2.0) - 1.0).abs());
    let m = target_lightness - chroma / 2.0;
    let (r1, g1, b1) = match (hue * 6.0).floor() as i32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };

    red = r1 + m;
    green = g1 + m;
    blue = b1 + m;

    // White text has a contrast ratio of at least 4.5:1 when relative
    // luminance is <= ~0.183. The fixed lightness above keeps us comfortably
    // below that threshold while retaining the artwork's hue.
    (
        (red * 255.0).round() as u8,
        (green * 255.0).round() as u8,
        (blue * 255.0).round() as u8,
    )
}
