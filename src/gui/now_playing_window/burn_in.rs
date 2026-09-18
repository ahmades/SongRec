//! Idle dimming for the optional Now Playing burn-in protection.

use crate::core::preferences::normalize_burn_in_inactivity_minutes;
use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

const DIMMED_OVERLAY_OPACITY: f64 = 0.70;
const DIM_ANIMATION_DURATION_MS: u32 = 350;
const MICROSECONDS_PER_MINUTE: i64 = 60 * 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeAction {
    None,
    /// A newly viewable window starts bright with a fresh deadline.
    Start(Duration),
    /// A configuration change or activity while dimmed brightens smoothly.
    Restart(Duration),
    /// The original timer fired after newer activity; wait only for the remainder.
    Schedule(Duration),
    Dim,
    /// Hidden, suspended, released, or disabled windows are immediately bright.
    Stop,
}

/// Toolkit-independent inactivity state.
///
/// Pointer motion updates only `deadline_us`. The already-armed timer is left
/// alone and, when it fires, schedules the remaining interval. This avoids
/// removing and recreating a GLib source for every motion event.
#[derive(Debug, Default, Clone, Copy)]
struct InactivityState {
    enabled: bool,
    viewable: bool,
    inactivity_minutes: u16,
    deadline_us: Option<i64>,
    timer_armed: bool,
    dimmed: bool,
}

impl InactivityState {
    fn configure(&mut self, enabled: bool, inactivity_minutes: u16, now_us: i64) -> RuntimeAction {
        if self.enabled == enabled && self.inactivity_minutes == inactivity_minutes {
            return RuntimeAction::None;
        }

        self.enabled = enabled;
        self.inactivity_minutes = inactivity_minutes;
        self.restart(now_us, false)
    }

    fn set_viewable(&mut self, viewable: bool, now_us: i64) -> RuntimeAction {
        if self.viewable == viewable {
            return RuntimeAction::None;
        }

        self.viewable = viewable;
        self.restart(now_us, true)
    }

    fn activity(&mut self, now_us: i64) -> RuntimeAction {
        if !self.enabled || !self.viewable {
            return RuntimeAction::None;
        }

        let delay = self.inactivity_duration();
        self.deadline_us = Some(deadline_after(now_us, delay));
        if self.dimmed {
            self.dimmed = false;
            self.timer_armed = true;
            return RuntimeAction::Restart(delay);
        }

        if self.timer_armed {
            RuntimeAction::None
        } else {
            self.timer_armed = true;
            RuntimeAction::Schedule(delay)
        }
    }

    fn timer_fired(&mut self, now_us: i64) -> RuntimeAction {
        self.timer_armed = false;
        if !self.enabled || !self.viewable {
            return RuntimeAction::Stop;
        }

        let Some(deadline_us) = self.deadline_us else {
            return RuntimeAction::Stop;
        };
        if deadline_us > now_us {
            self.timer_armed = true;
            return RuntimeAction::Schedule(duration_from_micros(deadline_us - now_us));
        }

        self.deadline_us = None;
        self.dimmed = true;
        RuntimeAction::Dim
    }

    fn release(&mut self) -> RuntimeAction {
        self.viewable = false;
        self.deadline_us = None;
        self.timer_armed = false;
        self.dimmed = false;
        RuntimeAction::Stop
    }

    fn restart(&mut self, now_us: i64, newly_viewable: bool) -> RuntimeAction {
        self.deadline_us = None;
        self.timer_armed = false;
        self.dimmed = false;
        if !self.enabled || !self.viewable {
            return RuntimeAction::Stop;
        }

        let delay = self.inactivity_duration();
        self.deadline_us = Some(deadline_after(now_us, delay));
        self.timer_armed = true;
        if newly_viewable {
            RuntimeAction::Start(delay)
        } else {
            RuntimeAction::Restart(delay)
        }
    }

    fn inactivity_duration(self) -> Duration {
        duration_from_micros(
            i64::from(self.inactivity_minutes).saturating_mul(MICROSECONDS_PER_MINUTE),
        )
    }
}

fn deadline_after(now_us: i64, delay: Duration) -> i64 {
    let delay_us = i64::try_from(delay.as_micros()).unwrap_or(i64::MAX);
    now_us.saturating_add(delay_us)
}

fn duration_from_micros(microseconds: i64) -> Duration {
    Duration::from_micros(u64::try_from(microseconds.max(1)).unwrap_or(u64::MAX))
}

fn is_activity_event(event_type: gdk::EventType) -> bool {
    matches!(
        event_type,
        gdk::EventType::MotionNotify
            | gdk::EventType::ButtonPress
            | gdk::EventType::ButtonRelease
            | gdk::EventType::KeyPress
            | gdk::EventType::KeyRelease
            | gdk::EventType::EnterNotify
            | gdk::EventType::ProximityIn
            | gdk::EventType::DragEnter
            | gdk::EventType::DragMotion
            | gdk::EventType::DropStart
            | gdk::EventType::Scroll
            | gdk::EventType::TouchBegin
            | gdk::EventType::TouchUpdate
            | gdk::EventType::TouchEnd
            | gdk::EventType::TouchpadSwipe
            | gdk::EventType::TouchpadPinch
            | gdk::EventType::PadButtonPress
            | gdk::EventType::PadButtonRelease
            | gdk::EventType::PadRing
            | gdk::EventType::PadStrip
            | gdk::EventType::PadGroupMode
    )
}

struct BurnInInner {
    dim_layer: glib::WeakRef<gtk::DrawingArea>,
    animation: adw::TimedAnimation,
    state: Cell<InactivityState>,
    pending_timeout: RefCell<Option<glib::SourceId>>,
}

impl BurnInInner {
    fn configure(self: &Rc<Self>, enabled: bool, inactivity_minutes: u16) {
        let mut state = self.state.get();
        let action = state.configure(enabled, inactivity_minutes, glib::monotonic_time());
        self.state.set(state);
        self.apply(action);
    }

    fn set_viewable(self: &Rc<Self>, viewable: bool) {
        let mut state = self.state.get();
        let action = state.set_viewable(viewable, glib::monotonic_time());
        self.state.set(state);
        self.apply(action);
    }

    fn activity(self: &Rc<Self>) {
        let mut state = self.state.get();
        let action = state.activity(glib::monotonic_time());
        self.state.set(state);
        self.apply(action);
    }

    fn release(self: &Rc<Self>) {
        let mut state = self.state.get();
        let action = state.release();
        self.state.set(state);
        self.apply(action);
    }

    fn apply(self: &Rc<Self>, action: RuntimeAction) {
        match action {
            RuntimeAction::None => {}
            RuntimeAction::Start(delay) => {
                self.cancel_timeout();
                self.restore_bright();
                self.schedule_timeout(delay);
            }
            RuntimeAction::Restart(delay) => {
                self.cancel_timeout();
                self.animate_to(0.0);
                self.schedule_timeout(delay);
            }
            RuntimeAction::Schedule(delay) => self.schedule_timeout(delay),
            RuntimeAction::Dim => self.animate_to(DIMMED_OVERLAY_OPACITY),
            RuntimeAction::Stop => {
                self.cancel_timeout();
                self.restore_bright();
            }
        }
    }

    fn schedule_timeout(self: &Rc<Self>, delay: Duration) {
        debug_assert!(self.pending_timeout.borrow().is_none());
        let weak = Rc::downgrade(self);
        let source_id =
            glib::timeout_add_local_once(delay.max(Duration::from_micros(1)), move || {
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                // A one-shot source is already being removed by GLib. Only clear
                // our bookkeeping before the state machine optionally arms another.
                inner.pending_timeout.borrow_mut().take();
                let mut state = inner.state.get();
                let action = state.timer_fired(glib::monotonic_time());
                inner.state.set(state);
                inner.apply(action);
            });
        self.pending_timeout.borrow_mut().replace(source_id);
    }

    fn cancel_timeout(&self) {
        if let Some(source_id) = self.pending_timeout.borrow_mut().take() {
            source_id.remove();
        }
    }

    fn animate_to(&self, target_opacity: f64) {
        let Some(dim_layer) = self.dim_layer.upgrade() else {
            return;
        };
        let current_opacity = dim_layer.opacity();
        if (current_opacity - target_opacity).abs() < f64::EPSILON {
            return;
        }

        self.animation.pause();
        self.animation.set_value_from(current_opacity);
        self.animation.set_value_to(target_opacity);
        self.animation.reset();
        self.animation.play();
    }

    fn restore_bright(&self) {
        self.animation.reset();
        if let Some(dim_layer) = self.dim_layer.upgrade() {
            dim_layer.set_opacity(0.0);
        }
    }
}

impl Drop for BurnInInner {
    fn drop(&mut self) {
        if let Some(source_id) = self.pending_timeout.get_mut().take() {
            source_id.remove();
        }
        self.animation.reset();
        if let Some(dim_layer) = self.dim_layer.upgrade() {
            dim_layer.set_opacity(0.0);
        }
    }
}

/// Dims the Now Playing canvas after a period without user input.
///
/// The overlay never accepts input, and every installed event handler returns
/// `Proceed`, so burn-in protection cannot alter the window's normal gestures,
/// shortcuts, or context-menu dismissal behavior.
#[derive(Clone)]
pub(super) struct BurnInController {
    inner: Rc<BurnInInner>,
}

impl BurnInController {
    pub(super) fn new(dim_layer: &gtk::DrawingArea) -> Self {
        dim_layer.set_can_target(false);
        dim_layer.set_opacity(0.0);
        dim_layer.set_draw_func(|_, context, _, _| {
            context.set_source_rgb(0.0, 0.0, 0.0);
            if let Err(error) = context.paint() {
                log::warn!("Failed to paint the Now Playing dim layer: {error}");
            }
        });

        let dim_layer_weak = dim_layer.downgrade();
        let target = adw::CallbackAnimationTarget::new(move |opacity| {
            if let Some(dim_layer) = dim_layer_weak.upgrade() {
                dim_layer.set_opacity(opacity);
            }
        });
        let animation = adw::TimedAnimation::new(
            dim_layer,
            0.0,
            DIMMED_OVERLAY_OPACITY,
            DIM_ANIMATION_DURATION_MS,
            target,
        );
        animation.set_easing(adw::Easing::EaseInOutCubic);

        Self {
            inner: Rc::new(BurnInInner {
                dim_layer: dim_layer.downgrade(),
                animation,
                state: Cell::new(InactivityState::default()),
                pending_timeout: RefCell::new(None),
            }),
        }
    }

    /// Tracks window viewability and all user input without consuming events.
    pub(super) fn bind_window(&self, window: &gtk::Window) {
        let inner = Rc::downgrade(&self.inner);
        window.connect_map(move |window| {
            if let Some(inner) = inner.upgrade() {
                inner.set_viewable(!window.is_suspended());
            }
        });

        let inner = Rc::downgrade(&self.inner);
        window.connect_unmap(move |_| {
            if let Some(inner) = inner.upgrade() {
                inner.set_viewable(false);
            }
        });

        let inner = Rc::downgrade(&self.inner);
        window.connect_suspended_notify(move |window| {
            if let Some(inner) = inner.upgrade() {
                inner.set_viewable(window.is_mapped() && !window.is_suspended());
            }
        });

        let inner = Rc::downgrade(&self.inner);
        let input = gtk::EventControllerLegacy::new();
        input.set_propagation_phase(gtk::PropagationPhase::Capture);
        input.connect_event(move |_, event| {
            if is_activity_event(event.event_type())
                && let Some(inner) = inner.upgrade()
            {
                inner.activity();
            }
            glib::Propagation::Proceed
        });
        window.add_controller(input);

        self.inner
            .set_viewable(window.is_mapped() && !window.is_suspended());
    }

    /// Applies normalized settings and restarts a visible window's deadline.
    pub(super) fn configure(&self, enabled: bool, inactivity_minutes: u16) {
        self.inner.configure(
            enabled,
            normalize_burn_in_inactivity_minutes(inactivity_minutes),
        );
    }

    /// Records explicit activity that did not originate on this window.
    pub(super) fn activity(&self) {
        self.inner.activity();
    }

    /// Cancels pending work and immediately restores full brightness.
    pub(super) fn release(&self) {
        self.inner.release();
    }
}

#[cfg(test)]
mod tests {
    use super::{InactivityState, MICROSECONDS_PER_MINUTE, RuntimeAction};
    use std::time::Duration;

    fn minutes(value: u16) -> Duration {
        Duration::from_secs(u64::from(value) * 60)
    }

    fn active_state(inactivity_minutes: u16, now_us: i64) -> InactivityState {
        let mut state = InactivityState::default();
        assert_eq!(
            state.configure(true, inactivity_minutes, now_us),
            RuntimeAction::Stop
        );
        assert_eq!(
            state.set_viewable(true, now_us),
            RuntimeAction::Start(minutes(inactivity_minutes))
        );
        state
    }

    #[test]
    fn frequent_activity_moves_the_deadline_without_rearming_the_timer() {
        let mut state = active_state(5, 0);
        let original_deadline = 5 * MICROSECONDS_PER_MINUTE;

        assert_eq!(state.activity(MICROSECONDS_PER_MINUTE), RuntimeAction::None);
        assert_eq!(
            state.timer_fired(original_deadline),
            RuntimeAction::Schedule(minutes(1))
        );
        assert!(state.timer_armed);
        assert!(!state.dimmed);
    }

    #[test]
    fn deadline_dims_once_and_activity_brightens_with_a_fresh_interval() {
        let mut state = active_state(10, 100);
        let deadline = 100 + 10 * MICROSECONDS_PER_MINUTE;

        assert_eq!(state.timer_fired(deadline), RuntimeAction::Dim);
        assert!(state.dimmed);
        assert!(!state.timer_armed);

        assert_eq!(
            state.activity(deadline + 1),
            RuntimeAction::Restart(minutes(10))
        );
        assert!(!state.dimmed);
        assert!(state.timer_armed);
    }

    #[test]
    fn hiding_or_releasing_cancels_and_restores_brightness() {
        let mut state = active_state(5, 0);
        assert_eq!(state.set_viewable(false, 1), RuntimeAction::Stop);
        assert!(!state.timer_armed);
        assert!(!state.dimmed);
        assert_eq!(state.activity(2), RuntimeAction::None);

        assert_eq!(
            state.set_viewable(true, 3),
            RuntimeAction::Start(minutes(5))
        );
        assert_eq!(state.release(), RuntimeAction::Stop);
        assert!(!state.viewable);
        assert!(!state.timer_armed);
    }

    #[test]
    fn configuration_changes_restart_only_an_active_window() {
        let mut state = active_state(5, 0);
        assert_eq!(state.configure(true, 5, 1), RuntimeAction::None);
        assert_eq!(
            state.configure(true, 15, 1),
            RuntimeAction::Restart(minutes(15))
        );
        assert_eq!(state.configure(false, 15, 2), RuntimeAction::Stop);
        assert_eq!(state.configure(false, 30, 3), RuntimeAction::Stop);
        assert!(!state.timer_armed);
    }
}
