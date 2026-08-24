//! Displays the floating Now Playing UI for the current song.
//!
//! The public façade keeps integration with the main window stable while the
//! implementation is divided by responsibility: widget construction, menu
//! bindings, persisted-preference application, track presentation, and
//! background rendering.

mod background;
mod menu;
mod palette;
mod preferences;
mod state;
mod style;
mod track;
mod types;
mod ui;

use crate::core::preferences::{Preferences, PreferencesInterface};
use crate::core::thread_messages::GUIMessage;
use adw::prelude::*;
use menu::NowPlayingControls;
use state::NowPlayingState;
use std::sync::{Arc, Mutex};
use ui::NowPlayingWidgets;

pub use types::{BackgroundStyle, TrackInfoAlignment, TransitionEffect};
pub(crate) use types::{
    NowPlayingSettings, TRANSITION_DURATION_DEFAULT_MS, TRANSITION_DURATION_MAX_MS,
    TRANSITION_DURATION_MIN_MS, clamp_transition_duration_ms, reconcile_transition_duration,
    transition_duration_from_scale,
};

/// A reusable floating window that presents the current recognition result.
pub struct NowPlayingWindow {
    ui: NowPlayingWidgets,
    controls: NowPlayingControls,
    state: NowPlayingState,
    gui_tx: Option<async_channel::Sender<GUIMessage>>,
}

impl NowPlayingWindow {
    /// Constructs a Now Playing window initialized from the current preferences.
    pub fn new_with_settings(
        gui_tx: Option<async_channel::Sender<GUIMessage>>,
        preferences_interface: Option<Arc<Mutex<PreferencesInterface>>>,
    ) -> Self {
        let preferences = Self::current_preferences(preferences_interface.as_ref());
        let settings = NowPlayingSettings::from(&preferences);
        let (ui, text_css) = ui::build_ui();
        let controls = menu::build_controls();
        let state = NowPlayingState::new(settings);

        let now_playing = Self {
            ui,
            controls,
            state,
            gui_tx,
        };

        now_playing.setup_rendering(&text_css);
        now_playing.apply_initial_preferences(settings);
        now_playing.setup_context_menu(settings);
        now_playing.connect_control_handlers();

        now_playing
    }

    /// Reads the current preferences from the shared interface, if available.
    fn current_preferences(
        preferences_interface: Option<&Arc<Mutex<PreferencesInterface>>>,
    ) -> Preferences {
        preferences_interface
            .map(|interface| interface.lock().unwrap().preferences.clone())
            .unwrap_or_default()
    }

    /// Presents the Now Playing window to the user.
    pub fn present(&self) {
        self.ui.window.present();
    }

    /// Closes the window while keeping its internal state available for reuse.
    pub fn close(&self) {
        self.ui.window.close();
    }
}
