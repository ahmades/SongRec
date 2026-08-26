use gettextrs::gettext;
use serde_json::Value;
use soup::prelude::SessionExt;
use std::error::Error;

use crate::core::thread_messages::*;

use crate::core::fingerprinting::communication::{
    obtain_raw_cover_image, recognize_song_from_signature,
};
use crate::core::fingerprinting::signature_format::DecodedSignature;

const PREFERRED_COVER_ART_SIZE_PX: u32 = 1_600;

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
/// if a higher-resolution rendition is unavailable.
async fn obtain_preferred_cover_image(
    session: &soup::Session,
    images: &Value,
) -> Result<Option<Vec<u8>>, Box<dyn Error>> {
    let mut last_error = None;

    for url in preferred_cover_image_urls(images) {
        match obtain_raw_cover_image(session, &url).await {
            Ok(image) => return Ok(Some(image)),
            Err(error) => last_error = Some(error),
        }
    }

    match last_error {
        Some(error) => Err(error),
        None => Ok(None),
    }
}

async fn try_recognize_song(
    session: &soup::Session,
    signature: DecodedSignature,
) -> Result<SongRecognizedMessage, Box<dyn Error>> {
    let json_object = recognize_song_from_signature(session, &signature).await?;

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

    Ok(SongRecognizedMessage {
        artist_name: match &json_object["track"]["subtitle"] {
            Value::String(string) => string.to_string(),
            _ => {
                return Err(Box::new(std::io::Error::other(
                    gettext("No match for this song").as_str(),
                )));
            }
        },
        album_name,
        song_name: match &json_object["track"]["title"] {
            Value::String(string) => string.to_string(),
            _ => {
                return Err(Box::new(std::io::Error::other(
                    gettext("No match for this song").as_str(),
                )));
            }
        },
        cover_image: obtain_preferred_cover_image(session, &json_object["track"]["images"]).await?,
        track_key: match &json_object["track"]["key"] {
            Value::String(string) => string.to_string(),
            _ => {
                return Err(Box::new(std::io::Error::other(
                    gettext("No match for this song").as_str(),
                )));
            }
        },
        release_year,
        genre: match &json_object["track"]["genres"]["primary"] {
            Value::String(string) => Some(string.to_string()),
            _ => None,
        },
        shazam_json: serde_json::to_string(&json_object).unwrap(),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        PREFERRED_COVER_ART_SIZE_PX, preferred_cover_image_urls, upscale_mzstatic_artwork_url,
    };

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
}

pub async fn http_task(
    http_rx: async_channel::Receiver<HTTPMessage>,
    gui_tx: async_channel::Sender<GUIMessage>,
    microphone_tx: async_channel::Sender<MicrophoneMessage>,
) {
    let session = soup::Session::new();
    session.set_timeout(20);
    session.set_idle_timeout(2);

    while let Ok(message) = http_rx.recv().await {
        // XX USE SOUP3 CF. https://github.com/marin-m/SongRec/issues/223
        match message {
            HTTPMessage::RecognizeSignature(signature) => {
                match try_recognize_song(&session, *signature).await {
                    Ok(recognized_song) => {
                        gui_tx
                            .try_send(GUIMessage::SongRecognized(Box::new(recognized_song)))
                            .unwrap();
                        gui_tx.try_send(GUIMessage::NetworkStatus(true)).unwrap();
                        gui_tx.try_send(GUIMessage::RateLimitState(false)).unwrap();
                    }
                    Err(error) => match error.to_string().as_str() {
                        a if a == gettext("No match for this song") => {
                            gui_tx
                                .try_send(GUIMessage::ErrorMessage(error.to_string()))
                                .unwrap();
                            gui_tx.try_send(GUIMessage::NetworkStatus(true)).unwrap();
                            gui_tx.try_send(GUIMessage::RateLimitState(false)).unwrap();
                        }
                        a if a == gettext("Your IP has been rate-limited") => {
                            gui_tx.try_send(GUIMessage::RateLimitState(true)).unwrap();
                        }
                        _ => {
                            log::error!("Network reach error: {:?}", error);
                            gui_tx.try_send(GUIMessage::NetworkStatus(false)).unwrap();
                        }
                    },
                };

                microphone_tx
                    .try_send(MicrophoneMessage::ProcessingDone)
                    .unwrap();
            }
        }
    }
}
