//! GTK selector presentation for the Now Playing display mode.

use crate::core::preferences::DisplayMode;
use gettextrs::gettext;

impl DisplayMode {
    /// Whether the Classic-only settings group applies to this mode.
    pub(crate) const fn shows_classic_settings(self) -> bool {
        matches!(self, Self::Classic)
    }

    /// Returns the localized label used by display-mode selectors.
    pub(crate) fn translated_label(self) -> String {
        match self {
            Self::Classic => gettext("Classic"),
            Self::FullBleed => gettext("Full bleed / Cinema"),
            Self::Ambient => gettext("Ambient"),
            Self::LightsOff => gettext("Lights Off"),
        }
    }

    /// Returns the zero-based index used by display-mode dropdowns.
    pub(crate) fn index(self) -> u32 {
        Self::ALL
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or_default() as u32
    }

    /// Converts a display-mode dropdown index into a mode.
    pub(crate) fn from_index(index: u32) -> Self {
        Self::ALL.get(index as usize).copied().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::DisplayMode;

    #[test]
    fn display_mode_dropdown_indices_follow_the_all_table() {
        for (index, display_mode) in DisplayMode::ALL.into_iter().enumerate() {
            assert_eq!(display_mode.index(), index as u32);
            assert_eq!(DisplayMode::from_index(index as u32), display_mode);
        }
        assert_eq!(DisplayMode::from_index(u32::MAX), DisplayMode::Classic);
    }

    #[test]
    fn only_classic_exposes_classic_settings() {
        for display_mode in DisplayMode::ALL {
            assert_eq!(
                display_mode.shows_classic_settings(),
                matches!(display_mode, DisplayMode::Classic)
            );
        }
    }
}
