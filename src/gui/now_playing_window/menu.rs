//! Context-menu construction and preference-update signal bindings.

use super::{
    BackgroundStyle, NowPlayingSettings, NowPlayingWindow, TRANSITION_DURATION_DEFAULT_MS,
    TRANSITION_DURATION_MAX_MS, TRANSITION_DURATION_MIN_MS, TrackInfoAlignment, TransitionEffect,
    transition_duration_from_scale,
};
use crate::core::preferences::Preferences;
use crate::core::thread_messages::GUIMessage;
use adw::prelude::*;
use gettextrs::gettext;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

const PREFERENCE_UPDATE_DEBOUNCE_MS: u64 = 150;
const TRANSITION_DURATION_STEP_MS: f64 = 100.0;

/// The context-menu controls whose state mirrors the active presentation settings.
pub(super) struct NowPlayingControls {
    pub(super) round_corners: gtk::Switch,
    pub(super) hide_track_info: gtk::Switch,
    pub(super) background_style_gradient: gtk::ToggleButton,
    pub(super) background_style_solid: gtk::ToggleButton,
    pub(super) track_info_alignment_left: gtk::ToggleButton,
    pub(super) track_info_alignment_center: gtk::ToggleButton,
    pub(super) always_display_last_recognized_song: gtk::Switch,
    pub(super) transition_menu: gtk::DropDown,
    pub(super) transition_duration: gtk::Scale,
    pub(super) lights_off_menu: gtk::Switch,
    pub(super) fullscreen: gtk::Switch,
}

/// Creates the switches and segmented controls used by the Now Playing context menu.
pub(super) fn build_controls() -> NowPlayingControls {
    let round_corners = gtk::Switch::new();
    let hide_track_info = gtk::Switch::new();
    let always_display_last_recognized_song = gtk::Switch::new();
    let transition_labels: Vec<_> = TransitionEffect::ALL
        .into_iter()
        .map(TransitionEffect::translated_label)
        .collect();
    let transition_label_references: Vec<_> =
        transition_labels.iter().map(String::as_str).collect();
    let transition_menu = gtk::DropDown::from_strings(&transition_label_references);
    let transition_duration = gtk::Scale::with_range(
        gtk::Orientation::Horizontal,
        TRANSITION_DURATION_MIN_MS as f64,
        TRANSITION_DURATION_MAX_MS as f64,
        TRANSITION_DURATION_STEP_MS,
    );
    transition_duration.set_value(TRANSITION_DURATION_DEFAULT_MS as f64);
    transition_duration.set_digits(0);
    transition_duration.set_draw_value(true);
    transition_duration.set_hexpand(true);
    transition_duration.set_width_request(190);
    let lights_off_menu = gtk::Switch::new();
    let fullscreen = gtk::Switch::new();
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
        fullscreen,
    }
}

impl NowPlayingWindow {
    /// Builds and sends a preference update message to the main GUI task.
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
    fn schedule_transition_duration_update(
        gui_tx: Option<async_channel::Sender<GUIMessage>>,
        generation: Rc<Cell<u64>>,
        pending_duration_ms: Rc<Cell<Option<u64>>>,
        duration_ms: u64,
    ) {
        let current_generation = generation.get().wrapping_add(1);
        generation.set(current_generation);

        glib::timeout_add_local_once(
            Duration::from_millis(PREFERENCE_UPDATE_DEBOUNCE_MS),
            move || {
                if generation.get() != current_generation
                    || pending_duration_ms.get() != Some(duration_ms)
                {
                    return;
                }

                let mut preference = Preferences::new();
                preference.now_playing_transition_duration_ms = Some(duration_ms);
                let sent = if let Some(gui_tx) = gui_tx.as_ref() {
                    if let Err(error) = gui_tx.try_send(GUIMessage::UpdatePreference(preference)) {
                        eprintln!("failed to send preference update: {error}");
                        false
                    } else {
                        true
                    }
                } else {
                    false
                };

                // A standalone window has no shared preference task to
                // acknowledge this value. Do not let its local pending state
                // shadow a later explicit refresh forever.
                if !sent && pending_duration_ms.get() == Some(duration_ms) {
                    pending_duration_ms.set(None);
                }
            },
        );
    }

    /// Builds and installs the right-click context menu for the Now Playing window.
    pub(super) fn setup_context_menu(&self, settings: NowPlayingSettings) {
        let popover = gtk::Popover::new();
        popover.set_has_arrow(false);
        let menu_grid = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(12)
            .halign(gtk::Align::Start)
            .build();

        let reset_button = gtk::Button::with_label(&gettext("Reset"));
        reset_button.set_halign(gtk::Align::Center);
        menu_grid.attach(&reset_button, 0, 0, 2, 1);
        let gui_tx_for_reset = self.gui_tx.clone();
        let pending_duration_for_reset = self.state.pending_transition_duration_ms.clone();
        let duration_update_generation_for_reset =
            self.state.transition_duration_update_generation.clone();
        reset_button.connect_clicked(move |_| {
            pending_duration_for_reset.set(None);
            duration_update_generation_for_reset
                .set(duration_update_generation_for_reset.get().wrapping_add(1));
            if let Some(gui_tx) = gui_tx_for_reset.as_ref()
                && let Err(error) = gui_tx.try_send(GUIMessage::ResetNowPlayingPreferences)
            {
                eprintln!("failed to reset Now Playing preferences: {error}");
            }
        });

        self.add_switch_menu_row(
            &menu_grid,
            1,
            &gettext("Round corners of album cover"),
            &self.controls.round_corners,
            settings.round_corners,
            !settings.lights_off,
        );
        self.add_switch_menu_row(
            &menu_grid,
            2,
            &gettext("Hide track info"),
            &self.controls.hide_track_info,
            settings.hide_track_info,
            !settings.lights_off,
        );
        self.add_alignment_menu_row(
            &menu_grid,
            3,
            settings.track_info_alignment,
            !settings.hide_track_info,
        );
        self.add_background_style_menu_row(&menu_grid, 4, settings.background_style);
        self.add_switch_menu_row(
            &menu_grid,
            5,
            &gettext("Always display last recognized song"),
            &self.controls.always_display_last_recognized_song,
            settings.always_display_last_recognized_song,
            true,
        );
        self.add_transition_menu_row(
            &menu_grid,
            6,
            &self.controls.transition_menu,
            settings.transition,
        );
        self.add_transition_duration_menu_row(
            &menu_grid,
            7,
            &self.controls.transition_duration,
            settings.transition_duration_ms,
            !matches!(settings.transition, TransitionEffect::None),
        );
        self.add_switch_menu_row(
            &menu_grid,
            8,
            &gettext("Lights off"),
            &self.controls.lights_off_menu,
            settings.lights_off,
            true,
        );
        self.add_switch_menu_row(
            &menu_grid,
            9,
            &gettext("Full screen"),
            &self.controls.fullscreen,
            self.ui.window.is_fullscreen(),
            true,
        );

        popover.set_child(Some(&menu_grid));
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

        let window_for_fullscreen = self.ui.window.clone();
        self.controls
            .fullscreen
            .connect_active_notify(move |switch| {
                if switch.is_active() {
                    window_for_fullscreen.fullscreen();
                } else {
                    window_for_fullscreen.unfullscreen();
                }
            });

        let fullscreen_switch = self.controls.fullscreen.clone();
        self.ui.window.connect_fullscreened_notify(move |window| {
            let fullscreened = window.is_fullscreen();
            if fullscreen_switch.is_active() != fullscreened {
                fullscreen_switch.set_active(fullscreened);
            }
        });
    }

    /// Adds a label-and-switch row to the context menu.
    fn add_switch_menu_row(
        &self,
        menu_grid: &gtk::Grid,
        row: i32,
        title: &str,
        switch: &gtk::Switch,
        active: bool,
        sensitive: bool,
    ) {
        let label = gtk::Label::new(Some(title));
        label.set_halign(gtk::Align::Start);
        label.set_valign(gtk::Align::Center);
        menu_grid.attach(&label, 0, row, 1, 1);
        switch.set_halign(gtk::Align::Start);
        switch.set_valign(gtk::Align::Center);
        switch.set_active(active);
        switch.set_sensitive(sensitive);
        menu_grid.attach(switch, 1, row, 1, 1);
    }

    /// Adds the transition effect drop-down to the context menu and selects the saved effect.
    fn add_transition_menu_row(
        &self,
        menu_grid: &gtk::Grid,
        row: i32,
        dropdown: &gtk::DropDown,
        effect: TransitionEffect,
    ) {
        let label = gtk::Label::new(Some(&gettext("Transition effect")));
        label.set_halign(gtk::Align::Start);
        label.set_valign(gtk::Align::Center);
        menu_grid.attach(&label, 0, row, 1, 1);
        dropdown.set_selected(effect.index());
        dropdown.set_halign(gtk::Align::Start);
        dropdown.set_valign(gtk::Align::Center);
        dropdown.set_hexpand(false);
        menu_grid.attach(dropdown, 1, row, 1, 1);
    }

    /// Adds the transition-duration slider to the context menu.
    fn add_transition_duration_menu_row(
        &self,
        menu_grid: &gtk::Grid,
        row: i32,
        scale: &gtk::Scale,
        duration_ms: u64,
        sensitive: bool,
    ) {
        let label = gtk::Label::new(Some(&gettext("Transition duration")));
        label.set_halign(gtk::Align::Start);
        label.set_valign(gtk::Align::Center);
        menu_grid.attach(&label, 0, row, 1, 1);
        scale.set_value(duration_ms as f64);
        scale.set_sensitive(sensitive);
        scale.set_halign(gtk::Align::Start);
        scale.set_valign(gtk::Align::Center);
        scale.set_hexpand(false);
        menu_grid.attach(scale, 1, row, 1, 1);
    }

    fn add_alignment_menu_row(
        &self,
        menu_grid: &gtk::Grid,
        row: i32,
        alignment: TrackInfoAlignment,
        sensitive: bool,
    ) {
        let label = gtk::Label::new(Some(&gettext("Track info alignment")));
        label.set_halign(gtk::Align::Start);
        label.set_valign(gtk::Align::Center);
        menu_grid.attach(&label, 0, row, 1, 1);
        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(0)
            .css_classes(["linked"])
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Center)
            .build();
        buttons.append(&self.controls.track_info_alignment_left);
        buttons.append(&self.controls.track_info_alignment_center);
        buttons.set_sensitive(sensitive);
        match alignment {
            TrackInfoAlignment::Left => self.controls.track_info_alignment_left.set_active(true),
            TrackInfoAlignment::Center => {
                self.controls.track_info_alignment_center.set_active(true)
            }
        }
        menu_grid.attach(&buttons, 1, row, 1, 1);
    }

    /// Adds the background-style segmented control to the context menu.
    fn add_background_style_menu_row(
        &self,
        menu_grid: &gtk::Grid,
        row: i32,
        style: BackgroundStyle,
    ) {
        let label = gtk::Label::new(Some(&gettext("Background style")));
        label.set_halign(gtk::Align::Start);
        label.set_valign(gtk::Align::Center);
        menu_grid.attach(&label, 0, row, 1, 1);
        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(0)
            .css_classes(["linked"])
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Center)
            .hexpand(false)
            .build();
        buttons.append(&self.controls.background_style_gradient);
        buttons.append(&self.controls.background_style_solid);
        match style {
            BackgroundStyle::Gradient => self.controls.background_style_gradient.set_active(true),
            BackgroundStyle::Solid => self.controls.background_style_solid.set_active(true),
        }
        menu_grid.attach(&buttons, 1, row, 1, 1);
    }

    /// Connects preference controls to GUI preference update messages.
    pub(super) fn connect_control_handlers(&self) {
        let applying_settings_for_round_corners = self.state.applying_settings.clone();
        let settings_for_round_corners = self.state.settings.clone();
        let gui_tx_for_round_corners = self.gui_tx.clone();
        let artwork_overlay_for_round_corners = self.ui.artwork_overlay.clone();
        self.controls
            .round_corners
            .connect_active_notify(move |switch| {
                if applying_settings_for_round_corners.get() {
                    return;
                }

                let active = switch.is_active();
                let mut settings = settings_for_round_corners.get();
                settings.round_corners = active;
                settings_for_round_corners.set(settings);
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

        let applying_settings_for_hide = self.state.applying_settings.clone();
        let settings_for_hide = self.state.settings.clone();
        let gui_tx_for_hide = self.gui_tx.clone();
        let info_box_for_hide = self.ui.info_box.clone();
        let alignment_left_for_hide = self.controls.track_info_alignment_left.clone();
        let alignment_center_for_hide = self.controls.track_info_alignment_center.clone();
        self.controls
            .hide_track_info
            .connect_active_notify(move |button| {
                if applying_settings_for_hide.get() {
                    return;
                }

                let hide_track_info = button.is_active();
                let mut settings = settings_for_hide.get();
                settings.hide_track_info = hide_track_info;
                settings_for_hide.set(settings);
                info_box_for_hide.set_visible(!hide_track_info);
                alignment_left_for_hide.set_sensitive(!hide_track_info);
                alignment_center_for_hide.set_sensitive(!hide_track_info);
                Self::send_preference_update(&gui_tx_for_hide, |preference| {
                    preference.hide_now_playing_info = Some(hide_track_info);
                });
            });

        let applying_settings_for_alignment_left = self.state.applying_settings.clone();
        let settings_for_alignment_left = self.state.settings.clone();
        let gui_tx_for_alignment_left = self.gui_tx.clone();
        let info_box_for_alignment_left = self.ui.info_box.clone();
        let title_for_alignment_left = self.ui.title_label.clone();
        let artist_for_alignment_left = self.ui.artist_label.clone();
        let album_for_alignment_left = self.ui.album_label.clone();
        let details_for_alignment_left = self.ui.details_label.clone();
        self.controls
            .track_info_alignment_left
            .connect_toggled(move |button| {
                if applying_settings_for_alignment_left.get() || !button.is_active() {
                    return;
                }

                let mut settings = settings_for_alignment_left.get();
                settings.track_info_alignment = TrackInfoAlignment::Left;
                settings_for_alignment_left.set(settings);
                apply_track_info_alignment(
                    &info_box_for_alignment_left,
                    &title_for_alignment_left,
                    &artist_for_alignment_left,
                    &album_for_alignment_left,
                    &details_for_alignment_left,
                    TrackInfoAlignment::Left,
                );
                Self::send_preference_update(&gui_tx_for_alignment_left, |preference| {
                    preference.now_playing_track_info_alignment =
                        Some(TrackInfoAlignment::Left.as_preference_value().to_string());
                });
            });

        let applying_settings_for_alignment_center = self.state.applying_settings.clone();
        let settings_for_alignment_center = self.state.settings.clone();
        let gui_tx_for_alignment_center = self.gui_tx.clone();
        let info_box_for_alignment_center = self.ui.info_box.clone();
        let title_for_alignment_center = self.ui.title_label.clone();
        let artist_for_alignment_center = self.ui.artist_label.clone();
        let album_for_alignment_center = self.ui.album_label.clone();
        let details_for_alignment_center = self.ui.details_label.clone();
        self.controls
            .track_info_alignment_center
            .connect_toggled(move |button| {
                if applying_settings_for_alignment_center.get() || !button.is_active() {
                    return;
                }

                let mut settings = settings_for_alignment_center.get();
                settings.track_info_alignment = TrackInfoAlignment::Center;
                settings_for_alignment_center.set(settings);
                apply_track_info_alignment(
                    &info_box_for_alignment_center,
                    &title_for_alignment_center,
                    &artist_for_alignment_center,
                    &album_for_alignment_center,
                    &details_for_alignment_center,
                    TrackInfoAlignment::Center,
                );
                Self::send_preference_update(&gui_tx_for_alignment_center, |preference| {
                    preference.now_playing_track_info_alignment =
                        Some(TrackInfoAlignment::Center.as_preference_value().to_string());
                });
            });

        let applying_settings_for_always_display_last = self.state.applying_settings.clone();
        let settings_for_always_display_last = self.state.settings.clone();
        let gui_tx_for_always_display_last = self.gui_tx.clone();
        self.controls
            .always_display_last_recognized_song
            .connect_active_notify(move |button| {
                if applying_settings_for_always_display_last.get() {
                    return;
                }

                let always_display_last_recognized_song = button.is_active();
                let mut settings = settings_for_always_display_last.get();
                settings.always_display_last_recognized_song = always_display_last_recognized_song;
                settings_for_always_display_last.set(settings);
                Self::send_preference_update(&gui_tx_for_always_display_last, |preference| {
                    preference.always_display_last_recognized_song =
                        Some(always_display_last_recognized_song);
                });
            });

        let applying_settings_for_transition = self.state.applying_settings.clone();
        let settings_for_transition = self.state.settings.clone();
        let gui_tx_for_transition = self.gui_tx.clone();
        let transition_state = self.state.transition.clone();
        let transition_duration_control = self.controls.transition_duration.clone();
        let transition_duration_update_generation_for_transition =
            self.state.transition_duration_update_generation.clone();
        let pending_transition_duration_for_transition =
            self.state.pending_transition_duration_ms.clone();
        self.controls
            .transition_menu
            .connect_selected_notify(move |dropdown| {
                if applying_settings_for_transition.get() {
                    return;
                }

                let effect = TransitionEffect::from_index(dropdown.selected());
                let mut settings = settings_for_transition.get();
                settings.transition = effect;
                settings_for_transition.set(settings);
                transition_state.set(effect);
                pending_transition_duration_for_transition.set(None);
                transition_duration_update_generation_for_transition.set(
                    transition_duration_update_generation_for_transition
                        .get()
                        .wrapping_add(1),
                );
                transition_duration_control
                    .set_sensitive(!matches!(effect, TransitionEffect::None));
                Self::send_preference_update(&gui_tx_for_transition, |preference| {
                    preference.now_playing_transition =
                        Some(effect.as_preference_value().to_string());
                    preference.now_playing_transition_duration_ms =
                        Some(settings.transition_duration_ms);
                });
            });

        let applying_settings_for_duration = self.state.applying_settings.clone();
        let settings_for_duration = self.state.settings.clone();
        let transition_duration_state = self.state.transition_duration_ms.clone();
        let gui_tx_for_transition_duration = self.gui_tx.clone();
        let transition_duration_update = self.state.transition_duration_update_generation.clone();
        let pending_transition_duration = self.state.pending_transition_duration_ms.clone();
        self.controls
            .transition_duration
            .connect_value_changed(move |scale| {
                if applying_settings_for_duration.get() {
                    return;
                }

                let duration_ms = transition_duration_from_scale(scale.value());
                let mut settings = settings_for_duration.get();
                settings.transition_duration_ms = duration_ms;
                settings_for_duration.set(settings);
                transition_duration_state.set(duration_ms);
                pending_transition_duration.set(Some(duration_ms));

                // Persist only the duration. Capturing the effect here caused
                // an older debounce to overwrite a transition chosen later.
                Self::schedule_transition_duration_update(
                    gui_tx_for_transition_duration.clone(),
                    transition_duration_update.clone(),
                    pending_transition_duration.clone(),
                    duration_ms,
                );
            });

        let applying_settings_for_lights = self.state.applying_settings.clone();
        let settings_for_lights = self.state.settings.clone();
        let lights_off_state = self.state.lights_off.clone();
        let showing_listening_for_lights = self.state.showing_listening.clone();
        let gui_tx_for_lights = self.gui_tx.clone();
        let round_for_lights_menu = self.controls.round_corners.clone();
        let hide_for_lights_menu = self.controls.hide_track_info.clone();
        let alignment_left_for_lights_menu = self.controls.track_info_alignment_left.clone();
        let alignment_center_for_lights_menu = self.controls.track_info_alignment_center.clone();
        let info_box_for_lights = self.ui.info_box.clone();
        let artwork_for_lights = self.ui.artwork.clone();
        let artwork_placeholder_for_lights = self.ui.artwork_placeholder.clone();
        let background_area_for_lights = self.ui.background_area.clone();
        self.controls
            .lights_off_menu
            .connect_active_notify(move |button| {
                if applying_settings_for_lights.get() {
                    return;
                }

                let active = button.is_active();
                let mut settings = settings_for_lights.get();
                settings.lights_off = active;
                if active {
                    settings.hide_track_info = false;
                }
                settings_for_lights.set(settings);
                lights_off_state.set(active);
                round_for_lights_menu.set_sensitive(!active);
                hide_for_lights_menu.set_sensitive(!active);
                alignment_left_for_lights_menu.set_sensitive(!settings.hide_track_info);
                alignment_center_for_lights_menu.set_sensitive(!settings.hide_track_info);

                if active {
                    let was_applying_settings = applying_settings_for_lights.replace(true);
                    hide_for_lights_menu.set_active(false);
                    applying_settings_for_lights.set(was_applying_settings);
                    info_box_for_lights.set_visible(true);
                }

                sync_artwork_visibility(
                    &artwork_for_lights,
                    &artwork_placeholder_for_lights,
                    active,
                    showing_listening_for_lights.get(),
                );
                background_area_for_lights.queue_draw();

                Self::send_preference_update(&gui_tx_for_lights, |preference| {
                    preference.lights_off_enabled = Some(active);
                    if active {
                        preference.hide_now_playing_info = Some(false);
                    }
                });
            });

        let applying_settings_for_gradient = self.state.applying_settings.clone();
        let settings_for_gradient = self.state.settings.clone();
        let background_style_for_gradient = self.state.background_style.clone();
        let background_area_for_gradient = self.ui.background_area.clone();
        let gui_tx_for_gradient = self.gui_tx.clone();
        self.controls
            .background_style_gradient
            .connect_toggled(move |button| {
                if applying_settings_for_gradient.get() || !button.is_active() {
                    return;
                }

                let mut settings = settings_for_gradient.get();
                settings.background_style = BackgroundStyle::Gradient;
                settings_for_gradient.set(settings);
                background_style_for_gradient.set(BackgroundStyle::Gradient);
                background_area_for_gradient.queue_draw();
                Self::send_preference_update(&gui_tx_for_gradient, |preference| {
                    preference.now_playing_background_style =
                        Some(BackgroundStyle::Gradient.as_preference_value().to_string());
                });
            });

        let applying_settings_for_solid = self.state.applying_settings.clone();
        let settings_for_solid = self.state.settings.clone();
        let background_style_for_solid = self.state.background_style.clone();
        let background_area_for_solid = self.ui.background_area.clone();
        let gui_tx_for_solid = self.gui_tx.clone();
        self.controls
            .background_style_solid
            .connect_toggled(move |button| {
                if applying_settings_for_solid.get() || !button.is_active() {
                    return;
                }

                let mut settings = settings_for_solid.get();
                settings.background_style = BackgroundStyle::Solid;
                settings_for_solid.set(settings);
                background_style_for_solid.set(BackgroundStyle::Solid);
                background_area_for_solid.queue_draw();
                Self::send_preference_update(&gui_tx_for_solid, |preference| {
                    preference.now_playing_background_style =
                        Some(BackgroundStyle::Solid.as_preference_value().to_string());
                });
            });
    }
}

/// Applies a metadata alignment without needing to retain a window reference in a GTK callback.
fn apply_track_info_alignment(
    info_box: &gtk::Box,
    title_label: &gtk::Label,
    artist_label: &gtk::Label,
    album_label: &gtk::Label,
    details_label: &gtk::Label,
    alignment: TrackInfoAlignment,
) {
    let alignment = match alignment {
        TrackInfoAlignment::Left => gtk::Align::Start,
        TrackInfoAlignment::Center => gtk::Align::Center,
    };
    info_box.set_halign(alignment);
    title_label.set_halign(alignment);
    artist_label.set_halign(alignment);
    album_label.set_halign(alignment);
    details_label.set_halign(alignment);
}

/// Mirrors the artwork visibility portion of `NowPlayingWindow::sync_artwork_visibility`.
fn sync_artwork_visibility(
    artwork: &gtk::Picture,
    placeholder: &gtk::Label,
    lights_off: bool,
    showing_listening: bool,
) {
    if lights_off {
        artwork.set_visible(false);
        placeholder.set_visible(showing_listening);
    } else if showing_listening {
        artwork.set_visible(false);
        placeholder.set_visible(true);
    } else if artwork.paintable().is_some() {
        artwork.set_visible(true);
        placeholder.set_visible(false);
    } else {
        artwork.set_visible(false);
        placeholder.set_visible(true);
    }
}
