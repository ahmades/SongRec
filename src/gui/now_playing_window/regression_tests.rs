//! Display-backed checks. Each ignored test runs in a separate process.

use super::{NowPlayingSettings, NowPlayingWindow, SettingsController};
use adw::prelude::*;
use std::process::Command;
use std::time::{Duration, Instant};

async fn settle() {
    glib::timeout_future(Duration::from_millis(100)).await;
}

async fn wait_until(description: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(4);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out: {description}");
        glib::timeout_future(Duration::from_millis(10)).await;
    }
}

async fn xdotool(args: &[&str]) -> String {
    let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    let output = gio::spawn_blocking(move || Command::new("xdotool").args(args).output())
        .await
        .unwrap()
        .expect("this test requires xdotool");
    assert!(
        output.status.success(),
        "xdotool: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

// Send input through our private compositor, not Gtk signals or the user's
// desktop's XTest/portal permissions. xdotool only queries X11 geometry.
struct TestInput {
    connection: gio::DBusConnection,
    session: String,
}

impl TestInput {
    async fn new() -> Self {
        assert_eq!(
            std::env::var("SONGREC_TEST_HEADLESS").as_deref(),
            Ok("1"),
            "run inside the documented private headless Mutter session"
        );
        let connection = gio::bus_get_future(gio::BusType::Session).await.unwrap();
        let response = connection
            .call_future(
                Some("org.gnome.Mutter.RemoteDesktop"),
                "/org/gnome/Mutter/RemoteDesktop",
                "org.gnome.Mutter.RemoteDesktop",
                "CreateSession",
                None,
                None,
                gio::DBusCallFlags::NONE,
                3000,
            )
            .await
            .unwrap();
        let input = Self {
            connection,
            session: response.child_value(0).str().unwrap().into(),
        };
        input.send("Start", None).await;
        // Materialize the virtual keyboard before testing its first key press;
        // Xwayland needs a frame to install the new device's keymap.
        input.key(42).await;
        input
    }

    async fn send(&self, method: &str, parameters: Option<&glib::Variant>) {
        self.connection
            .call_future(
                Some("org.gnome.Mutter.RemoteDesktop"),
                &self.session,
                "org.gnome.Mutter.RemoteDesktop.Session",
                method,
                parameters,
                None,
                gio::DBusCallFlags::NONE,
                3000,
            )
            .await
            .unwrap();
    }

    async fn key(&self, keycode: u32) {
        self.send("NotifyKeyboardKeycode", Some(&(keycode, true).to_variant()))
            .await;
        settle().await;
        self.send(
            "NotifyKeyboardKeycode",
            Some(&(keycode, false).to_variant()),
        )
        .await;
        settle().await;
    }

    async fn click(
        &self,
        window_id: Option<&str>,
        window: &gtk::Window,
        x: i32,
        y: i32,
        button: u32,
    ) {
        let (origin_x, origin_y) = if let Some(window_id) = window_id {
            let geometry = xdotool(&["getwindowgeometry", "--shell", window_id]).await;
            let field = |name: &str| -> f64 {
                geometry
                    .lines()
                    .find_map(|line| line.strip_prefix(name))
                    .unwrap()
                    .parse()
                    .unwrap()
            };
            (field("X="), field("Y="))
        } else {
            // Wayland deliberately hides global window coordinates. A maximized,
            // undecorated window on our single, panel-free monitor starts at (0,0).
            assert!(window.is_maximized() || window.is_fullscreen());
            (0.0, 0.0)
        };
        let (offset_x, offset_y) = window.surface_transform();
        let x = origin_x + f64::from(x) - offset_x;
        let y = origin_y + f64::from(y) - offset_y;
        // One virtual monitor makes a clamped move to its origin deterministic.
        self.send(
            "NotifyPointerMotionRelative",
            Some(&(-10000.0f64, -10000.0f64).to_variant()),
        )
        .await;
        settle().await;
        self.send("NotifyPointerMotionRelative", Some(&(x, y).to_variant()))
            .await;
        settle().await;
        self.click_at_pointer(button).await;
        settle().await;
    }

    async fn click_at_pointer(&self, button: u32) {
        let evdev_button: i32 = if button == 1 { 272 } else { 273 };
        self.send(
            "NotifyPointerButton",
            Some(&(evdev_button, true).to_variant()),
        )
        .await;
        self.send(
            "NotifyPointerButton",
            Some(&(evdev_button, false).to_variant()),
        )
        .await;
    }

    async fn double_click(
        &self,
        window_id: Option<&str>,
        window: &gtk::Window,
        x: i32,
        y: i32,
        button: u32,
    ) {
        self.click(window_id, window, x, y, button).await;
        // Do not move the pointer between presses: doing so resets GTK's
        // multi-click sequence, even when both clicks end at the same point.
        self.click_at_pointer(button).await;
        settle().await;
    }
}

fn widget_center(widget: &impl IsA<gtk::Widget>, window: &gtk::Window) -> (i32, i32) {
    let widget = widget.as_ref();
    let native = widget.native().unwrap();
    let native_widget = native.clone().dynamic_cast::<gtk::Widget>().unwrap();
    let center = widget
        .compute_point(
            &native_widget,
            &gtk::graphene::Point::new(widget.width() as f32 / 2.0, widget.height() as f32 / 2.0),
        )
        .expect("widget belongs to the test window");
    let (offset_x, offset_y) = native.surface_transform();
    let (mut x, mut y) = (
        f64::from(center.x()) - offset_x,
        f64::from(center.y()) - offset_y,
    );
    let mut surface = native.surface().unwrap();
    while let Some(popup) = surface.downcast_ref::<gdk::Popup>() {
        x += f64::from(popup.position_x());
        y += f64::from(popup.position_y());
        surface = popup.parent().unwrap();
    }
    assert_eq!(Some(surface), window.surface());
    let (offset_x, offset_y) = window.surface_transform();
    ((x + offset_x).round() as i32, (y + offset_y).round() as i32)
}

fn nested_popover(widget: &impl IsA<gtk::Widget>) -> Option<gtk::Popover> {
    let mut child = widget.as_ref().first_child();
    while let Some(current) = child {
        if let Ok(popover) = current.clone().downcast::<gtk::Popover>() {
            return Some(popover);
        }
        if let Some(popover) = nested_popover(&current) {
            return Some(popover);
        }
        child = current.next_sibling();
    }
    None
}

fn find_label(widget: &impl IsA<gtk::Widget>, text: &str) -> Option<gtk::Label> {
    if let Some(label) = widget.as_ref().downcast_ref::<gtk::Label>()
        && label.label() == text
        && label.is_mapped()
    {
        return Some(label.clone());
    }
    let mut child = widget.as_ref().first_child();
    while let Some(current) = child {
        if let Some(label) = find_label(&current, text) {
            return Some(label);
        }
        child = current.next_sibling();
    }
    None
}

pub(super) struct TestWindow(pub(super) NowPlayingWindow);

impl Drop for TestWindow {
    fn drop(&mut self) {
        if let Some(popover) = self
            .0
            .controls
            .display_mode_menu
            .ancestor(gtk::Popover::static_type())
        {
            popover.unparent();
        }
        self.0.ui.window.destroy();
    }
}

#[test]
#[ignore = "requires a GTK display and private D-Bus session"]
fn settings_views_stay_in_sync_and_immediate_quit_preserves_sliders() {
    use crate::core::preferences::{
        BackdropIntensity, CinemaArtworkFraming, CinemaCropFocus, Preferences, PreferencesInterface,
    };
    use crate::core::thread_messages::GUIMessage;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};
    adw::init().unwrap();
    gio::resources_register_include!("compiled.gresource").unwrap();
    crate::gui::history_entry::HistoryEntry::static_type();
    crate::gui::listed_device::ListedDevice::static_type();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("preferences.toml");
    let preferences = Arc::new(Mutex::new(PreferencesInterface {
        preferences_file_path: Some(path.clone()),
        preferences: Preferences::default(),
    }));
    let (tx, rx) = async_channel::unbounded();
    let controller = SettingsController::new(NowPlayingSettings::default(), Some(tx));
    let application = gio::Application::new(
        Some("re.fossplant.songrec.SettingsTest"),
        gio::ApplicationFlags::NON_UNIQUE,
    );
    controller.connect_shutdown(&application, preferences.clone());
    let window = Rc::new(TestWindow(NowPlayingWindow::new_with_controller(
        controller.clone(),
    )));
    let builder = gtk::Builder::new();
    let scope = gtk::BuilderRustScope::new();
    // Load the real preferences widgets without starting unrelated audio,
    // history, or search services. NowPlayingPreferencesView wires its own
    // production callbacks below.
    for name in [
        "microphone_option_switched",
        "loopback_options_switched",
        "input_device_switched",
        "history_cell_setup_cb",
        "history_cell_bind_cb",
        "interval_changed",
        "search_engine_action_changed",
        "search_engine_url_changed",
        "about_dialog_closed",
    ] {
        scope.add_callback(name, |_| None);
    }
    builder.set_scope(Some(&scope));
    builder
        .add_from_resource("/re/fossplant/songrec/interface-autogenerated.ui")
        .unwrap();
    let view = super::NowPlayingPreferencesView::new(&builder, controller.clone());
    let text: gtk::Scale = builder.object("text_size_setting_scale").unwrap();
    let album: gtk::Scale = builder.object("album_cover_size_setting_scale").unwrap();
    let hide: adw::SwitchRow = builder.object("hide_track_info_setting").unwrap();
    let keep_screen_awake: adw::SwitchRow = builder.object("keep_screen_awake_setting").unwrap();
    let mode: adw::ComboRow = builder.object("display_mode_setting").unwrap();
    let artist_background: gtk::ToggleButton = builder
        .object("immersive_background_source_artist")
        .unwrap();
    let album_background: gtk::ToggleButton = builder
        .object("immersive_background_source_album_cover")
        .unwrap();
    let backdrop_soft: gtk::ToggleButton = builder.object("backdrop_intensity_soft").unwrap();
    let backdrop_bold: gtk::ToggleButton = builder.object("backdrop_intensity_bold").unwrap();
    let text_row: adw::ActionRow = builder.object("text_size_setting").unwrap();
    let cinema_group: adw::PreferencesGroup =
        builder.object("cinema_now_playing_preferences").unwrap();
    let cinema_crop_focus_row: adw::ActionRow =
        builder.object("cinema_crop_focus_setting").unwrap();
    let expected = Rc::new(Cell::new(controller.settings()));
    let saved_expected = expected.clone();
    let preferences_for_activate = preferences.clone();
    application.connect_activate(move |application| {
        let dispatch = || {
            while let Ok(message) = rx.try_recv() {
                let GUIMessage::NowPlayingPreferenceChanged { settings, persist } = message else {
                    panic!("unexpected settings message");
                };
                preferences_for_activate
                    .lock()
                    .unwrap()
                    .set_now_playing(settings, persist);
                view.apply(controller.settings());
                window.0.refresh_from_controller();
            }
        };
        text.set_value(107.0);
        dispatch();
        assert_eq!(window.0.controls.text_size.value(), 107.0);
        window.0.controls.album_cover_size.set_value(83.0);
        dispatch();
        assert_eq!(album.value(), 83.0);
        window.0.controls.hide_track_info.set_active(true);
        dispatch();
        assert!(hide.is_active());
        assert!(!text_row.get_visible());
        window.0.controls.keep_screen_awake.set_active(true);
        dispatch();
        assert!(keep_screen_awake.is_active());
        keep_screen_awake.set_active(false);
        dispatch();
        assert!(!window.0.controls.keep_screen_awake.is_active());
        keep_screen_awake.set_active(true);
        dispatch();
        mode.set_selected(super::DisplayMode::LightsOff.index());
        dispatch();
        assert!(!window.0.controls.hide_track_info.get_visible());
        mode.set_selected(super::DisplayMode::Classic.index());
        dispatch();
        hide.set_active(false);
        dispatch();
        mode.set_selected(super::DisplayMode::Cinema.index());
        dispatch();
        assert!(cinema_group.get_visible());
        assert!(!cinema_crop_focus_row.get_visible());
        window
            .0
            .controls
            .cinema_artwork_framing
            .set_value(CinemaArtworkFraming::Fill);
        dispatch();
        assert!(cinema_crop_focus_row.get_visible());
        window
            .0
            .controls
            .cinema_crop_focus
            .set_value(CinemaCropFocus::BottomRight);
        dispatch();
        assert_eq!(
            controller.settings().cinema.crop_focus,
            CinemaCropFocus::BottomRight
        );
        mode.set_selected(super::DisplayMode::Ambient.index());
        dispatch();
        assert!(!cinema_group.get_visible());
        assert_eq!(
            controller.settings().cinema.artwork_framing,
            CinemaArtworkFraming::Fill
        );
        mode.set_selected(super::DisplayMode::Cinema.index());
        dispatch();
        assert!(cinema_crop_focus_row.get_visible());
        window
            .0
            .controls
            .immersive_background_source_artist
            .set_active(true);
        dispatch();
        assert!(artist_background.is_active());
        album_background.set_active(true);
        dispatch();
        assert!(
            window
                .0
                .controls
                .immersive_background_source_album_cover
                .is_active()
        );
        window.0.controls.backdrop_intensity_bold.set_active(true);
        dispatch();
        assert!(backdrop_bold.is_active());
        assert_eq!(
            controller.settings().shared.backdrop_intensity,
            BackdropIntensity::Bold
        );
        backdrop_soft.set_active(true);
        dispatch();
        assert!(window.0.controls.backdrop_intensity_soft.is_active());
        assert_eq!(
            controller.settings().shared.backdrop_intensity,
            BackdropIntensity::Soft
        );
        mode.set_selected(super::DisplayMode::Classic.index());
        dispatch();
        // Test the controls' own visibility, not that of the closed outer menu
        // or the unpresented preferences page.
        assert!(window.0.controls.text_size.get_visible());
        assert!(text_row.get_visible());

        // No main-loop iteration between the final UI signals and quitting:
        // neither their GUI messages nor the debounce timer can save them.
        window.0.controls.album_cover_size.set_value(122.0);
        text.set_value(117.0);
        assert!(!rx.is_empty());
        expected.set(controller.settings());
        application.quit();
    });
    assert_eq!(
        application.run_with_args(&["songrec-settings-test"]),
        glib::ExitCode::SUCCESS
    );
    let saved: Preferences = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved.now_playing, saved_expected.get());
    let reopened = TestWindow(NowPlayingWindow::new_with_controller(
        SettingsController::new(saved.now_playing, None),
    ));
    assert_eq!(reopened.0.controls.text_size.value(), 117.0);
    assert_eq!(reopened.0.controls.album_cover_size.value(), 122.0);
    assert!(reopened.0.controls.keep_screen_awake.is_active());
}

#[test]
#[ignore = "requires private headless Mutter and xdotool; see docs/now-playing-testing.md"]
fn real_pointer_dismisses_menu_after_using_nested_dropdowns() {
    let _ = fern::Dispatch::new()
        .level(log::LevelFilter::Debug)
        .chain(std::io::stderr())
        .apply();
    adw::init().unwrap();
    gtk::Settings::default()
        .unwrap()
        .set_gtk_double_click_time(400);
    let backend = gdk::Display::default().unwrap().type_().name();
    assert!(matches!(backend, "GdkX11Display" | "GdkWaylandDisplay"));
    let x11 = backend == "GdkX11Display";
    let window = TestWindow(NowPlayingWindow::new_with_controller(
        SettingsController::new(NowPlayingSettings::default(), None),
    ));
    let title = format!("SongRec pointer regression {}", std::process::id());
    window.0.ui.window.set_title(Some(&title));
    // Keep input coordinates independent of compositor-owned titlebars.
    window.0.ui.window.set_decorated(false);
    if !x11 {
        window.0.ui.window.maximize();
    }
    window.0.present();
    let menu = window
        .0
        .controls
        .display_mode_menu
        .ancestor(gtk::Popover::static_type())
        .unwrap()
        .downcast::<gtk::Popover>()
        .unwrap();

    glib::MainContext::default().block_on(async {
        let input = TestInput::new().await;
        wait_until("window mapped", || window.0.ui.window.is_mapped()).await;
        let id = if x11 {
            let id = xdotool(&["search", "--onlyvisible", "--pid", &std::process::id().to_string(), "--name", &title]).await;
            let id = id.lines().next().unwrap().to_string();
            xdotool(&["windowactivate", &id]).await;
            Some(id)
        } else {
            wait_until("window maximized", || window.0.ui.window.is_maximized()).await;
            None
        };
        let id = id.as_deref();
        settle().await;
        for fullscreen in [false, true] {
            if fullscreen {
                window.0.ui.window.fullscreen();
                wait_until("fullscreen", || window.0.ui.window.is_fullscreen()).await;
                settle().await;
            }
            for (name, dropdown) in [
                ("display mode", &window.0.controls.display_mode_menu),
                ("transition", &window.0.controls.transition_menu),
            ] {
                for select in [false, true] {
                    for outside_button in [1, 3] {
                        eprintln!("pointer case: fullscreen={fullscreen}, dropdown={name}, select={select}, button={outside_button}");
                        input.click(id, &window.0.ui.window, window.0.ui.window.width() / 2, window.0.ui.window.height() / 2, 3).await;
                        wait_until("outer menu opened", || menu.is_visible()).await;
                        settle().await;
                        let (x, y) = widget_center(dropdown, &window.0.ui.window);
                        input.click(id, &window.0.ui.window, x, y, 1).await;
                        let nested = nested_popover(dropdown).expect("dropdown popup");
                        wait_until("nested dropdown opened", || nested.is_visible()).await;
                        // Wait for Wayland's popup configure/reposition and GTK's
                        // opening animation before measuring an option's center.
                        settle().await;
                        if select {
                            let selected = dropdown.selected();
                            let index = if selected == 0 { 1 } else { 0 };
                            if x11 {
                                let label = if name == "display mode" {
                                    super::DisplayMode::from_index(index).translated_label()
                                } else {
                                    super::TransitionEffect::from_index(index).translated_label()
                                };
                                let item = find_label(&nested, &label).expect("visible dropdown option");
                                let (item_x, item_y) = widget_center(&item, &window.0.ui.window);
                                input.click(id, &window.0.ui.window, item_x, item_y, 1).await;
                            } else {
                                // Mutter/GTK may report stale parent-popup positions
                                // after opening a nested Wayland popup. Select with
                                // real keys; opening, toggle-closing, and all outside
                                // dismissal checks still use compositor pointer input.
                                input.key(if index > selected { 108 } else { 103 }).await;
                                input.key(28).await;
                            }
                            assert_eq!(dropdown.selected(), index);
                        } else {
                            input.click(id, &window.0.ui.window, x, y, 1).await;
                        }
                        wait_until("nested dropdown closed", || !nested.is_visible()).await;
                        assert!(menu.is_visible(), "closing the nested popup must leave the settings menu open");
                        // This corner is outside the centered menu in both window sizes.
                        input.click(id, &window.0.ui.window, 20, window.0.ui.window.height() - 20, outside_button).await;
                        wait_until("outer menu dismissed", || !menu.is_visible()).await;
                        settle().await;
                        assert!(!menu.is_visible(), "the dismissal click must not reopen the menu");
                        window.0.refresh_from_controller();
                    }
                }
            }
        }
        for shift in [false, true] {
            eprintln!("keyboard case: Shift+F10={shift}");
            if shift {
                input.send("NotifyKeyboardKeycode", Some(&(42u32, true).to_variant())).await;
                input.key(68).await;
                input.send("NotifyKeyboardKeycode", Some(&(42u32, false).to_variant())).await;
            } else {
                input.key(127).await;
            }
            wait_until("keyboard menu opened", || menu.is_visible()).await;
            let text_size_widget: &gtk::Widget = window.0.controls.text_size.upcast_ref();
            for _ in 0..32 {
                if gtk::prelude::GtkWindowExt::focus(&window.0.ui.window).as_ref() == Some(text_size_widget) {
                    break;
                }
                input.key(15).await; // Tab
            }
            assert_eq!(gtk::prelude::GtkWindowExt::focus(&window.0.ui.window).as_ref(), Some(text_size_widget), "Tab must reach the text-size slider");
            let before = window.0.controls.text_size.value();
            input.key(106).await; // Right arrow
            assert!(window.0.controls.text_size.value() > before);
            input.key(1).await;
            wait_until("Escape dismissed menu", || !menu.is_visible()).await;
        }
        window.0.ui.window.unfullscreen();
        wait_until("windowed for double-click checks", || !window.0.ui.window.is_fullscreen()).await;
        for mode in [super::DisplayMode::Classic, super::DisplayMode::Cinema, super::DisplayMode::Ambient, super::DisplayMode::LightsOff] {
            window.0.controller.update(crate::core::preferences::NowPlayingPreferenceChange::DisplayMode(mode));
            window.0.refresh_from_controller();
            for fullscreen in [true, false] {
                glib::timeout_future(Duration::from_millis(500)).await;
                eprintln!("canvas double-click: mode={mode:?}, target fullscreen={fullscreen}");
                input.double_click(id, &window.0.ui.window, 30, window.0.ui.window.height() - 30, 1).await;
                wait_until("double-click toggled fullscreen", || window.0.ui.window.is_fullscreen() == fullscreen).await;
                assert_eq!(window.0.controls.fullscreen_button_content.label(), gettextrs::gettext(if fullscreen { "Exit full screen" } else { "Enter full screen" }));
            }
        }
        glib::timeout_future(Duration::from_millis(500)).await;
        input.click(id, &window.0.ui.window, 30, window.0.ui.window.height() - 30, 1).await;
        assert!(!window.0.ui.window.is_fullscreen(), "single click must not toggle fullscreen");
        // A secondary double-click opens/dismisses the menu, never fullscreen.
        input.double_click(id, &window.0.ui.window, 30, window.0.ui.window.height() - 30, 3).await;
        assert!(!window.0.ui.window.is_fullscreen());
        if menu.is_visible() {
            input.key(1).await;
        }
        // Menu controls live outside the canvas gesture's propagation path.
        input.click(id, &window.0.ui.window, window.0.ui.window.width() / 2, window.0.ui.window.height() / 2, 3).await;
        wait_until("menu reopened for double-click check", || menu.is_visible()).await;
        let (x, y) = widget_center(&window.0.controls.text_size_label, &window.0.ui.window);
        input.double_click(id, &window.0.ui.window, x, y, 1).await;
        assert!(menu.is_visible());
        assert!(!window.0.ui.window.is_fullscreen(), "menu double-click must not toggle fullscreen");
        // Explicit teardown avoids testing backend-dependent popup keyboard
        // focus a second time; Escape was exercised above. F11 belongs to the
        // toplevel window and should be checked with no native popup active.
        menu.popdown();
        wait_until("menu dismissed before F11", || !menu.is_visible()).await;
        if let Some(id) = id {
            xdotool(&["windowactivate", id]).await;
        }
        settle().await;
        // F11 still uses the same action, including after canvas gestures.
        input.key(87).await;
        wait_until("F11 entered fullscreen", || window.0.ui.window.is_fullscreen()).await;
        input.key(87).await;
        wait_until("F11 exited fullscreen", || !window.0.ui.window.is_fullscreen()).await;
        input.send("Stop", None).await;
    });
}
