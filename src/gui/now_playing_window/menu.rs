//! Context-menu construction and preference-update signal bindings.

use super::track::TrackPresentation;
use super::ui::apply_classic_track_info_alignment;
use super::{
    AlbumCoverSize, BACKGROUND_MOTION_REVERSAL_DURATION_DEFAULT_SECS,
    BACKGROUND_MOTION_REVERSAL_DURATION_MAX_SECS, BACKGROUND_MOTION_REVERSAL_DURATION_MIN_SECS,
    BACKGROUND_MOTION_REVERSAL_DURATION_STEP_SECS, BACKGROUND_MOTION_ZOOM_DEFAULT_PERCENT,
    BACKGROUND_MOTION_ZOOM_MAX_PERCENT, BACKGROUND_MOTION_ZOOM_MIN_PERCENT,
    BACKGROUND_MOTION_ZOOM_STEP_PERCENT, BackgroundStyle, DisplayMode, NowPlayingSettings,
    NowPlayingWindow, TRANSITION_DURATION_DEFAULT_MS, TRANSITION_DURATION_MAX_MS,
    TRANSITION_DURATION_MIN_MS, TrackInfoAlignment, TransitionEffect,
    clamp_background_motion_zoom_percent, normalize_background_motion_reversal_duration_secs,
    transition_duration_from_scale,
};
use crate::core::preferences::NowPlayingPreferenceChange;
use adw::prelude::*;
use gettextrs::gettext;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

const TRANSITION_DURATION_STEP_MS: f64 = 100.0;
const FULLSCREEN_CURSOR_HIDE_DELAY_MS: u64 = 1_500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextMenuPointerAction {
    Open,
    Dismiss,
    Consume,
    Ignore,
}

fn context_menu_pointer_action(
    menu_visible: bool,
    clicked_inside: bool,
    button: u32,
    suppress_secondary_open: bool,
) -> ContextMenuPointerAction {
    if menu_visible {
        return if clicked_inside {
            ContextMenuPointerAction::Ignore
        } else {
            ContextMenuPointerAction::Dismiss
        };
    }

    if button == gdk::BUTTON_SECONDARY && suppress_secondary_open {
        ContextMenuPointerAction::Consume
    } else if button == gdk::BUTTON_SECONDARY {
        ContextMenuPointerAction::Open
    } else {
        ContextMenuPointerAction::Ignore
    }
}

/// Owns one reschedulable GLib timeout.
///
/// Replacing the pending source keeps high-frequency UI events from leaving a
/// queue of stale callbacks behind merely to discover that they are obsolete.
#[derive(Clone, Default)]
struct DebouncedAction {
    source_id: Rc<RefCell<Option<glib::SourceId>>>,
}

impl DebouncedAction {
    fn schedule(&self, delay: Duration, action: impl FnOnce() + 'static) {
        self.cancel();

        let source_id_for_callback = self.source_id.clone();
        let source_id = glib::timeout_add_local_once(delay, move || {
            source_id_for_callback.borrow_mut().take();
            action();
        });
        self.source_id.borrow_mut().replace(source_id);
    }

    fn cancel(&self) {
        if let Some(source_id) = self.source_id.borrow_mut().take() {
            source_id.remove();
        }
    }
}

fn menu_grid() -> gtk::Grid {
    gtk::Grid::builder()
        .row_spacing(6)
        .column_spacing(12)
        .hexpand(true)
        .build()
}

fn section_heading(title: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(title)
        .halign(gtk::Align::Start)
        .css_classes(["heading"])
        .build()
}

/// The context-menu controls whose state mirrors the active presentation settings.
pub(super) struct NowPlayingControls {
    pub(super) display_mode_menu: gtk::DropDown,
    pub(super) classic_settings: gtk::Box,
    pub(super) background_motion_settings: gtk::Box,
    pub(super) round_corners: gtk::Switch,
    pub(super) hide_track_info_label: gtk::Label,
    pub(super) hide_track_info: gtk::Switch,
    pub(super) background_motion_enabled_label: gtk::Label,
    pub(super) background_motion_enabled: gtk::Switch,
    pub(super) background_motion_zoom_label: gtk::Label,
    pub(super) background_motion_zoom: gtk::Scale,
    pub(super) background_motion_reversal_duration_label: gtk::Label,
    pub(super) background_motion_reversal_duration: gtk::Scale,
    pub(super) background_style_gradient: gtk::ToggleButton,
    pub(super) background_style_solid: gtk::ToggleButton,
    pub(super) track_info_alignment_left: gtk::ToggleButton,
    pub(super) track_info_alignment_center: gtk::ToggleButton,
    pub(super) track_info_alignment_right: gtk::ToggleButton,
    pub(super) album_cover_size: gtk::Scale,
    pub(super) always_display_last_recognized_song: gtk::Switch,
    pub(super) transition_menu: gtk::DropDown,
    pub(super) transition_duration: gtk::Scale,
    pub(super) fullscreen_button: gtk::Button,
    pub(super) fullscreen_button_content: adw::ButtonContent,
}

/// Creates the controls used by the Now Playing context menu.
pub(super) fn build_controls() -> NowPlayingControls {
    let display_mode_labels = DisplayMode::ALL
        .into_iter()
        .map(DisplayMode::translated_label)
        .collect::<Vec<_>>();
    let display_mode_label_references = display_mode_labels
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let display_mode_menu = gtk::DropDown::from_strings(&display_mode_label_references);
    let classic_settings = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    let background_motion_settings = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    let round_corners = gtk::Switch::new();
    let hide_track_info_label = gtk::Label::new(Some(&gettext("Hide track info")));
    let hide_track_info = gtk::Switch::new();
    let background_motion_enabled_label = gtk::Label::new(Some(&gettext("Subtle ambient motion")));
    let background_motion_enabled = gtk::Switch::new();
    let background_motion_zoom_label = gtk::Label::new(Some(&gettext("Zoom level (%)")));
    let background_motion_zoom = gtk::Scale::with_range(
        gtk::Orientation::Horizontal,
        f64::from(BACKGROUND_MOTION_ZOOM_MIN_PERCENT),
        f64::from(BACKGROUND_MOTION_ZOOM_MAX_PERCENT),
        f64::from(BACKGROUND_MOTION_ZOOM_STEP_PERCENT),
    );
    background_motion_zoom.set_value(f64::from(BACKGROUND_MOTION_ZOOM_DEFAULT_PERCENT));
    background_motion_zoom.set_digits(0);
    background_motion_zoom.set_draw_value(true);
    background_motion_zoom.set_width_request(190);
    let background_motion_reversal_duration_label =
        gtk::Label::new(Some(&gettext("Direction change interval (seconds)")));
    let background_motion_reversal_duration = gtk::Scale::with_range(
        gtk::Orientation::Horizontal,
        BACKGROUND_MOTION_REVERSAL_DURATION_MIN_SECS as f64,
        BACKGROUND_MOTION_REVERSAL_DURATION_MAX_SECS as f64,
        BACKGROUND_MOTION_REVERSAL_DURATION_STEP_SECS as f64,
    );
    background_motion_reversal_duration
        .set_value(BACKGROUND_MOTION_REVERSAL_DURATION_DEFAULT_SECS as f64);
    background_motion_reversal_duration.set_digits(0);
    background_motion_reversal_duration.set_draw_value(true);
    background_motion_reversal_duration.set_width_request(190);
    let album_cover_size = gtk::Scale::with_range(
        gtk::Orientation::Horizontal,
        AlbumCoverSize::MIN_SCALE_VALUE,
        AlbumCoverSize::MAX_SCALE_VALUE,
        AlbumCoverSize::SCALE_STEP,
    );
    AlbumCoverSize::configure_scale(&album_cover_size);
    AlbumCoverSize::install_slider_snap(&album_cover_size);
    album_cover_size.set_value(AlbumCoverSize::default().scale_value());
    album_cover_size.set_width_request(190);
    let always_display_last_recognized_song = gtk::Switch::new();
    let transition_labels: Vec<_> = TransitionEffect::ALL
        .into_iter()
        .map(TransitionEffect::translated_label)
        .collect();
    let transition_label_references: Vec<_> =
        transition_labels.iter().map(String::as_str).collect();
    let transition_menu = gtk::DropDown::from_strings(&transition_label_references);
    let transition_duration = gtk::Scale::with_range(
        gtk::Orientation::Horizontal,
        TRANSITION_DURATION_MIN_MS as f64,
        TRANSITION_DURATION_MAX_MS as f64,
        TRANSITION_DURATION_STEP_MS,
    );
    transition_duration.set_value(TRANSITION_DURATION_DEFAULT_MS as f64);
    transition_duration.set_digits(0);
    transition_duration.set_draw_value(true);
    transition_duration.set_hexpand(true);
    transition_duration.set_width_request(190);
    let fullscreen_button_content = adw::ButtonContent::new();
    let fullscreen_button = gtk::Button::builder()
        .halign(gtk::Align::Fill)
        .hexpand(true)
        .build();
    fullscreen_button.set_child(Some(&fullscreen_button_content));
    let background_style_gradient = gtk::ToggleButton::with_label(&gettext("Gradient"));
    let background_style_solid = gtk::ToggleButton::with_label(&gettext("Solid"));
    background_style_solid.set_group(Some(&background_style_gradient));
    let track_info_alignment_left = gtk::ToggleButton::with_label(&gettext("Left"));
    let track_info_alignment_center = gtk::ToggleButton::with_label(&gettext("Center"));
    let track_info_alignment_right = gtk::ToggleButton::with_label(&gettext("Right"));
    track_info_alignment_center.set_group(Some(&track_info_alignment_left));
    track_info_alignment_right.set_group(Some(&track_info_alignment_left));

    NowPlayingControls {
        display_mode_menu,
        classic_settings,
        background_motion_settings,
        round_corners,
        hide_track_info_label,
        hide_track_info,
        background_motion_enabled_label,
        background_motion_enabled,
        background_motion_zoom_label,
        background_motion_zoom,
        background_motion_reversal_duration_label,
        background_motion_reversal_duration,
        background_style_gradient,
        background_style_solid,
        track_info_alignment_left,
        track_info_alignment_center,
        track_info_alignment_right,
        album_cover_size,
        always_display_last_recognized_song,
        transition_menu,
        transition_duration,
        fullscreen_button,
        fullscreen_button_content,
    }
}

impl NowPlayingWindow {
    /// Builds and installs the right-click context menu for the Now Playing window.
    pub(super) fn setup_context_menu(&self, settings: NowPlayingSettings) {
        let popover = gtk::Popover::new();
        popover.set_has_arrow(false);
        let menu_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .build();
        menu_box.set_margin_top(12);
        menu_box.set_margin_bottom(12);
        menu_box.set_margin_start(12);
        menu_box.set_margin_end(12);

        Self::update_fullscreen_button(
            &self.controls.fullscreen_button_content,
            self.ui.window.is_fullscreen(),
        );
        menu_box.append(&self.controls.fullscreen_button);
        menu_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        let reset_button = gtk::Button::builder()
            .label(gettext("Reset"))
            .halign(gtk::Align::Fill)
            .hexpand(true)
            .build();
        menu_box.append(&reset_button);
        let controller_for_reset = self.controller.clone();
        reset_button.connect_clicked(move |_| {
            controller_for_reset.reset();
        });

        let shared_grid = menu_grid();
        self.add_display_mode_menu_row(
            &shared_grid,
            &self.controls.display_mode_menu,
            settings.display_mode,
        );
        self.add_switch_menu_row_with_label(
            &shared_grid,
            1,
            &self.controls.hide_track_info_label,
            &self.controls.hide_track_info,
            settings.shared.hide_track_info,
            true,
        );
        let show_hide_track_info = settings.display_mode.supports_hiding_track_info();
        self.controls
            .hide_track_info_label
            .set_visible(show_hide_track_info);
        self.controls
            .hide_track_info
            .set_visible(show_hide_track_info);
        self.add_switch_menu_row(
            &shared_grid,
            2,
            &gettext("Always display last recognized song"),
            &self.controls.always_display_last_recognized_song,
            settings.shared.always_display_last_recognized_song,
            true,
        );
        self.add_transition_menu_row(
            &shared_grid,
            3,
            &self.controls.transition_menu,
            settings.shared.transition,
        );
        self.add_transition_duration_menu_row(
            &shared_grid,
            4,
            &self.controls.transition_duration,
            settings.shared.transition_duration_ms,
            !matches!(settings.shared.transition, TransitionEffect::None),
        );
        menu_box.append(&shared_grid);

        let classic_heading = section_heading(&gettext("Classic settings"));
        self.controls.classic_settings.append(&classic_heading);
        let classic_grid = menu_grid();
        self.add_switch_menu_row(
            &classic_grid,
            0,
            &gettext("Round corners of album cover"),
            &self.controls.round_corners,
            settings.classic.round_corners,
            true,
        );
        self.add_alignment_menu_row(
            &classic_grid,
            1,
            settings.classic.track_info_alignment,
            !settings.shared.hide_track_info,
        );
        self.add_album_cover_size_menu_row(
            &classic_grid,
            2,
            settings.classic.album_cover_size,
            true,
        );
        self.add_background_style_menu_row(&classic_grid, 3, settings.classic.background_style);
        self.controls.classic_settings.append(&classic_grid);
        self.controls
            .classic_settings
            .set_visible(settings.display_mode.shows_classic_settings());
        menu_box.append(&self.controls.classic_settings);

        let background_motion_heading = section_heading(&gettext("Cinema and Ambient settings"));
        self.controls
            .background_motion_settings
            .append(&background_motion_heading);
        let background_motion_grid = menu_grid();
        self.add_switch_menu_row_with_label(
            &background_motion_grid,
            0,
            &self.controls.background_motion_enabled_label,
            &self.controls.background_motion_enabled,
            settings.shared.background_motion_enabled,
            true,
        );
        self.add_scale_menu_row_with_label(
            &background_motion_grid,
            1,
            &self.controls.background_motion_zoom_label,
            &self.controls.background_motion_zoom,
            f64::from(settings.shared.background_motion_zoom_percent),
        );
        self.add_scale_menu_row_with_label(
            &background_motion_grid,
            2,
            &self.controls.background_motion_reversal_duration_label,
            &self.controls.background_motion_reversal_duration,
            settings.shared.background_motion_reversal_duration_secs as f64,
        );
        self.controls
            .background_motion_settings
            .append(&background_motion_grid);
        self.update_background_motion_control_visibility(
            settings.display_mode,
            settings.shared.background_motion_enabled,
        );
        menu_box.append(&self.controls.background_motion_settings);

        let menu_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .propagate_natural_width(true)
            .propagate_natural_height(true)
            .max_content_height(520)
            .child(&menu_box)
            .build();
        popover.set_child(Some(&menu_scroll));
        popover.set_parent(&self.ui.window);
        // Keep GTK's modal grab for Escape and focus handling. Resolve pointer
        // clicks in capture phase so autohide cannot race a secondary click and
        // reinterpret the same sequence as a request to reopen the menu.
        popover.set_autohide(true);
        let suppress_secondary_open = Rc::new(Cell::new(false));
        let suppress_secondary_open_for_closed = suppress_secondary_open.clone();
        popover.connect_closed(move |_| {
            suppress_secondary_open_for_closed.set(true);
            let suppress_secondary_open_for_idle = suppress_secondary_open_for_closed.clone();
            glib::idle_add_local_once(move || {
                suppress_secondary_open_for_idle.set(false);
            });
        });

        let popover_for_pointer = popover.downgrade();
        let suppress_secondary_open_for_pointer = suppress_secondary_open;
        let pointer = gtk::GestureClick::new();
        pointer.set_button(0);
        pointer.set_propagation_phase(gtk::PropagationPhase::Capture);
        pointer.connect_pressed(move |gesture, _, x, y| {
            let Some(popover) = popover_for_pointer.upgrade() else {
                return;
            };
            let menu_visible = popover.is_visible();
            let clicked_inside = menu_visible
                && gesture
                    .widget()
                    .and_then(|window| {
                        window
                            .compute_point(&popover, &gtk::graphene::Point::new(x as f32, y as f32))
                    })
                    .is_some_and(|point| {
                        popover.contains(f64::from(point.x()), f64::from(point.y()))
                    });

            match context_menu_pointer_action(
                menu_visible,
                clicked_inside,
                gesture.current_button(),
                suppress_secondary_open_for_pointer.get(),
            ) {
                ContextMenuPointerAction::Open => {
                    let pointing_rect = gdk::Rectangle::new(x as i32, y as i32, 1, 1);
                    popover.set_pointing_to(Some(&pointing_rect));
                    popover.popup();
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                }
                ContextMenuPointerAction::Dismiss => {
                    popover.popdown();
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                }
                ContextMenuPointerAction::Consume => {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                }
                ContextMenuPointerAction::Ignore => {}
            }
        });
        self.ui.window.add_controller(pointer);

        let window_for_fullscreen_button = self.ui.window.downgrade();
        let popover_for_fullscreen_button = popover.downgrade();
        self.controls.fullscreen_button.connect_clicked(move |_| {
            let Some(window) = window_for_fullscreen_button.upgrade() else {
                return;
            };

            if let Some(popover) = popover_for_fullscreen_button.upgrade() {
                popover.popdown();
            }
            if window.is_fullscreen() {
                window.unfullscreen();
            } else {
                window.fullscreen();
            }
        });

        let fullscreen_cursor_hide = DebouncedAction::default();
        let window_for_cursor_motion = self.ui.window.downgrade();
        let cursor_hide_for_motion = fullscreen_cursor_hide.clone();
        let cursor_motion = gtk::EventControllerMotion::new();
        cursor_motion.set_propagation_phase(gtk::PropagationPhase::Capture);
        cursor_motion.connect_motion(move |_, _, _| {
            if let Some(window) = window_for_cursor_motion.upgrade() {
                Self::reveal_fullscreen_cursor(&window, &cursor_hide_for_motion);
            }
        });
        let window_for_cursor_enter = self.ui.window.downgrade();
        let cursor_hide_for_enter = fullscreen_cursor_hide.clone();
        cursor_motion.connect_enter(move |_, _, _| {
            if let Some(window) = window_for_cursor_enter.upgrade() {
                Self::reveal_fullscreen_cursor(&window, &cursor_hide_for_enter);
            }
        });
        self.ui.window.add_controller(cursor_motion);

        let fullscreen_button_content = self.controls.fullscreen_button_content.clone();
        let cursor_hide_for_fullscreen = fullscreen_cursor_hide;
        self.ui.window.connect_fullscreened_notify(move |window| {
            let fullscreened = window.is_fullscreen();
            cursor_hide_for_fullscreen.cancel();
            window.set_cursor_from_name(if fullscreened { Some("none") } else { None });
            Self::update_fullscreen_button(&fullscreen_button_content, fullscreened);
        });
    }

    /// Keeps the menu action in sync with fullscreen changes from any source.
    fn update_fullscreen_button(content: &adw::ButtonContent, fullscreened: bool) {
        if fullscreened {
            content.set_icon_name("view-restore-symbolic");
            content.set_label(&gettext("Exit full screen"));
        } else {
            content.set_icon_name("view-fullscreen-symbolic");
            content.set_label(&gettext("Enter full screen"));
        }
    }

    /// Temporarily reveals the cursor in fullscreen, then hides it after the pointer is idle.
    fn reveal_fullscreen_cursor(window: &gtk::Window, pending_hide: &DebouncedAction) {
        if !window.is_fullscreen() {
            pending_hide.cancel();
            return;
        }

        window.set_cursor_from_name(None);
        let window_for_cursor_timeout = window.downgrade();
        pending_hide.schedule(
            Duration::from_millis(FULLSCREEN_CURSOR_HIDE_DELAY_MS),
            move || {
                if let Some(window) = window_for_cursor_timeout.upgrade()
                    && window.is_fullscreen()
                {
                    window.set_cursor_from_name(Some("none"));
                }
            },
        );
    }

    /// Adds the first-class display-mode selector to the context menu.
    fn add_display_mode_menu_row(
        &self,
        menu_grid: &gtk::Grid,
        dropdown: &gtk::DropDown,
        display_mode: DisplayMode,
    ) {
        let label = gtk::Label::new(Some(&gettext("Display mode")));
        label.set_halign(gtk::Align::Start);
        label.set_valign(gtk::Align::Center);
        label.set_hexpand(true);
        menu_grid.attach(&label, 0, 0, 1, 1);
        dropdown.set_selected(display_mode.index());
        dropdown.set_halign(gtk::Align::End);
        dropdown.set_valign(gtk::Align::Center);
        menu_grid.attach(dropdown, 1, 0, 1, 1);
    }

    /// Adds a label-and-switch row to the context menu.
    fn add_switch_menu_row(
        &self,
        menu_grid: &gtk::Grid,
        row: i32,
        title: &str,
        switch: &gtk::Switch,
        active: bool,
        sensitive: bool,
    ) {
        let label = gtk::Label::new(Some(title));
        self.add_switch_menu_row_with_label(menu_grid, row, &label, switch, active, sensitive);
    }

    /// Adds a pre-built label-and-switch row to the context menu.
    fn add_switch_menu_row_with_label(
        &self,
        menu_grid: &gtk::Grid,
        row: i32,
        label: &gtk::Label,
        switch: &gtk::Switch,
        active: bool,
        sensitive: bool,
    ) {
        label.set_halign(gtk::Align::Start);
        label.set_valign(gtk::Align::Center);
        label.set_hexpand(true);
        menu_grid.attach(label, 0, row, 1, 1);
        switch.set_halign(gtk::Align::End);
        switch.set_valign(gtk::Align::Center);
        switch.set_active(active);
        switch.set_sensitive(sensitive);
        menu_grid.attach(switch, 1, row, 1, 1);
    }

    /// Adds a pre-built label-and-slider row whose widgets can later be hidden together.
    fn add_scale_menu_row_with_label(
        &self,
        menu_grid: &gtk::Grid,
        row: i32,
        label: &gtk::Label,
        scale: &gtk::Scale,
        value: f64,
    ) {
        label.set_halign(gtk::Align::Start);
        label.set_valign(gtk::Align::Center);
        label.set_hexpand(true);
        menu_grid.attach(label, 0, row, 1, 1);
        scale.set_value(value);
        scale.set_halign(gtk::Align::End);
        scale.set_valign(gtk::Align::Center);
        scale.set_hexpand(false);
        menu_grid.attach(scale, 1, row, 1, 1);
    }

    pub(super) fn update_background_motion_control_visibility(
        &self,
        display_mode: DisplayMode,
        enabled: bool,
    ) {
        let supported = display_mode.supports_background_motion();
        self.controls
            .background_motion_settings
            .set_visible(supported);
        let show_details = supported && enabled;
        self.controls
            .background_motion_zoom_label
            .set_visible(show_details);
        self.controls
            .background_motion_zoom
            .set_visible(show_details);
        self.controls
            .background_motion_reversal_duration_label
            .set_visible(show_details);
        self.controls
            .background_motion_reversal_duration
            .set_visible(show_details);
    }

    /// Adds the transition effect drop-down to the context menu and selects the saved effect.
    fn add_transition_menu_row(
        &self,
        menu_grid: &gtk::Grid,
        row: i32,
        dropdown: &gtk::DropDown,
        effect: TransitionEffect,
    ) {
        let label = gtk::Label::new(Some(&gettext("Transition effect")));
        label.set_halign(gtk::Align::Start);
        label.set_valign(gtk::Align::Center);
        label.set_hexpand(true);
        menu_grid.attach(&label, 0, row, 1, 1);
        dropdown.set_selected(effect.index());
        dropdown.set_halign(gtk::Align::End);
        dropdown.set_valign(gtk::Align::Center);
        dropdown.set_hexpand(false);
        menu_grid.attach(dropdown, 1, row, 1, 1);
    }

    /// Adds the transition-duration slider to the context menu.
    fn add_transition_duration_menu_row(
        &self,
        menu_grid: &gtk::Grid,
        row: i32,
        scale: &gtk::Scale,
        duration_ms: u64,
        sensitive: bool,
    ) {
        let label = gtk::Label::new(Some(&gettext("Transition duration (ms)")));
        label.set_halign(gtk::Align::Start);
        label.set_valign(gtk::Align::Center);
        label.set_hexpand(true);
        menu_grid.attach(&label, 0, row, 1, 1);
        scale.set_value(duration_ms as f64);
        scale.set_sensitive(sensitive);
        scale.set_halign(gtk::Align::End);
        scale.set_valign(gtk::Align::Center);
        scale.set_hexpand(false);
        menu_grid.attach(scale, 1, row, 1, 1);
    }

    /// Adds the continuous album-cover-size slider above the background-style control.
    fn add_album_cover_size_menu_row(
        &self,
        menu_grid: &gtk::Grid,
        row: i32,
        size: AlbumCoverSize,
        sensitive: bool,
    ) {
        let label = gtk::Label::new(Some(&gettext("Album cover size")));
        label.set_halign(gtk::Align::Start);
        label.set_valign(gtk::Align::Center);
        label.set_hexpand(true);
        menu_grid.attach(&label, 0, row, 1, 1);
        self.controls.album_cover_size.set_value(size.scale_value());
        self.controls.album_cover_size.set_sensitive(sensitive);
        self.controls.album_cover_size.set_halign(gtk::Align::End);
        self.controls
            .album_cover_size
            .set_valign(gtk::Align::Center);
        self.controls.album_cover_size.set_hexpand(false);
        menu_grid.attach(&self.controls.album_cover_size, 1, row, 1, 1);
    }

    fn add_alignment_menu_row(
        &self,
        menu_grid: &gtk::Grid,
        row: i32,
        alignment: TrackInfoAlignment,
        sensitive: bool,
    ) {
        let label = gtk::Label::new(Some(&gettext("Track info alignment")));
        label.set_halign(gtk::Align::Start);
        label.set_valign(gtk::Align::Center);
        label.set_hexpand(true);
        menu_grid.attach(&label, 0, row, 1, 1);
        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(0)
            .css_classes(["linked"])
            .halign(gtk::Align::End)
            .valign(gtk::Align::Center)
            .build();
        buttons.append(&self.controls.track_info_alignment_left);
        buttons.append(&self.controls.track_info_alignment_center);
        buttons.append(&self.controls.track_info_alignment_right);
        // Keep sensitivity on the retained controls themselves. If this local
        // container is disabled, later preference updates cannot effectively
        // re-enable its children because the container is not retained.
        self.controls
            .track_info_alignment_left
            .set_sensitive(sensitive);
        self.controls
            .track_info_alignment_center
            .set_sensitive(sensitive);
        self.controls
            .track_info_alignment_right
            .set_sensitive(sensitive);
        match alignment {
            TrackInfoAlignment::Left => self.controls.track_info_alignment_left.set_active(true),
            TrackInfoAlignment::Center => {
                self.controls.track_info_alignment_center.set_active(true)
            }
            TrackInfoAlignment::Right => self.controls.track_info_alignment_right.set_active(true),
        }
        menu_grid.attach(&buttons, 1, row, 1, 1);
    }

    /// Adds the background-style segmented control to the context menu.
    fn add_background_style_menu_row(
        &self,
        menu_grid: &gtk::Grid,
        row: i32,
        style: BackgroundStyle,
    ) {
        let label = gtk::Label::new(Some(&gettext("Background style")));
        label.set_halign(gtk::Align::Start);
        label.set_valign(gtk::Align::Center);
        label.set_hexpand(true);
        menu_grid.attach(&label, 0, row, 1, 1);
        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(0)
            .css_classes(["linked"])
            .halign(gtk::Align::End)
            .valign(gtk::Align::Center)
            .hexpand(false)
            .build();
        buttons.append(&self.controls.background_style_gradient);
        buttons.append(&self.controls.background_style_solid);
        match style {
            BackgroundStyle::Gradient => self.controls.background_style_gradient.set_active(true),
            BackgroundStyle::Solid => self.controls.background_style_solid.set_active(true),
        }
        menu_grid.attach(&buttons, 1, row, 1, 1);
    }

    /// Connects preference controls to GUI preference update messages.
    pub(super) fn connect_control_handlers(&self) {
        let applying_settings_for_display_mode = self.state.applying_settings.clone();
        let controller_for_display_mode = self.controller.clone();
        let classic_settings_for_display_mode = self.controls.classic_settings.clone();
        let hide_track_info_label_for_display_mode = self.controls.hide_track_info_label.clone();
        let hide_track_info_for_display_mode = self.controls.hide_track_info.clone();
        let background_motion_settings_for_display_mode =
            self.controls.background_motion_settings.clone();
        let background_motion_enabled_for_display_mode =
            self.controls.background_motion_enabled.clone();
        let background_motion_zoom_label_for_display_mode =
            self.controls.background_motion_zoom_label.clone();
        let background_motion_zoom_for_display_mode = self.controls.background_motion_zoom.clone();
        let background_motion_reversal_label_for_display_mode = self
            .controls
            .background_motion_reversal_duration_label
            .clone();
        let background_motion_reversal_for_display_mode =
            self.controls.background_motion_reversal_duration.clone();
        let presentation_for_display_mode = TrackPresentation::from_window(self);
        self.controls
            .display_mode_menu
            .connect_selected_notify(move |dropdown| {
                if applying_settings_for_display_mode.get() {
                    return;
                }

                let display_mode = DisplayMode::from_index(dropdown.selected());
                controller_for_display_mode
                    .update(NowPlayingPreferenceChange::DisplayMode(display_mode));
                classic_settings_for_display_mode
                    .set_visible(display_mode.shows_classic_settings());
                hide_track_info_label_for_display_mode
                    .set_visible(display_mode.supports_hiding_track_info());
                hide_track_info_for_display_mode
                    .set_visible(display_mode.supports_hiding_track_info());
                let supports_background_motion = display_mode.supports_background_motion();
                background_motion_settings_for_display_mode.set_visible(supports_background_motion);
                let show_motion_details = supports_background_motion
                    && background_motion_enabled_for_display_mode.is_active();
                background_motion_zoom_label_for_display_mode.set_visible(show_motion_details);
                background_motion_zoom_for_display_mode.set_visible(show_motion_details);
                background_motion_reversal_label_for_display_mode.set_visible(show_motion_details);
                background_motion_reversal_for_display_mode.set_visible(show_motion_details);
                presentation_for_display_mode.refresh_mode();
            });

        let applying_settings_for_round_corners = self.state.applying_settings.clone();
        let controller_for_round_corners = self.controller.clone();
        let artwork_overlay_for_round_corners = self.ui.artwork_overlay.clone();
        self.controls
            .round_corners
            .connect_active_notify(move |switch| {
                if applying_settings_for_round_corners.get() {
                    return;
                }

                let active = switch.is_active();
                controller_for_round_corners
                    .update(NowPlayingPreferenceChange::RoundCorners(active));
                if active {
                    artwork_overlay_for_round_corners.add_css_class("now-playing-artwork-rounded");
                } else {
                    artwork_overlay_for_round_corners
                        .remove_css_class("now-playing-artwork-rounded");
                }
            });

        let applying_settings_for_hide = self.state.applying_settings.clone();
        let controller_for_hide = self.controller.clone();
        let info_box_for_hide = self.ui.info_box.clone();
        let alignment_left_for_hide = self.controls.track_info_alignment_left.clone();
        let alignment_center_for_hide = self.controls.track_info_alignment_center.clone();
        let alignment_right_for_hide = self.controls.track_info_alignment_right.clone();
        let presentation_for_hide = TrackPresentation::from_window(self);
        self.controls
            .hide_track_info
            .connect_active_notify(move |button| {
                if applying_settings_for_hide.get() {
                    return;
                }

                let hide_track_info = button.is_active();
                controller_for_hide
                    .update(NowPlayingPreferenceChange::HideTrackInfo(hide_track_info));
                info_box_for_hide.set_visible(!hide_track_info);
                alignment_left_for_hide.set_sensitive(!hide_track_info);
                alignment_center_for_hide.set_sensitive(!hide_track_info);
                alignment_right_for_hide.set_sensitive(!hide_track_info);
                presentation_for_hide.refresh_mode();
            });

        let applying_settings_for_background_motion = self.state.applying_settings.clone();
        let controller_for_background_motion = self.controller.clone();
        let background_motion_zoom_label = self.controls.background_motion_zoom_label.clone();
        let background_motion_zoom = self.controls.background_motion_zoom.clone();
        let background_motion_reversal_label = self
            .controls
            .background_motion_reversal_duration_label
            .clone();
        let background_motion_reversal = self.controls.background_motion_reversal_duration.clone();
        let presentation_for_background_motion = TrackPresentation::from_window(self);
        self.controls
            .background_motion_enabled
            .connect_active_notify(move |switch| {
                if applying_settings_for_background_motion.get() {
                    return;
                }

                let enabled = switch.is_active();
                controller_for_background_motion
                    .update(NowPlayingPreferenceChange::BackgroundMotionEnabled(enabled));
                background_motion_zoom_label.set_visible(enabled);
                background_motion_zoom.set_visible(enabled);
                background_motion_reversal_label.set_visible(enabled);
                background_motion_reversal.set_visible(enabled);
                presentation_for_background_motion.refresh_mode();
            });

        let applying_settings_for_background_motion_zoom = self.state.applying_settings.clone();
        let controller_for_background_motion_zoom = self.controller.clone();
        let presentation_for_background_motion_zoom = TrackPresentation::from_window(self);
        self.controls
            .background_motion_zoom
            .connect_value_changed(move |scale| {
                if applying_settings_for_background_motion_zoom.get() {
                    return;
                }

                let zoom_percent =
                    clamp_background_motion_zoom_percent(scale.value().round().max(0.0) as u16);
                controller_for_background_motion_zoom.update_debounced(
                    NowPlayingPreferenceChange::BackgroundMotionZoomPercent(zoom_percent),
                );
                presentation_for_background_motion_zoom.refresh_mode();
            });

        let applying_settings_for_background_motion_duration = self.state.applying_settings.clone();
        let controller_for_background_motion_duration = self.controller.clone();
        let presentation_for_background_motion_duration = TrackPresentation::from_window(self);
        self.controls
            .background_motion_reversal_duration
            .connect_value_changed(move |scale| {
                if applying_settings_for_background_motion_duration.get() {
                    return;
                }

                let duration_secs = normalize_background_motion_reversal_duration_secs(
                    scale.value().round().max(0.0) as u64,
                );
                controller_for_background_motion_duration.update_debounced(
                    NowPlayingPreferenceChange::BackgroundMotionReversalDurationSecs(duration_secs),
                );
                presentation_for_background_motion_duration.refresh_mode();
            });

        let applying_settings_for_alignment_left = self.state.applying_settings.clone();
        let controller_for_alignment_left = self.controller.clone();
        let info_box_for_alignment_left = self.ui.info_box.clone();
        let title_for_alignment_left = self.ui.title_label.clone();
        let artist_for_alignment_left = self.ui.artist_label.clone();
        let album_for_alignment_left = self.ui.album_label.clone();
        let details_for_alignment_left = self.ui.details_label.clone();
        self.controls
            .track_info_alignment_left
            .connect_toggled(move |button| {
                if applying_settings_for_alignment_left.get() || !button.is_active() {
                    return;
                }

                controller_for_alignment_left.update(
                    NowPlayingPreferenceChange::TrackInfoAlignment(TrackInfoAlignment::Left),
                );
                apply_classic_track_info_alignment(
                    &info_box_for_alignment_left,
                    [
                        &title_for_alignment_left,
                        &artist_for_alignment_left,
                        &album_for_alignment_left,
                        &details_for_alignment_left,
                    ],
                    TrackInfoAlignment::Left,
                );
            });

        let applying_settings_for_alignment_center = self.state.applying_settings.clone();
        let controller_for_alignment_center = self.controller.clone();
        let info_box_for_alignment_center = self.ui.info_box.clone();
        let title_for_alignment_center = self.ui.title_label.clone();
        let artist_for_alignment_center = self.ui.artist_label.clone();
        let album_for_alignment_center = self.ui.album_label.clone();
        let details_for_alignment_center = self.ui.details_label.clone();
        self.controls
            .track_info_alignment_center
            .connect_toggled(move |button| {
                if applying_settings_for_alignment_center.get() || !button.is_active() {
                    return;
                }

                controller_for_alignment_center.update(
                    NowPlayingPreferenceChange::TrackInfoAlignment(TrackInfoAlignment::Center),
                );
                apply_classic_track_info_alignment(
                    &info_box_for_alignment_center,
                    [
                        &title_for_alignment_center,
                        &artist_for_alignment_center,
                        &album_for_alignment_center,
                        &details_for_alignment_center,
                    ],
                    TrackInfoAlignment::Center,
                );
            });

        let applying_settings_for_alignment_right = self.state.applying_settings.clone();
        let controller_for_alignment_right = self.controller.clone();
        let info_box_for_alignment_right = self.ui.info_box.clone();
        let title_for_alignment_right = self.ui.title_label.clone();
        let artist_for_alignment_right = self.ui.artist_label.clone();
        let album_for_alignment_right = self.ui.album_label.clone();
        let details_for_alignment_right = self.ui.details_label.clone();
        self.controls
            .track_info_alignment_right
            .connect_toggled(move |button| {
                if applying_settings_for_alignment_right.get() || !button.is_active() {
                    return;
                }

                controller_for_alignment_right.update(
                    NowPlayingPreferenceChange::TrackInfoAlignment(TrackInfoAlignment::Right),
                );
                apply_classic_track_info_alignment(
                    &info_box_for_alignment_right,
                    [
                        &title_for_alignment_right,
                        &artist_for_alignment_right,
                        &album_for_alignment_right,
                        &details_for_alignment_right,
                    ],
                    TrackInfoAlignment::Right,
                );
            });

        let applying_settings_for_album_cover_size = self.state.applying_settings.clone();
        let album_cover_layout = self.ui.album_cover_layout.clone();
        let controller_for_album_cover_size = self.controller.clone();
        self.controls
            .album_cover_size
            .connect_value_changed(move |scale| {
                if applying_settings_for_album_cover_size.get() {
                    return;
                }

                let size = AlbumCoverSize::from_scale_value(scale.value());
                album_cover_layout.set_size(size);
                controller_for_album_cover_size
                    .update_debounced(NowPlayingPreferenceChange::AlbumCoverSize(size));
            });

        let applying_settings_for_always_display_last = self.state.applying_settings.clone();
        let controller_for_always_display_last = self.controller.clone();
        self.controls
            .always_display_last_recognized_song
            .connect_active_notify(move |button| {
                if applying_settings_for_always_display_last.get() {
                    return;
                }

                let always_display_last_recognized_song = button.is_active();
                controller_for_always_display_last.update(
                    NowPlayingPreferenceChange::AlwaysDisplayLastRecognizedSong(
                        always_display_last_recognized_song,
                    ),
                );
            });

        let applying_settings_for_transition = self.state.applying_settings.clone();
        let controller_for_transition = self.controller.clone();
        let transition_duration_control = self.controls.transition_duration.clone();
        self.controls
            .transition_menu
            .connect_selected_notify(move |dropdown| {
                if applying_settings_for_transition.get() {
                    return;
                }

                let effect = TransitionEffect::from_index(dropdown.selected());
                controller_for_transition.update(NowPlayingPreferenceChange::Transition(effect));
                transition_duration_control
                    .set_sensitive(!matches!(effect, TransitionEffect::None));
            });

        let applying_settings_for_duration = self.state.applying_settings.clone();
        let controller_for_transition_duration = self.controller.clone();
        self.controls
            .transition_duration
            .connect_value_changed(move |scale| {
                if applying_settings_for_duration.get() {
                    return;
                }

                let duration_ms = transition_duration_from_scale(scale.value());
                controller_for_transition_duration.update_debounced(
                    NowPlayingPreferenceChange::TransitionDurationMs(duration_ms),
                );
            });

        let applying_settings_for_gradient = self.state.applying_settings.clone();
        let controller_for_gradient = self.controller.clone();
        let background_area_for_gradient = self.ui.background_area.clone();
        self.controls
            .background_style_gradient
            .connect_toggled(move |button| {
                if applying_settings_for_gradient.get() || !button.is_active() {
                    return;
                }

                controller_for_gradient.update(NowPlayingPreferenceChange::BackgroundStyle(
                    BackgroundStyle::Gradient,
                ));
                background_area_for_gradient.queue_draw();
            });

        let applying_settings_for_solid = self.state.applying_settings.clone();
        let controller_for_solid = self.controller.clone();
        let background_area_for_solid = self.ui.background_area.clone();
        self.controls
            .background_style_solid
            .connect_toggled(move |button| {
                if applying_settings_for_solid.get() || !button.is_active() {
                    return;
                }

                controller_for_solid.update(NowPlayingPreferenceChange::BackgroundStyle(
                    BackgroundStyle::Solid,
                ));
                background_area_for_solid.queue_draw();
            });
    }
}

#[cfg(test)]
mod tests {
    use super::{ContextMenuPointerAction, context_menu_pointer_action};

    #[test]
    fn context_menu_pointer_actions_do_not_reopen_an_outside_secondary_click() {
        assert_eq!(
            context_menu_pointer_action(false, false, gdk::BUTTON_SECONDARY, false),
            ContextMenuPointerAction::Open
        );
        assert_eq!(
            context_menu_pointer_action(true, false, gdk::BUTTON_SECONDARY, false),
            ContextMenuPointerAction::Dismiss
        );
        assert_eq!(
            context_menu_pointer_action(true, false, gdk::BUTTON_PRIMARY, false),
            ContextMenuPointerAction::Dismiss
        );
        assert_eq!(
            context_menu_pointer_action(true, true, gdk::BUTTON_SECONDARY, false),
            ContextMenuPointerAction::Ignore
        );
        assert_eq!(
            context_menu_pointer_action(true, true, gdk::BUTTON_PRIMARY, false),
            ContextMenuPointerAction::Ignore
        );
        assert_eq!(
            context_menu_pointer_action(false, false, gdk::BUTTON_PRIMARY, false),
            ContextMenuPointerAction::Ignore
        );
        assert_eq!(
            context_menu_pointer_action(false, false, gdk::BUTTON_SECONDARY, true),
            ContextMenuPointerAction::Consume
        );
    }
}
