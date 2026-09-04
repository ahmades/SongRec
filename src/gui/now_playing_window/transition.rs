//! GTK presentation and slider conversion for track-transition preferences.

use crate::core::preferences::{TransitionEffect, clamp_transition_duration_ms};
use gettextrs::gettext;

/// Placement used while a size-changing `GtkRevealer` transition is running.
///
/// A non-fill alignment lets the revealer itself shrink on the animated axis
/// while its child keeps the viewport-sized allocation it was designed for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RevealerLayout {
    HorizontalStart,
    HorizontalEnd,
    VerticalStart,
    VerticalEnd,
}

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

    /// Returns the edge anchoring needed by GTK's size-changing transitions.
    pub(super) const fn revealer_layout(self) -> Option<RevealerLayout> {
        match self {
            Self::SlideRight | Self::SwingRight => Some(RevealerLayout::HorizontalStart),
            Self::SlideLeft | Self::SwingLeft => Some(RevealerLayout::HorizontalEnd),
            Self::SlideDown | Self::SwingDown => Some(RevealerLayout::VerticalStart),
            Self::SlideUp | Self::SwingUp => Some(RevealerLayout::VerticalEnd),
            Self::None | Self::Crossfade => None,
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
    use super::{RevealerLayout, TransitionEffect};

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

    #[test]
    fn size_changing_transitions_are_anchored_to_their_reveal_edge() {
        for effect in [TransitionEffect::SlideRight, TransitionEffect::SwingRight] {
            assert_eq!(
                effect.revealer_layout(),
                Some(RevealerLayout::HorizontalStart)
            );
        }
        for effect in [TransitionEffect::SlideLeft, TransitionEffect::SwingLeft] {
            assert_eq!(
                effect.revealer_layout(),
                Some(RevealerLayout::HorizontalEnd)
            );
        }
        for effect in [TransitionEffect::SlideDown, TransitionEffect::SwingDown] {
            assert_eq!(
                effect.revealer_layout(),
                Some(RevealerLayout::VerticalStart)
            );
        }
        for effect in [TransitionEffect::SlideUp, TransitionEffect::SwingUp] {
            assert_eq!(effect.revealer_layout(), Some(RevealerLayout::VerticalEnd));
        }
        for effect in [TransitionEffect::None, TransitionEffect::Crossfade] {
            assert_eq!(effect.revealer_layout(), None);
        }
    }
}
