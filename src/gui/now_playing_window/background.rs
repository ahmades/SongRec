//! Background rendering, gradient caching, and responsive metadata typography.

use super::palette::Background;
use super::style::font_css_for_size;
use super::{BackgroundStyle, NowPlayingWindow};
use adw::prelude::*;
use cairo::{Context, Format, ImageSurface};
use std::cell::RefCell;

const GRADIENT_SURFACE_WIDTH: i32 = 256;
const TRANSITION_START: f64 = 0.20;

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
                    settings.background_style,
                    settings.lights_off,
                    &gradient_surface_for_draw,
                );
            });

        let text_css_for_resize = text_css.clone();
        let gradient_surface_for_resize = self.state.gradient_surface.clone();
        let background_state_for_resize = self.state.current_background.clone();
        let settings_for_resize = self.state.settings.clone();
        self.ui
            .background_area
            .connect_resize(move |area, width, height| {
                if width <= 0 || height <= 0 {
                    return;
                }

                text_css_for_resize.load_from_string(&font_css_for_size((width, height)));

                let settings = settings_for_resize.get();
                if matches!(settings.background_style, BackgroundStyle::Gradient) {
                    rebuild_gradient_surface(
                        &gradient_surface_for_resize,
                        effective_background(
                            background_state_for_resize.get(),
                            settings.background_style,
                            settings.lights_off,
                        ),
                        height,
                    );
                    area.queue_draw();
                }
            });
    }

    /// Enables or disables Lights Off mode and updates artwork/background visibility.
    pub(super) fn set_lights_off(&self, enabled: bool) {
        self.with_preference_updates_suspended(|| {
            if self.controls.lights_off_menu.is_active() != enabled {
                self.controls.lights_off_menu.set_active(enabled);
            }
            self.controls.hide_track_info.set_sensitive(!enabled);
            self.controls.round_corners.set_sensitive(!enabled);
            self.controls.album_cover_size.set_sensitive(!enabled);

            if enabled {
                if self.controls.hide_track_info.is_active() {
                    self.controls.hide_track_info.set_active(false);
                }
                self.ui.info_box.set_visible(true);
            }
        });
        self.apply_background();
        self.sync_artwork_visibility();
    }

    /// Applies the active background style, including the Lights Off override.
    pub(super) fn apply_background(&self) {
        let settings = self.state.settings.get();
        redraw_background(
            &self.ui.background_area,
            &self.state.gradient_surface,
            self.state.current_background.get(),
            settings.background_style,
            settings.lights_off,
        );
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
/// they apply the same Lights Off override and gradient-cache invalidation rules.
pub(super) fn redraw_background(
    area: &gtk::DrawingArea,
    cache: &RefCell<Option<CachedGradient>>,
    background: Background,
    style: BackgroundStyle,
    lights_off: bool,
) {
    if matches!(style, BackgroundStyle::Gradient) {
        rebuild_gradient_surface(
            cache,
            effective_background(background, style, lights_off),
            area.height(),
        );
    }
    area.queue_draw();
}

/// Resolves the background colors used for rendering, overriding them for Lights Off.
fn effective_background(
    background: Background,
    style: BackgroundStyle,
    lights_off: bool,
) -> Background {
    if lights_off {
        match style {
            BackgroundStyle::Gradient => Background {
                top: (38, 38, 38),
                bottom: (0, 0, 0),
            },
            BackgroundStyle::Solid => Background {
                top: (0, 0, 0),
                bottom: (0, 0, 0),
            },
        }
    } else {
        background
    }
}

/// Paints the current background into the supplied Cairo context.
fn draw_background(
    context: &Context,
    width: i32,
    height: i32,
    background: Background,
    style: BackgroundStyle,
    lights_off: bool,
    cache: &RefCell<Option<CachedGradient>>,
) {
    if width <= 0 || height <= 0 {
        return;
    }

    let background = effective_background(background, style, lights_off);

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
    use super::{argb32_pixel_bytes, effective_background};
    use crate::gui::now_playing_window::BackgroundStyle;
    use crate::gui::now_playing_window::palette::Background;

    #[test]
    fn argb32_pixels_follow_cairos_native_endian_layout() {
        assert_eq!(
            argb32_pixel_bytes(0x12, 0x34, 0x56),
            0xff12_3456_u32.to_ne_bytes()
        );
    }

    #[test]
    fn lights_off_uses_the_expected_background_for_each_style() {
        let artwork_background = Background {
            top: (10, 20, 30),
            bottom: (40, 50, 60),
        };

        assert_eq!(
            effective_background(artwork_background, BackgroundStyle::Gradient, false),
            artwork_background
        );
        assert_eq!(
            effective_background(artwork_background, BackgroundStyle::Solid, false),
            artwork_background
        );
        assert_eq!(
            effective_background(artwork_background, BackgroundStyle::Gradient, true),
            Background {
                top: (38, 38, 38),
                bottom: (0, 0, 0),
            }
        );
        assert_eq!(
            effective_background(artwork_background, BackgroundStyle::Solid, true),
            Background {
                top: (0, 0, 0),
                bottom: (0, 0, 0),
            }
        );
    }
}
