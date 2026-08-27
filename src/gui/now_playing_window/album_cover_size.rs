//! GTK slider presentation for the album-cover-size preference.

use crate::core::preferences::AlbumCoverSize;
use adw::prelude::*;
use gettextrs::gettext;

impl AlbumCoverSize {
    /// Returns the localized label used by the named slider marks.
    pub(crate) fn translated_label(self) -> String {
        if self == Self::SMALL {
            gettext("Small")
        } else if self == Self::MEDIUM {
            gettext("Medium")
        } else {
            gettext("Large")
        }
    }

    /// Configures a continuous slider with three labeled snap anchors and two
    /// unlabeled midpoint ticks.
    pub(crate) fn configure_scale(scale: &gtk::Scale) {
        scale.set_round_digits(0);
        scale.set_digits(0);
        scale.set_draw_value(false);

        for size in Self::ALL {
            let label = size.translated_label();
            scale.add_mark(size.scale_value(), gtk::PositionType::Bottom, Some(&label));
        }
        for midpoint in [Self::SMALL_MEDIUM_MIDPOINT, Self::MEDIUM_LARGE_MIDPOINT] {
            scale.add_mark(midpoint.scale_value(), gtk::PositionType::Bottom, None);
        }
    }

    /// Snaps a drag near one of the labeled slider marks once it ends.
    pub(crate) fn install_slider_snap(scale: &gtk::Scale) {
        let scale = scale.clone();
        let scale_for_drag_end = scale.clone();
        let drag = gtk::GestureDrag::new();
        drag.set_propagation_phase(gtk::PropagationPhase::Capture);
        drag.connect_drag_end(move |_, _, _| {
            let snapped_value =
                Self::from_snapped_scale_value(scale_for_drag_end.value()).scale_value();
            if scale_for_drag_end.value() != snapped_value {
                scale_for_drag_end.set_value(snapped_value);
            }
        });
        scale.add_controller(drag);
    }
}
