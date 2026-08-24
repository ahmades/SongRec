//! Persisted presentation choices for the Now Playing window.

use crate::core::preferences::Preferences;
use gettextrs::gettext;

/// The shortest transition duration exposed by the UI.
pub(crate) const TRANSITION_DURATION_MIN_MS: u64 = 500;
/// The longest transition duration exposed by the UI.
pub(crate) const TRANSITION_DURATION_MAX_MS: u64 = 5000;
/// The transition duration used when no preference has been persisted yet.
pub(crate) const TRANSITION_DURATION_DEFAULT_MS: u64 = 2000;

/// Clamps a persisted transition duration to the range supported by the UI.
pub(crate) fn clamp_transition_duration_ms(duration_ms: u64) -> u64 {
    duration_ms.clamp(TRANSITION_DURATION_MIN_MS, TRANSITION_DURATION_MAX_MS)
}

/// Converts a transition-duration scale value into its persisted representation.
pub(crate) fn transition_duration_from_scale(value: f64) -> u64 {
    clamp_transition_duration_ms(value.round().max(0.0) as u64)
}

/// Keeps a local, debounced duration change from being overwritten by an
/// unrelated preference refresh.
///
/// The returned duration is the one that should be shown in the UI.  A
/// pending value is cleared only once a refreshed preference snapshot has
/// acknowledged that exact value.
pub(crate) fn reconcile_transition_duration(
    persisted_duration_ms: u64,
    pending_duration_ms: Option<u64>,
) -> (u64, Option<u64>) {
    match pending_duration_ms {
        Some(pending_duration_ms) if pending_duration_ms == persisted_duration_ms => {
            (persisted_duration_ms, None)
        }
        Some(pending_duration_ms) => (pending_duration_ms, Some(pending_duration_ms)),
        None => (persisted_duration_ms, None),
    }
}

/// Defines the lightweight visual transition used when a new track is displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionEffect {
    /// Replace the current track immediately without animation.
    None,
    /// Crossfade the current track into the new track.
    Fade,
    /// Slide the new track in from the right.
    SlideRight,
    /// Slide the new track in from the left.
    SlideLeft,
    /// Slide the new track in from the bottom.
    SlideUp,
    /// Slide the new track in from the top.
    SlideDown,
    /// Swing the new track in from the left.
    SwingRight,
    /// Swing the new track in from the right.
    SwingLeft,
    /// Swing the new track in from the bottom.
    SwingUp,
    /// Swing the new track in from the top.
    SwingDown,
}

impl TransitionEffect {
    /// All transition effects in the order used by the dropdown controls.
    pub const ALL: [Self; 10] = [
        Self::None,
        Self::Fade,
        Self::SlideRight,
        Self::SlideLeft,
        Self::SlideUp,
        Self::SlideDown,
        Self::SwingRight,
        Self::SwingLeft,
        Self::SwingUp,
        Self::SwingDown,
    ];

    /// Returns the localized label used by transition-effect selectors.
    ///
    /// Keep the literals in this match so gettext extraction continues to see
    /// every selectable effect rather than a dynamically supplied string.
    pub(crate) fn translated_label(self) -> String {
        match self {
            Self::None => gettext("None"),
            Self::Fade => gettext("Fade"),
            Self::SlideRight => gettext("Slide right"),
            Self::SlideLeft => gettext("Slide left"),
            Self::SlideUp => gettext("Slide up"),
            Self::SlideDown => gettext("Slide down"),
            Self::SwingRight => gettext("Swing right"),
            Self::SwingLeft => gettext("Swing left"),
            Self::SwingUp => gettext("Swing up"),
            Self::SwingDown => gettext("Swing down"),
        }
    }

    /// Returns the persisted string representation of this transition effect.
    pub fn as_preference_value(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Fade => "fade",
            Self::SlideRight => "slide-right",
            Self::SlideLeft => "slide-left",
            Self::SlideUp => "slide-up",
            Self::SlideDown => "slide-down",
            Self::SwingRight => "swing-right",
            Self::SwingLeft => "swing-left",
            Self::SwingUp => "swing-up",
            Self::SwingDown => "swing-down",
        }
    }

    /// Parses a persisted transition value, defaulting to no animation.
    pub fn from_preference(value: Option<&str>) -> Self {
        match value {
            Some("fade") => Self::Fade,
            Some("slide-right") => Self::SlideRight,
            Some("slide-left") => Self::SlideLeft,
            Some("slide-up") => Self::SlideUp,
            Some("slide-down") => Self::SlideDown,
            Some("swing-right") => Self::SwingRight,
            Some("swing-left") => Self::SwingLeft,
            Some("swing-up") => Self::SwingUp,
            Some("swing-down") => Self::SwingDown,
            _ => Self::None,
        }
    }

    /// Converts the effect into the corresponding GTK revealer animation.
    pub fn revealer_type(self) -> gtk::RevealerTransitionType {
        match self {
            Self::None => gtk::RevealerTransitionType::None,
            Self::Fade => gtk::RevealerTransitionType::Crossfade,
            Self::SlideRight => gtk::RevealerTransitionType::SlideRight,
            Self::SlideLeft => gtk::RevealerTransitionType::SlideLeft,
            Self::SlideUp => gtk::RevealerTransitionType::SlideUp,
            Self::SlideDown => gtk::RevealerTransitionType::SlideDown,
            Self::SwingRight => gtk::RevealerTransitionType::SwingRight,
            Self::SwingLeft => gtk::RevealerTransitionType::SwingLeft,
            Self::SwingUp => gtk::RevealerTransitionType::SwingUp,
            Self::SwingDown => gtk::RevealerTransitionType::SwingDown,
        }
    }

    /// Returns the zero-based index used by the transition-effect dropdown.
    pub fn index(self) -> u32 {
        self as u32
    }

    /// Converts a transition-effect dropdown index into an effect.
    pub fn from_index(index: u32) -> Self {
        match index {
            0 => Self::None,
            1 => Self::Fade,
            2 => Self::SlideRight,
            3 => Self::SlideLeft,
            4 => Self::SlideUp,
            5 => Self::SlideDown,
            6 => Self::SwingRight,
            7 => Self::SwingLeft,
            8 => Self::SwingUp,
            9 => Self::SwingDown,
            _ => Self::None,
        }
    }
}

/// Controls where the track metadata is aligned in the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackInfoAlignment {
    Left,
    Center,
}

impl TrackInfoAlignment {
    /// Returns the persisted string representation of this track-info alignment.
    pub fn as_preference_value(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Center => "center",
        }
    }

    /// Parses a persisted track-info alignment value, defaulting to center.
    pub fn from_preference(value: Option<&str>) -> Self {
        match value {
            Some("left") => Self::Left,
            _ => Self::Center,
        }
    }
}

/// Controls the visual treatment of the Now Playing background.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundStyle {
    Gradient,
    Solid,
}

impl BackgroundStyle {
    /// Returns the persisted string representation of this background style.
    pub fn as_preference_value(self) -> &'static str {
        match self {
            Self::Gradient => "gradient",
            Self::Solid => "solid",
        }
    }

    /// Parses a persisted background style value, defaulting to gradient.
    pub fn from_preference(value: Option<&str>) -> Self {
        match value {
            Some("solid") => Self::Solid,
            _ => Self::Gradient,
        }
    }
}

/// The complete persisted presentation state for a Now Playing window.
///
/// Resolving preferences once prevents the UI, renderer, and transition logic
/// from each applying subtly different defaults or bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NowPlayingSettings {
    pub(crate) round_corners: bool,
    pub(crate) hide_track_info: bool,
    pub(crate) track_info_alignment: TrackInfoAlignment,
    pub(crate) background_style: BackgroundStyle,
    pub(crate) always_display_last_recognized_song: bool,
    pub(crate) transition: TransitionEffect,
    pub(crate) transition_duration_ms: u64,
    pub(crate) lights_off: bool,
}

impl From<&Preferences> for NowPlayingSettings {
    fn from(preferences: &Preferences) -> Self {
        let lights_off = preferences.lights_off_enabled.unwrap_or(false);

        Self {
            round_corners: preferences.now_playing_round_corners.unwrap_or(true),
            // Enabling Lights Off has always reset this preference. Keep the
            // resolved settings internally consistent even for an older or
            // manually edited preferences file that contains both values.
            hide_track_info: !lights_off && preferences.hide_now_playing_info.unwrap_or(false),
            track_info_alignment: TrackInfoAlignment::from_preference(
                preferences.now_playing_track_info_alignment.as_deref(),
            ),
            background_style: BackgroundStyle::from_preference(
                preferences.now_playing_background_style.as_deref(),
            ),
            always_display_last_recognized_song: preferences
                .always_display_last_recognized_song
                .unwrap_or(true),
            transition: TransitionEffect::from_preference(
                preferences.now_playing_transition.as_deref(),
            ),
            transition_duration_ms: clamp_transition_duration_ms(
                preferences
                    .now_playing_transition_duration_ms
                    .unwrap_or(TRANSITION_DURATION_DEFAULT_MS),
            ),
            lights_off,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BackgroundStyle, NowPlayingSettings, TRANSITION_DURATION_DEFAULT_MS,
        TRANSITION_DURATION_MAX_MS, TRANSITION_DURATION_MIN_MS, TrackInfoAlignment,
        TransitionEffect, clamp_transition_duration_ms, reconcile_transition_duration,
    };
    use crate::core::preferences::Preferences;

    #[test]
    fn persisted_display_options_round_trip() {
        for effect in TransitionEffect::ALL {
            assert_eq!(
                TransitionEffect::from_preference(Some(effect.as_preference_value())),
                effect
            );
        }

        for alignment in [TrackInfoAlignment::Left, TrackInfoAlignment::Center] {
            assert_eq!(
                TrackInfoAlignment::from_preference(Some(alignment.as_preference_value())),
                alignment
            );
        }

        for style in [BackgroundStyle::Gradient, BackgroundStyle::Solid] {
            assert_eq!(
                BackgroundStyle::from_preference(Some(style.as_preference_value())),
                style
            );
        }
    }

    #[test]
    fn settings_resolve_defaults_and_normalize_duration() {
        let defaults = NowPlayingSettings::from(&Preferences::new());
        assert_eq!(
            defaults.transition_duration_ms,
            TRANSITION_DURATION_DEFAULT_MS
        );
        assert!(!defaults.hide_track_info);
        assert!(!defaults.lights_off);

        let mut preferences = Preferences::new();
        preferences.now_playing_transition_duration_ms = Some(1);
        assert_eq!(
            NowPlayingSettings::from(&preferences).transition_duration_ms,
            TRANSITION_DURATION_MIN_MS
        );

        preferences.now_playing_transition_duration_ms = Some(u64::MAX);
        preferences.hide_now_playing_info = Some(true);
        preferences.lights_off_enabled = Some(true);
        let settings = NowPlayingSettings::from(&preferences);
        assert_eq!(settings.transition_duration_ms, TRANSITION_DURATION_MAX_MS);
        assert!(!settings.hide_track_info);
        assert!(settings.lights_off);

        assert_eq!(clamp_transition_duration_ms(1200), 1200);
    }

    #[test]
    fn pending_duration_survives_unrelated_preference_refreshes() {
        assert_eq!(reconcile_transition_duration(2000, None), (2000, None));
        assert_eq!(
            reconcile_transition_duration(2000, Some(2500)),
            (2500, Some(2500))
        );
        assert_eq!(
            reconcile_transition_duration(2500, Some(2500)),
            (2500, None)
        );
    }
}
