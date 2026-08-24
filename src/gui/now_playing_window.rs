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
use cairo::{Context, Format, ImageSurface};
use gettextrs::gettext;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const WINDOW_WIDTH: i32 = 720;
const WINDOW_HEIGHT: i32 = 820;
const MIN_WINDOW_WIDTH: i32 = 360;
const MIN_WINDOW_HEIGHT: i32 = 410;
const MIN_ARTWORK_SIZE: i32 = 135;
const ARTWORK_MARGIN_PX: i32 = 24;
const ARTWORK_CORNER_RADIUS_PX: i32 = 18;
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
const GRADIENT_SURFACE_WIDTH: i32 = 256;
const TRANSITION_START: f64 = 0.20;
const TRANSITION_DURATION_MIN_MS: f64 = 500.0;
const TRANSITION_DURATION_MAX_MS: f64 = 5000.0;
const TRANSITION_DURATION_DEFAULT_MS: f64 = 2000.0;
const TRANSITION_DURATION_STEP_MS: f64 = 100.0;
const PREFERENCE_UPDATE_DEBOUNCE_MS: u64 = 150;

struct NowPlayingWidgets {
    window: gtk::Window,
    artwork: gtk::Picture,
    artwork_overlay: gtk::Overlay,
    artwork_placeholder: gtk::Label,
    title_label: gtk::Label,
    artist_label: gtk::Label,
    album_label: gtk::Label,
    details_label: gtk::Label,
    info_box: gtk::Box,
    background_area: gtk::DrawingArea,
    content_revealer: gtk::Revealer,
}

struct NowPlayingControls {
    round_corners: gtk::Switch,
    hide_track_info: gtk::Switch,
    background_style_gradient: gtk::ToggleButton,
    background_style_solid: gtk::ToggleButton,
    track_info_alignment_left: gtk::ToggleButton,
    track_info_alignment_center: gtk::ToggleButton,
    always_display_last_recognized_song: gtk::Switch,
    transition_menu: gtk::DropDown,
    transition_duration: gtk::Scale,
    lights_off_menu: gtk::Switch,
}

struct NowPlayingState {
    gradient_surface: Rc<RefCell<Option<CachedGradient>>>,
    background_style: Rc<Cell<BackgroundStyle>>,
    current_background: Rc<Cell<Background>>,
    lights_off: Rc<Cell<bool>>,
    showing_listening: Rc<Cell<bool>>,
    transition: Rc<Cell<TransitionEffect>>,
    transition_duration_ms: Rc<Cell<u64>>,
    transition_generation: Rc<Cell<u64>>,
    transition_duration_update_generation: Rc<Cell<u64>>,
    last_track_key: Rc<RefCell<Option<String>>>,
}

pub struct NowPlayingWindow {
    ui: NowPlayingWidgets,
    controls: NowPlayingControls,
    state: NowPlayingState,
    gui_tx: Option<async_channel::Sender<GUIMessage>>,
}

impl NowPlayingWindow {
    /// Builds and sends a preference update message to the main GUI task.
    ///
    /// The closure mutates a temporary `Preferences` value, which is sent through
    /// the GUI channel when one is available.
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

    /// Debounces a preference update so dragging the transition-duration slider does not persist every intermediate value.
    fn schedule_preference_update(
        gui_tx: Option<async_channel::Sender<GUIMessage>>,
        generation: Rc<Cell<u64>>,
        mut configure: impl FnMut(&mut Preferences) + 'static,
    ) {
        let current_generation = generation.get().wrapping_add(1);
        generation.set(current_generation);

        glib::timeout_add_local_once(
            Duration::from_millis(PREFERENCE_UPDATE_DEBOUNCE_MS),
            move || {
                if generation.get() != current_generation {
                    return;
                }

                let mut preference = Preferences::new();
                configure(&mut preference);
                if let Some(gui_tx) = gui_tx.as_ref() {
                    if let Err(error) = gui_tx.try_send(GUIMessage::UpdatePreference(preference)) {
                        eprintln!("failed to send preference update: {error}");
                    }
                }
            },
        );
    }

    /// Constructs a Now Playing window initialized from the current preferences.
    pub fn new_with_settings(
        gui_tx: Option<async_channel::Sender<GUIMessage>>,
        preferences_interface: Option<Arc<Mutex<PreferencesInterface>>>,
    ) -> Self {
        let preferences = Self::current_preferences(preferences_interface.as_ref());
        let (ui, text_css) = Self::build_ui();
        let controls = Self::build_controls();
        let state = Self::build_state();

        let now_playing = Self {
            ui,
            controls,
            state,
            gui_tx,
        };

        now_playing.setup_rendering(&text_css);
        now_playing.apply_initial_preferences(&preferences);
        now_playing.setup_context_menu(&preferences);
        now_playing.connect_control_handlers();

        now_playing
    }

    /// Reads the current preferences from the shared interface, if available.
    fn current_preferences(
        preferences_interface: Option<&Arc<Mutex<PreferencesInterface>>>,
    ) -> Preferences {
        preferences_interface
            .map(|interface| interface.lock().unwrap().preferences.clone())
            .unwrap_or_default()
    }

    /// Builds the window, artwork presentation, metadata widgets, and static CSS providers.
    fn build_ui() -> (NowPlayingWidgets, gtk::CssProvider) {
        let window = gtk::Window::builder()
            .title("SongRec")
            .default_width(WINDOW_WIDTH)
            .default_height(WINDOW_HEIGHT)
            .resizable(true)
            .hide_on_close(true)
            .build();
        window.set_size_request(MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT);

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
        cover_frame.set_margin_top(ARTWORK_MARGIN_PX);
        cover_frame.set_margin_bottom(ARTWORK_MARGIN_PX);
        cover_frame.set_margin_start(ARTWORK_MARGIN_PX);
        cover_frame.set_margin_end(ARTWORK_MARGIN_PX);
        cover_frame.set_size_request(MIN_ARTWORK_SIZE, MIN_ARTWORK_SIZE);

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
        cover_overlay.set_overflow(gtk::Overflow::Hidden);
        cover_overlay.add_css_class("now-playing-artwork-rounded");
        cover_frame.set_child(Some(&cover_overlay));

        let title_label = gtk::Label::builder()
            .halign(gtk::Align::Center)
            .wrap(false)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes([TITLE_CSS_CLASS])
            .build();
        let artist_label = gtk::Label::builder()
            .halign(gtk::Align::Center)
            .wrap(false)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes([ARTIST_CSS_CLASS])
            .build();
        let album_label = gtk::Label::builder()
            .halign(gtk::Align::Center)
            .wrap(false)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes([ALBUM_CSS_CLASS])
            .build();
        let details_label = gtk::Label::builder()
            .halign(gtk::Align::Center)
            .wrap(false)
            .ellipsize(gtk::pango::EllipsizeMode::End)
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

        let content_revealer = gtk::Revealer::builder()
            .reveal_child(true)
            .transition_duration(TRANSITION_DURATION_DEFAULT_MS as u32)
            .transition_type(gtk::RevealerTransitionType::Crossfade)
            .hexpand(true)
            .vexpand(true)
            .build();
        content_revealer.set_child(Some(&root));

        let background_area = gtk::DrawingArea::new();
        background_area.set_hexpand(true);
        background_area.set_vexpand(true);
        background_area.set_can_target(false);

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&background_area));
        overlay.add_overlay(&content_revealer);
        overlay.add_overlay(&artwork_placeholder);
        artwork_placeholder.set_halign(gtk::Align::Center);
        artwork_placeholder.set_valign(gtk::Align::Center);
        artwork_placeholder.set_css_classes(&[TITLE_CSS_CLASS]);
        window.set_child(Some(&overlay));

        let background_css = gtk::CssProvider::new();
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &background_css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        background_css.load_from_string(&format!(
            ".{BACKGROUND_CSS_CLASS} {{ background-color: transparent; color: #ffffff; padding: {BACKGROUND_PADDING_PX}px; }}\n             .{BACKGROUND_CSS_CLASS} > label {{ color: #ffffff; }}\n             .now-playing-artwork-rounded {{ border-radius: {ARTWORK_CORNER_RADIUS_PX}px; }}"
        ));

        let text_css = gtk::CssProvider::new();
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &text_css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        text_css.load_from_string(&font_css_for_size((WINDOW_WIDTH, WINDOW_HEIGHT)));

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

        (
            NowPlayingWidgets {
                window,
                artwork: cover_picture,
                artwork_overlay: cover_overlay,
                artwork_placeholder,
                title_label,
                artist_label,
                album_label,
                details_label,
                info_box,
                background_area,
                content_revealer,
            },
            text_css,
        )
    }

    /// Creates the switches and segmented controls used by the Now Playing context menu.
    fn build_controls() -> NowPlayingControls {
        let round_corners = gtk::Switch::new();
        let hide_track_info = gtk::Switch::new();
        let always_display_last_recognized_song = gtk::Switch::new();
        let transition_menu = gtk::DropDown::from_strings(&[
            &gettext("None"),
            &gettext("Fade"),
            &gettext("Slide right"),
            &gettext("Slide left"),
            &gettext("Slide up"),
            &gettext("Slide down"),
            &gettext("Swing right"),
            &gettext("Swing left"),
            &gettext("Swing up"),
            &gettext("Swing down"),
        ]);
        let transition_duration = gtk::Scale::with_range(
            gtk::Orientation::Horizontal,
            TRANSITION_DURATION_MIN_MS,
            TRANSITION_DURATION_MAX_MS,
            TRANSITION_DURATION_STEP_MS,
        );
        transition_duration.set_value(TRANSITION_DURATION_DEFAULT_MS);
        transition_duration.set_digits(0);
        transition_duration.set_draw_value(true);
        transition_duration.set_hexpand(true);
        transition_duration.set_width_request(190);
        let lights_off_menu = gtk::Switch::new();
        let background_style_gradient = gtk::ToggleButton::with_label(&gettext("Gradient"));
        let background_style_solid = gtk::ToggleButton::with_label(&gettext("Solid"));
        background_style_solid.set_group(Some(&background_style_gradient));
        let track_info_alignment_left = gtk::ToggleButton::with_label(&gettext("Left"));
        let track_info_alignment_center = gtk::ToggleButton::with_label(&gettext("Center"));
        track_info_alignment_center.set_group(Some(&track_info_alignment_left));

        NowPlayingControls {
            round_corners,
            hide_track_info,
            background_style_gradient,
            background_style_solid,
            track_info_alignment_left,
            track_info_alignment_center,
            always_display_last_recognized_song,
            transition_menu,
            transition_duration,
            lights_off_menu,
        }
    }

    /// Creates the mutable rendering and preference state used by the window.
    fn build_state() -> NowPlayingState {
        NowPlayingState {
            gradient_surface: Rc::new(RefCell::new(None)),
            background_style: Rc::new(Cell::new(BackgroundStyle::Gradient)),
            current_background: Rc::new(Cell::new(Background::fallback())),
            lights_off: Rc::new(Cell::new(false)),
            showing_listening: Rc::new(Cell::new(true)),
            transition: Rc::new(Cell::new(TransitionEffect::None)),
            transition_duration_ms: Rc::new(Cell::new(TRANSITION_DURATION_DEFAULT_MS as u64)),
            transition_generation: Rc::new(Cell::new(0)),
            transition_duration_update_generation: Rc::new(Cell::new(0)),
            last_track_key: Rc::new(RefCell::new(None)),
        }
    }

    /// Connects background drawing and resize handling for the window.
    fn setup_rendering(&self, text_css: &gtk::CssProvider) {
        let background_state_for_draw = self.state.current_background.clone();
        let background_style_state_for_draw = self.state.background_style.clone();
        let lights_off_state_for_draw = self.state.lights_off.clone();
        let gradient_surface_for_draw = self.state.gradient_surface.clone();
        self.ui
            .background_area
            .set_draw_func(move |_, context, width, height| {
                draw_background(
                    context,
                    width,
                    height,
                    background_state_for_draw.get(),
                    background_style_state_for_draw.get(),
                    lights_off_state_for_draw.get(),
                    &gradient_surface_for_draw,
                );
            });

        let last_size = Rc::new(Cell::new((0, 0)));
        let text_css_for_resize = text_css.clone();
        let last_size_for_resize = last_size.clone();
        let root_for_resize = self.ui.background_area.clone();
        let gradient_surface_for_resize = self.state.gradient_surface.clone();
        let background_state_for_resize = self.state.current_background.clone();
        let background_style_state_for_resize = self.state.background_style.clone();
        let lights_off_state_for_resize = self.state.lights_off.clone();
        self.ui.background_area.add_tick_callback(move |area, _| {
            let size = (area.width(), area.height());
            if size.0 > 0 && size.1 > 0 && size != last_size_for_resize.get() {
                last_size_for_resize.set(size);
                text_css_for_resize.load_from_string(&font_css_for_size(size));

                if matches!(
                    background_style_state_for_resize.get(),
                    BackgroundStyle::Gradient
                ) {
                    rebuild_gradient_surface(
                        &gradient_surface_for_resize,
                        effective_background(
                            background_state_for_resize.get(),
                            background_style_state_for_resize.get(),
                            lights_off_state_for_resize.get(),
                        ),
                        root_for_resize.height(),
                    );
                    area.queue_draw();
                }
            }
            glib::ControlFlow::Continue
        });
    }

    /// Applies the initial Now Playing preferences to the newly created window.
    fn apply_initial_preferences(&self, preferences: &Preferences) {
        self.set_round_corners(preferences.now_playing_round_corners.unwrap_or(true));
        self.set_track_info_alignment(TrackInfoAlignment::from_preference(
            preferences.now_playing_track_info_alignment.as_deref(),
        ));
        self.set_always_display_last_recognized_song(
            preferences
                .always_display_last_recognized_song
                .unwrap_or(true),
        );
        self.set_transition(TransitionEffect::from_preference(
            preferences.now_playing_transition.as_deref(),
        ));
        self.set_listening_state();
    }

    /// Builds and installs the right-click context menu for the Now Playing window.
    fn setup_context_menu(&self, preferences: &Preferences) {
        let popover = gtk::Popover::new();
        popover.set_has_arrow(false);
        let menu_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();

        self.add_switch_menu_row(
            &menu_box,
            &gettext("Round corners of album cover"),
            &self.controls.round_corners,
            preferences.now_playing_round_corners.unwrap_or(true),
            !preferences.lights_off_enabled.unwrap_or(false),
        );
        self.add_switch_menu_row(
            &menu_box,
            &gettext("Hide track info"),
            &self.controls.hide_track_info,
            preferences.hide_now_playing_info.unwrap_or(false),
            !preferences.lights_off_enabled.unwrap_or(false),
        );
        self.add_alignment_menu_row(&menu_box, preferences);
        self.add_background_style_menu_row(&menu_box, preferences);
        self.add_switch_menu_row(
            &menu_box,
            &gettext("Always display last recognized song"),
            &self.controls.always_display_last_recognized_song,
            preferences
                .always_display_last_recognized_song
                .unwrap_or(true),
            true,
        );
        let transition_effect =
            TransitionEffect::from_preference(preferences.now_playing_transition.as_deref());
        self.add_transition_menu_row(&menu_box, &self.controls.transition_menu, transition_effect);
        self.add_transition_duration_menu_row(
            &menu_box,
            &self.controls.transition_duration,
            preferences
                .now_playing_transition_duration_ms
                .unwrap_or(2000),
            !matches!(transition_effect, TransitionEffect::None),
        );
        self.add_switch_menu_row(
            &menu_box,
            &gettext("Lights off"),
            &self.controls.lights_off_menu,
            preferences.lights_off_enabled.unwrap_or(false),
            true,
        );

        popover.set_child(Some(&menu_box));
        let popover_for_click = popover.clone();
        let window_for_click = self.ui.window.clone();
        popover_for_click.set_parent(&window_for_click);

        let gesture = gtk::GestureClick::new();
        gesture.set_button(3);
        gesture.connect_pressed(move |_, _, x, y| {
            if popover_for_click.is_visible() {
                popover_for_click.popdown();
                return;
            }
            let pointing_rect = gdk::Rectangle::new(x as i32, y as i32, 1, 1);
            popover_for_click.set_pointing_to(Some(&pointing_rect));
            popover_for_click.popup();
        });
        self.ui.window.add_controller(gesture);
    }

    /// Adds a label-and-switch row to the context menu.
    fn add_switch_menu_row(
        &self,
        menu_box: &gtk::Box,
        title: &str,
        switch: &gtk::Switch,
        active: bool,
        sensitive: bool,
    ) {
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .hexpand(true)
            .build();
        let label = gtk::Label::new(Some(title));
        label.set_hexpand(true);
        label.set_halign(gtk::Align::Start);
        row.append(&label);
        switch.set_halign(gtk::Align::End);
        switch.set_active(active);
        switch.set_sensitive(sensitive);
        row.append(switch);
        menu_box.append(&row);
    }

    /// Adds the transition effect drop-down to the context menu and selects the saved effect.
    fn add_transition_menu_row(
        &self,
        menu_box: &gtk::Box,
        dropdown: &gtk::DropDown,
        effect: TransitionEffect,
    ) {
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .hexpand(true)
            .build();
        let label = gtk::Label::new(Some(&gettext("Transition effect")));
        label.set_hexpand(true);
        label.set_halign(gtk::Align::Start);
        row.append(&label);
        dropdown.set_selected(effect.index());
        row.append(dropdown);
        menu_box.append(&row);
    }

    /// Adds the transition-duration slider to the context menu.
    fn add_transition_duration_menu_row(
        &self,
        menu_box: &gtk::Box,
        scale: &gtk::Scale,
        duration_ms: u64,
        sensitive: bool,
    ) {
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .hexpand(true)
            .build();
        let label = gtk::Label::new(Some(&gettext("Transition duration")));
        label.set_hexpand(true);
        label.set_halign(gtk::Align::Start);
        row.append(&label);
        scale.set_value(duration_ms.clamp(500, 5000) as f64);
        scale.set_sensitive(sensitive);
        row.append(scale);
        menu_box.append(&row);
    }

    fn add_alignment_menu_row(&self, menu_box: &gtk::Box, preferences: &Preferences) {
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .hexpand(true)
            .build();
        let label = gtk::Label::new(Some(&gettext("Track info alignment")));
        label.set_hexpand(true);
        label.set_halign(gtk::Align::Start);
        row.append(&label);
        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(0)
            .css_classes(["linked"])
            .build();
        buttons.append(&self.controls.track_info_alignment_left);
        buttons.append(&self.controls.track_info_alignment_center);
        match TrackInfoAlignment::from_preference(
            preferences.now_playing_track_info_alignment.as_deref(),
        ) {
            TrackInfoAlignment::Left => self.controls.track_info_alignment_left.set_active(true),
            TrackInfoAlignment::Center => {
                self.controls.track_info_alignment_center.set_active(true)
            }
        }
        row.append(&buttons);
        menu_box.append(&row);
    }

    /// Adds the background-style segmented control to the context menu.
    fn add_background_style_menu_row(&self, menu_box: &gtk::Box, preferences: &Preferences) {
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .hexpand(true)
            .build();
        let label = gtk::Label::new(Some(&gettext("Background style")));
        row.append(&label);
        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(0)
            .css_classes(["linked"])
            .hexpand(false)
            .build();
        buttons.append(&self.controls.background_style_gradient);
        buttons.append(&self.controls.background_style_solid);
        row.append(&buttons);
        match BackgroundStyle::from_preference(preferences.now_playing_background_style.as_deref())
        {
            BackgroundStyle::Gradient => self.controls.background_style_gradient.set_active(true),
            BackgroundStyle::Solid => self.controls.background_style_solid.set_active(true),
        }
        menu_box.append(&row);
    }

    /// Connects preference controls to GUI preference update messages.
    fn connect_control_handlers(&self) {
        let gui_tx_for_round_corners = self.gui_tx.clone();
        let artwork_overlay_for_round_corners = self.ui.artwork_overlay.clone();
        self.controls
            .round_corners
            .connect_active_notify(move |switch| {
                let active = switch.is_active();
                if active {
                    artwork_overlay_for_round_corners.add_css_class("now-playing-artwork-rounded");
                } else {
                    artwork_overlay_for_round_corners
                        .remove_css_class("now-playing-artwork-rounded");
                }
                Self::send_preference_update(&gui_tx_for_round_corners, |preference| {
                    preference.now_playing_round_corners = Some(active);
                });
            });

        let gui_tx_for_hide = self.gui_tx.clone();
        self.controls
            .hide_track_info
            .connect_active_notify(move |button| {
                Self::send_preference_update(&gui_tx_for_hide, |preference| {
                    preference.hide_now_playing_info = Some(button.is_active());
                });
            });

        let gui_tx_for_alignment_left = self.gui_tx.clone();
        self.controls
            .track_info_alignment_left
            .connect_toggled(move |button| {
                if button.is_active() {
                    Self::send_preference_update(&gui_tx_for_alignment_left, |preference| {
                        preference.now_playing_track_info_alignment =
                            Some(TrackInfoAlignment::Left.as_preference_value().to_string());
                    });
                }
            });

        let gui_tx_for_alignment_center = self.gui_tx.clone();
        self.controls
            .track_info_alignment_center
            .connect_toggled(move |button| {
                if button.is_active() {
                    Self::send_preference_update(&gui_tx_for_alignment_center, |preference| {
                        preference.now_playing_track_info_alignment =
                            Some(TrackInfoAlignment::Center.as_preference_value().to_string());
                    });
                }
            });

        let gui_tx_for_always_display_last = self.gui_tx.clone();
        self.controls
            .always_display_last_recognized_song
            .connect_active_notify(move |button| {
                Self::send_preference_update(&gui_tx_for_always_display_last, |preference| {
                    preference.always_display_last_recognized_song = Some(button.is_active());
                });
            });

        let gui_tx_for_transition = self.gui_tx.clone();
        let transition_state = self.state.transition.clone();
        let transition_duration_state = self.state.transition_duration_ms.clone();
        let transition_duration_control = self.controls.transition_duration.clone();
        self.controls
            .transition_menu
            .connect_selected_notify(move |dropdown| {
                let effect = TransitionEffect::from_index(dropdown.selected());
                transition_state.set(effect);
                transition_duration_control
                    .set_sensitive(!matches!(effect, TransitionEffect::None));
                Self::send_preference_update(&gui_tx_for_transition, |preference| {
                    preference.now_playing_transition =
                        Some(effect.as_preference_value().to_string());
                    preference.now_playing_transition_duration_ms =
                        Some(transition_duration_state.get());
                });
            });

        let transition_duration_state = self.state.transition_duration_ms.clone();
        let transition_state_for_duration = self.state.transition.clone();
        let gui_tx_for_transition_duration = self.gui_tx.clone();
        let transition_duration_update = self.state.transition_duration_update_generation.clone();
        self.controls
            .transition_duration
            .connect_value_changed(move |scale| {
                let duration_ms = scale.value().round().clamp(500.0, 5000.0) as u64;
                transition_duration_state.set(duration_ms);
                let effect = transition_state_for_duration.get();
                let gui_tx = gui_tx_for_transition_duration.clone();
                let pending = transition_duration_update.clone();
                Self::schedule_preference_update(gui_tx, pending, move |preference| {
                    preference.now_playing_transition_duration_ms = Some(duration_ms);
                    preference.now_playing_transition =
                        Some(effect.as_preference_value().to_string());
                });
            });

        let gui_tx_for_lights = self.gui_tx.clone();
        let round_for_lights_menu = self.controls.round_corners.clone();
        let hide_for_lights_menu = self.controls.hide_track_info.clone();
        self.controls
            .lights_off_menu
            .connect_active_notify(move |button| {
                let active = button.is_active();
                round_for_lights_menu.set_sensitive(!active);
                hide_for_lights_menu.set_sensitive(!active);
                Self::send_preference_update(&gui_tx_for_lights, |preference| {
                    preference.lights_off_enabled = Some(active);
                    if active {
                        preference.hide_now_playing_info = Some(false);
                    }
                });
            });

        let gui_tx_for_bg = self.gui_tx.clone();
        self.controls
            .background_style_gradient
            .connect_toggled(move |button| {
                if button.is_active() {
                    Self::send_preference_update(&gui_tx_for_bg, |preference| {
                        preference.now_playing_background_style =
                            Some(BackgroundStyle::Gradient.as_preference_value().to_string());
                    });
                }
            });

        let gui_tx_for_bg_solid = self.gui_tx.clone();
        self.controls
            .background_style_solid
            .connect_toggled(move |button| {
                if button.is_active() {
                    Self::send_preference_update(&gui_tx_for_bg_solid, |preference| {
                        preference.now_playing_background_style =
                            Some(BackgroundStyle::Solid.as_preference_value().to_string());
                    });
                }
            });
    }
    /// Refreshes all Now Playing controls and rendering state from persisted preferences.
    pub fn refresh_from_preferences(&self, preferences: &Preferences) {
        let hide_info = preferences.hide_now_playing_info.unwrap_or(false);
        let lights_off = preferences.lights_off_enabled.unwrap_or(false);
        let desired_style =
            BackgroundStyle::from_preference(preferences.now_playing_background_style.as_deref());

        let round_corners = preferences.now_playing_round_corners.unwrap_or(true);
        if self.controls.round_corners.is_active() != round_corners {
            self.controls.round_corners.set_active(round_corners);
        }
        self.set_round_corners(round_corners);

        if self.controls.hide_track_info.is_active() != hide_info {
            self.controls.hide_track_info.set_active(hide_info);
        }
        self.set_show_track_info(!hide_info);

        self.set_track_info_alignment(TrackInfoAlignment::from_preference(
            preferences.now_playing_track_info_alignment.as_deref(),
        ));

        let always_display_last = preferences
            .always_display_last_recognized_song
            .unwrap_or(true);
        if self
            .controls
            .always_display_last_recognized_song
            .is_active()
            != always_display_last
        {
            self.controls
                .always_display_last_recognized_song
                .set_active(always_display_last);
        }

        let desired_transition =
            TransitionEffect::from_preference(preferences.now_playing_transition.as_deref());
        if self.controls.transition_menu.selected() != desired_transition.index() {
            self.controls
                .transition_menu
                .set_selected(desired_transition.index());
        }
        self.state.transition.set(desired_transition);

        let transition_duration = preferences
            .now_playing_transition_duration_ms
            .unwrap_or(2000)
            .clamp(500, 5000);
        self.set_transition_duration(transition_duration);
        self.controls
            .transition_duration
            .set_sensitive(!matches!(desired_transition, TransitionEffect::None));

        match desired_style {
            BackgroundStyle::Gradient => {
                if self.controls.background_style_gradient.is_active() != true {
                    self.controls.background_style_gradient.set_active(true);
                }
                if self.controls.background_style_solid.is_active() != false {
                    self.controls.background_style_solid.set_active(false);
                }
            }
            BackgroundStyle::Solid => {
                if self.controls.background_style_gradient.is_active() != false {
                    self.controls.background_style_gradient.set_active(false);
                }
                if self.controls.background_style_solid.is_active() != true {
                    self.controls.background_style_solid.set_active(true);
                }
            }
        }

        if self.controls.lights_off_menu.is_active() != lights_off {
            self.controls.lights_off_menu.set_active(lights_off);
        }
        self.controls.hide_track_info.set_sensitive(!lights_off);
        self.controls.round_corners.set_sensitive(!lights_off);
        self.set_lights_off(lights_off);
        self.set_background_style(desired_style);
    }

    /// Enable or disable Lights Off mode.
    /// When enabled the artwork is hidden and the background becomes pure black
    /// (either solid or gradient black depending on preference).
    /// Enables or disables Lights Off mode and updates artwork/background visibility.
    pub fn set_lights_off(&self, enabled: bool) {
        self.state.lights_off.set(enabled);
        self.controls.hide_track_info.set_sensitive(!enabled);
        self.controls.round_corners.set_sensitive(!enabled);
        self.apply_background();
        self.sync_artwork_visibility();
    }

    /// Synchronizes artwork and placeholder visibility with the current window state.
    fn sync_artwork_visibility(&self) {
        let has_paintable = self.ui.artwork.paintable().is_some();

        if self.state.lights_off.get() {
            self.ui.artwork.set_visible(false);
            self.ui
                .artwork_placeholder
                .set_visible(self.state.showing_listening.get());
            return;
        }

        if self.state.showing_listening.get() {
            self.ui.artwork.set_visible(false);
            self.ui.artwork_placeholder.set_visible(true);
            return;
        }

        if has_paintable {
            self.ui.artwork.set_visible(true);
            self.ui.artwork_placeholder.set_visible(false);
        } else {
            self.ui.artwork.set_visible(false);
            self.ui.artwork_placeholder.set_visible(true);
        }
    }

    /// Refreshes the displayed song metadata and artwork using a recognized track.
    ///
    /// If the message contains cover art, it is applied to the main image area and
    /// used to derive the window background; otherwise the fallback state is used.
    /// Refreshes the displayed song metadata and artwork from a recognition result.
    pub fn update(&self, message: &SongRecognizedMessage) {
        let has_track_info = !message.song_name.trim().is_empty()
            || !message.artist_name.trim().is_empty()
            || message.cover_image.is_some();

        if !has_track_info {
            self.handle_no_recognition();
            return;
        }

        let should_transition = self
            .state
            .last_track_key
            .borrow()
            .as_deref()
            .is_some_and(|previous| previous != message.track_key)
            && !message.track_key.is_empty();

        *self.state.last_track_key.borrow_mut() = Some(message.track_key.clone());

        if should_transition {
            self.transition_to_track(message.clone());
        } else {
            self.apply_track_update(message);
        }
    }

    /// Applies a recognized track immediately without running a visual transition.
    fn apply_track_update(&self, message: &SongRecognizedMessage) {
        self.state.showing_listening.set(false);
        self.set_metadata(message);

        if let Some(bytes) = message.cover_image.as_ref() {
            self.apply_cover(bytes);
        } else {
            self.set_missing_cover_state();
        }
    }

    /// Runs the configured lightweight transition before displaying a newly recognized track.
    fn transition_to_track(&self, message: SongRecognizedMessage) {
        let effect = self.state.transition.get();
        let duration_ms = self.state.transition_duration_ms.get();
        if matches!(effect, TransitionEffect::None) {
            self.apply_track_update(&message);
            return;
        }

        let generation = self.state.transition_generation.get().wrapping_add(1);
        self.state.transition_generation.set(generation);
        self.ui
            .content_revealer
            .set_transition_duration(duration_ms as u32);
        self.ui
            .content_revealer
            .set_transition_type(effect.revealer_type());

        // Keep the old track visible while it animates out. The new track is only
        // written after that animation has finished, preventing it from flashing
        // before the transition starts.
        self.ui.content_revealer.set_reveal_child(false);

        let revealer = self.ui.content_revealer.clone();
        let generation_state = self.state.transition_generation.clone();
        let title_label = self.ui.title_label.clone();
        let artist_label = self.ui.artist_label.clone();
        let album_label = self.ui.album_label.clone();
        let details_label = self.ui.details_label.clone();
        let artwork = self.ui.artwork.clone();
        let artwork_placeholder = self.ui.artwork_placeholder.clone();
        let artwork_overlay = self.ui.artwork_overlay.clone();
        let background_area = self.ui.background_area.clone();
        let current_background = self.state.current_background.clone();
        let background_style = self.state.background_style.clone();
        let lights_off = self.state.lights_off.clone();
        let showing_listening = self.state.showing_listening.clone();
        let gradient_surface = self.state.gradient_surface.clone();

        glib::timeout_add_local_once(Duration::from_millis(duration_ms), move || {
            if generation_state.get() != generation {
                return;
            }

            showing_listening.set(false);
            title_label.set_label(&message.song_name);
            artist_label.set_label(&message.artist_name);
            album_label.set_label(
                message
                    .album_name
                    .as_deref()
                    .filter(|value| !value.is_empty())
                    .unwrap_or(""),
            );
            details_label.set_label(
                message
                    .release_year
                    .as_deref()
                    .filter(|value| !value.is_empty())
                    .unwrap_or(""),
            );

            if let Some(bytes) = message.cover_image.as_ref() {
                if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from(bytes)) {
                    artwork.set_paintable(Some(&texture));
                    current_background.set(from_cover_image(bytes));
                    if lights_off.get() {
                        match background_style.get() {
                            BackgroundStyle::Gradient => rebuild_gradient_surface(
                                &gradient_surface,
                                Background {
                                    top: (38, 38, 38),
                                    bottom: (0, 0, 0),
                                },
                                background_area.height(),
                            ),
                            BackgroundStyle::Solid => {}
                        }
                    } else if matches!(background_style.get(), BackgroundStyle::Gradient) {
                        rebuild_gradient_surface(
                            &gradient_surface,
                            current_background.get(),
                            background_area.height(),
                        );
                    }
                } else {
                    artwork.set_paintable(Option::<&gdk::Texture>::None);
                    current_background.set(Background::fallback());
                }
            } else {
                artwork.set_paintable(Option::<&gdk::Texture>::None);
                current_background.set(Background::fallback());
            }

            artwork_overlay.set_visible(true);
            artwork_placeholder.set_visible(false);
            revealer.set_reveal_child(true);

            if lights_off.get() {
                // Keep the existing Lights Off background and only replace the content.
            } else if matches!(background_style.get(), BackgroundStyle::Gradient) {
                background_area.queue_draw();
            } else {
                background_area.queue_draw();
            }
        });
    }

    /// Clears the current track and shows the listening placeholder while preserving the background.
    pub fn set_listening_state(&self) {
        self.state
            .transition_generation
            .set(self.state.transition_generation.get().wrapping_add(1));
        self.ui.content_revealer.set_reveal_child(true);
        self.state.showing_listening.set(true);
        self.ui.artwork.set_paintable(Option::<&gdk::Texture>::None);
        self.ui.title_label.set_label("");
        self.ui.artist_label.set_label("");
        self.ui.album_label.set_label("");
        self.ui.details_label.set_label("");
        self.sync_artwork_visibility();
    }

    /// Handles a recognition attempt that produced no track information according to the active preference.
    pub fn handle_no_recognition(&self) {
        if !self
            .controls
            .always_display_last_recognized_song
            .is_active()
        {
            self.set_listening_state();
        }
    }

    /// Selects and applies the transition effect used when a new track replaces the current one.
    pub fn set_transition(&self, effect: TransitionEffect) {
        self.state.transition.set(effect);
        self.controls.transition_menu.set_selected(effect.index());
        self.controls
            .transition_duration
            .set_sensitive(!matches!(effect, TransitionEffect::None));
    }

    /// Sets the transition duration in milliseconds and updates the duration control.
    pub fn set_transition_duration(&self, duration_ms: u64) {
        let duration_ms = duration_ms.clamp(500, 5000);
        self.state.transition_duration_ms.set(duration_ms);
        self.controls
            .transition_duration
            .set_value(duration_ms as f64);
    }

    /// Enables or disables keeping the last recognized song when no new match is available.
    pub fn set_always_display_last_recognized_song(&self, enabled: bool) {
        if self
            .controls
            .always_display_last_recognized_song
            .is_active()
            != enabled
        {
            self.controls
                .always_display_last_recognized_song
                .set_active(enabled);
        }
    }

    /// Updates the title, artist, album, and release-year labels.
    fn set_metadata(&self, message: &SongRecognizedMessage) {
        self.ui.title_label.set_label(&message.song_name);
        self.ui.artist_label.set_label(&message.artist_name);
        self.ui.album_label.set_label(
            message
                .album_name
                .as_deref()
                .filter(|value| !value.is_empty())
                .unwrap_or(""),
        );
        self.ui.details_label.set_label(
            message
                .release_year
                .as_deref()
                .filter(|value| !value.is_empty())
                .unwrap_or(""),
        );
    }

    /// Decodes cover-art bytes into a GTK texture and derives the matching background.
    fn apply_cover(&self, bytes: &[u8]) {
        if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from(bytes)) {
            self.ui.artwork.set_paintable(Some(&texture));
            self.set_background_from_cover(bytes);
            self.sync_artwork_visibility();
        } else {
            self.set_missing_cover_state();
        }
    }

    /// Clears the artwork and restores the fallback background used without cover art.
    fn set_missing_cover_state(&self) {
        self.ui.artwork.set_paintable(Option::<&gdk::Texture>::None);
        self.state.current_background.set(Background::fallback());
        self.apply_background();
        self.sync_artwork_visibility();
    }

    /// Derives the window background colors from the supplied cover-art bytes.
    fn set_background_from_cover(&self, bytes: &[u8]) {
        let background = from_cover_image(bytes);
        self.state.current_background.set(background);
        self.apply_background();
    }

    /// Applies the active background style, including the Lights Off override.
    fn apply_background(&self) {
        if self.state.lights_off.get() {
            // Force pure black background when lights off is active
            match self.state.background_style.get() {
                BackgroundStyle::Gradient => self.set_gradient_background(Background {
                    top: (38, 38, 38),
                    bottom: (0, 0, 0),
                }),
                BackgroundStyle::Solid => self.set_solid_background(),
            }
            return;
        }

        let background = self.state.current_background.get();
        match self.state.background_style.get() {
            BackgroundStyle::Gradient => self.set_gradient_background(background),
            BackgroundStyle::Solid => self.set_solid_background(),
        }
    }

    /// Invalidates the cached gradient and requests a redraw using the given background.
    fn set_gradient_background(&self, background: Background) {
        let height = self.ui.background_area.height();
        rebuild_gradient_surface(&self.state.gradient_surface, background, height);
        self.ui.background_area.queue_draw();
    }

    /// Requests a redraw of the background using the currently selected solid color.
    ///
    /// The actual color is read by `draw_background` from the current background state,
    /// so this method only needs to invalidate the drawing area.
    fn set_solid_background(&self) {
        self.ui.background_area.queue_draw();
    }

    /// Enables or disables rounded corners on the album-art overlay.
    pub fn set_round_corners(&self, enabled: bool) {
        if enabled {
            self.ui
                .artwork_overlay
                .add_css_class("now-playing-artwork-rounded");
        } else {
            self.ui
                .artwork_overlay
                .remove_css_class("now-playing-artwork-rounded");
        }
        if self.controls.round_corners.is_active() != enabled {
            self.controls.round_corners.set_active(enabled);
        }
    }

    /// Applies the requested alignment to the metadata block and its labels.
    pub fn set_track_info_alignment(&self, alignment: TrackInfoAlignment) {
        match alignment {
            TrackInfoAlignment::Left => {
                self.ui.info_box.set_halign(gtk::Align::Start);
                self.ui.title_label.set_halign(gtk::Align::Start);
                self.ui.artist_label.set_halign(gtk::Align::Start);
                self.ui.album_label.set_halign(gtk::Align::Start);
                self.ui.details_label.set_halign(gtk::Align::Start);
                if !self.controls.track_info_alignment_left.is_active() {
                    self.controls.track_info_alignment_left.set_active(true);
                }
            }
            TrackInfoAlignment::Center => {
                self.ui.info_box.set_halign(gtk::Align::Center);
                self.ui.title_label.set_halign(gtk::Align::Center);
                self.ui.artist_label.set_halign(gtk::Align::Center);
                self.ui.album_label.set_halign(gtk::Align::Center);
                self.ui.details_label.set_halign(gtk::Align::Center);
                if !self.controls.track_info_alignment_center.is_active() {
                    self.controls.track_info_alignment_center.set_active(true);
                }
            }
        }
    }

    /// Shows or hides the metadata box for the active track.
    /// Shows or hides the metadata block for the current track.
    pub fn set_show_track_info(&self, show: bool) {
        self.ui.info_box.set_visible(show);
    }

    /// Sets the background rendering style for the window.
    ///
    /// `Gradient` derives the backdrop from the cover art, while `Solid` uses a
    /// single tone based on the artwork palette.
    /// Selects the solid or gradient background rendering style.
    pub fn set_background_style(&self, style: BackgroundStyle) {
        self.state.background_style.set(style);
        self.apply_background();
    }

    /// Presents the window to the user.
    /// Presents the Now Playing window to the user.
    pub fn present(&self) {
        self.ui.window.present();
    }

    /// Closes the window while keeping its internal state available for reuse.
    pub fn close(&self) {
        self.ui.window.close();
    }
}

/// Defines the lightweight visual transition used when a new track is displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionEffect {
    /// Replace the current track immediately without animation.
    None,
    /// Crossfade the current track into the new track.
    Fade,
    /// Slide the new track in from the right.
    SlideRight,
    /// Slide the new track in from the left.
    SlideLeft,
    /// Slide the new track in from the bottom.
    SlideUp,
    /// Slide the new track in from the top.
    SlideDown,
    /// Swing the new track in from the left.
    SwingRight,
    /// Swing the new track in from the right.
    SwingLeft,
    /// Swing the new track in from the bottom.
    SwingUp,
    /// Swing the new track in from the top.
    SwingDown,
}

impl TransitionEffect {
    /// Returns the persisted string representation of this transition effect.
    pub fn as_preference_value(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Fade => "fade",
            Self::SlideRight => "slide-right",
            Self::SlideLeft => "slide-left",
            Self::SlideUp => "slide-up",
            Self::SlideDown => "slide-down",
            Self::SwingRight => "swing-right",
            Self::SwingLeft => "swing-left",
            Self::SwingUp => "swing-up",
            Self::SwingDown => "swing-down",
        }
    }

    /// Parses a persisted transition value, defaulting to no animation.
    pub fn from_preference(value: Option<&str>) -> Self {
        match value {
            Some("fade") => Self::Fade,
            Some("slide-right") => Self::SlideRight,
            Some("slide-left") => Self::SlideLeft,
            Some("slide-up") => Self::SlideUp,
            Some("slide-down") => Self::SlideDown,
            Some("swing-right") => Self::SwingRight,
            Some("swing-left") => Self::SwingLeft,
            Some("swing-up") => Self::SwingUp,
            Some("swing-down") => Self::SwingDown,
            _ => Self::None,
        }
    }

    /// Converts the effect into the corresponding GTK revealer animation.
    pub fn revealer_type(self) -> gtk::RevealerTransitionType {
        match self {
            Self::None => gtk::RevealerTransitionType::None,
            Self::Fade => gtk::RevealerTransitionType::Crossfade,
            Self::SlideRight => gtk::RevealerTransitionType::SlideRight,
            Self::SlideLeft => gtk::RevealerTransitionType::SlideLeft,
            Self::SlideUp => gtk::RevealerTransitionType::SlideUp,
            Self::SlideDown => gtk::RevealerTransitionType::SlideDown,
            Self::SwingRight => gtk::RevealerTransitionType::SwingRight,
            Self::SwingLeft => gtk::RevealerTransitionType::SwingLeft,
            Self::SwingUp => gtk::RevealerTransitionType::SwingUp,
            Self::SwingDown => gtk::RevealerTransitionType::SwingDown,
        }
    }

    /// Returns the zero-based index used by the Transition effect dropdown.
    pub fn index(self) -> u32 {
        self as u32
    }

    /// Converts a Transition effect dropdown index into an effect.
    pub fn from_index(index: u32) -> Self {
        match index {
            0 => Self::None,
            1 => Self::Fade,
            2 => Self::SlideRight,
            3 => Self::SlideLeft,
            4 => Self::SlideUp,
            5 => Self::SlideDown,
            6 => Self::SwingRight,
            7 => Self::SwingLeft,
            8 => Self::SwingUp,
            9 => Self::SwingDown,
            _ => Self::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackInfoAlignment {
    Left,
    Center,
}

impl TrackInfoAlignment {
    /// Returns the persisted string representation of this track-info alignment.
    pub fn as_preference_value(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Center => "center",
        }
    }

    /// Parses a persisted track-info alignment value, defaulting to center.
    pub fn from_preference(value: Option<&str>) -> Self {
        match value {
            Some("left") => Self::Left,
            _ => Self::Center,
        }
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
    /// Returns the persisted string representation of this background style.
    pub fn as_preference_value(self) -> &'static str {
        match self {
            Self::Gradient => "gradient",
            Self::Solid => "solid",
        }
    }

    /// Parses a persisted background style value, defaulting to gradient.
    pub fn from_preference(value: Option<&str>) -> Self {
        match value {
            Some("solid") => Self::Solid,
            _ => Self::Gradient,
        }
    }
}

#[derive(Debug, Clone)]
struct CachedGradient {
    background: Background,
    height: i32,
    surface: ImageSurface,
}

/// Resolves the background colors used for rendering, overriding them for Lights Off.
fn effective_background(
    background: Background,
    style: BackgroundStyle,
    lights_off: bool,
) -> Background {
    if lights_off {
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
    }
}

/// Paints the current background into the supplied Cairo context.
fn draw_background(
    context: &Context,
    width: i32,
    height: i32,
    background: Background,
    style: BackgroundStyle,
    lights_off: bool,
    cache: &RefCell<Option<CachedGradient>>,
) {
    if width <= 0 || height <= 0 {
        return;
    }

    let background = effective_background(background, style, lights_off);

    if matches!(style, BackgroundStyle::Solid) {
        context.set_source_rgb(
            f64::from(background.top.0) / 255.0,
            f64::from(background.top.1) / 255.0,
            f64::from(background.top.2) / 255.0,
        );
        let _ = context.paint();
        return;
    }

    let needs_rebuild = cache
        .borrow()
        .as_ref()
        .map(|cached| cached.background != background || cached.height != height)
        .unwrap_or(true);

    if needs_rebuild {
        rebuild_gradient_surface(cache, background, height);
    }

    let guard = cache.borrow();
    let Some(cached) = guard.as_ref() else {
        return;
    };

    if let Err(error) = context.save() {
        log::warn!("Failed to save Cairo state for gradient: {error}");
        return;
    }

    context.scale(f64::from(width) / f64::from(GRADIENT_SURFACE_WIDTH), 1.0);

    if let Err(error) = context.set_source_surface(&cached.surface, 0.0, 0.0) {
        log::warn!("Failed to set cached gradient surface: {error}");
        let _ = context.restore();
        return;
    }

    if let Err(error) = context.paint() {
        log::warn!("Failed to paint cached gradient surface: {error}");
    }
    let _ = context.restore();
}

/// Rebuilds and caches the vertical gradient surface when its inputs change.
fn rebuild_gradient_surface(
    cache: &RefCell<Option<CachedGradient>>,
    background: Background,
    height: i32,
) {
    if height <= 0 {
        return;
    }

    if cache
        .borrow()
        .as_ref()
        .map(|cached| cached.background == background && cached.height == height)
        .unwrap_or(false)
    {
        return;
    }

    let Ok(mut surface) = ImageSurface::create(Format::ARgb32, GRADIENT_SURFACE_WIDTH, height)
    else {
        log::warn!("Failed to create cached gradient surface");
        return;
    };

    let stride = surface.stride() as usize;
    let width = GRADIENT_SURFACE_WIDTH as usize;
    let top = srgb_triplet_to_linear(background.top);
    let bottom = srgb_triplet_to_linear(background.bottom);

    let Ok(mut data) = surface.data() else {
        log::warn!("Failed to access cached gradient surface data");
        return;
    };

    for y in 0..height as usize {
        let position = if height <= 1 {
            1.0
        } else {
            y as f64 / f64::from(height - 1)
        };
        let t = ((position - TRANSITION_START) / (1.0 - TRANSITION_START)).clamp(0.0, 1.0);
        let t = t * t * (3.0 - 2.0 * t);

        let red = linear_to_srgb(top.0 + (bottom.0 - top.0) * t) * 255.0;
        let green = linear_to_srgb(top.1 + (bottom.1 - top.1) * t) * 255.0;
        let blue = linear_to_srgb(top.2 + (bottom.2 - top.2) * t) * 255.0;

        for x in 0..width {
            // Unbiased stochastic rounding removes visible 8-bit steps without
            // introducing a repeating Bayer/diamond pattern. The cached surface
            // is tiny compared with the actual window and is generated only when
            // the background or window height changes.
            let noise = hash_noise(x as u32, y as u32);
            let r = stochastic_round(red, noise);
            let g = stochastic_round(green, noise);
            let b = stochastic_round(blue, noise);
            let pixel = u32::from_ne_bytes([b, g, r, 255]);
            let offset = y * stride + x * 4;
            data[offset..offset + 4].copy_from_slice(&pixel.to_ne_bytes());
        }
    }
    drop(data);
    surface.flush();

    *cache.borrow_mut() = Some(CachedGradient {
        background,
        height,
        surface,
    });
}

/// Converts an sRGB RGB triplet into linear-light RGB components.
fn srgb_triplet_to_linear(rgb: (u8, u8, u8)) -> (f64, f64, f64) {
    (
        srgb_to_linear(f64::from(rgb.0) / 255.0),
        srgb_to_linear(f64::from(rgb.1) / 255.0),
        srgb_to_linear(f64::from(rgb.2) / 255.0),
    )
}

/// Converts one normalized sRGB channel to linear-light space.
fn srgb_to_linear(channel: f64) -> f64 {
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

/// Converts one normalized linear-light channel back to sRGB space.
fn linear_to_srgb(channel: f64) -> f64 {
    let channel = channel.clamp(0.0, 1.0);
    if channel <= 0.0031308 {
        channel * 12.92
    } else {
        1.055 * channel.powf(1.0 / 2.4) - 0.055
    }
}

/// Applies unbiased stochastic rounding to an 8-bit channel value.
fn stochastic_round(value: f64, noise: f64) -> u8 {
    let value = value.clamp(0.0, 255.0);
    let floor = value.floor();
    let fraction = value - floor;
    let rounded = if noise < fraction { floor + 1.0 } else { floor };
    rounded.clamp(0.0, 255.0) as u8
}

/// Produces deterministic pseudo-random noise in the range `[0, 1]` for dithering.
fn hash_noise(x: u32, y: u32) -> f64 {
    let mut value = x
        .wrapping_mul(0x9E37_79B9)
        .wrapping_add(y.wrapping_mul(0x85EB_CA6B));
    value ^= value >> 16;
    value = value.wrapping_mul(0x7FEB_352D);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846C_A68B);
    value ^= value >> 16;
    f64::from(value) / f64::from(u32::MAX)
}

/// Generates CSS for metadata font sizes scaled to the supplied window dimensions.
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
