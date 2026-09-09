use serde_json::Value;
use soup::prelude::SessionExt;
use std::error::Error;
use std::sync::Arc;

use crate::core::artwork_service::{ArtworkPolicy, ArtworkService};
use crate::core::fingerprinting::communication::{RateLimitError, recognize_song_from_signature};
use crate::core::fingerprinting::signature_format::DecodedSignature;
use crate::core::thread_messages::*;

struct ParsedRecognition {
    message: SongRecognizedMessage,
    images: Value,
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

    Ok(Some(ParsedRecognition {
        message: SongRecognizedMessage {
            artist_name,
            album_name,
            song_name,
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
    }))
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
