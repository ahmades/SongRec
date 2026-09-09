//! Send one notification per recognition, after optional artwork has resolved.

use crate::core::thread_messages::SongRecognizedMessage;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

// A backstop for missing completion messages, slightly longer than the artwork
// service's fetch budget. Normal completions (including failure) send immediately.
const NOTIFICATION_ARTWORK_WAIT: Duration = Duration::from_millis(6_500);

pub(super) struct RecognitionNotifications {
    pending: Rc<RefCell<Option<Arc<SongRecognizedMessage>>>>,
    timeout: Option<glib::SourceId>,
    send: Rc<dyn Fn(&SongRecognizedMessage)>,
}

impl RecognitionNotifications {
    pub(super) fn new(send: impl Fn(&SongRecognizedMessage) + 'static) -> Self {
        Self {
            pending: Rc::new(RefCell::new(None)),
            timeout: None,
            send: Rc::new(send),
        }
    }

    pub(super) fn recognized(&mut self, track: Arc<SongRecognizedMessage>) {
        self.cancel();
        if !track.artwork_pending() {
            (self.send)(&track);
            return;
        }
        *self.pending.borrow_mut() = Some(track);
        let pending = self.pending.clone();
        let send = self.send.clone();
        self.timeout = Some(glib::timeout_add_local_once(
            NOTIFICATION_ARTWORK_WAIT,
            move || {
                let track = pending.borrow_mut().take();
                if let Some(track) = track {
                    send(&track);
                }
            },
        ));
    }

    pub(super) fn artwork_resolved(&mut self, track: &SongRecognizedMessage) {
        let matches = self
            .pending
            .borrow()
            .as_ref()
            .is_some_and(|pending| pending.track_key == track.track_key);
        if matches && !track.artwork_pending() {
            self.cancel();
            (self.send)(track);
        }
    }

    fn cancel(&mut self) {
        // Once the callback ran, its SourceId is no longer valid. A remaining
        // pending track tells us that the timer is still registered.
        let still_pending = self.pending.borrow_mut().take().is_some();
        if let Some(timeout) = self.timeout.take() {
            if still_pending {
                timeout.remove();
            }
        }
    }
}

impl Drop for RecognitionNotifications {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(key: &str, pending: bool) -> Arc<SongRecognizedMessage> {
        Arc::new(SongRecognizedMessage {
            track_key: key.into(),
            song_name: key.into(),
            artist_name: "Artist".into(),
            album_name: None,
            release_year: None,
            genre: None,
            artwork: if pending {
                crate::core::artwork::ArtworkStatus::Pending
            } else {
                crate::core::artwork::ArtworkStatus::Unavailable
            },
            shazam_json: String::new(),
        })
    }

    #[test]
    fn notifications_wait_for_completion_and_reject_superseded_results() {
        let _serial = crate::MAIN_CONTEXT_TEST_LOCK.lock().unwrap();
        let context = glib::MainContext::default();
        let _guard = context.acquire().unwrap();
        let sent = Rc::new(RefCell::new(Vec::new()));
        let output = sent.clone();
        let mut notifications = RecognitionNotifications::new(move |track| {
            output.borrow_mut().push(track.track_key.clone());
        });
        notifications.recognized(track("a", true));
        notifications.recognized(track("b", true));
        notifications.artwork_resolved(&track("a", false));
        assert!(sent.borrow().is_empty());
        notifications.artwork_resolved(&track("b", false));
        notifications.artwork_resolved(&track("b", false));
        assert_eq!(&*sent.borrow(), &["b"]);
        notifications.recognized(track("cached", false));
        assert_eq!(&*sent.borrow(), &["b", "cached"]);
    }
}
