//! Derives Now Playing colors and combines them with GTK artwork textures.

use crate::core::artwork::Artwork;
use image::{ImageBuffer, Rgba};
use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, Weak};

type Rgb = (u8, u8, u8);
type Hsl = (f32, f32, f32);

const AMBIENT_MAXIMUM_DIMENSION: u32 = 1024;
const AMBIENT_BLUR_SIGMA: f32 = 4.0;
const AMBIENT_MAX_SATURATION: f32 = 0.60;
const AMBIENT_LIGHTNESS_MULTIPLIER: f32 = 0.80;
const AMBIENT_MAX_LIGHTNESS: f32 = 0.50;
const AMBIENT_VIGNETTE_STRENGTH: f32 = 0.0;
const NEUTRAL_COLORFULNESS_START: f32 = 0.10;
const FULL_COLORFULNESS_START: f32 = 0.28;

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

/// A GTK texture, ambient texture, and palette derived from the same decoded [`Artwork`].
#[derive(Clone)]
pub(super) struct PreparedArtwork {
    pub(super) texture: gdk::MemoryTexture,
    pub(super) ambient_texture: gdk::MemoryTexture,
    pub(super) background: Background,
    source: Weak<Artwork>,
}

impl PreparedArtwork {
    /// Whether this presentation was derived from the same decoded artwork.
    pub(super) fn matches(&self, artwork: &Arc<Artwork>) -> bool {
        Weak::ptr_eq(&self.source, &Arc::downgrade(artwork))
    }
}

/// GTK-free output produced from artwork on a blocking worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ArtworkVisuals {
    background: Background,
    ambient: AmbientImage,
}

impl ArtworkVisuals {
    /// Supplies a deterministic neutral result if background preparation panics.
    pub(super) fn fallback() -> Self {
        let background = Background::fallback();
        Self {
            background,
            ambient: AmbientImage {
                width: 1,
                height: 1,
                rgba: vec![background.top.0, background.top.1, background.top.2, 255],
            },
        }
    }
}

/// Small, opaque RGBA image that GTK later scales to fill the Ambient background.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AmbientImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

/// Computes all Now Playing artwork visuals without constructing or accessing GTK objects.
///
/// Cloning [`Artwork::rgba`] only increments its `Arc` reference count. The
/// source image is not copied; only small thumbnails used for color analysis
/// and the Ambient background are allocated. This function is safe to run on a
/// blocking worker.
pub(super) fn visuals_from_artwork(artwork: &Artwork) -> ArtworkVisuals {
    let image =
        ImageBuffer::<Rgba<u8>, _>::from_raw(artwork.width(), artwork.height(), artwork.rgba())
            .expect("validated artwork has a complete RGBA pixel buffer");
    let background = generate_background(&image);

    ArtworkVisuals {
        background,
        ambient: generate_ambient_image(&image, background),
    }
}

/// Creates GTK textures on the main thread after worker-side visual preparation.
pub(super) fn prepare_artwork(artwork: &Arc<Artwork>, visuals: ArtworkVisuals) -> PreparedArtwork {
    let ArtworkVisuals {
        background,
        ambient,
    } = visuals;
    let ambient_stride = ambient.width as usize * 4;
    let ambient_bytes = glib::Bytes::from_owned(ambient.rgba);
    let ambient_texture = gdk::MemoryTexture::new(
        i32::try_from(ambient.width).expect("bounded ambient width fits i32"),
        i32::try_from(ambient.height).expect("bounded ambient height fits i32"),
        gdk::MemoryFormat::R8g8b8a8,
        &ambient_bytes,
        ambient_stride,
    );

    PreparedArtwork {
        texture: crate::gui::artwork::texture(artwork),
        ambient_texture,
        background,
        source: Arc::downgrade(artwork),
    }
}

fn generate_ambient_image<Container>(
    image: &ImageBuffer<Rgba<u8>, Container>,
    background: Background,
) -> AmbientImage
where
    Container: Deref<Target = [u8]>,
{
    let (width, height) =
        thumbnail_dimensions(image.width(), image.height(), AMBIENT_MAXIMUM_DIMENSION);
    let mut thumbnail = image::imageops::thumbnail(image, width, height);

    // Album covers are normally opaque, but compositing transparent pixels
    // before blurring avoids dark color fringes and guarantees an opaque GTK
    // background texture.
    for pixel in thumbnail.pixels_mut() {
        let [red, green, blue, alpha] = pixel.0;
        let alpha = f32::from(alpha) / 255.0;
        pixel.0 = [
            composite_channel(red, background.top.0, alpha),
            composite_channel(green, background.top.1, alpha),
            composite_channel(blue, background.top.2, alpha),
            255,
        ];
    }

    let mut ambient = image::imageops::blur(&thumbnail, AMBIENT_BLUR_SIGMA);
    apply_ambient_tone(&mut ambient);

    AmbientImage {
        width,
        height,
        rgba: ambient.into_raw(),
    }
}

fn composite_channel(foreground: u8, background: u8, alpha: f32) -> u8 {
    (f32::from(foreground) * alpha + f32::from(background) * (1.0 - alpha))
        .round()
        .clamp(0.0, 255.0) as u8
}

fn apply_ambient_tone(image: &mut ImageBuffer<Rgba<u8>, Vec<u8>>) {
    let width = image.width() as f32;
    let height = image.height() as f32;

    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let [red, green, blue, _alpha] = pixel.0;
        let source = (red, green, blue);
        let (hue, _saturation, lightness) = rgb_to_hsl(source);
        let target_chroma = rgb_chroma(source) * colorfulness_strength(source);
        let lightness = (lightness * AMBIENT_LIGHTNESS_MULTIPLIER).min(AMBIENT_MAX_LIGHTNESS);
        let available_chroma = 1.0 - (2.0 * lightness - 1.0).abs();
        let saturation = if available_chroma <= f32::EPSILON {
            0.0
        } else {
            (target_chroma / available_chroma).min(AMBIENT_MAX_SATURATION)
        };
        let (red, green, blue) = hsl_to_rgb((hue, saturation, lightness));

        let normalized_x = ((x as f32 + 0.5) / width) * 2.0 - 1.0;
        let normalized_y = ((y as f32 + 0.5) / height) * 2.0 - 1.0;
        let edge_distance =
            ((normalized_x * normalized_x + normalized_y * normalized_y) / 2.0).clamp(0.0, 1.0);
        let vignette = 1.0 - AMBIENT_VIGNETTE_STRENGTH * edge_distance.sqrt();

        pixel.0 = [
            (f32::from(red) * vignette).round() as u8,
            (f32::from(green) * vignette).round() as u8,
            (f32::from(blue) * vignette).round() as u8,
            255,
        ];
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
        let source = (red, green, blue);
        let colorfulness = hsv_saturation(source);
        let color_strength = colorfulness_strength(source);
        let luminance = relative_luminance(rf, gf, bf);

        if luminance > 0.92 || luminance < 0.018 {
            continue;
        }

        let saturation_weight = 0.35 + colorfulness.powf(1.35) * 3.5;
        let luminance_weight = (1.0 - ((luminance - 0.38).abs() / 0.38)).clamp(0.15, 1.0);
        let nx = (x as f32 + 0.5) / small.width() as f32;
        let ny = (y as f32 + 0.5) / small.height() as f32;
        let center_distance = ((nx - 0.5).powi(2) + (ny - 0.5).powi(2)).sqrt();
        let spatial_weight = (1.15 - center_distance).clamp(0.55, 1.15);
        let chroma_weight = 0.55 + 0.45 * color_strength;
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

    let representative = (
        (bucket.red / bucket.weight * 255.0).round() as u8,
        (bucket.green / bucket.weight * 255.0).round() as u8,
        (bucket.blue / bucket.weight * 255.0).round() as u8,
    );
    let (hue, saturation, _) = rgb_to_hsl(representative);
    let color_strength = colorfulness_strength(representative);
    let target_saturation = (saturation * 1.08).clamp(0.22, 0.70) * color_strength;

    let mut top_lightness = 0.135 + 0.03 * color_strength;
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
    let ratio = (f64::from(maximum) / f64::from(width))
        .min(f64::from(maximum) / f64::from(height))
        .min(1.0);
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

/// Absolute RGB chroma on a normalized 0–1 scale.
fn rgb_chroma((red, green, blue): Rgb) -> f32 {
    let maximum = red.max(green).max(blue);
    let minimum = red.min(green).min(blue);
    f32::from(maximum - minimum) / 255.0
}

/// HSV saturation, used only as a brightness-independent colorfulness signal.
fn hsv_saturation(rgb @ (red, green, blue): Rgb) -> f32 {
    let maximum = red.max(green).max(blue);
    if maximum == 0 {
        0.0
    } else {
        rgb_chroma(rgb) / (f32::from(maximum) / 255.0)
    }
}

/// Smoothly suppresses near-neutral color while avoiding threshold jumps from JPEG noise.
fn colorfulness_strength(rgb: Rgb) -> f32 {
    let progress = ((hsv_saturation(rgb) - NEUTRAL_COLORFULNESS_START)
        / (FULL_COLORFULNESS_START - NEUTRAL_COLORFULNESS_START))
        .clamp(0.0, 1.0);
    progress * progress * (3.0 - 2.0 * progress)
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
        AMBIENT_MAXIMUM_DIMENSION, ArtworkVisuals, Background, colorfulness_strength,
        contrast_ratio, generate_ambient_image, generate_background, hsv_saturation,
        thumbnail_dimensions, visuals_from_artwork,
    };
    use crate::core::artwork::Artwork;
    use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba};
    use std::io::Cursor;

    fn channel_range(channels: [u8; 3]) -> u8 {
        channels.iter().max().unwrap() - channels.iter().min().unwrap()
    }

    fn center_rgb(image: &super::AmbientImage) -> [u8; 3] {
        let x = image.width as usize / 2;
        let y = image.height as usize / 2;
        let offset = (y * image.width as usize + x) * 4;
        image.rgba[offset..offset + 3].try_into().unwrap()
    }

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
        assert!(channel_range([background.top.0, background.top.1, background.top.2,]) >= 40);
    }

    #[test]
    fn warm_cream_generates_a_neutral_background() {
        let image = ImageBuffer::from_pixel(8, 8, Rgba([232, 230, 211, 255]));
        let background = generate_background(&image);

        assert!(channel_range([background.top.0, background.top.1, background.top.2,]) <= 4);
        assert!(
            channel_range([
                background.bottom.0,
                background.bottom.1,
                background.bottom.2,
            ]) <= 2
        );
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
            visuals_from_artwork(&artwork).background,
            generate_background(&image)
        );
    }

    #[test]
    fn palette_thumbnail_dimensions_preserve_aspect_ratio() {
        assert_eq!(thumbnail_dimensions(400, 200, 72), (72, 36));
        assert_eq!(thumbnail_dimensions(200, 400, 72), (36, 72));
        assert_eq!(thumbnail_dimensions(400, 400, 72), (72, 72));
        assert_eq!(thumbnail_dimensions(400, 200, 512), (400, 200));
    }

    #[test]
    fn ambient_image_is_small_opaque_and_aspect_preserving() {
        let image = ImageBuffer::from_pixel(1_024, 512, Rgba([180, 90, 45, 96]));
        let ambient = generate_ambient_image(&image, Background::fallback());

        assert_eq!(
            (ambient.width, ambient.height),
            (AMBIENT_MAXIMUM_DIMENSION, AMBIENT_MAXIMUM_DIMENSION / 2)
        );
        assert_eq!(
            ambient.rgba.len(),
            ambient.width as usize * ambient.height as usize * 4
        );
        assert!(ambient.rgba.chunks_exact(4).all(|pixel| pixel[3] == 255));
    }

    #[test]
    fn ambient_treatment_is_deterministic_dark_and_has_no_radial_shading() {
        let image = ImageBuffer::from_pixel(40, 40, Rgba([240, 160, 80, 255]));
        let first = generate_ambient_image(&image, Background::fallback());
        let second = generate_ambient_image(&image, Background::fallback());

        assert_eq!(first, second);

        let pixel_sum = |x: u32, y: u32| {
            let offset = (y as usize * first.width as usize + x as usize) * 4;
            first.rgba[offset..offset + 3]
                .iter()
                .map(|channel| u32::from(*channel))
                .sum::<u32>()
        };
        let center = pixel_sum(first.width / 2, first.height / 2);
        let corner = pixel_sum(0, 0);
        let source_sum = 240 + 160 + 80;

        assert!(center < source_sum);
        assert_eq!(corner, center);
    }

    #[test]
    fn ambient_treatment_does_not_invent_chroma_in_near_white_artwork() {
        for source in [
            [255, 254, 250],
            [232, 230, 211],
            [215, 224, 232],
            [220, 220, 220],
        ] {
            let image =
                ImageBuffer::from_pixel(40, 40, Rgba([source[0], source[1], source[2], 255]));
            let ambient = generate_ambient_image(&image, Background::fallback());
            let output = center_rgb(&ambient);

            assert!(
                channel_range(output) <= channel_range(source) + 2,
                "near-neutral {source:?} became {output:?}"
            );
            let source_saturation = hsv_saturation((source[0], source[1], source[2]));
            let output_saturation = hsv_saturation((output[0], output[1], output[2]));
            assert!(
                output_saturation <= source_saturation + 0.01,
                "near-neutral {source:?} became relatively more saturated: {output:?}"
            );
            assert!(output.iter().max() < source.iter().max());
        }
    }

    #[test]
    fn neutral_color_strength_is_continuous_around_its_threshold() {
        let below = colorfulness_strength((200, 181, 181));
        let just_above = colorfulness_strength((200, 179, 179));

        assert_eq!(below, 0.0);
        assert!(just_above > 0.0 && just_above < 0.02);
    }

    #[test]
    fn ambient_treatment_preserves_vivid_color_order_and_chroma() {
        let image = ImageBuffer::from_pixel(40, 40, Rgba([240, 160, 80, 255]));
        let ambient = generate_ambient_image(&image, Background::fallback());
        let output = center_rgb(&ambient);

        assert!(output[0] > output[1] && output[1] > output[2]);
        assert!(channel_range(output) >= 150);
    }

    #[test]
    fn fallback_visuals_have_an_opaque_neutral_ambient_image() {
        let visuals = ArtworkVisuals::fallback();

        assert_eq!(visuals.background, Background::fallback());
        assert_eq!((visuals.ambient.width, visuals.ambient.height), (1, 1));
        assert_eq!(visuals.ambient.rgba, vec![38, 38, 38, 255]);
    }
}
