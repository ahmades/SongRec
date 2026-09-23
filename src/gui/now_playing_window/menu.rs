//! Context-menu construction and preference-update signal bindings.

use super::cinema_framing::{CinemaArtworkFramingControls, CinemaCropFocusControls};
use super::display_mode::{DisplayModeControls, NowPlayingControlState};
use super::displayed_information::{DisplayedInformationEditor, selection_summary};
use super::monitor_selection::{MonitorSelector, reapply_fullscreen_target, toggle_fullscreen};
use super::presets::PresetManagerLauncher;
use super::{
    AlbumCoverSize, BackgroundStyle, CinemaArtworkFraming, CinemaCropFocus, DisplayMode,
    ImmersiveBackgroundSource, NowPlayingSettings, NowPlayingWindow, TextSize, TrackInfoAlignment,
    TransitionEffect, transition_duration_from_scale,
};
use crate::core::preferences::{BackdropIntensity, NowPlayingPreferenceChange};
use adw::prelude::*;
use gettextrs::gettext;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

const FULLSCREEN_CURSOR_HIDE_DELAY_MS: u64 = 1_500;
const DEPENDENT_ROW_INDENT: i32 = 12;
const DEPENDENT_ROW_SPACING: i32 = 10;
const DEPENDENT_REVEAL_DURATION_MS: u32 = 150;

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

fn dependent_revealer() -> gtk::Revealer {
    gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideDown)
        .transition_duration(DEPENDENT_REVEAL_DURATION_MS)
        .reveal_child(false)
        .build()
}

/// Places dependent rows behind a small inset and a vertical guide so they
/// remain visibly associated with the setting that controls their relevance.
fn set_dependent_rows(revealer: &gtk::Revealer, grid: &gtk::Grid) {
    let guide = gtk::Separator::builder()
        .orientation(gtk::Orientation::Vertical)
        .accessible_role(gtk::AccessibleRole::None)
        .build();
    guide.set_vexpand(true);
    guide.set_margin_top(3);
    guide.set_margin_bottom(3);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(DEPENDENT_ROW_SPACING)
        .margin_start(DEPENDENT_ROW_INDENT)
        .build();
    content.append(&guide);
    content.append(grid);
    revealer.set_child(Some(&content));
}

fn section_heading(title: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(title)
        .halign(gtk::Align::Start)
        .css_classes(["heading"])
        .build()
}

fn label_control(label: &gtk::Label, control: &impl IsA<gtk::Widget>) {
    control
        .as_ref()
        .update_relation(&[gtk::accessible::Relation::LabelledBy(&[label.upcast_ref()])]);
}

/// Presents the preset manager only after the context popover has completely
/// closed. Waiting for `closed` avoids creating another transient surface
/// while GTK is still tearing down the popup.
fn connect_preset_manager_handoff(
    button: &gtk::Button,
    popover: &gtk::Popover,
    window: glib::WeakRef<gtk::Window>,
    manager: PresetManagerLauncher,
) {
    let pending = Rc::new(Cell::new(false));
    let pending_for_button = pending.clone();
    let popover_for_button = popover.downgrade();
    button.connect_clicked(move |_| {
        let Some(popover) = popover_for_button.upgrade() else {
            return;
        };
        pending_for_button.set(true);
        popover.popdown();
    });

    popover.connect_closed(move |_| {
        if !pending.replace(false) {
            return;
        }
        let window = window.clone();
        let manager = manager.clone();
        glib::idle_add_local_once(move || {
            if let Some(window) = window.upgrade()
                && window.is_visible()
            {
                manager.present(&window);
            }
        });
    });
}

/// The context-menu controls whose state mirrors the active presentation settings.
pub(super) struct NowPlayingControls {
    pub(super) context_menu: gtk::Popover,
    pub(super) display_mode: DisplayModeControls,
    pub(super) keep_screen_awake: gtk::Switch,
    pub(super) burn_in_protection: gtk::Switch,
    pub(super) burn_in_inactivity_minutes_label: gtk::Label,
    pub(super) burn_in_inactivity_minutes: gtk::SpinButton,
    pub(super) burn_in_details: gtk::Revealer,
    pub(super) classic_settings: gtk::Box,
    pub(super) cinema_settings: gtk::Box,
    pub(super) immersive_settings: gtk::Box,
    pub(super) round_corners: gtk::Switch,
    pub(super) hide_track_info_label: gtk::Label,
    pub(super) hide_track_info: gtk::Switch,
    pub(super) text_size_label: gtk::Label,
    pub(super) text_size: gtk::Scale,
    pub(super) text_size_details: gtk::Revealer,
    pub(super) displayed_information_label: gtk::Label,
    pub(super) displayed_information_button: gtk::ToggleButton,
    pub(super) displayed_information: DisplayedInformationEditor,
    pub(super) displayed_information_details: gtk::Revealer,
    pub(super) immersive_background_source_album_cover: gtk::ToggleButton,
    pub(super) immersive_background_source_artist: gtk::ToggleButton,
    pub(super) backdrop_intensity_soft: gtk::ToggleButton,
    pub(super) backdrop_intensity_balanced: gtk::ToggleButton,
    pub(super) backdrop_intensity_bold: gtk::ToggleButton,
    pub(super) background_motion_enabled_label: gtk::Label,
    pub(super) background_motion_enabled: gtk::Switch,
    pub(super) background_motion_zoom_label: gtk::Label,
    pub(super) background_motion_zoom: gtk::Scale,
    pub(super) background_motion_reversal_duration_label: gtk::Label,
    pub(super) background_motion_reversal_duration: gtk::Scale,
    pub(super) background_motion_details: gtk::Revealer,
    pub(super) background_style_gradient: gtk::ToggleButton,
    pub(super) background_style_solid: gtk::ToggleButton,
    pub(super) track_info_alignment_label: gtk::Label,
    pub(super) track_info_alignment_left: gtk::ToggleButton,
    pub(super) track_info_alignment_center: gtk::ToggleButton,
    pub(super) track_info_alignment_right: gtk::ToggleButton,
    pub(super) album_cover_size: gtk::Scale,
    pub(super) cinema_artwork_framing: CinemaArtworkFramingControls,
    pub(super) cinema_crop_focus_label: gtk::Label,
    pub(super) cinema_crop_focus: CinemaCropFocusControls,
    pub(super) cinema_crop_focus_details: gtk::Revealer,
    pub(super) always_display_last_recognized_song: gtk::Switch,
    pub(super) transition_menu: gtk::DropDown,
    pub(super) transition_duration: gtk::Scale,
    pub(super) transition_duration_details: gtk::Revealer,
    pub(super) fullscreen_monitor: MonitorSelector,
    pub(super) fullscreen_button: gtk::Button,
    pub(super) fullscreen_button_content: adw::ButtonContent,
}

/// Creates the controls used by the Now Playing context menu.
pub(super) fn build_controls() -> NowPlayingControls {
    let context_menu = gtk::Popover::new();
    let display_mode = DisplayModeControls::new();
    let keep_screen_awake = gtk::Switch::new();
    let burn_in_protection = gtk::Switch::new();
    let burn_in_inactivity_minutes_label = gtk::Label::new(Some(&gettext("Dim after (min)")));
    let burn_in_inactivity_minutes = gtk::SpinButton::with_range(
        f64::from(crate::core::preferences::BURN_IN_INACTIVITY_MIN_MINUTES),
        f64::from(crate::core::preferences::BURN_IN_INACTIVITY_MAX_MINUTES),
        f64::from(crate::core::preferences::BURN_IN_INACTIVITY_STEP_MINUTES),
    );
    burn_in_inactivity_minutes.set_numeric(true);
    burn_in_inactivity_minutes.set_snap_to_ticks(true);
    burn_in_inactivity_minutes.set_value(f64::from(
        crate::core::preferences::BURN_IN_INACTIVITY_DEFAULT_MINUTES,
    ));
    let burn_in_details = dependent_revealer();
    let classic_settings = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    let cinema_settings = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    let immersive_settings = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    let round_corners = gtk::Switch::new();
    let hide_track_info_label = gtk::Label::new(Some(&gettext("Hide track info")));
    let hide_track_info = gtk::Switch::new();
    let text_size_label = gtk::Label::new(Some(&gettext("Text size")));
    let text_size = gtk::Scale::with_range(
        gtk::Orientation::Horizontal,
        TextSize::MIN_SCALE_VALUE,
        TextSize::MAX_SCALE_VALUE,
        TextSize::SCALE_STEP,
    );
    TextSize::configure_scale(&text_size);
    TextSize::install_slider_snap(&text_size);
    text_size.set_value(TextSize::default().scale_value());
    text_size.set_width_request(190);
    let text_size_details = dependent_revealer();
    let displayed_information_label = gtk::Label::new(Some(&gettext("Displayed information")));
    let displayed_information_button = gtk::ToggleButton::builder()
        .label(selection_summary(super::DisplayedInformation::default()))
        .build();
    let displayed_information = DisplayedInformationEditor::new();
    let displayed_information_details = dependent_revealer();
    let immersive_background_source_album_cover =
        gtk::ToggleButton::with_label(&gettext("Album cover"));
    let immersive_background_source_artist = gtk::ToggleButton::with_label(&gettext("Artist"));
    immersive_background_source_artist.set_group(Some(&immersive_background_source_album_cover));
    let backdrop_intensity_soft = gtk::ToggleButton::with_label(&gettext("Soft"));
    let backdrop_intensity_balanced = gtk::ToggleButton::with_label(&gettext("Balanced"));
    let backdrop_intensity_bold = gtk::ToggleButton::with_label(&gettext("Bold"));
    backdrop_intensity_balanced.set_group(Some(&backdrop_intensity_soft));
    backdrop_intensity_bold.set_group(Some(&backdrop_intensity_soft));
    let background_motion_enabled_label = gtk::Label::new(Some(&gettext("Background motion")));
    let background_motion_enabled = gtk::Switch::new();
    let background_motion_zoom_label = gtk::Label::new(Some(&gettext("Zoom level (%)")));
    let background_motion_zoom =
        gtk::Scale::new(gtk::Orientation::Horizontal, None::<&gtk::Adjustment>);
    super::settings_scale::configure_background_motion_zoom_scale(&background_motion_zoom);
    background_motion_zoom.set_width_request(190);
    let background_motion_reversal_duration_label =
        gtk::Label::new(Some(&gettext("Direction change interval (s)")));
    let background_motion_reversal_duration =
        gtk::Scale::new(gtk::Orientation::Horizontal, None::<&gtk::Adjustment>);
    super::settings_scale::configure_background_motion_reversal_duration_scale(
        &background_motion_reversal_duration,
    );
    background_motion_reversal_duration.set_width_request(190);
    let background_motion_details = dependent_revealer();
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
    let cinema_artwork_framing = CinemaArtworkFramingControls::new();
    let cinema_crop_focus_label = gtk::Label::new(Some(&gettext("Crop focus")));
    let cinema_crop_focus = CinemaCropFocusControls::new();
    cinema_crop_focus.set_compact(true);
    let cinema_crop_focus_details = dependent_revealer();
    let always_display_last_recognized_song = gtk::Switch::new();
    let transition_labels: Vec<_> = TransitionEffect::ALL
        .into_iter()
        .map(TransitionEffect::translated_label)
        .collect();
    let transition_label_references: Vec<_> =
        transition_labels.iter().map(String::as_str).collect();
    let transition_menu = gtk::DropDown::from_strings(&transition_label_references);
    let transition_duration =
        gtk::Scale::new(gtk::Orientation::Horizontal, None::<&gtk::Adjustment>);
    super::transition::configure_transition_duration_scale(&transition_duration);
    transition_duration.set_hexpand(true);
    transition_duration.set_width_request(190);
    let transition_duration_details = dependent_revealer();
    let fullscreen_monitor = MonitorSelector::new(None);
    let fullscreen_button_content = adw::ButtonContent::new();
    let fullscreen_button = gtk::Button::builder()
        .halign(gtk::Align::Fill)
        .hexpand(true)
        .build();
    fullscreen_button.set_child(Some(&fullscreen_button_content));
    let background_style_gradient = gtk::ToggleButton::with_label(&gettext("Gradient"));
    let background_style_solid = gtk::ToggleButton::with_label(&gettext("Solid"));
    background_style_solid.set_group(Some(&background_style_gradient));
    let track_info_alignment_label = gtk::Label::new(Some(&gettext("Track info alignment")));
    let track_info_alignment_left = gtk::ToggleButton::with_label(&gettext("Left"));
    let track_info_alignment_center = gtk::ToggleButton::with_label(&gettext("Center"));
    let track_info_alignment_right = gtk::ToggleButton::with_label(&gettext("Right"));
    track_info_alignment_center.set_group(Some(&track_info_alignment_left));
    track_info_alignment_right.set_group(Some(&track_info_alignment_left));

    NowPlayingControls {
        context_menu,
        display_mode,
        keep_screen_awake,
        burn_in_protection,
        burn_in_inactivity_minutes_label,
        burn_in_inactivity_minutes,
        burn_in_details,
        classic_settings,
        cinema_settings,
        immersive_settings,
        round_corners,
        hide_track_info_label,
        hide_track_info,
        text_size_label,
        text_size,
        text_size_details,
        displayed_information_label,
        displayed_information_button,
        displayed_information,
        displayed_information_details,
        immersive_background_source_album_cover,
        immersive_background_source_artist,
        backdrop_intensity_soft,
        backdrop_intensity_balanced,
        backdrop_intensity_bold,
        background_motion_enabled_label,
        background_motion_enabled,
        background_motion_zoom_label,
        background_motion_zoom,
        background_motion_reversal_duration_label,
        background_motion_reversal_duration,
        background_motion_details,
        background_style_gradient,
        background_style_solid,
        track_info_alignment_label,
        track_info_alignment_left,
        track_info_alignment_center,
        track_info_alignment_right,
        album_cover_size,
        cinema_artwork_framing,
        cinema_crop_focus_label,
        cinema_crop_focus,
        cinema_crop_focus_details,
        always_display_last_recognized_song,
        transition_menu,
        transition_duration,
        transition_duration_details,
        fullscreen_monitor,
        fullscreen_button,
        fullscreen_button_content,
    }
}

impl NowPlayingWindow {
    /// Builds and installs the right-click context menu for the Now Playing window.
    pub(super) fn setup_context_menu(&self, settings: NowPlayingSettings) {
        let control_state = NowPlayingControlState::from_settings(settings);
        let popover = self.controls.context_menu.clone();
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

        self.controls
            .fullscreen_monitor
            .set_target(self.controller.fullscreen_monitor());
        let monitor_row = menu_grid();
        monitor_row.set_margin_start(DEPENDENT_ROW_INDENT);
        let monitor_label = gtk::Label::new(Some(&gettext("Fullscreen monitor")));
        monitor_label.set_halign(gtk::Align::Start);
        monitor_label.set_valign(gtk::Align::Center);
        monitor_label.set_hexpand(true);
        monitor_row.attach(&monitor_label, 0, 0, 1, 1);
        self.controls
            .fullscreen_monitor
            .widget()
            .set_width_request(210);
        label_control(&monitor_label, self.controls.fullscreen_monitor.widget());
        monitor_row.attach(self.controls.fullscreen_monitor.widget(), 1, 0, 1, 1);
        self.controls
            .fullscreen_monitor
            .show_only_with_multiple_monitors(&monitor_row);
        menu_box.append(&monitor_row);

        let window_for_topology = self.ui.window.downgrade();
        let controller_for_topology = self.controller.clone();
        self.controls
            .fullscreen_monitor
            .connect_topology_changed(move || {
                let Some(window) = window_for_topology.upgrade() else {
                    return;
                };
                let target = controller_for_topology.fullscreen_monitor();
                reapply_fullscreen_target(&window, target.as_ref());
            });
        menu_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        let presets_button = gtk::Button::builder()
            .label(gettext("Presets…"))
            .halign(gtk::Align::Fill)
            .hexpand(true)
            .build();
        menu_box.append(&presets_button);

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

        connect_preset_manager_handoff(
            &presets_button,
            &popover,
            self.ui.window.downgrade(),
            self.preset_manager.clone(),
        );

        let shared_grid = menu_grid();
        self.add_display_mode_menu_row(
            &shared_grid,
            &self.controls.display_mode,
            settings.display_mode,
        );
        self.add_switch_menu_row(
            &shared_grid,
            1,
            &gettext("Keep screen awake"),
            &self.controls.keep_screen_awake,
            settings.shared.keep_screen_awake,
            true,
        );
        self.add_switch_menu_row(
            &shared_grid,
            2,
            &gettext("Burn-in protection"),
            &self.controls.burn_in_protection,
            settings.shared.burn_in_protection_enabled,
            true,
        );
        let burn_in_grid = menu_grid();
        self.add_spin_menu_row_with_label(
            &burn_in_grid,
            0,
            &self.controls.burn_in_inactivity_minutes_label,
            &self.controls.burn_in_inactivity_minutes,
            settings.shared.burn_in_inactivity_minutes,
        );
        set_dependent_rows(&self.controls.burn_in_details, &burn_in_grid);
        shared_grid.attach(&self.controls.burn_in_details, 0, 3, 2, 1);
        self.add_switch_menu_row_with_label(
            &shared_grid,
            4,
            &self.controls.hide_track_info_label,
            &self.controls.hide_track_info,
            settings.shared.hide_track_info,
            true,
        );
        let text_size_grid = menu_grid();
        self.add_scale_menu_row_with_label(
            &text_size_grid,
            0,
            &self.controls.text_size_label,
            &self.controls.text_size,
            settings.shared.text_size.scale_value(),
        );
        self.controls
            .displayed_information_label
            .set_halign(gtk::Align::Start);
        self.controls
            .displayed_information_label
            .set_valign(gtk::Align::Center);
        self.controls.displayed_information_label.set_hexpand(true);
        text_size_grid.attach(&self.controls.displayed_information_label, 0, 1, 1, 1);
        self.controls
            .displayed_information
            .set_value(settings.shared.displayed_information);
        self.controls
            .displayed_information_button
            .set_label(&selection_summary(settings.shared.displayed_information));
        self.controls
            .displayed_information_button
            .set_halign(gtk::Align::End);
        self.controls
            .displayed_information_button
            .set_valign(gtk::Align::Center);
        label_control(
            &self.controls.displayed_information_label,
            &self.controls.displayed_information_button,
        );
        text_size_grid.attach(&self.controls.displayed_information_button, 1, 1, 1, 1);
        let displayed_information_grid = menu_grid();
        displayed_information_grid.attach(self.controls.displayed_information.widget(), 0, 0, 2, 1);
        set_dependent_rows(
            &self.controls.displayed_information_details,
            &displayed_information_grid,
        );
        text_size_grid.attach(&self.controls.displayed_information_details, 0, 2, 2, 1);
        set_dependent_rows(&self.controls.text_size_details, &text_size_grid);
        shared_grid.attach(&self.controls.text_size_details, 0, 5, 2, 1);
        self.add_switch_menu_row(
            &shared_grid,
            6,
            &gettext("Always display last recognized song"),
            &self.controls.always_display_last_recognized_song,
            settings.shared.always_display_last_recognized_song,
            true,
        );
        self.add_transition_menu_row(
            &shared_grid,
            7,
            &self.controls.transition_menu,
            settings.shared.transition,
        );
        let transition_duration_grid = menu_grid();
        self.add_transition_duration_menu_row(
            &transition_duration_grid,
            0,
            &self.controls.transition_duration,
            settings.shared.transition_duration_ms,
            true,
        );
        set_dependent_rows(
            &self.controls.transition_duration_details,
            &transition_duration_grid,
        );
        shared_grid.attach(&self.controls.transition_duration_details, 0, 8, 2, 1);
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
            control_state.track_info_alignment_sensitive,
        );
        self.add_album_cover_size_menu_row(
            &classic_grid,
            2,
            settings.classic.album_cover_size,
            true,
        );
        self.add_background_style_menu_row(&classic_grid, 3, settings.classic.background_style);
        self.controls.classic_settings.append(&classic_grid);
        menu_box.append(&self.controls.classic_settings);

        let cinema_heading = section_heading(&gettext("Cinema settings"));
        self.controls.cinema_settings.append(&cinema_heading);
        let cinema_grid = menu_grid();
        self.add_cinema_artwork_framing_menu_row(&cinema_grid, 0, settings.cinema.artwork_framing);
        let cinema_crop_focus_grid = menu_grid();
        self.add_cinema_crop_focus_menu_row(&cinema_crop_focus_grid, 0, settings.cinema.crop_focus);
        set_dependent_rows(
            &self.controls.cinema_crop_focus_details,
            &cinema_crop_focus_grid,
        );
        cinema_grid.attach(&self.controls.cinema_crop_focus_details, 0, 1, 2, 1);
        self.controls.cinema_settings.append(&cinema_grid);
        menu_box.append(&self.controls.cinema_settings);

        let immersive_heading = section_heading(&gettext("Cinema and Ambient settings"));
        self.controls.immersive_settings.append(&immersive_heading);
        let immersive_grid = menu_grid();
        self.add_immersive_background_source_menu_row(
            &immersive_grid,
            0,
            settings.shared.immersive_background_source,
        );
        self.add_backdrop_intensity_menu_row(
            &immersive_grid,
            1,
            settings.shared.backdrop_intensity,
        );
        self.add_switch_menu_row_with_label(
            &immersive_grid,
            2,
            &self.controls.background_motion_enabled_label,
            &self.controls.background_motion_enabled,
            settings.shared.background_motion_enabled,
            true,
        );
        let background_motion_grid = menu_grid();
        self.add_scale_menu_row_with_label(
            &background_motion_grid,
            0,
            &self.controls.background_motion_zoom_label,
            &self.controls.background_motion_zoom,
            f64::from(settings.shared.background_motion_zoom_percent),
        );
        self.add_scale_menu_row_with_label(
            &background_motion_grid,
            1,
            &self.controls.background_motion_reversal_duration_label,
            &self.controls.background_motion_reversal_duration,
            settings.shared.background_motion_reversal_duration_secs as f64,
        );
        set_dependent_rows(
            &self.controls.background_motion_details,
            &background_motion_grid,
        );
        immersive_grid.attach(&self.controls.background_motion_details, 0, 3, 2, 1);
        self.controls.immersive_settings.append(&immersive_grid);
        menu_box.append(&self.controls.immersive_settings);

        self.apply_control_state(control_state);

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
        // The dropdowns own native modal popups. Nesting those inside another
        // modal grab can strand canvas events after a dropdown is toggled closed.
        // Keep the outer menu nonmodal and own outside-click/Escape dismissal.
        popover.set_autohide(false);
        let suppress_secondary_open = Rc::new(Cell::new(false));
        let suppress_secondary_open_for_closed = suppress_secondary_open.clone();
        popover.connect_closed(move |_| {
            log::debug!("Now Playing menu: closed");
            suppress_secondary_open_for_closed.set(true);
            let suppress_secondary_open_for_idle = suppress_secondary_open_for_closed.clone();
            glib::idle_add_local_once(move || {
                suppress_secondary_open_for_idle.set(false);
            });
        });

        let popover_for_pointer = popover.downgrade();
        let suppress_secondary_open_for_pointer = suppress_secondary_open;
        // Handle each press independently; interrupted dropdown gestures must
        // not disable later outside-click dismissal on the parent window.
        let pointer = gtk::EventControllerLegacy::new();
        pointer.set_propagation_phase(gtk::PropagationPhase::Capture);
        pointer.connect_event(move |controller, event| {
            if event.event_type() != gdk::EventType::ButtonPress {
                return glib::Propagation::Proceed;
            }
            let Some(button) = event.downcast_ref::<gdk::ButtonEvent>() else {
                return glib::Propagation::Proceed;
            };
            let Some(popover) = popover_for_pointer.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let Some(window) = controller.widget() else {
                return glib::Propagation::Proceed;
            };
            let Some(native) = window.native() else {
                return glib::Propagation::Proceed;
            };
            // Events on a descendant popup belong to its controls, not the canvas.
            if event.surface() != native.surface() {
                return glib::Propagation::Proceed;
            }
            let Some((surface_x, surface_y)) = event.position() else {
                return glib::Propagation::Proceed;
            };
            let (offset_x, offset_y) = native.surface_transform();
            let (x, y) = (surface_x + offset_x, surface_y + offset_y);
            let menu_visible = popover.is_visible();
            let clicked_inside = menu_visible
                && window
                    .compute_point(&popover, &gtk::graphene::Point::new(x as f32, y as f32))
                    .is_some_and(|point| {
                        popover.contains(f64::from(point.x()), f64::from(point.y()))
                    });

            let action = context_menu_pointer_action(
                menu_visible,
                clicked_inside,
                button.button(),
                suppress_secondary_open_for_pointer.get(),
            );
            log::debug!("Now Playing menu: button={}, visible={menu_visible}, inside={clicked_inside}, action={action:?}", button.button());
            match action {
                ContextMenuPointerAction::Open => {
                    let pointing_rect = gdk::Rectangle::new(x as i32, y as i32, 1, 1);
                    popover.set_pointing_to(Some(&pointing_rect));
                    popover.popup();
                    glib::Propagation::Stop
                }
                ContextMenuPointerAction::Dismiss => {
                    popover.popdown();
                    glib::Propagation::Stop
                }
                ContextMenuPointerAction::Consume => glib::Propagation::Stop,
                ContextMenuPointerAction::Ignore => glib::Propagation::Proceed,
            }
        });
        self.ui.window.add_controller(pointer);

        let popover_for_keyboard = popover.downgrade();
        let keyboard = gtk::EventControllerKey::new();
        keyboard.connect_key_pressed(move |controller, key, _, modifiers| {
            if key == gdk::Key::Escape
                && let Some(popover) = popover_for_keyboard.upgrade()
                && popover.is_visible()
            {
                popover.popdown();
                return glib::Propagation::Stop;
            }
            let opens_menu = key == gdk::Key::Menu
                || (key == gdk::Key::F10 && modifiers.contains(gdk::ModifierType::SHIFT_MASK));
            if !opens_menu {
                return glib::Propagation::Proceed;
            }
            if let Some(popover) = popover_for_keyboard.upgrade() {
                if popover.is_visible() {
                    popover.popdown();
                } else if let Some(window) = controller.widget() {
                    popover.set_pointing_to(Some(&gdk::Rectangle::new(
                        window.width() / 2,
                        window.height() / 2,
                        1,
                        1,
                    )));
                    popover.popup();
                    // Nonmodal popovers do not acquire keyboard focus on their
                    // own. Start navigation at the first settings control.
                    popover.child_focus(gtk::DirectionType::TabForward);
                }
            }
            glib::Propagation::Stop
        });
        self.ui.window.add_controller(keyboard);

        let window_for_fullscreen_button = self.ui.window.downgrade();
        let popover_for_fullscreen_button = popover.downgrade();
        let controller_for_fullscreen_button = self.controller.clone();
        self.controls.fullscreen_button.connect_clicked(move |_| {
            let Some(window) = window_for_fullscreen_button.upgrade() else {
                return;
            };

            if let Some(popover) = popover_for_fullscreen_button.upgrade() {
                popover.popdown();
            }
            let target = controller_for_fullscreen_button.fullscreen_monitor();
            toggle_fullscreen(&window, target.as_ref());
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

    /// Synchronizes the menu and immediately retargets an active fullscreen window.
    pub(crate) fn apply_fullscreen_monitor(&self) {
        let target = self.controller.fullscreen_monitor();
        self.controls.fullscreen_monitor.set_target(target.clone());
        reapply_fullscreen_target(&self.ui.window, target.as_ref());
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
        controls: &DisplayModeControls,
        display_mode: DisplayMode,
    ) {
        let label = gtk::Label::new(Some(&gettext("Display mode")));
        label.set_halign(gtk::Align::Start);
        label.set_valign(gtk::Align::Center);
        label.set_hexpand(true);
        menu_grid.attach(&label, 0, 0, 1, 1);
        controls.set_value(display_mode);
        controls.widget().set_halign(gtk::Align::End);
        controls.widget().set_valign(gtk::Align::Center);
        label_control(&label, controls.widget());
        menu_grid.attach(controls.widget(), 1, 0, 1, 1);
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
        label_control(label, switch);
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
        label_control(label, scale);
        menu_grid.attach(scale, 1, row, 1, 1);
    }

    /// Adds a compact, exact numeric control to a dependent menu row.
    fn add_spin_menu_row_with_label(
        &self,
        menu_grid: &gtk::Grid,
        row: i32,
        label: &gtk::Label,
        spin: &gtk::SpinButton,
        value: u16,
    ) {
        label.set_halign(gtk::Align::Start);
        label.set_valign(gtk::Align::Center);
        label.set_hexpand(true);
        menu_grid.attach(label, 0, row, 1, 1);
        spin.set_value(f64::from(value));
        spin.set_halign(gtk::Align::End);
        spin.set_valign(gtk::Align::Center);
        label_control(label, spin);
        menu_grid.attach(spin, 1, row, 1, 1);
    }

    /// Applies the dependency policy shared by the context menu and the main
    /// preferences page without duplicating mode-specific conditions here.
    pub(super) fn apply_control_state(&self, state: NowPlayingControlState) {
        self.controls
            .classic_settings
            .set_visible(state.show_classic_settings);
        self.controls
            .cinema_settings
            .set_visible(state.show_cinema_settings);
        self.controls
            .immersive_settings
            .set_visible(state.show_immersive_settings);
        self.controls
            .hide_track_info_label
            .set_sensitive(state.hide_track_info_sensitive);
        self.controls
            .hide_track_info
            .set_sensitive(state.hide_track_info_sensitive);
        self.controls
            .text_size_details
            .set_reveal_child(state.show_text_size);
        self.controls
            .displayed_information_details
            .set_reveal_child(
                state.show_displayed_information
                    && self.controls.displayed_information_button.is_active(),
            );
        self.controls
            .burn_in_details
            .set_reveal_child(state.show_burn_in_details);
        self.controls
            .cinema_crop_focus_details
            .set_reveal_child(state.show_crop_focus);
        self.controls
            .background_motion_details
            .set_reveal_child(state.show_motion_details);
        self.controls
            .transition_duration_details
            .set_reveal_child(state.show_transition_duration);
        self.controls
            .track_info_alignment_label
            .set_sensitive(state.track_info_alignment_sensitive);
        self.controls
            .track_info_alignment_left
            .set_sensitive(state.track_info_alignment_sensitive);
        self.controls
            .track_info_alignment_center
            .set_sensitive(state.track_info_alignment_sensitive);
        self.controls
            .track_info_alignment_right
            .set_sensitive(state.track_info_alignment_sensitive);
    }

    /// Adds Cinema's framing selector without changing its responsive outer layout.
    fn add_cinema_artwork_framing_menu_row(
        &self,
        menu_grid: &gtk::Grid,
        row: i32,
        framing: CinemaArtworkFraming,
    ) {
        let label = gtk::Label::new(Some(&gettext("Album cover framing")));
        label.set_halign(gtk::Align::Start);
        label.set_valign(gtk::Align::Center);
        label.set_hexpand(true);
        menu_grid.attach(&label, 0, row, 1, 1);
        self.controls.cinema_artwork_framing.set_value(framing);
        label_control(&label, self.controls.cinema_artwork_framing.widget());
        menu_grid.attach(self.controls.cinema_artwork_framing.widget(), 1, row, 1, 1);
    }

    /// Adds the interactive focus preview used only by Cinema's Fill framing.
    fn add_cinema_crop_focus_menu_row(
        &self,
        menu_grid: &gtk::Grid,
        row: i32,
        focus: CinemaCropFocus,
    ) {
        let label = &self.controls.cinema_crop_focus_label;
        label.set_halign(gtk::Align::Start);
        label.set_valign(gtk::Align::Center);
        label.set_hexpand(true);
        menu_grid.attach(label, 0, row, 1, 1);
        self.controls.cinema_crop_focus.set_value(focus);
        label_control(label, self.controls.cinema_crop_focus.widget());
        menu_grid.attach(self.controls.cinema_crop_focus.widget(), 1, row, 1, 1);
    }

    /// Adds the shared Cinema/Ambient background-source segmented control.
    fn add_immersive_background_source_menu_row(
        &self,
        menu_grid: &gtk::Grid,
        row: i32,
        source: ImmersiveBackgroundSource,
    ) {
        let label = gtk::Label::new(Some(&gettext("Background")));
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
        buttons.append(&self.controls.immersive_background_source_album_cover);
        buttons.append(&self.controls.immersive_background_source_artist);
        match source {
            ImmersiveBackgroundSource::AlbumCover => self
                .controls
                .immersive_background_source_album_cover
                .set_active(true),
            ImmersiveBackgroundSource::Artist => self
                .controls
                .immersive_background_source_artist
                .set_active(true),
        }
        label_control(&label, &buttons);
        menu_grid.attach(&buttons, 1, row, 1, 1);
    }

    /// Adds the shared Cinema/Ambient background-intensity segmented control.
    fn add_backdrop_intensity_menu_row(
        &self,
        menu_grid: &gtk::Grid,
        row: i32,
        intensity: BackdropIntensity,
    ) {
        let label = gtk::Label::new(Some(&gettext("Background intensity")));
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
        buttons.append(&self.controls.backdrop_intensity_soft);
        buttons.append(&self.controls.backdrop_intensity_balanced);
        buttons.append(&self.controls.backdrop_intensity_bold);
        self.sync_backdrop_intensity_controls(intensity);
        label_control(&label, &buttons);
        menu_grid.attach(&buttons, 1, row, 1, 1);
    }

    /// Selects the active backdrop-intensity button in the context menu.
    pub(super) fn sync_backdrop_intensity_controls(&self, intensity: BackdropIntensity) {
        match intensity {
            BackdropIntensity::Soft => self.controls.backdrop_intensity_soft.set_active(true),
            BackdropIntensity::Balanced => {
                self.controls.backdrop_intensity_balanced.set_active(true)
            }
            BackdropIntensity::Bold => self.controls.backdrop_intensity_bold.set_active(true),
        }
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
        label_control(&label, dropdown);
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
        label_control(&label, scale);
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
        label_control(&label, &self.controls.album_cover_size);
        menu_grid.attach(&self.controls.album_cover_size, 1, row, 1, 1);
    }

    fn add_alignment_menu_row(
        &self,
        menu_grid: &gtk::Grid,
        row: i32,
        alignment: TrackInfoAlignment,
        sensitive: bool,
    ) {
        let label = &self.controls.track_info_alignment_label;
        label.set_halign(gtk::Align::Start);
        label.set_valign(gtk::Align::Center);
        label.set_hexpand(true);
        menu_grid.attach(label, 0, row, 1, 1);
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
        label_control(label, &buttons);
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
        label_control(&label, &buttons);
        menu_grid.attach(&buttons, 1, row, 1, 1);
    }

    /// Controls only emit model changes; renderer updates have one application path.
    pub(super) fn connect_control_handlers(&self) {
        let controller = self.controller.clone();
        self.controls
            .fullscreen_monitor
            .connect_changed(move |target| controller.update_fullscreen_monitor(target));

        let bind_switch = |switch: &gtk::Switch, change: fn(bool) -> NowPlayingPreferenceChange| {
            let applying = self.state.applying_settings.clone();
            let controller = self.controller.clone();
            switch.connect_active_notify(move |switch| {
                if !applying.get() {
                    controller.update(change(switch.is_active()));
                }
            });
        };
        bind_switch(
            &self.controls.round_corners,
            NowPlayingPreferenceChange::RoundCorners,
        );
        bind_switch(
            &self.controls.keep_screen_awake,
            NowPlayingPreferenceChange::KeepScreenAwake,
        );
        bind_switch(
            &self.controls.burn_in_protection,
            NowPlayingPreferenceChange::BurnInProtectionEnabled,
        );
        bind_switch(
            &self.controls.hide_track_info,
            NowPlayingPreferenceChange::HideTrackInfo,
        );

        let displayed_information_details = self.controls.displayed_information_details.clone();
        self.controls
            .displayed_information_button
            .connect_toggled(move |button| {
                displayed_information_details.set_reveal_child(button.is_active());
            });
        let applying = self.state.applying_settings.clone();
        let controller = self.controller.clone();
        let displayed_information_button = self.controls.displayed_information_button.clone();
        self.controls
            .displayed_information
            .connect_changed(move |value| {
                displayed_information_button.set_label(&selection_summary(value));
                if !applying.get() {
                    controller.update(NowPlayingPreferenceChange::DisplayedInformation(value));
                }
            });
        bind_switch(
            &self.controls.background_motion_enabled,
            NowPlayingPreferenceChange::BackgroundMotionEnabled,
        );
        bind_switch(
            &self.controls.always_display_last_recognized_song,
            NowPlayingPreferenceChange::AlwaysDisplayLastRecognizedSong,
        );

        let applying = self.state.applying_settings.clone();
        let controller = self.controller.clone();
        self.controls
            .display_mode
            .connect_changed(move |display_mode| {
                if !applying.get() {
                    controller.update(NowPlayingPreferenceChange::DisplayMode(display_mode));
                }
            });
        let applying = self.state.applying_settings.clone();
        let controller = self.controller.clone();
        self.controls
            .transition_menu
            .connect_selected_notify(move |dropdown| {
                if !applying.get() {
                    controller.update(NowPlayingPreferenceChange::Transition(
                        TransitionEffect::from_index(dropdown.selected()),
                    ));
                }
            });

        let applying = self.state.applying_settings.clone();
        let controller = self.controller.clone();
        self.controls
            .cinema_artwork_framing
            .connect_changed(move |framing| {
                if !applying.get() {
                    controller.update(NowPlayingPreferenceChange::CinemaArtworkFraming(framing));
                }
            });
        let applying = self.state.applying_settings.clone();
        let controller = self.controller.clone();
        self.controls
            .cinema_crop_focus
            .connect_changed(move |focus| {
                if !applying.get() {
                    controller.update_debounced(NowPlayingPreferenceChange::CinemaCropFocus(focus));
                }
            });

        for (button, change) in [
            (
                &self.controls.immersive_background_source_album_cover,
                NowPlayingPreferenceChange::ImmersiveBackgroundSource(
                    ImmersiveBackgroundSource::AlbumCover,
                ),
            ),
            (
                &self.controls.immersive_background_source_artist,
                NowPlayingPreferenceChange::ImmersiveBackgroundSource(
                    ImmersiveBackgroundSource::Artist,
                ),
            ),
            (
                &self.controls.backdrop_intensity_soft,
                NowPlayingPreferenceChange::BackdropIntensity(BackdropIntensity::Soft),
            ),
            (
                &self.controls.backdrop_intensity_balanced,
                NowPlayingPreferenceChange::BackdropIntensity(BackdropIntensity::Balanced),
            ),
            (
                &self.controls.backdrop_intensity_bold,
                NowPlayingPreferenceChange::BackdropIntensity(BackdropIntensity::Bold),
            ),
            (
                &self.controls.track_info_alignment_left,
                NowPlayingPreferenceChange::TrackInfoAlignment(TrackInfoAlignment::Left),
            ),
            (
                &self.controls.track_info_alignment_center,
                NowPlayingPreferenceChange::TrackInfoAlignment(TrackInfoAlignment::Center),
            ),
            (
                &self.controls.track_info_alignment_right,
                NowPlayingPreferenceChange::TrackInfoAlignment(TrackInfoAlignment::Right),
            ),
            (
                &self.controls.background_style_gradient,
                NowPlayingPreferenceChange::BackgroundStyle(BackgroundStyle::Gradient),
            ),
            (
                &self.controls.background_style_solid,
                NowPlayingPreferenceChange::BackgroundStyle(BackgroundStyle::Solid),
            ),
        ] {
            let applying = self.state.applying_settings.clone();
            let controller = self.controller.clone();
            button.connect_toggled(move |button| {
                if !applying.get() && button.is_active() {
                    controller.update(change);
                }
            });
        }

        let bind_scale = |scale: &gtk::Scale, change: fn(f64) -> NowPlayingPreferenceChange| {
            let applying = self.state.applying_settings.clone();
            let controller = self.controller.clone();
            scale.connect_value_changed(move |scale| {
                if !applying.get() {
                    controller.update_debounced(change(scale.value()));
                }
            });
        };
        bind_scale(&self.controls.text_size, |value| {
            NowPlayingPreferenceChange::TextSize(TextSize::from_scale_value(value))
        });
        bind_scale(&self.controls.album_cover_size, |value| {
            NowPlayingPreferenceChange::AlbumCoverSize(AlbumCoverSize::from_scale_value(value))
        });
        bind_scale(&self.controls.background_motion_zoom, |value| {
            NowPlayingPreferenceChange::BackgroundMotionZoomPercent(value.round().max(0.0) as u16)
        });
        bind_scale(
            &self.controls.background_motion_reversal_duration,
            |value| {
                NowPlayingPreferenceChange::BackgroundMotionReversalDurationSecs(
                    value.round().max(0.0) as u64,
                )
            },
        );
        bind_scale(&self.controls.transition_duration, |value| {
            NowPlayingPreferenceChange::TransitionDurationMs(transition_duration_from_scale(value))
        });

        let applying = self.state.applying_settings.clone();
        let controller = self.controller.clone();
        self.controls
            .burn_in_inactivity_minutes
            .connect_value_changed(move |spin| {
                if !applying.get() {
                    controller.update_debounced(
                        NowPlayingPreferenceChange::BurnInInactivityMinutes(
                            spin.value_as_int().max(0) as u16,
                        ),
                    );
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ContextMenuPointerAction, connect_preset_manager_handoff, context_menu_pointer_action,
    };
    use crate::gui::now_playing_window::presets::PresetManagerLauncher;
    use crate::gui::now_playing_window::{NowPlayingSettings, SettingsController};

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

    #[test]
    #[ignore = "requires a GTK display"]
    fn settings_controls_are_labelled_and_slider_gestures_do_not_retain_widgets() {
        use adw::prelude::*;

        gtk::init().unwrap();
        let controls = super::build_controls();
        let label = gtk::Label::new(Some("Text size"));
        super::label_control(&label, &controls.text_size);
        assert!(gtk::test_accessible_has_relation(
            &controls.text_size,
            gtk::AccessibleRelation::LabelledBy,
        ));

        controls.background_motion_zoom.set_value(102.0);
        assert_eq!(controls.background_motion_zoom.value(), 100.0);
        controls.background_motion_zoom.set_value(103.0);
        assert_eq!(controls.background_motion_zoom.value(), 105.0);
        controls.background_motion_reversal_duration.set_value(7.0);
        assert_eq!(controls.background_motion_reversal_duration.value(), 5.0);
        controls.background_motion_reversal_duration.set_value(8.0);
        assert_eq!(controls.background_motion_reversal_duration.value(), 10.0);
        controls.transition_duration.set_value(749.0);
        assert_eq!(controls.transition_duration.value(), 500.0);
        controls.transition_duration.set_value(750.0);
        assert_eq!(controls.transition_duration.value(), 1_000.0);

        let text_size = controls.text_size.downgrade();
        let album_cover_size = controls.album_cover_size.downgrade();
        drop(controls);
        assert!(text_size.upgrade().is_none());
        assert!(album_cover_size.upgrade().is_none());
    }

    #[test]
    #[ignore = "requires a GTK display; run with G_DEBUG=fatal-warnings"]
    fn presets_open_after_the_context_popover_has_closed() {
        use adw::prelude::*;

        let _serial = crate::MAIN_CONTEXT_TEST_LOCK.lock().unwrap();
        adw::init().unwrap();

        let parent = gtk::Window::new();
        let popover = gtk::Popover::new();
        let button = gtk::Button::with_label("Presets");
        popover.set_child(Some(&button));
        popover.set_parent(&parent);

        let manager = PresetManagerLauncher::new(SettingsController::new(
            NowPlayingSettings::default(),
            None,
        ));
        manager.bind_parent(&parent);
        connect_preset_manager_handoff(&button, &popover, parent.downgrade(), manager.clone());

        parent.present();
        popover.popup();
        while glib::MainContext::default().iteration(false) {}
        assert!(popover.is_visible());
        button.emit_clicked();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !manager.dialog_is_mapped() && std::time::Instant::now() < deadline {
            while glib::MainContext::default().iteration(false) {}
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!popover.is_visible());
        assert!(manager.is_open());
        assert!(manager.dialog_is_mapped());

        parent.set_visible(false);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while manager.dialog_is_mapped() && std::time::Instant::now() < deadline {
            while glib::MainContext::default().iteration(false) {}
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!manager.dialog_is_mapped());
        popover.unparent();
        parent.destroy();
    }
}
