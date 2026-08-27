//! Displays the floating Now Playing UI for the current song.
//!
//! The public façade keeps integration with the main window stable while the
//! implementation is divided by responsibility: widget construction, menu
//! bindings, persisted-preference application, track presentation, and
//! background rendering.

mod album_cover_size;
mod background;
mod controller;
mod main_preferences;
mod menu;
mod palette;
mod preferences;
mod state;
mod style;
mod track;
mod transition;
mod ui;

use adw::prelude::*;
use controller::NowPlayingSettingsController;
use menu::NowPlayingControls;
use state::NowPlayingState;
use ui::NowPlayingWidgets;

pub use crate::core::preferences::{
    AlbumCoverSize, BackgroundStyle, TrackInfoAlignment, TransitionEffect,
};
pub(crate) use crate::core::preferences::{
    NowPlayingPreferences as NowPlayingSettings, TRANSITION_DURATION_DEFAULT_MS,
    TRANSITION_DURATION_MAX_MS, TRANSITION_DURATION_MIN_MS, clamp_transition_duration_ms,
};
pub(crate) use controller::NowPlayingSettingsController as SettingsController;
pub(crate) use main_preferences::NowPlayingPreferencesView;
pub(crate) use transition::transition_duration_from_scale;

/// A reusable floating window that presents the current recognition result.
pub struct NowPlayingWindow {
    ui: NowPlayingWidgets,
    controls: NowPlayingControls,
    state: NowPlayingState,
    controller: NowPlayingSettingsController,
}

impl NowPlayingWindow {
    pub(crate) fn new_with_controller(controller: NowPlayingSettingsController) -> Self {
        let settings = controller.settings();
        let (ui, text_css) = ui::build_ui();
        let controls = menu::build_controls();
        let state = NowPlayingState::new(controller.settings_cell());

        let now_playing = Self {
            ui,
            controls,
            state,
            controller,
        };

        now_playing.setup_rendering(&text_css);
        now_playing.setup_track_transition_handlers();
        now_playing.apply_initial_preferences(settings);
        now_playing.setup_context_menu(settings);
        now_playing.connect_control_handlers();

        now_playing
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
