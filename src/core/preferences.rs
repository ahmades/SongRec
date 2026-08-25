use gettextrs::gettext;
use log::{debug, error};
use serde::Deserialize;
use serde::Serialize;
use std::error::Error;
use std::path::PathBuf;

use crate::utils::filesystem_operations::obtain_preferences_file_path;

/// The default duration for a Now Playing transition.
///
/// This lives with the persisted preferences rather than the GUI so both
/// preference defaults and the Now Playing controls share one source of truth.
pub(crate) const TRANSITION_DURATION_DEFAULT_MS: u64 = 2_000;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Preferences {
    pub enable_notifications: Option<bool>,
    pub enable_systray: Option<bool>,
    pub enable_mpris: Option<bool>, // Legacy, before setting default to true
    pub enable_mpris_v2: Option<bool>,
    pub no_duplicates: Option<bool>,
    pub buffer_size_secs: Option<u64>,         // Removed in 0.7.3
    pub request_interval_secs: Option<u64>,    // Legacy, before increasing default from 4 to 10
    pub request_interval_secs_v2: Option<u64>, // before decreasing from 10 to 8
    pub request_interval_secs_v3: Option<u64>,
    pub current_device_name: Option<String>,
    pub website_search_url: Option<String>,
    pub website_search_text: Option<String>,
    pub now_playing_round_corners: Option<bool>,
    pub hide_now_playing_info: Option<bool>,
    pub now_playing_track_info_alignment: Option<String>,
    pub now_playing_background_style: Option<String>,
    pub always_display_last_recognized_song: Option<bool>,
    pub now_playing_transition: Option<String>,
    pub now_playing_transition_duration_ms: Option<u64>,
    pub lights_off_enabled: Option<bool>,
}

impl Preferences {
    pub fn new() -> Self {
        Preferences {
            enable_notifications: None,
            enable_systray: None,
            enable_mpris: None,
            enable_mpris_v2: None,
            no_duplicates: None,
            buffer_size_secs: None,
            request_interval_secs: None,
            request_interval_secs_v2: None,
            request_interval_secs_v3: None,
            current_device_name: None,
            website_search_url: None,
            website_search_text: None,
            now_playing_round_corners: None,
            hide_now_playing_info: None,
            now_playing_track_info_alignment: None,
            now_playing_background_style: None,
            always_display_last_recognized_song: None,
            now_playing_transition: None,
            now_playing_transition_duration_ms: None,
            lights_off_enabled: None,
        }
    }

    /// Returns a partial update that restores every Now Playing preference to
    /// its default value without changing unrelated application preferences.
    pub(crate) fn now_playing_defaults() -> Self {
        Preferences {
            now_playing_round_corners: Some(true),
            hide_now_playing_info: Some(false),
            now_playing_track_info_alignment: Some("center".to_string()),
            now_playing_background_style: Some("gradient".to_string()),
            always_display_last_recognized_song: Some(true),
            now_playing_transition: Some("none".to_string()),
            now_playing_transition_duration_ms: Some(TRANSITION_DURATION_DEFAULT_MS),
            lights_off_enabled: Some(false),
            ..Self::new()
        }
    }

    pub fn with_interval(interval: u64) -> Self {
        let mut preferences = Self::default();
        preferences.request_interval_secs_v3 = Some(interval);
        preferences
    }
}

impl Default for Preferences {
    fn default() -> Self {
        Preferences {
            enable_notifications: Some(true),
            enable_systray: Some(false),
            enable_mpris: None,
            enable_mpris_v2: Some(true),
            no_duplicates: Some(false),
            buffer_size_secs: None,
            request_interval_secs: None,
            request_interval_secs_v2: None,
            request_interval_secs_v3: Some(8),
            current_device_name: None,
            website_search_url: Some("https://www.youtube.com/results?search_query=".to_string()),
            website_search_text: Some(gettext("Search on YouTube".to_string())),
            ..Self::now_playing_defaults()
        }
    }
}

#[derive(Clone, Debug)]
pub struct PreferencesInterface {
    pub preferences_file_path: Option<PathBuf>,
    pub preferences: Preferences,
}

impl PreferencesInterface {
    pub fn new() -> Self {
        match PreferencesInterface::load() {
            Ok(preferences_interface) => preferences_interface,
            Err(e) => {
                error!("{} {}", gettext("When parsing the preferences file:"), e);
                PreferencesInterface {
                    preferences_file_path: obtain_preferences_file_path().ok(),
                    preferences: Preferences::default(),
                }
            }
        }
    }

    fn load() -> Result<PreferencesInterface, Box<dyn Error>> {
        let preferences_file_path = obtain_preferences_file_path()?;
        let contents = std::fs::read_to_string(&preferences_file_path).unwrap_or_default();
        let preferences: Preferences = toml::from_str(&contents)?;
        debug!(
            "Loaded preferences from {}: {:?}",
            preferences_file_path.display(),
            preferences
        );
        Ok(PreferencesInterface {
            preferences_file_path: Some(preferences_file_path),
            preferences,
        })
    }

    pub fn update(&mut self, update_preferences: Preferences) {
        let current_preferences = &self.preferences;
        self.preferences = Preferences {
            enable_notifications: update_preferences
                .enable_notifications
                .or(current_preferences.enable_notifications),
            enable_mpris: None,
            enable_mpris_v2: update_preferences
                .enable_mpris_v2
                .or(current_preferences.enable_mpris_v2)
                .or(current_preferences.enable_mpris),
            enable_systray: update_preferences
                .enable_systray
                .or(current_preferences.enable_systray),
            no_duplicates: update_preferences
                .no_duplicates
                .or(current_preferences.no_duplicates),
            buffer_size_secs: None,
            request_interval_secs: None,
            request_interval_secs_v2: None,
            request_interval_secs_v3: update_preferences
                .request_interval_secs_v3
                .or(match current_preferences.request_interval_secs {
                    Some(4) => None,
                    Some(val) => Some(val),
                    None => None,
                })
                .or(match current_preferences.request_interval_secs_v2 {
                    Some(10) => None,
                    Some(val) => Some(val),
                    None => None,
                })
                .or(current_preferences.request_interval_secs_v3),
            current_device_name: update_preferences
                .current_device_name
                .or_else(|| current_preferences.current_device_name.clone()),
            website_search_url: update_preferences
                .website_search_url
                .or_else(|| current_preferences.website_search_url.clone()),
            website_search_text: update_preferences
                .website_search_text
                .or_else(|| current_preferences.website_search_text.clone()),
            now_playing_round_corners: update_preferences
                .now_playing_round_corners
                .or(current_preferences.now_playing_round_corners),
            hide_now_playing_info: update_preferences
                .hide_now_playing_info
                .or(current_preferences.hide_now_playing_info),
            now_playing_track_info_alignment: update_preferences
                .now_playing_track_info_alignment
                .or_else(|| current_preferences.now_playing_track_info_alignment.clone()),
            now_playing_background_style: update_preferences
                .now_playing_background_style
                .or_else(|| current_preferences.now_playing_background_style.clone()),
            always_display_last_recognized_song: update_preferences
                .always_display_last_recognized_song
                .or(current_preferences.always_display_last_recognized_song),
            now_playing_transition: update_preferences
                .now_playing_transition
                .or_else(|| current_preferences.now_playing_transition.clone()),
            now_playing_transition_duration_ms: update_preferences
                .now_playing_transition_duration_ms
                .or(current_preferences.now_playing_transition_duration_ms),
            lights_off_enabled: update_preferences
                .lights_off_enabled
                .or(current_preferences.lights_off_enabled),
        };
        if let Err(error) = self.write() {
            error!("{} {}", gettext("When saving the preferences file:"), error);
        }
    }

    fn write(&mut self) -> Result<(), Box<dyn Error>> {
        if let Some(preferences_file_path) = &self.preferences_file_path {
            let contents: String = toml::to_string(&self.preferences)?;
            std::fs::write(preferences_file_path, contents)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Preferences, PreferencesInterface, TRANSITION_DURATION_DEFAULT_MS};

    #[test]
    fn now_playing_defaults_are_a_partial_preference_update() {
        let preferences = Preferences::now_playing_defaults();

        assert_eq!(preferences.now_playing_round_corners, Some(true));
        assert_eq!(preferences.hide_now_playing_info, Some(false));
        assert_eq!(
            preferences.now_playing_track_info_alignment.as_deref(),
            Some("center")
        );
        assert_eq!(
            preferences.now_playing_background_style.as_deref(),
            Some("gradient")
        );
        assert_eq!(preferences.always_display_last_recognized_song, Some(true));
        assert_eq!(preferences.now_playing_transition.as_deref(), Some("none"));
        assert_eq!(
            preferences.now_playing_transition_duration_ms,
            Some(TRANSITION_DURATION_DEFAULT_MS)
        );
        assert_eq!(preferences.lights_off_enabled, Some(false));

        assert_eq!(preferences.enable_notifications, None);
        assert_eq!(preferences.request_interval_secs_v3, None);
        assert_eq!(preferences.website_search_url, None);
    }

    #[test]
    fn reset_update_preserves_unrelated_preferences() {
        let mut preferences = Preferences::new();
        preferences.enable_notifications = Some(false);
        preferences.now_playing_round_corners = Some(false);
        preferences.now_playing_transition_duration_ms = Some(5_000);
        let mut interface = PreferencesInterface {
            preferences_file_path: None,
            preferences,
        };

        interface.update(Preferences::now_playing_defaults());

        assert_eq!(interface.preferences.enable_notifications, Some(false));
        assert_eq!(interface.preferences.now_playing_round_corners, Some(true));
        assert_eq!(
            interface.preferences.now_playing_transition_duration_ms,
            Some(TRANSITION_DURATION_DEFAULT_MS)
        );
    }
}
