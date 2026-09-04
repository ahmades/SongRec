//! GTK slider presentation for the track-info text-size preference.

use super::TextSize;
use adw::prelude::*;
use gettextrs::gettext;

impl TextSize {
    /// Returns the localized label used by the named slider marks.
    fn translated_label(self) -> String {
        if self == Self::SMALL {
            gettext("Small")
        } else if self == Self::MEDIUM {
            gettext("Medium")
        } else {
            gettext("Large")
        }
    }

    /// Configures a continuous slider with three labeled snap anchors.
    pub(crate) fn configure_scale(scale: &gtk::Scale) {
        scale.adjustment().configure(
            Self::default().scale_value(),
            Self::MIN_SCALE_VALUE,
            Self::MAX_SCALE_VALUE,
            Self::SCALE_STEP,
            Self::PAGE_STEP,
            0.0,
        );
        scale.set_round_digits(0);
        scale.set_digits(0);
        scale.set_draw_value(false);

        for size in Self::ALL {
            let label = size.translated_label();
            scale.add_mark(size.scale_value(), gtk::PositionType::Bottom, Some(&label));
        }
    }

    /// Snaps a drag near one of the named positions once it ends.
    pub(crate) fn install_slider_snap(scale: &gtk::Scale) {
        let scale = scale.clone();
        let scale_for_drag_end = scale.clone();
        let drag = gtk::GestureDrag::new();
        drag.set_propagation_phase(gtk::PropagationPhase::Capture);
        drag.connect_drag_end(move |_, _, _| {
            let snapped_value = Self::snapped_scale_value(scale_for_drag_end.value());
            if scale_for_drag_end.value() != snapped_value {
                scale_for_drag_end.set_value(snapped_value);
            }
        });
        scale.add_controller(drag);
    }
}
