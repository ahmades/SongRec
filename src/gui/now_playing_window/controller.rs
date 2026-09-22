//! Shared Now Playing settings state and debounced snapshot persistence.

use super::NowPlayingSettings;
use crate::core::preferences::{
    FullscreenMonitorTarget, NowPlayingPreferenceChange, NowPlayingPresetCatalog,
    NowPlayingPresetId, PresetError,
};
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
    fullscreen_monitor: Rc<RefCell<Option<FullscreenMonitorTarget>>>,
    presets: Rc<RefCell<NowPlayingPresetCatalog>>,
    selected_preset_id: Rc<Cell<Option<NowPlayingPresetId>>>,
    gui_tx: Option<async_channel::Sender<GUIMessage>>,
    pending_save: Rc<RefCell<Option<glib::SourceId>>>,
}

impl NowPlayingSettingsController {
    #[cfg(test)]
    pub(crate) fn new(
        settings: NowPlayingSettings,
        gui_tx: Option<async_channel::Sender<GUIMessage>>,
    ) -> Self {
        Self::new_with_presets(settings, NowPlayingPresetCatalog::default(), gui_tx)
    }

    #[cfg(test)]
    pub(crate) fn new_with_presets(
        settings: NowPlayingSettings,
        presets: NowPlayingPresetCatalog,
        gui_tx: Option<async_channel::Sender<GUIMessage>>,
    ) -> Self {
        Self::new_with_presets_and_fullscreen_monitor(settings, presets, None, gui_tx)
    }

    pub(crate) fn new_with_presets_and_fullscreen_monitor(
        settings: NowPlayingSettings,
        presets: NowPlayingPresetCatalog,
        fullscreen_monitor: Option<FullscreenMonitorTarget>,
        gui_tx: Option<async_channel::Sender<GUIMessage>>,
    ) -> Self {
        Self {
            settings: Rc::new(Cell::new(settings)),
            fullscreen_monitor: Rc::new(RefCell::new(fullscreen_monitor)),
            presets: Rc::new(RefCell::new(presets)),
            selected_preset_id: Rc::new(Cell::new(None)),
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

    pub(crate) fn fullscreen_monitor(&self) -> Option<FullscreenMonitorTarget> {
        self.fullscreen_monitor.borrow().clone()
    }

    pub(crate) fn fullscreen_monitor_cell(&self) -> Rc<RefCell<Option<FullscreenMonitorTarget>>> {
        self.fullscreen_monitor.clone()
    }

    pub(crate) fn update_fullscreen_monitor(&self, target: Option<FullscreenMonitorTarget>) {
        if *self.fullscreen_monitor.borrow() == target {
            return;
        }
        *self.fullscreen_monitor.borrow_mut() = target;
        self.send_fullscreen_monitor(true);
    }

    pub(crate) fn presets(&self) -> NowPlayingPresetCatalog {
        self.presets.borrow().clone()
    }

    pub(crate) fn selected_preset_id(&self) -> Option<NowPlayingPresetId> {
        self.selected_preset_id.get()
    }

    /// Whether the active settings have diverged from the selected snapshot.
    /// This is derived instead of stored so every ordinary settings mutation
    /// automatically keeps the state accurate.
    pub(crate) fn selected_preset_is_modified(&self) -> bool {
        let Some(id) = self.selected_preset_id() else {
            return false;
        };
        self.presets
            .borrow()
            .get(id)
            .is_none_or(|preset| preset.settings != self.settings())
    }

    pub(crate) fn create_preset(&self, name: &str) -> Result<NowPlayingPresetId, PresetError> {
        let id = self.presets.borrow_mut().create(name, self.settings())?;
        self.selected_preset_id.set(Some(id));
        self.send_presets();
        Ok(id)
    }

    /// Replaces the complete active settings snapshot in one update. Any
    /// pending slider save is cancelled so it cannot later overwrite the
    /// loaded preset with stale settings.
    pub(crate) fn load_preset(&self, id: NowPlayingPresetId) -> Result<(), PresetError> {
        let settings = self
            .presets
            .borrow()
            .get(id)
            .map(|preset| preset.settings)
            .ok_or(PresetError::NotFound)?;
        self.cancel_all();
        self.settings.set(settings);
        self.selected_preset_id.set(Some(id));
        self.send(true);
        Ok(())
    }

    pub(crate) fn update_preset(&self, id: NowPlayingPresetId) -> Result<(), PresetError> {
        self.presets.borrow_mut().update(id, self.settings())?;
        self.selected_preset_id.set(Some(id));
        self.send_presets();
        Ok(())
    }

    pub(crate) fn rename_preset(
        &self,
        id: NowPlayingPresetId,
        name: &str,
    ) -> Result<(), PresetError> {
        self.presets.borrow_mut().rename(id, name)?;
        self.send_presets();
        Ok(())
    }

    pub(crate) fn delete_preset(&self, id: NowPlayingPresetId) -> Result<(), PresetError> {
        self.presets.borrow_mut().delete(id)?;
        if self.selected_preset_id() == Some(id) {
            self.selected_preset_id.set(None);
        }
        self.send_presets();
        Ok(())
    }

    pub(crate) fn update(&self, change: NowPlayingPreferenceChange) {
        if matches!(change, NowPlayingPreferenceChange::Reset) {
            self.reset();
            return;
        }
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
        self.cancel_all();
        self.apply(NowPlayingPreferenceChange::Reset);
        let monitor_changed = self.fullscreen_monitor.borrow_mut().take().is_some();
        if monitor_changed {
            // The settings message that follows performs the single disk write.
            self.send_fullscreen_monitor(false);
        }
        self.send(true);
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
            let mut preferences = preferences.lock().unwrap();
            preferences.set_now_playing(controller.settings(), false);
            preferences.set_now_playing_fullscreen_monitor(controller.fullscreen_monitor(), false);
            preferences.set_now_playing_presets(controller.presets(), true);
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

    fn send_presets(&self) {
        if let Some(sender) = self.gui_tx.as_ref()
            && let Err(error) = sender.try_send(GUIMessage::NowPlayingPresetCatalogChanged {
                presets: self.presets(),
            })
        {
            log::error!("Failed to update Now Playing presets: {error}");
        }
    }

    fn send_fullscreen_monitor(&self, persist: bool) {
        if let Some(sender) = self.gui_tx.as_ref()
            && let Err(error) = sender.try_send(GUIMessage::NowPlayingFullscreenMonitorChanged {
                target: self.fullscreen_monitor(),
                persist,
            })
        {
            log::error!("Failed to update the Now Playing fullscreen monitor: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::NowPlayingSettingsController;
    use crate::core::preferences::{
        BACKGROUND_MOTION_REVERSAL_DURATION_MIN_SECS, BACKGROUND_MOTION_ZOOM_MAX_PERCENT,
        CinemaArtworkFraming, CinemaCropFocus, DisplayMode, FullscreenMonitorTarget,
        NowPlayingPreferenceChange, NowPlayingPreferences, NowPlayingPresetCatalog, TextSize,
    };
    use crate::core::thread_messages::GUIMessage;

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
        controller.update(NowPlayingPreferenceChange::CinemaArtworkFraming(
            CinemaArtworkFraming::Fill,
        ));
        controller.update(NowPlayingPreferenceChange::CinemaCropFocus(
            CinemaCropFocus::TopRight,
        ));
        controller.update(NowPlayingPreferenceChange::DisplayMode(DisplayMode::Cinema));
        assert_eq!(
            controller.settings().cinema.artwork_framing,
            CinemaArtworkFraming::Fill
        );
        assert_eq!(
            controller.settings().cinema.crop_focus,
            CinemaCropFocus::TopRight
        );
        controller.update(NowPlayingPreferenceChange::BackgroundMotionEnabled(true));
        controller.update(NowPlayingPreferenceChange::BackgroundMotionZoomPercent(
            u16::MAX,
        ));
        controller.update(NowPlayingPreferenceChange::BackgroundMotionReversalDurationSecs(1));
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
        assert_eq!(settings.shared.background_motion_zoom_percent, 120);
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

    #[test]
    fn catalog_operations_do_not_emit_active_settings_updates() {
        let (sender, receiver) = async_channel::unbounded();
        let controller =
            NowPlayingSettingsController::new(NowPlayingPreferences::default(), Some(sender));

        let id = controller.create_preset("  Living room  ").unwrap();
        let catalog = match receiver.try_recv().unwrap() {
            GUIMessage::NowPlayingPresetCatalogChanged { presets } => presets,
            message => panic!("unexpected GUI message: {message:?}"),
        };
        assert_eq!(catalog.items()[0].name, "Living room");
        assert_eq!(controller.selected_preset_id(), Some(id));
        assert!(!controller.selected_preset_is_modified());

        controller.update(NowPlayingPreferenceChange::KeepScreenAwake(true));
        assert!(matches!(
            receiver.try_recv().unwrap(),
            GUIMessage::NowPlayingPreferenceChanged { .. }
        ));
        assert_eq!(controller.selected_preset_id(), Some(id));
        assert!(controller.selected_preset_is_modified());

        controller.update_preset(id).unwrap();
        assert!(matches!(
            receiver.try_recv().unwrap(),
            GUIMessage::NowPlayingPresetCatalogChanged { .. }
        ));
        assert!(!controller.selected_preset_is_modified());

        controller.rename_preset(id, "Projector").unwrap();
        let catalog = match receiver.try_recv().unwrap() {
            GUIMessage::NowPlayingPresetCatalogChanged { presets } => presets,
            message => panic!("unexpected GUI message: {message:?}"),
        };
        assert_eq!(catalog.get(id).unwrap().name, "Projector");

        controller.delete_preset(id).unwrap();
        assert!(matches!(
            receiver.try_recv().unwrap(),
            GUIMessage::NowPlayingPresetCatalogChanged { .. }
        ));
        assert_eq!(controller.selected_preset_id(), None);
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn monitor_selection_is_persisted_but_excluded_from_presets() {
        let (sender, receiver) = async_channel::unbounded();
        let controller =
            NowPlayingSettingsController::new(NowPlayingPreferences::default(), Some(sender));
        let preset_id = controller.create_preset("Projector").unwrap();
        assert!(matches!(
            receiver.try_recv().unwrap(),
            GUIMessage::NowPlayingPresetCatalogChanged { .. }
        ));

        let target = FullscreenMonitorTarget {
            connector: Some("HDMI-1".to_string()),
            description: Some("Projector".to_string()),
            manufacturer: Some("Epson".to_string()),
            model: Some("EH-TW7100".to_string()),
            width: 3840,
            height: 2160,
        };
        controller.update_fullscreen_monitor(Some(target.clone()));
        match receiver.try_recv().unwrap() {
            GUIMessage::NowPlayingFullscreenMonitorChanged {
                target: updated,
                persist,
            } => {
                assert_eq!(updated, Some(target));
                assert!(persist);
            }
            message => panic!("unexpected GUI message: {message:?}"),
        }
        assert_eq!(controller.selected_preset_id(), Some(preset_id));
        assert!(!controller.selected_preset_is_modified());

        controller.reset();
        assert!(matches!(
            receiver.try_recv().unwrap(),
            GUIMessage::NowPlayingFullscreenMonitorChanged {
                target: None,
                persist: false
            }
        ));
        assert!(matches!(
            receiver.try_recv().unwrap(),
            GUIMessage::NowPlayingPreferenceChanged { persist: true, .. }
        ));
        assert_eq!(controller.fullscreen_monitor(), None);
    }

    #[test]
    fn loading_a_preset_cancels_debounce_and_emits_one_atomic_snapshot() {
        let _serial = crate::MAIN_CONTEXT_TEST_LOCK.lock().unwrap();
        use std::time::Duration;

        let context = glib::MainContext::default();
        let _guard = context.acquire().unwrap();
        let mut saved = NowPlayingPreferences::default();
        saved.shared.text_size = TextSize::LARGE;
        let mut presets = NowPlayingPresetCatalog::default();
        let id = presets.create("Large", saved).unwrap();
        let (sender, receiver) = async_channel::unbounded();
        let controller = NowPlayingSettingsController::new_with_presets(
            NowPlayingPreferences::default(),
            presets,
            Some(sender),
        );

        controller.update_debounced(NowPlayingPreferenceChange::KeepScreenAwake(true));
        assert!(matches!(
            receiver.try_recv().unwrap(),
            GUIMessage::NowPlayingPreferenceChanged { persist: false, .. }
        ));

        controller.load_preset(id).unwrap();
        match receiver.try_recv().unwrap() {
            GUIMessage::NowPlayingPreferenceChanged { settings, persist } => {
                assert!(persist);
                assert_eq!(settings, saved);
            }
            message => panic!("unexpected GUI message: {message:?}"),
        }
        assert_eq!(controller.settings(), saved);
        assert_eq!(controller.selected_preset_id(), Some(id));
        assert!(!controller.selected_preset_is_modified());
        assert!(controller.pending_save.borrow().is_none());

        context.block_on(glib::timeout_future(Duration::from_millis(190)));
        assert!(receiver.try_recv().is_err());
    }
}
