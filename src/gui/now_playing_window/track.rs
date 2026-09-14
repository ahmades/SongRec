//! Recognition-result presentation and transition handling.

use super::background::{CachedGradient, redraw_background};
use super::palette::{
    ArtworkRequirement, ArtworkVisuals, Background, prepare_artwork, prepare_immersive_background,
    visuals_from_artwork,
};
use super::state::{
    ArtistBackgroundState, ArtistBackgroundWork, PresentationAction, PresentationMode,
    PresentedTrack, TrackPresentationState,
};
use super::ui::{
    AmbientArtworkLayout, CinemaArtworkLayout, TrackTransitionLayout, configure_immersive_info,
};
use super::{
    BackdropIntensity, DisplayMode, NowPlayingSettings, NowPlayingWindow, TransitionEffect,
};
use crate::core::artwork::{Artwork, ArtworkStatus};
use crate::core::thread_messages::SongRecognizedMessage;
use adw::prelude::*;
use gettextrs::gettext;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

/// Converts the user-facing total transition duration into one hide/reveal leg.
pub(super) fn transition_leg_duration_ms(total_duration_ms: u64) -> u32 {
    (total_duration_ms / 2).max(1).min(u64::from(u32::MAX)) as u32
}

/// Starts the hide leg using the currently selected, already-supported effect.
fn begin_track_transition(transition: &TrackTransitionLayout, settings: NowPlayingSettings) {
    log::debug!(
        "Now Playing transition: begin {:?}, total {} ms",
        settings.shared.transition,
        settings.shared.transition_duration_ms
    );
    transition.begin(
        settings.shared.transition,
        transition_leg_duration_ms(settings.shared.transition_duration_ms),
    );
}

/// The GTK objects and shared state required to render one presentation.
///
/// GTK callbacks own this lightweight clone instead of borrowing the window.
#[derive(Clone)]
pub(super) struct TrackPresentation {
    classic_content: gtk::Box,
    artwork: gtk::Picture,
    classic_missing_artwork: gtk::Image,
    cinema_artwork: CinemaArtworkLayout,
    ambient_artwork: AmbientArtworkLayout,
    scrim_area: gtk::DrawingArea,
    artwork_placeholder: gtk::Label,
    title_label: gtk::Label,
    artist_label: gtk::Label,
    album_label: gtk::Label,
    details_label: gtk::Label,
    immersive_info_box: gtk::Box,
    immersive_title_label: gtk::Label,
    immersive_artist_label: gtk::Label,
    immersive_album_label: gtk::Label,
    immersive_details_label: gtk::Label,
    background_area: gtk::DrawingArea,
    gradient_surface: Rc<RefCell<Option<CachedGradient>>>,
    settings: Rc<Cell<NowPlayingSettings>>,
    applied_backdrop_intensity: Rc<Cell<BackdropIntensity>>,
    current_background: Rc<Cell<Background>>,
    track_state: Rc<RefCell<TrackPresentationState>>,
}

/// Cloneable callback context for lazy artist-background preparation.
#[derive(Clone)]
struct ArtistBackgroundPreparation {
    window: glib::WeakRef<gtk::Window>,
    track_state: Rc<RefCell<TrackPresentationState>>,
    preparation_jobs: Rc<RefCell<super::state::ArtworkPreparationJobs>>,
    presentation: TrackPresentation,
    transition: TrackTransitionLayout,
    settings: Rc<Cell<NowPlayingSettings>>,
}

impl ArtistBackgroundPreparation {
    fn from_window(window: &NowPlayingWindow) -> Self {
        Self {
            window: window.ui.window.downgrade(),
            track_state: window.state.track_presentation.clone(),
            preparation_jobs: window.state.artist_background_preparations.clone(),
            presentation: TrackPresentation::from_window(window),
            transition: window.ui.content_transition.clone(),
            settings: window.state.settings.clone(),
        }
    }

    fn apply_action(&self, action: PresentationAction) {
        match action {
            PresentationAction::BeginTransition => {
                begin_track_transition(&self.transition, self.settings.get());
            }
            PresentationAction::RenderTrack(track) => {
                self.presentation
                    .apply_action(PresentationAction::RenderTrack(track));
                self.transition.reveal();
            }
            PresentationAction::None
            | PresentationAction::HoldTransition
            | PresentationAction::RenderListening => {}
        }
    }

    fn set_state(&self, track_key: &str, url: &str, state: ArtistBackgroundState) {
        let requirement = ArtworkRequirement::for_settings(self.settings.get());
        let action = self.track_state.borrow_mut().apply_artist_background_state(
            track_key,
            url,
            state,
            requirement,
        );
        self.apply_action(action);
    }

    fn download_completed(&self, track_key: String, url: String, artwork: Option<Arc<Artwork>>) {
        match artwork {
            Some(artwork) => {
                self.set_state(
                    &track_key,
                    &url,
                    ArtistBackgroundState::Downloaded(artwork.clone()),
                );
                self.start_preparation(track_key, url, artwork);
            }
            None => self.set_state(&track_key, &url, ArtistBackgroundState::Unavailable),
        }
    }

    fn start_preparation(&self, track_key: String, url: String, artwork: Arc<Artwork>) {
        let settings = self.settings.get();
        let requirement = ArtworkRequirement::for_settings(settings);
        if !requirement.needs_artist_background()
            || !self
                .window
                .upgrade()
                .is_some_and(|window| window.is_visible())
        {
            return;
        }
        if self
            .track_state
            .borrow()
            .track_key_for_artist_background(&url, &artwork)
            .as_deref()
            != Some(track_key.as_str())
        {
            return;
        }

        let prepared = {
            self.track_state
                .borrow()
                .prepared_immersive_background_for(&artwork, requirement)
        };
        if let Some(prepared) = prepared {
            self.set_state(
                &track_key,
                &url,
                ArtistBackgroundState::Ready(prepared, artwork),
            );
            return;
        }

        self.set_state(
            &track_key,
            &url,
            ArtistBackgroundState::Preparing(artwork.clone()),
        );
        let Some(job) = self.preparation_jobs.borrow_mut().enqueue(artwork) else {
            return;
        };

        let context = self.clone();
        glib::spawn_future_local(async move {
            let mut next_job = Some(job);
            while let Some(job) = next_job {
                let artwork = job.artwork;
                let requirement = ArtworkRequirement::for_settings(context.settings.get());
                let recipient = {
                    context
                        .track_state
                        .borrow()
                        .artist_background_recipient(&artwork)
                };
                let Some((job_track_key, _)) = recipient else {
                    next_job = context.preparation_jobs.borrow_mut().finish(&artwork);
                    continue;
                };

                let artwork_for_worker = artwork.clone();
                let log_track_key = job_track_key.clone();
                let visuals = match gio::spawn_blocking(move || {
                    let started = Instant::now();
                    let queue_time = started.duration_since(job.queued_at);
                    let visuals = visuals_from_artwork(&artwork_for_worker, requirement);
                    log::debug!(
                        "Now Playing track {log_track_key}, artist background {}x{}: queue {:?}, processing {:?}",
                        artwork_for_worker.width(),
                        artwork_for_worker.height(),
                        queue_time,
                        started.elapsed()
                    );
                    visuals
                })
                .await
                {
                    Ok(visuals) => visuals,
                    Err(_) => {
                        log::warn!("Now Playing artist-background preparation task panicked");
                        ArtworkVisuals::fallback_for(requirement)
                    }
                };

                // Like album preparation, one in-flight result may follow a
                // newer track that uses the same decoded source. Resolve the
                // recipient after worker completion instead of discarding the
                // only job merely because its track key changed meanwhile.
                let recipient = context
                    .track_state
                    .borrow()
                    .artist_background_recipient(&artwork);
                if let Some((track_key, url)) = recipient {
                    let prepared = prepare_immersive_background(&artwork, visuals);
                    context.set_state(
                        &track_key,
                        &url,
                        ArtistBackgroundState::Ready(prepared, artwork.clone()),
                    );
                }
                next_job = context.preparation_jobs.borrow_mut().finish(&artwork);
                if next_job.is_none() {
                    let current_requirement =
                        ArtworkRequirement::for_settings(context.settings.get());
                    let retry = context
                        .track_state
                        .borrow()
                        .artist_background_work(current_requirement);
                    if let Some(ArtistBackgroundWork::Prepare {
                        track_key,
                        url,
                        artwork,
                    }) = retry
                    {
                        context.start_preparation(track_key, url, artwork);
                    }
                }
            }
        });
    }
}

impl TrackPresentation {
    pub(super) fn from_window(window: &NowPlayingWindow) -> Self {
        Self {
            classic_content: window.ui.classic_content.clone(),
            artwork: window.ui.artwork.clone(),
            classic_missing_artwork: window.ui.classic_missing_artwork.clone(),
            cinema_artwork: window.ui.cinema_artwork.clone(),
            ambient_artwork: window.ui.ambient_artwork.clone(),
            scrim_area: window.ui.scrim_area.clone(),
            artwork_placeholder: window.ui.artwork_placeholder.clone(),
            title_label: window.ui.title_label.clone(),
            artist_label: window.ui.artist_label.clone(),
            album_label: window.ui.album_label.clone(),
            details_label: window.ui.details_label.clone(),
            immersive_info_box: window.ui.immersive_info_box.clone(),
            immersive_title_label: window.ui.immersive_title_label.clone(),
            immersive_artist_label: window.ui.immersive_artist_label.clone(),
            immersive_album_label: window.ui.immersive_album_label.clone(),
            immersive_details_label: window.ui.immersive_details_label.clone(),
            background_area: window.ui.background_area.clone(),
            gradient_surface: window.state.gradient_surface.clone(),
            settings: window.state.settings.clone(),
            applied_backdrop_intensity: window.state.applied_backdrop_intensity.clone(),
            current_background: window.state.current_background.clone(),
            track_state: window.state.track_presentation.clone(),
        }
    }

    fn apply_action(&self, action: PresentationAction) {
        match action {
            PresentationAction::RenderTrack(track) => self.render_track(&track),
            PresentationAction::RenderListening => self.render_listening(),
            PresentationAction::None
            | PresentationAction::BeginTransition
            | PresentationAction::HoldTransition => {}
        }
    }

    /// Renders a recognized track whose artwork has already been decoded.
    fn render_track(&self, track: &PresentedTrack) {
        let started = Instant::now();
        // The global placeholder belongs exclusively to Listening mode; a
        // recognized track without artwork intentionally leaves this empty.
        self.artwork_placeholder.set_label("");
        self.set_metadata(track);

        let settings = self.settings.get();
        let requirement = ArtworkRequirement::for_settings(settings);
        let use_artist_background = requirement.needs_artist_background();
        let artist_background = match (use_artist_background, &track.artist_background) {
            (true, ArtistBackgroundState::Ready(background, _))
                if background.is_ready(requirement) =>
            {
                Some(background)
            }
            _ => None,
        };
        let foreground = track.artwork.as_ref().map(|artwork| &artwork.texture);
        let album_backdrop = track
            .artwork
            .as_ref()
            .and_then(|artwork| artwork.ambient_texture_for(requirement));
        let artist_pending = use_artist_background
            && track.artist_background_url.is_some()
            && track.artist_background.is_waiting(requirement);
        let backdrop = artist_background
            .map(|background| &background.texture)
            .or_else(|| (!artist_pending).then_some(()).and(album_backdrop));
        let album_background = track
            .artwork
            .as_ref()
            .filter(|artwork| artwork.is_ready(requirement))
            .map(|artwork| artwork.background);
        let artwork_background = artist_background
            .map(|background| background.background)
            .or_else(|| (!artist_pending).then_some(()).and(album_background));
        let artwork_pending = track.awaits_artwork(requirement);

        // The sharp cover is shared by every mode. Immersive layers retain
        // their already-painted backdrop until pixels for the exact selected
        // intensity are ready, avoiding a fallback flash during reprocessing.
        self.artwork.set_paintable(foreground);
        if backdrop.is_some() || !settings.display_mode.uses_immersive_artwork() {
            self.cinema_artwork.set_artwork(foreground, backdrop);
            self.ambient_artwork.set_artwork(foreground, backdrop);
        } else if !artwork_pending {
            self.cinema_artwork.set_artwork(foreground, None);
            self.ambient_artwork.set_artwork(foreground, None);
        }
        if foreground.is_none() && backdrop.is_none() && !artwork_pending {
            self.clear_artwork();
        }
        self.current_background.set(background_after_track_update(
            self.current_background.get(),
            artwork_background,
            artwork_pending,
        ));

        self.apply_background();
        self.sync_artwork_visibility();
        log::debug!(
            "Now Playing track {}: scene applied in {:?}, artwork ready={}, response age {:?} ms",
            track.track_key,
            started.elapsed(),
            track.artwork.is_some(),
            track
                .response_received_at
                .map(|received| (glib::monotonic_time() - received) as f64 / 1000.0)
        );
    }

    /// Renders the deterministic empty/listening state while preserving the background.
    fn render_listening(&self) {
        self.artwork_placeholder.set_label(&gettext("Listening..."));
        self.clear_artwork();
        self.title_label.set_label("");
        self.artist_label.set_label("");
        self.album_label.set_label("");
        self.details_label.set_label("");
        self.immersive_title_label.set_label("");
        self.immersive_artist_label.set_label("");
        self.immersive_album_label.set_label("");
        self.immersive_details_label.set_label("");
        self.sync_artwork_visibility();
    }

    fn set_metadata(&self, track: &PresentedTrack) {
        self.title_label.set_label(&track.song_name);
        self.artist_label.set_label(&track.artist_name);
        self.album_label
            .set_label(optional_metadata(&track.album_name));
        self.details_label
            .set_label(optional_metadata(&track.release_year));
        self.immersive_title_label.set_label(&track.song_name);
        self.immersive_artist_label.set_label(&track.artist_name);
        self.immersive_album_label
            .set_label(optional_metadata(&track.album_name));
        self.immersive_details_label
            .set_label(optional_metadata(&track.release_year));
    }

    fn clear_artwork(&self) {
        self.artwork.set_paintable(Option::<&gdk::Texture>::None);
        self.cinema_artwork.set_artwork(None, None);
        self.ambient_artwork.set_artwork(None, None);
    }

    fn sync_artwork_visibility(&self) {
        let state = self.track_state.borrow();
        let settings = self.settings.get();
        let requirement = ArtworkRequirement::for_settings(settings);
        let current_cover_available = state
            .displayed_track
            .as_ref()
            .is_some_and(|track| track.artwork.is_some());
        let current_artist_background_available =
            state.displayed_track.as_ref().is_some_and(|track| {
                matches!(
                    &track.artist_background,
                    ArtistBackgroundState::Ready(background, _)
                        if background.is_ready(requirement)
                )
            });
        let use_artist_background = requirement.needs_artist_background();
        let current_immersive_artwork_available = current_cover_available
            || (use_artist_background && current_artist_background_available);
        let artwork_pending = state
            .displayed_track
            .as_ref()
            .is_some_and(|track| track.awaits_artwork(requirement));
        let retained_immersive_artwork_available = current_immersive_artwork_available
            || state.displayed_track.as_ref().is_some_and(|track| {
                (track.artwork_pending
                    || (use_artist_background
                        && track.artist_background_url.is_some()
                        && track.artist_background.is_waiting(requirement)))
                    && !matches!(state.mode, PresentationMode::Listening)
            });
        let visibility = presentation_visibility(
            state.mode,
            settings.display_mode,
            current_cover_available,
            retained_immersive_artwork_available,
            artwork_pending,
            settings.shared.hide_track_info,
        );
        drop(state);

        let width = self.background_area.width();
        let height = self.background_area.height();
        configure_immersive_info(
            &self.immersive_info_box,
            [
                &self.immersive_title_label,
                &self.immersive_artist_label,
                &self.immersive_album_label,
                &self.immersive_details_label,
            ],
            settings.display_mode,
            self.cinema_artwork.layout(width, height),
            width,
            height,
        );

        self.classic_content.set_visible(visibility.classic_content);
        self.artwork.set_visible(visibility.classic_artwork);
        self.classic_missing_artwork
            .set_visible(visibility.classic_missing_artwork);
        self.cinema_artwork
            .container
            .set_visible(visibility.cinema_artwork);
        self.ambient_artwork
            .container
            .set_visible(visibility.ambient_artwork);
        self.cinema_artwork.set_background_motion(
            settings.shared.background_motion_enabled && visibility.cinema_artwork,
            settings.shared.background_motion_zoom_percent,
            settings.shared.background_motion_reversal_duration_secs,
        );
        self.ambient_artwork.set_background_motion(
            settings.shared.background_motion_enabled && visibility.ambient_artwork,
            settings.shared.background_motion_zoom_percent,
            settings.shared.background_motion_reversal_duration_secs,
        );
        self.scrim_area.set_visible(visibility.immersive_scrim);
        self.immersive_info_box
            .set_visible(visibility.immersive_info);
        self.artwork_placeholder.set_visible(visibility.listening);
    }

    fn apply_background(&self) {
        let settings = self.settings.get();
        self.applied_backdrop_intensity
            .set(settings.shared.backdrop_intensity);
        redraw_background(
            &self.background_area,
            &self.gradient_surface,
            self.current_background.get(),
            settings.classic.background_style,
            settings.display_mode,
        );
        self.scrim_area.queue_draw();
    }

    /// Re-resolves all mode-dependent layers after a display-mode change.
    pub(super) fn refresh_mode(&self) {
        self.sync_artwork_visibility();
        self.apply_background();
    }

    /// Re-renders the committed track after a visual source preference changes.
    pub(super) fn refresh_current_track(&self) {
        let track = self.track_state.borrow().displayed_track.clone();
        if let Some(track) = track {
            self.render_track(&track);
        } else {
            self.refresh_mode();
        }
    }
}

impl NowPlayingWindow {
    /// Installs one completion callback for every track transition made by this window.
    pub(super) fn setup_track_transition_handlers(&self) {
        let track_state_for_completion = self.state.track_presentation.clone();
        let settings_for_completion = self.state.settings.clone();
        let presentation_for_completion = TrackPresentation::from_window(self);
        self.ui
            .content_transition
            .connect_completed(move |is_revealed| {
                let settings = settings_for_completion.get();
                let (action, refresh_deferred_scene) = {
                    let mut track_state = track_state_for_completion.borrow_mut();
                    let action = if is_revealed {
                        log::debug!("Now Playing transition: fully revealed");
                        track_state.transition_revealed(
                            settings.shared.transition != TransitionEffect::None,
                            ArtworkRequirement::for_settings(settings),
                        )
                    } else {
                        log::debug!("Now Playing transition: hidden midpoint");
                        track_state.transition_hidden()
                    };
                    let refresh_deferred_scene =
                        is_revealed && track_state.take_deferred_scene_refresh();
                    (action, refresh_deferred_scene)
                };
                if refresh_deferred_scene {
                    presentation_for_completion.refresh_current_track();
                }
                if matches!(action, PresentationAction::BeginTransition) {
                    return Some((
                        settings.shared.transition,
                        transition_leg_duration_ms(settings.shared.transition_duration_ms),
                    ));
                }
                presentation_for_completion.apply_action(action);
                None
            });

        let track_state_for_hide = self.state.track_presentation.clone();
        let presentation_for_hide = TrackPresentation::from_window(self);
        let transition_for_hide = self.ui.content_transition.clone();
        self.ui.window.connect_visible_notify(move |window| {
            if window.is_visible() {
                return;
            }

            let action = track_state_for_hide.borrow_mut().flush_pending_track();
            presentation_for_hide.apply_action(action);
            transition_for_hide.reveal_immediately();
        });
    }

    /// Refreshes the displayed song metadata and artwork from a recognition result.
    pub fn update(&self, message: &SongRecognizedMessage) {
        self.state.deferred_artwork.borrow_mut().take();
        let requirement = ArtworkRequirement::for_settings(self.state.settings.get());
        let prepared_artwork = message.cover_image().and_then(|artwork| {
            self.state
                .track_presentation
                .borrow()
                .prepared_artwork_for(artwork, requirement)
        });
        let artist_background = self
            .state
            .track_presentation
            .borrow()
            .artist_background_state_for(
                &message.track_key,
                message.artist_background_url.as_deref(),
                message.response_received_at,
            );
        let visuals_pending = message.cover_image().is_some()
            && prepared_artwork
                .as_ref()
                .is_none_or(|artwork| !artwork.is_ready(requirement));
        log::debug!(
            "Now Playing track {}: received, download pending={}, preparation needed={}, response age {:?} ms",
            message.track_key,
            message.artwork_pending(),
            visuals_pending,
            message
                .response_received_at
                .map(|received| (glib::monotonic_time() - received) as f64 / 1000.0)
        );
        let track = Rc::new(PresentedTrack::from_message(
            message,
            prepared_artwork,
            visuals_pending,
            artist_background,
        ));
        if !track.has_visible_information() {
            self.handle_no_recognition();
            return;
        }

        let settings = self.state.settings.get();
        let can_animate = !matches!(settings.shared.transition, TransitionEffect::None)
            && self.ui.window.is_mapped()
            && self.ui.content_transition.is_child_revealed();
        let action = self.state.track_presentation.borrow_mut().receive_track(
            track,
            can_animate,
            requirement,
        );

        match action {
            PresentationAction::BeginTransition => {
                begin_track_transition(&self.ui.content_transition, settings);
            }
            PresentationAction::RenderTrack(track) => {
                TrackPresentation::from_window(self)
                    .apply_action(PresentationAction::RenderTrack(track));
                self.ui.content_transition.reveal();
            }
            PresentationAction::None
            | PresentationAction::HoldTransition
            | PresentationAction::RenderListening => {}
        }

        if visuals_pending {
            self.prepare_artwork_visuals(
                message
                    .cover_image()
                    .expect("artwork visual preparation requires artwork")
                    .clone(),
            );
        }
        self.ensure_artist_background();
    }

    /// Re-evaluates an artwork-staged transition after its applicable settings change.
    pub(super) fn reconcile_pending_transition(&self) {
        let settings = self.state.settings.get();
        let animations_enabled = !matches!(settings.shared.transition, TransitionEffect::None)
            && self.ui.window.is_mapped();
        let action = self
            .state
            .track_presentation
            .borrow_mut()
            .reconcile_pending_transition(
                animations_enabled,
                ArtworkRequirement::for_settings(settings),
            );

        match action {
            PresentationAction::BeginTransition => {
                begin_track_transition(&self.ui.content_transition, settings);
            }
            PresentationAction::RenderTrack(track) => {
                TrackPresentation::from_window(self)
                    .apply_action(PresentationAction::RenderTrack(track));
                self.ui.content_transition.reveal();
            }
            PresentationAction::RenderListening => {
                TrackPresentation::from_window(self).apply_action(action);
                self.ui.content_transition.reveal();
            }
            PresentationAction::None | PresentationAction::HoldTransition => {}
        }
    }

    /// Prepares Now Playing-only palettes and Ambient pixels away from GTK's main thread.
    fn prepare_artwork_visuals(&self, artwork: Arc<Artwork>) {
        if !self.ui.window.is_visible() || !self.state.settings.get().display_mode.uses_artwork() {
            *self.state.deferred_artwork.borrow_mut() = Some(artwork);
            return;
        }
        let requirement = ArtworkRequirement::for_settings(self.state.settings.get());
        let cached = self
            .state
            .track_presentation
            .borrow()
            .prepared_artwork_for(&artwork, requirement);
        if let Some(cached) = cached {
            let track_key = self
                .state
                .track_presentation
                .borrow()
                .track_key_for_artwork(&artwork, requirement);
            if let Some(track_key) = track_key {
                let action = self
                    .state
                    .track_presentation
                    .borrow_mut()
                    .apply_prepared_artwork(&track_key, &artwork, cached, requirement);
                match action {
                    PresentationAction::BeginTransition => {
                        begin_track_transition(
                            &self.ui.content_transition,
                            self.state.settings.get(),
                        );
                    }
                    PresentationAction::RenderTrack(track) => {
                        TrackPresentation::from_window(self)
                            .apply_action(PresentationAction::RenderTrack(track));
                        self.ui.content_transition.reveal();
                    }
                    PresentationAction::None
                    | PresentationAction::HoldTransition
                    | PresentationAction::RenderListening => {}
                }
            }
            return;
        }
        let Some(job) = self
            .state
            .artwork_preparations
            .borrow_mut()
            .enqueue(artwork)
        else {
            log::debug!("Now Playing artwork: preparation already active or queued");
            return;
        };

        let preparation_jobs = self.state.artwork_preparations.clone();
        let track_state = self.state.track_presentation.clone();
        let presentation = TrackPresentation::from_window(self);
        let transition = self.ui.content_transition.clone();
        let settings = self.state.settings.clone();
        let window = self.ui.window.downgrade();
        let deferred_artwork = self.state.deferred_artwork.clone();
        glib::spawn_future_local(async move {
            let mut next_artwork = Some(job);
            while let Some(job) = next_artwork {
                let artwork = job.artwork;
                // A newer recognition may supersede queued work before this future runs.
                let requirement = ArtworkRequirement::for_settings(settings.get());
                let Some(job_track_key) = track_state
                    .borrow()
                    .track_key_for_artwork(&artwork, requirement)
                else {
                    next_artwork = preparation_jobs.borrow_mut().finish(&artwork);
                    continue;
                };

                // Visibility or mode may change while a previous job is finishing.
                if !window.upgrade().is_some_and(|window| window.is_visible())
                    || !settings.get().display_mode.uses_artwork()
                {
                    next_artwork = preparation_jobs.borrow_mut().finish(&artwork);
                    *deferred_artwork.borrow_mut() = Some(artwork);
                    continue;
                }

                let artwork_for_worker = artwork.clone();
                let visuals = match gio::spawn_blocking(move || {
                    let started = Instant::now();
                    let queue_time = started.duration_since(job.queued_at);
                    let visuals = visuals_from_artwork(&artwork_for_worker, requirement);
                    log::debug!(
                        "Now Playing track {job_track_key}, artwork {}x{}: queue {:?}, processing {:?}",
                        artwork_for_worker.width(),
                        artwork_for_worker.height(),
                        queue_time,
                        started.elapsed()
                    );
                    visuals
                })
                .await
                {
                    Ok(visuals) => visuals,
                    Err(_) => {
                        log::warn!("Now Playing artwork preparation task panicked");
                        ArtworkVisuals::fallback_for(requirement)
                    }
                };

                // Check again before allocating textures on GTK's thread. A completed
                // source can serve a newer track on the same album, but never stale art.
                let requirement = ArtworkRequirement::for_settings(settings.get());
                let track_key = track_state
                    .borrow()
                    .track_key_for_artwork(&artwork, requirement);
                if let Some(track_key) = track_key {
                    let texture_started = Instant::now();
                    let previous = track_state.borrow().prepared_artwork_for_source(&artwork);
                    let prepared = prepare_artwork(&artwork, visuals, previous.as_ref());
                    log::debug!(
                        "Now Playing track {track_key}: textures prepared in {:?}",
                        texture_started.elapsed()
                    );
                    let action = track_state.borrow_mut().apply_prepared_artwork(
                        &track_key,
                        &artwork,
                        prepared,
                        requirement,
                    );
                    match action {
                        PresentationAction::BeginTransition => {
                            begin_track_transition(&transition, settings.get());
                        }
                        PresentationAction::RenderTrack(track) => {
                            presentation.apply_action(PresentationAction::RenderTrack(track));
                            transition.reveal();
                        }
                        PresentationAction::None
                        | PresentationAction::HoldTransition
                        | PresentationAction::RenderListening => {}
                    }
                }
                next_artwork = preparation_jobs.borrow_mut().finish(&artwork);
                // A mode change can request the immersive layer while a fast Classic
                // preparation is in flight. Upgrade it without losing the shared cover.
                if next_artwork.is_none()
                    && window.upgrade().is_some_and(|window| window.is_visible())
                    && let Some(source) = track_state.borrow().artwork_to_prepare(requirement)
                {
                    next_artwork = preparation_jobs.borrow_mut().enqueue(source);
                }
            }
        });
    }

    /// Resumes the latest deferred source when artwork becomes useful again.
    pub(super) fn resume_artwork_preparation(&self) {
        let requirement = ArtworkRequirement::for_settings(self.state.settings.get());
        let artwork = self.state.deferred_artwork.borrow_mut().take().or_else(|| {
            self.state
                .track_presentation
                .borrow()
                .artwork_to_prepare(requirement)
        });
        if let Some(artwork) = artwork
            && self
                .state
                .track_presentation
                .borrow()
                .track_key_for_artwork(&artwork, requirement)
                .is_some()
        {
            self.prepare_artwork_visuals(artwork);
        }
    }

    /// Starts only the artist-background work required by the current visible
    /// immersive presentation. Merely receiving the URL never downloads it.
    pub(super) fn ensure_artist_background(&self) {
        let settings = self.state.settings.get();
        if !self.ui.window.is_visible()
            || !ArtworkRequirement::for_settings(settings).needs_artist_background()
        {
            return;
        }

        // End the immutable state borrow before a synchronous cache hit updates
        // the artist lifecycle through `set_state` below.
        let work = {
            self.state
                .track_presentation
                .borrow()
                .artist_background_work(ArtworkRequirement::for_settings(settings))
        };
        let Some(work) = work else {
            return;
        };
        let context = ArtistBackgroundPreparation::from_window(self);
        match work {
            ArtistBackgroundWork::Request { track_key, url } => {
                context.set_state(&track_key, &url, ArtistBackgroundState::Downloading);
                let context_for_completion = context.clone();
                let completion_url = url.clone();
                let status = self.artist_background_service.request_exact_url(
                    &track_key,
                    &url,
                    move |completed_track_key, artwork| {
                        context_for_completion.download_completed(
                            completed_track_key,
                            completion_url.clone(),
                            artwork,
                        );
                    },
                );
                match status {
                    ArtworkStatus::Ready(artwork) => {
                        context.download_completed(track_key, url, Some(artwork));
                    }
                    ArtworkStatus::Pending => {}
                    ArtworkStatus::Unavailable => {
                        context.download_completed(track_key, url, None);
                    }
                }
            }
            ArtistBackgroundWork::Prepare {
                track_key,
                url,
                artwork,
            } => context.start_preparation(track_key, url, artwork),
        }
    }

    /// Clears the current track and shows the listening placeholder.
    pub fn set_listening_state(&self) {
        self.state.deferred_artwork.borrow_mut().take();
        let action = self.state.track_presentation.borrow_mut().show_listening();
        TrackPresentation::from_window(self).apply_action(action);
        self.ui.content_transition.reveal_immediately();
    }

    /// Handles an unmatched recognition according to the active keep-last preference.
    pub fn handle_no_recognition(&self) {
        let keep_last = self
            .state
            .settings
            .get()
            .shared
            .always_display_last_recognized_song;
        if !keep_last {
            self.state.deferred_artwork.borrow_mut().take();
        }
        let action = self
            .state
            .track_presentation
            .borrow_mut()
            .no_recognition(keep_last);
        let hold_transition = matches!(action, PresentationAction::HoldTransition);
        TrackPresentation::from_window(self).apply_action(action);
        if !hold_transition {
            self.ui.content_transition.reveal_immediately();
        }
    }
}

fn optional_metadata(value: &Option<String>) -> &str {
    value
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("")
}

/// Selects a new artwork palette without introducing an intermediate fallback.
///
/// Recognition metadata is delivered before its separately fetched artwork.
/// Retaining the current palette only during that gap makes the eventual update
/// go directly from the old song's background to the new song's background,
/// while a definitive fetch failure selects the intentional missing-artwork
/// canvas rather than a generic renderer fallback.
fn background_after_track_update(
    current: Background,
    artwork_background: Option<Background>,
    artwork_pending: bool,
) -> Background {
    match artwork_background {
        Some(background) => background,
        None if artwork_pending => current,
        None => Background::missing_artwork(),
    }
}

/// Visibility decisions for the independent Classic and immersive layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PresentationVisibility {
    classic_content: bool,
    classic_artwork: bool,
    classic_missing_artwork: bool,
    cinema_artwork: bool,
    ambient_artwork: bool,
    missing_artwork_title_card: bool,
    immersive_scrim: bool,
    immersive_info: bool,
    listening: bool,
}

/// Returns widget visibility for each explicit content and display mode.
fn presentation_visibility(
    presentation_mode: PresentationMode,
    display_mode: DisplayMode,
    current_artwork_available: bool,
    retained_artwork_available: bool,
    artwork_pending: bool,
    hide_track_info: bool,
) -> PresentationVisibility {
    if matches!(presentation_mode, PresentationMode::Listening) {
        return PresentationVisibility {
            classic_content: false,
            classic_artwork: false,
            classic_missing_artwork: false,
            cinema_artwork: false,
            ambient_artwork: false,
            missing_artwork_title_card: false,
            immersive_scrim: false,
            immersive_info: false,
            listening: true,
        };
    }

    let show_track_info = display_mode.shows_track_info(hide_track_info);
    let classic_missing_artwork = matches!(display_mode, DisplayMode::Classic)
        && !current_artwork_available
        && !artwork_pending;
    let missing_artwork_title_card =
        matches!(display_mode, DisplayMode::Cinema | DisplayMode::Ambient)
            && !retained_artwork_available
            && !artwork_pending;

    PresentationVisibility {
        classic_content: matches!(display_mode, DisplayMode::Classic),
        // A retained cover is useful as an immersive backdrop while the next
        // cover is prepared, but beside the new metadata in Classic it reads as
        // belonging to the new song. Keep that Classic slot empty until its
        // matching PreparedArtwork is ready. If it becomes definitively
        // unavailable, the neutral Classic card occupies the same stable slot.
        classic_artwork: matches!(display_mode, DisplayMode::Classic) && current_artwork_available,
        classic_missing_artwork,
        cinema_artwork: matches!(display_mode, DisplayMode::Cinema) && retained_artwork_available,
        ambient_artwork: matches!(display_mode, DisplayMode::Ambient) && retained_artwork_available,
        missing_artwork_title_card,
        immersive_scrim: !missing_artwork_title_card
            && show_track_info
            && matches!(display_mode, DisplayMode::Cinema | DisplayMode::Ambient),
        immersive_info: show_track_info && !matches!(display_mode, DisplayMode::Classic),
        listening: false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PresentationVisibility, background_after_track_update, presentation_visibility,
        transition_leg_duration_ms,
    };
    use crate::gui::now_playing_window::DisplayMode;
    use crate::gui::now_playing_window::palette::Background;
    use crate::gui::now_playing_window::state::PresentationMode;

    #[test]
    #[ignore = "requires a GTK display"]
    fn selecting_a_cached_artist_background_does_not_reenter_track_state() {
        use crate::core::artwork::{Artwork, ArtworkStatus};
        use crate::core::preferences::ImmersiveBackgroundSource;
        use crate::core::thread_messages::SongRecognizedMessage;
        use crate::gui::now_playing_window::controller::NowPlayingSettingsController;
        use crate::gui::now_playing_window::palette::{
            ArtworkRequirement, ArtworkVisuals, prepare_immersive_background,
        };
        use crate::gui::now_playing_window::state::ArtistBackgroundState;
        use crate::gui::now_playing_window::{NowPlayingSettings, NowPlayingWindow};
        use adw::prelude::*;
        use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
        use std::io::Cursor;
        use std::sync::Arc;

        adw::init().expect("GTK initialization");
        let mut settings = NowPlayingSettings::default();
        settings.display_mode = DisplayMode::Cinema;
        let window = NowPlayingWindow::new_with_controller(NowPlayingSettingsController::new(
            settings, None,
        ));
        let artist_url = "https://example.test/artist.jpg";
        window.update(&SongRecognizedMessage {
            response_received_at: Some(1),
            track_key: "track".to_string(),
            song_name: "Song".to_string(),
            artist_name: "Artist".to_string(),
            album_name: None,
            release_year: None,
            genre: None,
            artist_background_url: Some(artist_url.to_string()),
            shazam_json: String::new(),
            artwork: ArtworkStatus::Unavailable,
        });

        let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(2, 2, Rgba([40, 80, 120, 255])));
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, ImageFormat::Png).unwrap();
        let source = Arc::new(Artwork::decode(bytes.into_inner()).unwrap());
        let prepared = prepare_immersive_background(&source, ArtworkVisuals::fallback());
        let mut state = window.state.track_presentation.borrow_mut();
        state.apply_artist_background_state(
            "track",
            artist_url,
            ArtistBackgroundState::Ready(prepared, source.clone()),
            ArtworkRequirement::IMMERSIVE_ARTIST,
        );
        state.apply_artist_background_state(
            "track",
            artist_url,
            ArtistBackgroundState::Downloaded(source),
            ArtworkRequirement::IMMERSIVE_ARTIST,
        );
        drop(state);

        window.present();
        settings.shared.immersive_background_source = ImmersiveBackgroundSource::Artist;
        window.state.settings.set(settings);
        window.set_immersive_background_source(ImmersiveBackgroundSource::Artist);

        assert!(matches!(
            &window
                .state
                .track_presentation
                .borrow()
                .displayed_track
                .as_ref()
                .unwrap()
                .artist_background,
            ArtistBackgroundState::Ready(..)
        ));
        if let Some(popover) = window
            .controls
            .display_mode_menu
            .ancestor(gtk::Popover::static_type())
        {
            popover.unparent();
        }
        window.ui.window.destroy();
    }

    #[test]
    #[ignore = "requires a GTK display"]
    fn deferred_artwork_resumes_and_transitions_with_metadata() {
        use crate::core::artwork::Artwork;
        use crate::core::thread_messages::SongRecognizedMessage;
        use crate::gui::now_playing_window::controller::NowPlayingSettingsController;
        use crate::gui::now_playing_window::{
            NowPlayingSettings, NowPlayingWindow, TransitionEffect,
        };
        use adw::prelude::*;
        use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
        use std::io::Cursor;
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        fn message(key: &str, red: u8) -> SongRecognizedMessage {
            let image =
                DynamicImage::ImageRgba8(RgbaImage::from_pixel(32, 32, Rgba([red, 0, 0, 255])));
            let mut bytes = Cursor::new(Vec::new());
            image.write_to(&mut bytes, ImageFormat::Png).unwrap();
            SongRecognizedMessage {
                response_received_at: None,
                track_key: key.to_string(),
                song_name: key.to_string(),
                artist_name: "Artist".to_string(),
                album_name: None,
                release_year: None,
                genre: None,
                artist_background_url: None,
                shazam_json: String::new(),
                artwork: crate::core::artwork::ArtworkStatus::Ready(Arc::new(
                    Artwork::decode(bytes.into_inner()).unwrap(),
                )),
            }
        }
        fn wait_until(predicate: impl Fn() -> bool) {
            glib::MainContext::default().block_on(async {
                let deadline = Instant::now() + Duration::from_secs(5);
                while !predicate() {
                    assert!(Instant::now() < deadline, "presentation did not settle");
                    glib::timeout_future(Duration::from_millis(10)).await;
                }
            });
        }
        fn displayed(window: &NowPlayingWindow, key: &str) -> bool {
            window
                .state
                .track_presentation
                .borrow()
                .displayed_track
                .as_ref()
                .is_some_and(|track| track.track_key == key && track.artwork.is_some())
                && window.ui.content_transition.is_child_revealed()
        }

        adw::init().expect("GTK initialization");
        let mut settings = NowPlayingSettings::default();
        settings.shared.transition = TransitionEffect::Crossfade;
        settings.shared.transition_duration_ms = 500;
        let window = NowPlayingWindow::new_with_controller(NowPlayingSettingsController::new(
            settings, None,
        ));
        window.update(&message("a", 40));
        assert!(window.state.deferred_artwork.borrow().is_some());
        assert!(!displayed(&window, "a"));
        window.present();
        wait_until(|| displayed(&window, "a") && window.ui.window.is_mapped());

        let second = message("b", 80);
        let mut metadata_only = second.clone();
        metadata_only.artwork = crate::core::artwork::ArtworkStatus::Pending;
        window.update(&metadata_only);
        glib::MainContext::default().block_on(glib::timeout_future(Duration::from_millis(600)));
        assert!(
            displayed(&window, "a"),
            "old scene must stay visible during the download"
        );
        window.update(&second);
        wait_until(|| displayed(&window, "b"));
        assert_eq!(window.ui.title_label.label(), "b");
        assert!(window.ui.artwork.paintable().is_some());

        // A missing newer cover must not replace the ready target of either
        // an active hide leg or a retained-scene crossfade/reveal leg.
        for (mode, effect, first_key, next_key) in [
            (
                DisplayMode::Cinema,
                TransitionEffect::SlideLeft,
                "slide-ready",
                "slide-late",
            ),
            (
                DisplayMode::Ambient,
                TransitionEffect::Crossfade,
                "fade-ready",
                "fade-late",
            ),
        ] {
            settings.display_mode = mode;
            settings.shared.transition = effect;
            window.state.settings.set(settings);
            window.set_display_mode(mode);
            let first = message(first_key, 91);
            window.update(&first);
            wait_until(|| {
                let state = window.state.track_presentation.borrow();
                state.pending_track.as_ref().is_some_and(|track| {
                    track.track_key == first_key
                        && track
                            .artwork
                            .as_ref()
                            .is_some_and(|artwork| artwork.ambient_texture.is_some())
                }) || state
                    .displayed_track
                    .as_ref()
                    .is_some_and(|track| track.track_key == first_key)
                    && !window.ui.content_transition.is_child_revealed()
            });
            let next = message(next_key, 111);
            let mut pending = next.clone();
            pending.artwork = crate::core::artwork::ArtworkStatus::Pending;
            window.update(&pending);
            wait_until(|| displayed(&window, first_key));
            context_wait(Duration::from_millis(100));
            assert!(
                displayed(&window, first_key),
                "the ready scene remains visible while newer artwork is missing"
            );
            window.update(&next);
            wait_until(|| displayed(&window, next_key));
        }

        settings.display_mode = DisplayMode::LightsOff;
        window.state.settings.set(settings);
        window.set_display_mode(DisplayMode::LightsOff);
        window.update(&message("c", 120));
        assert!(window.state.deferred_artwork.borrow().is_some());
        assert!(!displayed(&window, "c"));
        settings.display_mode = DisplayMode::Classic;
        window.state.settings.set(settings);
        window.set_display_mode(DisplayMode::Classic);
        wait_until(|| displayed(&window, "c"));

        window.close();
        window.update(&message("d", 160));
        window.update(&message("e", 200));
        assert!(!displayed(&window, "e"));
        window.present();
        wait_until(|| displayed(&window, "e"));
        // The reusable window normally lives until shutdown. Explicit test
        // teardown must also detach its manually parented context popover.
        if let Some(popover) = window
            .controls
            .display_mode_menu
            .ancestor(gtk::Popover::static_type())
        {
            popover.unparent();
        }
        window.ui.window.destroy();
    }

    fn context_wait(duration: std::time::Duration) {
        glib::MainContext::default().block_on(glib::timeout_future(duration));
    }

    #[test]
    fn transition_duration_is_split_across_hide_and_reveal() {
        assert_eq!(transition_leg_duration_ms(500), 250);
        assert_eq!(transition_leg_duration_ms(2_000), 1_000);
        assert_eq!(transition_leg_duration_ms(5_000), 2_500);
    }

    #[test]
    fn missing_artwork_never_displays_the_listening_placeholder() {
        assert_eq!(
            presentation_visibility(
                PresentationMode::TrackWithoutArtwork,
                DisplayMode::Ambient,
                false,
                false,
                false,
                false,
            ),
            PresentationVisibility {
                classic_content: false,
                classic_artwork: false,
                classic_missing_artwork: false,
                cinema_artwork: false,
                ambient_artwork: false,
                missing_artwork_title_card: true,
                immersive_scrim: false,
                immersive_info: true,
                listening: false,
            }
        );
    }

    #[test]
    fn listening_is_mode_independent() {
        for display_mode in DisplayMode::ALL {
            let visibility = presentation_visibility(
                PresentationMode::Listening,
                display_mode,
                true,
                true,
                false,
                // Listening hides all metadata regardless of this preference.
                true,
            );
            assert!(visibility.listening);
            assert!(!visibility.classic_content);
            assert!(!visibility.cinema_artwork);
            assert!(!visibility.ambient_artwork);
            assert!(!visibility.immersive_info);
        }
    }

    #[test]
    fn artwork_is_routed_to_the_selected_mode_only() {
        let classic = presentation_visibility(
            PresentationMode::TrackWithArtwork,
            DisplayMode::Classic,
            true,
            true,
            false,
            false,
        );
        assert!(classic.classic_content);
        assert!(classic.classic_artwork);
        assert!(!classic.immersive_info);

        let cinema = presentation_visibility(
            PresentationMode::TrackWithArtwork,
            DisplayMode::Cinema,
            true,
            true,
            false,
            false,
        );
        assert!(cinema.cinema_artwork);
        assert!(cinema.immersive_info);

        let ambient = presentation_visibility(
            PresentationMode::TrackWithArtwork,
            DisplayMode::Ambient,
            true,
            true,
            false,
            false,
        );
        assert!(ambient.ambient_artwork);
        assert!(ambient.immersive_info);

        let lights_off = presentation_visibility(
            PresentationMode::TrackWithArtwork,
            DisplayMode::LightsOff,
            true,
            true,
            false,
            false,
        );
        assert!(!lights_off.classic_artwork);
        assert!(!lights_off.cinema_artwork);
        assert!(!lights_off.ambient_artwork);
        assert!(lights_off.immersive_info);
    }

    #[test]
    fn pending_artwork_is_retained_only_as_an_immersive_backdrop() {
        let classic = presentation_visibility(
            PresentationMode::TrackWithoutArtwork,
            DisplayMode::Classic,
            false,
            true,
            true,
            false,
        );
        assert!(!classic.classic_artwork);
        assert!(!classic.classic_missing_artwork);

        let cinema = presentation_visibility(
            PresentationMode::TrackWithoutArtwork,
            DisplayMode::Cinema,
            false,
            true,
            true,
            false,
        );
        assert!(cinema.cinema_artwork);
        assert!(!cinema.missing_artwork_title_card);

        let ambient = presentation_visibility(
            PresentationMode::TrackWithoutArtwork,
            DisplayMode::Ambient,
            false,
            true,
            true,
            false,
        );
        assert!(ambient.ambient_artwork);
        assert!(!ambient.missing_artwork_title_card);
    }

    #[test]
    fn pending_artwork_does_not_prematurely_show_a_missing_artwork_card() {
        for display_mode in [
            DisplayMode::Classic,
            DisplayMode::Cinema,
            DisplayMode::Ambient,
        ] {
            let visibility = presentation_visibility(
                PresentationMode::TrackWithoutArtwork,
                display_mode,
                false,
                false,
                true,
                false,
            );
            assert!(!visibility.classic_missing_artwork);
            assert!(!visibility.missing_artwork_title_card);
        }
    }

    #[test]
    fn definitive_missing_artwork_selects_the_mode_appropriate_title_card() {
        let classic = presentation_visibility(
            PresentationMode::TrackWithoutArtwork,
            DisplayMode::Classic,
            false,
            false,
            false,
            false,
        );
        assert!(classic.classic_missing_artwork);
        assert!(!classic.missing_artwork_title_card);

        for display_mode in [DisplayMode::Cinema, DisplayMode::Ambient] {
            let immersive = presentation_visibility(
                PresentationMode::TrackWithoutArtwork,
                display_mode,
                false,
                false,
                false,
                false,
            );
            assert!(!immersive.classic_missing_artwork);
            assert!(immersive.missing_artwork_title_card);
            assert!(!immersive.immersive_scrim);
        }

        let lights_off = presentation_visibility(
            PresentationMode::TrackWithoutArtwork,
            DisplayMode::LightsOff,
            false,
            false,
            false,
            false,
        );
        assert!(!lights_off.classic_missing_artwork);
        assert!(!lights_off.missing_artwork_title_card);
    }

    #[test]
    fn artist_only_artwork_still_uses_the_classic_missing_cover_card() {
        let visibility = presentation_visibility(
            PresentationMode::TrackWithArtwork,
            DisplayMode::Classic,
            false,
            true,
            false,
            false,
        );

        assert!(visibility.classic_missing_artwork);
        assert!(!visibility.missing_artwork_title_card);
    }

    #[test]
    fn listening_never_displays_a_missing_artwork_card() {
        for display_mode in DisplayMode::ALL {
            let visibility = presentation_visibility(
                PresentationMode::Listening,
                display_mode,
                false,
                false,
                false,
                false,
            );
            assert!(!visibility.classic_missing_artwork);
            assert!(!visibility.missing_artwork_title_card);
        }
    }

    #[test]
    fn hidden_track_info_suppresses_metadata_in_visual_immersive_modes() {
        for display_mode in [DisplayMode::Cinema, DisplayMode::Ambient] {
            let visibility = presentation_visibility(
                PresentationMode::TrackWithArtwork,
                display_mode,
                true,
                true,
                false,
                true,
            );
            assert!(!visibility.immersive_info);
            assert!(!visibility.immersive_scrim);
        }
    }

    #[test]
    fn lights_off_always_keeps_track_info_visible() {
        let visibility = presentation_visibility(
            PresentationMode::TrackWithArtwork,
            DisplayMode::LightsOff,
            true,
            true,
            false,
            true,
        );
        assert!(visibility.immersive_info);
    }

    #[test]
    fn pending_artwork_preserves_the_previous_background() {
        let previous = Background {
            top: (10, 20, 30),
            bottom: (1, 2, 3),
        };

        assert_eq!(
            background_after_track_update(previous, None, true),
            previous
        );
    }

    #[test]
    fn downloaded_artwork_replaces_the_previous_background() {
        let previous = Background {
            top: (10, 20, 30),
            bottom: (1, 2, 3),
        };
        let next = Background {
            top: (40, 50, 60),
            bottom: (4, 5, 6),
        };

        assert_eq!(
            background_after_track_update(previous, Some(next), false),
            next
        );
    }

    #[test]
    fn unavailable_artwork_uses_the_missing_artwork_background() {
        let previous = Background {
            top: (10, 20, 30),
            bottom: (1, 2, 3),
        };

        assert_eq!(
            background_after_track_update(previous, None, false),
            Background::missing_artwork()
        );
    }
}
