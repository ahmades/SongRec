//! Recognition-result presentation and transition handling.

use super::background::{CachedGradient, redraw_background};
use super::palette::{Background, prepare_artwork};
use super::state::{PresentationAction, PresentationMode, PresentedTrack, TrackPresentationState};
use super::{NowPlayingSettings, NowPlayingWindow, TransitionEffect};
use crate::core::thread_messages::SongRecognizedMessage;
use adw::prelude::*;
use gettextrs::gettext;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Converts the user-facing total transition duration into one hide/reveal leg.
pub(super) fn transition_leg_duration_ms(total_duration_ms: u64) -> u32 {
    (total_duration_ms / 2).max(1).min(u64::from(u32::MAX)) as u32
}

/// The GTK objects and shared state required to render one presentation.
///
/// GTK callbacks own this lightweight clone instead of borrowing the window.
#[derive(Clone)]
struct TrackPresentation {
    artwork: gtk::Picture,
    artwork_placeholder: gtk::Label,
    title_label: gtk::Label,
    artist_label: gtk::Label,
    album_label: gtk::Label,
    details_label: gtk::Label,
    background_area: gtk::DrawingArea,
    gradient_surface: Rc<RefCell<Option<CachedGradient>>>,
    settings: Rc<Cell<NowPlayingSettings>>,
    current_background: Rc<Cell<Background>>,
    track_state: Rc<RefCell<TrackPresentationState>>,
}

impl TrackPresentation {
    fn from_window(window: &NowPlayingWindow) -> Self {
        Self {
            artwork: window.ui.artwork.clone(),
            artwork_placeholder: window.ui.artwork_placeholder.clone(),
            title_label: window.ui.title_label.clone(),
            artist_label: window.ui.artist_label.clone(),
            album_label: window.ui.album_label.clone(),
            details_label: window.ui.details_label.clone(),
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
            PresentationAction::None | PresentationAction::BeginTransition => {}
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
            Some(artwork.background)
        } else {
            self.artwork.set_paintable(Option::<&gdk::Texture>::None);
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
        self.artwork.set_paintable(Option::<&gdk::Texture>::None);
        self.title_label.set_label("");
        self.artist_label.set_label("");
        self.album_label.set_label("");
        self.details_label.set_label("");
        self.sync_artwork_visibility();
    }

    fn set_metadata(&self, track: &PresentedTrack) {
        self.title_label.set_label(&track.song_name);
        self.artist_label.set_label(&track.artist_name);
        self.album_label
            .set_label(optional_metadata(&track.album_name));
        self.details_label
            .set_label(optional_metadata(&track.release_year));
    }

    fn sync_artwork_visibility(&self) {
        let mode = self.track_state.borrow().mode;
        let (show_artwork, show_listening) =
            artwork_visibility(mode, self.settings.get().lights_off);
        self.artwork.set_visible(show_artwork);
        self.artwork_placeholder.set_visible(show_listening);
    }

    fn apply_background(&self) {
        let settings = self.settings.get();
        redraw_background(
            &self.background_area,
            &self.gradient_surface,
            self.current_background.get(),
            settings.background_style,
            settings.lights_off,
        );
    }
}

impl NowPlayingWindow {
    /// Synchronizes artwork and Listening visibility with the explicit presentation mode.
    pub(super) fn sync_artwork_visibility(&self) {
        TrackPresentation::from_window(self).sync_artwork_visibility();
    }

    /// Installs one completion callback for every track transition made by this window.
    pub(super) fn setup_track_transition_handlers(&self) {
        let track_state_for_completion = self.state.track_presentation.clone();
        let presentation_for_completion = TrackPresentation::from_window(self);
        self.ui
            .content_revealer
            .connect_child_revealed_notify(move |revealer| {
                if revealer.is_child_revealed() {
                    return;
                }

                let action = track_state_for_completion.borrow_mut().transition_hidden();
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
        let artwork = message.cover_image.as_deref().map(prepare_artwork);
        let track = Rc::new(PresentedTrack::from_message(message, artwork));
        if !track.has_visible_information() {
            self.handle_no_recognition();
            return;
        }

        let settings = self.state.settings.get();
        let can_animate = !matches!(settings.transition, TransitionEffect::None)
            && self.ui.window.is_mapped()
            && self.ui.content_revealer.is_child_revealed();
        let action = self
            .state
            .track_presentation
            .borrow_mut()
            .receive_track(track, can_animate);

        match action {
            PresentationAction::BeginTransition => {
                self.ui
                    .content_revealer
                    .set_transition_duration(transition_leg_duration_ms(
                        settings.transition_duration_ms,
                    ));
                self.ui
                    .content_revealer
                    .set_transition_type(settings.transition.revealer_type());
                self.ui.content_revealer.set_reveal_child(false);
            }
            PresentationAction::RenderTrack(track) => {
                TrackPresentation::from_window(self)
                    .apply_action(PresentationAction::RenderTrack(track));
                self.ui.content_revealer.set_reveal_child(true);
            }
            PresentationAction::None | PresentationAction::RenderListening => {}
        }
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
            .always_display_last_recognized_song;
        let action = self
            .state
            .track_presentation
            .borrow_mut()
            .no_recognition(keep_last);
        TrackPresentation::from_window(self).apply_action(action);
        self.ui.content_revealer.set_reveal_child(true);
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

/// Returns widget visibility for each explicit presentation state.
pub(super) fn artwork_visibility(mode: PresentationMode, lights_off: bool) -> (bool, bool) {
    match (mode, lights_off) {
        (PresentationMode::Listening, _) => (false, true),
        (PresentationMode::TrackWithArtwork, false) => (true, false),
        (PresentationMode::TrackWithArtwork, true) | (PresentationMode::TrackWithoutArtwork, _) => {
            (false, false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{artwork_visibility, background_after_track_update, transition_leg_duration_ms};
    use crate::core::artwork::ArtworkBackground as Background;
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
            artwork_visibility(PresentationMode::TrackWithoutArtwork, false),
            (false, false)
        );
        assert_eq!(
            artwork_visibility(PresentationMode::TrackWithoutArtwork, true),
            (false, false)
        );
    }

    #[test]
    fn listening_remains_visible_in_lights_off_mode() {
        assert_eq!(
            artwork_visibility(PresentationMode::Listening, true),
            (false, true)
        );
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
