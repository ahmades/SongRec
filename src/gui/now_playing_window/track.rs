//! Recognition-result presentation and transition handling.

use super::background::{CachedGradient, redraw_background};
use super::palette::{ArtworkVisuals, Background, prepare_artwork, visuals_from_artwork};
use super::state::{PresentationAction, PresentationMode, PresentedTrack, TrackPresentationState};
use super::ui::{AmbientArtworkLayout, CinemaArtworkLayout, configure_immersive_info};
use super::{DisplayMode, NowPlayingSettings, NowPlayingWindow, TransitionEffect};
use crate::core::artwork::Artwork;
use crate::core::thread_messages::SongRecognizedMessage;
use adw::prelude::*;
use gettextrs::gettext;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

/// Converts the user-facing total transition duration into one hide/reveal leg.
pub(super) fn transition_leg_duration_ms(total_duration_ms: u64) -> u32 {
    (total_duration_ms / 2).max(1).min(u64::from(u32::MAX)) as u32
}

/// Starts the hide leg using the currently selected, already-supported effect.
fn begin_track_transition(revealer: &gtk::Revealer, settings: NowPlayingSettings) {
    revealer.set_transition_duration(transition_leg_duration_ms(
        settings.shared.transition_duration_ms,
    ));
    revealer.set_transition_type(settings.shared.transition.revealer_type());
    revealer.set_reveal_child(false);
}

/// The GTK objects and shared state required to render one presentation.
///
/// GTK callbacks own this lightweight clone instead of borrowing the window.
#[derive(Clone)]
pub(super) struct TrackPresentation {
    classic_content: gtk::Box,
    artwork: gtk::Picture,
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
    current_background: Rc<Cell<Background>>,
    track_state: Rc<RefCell<TrackPresentationState>>,
}

impl TrackPresentation {
    pub(super) fn from_window(window: &NowPlayingWindow) -> Self {
        Self {
            classic_content: window.ui.classic_content.clone(),
            artwork: window.ui.artwork.clone(),
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
        // The global placeholder belongs exclusively to Listening mode; a
        // recognized track without artwork intentionally leaves this empty.
        self.artwork_placeholder.set_label("");
        self.set_metadata(track);

        let artwork_background = if let Some(artwork) = track.artwork.as_ref() {
            self.artwork.set_paintable(Some(&artwork.texture));
            self.cinema_artwork
                .set_artwork(Some(&artwork.texture), Some(&artwork.ambient_texture));
            self.ambient_artwork
                .set_artwork(Some(&artwork.texture), Some(&artwork.ambient_texture));
            Some(artwork.background)
        } else {
            // Recognition metadata arrives before its separately downloaded
            // cover. Keep the outgoing visuals until the new preparation is
            // ready so immersive modes never flash through an empty frame.
            if !track.artwork_pending {
                self.clear_artwork();
            }
            None
        };
        self.current_background.set(background_after_track_update(
            self.current_background.get(),
            artwork_background,
            track.artwork_pending,
        ));

        self.apply_background();
        self.sync_artwork_visibility();
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
        let current_artwork_available = matches!(state.mode, PresentationMode::TrackWithArtwork);
        let retained_artwork_available = current_artwork_available
            || state.displayed_track.as_ref().is_some_and(|track| {
                track.artwork_pending && !matches!(state.mode, PresentationMode::Listening)
            });
        let settings = self.settings.get();
        let visibility = presentation_visibility(
            state.mode,
            settings.display_mode,
            current_artwork_available,
            retained_artwork_available,
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
            self.cinema_artwork.framing(width, height),
            width,
            height,
        );

        self.classic_content.set_visible(visibility.classic_content);
        self.artwork.set_visible(visibility.classic_artwork);
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
}

impl NowPlayingWindow {
    /// Installs one completion callback for every track transition made by this window.
    pub(super) fn setup_track_transition_handlers(&self) {
        let track_state_for_completion = self.state.track_presentation.clone();
        let settings_for_completion = self.state.settings.clone();
        let presentation_for_completion = TrackPresentation::from_window(self);
        self.ui
            .content_revealer
            .connect_child_revealed_notify(move |revealer| {
                if revealer.is_child_revealed() {
                    return;
                }

                let wait_for_artwork = settings_for_completion
                    .get()
                    .display_mode
                    .uses_immersive_artwork();
                let action = track_state_for_completion
                    .borrow_mut()
                    .transition_hidden(wait_for_artwork);
                if matches!(action, PresentationAction::HoldTransition) {
                    return;
                }

                presentation_for_completion.apply_action(action);
                // Even a stale/spurious hidden notification must not leave the
                // reusable window exposing only its background.
                revealer.set_reveal_child(true);
            });

        let track_state_for_hide = self.state.track_presentation.clone();
        let presentation_for_hide = TrackPresentation::from_window(self);
        let revealer_for_hide = self.ui.content_revealer.clone();
        self.ui.window.connect_visible_notify(move |window| {
            if window.is_visible() {
                return;
            }

            let action = track_state_for_hide.borrow_mut().flush_pending_track();
            presentation_for_hide.apply_action(action);
            revealer_for_hide.set_reveal_child(true);
        });
    }

    /// Refreshes the displayed song metadata and artwork from a recognition result.
    pub fn update(&self, message: &SongRecognizedMessage) {
        let prepared_artwork = message.cover_image.as_ref().and_then(|artwork| {
            self.state
                .track_presentation
                .borrow()
                .prepared_artwork_for(&message.track_key, artwork)
        });
        let visuals_pending = message.cover_image.is_some() && prepared_artwork.is_none();
        let track = Rc::new(PresentedTrack::from_message(
            message,
            prepared_artwork,
            visuals_pending,
        ));
        if !track.has_visible_information() {
            self.handle_no_recognition();
            return;
        }

        let settings = self.state.settings.get();
        let can_animate = !matches!(settings.shared.transition, TransitionEffect::None)
            && self.ui.window.is_mapped()
            && self.ui.content_revealer.is_child_revealed();
        let action = self.state.track_presentation.borrow_mut().receive_track(
            track,
            can_animate,
            settings.display_mode.uses_immersive_artwork(),
        );

        match action {
            PresentationAction::BeginTransition => {
                begin_track_transition(&self.ui.content_revealer, settings);
            }
            PresentationAction::RenderTrack(track) => {
                TrackPresentation::from_window(self)
                    .apply_action(PresentationAction::RenderTrack(track));
                self.ui.content_revealer.set_reveal_child(true);
            }
            PresentationAction::None
            | PresentationAction::HoldTransition
            | PresentationAction::RenderListening => {}
        }

        if visuals_pending {
            self.prepare_artwork_visuals(
                message.track_key.clone(),
                message
                    .cover_image
                    .as_ref()
                    .expect("artwork visual preparation requires artwork")
                    .clone(),
            );
        }
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
                settings.display_mode.uses_immersive_artwork(),
            );

        match action {
            PresentationAction::BeginTransition => {
                begin_track_transition(&self.ui.content_revealer, settings);
            }
            PresentationAction::RenderTrack(track) => {
                TrackPresentation::from_window(self)
                    .apply_action(PresentationAction::RenderTrack(track));
                self.ui.content_revealer.set_reveal_child(true);
            }
            PresentationAction::RenderListening => {
                TrackPresentation::from_window(self).apply_action(action);
                self.ui.content_revealer.set_reveal_child(true);
            }
            PresentationAction::None | PresentationAction::HoldTransition => {}
        }
    }

    /// Prepares Now Playing-only palettes and Ambient pixels away from GTK's main thread.
    fn prepare_artwork_visuals(&self, track_key: String, artwork: Arc<Artwork>) {
        if !self
            .state
            .artwork_preparations
            .borrow_mut()
            .start(&track_key, &artwork)
        {
            return;
        }

        let preparation_jobs = self.state.artwork_preparations.clone();
        let track_state = self.state.track_presentation.clone();
        let presentation = TrackPresentation::from_window(self);
        let revealer = self.ui.content_revealer.clone();
        let settings = self.state.settings.clone();
        glib::spawn_future_local(async move {
            let artwork_for_worker = artwork.clone();
            let visuals = match gio::spawn_blocking(move || {
                visuals_from_artwork(&artwork_for_worker)
            })
            .await
            {
                Ok(visuals) => visuals,
                Err(_) => {
                    log::warn!("Now Playing artwork preparation task panicked");
                    ArtworkVisuals::fallback()
                }
            };

            if !preparation_jobs.borrow_mut().finish(&track_key, &artwork) {
                return;
            }

            let prepared = prepare_artwork(&artwork, visuals);
            let action = track_state
                .borrow_mut()
                .apply_prepared_artwork(&track_key, &artwork, prepared);
            match action {
                PresentationAction::BeginTransition => {
                    begin_track_transition(&revealer, settings.get());
                }
                PresentationAction::RenderTrack(track) => {
                    presentation.apply_action(PresentationAction::RenderTrack(track));
                    revealer.set_reveal_child(true);
                }
                PresentationAction::None
                | PresentationAction::HoldTransition
                | PresentationAction::RenderListening => {}
            }
        });
    }

    /// Clears the current track and shows the listening placeholder.
    pub fn set_listening_state(&self) {
        let action = self.state.track_presentation.borrow_mut().show_listening();
        TrackPresentation::from_window(self).apply_action(action);
        self.ui.content_revealer.set_reveal_child(true);
    }

    /// Handles an unmatched recognition according to the active keep-last preference.
    pub fn handle_no_recognition(&self) {
        let keep_last = self
            .state
            .settings
            .get()
            .shared
            .always_display_last_recognized_song;
        let action = self
            .state
            .track_presentation
            .borrow_mut()
            .no_recognition(keep_last);
        let hold_transition = matches!(action, PresentationAction::HoldTransition);
        TrackPresentation::from_window(self).apply_action(action);
        if !hold_transition {
            self.ui.content_revealer.set_reveal_child(true);
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
/// while a definitive fetch failure still restores the neutral fallback.
fn background_after_track_update(
    current: Background,
    artwork_background: Option<Background>,
    artwork_pending: bool,
) -> Background {
    match artwork_background {
        Some(background) => background,
        None if artwork_pending => current,
        None => Background::fallback(),
    }
}

/// Visibility decisions for the independent Classic and immersive layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PresentationVisibility {
    classic_content: bool,
    classic_artwork: bool,
    cinema_artwork: bool,
    ambient_artwork: bool,
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
    hide_track_info: bool,
) -> PresentationVisibility {
    if matches!(presentation_mode, PresentationMode::Listening) {
        return PresentationVisibility {
            classic_content: false,
            classic_artwork: false,
            cinema_artwork: false,
            ambient_artwork: false,
            immersive_scrim: false,
            immersive_info: false,
            listening: true,
        };
    }

    let show_track_info = !hide_track_info || !display_mode.supports_hiding_track_info();

    PresentationVisibility {
        classic_content: matches!(display_mode, DisplayMode::Classic),
        // A retained cover is useful as an immersive backdrop while the next
        // cover is prepared, but beside the new metadata in Classic it reads as
        // belonging to the new song. Keep that Classic slot empty until its
        // matching PreparedArtwork is ready.
        classic_artwork: matches!(display_mode, DisplayMode::Classic) && current_artwork_available,
        cinema_artwork: matches!(display_mode, DisplayMode::Cinema) && retained_artwork_available,
        ambient_artwork: matches!(display_mode, DisplayMode::Ambient) && retained_artwork_available,
        immersive_scrim: show_track_info
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
            ),
            PresentationVisibility {
                classic_content: false,
                classic_artwork: false,
                cinema_artwork: false,
                ambient_artwork: false,
                immersive_scrim: true,
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
        );
        assert!(cinema.cinema_artwork);
        assert!(cinema.immersive_info);

        let ambient = presentation_visibility(
            PresentationMode::TrackWithArtwork,
            DisplayMode::Ambient,
            true,
            true,
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
            false,
        );
        assert!(!classic.classic_artwork);

        let cinema = presentation_visibility(
            PresentationMode::TrackWithoutArtwork,
            DisplayMode::Cinema,
            false,
            true,
            false,
        );
        assert!(cinema.cinema_artwork);

        let ambient = presentation_visibility(
            PresentationMode::TrackWithoutArtwork,
            DisplayMode::Ambient,
            false,
            true,
            false,
        );
        assert!(ambient.ambient_artwork);
    }

    #[test]
    fn hidden_track_info_suppresses_metadata_in_visual_immersive_modes() {
        for display_mode in [DisplayMode::Cinema, DisplayMode::Ambient] {
            let visibility = presentation_visibility(
                PresentationMode::TrackWithArtwork,
                display_mode,
                true,
                true,
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
    fn unavailable_artwork_uses_the_fallback_background() {
        let previous = Background {
            top: (10, 20, 30),
            bottom: (1, 2, 3),
        };

        assert_eq!(
            background_after_track_update(previous, None, false),
            Background::fallback()
        );
    }
}
