//! Shared setup for preference sliders whose values are discrete.

use adw::prelude::*;

use crate::core::preferences::{
    BACKGROUND_MOTION_REVERSAL_DURATION_DEFAULT_SECS, BACKGROUND_MOTION_REVERSAL_DURATION_MAX_SECS,
    BACKGROUND_MOTION_REVERSAL_DURATION_MIN_SECS, BACKGROUND_MOTION_REVERSAL_DURATION_STEP_SECS,
    BACKGROUND_MOTION_ZOOM_DEFAULT_PERCENT, BACKGROUND_MOTION_ZOOM_MAX_PERCENT,
    BACKGROUND_MOTION_ZOOM_MIN_PERCENT, BACKGROUND_MOTION_ZOOM_STEP_PERCENT,
    clamp_background_motion_zoom_percent, normalize_background_motion_reversal_duration_secs,
};

pub(super) fn configure_background_motion_zoom_scale(scale: &gtk::Scale) {
    configure_discrete_scale(
        scale,
        f64::from(BACKGROUND_MOTION_ZOOM_DEFAULT_PERCENT),
        f64::from(BACKGROUND_MOTION_ZOOM_MIN_PERCENT),
        f64::from(BACKGROUND_MOTION_ZOOM_MAX_PERCENT),
        f64::from(BACKGROUND_MOTION_ZOOM_STEP_PERCENT),
        |value| {
            f64::from(clamp_background_motion_zoom_percent(
                value.round().max(0.0) as u16
            ))
        },
    );
}

pub(super) fn configure_background_motion_reversal_duration_scale(scale: &gtk::Scale) {
    configure_discrete_scale(
        scale,
        BACKGROUND_MOTION_REVERSAL_DURATION_DEFAULT_SECS as f64,
        BACKGROUND_MOTION_REVERSAL_DURATION_MIN_SECS as f64,
        BACKGROUND_MOTION_REVERSAL_DURATION_MAX_SECS as f64,
        BACKGROUND_MOTION_REVERSAL_DURATION_STEP_SECS as f64,
        |value| {
            normalize_background_motion_reversal_duration_secs(value.round().max(0.0) as u64) as f64
        },
    );
}

/// Configures an integer-valued scale and keeps pointer drags on supported steps.
///
/// `GtkAdjustment::step_increment` controls keyboard and scroll input, but a
/// pointer drag can still land between increments. Normalizing on every value
/// change keeps both settings surfaces and the persisted model in agreement.
pub(super) fn configure_discrete_scale(
    scale: &gtk::Scale,
    value: f64,
    lower: f64,
    upper: f64,
    step: f64,
    normalize: fn(f64) -> f64,
) {
    scale
        .adjustment()
        .configure(value, lower, upper, step, step, 0.0);
    scale.set_digits(0);
    scale.set_round_digits(0);
    scale.set_draw_value(true);
    scale.connect_value_changed(move |scale| {
        let normalized = normalize(scale.value());
        if (normalized - scale.value()).abs() > f64::EPSILON {
            scale.set_value(normalized);
        }
    });
}
