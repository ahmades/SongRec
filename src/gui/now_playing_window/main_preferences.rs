//! Bindings for the Now Playing section of the main preferences page.

use super::{
    AlbumCoverSize, BackgroundStyle, NowPlayingSettings, SettingsController, TrackInfoAlignment,
    TransitionEffect, transition_duration_from_scale,
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
    round_corners: adw::SwitchRow,
    hide_track_info: adw::SwitchRow,
    track_info_alignment: adw::ActionRow,
    track_info_alignment_left: gtk::ToggleButton,
    track_info_alignment_center: gtk::ToggleButton,
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
            round_corners: builder.object("round_corners_setting").unwrap(),
            hide_track_info: builder.object("hide_track_info_setting").unwrap(),
            track_info_alignment: builder.object("track_info_alignment_setting").unwrap(),
            track_info_alignment_left: builder.object("track_info_alignment_left").unwrap(),
            track_info_alignment_center: builder.object("track_info_alignment_center").unwrap(),
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
            .set_active(settings.classic.hide_track_info);
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

        self.apply_sensitivity(settings);
        self.applying.set(was_applying);
    }

    fn apply_sensitivity(&self, settings: NowPlayingSettings) {
        self.widgets
            .track_info_alignment
            .set_sensitive(!settings.classic.hide_track_info);
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
