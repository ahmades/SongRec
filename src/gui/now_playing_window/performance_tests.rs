//! Explicitly opted-in live CDN measurements; never sends Shazam requests.

use super::regression_tests::TestWindow;
use super::timing::{ArtworkTimingProbe, FinalArtworkFrame};
use super::{
    DisplayMode, NowPlayingSettings, NowPlayingWindow, SettingsController, TransitionEffect,
};
use crate::core::artwork::{Artwork, ArtworkStatus};
use crate::core::artwork_service::{ArtworkPolicy, ArtworkService};
use crate::core::http_task::parse_recognition;
use crate::core::thread_messages::{GUIMessage, RecognitionState};
use adw::prelude::*;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

enum Event {
    Message(GUIMessage),
    Frame(FinalArtworkFrame),
}

fn synthetic_artwork() -> Arc<Artwork> {
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(32, 32)
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    Arc::new(Artwork::decode(encoded.into_inner()).unwrap())
}

fn response(key: &str, url: &str) -> Value {
    json!({"track": {
        "key": key, "title": key, "subtitle": "Latency replay",
        "images": {"coverart": url, "coverarthq": url},
        "sections": [{"type": "SONG", "metadata": [
            {"title": "Album", "text": key}, {"title": "Released", "text": "1977"}
        ]}]
    }})
}

async fn display_change(
    window: &NowPlayingWindow,
    events: &async_channel::Receiver<Event>,
    response: i64,
) -> (FinalArtworkFrame, Option<f64>) {
    let mut recognition = RecognitionState::default();
    let mut download_decode_ms = None;
    loop {
        let event = glib::future_with_timeout(Duration::from_secs(12), events.recv())
            .await
            .expect("timed out waiting for artwork's final frame")
            .unwrap();
        match event {
            Event::Message(GUIMessage::SongRecognized(message)) => {
                recognition.record_recognition(message.clone());
                window.update(&message);
            }
            Event::Message(GUIMessage::ArtworkDownloaded { track_key, artwork }) => {
                download_decode_ms = Some((glib::monotonic_time() - response) as f64 / 1000.0);
                assert!(recognition.apply_artwork(&track_key, artwork));
                window.update(&recognition.visible_track(false).unwrap());
            }
            Event::Message(GUIMessage::ArtworkUnavailable { track_key }) => {
                panic!("live artwork unavailable for {track_key}; no high-resolution sample");
            }
            Event::Frame(frame) => {
                assert_eq!(frame.response_received_at, response, "stale frame sample");
                assert!(window.ui.content_transition.is_child_revealed());
                return (frame, download_decode_ms);
            }
            _ => panic!("unexpected replay event"),
        }
    }
}

#[test]
#[ignore = "requires GTK display and SONGREC_TEST_LIVE_ARTWORK=1; downloads public Apple CDN artwork"]
fn live_response_to_final_artwork_frame() {
    assert_eq!(
        std::env::var("SONGREC_TEST_LIVE_ARTWORK").as_deref(),
        Ok("1")
    );
    crate::core::logging::Logging::setup_logging(log::LevelFilter::Warn, log::LevelFilter::Debug);
    adw::init().unwrap();
    gio::resources_register_include!("compiled.gresource").unwrap();
    gtk::Settings::default()
        .unwrap()
        .set_gtk_enable_animations(true);
    let albums = [
        (
            "Rumours",
            "https://is1-ssl.mzstatic.com/image/thumb/Music124/v4/4d/13/ba/4d13bac3-d3d5-7581-2c74-034219eadf2b/081227970949.jpg/400x400bb.jpg",
        ),
        (
            "The Wall",
            "https://is1-ssl.mzstatic.com/image/thumb/Music221/v4/3e/17/ec/3e17ec6d-f980-c64f-19e0-a6fd8bbf0c10/886445635850.jpg/400x400bb.jpg",
        ),
    ];
    glib::MainContext::default().block_on(async {
        for mode in [DisplayMode::Classic, DisplayMode::Cinema, DisplayMode::Ambient] {
            let mut settings = NowPlayingSettings { display_mode: mode, ..NowPlayingSettings::default() };
            settings.shared.transition_duration_ms = 2000;
            let controller = SettingsController::new(settings, None);
            let window = TestWindow(NowPlayingWindow::new_with_controller(controller));
            let (tx, rx) = async_channel::unbounded();
            let frame_tx = tx.clone();
            let probe = ArtworkTimingProbe::attach(&window.0, move |frame| {
                frame_tx.try_send(Event::Frame(frame)).unwrap();
            });
            window.0.ui.window.fullscreen();
            window.0.present();
            glib::timeout_future(Duration::from_millis(250)).await;
            assert!(window.0.ui.window.is_mapped());
            eprintln!("FRAME ENV: backend={}, renderer={}, canvas={}x{}",
                gdk::Display::default().unwrap().type_().name(),
                window.0.ui.window.renderer().unwrap().type_().name(),
                window.0.ui.background_area.width(), window.0.ui.background_area.height());

            // Start with an already displayed song, not an empty/listening window.
            let mut seed = parse_recognition(response("seed", "")).unwrap().message;
            seed.artwork = ArtworkStatus::Ready(synthetic_artwork());
            let start = seed.response_received_at.unwrap();
            tx.try_send(Event::Message(GUIMessage::SongRecognized(Arc::new(seed)))).unwrap();
            display_change(&window.0, &rx, start).await;

            // A new service and window per mode make the first two requests
            // application-cache cold. This cannot promise a cold CDN cache.
            let service = ArtworkService::new(ArtworkPolicy::Display);
            for (index, effect) in [
                TransitionEffect::None, TransitionEffect::Crossfade,
                TransitionEffect::None, TransitionEffect::None,
                TransitionEffect::Crossfade, TransitionEffect::Crossfade,
                TransitionEffect::SlideLeft, TransitionEffect::SlideLeft,
            ].into_iter().enumerate() {
                settings.shared.transition = effect;
                window.0.controller.settings_cell().set(settings);
                window.0.refresh_from_controller();
                let (album, url) = albums[index % albums.len()];
                let mut parsed = parse_recognition(response(album, url)).unwrap();
                let start = parsed.message.response_received_at.unwrap();
                let artwork_tx = tx.clone();
                parsed.message.artwork = service.request(&parsed.message.track_key, &parsed.images, move |track_key, artwork| {
                    let message = match artwork {
                        Some(artwork) => GUIMessage::ArtworkDownloaded { track_key, artwork },
                        None => GUIMessage::ArtworkUnavailable { track_key },
                    };
                    artwork_tx.try_send(Event::Message(message)).unwrap();
                });
                let cached = parsed.message.cover_image().is_some();
                assert_eq!(cached, index >= 2);
                tx.try_send(Event::Message(GUIMessage::SongRecognized(Arc::new(parsed.message)))).unwrap();
                let (frame, download_decode_ms) = display_change(&window.0, &rx, start).await;
                frame.log();
                eprintln!("REPLAY: mode={mode:?}, effect={effect:?}, album={album}, cached={cached}, download+decode+dispatch_ms={download_decode_ms:?}, final_ms={:.1}", frame.elapsed_ms());
                // Apple can cap a requested 1600px rendition at the source's
                // native size (The Wall is 1414px). Reject thumbnail fallbacks,
                // but report the actual high-resolution dimensions, not the URL.
                assert!(frame.width >= 1000 && frame.height >= 1000, "fallback resolution is not a high-resolution sample");
            }
            drop(probe);
        }
    });
}

#[test]
#[ignore = "requires a GTK display; no network requests"]
fn artwork_timing_waits_for_ready_scene_and_cleans_up() {
    use std::rc::Rc;
    adw::init().unwrap();
    gio::resources_register_include!("compiled.gresource").unwrap();
    gtk::Settings::default()
        .unwrap()
        .set_gtk_enable_animations(true);
    glib::MainContext::default().block_on(async {
        let mut settings = NowPlayingSettings::default();
        settings.shared.transition_duration_ms = 200;
        let window = TestWindow(NowPlayingWindow::new_with_controller(
            SettingsController::new(settings, None),
        ));
        let (tx, rx) = async_channel::unbounded();
        let reporter = Rc::new(tx.clone());
        let weak_reporter = Rc::downgrade(&reporter);
        let probe = ArtworkTimingProbe::attach(&window.0, move |frame| {
            reporter.try_send(Event::Frame(frame)).unwrap();
        });
        window.0.present();
        glib::timeout_future(Duration::from_millis(150)).await;
        let artwork = synthetic_artwork();
        for (index, effect) in [
            TransitionEffect::None,
            TransitionEffect::Crossfade,
            TransitionEffect::SlideLeft,
        ]
        .into_iter()
        .enumerate()
        {
            settings.shared.transition = effect;
            window.0.controller.settings_cell().set(settings);
            window.0.refresh_from_controller();
            let mut message = parse_recognition(response(&index.to_string(), ""))
                .unwrap()
                .message;
            message.artwork = ArtworkStatus::Pending;
            let start = message.response_received_at.unwrap();
            let key = message.track_key.clone();
            tx.try_send(Event::Message(GUIMessage::SongRecognized(Arc::new(
                message,
            ))))
            .unwrap();
            let delayed_tx = tx.clone();
            let artwork = artwork.clone();
            glib::spawn_future_local(async move {
                glib::timeout_future(Duration::from_millis(150)).await;
                delayed_tx
                    .try_send(Event::Message(GUIMessage::ArtworkDownloaded {
                        track_key: key,
                        artwork,
                    }))
                    .unwrap();
            });
            let (frame, downloaded_ms) = display_change(&window.0, &rx, start).await;
            assert!(frame.elapsed_ms() >= downloaded_ms.unwrap());
            if effect != TransitionEffect::None {
                // Allow frame-clock rounding, but never accept the midpoint as
                // the fully revealed frame. This is not a speed threshold.
                assert!(frame.elapsed_ms() >= downloaded_ms.unwrap() + 150.0);
            }
            assert_eq!((frame.width, frame.height), (32, 32));
        }
        window.0.ui.window.queue_draw();
        glib::timeout_future(Duration::from_millis(550)).await;
        assert!(rx.is_empty(), "repaint must not repeat a timing sample");
        window.0.ui.window.set_visible(false);
        window.0.present();
        glib::timeout_future(Duration::from_millis(550)).await;
        assert!(rx.is_empty(), "remap must not report an old response");
        drop(probe);
        assert!(
            weak_reporter.upgrade().is_none(),
            "probe handlers retained their callback after drop"
        );
        window.0.ui.window.queue_draw();
        glib::timeout_future(Duration::from_millis(100)).await;
        assert!(rx.is_empty());
    });
}
