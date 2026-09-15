//! Displays the floating Now Playing UI for the current song.
//!
//! The public façade keeps integration with the main window stable while the
//! implementation is divided by responsibility: widget construction, menu
//! bindings, persisted-preference application, track presentation, and
//! background rendering.

mod album_cover_size;
mod background;
mod cinema_framing;
mod controller;
mod display_mode;
mod main_preferences;
mod menu;
mod motion;
mod palette;
#[cfg(test)]
mod performance_tests;
mod preferences;
#[cfg(test)]
mod regression_tests;
mod screen_awake;
mod state;
mod style;
mod text_size;
mod timing;
mod track;
mod transition;
mod tuning;
mod ui;

use crate::core::artwork_service::{ArtworkPolicy, ArtworkService};
use adw::prelude::*;
use controller::NowPlayingSettingsController;
use menu::NowPlayingControls;
use screen_awake::ScreenAwakeController;
use state::NowPlayingState;
use ui::NowPlayingWidgets;

pub use crate::core::preferences::{
    AlbumCoverSize, BackdropIntensity, BackgroundStyle, CinemaArtworkFraming, CinemaCropFocus,
    DisplayMode, ImmersiveBackgroundSource, TextSize, TrackInfoAlignment, TransitionEffect,
};
pub(crate) use crate::core::preferences::{
    BACKGROUND_MOTION_REVERSAL_DURATION_DEFAULT_SECS, BACKGROUND_MOTION_REVERSAL_DURATION_MAX_SECS,
    BACKGROUND_MOTION_REVERSAL_DURATION_MIN_SECS, BACKGROUND_MOTION_REVERSAL_DURATION_STEP_SECS,
    BACKGROUND_MOTION_ZOOM_DEFAULT_PERCENT, BACKGROUND_MOTION_ZOOM_MAX_PERCENT,
    BACKGROUND_MOTION_ZOOM_MIN_PERCENT, BACKGROUND_MOTION_ZOOM_STEP_PERCENT,
    NowPlayingPreferences as NowPlayingSettings, TRANSITION_DURATION_DEFAULT_MS,
    TRANSITION_DURATION_MAX_MS, TRANSITION_DURATION_MIN_MS, clamp_background_motion_zoom_percent,
    clamp_transition_duration_ms, normalize_background_motion_reversal_duration_secs,
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
    text_css: style::TextCss,
    applied_settings: std::cell::Cell<Option<NowPlayingSettings>>,
    screen_awake: ScreenAwakeController,
    artwork_timing: Option<timing::ArtworkTimingProbe>,
    artist_background_service: ArtworkService,
}

impl NowPlayingWindow {
    #[cfg(test)]
    pub(crate) fn new_with_controller(controller: NowPlayingSettingsController) -> Self {
        Self::new(controller, None)
    }

    pub(crate) fn new_with_controller_and_application(
        controller: NowPlayingSettingsController,
        application: &impl IsA<gtk::Application>,
    ) -> Self {
        Self::new(controller, Some(application.as_ref()))
    }

    fn new(
        controller: NowPlayingSettingsController,
        application: Option<&gtk::Application>,
    ) -> Self {
        let settings = controller.settings();
        let (ui, text_css) = ui::build_ui();
        let controls = menu::build_controls();
        let state = NowPlayingState::new(controller.settings_cell());
        let screen_awake = ScreenAwakeController::new(application);

        let mut now_playing = Self {
            ui,
            controls,
            state,
            controller,
            text_css,
            applied_settings: std::cell::Cell::new(None),
            screen_awake,
            artwork_timing: None,
            artist_background_service: ArtworkService::new(ArtworkPolicy::Thumbnail),
        };

        now_playing.setup_rendering();
        now_playing.screen_awake.bind_window(&now_playing.ui.window);
        now_playing.setup_track_transition_handlers();
        now_playing.apply_initial_preferences(settings);
        now_playing.setup_context_menu(settings);
        now_playing.connect_control_handlers();

        if std::env::var("SONGREC_ARTWORK_TIMING").as_deref() == Ok("1") {
            now_playing.artwork_timing =
                Some(timing::ArtworkTimingProbe::attach(&now_playing, |sample| {
                    sample.log()
                }));
        }

        now_playing
    }

    /// Presents the Now Playing window to the user.
    pub fn present(&self) {
        self.ui.window.present();
        self.resume_artwork_preparation();
        self.ensure_artist_background();
    }

    /// Closes the window while keeping its internal state available for reuse.
    pub fn close(&self) {
        self.screen_awake.release();
        self.ui.window.close();
    }
}
