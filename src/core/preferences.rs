use gettextrs::gettext;
use log::{debug, error};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::error::Error;
use std::path::PathBuf;

use crate::utils::filesystem_operations::obtain_preferences_file_path;

/// The default duration for a Now Playing transition.
pub(crate) const TRANSITION_DURATION_DEFAULT_MS: u64 = 2_000;
/// The shortest transition duration exposed by the UI.
pub(crate) const TRANSITION_DURATION_MIN_MS: u64 = 500;
/// The longest transition duration exposed by the UI.
pub(crate) const TRANSITION_DURATION_MAX_MS: u64 = 5_000;

/// Clamps a transition duration to the range supported by Now Playing.
pub(crate) fn clamp_transition_duration_ms(duration_ms: u64) -> u64 {
    duration_ms.clamp(TRANSITION_DURATION_MIN_MS, TRANSITION_DURATION_MAX_MS)
}

/// Controls the relative size of the album artwork within its reserved layout area.
///
/// The value is continuous so users can choose an exact size, while the named
/// positions remain convenient slider anchors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlbumCoverSize(u16);

impl Default for AlbumCoverSize {
    fn default() -> Self {
        Self::MEDIUM_LARGE_MIDPOINT
    }
}

impl AlbumCoverSize {
    pub const SMALL: Self = Self(4_500);
    pub const MEDIUM: Self = Self(7_000);
    pub const LARGE: Self = Self(10_000);
    pub const SMALL_MEDIUM_MIDPOINT: Self = Self((Self::SMALL.0 + Self::MEDIUM.0) / 2);
    pub const MEDIUM_LARGE_MIDPOINT: Self = Self((Self::MEDIUM.0 + Self::LARGE.0) / 2);
    pub const ALL: [Self; 3] = [Self::SMALL, Self::MEDIUM, Self::LARGE];
    const SNAP_ANCHORS: [Self; 5] = [
        Self::SMALL,
        Self::SMALL_MEDIUM_MIDPOINT,
        Self::MEDIUM,
        Self::MEDIUM_LARGE_MIDPOINT,
        Self::LARGE,
    ];
    pub(crate) const MIN_SCALE_VALUE: f64 = 0.0;
    pub(crate) const MAX_SCALE_VALUE: f64 = 200.0;
    pub(crate) const SCALE_STEP: f64 = 1.0;

    // A little more than one slider step makes the marked presets easy to
    // acquire without preventing a custom value elsewhere on the track.
    const SNAP_DISTANCE: f64 = Self::SCALE_STEP * 2.0;

    /// Returns the persisted representation of this size.
    pub fn as_preference_value(self) -> String {
        format!("{:.4}", self.layout_fraction())
    }

    /// Parses both the legacy named positions and a custom persisted size.
    pub fn from_preference(value: Option<&str>) -> Self {
        match value {
            Some("small") => Self::SMALL,
            Some("medium") => Self::MEDIUM,
            Some("large") => Self::LARGE,
            Some(value) => value
                .parse::<f64>()
                .map(Self::from_layout_fraction)
                .unwrap_or_default(),
            None => Self::default(),
        }
    }

    /// Returns the exact value represented by this size on the slider.
    pub(crate) fn scale_value(self) -> f64 {
        if self.0 <= Self::MEDIUM.0 {
            (self.0 - Self::SMALL.0) as f64 * 100.0 / (Self::MEDIUM.0 - Self::SMALL.0) as f64
        } else {
            100.0
                + (self.0 - Self::MEDIUM.0) as f64 * 100.0 / (Self::LARGE.0 - Self::MEDIUM.0) as f64
        }
    }

    /// Converts a slider value into a valid custom artwork size.
    pub(crate) fn from_scale_value(value: f64) -> Self {
        if !value.is_finite() {
            return Self::default();
        }

        let value = value
            .round()
            .clamp(Self::MIN_SCALE_VALUE, Self::MAX_SCALE_VALUE) as u16;
        if value <= 100 {
            Self(
                Self::SMALL.0
                    + (u32::from(value) * u32::from(Self::MEDIUM.0 - Self::SMALL.0) / 100) as u16,
            )
        } else {
            Self(
                Self::MEDIUM.0
                    + (u32::from(value - 100) * u32::from(Self::LARGE.0 - Self::MEDIUM.0) / 100)
                        as u16,
            )
        }
    }

    /// Returns the nearest marked preset when the slider is close enough to
    /// it to feel magnetic, otherwise retains the user's custom value.
    pub(crate) fn snapped_scale_value(value: f64) -> f64 {
        let value = Self::from_scale_value(value).scale_value();
        Self::SNAP_ANCHORS
            .into_iter()
            .find(|anchor| (value - anchor.scale_value()).abs() <= Self::SNAP_DISTANCE)
            .map_or(value, Self::scale_value)
    }

    /// Converts a slider value to a size after applying the marked-preset snap.
    pub(crate) fn from_snapped_scale_value(value: f64) -> Self {
        Self::from_scale_value(Self::snapped_scale_value(value))
    }

    /// Returns the fraction of the reserved artwork area occupied by the cover.
    pub(crate) fn layout_fraction(self) -> f64 {
        self.0 as f64 / Self::LARGE.0 as f64
    }

    /// Converts a persisted artwork fraction into the nearest slider value.
    fn from_layout_fraction(value: f64) -> Self {
        if !value.is_finite() {
            return Self::default();
        }

        let value = value.clamp(Self::SMALL.layout_fraction(), Self::LARGE.layout_fraction());
        let scale_value = if value <= Self::MEDIUM.layout_fraction() {
            (value - Self::SMALL.layout_fraction()) * 100.0
                / (Self::MEDIUM.layout_fraction() - Self::SMALL.layout_fraction())
        } else {
            100.0
                + (value - Self::MEDIUM.layout_fraction()) * 100.0
                    / (Self::LARGE.layout_fraction() - Self::MEDIUM.layout_fraction())
        };
        Self::from_scale_value(scale_value)
    }
}

impl Serialize for AlbumCoverSize {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.as_preference_value())
    }
}

impl<'de> Deserialize<'de> for AlbumCoverSize {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(|value| Self::from_preference(Some(&value)))
    }
}

/// Defines the lightweight visual transition used when a new track is displayed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum TransitionEffect {
    /// Replace the current track immediately without animation.
    #[default]
    None,
    /// Crossfade the current track into the new track.
    Crossfade,
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
        Self::Crossfade,
        Self::SlideRight,
        Self::SlideLeft,
        Self::SlideUp,
        Self::SlideDown,
        Self::SwingRight,
        Self::SwingLeft,
        Self::SwingUp,
        Self::SwingDown,
    ];

    /// Returns the persisted string representation of this transition effect.
    pub fn as_preference_value(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Crossfade => "fade",
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
            Some("fade") => Self::Crossfade,
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
}

impl Serialize for TransitionEffect {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_preference_value())
    }
}

impl<'de> Deserialize<'de> for TransitionEffect {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(|value| Self::from_preference(Some(&value)))
    }
}

/// Controls where the track metadata is aligned in the window.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum TrackInfoAlignment {
    Left,
    #[default]
    Center,
    Right,
}

impl TrackInfoAlignment {
    /// Returns the persisted string representation of this track-info alignment.
    pub fn as_preference_value(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Center => "center",
            Self::Right => "right",
        }
    }

    /// Parses a persisted track-info alignment value, defaulting to center.
    pub fn from_preference(value: Option<&str>) -> Self {
        match value {
            Some("left") => Self::Left,
            Some("right") => Self::Right,
            _ => Self::Center,
        }
    }
}

impl Serialize for TrackInfoAlignment {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_preference_value())
    }
}

impl<'de> Deserialize<'de> for TrackInfoAlignment {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(|value| Self::from_preference(Some(&value)))
    }
}

/// Selects the overall presentation used by the Now Playing window.
///
/// Settings that are meaningful only to one mode remain persisted when a
/// different mode is selected, so returning to that mode restores its prior
/// configuration.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum DisplayMode {
    #[default]
    Classic,
    Cinema,
    Ambient,
    LightsOff,
}

impl DisplayMode {
    /// All display modes in the order used by selector controls.
    pub const ALL: [Self; 4] = [Self::Classic, Self::Cinema, Self::Ambient, Self::LightsOff];

    /// Returns the persisted string representation of this display mode.
    pub fn as_preference_value(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Cinema => "cinema",
            Self::Ambient => "ambient",
            Self::LightsOff => "lights-off",
        }
    }

    /// Parses a persisted display mode, defaulting to the classic layout.
    pub fn from_preference(value: Option<&str>) -> Self {
        match value {
            // Accept the former value so existing preference files migrate
            // to the canonical Cinema value the next time they are written.
            Some("cinema" | "full-bleed") => Self::Cinema,
            Some("ambient") => Self::Ambient,
            Some("lights-off") => Self::LightsOff,
            _ => Self::Classic,
        }
    }
}

impl Serialize for DisplayMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_preference_value())
    }
}

impl<'de> Deserialize<'de> for DisplayMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(|value| Self::from_preference(Some(&value)))
    }
}

/// Controls the visual treatment of the Now Playing background.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundStyle {
    #[default]
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

    /// Parses a persisted background style, defaulting to a gradient.
    pub fn from_preference(value: Option<&str>) -> Self {
        match value {
            Some("solid") => Self::Solid,
            _ => Self::Gradient,
        }
    }
}

impl Serialize for BackgroundStyle {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_preference_value())
    }
}

impl<'de> Deserialize<'de> for BackgroundStyle {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(|value| Self::from_preference(Some(&value)))
    }
}

/// Settings that apply only to the Classic Now Playing presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ClassicNowPlayingPreferences {
    #[serde(rename = "now_playing_round_corners")]
    pub round_corners: bool,
    #[serde(rename = "now_playing_track_info_alignment")]
    pub track_info_alignment: TrackInfoAlignment,
    #[serde(rename = "now_playing_album_cover_size")]
    pub album_cover_size: AlbumCoverSize,
    #[serde(rename = "now_playing_background_style")]
    pub background_style: BackgroundStyle,
}

impl Default for ClassicNowPlayingPreferences {
    fn default() -> Self {
        Self {
            round_corners: true,
            track_info_alignment: TrackInfoAlignment::default(),
            album_cover_size: AlbumCoverSize::default(),
            background_style: BackgroundStyle::default(),
        }
    }
}

/// Settings and behavior shared by every display mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SharedNowPlayingPreferences {
    #[serde(rename = "hide_now_playing_info")]
    pub hide_track_info: bool,
    pub always_display_last_recognized_song: bool,
    #[serde(rename = "now_playing_transition")]
    pub transition: TransitionEffect,
    #[serde(rename = "now_playing_transition_duration_ms")]
    pub transition_duration_ms: u64,
}

impl Default for SharedNowPlayingPreferences {
    fn default() -> Self {
        Self {
            hide_track_info: false,
            always_display_last_recognized_song: true,
            transition: TransitionEffect::default(),
            transition_duration_ms: TRANSITION_DURATION_DEFAULT_MS,
        }
    }
}

/// The complete persisted presentation state for the Now Playing window.
///
/// The Rust model separates mode applicability, while the flattened serde
/// fields preserve the existing top-level preferences-file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct NowPlayingPreferences {
    #[serde(rename = "now_playing_display_mode")]
    pub display_mode: DisplayMode,
    #[serde(flatten)]
    pub classic: ClassicNowPlayingPreferences,
    #[serde(flatten)]
    pub shared: SharedNowPlayingPreferences,
}

impl Default for NowPlayingPreferences {
    fn default() -> Self {
        Self {
            display_mode: DisplayMode::default(),
            classic: ClassicNowPlayingPreferences::default(),
            shared: SharedNowPlayingPreferences::default(),
        }
    }
}

#[derive(Default, Deserialize)]
struct NowPlayingPreferencesWire {
    #[serde(rename = "now_playing_display_mode")]
    display_mode: Option<DisplayMode>,
    #[serde(rename = "now_playing_round_corners")]
    round_corners: Option<bool>,
    #[serde(rename = "hide_now_playing_info")]
    hide_track_info: Option<bool>,
    #[serde(rename = "now_playing_track_info_alignment")]
    track_info_alignment: Option<TrackInfoAlignment>,
    #[serde(rename = "now_playing_album_cover_size")]
    album_cover_size: Option<AlbumCoverSize>,
    #[serde(rename = "now_playing_background_style")]
    background_style: Option<BackgroundStyle>,
    always_display_last_recognized_song: Option<bool>,
    #[serde(rename = "now_playing_transition")]
    transition: Option<TransitionEffect>,
    #[serde(rename = "now_playing_transition_duration_ms")]
    transition_duration_ms: Option<u64>,
    #[serde(rename = "lights_off_enabled")]
    legacy_lights_off: Option<bool>,
}

impl<'de> Deserialize<'de> for NowPlayingPreferences {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = NowPlayingPreferencesWire::deserialize(deserializer)?;
        let defaults = Self::default();
        let mut preferences = Self {
            display_mode: wire.display_mode.unwrap_or_else(|| {
                if wire.legacy_lights_off.unwrap_or(false) {
                    DisplayMode::LightsOff
                } else {
                    defaults.display_mode
                }
            }),
            classic: ClassicNowPlayingPreferences {
                round_corners: wire.round_corners.unwrap_or(defaults.classic.round_corners),
                track_info_alignment: wire
                    .track_info_alignment
                    .unwrap_or(defaults.classic.track_info_alignment),
                album_cover_size: wire
                    .album_cover_size
                    .unwrap_or(defaults.classic.album_cover_size),
                background_style: wire
                    .background_style
                    .unwrap_or(defaults.classic.background_style),
            },
            shared: SharedNowPlayingPreferences {
                hide_track_info: wire
                    .hide_track_info
                    .unwrap_or(defaults.shared.hide_track_info),
                always_display_last_recognized_song: wire
                    .always_display_last_recognized_song
                    .unwrap_or(defaults.shared.always_display_last_recognized_song),
                transition: wire.transition.unwrap_or(defaults.shared.transition),
                transition_duration_ms: wire
                    .transition_duration_ms
                    .unwrap_or(defaults.shared.transition_duration_ms),
            },
        };
        preferences.normalize();
        Ok(preferences)
    }
}

impl NowPlayingPreferences {
    fn normalize(&mut self) {
        self.shared.transition_duration_ms =
            clamp_transition_duration_ms(self.shared.transition_duration_ms);
    }

    pub(crate) fn apply_change(&mut self, change: NowPlayingPreferenceChange) {
        match change {
            NowPlayingPreferenceChange::Reset => *self = Self::default(),
            NowPlayingPreferenceChange::DisplayMode(value) => self.display_mode = value,
            NowPlayingPreferenceChange::RoundCorners(value) => self.classic.round_corners = value,
            NowPlayingPreferenceChange::HideTrackInfo(value) => {
                self.shared.hide_track_info = value;
            }
            NowPlayingPreferenceChange::TrackInfoAlignment(value) => {
                self.classic.track_info_alignment = value;
            }
            NowPlayingPreferenceChange::AlbumCoverSize(value) => {
                self.classic.album_cover_size = value;
            }
            NowPlayingPreferenceChange::BackgroundStyle(value) => {
                self.classic.background_style = value;
            }
            NowPlayingPreferenceChange::AlwaysDisplayLastRecognizedSong(value) => {
                self.shared.always_display_last_recognized_song = value;
            }
            NowPlayingPreferenceChange::Transition(value) => self.shared.transition = value,
            NowPlayingPreferenceChange::TransitionDurationMs(value) => {
                self.shared.transition_duration_ms = value;
            }
        }
        self.normalize();
    }
}

/// A typed mutation of the persisted Now Playing preferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NowPlayingPreferenceChange {
    Reset,
    DisplayMode(DisplayMode),
    RoundCorners(bool),
    HideTrackInfo(bool),
    TrackInfoAlignment(TrackInfoAlignment),
    AlbumCoverSize(AlbumCoverSize),
    BackgroundStyle(BackgroundStyle),
    AlwaysDisplayLastRecognizedSong(bool),
    Transition(TransitionEffect),
    TransitionDurationMs(u64),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
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
    #[serde(flatten)]
    pub now_playing: NowPlayingPreferences,
}

impl Preferences {
    pub fn with_interval(interval: u64) -> Self {
        Self {
            request_interval_secs_v3: Some(interval),
            ..Self::default()
        }
    }
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
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
            now_playing: NowPlayingPreferences::default(),
        }
    }
}

/// A partial update for application preferences unrelated to Now Playing.
///
/// Keeping this distinct from [`Preferences`] prevents an ordinary preference
/// change from accidentally carrying a second set of Now Playing defaults.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreferencesPatch {
    pub enable_notifications: Option<bool>,
    pub enable_systray: Option<bool>,
    pub enable_mpris_v2: Option<bool>,
    pub no_duplicates: Option<bool>,
    pub request_interval_secs_v3: Option<u64>,
    pub current_device_name: Option<String>,
    pub website_search_url: Option<String>,
    pub website_search_text: Option<String>,
}

impl PreferencesPatch {
    pub fn new() -> Self {
        Self::default()
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

    pub fn update(&mut self, update: PreferencesPatch) {
        let current = &mut self.preferences;
        current.enable_notifications = update.enable_notifications.or(current.enable_notifications);
        current.enable_mpris_v2 = update
            .enable_mpris_v2
            .or(current.enable_mpris_v2)
            .or(current.enable_mpris);
        current.enable_mpris = None;
        current.enable_systray = update.enable_systray.or(current.enable_systray);
        current.no_duplicates = update.no_duplicates.or(current.no_duplicates);

        let migrated_interval = match current.request_interval_secs {
            Some(4) | None => None,
            Some(value) => Some(value),
        }
        .or(match current.request_interval_secs_v2 {
            Some(10) | None => None,
            Some(value) => Some(value),
        });
        current.request_interval_secs_v3 = update
            .request_interval_secs_v3
            .or(migrated_interval)
            .or(current.request_interval_secs_v3);
        current.buffer_size_secs = None;
        current.request_interval_secs = None;
        current.request_interval_secs_v2 = None;

        current.current_device_name = update
            .current_device_name
            .or_else(|| current.current_device_name.clone());
        current.website_search_url = update
            .website_search_url
            .or_else(|| current.website_search_url.clone());
        current.website_search_text = update
            .website_search_text
            .or_else(|| current.website_search_text.clone());

        self.write_after_update();
    }

    pub fn update_now_playing(&mut self, change: NowPlayingPreferenceChange) {
        self.preferences.now_playing.apply_change(change);
        self.write_after_update();
    }

    fn write_after_update(&self) {
        if let Err(error) = self.write() {
            error!("{} {}", gettext("When saving the preferences file:"), error);
        }
    }

    fn write(&self) -> Result<(), Box<dyn Error>> {
        if let Some(preferences_file_path) = &self.preferences_file_path {
            let contents = toml::to_string(&self.preferences)?;
            std::fs::write(preferences_file_path, contents)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AlbumCoverSize, BackgroundStyle, ClassicNowPlayingPreferences, DisplayMode,
        NowPlayingPreferenceChange, NowPlayingPreferences, Preferences, PreferencesInterface,
        PreferencesPatch, SharedNowPlayingPreferences, TRANSITION_DURATION_DEFAULT_MS,
        TRANSITION_DURATION_MAX_MS, TRANSITION_DURATION_MIN_MS, TrackInfoAlignment,
        TransitionEffect,
    };

    #[test]
    fn now_playing_defaults_have_one_typed_source() {
        let defaults = NowPlayingPreferences::default();

        assert_eq!(defaults.display_mode, DisplayMode::Classic);
        assert!(defaults.classic.round_corners);
        assert_eq!(
            defaults.classic.track_info_alignment,
            TrackInfoAlignment::Center
        );
        assert_eq!(
            defaults.classic.album_cover_size,
            AlbumCoverSize::MEDIUM_LARGE_MIDPOINT
        );
        assert_eq!(defaults.classic.background_style, BackgroundStyle::Gradient);
        assert!(!defaults.shared.hide_track_info);
        assert!(defaults.shared.always_display_last_recognized_song);
        assert_eq!(defaults.shared.transition, TransitionEffect::None);
        assert_eq!(
            defaults.shared.transition_duration_ms,
            TRANSITION_DURATION_DEFAULT_MS
        );
        assert_eq!(Preferences::default().now_playing, defaults);
    }

    #[test]
    fn now_playing_preferences_keep_the_existing_flat_toml_schema() {
        let serialized = toml::to_string(&Preferences::default()).unwrap();
        let table = serialized.parse::<toml::Table>().unwrap();

        assert!(!table.contains_key("now_playing"));
        assert_eq!(table["now_playing_display_mode"].as_str(), Some("classic"));
        assert_eq!(table["now_playing_round_corners"].as_bool(), Some(true));
        assert_eq!(table["hide_now_playing_info"].as_bool(), Some(false));
        assert_eq!(
            table["now_playing_track_info_alignment"].as_str(),
            Some("center")
        );
        assert_eq!(
            table["now_playing_album_cover_size"].as_str(),
            Some("0.8500")
        );
        assert_eq!(
            table["now_playing_background_style"].as_str(),
            Some("gradient")
        );
        assert_eq!(table["now_playing_transition"].as_str(), Some("none"));
        assert_eq!(
            table["now_playing_transition_duration_ms"].as_integer(),
            Some(2_000)
        );
        assert!(!table.contains_key("lights_off_enabled"));
    }

    #[test]
    fn existing_flat_preferences_deserialize_into_typed_values() {
        let preferences: Preferences = toml::from_str(
            r#"
now_playing_round_corners = false
hide_now_playing_info = true
now_playing_track_info_alignment = "left"
now_playing_album_cover_size = "small"
now_playing_background_style = "solid"
always_display_last_recognized_song = false
now_playing_transition = "slide-up"
now_playing_transition_duration_ms = 3500
lights_off_enabled = false
"#,
        )
        .unwrap();

        assert_eq!(
            preferences.now_playing,
            NowPlayingPreferences {
                display_mode: DisplayMode::Classic,
                classic: ClassicNowPlayingPreferences {
                    round_corners: false,
                    track_info_alignment: TrackInfoAlignment::Left,
                    album_cover_size: AlbumCoverSize::SMALL,
                    background_style: BackgroundStyle::Solid,
                },
                shared: SharedNowPlayingPreferences {
                    hide_track_info: true,
                    always_display_last_recognized_song: false,
                    transition: TransitionEffect::SlideUp,
                    transition_duration_ms: 3_500,
                },
            }
        );
    }

    #[test]
    fn legacy_lights_off_is_migrated_without_discarding_classic_settings() {
        let preferences: Preferences = toml::from_str(
            r#"
hide_now_playing_info = true
now_playing_round_corners = false
now_playing_transition_duration_ms = 1
lights_off_enabled = true
"#,
        )
        .unwrap();

        assert_eq!(preferences.now_playing.display_mode, DisplayMode::LightsOff);
        assert!(preferences.now_playing.shared.hide_track_info);
        assert!(!preferences.now_playing.classic.round_corners);
        assert_eq!(
            preferences.now_playing.shared.transition_duration_ms,
            TRANSITION_DURATION_MIN_MS
        );

        let migrated = toml::to_string(&preferences).unwrap();
        let migrated_table = migrated.parse::<toml::Table>().unwrap();
        assert_eq!(
            migrated_table["now_playing_display_mode"].as_str(),
            Some("lights-off")
        );
        assert_eq!(
            migrated_table["hide_now_playing_info"].as_bool(),
            Some(true)
        );
        assert!(!migrated_table.contains_key("lights_off_enabled"));
    }

    #[test]
    fn explicit_display_mode_takes_precedence_over_legacy_lights_off() {
        let preferences: Preferences = toml::from_str(
            r#"
now_playing_display_mode = "ambient"
lights_off_enabled = true
"#,
        )
        .unwrap();

        assert_eq!(preferences.now_playing.display_mode, DisplayMode::Ambient);
    }

    #[test]
    fn previous_cinema_value_migrates_to_the_canonical_value() {
        let preferences: Preferences = toml::from_str(
            r#"
now_playing_display_mode = "full-bleed"
"#,
        )
        .unwrap();

        assert_eq!(preferences.now_playing.display_mode, DisplayMode::Cinema);

        let migrated = toml::to_string(&preferences).unwrap();
        let migrated_table = migrated.parse::<toml::Table>().unwrap();
        assert_eq!(
            migrated_table["now_playing_display_mode"].as_str(),
            Some("cinema")
        );
    }

    #[test]
    fn display_mode_and_scoped_settings_round_trip_together() {
        for display_mode in DisplayMode::ALL {
            let preferences = Preferences {
                now_playing: NowPlayingPreferences {
                    display_mode,
                    classic: ClassicNowPlayingPreferences {
                        round_corners: false,
                        background_style: BackgroundStyle::Solid,
                        ..ClassicNowPlayingPreferences::default()
                    },
                    shared: SharedNowPlayingPreferences {
                        hide_track_info: true,
                        ..SharedNowPlayingPreferences::default()
                    },
                    ..NowPlayingPreferences::default()
                },
                ..Preferences::default()
            };

            let serialized = toml::to_string(&preferences).unwrap();
            let deserialized: Preferences = toml::from_str(&serialized).unwrap();

            assert_eq!(deserialized.now_playing, preferences.now_playing);
        }
    }

    #[test]
    fn general_patch_preserves_now_playing_preferences() {
        let preferences = Preferences {
            now_playing: NowPlayingPreferences {
                classic: ClassicNowPlayingPreferences {
                    background_style: BackgroundStyle::Solid,
                    ..ClassicNowPlayingPreferences::default()
                },
                ..NowPlayingPreferences::default()
            },
            ..Preferences::default()
        };
        let expected_now_playing = preferences.now_playing;
        let mut interface = PreferencesInterface {
            preferences_file_path: None,
            preferences,
        };
        let mut patch = PreferencesPatch::new();
        patch.enable_notifications = Some(false);

        interface.update(patch);

        assert_eq!(interface.preferences.enable_notifications, Some(false));
        assert_eq!(interface.preferences.now_playing, expected_now_playing);
    }

    #[test]
    fn reset_preserves_general_preferences() {
        let preferences = Preferences {
            enable_notifications: Some(false),
            now_playing: NowPlayingPreferences {
                display_mode: DisplayMode::LightsOff,
                classic: ClassicNowPlayingPreferences {
                    round_corners: false,
                    album_cover_size: AlbumCoverSize::SMALL,
                    ..ClassicNowPlayingPreferences::default()
                },
                shared: SharedNowPlayingPreferences {
                    hide_track_info: true,
                    transition_duration_ms: TRANSITION_DURATION_MAX_MS,
                    ..SharedNowPlayingPreferences::default()
                },
                ..NowPlayingPreferences::default()
            },
            ..Preferences::default()
        };
        let mut interface = PreferencesInterface {
            preferences_file_path: None,
            preferences,
        };

        interface.update_now_playing(NowPlayingPreferenceChange::Reset);

        assert_eq!(interface.preferences.enable_notifications, Some(false));
        assert_eq!(
            interface.preferences.now_playing,
            NowPlayingPreferences::default()
        );
    }

    #[test]
    fn now_playing_updates_are_normalized() {
        let mut interface = PreferencesInterface {
            preferences_file_path: None,
            preferences: Preferences::default(),
        };

        interface.update_now_playing(NowPlayingPreferenceChange::HideTrackInfo(true));
        interface.update_now_playing(NowPlayingPreferenceChange::RoundCorners(false));
        interface.update_now_playing(NowPlayingPreferenceChange::DisplayMode(
            DisplayMode::Ambient,
        ));
        interface.update_now_playing(NowPlayingPreferenceChange::DisplayMode(
            DisplayMode::LightsOff,
        ));
        interface.update_now_playing(NowPlayingPreferenceChange::DisplayMode(
            DisplayMode::Classic,
        ));
        assert!(interface.preferences.now_playing.shared.hide_track_info);
        assert!(!interface.preferences.now_playing.classic.round_corners);

        interface.update_now_playing(NowPlayingPreferenceChange::TransitionDurationMs(1));
        assert_eq!(
            interface
                .preferences
                .now_playing
                .shared
                .transition_duration_ms,
            TRANSITION_DURATION_MIN_MS
        );
        interface.update_now_playing(NowPlayingPreferenceChange::TransitionDurationMs(u64::MAX));
        assert_eq!(
            interface
                .preferences
                .now_playing
                .shared
                .transition_duration_ms,
            TRANSITION_DURATION_MAX_MS
        );
    }

    #[test]
    fn persisted_display_options_round_trip() {
        for mode in DisplayMode::ALL {
            assert_eq!(
                DisplayMode::from_preference(Some(mode.as_preference_value())),
                mode
            );
        }

        for effect in TransitionEffect::ALL {
            assert_eq!(
                TransitionEffect::from_preference(Some(effect.as_preference_value())),
                effect
            );
        }

        for alignment in [
            TrackInfoAlignment::Left,
            TrackInfoAlignment::Center,
            TrackInfoAlignment::Right,
        ] {
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

        for size in [
            AlbumCoverSize::SMALL,
            AlbumCoverSize::SMALL_MEDIUM_MIDPOINT,
            AlbumCoverSize::MEDIUM,
            AlbumCoverSize::MEDIUM_LARGE_MIDPOINT,
            AlbumCoverSize::LARGE,
        ] {
            let preference_value = size.as_preference_value();
            assert_eq!(
                AlbumCoverSize::from_preference(Some(&preference_value)),
                size
            );
            assert_eq!(AlbumCoverSize::from_scale_value(size.scale_value()), size);
            assert_eq!(
                AlbumCoverSize::from_snapped_scale_value(size.scale_value() + 1.0),
                size
            );
        }
    }

    #[test]
    fn right_track_info_alignment_round_trips_through_flat_toml() {
        let preferences: Preferences = toml::from_str(
            r#"
now_playing_track_info_alignment = "right"
"#,
        )
        .unwrap();

        assert_eq!(
            preferences.now_playing.classic.track_info_alignment,
            TrackInfoAlignment::Right
        );

        let serialized = toml::to_string(&preferences).unwrap();
        let table = serialized.parse::<toml::Table>().unwrap();
        assert_eq!(
            table["now_playing_track_info_alignment"].as_str(),
            Some("right")
        );
    }
}
