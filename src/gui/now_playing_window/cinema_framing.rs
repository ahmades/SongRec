//! Reusable controls for Cinema artwork fitting and crop focus.

use super::{CinemaArtworkFraming, CinemaCropFocus};
use adw::prelude::*;
use gettextrs::gettext;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const REGULAR_PREVIEW_SIZE: i32 = 220;
const COMPACT_PREVIEW_SIZE: i32 = 156;
const KEYBOARD_STEP_BASIS_POINTS: u16 = 100;
const KEYBOARD_LARGE_STEP_BASIS_POINTS: u16 = 1_000;

type CropFocusCallback = Rc<dyn Fn(CinemaCropFocus)>;

struct PointerFocusContext<'a> {
    preview: &'a gtk::Overlay,
    marker: &'a gtk::DrawingArea,
    focus: &'a Cell<CinemaCropFocus>,
    source_dimensions: &'a Cell<(i32, i32)>,
    callbacks: &'a RefCell<Vec<CropFocusCallback>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct PreviewRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl PreviewRect {
    fn for_contained_image(
        viewport_width: i32,
        viewport_height: i32,
        source_dimensions: (i32, i32),
    ) -> Option<Self> {
        let (source_width, source_height) = source_dimensions;
        if viewport_width <= 0 || viewport_height <= 0 || source_width <= 0 || source_height <= 0 {
            return None;
        }

        let scale = (f64::from(viewport_width) / f64::from(source_width))
            .min(f64::from(viewport_height) / f64::from(source_height));
        let width = f64::from(source_width) * scale;
        let height = f64::from(source_height) * scale;
        Some(Self {
            x: (f64::from(viewport_width) - width) / 2.0,
            y: (f64::from(viewport_height) - height) / 2.0,
            width,
            height,
        })
    }

    fn focus_at(self, x: f64, y: f64) -> CinemaCropFocus {
        CinemaCropFocus::from_fractions(
            ((x - self.x) / self.width).clamp(0.0, 1.0),
            ((y - self.y) / self.height).clamp(0.0, 1.0),
        )
    }

    fn point_for_focus(self, focus: CinemaCropFocus) -> (f64, f64) {
        let (focus_x, focus_y) = focus.normalized_coordinates();
        (
            self.x + self.width * focus_x,
            self.y + self.height * focus_y,
        )
    }
}

/// Linked buttons for the user-facing Automatic/Fit/Fill choice.
#[derive(Clone)]
pub(super) struct CinemaArtworkFramingControls {
    widget: gtk::Box,
    buttons: [(CinemaArtworkFraming, gtk::ToggleButton); 3],
}

impl CinemaArtworkFramingControls {
    pub(super) fn new() -> Self {
        let automatic = gtk::ToggleButton::with_label(&gettext("Automatic"));
        let fit = gtk::ToggleButton::with_label(&gettext("Fit"));
        let fill = gtk::ToggleButton::with_label(&gettext("Fill"));
        fit.set_group(Some(&automatic));
        fill.set_group(Some(&automatic));

        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(0)
            .css_classes(["linked"])
            .halign(gtk::Align::End)
            .valign(gtk::Align::Center)
            .build();
        widget.append(&automatic);
        widget.append(&fit);
        widget.append(&fill);

        let controls = Self {
            widget,
            buttons: [
                (CinemaArtworkFraming::Automatic, automatic),
                (CinemaArtworkFraming::Fit, fit),
                (CinemaArtworkFraming::Fill, fill),
            ],
        };
        controls.set_value(CinemaArtworkFraming::default());
        controls
    }

    pub(super) fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    pub(super) fn set_value(&self, value: CinemaArtworkFraming) {
        if let Some((_, button)) = self
            .buttons
            .iter()
            .find(|(candidate, _)| *candidate == value)
            && !button.is_active()
        {
            button.set_active(true);
        }
    }

    pub(super) fn connect_changed(&self, callback: impl Fn(CinemaArtworkFraming) + 'static) {
        let callback = Rc::new(callback);
        for (value, button) in &self.buttons {
            let value = *value;
            let callback = callback.clone();
            button.connect_toggled(move |button| {
                if button.is_active() {
                    callback(value);
                }
            });
        }
    }
}

/// Interactive album-cover preview for the point retained by a Fill crop.
#[derive(Clone)]
pub(super) struct CinemaCropFocusControls {
    widget: gtk::Box,
    preview: gtk::Overlay,
    preview_reservation: gtk::Box,
    picture: gtk::Picture,
    marker: gtk::DrawingArea,
    placeholder: gtk::Box,
    focus: Rc<Cell<CinemaCropFocus>>,
    source_dimensions: Rc<Cell<(i32, i32)>>,
    callbacks: Rc<RefCell<Vec<CropFocusCallback>>>,
}

impl CinemaCropFocusControls {
    pub(super) fn new() -> Self {
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .can_shrink(true)
            .hexpand(true)
            .vexpand(true)
            .build();
        picture.set_can_target(false);

        let marker = gtk::DrawingArea::builder()
            .hexpand(true)
            .vexpand(true)
            .build();
        marker.set_can_target(false);

        let placeholder_icon = gtk::Image::from_icon_name("image-missing-symbolic");
        placeholder_icon.set_pixel_size(32);
        let placeholder_label = gtk::Label::builder()
            .label(gettext("No album cover available"))
            .css_classes(["dim-label"])
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build();
        let placeholder = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        placeholder.append(&placeholder_icon);
        placeholder.append(&placeholder_label);
        placeholder.set_can_target(false);

        let preview_label = gettext("Crop focus");
        let preview_description = gettext(
            "Drag on the album cover or use the arrow keys to choose which point remains visible",
        );
        let preview = gtk::Overlay::builder()
            .width_request(REGULAR_PREVIEW_SIZE)
            .height_request(REGULAR_PREVIEW_SIZE)
            .overflow(gtk::Overflow::Hidden)
            .focusable(true)
            .accessible_role(gtk::AccessibleRole::Group)
            .css_classes(["card"])
            .build();
        let preview_reservation = gtk::Box::builder()
            .width_request(REGULAR_PREVIEW_SIZE)
            .height_request(REGULAR_PREVIEW_SIZE)
            .hexpand(true)
            .vexpand(true)
            .build();
        preview_reservation.set_can_target(false);
        preview.set_child(Some(&preview_reservation));
        preview.add_overlay(&picture);
        preview.set_measure_overlay(&picture, false);
        preview.set_clip_overlay(&picture, true);
        preview.add_overlay(&marker);
        preview.set_measure_overlay(&marker, false);
        preview.add_overlay(&placeholder);
        preview.set_measure_overlay(&placeholder, false);
        preview.update_property(&[
            gtk::accessible::Property::Label(preview_label.as_str()),
            gtk::accessible::Property::Description(preview_description.as_str()),
        ]);
        preview.set_tooltip_text(Some(&preview_description));

        let center = gtk::Button::with_label(&gettext("Center"));
        center.set_halign(gtk::Align::End);

        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .halign(gtk::Align::End)
            .valign(gtk::Align::Center)
            .build();
        widget.append(&preview);
        widget.append(&center);

        let focus = Rc::new(Cell::new(CinemaCropFocus::default()));
        let source_dimensions = Rc::new(Cell::new((0, 0)));
        let callbacks = Rc::new(RefCell::new(Vec::<CropFocusCallback>::new()));

        let focus_for_draw = focus.clone();
        let dimensions_for_draw = source_dimensions.clone();
        marker.set_draw_func(move |_, context, width, height| {
            let Some(rect) =
                PreviewRect::for_contained_image(width, height, dimensions_for_draw.get())
            else {
                return;
            };
            let (x, y) = rect.point_for_focus(focus_for_draw.get());
            draw_focus_marker(context, x, y, rect);
        });

        let focus_for_click = focus.clone();
        let dimensions_for_click = source_dimensions.clone();
        let marker_for_click = marker.clone();
        let preview_for_click = preview.clone();
        let callbacks_for_click = callbacks.clone();
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_PRIMARY);
        click.connect_pressed(move |_, _, x, y| {
            preview_for_click.grab_focus();
            set_focus_from_pointer(
                PointerFocusContext {
                    preview: &preview_for_click,
                    marker: &marker_for_click,
                    focus: &focus_for_click,
                    source_dimensions: &dimensions_for_click,
                    callbacks: &callbacks_for_click,
                },
                x,
                y,
                true,
            );
        });
        preview.add_controller(click);

        let focus_for_drag = focus.clone();
        let dimensions_for_drag = source_dimensions.clone();
        let marker_for_drag = marker.clone();
        let preview_for_drag = preview.clone();
        let callbacks_for_drag = callbacks.clone();
        let drag_origin = Rc::new(Cell::new((0.0, 0.0)));
        let drag_origin_for_begin = drag_origin.clone();
        let drag = gtk::GestureDrag::new();
        drag.set_button(gdk::BUTTON_PRIMARY);
        drag.connect_drag_begin(move |_, x, y| {
            drag_origin_for_begin.set((x, y));
            preview_for_drag.grab_focus();
            preview_for_drag.set_cursor_from_name(Some("grabbing"));
            set_focus_from_pointer(
                PointerFocusContext {
                    preview: &preview_for_drag,
                    marker: &marker_for_drag,
                    focus: &focus_for_drag,
                    source_dimensions: &dimensions_for_drag,
                    callbacks: &callbacks_for_drag,
                },
                x,
                y,
                false,
            );
        });
        let focus_for_drag_update = focus.clone();
        let dimensions_for_drag_update = source_dimensions.clone();
        let marker_for_drag_update = marker.clone();
        let preview_for_drag_update = preview.clone();
        let callbacks_for_drag_update = callbacks.clone();
        let drag_origin_for_update = drag_origin.clone();
        drag.connect_drag_update(move |_, offset_x, offset_y| {
            let (origin_x, origin_y) = drag_origin_for_update.get();
            set_focus_from_pointer(
                PointerFocusContext {
                    preview: &preview_for_drag_update,
                    marker: &marker_for_drag_update,
                    focus: &focus_for_drag_update,
                    source_dimensions: &dimensions_for_drag_update,
                    callbacks: &callbacks_for_drag_update,
                },
                origin_x + offset_x,
                origin_y + offset_y,
                false,
            );
        });
        let preview_for_drag_end = preview.clone();
        let focus_for_drag_end = focus.clone();
        let dimensions_for_drag_end = source_dimensions.clone();
        drag.connect_drag_end(move |_, _, _| {
            if dimensions_for_drag_end.get().0 > 0 {
                preview_for_drag_end.set_cursor_from_name(Some("crosshair"));
                announce_focus(&preview_for_drag_end, focus_for_drag_end.get());
            }
        });
        preview.add_controller(drag);

        let focus_for_keyboard = focus.clone();
        let marker_for_keyboard = marker.clone();
        let preview_for_keyboard = preview.clone();
        let callbacks_for_keyboard = callbacks.clone();
        let dimensions_for_keyboard = source_dimensions.clone();
        let keyboard = gtk::EventControllerKey::new();
        keyboard.connect_key_pressed(move |_, key, _, modifiers| {
            if dimensions_for_keyboard.get().0 <= 0 {
                return glib::Propagation::Proceed;
            }
            let step = if modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
                KEYBOARD_LARGE_STEP_BASIS_POINTS
            } else {
                KEYBOARD_STEP_BASIS_POINTS
            };
            let Some(next) = focus_for_key(focus_for_keyboard.get(), key, step) else {
                return glib::Propagation::Proceed;
            };
            apply_user_focus(
                &preview_for_keyboard,
                &marker_for_keyboard,
                &focus_for_keyboard,
                &callbacks_for_keyboard,
                next,
                true,
            );
            glib::Propagation::Stop
        });
        preview.add_controller(keyboard);

        let preview_for_center = preview.clone();
        let marker_for_center = marker.clone();
        let focus_for_center = focus.clone();
        let callbacks_for_center = callbacks.clone();
        center.connect_clicked(move |_| {
            apply_user_focus(
                &preview_for_center,
                &marker_for_center,
                &focus_for_center,
                &callbacks_for_center,
                CinemaCropFocus::CENTER,
                true,
            );
        });

        let controls = Self {
            widget,
            preview,
            preview_reservation,
            picture,
            marker,
            placeholder,
            focus,
            source_dimensions,
            callbacks,
        };
        controls.set_value(CinemaCropFocus::default());
        controls
    }

    pub(super) fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    pub(super) fn set_value(&self, value: CinemaCropFocus) {
        let changed = self.focus.replace(value) != value;
        if changed {
            self.marker.queue_draw();
            let callbacks = self.callbacks.borrow().clone();
            for callback in callbacks {
                callback(value);
            }
        }
        update_focus_description(&self.preview, value);
    }

    pub(super) fn connect_changed(&self, callback: impl Fn(CinemaCropFocus) + 'static) {
        self.callbacks.borrow_mut().push(Rc::new(callback));
    }

    /// Sets the artwork shown by the editor without decoding or copying pixels.
    pub(super) fn set_artwork(&self, paintable: Option<&gdk::Paintable>) {
        self.picture.set_paintable(paintable);
        let dimensions = paintable
            .map(|paintable| (paintable.intrinsic_width(), paintable.intrinsic_height()))
            .filter(|(width, height)| *width > 0 && *height > 0)
            .unwrap_or((0, 0));
        let has_artwork = dimensions.0 > 0 && dimensions.1 > 0;
        self.source_dimensions.set(dimensions);
        self.placeholder.set_visible(!has_artwork);
        self.preview
            .set_cursor_from_name(has_artwork.then_some("crosshair"));
        self.marker.queue_draw();
        if !has_artwork {
            let description = gettext("No album cover is available for crop-focus preview");
            self.preview
                .update_property(&[gtk::accessible::Property::Description(description.as_str())]);
        } else {
            update_focus_description(&self.preview, self.focus.get());
        }
    }

    /// Uses a smaller preview in the constrained Now Playing context menu.
    pub(super) fn set_compact(&self, compact: bool) {
        let size = if compact {
            COMPACT_PREVIEW_SIZE
        } else {
            REGULAR_PREVIEW_SIZE
        };
        self.preview.set_size_request(size, size);
        self.preview_reservation.set_size_request(size, size);
    }
}

fn set_focus_from_pointer(context: PointerFocusContext<'_>, x: f64, y: f64, announce: bool) {
    let Some(rect) = PreviewRect::for_contained_image(
        context.preview.width(),
        context.preview.height(),
        context.source_dimensions.get(),
    ) else {
        return;
    };
    apply_user_focus(
        context.preview,
        context.marker,
        context.focus,
        context.callbacks,
        rect.focus_at(x, y),
        announce,
    );
}

fn apply_user_focus(
    preview: &gtk::Overlay,
    marker: &gtk::DrawingArea,
    focus: &Cell<CinemaCropFocus>,
    callbacks: &RefCell<Vec<CropFocusCallback>>,
    value: CinemaCropFocus,
    announce: bool,
) {
    let changed = focus.replace(value) != value;
    update_focus_description(preview, value);
    if changed {
        marker.queue_draw();
        let callbacks = callbacks.borrow().clone();
        for callback in callbacks {
            callback(value);
        }
    }
    if announce {
        announce_focus(preview, value);
    }
}

fn focus_description(focus: CinemaCropFocus) -> String {
    let x = (u32::from(focus.x_basis_points()) + 50) / 100;
    let y = (u32::from(focus.y_basis_points()) + 50) / 100;
    gettext("Crop focus: {x}% from left, {y}% from top")
        .replace("{x}", &x.to_string())
        .replace("{y}", &y.to_string())
}

fn update_focus_description(preview: &gtk::Overlay, focus: CinemaCropFocus) {
    let description =
        gettext("{position}. Drag on the album cover or use the arrow keys to change the focus")
            .replace("{position}", &focus_description(focus));
    preview.update_property(&[gtk::accessible::Property::Description(description.as_str())]);
}

fn announce_focus(preview: &gtk::Overlay, focus: CinemaCropFocus) {
    preview.announce(
        &focus_description(focus),
        gtk::AccessibleAnnouncementPriority::Low,
    );
}

fn focus_for_key(focus: CinemaCropFocus, key: gdk::Key, step: u16) -> Option<CinemaCropFocus> {
    if key == gdk::Key::Home {
        return Some(CinemaCropFocus::CENTER);
    }

    let (delta_x, delta_y) = if key == gdk::Key::Left {
        (-i32::from(step), 0)
    } else if key == gdk::Key::Right {
        (i32::from(step), 0)
    } else if key == gdk::Key::Up {
        (0, -i32::from(step))
    } else if key == gdk::Key::Down {
        (0, i32::from(step))
    } else {
        return None;
    };

    let x = (i32::from(focus.x_basis_points()) + delta_x)
        .clamp(0, i32::from(CinemaCropFocus::SCALE)) as u16;
    let y = (i32::from(focus.y_basis_points()) + delta_y)
        .clamp(0, i32::from(CinemaCropFocus::SCALE)) as u16;
    Some(CinemaCropFocus::new(x, y))
}

fn draw_focus_marker(context: &cairo::Context, x: f64, y: f64, rect: PreviewRect) {
    const RADIUS: f64 = 9.0;
    const CROSSHAIR_LENGTH: f64 = 15.0;

    let inset = |value: f64, start: f64, length: f64| {
        if length >= RADIUS * 2.0 {
            value.clamp(start + RADIUS, start + length - RADIUS)
        } else {
            start + length / 2.0
        }
    };
    let x = inset(x, rect.x, rect.width);
    let y = inset(y, rect.y, rect.height);

    context.set_line_width(4.0);
    context.set_source_rgba(0.0, 0.0, 0.0, 0.72);
    context.arc(x, y, RADIUS, 0.0, std::f64::consts::TAU);
    let _ = context.stroke();

    context.set_line_width(2.0);
    context.set_source_rgba(1.0, 1.0, 1.0, 0.96);
    context.arc(x, y, RADIUS, 0.0, std::f64::consts::TAU);
    let _ = context.stroke();

    context.move_to(x - CROSSHAIR_LENGTH, y);
    context.line_to(x - RADIUS - 2.0, y);
    context.move_to(x + RADIUS + 2.0, y);
    context.line_to(x + CROSSHAIR_LENGTH, y);
    context.move_to(x, y - CROSSHAIR_LENGTH);
    context.line_to(x, y - RADIUS - 2.0);
    context.move_to(x, y + RADIUS + 2.0);
    context.line_to(x, y + CROSSHAIR_LENGTH);
    let _ = context.stroke();
}

#[cfg(test)]
mod tests {
    use super::{
        KEYBOARD_LARGE_STEP_BASIS_POINTS, KEYBOARD_STEP_BASIS_POINTS, PreviewRect, focus_for_key,
    };
    use crate::gui::now_playing_window::CinemaCropFocus;
    use gtk::prelude::*;

    #[test]
    fn contained_image_mapping_ignores_letterboxing_and_clamps_edges() {
        let rect = PreviewRect::for_contained_image(300, 200, (1_000, 1_000)).unwrap();
        assert_eq!(
            rect,
            PreviewRect {
                x: 50.0,
                y: 0.0,
                width: 200.0,
                height: 200.0
            }
        );
        assert_eq!(rect.focus_at(0.0, 100.0), CinemaCropFocus::LEFT);
        assert_eq!(rect.focus_at(150.0, 100.0), CinemaCropFocus::CENTER);
        assert_eq!(rect.focus_at(300.0, 100.0), CinemaCropFocus::RIGHT);
    }

    #[test]
    fn pointer_mapping_preserves_continuous_two_dimensional_focus() {
        let rect = PreviewRect::for_contained_image(200, 300, (2_000, 1_000)).unwrap();
        assert_eq!(
            rect,
            PreviewRect {
                x: 0.0,
                y: 100.0,
                width: 200.0,
                height: 100.0
            }
        );
        assert_eq!(
            rect.focus_at(50.0, 175.0),
            CinemaCropFocus::new(2_500, 7_500)
        );
    }

    #[test]
    fn keyboard_steps_clamp_and_home_centers() {
        let focus = CinemaCropFocus::new(50, 9_950);
        assert_eq!(
            focus_for_key(focus, gdk::Key::Left, KEYBOARD_STEP_BASIS_POINTS),
            Some(CinemaCropFocus::new(0, 9_950))
        );
        assert_eq!(
            focus_for_key(focus, gdk::Key::Down, KEYBOARD_LARGE_STEP_BASIS_POINTS),
            Some(CinemaCropFocus::new(50, CinemaCropFocus::SCALE))
        );
        assert_eq!(
            focus_for_key(focus, gdk::Key::Home, KEYBOARD_STEP_BASIS_POINTS),
            Some(CinemaCropFocus::CENTER)
        );
        assert_eq!(
            focus_for_key(focus, gdk::Key::space, KEYBOARD_STEP_BASIS_POINTS),
            None
        );
    }

    #[test]
    #[ignore = "requires a GTK display"]
    fn high_resolution_artwork_does_not_expand_the_compact_preview() {
        gtk::init().expect("GTK initialization");
        let controls = super::CinemaCropFocusControls::new();
        controls.set_compact(true);

        let dimension = 800;
        let bytes = glib::Bytes::from_owned(vec![0_u8; dimension * dimension * 4]);
        let texture = gdk::MemoryTexture::new(
            dimension as i32,
            dimension as i32,
            gdk::MemoryFormat::R8g8b8a8,
            &bytes,
            dimension * 4,
        );
        controls.set_artwork(Some(texture.upcast_ref()));

        let (minimum, natural, _, _) = controls.preview.measure(gtk::Orientation::Horizontal, -1);
        assert_eq!(minimum, super::COMPACT_PREVIEW_SIZE);
        assert_eq!(natural, super::COMPACT_PREVIEW_SIZE);

        let (minimum, natural, _, _) = controls.widget().measure(gtk::Orientation::Horizontal, -1);
        assert_eq!(minimum, super::COMPACT_PREVIEW_SIZE);
        assert_eq!(natural, super::COMPACT_PREVIEW_SIZE);
    }
}
