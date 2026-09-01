//! Applying persisted Now Playing preferences to the active window.

use super::track::TrackPresentation;
use super::ui::apply_classic_track_info_alignment;
use super::{
    AlbumCoverSize, DisplayMode, NowPlayingSettings, NowPlayingWindow, TrackInfoAlignment,
    TransitionEffect, clamp_background_motion_zoom_percent, clamp_transition_duration_ms,
    normalize_background_motion_reversal_duration_secs,
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

            self.set_display_mode(settings.display_mode);
            self.set_round_corners(settings.classic.round_corners);
            self.set_show_track_info(!settings.shared.hide_track_info);
            self.set_background_motion(
                settings.shared.background_motion_enabled,
                settings.shared.background_motion_zoom_percent,
                settings.shared.background_motion_reversal_duration_secs,
            );
            self.set_track_info_alignment(settings.classic.track_info_alignment);
            self.set_album_cover_size(settings.classic.album_cover_size);
            self.set_always_display_last_recognized_song(
                settings.shared.always_display_last_recognized_song,
            );
            self.set_transition(settings.shared.transition);
            self.set_transition_duration(settings.shared.transition_duration_ms);
            self.set_background_style(settings.classic.background_style);
        });
    }

    /// Runs a control update while suppressing feedback through GTK signal handlers.
    pub(super) fn with_preference_updates_suspended(&self, update: impl FnOnce()) {
        let was_applying_settings = self.state.applying_settings.replace(true);
        update();
        self.state.applying_settings.set(was_applying_settings);
    }

    /// Selects the overall presentation while retaining every mode's stored settings.
    pub(super) fn set_display_mode(&self, display_mode: DisplayMode) {
        self.with_preference_updates_suspended(|| {
            let selected = display_mode.index();
            if self.controls.display_mode_menu.selected() != selected {
                self.controls.display_mode_menu.set_selected(selected);
            }
            self.controls
                .classic_settings
                .set_visible(display_mode.shows_classic_settings());
            self.controls
                .hide_track_info_label
                .set_visible(display_mode.supports_hiding_track_info());
            self.controls
                .hide_track_info
                .set_visible(display_mode.supports_hiding_track_info());
            self.update_background_motion_control_visibility(
                display_mode,
                self.controls.background_motion_enabled.is_active(),
            );
        });
        TrackPresentation::from_window(self).refresh_mode();
    }

    /// Synchronizes the shared immersive-background motion controls and renderer.
    pub(super) fn set_background_motion(
        &self,
        enabled: bool,
        zoom_percent: u16,
        reversal_duration_secs: u64,
    ) {
        let zoom_percent = clamp_background_motion_zoom_percent(zoom_percent);
        let reversal_duration_secs =
            normalize_background_motion_reversal_duration_secs(reversal_duration_secs);
        self.with_preference_updates_suspended(|| {
            if self.controls.background_motion_enabled.is_active() != enabled {
                self.controls.background_motion_enabled.set_active(enabled);
            }
            if self.controls.background_motion_zoom.value() != f64::from(zoom_percent) {
                self.controls
                    .background_motion_zoom
                    .set_value(f64::from(zoom_percent));
            }
            if self.controls.background_motion_reversal_duration.value()
                != reversal_duration_secs as f64
            {
                self.controls
                    .background_motion_reversal_duration
                    .set_value(reversal_duration_secs as f64);
            }
            self.update_background_motion_control_visibility(
                self.state.settings.get().display_mode,
                enabled,
            );
        });
        TrackPresentation::from_window(self).refresh_mode();
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
        self.with_preference_updates_suspended(|| {
            apply_classic_track_info_alignment(
                &self.ui.info_box,
                [
                    &self.ui.title_label,
                    &self.ui.artist_label,
                    &self.ui.album_label,
                    &self.ui.details_label,
                ],
                alignment,
            );

            let selected = match alignment {
                TrackInfoAlignment::Left => &self.controls.track_info_alignment_left,
                TrackInfoAlignment::Center => &self.controls.track_info_alignment_center,
                TrackInfoAlignment::Right => &self.controls.track_info_alignment_right,
            };
            if !selected.is_active() {
                selected.set_active(true);
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
            self.controls.track_info_alignment_right.set_sensitive(show);
        });
        TrackPresentation::from_window(self).refresh_mode();
    }
}
