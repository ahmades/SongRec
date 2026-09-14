//! GUI rendering knobs, grouped by responsibility for a single tuning location.
//!
//! User-facing defaults/ranges remain in core preferences. Motion settings,
//! motion-path parameters, layout, and typography are intentionally independent
//! from the backdrop-intensity profiles below.

use crate::core::preferences::BackdropIntensity;

/// Rendering parameters shared by the generated Cinema and Ambient backdrops.
///
/// Keeping the values together makes each user-facing intensity a coherent
/// visual treatment instead of a collection of independently tuned constants.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct BackdropProfile {
    pub blur_sigma: f32,
    pub lightness_multiplier: f32,
    pub max_lightness: f32,
    pub max_saturation: f32,
    pub scrim_alpha_scale: f64,
}

impl BackdropProfile {
    pub const SOFT: Self = Self {
        blur_sigma: 7.0,
        lightness_multiplier: 0.70,
        max_lightness: 0.42,
        max_saturation: 0.42,
        scrim_alpha_scale: 1.20,
    };

    /// The pre-intensity backdrop treatment, preserved exactly.
    pub const BALANCED: Self = Self {
        blur_sigma: 4.0,
        lightness_multiplier: 0.80,
        max_lightness: 0.50,
        max_saturation: 0.60,
        scrim_alpha_scale: 1.0,
    };

    pub const BOLD: Self = Self {
        blur_sigma: 2.5,
        lightness_multiplier: 0.92,
        max_lightness: 0.50,
        max_saturation: 0.78,
        scrim_alpha_scale: 0.95,
    };

    pub const fn for_intensity(intensity: BackdropIntensity) -> Self {
        match intensity {
            BackdropIntensity::Soft => Self::SOFT,
            BackdropIntensity::Balanced => Self::BALANCED,
            BackdropIntensity::Bold => Self::BOLD,
        }
    }

    /// Scales a base scrim alpha while keeping the Cairo input valid.
    pub fn scrim_alpha(self, alpha: f64) -> f64 {
        (alpha * self.scrim_alpha_scale).clamp(0.0, 1.0)
    }
}

pub(super) mod ambient {
    pub const AMBIENT_MAXIMUM_DIMENSION: u32 = 1024;
    #[cfg(test)]
    pub const AMBIENT_BLUR_SIGMA: f32 = super::BackdropProfile::BALANCED.blur_sigma;
    #[cfg(test)]
    pub const AMBIENT_MAX_SATURATION: f32 = super::BackdropProfile::BALANCED.max_saturation;
    #[cfg(test)]
    pub const AMBIENT_LIGHTNESS_MULTIPLIER: f32 =
        super::BackdropProfile::BALANCED.lightness_multiplier;
    #[cfg(test)]
    pub const AMBIENT_MAX_LIGHTNESS: f32 = super::BackdropProfile::BALANCED.max_lightness;
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

/// Neutral presentation used when a recognized track has no usable artwork.
///
/// These colors are deliberately separate from the generic rendering fallback:
/// the latter also covers initialization and failed palette extraction, while
/// this palette is part of the user-visible missing-artwork design.
pub(super) mod missing_artwork {
    pub const BACKGROUND_TOP: (u8, u8, u8) = (48, 50, 58);
    pub const BACKGROUND_BOTTOM: (u8, u8, u8) = (10, 11, 14);
    pub const CARD_MIDDLE: (u8, u8, u8) = (35, 38, 45);
    pub const CARD_BORDER_ALPHA: f64 = 0.10;
    pub const CARD_ICON_ALPHA: f64 = 0.42;
    pub const CARD_ICON_SIZE_PX: i32 = 88;
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
