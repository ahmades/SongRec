//! Window-local keyboard controls and transient action feedback.

use super::monitor_selection::toggle_fullscreen;
use super::presets::preset_at_display_index;
use super::{DisplayMode, NowPlayingWindow};
use crate::core::preferences::NowPlayingPreferenceChange;
use adw::prelude::*;
use gettextrs::gettext;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

pub(super) const SHORTCUT_FEEDBACK_CSS_CLASS: &str = "now-playing-shortcut-feedback";
const FEEDBACK_DURATION: Duration = Duration::from_secs(2);
const FEEDBACK_TRANSITION_MS: u32 = 150;
const SHORTCUTS_DIALOG_RESOURCE: &str = "/re/fossplant/songrec/shortcuts-dialog.ui";

#[derive(Clone)]
pub(super) struct ShortcutFeedback {
    revealer: gtk::Revealer,
    label: gtk::Label,
    pending_hide: Rc<RefCell<Option<glib::SourceId>>>,
}

impl ShortcutFeedback {
    pub(super) fn new(overlay: &gtk::Overlay) -> Self {
        let label = gtk::Label::builder()
            .accessible_role(gtk::AccessibleRole::Status)
            .css_classes([SHORTCUT_FEEDBACK_CSS_CLASS])
            .max_width_chars(52)
            .wrap(true)
            .xalign(0.5)
            .build();
        label.set_can_target(false);

        let revealer = gtk::Revealer::builder()
            .child(&label)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::End)
            .transition_duration(FEEDBACK_TRANSITION_MS)
            .transition_type(gtk::RevealerTransitionType::Crossfade)
            .reveal_child(false)
            .build();
        revealer.set_can_target(false);
        revealer.set_margin_bottom(28);
        revealer.set_margin_start(18);
        revealer.set_margin_end(18);
        overlay.add_overlay(&revealer);
        overlay.set_measure_overlay(&revealer, false);

        Self {
            revealer,
            label,
            pending_hide: Rc::new(RefCell::new(None)),
        }
    }

    fn show(&self, message: String) {
        self.cancel_pending_hide();
        self.label.set_label(&message);
        self.label
            .announce(&message, gtk::AccessibleAnnouncementPriority::Medium);
        self.revealer.set_reveal_child(true);

        let revealer = self.revealer.downgrade();
        let pending_hide = self.pending_hide.clone();
        let source_id = glib::timeout_add_local_once(FEEDBACK_DURATION, move || {
            pending_hide.borrow_mut().take();
            if let Some(revealer) = revealer.upgrade() {
                revealer.set_reveal_child(false);
            }
        });
        self.pending_hide.borrow_mut().replace(source_id);
    }

    pub(super) fn hide(&self) {
        self.cancel_pending_hide();
        self.revealer.set_reveal_child(false);
    }

    fn cancel_pending_hide(&self) {
        if let Some(source_id) = self.pending_hide.borrow_mut().take() {
            source_id.remove();
        }
    }
}

/// Presents the application's shared shortcuts definition within the Now
/// Playing window, where it remains visible even while that window is
/// fullscreen and independent from `GtkApplication`.
#[derive(Clone, Default)]
struct ShortcutDialogLauncher {
    dialog: Rc<RefCell<Option<adw::ShortcutsDialog>>>,
    is_open: Rc<Cell<bool>>,
}

impl ShortcutDialogLauncher {
    fn dialog(&self) -> adw::ShortcutsDialog {
        let mut dialog = self.dialog.borrow_mut();
        dialog
            .get_or_insert_with(|| {
                let builder = gtk::Builder::from_resource(SHORTCUTS_DIALOG_RESOURCE);
                let shortcuts: adw::ShortcutsDialog = builder
                    .object("shortcuts_dialog")
                    .expect("shortcuts dialog resource must define shortcuts_dialog");
                let is_open = self.is_open.clone();
                shortcuts.connect_closed(move |_| is_open.set(false));
                shortcuts
            })
            .clone()
    }

    fn present(&self, parent: &impl IsA<gtk::Widget>) {
        let dialog = self.dialog();
        if self.is_open.replace(true) {
            return;
        }
        dialog.present(Some(parent));
    }

    fn bind_parent(&self, parent: &gtk::Window) {
        let launcher = self.clone();
        parent.connect_visible_notify(move |parent| {
            if !parent.is_visible() {
                launcher.close();
            }
        });
    }

    fn close(&self) {
        let dialog = self.dialog.borrow().as_ref().cloned();
        if self.is_open.replace(false)
            && let Some(dialog) = dialog
        {
            dialog.force_close();
        }
    }

    fn is_open(&self) -> bool {
        self.is_open.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShortcutCommand {
    ToggleFullscreen,
    SelectDisplayMode(DisplayMode),
    ToggleKeepScreenAwake,
    ToggleBurnInProtection,
    OpenPresets,
    LoadPreset(usize),
    ToggleTrackInfo,
    ToggleBackgroundMotion,
    ShowShortcuts,
}

fn significant_modifiers(modifiers: gdk::ModifierType) -> gdk::ModifierType {
    modifiers
        & (gdk::ModifierType::SHIFT_MASK
            | gdk::ModifierType::CONTROL_MASK
            | gdk::ModifierType::ALT_MASK
            | gdk::ModifierType::SUPER_MASK
            | gdk::ModifierType::HYPER_MASK
            | gdk::ModifierType::META_MASK)
}

fn digit_index(key: gdk::Key) -> Option<usize> {
    match key {
        gdk::Key::_1 | gdk::Key::KP_1 => Some(0),
        gdk::Key::_2 | gdk::Key::KP_2 => Some(1),
        gdk::Key::_3 | gdk::Key::KP_3 => Some(2),
        gdk::Key::_4 | gdk::Key::KP_4 => Some(3),
        gdk::Key::_5 | gdk::Key::KP_5 => Some(4),
        gdk::Key::_6 | gdk::Key::KP_6 => Some(5),
        gdk::Key::_7 | gdk::Key::KP_7 => Some(6),
        gdk::Key::_8 | gdk::Key::KP_8 => Some(7),
        gdk::Key::_9 | gdk::Key::KP_9 => Some(8),
        gdk::Key::_0 | gdk::Key::KP_0 => Some(9),
        _ => None,
    }
}

fn command_for_key(key: gdk::Key, modifiers: gdk::ModifierType) -> Option<ShortcutCommand> {
    let modifiers = significant_modifiers(modifiers);
    let control_and_optional_shift = modifiers == gdk::ModifierType::CONTROL_MASK
        || modifiers == (gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK);

    if control_and_optional_shift && let Some(index) = digit_index(key) {
        return Some(ShortcutCommand::LoadPreset(index));
    }

    if key == gdk::Key::question && control_and_optional_shift {
        return Some(ShortcutCommand::ShowShortcuts);
    }

    if (modifiers.is_empty() || modifiers == gdk::ModifierType::SHIFT_MASK)
        && let Some(index) = digit_index(key)
    {
        return match index {
            0 => Some(ShortcutCommand::SelectDisplayMode(DisplayMode::Classic)),
            1 => Some(ShortcutCommand::SelectDisplayMode(DisplayMode::Cinema)),
            2 => Some(ShortcutCommand::SelectDisplayMode(DisplayMode::Ambient)),
            3 => Some(ShortcutCommand::SelectDisplayMode(DisplayMode::LightsOff)),
            _ => None,
        };
    }

    if !modifiers.is_empty() {
        return None;
    }

    match key.to_lower() {
        gdk::Key::f => Some(ShortcutCommand::ToggleFullscreen),
        gdk::Key::a => Some(ShortcutCommand::ToggleKeepScreenAwake),
        gdk::Key::b => Some(ShortcutCommand::ToggleBurnInProtection),
        gdk::Key::p => Some(ShortcutCommand::OpenPresets),
        gdk::Key::i => Some(ShortcutCommand::ToggleTrackInfo),
        gdk::Key::m => Some(ShortcutCommand::ToggleBackgroundMotion),
        _ => None,
    }
}

fn event_targets_window(controller: &gtk::EventControllerKey, window: &gtk::Window) -> bool {
    let Some(event) = controller.current_event() else {
        return true;
    };
    let Some(native) = window.native() else {
        return false;
    };
    event.surface() == native.surface()
}

impl NowPlayingWindow {
    pub(super) fn setup_keyboard_shortcuts(&self) {
        let window = self.ui.window.downgrade();
        let popover = self.controls.context_menu.downgrade();
        let controller = self.controller.clone();
        let feedback = self.shortcut_feedback.clone();
        let preset_manager = self.preset_manager.clone();
        let shortcuts_dialog = ShortcutDialogLauncher::default();
        shortcuts_dialog.bind_parent(&self.ui.window);
        let feedback_for_hide = feedback.clone();
        self.ui.window.connect_visible_notify(move |window| {
            if !window.is_visible() {
                feedback_for_hide.hide();
            }
        });
        let pressed_keys = Rc::new(RefCell::new(HashSet::<u32>::new()));
        let pressed_keys_for_press = pressed_keys.clone();
        let pressed_keys_for_active = pressed_keys.clone();

        let keyboard = gtk::EventControllerKey::new();
        keyboard.connect_key_pressed(move |event_controller, key, keycode, modifiers| {
            let Some(command) = command_for_key(key, modifiers) else {
                return glib::Propagation::Proceed;
            };
            let Some(window) = window.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if !window.is_active()
                || !event_targets_window(event_controller, &window)
                || popover
                    .upgrade()
                    .is_some_and(|popover| popover.is_visible())
                || preset_manager.is_open()
                || shortcuts_dialog.is_open()
            {
                return glib::Propagation::Proceed;
            }
            if !pressed_keys_for_press.borrow_mut().insert(keycode) {
                return glib::Propagation::Stop;
            }

            match command {
                ShortcutCommand::ToggleFullscreen => {
                    let target = controller.fullscreen_monitor();
                    toggle_fullscreen(&window, target.as_ref());
                }
                ShortcutCommand::SelectDisplayMode(mode) => {
                    if controller.settings().display_mode != mode {
                        controller.update(NowPlayingPreferenceChange::DisplayMode(mode));
                    }
                    feedback.show(
                        gettext("Display mode: {mode}").replace("{mode}", &mode.translated_label()),
                    );
                }
                ShortcutCommand::ToggleKeepScreenAwake => {
                    let enabled = !controller.settings().shared.keep_screen_awake;
                    controller.update(NowPlayingPreferenceChange::KeepScreenAwake(enabled));
                    feedback.show(if enabled {
                        gettext("Keep screen awake: On")
                    } else {
                        gettext("Keep screen awake: Off")
                    });
                }
                ShortcutCommand::ToggleBurnInProtection => {
                    let enabled = !controller.settings().shared.burn_in_protection_enabled;
                    controller.update(NowPlayingPreferenceChange::BurnInProtectionEnabled(enabled));
                    feedback.show(if enabled {
                        gettext("Burn-in protection: On")
                    } else {
                        gettext("Burn-in protection: Off")
                    });
                }
                ShortcutCommand::OpenPresets => preset_manager.present(&window),
                ShortcutCommand::LoadPreset(index) => {
                    if let Some((id, name)) = preset_at_display_index(&controller, index) {
                        match controller.load_preset(id) {
                            Ok(()) => feedback
                                .show(gettext("Preset loaded: {name}").replace("{name}", &name)),
                            Err(error) => {
                                log::error!("Failed to load a Now Playing preset: {error:?}");
                                feedback.show(gettext("Could not load preset"));
                            }
                        }
                    } else {
                        let number = if index == 9 { 0 } else { index + 1 };
                        feedback.show(
                            gettext("No preset saved for Ctrl+{number}")
                                .replace("{number}", &number.to_string()),
                        );
                    }
                }
                ShortcutCommand::ToggleTrackInfo => {
                    let settings = controller.settings();
                    if !settings.display_mode.supports_hiding_track_info() {
                        feedback.show(gettext("Track information is always shown in Lights Off"));
                    } else {
                        let hidden = !settings.shared.hide_track_info;
                        controller.update(NowPlayingPreferenceChange::HideTrackInfo(hidden));
                        feedback.show(if hidden {
                            gettext("Track information: Hidden")
                        } else {
                            gettext("Track information: Shown")
                        });
                    }
                }
                ShortcutCommand::ToggleBackgroundMotion => {
                    let settings = controller.settings();
                    if !settings.display_mode.supports_background_motion() {
                        feedback.show(gettext(
                            "Background motion is available in Cinema and Ambient",
                        ));
                    } else {
                        let enabled = !settings.shared.background_motion_enabled;
                        controller
                            .update(NowPlayingPreferenceChange::BackgroundMotionEnabled(enabled));
                        feedback.show(if enabled {
                            gettext("Background motion: On")
                        } else {
                            gettext("Background motion: Off")
                        });
                    }
                }
                ShortcutCommand::ShowShortcuts => shortcuts_dialog.present(&window),
            }

            glib::Propagation::Stop
        });

        keyboard.connect_key_released(move |_, _, keycode, _| {
            pressed_keys.borrow_mut().remove(&keycode);
        });
        self.ui.window.add_controller(keyboard);

        self.ui.window.connect_is_active_notify(move |window| {
            if !window.is_active() {
                pressed_keys_for_active.borrow_mut().clear();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SHORTCUTS_DIALOG_RESOURCE, ShortcutCommand, ShortcutDialogLauncher, ShortcutFeedback,
        command_for_key,
    };
    use crate::core::preferences::DisplayMode;
    use adw::prelude::*;

    #[test]
    fn unmodified_keys_map_to_window_local_commands() {
        let none = gdk::ModifierType::empty();
        assert_eq!(
            command_for_key(gdk::Key::f, none),
            Some(ShortcutCommand::ToggleFullscreen)
        );
        assert_eq!(
            command_for_key(gdk::Key::_1, none),
            Some(ShortcutCommand::SelectDisplayMode(DisplayMode::Classic))
        );
        assert_eq!(
            command_for_key(gdk::Key::_4, none),
            Some(ShortcutCommand::SelectDisplayMode(DisplayMode::LightsOff))
        );
        assert_eq!(
            command_for_key(gdk::Key::M, none),
            Some(ShortcutCommand::ToggleBackgroundMotion)
        );
        assert_eq!(command_for_key(gdk::Key::F11, none), None);
    }

    #[test]
    fn control_digits_load_presets_and_zero_is_the_tenth() {
        let control = gdk::ModifierType::CONTROL_MASK;
        assert_eq!(
            command_for_key(gdk::Key::_1, control),
            Some(ShortcutCommand::LoadPreset(0))
        );
        assert_eq!(
            command_for_key(gdk::Key::KP_0, control),
            Some(ShortcutCommand::LoadPreset(9))
        );
        assert_eq!(
            command_for_key(gdk::Key::_1, control | gdk::ModifierType::SHIFT_MASK),
            Some(ShortcutCommand::LoadPreset(0))
        );
        assert_eq!(
            command_for_key(gdk::Key::_1, control | gdk::ModifierType::ALT_MASK),
            None
        );
    }

    #[test]
    fn shifted_digit_keyvals_support_layouts_that_require_shift_for_numbers() {
        assert_eq!(
            command_for_key(gdk::Key::_1, gdk::ModifierType::SHIFT_MASK),
            Some(ShortcutCommand::SelectDisplayMode(DisplayMode::Classic))
        );
        assert_eq!(
            command_for_key(gdk::Key::_4, gdk::ModifierType::SHIFT_MASK),
            Some(ShortcutCommand::SelectDisplayMode(DisplayMode::LightsOff))
        );
        assert_eq!(
            command_for_key(gdk::Key::_5, gdk::ModifierType::SHIFT_MASK),
            None
        );
    }

    #[test]
    fn control_question_uses_the_shared_shortcuts_dialog() {
        assert_eq!(
            command_for_key(gdk::Key::question, gdk::ModifierType::CONTROL_MASK),
            Some(ShortcutCommand::ShowShortcuts)
        );
        assert_eq!(
            command_for_key(
                gdk::Key::question,
                gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK,
            ),
            Some(ShortcutCommand::ShowShortcuts)
        );
    }

    #[test]
    #[ignore = "requires a GTK display"]
    fn feedback_replaces_messages_and_shared_shortcuts_dialog_loads() {
        let _serial = crate::MAIN_CONTEXT_TEST_LOCK.lock().unwrap();
        adw::init().unwrap();
        gio::resources_register_include!("compiled.gresource").unwrap();

        let overlay = gtk::Overlay::new();
        let window = gtk::Window::builder().child(&overlay).build();
        let feedback = ShortcutFeedback::new(&overlay);
        window.present();
        while glib::MainContext::default().iteration(false) {}

        feedback.show("First".to_string());
        feedback.show("Second".to_string());
        assert_eq!(feedback.label.label(), "Second");
        assert!(feedback.revealer.reveals_child());

        feedback.hide();
        assert!(!feedback.revealer.reveals_child());

        let builder = gtk::Builder::from_resource(SHORTCUTS_DIALOG_RESOURCE);
        let close_shortcut: adw::ShortcutsItem = builder.object("close_shortcut").unwrap();
        assert_eq!(close_shortcut.accelerator(), "<Primary>Q <Primary>W");

        let shortcuts = ShortcutDialogLauncher::default();
        shortcuts.bind_parent(&window);
        shortcuts.present(&window);
        while glib::MainContext::default().iteration(false) {}
        let dialog = shortcuts.dialog();
        assert_eq!(dialog.accessible_role(), gtk::AccessibleRole::Dialog);
        assert!(dialog.is_mapped());
        let dialog_window = dialog.root().and_downcast::<gtk::Window>().unwrap();
        assert_eq!(dialog_window.transient_for(), Some(window.clone()));
        assert!(shortcuts.is_open());
        window.set_visible(false);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while shortcuts.is_open() && std::time::Instant::now() < deadline {
            while glib::MainContext::default().iteration(false) {}
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            !shortcuts.is_open(),
            "parent visible={}, dialog visible={}, dialog mapped={}",
            window.is_visible(),
            dialog_window.is_visible(),
            dialog.is_mapped()
        );
        assert!(!dialog.is_mapped());

        window.destroy();
    }
}
