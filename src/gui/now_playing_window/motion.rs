//! Slow, varied motion for immersive-mode blurred backdrops.

use super::{
    BACKGROUND_MOTION_REVERSAL_DURATION_DEFAULT_SECS, BACKGROUND_MOTION_ZOOM_DEFAULT_PERCENT,
    clamp_background_motion_zoom_percent, normalize_background_motion_reversal_duration_secs,
};
use adw::prelude::*;
use rand::RngExt as _;
use std::cell::Cell;
use std::f64::consts::{PI, TAU};
use std::rc::Rc;

const RESTING_SCALE: f64 = 1.0;
const PATH_WAYPOINT_COUNT: usize = 12;
const MIN_WAYPOINT_RADIUS: f64 = 0.65;
const MAX_WAYPOINT_RADIUS: f64 = 0.95;
const MIN_TRAVEL_DISTANCE_SQUARED: f64 = 0.4;
const RANDOM_WAYPOINT_ATTEMPTS: usize = 32;
const TRAVEL_FRACTION: f64 = 0.9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MotionConfiguration {
    enabled: bool,
    zoom_percent: u16,
    direction_change_interval_secs: u64,
}

impl Default for MotionConfiguration {
    fn default() -> Self {
        Self {
            enabled: false,
            zoom_percent: BACKGROUND_MOTION_ZOOM_DEFAULT_PERCENT,
            direction_change_interval_secs: BACKGROUND_MOTION_REVERSAL_DURATION_DEFAULT_SECS,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct MotionOffset {
    x: f64,
    y: f64,
}

impl MotionOffset {
    const CENTER: Self = Self { x: 0.0, y: 0.0 };

    fn interpolate(self, target: Self, progress: f64) -> Self {
        Self {
            x: self.x + (target.x - self.x) * progress,
            y: self.y + (target.y - self.y) * progress,
        }
    }

    fn distance_squared(self, other: Self) -> f64 {
        (self.x - other.x).powi(2) + (self.y - other.y).powi(2)
    }
}

/// Drives a continuously zoomed backdrop around a random, smoothly looping path.
///
/// The animation target only invalidates allocation; artwork pixels are never
/// regenerated while the animation is running.
#[derive(Clone)]
pub(super) struct BackdropMotion {
    widget: gtk::Widget,
    animation: adw::TimedAnimation,
    offset: Rc<Cell<MotionOffset>>,
    configuration: Rc<Cell<MotionConfiguration>>,
}

impl BackdropMotion {
    pub(super) fn new(widget: &impl IsA<gtk::Widget>) -> Self {
        let widget = widget.clone().upcast::<gtk::Widget>();
        let offset = Rc::new(Cell::new(MotionOffset::CENTER));
        let path = Rc::new(random_motion_path());
        let offset_for_target = offset.clone();
        let path_for_target = path;
        let widget_for_target = widget.downgrade();
        let target = adw::CallbackAnimationTarget::new(move |path_position| {
            offset_for_target.set(path_offset_at(&path_for_target, path_position));
            if let Some(widget) = widget_for_target.upgrade() {
                widget.queue_allocate();
            }
        });
        let animation = adw::TimedAnimation::new(
            &widget,
            0.0,
            PATH_WAYPOINT_COUNT as f64,
            path_duration_ms(BACKGROUND_MOTION_REVERSAL_DURATION_DEFAULT_SECS),
            target,
        );
        animation.set_easing(adw::Easing::Linear);
        animation.set_repeat_count(0);

        let configuration = Rc::new(Cell::new(MotionConfiguration::default()));
        let configuration_for_map = configuration.clone();
        let animation_for_map = animation.downgrade();
        widget.connect_map(move |_| {
            if configuration_for_map.get().enabled
                && let Some(animation) = animation_for_map.upgrade()
            {
                animation.play();
            }
        });

        let offset_for_unmap = offset.clone();
        let animation_for_unmap = animation.downgrade();
        widget.connect_unmap(move |widget| {
            if let Some(animation) = animation_for_unmap.upgrade() {
                animation.reset();
            }
            offset_for_unmap.set(MotionOffset::CENTER);
            widget.queue_allocate();
        });

        Self {
            widget,
            animation,
            offset,
            configuration,
        }
    }

    /// Applies one normalized configuration, restarting only when enabled state changes.
    pub(super) fn configure(
        &self,
        enabled: bool,
        zoom_percent: u16,
        direction_change_interval_secs: u64,
    ) {
        let configuration = MotionConfiguration {
            enabled,
            zoom_percent: clamp_background_motion_zoom_percent(zoom_percent),
            direction_change_interval_secs: normalize_background_motion_reversal_duration_secs(
                direction_change_interval_secs,
            ),
        };
        let previous = self.configuration.replace(configuration);
        if previous == configuration {
            return;
        }

        self.animation.set_duration(path_duration_ms(
            configuration.direction_change_interval_secs,
        ));
        self.widget.queue_allocate();

        if previous.enabled == configuration.enabled {
            return;
        }

        self.animation.reset();
        self.offset.set(MotionOffset::CENTER);
        if configuration.enabled && self.widget.is_mapped() {
            self.animation.play();
        }
    }

    pub(super) fn backdrop_rect(&self, width: i32, height: i32) -> gdk::Rectangle {
        let configuration = self.configuration.get();
        let scale = if configuration.enabled {
            f64::from(configuration.zoom_percent) / 100.0
        } else {
            RESTING_SCALE
        };
        scaled_backdrop_rect(width, height, scale, self.offset.get())
    }
}

fn path_duration_ms(direction_change_interval_secs: u64) -> u32 {
    direction_change_interval_secs
        .saturating_mul(PATH_WAYPOINT_COUNT as u64)
        .saturating_mul(1_000)
        .min(u64::from(u32::MAX)) as u32
}

fn random_motion_path() -> Vec<MotionOffset> {
    let mut path = Vec::with_capacity(PATH_WAYPOINT_COUNT);
    path.push(MotionOffset::CENTER);
    let mut rng = rand::rng();

    while path.len() < PATH_WAYPOINT_COUNT {
        let previous = *path.last().expect("motion path always starts at center");
        let waypoint = (0..RANDOM_WAYPOINT_ATTEMPTS)
            .find_map(|_| {
                let angle = rng.random_range(0.0_f64..TAU);
                let radius = rng.random_range(MIN_WAYPOINT_RADIUS..=MAX_WAYPOINT_RADIUS);
                let candidate = MotionOffset {
                    x: angle.cos() * radius,
                    y: angle.sin() * radius,
                };
                (previous.distance_squared(candidate) >= MIN_TRAVEL_DISTANCE_SQUARED)
                    .then_some(candidate)
            })
            .unwrap_or_else(|| fallback_offset_away_from(previous));
        path.push(waypoint);
    }

    path
}

fn fallback_offset_away_from(current: MotionOffset) -> MotionOffset {
    const FALLBACKS: [MotionOffset; 8] = [
        MotionOffset { x: 0.9, y: 0.0 },
        MotionOffset { x: -0.9, y: 0.0 },
        MotionOffset { x: 0.0, y: 0.9 },
        MotionOffset { x: 0.0, y: -0.9 },
        MotionOffset { x: 0.64, y: 0.64 },
        MotionOffset { x: -0.64, y: 0.64 },
        MotionOffset { x: 0.64, y: -0.64 },
        MotionOffset { x: -0.64, y: -0.64 },
    ];

    FALLBACKS
        .into_iter()
        .max_by(|left, right| {
            current
                .distance_squared(*left)
                .total_cmp(&current.distance_squared(*right))
        })
        .expect("fallback motion path is non-empty")
}

fn path_offset_at(path: &[MotionOffset], path_position: f64) -> MotionOffset {
    if path.is_empty() {
        return MotionOffset::CENTER;
    }

    let wrapped_position = path_position.rem_euclid(path.len() as f64);
    let from_index = wrapped_position.floor() as usize;
    let to_index = (from_index + 1) % path.len();
    let segment_progress = wrapped_position - from_index as f64;
    let eased_progress = 0.5 - 0.5 * (PI * segment_progress).cos();
    path[from_index].interpolate(path[to_index], eased_progress)
}

fn scaled_backdrop_rect(
    width: i32,
    height: i32,
    scale: f64,
    offset: MotionOffset,
) -> gdk::Rectangle {
    let width = width.max(0);
    let height = height.max(0);
    let scale = scale.max(RESTING_SCALE);
    let scaled_width = (f64::from(width) * scale).ceil() as i32;
    let scaled_height = (f64::from(height) * scale).ceil() as i32;

    gdk::Rectangle::new(
        backdrop_origin(width, scaled_width, offset.x),
        backdrop_origin(height, scaled_height, offset.y),
        scaled_width,
        scaled_height,
    )
}

fn backdrop_origin(viewport: i32, backdrop: i32, normalized_offset: f64) -> i32 {
    let overhang = (backdrop - viewport).max(0);
    let centered = -f64::from(overhang) / 2.0;
    let travel = normalized_offset.clamp(-1.0, 1.0) * f64::from(overhang) / 2.0 * TRAVEL_FRACTION;
    (centered + travel).round() as i32
}

#[cfg(test)]
mod tests {
    use super::{
        MIN_TRAVEL_DISTANCE_SQUARED, MotionOffset, PATH_WAYPOINT_COUNT, path_offset_at,
        random_motion_path, scaled_backdrop_rect,
    };

    #[test]
    fn resting_backdrop_exactly_matches_the_viewport() {
        assert_eq!(
            scaled_backdrop_rect(1920, 1080, 1.0, MotionOffset::CENTER),
            gtk::gdk::Rectangle::new(0, 0, 1920, 1080)
        );
    }

    #[test]
    fn every_pan_extreme_keeps_the_zoomed_backdrop_over_the_viewport() {
        for offset in [
            MotionOffset { x: -1.0, y: -1.0 },
            MotionOffset { x: 1.0, y: -1.0 },
            MotionOffset { x: -1.0, y: 1.0 },
            MotionOffset { x: 1.0, y: 1.0 },
        ] {
            let rect = scaled_backdrop_rect(101, 51, 1.1, offset);
            assert_eq!(rect.width(), 112);
            assert_eq!(rect.height(), 57);
            assert!(rect.x() <= 0);
            assert!(rect.y() <= 0);
            assert!(rect.x() + rect.width() >= 101);
            assert!(rect.y() + rect.height() >= 51);
        }
    }

    #[test]
    fn random_path_is_bounded_and_has_visible_travel_between_waypoints() {
        let path = random_motion_path();
        assert_eq!(path.len(), PATH_WAYPOINT_COUNT);
        assert_eq!(path[0], MotionOffset::CENTER);
        for pair in path.windows(2) {
            assert!(pair[0].distance_squared(pair[1]) >= MIN_TRAVEL_DISTANCE_SQUARED);
            assert!((-1.0..=1.0).contains(&pair[1].x));
            assert!((-1.0..=1.0).contains(&pair[1].y));
        }
        assert!(
            path.last().unwrap().distance_squared(MotionOffset::CENTER)
                >= MIN_TRAVEL_DISTANCE_SQUARED
        );
    }

    #[test]
    fn looping_path_interpolates_smoothly_through_both_axes() {
        let path = [
            MotionOffset::CENTER,
            MotionOffset { x: 0.8, y: -0.6 },
            MotionOffset { x: -0.4, y: 0.9 },
        ];

        assert_eq!(path_offset_at(&path, 0.0), path[0]);
        assert_eq!(path_offset_at(&path, 1.0), path[1]);
        assert_eq!(path_offset_at(&path, 2.0), path[2]);
        assert_eq!(path_offset_at(&path, 3.0), path[0]);
        let halfway = path_offset_at(&path, 0.5);
        assert!((halfway.x - 0.4).abs() < f64::EPSILON);
        assert!((halfway.y + 0.3).abs() < f64::EPSILON);
    }
}
