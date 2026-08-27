//! Applying persisted Now Playing preferences to the active window.

use super::{
    AlbumCoverSize, NowPlayingSettings, NowPlayingWindow, TrackInfoAlignment, TransitionEffect,
    clamp_transition_duration_ms,
};
use adw::prelude::*;

impl NowPlayingWindow {
    /// Applies the initial Now Playing preferences to the newly created window.
    pub(super) fn apply_initial_preferences(&self, settings: NowPlayingSettings) {
        self.apply_settings(settings);
        self.set_listening_state();
    }

    /// Reapplies the shared model to every control and renderer.
    pub(crate) fn refresh_from_controller(&self) {
        self.apply_settings(self.controller.settings());
    }

    /// Applies one resolved settings snapshot without treating control notifications as user input.
    fn apply_settings(&self, settings: NowPlayingSettings) {
        self.with_preference_updates_suspended(|| {
            self.state.settings.set(settings);

            self.set_round_corners(settings.round_corners);
            self.set_show_track_info(!settings.hide_track_info);
            self.set_track_info_alignment(settings.track_info_alignment);
            self.set_album_cover_size(settings.album_cover_size);
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

    /// Selects and applies the transition effect used when a new track replaces the current one.
    pub(super) fn set_transition(&self, effect: TransitionEffect) {
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
    pub(super) fn set_transition_duration(&self, duration_ms: u64) {
        let duration_ms = clamp_transition_duration_ms(duration_ms);
        self.with_preference_updates_suspended(|| {
            if self.controls.transition_duration.value() != duration_ms as f64 {
                self.controls
                    .transition_duration
                    .set_value(duration_ms as f64);
            }
        });
    }

    /// Enables or disables keeping the last recognized song when no new match is available.
    pub(super) fn set_always_display_last_recognized_song(&self, enabled: bool) {
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
    pub(super) fn set_round_corners(&self, enabled: bool) {
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
    pub(super) fn set_track_info_alignment(&self, alignment: TrackInfoAlignment) {
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

    /// Sets the constrained artwork size and synchronizes the context-menu slider.
    pub(super) fn set_album_cover_size(&self, size: AlbumCoverSize) {
        self.ui.album_cover_layout.set_size(size);
        self.with_preference_updates_suspended(|| {
            if self.controls.album_cover_size.value() != size.scale_value() {
                self.controls.album_cover_size.set_value(size.scale_value());
            }
        });
    }

    /// Shows or hides the metadata block for the current track.
    pub(super) fn set_show_track_info(&self, show: bool) {
        let hide_track_info = !show;
        self.with_preference_updates_suspended(|| {
            if self.controls.hide_track_info.is_active() != hide_track_info {
                self.controls.hide_track_info.set_active(hide_track_info);
            }
            self.ui.info_box.set_visible(show);
            self.controls.track_info_alignment_left.set_sensitive(show);
            self.controls
                .track_info_alignment_center
                .set_sensitive(show);
        });
    }
}
