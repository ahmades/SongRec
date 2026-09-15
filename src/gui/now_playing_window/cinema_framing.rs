//! Reusable controls for Cinema artwork fitting and crop focus.

use super::{CinemaArtworkFraming, CinemaCropFocus};
use adw::prelude::*;
use gettextrs::gettext;
use std::rc::Rc;

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

/// Compact 3×3 selector for the point retained by a Fill crop.
#[derive(Clone)]
pub(super) struct CinemaCropFocusControls {
    widget: gtk::Grid,
    buttons: Vec<(CinemaCropFocus, gtk::ToggleButton)>,
}

impl CinemaCropFocusControls {
    pub(super) fn new() -> Self {
        let widget = gtk::Grid::builder()
            .row_spacing(0)
            .column_spacing(0)
            .row_homogeneous(true)
            .column_homogeneous(true)
            .css_classes(["linked"])
            .halign(gtk::Align::End)
            .valign(gtk::Align::Center)
            .build();
        let positions = [
            (CinemaCropFocus::TopLeft, "↖", 0, 0),
            (CinemaCropFocus::Top, "↑", 1, 0),
            (CinemaCropFocus::TopRight, "↗", 2, 0),
            (CinemaCropFocus::Left, "←", 0, 1),
            (CinemaCropFocus::Center, "●", 1, 1),
            (CinemaCropFocus::Right, "→", 2, 1),
            (CinemaCropFocus::BottomLeft, "↙", 0, 2),
            (CinemaCropFocus::Bottom, "↓", 1, 2),
            (CinemaCropFocus::BottomRight, "↘", 2, 2),
        ];
        let mut buttons = Vec::with_capacity(positions.len());
        let mut group = None::<gtk::ToggleButton>;
        for (focus, symbol, column, row) in positions {
            let button = gtk::ToggleButton::with_label(symbol);
            button.set_size_request(34, 30);
            if let Some(group) = &group {
                button.set_group(Some(group));
            } else {
                group = Some(button.clone());
            }
            let description = focus_description(focus);
            button.set_tooltip_text(Some(&description));
            button.update_property(&[gtk::accessible::Property::Label(description.as_str())]);
            widget.attach(&button, column, row, 1, 1);
            buttons.push((focus, button));
        }

        let controls = Self { widget, buttons };
        controls.set_value(CinemaCropFocus::default());
        controls
    }

    pub(super) fn widget(&self) -> &gtk::Grid {
        &self.widget
    }

    pub(super) fn set_value(&self, value: CinemaCropFocus) {
        if let Some((_, button)) = self
            .buttons
            .iter()
            .find(|(candidate, _)| *candidate == value)
            && !button.is_active()
        {
            button.set_active(true);
        }
    }

    pub(super) fn connect_changed(&self, callback: impl Fn(CinemaCropFocus) + 'static) {
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

fn focus_description(focus: CinemaCropFocus) -> String {
    match focus {
        CinemaCropFocus::TopLeft => gettext("Top left"),
        CinemaCropFocus::Top => gettext("Top"),
        CinemaCropFocus::TopRight => gettext("Top right"),
        CinemaCropFocus::Left => gettext("Left"),
        CinemaCropFocus::Center => gettext("Center"),
        CinemaCropFocus::Right => gettext("Right"),
        CinemaCropFocus::BottomLeft => gettext("Bottom left"),
        CinemaCropFocus::Bottom => gettext("Bottom"),
        CinemaCropFocus::BottomRight => gettext("Bottom right"),
    }
}
