//! GUI rendering knobs, grouped by responsibility for a single tuning location.
//!
//! User-facing defaults/ranges remain in core preferences. Motion settings and
//! motion-path parameters are intentionally unchanged. Pixel, opacity and
//! typography values here preserve the existing appearance.

pub(super) mod ambient {
    pub const AMBIENT_MAXIMUM_DIMENSION: u32 = 1024;
    pub const AMBIENT_BLUR_SIGMA: f32 = 4.0;
    pub const AMBIENT_MAX_SATURATION: f32 = 0.60;
    pub const AMBIENT_LIGHTNESS_MULTIPLIER: f32 = 0.80;
    pub const AMBIENT_MAX_LIGHTNESS: f32 = 0.50;
    pub const AMBIENT_VIGNETTE_STRENGTH: f32 = 0.0;
    pub const NEUTRAL_COLORFULNESS_START: f32 = 0.10;
    pub const FULL_COLORFULNESS_START: f32 = 0.28;
}

pub(super) mod layout {
    pub const WINDOW_WIDTH: i32 = 720;
    pub const WINDOW_HEIGHT: i32 = 820;
    pub const MIN_WINDOW_WIDTH: i32 = 360;
    pub const MIN_WINDOW_HEIGHT: i32 = 410;
    pub const MIN_ARTWORK_SIZE: i32 = 135;
    pub const ARTWORK_MARGIN_PX: i32 = 24;
    pub const ARTWORK_CORNER_RADIUS_PX: i32 = 18;
    pub const ROOT_SPACING: i32 = 18;
    pub const INFO_BOX_SPACING: i32 = 2;
    pub const CLASSIC_PADDING_MIN_PX: i32 = 32;
    pub const CLASSIC_PADDING_MAX_PX: i32 = 96;
    pub const IMMERSIVE_MARGIN_MIN_PX: i32 = 28;
    pub const IMMERSIVE_MARGIN_MAX_PX: i32 = 96;
    pub const CINEMA_CROP_RETENTION_MINIMUM: f64 = 0.75;
    pub const CINEMA_PORTRAIT_ARTWORK_MAX_HEIGHT_FRACTION: f64 = 0.70;
    pub const AMBIENT_FOREGROUND_OPACITY: f64 = 0.4;
    pub const SECONDARY_METADATA_OPACITY: f64 = 0.72;
}

pub(super) mod background {
    pub const GRADIENT_SURFACE_WIDTH: i32 = 256;
    pub const TRANSITION_START: f64 = 0.20;
    pub const AMBIENT_BASE_SCRIM_ALPHA: f64 = 0.18;
    pub const AMBIENT_TOP_SCRIM_ALPHA: f64 = 0.05;
    pub const AMBIENT_BOTTOM_SCRIM_ALPHA: f64 = 0.08;
}

pub(super) mod typography {
    pub const BASE_SCALE_WIDTH: f64 = 720.0;
    pub const BASE_SCALE_HEIGHT: f64 = 820.0;
    pub const MIN_FONT_SCALE: f64 = 0.60;
    pub const MAX_FONT_SCALE: f64 = 2.25;
    pub const TITLE_BASE_FONT_SIZE: f64 = 32.0;
    pub const ARTIST_BASE_FONT_SIZE: f64 = 24.0;
    pub const ALBUM_BASE_FONT_SIZE: f64 = 18.0;
    pub const DETAILS_BASE_FONT_SIZE: f64 = 18.0;
}

pub(super) mod cinema {
    pub const BASE_SCRIM_ALPHA: f64 = 0.08;
    pub const PORTRAIT_SCRIM_EXTENT: f64 = 0.52;
    pub const PORTRAIT_SCRIM_STOPS: [(f64, f64); 3] = [(0.0, 0.74), (0.40, 0.28), (1.0, 0.0)];
    pub const WIDE_SCRIM_EXTENT: f64 = 0.62;
    pub const WIDE_SCRIM_STOPS: [(f64, f64); 3] = [(0.0, 0.72), (0.72, 0.26), (1.0, 0.0)];
    pub const COVER_SCRIM_START: f64 = 0.50;
    pub const COVER_SCRIM_STOPS: [(f64, f64); 3] = [(0.0, 0.0), (0.60, 0.28), (1.0, 0.74)];
}
