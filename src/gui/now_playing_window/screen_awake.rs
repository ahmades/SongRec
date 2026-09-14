//! Idle-inhibitor lifecycle for the optional persistent Now Playing display.

use adw::prelude::*;
use gettextrs::gettext;
use std::cell::Cell;
use std::num::NonZeroU32;
use std::rc::Rc;

trait IdleInhibitor {
    fn inhibit(&self) -> u32;
    fn uninhibit(&self, cookie: u32);
}

struct GtkIdleInhibitor {
    application: glib::WeakRef<gtk::Application>,
}

impl GtkIdleInhibitor {
    fn new(application: Option<&gtk::Application>) -> Self {
        let weak = glib::WeakRef::new();
        weak.set(application);
        Self { application: weak }
    }
}

impl IdleInhibitor for GtkIdleInhibitor {
    fn inhibit(&self) -> u32 {
        self.application.upgrade().map_or(0, |application| {
            application.inhibit(
                None::<&gtk::Window>,
                gtk::ApplicationInhibitFlags::IDLE,
                Some(&gettext("Now Playing window is visible")),
            )
        })
    }

    fn uninhibit(&self, cookie: u32) {
        if let Some(application) = self.application.upgrade() {
            application.uninhibit(cookie);
        }
    }
}

struct ScreenAwakeInner {
    inhibitor: Rc<dyn IdleInhibitor>,
    enabled: Cell<bool>,
    window_viewable: Cell<bool>,
    acquisition_attempted: Cell<bool>,
    cookie: Cell<Option<NonZeroU32>>,
}

impl ScreenAwakeInner {
    fn reconcile(&self) {
        let should_inhibit = self.enabled.get() && self.window_viewable.get();
        if !should_inhibit {
            self.acquisition_attempted.set(false);
            if let Some(cookie) = self.cookie.take() {
                self.inhibitor.uninhibit(cookie.get());
            }
            return;
        }

        if self.cookie.get().is_some() || self.acquisition_attempted.replace(true) {
            return;
        }

        let cookie = NonZeroU32::new(self.inhibitor.inhibit());
        if cookie.is_none() {
            log::debug!("The desktop did not provide a screen-awake inhibitor");
        }
        self.cookie.set(cookie);
    }
}

impl Drop for ScreenAwakeInner {
    fn drop(&mut self) {
        if let Some(cookie) = self.cookie.take() {
            self.inhibitor.uninhibit(cookie.get());
        }
    }
}

/// Owns at most one idle-inhibitor cookie and mirrors the window lifecycle.
#[derive(Clone)]
pub(super) struct ScreenAwakeController {
    inner: Rc<ScreenAwakeInner>,
}

impl ScreenAwakeController {
    pub(super) fn new(application: Option<&gtk::Application>) -> Self {
        Self::with_inhibitor(Rc::new(GtkIdleInhibitor::new(application)))
    }

    fn with_inhibitor(inhibitor: Rc<dyn IdleInhibitor>) -> Self {
        Self {
            inner: Rc::new(ScreenAwakeInner {
                inhibitor,
                enabled: Cell::new(false),
                window_viewable: Cell::new(false),
                acquisition_attempted: Cell::new(false),
                cookie: Cell::new(None),
            }),
        }
    }

    /// Tracks mapping and compositor suspension without making the standalone
    /// Now Playing window part of the application's window ownership.
    pub(super) fn bind_window(&self, window: &gtk::Window) {
        let controller = self.clone();
        window.connect_map(move |window| {
            controller.set_window_viewable(!window.is_suspended());
        });

        let controller = self.clone();
        window.connect_unmap(move |_| {
            controller.set_window_viewable(false);
        });

        let controller = self.clone();
        window.connect_suspended_notify(move |window| {
            controller.set_window_viewable(window.is_mapped() && !window.is_suspended());
        });

        self.set_window_viewable(window.is_mapped() && !window.is_suspended());
    }

    pub(super) fn set_enabled(&self, enabled: bool) {
        self.inner.enabled.set(enabled);
        self.inner.reconcile();
    }

    /// Explicit close-path backstop; the subsequent unmap is idempotent.
    pub(super) fn release(&self) {
        self.set_window_viewable(false);
    }

    fn set_window_viewable(&self, viewable: bool) {
        self.inner.window_viewable.set(viewable);
        self.inner.reconcile();
    }
}

#[cfg(test)]
mod tests {
    use super::{IdleInhibitor, ScreenAwakeController};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    #[derive(Default)]
    struct FakeInhibitor {
        next_cookie: Cell<u32>,
        acquisitions: Cell<usize>,
        releases: RefCell<Vec<u32>>,
    }

    impl FakeInhibitor {
        fn with_cookie(cookie: u32) -> Rc<Self> {
            Rc::new(Self {
                next_cookie: Cell::new(cookie),
                ..Self::default()
            })
        }
    }

    impl IdleInhibitor for FakeInhibitor {
        fn inhibit(&self) -> u32 {
            self.acquisitions.set(self.acquisitions.get() + 1);
            self.next_cookie.get()
        }

        fn uninhibit(&self, cookie: u32) {
            self.releases.borrow_mut().push(cookie);
        }
    }

    #[test]
    fn inhibition_requires_both_the_setting_and_a_viewable_window() {
        let backend = FakeInhibitor::with_cookie(7);
        let controller = ScreenAwakeController::with_inhibitor(backend.clone());

        controller.set_enabled(true);
        assert_eq!(backend.acquisitions.get(), 0);

        controller.set_enabled(false);
        controller.set_window_viewable(true);
        assert_eq!(backend.acquisitions.get(), 0);

        controller.set_enabled(true);
        controller.set_enabled(true);
        controller.set_window_viewable(true);
        assert_eq!(backend.acquisitions.get(), 1);
        assert!(backend.releases.borrow().is_empty());
    }

    #[test]
    fn hiding_disabling_and_remapping_release_and_reacquire_exactly_once() {
        let backend = FakeInhibitor::with_cookie(11);
        let controller = ScreenAwakeController::with_inhibitor(backend.clone());
        controller.set_enabled(true);
        controller.set_window_viewable(true);

        controller.set_window_viewable(false);
        controller.set_window_viewable(false);
        assert_eq!(&*backend.releases.borrow(), &[11]);

        backend.next_cookie.set(12);
        controller.set_window_viewable(true);
        assert_eq!(backend.acquisitions.get(), 2);
        controller.set_enabled(false);
        controller.set_enabled(false);
        assert_eq!(&*backend.releases.borrow(), &[11, 12]);
    }

    #[test]
    fn a_failed_acquisition_is_never_passed_to_uninhibit() {
        let backend = FakeInhibitor::with_cookie(0);
        let controller = ScreenAwakeController::with_inhibitor(backend.clone());
        controller.set_enabled(true);
        controller.set_window_viewable(true);
        controller.set_window_viewable(true);
        controller.set_enabled(false);

        assert_eq!(backend.acquisitions.get(), 1);
        assert!(backend.releases.borrow().is_empty());
    }

    #[test]
    fn dropping_the_last_controller_releases_an_active_cookie() {
        let backend = FakeInhibitor::with_cookie(21);
        {
            let controller = ScreenAwakeController::with_inhibitor(backend.clone());
            controller.set_enabled(true);
            controller.set_window_viewable(true);
        }

        assert_eq!(&*backend.releases.borrow(), &[21]);
    }
}
