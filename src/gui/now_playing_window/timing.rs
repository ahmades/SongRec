//! Opt-in response-to-final-frame measurement, not part of rendering decisions.

use super::palette::ArtworkRequirement;
use super::state::ArtistBackgroundState;
use super::{DisplayMode, NowPlayingWindow, TransitionEffect};
use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

#[derive(Debug)]
pub(super) struct FinalArtworkFrame {
    pub(super) track_key: String,
    pub(super) response_received_at: i64,
    after_paint_at: i64,
    pub(super) presented_at: Option<i64>,
    pub(super) width: u32,
    pub(super) height: u32,
    mode: DisplayMode,
    transition: TransitionEffect,
    transition_duration_ms: u64,
}

impl FinalArtworkFrame {
    pub(super) fn elapsed_ms(&self) -> f64 {
        (self.presented_at.unwrap_or(self.after_paint_at) - self.response_received_at) as f64
            / 1000.0
    }

    pub(super) fn log(&self) {
        log::info!(
            "Artwork latency: track={}, JSON -> final {}={:.1} ms, source={}x{}, \
             mode={:?}, transition={:?}, configured duration={} ms",
            self.track_key,
            if self.presented_at.is_some() {
                "presentation"
            } else {
                "after-paint (no presentation feedback)"
            },
            self.elapsed_ms(),
            self.width,
            self.height,
            self.mode,
            self.transition,
            self.transition_duration_ms,
        );
    }
}

#[derive(Default)]
struct Observation {
    mapped_at: Cell<i64>,
    last_response: Cell<Option<i64>>,
}

impl Observation {
    fn claim(&self, response: i64) -> bool {
        // Reopening a hidden window is not a normal song-change sample. Never
        // report the old scene again on resize, motion frames, or a mode change.
        if response < self.mapped_at.get()
            || self
                .last_response
                .get()
                .is_some_and(|last| response <= last)
        {
            return false;
        }
        self.last_response.set(Some(response));
        true
    }
}

type ClockConnection = Rc<RefCell<Option<(gdk::FrameClock, glib::SignalHandlerId)>>>;

fn disconnect_clock(connection: &ClockConnection) {
    if let Some((clock, handler)) = connection.borrow_mut().take() {
        clock.disconnect(handler);
    }
}

/// No per-frame hook exists unless SONGREC_ARTWORK_TIMING=1 (or a test attaches
/// one). Disconnect on unmap and drop; callbacks must not keep a window alive.
pub(super) struct ArtworkTimingProbe {
    window: glib::WeakRef<gtk::Window>,
    map_handler: Option<glib::SignalHandlerId>,
    unmap_handler: Option<glib::SignalHandlerId>,
    connection: ClockConnection,
}

impl ArtworkTimingProbe {
    pub(super) fn attach(
        window: &NowPlayingWindow,
        report: impl Fn(FinalArtworkFrame) + 'static,
    ) -> Self {
        let connection = ClockConnection::default();
        let observation = Rc::new(Observation::default());
        let report = Rc::new(report);
        let track_state = Rc::downgrade(&window.state.track_presentation);
        let settings = window.state.settings.clone();
        let transition = window.ui.content_transition.clone();
        let connect = {
            let connection = connection.clone();
            let observation = observation.clone();
            move |window: &gtk::Window| {
                disconnect_clock(&connection);
                observation.mapped_at.set(glib::monotonic_time());
                let Some(clock) = window.frame_clock() else {
                    return;
                };
                let track_state = track_state.clone();
                let settings = settings.clone();
                let transition = transition.clone();
                let observation = observation.clone();
                let report = report.clone();
                let handler = clock.connect_after_paint(move |clock| {
                    if !transition.is_child_revealed() {
                        return;
                    }
                    let Some(state) = track_state.upgrade() else {
                        return;
                    };
                    let state = state.borrow();
                    let Some(track) = &state.displayed_track else {
                        return;
                    };
                    let Some(response_received_at) = track.response_received_at else {
                        return;
                    };
                    let settings = settings.get();
                    let requirement = ArtworkRequirement::for_settings(settings);
                    if requirement == ArtworkRequirement::NONE || track.awaits_artwork(requirement)
                    {
                        return;
                    }
                    let source = if requirement.needs_artist_background() {
                        match &track.artist_background {
                            ArtistBackgroundState::Ready(background, source)
                                if background.is_ready(requirement) =>
                            {
                                source
                            }
                            // Artist artwork is optional. Once its request is
                            // definitively unavailable (or the payload did not
                            // provide a URL), rendering falls back to the album
                            // backdrop prepared for this exact intensity.
                            _ => match track.artwork.as_ref() {
                                Some(artwork) if artwork.is_ready(requirement) => artwork.source(),
                                _ => return,
                            },
                        }
                    } else {
                        let artwork = match track.artwork.as_ref() {
                            Some(artwork) if artwork.is_ready(requirement) => artwork,
                            _ => return,
                        };
                        artwork.source()
                    };
                    if !observation.claim(response_received_at) {
                        return;
                    }
                    let sample = FinalArtworkFrame {
                        track_key: track.track_key.clone(),
                        response_received_at,
                        after_paint_at: glib::monotonic_time(),
                        presented_at: None,
                        width: source.width(),
                        height: source.height(),
                        mode: settings.display_mode,
                        transition: settings.shared.transition,
                        transition_duration_ms: settings.shared.transition_duration_ms,
                    };
                    let timings = clock.timings(clock.frame_counter());
                    let report = report.clone();
                    let mapped_at = observation.mapped_at.get();
                    let observation = Rc::downgrade(&observation);
                    glib::spawn_future_local(async move {
                        let sample = presentation_feedback(sample, timings).await;
                        // A close/remap or destruction invalidates this observation.
                        if observation
                            .upgrade()
                            .is_some_and(|observation| observation.mapped_at.get() == mapped_at)
                        {
                            report(sample);
                        }
                    });
                });
                connection.borrow_mut().replace((clock, handler));
            }
        };
        if window.ui.window.is_mapped() {
            connect(&window.ui.window);
        }
        let map_handler = window.ui.window.connect_map(connect);
        let unmap_handler = {
            let connection = connection.clone();
            window.ui.window.connect_unmap(move |_| {
                observation.mapped_at.set(i64::MAX);
                disconnect_clock(&connection);
            })
        };
        Self {
            window: window.ui.window.downgrade(),
            map_handler: Some(map_handler),
            unmap_handler: Some(unmap_handler),
            connection,
        }
    }
}

impl Drop for ArtworkTimingProbe {
    fn drop(&mut self) {
        disconnect_clock(&self.connection);
        if let Some(window) = self.window.upgrade() {
            window.disconnect(self.map_handler.take().unwrap());
            window.disconnect(self.unmap_handler.take().unwrap());
        }
    }
}

async fn presentation_feedback(
    mut sample: FinalArtworkFrame,
    timings: Option<gdk::FrameTimings>,
) -> FinalArtworkFrame {
    if let Some(timings) = timings {
        // Feedback may arrive after after-paint. This wait delays only the log,
        // never rendering, and does not request extra frames or use predictions.
        let deadline = sample.after_paint_at + 500_000;
        while !timings.is_complete() && glib::monotonic_time() < deadline {
            glib::timeout_future(Duration::from_millis(10)).await;
        }
        if timings.is_complete() {
            sample.presented_at =
                valid_presentation_time(timings.presentation_time(), sample.response_received_at);
        }
    }
    sample
}

fn valid_presentation_time(presented_at: i64, response: i64) -> Option<i64> {
    (presented_at > 0 && presented_at >= response).then_some(presented_at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_deduplicates_frames_and_excludes_hidden_responses() {
        let observation = Observation::default();
        observation.mapped_at.set(100);
        assert!(!observation.claim(99));
        assert!(observation.claim(101));
        assert!(!observation.claim(101));
        assert!(observation.claim(102));
        assert!(!observation.claim(101));
        observation.mapped_at.set(i64::MAX);
        assert!(!observation.claim(103));
    }

    #[test]
    fn absent_or_invalid_presentation_feedback_is_not_display_latency() {
        assert_eq!(valid_presentation_time(0, 100), None);
        assert_eq!(valid_presentation_time(99, 100), None);
        assert_eq!(valid_presentation_time(110, 100), Some(110));
    }
}
