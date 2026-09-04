//! Shared CSS rules for the Now Playing window.
//!
//! Keeping typography here lets widget construction and background rendering
//! depend on the same visual vocabulary without depending on each other.

use super::{NowPlayingWindow, TextSize};
use adw::prelude::*;

const BASE_SCALE_WIDTH: f64 = 720.0;
const BASE_SCALE_HEIGHT: f64 = 820.0;
const MIN_FONT_SCALE: f64 = 0.60;
const MAX_FONT_SCALE: f64 = 2.25;
const TITLE_BASE_FONT_SIZE: f64 = 32.0;
const ARTIST_BASE_FONT_SIZE: f64 = 24.0;
const ALBUM_BASE_FONT_SIZE: f64 = 18.0;
const DETAILS_BASE_FONT_SIZE: f64 = 18.0;

pub(super) const TITLE_CSS_CLASS: &str = "now-playing-title";
pub(super) const ARTIST_CSS_CLASS: &str = "now-playing-artist";
pub(super) const ALBUM_CSS_CLASS: &str = "now-playing-album";
pub(super) const DETAILS_CSS_CLASS: &str = "now-playing-details";
pub(super) const TITLE_RESERVATION_CSS_CLASS: &str = "now-playing-title-reservation";
pub(super) const ARTIST_RESERVATION_CSS_CLASS: &str = "now-playing-artist-reservation";
pub(super) const ALBUM_RESERVATION_CSS_CLASS: &str = "now-playing-album-reservation";
pub(super) const DETAILS_RESERVATION_CSS_CLASS: &str = "now-playing-details-reservation";

/// Loads metadata CSS for the current viewport and selected relative size.
pub(super) fn load_text_css(
    provider: &gtk::CssProvider,
    viewport: &gtk::DrawingArea,
    text_size: TextSize,
) {
    let width = viewport.width();
    let height = viewport.height();
    let viewport_size = if width > 0 && height > 0 {
        (width, height)
    } else {
        (BASE_SCALE_WIDTH as i32, BASE_SCALE_HEIGHT as i32)
    };
    provider.load_from_string(&font_css_for_size(viewport_size, text_size));
}

impl NowPlayingWindow {
    /// Reloads responsive metadata CSS after the user changes its size.
    pub(super) fn refresh_text_css(&self, text_size: TextSize) {
        load_text_css(&self.text_css, &self.ui.background_area, text_size);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MetadataFontSizes {
    title: i32,
    artist: i32,
    album: i32,
    details: i32,
}

/// Generates CSS for metadata font sizes scaled to the supplied window dimensions.
pub(super) fn font_css_for_size(size: (i32, i32), text_size: TextSize) -> String {
    let sizes = metadata_font_sizes_for_size(size, text_size);
    let reservation_sizes = metadata_font_sizes_for_size(size, TextSize::LARGE);

    format!(
        ".{TITLE_CSS_CLASS} {{ font-size: {}px; font-weight: bold; }}
         .{ARTIST_CSS_CLASS} {{ font-size: {}px; font-weight: bold; }}
         .{ALBUM_CSS_CLASS} {{ font-size: {}px; font-weight: bold; }}
         .{DETAILS_CSS_CLASS} {{ font-size: {}px; font-weight: bold; }}
         .{TITLE_RESERVATION_CSS_CLASS} {{ font-size: {}px; font-weight: bold; }}
         .{ARTIST_RESERVATION_CSS_CLASS} {{ font-size: {}px; font-weight: bold; }}
         .{ALBUM_RESERVATION_CSS_CLASS} {{ font-size: {}px; font-weight: bold; }}
         .{DETAILS_RESERVATION_CSS_CLASS} {{ font-size: {}px; font-weight: bold; }}",
        sizes.title,
        sizes.artist,
        sizes.album,
        sizes.details,
        reservation_sizes.title,
        reservation_sizes.artist,
        reservation_sizes.album,
        reservation_sizes.details,
    )
}

fn metadata_font_sizes_for_size(size: (i32, i32), text_size: TextSize) -> MetadataFontSizes {
    let width_scale = size.0.max(0) as f64 / BASE_SCALE_WIDTH;
    let height_scale = size.1.max(0) as f64 / BASE_SCALE_HEIGHT;
    let scale = width_scale
        .min(height_scale)
        .clamp(MIN_FONT_SCALE, MAX_FONT_SCALE)
        * text_size.multiplier();

    MetadataFontSizes {
        title: (TITLE_BASE_FONT_SIZE * scale).round() as i32,
        artist: (ARTIST_BASE_FONT_SIZE * scale).round() as i32,
        album: (ALBUM_BASE_FONT_SIZE * scale).round() as i32,
        details: (DETAILS_BASE_FONT_SIZE * scale).round() as i32,
    }
}

#[cfg(test)]
mod tests {
    use super::{MetadataFontSizes, font_css_for_size, metadata_font_sizes_for_size};
    use crate::core::preferences::TextSize;

    #[test]
    fn metadata_font_sizes_are_bounded_at_small_and_large_window_sizes() {
        assert_eq!(
            metadata_font_sizes_for_size((1, 1), TextSize::MEDIUM),
            MetadataFontSizes {
                title: 19,
                artist: 14,
                album: 11,
                details: 11,
            }
        );
        assert_eq!(
            metadata_font_sizes_for_size((10_000, 10_000), TextSize::MEDIUM),
            MetadataFontSizes {
                title: 72,
                artist: 54,
                album: 41,
                details: 41,
            }
        );
    }

    #[test]
    fn metadata_font_sizes_match_the_default_window() {
        assert_eq!(
            metadata_font_sizes_for_size((720, 820), TextSize::MEDIUM),
            MetadataFontSizes {
                title: 32,
                artist: 24,
                album: 18,
                details: 18,
            }
        );
        assert!(font_css_for_size((720, 820), TextSize::MEDIUM).contains("font-size: 32px"));
    }

    #[test]
    fn selected_text_size_composes_with_responsive_scaling() {
        assert_eq!(
            metadata_font_sizes_for_size((720, 820), TextSize::SMALL),
            MetadataFontSizes {
                title: 26,
                artist: 19,
                album: 14,
                details: 14,
            }
        );
        assert_eq!(
            metadata_font_sizes_for_size((720, 820), TextSize::LARGE),
            MetadataFontSizes {
                title: 38,
                artist: 29,
                album: 22,
                details: 22,
            }
        );

        let small_css = font_css_for_size((720, 820), TextSize::SMALL);
        assert!(small_css.contains(".now-playing-title { font-size: 26px"));
        assert!(small_css.contains(".now-playing-title-reservation { font-size: 38px"));
    }
}
