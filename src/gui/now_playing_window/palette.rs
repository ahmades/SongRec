//! Derives Now Playing colors and combines them with GTK artwork textures.

use crate::core::artwork::Artwork;
use image::{ImageBuffer, Rgba};
use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, Weak};

type Rgb = (u8, u8, u8);
type Hsl = (f32, f32, f32);

/// Dark colors derived from an album cover for the Now Playing background.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Background {
    pub(super) top: Rgb,
    pub(super) bottom: Rgb,
}

impl Background {
    pub(super) const fn fallback() -> Self {
        Self {
            top: (38, 38, 38),
            bottom: (0, 0, 0),
        }
    }
}

/// A GTK texture and palette derived from the same decoded [`Artwork`].
#[derive(Clone)]
pub(super) struct PreparedArtwork {
    pub(super) texture: gdk::MemoryTexture,
    pub(super) background: Background,
    source: Weak<Artwork>,
}

impl PreparedArtwork {
    /// Whether this presentation was derived from the same decoded artwork.
    pub(super) fn matches(&self, artwork: &Arc<Artwork>) -> bool {
        Weak::ptr_eq(&self.source, &Arc::downgrade(artwork))
    }
}

/// Computes the Now Playing palette without constructing or accessing GTK objects.
///
/// Cloning [`Artwork::rgba`] only increments its `Arc` reference count. The
/// source image is not copied; only the small thumbnail used for color analysis
/// is allocated. This function is therefore safe to run on a blocking worker.
pub(super) fn background_from_artwork(artwork: &Artwork) -> Background {
    let image =
        ImageBuffer::<Rgba<u8>, _>::from_raw(artwork.width(), artwork.height(), artwork.rgba())
            .expect("validated artwork has a complete RGBA pixel buffer");

    generate_background(&image)
}

/// Creates the GTK texture on the main thread after palette preparation.
pub(super) fn prepare_artwork(artwork: &Arc<Artwork>, background: Background) -> PreparedArtwork {
    PreparedArtwork {
        texture: crate::gui::artwork::texture(artwork),
        background,
        source: Arc::downgrade(artwork),
    }
}

fn generate_background<Container>(image: &ImageBuffer<Rgba<u8>, Container>) -> Background
where
    Container: Deref<Target = [u8]>,
{
    let (thumbnail_width, thumbnail_height) =
        thumbnail_dimensions(image.width(), image.height(), 72);
    let small = image::imageops::thumbnail(image, thumbnail_width, thumbnail_height);

    #[derive(Default, Clone, Copy)]
    struct Bucket {
        weight: f32,
        red: f32,
        green: f32,
        blue: f32,
    }

    let mut buckets = HashMap::<u32, Bucket>::new();
    for (x, y, pixel) in small.enumerate_pixels() {
        let [red, green, blue, _alpha] = pixel.0;
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
        return Background::fallback();
    };
    if bucket.weight <= f32::EPSILON {
        return Background::fallback();
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

    Background { top, bottom }
}

/// Computes an aspect-ratio-preserving thumbnail size within a square bound.
fn thumbnail_dimensions(width: u32, height: u32, maximum: u32) -> (u32, u32) {
    let ratio = (f64::from(maximum) / f64::from(width)).min(f64::from(maximum) / f64::from(height));
    (
        (f64::from(width) * ratio).round().max(1.0) as u32,
        (f64::from(height) * ratio).round().max(1.0) as u32,
    )
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
        Background, background_from_artwork, contrast_ratio, generate_background,
        thumbnail_dimensions,
    };
    use crate::core::artwork::Artwork;
    use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba};
    use std::io::Cursor;

    #[test]
    fn bright_artwork_uses_the_fallback_background() {
        let image = ImageBuffer::from_pixel(8, 8, Rgba([255, 255, 255, 255]));
        assert_eq!(generate_background(&image), Background::fallback());
    }

    #[test]
    fn generated_palette_preserves_white_text_contrast() {
        let image = ImageBuffer::from_pixel(8, 8, Rgba([224, 54, 72, 255]));
        let background = generate_background(&image);
        assert!(contrast_ratio(background.top, (255, 255, 255)) >= 4.75);
        assert!(contrast_ratio(background.bottom, (255, 255, 255)) >= 7.0);
    }

    #[test]
    fn decoded_artwork_pixels_feed_the_palette_generator() {
        let image = ImageBuffer::from_pixel(8, 4, Rgba([224, 54, 72, 255]));
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image.clone())
            .write_to(&mut encoded, ImageFormat::Png)
            .unwrap();
        let artwork = Artwork::decode(encoded.into_inner()).unwrap();

        assert_eq!(
            background_from_artwork(&artwork),
            generate_background(&image)
        );
    }

    #[test]
    fn palette_thumbnail_dimensions_preserve_aspect_ratio() {
        assert_eq!(thumbnail_dimensions(400, 200, 72), (72, 36));
        assert_eq!(thumbnail_dimensions(200, 400, 72), (36, 72));
        assert_eq!(thumbnail_dimensions(400, 400, 72), (72, 72));
    }
}
