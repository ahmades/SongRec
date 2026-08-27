//! Context-menu construction and preference-update signal bindings.

use super::track::artwork_visibility;
use super::{
    AlbumCoverSize, BackgroundStyle, NowPlayingSettings, NowPlayingWindow,
    TRANSITION_DURATION_DEFAULT_MS, TRANSITION_DURATION_MAX_MS, TRANSITION_DURATION_MIN_MS,
    TrackInfoAlignment, TransitionEffect, transition_duration_from_scale,
};
use crate::core::preferences::NowPlayingPreferenceChange;
use adw::prelude::*;
use gettextrs::gettext;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

const TRANSITION_DURATION_STEP_MS: f64 = 100.0;
const FULLSCREEN_CURSOR_HIDE_DELAY_MS: u64 = 1_500;

/// Owns one reschedulable GLib timeout.
///
/// Replacing the pending source keeps high-frequency UI events from leaving a
/// queue of stale callbacks behind merely to discover that they are obsolete.
#[derive(Clone, Default)]
struct DebouncedAction {
    source_id: Rc<RefCell<Option<glib::SourceId>>>,
}

impl DebouncedAction {
    fn schedule(&self, delay: Duration, action: impl FnOnce() + 'static) {
        self.cancel();

        let source_id_for_callback = self.source_id.clone();
        let source_id = glib::timeout_add_local_once(delay, move || {
            source_id_for_callback.borrow_mut().take();
            action();
        });
        self.source_id.borrow_mut().replace(source_id);
    }

    fn cancel(&self) {
        if let Some(source_id) = self.source_id.borrow_mut().take() {
            source_id.remove();
        }
    }
}

/// The context-menu controls whose state mirrors the active presentation settings.
pub(super) struct NowPlayingControls {
    pub(super) round_corners: gtk::Switch,
    pub(super) hide_track_info: gtk::Switch,
    pub(super) background_style_gradient: gtk::ToggleButton,
    pub(super) background_style_solid: gtk::ToggleButton,
    pub(super) track_info_alignment_left: gtk::ToggleButton,
    pub(super) track_info_alignment_center: gtk::ToggleButton,
    pub(super) album_cover_size: gtk::Scale,
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
    let album_cover_size = gtk::Scale::with_range(
        gtk::Orientation::Horizontal,
        AlbumCoverSize::MIN_SCALE_VALUE,
        AlbumCoverSize::MAX_SCALE_VALUE,
        AlbumCoverSize::SCALE_STEP,
    );
    AlbumCoverSize::configure_scale(&album_cover_size);
    AlbumCoverSize::install_slider_snap(&album_cover_size);
    album_cover_size.set_value(AlbumCoverSize::default().scale_value());
    album_cover_size.set_width_request(190);
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
        album_cover_size,
        always_display_last_recognized_song,
        transition_menu,
        transition_duration,
        lights_off_menu,
        fullscreen,
    }
}

impl NowPlayingWindow {
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
        reset_button.set_halign(gtk::Align::End);
        menu_grid.attach(&reset_button, 0, 0, 2, 1);
        let controller_for_reset = self.controller.clone();
        reset_button.connect_clicked(move |_| {
            controller_for_reset.reset();
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
        self.add_album_cover_size_menu_row(
            &menu_grid,
            4,
            settings.album_cover_size,
            !settings.lights_off,
        );
        self.add_background_style_menu_row(&menu_grid, 5, settings.background_style);
        self.add_switch_menu_row(
            &menu_grid,
            6,
            &gettext("Always display last recognized song"),
            &self.controls.always_display_last_recognized_song,
            settings.always_display_last_recognized_song,
            true,
        );
        self.add_transition_menu_row(
            &menu_grid,
            7,
            &self.controls.transition_menu,
            settings.transition,
        );
        self.add_transition_duration_menu_row(
            &menu_grid,
            8,
            &self.controls.transition_duration,
            settings.transition_duration_ms,
            !matches!(settings.transition, TransitionEffect::None),
        );
        self.add_switch_menu_row(
            &menu_grid,
            9,
            &gettext("Lights off"),
            &self.controls.lights_off_menu,
            settings.lights_off,
            true,
        );
        self.add_switch_menu_row(
            &menu_grid,
            10,
            &gettext("Full screen"),
            &self.controls.fullscreen,
            self.ui.window.is_fullscreen(),
            true,
        );

        popover.set_child(Some(&menu_grid));
        popover.set_parent(&self.ui.window);
        // Keep GTK's modal popover grab enabled. It normally handles outside
        // clicks by itself, while the capture-phase controller below covers
        // pointer sequences that were started by one of the interactive menu
        // controls (notably scales and dropdowns).
        popover.set_autohide(true);
        let popover_for_click = popover.downgrade();

        let gesture = gtk::GestureClick::new();
        gesture.set_button(3);
        gesture.connect_pressed(move |_, _, x, y| {
            let Some(popover) = popover_for_click.upgrade() else {
                return;
            };
            if popover.is_visible() {
                popover.popdown();
                return;
            }
            let pointing_rect = gdk::Rectangle::new(x as i32, y as i32, 1, 1);
            popover.set_pointing_to(Some(&pointing_rect));
            popover.popup();
        });
        self.ui.window.add_controller(gesture);

        let popover_for_outside_click = popover.downgrade();
        let outside_click = gtk::GestureClick::new();
        outside_click.set_button(1);
        outside_click.set_propagation_phase(gtk::PropagationPhase::Capture);
        outside_click.connect_pressed(move |gesture, _, x, y| {
            let Some(popover) = popover_for_outside_click.upgrade() else {
                return;
            };
            if !popover.is_visible() {
                return;
            }

            let clicked_inside = gesture
                .widget()
                .and_then(|window| {
                    window.compute_point(&popover, &gtk::graphene::Point::new(x as f32, y as f32))
                })
                .is_some_and(|point| popover.contains(f64::from(point.x()), f64::from(point.y())));

            if !clicked_inside {
                popover.popdown();
            }
        });
        self.ui.window.add_controller(outside_click);

        let window_for_fullscreen = self.ui.window.downgrade();
        self.controls
            .fullscreen
            .connect_active_notify(move |switch| {
                let Some(window) = window_for_fullscreen.upgrade() else {
                    return;
                };
                if switch.is_active() {
                    window.fullscreen();
                } else {
                    window.unfullscreen();
                }
            });

        let fullscreen_cursor_hide = DebouncedAction::default();
        let window_for_cursor_motion = self.ui.window.downgrade();
        let cursor_hide_for_motion = fullscreen_cursor_hide.clone();
        let cursor_motion = gtk::EventControllerMotion::new();
        cursor_motion.set_propagation_phase(gtk::PropagationPhase::Capture);
        cursor_motion.connect_motion(move |_, _, _| {
            if let Some(window) = window_for_cursor_motion.upgrade() {
                Self::reveal_fullscreen_cursor(&window, &cursor_hide_for_motion);
            }
        });
        let window_for_cursor_enter = self.ui.window.downgrade();
        let cursor_hide_for_enter = fullscreen_cursor_hide.clone();
        cursor_motion.connect_enter(move |_, _, _| {
            if let Some(window) = window_for_cursor_enter.upgrade() {
                Self::reveal_fullscreen_cursor(&window, &cursor_hide_for_enter);
            }
        });
        self.ui.window.add_controller(cursor_motion);

        let fullscreen_switch = self.controls.fullscreen.clone();
        let cursor_hide_for_fullscreen = fullscreen_cursor_hide;
        self.ui.window.connect_fullscreened_notify(move |window| {
            let fullscreened = window.is_fullscreen();
            cursor_hide_for_fullscreen.cancel();
            window.set_cursor_from_name(if fullscreened { Some("none") } else { None });
            if fullscreen_switch.is_active() != fullscreened {
                fullscreen_switch.set_active(fullscreened);
            }
        });
    }

    /// Temporarily reveals the cursor in fullscreen, then hides it after the pointer is idle.
    fn reveal_fullscreen_cursor(window: &gtk::Window, pending_hide: &DebouncedAction) {
        if !window.is_fullscreen() {
            pending_hide.cancel();
            return;
        }

        window.set_cursor_from_name(None);
        let window_for_cursor_timeout = window.downgrade();
        pending_hide.schedule(
            Duration::from_millis(FULLSCREEN_CURSOR_HIDE_DELAY_MS),
            move || {
                if let Some(window) = window_for_cursor_timeout.upgrade()
                    && window.is_fullscreen()
                {
                    window.set_cursor_from_name(Some("none"));
                }
            },
        );
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
        switch.set_halign(gtk::Align::End);
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
        dropdown.set_halign(gtk::Align::End);
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
        let label = gtk::Label::new(Some(&gettext("Transition duration (ms)")));
        label.set_halign(gtk::Align::Start);
        label.set_valign(gtk::Align::Center);
        menu_grid.attach(&label, 0, row, 1, 1);
        scale.set_value(duration_ms as f64);
        scale.set_sensitive(sensitive);
        scale.set_halign(gtk::Align::End);
        scale.set_valign(gtk::Align::Center);
        scale.set_hexpand(false);
        menu_grid.attach(scale, 1, row, 1, 1);
    }

    /// Adds the continuous album-cover-size slider above the background-style control.
    fn add_album_cover_size_menu_row(
        &self,
        menu_grid: &gtk::Grid,
        row: i32,
        size: AlbumCoverSize,
        sensitive: bool,
    ) {
        let label = gtk::Label::new(Some(&gettext("Album cover size")));
        label.set_halign(gtk::Align::Start);
        label.set_valign(gtk::Align::Center);
        menu_grid.attach(&label, 0, row, 1, 1);
        self.controls.album_cover_size.set_value(size.scale_value());
        self.controls.album_cover_size.set_sensitive(sensitive);
        self.controls.album_cover_size.set_halign(gtk::Align::End);
        self.controls
            .album_cover_size
            .set_valign(gtk::Align::Center);
        self.controls.album_cover_size.set_hexpand(false);
        menu_grid.attach(&self.controls.album_cover_size, 1, row, 1, 1);
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
            .halign(gtk::Align::End)
            .valign(gtk::Align::Center)
            .build();
        buttons.append(&self.controls.track_info_alignment_left);
        buttons.append(&self.controls.track_info_alignment_center);
        // Keep sensitivity on the retained controls themselves. If this local
        // container is disabled, later preference updates cannot effectively
        // re-enable its children because the container is not retained.
        self.controls
            .track_info_alignment_left
            .set_sensitive(sensitive);
        self.controls
            .track_info_alignment_center
            .set_sensitive(sensitive);
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
            .halign(gtk::Align::End)
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
        let controller_for_round_corners = self.controller.clone();
        let artwork_overlay_for_round_corners = self.ui.artwork_overlay.clone();
        self.controls
            .round_corners
            .connect_active_notify(move |switch| {
                if applying_settings_for_round_corners.get() {
                    return;
                }

                let active = switch.is_active();
                controller_for_round_corners
                    .update(NowPlayingPreferenceChange::RoundCorners(active));
                if active {
                    artwork_overlay_for_round_corners.add_css_class("now-playing-artwork-rounded");
                } else {
                    artwork_overlay_for_round_corners
                        .remove_css_class("now-playing-artwork-rounded");
                }
            });

        let applying_settings_for_hide = self.state.applying_settings.clone();
        let controller_for_hide = self.controller.clone();
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
                controller_for_hide
                    .update(NowPlayingPreferenceChange::HideTrackInfo(hide_track_info));
                info_box_for_hide.set_visible(!hide_track_info);
                alignment_left_for_hide.set_sensitive(!hide_track_info);
                alignment_center_for_hide.set_sensitive(!hide_track_info);
            });

        let applying_settings_for_alignment_left = self.state.applying_settings.clone();
        let controller_for_alignment_left = self.controller.clone();
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

                controller_for_alignment_left.update(
                    NowPlayingPreferenceChange::TrackInfoAlignment(TrackInfoAlignment::Left),
                );
                apply_track_info_alignment(
                    &info_box_for_alignment_left,
                    &title_for_alignment_left,
                    &artist_for_alignment_left,
                    &album_for_alignment_left,
                    &details_for_alignment_left,
                    TrackInfoAlignment::Left,
                );
            });

        let applying_settings_for_alignment_center = self.state.applying_settings.clone();
        let controller_for_alignment_center = self.controller.clone();
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

                controller_for_alignment_center.update(
                    NowPlayingPreferenceChange::TrackInfoAlignment(TrackInfoAlignment::Center),
                );
                apply_track_info_alignment(
                    &info_box_for_alignment_center,
                    &title_for_alignment_center,
                    &artist_for_alignment_center,
                    &album_for_alignment_center,
                    &details_for_alignment_center,
                    TrackInfoAlignment::Center,
                );
            });

        let applying_settings_for_album_cover_size = self.state.applying_settings.clone();
        let album_cover_layout = self.ui.album_cover_layout.clone();
        let controller_for_album_cover_size = self.controller.clone();
        self.controls
            .album_cover_size
            .connect_value_changed(move |scale| {
                if applying_settings_for_album_cover_size.get() {
                    return;
                }

                let size = AlbumCoverSize::from_scale_value(scale.value());
                album_cover_layout.set_size(size);
                controller_for_album_cover_size
                    .update_debounced(NowPlayingPreferenceChange::AlbumCoverSize(size));
            });

        let applying_settings_for_always_display_last = self.state.applying_settings.clone();
        let controller_for_always_display_last = self.controller.clone();
        self.controls
            .always_display_last_recognized_song
            .connect_active_notify(move |button| {
                if applying_settings_for_always_display_last.get() {
                    return;
                }

                let always_display_last_recognized_song = button.is_active();
                controller_for_always_display_last.update(
                    NowPlayingPreferenceChange::AlwaysDisplayLastRecognizedSong(
                        always_display_last_recognized_song,
                    ),
                );
            });

        let applying_settings_for_transition = self.state.applying_settings.clone();
        let controller_for_transition = self.controller.clone();
        let transition_duration_control = self.controls.transition_duration.clone();
        self.controls
            .transition_menu
            .connect_selected_notify(move |dropdown| {
                if applying_settings_for_transition.get() {
                    return;
                }

                let effect = TransitionEffect::from_index(dropdown.selected());
                controller_for_transition.update(NowPlayingPreferenceChange::Transition(effect));
                transition_duration_control
                    .set_sensitive(!matches!(effect, TransitionEffect::None));
            });

        let applying_settings_for_duration = self.state.applying_settings.clone();
        let controller_for_transition_duration = self.controller.clone();
        self.controls
            .transition_duration
            .connect_value_changed(move |scale| {
                if applying_settings_for_duration.get() {
                    return;
                }

                let duration_ms = transition_duration_from_scale(scale.value());
                controller_for_transition_duration.update_debounced(
                    NowPlayingPreferenceChange::TransitionDurationMs(duration_ms),
                );
            });

        let applying_settings_for_lights = self.state.applying_settings.clone();
        let controller_for_lights = self.controller.clone();
        let track_state_for_lights = self.state.track_presentation.clone();
        let round_for_lights_menu = self.controls.round_corners.clone();
        let hide_for_lights_menu = self.controls.hide_track_info.clone();
        let alignment_left_for_lights_menu = self.controls.track_info_alignment_left.clone();
        let alignment_center_for_lights_menu = self.controls.track_info_alignment_center.clone();
        let info_box_for_lights = self.ui.info_box.clone();
        let album_cover_size_for_lights = self.controls.album_cover_size.clone();
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
                controller_for_lights.update(NowPlayingPreferenceChange::LightsOff(active));
                let settings = controller_for_lights.settings();
                round_for_lights_menu.set_sensitive(!active);
                hide_for_lights_menu.set_sensitive(!active);
                album_cover_size_for_lights.set_sensitive(!active);
                alignment_left_for_lights_menu.set_sensitive(!settings.hide_track_info);
                alignment_center_for_lights_menu.set_sensitive(!settings.hide_track_info);

                if active {
                    let was_applying_settings = applying_settings_for_lights.replace(true);
                    hide_for_lights_menu.set_active(false);
                    applying_settings_for_lights.set(was_applying_settings);
                    info_box_for_lights.set_visible(true);
                }

                let mode = track_state_for_lights.borrow().mode;
                let (show_artwork, show_listening) = artwork_visibility(mode, active);
                artwork_for_lights.set_visible(show_artwork);
                artwork_placeholder_for_lights.set_visible(show_listening);
                background_area_for_lights.queue_draw();
            });

        let applying_settings_for_gradient = self.state.applying_settings.clone();
        let controller_for_gradient = self.controller.clone();
        let background_area_for_gradient = self.ui.background_area.clone();
        self.controls
            .background_style_gradient
            .connect_toggled(move |button| {
                if applying_settings_for_gradient.get() || !button.is_active() {
                    return;
                }

                controller_for_gradient.update(NowPlayingPreferenceChange::BackgroundStyle(
                    BackgroundStyle::Gradient,
                ));
                background_area_for_gradient.queue_draw();
            });

        let applying_settings_for_solid = self.state.applying_settings.clone();
        let controller_for_solid = self.controller.clone();
        let background_area_for_solid = self.ui.background_area.clone();
        self.controls
            .background_style_solid
            .connect_toggled(move |button| {
                if applying_settings_for_solid.get() || !button.is_active() {
                    return;
                }

                controller_for_solid.update(NowPlayingPreferenceChange::BackgroundStyle(
                    BackgroundStyle::Solid,
                ));
                background_area_for_solid.queue_draw();
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
