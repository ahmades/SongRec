//! Shared Now Playing settings state, persistence commands, and debouncing.

use super::NowPlayingSettings;
use crate::core::preferences::NowPlayingPreferenceChange;
use crate::core::thread_messages::GUIMessage;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

const PERSISTENCE_DEBOUNCE_MS: u64 = 150;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SettingKey {
    Reset,
    DisplayMode,
    RoundCorners,
    HideTrackInfo,
    TrackInfoAlignment,
    AlbumCoverSize,
    BackgroundStyle,
    AlwaysDisplayLastRecognizedSong,
    Transition,
    TransitionDuration,
}

impl From<NowPlayingPreferenceChange> for SettingKey {
    fn from(change: NowPlayingPreferenceChange) -> Self {
        match change {
            NowPlayingPreferenceChange::Reset => Self::Reset,
            NowPlayingPreferenceChange::DisplayMode(_) => Self::DisplayMode,
            NowPlayingPreferenceChange::RoundCorners(_) => Self::RoundCorners,
            NowPlayingPreferenceChange::HideTrackInfo(_) => Self::HideTrackInfo,
            NowPlayingPreferenceChange::TrackInfoAlignment(_) => Self::TrackInfoAlignment,
            NowPlayingPreferenceChange::AlbumCoverSize(_) => Self::AlbumCoverSize,
            NowPlayingPreferenceChange::BackgroundStyle(_) => Self::BackgroundStyle,
            NowPlayingPreferenceChange::AlwaysDisplayLastRecognizedSong(_) => {
                Self::AlwaysDisplayLastRecognizedSong
            }
            NowPlayingPreferenceChange::Transition(_) => Self::Transition,
            NowPlayingPreferenceChange::TransitionDurationMs(_) => Self::TransitionDuration,
        }
    }
}

/// One model shared by the preferences page, context menu, and renderer.
#[derive(Clone)]
pub(crate) struct NowPlayingSettingsController {
    settings: Rc<Cell<NowPlayingSettings>>,
    gui_tx: Option<async_channel::Sender<GUIMessage>>,
    pending: Rc<RefCell<HashMap<SettingKey, glib::SourceId>>>,
}

impl NowPlayingSettingsController {
    pub(crate) fn new(
        settings: NowPlayingSettings,
        gui_tx: Option<async_channel::Sender<GUIMessage>>,
    ) -> Self {
        Self {
            settings: Rc::new(Cell::new(settings)),
            gui_tx,
            pending: Rc::new(RefCell::new(HashMap::new())),
        }
    }

    pub(crate) fn settings(&self) -> NowPlayingSettings {
        self.settings.get()
    }

    pub(crate) fn settings_cell(&self) -> Rc<Cell<NowPlayingSettings>> {
        self.settings.clone()
    }

    pub(crate) fn update(&self, change: NowPlayingPreferenceChange) {
        if matches!(change, NowPlayingPreferenceChange::Reset) {
            self.reset();
            return;
        }
        self.cancel(SettingKey::from(change));
        self.apply(change);
        self.send(change, true);
    }

    /// Applies a value immediately to the shared model but coalesces disk persistence.
    pub(crate) fn update_debounced(&self, change: NowPlayingPreferenceChange) {
        if matches!(change, NowPlayingPreferenceChange::Reset) {
            self.reset();
            return;
        }
        let key = SettingKey::from(change);
        self.cancel(key);
        self.apply(change);
        self.send(change, false);

        let pending = self.pending.clone();
        let sender = self.gui_tx.clone();
        let source_id = glib::timeout_add_local_once(
            Duration::from_millis(PERSISTENCE_DEBOUNCE_MS),
            move || {
                pending.borrow_mut().remove(&key);
                if let Some(sender) = sender
                    && let Err(error) = sender.try_send(GUIMessage::NowPlayingPreferenceChanged {
                        change,
                        persist: true,
                    })
                {
                    log::error!("Failed to persist Now Playing preference: {error}");
                }
            },
        );
        self.pending.borrow_mut().insert(key, source_id);
    }

    pub(crate) fn reset(&self) {
        self.cancel_all();
        self.apply(NowPlayingPreferenceChange::Reset);
        self.send(NowPlayingPreferenceChange::Reset, true);
    }

    pub(crate) fn cancel_all(&self) {
        for (_, source_id) in self.pending.borrow_mut().drain() {
            source_id.remove();
        }
    }

    fn apply(&self, change: NowPlayingPreferenceChange) {
        let mut settings = self.settings.get();
        settings.apply_change(change);
        self.settings.set(settings);
    }

    fn send(&self, change: NowPlayingPreferenceChange, persist: bool) {
        if let Some(sender) = self.gui_tx.as_ref()
            && let Err(error) =
                sender.try_send(GUIMessage::NowPlayingPreferenceChanged { change, persist })
        {
            log::error!("Failed to update Now Playing preference: {error}");
        }
    }

    fn cancel(&self, key: SettingKey) {
        if let Some(source_id) = self.pending.borrow_mut().remove(&key) {
            source_id.remove();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::NowPlayingSettingsController;
    use crate::core::preferences::{
        DisplayMode, NowPlayingPreferenceChange, NowPlayingPreferences,
    };

    #[test]
    fn immediate_updates_and_reset_share_one_normalized_model() {
        let controller = NowPlayingSettingsController::new(NowPlayingPreferences::default(), None);

        controller.update(NowPlayingPreferenceChange::HideTrackInfo(true));
        assert!(controller.settings().shared.hide_track_info);

        controller.update(NowPlayingPreferenceChange::DisplayMode(
            DisplayMode::LightsOff,
        ));
        assert_eq!(controller.settings().display_mode, DisplayMode::LightsOff);
        assert!(controller.settings().shared.hide_track_info);

        controller.update(NowPlayingPreferenceChange::DisplayMode(
            DisplayMode::Classic,
        ));
        assert!(controller.settings().shared.hide_track_info);

        controller.update(NowPlayingPreferenceChange::DisplayMode(
            DisplayMode::Ambient,
        ));
        controller.reset();
        assert_eq!(controller.settings(), NowPlayingPreferences::default());
    }
}
