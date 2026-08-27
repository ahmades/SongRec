use serde_json::Value;
use soup::prelude::SessionExt;
use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::error::Error;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use crate::core::artwork::Artwork;
use crate::core::thread_messages::*;

use crate::core::fingerprinting::communication::{
    RateLimitError, obtain_raw_cover_image, recognize_song_from_signature,
};
use crate::core::fingerprinting::signature_format::DecodedSignature;

const PREFERRED_COVER_ART_SIZE_PX: u32 = 1_600;
const ARTWORK_REQUEST_TIMEOUT_SECS: u32 = 4;
const ARTWORK_FETCH_BUDGET: Duration = Duration::from_secs(6);
const COVER_IMAGE_CACHE_CAPACITY: usize = 8;
const COVER_IMAGE_CACHE_MAX_BYTES: usize = 96 * 1024 * 1024;

/// A small process-local LRU cache avoids downloading the same artwork on each
/// recognition interval while keeping the memory bound predictable.
#[derive(Default)]
struct CoverImageCache {
    entries: VecDeque<(String, Arc<Artwork>)>,
    retained_bytes: usize,
}

impl CoverImageCache {
    fn get(&mut self, track_key: &str) -> Option<Arc<Artwork>> {
        let position = self
            .entries
            .iter()
            .position(|(cached_track_key, _)| cached_track_key == track_key)?;
        let entry = self.entries.remove(position)?;
        let image = Arc::clone(&entry.1);
        self.entries.push_back(entry);
        Some(image)
    }

    fn insert(&mut self, track_key: String, image: Arc<Artwork>) {
        if let Some(position) = self
            .entries
            .iter()
            .position(|(cached_track_key, _)| cached_track_key == &track_key)
        {
            if let Some((_, previous)) = self.entries.remove(position) {
                self.retained_bytes = self.retained_bytes.saturating_sub(previous.storage_bytes());
            }
        }

        let image_bytes = image.storage_bytes();
        if image_bytes > COVER_IMAGE_CACHE_MAX_BYTES {
            return;
        }
        while self.entries.len() >= COVER_IMAGE_CACHE_CAPACITY
            || self.retained_bytes.saturating_add(image_bytes) > COVER_IMAGE_CACHE_MAX_BYTES
        {
            let Some((_, evicted)) = self.entries.pop_front() else {
                break;
            };
            self.retained_bytes = self.retained_bytes.saturating_sub(evicted.storage_bytes());
        }

        self.retained_bytes = self.retained_bytes.saturating_add(image_bytes);
        self.entries.push_back((track_key, image));
    }
}

#[derive(Default)]
struct ArtworkRequestState {
    cache: CoverImageCache,
    in_flight: HashSet<String>,
}

struct ParsedRecognition {
    message: SongRecognizedMessage,
    artwork_urls: Vec<String>,
}

/// Returns a higher-resolution rendition URL for an Apple CDN artwork image.
///
/// Shazam returns concrete Apple Music CDN URLs such as
/// `.../400x400bb.jpg`. Only rewrite the final rendition component of the
/// recognized `*.mzstatic.com/image/thumb/` form; other URL shapes are
/// returned as `None` so callers can use the original URL unchanged.
fn upscale_mzstatic_artwork_url(url: &str, target_size: u32) -> Option<String> {
    if target_size == 0 || !is_mzstatic_artwork_url(url) {
        return None;
    }

    let path_end = url
        .find(|character| matches!(character, '?' | '#'))
        .unwrap_or(url.len());
    let (path, query_or_fragment) = url.split_at(path_end);
    let (prefix, rendition) = path.rsplit_once('/')?;
    let (width, height_and_suffix) = rendition.split_once('x')?;
    if width.is_empty() || !width.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }

    let height_length = height_and_suffix
        .bytes()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    let (height, suffix) = height_and_suffix.split_at(height_length);
    let width = width.parse::<u32>().ok()?;
    let height = height.parse::<u32>().ok()?;
    if height == 0
        || !matches!(suffix.get(..2), Some("bb") | Some("cc"))
        || width != height
        || width == target_size
    {
        return None;
    }

    Some(format!(
        "{prefix}/{target_size}x{target_size}{suffix}{query_or_fragment}"
    ))
}

fn is_mzstatic_artwork_url(url: &str) -> bool {
    let Some(url_without_scheme) = url.strip_prefix("https://") else {
        return false;
    };
    let Some((host, path)) = url_without_scheme.split_once('/') else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    (host == "mzstatic.com" || host.ends_with(".mzstatic.com")) && path.starts_with("image/thumb/")
}

/// Returns artwork URLs in preference order, including a guarded high-resolution
/// Apple CDN rendition before each response-provided URL.
fn preferred_cover_image_urls(images: &Value) -> Vec<String> {
    let mut urls = Vec::with_capacity(4);

    for url in [
        images.get("coverarthq").and_then(Value::as_str),
        images.get("coverart").and_then(Value::as_str),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(upscaled_url) = upscale_mzstatic_artwork_url(url, PREFERRED_COVER_ART_SIZE_PX)
            && upscaled_url.as_str() != url
            && !urls.contains(&upscaled_url)
        {
            urls.push(upscaled_url);
        }
        if !urls.iter().any(|candidate| candidate == url) {
            urls.push(url.to_string());
        }
    }

    urls
}

/// Downloads the best available cover, falling back through the original URLs
/// if a higher-resolution rendition is unavailable. Artwork is optional, so
/// candidate failures are logged and do not escape into recognition handling.
async fn decode_artwork(bytes: Vec<u8>) -> Option<Arc<Artwork>> {
    #[cfg(feature = "gui")]
    {
        return gio::spawn_blocking(move || Artwork::decode(bytes).map(Arc::new))
            .await
            .map_err(|_| log::warn!("Artwork decoding task panicked"))
            .ok()
            .flatten();
    }

    #[cfg(not(feature = "gui"))]
    Artwork::decode(bytes).map(Arc::new)
}

async fn obtain_preferred_cover_image(
    session: &soup::Session,
    urls: Vec<String>,
) -> Option<Arc<Artwork>> {
    for url in urls {
        match obtain_raw_cover_image(session, &url).await {
            Ok(bytes) => {
                if let Some(artwork) = decode_artwork(bytes).await {
                    return Some(artwork);
                }
                log::debug!("Artwork candidate {url} contained invalid image data");
            }
            Err(error) => log::debug!("Artwork candidate {url} was unavailable: {error}"),
        }
    }

    None
}

async fn fetch_artwork(session: &soup::Session, urls: Vec<String>) -> Option<Arc<Artwork>> {
    match glib::future_with_timeout(
        ARTWORK_FETCH_BUDGET,
        obtain_preferred_cover_image(session, urls),
    )
    .await
    {
        Ok(artwork) => artwork,
        Err(error) => {
            log::debug!("Artwork fetch exceeded its time budget: {error}");
            None
        }
    }
}

async fn try_recognize_song(
    recognition_session: &soup::Session,
    signature: DecodedSignature,
) -> Result<Option<ParsedRecognition>, Box<dyn Error>> {
    let json_object = recognize_song_from_signature(recognition_session, &signature).await?;

    let mut album_name: Option<String> = None;
    let mut release_year: Option<String> = None;

    // Sometimes the idea of trying to write functional poetry hurts

    if let Value::Array(sections) = &json_object["track"]["sections"] {
        for section in sections {
            if let Value::String(string) = &section["type"]
                && string == "SONG"
            {
                if let Value::Array(metadata) = &section["metadata"] {
                    for metadatum in metadata {
                        if let Value::String(title) = &metadatum["title"] {
                            if title == "Album"
                                && let Value::String(text) = &metadatum["text"]
                            {
                                album_name = Some(text.to_string());
                            } else if title == "Released"
                                && let Value::String(text) = &metadatum["text"]
                            {
                                release_year = Some(text.to_string());
                            }
                        }
                    }
                    break;
                }
            }
        }
    }

    let required_track_field =
        |field: &str| json_object["track"][field].as_str().map(str::to_owned);
    let Some(artist_name) = required_track_field("subtitle") else {
        return Ok(None);
    };
    let Some(song_name) = required_track_field("title") else {
        return Ok(None);
    };
    let Some(track_key) = required_track_field("key") else {
        return Ok(None);
    };
    let artwork_urls = preferred_cover_image_urls(&json_object["track"]["images"]);
    let artwork_pending = !artwork_urls.is_empty();
    Ok(Some(ParsedRecognition {
        message: SongRecognizedMessage {
            artist_name,
            album_name,
            song_name,
            cover_image: None,
            artwork_pending,
            track_key,
            release_year,
            genre: match &json_object["track"]["genres"]["primary"] {
                Value::String(string) => Some(string.to_string()),
                _ => None,
            },
            shazam_json: serde_json::to_string(&json_object).unwrap(),
        },
        artwork_urls,
    }))
}

#[cfg(test)]
mod tests {
    use image::{DynamicImage, ImageFormat};
    use serde_json::json;
    use std::io::Cursor;
    use std::sync::Arc;

    use super::{
        COVER_IMAGE_CACHE_CAPACITY, COVER_IMAGE_CACHE_MAX_BYTES, CoverImageCache,
        PREFERRED_COVER_ART_SIZE_PX, preferred_cover_image_urls, upscale_mzstatic_artwork_url,
    };
    use crate::core::artwork::Artwork;

    fn artwork(marker: u8) -> Arc<Artwork> {
        let mut image = DynamicImage::new_rgba8(1, 1).to_rgba8();
        image.get_pixel_mut(0, 0).0 = [marker, marker, marker, 255];
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut encoded, ImageFormat::Png)
            .unwrap();
        Arc::new(Artwork::decode(encoded.into_inner()).unwrap())
    }

    #[test]
    fn prefers_upscaled_hq_artwork_then_response_urls() {
        let hq = "https://is1-ssl.mzstatic.com/image/thumb/Music/a/b/c/400x400bb.jpg";
        let artwork = "https://is2-ssl.mzstatic.com/image/thumb/Music/d/e/f/400x400bb.jpg";
        let images = json!({
            "coverarthq": hq,
            "coverart": artwork,
        });

        assert_eq!(
            preferred_cover_image_urls(&images),
            vec![
                format!(
                    "https://is1-ssl.mzstatic.com/image/thumb/Music/a/b/c/{0}x{0}bb.jpg",
                    PREFERRED_COVER_ART_SIZE_PX
                ),
                hq.to_string(),
                format!(
                    "https://is2-ssl.mzstatic.com/image/thumb/Music/d/e/f/{0}x{0}bb.jpg",
                    PREFERRED_COVER_ART_SIZE_PX
                ),
                artwork.to_string(),
            ]
        );
    }

    #[test]
    fn does_not_retry_identical_artwork_urls() {
        let artwork = "https://is1-ssl.mzstatic.com/image/thumb/Music/a/b/c/400x400bb.jpg";
        let images = json!({
            "coverarthq": artwork,
            "coverart": artwork,
        });

        assert_eq!(preferred_cover_image_urls(&images).len(), 2);
    }

    #[test]
    fn upscales_the_final_mzstatic_rendition() {
        let source = "https://is1-ssl.mzstatic.com/image/thumb/Music/a/b/c/cover.jpg/400x400bb.jpg";

        assert_eq!(
            upscale_mzstatic_artwork_url(source, 1_600).as_deref(),
            Some("https://is1-ssl.mzstatic.com/image/thumb/Music/a/b/c/cover.jpg/1600x1600bb.jpg")
        );
    }

    #[test]
    fn preserves_the_rendition_suffix_query_and_fragment() {
        let source =
            "https://is1-ssl.mzstatic.com/image/thumb/Music/a/b/c/400x400bb-60.jpg?foo=bar#section";

        assert_eq!(
            upscale_mzstatic_artwork_url(source, 1_600).as_deref(),
            Some(
                "https://is1-ssl.mzstatic.com/image/thumb/Music/a/b/c/1600x1600bb-60.jpg?foo=bar#section"
            )
        );
    }

    #[test]
    fn caps_larger_renditions_at_the_requested_size() {
        let source = "https://is1-ssl.mzstatic.com/image/thumb/Music/a/b/c/3000x3000bb.jpg";

        assert_eq!(
            upscale_mzstatic_artwork_url(source, 1_600).as_deref(),
            Some("https://is1-ssl.mzstatic.com/image/thumb/Music/a/b/c/1600x1600bb.jpg")
        );
    }

    #[test]
    fn leaves_unsupported_or_already_target_urls_alone() {
        assert_eq!(
            upscale_mzstatic_artwork_url(
                "https://is1-ssl.mzstatic.com/image/thumb/Music/a/b/c/1600x1600bb.jpg",
                1_600,
            ),
            None
        );
        assert_eq!(
            upscale_mzstatic_artwork_url(
                "https://example.com/image/thumb/Music/a/b/c/400x400bb.jpg",
                1_600,
            ),
            None
        );
        assert_eq!(
            upscale_mzstatic_artwork_url(
                "https://is1-ssl.mzstatic.com/image/thumb/Music/a/b/c/400x300bb.jpg",
                1_600,
            ),
            None
        );
    }

    #[test]
    fn cover_cache_is_bounded_and_recently_used_entries_survive() {
        let mut cache = CoverImageCache::default();
        for index in 0..COVER_IMAGE_CACHE_CAPACITY {
            cache.insert(format!("track-{index}"), artwork(index as u8));
        }

        assert!(cache.get("track-0").is_some());
        cache.insert("new-track".to_string(), artwork(255));

        assert!(cache.get("track-1").is_none());
        assert!(cache.get("track-0").is_some());
        assert!(cache.get("new-track").is_some());
        assert_eq!(cache.entries.len(), COVER_IMAGE_CACHE_CAPACITY);
        assert!(cache.retained_bytes <= COVER_IMAGE_CACHE_MAX_BYTES);
    }
}

pub async fn http_task(
    http_rx: async_channel::Receiver<HTTPMessage>,
    gui_tx: async_channel::Sender<GUIMessage>,
    microphone_tx: async_channel::Sender<MicrophoneMessage>,
) {
    let recognition_session = soup::Session::new();
    recognition_session.set_timeout(20);
    recognition_session.set_idle_timeout(2);

    let artwork_session = soup::Session::new();
    artwork_session.set_timeout(ARTWORK_REQUEST_TIMEOUT_SECS);
    artwork_session.set_idle_timeout(2);
    let artwork_state = Rc::new(RefCell::new(ArtworkRequestState::default()));

    while let Ok(message) = http_rx.recv().await {
        // XX USE SOUP3 CF. https://github.com/marin-m/SongRec/issues/223
        match message {
            HTTPMessage::RecognizeSignature(signature) => {
                match try_recognize_song(&recognition_session, *signature).await {
                    Ok(Some(parsed)) => {
                        let track_key = parsed.message.track_key.clone();
                        let cached_artwork = artwork_state.borrow_mut().cache.get(&track_key);
                        let recognized_song = Arc::new(match cached_artwork {
                            Some(artwork) => parsed.message.with_cover_image(artwork),
                            None => parsed.message,
                        });
                        let should_fetch_artwork = recognized_song.artwork_pending;
                        gui_tx
                            .try_send(GUIMessage::SongRecognized(recognized_song))
                            .unwrap();
                        gui_tx.try_send(GUIMessage::NetworkStatus(true)).unwrap();
                        gui_tx.try_send(GUIMessage::RateLimitState(false)).unwrap();

                        if should_fetch_artwork
                            && artwork_state
                                .borrow_mut()
                                .in_flight
                                .insert(track_key.clone())
                        {
                            let artwork_session = artwork_session.clone();
                            let artwork_state = artwork_state.clone();
                            let gui_tx = gui_tx.clone();
                            glib::spawn_future_local(async move {
                                let artwork =
                                    fetch_artwork(&artwork_session, parsed.artwork_urls).await;
                                artwork_state.borrow_mut().in_flight.remove(&track_key);

                                if let Some(artwork) = artwork {
                                    artwork_state
                                        .borrow_mut()
                                        .cache
                                        .insert(track_key.clone(), artwork.clone());
                                    if let Err(error) =
                                        gui_tx.try_send(GUIMessage::ArtworkDownloaded {
                                            track_key,
                                            artwork,
                                        })
                                    {
                                        log::debug!(
                                            "Unable to deliver downloaded artwork: {error}"
                                        );
                                    }
                                } else if let Err(error) =
                                    gui_tx.try_send(GUIMessage::ArtworkUnavailable { track_key })
                                {
                                    log::debug!("Unable to report unavailable artwork: {error}");
                                }
                            });
                        }
                    }
                    Ok(None) => {
                        gui_tx.try_send(GUIMessage::NoRecognition).unwrap();
                        gui_tx.try_send(GUIMessage::NetworkStatus(true)).unwrap();
                        gui_tx.try_send(GUIMessage::RateLimitState(false)).unwrap();
                    }
                    Err(error) => {
                        if error.downcast_ref::<RateLimitError>().is_some() {
                            gui_tx.try_send(GUIMessage::RateLimitState(true)).unwrap();
                        } else {
                            log::error!("Network reach error: {:?}", error);
                            gui_tx.try_send(GUIMessage::NetworkStatus(false)).unwrap();
                        }
                    }
                };

                microphone_tx
                    .try_send(MicrophoneMessage::ProcessingDone)
                    .unwrap();
            }
        }
    }
}
