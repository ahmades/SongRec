//! Compact editors shared by the two Now Playing settings surfaces.

use super::DisplayedInformation;
use adw::prelude::*;
use gettextrs::{gettext, ngettext};
use std::cell::Cell;
use std::rc::Rc;

const CHECKBOX_COLUMN_SPACING: i32 = 40;

#[derive(Clone)]
pub(super) struct DisplayedInformationEditor {
    widget: gtk::Grid,
    album: gtk::CheckButton,
    record_label: gtk::CheckButton,
    release_year: gtk::CheckButton,
    genre: gtk::CheckButton,
    recognition_age: gtk::CheckButton,
    applying: Rc<Cell<bool>>,
}

impl DisplayedInformationEditor {
    pub(super) fn new() -> Self {
        let widget = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(CHECKBOX_COLUMN_SPACING)
            .hexpand(true)
            .build();
        let album = gtk::CheckButton::with_label(&gettext("Album"));
        let record_label = gtk::CheckButton::with_label(&gettext("Record label"));
        let release_year = gtk::CheckButton::with_label(&gettext("Year"));
        let genre = gtk::CheckButton::with_label(&gettext("Genre"));
        let recognition_age = gtk::CheckButton::with_label(&gettext("Recognition age"));

        widget.attach(&album, 0, 0, 1, 1);
        widget.attach(&release_year, 1, 0, 1, 1);
        widget.attach(&record_label, 0, 1, 1, 1);
        widget.attach(&genre, 1, 1, 1, 1);
        widget.attach(&recognition_age, 0, 2, 2, 1);

        let editor = Self {
            widget,
            album,
            record_label,
            release_year,
            genre,
            recognition_age,
            applying: Rc::new(Cell::new(false)),
        };
        editor.set_value(DisplayedInformation::default());
        editor
    }

    pub(super) fn widget(&self) -> &gtk::Grid {
        &self.widget
    }

    pub(super) fn value(&self) -> DisplayedInformation {
        DisplayedInformation {
            album: self.album.is_active(),
            record_label: self.record_label.is_active(),
            release_year: self.release_year.is_active(),
            genre: self.genre.is_active(),
            recognition_age: self.recognition_age.is_active(),
        }
    }

    pub(super) fn set_value(&self, value: DisplayedInformation) {
        let was_applying = self.applying.replace(true);
        self.album.set_active(value.album);
        self.record_label.set_active(value.record_label);
        self.release_year.set_active(value.release_year);
        self.genre.set_active(value.genre);
        self.recognition_age.set_active(value.recognition_age);
        self.applying.set(was_applying);
    }

    pub(super) fn connect_changed(&self, callback: impl Fn(DisplayedInformation) + 'static) {
        let callback: Rc<dyn Fn(DisplayedInformation)> = Rc::new(callback);
        for button in [
            &self.album,
            &self.record_label,
            &self.release_year,
            &self.genre,
            &self.recognition_age,
        ] {
            let editor = self.clone();
            let callback = callback.clone();
            button.connect_toggled(move |_| {
                if !editor.applying.get() {
                    callback(editor.value());
                }
            });
        }
    }
}

pub(super) fn selection_summary(value: DisplayedInformation) -> String {
    let count = value.enabled_count() as u32;
    match count {
        0 => gettext("None"),
        5 => gettext("All"),
        _ => {
            ngettext("1 selected", "{count} selected", count).replace("{count}", &count.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::selection_summary;
    use crate::core::preferences::DisplayedInformation;

    #[test]
    fn summary_reports_empty_partial_and_complete_selections() {
        let mut value = DisplayedInformation {
            album: false,
            record_label: false,
            release_year: false,
            genre: false,
            recognition_age: false,
        };
        assert_eq!(selection_summary(value), "None");
        value.album = true;
        assert_eq!(selection_summary(value), "1 selected");
        value.release_year = true;
        assert_eq!(selection_summary(value), "2 selected");
        value.record_label = true;
        value.genre = true;
        value.recognition_age = true;
        assert_eq!(selection_summary(value), "All");
    }
}
