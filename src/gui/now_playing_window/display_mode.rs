//! GTK selector presentation and derived control state for Now Playing modes.

use crate::core::preferences::{
    CinemaArtworkFraming, DisplayMode, NowPlayingPreferences, TransitionEffect,
};
use gettextrs::gettext;

/// Native segmented control shared by Now Playing settings surfaces.
#[derive(Clone)]
pub(super) struct DisplayModeControls {
    widget: adw::ToggleGroup,
}

impl DisplayModeControls {
    pub(super) fn new() -> Self {
        let widget = adw::ToggleGroup::builder()
            .can_shrink(true)
            .halign(gtk::Align::End)
            .homogeneous(true)
            .valign(gtk::Align::Center)
            .build();

        for display_mode in DisplayMode::ALL {
            widget.add(
                adw::Toggle::builder()
                    .label(display_mode.translated_label())
                    .name(display_mode.as_preference_value())
                    .build(),
            );
        }

        let controls = Self { widget };
        controls.set_value(DisplayMode::default());
        controls
    }

    pub(super) fn widget(&self) -> &adw::ToggleGroup {
        &self.widget
    }

    pub(super) fn set_value(&self, value: DisplayMode) {
        let name = value.as_preference_value();
        if self.widget.active_name().as_deref() != Some(name) {
            self.widget.set_active_name(Some(name));
        }
    }

    pub(super) fn connect_changed(&self, callback: impl Fn(DisplayMode) + 'static) {
        self.widget.connect_active_name_notify(move |group| {
            let Some(name) = group.active_name() else {
                return;
            };
            callback(DisplayMode::from_preference(Some(name.as_str())));
        });
    }
}

/// Visibility and sensitivity derived from one complete preference snapshot.
///
/// Keeping this policy free of widgets ensures the context menu and main
/// preferences present the same dependencies without duplicating conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NowPlayingControlState {
    pub(crate) show_classic_settings: bool,
    pub(crate) show_cinema_settings: bool,
    pub(crate) show_immersive_settings: bool,
    pub(crate) hide_track_info_sensitive: bool,
    pub(crate) show_burn_in_details: bool,
    pub(crate) show_text_size: bool,
    pub(crate) show_crop_focus: bool,
    pub(crate) show_motion_details: bool,
    pub(crate) show_transition_duration: bool,
    pub(crate) track_info_alignment_sensitive: bool,
}

impl NowPlayingControlState {
    pub(crate) const fn from_settings(settings: NowPlayingPreferences) -> Self {
        let display_mode = settings.display_mode;
        let track_info_visible = display_mode.shows_track_info(settings.shared.hide_track_info);

        Self {
            show_classic_settings: display_mode.shows_classic_settings(),
            show_cinema_settings: display_mode.shows_cinema_settings(),
            show_immersive_settings: display_mode.uses_immersive_artwork(),
            hide_track_info_sensitive: display_mode.supports_hiding_track_info(),
            show_burn_in_details: settings.shared.burn_in_protection_enabled,
            show_text_size: track_info_visible,
            show_crop_focus: display_mode.shows_cinema_crop_focus(settings.cinema.artwork_framing),
            show_motion_details: display_mode.supports_background_motion()
                && settings.shared.background_motion_enabled,
            show_transition_duration: !matches!(settings.shared.transition, TransitionEffect::None),
            track_info_alignment_sensitive: display_mode.shows_classic_settings()
                && track_info_visible,
        }
    }
}

impl DisplayMode {
    /// Whether the Classic-only settings group applies to this mode.
    pub(crate) const fn shows_classic_settings(self) -> bool {
        matches!(self, Self::Classic)
    }

    /// Whether the Cinema-only artwork-framing controls apply to this mode.
    pub(crate) const fn shows_cinema_settings(self) -> bool {
        matches!(self, Self::Cinema)
    }

    /// Whether the crop-focus detail is relevant to the current Cinema framing.
    pub(crate) const fn shows_cinema_crop_focus(self, framing: CinemaArtworkFraming) -> bool {
        self.shows_cinema_settings() && matches!(framing, CinemaArtworkFraming::Fill)
    }

    /// Whether this mode can remain useful after its metadata is hidden.
    pub(crate) const fn supports_hiding_track_info(self) -> bool {
        !matches!(self, Self::LightsOff)
    }

    /// Whether track metadata is effectively visible in this mode.
    pub(crate) const fn shows_track_info(self, hide_track_info: bool) -> bool {
        !hide_track_info || !self.supports_hiding_track_info()
    }

    /// Whether this mode's track-change scene depends on prepared immersive artwork.
    pub(crate) const fn uses_immersive_artwork(self) -> bool {
        matches!(self, Self::Cinema | Self::Ambient)
    }

    /// Whether this mode renders the blurred background used by ambient motion.
    pub(crate) const fn supports_background_motion(self) -> bool {
        self.uses_immersive_artwork()
    }

    /// Returns the localized label used by display-mode selectors.
    pub(crate) fn translated_label(self) -> String {
        match self {
            Self::Classic => gettext("Classic"),
            Self::Cinema => gettext("Cinema"),
            Self::Ambient => gettext("Ambient"),
            Self::LightsOff => gettext("Lights Off"),
        }
    }

    /// Whether a track-change scene should wait for its prepared artwork.
    pub(crate) const fn uses_artwork(self) -> bool {
        !matches!(self, Self::LightsOff)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CinemaArtworkFraming, DisplayMode, NowPlayingControlState, NowPlayingPreferences,
        TransitionEffect,
    };

    #[test]
    fn only_classic_exposes_classic_settings() {
        for display_mode in DisplayMode::ALL {
            assert_eq!(
                display_mode.shows_classic_settings(),
                matches!(display_mode, DisplayMode::Classic)
            );
        }
    }

    #[test]
    fn only_cinema_exposes_cinema_settings() {
        for display_mode in DisplayMode::ALL {
            assert_eq!(
                display_mode.shows_cinema_settings(),
                matches!(display_mode, DisplayMode::Cinema)
            );
        }

        assert!(!DisplayMode::Cinema.shows_cinema_crop_focus(CinemaArtworkFraming::Automatic));
        assert!(!DisplayMode::Cinema.shows_cinema_crop_focus(CinemaArtworkFraming::Fit));
        assert!(DisplayMode::Cinema.shows_cinema_crop_focus(CinemaArtworkFraming::Fill));
        assert!(!DisplayMode::Ambient.shows_cinema_crop_focus(CinemaArtworkFraming::Fill));
    }

    #[test]
    fn lights_off_keeps_track_info_available() {
        for display_mode in DisplayMode::ALL {
            assert_eq!(
                display_mode.supports_hiding_track_info(),
                !matches!(display_mode, DisplayMode::LightsOff)
            );
        }

        assert!(!DisplayMode::Classic.shows_track_info(true));
        assert!(DisplayMode::Classic.shows_track_info(false));
        assert!(DisplayMode::LightsOff.shows_track_info(true));
    }

    #[test]
    fn only_immersive_modes_support_background_motion() {
        for display_mode in DisplayMode::ALL {
            let is_immersive = matches!(display_mode, DisplayMode::Cinema | DisplayMode::Ambient);
            assert_eq!(display_mode.supports_background_motion(), is_immersive);
            assert_eq!(display_mode.uses_immersive_artwork(), is_immersive);
        }
    }

    #[test]
    fn every_visual_mode_except_lights_off_uses_artwork() {
        for display_mode in DisplayMode::ALL {
            assert_eq!(
                display_mode.uses_artwork(),
                !matches!(display_mode, DisplayMode::LightsOff)
            );
        }
    }

    #[test]
    fn control_state_selects_the_mode_specific_groups() {
        for display_mode in DisplayMode::ALL {
            let settings = NowPlayingPreferences {
                display_mode,
                ..NowPlayingPreferences::default()
            };
            let state = NowPlayingControlState::from_settings(settings);

            assert_eq!(
                state.show_classic_settings,
                matches!(display_mode, DisplayMode::Classic)
            );
            assert_eq!(
                state.show_cinema_settings,
                matches!(display_mode, DisplayMode::Cinema)
            );
            assert_eq!(
                state.show_immersive_settings,
                matches!(display_mode, DisplayMode::Cinema | DisplayMode::Ambient)
            );
        }
    }

    #[test]
    fn track_info_dependencies_use_effective_visibility() {
        for display_mode in DisplayMode::ALL {
            for hide_track_info in [false, true] {
                let mut settings = NowPlayingPreferences {
                    display_mode,
                    ..NowPlayingPreferences::default()
                };
                settings.shared.hide_track_info = hide_track_info;

                let state = NowPlayingControlState::from_settings(settings);
                let can_hide = !matches!(display_mode, DisplayMode::LightsOff);
                let track_info_visible = !hide_track_info || !can_hide;

                assert_eq!(state.hide_track_info_sensitive, can_hide);
                assert_eq!(state.show_text_size, track_info_visible);
                assert_eq!(
                    state.track_info_alignment_sensitive,
                    matches!(display_mode, DisplayMode::Classic) && track_info_visible
                );
            }
        }
    }

    #[test]
    fn cinema_crop_focus_is_visible_only_for_fill() {
        for display_mode in DisplayMode::ALL {
            for framing in [
                CinemaArtworkFraming::Automatic,
                CinemaArtworkFraming::Fit,
                CinemaArtworkFraming::Fill,
            ] {
                let mut settings = NowPlayingPreferences {
                    display_mode,
                    ..NowPlayingPreferences::default()
                };
                settings.cinema.artwork_framing = framing;

                assert_eq!(
                    NowPlayingControlState::from_settings(settings).show_crop_focus,
                    matches!(display_mode, DisplayMode::Cinema)
                        && matches!(framing, CinemaArtworkFraming::Fill)
                );
            }
        }
    }

    #[test]
    fn motion_details_require_an_enabled_immersive_mode() {
        for display_mode in DisplayMode::ALL {
            for enabled in [false, true] {
                let mut settings = NowPlayingPreferences {
                    display_mode,
                    ..NowPlayingPreferences::default()
                };
                settings.shared.background_motion_enabled = enabled;

                assert_eq!(
                    NowPlayingControlState::from_settings(settings).show_motion_details,
                    enabled && matches!(display_mode, DisplayMode::Cinema | DisplayMode::Ambient)
                );
            }
        }
    }

    #[test]
    fn transition_duration_is_visible_only_for_an_effect() {
        let mut settings = NowPlayingPreferences::default();
        settings.shared.transition = TransitionEffect::None;
        assert!(!NowPlayingControlState::from_settings(settings).show_transition_duration);

        settings.shared.transition = TransitionEffect::Crossfade;
        assert!(NowPlayingControlState::from_settings(settings).show_transition_duration);
    }

    #[test]
    fn burn_in_timeout_is_visible_only_when_protection_is_enabled() {
        let mut settings = NowPlayingPreferences::default();
        assert!(!NowPlayingControlState::from_settings(settings).show_burn_in_details);

        settings.shared.burn_in_protection_enabled = true;
        assert!(NowPlayingControlState::from_settings(settings).show_burn_in_details);
    }
}
