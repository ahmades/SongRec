//! Displays the floating Now Playing UI for the current song.
//!
//! This window keeps a custom dark presentation based on the album art, applies
//! a readable high-contrast foreground, and resizes its typography according to
//! the actual window dimensions so fullscreen and regular resizing share the same
//! sizing logic.

use crate::core::preferences::{Preferences, PreferencesInterface};
use crate::core::thread_messages::{GUIMessage, SongRecognizedMessage};
use crate::gui::now_playing_background::{Background, from_cover_image};
use adw::prelude::*;
use cairo::Context;
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
    background_area: gtk::DrawingArea,
    background_style: Rc<Cell<BackgroundStyle>>,
    current_background: Rc<Cell<Background>>,
    lights_off: Rc<Cell<bool>>,
    hide_track_info: gtk::Switch,
    lights_off_menu: gtk::Switch,
    background_style_gradient: gtk::ToggleButton,
    background_style_solid: gtk::ToggleButton,
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
            .hexpand(true)
            .vexpand(true)
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

        // Render the artwork-derived background directly in one DrawingArea.
        // Using a single Cairo gradient avoids the visible rectangular zones that
        // occurred when multiple background renderers were composited.
        let background_area = gtk::DrawingArea::new();
        background_area.set_hexpand(true);
        background_area.set_vexpand(true);
        background_area.set_can_target(false);

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&background_area));
        overlay.add_overlay(&root);
        window.set_child(Some(&overlay));

        // The Now Playing background deliberately uses its own CSS provider so
        // the window does not follow the application's system theme. The actual
        // gradient is painted by background_area; CSS is kept for padding/text.
        let background_css = gtk::CssProvider::new();
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &background_css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        background_css.load_from_string(&format!(
            ".{BACKGROUND_CSS_CLASS} {{ background-color: transparent; color: #ffffff; padding: {BACKGROUND_PADDING_PX}px; }}\n             .{BACKGROUND_CSS_CLASS} > label {{ color: #ffffff; }}"
        ));

        // Typography follows the actual allocated content size. GTK4 does not
        // expose a reliable size-allocation signal on GtkWidget, so we sample
        // the root allocation from a tick callback. The callback itself is very
        // cheap: CSS is regenerated only when the allocation actually changes.
        let text_css = gtk::CssProvider::new();
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &text_css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        text_css.load_from_string(&font_css_for_size((WINDOW_WIDTH, WINDOW_HEIGHT)));

        let last_size = Rc::new(Cell::new((0, 0)));
        let text_css_for_resize = text_css.clone();
        let last_size_for_resize = last_size.clone();
        let root_for_resize = root.clone();
        root.add_tick_callback(move |_, _| {
            let size = (root_for_resize.width(), root_for_resize.height());
            if size.0 > 0 && size.1 > 0 && size != last_size_for_resize.get() {
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
        let background_state = Rc::new(Cell::new(current_background));
        let background_style_state = Rc::new(Cell::new(BackgroundStyle::Gradient));
        let lights_off_state = Rc::new(Cell::new(false));

        let background_state_for_draw = background_state.clone();
        let background_style_state_for_draw = background_style_state.clone();
        let lights_off_state_for_draw = lights_off_state.clone();
        background_area.set_draw_func(move |_, context, width, height| {
            draw_background(
                context,
                width,
                height,
                background_state_for_draw.get(),
                background_style_state_for_draw.get(),
                lights_off_state_for_draw.get(),
            );
        });

        let hide_track_info = gtk::Switch::new();
        let lights_off_menu = gtk::Switch::new();
        let background_style_gradient = gtk::ToggleButton::with_label(&gettext("Gradient"));
        let background_style_solid = gtk::ToggleButton::with_label(&gettext("Solid"));
        background_style_solid.set_group(Some(&background_style_gradient));

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
            background_area: background_area.clone(),
            background_style: background_style_state.clone(),
            current_background: background_state.clone(),
            lights_off: lights_off_state.clone(),
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
        hide_track_info_label.set_hexpand(true);
        hide_track_info_label.set_halign(gtk::Align::Start);
        hide_track_info_row.append(&hide_track_info_label);
        hide_track_info.set_halign(gtk::Align::End);
        hide_track_info_row.append(&hide_track_info);
        hide_track_info.set_active(prefs.hide_now_playing_info.unwrap_or(false));
        menu_box.append(&hide_track_info_row);

        let lights_off_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .hexpand(true)
            .build();
        let lights_off_label = gtk::Label::new(Some(&gettext("Lights off")));
        lights_off_label.set_hexpand(true);
        lights_off_label.set_halign(gtk::Align::Start);
        lights_off_row.append(&lights_off_label);
        lights_off_menu.set_halign(gtk::Align::End);
        lights_off_row.append(&lights_off_menu);
        lights_off_menu.set_active(prefs.lights_off_enabled.unwrap_or(false));
        menu_box.append(&lights_off_row);

        let background_style_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .hexpand(true)
            .build();
        let background_style_label = gtk::Label::new(Some(&gettext("Background style")));
        background_style_row.append(&background_style_label);

        let background_style_buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(0)
            .css_classes(["linked"])
            .hexpand(false)
            .build();
        background_style_buttons.append(&background_style_gradient);
        background_style_buttons.append(&background_style_solid);
        background_style_row.append(&background_style_buttons);
        match BackgroundStyle::from_preference(prefs.now_playing_background_style.as_deref()) {
            BackgroundStyle::Gradient => background_style_gradient.set_active(true),
            BackgroundStyle::Solid => background_style_solid.set_active(true),
        }
        menu_box.append(&background_style_row);
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
        now_playing
            .hide_track_info
            .connect_active_notify(move |button| {
                Self::send_preference_update(&gui_tx_for_hide, |preference| {
                    preference.hide_now_playing_info = Some(button.is_active());
                });
            });

        let gui_tx_for_lights = now_playing.gui_tx.clone();
        now_playing
            .lights_off_menu
            .connect_active_notify(move |button| {
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
        let background = from_cover_image(bytes);
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
            r#".now-playing-background {{
                background-color: transparent;
                color: #ffffff;
                padding: 96px;
            }}
            .now-playing-background > label {{ color: #ffffff; }}
            .now-playing-background .now-playing-title,
            .now-playing-background .now-playing-artist,
            .now-playing-background .now-playing-album,
            .now-playing-background .now-playing-details {{ color: #ffffff; }}"#,
        );
        self.background_css.load_from_string(&css);
        self.current_background.set(background);
        self.background_area.queue_draw();
    }

    fn set_solid_background(&self, color: (u8, u8, u8)) {
        let css = format!(
            r#".now-playing-background {{
                background-color: transparent;
                color: #ffffff;
                padding: 96px;
            }}
            .now-playing-background > label {{ color: #ffffff; }}
            .now-playing-background .now-playing-title,
            .now-playing-background .now-playing-artist,
            .now-playing-background .now-playing-album,
            .now-playing-background .now-playing-details {{ color: #ffffff; }}"#,
        );
        self.background_css.load_from_string(&css);
        self.current_background.set(Background {
            top: color,
            bottom: color,
        });
        self.background_area.queue_draw();
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

/**
 * Computes a CSS rule for the label font sizes based on the current window dimensions.
 *
 * The size is derived from a base font size and scaled to a width/height ratio
 * that keeps the typography readable across both standard and fullscreen layouts.
 */
fn draw_background(
    context: &Context,
    width: i32,
    height: i32,
    background: Background,
    style: BackgroundStyle,
    lights_off: bool,
) {
    if width <= 0 || height <= 0 {
        return;
    }

    let background = if lights_off {
        match style {
            BackgroundStyle::Gradient => Background {
                top: (38, 38, 38),
                bottom: (0, 0, 0),
            },
            BackgroundStyle::Solid => Background {
                top: (0, 0, 0),
                bottom: (0, 0, 0),
            },
        }
    } else {
        background
    };

    if matches!(style, BackgroundStyle::Solid) {
        context.set_source_rgb(
            f64::from(background.top.0) / 255.0,
            f64::from(background.top.1) / 255.0,
            f64::from(background.top.2) / 255.0,
        );
        let _ = context.paint();
        return;
    }

    // Cairo's built-in gradient interpolation can expose 8-bit banding on large
    // displays, especially in the dark lower part of the gradient. Render one
    // horizontal row at a time instead. This is still O(height), rather than
    // O(width * height), and lets us apply a tiny ordered dither per row.
    const TRANSITION_START: f64 = 0.20;
    const BAYER_4X4: [[f64; 4]; 4] = [
        [0.0, 0.5, 0.125, 0.625],
        [0.75, 0.25, 0.875, 0.375],
        [0.1875, 0.6875, 0.0625, 0.5625],
        [0.9375, 0.4375, 0.8125, 0.3125],
    ];

    let top = (
        f64::from(background.top.0) / 255.0,
        f64::from(background.top.1) / 255.0,
        f64::from(background.top.2) / 255.0,
    );
    let bottom = (
        f64::from(background.bottom.0) / 255.0,
        f64::from(background.bottom.1) / 255.0,
        f64::from(background.bottom.2) / 255.0,
    );

    for y in 0..height {
        let position = if height <= 1 {
            1.0
        } else {
            f64::from(y) / f64::from(height - 1)
        };

        // Keep the requested 20% top-color plateau, then use a smoothstep
        // curve for a more gradual and natural interpolation to the bottom.
        let t = ((position - TRANSITION_START) / (1.0 - TRANSITION_START)).clamp(0.0, 1.0);
        let t = t * t * (3.0 - 2.0 * t);

        let mut color = (
            top.0 + (bottom.0 - top.0) * t,
            top.1 + (bottom.1 - top.1) * t,
            top.2 + (bottom.2 - top.2) * t,
        );

        // ±0.5/255 ordered dithering is enough to break visible 8-bit bands
        // while remaining imperceptible at normal viewing distance.
        let dither = (BAYER_4X4[(y as usize) & 3][(y as usize) & 3] - 0.5) / 255.0;
        color.0 = (color.0 + dither).clamp(0.0, 1.0);
        color.1 = (color.1 + dither).clamp(0.0, 1.0);
        color.2 = (color.2 + dither).clamp(0.0, 1.0);

        context.set_source_rgb(color.0, color.1, color.2);
        context.rectangle(0.0, f64::from(y), f64::from(width), 1.0);
        let _ = context.fill();
    }
}

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
