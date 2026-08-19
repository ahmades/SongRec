//! Displays the floating Now Playing UI for the current song.
//!
//! This window keeps a custom dark presentation based on the album art, applies
//! a readable high-contrast foreground, and resizes its typography according to
//! the actual window dimensions so fullscreen and regular resizing share the same
//! sizing logic.

use crate::core::preferences::{Preferences, PreferencesInterface};
use crate::core::thread_messages::{GUIMessage, SongRecognizedMessage};
use adw::prelude::*;
use gettextrs::gettext;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

const WINDOW_WIDTH: i32 = 720;
const WINDOW_HEIGHT: i32 = 820;
const ROOT_SPACING: i32 = 18;
const INFO_BOX_SPACING: i32 = 2;
const BACKGROUND_PADDING_PX: i32 = 96;
const BASE_SCALE_WIDTH: f64 = 720.0;
const BASE_SCALE_HEIGHT: f64 = 820.0;
const MIN_FONT_SCALE: f64 = 0.60;
const MAX_FONT_SCALE: f64 = 2.25;
const TITLE_BASE_FONT_SIZE: f64 = 32.0;
const ARTIST_BASE_FONT_SIZE: f64 = 24.0;
const ALBUM_BASE_FONT_SIZE: f64 = 18.0;
const DETAILS_BASE_FONT_SIZE: f64 = 18.0;
const TITLE_CSS_CLASS: &str = "now-playing-title";
const ARTIST_CSS_CLASS: &str = "now-playing-artist";
const ALBUM_CSS_CLASS: &str = "now-playing-album";
const DETAILS_CSS_CLASS: &str = "now-playing-details";
const BACKGROUND_CSS_CLASS: &str = "now-playing-background";

pub struct NowPlayingWindow {
    window: gtk::Window,
    artwork: gtk::Picture,
    artwork_placeholder: gtk::Label,
    title_label: gtk::Label,
    artist_label: gtk::Label,
    album_label: gtk::Label,
    details_label: gtk::Label,
    info_box: gtk::Box,
    background_css: gtk::CssProvider,
    background_style: Cell<BackgroundStyle>,
    current_background: Cell<Background>,
    lights_off: Cell<bool>,
    hide_track_info: gtk::CheckButton,
    lights_off_menu: gtk::CheckButton,
    background_style_gradient: gtk::CheckButton,
    background_style_solid: gtk::CheckButton,
    gui_tx: Option<async_channel::Sender<GUIMessage>>,
}

impl NowPlayingWindow {
    fn send_preference_update(
        gui_tx: &Option<async_channel::Sender<GUIMessage>>,
        configure: impl FnOnce(&mut Preferences),
    ) {
        let mut preference = Preferences::new();
        configure(&mut preference);

        if let Some(gui_tx) = gui_tx {
            if let Err(error) = gui_tx.try_send(GUIMessage::UpdatePreference(preference)) {
                eprintln!("failed to send preference update: {error}");
            }
        }
    }

    pub fn new_with_settings(
        gui_tx: Option<async_channel::Sender<GUIMessage>>,
        preferences_interface: Option<Arc<Mutex<PreferencesInterface>>>,
    ) -> Self {
        let window = gtk::Window::builder()
            .title("SongRec")
            .default_width(WINDOW_WIDTH)
            .default_height(WINDOW_HEIGHT)
            .resizable(true)
            .hide_on_close(true)
            .build();

        let header = gtk::HeaderBar::new();
        header.set_title_widget(Some(&gtk::Label::new(Some(&gettext("Now playing")))));
        window.set_titlebar(Some(&header));

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(ROOT_SPACING)
            .build();
        root.add_css_class(BACKGROUND_CSS_CLASS);

        let cover_frame = gtk::AspectFrame::builder()
            .ratio(1.0)
            .obey_child(false)
            .hexpand(true)
            .vexpand(true)
            .build();

        let cover_picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .can_shrink(true)
            .hexpand(true)
            .vexpand(true)
            .build();

        let artwork_placeholder = gtk::Label::builder()
            .label(&gettext("Listening..."))
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        let cover_overlay = gtk::Overlay::new();
        cover_overlay.set_child(Some(&cover_picture));
        cover_overlay.add_overlay(&artwork_placeholder);
        cover_frame.set_child(Some(&cover_overlay));

        let title_label = gtk::Label::builder()
            .halign(gtk::Align::Center)
            .wrap(true)
            .css_classes([TITLE_CSS_CLASS])
            .build();
        let artist_label = gtk::Label::builder()
            .halign(gtk::Align::Center)
            .wrap(true)
            .css_classes([ARTIST_CSS_CLASS])
            .build();
        let album_label = gtk::Label::builder()
            .halign(gtk::Align::Center)
            .wrap(true)
            .css_classes([ALBUM_CSS_CLASS])
            .build();
        let details_label = gtk::Label::builder()
            .halign(gtk::Align::Center)
            .wrap(true)
            .css_classes([DETAILS_CSS_CLASS])
            .build();

        let info_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(INFO_BOX_SPACING)
            .halign(gtk::Align::Center)
            .build();
        info_box.append(&title_label);
        info_box.append(&artist_label);
        info_box.append(&album_label);
        info_box.append(&details_label);

        root.append(&cover_frame);
        root.append(&info_box);
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
            &format!(
                ".{BACKGROUND_CSS_CLASS} {{ background-color: #181818; color: #ffffff; padding: {BACKGROUND_PADDING_PX}px; }}
                 .{BACKGROUND_CSS_CLASS} > label {{ color: #ffffff; }}",
            ),
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
                text_css_for_resize.load_from_string(&font_css_for_size(size));
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

        let current_background = Background::fallback();

        let hide_track_info = gtk::CheckButton::new();
        let lights_off_menu = gtk::CheckButton::new();
        let background_style_gradient = gtk::CheckButton::new();
        let background_style_solid = gtk::CheckButton::new();

        let prefs = preferences_interface
            .as_ref()
            .map(|preferences_interface| preferences_interface.lock().unwrap().preferences.clone())
            .unwrap_or_default();

        let now_playing = Self {
            window,
            artwork: cover_picture,
            artwork_placeholder,
            title_label,
            artist_label,
            album_label,
            details_label,
            info_box,
            background_css,
            background_style: Cell::new(BackgroundStyle::Gradient),
            current_background: Cell::new(current_background),
            lights_off: Cell::new(false),
            hide_track_info: hide_track_info.clone(),
            lights_off_menu: lights_off_menu.clone(),
            background_style_gradient: background_style_gradient.clone(),
            background_style_solid: background_style_solid.clone(),
            gui_tx,
        };

        let popover = gtk::Popover::new();
        popover.set_has_arrow(false);
        let menu_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();

        let hide_track_info_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .hexpand(true)
            .build();
        let hide_track_info_label = gtk::Label::new(Some(&gettext("Hide track info")));
        hide_track_info_row.append(&hide_track_info_label);
        hide_track_info.set_halign(gtk::Align::End);
        hide_track_info.set_hexpand(true);
        hide_track_info_row.append(&hide_track_info);
        hide_track_info.set_active(prefs.hide_now_playing_info.unwrap_or(false));
        menu_box.append(&hide_track_info_row);

        let lights_off_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .hexpand(true)
            .build();
        let lights_off_label = gtk::Label::new(Some(&gettext("Lights off")));
        lights_off_row.append(&lights_off_label);
        lights_off_menu.set_halign(gtk::Align::End);
        lights_off_menu.set_hexpand(true);
        lights_off_row.append(&lights_off_menu);
        lights_off_menu.set_active(prefs.lights_off_enabled.unwrap_or(false));
        menu_box.append(&lights_off_row);

        let background_style_button = gtk::MenuButton::builder()
            .label(&gettext("Background style"))
            .halign(gtk::Align::Start)
            .build();
        let background_style_popover = gtk::Popover::new();
        background_style_popover.set_has_arrow(false);
        let background_style_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();

        let gradient_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .hexpand(true)
            .build();
        let gradient_label = gtk::Label::new(Some(&gettext("Gradient")));
        gradient_row.append(&gradient_label);
        background_style_gradient.set_halign(gtk::Align::End);
        background_style_gradient.set_hexpand(true);
        gradient_row.append(&background_style_gradient);
        background_style_box.append(&gradient_row);

        let solid_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .hexpand(true)
            .build();
        let solid_label = gtk::Label::new(Some(&gettext("Solid")));
        solid_row.append(&solid_label);
        background_style_solid.set_halign(gtk::Align::End);
        background_style_solid.set_hexpand(true);
        solid_row.append(&background_style_solid);
        background_style_box.append(&solid_row);

        background_style_popover.set_child(Some(&background_style_box));
        background_style_button.set_popover(Some(&background_style_popover));
        match BackgroundStyle::from_preference(prefs.now_playing_background_style.as_deref()) {
            BackgroundStyle::Gradient => background_style_gradient.set_active(true),
            BackgroundStyle::Solid => background_style_solid.set_active(true),
        }
        menu_box.append(&background_style_button);
        popover.set_child(Some(&menu_box));

        let popover_for_click = popover.clone();
        let window_for_click = now_playing.window.clone();
        popover_for_click.set_parent(&window_for_click);

        let gesture = gtk::GestureClick::new();
        gesture.set_button(3);
        gesture.connect_pressed(move |_, _, x, y| {
            let pointing_rect = gdk::Rectangle::new(x as i32, y as i32, 1, 1);
            popover_for_click.set_pointing_to(Some(&pointing_rect));
            popover_for_click.popup();
        });
        now_playing.window.add_controller(gesture);

        let gui_tx_for_hide = now_playing.gui_tx.clone();
        now_playing.hide_track_info.connect_toggled(move |button| {
            Self::send_preference_update(&gui_tx_for_hide, |preference| {
                preference.hide_now_playing_info = Some(button.is_active());
            });
        });

        let gui_tx_for_lights = now_playing.gui_tx.clone();
        now_playing.lights_off_menu.connect_toggled(move |button| {
            Self::send_preference_update(&gui_tx_for_lights, |preference| {
                preference.lights_off_enabled = Some(button.is_active());
                if button.is_active() {
                    preference.hide_now_playing_info = Some(false);
                }
            });
        });

        let gui_tx_for_bg = now_playing.gui_tx.clone();
        now_playing
            .background_style_gradient
            .connect_toggled(move |button| {
                if button.is_active() {
                    Self::send_preference_update(&gui_tx_for_bg, |preference| {
                        preference.now_playing_background_style =
                            Some(BackgroundStyle::Gradient.as_preference_value().to_string());
                    });
                }
            });

        let gui_tx_for_bg_solid = now_playing.gui_tx.clone();
        now_playing
            .background_style_solid
            .connect_toggled(move |button| {
                if button.is_active() {
                    Self::send_preference_update(&gui_tx_for_bg_solid, |preference| {
                        preference.now_playing_background_style =
                            Some(BackgroundStyle::Solid.as_preference_value().to_string());
                    });
                }
            });

        now_playing
    }

    pub fn refresh_from_preferences(&self, preferences: &Preferences) {
        let hide_info = preferences.hide_now_playing_info.unwrap_or(false);
        let lights_off = preferences.lights_off_enabled.unwrap_or(false);
        let desired_style =
            BackgroundStyle::from_preference(preferences.now_playing_background_style.as_deref());

        if self.hide_track_info.is_active() != hide_info {
            self.hide_track_info.set_active(hide_info);
        }
        self.set_show_track_info(!hide_info);

        if self.lights_off_menu.is_active() != lights_off {
            self.lights_off_menu.set_active(lights_off);
        }
        self.hide_track_info.set_sensitive(!lights_off);
        self.set_lights(lights_off);

        match desired_style {
            BackgroundStyle::Gradient => {
                if self.background_style_gradient.is_active() != true {
                    self.background_style_gradient.set_active(true);
                }
                if self.background_style_solid.is_active() != false {
                    self.background_style_solid.set_active(false);
                }
            }
            BackgroundStyle::Solid => {
                if self.background_style_gradient.is_active() != false {
                    self.background_style_gradient.set_active(false);
                }
                if self.background_style_solid.is_active() != true {
                    self.background_style_solid.set_active(true);
                }
            }
        }
        self.set_background_style(desired_style);
    }

    /// Enable or disable Lights Off mode.
    /// When enabled the artwork is hidden and the background becomes pure black
    /// (either solid or gradient black depending on preference).
    pub fn set_lights_off(&self, enabled: bool) {
        self.lights_off.set(enabled);
        self.apply_background();
        self.sync_artwork_visibility();
    }

    /// Compatibility alias for the existing call sites.
    pub fn set_lights(&self, enabled: bool) {
        self.set_lights_off(enabled);
    }

    fn sync_artwork_visibility(&self) {
        let has_paintable = self.artwork.paintable().is_some();

        if self.lights_off.get() {
            self.artwork.set_visible(false);
            self.artwork_placeholder.set_visible(false);
            return;
        }

        if has_paintable {
            self.artwork.set_visible(true);
            self.artwork_placeholder.set_visible(false);
        } else {
            self.artwork.set_visible(false);
            self.artwork_placeholder.set_visible(true);
        }
    }

    /// Refreshes the displayed song metadata and artwork using a recognized track.
    ///
    /// If the message contains cover art, it is applied to the main image area and
    /// used to derive the window background; otherwise the fallback state is used.
    pub fn update(&self, message: &SongRecognizedMessage) {
        self.set_metadata(message);

        if let Some(bytes) = message.cover_image.as_ref() {
            self.apply_cover(bytes);
        } else {
            self.set_missing_cover_state();
        }
    }

    fn set_metadata(&self, message: &SongRecognizedMessage) {
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
    }

    fn apply_cover(&self, bytes: &[u8]) {
        if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from(bytes)) {
            self.artwork.set_paintable(Some(&texture));
            self.set_background_from_cover(bytes);
            self.sync_artwork_visibility();
        } else {
            self.set_missing_cover_state();
        }
    }

    fn set_missing_cover_state(&self) {
        self.artwork.set_paintable(Option::<&gdk::Texture>::None);
        self.current_background.set(Background::fallback());
        self.apply_background();
        self.sync_artwork_visibility();
    }

    fn set_background_from_cover(&self, bytes: &[u8]) {
        let background = background_from_cover_image(bytes);
        self.current_background.set(background);
        self.apply_background();
    }

    fn apply_background(&self) {
        if self.lights_off.get() {
            // Force pure black background when lights off is active
            match self.background_style.get() {
                BackgroundStyle::Gradient => self.set_gradient_background(Background {
                    top: (38, 38, 38),
                    bottom: (0, 0, 0),
                }),
                BackgroundStyle::Solid => self.set_solid_background((0, 0, 0)),
            }
            return;
        }

        let background = self.current_background.get();
        match self.background_style.get() {
            BackgroundStyle::Gradient => self.set_gradient_background(background),
            BackgroundStyle::Solid => self.set_solid_background(background.top),
        }
    }

    fn set_gradient_background(&self, background: Background) {
        let css = format!(
            ".now-playing-background {{
                background: linear-gradient(to bottom,
                    rgb({}, {}, {}),
                    rgb({}, {}, {}));
                color: #ffffff;
                padding: 96px;
            }}
            .now-playing-background > label {{ color: #ffffff; }}
            .now-playing-background .now-playing-title,
            .now-playing-background .now-playing-artist,
            .now-playing-background .now-playing-album,
            .now-playing-background .now-playing-details {{ color: #ffffff; }}",
            background.top.0,
            background.top.1,
            background.top.2,
            background.bottom.0,
            background.bottom.1,
            background.bottom.2,
        );
        self.background_css.load_from_string(&css);
    }

    fn set_solid_background(&self, color: (u8, u8, u8)) {
        let css = format!(
            ".now-playing-background {{
                background-color: rgb({}, {}, {});
                color: #ffffff;
                padding: 96px;
            }}
            .now-playing-background > label {{ color: #ffffff; }}
            .now-playing-background .now-playing-title,
            .now-playing-background .now-playing-artist,
            .now-playing-background .now-playing-album,
            .now-playing-background .now-playing-details {{ color: #ffffff; }}",
            color.0, color.1, color.2,
        );
        self.background_css.load_from_string(&css);
    }

    /// Shows or hides the metadata box for the active track.
    pub fn set_show_track_info(&self, show: bool) {
        self.info_box.set_visible(show);
    }

    /// Sets the background rendering style for the window.
    ///
    /// `Gradient` derives the backdrop from the cover art, while `Solid` uses a
    /// single tone based on the artwork palette.
    pub fn set_background_style(&self, style: BackgroundStyle) {
        self.background_style.set(style);
        self.apply_background();
    }

    /// Presents the window to the user.
    pub fn present(&self) {
        self.window.present();
    }

    /// Closes the window without destroying its internal state.
    pub fn close(&self) {
        self.window.close();
    }
}

/// Controls the visual treatment of the Now Playing background.
///
/// The gradient variant keeps the artwork-derived palette while the solid
/// variant uses a single accent color to keep the window light and readable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundStyle {
    Gradient,
    Solid,
}

impl BackgroundStyle {
    pub fn as_preference_value(self) -> &'static str {
        match self {
            Self::Gradient => "gradient",
            Self::Solid => "solid",
        }
    }

    pub fn from_preference(value: Option<&str>) -> Self {
        match value {
            Some("solid") => Self::Solid,
            _ => Self::Gradient,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Background {
    top: (u8, u8, u8),
    bottom: (u8, u8, u8),
}

impl Background {
    fn fallback() -> Self {
        Self {
            top: (28, 27, 30),
            bottom: (9, 9, 11),
        }
    }
}

/**
 * Computes a CSS rule for the label font sizes based on the current window dimensions.
 *
 * The size is derived from a base font size and scaled to a width/height ratio
 * that keeps the typography readable across both standard and fullscreen layouts.
 */
fn font_css_for_size(size: (i32, i32)) -> String {
    let width_scale = size.0 as f64 / BASE_SCALE_WIDTH;
    let height_scale = size.1 as f64 / BASE_SCALE_HEIGHT;
    let scale = width_scale
        .min(height_scale)
        .clamp(MIN_FONT_SCALE, MAX_FONT_SCALE);

    let title_size = (TITLE_BASE_FONT_SIZE * scale).round() as i32;
    let artist_size = (ARTIST_BASE_FONT_SIZE * scale).round() as i32;
    let album_size = (ALBUM_BASE_FONT_SIZE * scale).round() as i32;
    let details_size = (DETAILS_BASE_FONT_SIZE * scale).round() as i32;

    format!(
        ".{TITLE_CSS_CLASS} {{ font-size: {title_size}px; font-weight: bold; }}
         .{ARTIST_CSS_CLASS} {{ font-size: {artist_size}px; font-weight: bold; }}
         .{ALBUM_CSS_CLASS} {{ font-size: {album_size}px; font-weight: bold; }}
         .{DETAILS_CSS_CLASS} {{ font-size: {details_size}px; font-weight: bold; }}"
    )
}

/**
 * Builds a dark, artwork-derived background from a cover-image byte stream.
 *
 * If the image cannot be decoded, the function falls back to the default theme
 * colors so the UI remains readable.
 */
fn background_from_cover_image(bytes: &[u8]) -> Background {
    image::load_from_memory(bytes)
        .map(|image| generate_cover_background(&image))
        .unwrap_or_else(|_| Background::fallback())
}

/**
 * Extracts a dark, high-contrast palette from cover art for the window background.
 *
 * The function emphasizes saturated artwork colors while avoiding near-white and
 * near-black values so the white metadata remains readable against the resulting
 * gradient.
 */
fn generate_cover_background(image: &image::DynamicImage) -> Background {
    let small = image.thumbnail(72, 72).to_rgb8();

    // Quantize into compact RGB buckets while keeping weighted sums so the
    // chosen color is still representative of the source art.
    #[derive(Default, Clone, Copy)]
    struct Bucket {
        weight: f32,
        red: f32,
        green: f32,
        blue: f32,
    }

    let mut buckets = std::collections::HashMap::<u32, Bucket>::new();

    for (x, y, pixel) in small.enumerate_pixels() {
        let [red, green, blue] = pixel.0;
        let rf = red as f32 / 255.0;
        let gf = green as f32 / 255.0;
        let bf = blue as f32 / 255.0;

        let (_, saturation, _) = rgb_to_hsl(rf, gf, bf);
        let luminance = relative_luminance(rf, gf, bf);

        // Skip colors that are effectively white or black. Neither gives us a
        // useful accent, and the former is especially bad with white text.
        if luminance > 0.92 || luminance < 0.018 {
            continue;
        }

        // Rich colors should contribute more than grayish colors, while colors
        // around the middle of the luminance range are generally better UI
        // accents than highlights or deep shadows.
        let saturation_weight = 0.35 + saturation.powf(1.35) * 3.5;
        let luminance_weight = (1.0 - ((luminance - 0.38).abs() / 0.38)).clamp(0.15, 1.0);

        // Slightly favor the center of the artwork. This helps avoid picking
        // a tiny bright corner or border as the whole application's mood.
        let nx = (x as f32 + 0.5) / small.width() as f32;
        let ny = (y as f32 + 0.5) / small.height() as f32;
        let center_distance = ((nx - 0.5).powi(2) + (ny - 0.5).powi(2)).sqrt();
        let spatial_weight = (1.15 - center_distance).clamp(0.55, 1.15);

        // A tiny hue-distance factor prevents almost-black/gray colors from
        // winning simply because there are many of them.
        let chroma_weight = if saturation < 0.06 { 0.55 } else { 1.0 };
        let weight = saturation_weight * luminance_weight * spatial_weight * chroma_weight;

        let qr = red >> 4;
        let qg = green >> 4;
        let qb = blue >> 4;
        let key = ((qr as u32) << 8) | ((qg as u32) << 4) | qb as u32;

        let bucket = buckets.entry(key).or_default();
        bucket.weight += weight;
        bucket.red += rf * weight;
        bucket.green += gf * weight;
        bucket.blue += bf * weight;
    }

    let candidate = buckets.into_values().max_by(|a, b| {
        a.weight
            .partial_cmp(&b.weight)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let Some(bucket) = candidate else {
        return Background::fallback();
    };

    if bucket.weight <= f32::EPSILON {
        return Background::fallback();
    }

    let red = bucket.red / bucket.weight;
    let green = bucket.green / bucket.weight;
    let blue = bucket.blue / bucket.weight;
    let (hue, saturation, _) = rgb_to_hsl(red, green, blue);

    // A modest saturation floor gives colorful covers a rich mood without
    // inventing a strong hue for genuinely grayscale artwork.
    let target_saturation = if saturation < 0.08 {
        0.0
    } else {
        (saturation * 1.08).clamp(0.22, 0.70)
    };

    // Start with a dark accent, then move it darker as needed
    // until white text has a genuine WCAG-style contrast margin.
    let mut top_lightness = if target_saturation == 0.0 {
        0.135
    } else {
        0.165
    };
    let top = loop {
        let rgb = hsl_to_rgb(hue, target_saturation, top_lightness);
        if contrast_ratio(rgb, (255, 255, 255)) >= 4.75 || top_lightness <= 0.07 {
            break rgb;
        }
        top_lightness -= 0.01;
    };

    // The lower stop keeps the same hue, but is much darker and less saturated.
    // This makes the metadata area quiet and preserves readable white text.
    let mut bottom_saturation = (target_saturation * 0.42).min(0.30);
    let mut bottom_lightness = 0.055;
    let bottom = loop {
        let rgb = hsl_to_rgb(hue, bottom_saturation, bottom_lightness);
        if contrast_ratio(rgb, (255, 255, 255)) >= 7.0 || bottom_lightness <= 0.025 {
            break rgb;
        }
        bottom_lightness -= 0.005;
        bottom_saturation *= 0.96;
    };

    Background { top, bottom }
}

/**
 * Converts an RGB color into HSL space for palette extraction and contrast tuning.
 */
fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let lightness = (max + min) / 2.0;
    let delta = max - min;

    if delta <= f32::EPSILON {
        return (0.0, 0.0, lightness);
    }

    let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
    let hue = if max == r {
        ((g - b) / delta).rem_euclid(6.0) / 6.0
    } else if max == g {
        (((b - r) / delta) + 2.0) / 6.0
    } else {
        (((r - g) / delta) + 4.0) / 6.0
    };

    (hue, saturation, lightness)
}

/**
 * Converts HSL values back into an 8-bit RGB triplet for the generated window background.
 */
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let chroma = (1.0_f32 - (2.0_f32 * l - 1.0_f32).abs()) * s;
    let x = chroma * (1.0 - ((h * 6.0).rem_euclid(2.0) - 1.0).abs());
    let m = l - chroma / 2.0;

    let (r1, g1, b1) = match (h * 6.0).floor() as i32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };

    (
        ((r1 + m).clamp(0.0, 1.0) * 255.0).round() as u8,
        ((g1 + m).clamp(0.0, 1.0) * 255.0).round() as u8,
        ((b1 + m).clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

/**
 * Computes the relative luminance used to keep text contrast high against cover art.
 */
fn relative_luminance(r: f32, g: f32, b: f32) -> f32 {
    fn linearize(channel: f32) -> f32 {
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    }

    0.2126 * linearize(r) + 0.7152 * linearize(g) + 0.0722 * linearize(b)
}

/**
 * Returns the WCAG-style contrast ratio between two RGB colors.
 */
fn contrast_ratio(first: (u8, u8, u8), second: (u8, u8, u8)) -> f32 {
    let first = relative_luminance(
        first.0 as f32 / 255.0,
        first.1 as f32 / 255.0,
        first.2 as f32 / 255.0,
    );
    let second = relative_luminance(
        second.0 as f32 / 255.0,
        second.1 as f32 / 255.0,
        second.2 as f32 / 255.0,
    );

    let lighter = first.max(second);
    let darker = first.min(second);
    (lighter + 0.05) / (darker + 0.05)
}
