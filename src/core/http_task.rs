use serde_json::Value;
use soup::prelude::SessionExt;
use std::error::Error;
use std::sync::Arc;

use crate::core::artwork_service::{ArtworkPolicy, ArtworkService};
use crate::core::fingerprinting::communication::{RateLimitError, recognize_song_from_signature};
use crate::core::fingerprinting::signature_format::DecodedSignature;
use crate::core::thread_messages::*;

pub(crate) struct ParsedRecognition {
    pub(crate) message: SongRecognizedMessage,
    pub(crate) images: Value,
}

async fn try_recognize_song(
    recognition_session: &soup::Session,
    signature: DecodedSignature,
) -> Result<Option<ParsedRecognition>, Box<dyn Error>> {
    let json_object = recognize_song_from_signature(recognition_session, &signature).await?;
    Ok(parse_recognition(json_object))
}

/// The post-response entry point, also used to replay artwork latency without
/// making recognition requests. Keep the timestamp before metadata extraction.
pub(crate) fn parse_recognition(json_object: Value) -> Option<ParsedRecognition> {
    let response_received_at = Some(glib::monotonic_time());

    let mut album_name: Option<String> = None;
    let mut record_label: Option<String> = None;
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
                            } else if title == "Label"
                                && let Value::String(text) = &metadatum["text"]
                            {
                                record_label = Some(text.to_string());
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
        return None;
    };
    let Some(song_name) = required_track_field("title") else {
        return None;
    };
    let Some(track_key) = required_track_field("key") else {
        return None;
    };

    Some(ParsedRecognition {
        message: SongRecognizedMessage {
            response_received_at,
            artist_name,
            album_name,
            record_label,
            song_name,
            artist_background_url: json_object["track"]["images"]["background"]
                .as_str()
                .filter(|url| !url.is_empty())
                .map(str::to_owned),
            artwork: crate::core::artwork::ArtworkStatus::Unavailable,
            track_key,
            release_year,
            genre: match &json_object["track"]["genres"]["primary"] {
                Value::String(string) => Some(string.to_string()),
                _ => None,
            },
            shazam_json: serde_json::to_string(&json_object).unwrap(),
        },
        images: json_object["track"]["images"].clone(),
    })
}

pub async fn http_task(
    http_rx: async_channel::Receiver<HTTPMessage>,
    gui_tx: async_channel::Sender<GUIMessage>,
    microphone_tx: async_channel::Sender<MicrophoneMessage>,
    artwork_policy: ArtworkPolicy,
) {
    let recognition_session = soup::Session::new();
    recognition_session.set_timeout(20);
    recognition_session.set_idle_timeout(2);
    let artwork_service = ArtworkService::new(artwork_policy);

    while let Ok(message) = http_rx.recv().await {
        match message {
            HTTPMessage::RecognizeSignature(signature) => {
                match try_recognize_song(&recognition_session, *signature).await {
                    Ok(Some(mut parsed)) => {
                        let result_tx = gui_tx.clone();
                        let artwork = artwork_service.request(
                            &parsed.message.track_key,
                            &parsed.images,
                            move |track_key, artwork| {
                                let message = match artwork {
                                    Some(artwork) => {
                                        GUIMessage::ArtworkDownloaded { track_key, artwork }
                                    }
                                    None => GUIMessage::ArtworkUnavailable { track_key },
                                };
                                if let Err(error) = result_tx.try_send(message) {
                                    log::debug!("Unable to deliver artwork result: {error}");
                                }
                            },
                        );
                        parsed.message.artwork = artwork;
                        gui_tx
                            .try_send(GUIMessage::SongRecognized(Arc::new(parsed.message)))
                            .unwrap();
                        gui_tx.try_send(GUIMessage::NetworkStatus(true)).unwrap();
                        gui_tx.try_send(GUIMessage::RateLimitState(false)).unwrap();
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

#[cfg(test)]
mod tests {
    use super::parse_recognition;
    use serde_json::json;

    #[test]
    fn post_response_parser_preserves_metadata_and_arrival_time() {
        let artist_background =
            "https://is1-ssl.mzstatic.com/image/thumb/Features/a/b/c/800x800cc.jpg?x=1#image";
        let response = json!({"track": {
            "key": "123", "title": "Song", "subtitle": "Artist",
            "images": {"coverarthq": "cover.jpg", "background": artist_background},
            "genres": {"primary": "Rock"},
            "sections": [{"type": "SONG", "metadata": [
                {"title": "Album", "text": "Album"},
                {"title": "Label", "text": "Harvest Records"},
                {"title": "Released", "text": "1977"}
            ]}]
        }});
        let before = glib::monotonic_time();
        let parsed = parse_recognition(response.clone()).unwrap();
        let received = parsed.message.response_received_at.unwrap();
        assert!((before..=glib::monotonic_time()).contains(&received));
        assert_eq!(parsed.message.album_name.as_deref(), Some("Album"));
        assert_eq!(
            parsed.message.record_label.as_deref(),
            Some("Harvest Records")
        );
        assert_eq!(parsed.message.release_year.as_deref(), Some("1977"));
        assert_eq!(parsed.message.genre.as_deref(), Some("Rock"));
        assert_eq!(
            parsed.message.artist_background_url.as_deref(),
            Some(artist_background)
        );
        assert_eq!(parsed.images, response["track"]["images"]);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&parsed.message.shazam_json).unwrap(),
            response
        );
        assert_eq!(
            parsed
                .message
                .with_artwork_unavailable()
                .response_received_at,
            Some(received)
        );
    }

    #[test]
    fn post_response_parser_rejects_missing_required_track_fields() {
        assert!(parse_recognition(json!({})).is_none());
        for field in ["key", "title", "subtitle"] {
            let mut response =
                json!({"track": {"key": "123", "title": "Song", "subtitle": "Artist"}});
            response["track"].as_object_mut().unwrap().remove(field);
            assert!(parse_recognition(response).is_none());
        }
    }

    #[test]
    fn post_response_parser_treats_missing_invalid_or_empty_artist_background_as_unavailable() {
        let response = |background: serde_json::Value| {
            json!({"track": {
                "key": "123", "title": "Song", "subtitle": "Artist",
                "images": {"background": background}
            }})
        };

        assert!(
            parse_recognition(json!({"track": {
                "key": "123", "title": "Song", "subtitle": "Artist"
            }}))
            .unwrap()
            .message
            .artist_background_url
            .is_none()
        );
        for background in [json!(null), json!(123), json!("")] {
            assert!(
                parse_recognition(response(background))
                    .unwrap()
                    .message
                    .artist_background_url
                    .is_none()
            );
        }
    }

    #[test]
    fn post_response_parser_treats_missing_or_invalid_record_label_as_unavailable() {
        let response = |metadata: serde_json::Value| {
            json!({"track": {
                "key": "123", "title": "Song", "subtitle": "Artist",
                "sections": [{"type": "SONG", "metadata": metadata}]
            }})
        };

        for metadata in [
            json!([]),
            json!([{"title": "Label", "text": null}]),
            json!([{"title": "Label", "text": 123}]),
        ] {
            assert!(
                parse_recognition(response(metadata))
                    .unwrap()
                    .message
                    .record_label
                    .is_none()
            );
        }
    }
}
