//! Background rendering, gradient caching, and responsive metadata typography.

use super::palette::Background;
use super::style::font_css_for_size;
use super::ui::{CinemaFraming, configure_classic_content, configure_immersive_info};
use super::{BackgroundStyle, DisplayMode, NowPlayingWindow};
use adw::prelude::*;
use cairo::{Context, Format, ImageSurface, LinearGradient};
use std::cell::RefCell;

const GRADIENT_SURFACE_WIDTH: i32 = 256;
const TRANSITION_START: f64 = 0.20;
const AMBIENT_BASE_SCRIM_ALPHA: f64 = 0.22;
const AMBIENT_TOP_SCRIM_ALPHA: f64 = 0.08;
const AMBIENT_BOTTOM_SCRIM_ALPHA: f64 = 0.14;

#[derive(Debug, Clone)]
pub(super) struct CachedGradient {
    background: Background,
    height: i32,
    surface: ImageSurface,
}

impl NowPlayingWindow {
    /// Connects background drawing and resize handling for the window.
    pub(super) fn setup_rendering(&self, text_css: &gtk::CssProvider) {
        let background_state_for_draw = self.state.current_background.clone();
        let settings_for_draw = self.state.settings.clone();
        let gradient_surface_for_draw = self.state.gradient_surface.clone();
        self.ui
            .background_area
            .set_draw_func(move |_, context, width, height| {
                let settings = settings_for_draw.get();
                draw_background(
                    context,
                    width,
                    height,
                    background_state_for_draw.get(),
                    settings.classic.background_style,
                    settings.display_mode,
                    &gradient_surface_for_draw,
                );
            });

        let settings_for_scrim = self.state.settings.clone();
        let cinema_for_scrim = self.ui.cinema_artwork.clone();
        self.ui
            .scrim_area
            .set_draw_func(move |_, context, width, height| {
                draw_immersive_scrim(
                    context,
                    width,
                    height,
                    settings_for_scrim.get().display_mode,
                    cinema_for_scrim.framing(width, height),
                );
            });

        let text_css_for_resize = text_css.clone();
        let gradient_surface_for_resize = self.state.gradient_surface.clone();
        let background_state_for_resize = self.state.current_background.clone();
        let settings_for_resize = self.state.settings.clone();
        let cinema_for_resize = self.ui.cinema_artwork.clone();
        let scrim_for_resize = self.ui.scrim_area.clone();
        let classic_content_for_resize = self.ui.classic_content.clone();
        let immersive_info_for_resize = self.ui.immersive_info_box.clone();
        let immersive_title_for_resize = self.ui.immersive_title_label.clone();
        let immersive_artist_for_resize = self.ui.immersive_artist_label.clone();
        let immersive_album_for_resize = self.ui.immersive_album_label.clone();
        let immersive_details_for_resize = self.ui.immersive_details_label.clone();
        self.ui
            .background_area
            .connect_resize(move |area, width, height| {
                if width <= 0 || height <= 0 {
                    return;
                }

                text_css_for_resize.load_from_string(&font_css_for_size((width, height)));
                configure_classic_content(&classic_content_for_resize, width, height);

                let settings = settings_for_resize.get();
                let (background, style) = effective_background(
                    background_state_for_resize.get(),
                    settings.classic.background_style,
                    settings.display_mode,
                );
                if matches!(style, BackgroundStyle::Gradient) {
                    rebuild_gradient_surface(&gradient_surface_for_resize, background, height);
                    area.queue_draw();
                }
                cinema_for_resize.container.queue_allocate();
                configure_immersive_info(
                    &immersive_info_for_resize,
                    [
                        &immersive_title_for_resize,
                        &immersive_artist_for_resize,
                        &immersive_album_for_resize,
                        &immersive_details_for_resize,
                    ],
                    settings.display_mode,
                    cinema_for_resize.framing(width, height),
                    width,
                    height,
                );
                scrim_for_resize.queue_draw();
            });
    }

    /// Applies the mode-aware background underneath artwork layers.
    pub(super) fn apply_background(&self) {
        let settings = self.state.settings.get();
        redraw_background(
            &self.ui.background_area,
            &self.state.gradient_surface,
            self.state.current_background.get(),
            settings.classic.background_style,
            settings.display_mode,
        );
        self.ui.scrim_area.queue_draw();
    }

    /// Selects the solid or gradient background rendering style.
    pub(super) fn set_background_style(&self, style: BackgroundStyle) {
        self.with_preference_updates_suspended(|| match style {
            BackgroundStyle::Gradient => {
                if !self.controls.background_style_gradient.is_active() {
                    self.controls.background_style_gradient.set_active(true);
                }
            }
            BackgroundStyle::Solid => {
                if !self.controls.background_style_solid.is_active() {
                    self.controls.background_style_solid.set_active(true);
                }
            }
        });
        self.apply_background();
    }
}

/// Rebuilds the gradient cache when needed and requests the next background paint.
///
/// Both direct preference changes and delayed track updates use this operation so
/// they apply the same mode override and gradient-cache invalidation rules.
pub(super) fn redraw_background(
    area: &gtk::DrawingArea,
    cache: &RefCell<Option<CachedGradient>>,
    background: Background,
    style: BackgroundStyle,
    display_mode: DisplayMode,
) {
    let (background, style) = effective_background(background, style, display_mode);
    if matches!(style, BackgroundStyle::Gradient) {
        rebuild_gradient_surface(cache, background, area.height());
    }
    area.queue_draw();
}

/// Resolves the fallback beneath each presentation mode.
fn effective_background(
    background: Background,
    style: BackgroundStyle,
    display_mode: DisplayMode,
) -> (Background, BackgroundStyle) {
    match display_mode {
        DisplayMode::Classic => (background, style),
        DisplayMode::Cinema | DisplayMode::Ambient => (background, BackgroundStyle::Gradient),
        DisplayMode::LightsOff => (
            Background {
                top: (0, 0, 0),
                bottom: (0, 0, 0),
            },
            BackgroundStyle::Solid,
        ),
    }
}

/// Paints the current background into the supplied Cairo context.
fn draw_background(
    context: &Context,
    width: i32,
    height: i32,
    background: Background,
    style: BackgroundStyle,
    display_mode: DisplayMode,
    cache: &RefCell<Option<CachedGradient>>,
) {
    if width <= 0 || height <= 0 {
        return;
    }

    let (background, style) = effective_background(background, style, display_mode);

    if matches!(style, BackgroundStyle::Solid) {
        context.set_source_rgb(
            f64::from(background.top.0) / 255.0,
            f64::from(background.top.1) / 255.0,
            f64::from(background.top.2) / 255.0,
        );
        let _ = context.paint();
        return;
    }

    let needs_rebuild = cache
        .borrow()
        .as_ref()
        .map(|cached| cached.background != background || cached.height != height)
        .unwrap_or(true);

    if needs_rebuild {
        rebuild_gradient_surface(cache, background, height);
    }

    let guard = cache.borrow();
    let Some(cached) = guard.as_ref() else {
        return;
    };

    if let Err(error) = context.save() {
        log::warn!("Failed to save Cairo state for gradient: {error}");
        return;
    }

    context.scale(f64::from(width) / f64::from(GRADIENT_SURFACE_WIDTH), 1.0);

    if let Err(error) = context.set_source_surface(&cached.surface, 0.0, 0.0) {
        log::warn!("Failed to set cached gradient surface: {error}");
        let _ = context.restore();
        return;
    }

    if let Err(error) = context.paint() {
        log::warn!("Failed to paint cached gradient surface: {error}");
    }
    let _ = context.restore();
}

/// Paints neutral black scrims that keep white metadata readable without tinting it.
fn draw_immersive_scrim(
    context: &Context,
    width: i32,
    height: i32,
    display_mode: DisplayMode,
    framing: CinemaFraming,
) {
    if width <= 0 || height <= 0 {
        return;
    }

    match display_mode {
        DisplayMode::Classic | DisplayMode::LightsOff => return,
        DisplayMode::Ambient => {
            context.set_source_rgba(0.0, 0.0, 0.0, AMBIENT_BASE_SCRIM_ALPHA);
            let _ = context.paint();

            let gradient = LinearGradient::new(0.0, 0.0, 0.0, f64::from(height));
            gradient.add_color_stop_rgba(0.0, 0.0, 0.0, 0.0, AMBIENT_TOP_SCRIM_ALPHA);
            gradient.add_color_stop_rgba(0.5, 0.0, 0.0, 0.0, 0.0);
            gradient.add_color_stop_rgba(1.0, 0.0, 0.0, 0.0, AMBIENT_BOTTOM_SCRIM_ALPHA);
            if context.set_source(gradient).is_ok() {
                let _ = context.paint();
            }
        }
        DisplayMode::Cinema => {
            context.set_source_rgba(0.0, 0.0, 0.0, 0.12);
            let _ = context.paint();

            let gradient = if height > width {
                let gradient = LinearGradient::new(0.0, 0.0, 0.0, f64::from(height) * 0.60);
                gradient.add_color_stop_rgba(0.0, 0.0, 0.0, 0.0, 0.82);
                gradient.add_color_stop_rgba(0.40, 0.0, 0.0, 0.0, 0.34);
                gradient.add_color_stop_rgba(1.0, 0.0, 0.0, 0.0, 0.0);
                gradient
            } else {
                match framing {
                    CinemaFraming::Wide => {
                        let gradient = LinearGradient::new(0.0, 0.0, f64::from(width) * 0.68, 0.0);
                        gradient.add_color_stop_rgba(0.0, 0.0, 0.0, 0.0, 0.78);
                        gradient.add_color_stop_rgba(0.72, 0.0, 0.0, 0.0, 0.30);
                        gradient.add_color_stop_rgba(1.0, 0.0, 0.0, 0.0, 0.0);
                        gradient
                    }
                    CinemaFraming::Cover | CinemaFraming::Tall => {
                        let gradient = LinearGradient::new(
                            0.0,
                            f64::from(height) * 0.40,
                            0.0,
                            f64::from(height),
                        );
                        gradient.add_color_stop_rgba(0.0, 0.0, 0.0, 0.0, 0.0);
                        gradient.add_color_stop_rgba(0.60, 0.0, 0.0, 0.0, 0.34);
                        gradient.add_color_stop_rgba(1.0, 0.0, 0.0, 0.0, 0.82);
                        gradient
                    }
                }
            };
            if context.set_source(gradient).is_ok() {
                let _ = context.paint();
            }
        }
    }
}

/// Rebuilds and caches the vertical gradient surface when its inputs change.
pub(super) fn rebuild_gradient_surface(
    cache: &RefCell<Option<CachedGradient>>,
    background: Background,
    height: i32,
) {
    if height <= 0 {
        return;
    }

    if cache
        .borrow()
        .as_ref()
        .map(|cached| cached.background == background && cached.height == height)
        .unwrap_or(false)
    {
        return;
    }

    let Ok(mut surface) = ImageSurface::create(Format::ARgb32, GRADIENT_SURFACE_WIDTH, height)
    else {
        log::warn!("Failed to create cached gradient surface");
        return;
    };

    let stride = surface.stride() as usize;
    let width = GRADIENT_SURFACE_WIDTH as usize;
    let top = srgb_triplet_to_linear(background.top);
    let bottom = srgb_triplet_to_linear(background.bottom);

    let Ok(mut data) = surface.data() else {
        log::warn!("Failed to access cached gradient surface data");
        return;
    };

    for y in 0..height as usize {
        let position = if height <= 1 {
            1.0
        } else {
            y as f64 / f64::from(height - 1)
        };
        let t = ((position - TRANSITION_START) / (1.0 - TRANSITION_START)).clamp(0.0, 1.0);
        let t = t * t * (3.0 - 2.0 * t);

        let red = linear_to_srgb(top.0 + (bottom.0 - top.0) * t) * 255.0;
        let green = linear_to_srgb(top.1 + (bottom.1 - top.1) * t) * 255.0;
        let blue = linear_to_srgb(top.2 + (bottom.2 - top.2) * t) * 255.0;

        for x in 0..width {
            // Unbiased stochastic rounding removes visible 8-bit steps without
            // introducing a repeating Bayer/diamond pattern. The cached surface
            // is tiny compared with the actual window and is generated only when
            // the background or window height changes.
            let noise = hash_noise(x as u32, y as u32);
            let r = stochastic_round(red, noise);
            let g = stochastic_round(green, noise);
            let b = stochastic_round(blue, noise);
            let offset = y * stride + x * 4;
            data[offset..offset + 4].copy_from_slice(&argb32_pixel_bytes(r, g, b));
        }
    }
    drop(data);
    surface.flush();

    *cache.borrow_mut() = Some(CachedGradient {
        background,
        height,
        surface,
    });
}

/// Converts an sRGB RGB triplet into linear-light RGB components.
fn srgb_triplet_to_linear(rgb: (u8, u8, u8)) -> (f64, f64, f64) {
    (
        srgb_to_linear(f64::from(rgb.0) / 255.0),
        srgb_to_linear(f64::from(rgb.1) / 255.0),
        srgb_to_linear(f64::from(rgb.2) / 255.0),
    )
}

/// Converts one normalized sRGB channel to linear-light space.
fn srgb_to_linear(channel: f64) -> f64 {
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

/// Converts one normalized linear-light channel back to sRGB space.
fn linear_to_srgb(channel: f64) -> f64 {
    let channel = channel.clamp(0.0, 1.0);
    if channel <= 0.0031308 {
        channel * 12.92
    } else {
        1.055 * channel.powf(1.0 / 2.4) - 0.055
    }
}

/// Applies unbiased stochastic rounding to an 8-bit channel value.
fn stochastic_round(value: f64, noise: f64) -> u8 {
    let value = value.clamp(0.0, 255.0);
    let floor = value.floor();
    let fraction = value - floor;
    let rounded = if noise < fraction { floor + 1.0 } else { floor };
    rounded.clamp(0.0, 255.0) as u8
}

/// Produces deterministic pseudo-random noise in the range `[0, 1]` for dithering.
fn hash_noise(x: u32, y: u32) -> f64 {
    let mut value = x
        .wrapping_mul(0x9E37_79B9)
        .wrapping_add(y.wrapping_mul(0x85EB_CA6B));
    value ^= value >> 16;
    value = value.wrapping_mul(0x7FEB_352D);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846C_A68B);
    value ^= value >> 16;
    f64::from(value) / f64::from(u32::MAX)
}

/// Encodes an opaque RGB pixel in Cairo's native-endian `ARgb32` representation.
fn argb32_pixel_bytes(red: u8, green: u8, blue: u8) -> [u8; 4] {
    (0xff00_0000 | (u32::from(red) << 16) | (u32::from(green) << 8) | u32::from(blue)).to_ne_bytes()
}

#[cfg(test)]
mod tests {
    use super::{
        AMBIENT_BASE_SCRIM_ALPHA, argb32_pixel_bytes, draw_immersive_scrim, effective_background,
        srgb_to_linear,
    };
    use crate::gui::now_playing_window::palette::Background;
    use crate::gui::now_playing_window::ui::AMBIENT_FOREGROUND_OPACITY;
    use crate::gui::now_playing_window::{BackgroundStyle, DisplayMode};

    #[test]
    fn argb32_pixels_follow_cairos_native_endian_layout() {
        assert_eq!(
            argb32_pixel_bytes(0x12, 0x34, 0x56),
            0xff12_3456_u32.to_ne_bytes()
        );
    }

    #[test]
    fn modes_resolve_only_the_background_styles_that_apply_to_them() {
        let artwork_background = Background {
            top: (10, 20, 30),
            bottom: (40, 50, 60),
        };

        assert_eq!(
            effective_background(
                artwork_background,
                BackgroundStyle::Gradient,
                DisplayMode::Classic
            ),
            (artwork_background, BackgroundStyle::Gradient)
        );
        assert_eq!(
            effective_background(
                artwork_background,
                BackgroundStyle::Solid,
                DisplayMode::Classic
            ),
            (artwork_background, BackgroundStyle::Solid)
        );
        assert_eq!(
            effective_background(
                artwork_background,
                BackgroundStyle::Solid,
                DisplayMode::Ambient
            ),
            (artwork_background, BackgroundStyle::Gradient)
        );
        assert_eq!(
            effective_background(
                artwork_background,
                BackgroundStyle::Gradient,
                DisplayMode::LightsOff
            ),
            (
                Background {
                    top: (0, 0, 0),
                    bottom: (0, 0, 0),
                },
                BackgroundStyle::Solid
            )
        );
    }

    #[test]
    fn ambient_uniform_scrim_keeps_white_text_readable_over_white_artwork() {
        let toned_white = 0.50;
        let original_white = 1.0;
        let artwork = toned_white * (1.0 - AMBIENT_FOREGROUND_OPACITY)
            + original_white * AMBIENT_FOREGROUND_OPACITY;
        let composited = artwork * (1.0 - AMBIENT_BASE_SCRIM_ALPHA);
        let luminance = srgb_to_linear(composited);
        let contrast = 1.05 / (luminance + 0.05);

        assert!(
            contrast >= 3.0,
            "Ambient primary metadata contrast was {contrast:.2}:1"
        );
    }

    #[test]
    fn ambient_scrim_has_no_horizontal_shape() {
        for (width, height) in [(320, 180), (180, 320)] {
            let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height)
                .expect("test surface");
            let context = cairo::Context::new(&surface).expect("test context");
            draw_immersive_scrim(
                &context,
                width,
                height,
                DisplayMode::Ambient,
                super::CinemaFraming::Cover,
            );
            drop(context);
            surface.flush();

            let stride = surface.stride() as usize;
            let data = surface.data().expect("test surface data");
            let y = height as usize / 2;
            let pixel_at = |x: usize| {
                let offset = y * stride + x * 4;
                u32::from_ne_bytes(data[offset..offset + 4].try_into().unwrap())
            };
            let expected_pixel = pixel_at(0);
            let rendered_alpha = (expected_pixel >> 24) as i32;
            let expected_alpha = (AMBIENT_BASE_SCRIM_ALPHA * 255.0).round() as i32;

            assert!((rendered_alpha - expected_alpha).abs() <= 1);

            for x in 1..width {
                assert_eq!(pixel_at(x as usize), expected_pixel);
            }
        }
    }

    #[test]
    fn cinema_scrim_tracks_portrait_and_landscape_metadata_edges() {
        let render_alpha =
            |width: i32, height: i32, framing: super::CinemaFraming, x: usize, y: usize| {
                let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height)
                    .expect("test surface");
                let context = cairo::Context::new(&surface).expect("test context");
                draw_immersive_scrim(&context, width, height, DisplayMode::Cinema, framing);
                drop(context);
                surface.flush();

                let stride = surface.stride() as usize;
                let data = surface.data().expect("test surface data");
                let offset = y * stride + x * 4;
                (u32::from_ne_bytes(data[offset..offset + 4].try_into().unwrap()) >> 24) as u8
            };

        let portrait_top = render_alpha(180, 320, super::CinemaFraming::Cover, 90, 0);
        let portrait_bottom = render_alpha(180, 320, super::CinemaFraming::Cover, 90, 319);
        assert!(portrait_top > portrait_bottom);

        let landscape_left = render_alpha(320, 180, super::CinemaFraming::Wide, 0, 90);
        let landscape_right = render_alpha(320, 180, super::CinemaFraming::Wide, 319, 90);
        assert!(landscape_left > landscape_right);
    }
}
