//! Recognition-result presentation and transition handling.

use super::background::{CachedGradient, redraw_background};
use super::palette::{Background, from_cover_image};
use super::{BackgroundStyle, NowPlayingWindow, TransitionEffect};
use crate::core::thread_messages::SongRecognizedMessage;
use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// The GTK objects and shared state required to render one track.
///
/// Keeping this separate from [`NowPlayingWindow`] lets delayed transitions
/// use the exact same rendering path as immediate recognition updates. GTK
/// callbacks must own their captures, so they cannot borrow the window.
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
    background_style: Rc<Cell<BackgroundStyle>>,
    current_background: Rc<Cell<Background>>,
    lights_off: Rc<Cell<bool>>,
    showing_listening: Rc<Cell<bool>>,
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
            background_style: window.state.background_style.clone(),
            current_background: window.state.current_background.clone(),
            lights_off: window.state.lights_off.clone(),
            showing_listening: window.state.showing_listening.clone(),
        }
    }

    /// Synchronizes artwork and placeholder visibility with the rendered state.
    fn sync_artwork_visibility(&self) {
        let has_paintable = self.artwork.paintable().is_some();

        if self.lights_off.get() {
            self.artwork.set_visible(false);
            self.artwork_placeholder
                .set_visible(self.showing_listening.get());
            return;
        }

        if self.showing_listening.get() {
            self.artwork.set_visible(false);
            self.artwork_placeholder.set_visible(true);
            return;
        }

        if has_paintable {
            self.artwork.set_visible(true);
            self.artwork_placeholder.set_visible(false);
        } else {
            self.artwork.set_visible(false);
            self.artwork_placeholder.set_visible(true);
        }
    }

    /// Renders a recognized track immediately.
    fn apply_track_update(&self, message: &SongRecognizedMessage) {
        self.showing_listening.set(false);
        self.set_metadata(message);

        if let Some(bytes) = message.cover_image.as_deref() {
            self.apply_cover(bytes);
        } else {
            self.set_missing_cover_state();
        }
    }

    /// Renders the deterministic empty/listening state.
    fn show_listening_state(&self) {
        self.showing_listening.set(true);
        self.artwork.set_paintable(Option::<&gdk::Texture>::None);
        self.title_label.set_label("");
        self.artist_label.set_label("");
        self.album_label.set_label("");
        self.details_label.set_label("");
        self.sync_artwork_visibility();
    }

    /// Updates the title, artist, album, and release-year labels.
    fn set_metadata(&self, message: &SongRecognizedMessage) {
        self.title_label.set_label(&message.song_name);
        self.artist_label.set_label(&message.artist_name);
        self.album_label.set_label(
            message
                .album_name
                .as_deref()
                .filter(|value| !value.is_empty())
                .unwrap_or(""),
        );
        self.details_label.set_label(
            message
                .release_year
                .as_deref()
                .filter(|value| !value.is_empty())
                .unwrap_or(""),
        );
    }

    /// Decodes cover art and derives its background, falling back cleanly on failure.
    fn apply_cover(&self, bytes: &[u8]) {
        if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from(bytes)) {
            self.artwork.set_paintable(Some(&texture));
            self.current_background.set(from_cover_image(bytes));
            self.apply_background();
            self.sync_artwork_visibility();
        } else {
            self.set_missing_cover_state();
        }
    }

    /// Clears artwork and restores the fallback background used without cover art.
    fn set_missing_cover_state(&self) {
        self.artwork.set_paintable(Option::<&gdk::Texture>::None);
        self.current_background.set(Background::fallback());
        self.apply_background();
        self.sync_artwork_visibility();
    }

    /// Applies the current background state using the same rules as the window renderer.
    fn apply_background(&self) {
        redraw_background(
            &self.background_area,
            &self.gradient_surface,
            self.current_background.get(),
            self.background_style.get(),
            self.lights_off.get(),
        );
    }
}

impl NowPlayingWindow {
    /// Synchronizes artwork and placeholder visibility with the current window state.
    pub(super) fn sync_artwork_visibility(&self) {
        TrackPresentation::from_window(self).sync_artwork_visibility();
    }

    /// Refreshes the displayed song metadata and artwork from a recognition result.
    pub fn update(&self, message: &SongRecognizedMessage) {
        if !has_track_information(message) {
            self.handle_no_recognition();
            return;
        }

        // A recognized track is authoritative even when it has the same key.
        // Without this invalidation, a delayed callback from an earlier result
        // could overwrite newer metadata or cover art.
        self.cancel_pending_transition();

        let should_transition = should_transition_to_track(
            self.state.last_track_key.borrow().as_deref(),
            &message.track_key,
        );
        *self.state.last_track_key.borrow_mut() = Some(message.track_key.clone());

        if should_transition {
            self.transition_to_track(message.clone());
        } else {
            self.apply_track_update(message);
            self.ui.content_revealer.set_reveal_child(true);
        }
    }

    /// Applies a recognized track immediately without running a visual transition.
    fn apply_track_update(&self, message: &SongRecognizedMessage) {
        TrackPresentation::from_window(self).apply_track_update(message);
    }

    /// Invalidates a pending transition and restores the currently rendered content.
    fn cancel_pending_transition(&self) {
        self.state
            .transition_generation
            .set(next_generation(self.state.transition_generation.get()));
        self.ui.content_revealer.set_reveal_child(true);
    }

    /// Runs the configured lightweight transition before displaying a newly recognized track.
    fn transition_to_track(&self, message: SongRecognizedMessage) {
        let effect = self.state.transition.get();
        if should_apply_track_immediately(effect, self.ui.content_revealer.is_child_revealed()) {
            self.apply_track_update(&message);
            self.ui.content_revealer.set_reveal_child(true);
            return;
        }

        let generation = next_generation(self.state.transition_generation.get());
        self.state.transition_generation.set(generation);
        self.ui
            .content_revealer
            .set_transition_duration(self.state.transition_duration_ms.get() as u32);
        self.ui
            .content_revealer
            .set_transition_type(effect.revealer_type());

        let revealer = self.ui.content_revealer.clone();
        let generation_state = self.state.transition_generation.clone();
        let presentation = TrackPresentation::from_window(self);
        let handler_id = Rc::new(RefCell::new(None));
        let handler_id_for_callback = handler_id.clone();

        // `child-revealed` changes only after GTK has actually completed the
        // outgoing animation. This is more accurate than a duration-matched
        // timer and naturally accounts for GTK animation settings.
        let handler = revealer.connect_child_revealed_notify(move |revealer| {
            if generation_state.get() != generation {
                if let Some(handler) = handler_id_for_callback.borrow_mut().take() {
                    revealer.disconnect(handler);
                }
                return;
            }

            if revealer.is_child_revealed() {
                return;
            }

            if let Some(handler) = handler_id_for_callback.borrow_mut().take() {
                revealer.disconnect(handler);
            }

            presentation.apply_track_update(&message);
            revealer.set_reveal_child(true);
        });
        *handler_id.borrow_mut() = Some(handler);

        // Keep the old track visible while it animates out. The shared
        // presentation path writes the new track only once GTK reports that
        // the old child is fully hidden.
        revealer.set_reveal_child(false);
    }

    /// Clears the current track and shows the listening placeholder while preserving the background.
    pub fn set_listening_state(&self) {
        self.cancel_pending_transition();
        self.set_listening_state_after_transition_cancelled();
    }

    fn set_listening_state_after_transition_cancelled(&self) {
        TrackPresentation::from_window(self).show_listening_state();
        self.ui.content_revealer.set_reveal_child(true);
    }

    /// Handles a recognition attempt that produced no track information according to the active preference.
    pub fn handle_no_recognition(&self) {
        // An unmatched result must cancel any in-progress transition even if
        // the preference keeps the last committed presentation. Otherwise a
        // fullscreen/configure event can leave the revealer hidden, exposing
        // only the background.
        self.cancel_pending_transition();

        if should_show_listening_for_no_recognition(
            self.state
                .settings
                .get()
                .always_display_last_recognized_song,
        ) {
            self.set_listening_state_after_transition_cancelled();
        }
    }
}

/// Returns whether the message contains enough information to present a track.
fn has_track_information(message: &SongRecognizedMessage) -> bool {
    !message.song_name.trim().is_empty()
        || !message.artist_name.trim().is_empty()
        || message.cover_image.is_some()
}

/// Returns whether a non-empty incoming key should replace the last recognized track with a transition.
fn should_transition_to_track(previous_track_key: Option<&str>, incoming_track_key: &str) -> bool {
    !incoming_track_key.is_empty()
        && previous_track_key.is_some_and(|previous| previous != incoming_track_key)
}

/// Returns whether the newest track must be rendered without starting another revealer cycle.
///
/// `child-revealed` stays false while GTK is revealing a child. Reversing that
/// in-progress reveal into a second hide does not produce a new false edge, so
/// a callback waiting for one could otherwise remain pending forever. Rendering
/// the latest result immediately in that short interval keeps the presentation
/// current; the next stable update can animate normally.
fn should_apply_track_immediately(effect: TransitionEffect, child_revealed: bool) -> bool {
    matches!(effect, TransitionEffect::None) || !child_revealed
}

/// Returns whether an unmatched recognition should replace the current presentation.
fn should_show_listening_for_no_recognition(always_display_last_recognized_song: bool) -> bool {
    !always_display_last_recognized_song
}

/// Advances a generation token, including the defined wrapping behavior at its maximum value.
fn next_generation(generation: u64) -> u64 {
    generation.wrapping_add(1)
}

#[cfg(test)]
mod tests {
    use super::{
        has_track_information, next_generation, should_apply_track_immediately,
        should_show_listening_for_no_recognition, should_transition_to_track,
    };
    use crate::core::thread_messages::SongRecognizedMessage;
    use crate::gui::now_playing_window::TransitionEffect;

    fn message(
        song_name: &str,
        artist_name: &str,
        cover_image: Option<Vec<u8>>,
    ) -> SongRecognizedMessage {
        SongRecognizedMessage {
            artist_name: artist_name.to_string(),
            album_name: None,
            song_name: song_name.to_string(),
            cover_image,
            track_key: String::new(),
            release_year: None,
            genre: None,
            shazam_json: String::new(),
        }
    }

    #[test]
    fn track_information_accepts_any_visible_track_field() {
        assert!(!has_track_information(&message("  ", "\n", None)));
        assert!(has_track_information(&message("Song", "", None)));
        assert!(has_track_information(&message("", "Artist", None)));
        assert!(has_track_information(&message("", "", Some(Vec::new()))));
    }

    #[test]
    fn transition_requires_a_changed_non_empty_track_key() {
        assert!(!should_transition_to_track(None, "track-a"));
        assert!(!should_transition_to_track(Some("track-a"), "track-a"));
        assert!(!should_transition_to_track(Some("track-a"), ""));
        assert!(should_transition_to_track(Some("track-a"), "track-b"));
    }

    #[test]
    fn in_progress_reveals_apply_the_latest_track_without_a_second_transition() {
        assert!(should_apply_track_immediately(TransitionEffect::None, true));
        assert!(should_apply_track_immediately(
            TransitionEffect::Crossfade,
            false
        ));
        assert!(!should_apply_track_immediately(
            TransitionEffect::Crossfade,
            true
        ));
    }

    #[test]
    fn no_match_keeps_an_existing_or_transitioning_track_when_configured() {
        assert!(!should_show_listening_for_no_recognition(true));
        assert!(should_show_listening_for_no_recognition(false));
    }

    #[test]
    fn transition_generation_wraps_without_panicking() {
        assert_eq!(next_generation(41), 42);
        assert_eq!(next_generation(u64::MAX), 0);
    }
}
