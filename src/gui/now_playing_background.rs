//! Artwork-derived background color extraction for the Now Playing window.

type RGB = (u8, u8, u8);
type HSL = (f32, f32, f32);

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Background {
    pub(crate) top: RGB,
    pub(crate) bottom: RGB,
}

impl Background {
    pub(crate) fn fallback() -> Self {
        Self {
            top: (38, 38, 38),
            bottom: (0, 0, 0),
        }
    }
}

/// Builds a dark, artwork-derived background from a cover-image byte stream.
pub(crate) fn from_cover_image(bytes: &[u8]) -> Background {
    image::load_from_memory(bytes)
        .map(|image| generate_cover_background(&image))
        .unwrap_or_else(|_| Background::fallback())
}

fn generate_cover_background(image: &image::DynamicImage) -> Background {
    let small = image.thumbnail(72, 72).to_rgb8();

    // Quantize into compact RGB buckets while keeping weighted sums so the
    // chosen color is still representative of the source art.
    #[derive(Default, Clone, Copy)]
    struct Bucket {
        weight: f32,
        red: f32,
        green: f32,
        blue: f32,
    }

    let mut buckets = std::collections::HashMap::<u32, Bucket>::new();

    for (x, y, pixel) in small.enumerate_pixels() {
        let [red, green, blue] = pixel.0;
        let rf = red as f32 / 255.0;
        let gf = green as f32 / 255.0;
        let bf = blue as f32 / 255.0;

        let (_, saturation, _) = rgb_to_hsl((red, green, blue));
        let luminance = relative_luminance(rf, gf, bf);

        // Skip colors that are effectively white or black. Neither gives us a
        // useful accent, and the former is especially bad with white text.
        if luminance > 0.92 || luminance < 0.018 {
            continue;
        }

        // Rich colors should contribute more than grayish colors, while colors
        // around the middle of the luminance range are generally better UI
        // accents than highlights or deep shadows.
        let saturation_weight = 0.35 + saturation.powf(1.35) * 3.5;
        let luminance_weight = (1.0 - ((luminance - 0.38).abs() / 0.38)).clamp(0.15, 1.0);

        // Slightly favor the center of the artwork. This helps avoid picking
        // a tiny bright corner or border as the whole application's mood.
        let nx = (x as f32 + 0.5) / small.width() as f32;
        let ny = (y as f32 + 0.5) / small.height() as f32;
        let center_distance = ((nx - 0.5).powi(2) + (ny - 0.5).powi(2)).sqrt();
        let spatial_weight = (1.15 - center_distance).clamp(0.55, 1.15);

        // A tiny hue-distance factor prevents almost-black/gray colors from
        // winning simply because there are many of them.
        let chroma_weight = if saturation < 0.06 { 0.55 } else { 1.0 };
        let weight = saturation_weight * luminance_weight * spatial_weight * chroma_weight;

        let qr = red >> 4;
        let qg = green >> 4;
        let qb = blue >> 4;
        let key = ((qr as u32) << 8) | ((qg as u32) << 4) | qb as u32;

        let bucket = buckets.entry(key).or_default();
        bucket.weight += weight;
        bucket.red += rf * weight;
        bucket.green += gf * weight;
        bucket.blue += bf * weight;
    }

    let candidate = buckets.into_values().max_by(|a, b| {
        a.weight
            .partial_cmp(&b.weight)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let Some(bucket) = candidate else {
        return Background::fallback();
    };

    if bucket.weight <= f32::EPSILON {
        return Background::fallback();
    }

    let red = bucket.red / bucket.weight;
    let green = bucket.green / bucket.weight;
    let blue = bucket.blue / bucket.weight;
    let (hue, saturation, _) = rgb_to_hsl((
        (red * 255.0).round() as u8,
        (green * 255.0).round() as u8,
        (blue * 255.0).round() as u8,
    ));

    // A modest saturation floor gives colorful covers a rich mood without
    // inventing a strong hue for genuinely grayscale artwork.
    let target_saturation = if saturation < 0.08 {
        0.0
    } else {
        (saturation * 1.08).clamp(0.22, 0.70)
    };

    // Start with a dark accent, then move it darker as needed
    // until white text has a genuine WCAG-style contrast margin.
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

    // The lower stop keeps the same hue, but is much darker and less saturated.
    // This makes the metadata area quiet and preserves readable white text.
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

/**
 * Converts an RGB color into HSL space for palette extraction and contrast tuning.
 */
fn rgb_to_hsl(rgb: RGB) -> HSL {
    let r = rgb.0 as f32 / 255.0;
    let g = rgb.1 as f32 / 255.0;
    let b = rgb.2 as f32 / 255.0;

    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let lightness = (max + min) / 2.0;
    let delta = max - min;

    if delta <= f32::EPSILON {
        return (0.0, 0.0, lightness);
    }

    let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
    let hue = if max == r {
        ((g - b) / delta).rem_euclid(6.0) / 6.0
    } else if max == g {
        (((b - r) / delta) + 2.0) / 6.0
    } else {
        (((r - g) / delta) + 4.0) / 6.0
    };

    (hue, saturation, lightness)
}

/**
 * Converts HSL values back into an 8-bit RGB triplet for the generated window background.
 */
fn hsl_to_rgb(hsl: HSL) -> RGB {
    let (h, s, l) = hsl;
    let chroma = (1.0_f32 - (2.0_f32 * l - 1.0_f32).abs()) * s;
    let x = chroma * (1.0 - ((h * 6.0).rem_euclid(2.0) - 1.0).abs());
    let m = l - chroma / 2.0;

    let (r1, g1, b1) = match (h * 6.0).floor() as i32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };

    (
        ((r1 + m).clamp(0.0, 1.0) * 255.0).round() as u8,
        ((g1 + m).clamp(0.0, 1.0) * 255.0).round() as u8,
        ((b1 + m).clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

/**
 * Computes the relative luminance used to keep text contrast high against cover art.
 */
fn relative_luminance(r: f32, g: f32, b: f32) -> f32 {
    fn linearize(channel: f32) -> f32 {
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    }

    0.2126 * linearize(r) + 0.7152 * linearize(g) + 0.0722 * linearize(b)
}

/**
 * Returns the WCAG-style contrast ratio between two RGB colors.
 */
fn contrast_ratio(first: RGB, second: RGB) -> f32 {
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
