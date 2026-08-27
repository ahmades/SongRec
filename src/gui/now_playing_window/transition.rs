//! GTK presentation and slider conversion for track-transition preferences.

use crate::core::preferences::{TransitionEffect, clamp_transition_duration_ms};
use gettextrs::gettext;

/// Converts a transition-duration scale value into its persisted representation.
pub(crate) fn transition_duration_from_scale(value: f64) -> u64 {
    clamp_transition_duration_ms(value.round().max(0.0) as u64)
}

impl TransitionEffect {
    /// Returns the localized label used by transition-effect selectors.
    ///
    /// Keep the literals in this match so gettext extraction continues to see
    /// every selectable effect rather than a dynamically supplied string.
    pub(crate) fn translated_label(self) -> String {
        match self {
            Self::None => gettext("None"),
            Self::Crossfade => gettext("Crossfade"),
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

    /// Converts the effect into the corresponding GTK revealer animation.
    pub fn revealer_type(self) -> gtk::RevealerTransitionType {
        match self {
            Self::None => gtk::RevealerTransitionType::None,
            Self::Crossfade => gtk::RevealerTransitionType::Crossfade,
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
        Self::ALL
            .iter()
            .position(|effect| *effect == self)
            .unwrap_or_default() as u32
    }

    /// Converts a transition-effect dropdown index into an effect.
    pub fn from_index(index: u32) -> Self {
        Self::ALL.get(index as usize).copied().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::TransitionEffect;

    #[test]
    fn transition_dropdown_indices_follow_the_all_table() {
        for (index, effect) in TransitionEffect::ALL.into_iter().enumerate() {
            assert_eq!(effect.index(), index as u32);
            assert_eq!(TransitionEffect::from_index(index as u32), effect);
        }
        assert_eq!(
            TransitionEffect::from_index(u32::MAX),
            TransitionEffect::None
        );
    }
}
