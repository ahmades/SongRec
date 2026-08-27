//! Validated, decoded artwork shared by recognition and GUI presentation.

use image::DynamicImage;
use std::collections::HashMap;
use std::fmt;
use std::io::Cursor;
use std::sync::Arc;

type Rgb = (u8, u8, u8);
type Hsl = (f32, f32, f32);

pub const MAX_ARTWORK_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_ARTWORK_DIMENSION_PX: u32 = 4_096;

/// Dark colors derived from an album cover for the Now Playing background.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtworkBackground {
    pub top: Rgb,
    pub bottom: Rgb,
}

impl ArtworkBackground {
    pub const fn fallback() -> Self {
        Self {
            top: (38, 38, 38),
            bottom: (0, 0, 0),
        }
    }
}

/// One validated cover image, decoded once outside GTK's main-loop rendering path.
///
/// Encoded bytes are retained for MPRIS/cache-file consumers, while shared RGBA
/// pixels let GTK construct a memory texture without decoding the JPEG/PNG again.
#[derive(Clone)]
pub struct Artwork {
    encoded: Arc<[u8]>,
    rgba: Arc<[u8]>,
    width: u32,
    height: u32,
    stride: usize,
    background: ArtworkBackground,
}

impl fmt::Debug for Artwork {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Artwork")
            .field("encoded_bytes", &self.encoded.len())
            .field("rgba_bytes", &self.rgba.len())
            .field("width", &self.width)
            .field("height", &self.height)
            .field("background", &self.background)
            .finish()
    }
}

impl Artwork {
    /// Validates and decodes a PNG or JPEG response.
    pub fn decode(encoded: Vec<u8>) -> Option<Self> {
        if encoded.is_empty() || encoded.len() > MAX_ARTWORK_BYTES {
            return None;
        }

        let format = image::guess_format(&encoded).ok()?;
        if !matches!(format, image::ImageFormat::Jpeg | image::ImageFormat::Png) {
            return None;
        }

        let mut limits = image::Limits::default();
        limits.max_image_width = Some(MAX_ARTWORK_DIMENSION_PX);
        limits.max_image_height = Some(MAX_ARTWORK_DIMENSION_PX);
        // A validated RGBA image at the maximum dimensions occupies 64 MiB;
        // leave room for decoder scratch buffers while retaining a hard cap.
        limits.max_alloc = Some(128 * 1024 * 1024);

        let mut reader = image::ImageReader::with_format(Cursor::new(&encoded), format);
        reader.limits(limits);
        let image = reader.decode().ok()?;
        if image.width() == 0
            || image.height() == 0
            || image.width() > MAX_ARTWORK_DIMENSION_PX
            || image.height() > MAX_ARTWORK_DIMENSION_PX
        {
            return None;
        }

        let background = generate_background(&image);
        let rgba = image.to_rgba8();
        let width = rgba.width();
        let height = rgba.height();
        let stride = usize::try_from(width).ok()?.checked_mul(4)?;

        Some(Self {
            encoded: encoded.into(),
            rgba: rgba.into_raw().into(),
            width,
            height,
            stride,
            background,
        })
    }

    pub fn encoded(&self) -> &[u8] {
        &self.encoded
    }

    pub fn rgba(&self) -> Arc<[u8]> {
        self.rgba.clone()
    }

    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }

    pub const fn stride(&self) -> usize {
        self.stride
    }

    /// Approximate retained payload size used by the in-memory artwork cache.
    pub fn storage_bytes(&self) -> usize {
        self.encoded.len().saturating_add(self.rgba.len())
    }

    pub const fn background(&self) -> ArtworkBackground {
        self.background
    }
}

fn generate_background(image: &DynamicImage) -> ArtworkBackground {
    let small = image.thumbnail(72, 72).to_rgb8();

    #[derive(Default, Clone, Copy)]
    struct Bucket {
        weight: f32,
        red: f32,
        green: f32,
        blue: f32,
    }

    let mut buckets = HashMap::<u32, Bucket>::new();
    for (x, y, pixel) in small.enumerate_pixels() {
        let [red, green, blue] = pixel.0;
        let rf = red as f32 / 255.0;
        let gf = green as f32 / 255.0;
        let bf = blue as f32 / 255.0;
        let (_, saturation, _) = rgb_to_hsl((red, green, blue));
        let luminance = relative_luminance(rf, gf, bf);

        if luminance > 0.92 || luminance < 0.018 {
            continue;
        }

        let saturation_weight = 0.35 + saturation.powf(1.35) * 3.5;
        let luminance_weight = (1.0 - ((luminance - 0.38).abs() / 0.38)).clamp(0.15, 1.0);
        let nx = (x as f32 + 0.5) / small.width() as f32;
        let ny = (y as f32 + 0.5) / small.height() as f32;
        let center_distance = ((nx - 0.5).powi(2) + (ny - 0.5).powi(2)).sqrt();
        let spatial_weight = (1.15 - center_distance).clamp(0.55, 1.15);
        let chroma_weight = if saturation < 0.06 { 0.55 } else { 1.0 };
        let weight = saturation_weight * luminance_weight * spatial_weight * chroma_weight;

        let key =
            ((u32::from(red >> 4)) << 8) | ((u32::from(green >> 4)) << 4) | u32::from(blue >> 4);
        let bucket = buckets.entry(key).or_default();
        bucket.weight += weight;
        bucket.red += rf * weight;
        bucket.green += gf * weight;
        bucket.blue += bf * weight;
    }

    let Some(bucket) = buckets.into_values().max_by(|first, second| {
        first
            .weight
            .partial_cmp(&second.weight)
            .unwrap_or(std::cmp::Ordering::Equal)
    }) else {
        return ArtworkBackground::fallback();
    };
    if bucket.weight <= f32::EPSILON {
        return ArtworkBackground::fallback();
    }

    let (hue, saturation, _) = rgb_to_hsl((
        (bucket.red / bucket.weight * 255.0).round() as u8,
        (bucket.green / bucket.weight * 255.0).round() as u8,
        (bucket.blue / bucket.weight * 255.0).round() as u8,
    ));
    let target_saturation = if saturation < 0.08 {
        0.0
    } else {
        (saturation * 1.08).clamp(0.22, 0.70)
    };

    let mut top_lightness = if target_saturation == 0.0 {
        0.135
    } else {
        0.165
    };
    let top = loop {
        let rgb = hsl_to_rgb((hue, target_saturation, top_lightness));
        if contrast_ratio(rgb, (255, 255, 255)) >= 4.75 || top_lightness <= 0.07 {
            break rgb;
        }
        top_lightness -= 0.01;
    };

    let mut bottom_saturation = (target_saturation * 0.42).min(0.30);
    let mut bottom_lightness = 0.055;
    let bottom = loop {
        let rgb = hsl_to_rgb((hue, bottom_saturation, bottom_lightness));
        if contrast_ratio(rgb, (255, 255, 255)) >= 7.0 || bottom_lightness <= 0.025 {
            break rgb;
        }
        bottom_lightness -= 0.005;
        bottom_saturation *= 0.96;
    };

    ArtworkBackground { top, bottom }
}

fn rgb_to_hsl((red, green, blue): Rgb) -> Hsl {
    let red = red as f32 / 255.0;
    let green = green as f32 / 255.0;
    let blue = blue as f32 / 255.0;
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let lightness = (max + min) / 2.0;
    let delta = max - min;
    if delta <= f32::EPSILON {
        return (0.0, 0.0, lightness);
    }
    let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
    let hue = if max == red {
        ((green - blue) / delta).rem_euclid(6.0) / 6.0
    } else if max == green {
        (((blue - red) / delta) + 2.0) / 6.0
    } else {
        (((red - green) / delta) + 4.0) / 6.0
    };
    (hue, saturation, lightness)
}

fn hsl_to_rgb((hue, saturation, lightness): Hsl) -> Rgb {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let x = chroma * (1.0 - ((hue * 6.0).rem_euclid(2.0) - 1.0).abs());
    let offset = lightness - chroma / 2.0;
    let (red, green, blue) = match (hue * 6.0).floor() as i32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    (
        ((red + offset).clamp(0.0, 1.0) * 255.0).round() as u8,
        ((green + offset).clamp(0.0, 1.0) * 255.0).round() as u8,
        ((blue + offset).clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

fn relative_luminance(red: f32, green: f32, blue: f32) -> f32 {
    fn linearize(channel: f32) -> f32 {
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * linearize(red) + 0.7152 * linearize(green) + 0.0722 * linearize(blue)
}

fn contrast_ratio(first: Rgb, second: Rgb) -> f32 {
    let first = relative_luminance(
        first.0 as f32 / 255.0,
        first.1 as f32 / 255.0,
        first.2 as f32 / 255.0,
    );
    let second = relative_luminance(
        second.0 as f32 / 255.0,
        second.1 as f32 / 255.0,
        second.2 as f32 / 255.0,
    );
    let lighter = first.max(second);
    let darker = first.min(second);
    (lighter + 0.05) / (darker + 0.05)
}

#[cfg(test)]
mod tests {
    use super::{
        Artwork, ArtworkBackground, MAX_ARTWORK_DIMENSION_PX, contrast_ratio, generate_background,
    };
    use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
    use std::io::Cursor;

    fn encoded_png() -> Vec<u8> {
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::new_rgba8(2, 2)
            .write_to(&mut encoded, ImageFormat::Png)
            .unwrap();
        encoded.into_inner()
    }

    #[test]
    fn artwork_is_validated_and_decoded_once() {
        let artwork = Artwork::decode(encoded_png()).unwrap();
        assert_eq!((artwork.width(), artwork.height()), (2, 2));
        assert_eq!(artwork.stride(), 8);
        assert_eq!(artwork.rgba().len(), 16);
        assert!(Artwork::decode(Vec::new()).is_none());
        assert!(Artwork::decode(b"<html>not artwork</html>".to_vec()).is_none());
    }

    #[test]
    fn oversized_dimensions_are_rejected_before_allocating_the_decoded_image() {
        let image = DynamicImage::new_rgba8(MAX_ARTWORK_DIMENSION_PX + 1, 1);
        let mut encoded = Cursor::new(Vec::new());
        image.write_to(&mut encoded, ImageFormat::Png).unwrap();

        assert!(Artwork::decode(encoded.into_inner()).is_none());
    }

    #[test]
    fn bright_artwork_uses_the_fallback_background() {
        let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 8, Rgb([255, 255, 255])));
        assert_eq!(generate_background(&image), ArtworkBackground::fallback());
    }

    #[test]
    fn generated_palette_preserves_white_text_contrast() {
        let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 8, Rgb([224, 54, 72])));
        let background = generate_background(&image);
        assert!(contrast_ratio(background.top, (255, 255, 255)) >= 4.75);
        assert!(contrast_ratio(background.bottom, (255, 255, 255)) >= 7.0);
    }
}
