//! Send one notification per recognition, after optional artwork has resolved.

use crate::core::thread_messages::SongRecognizedMessage;
use gettextrs::gettext;
use gio::prelude::*;
use std::cell::{Cell, RefCell};
use std::io::Write;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

// A backstop for missing completion messages, slightly longer than the artwork
// service's fetch budget. Normal completions (including failure) send immediately.
const NOTIFICATION_ARTWORK_WAIT: Duration = Duration::from_millis(6_500);

/// Publishes notifications using an icon supported by both GNOME and the
/// Freedesktop backend. The latter only forwards file/themed icons, not BytesIcon.
pub(super) struct DesktopNotificationSender {
    application: glib::WeakRef<gio::Application>,
    enabled: Rc<dyn Fn() -> bool>,
    generation: Rc<Cell<u64>>,
    image: Rc<RefCell<Option<tempfile::NamedTempFile>>>,
}

impl DesktopNotificationSender {
    pub(super) fn new(
        application: &impl IsA<gio::Application>,
        enabled: impl Fn() -> bool + 'static,
    ) -> Self {
        let generation = Rc::new(Cell::new(0u64));
        let image = Rc::new(RefCell::new(None));
        let generation_on_shutdown = generation.clone();
        let image_on_shutdown = image.clone();
        application.connect_shutdown(move |_| {
            generation_on_shutdown.set(generation_on_shutdown.get().wrapping_add(1));
            image_on_shutdown.borrow_mut().take();
        });
        Self {
            application: application.as_ref().downgrade(),
            enabled: Rc::new(enabled),
            generation,
            image,
        }
    }

    pub(super) fn send(&self, track: &SongRecognizedMessage) {
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        if !(self.enabled)() {
            return;
        }
        let notification = gio::Notification::new(&gettext("Song recognized"));
        notification.set_body(Some(&format!(
            "{} - {}",
            track.artist_name, track.song_name
        )));
        let artwork = track.cover_image().cloned();
        let application = self.application.clone();
        let enabled = self.enabled.clone();
        let current_generation = self.generation.clone();
        let retained_image = self.image.clone();
        glib::spawn_future_local(async move {
            let image = if let Some(artwork) = artwork {
                match gio::spawn_blocking(move || -> std::io::Result<tempfile::NamedTempFile> {
                    let mut image = tempfile::Builder::new()
                        .prefix("songrec-notification-")
                        .tempfile()?;
                    // Reuse the encoded response; no full-size PNG re-encoding on GTK's thread.
                    image.write_all(artwork.encoded())?;
                    Ok(image)
                })
                .await
                {
                    Ok(Ok(image)) => Some(image),
                    Ok(Err(error)) => {
                        log::warn!("Unable to prepare notification artwork: {error}");
                        None
                    }
                    Err(_) => {
                        log::warn!("Notification artwork worker panicked");
                        None
                    }
                }
            } else {
                None
            };
            if current_generation.get() != generation || !enabled() {
                return;
            }
            let Some(application) = application.upgrade() else {
                return;
            };
            if let Some(image) = &image {
                notification.set_icon(&gio::FileIcon::new(&gio::File::for_path(image.path())));
            }
            // Keep the file readable while the asynchronous daemon handles Notify.
            // Replacing the single notification also releases its previous image.
            *retained_image.borrow_mut() = image;
            application.send_notification(Some("recognized-song"), &notification);
        });
    }
}

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

    #[test]
    #[ignore = "requires a private D-Bus session with GNOTIFICATION_BACKEND=freedesktop"]
    fn freedesktop_notification_delivers_readable_artwork_once() {
        use std::time::Instant;
        assert_eq!(
            std::env::var("SONGREC_TEST_PRIVATE_BUS").as_deref(),
            Ok("1")
        );
        assert_eq!(
            std::env::var("GNOTIFICATION_BACKEND").as_deref(),
            Ok("freedesktop")
        );
        let received = Rc::new(RefCell::new(Vec::<glib::Variant>::new()));
        glib::MainContext::default().block_on(async {
            let bus = gio::bus_get_future(gio::BusType::Session).await.unwrap();
            let name = bus.call_future(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus",
                "org.freedesktop.DBus", "RequestName",
                Some(&("org.freedesktop.Notifications", 4u32).to_variant()), None,
                gio::DBusCallFlags::NONE, 3000).await.unwrap();
            assert_eq!(name.child_get::<u32>(0), 1, "never replace a real notification daemon");
            let info = gio::DBusNodeInfo::for_xml(r#"<node><interface name="org.freedesktop.Notifications">
                <method name="Notify">
                <arg type="s" direction="in"/><arg type="u" direction="in"/>
                <arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/>
                <arg type="as" direction="in"/><arg type="a{sv}" direction="in"/><arg type="i" direction="in"/>
                <arg type="u" direction="out"/></method></interface></node>"#).unwrap();
            let output = received.clone();
            let registration = bus.register_object("/org/freedesktop/Notifications",
                &info.lookup_interface("org.freedesktop.Notifications").unwrap())
                .method_call(move |_, _, _, _, method, parameters, invocation| {
                    assert_eq!(method, "Notify");
                    output.borrow_mut().push(parameters);
                    invocation.return_value(Some(&(1u32,).to_variant()));
                }).build().unwrap();
            let application = gio::Application::new(Some("re.fossplant.songrec.NotificationTest"),
                gio::ApplicationFlags::NON_UNIQUE);
            application.register(None::<&gio::Cancellable>).unwrap();
            let enabled = Rc::new(Cell::new(true));
            let enabled_for_sender = enabled.clone();
            let sender = DesktopNotificationSender::new(&application, move || enabled_for_sender.get());
            let mut notifications = RecognitionNotifications::new(move |track| sender.send(track));
            notifications.recognized(track("cover", true));
            glib::timeout_future(Duration::from_millis(30)).await;
            assert!(received.borrow().is_empty());
            let mut bytes = std::io::Cursor::new(Vec::new());
            image::RgbaImage::from_pixel(16, 16, image::Rgba([100, 150, 200, 255]))
                .write_to(&mut bytes, image::ImageFormat::Png).unwrap();
            let encoded = bytes.into_inner();
            let artwork = Arc::new(crate::core::artwork::Artwork::decode(encoded.clone()).unwrap());
            let ready = track("cover", false).with_cover_image(artwork);
            notifications.artwork_resolved(&ready);
            let deadline = Instant::now() + Duration::from_secs(3);
            while received.borrow().is_empty() {
                assert!(Instant::now() < deadline, "notification was not delivered");
                glib::timeout_future(Duration::from_millis(10)).await;
            }
            let parameters = received.borrow()[0].clone();
            assert_eq!(parameters.child_get::<String>(4), "Artist - cover");
            let hints = glib::VariantDict::new(Some(&parameters.child_value(6)));
            let path = hints.lookup::<String>("image-path").unwrap().expect("daemon must receive artwork");
            assert_eq!(std::fs::read(&path).unwrap(), encoded, "image remains readable when Notify arrives");
            notifications.artwork_resolved(&ready);
            glib::timeout_future(Duration::from_millis(30)).await;
            assert_eq!(received.borrow().len(), 1, "no duplicate artwork notification");

            // Disable while a new asynchronous icon write is queued.
            notifications.recognized(Arc::new(ready));
            enabled.set(false);
            glib::timeout_future(Duration::from_millis(100)).await;
            assert_eq!(received.borrow().len(), 1);
            enabled.set(true);
            notifications.recognized(track("unavailable", false));
            let deadline = Instant::now() + Duration::from_secs(3);
            while received.borrow().len() < 2 {
                assert!(Instant::now() < deadline);
                glib::timeout_future(Duration::from_millis(10)).await;
            }
            assert!(!std::path::Path::new(&path).exists(), "replacement releases the old temporary icon");
            assert!(glib::VariantDict::new(Some(&received.borrow()[1].child_value(6)))
                .lookup_value("image-path", None).is_none());
            bus.unregister_object(registration).unwrap();
        });
    }

    fn track(key: &str, pending: bool) -> Arc<SongRecognizedMessage> {
        Arc::new(SongRecognizedMessage {
            response_received_at: None,
            track_key: key.into(),
            song_name: key.into(),
            artist_name: "Artist".into(),
            album_name: None,
            record_label: None,
            release_year: None,
            genre: None,
            artist_background_url: None,
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
