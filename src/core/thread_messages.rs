//! This module contains code used from message-based communication between threads.

use crate::core::artwork::Artwork;
use crate::core::fingerprinting::signature_format::DecodedSignature;
#[cfg(feature = "gui")]
use crate::core::preferences::{NowPlayingPreferenceChange, PreferencesPatch};

use std::sync::Arc;
use std::thread;

pub fn spawn_big_thread<F, T>(argument: F)
where
    F: std::ops::FnOnce() -> T,
    F: std::marker::Send + 'static,
    T: std::marker::Send + 'static,
{
    thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(argument)
        .unwrap();
}

#[derive(Debug, Clone)]
pub struct SongRecognizedMessage {
    pub artist_name: String,
    pub album_name: Option<String>,
    pub song_name: String,
    pub cover_image: Option<Arc<Artwork>>,
    /// Whether an out-of-band artwork request is still in progress.
    pub artwork_pending: bool,

    // Used only in the CSV export for now:
    pub track_key: String,
    pub release_year: Option<String>,
    pub genre: Option<String>,

    pub shazam_json: String,
}

impl SongRecognizedMessage {
    pub fn with_cover_image(&self, cover_image: Arc<Artwork>) -> Self {
        let mut updated = self.clone();
        updated.cover_image = Some(cover_image);
        updated.artwork_pending = false;
        updated
    }

    pub fn with_artwork_unavailable(&self) -> Self {
        let mut updated = self.clone();
        updated.artwork_pending = false;
        updated
    }
}

/// Latest recognition outcome plus the last successful track needed by the
/// "always display last recognized song" preference.
#[derive(Debug, Clone, Default)]
pub enum RecognitionState {
    #[default]
    NeverAttempted,
    NoMatch {
        last_recognized: Option<Arc<SongRecognizedMessage>>,
    },
    Recognized(Arc<SongRecognizedMessage>),
}

impl RecognitionState {
    pub fn record_recognition(&mut self, track: Arc<SongRecognizedMessage>) {
        *self = Self::Recognized(track);
    }

    pub fn record_no_match(&mut self) {
        let last_recognized = match self {
            Self::NeverAttempted => None,
            Self::NoMatch { last_recognized } => last_recognized.clone(),
            Self::Recognized(track) => Some(track.clone()),
        };
        *self = Self::NoMatch { last_recognized };
    }

    /// Attaches a separately downloaded cover to the matching cached track.
    pub fn apply_artwork(&mut self, track_key: &str, artwork: Arc<Artwork>) -> bool {
        let update = |track: &Arc<SongRecognizedMessage>| {
            (track.track_key == track_key)
                .then(|| Arc::new(track.with_cover_image(artwork.clone())))
        };

        match self {
            Self::Recognized(track) => {
                let Some(updated) = update(track) else {
                    return false;
                };
                *track = updated;
                true
            }
            Self::NoMatch { last_recognized } => {
                let Some(updated) = last_recognized.as_ref().and_then(update) else {
                    return false;
                };
                *last_recognized = Some(updated);
                true
            }
            Self::NeverAttempted => false,
        }
    }

    /// Marks a matching separately fetched cover as definitively unavailable.
    pub fn apply_artwork_unavailable(&mut self, track_key: &str) -> bool {
        let update = |track: &Arc<SongRecognizedMessage>| {
            (track.track_key == track_key).then(|| Arc::new(track.with_artwork_unavailable()))
        };

        match self {
            Self::Recognized(track) => {
                let Some(updated) = update(track) else {
                    return false;
                };
                *track = updated;
                true
            }
            Self::NoMatch { last_recognized } => {
                let Some(updated) = last_recognized.as_ref().and_then(update) else {
                    return false;
                };
                *last_recognized = Some(updated);
                true
            }
            Self::NeverAttempted => false,
        }
    }

    pub fn visible_track(
        &self,
        always_display_last_recognized_song: bool,
    ) -> Option<Arc<SongRecognizedMessage>> {
        match self {
            Self::Recognized(track) => Some(track.clone()),
            Self::NoMatch { last_recognized } if always_display_last_recognized_song => {
                last_recognized.clone()
            }
            Self::NeverAttempted | Self::NoMatch { .. } => None,
        }
    }

    pub fn last_recognized(&self) -> Option<Arc<SongRecognizedMessage>> {
        match self {
            Self::Recognized(track) => Some(track.clone()),
            Self::NoMatch { last_recognized } => last_recognized.clone(),
            Self::NeverAttempted => None,
        }
    }
}

#[derive(Debug)]
pub struct DeviceListItem {
    pub inner_name: String,
    pub display_name: String,
    // The checkbox option on the UI should select the first monitor
    // device present in the combo box, when specified
    pub is_monitor: bool,
}

#[derive(Debug)]
pub enum GUIMessage {
    ErrorMessage(String),
    ShowWindow,
    QuitApplication,
    // A list of audio devices, received from the microphone thread
    // because CPAL can't be called from the same thread as the GUI
    // under Windows
    DevicesList(Vec<DeviceListItem>),
    #[cfg(feature = "gui")]
    UpdatePreference(PreferencesPatch),
    #[cfg(feature = "gui")]
    NowPlayingPreferenceChanged {
        change: NowPlayingPreferenceChange,
        persist: bool,
    },
    NetworkStatus(bool),  // Is the network reachable?
    RateLimitState(bool), // Are we rate-limited?
    #[cfg(feature = "gui")]
    WipeSongHistory,
    #[cfg(feature = "gui")]
    AppendToLog(String),
    MicrophoneRecording,
    MicrophoneVolumePercent(f32),
    SongRecognized(Arc<SongRecognizedMessage>),
    ArtworkDownloaded {
        track_key: String,
        artwork: Arc<Artwork>,
    },
    ArtworkUnavailable {
        track_key: String,
    },
    NoRecognition,
}

pub enum MicrophoneMessage {
    MicrophoneRecordStart(String), // The argument is the audio device name
    MicrophoneRecordSetDevice(String), // The argument is the audio device name (with an initialization delay)
    RefreshDevices,
    MicrophoneRecordStop,
    ProcessingDone,
}

pub enum ProcessingMessage {
    ProcessAudioFile(String),
    ProcessAudioSamples(Vec<f32>), // Prefer to use heap across threads to avoid stack overflow
}

pub enum HTTPMessage {
    RecognizeSignature(Box<DecodedSignature>),
}

#[cfg(test)]
mod tests {
    use super::{RecognitionState, SongRecognizedMessage};
    use crate::core::artwork::Artwork;
    use image::{DynamicImage, ImageFormat};
    use std::io::Cursor;
    use std::sync::Arc;

    fn track(key: &str) -> Arc<SongRecognizedMessage> {
        Arc::new(SongRecognizedMessage {
            artist_name: "Artist".to_string(),
            album_name: None,
            song_name: format!("Song {key}"),
            cover_image: None,
            artwork_pending: true,
            track_key: key.to_string(),
            release_year: None,
            genre: None,
            shazam_json: "{}".to_string(),
        })
    }

    fn artwork() -> Arc<Artwork> {
        let image = DynamicImage::new_rgba8(1, 1);
        let mut encoded = Cursor::new(Vec::new());
        image.write_to(&mut encoded, ImageFormat::Png).unwrap();
        Arc::new(Artwork::decode(encoded.into_inner()).unwrap())
    }

    #[test]
    fn no_match_retains_last_track_without_making_it_unconditionally_visible() {
        let mut state = RecognitionState::default();
        state.record_recognition(track("a"));
        state.record_no_match();

        assert!(state.visible_track(false).is_none());
        assert_eq!(
            state
                .visible_track(true)
                .as_deref()
                .map(|track| track.track_key.as_str()),
            Some("a")
        );
    }

    #[test]
    fn never_attempted_and_no_match_are_distinct() {
        let mut state = RecognitionState::default();
        assert!(matches!(state, RecognitionState::NeverAttempted));
        state.record_no_match();
        assert!(matches!(
            state,
            RecognitionState::NoMatch {
                last_recognized: None
            }
        ));
    }

    #[test]
    fn downloaded_artwork_clears_the_pending_flag() {
        let mut state = RecognitionState::default();
        state.record_recognition(track("a"));

        assert!(state.apply_artwork("a", artwork()));
        let updated = state.last_recognized().unwrap();
        assert!(updated.cover_image.is_some());
        assert!(!updated.artwork_pending);
    }

    #[test]
    fn unavailable_artwork_clears_the_pending_flag_for_visible_or_retained_tracks() {
        let mut recognized = RecognitionState::default();
        recognized.record_recognition(track("a"));
        assert!(recognized.apply_artwork_unavailable("a"));
        assert!(!recognized.last_recognized().unwrap().artwork_pending);

        let mut retained = RecognitionState::default();
        retained.record_recognition(track("b"));
        retained.record_no_match();
        assert!(retained.apply_artwork_unavailable("b"));
        assert!(!retained.last_recognized().unwrap().artwork_pending);
    }

    #[test]
    fn artwork_updates_ignore_stale_track_keys() {
        let mut state = RecognitionState::default();
        state.record_recognition(track("current"));

        assert!(!state.apply_artwork_unavailable("stale"));
        assert!(state.last_recognized().unwrap().artwork_pending);
        assert!(!state.apply_artwork("stale", artwork()));
        let current = state.last_recognized().unwrap();
        assert!(current.cover_image.is_none());
        assert!(current.artwork_pending);
    }
}
