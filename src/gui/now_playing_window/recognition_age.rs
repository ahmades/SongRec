//! Live, low-frequency rendering of the latest successful recognition age.

use adw::prelude::*;
use gettextrs::{gettext, ngettext};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

const MICROSECONDS_PER_SECOND: u64 = 1_000_000;
const SECONDS_PER_MINUTE: u64 = 60;
const SECONDS_PER_HOUR: u64 = 60 * SECONDS_PER_MINUTE;
const SECONDS_PER_DAY: u64 = 24 * SECONDS_PER_HOUR;

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgeDisplay {
    text: String,
    next_update: Duration,
}

fn plural_count(count: u64) -> u32 {
    count.min(u64::from(u32::MAX)) as u32
}

fn insert_count(template: String, count: u64) -> String {
    template.replace("{count}", &count.to_string())
}

fn age_display(age_microseconds: u64) -> AgeDisplay {
    let seconds = age_microseconds / MICROSECONDS_PER_SECOND;
    let (text, next_boundary_seconds) = match seconds {
        0..=4 => (gettext("Recognized just now"), 5),
        5..=59 => (
            insert_count(
                ngettext(
                    "Recognized {count} second ago",
                    "Recognized {count} seconds ago",
                    plural_count(seconds),
                ),
                seconds,
            ),
            seconds + 1,
        ),
        60..=3_599 => {
            let minutes = seconds / SECONDS_PER_MINUTE;
            (
                insert_count(
                    ngettext(
                        "Recognized {count} minute ago",
                        "Recognized {count} minutes ago",
                        plural_count(minutes),
                    ),
                    minutes,
                ),
                (minutes + 1) * SECONDS_PER_MINUTE,
            )
        }
        3_600..=86_399 => {
            let hours = seconds / SECONDS_PER_HOUR;
            (
                insert_count(
                    ngettext(
                        "Recognized {count} hour ago",
                        "Recognized {count} hours ago",
                        plural_count(hours),
                    ),
                    hours,
                ),
                (hours + 1) * SECONDS_PER_HOUR,
            )
        }
        _ => {
            let days = seconds / SECONDS_PER_DAY;
            (
                insert_count(
                    ngettext(
                        "Recognized {count} day ago",
                        "Recognized {count} days ago",
                        plural_count(days),
                    ),
                    days,
                ),
                (days + 1).saturating_mul(SECONDS_PER_DAY),
            )
        }
    };
    let next_boundary_microseconds = next_boundary_seconds.saturating_mul(MICROSECONDS_PER_SECOND);
    let delay_microseconds = next_boundary_microseconds
        .saturating_sub(age_microseconds)
        .max(1);
    AgeDisplay {
        text,
        next_update: Duration::from_micros(delay_microseconds),
    }
}

struct RecognitionAgeInner {
    labels: [glib::WeakRef<gtk::Label>; 2],
    enabled: Cell<bool>,
    viewable: Cell<bool>,
    response_received_at: Cell<Option<i64>>,
    pending_timeout: RefCell<Option<glib::SourceId>>,
}

impl RecognitionAgeInner {
    fn reconcile(self: &Rc<Self>) {
        self.cancel_timeout();
        let Some(received_at) = self
            .enabled
            .get()
            .then_some(())
            .filter(|_| self.viewable.get())
            .and(self.response_received_at.get())
        else {
            self.clear();
            return;
        };

        let age_microseconds = glib::monotonic_time().saturating_sub(received_at).max(0) as u64;
        let display = age_display(age_microseconds);
        for label in &self.labels {
            if let Some(label) = label.upgrade() {
                label.set_label(&display.text);
                label.set_visible(true);
            }
        }

        let weak = Rc::downgrade(self);
        let source_id = glib::timeout_add_local_once(display.next_update, move || {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            inner.pending_timeout.borrow_mut().take();
            inner.reconcile();
        });
        self.pending_timeout.borrow_mut().replace(source_id);
    }

    fn clear(&self) {
        for label in &self.labels {
            if let Some(label) = label.upgrade() {
                label.set_label("");
                label.set_visible(false);
            }
        }
    }

    fn cancel_timeout(&self) {
        if let Some(source_id) = self.pending_timeout.borrow_mut().take() {
            source_id.remove();
        }
    }
}

impl Drop for RecognitionAgeInner {
    fn drop(&mut self) {
        if let Some(source_id) = self.pending_timeout.get_mut().take() {
            source_id.remove();
        }
    }
}

#[derive(Clone)]
pub(super) struct RecognitionAgeController {
    inner: Rc<RecognitionAgeInner>,
}

impl RecognitionAgeController {
    pub(super) fn new(classic: &gtk::Label, immersive: &gtk::Label) -> Self {
        Self {
            inner: Rc::new(RecognitionAgeInner {
                labels: [classic.downgrade(), immersive.downgrade()],
                enabled: Cell::new(false),
                viewable: Cell::new(false),
                response_received_at: Cell::new(None),
                pending_timeout: RefCell::new(None),
            }),
        }
    }

    pub(super) fn bind_window(&self, window: &gtk::Window) {
        let controller = self.clone();
        window.connect_map(move |window| {
            controller.set_viewable(!window.is_suspended());
        });

        let controller = self.clone();
        window.connect_unmap(move |_| controller.set_viewable(false));

        let controller = self.clone();
        window.connect_suspended_notify(move |window| {
            controller.set_viewable(window.is_mapped() && !window.is_suspended());
        });

        self.set_viewable(window.is_mapped() && !window.is_suspended());
    }

    pub(super) fn set_enabled(&self, enabled: bool) {
        if self.inner.enabled.replace(enabled) != enabled {
            self.inner.reconcile();
        }
    }

    pub(super) fn set_response_received_at(&self, received_at: Option<i64>) {
        if self.inner.response_received_at.replace(received_at) != received_at {
            self.inner.reconcile();
        }
    }

    pub(super) fn release(&self) {
        self.set_viewable(false);
    }

    fn set_viewable(&self, viewable: bool) {
        if self.inner.viewable.replace(viewable) != viewable {
            self.inner.reconcile();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MICROSECONDS_PER_SECOND, age_display};
    use std::time::Duration;

    fn at(seconds: u64) -> super::AgeDisplay {
        age_display(seconds * MICROSECONDS_PER_SECOND)
    }

    #[test]
    fn recognition_age_uses_human_scale_boundaries() {
        assert_eq!(at(0).text, "Recognized just now");
        assert_eq!(at(5).text, "Recognized 5 seconds ago");
        assert_eq!(at(59).text, "Recognized 59 seconds ago");
        assert_eq!(at(60).text, "Recognized 1 minute ago");
        assert_eq!(at(3_599).text, "Recognized 59 minutes ago");
        assert_eq!(at(3_600).text, "Recognized 1 hour ago");
        assert_eq!(at(86_400).text, "Recognized 1 day ago");
    }

    #[test]
    fn recognition_age_only_wakes_at_the_next_visible_change() {
        assert_eq!(at(0).next_update, Duration::from_secs(5));
        assert_eq!(at(20).next_update, Duration::from_secs(1));
        assert_eq!(at(60).next_update, Duration::from_secs(60));
        assert_eq!(at(3_600).next_update, Duration::from_secs(3_600));
        assert_eq!(at(86_400).next_update, Duration::from_secs(86_400));
    }
}
