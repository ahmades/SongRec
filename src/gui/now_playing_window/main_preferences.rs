//! Bindings for the Now Playing section of the main preferences page.

use super::cinema_framing::{CinemaArtworkFramingControls, CinemaCropFocusControls};
use super::display_mode::{DisplayModeControls, NowPlayingControlState};
use super::monitor_selection::MonitorSelector;
use super::presets::PresetManagerDialog;
use super::{
    AlbumCoverSize, BackgroundStyle, NowPlayingSettings, SettingsController, TextSize,
    TrackInfoAlignment, TransitionEffect, transition_duration_from_scale,
};
use crate::core::preferences::{
    BackdropIntensity, ImmersiveBackgroundSource, NowPlayingPreferenceChange,
};
use adw::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone)]
struct PreferenceSection {
    widgets: Vec<gtk::Widget>,
}

impl PreferenceSection {
    fn from_builder(builder: &gtk::Builder, names: &[&str]) -> Self {
        let widgets = names
            .iter()
            .map(|name| {
                builder
                    .object::<gtk::Widget>(*name)
                    .unwrap_or_else(|| panic!("missing Now Playing preference row: {name}"))
            })
            .collect();
        Self { widgets }
    }

    fn set_visible(&self, visible: bool) {
        for widget in &self.widgets {
            widget.set_visible(visible);
        }
    }
}

#[derive(Clone)]
struct PreferencesWidgets {
    window: adw::ApplicationWindow,
    reset: gtk::Button,
    presets: gtk::Button,
    display_mode: DisplayModeControls,
    fullscreen_monitor: MonitorSelector,
    keep_screen_awake: adw::SwitchRow,
    burn_in_protection_enabled: adw::SwitchRow,
    burn_in_inactivity_minutes: adw::SpinRow,
    classic_settings: PreferenceSection,
    cinema_settings: PreferenceSection,
    cinema_artwork_framing: CinemaArtworkFramingControls,
    cinema_crop_focus_row: adw::ActionRow,
    cinema_crop_focus: CinemaCropFocusControls,
    immersive_settings: PreferenceSection,
    round_corners: adw::SwitchRow,
    hide_track_info: adw::SwitchRow,
    text_size_row: adw::ActionRow,
    text_size: gtk::Scale,
    immersive_background_source_album_cover: gtk::ToggleButton,
    immersive_background_source_artist: gtk::ToggleButton,
    backdrop_intensity_soft: gtk::ToggleButton,
    backdrop_intensity_balanced: gtk::ToggleButton,
    backdrop_intensity_bold: gtk::ToggleButton,
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
    transition_duration_row: adw::ActionRow,
    transition_duration: gtk::Scale,
}

/// Owns the widgets and signal bindings for the main Now Playing preferences.
pub(crate) struct NowPlayingPreferencesView {
    widgets: PreferencesWidgets,
    applying: Rc<Cell<bool>>,
    applied_settings: Cell<Option<NowPlayingSettings>>,
}

impl NowPlayingPreferencesView {
    pub(crate) fn new(builder: &gtk::Builder, controller: SettingsController) -> Self {
        let display_mode = DisplayModeControls::new();
        let display_mode_row: adw::ActionRow = builder.object("display_mode_setting").unwrap();
        display_mode_row.add_suffix(display_mode.widget());
        display_mode
            .widget()
            .update_property(&[gtk::accessible::Property::Label(
                display_mode_row.title().as_str(),
            )]);

        let fullscreen_monitor = MonitorSelector::new(controller.fullscreen_monitor());
        let fullscreen_monitor_row: adw::ActionRow =
            builder.object("fullscreen_monitor_setting").unwrap();
        fullscreen_monitor_row.add_suffix(fullscreen_monitor.widget());
        fullscreen_monitor
            .widget()
            .update_property(&[gtk::accessible::Property::Label(
                fullscreen_monitor_row.title().as_str(),
            )]);
        fullscreen_monitor.enable_only_with_multiple_monitors(&fullscreen_monitor_row);

        let cinema_artwork_framing = CinemaArtworkFramingControls::new();
        let cinema_artwork_framing_row: adw::ActionRow =
            builder.object("cinema_artwork_framing_setting").unwrap();
        cinema_artwork_framing_row.add_suffix(cinema_artwork_framing.widget());

        let cinema_crop_focus = CinemaCropFocusControls::new();
        let cinema_crop_focus_row: adw::ActionRow =
            builder.object("cinema_crop_focus_setting").unwrap();
        cinema_crop_focus_row.add_suffix(cinema_crop_focus.widget());

        // Reuse the already-decoded result texture. The preview never starts
        // another artwork download or image-processing job.
        let results_image: gtk::Image = builder.object("results_image").unwrap();
        sync_crop_preview_artwork(&cinema_crop_focus, &results_image);
        let crop_focus_for_paintable = cinema_crop_focus.clone();
        results_image.connect_notify_local(Some("paintable"), move |image, _| {
            sync_crop_preview_artwork(&crop_focus_for_paintable, image);
        });
        let crop_focus_for_visibility = cinema_crop_focus.clone();
        results_image.connect_visible_notify(move |image| {
            sync_crop_preview_artwork(&crop_focus_for_visibility, image);
        });

        let widgets = PreferencesWidgets {
            window: builder.object("main_window").unwrap(),
            reset: builder
                .object("reset_now_playing_preferences_button")
                .unwrap(),
            presets: builder.object("now_playing_presets_button").unwrap(),
            display_mode,
            fullscreen_monitor,
            keep_screen_awake: builder.object("keep_screen_awake_setting").unwrap(),
            burn_in_protection_enabled: builder
                .object("burn_in_protection_enabled_setting")
                .unwrap(),
            burn_in_inactivity_minutes: builder
                .object("burn_in_inactivity_minutes_setting")
                .unwrap(),
            classic_settings: PreferenceSection::from_builder(
                builder,
                &[
                    "classic_now_playing_preferences",
                    "round_corners_setting",
                    "track_info_alignment_setting",
                    "album_cover_size_setting",
                    "background_style_setting",
                ],
            ),
            cinema_settings: PreferenceSection::from_builder(
                builder,
                &[
                    "cinema_now_playing_preferences",
                    "cinema_artwork_framing_setting",
                ],
            ),
            cinema_artwork_framing,
            cinema_crop_focus_row,
            cinema_crop_focus,
            immersive_settings: PreferenceSection::from_builder(
                builder,
                &[
                    "immersive_now_playing_preferences",
                    "immersive_background_source_setting",
                    "backdrop_intensity_setting",
                    "background_motion_enabled_setting",
                ],
            ),
            round_corners: builder.object("round_corners_setting").unwrap(),
            hide_track_info: builder.object("hide_track_info_setting").unwrap(),
            text_size_row: builder.object("text_size_setting").unwrap(),
            text_size: builder.object("text_size_setting_scale").unwrap(),
            immersive_background_source_album_cover: builder
                .object("immersive_background_source_album_cover")
                .unwrap(),
            immersive_background_source_artist: builder
                .object("immersive_background_source_artist")
                .unwrap(),
            backdrop_intensity_soft: builder.object("backdrop_intensity_soft").unwrap(),
            backdrop_intensity_balanced: builder.object("backdrop_intensity_balanced").unwrap(),
            backdrop_intensity_bold: builder.object("backdrop_intensity_bold").unwrap(),
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
            transition_duration_row: builder.object("transition_duration_setting").unwrap(),
            transition_duration: builder.object("transition_duration_setting_scale").unwrap(),
        };

        AlbumCoverSize::configure_scale(&widgets.album_cover_size);
        AlbumCoverSize::install_slider_snap(&widgets.album_cover_size);
        TextSize::configure_scale(&widgets.text_size);
        TextSize::install_slider_snap(&widgets.text_size);
        super::transition::configure_transition_duration_scale(&widgets.transition_duration);
        super::settings_scale::configure_background_motion_zoom_scale(
            &widgets.background_motion_zoom,
        );
        super::settings_scale::configure_background_motion_reversal_duration_scale(
            &widgets.background_motion_reversal_duration,
        );

        for row in [
            &widgets.text_size_row,
            &widgets.transition_duration_row,
            &widgets.cinema_crop_focus_row,
            &widgets.background_motion_zoom_row,
            &widgets.background_motion_reversal_duration_row,
        ] {
            mark_dependent_row(row);
        }
        mark_dependent_row(widgets.burn_in_inactivity_minutes.upcast_ref());

        for (scale, row_name) in [
            (&widgets.text_size, "text_size_setting"),
            (&widgets.album_cover_size, "album_cover_size_setting"),
            (
                &widgets.background_motion_zoom,
                "background_motion_zoom_setting",
            ),
            (
                &widgets.background_motion_reversal_duration,
                "background_motion_reversal_duration_setting",
            ),
            (&widgets.transition_duration, "transition_duration_setting"),
        ] {
            let row: adw::ActionRow = builder.object(row_name).unwrap();
            scale.update_property(&[gtk::accessible::Property::Label(row.title().as_str())]);
        }

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
            applied_settings: Cell::new(None),
        };
        view.apply(controller.settings());
        view.connect_handlers(controller);
        view
    }

    /// Applies a complete settings snapshot without treating widget changes as user input.
    pub(crate) fn apply(&self, settings: NowPlayingSettings) {
        if self.applied_settings.replace(Some(settings)) == Some(settings) {
            return;
        }
        let was_applying = self.applying.replace(true);

        self.widgets.display_mode.set_value(settings.display_mode);
        self.widgets
            .keep_screen_awake
            .set_active(settings.shared.keep_screen_awake);
        self.widgets
            .burn_in_protection_enabled
            .set_active(settings.shared.burn_in_protection_enabled);
        self.widgets
            .burn_in_inactivity_minutes
            .set_value(f64::from(settings.shared.burn_in_inactivity_minutes));
        self.widgets
            .cinema_artwork_framing
            .set_value(settings.cinema.artwork_framing);
        self.widgets
            .cinema_crop_focus
            .set_value(settings.cinema.crop_focus);
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
            .immersive_background_source_album_cover
            .set_active(matches!(
                settings.shared.immersive_background_source,
                ImmersiveBackgroundSource::AlbumCover
            ));
        self.widgets
            .immersive_background_source_artist
            .set_active(matches!(
                settings.shared.immersive_background_source,
                ImmersiveBackgroundSource::Artist
            ));
        self.widgets.backdrop_intensity_soft.set_active(matches!(
            settings.shared.backdrop_intensity,
            BackdropIntensity::Soft
        ));
        self.widgets
            .backdrop_intensity_balanced
            .set_active(matches!(
                settings.shared.backdrop_intensity,
                BackdropIntensity::Balanced
            ));
        self.widgets.backdrop_intensity_bold.set_active(matches!(
            settings.shared.backdrop_intensity,
            BackdropIntensity::Bold
        ));
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

    /// Applies window placement independently from visual preset settings.
    pub(crate) fn apply_fullscreen_monitor(
        &self,
        target: Option<crate::core::preferences::FullscreenMonitorTarget>,
    ) {
        self.widgets.fullscreen_monitor.set_target(target);
    }

    fn apply_control_state(&self, settings: NowPlayingSettings) {
        let state = NowPlayingControlState::from_settings(settings);
        self.widgets
            .burn_in_inactivity_minutes
            .set_visible(state.show_burn_in_details);
        self.widgets
            .hide_track_info
            .set_sensitive(state.hide_track_info_sensitive);
        self.widgets.text_size_row.set_visible(state.show_text_size);
        self.widgets
            .immersive_settings
            .set_visible(state.show_immersive_settings);
        self.widgets
            .cinema_settings
            .set_visible(state.show_cinema_settings);
        self.widgets
            .classic_settings
            .set_visible(state.show_classic_settings);
        self.widgets
            .cinema_crop_focus_row
            .set_visible(state.show_cinema_settings && state.show_crop_focus);
        self.widgets
            .background_motion_zoom_row
            .set_visible(state.show_immersive_settings && state.show_motion_details);
        self.widgets
            .background_motion_reversal_duration_row
            .set_visible(state.show_immersive_settings && state.show_motion_details);
        self.widgets
            .track_info_alignment
            .set_sensitive(state.track_info_alignment_sensitive);
        self.widgets
            .transition_duration_row
            .set_visible(state.show_transition_duration);
    }

    fn connect_handlers(&self, controller: SettingsController) {
        let controller_for_monitor = controller.clone();
        self.widgets
            .fullscreen_monitor
            .connect_changed(move |target| {
                controller_for_monitor.update_fullscreen_monitor(target)
            });

        let controller_for_presets = controller.clone();
        let preferences_window = self.widgets.window.downgrade();
        self.widgets.presets.connect_clicked(move |_| {
            if let Some(window) = preferences_window.upgrade() {
                PresetManagerDialog::new(controller_for_presets.clone()).present(&window);
            }
        });

        let controller_for_reset = controller.clone();
        self.widgets.reset.connect_clicked(move |_| {
            controller_for_reset.reset();
        });

        let bind_switch =
            |switch: &adw::SwitchRow, change: fn(bool) -> NowPlayingPreferenceChange| {
                let applying = self.applying.clone();
                let controller = controller.clone();
                switch.connect_active_notify(move |switch| {
                    if !applying.get() {
                        controller.update(change(switch.is_active()));
                    }
                });
            };
        bind_switch(
            &self.widgets.round_corners,
            NowPlayingPreferenceChange::RoundCorners,
        );
        bind_switch(
            &self.widgets.keep_screen_awake,
            NowPlayingPreferenceChange::KeepScreenAwake,
        );
        bind_switch(
            &self.widgets.burn_in_protection_enabled,
            NowPlayingPreferenceChange::BurnInProtectionEnabled,
        );
        bind_switch(
            &self.widgets.hide_track_info,
            NowPlayingPreferenceChange::HideTrackInfo,
        );
        bind_switch(
            &self.widgets.background_motion_enabled,
            NowPlayingPreferenceChange::BackgroundMotionEnabled,
        );
        bind_switch(
            &self.widgets.always_display_last_recognized_song,
            NowPlayingPreferenceChange::AlwaysDisplayLastRecognizedSong,
        );

        let applying = self.applying.clone();
        let controller_for_burn_in_inactivity = controller.clone();
        self.widgets
            .burn_in_inactivity_minutes
            .connect_value_notify(move |row| {
                if !applying.get() {
                    controller_for_burn_in_inactivity.update(
                        NowPlayingPreferenceChange::BurnInInactivityMinutes(
                            row.value().round().max(0.0) as u16,
                        ),
                    );
                }
            });

        let applying = self.applying.clone();
        let controller_for_mode = controller.clone();
        self.widgets
            .display_mode
            .connect_changed(move |display_mode| {
                if !applying.get() {
                    controller_for_mode
                        .update(NowPlayingPreferenceChange::DisplayMode(display_mode));
                }
            });
        let applying = self.applying.clone();
        let controller_for_transition = controller.clone();
        self.widgets
            .transition
            .connect_selected_notify(move |combo| {
                if !applying.get() {
                    controller_for_transition.update(NowPlayingPreferenceChange::Transition(
                        TransitionEffect::from_index(combo.selected()),
                    ));
                }
            });

        let applying = self.applying.clone();
        let controller_for_cinema_framing = controller.clone();
        self.widgets
            .cinema_artwork_framing
            .connect_changed(move |framing| {
                if !applying.get() {
                    controller_for_cinema_framing
                        .update(NowPlayingPreferenceChange::CinemaArtworkFraming(framing));
                }
            });

        let applying = self.applying.clone();
        let controller_for_crop_focus = controller.clone();
        self.widgets
            .cinema_crop_focus
            .connect_changed(move |focus| {
                if !applying.get() {
                    controller_for_crop_focus
                        .update_debounced(NowPlayingPreferenceChange::CinemaCropFocus(focus));
                }
            });

        for (button, change) in [
            (
                &self.widgets.track_info_alignment_left,
                NowPlayingPreferenceChange::TrackInfoAlignment(TrackInfoAlignment::Left),
            ),
            (
                &self.widgets.track_info_alignment_center,
                NowPlayingPreferenceChange::TrackInfoAlignment(TrackInfoAlignment::Center),
            ),
            (
                &self.widgets.track_info_alignment_right,
                NowPlayingPreferenceChange::TrackInfoAlignment(TrackInfoAlignment::Right),
            ),
            (
                &self.widgets.background_style_gradient,
                NowPlayingPreferenceChange::BackgroundStyle(BackgroundStyle::Gradient),
            ),
            (
                &self.widgets.background_style_solid,
                NowPlayingPreferenceChange::BackgroundStyle(BackgroundStyle::Solid),
            ),
            (
                &self.widgets.immersive_background_source_album_cover,
                NowPlayingPreferenceChange::ImmersiveBackgroundSource(
                    ImmersiveBackgroundSource::AlbumCover,
                ),
            ),
            (
                &self.widgets.immersive_background_source_artist,
                NowPlayingPreferenceChange::ImmersiveBackgroundSource(
                    ImmersiveBackgroundSource::Artist,
                ),
            ),
            (
                &self.widgets.backdrop_intensity_soft,
                NowPlayingPreferenceChange::BackdropIntensity(BackdropIntensity::Soft),
            ),
            (
                &self.widgets.backdrop_intensity_balanced,
                NowPlayingPreferenceChange::BackdropIntensity(BackdropIntensity::Balanced),
            ),
            (
                &self.widgets.backdrop_intensity_bold,
                NowPlayingPreferenceChange::BackdropIntensity(BackdropIntensity::Bold),
            ),
        ] {
            let applying = self.applying.clone();
            let controller = controller.clone();
            button.connect_toggled(move |button| {
                if !applying.get() && button.is_active() {
                    controller.update(change);
                }
            });
        }

        let bind_scale = |scale: &gtk::Scale, change: fn(f64) -> NowPlayingPreferenceChange| {
            let applying = self.applying.clone();
            let controller = controller.clone();
            scale.connect_value_changed(move |scale| {
                if !applying.get() {
                    controller.update_debounced(change(scale.value()));
                }
            });
        };
        bind_scale(&self.widgets.text_size, |value| {
            NowPlayingPreferenceChange::TextSize(TextSize::from_scale_value(value))
        });
        bind_scale(&self.widgets.album_cover_size, |value| {
            NowPlayingPreferenceChange::AlbumCoverSize(AlbumCoverSize::from_scale_value(value))
        });
        bind_scale(&self.widgets.background_motion_zoom, |value| {
            NowPlayingPreferenceChange::BackgroundMotionZoomPercent(value.round().max(0.0) as u16)
        });
        bind_scale(&self.widgets.background_motion_reversal_duration, |value| {
            NowPlayingPreferenceChange::BackgroundMotionReversalDurationSecs(
                value.round().max(0.0) as u64
            )
        });
        bind_scale(&self.widgets.transition_duration, |value| {
            NowPlayingPreferenceChange::TransitionDurationMs(transition_duration_from_scale(value))
        });
    }
}

fn sync_crop_preview_artwork(controls: &CinemaCropFocusControls, image: &gtk::Image) {
    let paintable = image.is_visible().then(|| image.paintable()).flatten();
    controls.set_artwork(paintable.as_ref());
}

/// Marks rows that are conditionally revealed by the immediately preceding row.
fn mark_dependent_row(row: &adw::ActionRow) {
    let guide = gtk::Separator::builder()
        .orientation(gtk::Orientation::Vertical)
        .accessible_role(gtk::AccessibleRole::None)
        .build();
    guide.set_margin_top(8);
    guide.set_margin_bottom(8);
    guide.set_margin_start(4);
    guide.set_margin_end(4);
    guide.add_css_class("dim-label");
    row.add_prefix(&guide);
}
