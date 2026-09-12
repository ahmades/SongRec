//! Mutable state that is shared by Now Playing event handlers.

use super::NowPlayingSettings;
use super::background::CachedGradient;
use super::palette::{ArtworkRequirement, Background, PreparedArtwork};
use crate::core::artwork::Artwork;
use crate::core::thread_messages::SongRecognizedMessage;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Weak};
use std::time::Instant;

const PREPARED_ARTWORK_CACHE_CAPACITY: usize = 4;
const PREPARED_ARTWORK_CACHE_MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_ARTWORK_PREPARATIONS: usize = 2;

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
    pub(super) response_received_at: Option<i64>,
    pub(super) track_key: String,
    pub(super) song_name: String,
    pub(super) artist_name: String,
    pub(super) album_name: Option<String>,
    pub(super) release_year: Option<String>,
    pub(super) artwork: Option<PreparedArtwork>,
    pub(super) artwork_pending: bool,
    expected_artwork_source: Option<Arc<Artwork>>,
}

impl PresentedTrack {
    pub(super) fn from_message(
        message: &SongRecognizedMessage,
        artwork: Option<PreparedArtwork>,
        visuals_pending: bool,
    ) -> Self {
        Self {
            response_received_at: message.response_received_at,
            track_key: message.track_key.clone(),
            song_name: message.song_name.clone(),
            artist_name: message.artist_name.clone(),
            album_name: message.album_name.clone(),
            release_year: message.release_year.clone(),
            artwork,
            artwork_pending: message.artwork_pending() || visuals_pending,
            expected_artwork_source: message.cover_image().cloned(),
        }
    }

    fn with_artwork(&self, artwork: PreparedArtwork) -> Self {
        Self {
            response_received_at: self.response_received_at,
            track_key: self.track_key.clone(),
            song_name: self.song_name.clone(),
            artist_name: self.artist_name.clone(),
            album_name: self.album_name.clone(),
            release_year: self.release_year.clone(),
            artwork: Some(artwork),
            artwork_pending: false,
            expected_artwork_source: self.expected_artwork_source.clone(),
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
            || self.expected_artwork_source.is_some()
    }

    fn presentation_mode(&self) -> PresentationMode {
        if self.artwork.is_some() {
            PresentationMode::TrackWithArtwork
        } else {
            PresentationMode::TrackWithoutArtwork
        }
    }

    fn matches_artwork_source(&self, track_key: &str, source: &Arc<Artwork>) -> bool {
        self.track_key == track_key
            && self
                .expected_artwork_source
                .as_ref()
                .is_some_and(|expected| expected.content_id() == source.content_id())
    }

    fn awaits_artwork(&self, requirement: ArtworkRequirement) -> bool {
        requirement != ArtworkRequirement::None
            && (self.artwork_pending
                || self
                    .artwork
                    .as_ref()
                    .is_some_and(|artwork| !artwork.is_ready(requirement)))
    }

    fn same_presentation(&self, other: &Self) -> bool {
        self.track_key == other.track_key
            && self.song_name == other.song_name
            && self.artist_name == other.artist_name
            && self.album_name == other.album_name
            && self.release_year == other.release_year
            && self.artwork_pending == other.artwork_pending
            && match (
                &self.expected_artwork_source,
                &other.expected_artwork_source,
            ) {
                (Some(a), Some(b)) => a.content_id() == b.content_id(),
                (None, None) => true,
                _ => false,
            }
            && match (&self.artwork, &other.artwork) {
                (Some(a), Some(b)) => {
                    a.texture == b.texture && a.ambient_texture == b.ambient_texture
                }
                (None, None) => true,
                _ => false,
            }
    }
}

/// A rendering operation produced by the pure track state machine.
pub(super) enum PresentationAction {
    None,
    BeginTransition,
    /// Preserve the revealer's current position while an in-flight replacement catches up.
    HoldTransition,
    RenderTrack(Rc<PresentedTrack>),
    RenderListening,
}

/// Tracks what is actually on screen separately from what is waiting behind a transition.
pub(super) struct TrackPresentationState {
    pub(super) displayed_track: Option<Rc<PresentedTrack>>,
    pub(super) pending_track: Option<Rc<PresentedTrack>>,
    /// Latest recognition received after the active transition's scene was fixed.
    queued_track: Option<Rc<PresentedTrack>>,
    pending_transition_phase: Option<PendingTransitionPhase>,
    prepared_cache: VecDeque<PreparedArtwork>,
    pub(super) mode: PresentationMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingTransitionPhase {
    /// The outgoing scene remains fully visible while its replacement is prepared.
    AwaitingArtworkVisible,
    /// The existing GTK revealer is animating towards its hidden midpoint.
    Hiding,
    /// The replacement is being revealed; newer recognitions wait for completion.
    Revealing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreparedArtworkTarget {
    Queued,
    Pending,
    Displayed,
}

impl Default for TrackPresentationState {
    fn default() -> Self {
        Self {
            displayed_track: None,
            pending_track: None,
            queued_track: None,
            pending_transition_phase: None,
            prepared_cache: VecDeque::new(),
            mode: PresentationMode::Listening,
        }
    }
}

impl TrackPresentationState {
    /// Reuses already prepared UI artwork for a repeated recognition update.
    pub(super) fn prepared_artwork_for(&self, artwork: &Arc<Artwork>) -> Option<PreparedArtwork> {
        self.queued_track
            .as_ref()
            .into_iter()
            .chain(self.pending_track.as_ref())
            .chain(self.displayed_track.as_ref())
            .filter_map(|track| track.artwork.as_ref())
            .chain(self.prepared_cache.iter())
            .filter(|prepared| prepared.matches(artwork))
            .max_by_key(|prepared| prepared.ambient_texture.is_some())
            .cloned()
    }

    /// Finds the current recipient before spending main-thread work on textures.
    /// Album tracks can share the same decoded source, including while a job runs.
    pub(super) fn track_key_for_artwork(
        &self,
        source: &Arc<Artwork>,
        requirement: ArtworkRequirement,
    ) -> Option<String> {
        if requirement == ArtworkRequirement::None {
            return None;
        }
        self.queued_track
            .as_ref()
            .or(self.pending_track.as_ref())
            .or(self.displayed_track.as_ref())
            .filter(|track| {
                track
                    .artwork
                    .as_ref()
                    .is_none_or(|artwork| !artwork.is_ready(requirement))
                    && track.matches_artwork_source(&track.track_key, source)
            })
            .map(|track| track.track_key.clone())
    }

    /// Retains a source for a lazy mode upgrade even after the acquisition cache evicts it.
    pub(super) fn artwork_to_prepare(
        &self,
        requirement: ArtworkRequirement,
    ) -> Option<Arc<Artwork>> {
        let track = self
            .queued_track
            .as_ref()
            .or(self.pending_track.as_ref())
            .or(self.displayed_track.as_ref())?;
        let source = track.expected_artwork_source.as_ref()?;
        self.track_key_for_artwork(source, requirement)
            .map(|_| source.clone())
    }

    fn cache_artwork(&mut self, artwork: &PreparedArtwork) {
        let bytes = artwork.storage_bytes();
        if bytes > PREPARED_ARTWORK_CACHE_MAX_BYTES {
            return;
        }
        self.prepared_cache
            .retain(|cached| !cached.matches(artwork.source()));
        let mut total_bytes = self
            .prepared_cache
            .iter()
            .map(PreparedArtwork::storage_bytes)
            .sum::<usize>();
        while self.prepared_cache.len() >= PREPARED_ARTWORK_CACHE_CAPACITY
            || total_bytes + bytes > PREPARED_ARTWORK_CACHE_MAX_BYTES
        {
            let Some(oldest) = self.prepared_cache.pop_front() else {
                break;
            };
            total_bytes -= oldest.storage_bytes();
        }
        self.prepared_cache.push_back(artwork.clone());
    }

    /// Attaches worker-prepared artwork only to the latest matching track.
    ///
    /// A pending track supersedes the still-visible outgoing track, so a late
    /// completion for that outgoing track must not alter the transition.
    pub(super) fn apply_prepared_artwork(
        &mut self,
        track_key: &str,
        source: &Arc<Artwork>,
        artwork: PreparedArtwork,
        requirement: ArtworkRequirement,
    ) -> PresentationAction {
        let target = self.prepared_artwork_target(track_key, source);
        if target.is_some() {
            self.cache_artwork(&artwork);
        }
        match target {
            Some(PreparedArtworkTarget::Queued) => {
                let queued = self
                    .queued_track
                    .as_ref()
                    .expect("queued artwork target exists");
                self.queued_track = Some(Rc::new(queued.with_artwork(artwork)));
                PresentationAction::None
            }
            Some(PreparedArtworkTarget::Pending) => {
                let pending = self
                    .pending_track
                    .as_ref()
                    .expect("prepared artwork target guarantees a pending track");
                self.pending_track = Some(Rc::new(pending.with_artwork(artwork)));
                match self.pending_transition_phase {
                    Some(PendingTransitionPhase::AwaitingArtworkVisible)
                        if !self
                            .pending_track
                            .as_ref()
                            .unwrap()
                            .awaits_artwork(requirement) =>
                    {
                        self.pending_transition_phase = Some(PendingTransitionPhase::Hiding);
                        PresentationAction::BeginTransition
                    }
                    _ => PresentationAction::None,
                }
            }
            Some(PreparedArtworkTarget::Displayed) => {
                let displayed = self
                    .displayed_track
                    .as_ref()
                    .expect("prepared artwork target guarantees a displayed track");
                let track = Rc::new(displayed.with_artwork(artwork));
                self.mode = PresentationMode::TrackWithArtwork;
                self.displayed_track = Some(track.clone());
                PresentationAction::RenderTrack(track)
            }
            None => PresentationAction::None,
        }
    }

    /// Accepts the latest recognition, either committing it or making it the
    /// sole replacement staged before or behind the current hide animation.
    pub(super) fn receive_track(
        &mut self,
        track: Rc<PresentedTrack>,
        can_animate: bool,
        requirement: ArtworkRequirement,
    ) -> PresentationAction {
        // Repeated recognition of an unchanged scene must not rebuild its labels,
        // textures, layouts and background, or restart the current reveal leg.
        if self
            .queued_track
            .as_ref()
            .or(self.pending_track.as_ref())
            .or(self.displayed_track.as_ref())
            .is_some_and(|current| current.same_presentation(&track))
        {
            return PresentationAction::None;
        }
        if let Some(phase) = self.pending_transition_phase {
            return match phase {
                PendingTransitionPhase::AwaitingArtworkVisible => {
                    self.pending_track = Some(track.clone());
                    if track.awaits_artwork(requirement) {
                        PresentationAction::None
                    } else if !can_animate {
                        self.commit_track(track)
                    } else {
                        self.pending_transition_phase = Some(PendingTransitionPhase::Hiding);
                        PresentationAction::BeginTransition
                    }
                }
                PendingTransitionPhase::Hiding | PendingTransitionPhase::Revealing => {
                    // Once animation starts, its scene is immutable. Downloads for
                    // the next recognition can proceed without changing either leg.
                    self.queued_track = Some(track);
                    PresentationAction::None
                }
            };
        }

        let should_transition = can_animate
            && !track.track_key.is_empty()
            && self.displayed_track.as_ref().is_some_and(|displayed| {
                !displayed.track_key.is_empty() && displayed.track_key != track.track_key
            });

        if should_transition {
            self.pending_track = Some(track);
            if self
                .pending_track
                .as_ref()
                .is_some_and(|track| track.awaits_artwork(requirement))
            {
                self.pending_transition_phase =
                    Some(PendingTransitionPhase::AwaitingArtworkVisible);
                PresentationAction::None
            } else {
                self.pending_transition_phase = Some(PendingTransitionPhase::Hiding);
                PresentationAction::BeginTransition
            }
        } else {
            self.commit_track(track)
        }
    }

    /// Commits the latest pending recognition once the old content is hidden.
    pub(super) fn transition_hidden(&mut self) -> PresentationAction {
        if self.pending_transition_phase != Some(PendingTransitionPhase::Hiding) {
            return PresentationAction::None;
        }
        let Some(track) = self.pending_track.take() else {
            return PresentationAction::None;
        };
        self.mode = track.presentation_mode();
        self.displayed_track = Some(track.clone());
        self.pending_transition_phase = Some(PendingTransitionPhase::Revealing);
        PresentationAction::RenderTrack(track)
    }

    /// Advances queued recognition only after the active scene has fully appeared.
    pub(super) fn transition_revealed(
        &mut self,
        can_animate: bool,
        requirement: ArtworkRequirement,
    ) -> PresentationAction {
        if self.pending_transition_phase != Some(PendingTransitionPhase::Revealing) {
            return PresentationAction::None;
        }
        self.pending_transition_phase = None;
        self.queued_track
            .take()
            .map_or(PresentationAction::None, |track| {
                self.receive_track(track, can_animate, requirement)
            })
    }

    /// Re-evaluates a queued track after display or transition settings change.
    pub(super) fn reconcile_pending_transition(
        &mut self,
        animations_enabled: bool,
        requirement: ArtworkRequirement,
    ) -> PresentationAction {
        let Some(phase) = self.pending_transition_phase else {
            return PresentationAction::None;
        };
        let artwork_pending = self
            .pending_track
            .as_ref()
            .is_some_and(|track| track.awaits_artwork(requirement));

        match phase {
            PendingTransitionPhase::AwaitingArtworkVisible => {
                if artwork_pending {
                    PresentationAction::None
                } else if !animations_enabled {
                    self.commit_pending_track()
                } else {
                    self.pending_transition_phase = Some(PendingTransitionPhase::Hiding);
                    PresentationAction::BeginTransition
                }
            }
            PendingTransitionPhase::Hiding | PendingTransitionPhase::Revealing => {
                PresentationAction::None
            }
        }
    }

    /// Makes a pending track authoritative without waiting for an animation.
    pub(super) fn flush_pending_track(&mut self) -> PresentationAction {
        if let Some(queued) = self.queued_track.take() {
            return self.commit_track(queued);
        }
        self.commit_pending_track()
    }

    /// Resolves a no-match result according to the keep-last preference.
    pub(super) fn no_recognition(&mut self, keep_last: bool) -> PresentationAction {
        if keep_last {
            return self
                .pending_transition_phase
                .map_or(PresentationAction::None, |_| {
                    PresentationAction::HoldTransition
                });
        }

        self.show_listening()
    }

    pub(super) fn show_listening(&mut self) -> PresentationAction {
        self.displayed_track = None;
        self.pending_track = None;
        self.queued_track = None;
        self.pending_transition_phase = None;
        self.mode = PresentationMode::Listening;
        PresentationAction::RenderListening
    }

    fn commit_track(&mut self, track: Rc<PresentedTrack>) -> PresentationAction {
        self.mode = track.presentation_mode();
        self.pending_track = None;
        self.queued_track = None;
        self.pending_transition_phase = None;
        self.displayed_track = Some(track.clone());
        PresentationAction::RenderTrack(track)
    }

    fn commit_pending_track(&mut self) -> PresentationAction {
        let pending = self.pending_track.take();
        self.pending_transition_phase = None;
        pending.map_or(PresentationAction::None, |track| self.commit_track(track))
    }

    fn prepared_artwork_target(
        &self,
        track_key: &str,
        source: &Arc<Artwork>,
    ) -> Option<PreparedArtworkTarget> {
        if self
            .queued_track
            .as_ref()
            .is_some_and(|track| track.matches_artwork_source(track_key, source))
        {
            return Some(PreparedArtworkTarget::Queued);
        }
        if self
            .pending_track
            .as_ref()
            .is_some_and(|track| track.matches_artwork_source(track_key, source))
        {
            return Some(PreparedArtworkTarget::Pending);
        }

        if self.pending_track.is_some() {
            return None;
        }

        self.displayed_track
            .as_ref()
            .is_some_and(|track| track.matches_artwork_source(track_key, source))
            .then_some(PreparedArtworkTarget::Displayed)
    }
}

pub(super) struct ArtworkPreparationJob {
    pub(super) artwork: Arc<Artwork>,
    pub(super) queued_at: Instant,
}

/// Allows the newest image to start beside obsolete work, with bounded concurrency.
#[derive(Default)]
pub(super) struct ArtworkPreparationJobs {
    active: Vec<Weak<Artwork>>,
    queued: Option<ArtworkPreparationJob>,
}

impl ArtworkPreparationJobs {
    /// Returns work when a worker is available; otherwise retains only the newest job.
    pub(super) fn enqueue(&mut self, artwork: Arc<Artwork>) -> Option<ArtworkPreparationJob> {
        let source = Arc::downgrade(&artwork);
        if self
            .active
            .iter()
            .any(|active| Weak::ptr_eq(active, &source))
        {
            self.queued = None;
            return None;
        }
        let job = ArtworkPreparationJob {
            artwork,
            queued_at: Instant::now(),
        };
        if self.active.len() >= MAX_ARTWORK_PREPARATIONS {
            self.queued = Some(job);
            return None;
        }
        self.active.push(source);
        Some(job)
    }

    pub(super) fn finish(&mut self, artwork: &Arc<Artwork>) -> Option<ArtworkPreparationJob> {
        let source = Arc::downgrade(artwork);
        let index = self
            .active
            .iter()
            .position(|active| Weak::ptr_eq(active, &source))?;
        self.active.swap_remove(index);
        let next = self.queued.take();
        if let Some(job) = &next {
            self.active.push(Arc::downgrade(&job.artwork));
        }
        next
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
    pub(super) artwork_preparations: Rc<RefCell<ArtworkPreparationJobs>>,
    /// Only the latest source is needed when presentation is hidden or artwork-free.
    pub(super) deferred_artwork: Rc<RefCell<Option<Arc<Artwork>>>>,
}

impl NowPlayingState {
    pub(super) fn new(settings: Rc<Cell<NowPlayingSettings>>) -> Self {
        Self {
            settings,
            applying_settings: Rc::new(Cell::new(false)),
            gradient_surface: Rc::new(RefCell::new(None)),
            current_background: Rc::new(Cell::new(Background::fallback())),
            track_presentation: Rc::new(RefCell::new(TrackPresentationState::default())),
            artwork_preparations: Rc::new(RefCell::new(ArtworkPreparationJobs::default())),
            deferred_artwork: Rc::new(RefCell::new(None)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::palette::{ArtworkRequirement, ArtworkVisuals, prepare_artwork};
    use super::{
        ArtworkPreparationJobs, PREPARED_ARTWORK_CACHE_CAPACITY, PendingTransitionPhase,
        PreparedArtworkTarget, PresentationAction, PresentationMode, PresentedTrack,
        TrackPresentationState,
    };
    use crate::core::artwork::Artwork;
    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
    use std::io::Cursor;
    use std::rc::Rc;
    use std::sync::Arc;

    fn track(key: &str) -> Rc<PresentedTrack> {
        Rc::new(PresentedTrack {
            response_received_at: None,
            track_key: key.to_string(),
            song_name: format!("Song {key}"),
            artist_name: "Artist".to_string(),
            album_name: None,
            release_year: None,
            artwork: None,
            artwork_pending: false,
            expected_artwork_source: None,
        })
    }

    fn track_expecting(key: &str, artwork: &Arc<Artwork>) -> Rc<PresentedTrack> {
        let mut track = (*track(key)).clone();
        track.expected_artwork_source = Some(artwork.clone());
        Rc::new(track)
    }

    fn pending_track_expecting(key: &str, artwork: &Arc<Artwork>) -> Rc<PresentedTrack> {
        let mut track = (*track_expecting(key, artwork)).clone();
        track.artwork_pending = true;
        Rc::new(track)
    }

    fn assert_rendered_track(action: PresentationAction, expected_key: &str) {
        let PresentationAction::RenderTrack(track) = action else {
            panic!("expected a rendered track");
        };
        assert_eq!(track.track_key, expected_key);
    }

    fn artwork(red: u8) -> Arc<Artwork> {
        let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(1, 1, Rgba([red, 0, 0, 255])));
        let mut encoded = Cursor::new(Vec::new());
        image.write_to(&mut encoded, ImageFormat::Png).unwrap();
        Arc::new(Artwork::decode(encoded.into_inner()).unwrap())
    }

    fn prepared_artwork(source: &Arc<Artwork>) -> super::PreparedArtwork {
        prepare_artwork(source, ArtworkVisuals::fallback(), None)
    }

    #[test]
    fn no_match_keeps_the_latest_recognition_in_its_active_transition() {
        let mut state = TrackPresentationState::default();
        assert_rendered_track(
            state.receive_track(track("a"), true, ArtworkRequirement::None),
            "a",
        );
        assert!(matches!(
            state.receive_track(track("b"), true, ArtworkRequirement::None),
            PresentationAction::BeginTransition
        ));

        assert!(matches!(
            state.no_recognition(true),
            PresentationAction::HoldTransition
        ));
        assert_eq!(
            state
                .displayed_track
                .as_ref()
                .map(|track| track.track_key.as_str()),
            Some("a")
        );
        assert_rendered_track(state.transition_hidden(), "b");
    }

    #[test]
    fn no_match_discards_pending_and_displayed_tracks_when_keep_last_is_disabled() {
        let mut state = TrackPresentationState::default();
        state.receive_track(track("a"), true, ArtworkRequirement::None);
        state.receive_track(track("b"), true, ArtworkRequirement::None);

        assert!(matches!(
            state.no_recognition(false),
            PresentationAction::RenderListening
        ));
        assert!(state.displayed_track.is_none());
        assert!(state.pending_track.is_none());
        assert_eq!(state.mode, PresentationMode::Listening);
    }

    #[test]
    fn in_flight_transition_keeps_its_target_and_queues_the_newest_track() {
        let mut state = TrackPresentationState::default();
        state.receive_track(track("a"), true, ArtworkRequirement::None);
        state.receive_track(track("b"), true, ArtworkRequirement::None);

        assert!(matches!(
            state.receive_track(track("c"), true, ArtworkRequirement::None),
            PresentationAction::None
        ));
        assert_rendered_track(state.transition_hidden(), "b");
        assert!(matches!(
            state.transition_revealed(true, ArtworkRequirement::None),
            PresentationAction::BeginTransition
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
        state.receive_track(track("a"), false, ArtworkRequirement::None);

        assert_rendered_track(
            state.receive_track(track("b"), false, ArtworkRequirement::None),
            "b",
        );
        assert!(state.pending_track.is_none());
    }

    #[test]
    fn track_without_artwork_has_a_distinct_mode_from_listening() {
        let mut state = TrackPresentationState::default();
        state.receive_track(track("a"), false, ArtworkRequirement::None);

        assert_eq!(state.mode, PresentationMode::TrackWithoutArtwork);
    }

    #[test]
    fn pending_track_supersedes_the_outgoing_palette_target() {
        let first = artwork(1);
        let second = artwork(2);
        let mut state = TrackPresentationState::default();
        state.receive_track(track_expecting("a", &first), true, ArtworkRequirement::None);
        assert_eq!(
            state.prepared_artwork_target("a", &first),
            Some(PreparedArtworkTarget::Displayed)
        );

        state.receive_track(
            track_expecting("b", &second),
            true,
            ArtworkRequirement::None,
        );
        assert_eq!(state.prepared_artwork_target("a", &first), None);
        assert_eq!(
            state.prepared_artwork_target("b", &second),
            Some(PreparedArtworkTarget::Pending)
        );

        state.transition_hidden();
        assert_eq!(
            state.prepared_artwork_target("b", &second),
            Some(PreparedArtworkTarget::Displayed)
        );
    }

    #[test]
    fn same_key_update_without_artwork_rejects_an_old_palette_result() {
        let old_artwork = artwork(1);
        let mut state = TrackPresentationState::default();
        state.receive_track(
            track_expecting("a", &old_artwork),
            false,
            ArtworkRequirement::None,
        );
        assert_eq!(
            state.prepared_artwork_target("a", &old_artwork),
            Some(PreparedArtworkTarget::Displayed)
        );

        state.receive_track(track("a"), false, ArtworkRequirement::None);
        assert_eq!(state.prepared_artwork_target("a", &old_artwork), None);
    }

    #[test]
    fn same_key_replacement_artwork_rejects_the_previous_source() {
        let old_artwork = artwork(1);
        let new_artwork = artwork(2);
        let mut state = TrackPresentationState::default();
        state.receive_track(
            track_expecting("a", &old_artwork),
            false,
            ArtworkRequirement::None,
        );
        state.receive_track(
            track_expecting("a", &new_artwork),
            false,
            ArtworkRequirement::None,
        );

        assert_eq!(state.prepared_artwork_target("a", &old_artwork), None);
        assert_eq!(
            state.prepared_artwork_target("a", &new_artwork),
            Some(PreparedArtworkTarget::Displayed)
        );
    }

    #[test]
    fn immersive_transition_waits_for_prepared_artwork_before_hiding() {
        let source = artwork(2);
        let mut state = TrackPresentationState::default();
        state.receive_track(track("a"), false, ArtworkRequirement::None);

        assert!(matches!(
            state.receive_track(
                pending_track_expecting("b", &source),
                true,
                ArtworkRequirement::Immersive
            ),
            PresentationAction::None
        ));
        assert_eq!(
            state.pending_transition_phase,
            Some(PendingTransitionPhase::AwaitingArtworkVisible)
        );
        assert_eq!(
            state
                .displayed_track
                .as_ref()
                .map(|track| track.track_key.as_str()),
            Some("a")
        );

        assert!(matches!(
            state.apply_prepared_artwork(
                "b",
                &source,
                prepared_artwork(&source),
                ArtworkRequirement::Immersive
            ),
            PresentationAction::BeginTransition
        ));
        assert_eq!(
            state.pending_transition_phase,
            Some(PendingTransitionPhase::Hiding)
        );
        assert_rendered_track(state.transition_hidden(), "b");
        assert!(
            state
                .displayed_track
                .as_ref()
                .is_some_and(|track| track.artwork.is_some())
        );
    }

    #[test]
    fn unavailable_immersive_artwork_starts_the_waiting_transition() {
        let source = artwork(2);
        let mut state = TrackPresentationState::default();
        state.receive_track(track("a"), false, ArtworkRequirement::None);
        state.receive_track(
            pending_track_expecting("b", &source),
            true,
            ArtworkRequirement::Immersive,
        );

        assert!(matches!(
            state.receive_track(track("b"), true, ArtworkRequirement::Immersive),
            PresentationAction::BeginTransition
        ));
        assert_rendered_track(state.transition_hidden(), "b");
    }

    #[test]
    fn mode_change_during_hide_never_parks_the_scene_at_the_midpoint() {
        let source = artwork(2);
        let mut state = TrackPresentationState::default();
        state.receive_track(track("a"), false, ArtworkRequirement::None);
        assert!(matches!(
            state.receive_track(
                pending_track_expecting("b", &source),
                true,
                ArtworkRequirement::None
            ),
            PresentationAction::BeginTransition
        ));

        assert_rendered_track(state.transition_hidden(), "b");
        assert_eq!(
            state.pending_transition_phase,
            Some(PendingTransitionPhase::Revealing)
        );
    }

    #[test]
    fn leaving_an_immersive_mode_releases_an_artwork_wait() {
        let source = artwork(2);
        let mut state = TrackPresentationState::default();
        state.receive_track(track("a"), false, ArtworkRequirement::None);
        state.receive_track(
            pending_track_expecting("b", &source),
            true,
            ArtworkRequirement::Immersive,
        );

        assert!(matches!(
            state.reconcile_pending_transition(true, ArtworkRequirement::None),
            PresentationAction::BeginTransition
        ));
        assert_rendered_track(state.transition_hidden(), "b");
    }

    #[test]
    fn disabling_transitions_does_not_expose_retained_immersive_artwork() {
        let source = artwork(2);
        let mut state = TrackPresentationState::default();
        state.receive_track(track("a"), false, ArtworkRequirement::None);
        state.receive_track(
            pending_track_expecting("b", &source),
            true,
            ArtworkRequirement::Immersive,
        );

        assert!(matches!(
            state.reconcile_pending_transition(false, ArtworkRequirement::Immersive),
            PresentationAction::None
        ));
        assert_eq!(
            state.pending_transition_phase,
            Some(PendingTransitionPhase::AwaitingArtworkVisible)
        );
        assert!(matches!(
            state.apply_prepared_artwork(
                "b",
                &source,
                prepared_artwork(&source),
                ArtworkRequirement::Immersive
            ),
            PresentationAction::BeginTransition
        ));
    }

    #[test]
    fn artwork_prepared_during_hide_is_committed_only_at_the_midpoint() {
        let source = artwork(2);
        let mut state = TrackPresentationState::default();
        state.receive_track(track("a"), false, ArtworkRequirement::None);
        state.receive_track(
            pending_track_expecting("b", &source),
            true,
            ArtworkRequirement::None,
        );

        assert!(matches!(
            state.apply_prepared_artwork(
                "b",
                &source,
                prepared_artwork(&source),
                ArtworkRequirement::Immersive
            ),
            PresentationAction::None
        ));
        assert_rendered_track(state.transition_hidden(), "b");
        assert!(
            state
                .displayed_track
                .as_ref()
                .is_some_and(|track| track.artwork.is_some())
        );
    }

    #[test]
    fn newer_pending_artwork_waits_behind_a_fully_visible_scene() {
        let source = artwork(3);
        let mut state = TrackPresentationState::default();
        state.receive_track(track("a"), false, ArtworkRequirement::None);
        assert!(matches!(
            state.receive_track(track("b"), true, ArtworkRequirement::Immersive),
            PresentationAction::BeginTransition
        ));
        state.receive_track(
            pending_track_expecting("c", &source),
            true,
            ArtworkRequirement::Immersive,
        );

        assert_rendered_track(state.transition_hidden(), "b");
        assert_eq!(
            state
                .displayed_track
                .as_ref()
                .map(|track| track.track_key.as_str()),
            Some("b")
        );
        assert!(matches!(
            state.no_recognition(true),
            PresentationAction::HoldTransition
        ));
        assert!(matches!(
            state.transition_revealed(true, ArtworkRequirement::Immersive),
            PresentationAction::None
        ));
        assert_eq!(
            state.pending_transition_phase,
            Some(PendingTransitionPhase::AwaitingArtworkVisible)
        );
        assert!(matches!(
            state.apply_prepared_artwork(
                "c",
                &source,
                prepared_artwork(&source),
                ArtworkRequirement::Immersive
            ),
            PresentationAction::BeginTransition
        ));
        assert_rendered_track(state.transition_hidden(), "c");
    }

    #[test]
    fn newest_artwork_source_wins_without_replacing_the_revealing_scene() {
        let stale_source = artwork(3);
        let current_source = artwork(4);
        let mut state = TrackPresentationState::default();
        state.receive_track(track("a"), false, ArtworkRequirement::None);
        state.receive_track(track("b"), true, ArtworkRequirement::Immersive);
        state.receive_track(
            pending_track_expecting("c", &stale_source),
            true,
            ArtworkRequirement::Immersive,
        );
        state.transition_hidden();

        assert!(matches!(
            state.receive_track(
                pending_track_expecting("d", &current_source),
                false,
                ArtworkRequirement::Immersive,
            ),
            PresentationAction::None
        ));
        assert!(matches!(
            state.apply_prepared_artwork(
                "c",
                &stale_source,
                prepared_artwork(&stale_source),
                ArtworkRequirement::Immersive
            ),
            PresentationAction::None
        ));
        assert!(matches!(
            state.apply_prepared_artwork(
                "d",
                &current_source,
                prepared_artwork(&current_source),
                ArtworkRequirement::Immersive
            ),
            PresentationAction::None
        ));
        assert_eq!(state.displayed_track.as_ref().unwrap().track_key, "b");
        assert!(matches!(
            state.transition_revealed(true, ArtworkRequirement::Immersive),
            PresentationAction::BeginTransition
        ));
        assert_rendered_track(state.transition_hidden(), "d");
    }

    #[test]
    fn stale_artwork_cannot_start_a_replaced_pending_transition() {
        let stale_source = artwork(2);
        let current_source = artwork(3);
        let mut state = TrackPresentationState::default();
        state.receive_track(track("a"), false, ArtworkRequirement::None);
        state.receive_track(
            pending_track_expecting("b", &stale_source),
            true,
            ArtworkRequirement::Immersive,
        );
        state.receive_track(
            pending_track_expecting("c", &current_source),
            true,
            ArtworkRequirement::Immersive,
        );

        assert!(matches!(
            state.apply_prepared_artwork(
                "b",
                &stale_source,
                prepared_artwork(&stale_source),
                ArtworkRequirement::Immersive
            ),
            PresentationAction::None
        ));
        assert_eq!(
            state.pending_transition_phase,
            Some(PendingTransitionPhase::AwaitingArtworkVisible)
        );
        assert!(matches!(
            state.apply_prepared_artwork(
                "c",
                &current_source,
                prepared_artwork(&current_source),
                ArtworkRequirement::Immersive
            ),
            PresentationAction::BeginTransition
        ));
    }

    #[test]
    fn new_artwork_starts_beside_obsolete_work_and_the_queue_stays_bounded() {
        let first = artwork(1);
        let replacement = artwork(2);
        let newest = artwork(3);
        let skipped = artwork(4);
        let mut jobs = ArtworkPreparationJobs::default();

        assert!(Arc::ptr_eq(
            &jobs.enqueue(first.clone()).unwrap().artwork,
            &first
        ));
        assert!(jobs.enqueue(first.clone()).is_none());
        assert!(Arc::ptr_eq(
            &jobs.enqueue(replacement.clone()).unwrap().artwork,
            &replacement
        ));
        assert!(jobs.enqueue(skipped).is_none());
        assert!(jobs.enqueue(newest.clone()).is_none());
        assert!(Arc::ptr_eq(&jobs.finish(&first).unwrap().artwork, &newest));
        assert_eq!(jobs.active.len(), 2);
        assert!(jobs.finish(&replacement).is_none());
        assert!(jobs.finish(&newest).is_none());
        assert!(jobs.active.is_empty());
    }

    #[test]
    fn returning_to_active_artwork_cancels_the_obsolete_queued_source() {
        let first = artwork(1);
        let second = artwork(2);
        let mut jobs = ArtworkPreparationJobs::default();
        jobs.enqueue(first.clone());
        jobs.enqueue(second.clone());
        jobs.enqueue(artwork(3));
        assert!(jobs.enqueue(first.clone()).is_none());
        assert!(jobs.finish(&second).is_none());
        assert!(jobs.finish(&first).is_none());
    }

    #[test]
    fn unchanged_recognition_does_not_reapply_the_scene_but_metadata_changes_do() {
        let source = artwork(1);
        let track = Rc::new(track_expecting("a", &source).with_artwork(prepared_artwork(&source)));
        let mut state = TrackPresentationState::default();
        state.receive_track(track.clone(), false, ArtworkRequirement::Immersive);
        assert!(matches!(
            state.receive_track(track.clone(), true, ArtworkRequirement::Immersive),
            PresentationAction::None
        ));

        let mut changed = (*track).clone();
        changed.album_name = Some("Updated album".to_string());
        assert_rendered_track(
            state.receive_track(Rc::new(changed), true, ArtworkRequirement::Immersive),
            "a",
        );
    }

    #[test]
    fn shared_album_artwork_is_reused_after_listening_without_another_preparation() {
        let source = artwork(1);
        let mut state = TrackPresentationState::default();
        state.receive_track(
            pending_track_expecting("a", &source),
            false,
            ArtworkRequirement::Immersive,
        );
        state.apply_prepared_artwork(
            "a",
            &source,
            prepared_artwork(&source),
            ArtworkRequirement::Immersive,
        );
        assert!(
            state
                .track_key_for_artwork(&source, ArtworkRequirement::Immersive)
                .is_none()
        );
        let texture = state.prepared_artwork_for(&source).unwrap().texture;
        state.show_listening();
        let cached = state.prepared_artwork_for(&source).unwrap();
        assert_eq!(cached.texture, texture);
        let next = Rc::new(track_expecting("b", &source).with_artwork(cached));
        assert_rendered_track(
            state.receive_track(next, true, ArtworkRequirement::Immersive),
            "b",
        );
        assert_eq!(state.mode, PresentationMode::TrackWithArtwork);
    }

    #[test]
    fn prepared_cache_evicts_old_entries_and_rejects_an_unrelated_source() {
        let mut state = TrackPresentationState::default();
        let sources = (0..=PREPARED_ARTWORK_CACHE_CAPACITY)
            .map(|i| artwork(i as u8))
            .collect::<Vec<_>>();
        for source in &sources {
            state.cache_artwork(&prepared_artwork(source));
        }
        assert_eq!(state.prepared_cache.len(), PREPARED_ARTWORK_CACHE_CAPACITY);
        assert!(state.prepared_artwork_for(&sources[0]).is_none());
        assert!(
            state
                .prepared_artwork_for(sources.last().unwrap())
                .is_some()
        );
        assert!(state.prepared_artwork_for(&artwork(200)).is_none());
        assert!(state.prepared_artwork_for(&artwork(4)).is_some());
    }

    #[test]
    fn shared_in_flight_artwork_follows_the_latest_track_on_the_album() {
        let source = artwork(1);
        let stale = artwork(2);
        let mut state = TrackPresentationState::default();
        state.receive_track(
            pending_track_expecting("a", &source),
            false,
            ArtworkRequirement::Immersive,
        );
        state.receive_track(
            pending_track_expecting("b", &source),
            true,
            ArtworkRequirement::Immersive,
        );
        assert_eq!(
            state
                .track_key_for_artwork(&source, ArtworkRequirement::Immersive)
                .as_deref(),
            Some("b")
        );
        assert!(
            state
                .track_key_for_artwork(&stale, ArtworkRequirement::Immersive)
                .is_none()
        );
        assert!(matches!(
            state.apply_prepared_artwork(
                "b",
                &source,
                prepared_artwork(&source),
                ArtworkRequirement::Immersive
            ),
            PresentationAction::BeginTransition
        ));
        assert_rendered_track(state.transition_hidden(), "b");
    }
}
