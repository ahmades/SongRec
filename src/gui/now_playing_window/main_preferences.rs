//! Bindings for the Now Playing section of the main preferences page.

use super::{
    AlbumCoverSize, BACKGROUND_MOTION_REVERSAL_DURATION_DEFAULT_SECS,
    BACKGROUND_MOTION_REVERSAL_DURATION_MAX_SECS, BACKGROUND_MOTION_REVERSAL_DURATION_MIN_SECS,
    BACKGROUND_MOTION_REVERSAL_DURATION_STEP_SECS, BACKGROUND_MOTION_ZOOM_DEFAULT_PERCENT,
    BACKGROUND_MOTION_ZOOM_MAX_PERCENT, BACKGROUND_MOTION_ZOOM_MIN_PERCENT,
    BACKGROUND_MOTION_ZOOM_STEP_PERCENT, BackgroundStyle, NowPlayingSettings, SettingsController,
    TextSize, TrackInfoAlignment, TransitionEffect, transition_duration_from_scale,
};
use crate::core::preferences::{DisplayMode, NowPlayingPreferenceChange};
use adw::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone)]
struct PreferencesWidgets {
    reset: gtk::Button,
    display_mode: adw::ComboRow,
    classic_settings: adw::PreferencesGroup,
    background_motion_settings: adw::PreferencesGroup,
    round_corners: adw::SwitchRow,
    hide_track_info: adw::SwitchRow,
    text_size_row: adw::ActionRow,
    text_size: gtk::Scale,
    background_motion_enabled: adw::SwitchRow,
    background_motion_zoom_row: adw::ActionRow,
    background_motion_zoom: gtk::Scale,
    background_motion_reversal_duration_row: adw::ActionRow,
    background_motion_reversal_duration: gtk::Scale,
    track_info_alignment: adw::ActionRow,
    track_info_alignment_left: gtk::ToggleButton,
    track_info_alignment_center: gtk::ToggleButton,
    track_info_alignment_right: gtk::ToggleButton,
    album_cover_size: gtk::Scale,
    background_style_gradient: gtk::ToggleButton,
    background_style_solid: gtk::ToggleButton,
    always_display_last_recognized_song: adw::SwitchRow,
    transition: adw::ComboRow,
    transition_duration: gtk::Scale,
}

/// Owns the widgets and signal bindings for the main Now Playing preferences.
pub(crate) struct NowPlayingPreferencesView {
    widgets: PreferencesWidgets,
    applying: Rc<Cell<bool>>,
}

impl NowPlayingPreferencesView {
    pub(crate) fn new(builder: &gtk::Builder, controller: SettingsController) -> Self {
        let widgets = PreferencesWidgets {
            reset: builder
                .object("reset_now_playing_preferences_button")
                .unwrap(),
            display_mode: builder.object("display_mode_setting").unwrap(),
            classic_settings: builder.object("classic_now_playing_preferences").unwrap(),
            background_motion_settings: builder.object("background_motion_preferences").unwrap(),
            round_corners: builder.object("round_corners_setting").unwrap(),
            hide_track_info: builder.object("hide_track_info_setting").unwrap(),
            text_size_row: builder.object("text_size_setting").unwrap(),
            text_size: builder.object("text_size_setting_scale").unwrap(),
            background_motion_enabled: builder.object("background_motion_enabled_setting").unwrap(),
            background_motion_zoom_row: builder.object("background_motion_zoom_setting").unwrap(),
            background_motion_zoom: builder
                .object("background_motion_zoom_setting_scale")
                .unwrap(),
            background_motion_reversal_duration_row: builder
                .object("background_motion_reversal_duration_setting")
                .unwrap(),
            background_motion_reversal_duration: builder
                .object("background_motion_reversal_duration_setting_scale")
                .unwrap(),
            track_info_alignment: builder.object("track_info_alignment_setting").unwrap(),
            track_info_alignment_left: builder.object("track_info_alignment_left").unwrap(),
            track_info_alignment_center: builder.object("track_info_alignment_center").unwrap(),
            track_info_alignment_right: builder.object("track_info_alignment_right").unwrap(),
            album_cover_size: builder.object("album_cover_size_setting_scale").unwrap(),
            background_style_gradient: builder.object("background_style_gradient").unwrap(),
            background_style_solid: builder.object("background_style_solid").unwrap(),
            always_display_last_recognized_song: builder
                .object("always_display_last_recognized_song_setting")
                .unwrap(),
            transition: builder.object("transition_setting").unwrap(),
            transition_duration: builder.object("transition_duration_setting_scale").unwrap(),
        };

        AlbumCoverSize::configure_scale(&widgets.album_cover_size);
        AlbumCoverSize::install_slider_snap(&widgets.album_cover_size);
        TextSize::configure_scale(&widgets.text_size);
        TextSize::install_slider_snap(&widgets.text_size);
        configure_integer_scale(
            &widgets.background_motion_zoom,
            f64::from(BACKGROUND_MOTION_ZOOM_DEFAULT_PERCENT),
            f64::from(BACKGROUND_MOTION_ZOOM_MIN_PERCENT),
            f64::from(BACKGROUND_MOTION_ZOOM_MAX_PERCENT),
            f64::from(BACKGROUND_MOTION_ZOOM_STEP_PERCENT),
        );
        configure_integer_scale(
            &widgets.background_motion_reversal_duration,
            BACKGROUND_MOTION_REVERSAL_DURATION_DEFAULT_SECS as f64,
            BACKGROUND_MOTION_REVERSAL_DURATION_MIN_SECS as f64,
            BACKGROUND_MOTION_REVERSAL_DURATION_MAX_SECS as f64,
            BACKGROUND_MOTION_REVERSAL_DURATION_STEP_SECS as f64,
        );

        let display_mode_labels = DisplayMode::ALL
            .into_iter()
            .map(DisplayMode::translated_label)
            .collect::<Vec<_>>();
        let display_mode_label_references = display_mode_labels
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        widgets
            .display_mode
            .set_model(Some(&gtk::StringList::new(&display_mode_label_references)));

        let transition_labels = TransitionEffect::ALL
            .into_iter()
            .map(TransitionEffect::translated_label)
            .collect::<Vec<_>>();
        let transition_label_references = transition_labels
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        widgets
            .transition
            .set_model(Some(&gtk::StringList::new(&transition_label_references)));

        let view = Self {
            widgets,
            applying: Rc::new(Cell::new(false)),
        };
        view.apply(controller.settings());
        view.connect_handlers(controller);
        view
    }

    /// Applies a complete settings snapshot without treating widget changes as user input.
    pub(crate) fn apply(&self, settings: NowPlayingSettings) {
        let was_applying = self.applying.replace(true);

        self.widgets
            .display_mode
            .set_selected(settings.display_mode.index());
        self.widgets
            .classic_settings
            .set_visible(settings.display_mode.shows_classic_settings());
        self.widgets
            .round_corners
            .set_active(settings.classic.round_corners);
        self.widgets
            .hide_track_info
            .set_active(settings.shared.hide_track_info);
        self.widgets
            .text_size
            .set_value(settings.shared.text_size.scale_value());
        self.widgets
            .background_motion_enabled
            .set_active(settings.shared.background_motion_enabled);
        self.widgets
            .background_motion_zoom
            .set_value(settings.shared.background_motion_zoom_percent as f64);
        self.widgets
            .background_motion_reversal_duration
            .set_value(settings.shared.background_motion_reversal_duration_secs as f64);
        self.widgets.track_info_alignment_left.set_active(matches!(
            settings.classic.track_info_alignment,
            TrackInfoAlignment::Left
        ));
        self.widgets
            .track_info_alignment_center
            .set_active(matches!(
                settings.classic.track_info_alignment,
                TrackInfoAlignment::Center
            ));
        self.widgets.track_info_alignment_right.set_active(matches!(
            settings.classic.track_info_alignment,
            TrackInfoAlignment::Right
        ));
        self.widgets
            .album_cover_size
            .set_value(settings.classic.album_cover_size.scale_value());
        self.widgets.background_style_gradient.set_active(matches!(
            settings.classic.background_style,
            BackgroundStyle::Gradient
        ));
        self.widgets.background_style_solid.set_active(matches!(
            settings.classic.background_style,
            BackgroundStyle::Solid
        ));
        self.widgets
            .always_display_last_recognized_song
            .set_active(settings.shared.always_display_last_recognized_song);
        self.widgets
            .transition
            .set_selected(settings.shared.transition.index());
        self.widgets
            .transition_duration
            .set_value(settings.shared.transition_duration_ms as f64);

        self.apply_control_state(settings);
        self.applying.set(was_applying);
    }

    fn apply_control_state(&self, settings: NowPlayingSettings) {
        self.widgets
            .hide_track_info
            .set_visible(settings.display_mode.supports_hiding_track_info());
        self.widgets.text_size_row.set_visible(
            settings
                .display_mode
                .shows_track_info(settings.shared.hide_track_info),
        );
        let supports_background_motion = settings.display_mode.supports_background_motion();
        self.widgets
            .background_motion_settings
            .set_visible(supports_background_motion);
        let show_motion_controls =
            supports_background_motion && settings.shared.background_motion_enabled;
        self.widgets
            .background_motion_zoom_row
            .set_visible(show_motion_controls);
        self.widgets
            .background_motion_reversal_duration_row
            .set_visible(show_motion_controls);
        self.widgets
            .track_info_alignment
            .set_sensitive(!settings.shared.hide_track_info);
        self.widgets.transition_duration.set_sensitive(!matches!(
            settings.shared.transition,
            TransitionEffect::None
        ));
    }

    fn connect_handlers(&self, controller: SettingsController) {
        let controller_for_reset = controller.clone();
        self.widgets.reset.connect_clicked(move |_| {
            controller_for_reset.reset();
        });

        let applying = self.applying.clone();
        let controller_for_display_mode = controller.clone();
        let classic_settings = self.widgets.classic_settings.clone();
        let hide_track_info = self.widgets.hide_track_info.clone();
        let text_size_row = self.widgets.text_size_row.clone();
        let background_motion_settings = self.widgets.background_motion_settings.clone();
        let background_motion_enabled = self.widgets.background_motion_enabled.clone();
        let background_motion_zoom_row = self.widgets.background_motion_zoom_row.clone();
        let background_motion_reversal_duration_row =
            self.widgets.background_motion_reversal_duration_row.clone();
        self.widgets
            .display_mode
            .connect_selected_notify(move |combo| {
                if applying.get() {
                    return;
                }

                let display_mode = DisplayMode::from_index(combo.selected());
                controller_for_display_mode
                    .update(NowPlayingPreferenceChange::DisplayMode(display_mode));
                classic_settings.set_visible(display_mode.shows_classic_settings());
                hide_track_info.set_visible(display_mode.supports_hiding_track_info());
                text_size_row
                    .set_visible(display_mode.shows_track_info(hide_track_info.is_active()));
                let supports_background_motion = display_mode.supports_background_motion();
                background_motion_settings.set_visible(supports_background_motion);
                let show_motion_controls =
                    supports_background_motion && background_motion_enabled.is_active();
                background_motion_zoom_row.set_visible(show_motion_controls);
                background_motion_reversal_duration_row.set_visible(show_motion_controls);
            });

        let applying = self.applying.clone();
        let controller_for_round_corners = controller.clone();
        self.widgets
            .round_corners
            .connect_active_notify(move |switch| {
                if !applying.get() {
                    controller_for_round_corners
                        .update(NowPlayingPreferenceChange::RoundCorners(switch.is_active()));
                }
            });

        let applying = self.applying.clone();
        let controller_for_hide_track_info = controller.clone();
        let track_info_alignment = self.widgets.track_info_alignment.clone();
        let text_size_row = self.widgets.text_size_row.clone();
        self.widgets
            .hide_track_info
            .connect_active_notify(move |switch| {
                if applying.get() {
                    return;
                }

                controller_for_hide_track_info.update(NowPlayingPreferenceChange::HideTrackInfo(
                    switch.is_active(),
                ));
                track_info_alignment.set_sensitive(!switch.is_active());
                text_size_row.set_visible(!switch.is_active());
            });

        let applying = self.applying.clone();
        let controller_for_text_size = controller.clone();
        self.widgets.text_size.connect_value_changed(move |scale| {
            if !applying.get() {
                controller_for_text_size.update_debounced(NowPlayingPreferenceChange::TextSize(
                    TextSize::from_scale_value(scale.value()),
                ));
            }
        });

        let applying = self.applying.clone();
        let controller_for_background_motion_enabled = controller.clone();
        let background_motion_zoom_row = self.widgets.background_motion_zoom_row.clone();
        let background_motion_reversal_duration_row =
            self.widgets.background_motion_reversal_duration_row.clone();
        self.widgets
            .background_motion_enabled
            .connect_active_notify(move |switch| {
                if applying.get() {
                    return;
                }

                let enabled = switch.is_active();
                controller_for_background_motion_enabled
                    .update(NowPlayingPreferenceChange::BackgroundMotionEnabled(enabled));
                background_motion_zoom_row.set_visible(enabled);
                background_motion_reversal_duration_row.set_visible(enabled);
            });

        let applying = self.applying.clone();
        let controller_for_background_motion_zoom = controller.clone();
        self.widgets
            .background_motion_zoom
            .connect_value_changed(move |scale| {
                if !applying.get() {
                    controller_for_background_motion_zoom.update_debounced(
                        NowPlayingPreferenceChange::BackgroundMotionZoomPercent(
                            scale.value().round().max(0.0) as u16,
                        ),
                    );
                }
            });

        let applying = self.applying.clone();
        let controller_for_background_motion_reversal_duration = controller.clone();
        self.widgets
            .background_motion_reversal_duration
            .connect_value_changed(move |scale| {
                if !applying.get() {
                    controller_for_background_motion_reversal_duration.update_debounced(
                        NowPlayingPreferenceChange::BackgroundMotionReversalDurationSecs(
                            scale.value().round().max(0.0) as u64,
                        ),
                    );
                }
            });

        let applying = self.applying.clone();
        let controller_for_alignment_left = controller.clone();
        self.widgets
            .track_info_alignment_left
            .connect_toggled(move |button| {
                if !applying.get() && button.is_active() {
                    controller_for_alignment_left.update(
                        NowPlayingPreferenceChange::TrackInfoAlignment(TrackInfoAlignment::Left),
                    );
                }
            });

        let applying = self.applying.clone();
        let controller_for_alignment_center = controller.clone();
        self.widgets
            .track_info_alignment_center
            .connect_toggled(move |button| {
                if !applying.get() && button.is_active() {
                    controller_for_alignment_center.update(
                        NowPlayingPreferenceChange::TrackInfoAlignment(TrackInfoAlignment::Center),
                    );
                }
            });

        let applying = self.applying.clone();
        let controller_for_alignment_right = controller.clone();
        self.widgets
            .track_info_alignment_right
            .connect_toggled(move |button| {
                if !applying.get() && button.is_active() {
                    controller_for_alignment_right.update(
                        NowPlayingPreferenceChange::TrackInfoAlignment(TrackInfoAlignment::Right),
                    );
                }
            });

        let applying = self.applying.clone();
        let controller_for_album_cover_size = controller.clone();
        self.widgets
            .album_cover_size
            .connect_value_changed(move |scale| {
                if !applying.get() {
                    controller_for_album_cover_size.update_debounced(
                        NowPlayingPreferenceChange::AlbumCoverSize(
                            AlbumCoverSize::from_scale_value(scale.value()),
                        ),
                    );
                }
            });

        let applying = self.applying.clone();
        let controller_for_gradient = controller.clone();
        self.widgets
            .background_style_gradient
            .connect_toggled(move |button| {
                if !applying.get() && button.is_active() {
                    controller_for_gradient.update(NowPlayingPreferenceChange::BackgroundStyle(
                        BackgroundStyle::Gradient,
                    ));
                }
            });

        let applying = self.applying.clone();
        let controller_for_solid = controller.clone();
        self.widgets
            .background_style_solid
            .connect_toggled(move |button| {
                if !applying.get() && button.is_active() {
                    controller_for_solid.update(NowPlayingPreferenceChange::BackgroundStyle(
                        BackgroundStyle::Solid,
                    ));
                }
            });

        let applying = self.applying.clone();
        let controller_for_always_display = controller.clone();
        self.widgets
            .always_display_last_recognized_song
            .connect_active_notify(move |switch| {
                if !applying.get() {
                    controller_for_always_display.update(
                        NowPlayingPreferenceChange::AlwaysDisplayLastRecognizedSong(
                            switch.is_active(),
                        ),
                    );
                }
            });

        let applying = self.applying.clone();
        let controller_for_transition = controller.clone();
        let transition_duration = self.widgets.transition_duration.clone();
        self.widgets
            .transition
            .connect_selected_notify(move |combo| {
                if applying.get() {
                    return;
                }

                let transition = TransitionEffect::from_index(combo.selected());
                controller_for_transition
                    .update(NowPlayingPreferenceChange::Transition(transition));
                transition_duration.set_sensitive(!matches!(transition, TransitionEffect::None));
            });

        let applying = self.applying.clone();
        let controller_for_transition_duration = controller.clone();
        self.widgets
            .transition_duration
            .connect_value_changed(move |scale| {
                if !applying.get() {
                    controller_for_transition_duration.update_debounced(
                        NowPlayingPreferenceChange::TransitionDurationMs(
                            transition_duration_from_scale(scale.value()),
                        ),
                    );
                }
            });
    }
}

/// Keeps the declarative widgets aligned with the persisted model's tuning limits.
fn configure_integer_scale(scale: &gtk::Scale, value: f64, lower: f64, upper: f64, step: f64) {
    scale
        .adjustment()
        .configure(value, lower, upper, step, step, 0.0);
    scale.set_digits(0);
    scale.set_round_digits(0);
}
