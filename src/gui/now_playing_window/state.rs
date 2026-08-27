//! Mutable state that is shared by Now Playing event handlers.

use super::NowPlayingSettings;
use super::background::CachedGradient;
use super::palette::{Background, PreparedArtwork};
use crate::core::thread_messages::SongRecognizedMessage;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// The content currently represented by the artwork portion of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PresentationMode {
    Listening,
    TrackWithArtwork,
    TrackWithoutArtwork,
}

/// The subset of a recognition result needed after its cover has been decoded.
///
/// Keeping prepared UI data here avoids retaining another encoded cover and the
/// full Shazam response while a transition is pending.
#[derive(Clone)]
pub(super) struct PresentedTrack {
    pub(super) track_key: String,
    pub(super) song_name: String,
    pub(super) artist_name: String,
    pub(super) album_name: Option<String>,
    pub(super) release_year: Option<String>,
    pub(super) artwork: Option<PreparedArtwork>,
    pub(super) artwork_pending: bool,
}

impl PresentedTrack {
    pub(super) fn from_message(
        message: &SongRecognizedMessage,
        artwork: Option<PreparedArtwork>,
    ) -> Self {
        Self {
            track_key: message.track_key.clone(),
            song_name: message.song_name.clone(),
            artist_name: message.artist_name.clone(),
            album_name: message.album_name.clone(),
            release_year: message.release_year.clone(),
            artwork,
            artwork_pending: message.artwork_pending,
        }
    }

    pub(super) fn has_visible_information(&self) -> bool {
        !self.song_name.trim().is_empty()
            || !self.artist_name.trim().is_empty()
            || self
                .album_name
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            || self
                .release_year
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            || self.artwork.is_some()
    }

    fn presentation_mode(&self) -> PresentationMode {
        if self.artwork.is_some() {
            PresentationMode::TrackWithArtwork
        } else {
            PresentationMode::TrackWithoutArtwork
        }
    }
}

/// A rendering operation produced by the pure track state machine.
pub(super) enum PresentationAction {
    None,
    BeginTransition,
    RenderTrack(Rc<PresentedTrack>),
    RenderListening,
}

/// Tracks what is actually on screen separately from what is waiting behind a transition.
pub(super) struct TrackPresentationState {
    pub(super) displayed_track: Option<Rc<PresentedTrack>>,
    pub(super) pending_track: Option<Rc<PresentedTrack>>,
    pub(super) mode: PresentationMode,
}

impl Default for TrackPresentationState {
    fn default() -> Self {
        Self {
            displayed_track: None,
            pending_track: None,
            mode: PresentationMode::Listening,
        }
    }
}

impl TrackPresentationState {
    /// Accepts the latest recognition, either committing it or making it the
    /// sole pending track behind the current hide animation.
    pub(super) fn receive_track(
        &mut self,
        track: Rc<PresentedTrack>,
        can_animate: bool,
    ) -> PresentationAction {
        if can_animate && self.pending_track.is_some() {
            self.pending_track = Some(track);
            return PresentationAction::None;
        }

        let should_transition = can_animate
            && !track.track_key.is_empty()
            && self.displayed_track.as_ref().is_some_and(|displayed| {
                !displayed.track_key.is_empty() && displayed.track_key != track.track_key
            });

        if should_transition {
            self.pending_track = Some(track);
            PresentationAction::BeginTransition
        } else {
            self.commit_track(track)
        }
    }

    /// Commits the latest pending recognition once the old content is hidden.
    pub(super) fn transition_hidden(&mut self) -> PresentationAction {
        self.pending_track
            .take()
            .map_or(PresentationAction::None, |track| self.commit_track(track))
    }

    /// Makes a pending track authoritative without waiting for an animation.
    pub(super) fn flush_pending_track(&mut self) -> PresentationAction {
        self.transition_hidden()
    }

    /// Resolves a no-match result according to the keep-last preference.
    pub(super) fn no_recognition(&mut self, keep_last: bool) -> PresentationAction {
        if keep_last {
            return self.flush_pending_track();
        }

        self.show_listening()
    }

    pub(super) fn show_listening(&mut self) -> PresentationAction {
        self.displayed_track = None;
        self.pending_track = None;
        self.mode = PresentationMode::Listening;
        PresentationAction::RenderListening
    }

    fn commit_track(&mut self, track: Rc<PresentedTrack>) -> PresentationAction {
        self.mode = track.presentation_mode();
        self.pending_track = None;
        self.displayed_track = Some(track.clone());
        PresentationAction::RenderTrack(track)
    }
}

pub(super) struct NowPlayingState {
    /// The resolved preference snapshot. Event handlers update this first so
    /// renderers and controls share the same presentation choices.
    pub(super) settings: Rc<Cell<NowPlayingSettings>>,
    /// Prevents GTK notifications emitted by programmatic control updates from
    /// being treated as fresh user preference changes.
    pub(super) applying_settings: Rc<Cell<bool>>,
    pub(super) gradient_surface: Rc<RefCell<Option<CachedGradient>>>,
    pub(super) current_background: Rc<Cell<Background>>,
    pub(super) track_presentation: Rc<RefCell<TrackPresentationState>>,
}

impl NowPlayingState {
    pub(super) fn new(settings: Rc<Cell<NowPlayingSettings>>) -> Self {
        Self {
            settings,
            applying_settings: Rc::new(Cell::new(false)),
            gradient_surface: Rc::new(RefCell::new(None)),
            current_background: Rc::new(Cell::new(Background::fallback())),
            track_presentation: Rc::new(RefCell::new(TrackPresentationState::default())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PresentationAction, PresentationMode, PresentedTrack, TrackPresentationState};
    use std::rc::Rc;

    fn track(key: &str) -> Rc<PresentedTrack> {
        Rc::new(PresentedTrack {
            track_key: key.to_string(),
            song_name: format!("Song {key}"),
            artist_name: "Artist".to_string(),
            album_name: None,
            release_year: None,
            artwork: None,
            artwork_pending: false,
        })
    }

    fn assert_rendered_track(action: PresentationAction, expected_key: &str) {
        let PresentationAction::RenderTrack(track) = action else {
            panic!("expected a rendered track");
        };
        assert_eq!(track.track_key, expected_key);
    }

    #[test]
    fn no_match_commits_the_latest_pending_track_when_keep_last_is_enabled() {
        let mut state = TrackPresentationState::default();
        assert_rendered_track(state.receive_track(track("a"), true), "a");
        assert!(matches!(
            state.receive_track(track("b"), true),
            PresentationAction::BeginTransition
        ));

        assert_rendered_track(state.no_recognition(true), "b");
        assert_eq!(
            state
                .displayed_track
                .as_ref()
                .map(|track| track.track_key.as_str()),
            Some("b")
        );
        assert!(state.pending_track.is_none());
    }

    #[test]
    fn no_match_discards_pending_and_displayed_tracks_when_keep_last_is_disabled() {
        let mut state = TrackPresentationState::default();
        state.receive_track(track("a"), true);
        state.receive_track(track("b"), true);

        assert!(matches!(
            state.no_recognition(false),
            PresentationAction::RenderListening
        ));
        assert!(state.displayed_track.is_none());
        assert!(state.pending_track.is_none());
        assert_eq!(state.mode, PresentationMode::Listening);
    }

    #[test]
    fn in_flight_transition_keeps_only_the_newest_pending_track() {
        let mut state = TrackPresentationState::default();
        state.receive_track(track("a"), true);
        state.receive_track(track("b"), true);

        assert!(matches!(
            state.receive_track(track("c"), true),
            PresentationAction::None
        ));
        assert_rendered_track(state.transition_hidden(), "c");
        assert_eq!(
            state
                .displayed_track
                .as_ref()
                .map(|track| track.track_key.as_str()),
            Some("c")
        );
    }

    #[test]
    fn hidden_window_updates_commit_immediately_without_a_pending_track() {
        let mut state = TrackPresentationState::default();
        state.receive_track(track("a"), false);

        assert_rendered_track(state.receive_track(track("b"), false), "b");
        assert!(state.pending_track.is_none());
    }

    #[test]
    fn track_without_artwork_has_a_distinct_mode_from_listening() {
        let mut state = TrackPresentationState::default();
        state.receive_track(track("a"), false);

        assert_eq!(state.mode, PresentationMode::TrackWithoutArtwork);
    }
}
