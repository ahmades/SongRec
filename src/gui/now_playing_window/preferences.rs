//! Applying persisted Now Playing preferences to the active window.

use super::{
    NowPlayingSettings, NowPlayingWindow, TrackInfoAlignment, TransitionEffect,
    clamp_transition_duration_ms, reconcile_transition_duration,
};
use crate::core::preferences::Preferences;
use adw::prelude::*;

impl NowPlayingWindow {
    /// Applies the initial Now Playing preferences to the newly created window.
    pub(super) fn apply_initial_preferences(&self, settings: NowPlayingSettings) {
        self.apply_settings(settings);
        self.set_listening_state();
    }

    /// Refreshes all Now Playing controls and rendering state from persisted preferences.
    pub fn refresh_from_preferences(&self, preferences: &Preferences) {
        self.apply_settings(NowPlayingSettings::from(preferences));
    }

    /// Applies one resolved settings snapshot without treating control notifications as user input.
    fn apply_settings(&self, mut settings: NowPlayingSettings) {
        let pending_duration_ms = self.state.pending_transition_duration_ms.get();
        let (duration_ms, pending_duration_ms_after_refresh) =
            reconcile_transition_duration(settings.transition_duration_ms, pending_duration_ms);
        settings.transition_duration_ms = duration_ms;

        if pending_duration_ms_after_refresh != pending_duration_ms {
            self.state
                .pending_transition_duration_ms
                .set(pending_duration_ms_after_refresh);
            self.state.transition_duration_update_generation.set(
                self.state
                    .transition_duration_update_generation
                    .get()
                    .wrapping_add(1),
            );
        }

        self.with_preference_updates_suspended(|| {
            self.state.settings.set(settings);

            self.set_round_corners(settings.round_corners);
            self.set_show_track_info(!settings.hide_track_info);
            self.set_track_info_alignment(settings.track_info_alignment);
            self.set_always_display_last_recognized_song(
                settings.always_display_last_recognized_song,
            );
            self.set_transition(settings.transition);
            self.set_transition_duration(settings.transition_duration_ms);
            self.set_lights_off(settings.lights_off);
            self.set_background_style(settings.background_style);
        });
    }

    /// Runs a control update while suppressing feedback through GTK signal handlers.
    pub(super) fn with_preference_updates_suspended(&self, update: impl FnOnce()) {
        let was_applying_settings = self.state.applying_settings.replace(true);
        update();
        self.state.applying_settings.set(was_applying_settings);
    }

    /// Updates the typed settings snapshot while retaining the rest of its values.
    pub(super) fn update_settings(&self, update: impl FnOnce(&mut NowPlayingSettings)) {
        let mut settings = self.state.settings.get();
        update(&mut settings);
        self.state.settings.set(settings);
    }

    /// Selects and applies the transition effect used when a new track replaces the current one.
    pub fn set_transition(&self, effect: TransitionEffect) {
        self.update_settings(|settings| settings.transition = effect);
        self.state.transition.set(effect);
        self.with_preference_updates_suspended(|| {
            if self.controls.transition_menu.selected() != effect.index() {
                self.controls.transition_menu.set_selected(effect.index());
            }
            self.controls
                .transition_duration
                .set_sensitive(!matches!(effect, TransitionEffect::None));
        });
    }

    /// Sets the transition duration in milliseconds and updates the duration control.
    pub fn set_transition_duration(&self, duration_ms: u64) {
        let duration_ms = clamp_transition_duration_ms(duration_ms);
        self.update_settings(|settings| settings.transition_duration_ms = duration_ms);
        self.state.transition_duration_ms.set(duration_ms);
        self.with_preference_updates_suspended(|| {
            if self.controls.transition_duration.value() != duration_ms as f64 {
                self.controls
                    .transition_duration
                    .set_value(duration_ms as f64);
            }
        });
    }

    /// Enables or disables keeping the last recognized song when no new match is available.
    pub fn set_always_display_last_recognized_song(&self, enabled: bool) {
        self.update_settings(|settings| settings.always_display_last_recognized_song = enabled);
        self.with_preference_updates_suspended(|| {
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
        });
    }

    /// Enables or disables rounded corners on the album-art overlay.
    pub fn set_round_corners(&self, enabled: bool) {
        self.update_settings(|settings| settings.round_corners = enabled);
        self.with_preference_updates_suspended(|| {
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
        });
    }

    /// Applies the requested alignment to the metadata block and its labels.
    pub fn set_track_info_alignment(&self, alignment: TrackInfoAlignment) {
        self.update_settings(|settings| settings.track_info_alignment = alignment);
        self.with_preference_updates_suspended(|| match alignment {
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
        });
    }

    /// Shows or hides the metadata block for the current track.
    pub fn set_show_track_info(&self, show: bool) {
        let hide_track_info = !show;
        self.update_settings(|settings| settings.hide_track_info = hide_track_info);
        self.with_preference_updates_suspended(|| {
            if self.controls.hide_track_info.is_active() != hide_track_info {
                self.controls.hide_track_info.set_active(hide_track_info);
            }
            self.ui.info_box.set_visible(show);
        });
    }
}
