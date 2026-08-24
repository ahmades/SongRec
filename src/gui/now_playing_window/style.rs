//! Shared CSS rules for the Now Playing window.
//!
//! Keeping typography here lets widget construction and background rendering
//! depend on the same visual vocabulary without depending on each other.

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MetadataFontSizes {
    title: i32,
    artist: i32,
    album: i32,
    details: i32,
}

/// Generates CSS for metadata font sizes scaled to the supplied window dimensions.
pub(super) fn font_css_for_size(size: (i32, i32)) -> String {
    let sizes = metadata_font_sizes_for_size(size);

    format!(
        ".{TITLE_CSS_CLASS} {{ font-size: {}px; font-weight: bold; }}
         .{ARTIST_CSS_CLASS} {{ font-size: {}px; font-weight: bold; }}
         .{ALBUM_CSS_CLASS} {{ font-size: {}px; font-weight: bold; }}
         .{DETAILS_CSS_CLASS} {{ font-size: {}px; font-weight: bold; }}",
        sizes.title, sizes.artist, sizes.album, sizes.details,
    )
}

fn metadata_font_sizes_for_size(size: (i32, i32)) -> MetadataFontSizes {
    let width_scale = size.0.max(0) as f64 / BASE_SCALE_WIDTH;
    let height_scale = size.1.max(0) as f64 / BASE_SCALE_HEIGHT;
    let scale = width_scale
        .min(height_scale)
        .clamp(MIN_FONT_SCALE, MAX_FONT_SCALE);

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

    #[test]
    fn metadata_font_sizes_are_bounded_at_small_and_large_window_sizes() {
        assert_eq!(
            metadata_font_sizes_for_size((1, 1)),
            MetadataFontSizes {
                title: 19,
                artist: 14,
                album: 11,
                details: 11,
            }
        );
        assert_eq!(
            metadata_font_sizes_for_size((10_000, 10_000)),
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
            metadata_font_sizes_for_size((720, 820)),
            MetadataFontSizes {
                title: 32,
                artist: 24,
                album: 18,
                details: 18,
            }
        );
        assert!(font_css_for_size((720, 820)).contains("font-size: 32px"));
    }
}
