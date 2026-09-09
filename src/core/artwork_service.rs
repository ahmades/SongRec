//! Consumer-aware artwork acquisition, bounded fallback, and shared decoded-image caching.

use super::artwork::{Artwork, ArtworkStatus};
use super::fingerprinting::communication::obtain_raw_cover_image;
use serde_json::Value;
use soup::prelude::SessionExt;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque, hash_map::Entry};
use std::future::Future;
use std::rc::Rc;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

const PREFERRED_COVER_ART_SIZE_PX: u32 = 1_600;
const ARTWORK_REQUEST_TIMEOUT_SECS: u32 = 4;
const ARTWORK_FETCH_BUDGET: Duration = Duration::from_secs(6);
const PREFERRED_CANDIDATE_BUDGET: Duration = Duration::from_secs(2);
const ORIGINAL_CANDIDATE_RESERVE: Duration = Duration::from_secs(1);
const COVER_IMAGE_CACHE_CAPACITY: usize = 8;
const COVER_IMAGE_CACHE_MAX_BYTES: usize = 96 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtworkPolicy {
    None,
    /// Use the response-provided rendition for small images and encoded-artwork consumers.
    Thumbnail,
    /// Prefer a display-sized rendition, with time reserved for the original image.
    Display,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    url: String,
    original: bool,
}

#[derive(Default)]
struct CoverImageCache {
    entries: VecDeque<(String, Arc<Artwork>)>,
}

impl CoverImageCache {
    fn get(&mut self, key: &str) -> Option<Arc<Artwork>> {
        let position = self.entries.iter().position(|(cached, _)| cached == key)?;
        let entry = self.entries.remove(position)?;
        let image = entry.1.clone();
        self.entries.push_back(entry);
        Some(image)
    }

    fn intern(&self, image: Arc<Artwork>) -> Arc<Artwork> {
        self.entries
            .iter()
            .find(|(_, cached)| cached.same_content(&image))
            .map(|(_, cached)| cached.clone())
            .unwrap_or(image)
    }

    fn storage_bytes(&self) -> usize {
        let mut sources = HashSet::new();
        self.entries
            .iter()
            .filter(|(_, image)| sources.insert(Arc::as_ptr(image)))
            .map(|(_, image)| image.storage_bytes())
            .sum()
    }

    fn insert(&mut self, key: String, image: Arc<Artwork>) {
        self.entries.retain(|(cached, _)| cached != &key);
        if image.storage_bytes() > COVER_IMAGE_CACHE_MAX_BYTES {
            return;
        }
        self.entries.push_back((key, image));
        while self.entries.len() > COVER_IMAGE_CACHE_CAPACITY
            || self.storage_bytes() > COVER_IMAGE_CACHE_MAX_BYTES
        {
            self.entries.pop_front();
        }
    }
}

struct TrackAlias {
    track_key: String,
    artwork_key: String,
    artwork: Weak<Artwork>,
}

#[derive(Default)]
struct ArtworkRequestState {
    cache: CoverImageCache,
    in_flight: HashMap<String, HashSet<String>>,
    recent_tracks: VecDeque<TrackAlias>,
}

impl ArtworkRequestState {
    /// An old download must not overwrite a newer cover for the same recognized track.
    fn select_artwork(&mut self, track_key: &str, artwork_key: Option<&str>) {
        for (key, tracks) in &mut self.in_flight {
            if Some(key.as_str()) != artwork_key {
                tracks.remove(track_key);
            }
        }
    }

    fn cached_artwork(
        &mut self,
        track_key: &str,
        artwork_key: Option<&str>,
    ) -> Option<Arc<Artwork>> {
        if let Some(key) = artwork_key
            && let Some(image) = self.cache.get(key)
        {
            self.remember_track(track_key, key, &image);
            return Some(image);
        }
        let alias = self.recent_tracks.iter().find(|alias| {
            alias.track_key == track_key && artwork_key.is_none_or(|key| key == alias.artwork_key)
        })?;
        let image = alias.artwork.upgrade()?;
        let key = alias.artwork_key.clone();
        // Refresh both the track alias and the URL cache's recency.
        self.cache.get(&key);
        self.remember_track(track_key, &key, &image);
        Some(image)
    }

    fn remember_track(&mut self, track_key: &str, artwork_key: &str, artwork: &Arc<Artwork>) {
        self.recent_tracks
            .retain(|alias| alias.track_key != track_key && alias.artwork.strong_count() > 0);
        if self.recent_tracks.len() >= COVER_IMAGE_CACHE_CAPACITY {
            self.recent_tracks.pop_front();
        }
        self.recent_tracks.push_back(TrackAlias {
            track_key: track_key.to_owned(),
            artwork_key: artwork_key.to_owned(),
            artwork: Arc::downgrade(artwork),
        });
    }

    fn begin_request(&mut self, artwork_key: &str, track_key: &str) -> bool {
        match self.in_flight.entry(artwork_key.to_owned()) {
            Entry::Occupied(mut request) => {
                request.get_mut().insert(track_key.to_owned());
                false
            }
            Entry::Vacant(request) => {
                request.insert(HashSet::from([track_key.to_owned()]));
                true
            }
        }
    }

    fn finish_request(
        &mut self,
        key: &str,
        artwork: Option<Arc<Artwork>>,
    ) -> (HashSet<String>, Option<Arc<Artwork>>) {
        let waiting = self.in_flight.remove(key).unwrap_or_default();
        let artwork = artwork.map(|image| self.cache.intern(image));
        if let Some(image) = &artwork {
            self.cache.insert(key.to_owned(), image.clone());
            for track_key in &waiting {
                self.remember_track(track_key, key, image);
            }
        }
        (waiting, artwork)
    }
}

pub struct ArtworkService {
    policy: ArtworkPolicy,
    session: soup::Session,
    state: Rc<RefCell<ArtworkRequestState>>,
}

impl ArtworkService {
    pub fn new(policy: ArtworkPolicy) -> Self {
        let session = soup::Session::new();
        session.set_timeout(ARTWORK_REQUEST_TIMEOUT_SECS);
        session.set_idle_timeout(60);
        Self {
            policy,
            session,
            state: Rc::default(),
        }
    }

    /// Returns cached artwork and whether a completion is pending. New downloads
    /// run on the next main-context turn, so callers can publish metadata first.
    pub fn request(
        &self,
        track_key: &str,
        images: &Value,
        complete: impl Fn(String, Option<Arc<Artwork>>) + 'static,
    ) -> ArtworkStatus {
        if self.policy == ArtworkPolicy::None {
            return ArtworkStatus::Unavailable;
        }
        let candidates = preferred_cover_image_urls(images, self.policy);
        let key = candidates
            .first()
            .map(|candidate| artwork_key(&candidate.url));
        let mut state = self.state.borrow_mut();
        state.select_artwork(track_key, key.as_deref());
        if let Some(image) = state.cached_artwork(track_key, key.as_deref()) {
            log::debug!("Artwork for track {track_key}: download cache hit");
            return ArtworkStatus::Ready(image);
        }
        let Some(key) = key else {
            return ArtworkStatus::Unavailable;
        };
        if !state.begin_request(&key, track_key) {
            return ArtworkStatus::Pending;
        }
        drop(state);
        let state = self.state.clone();
        let session = self.session.clone();
        let track_key = track_key.to_owned();
        glib::spawn_future_local(async move {
            let started = Instant::now();
            log::debug!("Artwork for track {track_key}: fetch started");
            let image = fetch_candidates(
                candidates,
                ARTWORK_FETCH_BUDGET,
                PREFERRED_CANDIDATE_BUDGET,
                ORIGINAL_CANDIDATE_RESERVE,
                |url| {
                    let session = session.clone();
                    async move { download_and_decode(&session, &url).await }
                },
            )
            .await;
            log::debug!(
                "Artwork for track {track_key}: fetch/decode completed in {:?}, success={}",
                started.elapsed(),
                image.is_some()
            );
            let (waiting, image) = state.borrow_mut().finish_request(&key, image);
            for track_key in waiting {
                complete(track_key, image.clone());
            }
        });
        ArtworkStatus::Pending
    }
}

async fn download_and_decode(session: &soup::Session, url: &str) -> Option<Arc<Artwork>> {
    let started = Instant::now();
    let bytes = match obtain_raw_cover_image(session, url).await {
        Ok(bytes) => bytes,
        Err(error) => {
            log::debug!(
                "Artwork candidate {url} failed after {:?}: {error}",
                started.elapsed()
            );
            return None;
        }
    };
    log::debug!(
        "Artwork: downloaded {} bytes in {:?}",
        bytes.len(),
        started.elapsed()
    );
    let started = Instant::now();
    let image = gio::spawn_blocking(move || Artwork::decode(bytes).map(Arc::new))
        .await
        .map_err(|_| log::warn!("Artwork decoding task panicked"))
        .ok()
        .flatten();
    log::debug!(
        "Artwork: decoded in {:?}, success={}",
        started.elapsed(),
        image.is_some()
    );
    image
}

/// Apply a wall-clock limit to each candidate, including decoding. Preferred
/// renditions cannot spend the portion reserved for response-provided fallbacks.
async fn fetch_candidates<F, Fut>(
    candidates: Vec<Candidate>,
    total_budget: Duration,
    preferred_budget: Duration,
    original_reserve: Duration,
    mut fetch: F,
) -> Option<Arc<Artwork>>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Option<Arc<Artwork>>>,
{
    let started = Instant::now();
    for (index, candidate) in candidates.iter().enumerate() {
        let remaining = total_budget.saturating_sub(started.elapsed());
        let originals_after = candidates[index + 1..]
            .iter()
            .filter(|item| item.original)
            .count();
        let reserved = original_reserve.saturating_mul(originals_after as u32);
        let available = remaining.saturating_sub(reserved);
        let budget = if candidate.original {
            available
        } else {
            available.min(preferred_budget)
        };
        if budget.is_zero() {
            continue;
        }
        match glib::future_with_timeout(budget, fetch(candidate.url.clone())).await {
            Ok(Some(image)) => return Some(image),
            Ok(None) => {}
            Err(_) => log::debug!(
                "Artwork candidate {} exhausted its {:?} time budget",
                index + 1,
                budget
            ),
        }
    }
    None
}

/// Ignore Apple's interchangeable CDN shard and URL authentication parameters,
/// while retaining the asset path so a genuinely different cover is refreshed.
fn artwork_key(url: &str) -> String {
    if is_mzstatic_artwork_url(url) {
        let (_, path) = url
            .strip_prefix("https://")
            .unwrap()
            .split_once('/')
            .unwrap();
        format!("mzstatic.com/{}", path.split(['?', '#']).next().unwrap())
    } else {
        url.to_owned()
    }
}

fn upscale_mzstatic_artwork_url(url: &str, target_size: u32) -> Option<String> {
    if target_size == 0 || !is_mzstatic_artwork_url(url) {
        return None;
    }
    let path_end = url.find(['?', '#']).unwrap_or(url.len());
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
    let Some(without_scheme) = url.strip_prefix("https://") else {
        return false;
    };
    let Some((host, path)) = without_scheme.split_once('/') else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    (host == "mzstatic.com" || host.ends_with(".mzstatic.com")) && path.starts_with("image/thumb/")
}

fn preferred_cover_image_urls(images: &Value, policy: ArtworkPolicy) -> Vec<Candidate> {
    let mut candidates: Vec<Candidate> = Vec::with_capacity(4);
    if policy == ArtworkPolicy::None {
        return candidates;
    }
    for url in [images.get("coverarthq"), images.get("coverart")]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if policy == ArtworkPolicy::Display
            && let Some(upscaled) = upscale_mzstatic_artwork_url(url, PREFERRED_COVER_ART_SIZE_PX)
            && !candidates.iter().any(|item| item.url == upscaled)
        {
            candidates.push(Candidate {
                url: upscaled,
                original: false,
            });
        }
        if let Some(existing) = candidates.iter_mut().find(|item| item.url == url) {
            existing.original = true;
        } else {
            candidates.push(Candidate {
                url: url.to_owned(),
                original: true,
            });
        }
    }
    candidates
}

#[cfg(test)]
mod tests {
    use image::{DynamicImage, ImageFormat};
    use serde_json::json;
    use std::io::Cursor;
    use std::sync::Arc;

    use super::{
        ArtworkRequestState, COVER_IMAGE_CACHE_CAPACITY, COVER_IMAGE_CACHE_MAX_BYTES,
        CoverImageCache, PREFERRED_COVER_ART_SIZE_PX, preferred_cover_image_urls,
        upscale_mzstatic_artwork_url,
    };
    use crate::core::artwork::Artwork;

    fn urls(images: &serde_json::Value) -> Vec<String> {
        preferred_cover_image_urls(images, super::ArtworkPolicy::Display)
            .into_iter()
            .map(|candidate| candidate.url)
            .collect()
    }

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
            urls(&images),
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

        assert_eq!(urls(&images).len(), 2);
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
        assert!(cache.storage_bytes() <= COVER_IMAGE_CACHE_MAX_BYTES);
    }

    #[test]
    fn tracks_on_the_same_album_share_one_download_and_cached_image() {
        let mut state = ArtworkRequestState::default();
        let url = "https://example.com/album.jpg";
        assert!(state.begin_request(url, "track-a"));
        assert!(!state.begin_request(url, "track-b"));
        assert!(!state.begin_request(url, "track-a"));

        let image = artwork(10);
        let (waiting_tracks, _) = state.finish_request(url, Some(image.clone()));
        assert_eq!(waiting_tracks.len(), 2);
        assert!(waiting_tracks.contains("track-a"));
        assert!(waiting_tracks.contains("track-b"));
        assert!(state.in_flight.is_empty());
        assert!(Arc::ptr_eq(&state.cache.get(url).unwrap(), &image));
        assert_eq!(state.cache.storage_bytes(), image.storage_bytes());
    }

    #[test]
    fn failed_shared_download_releases_all_waiters_and_allows_retry() {
        let mut state = ArtworkRequestState::default();
        let url = "https://example.com/album.jpg";
        assert!(state.begin_request(url, "track-a"));
        assert!(!state.begin_request(url, "track-b"));

        assert_eq!(state.finish_request(url, None).0.len(), 2);
        assert!(state.in_flight.is_empty());
        assert!(state.cache.get(url).is_none());
        assert!(state.begin_request(url, "track-b"));
    }

    #[test]
    fn changed_cover_does_not_reuse_an_old_track_alias_or_completion() {
        let mut state = ArtworkRequestState::default();
        let image = artwork(10);
        let old = "https://example.com/old.jpg";
        let new = "https://example.com/new.jpg";
        state.begin_request(old, "a");
        state.select_artwork("a", Some(new));
        state.begin_request(new, "a");
        let (old_waiters, _) = state.finish_request(old, Some(image.clone()));
        assert!(old_waiters.is_empty());
        assert!(state.cached_artwork("a", Some(new)).is_none());
        let (new_waiters, _) = state.finish_request(new, Some(artwork(11)));
        assert!(new_waiters.contains("a"));
        assert!(
            !state
                .cached_artwork("a", Some(new))
                .unwrap()
                .same_content(&image)
        );
    }

    #[test]
    fn track_cache_aliases_are_bounded_and_do_not_keep_evicted_pixels_alive() {
        let mut state = ArtworkRequestState::default();
        let image = artwork(10);
        for i in 0..=super::COVER_IMAGE_CACHE_CAPACITY {
            state.remember_track(&format!("track-{i}"), "url", &image);
        }
        assert_eq!(state.recent_tracks.len(), super::COVER_IMAGE_CACHE_CAPACITY);
        drop(image);
        assert!(state.cached_artwork("track-1", None).is_none());
    }

    #[test]
    fn consumer_policy_avoids_unneeded_downloads_and_upscaling() {
        let original = "https://is1-ssl.mzstatic.com/image/thumb/Music/id/400x400bb.jpg";
        let images = json!({"coverarthq": original});
        assert!(preferred_cover_image_urls(&images, super::ArtworkPolicy::None).is_empty());
        let thumbnails = preferred_cover_image_urls(&images, super::ArtworkPolicy::Thumbnail);
        assert_eq!(thumbnails.len(), 1);
        assert_eq!(thumbnails[0].url, original);
        assert!(thumbnails[0].original);
    }

    #[test]
    fn cdn_shard_churn_is_not_a_new_asset_but_different_paths_are() {
        let a = "https://is1-ssl.mzstatic.com/image/thumb/Music/one/400x400bb.jpg?token=a";
        let b = "https://is9-ssl.mzstatic.com/image/thumb/Music/one/400x400bb.jpg?token=b";
        let c = "https://is1-ssl.mzstatic.com/image/thumb/Music/two/400x400bb.jpg";
        assert_eq!(super::artwork_key(a), super::artwork_key(b));
        assert_ne!(super::artwork_key(a), super::artwork_key(c));
        for invalid in [
            "https://mzstatic.com.evil.test/image/thumb/a/400x400bb.jpg",
            "https://evil.test/?mzstatic.com/image/thumb/a/400x400bb.jpg",
            "https://mzstatic.com@evil.test/image/thumb/a/400x400bb.jpg",
        ] {
            assert_eq!(upscale_mzstatic_artwork_url(invalid, 1600), None);
        }
    }

    #[test]
    fn content_identity_and_cache_interning_survive_fresh_decodes() {
        let first = artwork(4);
        let second = artwork(4);
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(first.content_id(), second.content_id());
        assert_ne!(first.content_id(), artwork(5).content_id());
        let mut cache = CoverImageCache::default();
        cache.insert("one".into(), first.clone());
        let shared = cache.intern(second);
        assert!(Arc::ptr_eq(&first, &shared));
        cache.insert("two".into(), shared);
        assert_eq!(cache.storage_bytes(), first.storage_bytes());
    }

    #[test]
    fn slow_primary_cannot_starve_a_fast_original_fallback() {
        use std::cell::RefCell;
        use std::rc::Rc;
        use std::time::Duration;
        let context = glib::MainContext::new();
        let calls = Rc::new(RefCell::new(Vec::new()));
        let output = calls.clone();
        let result = context.block_on(super::fetch_candidates(
            vec![
                super::Candidate {
                    url: "hq".into(),
                    original: false,
                },
                super::Candidate {
                    url: "original".into(),
                    original: true,
                },
            ],
            Duration::from_millis(150),
            Duration::from_millis(20),
            Duration::from_millis(30),
            move |url| {
                output.borrow_mut().push(url.clone());
                async move {
                    if url == "hq" {
                        std::future::pending::<()>().await;
                    }
                    Some(artwork(9))
                }
            },
        ));
        assert_eq!(&*calls.borrow(), &["hq", "original"]);
        assert!(result.unwrap().same_content(&artwork(9)));
    }

    #[test]
    fn failing_hq_and_original_still_leave_time_for_coverart() {
        use std::time::Duration;
        let context = glib::MainContext::new();
        let result = context.block_on(super::fetch_candidates(
            vec![
                super::Candidate {
                    url: "hq".into(),
                    original: false,
                },
                super::Candidate {
                    url: "hq-original".into(),
                    original: true,
                },
                super::Candidate {
                    url: "coverart".into(),
                    original: true,
                },
            ],
            Duration::from_millis(120),
            Duration::from_millis(10),
            Duration::from_millis(30),
            move |url| async move {
                if url != "coverart" {
                    std::future::pending::<()>().await;
                }
                Some(artwork(9))
            },
        ));
        assert!(result.is_some());
    }
}
