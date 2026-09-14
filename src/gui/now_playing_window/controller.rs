//! Shared Now Playing settings state and debounced snapshot persistence.

use super::NowPlayingSettings;
use crate::core::preferences::NowPlayingPreferenceChange;
use crate::core::thread_messages::GUIMessage;
use gio::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

const PERSISTENCE_DEBOUNCE_MS: u64 = 150;

/// One model shared by the preferences page, context menu, and renderer.
#[derive(Clone)]
pub(crate) struct NowPlayingSettingsController {
    settings: Rc<Cell<NowPlayingSettings>>,
    gui_tx: Option<async_channel::Sender<GUIMessage>>,
    pending_save: Rc<RefCell<Option<glib::SourceId>>>,
}

impl NowPlayingSettingsController {
    pub(crate) fn new(
        settings: NowPlayingSettings,
        gui_tx: Option<async_channel::Sender<GUIMessage>>,
    ) -> Self {
        Self {
            settings: Rc::new(Cell::new(settings)),
            gui_tx,
            pending_save: Rc::new(RefCell::new(None)),
        }
    }

    pub(crate) fn settings(&self) -> NowPlayingSettings {
        self.settings.get()
    }

    pub(crate) fn settings_cell(&self) -> Rc<Cell<NowPlayingSettings>> {
        self.settings.clone()
    }

    pub(crate) fn update(&self, change: NowPlayingPreferenceChange) {
        self.cancel_all();
        self.apply(change);
        self.send(true);
    }

    /// Updates both views and the in-memory preferences immediately, coalescing
    /// only disk writes. Every control contributes to one latest snapshot.
    pub(crate) fn update_debounced(&self, change: NowPlayingPreferenceChange) {
        if matches!(change, NowPlayingPreferenceChange::Reset) {
            self.reset();
            return;
        }
        self.cancel_all();
        self.apply(change);
        self.send(false);

        let controller = self.clone();
        let source_id = glib::timeout_add_local_once(
            Duration::from_millis(PERSISTENCE_DEBOUNCE_MS),
            move || {
                controller.pending_save.borrow_mut().take();
                controller.send(true);
            },
        );
        self.pending_save.borrow_mut().replace(source_id);
    }

    pub(crate) fn reset(&self) {
        self.update(NowPlayingPreferenceChange::Reset);
    }

    /// Stops delayed saves. At shutdown the owner also saves `settings()`
    /// synchronously, rather than relying on another GUI message.
    pub(crate) fn cancel_all(&self) {
        if let Some(source_id) = self.pending_save.borrow_mut().take() {
            source_id.remove();
        }
    }

    /// Install the shutdown path once on the owning application. The snapshot
    /// may be newer than any preference message the main loop has dispatched.
    pub(crate) fn connect_shutdown(
        &self,
        application: &impl IsA<gio::Application>,
        preferences: std::sync::Arc<
            std::sync::Mutex<crate::core::preferences::PreferencesInterface>,
        >,
    ) {
        let controller = self.clone();
        application.connect_shutdown(move |_| {
            controller.cancel_all();
            preferences
                .lock()
                .unwrap()
                .set_now_playing(controller.settings(), true);
        });
    }

    fn apply(&self, change: NowPlayingPreferenceChange) {
        let mut settings = self.settings.get();
        settings.apply_change(change);
        self.settings.set(settings);
    }

    fn send(&self, persist: bool) {
        if let Some(sender) = self.gui_tx.as_ref()
            && let Err(error) = sender.try_send(GUIMessage::NowPlayingPreferenceChanged {
                settings: self.settings(),
                persist,
            })
        {
            log::error!("Failed to update Now Playing preference: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::NowPlayingSettingsController;
    use crate::core::preferences::{
        BACKGROUND_MOTION_REVERSAL_DURATION_MIN_SECS, BACKGROUND_MOTION_ZOOM_MAX_PERCENT,
        DisplayMode, NowPlayingPreferenceChange, NowPlayingPreferences, TextSize,
    };

    #[test]
    fn immediate_updates_and_reset_share_one_normalized_model() {
        let controller = NowPlayingSettingsController::new(NowPlayingPreferences::default(), None);

        controller.update(NowPlayingPreferenceChange::HideTrackInfo(true));
        assert!(controller.settings().shared.hide_track_info);
        controller.update(NowPlayingPreferenceChange::KeepScreenAwake(true));
        assert!(controller.settings().shared.keep_screen_awake);

        controller.update(NowPlayingPreferenceChange::DisplayMode(
            DisplayMode::LightsOff,
        ));
        assert_eq!(controller.settings().display_mode, DisplayMode::LightsOff);
        assert!(controller.settings().shared.hide_track_info);
        assert!(controller.settings().shared.keep_screen_awake);

        controller.update(NowPlayingPreferenceChange::DisplayMode(
            DisplayMode::Classic,
        ));
        assert!(controller.settings().shared.hide_track_info);

        controller.update(NowPlayingPreferenceChange::DisplayMode(
            DisplayMode::Ambient,
        ));
        controller.update(NowPlayingPreferenceChange::BackgroundMotionEnabled(true));
        controller.update(NowPlayingPreferenceChange::BackgroundMotionZoomPercent(
            u16::MAX,
        ));
        controller.update(NowPlayingPreferenceChange::BackgroundMotionReversalDurationSecs(21));
        assert!(controller.settings().shared.background_motion_enabled);
        assert_eq!(
            controller.settings().shared.background_motion_zoom_percent,
            BACKGROUND_MOTION_ZOOM_MAX_PERCENT
        );
        assert_eq!(
            controller
                .settings()
                .shared
                .background_motion_reversal_duration_secs,
            BACKGROUND_MOTION_REVERSAL_DURATION_MIN_SECS
        );
        controller.reset();
        assert_eq!(controller.settings(), NowPlayingPreferences::default());
    }

    #[test]
    fn sliders_share_one_save_and_reset_cancels_stale_snapshots() {
        let _serial = crate::MAIN_CONTEXT_TEST_LOCK.lock().unwrap();
        use crate::core::thread_messages::GUIMessage;
        use std::time::Duration;

        let context = glib::MainContext::default();
        let _guard = context.acquire().unwrap();
        let (sender, receiver) = async_channel::unbounded();
        let controller =
            NowPlayingSettingsController::new(NowPlayingPreferences::default(), Some(sender));
        let receive = || match receiver.try_recv().unwrap() {
            GUIMessage::NowPlayingPreferenceChanged { settings, persist } => (settings, persist),
            _ => panic!("unexpected GUI message"),
        };

        controller.update_debounced(NowPlayingPreferenceChange::TextSize(TextSize::LARGE));
        let (settings, persist) = receive();
        assert_eq!(settings.shared.text_size, TextSize::LARGE);
        assert!(!persist);
        controller.update_debounced(NowPlayingPreferenceChange::BackgroundMotionZoomPercent(119));
        let (settings, persist) = receive();
        assert_eq!(settings.shared.text_size, TextSize::LARGE);
        assert_eq!(settings.shared.background_motion_zoom_percent, 119);
        assert!(!persist);

        context.block_on(glib::timeout_future(Duration::from_millis(190)));
        let (saved, persist) = receive();
        assert!(persist);
        assert_eq!(saved, controller.settings());
        assert!(receiver.try_recv().is_err());

        controller.update_debounced(NowPlayingPreferenceChange::TextSize(TextSize::SMALL));
        receive();
        controller.reset();
        let (settings, persist) = receive();
        assert!(persist);
        assert_eq!(settings, NowPlayingPreferences::default());
        assert!(controller.pending_save.borrow().is_none());

        controller.update_debounced(NowPlayingPreferenceChange::TextSize(TextSize::LARGE));
        receive();
        // The shutdown owner can save the current value before cancelling the timer.
        assert_eq!(controller.settings().shared.text_size, TextSize::LARGE);
        controller.cancel_all();
        context.block_on(glib::timeout_future(Duration::from_millis(190)));
        assert!(receiver.try_recv().is_err());
    }
}
