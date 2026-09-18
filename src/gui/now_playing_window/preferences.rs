//! Applying persisted Now Playing preferences to the active window.

use super::display_mode::NowPlayingControlState;
use super::track::TrackPresentation;
use super::ui::apply_classic_track_info_alignment;
use super::{
    AlbumCoverSize, BackdropIntensity, CinemaArtworkFraming, CinemaCropFocus, DisplayMode,
    ImmersiveBackgroundSource, NowPlayingSettings, NowPlayingWindow, TextSize, TrackInfoAlignment,
    TransitionEffect, clamp_background_motion_zoom_percent, clamp_transition_duration_ms,
    normalize_background_motion_reversal_duration_secs, normalize_burn_in_inactivity_minutes,
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
        // Controller messages originate in an explicit settings interaction,
        // including loading a preset whose values already match the window.
        self.burn_in.activity();
        self.apply_settings(self.controller.settings());
    }

    /// Applies one resolved settings snapshot without treating control notifications as user input.
    fn apply_settings(&self, settings: NowPlayingSettings) {
        let previous = self.applied_settings.replace(Some(settings));
        if previous == Some(settings) {
            return;
        }
        // The controller's shared cell already holds the new values, so compare
        // against the snapshot actually applied to these widgets instead.
        macro_rules! changed {
            ($($field:ident).+) => {
                previous.is_none_or(|old| old.$($field).+ != settings.$($field).+)
            };
        }
        self.with_preference_updates_suspended(|| {
            self.state.settings.set(settings);

            if changed!(display_mode) {
                self.set_display_mode(settings.display_mode);
            }
            if changed!(shared.keep_screen_awake) {
                self.set_keep_screen_awake(settings.shared.keep_screen_awake);
            }
            if changed!(shared.burn_in_protection_enabled)
                || changed!(shared.burn_in_inactivity_minutes)
            {
                self.set_burn_in_protection(
                    settings.shared.burn_in_protection_enabled,
                    settings.shared.burn_in_inactivity_minutes,
                );
            }
            if changed!(classic.round_corners) {
                self.set_round_corners(settings.classic.round_corners);
            }
            if changed!(cinema.artwork_framing) || changed!(cinema.crop_focus) {
                self.set_cinema_artwork_framing(
                    settings.cinema.artwork_framing,
                    settings.cinema.crop_focus,
                );
            }
            if changed!(shared.hide_track_info) {
                self.set_show_track_info(!settings.shared.hide_track_info);
            }
            if changed!(shared.text_size) {
                self.set_text_size(settings.shared.text_size);
            }
            if changed!(shared.immersive_background_source) {
                self.set_immersive_background_source(settings.shared.immersive_background_source);
            }
            if changed!(shared.backdrop_intensity) {
                self.set_backdrop_intensity(settings.shared.backdrop_intensity);
            }
            if changed!(shared.background_motion_enabled)
                || changed!(shared.background_motion_zoom_percent)
                || changed!(shared.background_motion_reversal_duration_secs)
            {
                self.set_background_motion(
                    settings.shared.background_motion_enabled,
                    settings.shared.background_motion_zoom_percent,
                    settings.shared.background_motion_reversal_duration_secs,
                );
            }
            if changed!(classic.track_info_alignment) {
                self.set_track_info_alignment(settings.classic.track_info_alignment);
            }
            if changed!(classic.album_cover_size) {
                self.set_album_cover_size(settings.classic.album_cover_size);
            }
            if changed!(shared.always_display_last_recognized_song) {
                self.set_always_display_last_recognized_song(
                    settings.shared.always_display_last_recognized_song,
                );
            }
            if changed!(shared.transition) {
                self.set_transition(settings.shared.transition);
            }
            if changed!(shared.transition_duration_ms) {
                self.set_transition_duration(settings.shared.transition_duration_ms);
            }
            if changed!(classic.background_style) {
                self.set_background_style(settings.classic.background_style);
            }
            self.apply_control_state(NowPlayingControlState::from_settings(settings));
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
            self.controls.display_mode.set_value(display_mode);
        });
        self.refresh_track_for_visual_change();
        self.reconcile_pending_transition();
        self.resume_artwork_preparation();
        self.ensure_artist_background();
    }

    /// Applies Cinema foreground fitting and crop focus to the retained texture.
    pub(super) fn set_cinema_artwork_framing(
        &self,
        framing: CinemaArtworkFraming,
        crop_focus: CinemaCropFocus,
    ) {
        self.with_preference_updates_suspended(|| {
            self.controls.cinema_artwork_framing.set_value(framing);
            self.controls.cinema_crop_focus.set_value(crop_focus);
        });
        self.ui.cinema_artwork.set_artwork_framing(framing);
        self.ui.cinema_artwork.set_crop_focus(crop_focus);
    }

    /// Keeps the desktop session active only while this window is viewable.
    pub(super) fn set_keep_screen_awake(&self, enabled: bool) {
        self.with_preference_updates_suspended(|| {
            if self.controls.keep_screen_awake.is_active() != enabled {
                self.controls.keep_screen_awake.set_active(enabled);
            }
        });
        self.screen_awake.set_enabled(enabled);
    }

    /// Configures dim-only burn-in protection and mirrors it in the context menu.
    pub(super) fn set_burn_in_protection(&self, enabled: bool, inactivity_minutes: u16) {
        let inactivity_minutes = normalize_burn_in_inactivity_minutes(inactivity_minutes);
        self.with_preference_updates_suspended(|| {
            if self.controls.burn_in_protection.is_active() != enabled {
                self.controls.burn_in_protection.set_active(enabled);
            }
            if self.controls.burn_in_inactivity_minutes.value_as_int()
                != i32::from(inactivity_minutes)
            {
                self.controls
                    .burn_in_inactivity_minutes
                    .set_value(f64::from(inactivity_minutes));
            }
        });
        self.burn_in.configure(enabled, inactivity_minutes);
    }

    /// Selects the image that supplies the blurred Cinema/Ambient backdrop.
    pub(super) fn set_immersive_background_source(&self, source: ImmersiveBackgroundSource) {
        self.with_preference_updates_suspended(|| {
            let selected = match source {
                ImmersiveBackgroundSource::AlbumCover => {
                    &self.controls.immersive_background_source_album_cover
                }
                ImmersiveBackgroundSource::Artist => {
                    &self.controls.immersive_background_source_artist
                }
            };
            if !selected.is_active() {
                selected.set_active(true);
            }
        });
        self.refresh_track_for_visual_change();
        self.reconcile_pending_transition();
        self.resume_artwork_preparation();
        self.ensure_artist_background();
    }

    /// Applies the shared Cinema/Ambient backdrop treatment.
    pub(super) fn set_backdrop_intensity(&self, intensity: BackdropIntensity) {
        self.with_preference_updates_suspended(|| {
            self.sync_backdrop_intensity_controls(intensity);
        });

        // Keep an active hide/reveal leg immutable. Otherwise the cheap scrim
        // update can apply immediately while the previous exact-profile image
        // remains visible until worker preparation completes.
        let redraw_deferred = self
            .state
            .track_presentation
            .borrow_mut()
            .defer_scene_refresh_if_animating();
        if !redraw_deferred {
            self.apply_background();
        }
        self.reconcile_pending_transition();
        self.resume_artwork_preparation();
        self.ensure_artist_background();
    }

    /// Rebuilds a mode/source-dependent scene without mutating an animated leg.
    fn refresh_track_for_visual_change(&self) {
        let refresh_deferred = self
            .state
            .track_presentation
            .borrow_mut()
            .defer_scene_refresh_if_animating();
        if !refresh_deferred {
            TrackPresentation::from_window(self).refresh_current_track();
        }
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
        });
        TrackPresentation::from_window(self).refresh_mode();
    }

    /// Selects and applies the transition effect used when a new track replaces the current one.
    pub(super) fn set_transition(&self, effect: TransitionEffect) {
        self.with_preference_updates_suspended(|| {
            if self.controls.transition_menu.selected() != effect.index() {
                self.controls.transition_menu.set_selected(effect.index());
            }
        });
        self.reconcile_pending_transition();
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

    /// Applies the track-info scale and synchronizes the context-menu slider.
    pub(super) fn set_text_size(&self, size: TextSize) {
        self.refresh_text_css(size);
        self.with_preference_updates_suspended(|| {
            if self.controls.text_size.value() != size.scale_value() {
                self.controls.text_size.set_value(size.scale_value());
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
            self.ui.classic_info_layout.set_visible(show);
        });
        TrackPresentation::from_window(self).refresh_mode();
    }
}
