//! Derives Now Playing colors and combines them with GTK artwork textures.

use super::tuning::ambient::*;
use crate::core::artwork::{Artwork, ArtworkId};
use gdk::prelude::TextureExt;
use image::{DynamicImage, ImageBuffer, Rgba};
use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Arc;

/// The visuals needed before a mode can present an artwork-bearing track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ArtworkRequirement {
    None,
    Cover,
    Immersive,
    ImmersiveArtist,
}

impl ArtworkRequirement {
    pub(super) fn for_settings(settings: super::NowPlayingSettings) -> Self {
        use crate::core::preferences::ImmersiveBackgroundSource;

        if settings.display_mode.uses_immersive_artwork()
            && settings.shared.immersive_background_source == ImmersiveBackgroundSource::Artist
        {
            return Self::ImmersiveArtist;
        }
        Self::for_mode(settings.display_mode)
    }

    pub(super) fn for_mode(mode: super::DisplayMode) -> Self {
        match mode {
            super::DisplayMode::LightsOff => Self::None,
            super::DisplayMode::Classic => Self::Cover,
            super::DisplayMode::Cinema | super::DisplayMode::Ambient => Self::Immersive,
        }
    }

    pub(super) const fn needs_ambient_texture(self) -> bool {
        matches!(self, Self::Immersive | Self::ImmersiveArtist)
    }

    pub(super) const fn needs_artist_background(self) -> bool {
        matches!(self, Self::ImmersiveArtist)
    }
}

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

/// A GTK texture, ambient texture, and palette derived from the same decoded [`Artwork`].
#[derive(Clone)]
pub(super) struct PreparedArtwork {
    pub(super) texture: gdk::MemoryTexture,
    pub(super) ambient_texture: Option<gdk::MemoryTexture>,
    pub(super) background: Background,
    source: Arc<Artwork>,
}

/// A blurred immersive backdrop prepared independently from the album cover.
///
/// Cinema and Ambient still use [`PreparedArtwork::texture`] as their sharp
/// foreground. Keeping the backdrop separate prevents an artist photograph
/// from accidentally replacing that foreground when it is selected as the
/// background source.
#[derive(Clone)]
pub(super) struct PreparedImmersiveBackground {
    pub(super) texture: gdk::MemoryTexture,
    pub(super) background: Background,
    source_id: ArtworkId,
}

impl PreparedImmersiveBackground {
    pub(super) const fn source_id(&self) -> ArtworkId {
        self.source_id
    }

    pub(super) fn matches(&self, artwork: &Arc<Artwork>) -> bool {
        self.source_id == artwork.content_id()
    }

    pub(super) fn storage_bytes(&self) -> usize {
        (self.texture.width() as usize)
            .saturating_mul(self.texture.height() as usize)
            .saturating_mul(4)
    }
}

impl PreparedArtwork {
    pub(super) fn source(&self) -> &Arc<Artwork> {
        &self.source
    }

    /// Whether this presentation was derived from the same decoded artwork.
    pub(super) fn matches(&self, artwork: &Arc<Artwork>) -> bool {
        self.source.content_id() == artwork.content_id()
    }

    pub(super) fn is_ready(&self, requirement: ArtworkRequirement) -> bool {
        !requirement.needs_ambient_texture() || self.ambient_texture.is_some()
    }

    /// RGBA payload retained by the two textures, for the presentation cache budget.
    pub(super) fn storage_bytes(&self) -> usize {
        let rgba_bytes = |texture: &gdk::MemoryTexture| {
            (texture.width() as usize)
                .saturating_mul(texture.height() as usize)
                .saturating_mul(4)
        };
        rgba_bytes(&self.texture)
            .saturating_add(self.ambient_texture.as_ref().map_or(0, rgba_bytes))
            .saturating_add(self.source.encoded().len())
    }
}

/// GTK-free output produced from artwork on a blocking worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ArtworkVisuals {
    background: Background,
    ambient: Option<AmbientImage>,
}

impl ArtworkVisuals {
    /// Supplies a deterministic neutral result if background preparation panics.
    pub(super) fn fallback() -> Self {
        let background = Background::fallback();
        Self {
            background,
            ambient: Some(AmbientImage {
                width: 1,
                height: 1,
                rgba: vec![background.top.0, background.top.1, background.top.2, 255],
            }),
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
pub(super) fn visuals_from_artwork(
    artwork: &Artwork,
    requirement: ArtworkRequirement,
) -> ArtworkVisuals {
    let image =
        ImageBuffer::<Rgba<u8>, _>::from_raw(artwork.width(), artwork.height(), artwork.rgba())
            .expect("validated artwork has a complete RGBA pixel buffer");
    let background = generate_background(&image);

    ArtworkVisuals {
        background,
        ambient: requirement
            .needs_ambient_texture()
            .then(|| generate_ambient_image(&image, background)),
    }
}

/// Creates GTK textures on the main thread after worker-side visual preparation.
pub(super) fn prepare_artwork(
    artwork: &Arc<Artwork>,
    visuals: ArtworkVisuals,
    previous: Option<&PreparedArtwork>,
) -> PreparedArtwork {
    let ArtworkVisuals {
        background,
        ambient,
    } = visuals;
    let ambient_texture = ambient.map(memory_texture_from_ambient);
    let previous = previous.filter(|previous| previous.matches(artwork));

    PreparedArtwork {
        texture: previous.map_or_else(
            || crate::gui::artwork::texture(artwork),
            |previous| previous.texture.clone(),
        ),
        ambient_texture: ambient_texture
            .or_else(|| previous.and_then(|previous| previous.ambient_texture.clone())),
        background,
        source: artwork.clone(),
    }
}

/// Creates the blurred texture used when an artist image supplies the
/// Cinema/Ambient background. This intentionally creates no sharp texture.
pub(super) fn prepare_immersive_background(
    artwork: &Arc<Artwork>,
    visuals: ArtworkVisuals,
) -> PreparedImmersiveBackground {
    let ArtworkVisuals {
        background,
        ambient,
    } = visuals;
    let ambient = ambient.unwrap_or_else(|| {
        ArtworkVisuals::fallback()
            .ambient
            .expect("fallback visuals always contain an ambient image")
    });

    PreparedImmersiveBackground {
        texture: memory_texture_from_ambient(ambient),
        background,
        source_id: artwork.content_id(),
    }
}

fn memory_texture_from_ambient(ambient: AmbientImage) -> gdk::MemoryTexture {
    let ambient_stride = ambient.width as usize * 4;
    let ambient_bytes = glib::Bytes::from_owned(ambient.rgba);
    gdk::MemoryTexture::new(
        i32::try_from(ambient.width).expect("bounded ambient width fits i32"),
        i32::try_from(ambient.height).expect("bounded ambient height fits i32"),
        gdk::MemoryFormat::R8g8b8a8,
        &ambient_bytes,
        ambient_stride,
    )
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
    let mut thumbnail = if image.dimensions() == (width, height) {
        // No resampling is needed for covers already within the ambient bound.
        ImageBuffer::from_raw(width, height, image.as_raw().to_vec())
            .expect("source image has a complete RGBA pixel buffer")
    } else {
        image::imageops::thumbnail(image, width, height)
    };

    // Album covers are normally opaque, but compositing transparent pixels
    // before blurring avoids dark color fringes and guarantees an opaque GTK
    // background texture.
    for pixel in thumbnail.pixels_mut() {
        let [red, green, blue, alpha] = pixel.0;
        if alpha == 255 {
            continue;
        }
        let alpha = f32::from(alpha) / 255.0;
        pixel.0 = [
            composite_channel(red, background.top.0, alpha),
            composite_channel(green, background.top.1, alpha),
            composite_channel(blue, background.top.2, alpha),
            255,
        ];
    }

    // DynamicImage selects the packed-u8 Gaussian implementation. The generic
    // imageops route expands RGBA into two full-size f32 scratch buffers first.
    let mut ambient = DynamicImage::ImageRgba8(thumbnail)
        .blur(AMBIENT_BLUR_SIGMA)
        .into_rgba8();
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
        let channels = [red, green, blue].map(|channel| f32::from(channel) / 255.0);
        let maximum = channels[0].max(channels[1]).max(channels[2]);
        let minimum = channels[0].min(channels[1]).min(channels[2]);
        let chroma = maximum - minimum;
        let lightness = (maximum + minimum) / 2.0;
        let colorfulness = if maximum <= f32::EPSILON {
            0.0
        } else {
            chroma / maximum
        };
        let lightness = (lightness * AMBIENT_LIGHTNESS_MULTIPLIER).min(AMBIENT_MAX_LIGHTNESS);
        let available_chroma = 1.0 - (2.0 * lightness - 1.0).abs();
        let target_chroma = (chroma * colorfulness_from_saturation(colorfulness))
            .min(available_chroma * AMBIENT_MAX_SATURATION);
        let chroma_scale = if chroma <= f32::EPSILON {
            0.0
        } else {
            target_chroma / chroma
        };
        let offset = lightness - target_chroma / 2.0;
        // Scaling RGB around its minimum preserves hue without the per-pixel
        // RGB -> HSL -> RGB conversion and its hue divisions/remainders.
        let [red, green, blue] = channels
            .map(|channel| unit_channel_to_byte((channel - minimum) * chroma_scale + offset));

        pixel.0 = if AMBIENT_VIGNETTE_STRENGTH == 0.0 {
            // A disabled vignette must not calculate radial coordinates and
            // round three already-rounded channels for every background pixel.
            [red, green, blue, 255]
        } else {
            let normalized_x = ((x as f32 + 0.5) / width) * 2.0 - 1.0;
            let normalized_y = ((y as f32 + 0.5) / height) * 2.0 - 1.0;
            let edge_distance =
                ((normalized_x * normalized_x + normalized_y * normalized_y) / 2.0).clamp(0.0, 1.0);
            let vignette = 1.0 - AMBIENT_VIGNETTE_STRENGTH * edge_distance.sqrt();
            [
                (f32::from(red) * vignette).round() as u8,
                (f32::from(green) * vignette).round() as u8,
                (f32::from(blue) * vignette).round() as u8,
                255,
            ]
        };
    }
}

/// Round a finite unit channel without a per-channel libm round call. Subtracting
/// the integer part preserves the exact half-way test (adding 0.5 first can
/// incorrectly round values immediately below a half-way boundary).
fn unit_channel_to_byte(channel: f32) -> u8 {
    let scaled = channel.clamp(0.0, 1.0) * 255.0;
    let integral = scaled as u8;
    integral + u8::from(scaled - f32::from(integral) >= 0.5)
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
    colorfulness_from_saturation(hsv_saturation(rgb))
}

fn colorfulness_from_saturation(saturation: f32) -> f32 {
    let progress = ((saturation - NEUTRAL_COLORFULNESS_START)
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
    #[test]
    #[ignore = "release-only timing probe; run with --nocapture, not as a timing assertion"]
    fn artwork_pipeline_stage_timings() {
        use std::hint::black_box;
        use std::time::Instant;
        assert!(!cfg!(debug_assertions), "measure the release build");
        // Break down the most expensive preparation stage separately, without
        // changing the production path or including fixture construction.
        let source = ImageBuffer::from_fn(1600, 1600, |x, y| {
            Rgba([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, 255])
        });
        for _ in 0..3 {
            let start = Instant::now();
            let thumbnail = image::imageops::thumbnail(
                &source,
                AMBIENT_MAXIMUM_DIMENSION,
                AMBIENT_MAXIMUM_DIMENSION,
            );
            let resized = start.elapsed();
            let start = Instant::now();
            let mut blurred = DynamicImage::ImageRgba8(thumbnail)
                .blur(AMBIENT_BLUR_SIGMA)
                .into_rgba8();
            let blur = start.elapsed();
            let start = Instant::now();
            apply_ambient_tone(&mut blurred);
            eprintln!(
                "ambient breakdown: resize={resized:?}, blur={blur:?}, tone={:?}",
                start.elapsed()
            );
            black_box(blurred);
        }
        for size in [400, 1600] {
            let image = image::RgbImage::from_fn(size, size, |x, y| {
                image::Rgb([
                    ((x * 173 / size + y * 71 / size) % 256) as u8,
                    ((x * 53 / size + y * 199 / size) % 256) as u8,
                    ((x * 109 / size + y * 47 / size) % 256) as u8,
                ])
            });
            for format in [image::ImageFormat::Jpeg, image::ImageFormat::Png] {
                let mut encoded = std::io::Cursor::new(Vec::new());
                image.write_to(&mut encoded, format).unwrap();
                let encoded = encoded.into_inner();
                let mut samples = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
                let mut payload_bytes = 0;
                for iteration in 0..9 {
                    let start = Instant::now();
                    let artwork = std::sync::Arc::new(
                        crate::core::artwork::Artwork::decode(encoded.clone()).unwrap(),
                    );
                    let decoded = start.elapsed();
                    let start = Instant::now();
                    let classic =
                        super::visuals_from_artwork(&artwork, super::ArtworkRequirement::Cover);
                    let classic_time = start.elapsed();
                    let start = Instant::now();
                    // This is also the pre-refactor eager Classic workload, with
                    // exactly the same image treatment and tuning values.
                    let immersive =
                        super::visuals_from_artwork(&artwork, super::ArtworkRequirement::Immersive);
                    let immersive_time = start.elapsed();
                    let start = Instant::now();
                    let cover = super::prepare_artwork(&artwork, classic, None);
                    let prepared = super::prepare_artwork(&artwork, immersive, Some(&cover));
                    let texture_time = start.elapsed();
                    payload_bytes = prepared.storage_bytes();
                    black_box(prepared);
                    if iteration > 0 {
                        for (samples, elapsed) in samples.iter_mut().zip([
                            decoded,
                            classic_time,
                            immersive_time,
                            texture_time,
                        ]) {
                            samples.push(elapsed.as_secs_f64() * 1000.0);
                        }
                    }
                }
                let median = |values: &mut Vec<f64>| {
                    values.sort_by(f64::total_cmp);
                    (values[3] + values[4]) / 2.0
                };
                let timings = samples.each_mut().map(median);
                eprintln!(
                    "{size}x{size} {format:?}: decode+identity={:.3}ms, Classic={:.3}ms, eager/immersive={:.3}ms, texture-create+upgrade={:.3}ms, retained-payload={} bytes",
                    timings[0], timings[1], timings[2], timings[3], payload_bytes
                );
            }
        }
    }

    use super::{
        AMBIENT_BLUR_SIGMA, AMBIENT_LIGHTNESS_MULTIPLIER, AMBIENT_MAX_LIGHTNESS,
        AMBIENT_MAX_SATURATION, AMBIENT_MAXIMUM_DIMENSION, ArtworkVisuals, Background,
        apply_ambient_tone, colorfulness_strength, contrast_ratio, generate_ambient_image,
        generate_background, hsl_to_rgb, hsv_saturation, rgb_chroma, rgb_to_hsl,
        thumbnail_dimensions, visuals_from_artwork,
    };
    use crate::core::artwork::Artwork;
    use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba};
    use std::io::Cursor;

    #[test]
    fn artist_background_is_required_only_when_selected_in_an_immersive_mode() {
        use crate::core::preferences::{
            DisplayMode, ImmersiveBackgroundSource, NowPlayingPreferences,
            SharedNowPlayingPreferences,
        };

        for mode in DisplayMode::ALL {
            for source in [
                ImmersiveBackgroundSource::AlbumCover,
                ImmersiveBackgroundSource::Artist,
            ] {
                let settings = NowPlayingPreferences {
                    display_mode: mode,
                    shared: SharedNowPlayingPreferences {
                        immersive_background_source: source,
                        ..SharedNowPlayingPreferences::default()
                    },
                    ..NowPlayingPreferences::default()
                };
                assert_eq!(
                    super::ArtworkRequirement::for_settings(settings).needs_artist_background(),
                    matches!(mode, DisplayMode::Cinema | DisplayMode::Ambient)
                        && source == ImmersiveBackgroundSource::Artist
                );
            }
        }
    }

    fn channel_range(channels: [u8; 3]) -> u8 {
        channels.iter().max().unwrap() - channels.iter().min().unwrap()
    }

    #[test]
    fn channel_rounding_preserves_halfway_boundaries() {
        let check = |channel: f32| {
            assert_eq!(
                super::unit_channel_to_byte(channel),
                (channel.clamp(0.0, 1.0) * 255.0).round() as u8,
                "rounding changed for {channel}"
            );
        };
        for byte in 0..255 {
            let midpoint = (byte as f32 + 0.5) / 255.0;
            for bits in midpoint.to_bits() - 2..=midpoint.to_bits() + 2 {
                check(f32::from_bits(bits));
            }
        }
        for value in -100..=100_100 {
            check(value as f32 / 100_000.0);
        }
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
            visuals_from_artwork(&artwork, super::ArtworkRequirement::Cover).background,
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
    fn ambient_rgb_tone_matches_the_hsl_treatment_within_rounding() {
        // Include black/white, nearly neutral colors, and both sides of byte
        // midpoints to exercise low-chroma and saturation-clamped colors.
        let levels = [0, 1, 16, 32, 64, 96, 127, 128, 160, 192, 224, 254, 255];
        for red in levels {
            let mut image =
                ImageBuffer::from_fn(levels.len() as u32, levels.len() as u32, |x, y| {
                    Rgba([red, levels[x as usize], levels[y as usize], 255])
                });
            apply_ambient_tone(&mut image);
            for (x, y, actual) in image.enumerate_pixels() {
                let source = (red, levels[x as usize], levels[y as usize]);
                let (hue, _, lightness) = rgb_to_hsl(source);
                let lightness =
                    (lightness * AMBIENT_LIGHTNESS_MULTIPLIER).min(AMBIENT_MAX_LIGHTNESS);
                let available_chroma = 1.0 - (2.0 * lightness - 1.0).abs();
                let saturation = if available_chroma <= f32::EPSILON {
                    0.0
                } else {
                    (rgb_chroma(source) * colorfulness_strength(source) / available_chroma)
                        .min(AMBIENT_MAX_SATURATION)
                };
                let (r, g, b) = hsl_to_rgb((hue, saturation, lightness));
                for (actual, expected) in actual.0.into_iter().zip([r, g, b, 255]) {
                    assert!(
                        actual.abs_diff(expected) <= 1,
                        "tone changed for {source:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn packed_gaussian_preserves_the_generic_background_treatment() {
        // Odd dimensions and abrupt color changes exercise filter edges as
        // well as its interior; both paths use the same Gaussian sigma.
        let image = ImageBuffer::from_fn(97, 67, |x, y| {
            Rgba([
                ((x * 13 + y * 7) % 256) as u8,
                ((x * 3 + y * 19) % 256) as u8,
                ((x * 23 + y * 5) % 256) as u8,
                255,
            ])
        });
        let mut reference = image::imageops::blur(&image, AMBIENT_BLUR_SIGMA);
        apply_ambient_tone(&mut reference);
        let actual = generate_ambient_image(&image, Background::fallback());

        assert_eq!((actual.width, actual.height), reference.dimensions());
        for (actual, expected) in actual.rgba.iter().zip(reference.as_raw()) {
            assert!(
                actual.abs_diff(*expected) <= 2,
                "Gaussian treatment changed"
            );
        }
    }

    #[test]
    fn classic_preparation_does_not_generate_an_immersive_background() {
        let image = image::DynamicImage::new_rgb8(48, 48);
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let artwork = crate::core::artwork::Artwork::decode(bytes.into_inner()).unwrap();
        let cover = super::visuals_from_artwork(&artwork, super::ArtworkRequirement::Cover);
        let immersive = super::visuals_from_artwork(&artwork, super::ArtworkRequirement::Immersive);
        assert!(cover.ambient.is_none());
        assert!(immersive.ambient.is_some());
        assert_eq!(cover.background, immersive.background);
    }

    #[test]
    fn fallback_visuals_have_an_opaque_neutral_ambient_image() {
        let visuals = ArtworkVisuals::fallback();

        assert_eq!(visuals.background, Background::fallback());
        let ambient = visuals.ambient.unwrap();
        assert_eq!((ambient.width, ambient.height), (1, 1));
        assert_eq!(ambient.rgba, vec![38, 38, 38, 255]);
    }
}
