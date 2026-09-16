//! Searchable management UI for saved Now Playing preference snapshots.

use super::SettingsController;
use crate::core::preferences::{
    NOW_PLAYING_PRESET_NAME_MAX_CHARS, NowPlayingPresetId, PresetError,
};
use adw::prelude::*;
use gettextrs::gettext;
use std::cell::RefCell;
use std::rc::Rc;

const DIALOG_CONTENT_WIDTH: i32 = 560;
const DIALOG_CONTENT_HEIGHT: i32 = 520;
const CONTENT_MARGIN: i32 = 12;
const CONTENT_SPACING: i32 = 12;

#[derive(Clone)]
struct PresetSummary {
    id: NowPlayingPresetId,
    name: String,
    folded_name: String,
    is_current: bool,
    is_modified: bool,
}

#[derive(Clone)]
struct ManagerHandles {
    dialog: glib::WeakRef<adw::Dialog>,
    store: glib::WeakRef<gio::ListStore>,
    filtered: glib::WeakRef<gtk::FilterListModel>,
    selection: glib::WeakRef<gtk::SingleSelection>,
    content_stack: glib::WeakRef<gtk::Stack>,
    status_page: glib::WeakRef<adw::StatusPage>,
    load: glib::WeakRef<gtk::Button>,
    update: glib::WeakRef<gtk::Button>,
    rename: glib::WeakRef<gtk::Button>,
    delete: glib::WeakRef<gtk::Button>,
    controller: SettingsController,
}

impl ManagerHandles {
    fn selected(&self) -> Option<PresetSummary> {
        let selection = self.selection.upgrade()?;
        selected_summary(&selection)
    }

    fn refresh(&self, preferred_id: Option<NowPlayingPresetId>) {
        let Some(store) = self.store.upgrade() else {
            return;
        };
        let Some(filtered) = self.filtered.upgrade() else {
            return;
        };
        let Some(selection) = self.selection.upgrade() else {
            return;
        };

        let selected_id = self.controller.selected_preset_id();
        let selected_is_modified = self.controller.selected_preset_is_modified();
        let catalog = self.controller.presets();
        let catalog_is_empty = catalog.is_empty();
        let mut presets = Vec::with_capacity(catalog.len());
        presets.extend(catalog.items().iter().map(|preset| {
            let name = if preset.name.trim().is_empty() {
                gettext("Unnamed preset")
            } else {
                preset.name.trim().to_owned()
            };
            PresetSummary {
                id: preset.id,
                folded_name: name.to_lowercase(),
                name,
                is_current: selected_id == Some(preset.id),
                is_modified: selected_id == Some(preset.id) && selected_is_modified,
            }
        }));
        presets.sort_by(|left, right| {
            left.folded_name
                .cmp(&right.folded_name)
                .then_with(|| left.id.cmp(&right.id))
        });

        let objects = presets
            .into_iter()
            .map(glib::BoxedAnyObject::new)
            .collect::<Vec<_>>();
        store.splice(0, store.n_items(), &objects);

        let target_id = preferred_id.or(selected_id);
        let target_position = target_id.and_then(|id| {
            (0..filtered.n_items()).find(|&position| {
                filtered
                    .item(position)
                    .and_then(|item| item.downcast::<glib::BoxedAnyObject>().ok())
                    .and_then(|item| {
                        item.try_borrow::<PresetSummary>()
                            .ok()
                            .map(|preset| preset.id == id)
                    })
                    .unwrap_or(false)
            })
        });

        if catalog_is_empty {
            selection.set_selected(gtk::INVALID_LIST_POSITION);
        } else if let Some(position) = target_position {
            selection.set_selected(position);
        } else if filtered.n_items() > 0 {
            selection.set_selected(0);
        } else {
            selection.set_selected(gtk::INVALID_LIST_POSITION);
        }

        self.update_content_state();
        self.update_action_state();
    }

    fn update_content_state(&self) {
        let Some(stack) = self.content_stack.upgrade() else {
            return;
        };
        let Some(status_page) = self.status_page.upgrade() else {
            return;
        };
        let Some(store) = self.store.upgrade() else {
            return;
        };
        let Some(filtered) = self.filtered.upgrade() else {
            return;
        };

        if store.n_items() == 0 {
            status_page.set_title(&gettext("No saved presets"));
            status_page.set_description(Some(&gettext(
                "Save the current Now Playing settings to create your first preset.",
            )));
            stack.set_visible_child_name("status");
        } else if filtered.n_items() == 0 {
            status_page.set_title(&gettext("No matching presets"));
            status_page.set_description(Some(&gettext("Try a different search.")));
            stack.set_visible_child_name("status");
        } else {
            stack.set_visible_child_name("list");
        }
    }

    fn update_action_state(&self) {
        let has_selection = self.selected().is_some();
        for button in [&self.load, &self.update, &self.rename, &self.delete] {
            if let Some(button) = button.upgrade() {
                button.set_sensitive(has_selection);
            }
        }
    }

    fn load_selected(&self) {
        let Some(preset) = self.selected() else {
            return;
        };
        match self.controller.load_preset(preset.id) {
            Ok(()) => {
                let dialog = self.dialog.clone();
                // Loading can be triggered by activating a virtualized list
                // item. Let that activation finish before its dialog and
                // focused child are removed from the widget hierarchy.
                glib::idle_add_local_once(move || {
                    if let Some(dialog) = dialog.upgrade() {
                        let _ = dialog.close();
                    }
                });
            }
            Err(error) => self.show_error(&gettext("Could not load preset"), error),
        }
    }

    fn show_error(&self, heading: &str, error: PresetError) {
        let Some(parent) = self.dialog.upgrade() else {
            return;
        };
        let alert = adw::AlertDialog::builder()
            .heading(heading)
            .body(preset_error_message(error))
            .close_response("ok")
            .default_response("ok")
            .build();
        alert.add_responses(&[("ok", &gettext("_OK"))]);
        alert.choose(Some(&parent), None::<&gio::Cancellable>, |_| {});
    }
}

/// A searchable, virtualized manager for saved Now Playing presets.
pub(super) struct PresetManagerDialog {
    dialog: adw::Dialog,
    handles: ManagerHandles,
    #[cfg(test)]
    list: gtk::ListView,
}

impl PresetManagerDialog {
    pub(super) fn new(controller: SettingsController) -> Self {
        let title = adw::WindowTitle::new(&gettext("Now Playing presets"), "");
        let header = adw::HeaderBar::builder().title_widget(&title).build();
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);

        let search = gtk::SearchEntry::builder()
            .placeholder_text(gettext("Search presets"))
            .hexpand(true)
            .build();
        search.update_property(&[gtk::accessible::Property::Label(&gettext(
            "Search saved Now Playing presets",
        ))]);

        let query = Rc::new(RefCell::new(String::new()));
        let query_for_filter = query.clone();
        let filter = gtk::CustomFilter::new(move |item| {
            let Some(item) = item.downcast_ref::<glib::BoxedAnyObject>() else {
                return false;
            };
            let Ok(preset) = item.try_borrow::<PresetSummary>() else {
                return false;
            };
            let query = query_for_filter.borrow();
            query.is_empty() || preset.folded_name.contains(query.as_str())
        });

        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let filtered = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));
        let selection = gtk::SingleSelection::new(Some(filtered.clone()));
        selection.set_autoselect(true);
        selection.set_can_unselect(false);

        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };

            // AdwActionRow is a GtkListBoxRow and must only be parented by a
            // GtkListBox. GtkListView supplies its own virtualized item
            // wrapper, so use ordinary widgets for the item contents.
            let title = gtk::Label::builder()
                .halign(gtk::Align::Fill)
                .hexpand(true)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            let subtitle = gtk::Label::builder()
                .css_classes(["caption", "dim-label"])
                .halign(gtk::Align::Fill)
                .hexpand(true)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            let row = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(2)
                .margin_top(8)
                .margin_bottom(8)
                .margin_start(12)
                .margin_end(12)
                .build();
            row.append(&title);
            row.append(&subtitle);
            item.set_child(Some(&row));
        });
        factory.connect_bind(|_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(row) = item.child().and_downcast::<gtk::Box>() else {
                return;
            };
            let Some(title) = row.first_child().and_downcast::<gtk::Label>() else {
                return;
            };
            let Some(subtitle) = title.next_sibling().and_downcast::<gtk::Label>() else {
                return;
            };
            let Some(preset) =
                item.item()
                    .and_downcast::<glib::BoxedAnyObject>()
                    .and_then(|item| {
                        item.try_borrow::<PresetSummary>()
                            .ok()
                            .map(|preset| preset.clone())
                    })
            else {
                return;
            };

            title.set_label(&preset.name);
            let subtitle_text = if preset.is_current {
                if preset.is_modified {
                    gettext("Current preset — modified")
                } else {
                    gettext("Current preset")
                }
            } else {
                String::new()
            };
            subtitle.set_label(&subtitle_text);
            subtitle.set_visible(!subtitle_text.is_empty());
            row.set_tooltip_text(Some(&preset.name));
        });

        let list = gtk::ListView::new(Some(selection.clone()), Some(factory));
        list.set_single_click_activate(false);
        list.add_css_class("boxed-list");
        list.update_property(&[gtk::accessible::Property::Label(&gettext(
            "Saved Now Playing presets",
        ))]);

        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .child(&list)
            .build();

        let status_page = adw::StatusPage::builder()
            .icon_name("document-open-recent-symbolic")
            .vexpand(true)
            .build();
        let content_stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .vexpand(true)
            .build();
        content_stack.add_named(&scrolled, Some("list"));
        content_stack.add_named(&status_page, Some("status"));

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(CONTENT_SPACING)
            .margin_top(CONTENT_MARGIN)
            .margin_bottom(CONTENT_MARGIN)
            .margin_start(CONTENT_MARGIN)
            .margin_end(CONTENT_MARGIN)
            .build();
        content.append(&search);
        content.append(&content_stack);
        toolbar.set_content(Some(&content));

        let save_new = gtk::Button::with_label(&gettext("Save current as new"));
        let delete = gtk::Button::builder()
            .icon_name("edit-delete-symbolic")
            .tooltip_text(gettext("Delete preset"))
            .build();
        let rename = gtk::Button::builder()
            .icon_name("document-edit-symbolic")
            .tooltip_text(gettext("Rename preset"))
            .build();
        let update = gtk::Button::with_label(&gettext("Update"));
        let load = gtk::Button::with_label(&gettext("Load"));
        load.add_css_class("suggested-action");
        delete.update_property(&[gtk::accessible::Property::Label(&gettext(
            "Delete selected preset",
        ))]);
        rename.update_property(&[gtk::accessible::Property::Label(&gettext(
            "Rename selected preset",
        ))]);

        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        let actions = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .margin_top(CONTENT_MARGIN)
            .margin_bottom(CONTENT_MARGIN)
            .margin_start(CONTENT_MARGIN)
            .margin_end(CONTENT_MARGIN)
            .build();
        actions.append(&save_new);
        actions.append(&spacer);
        actions.append(&delete);
        actions.append(&rename);
        actions.append(&update);
        actions.append(&load);
        toolbar.add_bottom_bar(&actions);

        let dialog = adw::Dialog::builder()
            .title(gettext("Now Playing presets"))
            .content_width(DIALOG_CONTENT_WIDTH)
            .content_height(DIALOG_CONTENT_HEIGHT)
            .child(&toolbar)
            .build();

        let handles = ManagerHandles {
            dialog: dialog.downgrade(),
            store: store.downgrade(),
            filtered: filtered.downgrade(),
            selection: selection.downgrade(),
            content_stack: content_stack.downgrade(),
            status_page: status_page.downgrade(),
            load: load.downgrade(),
            update: update.downgrade(),
            rename: rename.downgrade(),
            delete: delete.downgrade(),
            controller,
        };

        let filter_for_search = filter.downgrade();
        let handles_for_search = handles.clone();
        search.connect_search_changed(move |search| {
            *query.borrow_mut() = search.text().trim().to_lowercase();
            if let Some(filter) = filter_for_search.upgrade() {
                filter.changed(gtk::FilterChange::Different);
            }
            if let Some(selection) = handles_for_search.selection.upgrade()
                && let Some(filtered) = handles_for_search.filtered.upgrade()
                && filtered.n_items() > 0
                && selection.selected_item().is_none()
            {
                selection.set_selected(0);
            }
            handles_for_search.update_content_state();
            handles_for_search.update_action_state();
        });

        let handles_for_selection = handles.clone();
        selection.connect_selected_notify(move |_| {
            handles_for_selection.update_action_state();
        });

        let handles_for_save = handles.clone();
        save_new.connect_clicked(move |_| {
            present_name_dialog(&handles_for_save, NameOperation::Create);
        });

        let handles_for_load = handles.clone();
        load.connect_clicked(move |_| handles_for_load.load_selected());

        let handles_for_activate = handles.clone();
        list.connect_activate(move |_, _| handles_for_activate.load_selected());

        let handles_for_update = handles.clone();
        update.connect_clicked(move |_| present_update_confirmation(&handles_for_update));

        let handles_for_rename = handles.clone();
        rename.connect_clicked(move |_| {
            if let Some(preset) = handles_for_rename.selected() {
                present_name_dialog(
                    &handles_for_rename,
                    NameOperation::Rename {
                        id: preset.id,
                        current_name: preset.name,
                    },
                );
            }
        });

        let handles_for_delete = handles.clone();
        delete.connect_clicked(move |_| present_delete_confirmation(&handles_for_delete));

        let manager = Self {
            dialog,
            handles,
            #[cfg(test)]
            list,
        };
        manager.refresh();
        manager
    }

    pub(super) fn present(&self, parent: &impl IsA<gtk::Widget>) {
        self.dialog.present(Some(parent));
    }

    /// Refreshes an already-open manager after a catalog update from another view.
    pub(super) fn refresh(&self) {
        let preferred_id = self.handles.selected().map(|preset| preset.id);
        self.handles.refresh(preferred_id);
    }
}

#[derive(Clone)]
enum NameOperation {
    Create,
    Rename {
        id: NowPlayingPresetId,
        current_name: String,
    },
}

impl NameOperation {
    fn excluded_id(&self) -> Option<NowPlayingPresetId> {
        match self {
            Self::Create => None,
            Self::Rename { id, .. } => Some(*id),
        }
    }
}

fn present_name_dialog(handles: &ManagerHandles, operation: NameOperation) {
    let Some(parent) = handles.dialog.upgrade() else {
        return;
    };
    let (heading, response_label, initial_name) = match &operation {
        NameOperation::Create => (
            gettext("Save Now Playing preset"),
            gettext("_Save"),
            String::new(),
        ),
        NameOperation::Rename { current_name, .. } => (
            gettext("Rename preset"),
            gettext("_Rename"),
            current_name.clone(),
        ),
    };

    let entry = gtk::Entry::builder()
        .activates_default(true)
        .hexpand(true)
        .placeholder_text(gettext("Preset name"))
        .text(&initial_name)
        .build();
    entry.update_property(&[gtk::accessible::Property::Label(&gettext("Preset name"))]);
    let validation = gtk::Label::builder()
        .css_classes(["caption", "error"])
        .halign(gtk::Align::Start)
        .wrap(true)
        .xalign(0.0)
        .build();
    entry.update_relation(&[gtk::accessible::Relation::DescribedBy(&[
        validation.upcast_ref()
    ])]);
    let extra = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .width_request(360)
        .build();
    extra.append(&entry);
    extra.append(&validation);

    let alert = adw::AlertDialog::builder()
        .heading(heading)
        .extra_child(&extra)
        .close_response("cancel")
        .default_response("confirm")
        .focus_widget(&entry)
        .build();
    alert.add_responses(&[
        ("cancel", &gettext("_Cancel")),
        ("confirm", &response_label),
    ]);

    update_name_validation(
        &alert,
        &validation,
        &handles.controller,
        entry.text().as_str(),
        operation.excluded_id(),
    );
    let alert_for_validation = alert.downgrade();
    let validation_for_entry = validation.clone();
    let controller_for_validation = handles.controller.clone();
    let operation_for_validation = operation.clone();
    entry.connect_changed(move |entry| {
        if let Some(alert) = alert_for_validation.upgrade() {
            update_name_validation(
                &alert,
                &validation_for_entry,
                &controller_for_validation,
                entry.text().as_str(),
                operation_for_validation.excluded_id(),
            );
        }
    });

    entry.select_region(0, -1);
    let handles_for_response = handles.clone();
    alert.choose(Some(&parent), None::<&gio::Cancellable>, move |response| {
        if response != "confirm" {
            return;
        }
        let name = entry.text();
        let (result, error_heading) = match operation {
            NameOperation::Create => (
                handles_for_response
                    .controller
                    .create_preset(name.as_str())
                    .map(Some),
                gettext("Could not save preset"),
            ),
            NameOperation::Rename { id, .. } => (
                handles_for_response
                    .controller
                    .rename_preset(id, name.as_str())
                    .map(|()| Some(id)),
                gettext("Could not rename preset"),
            ),
        };
        match result {
            Ok(preferred_id) => handles_for_response.refresh(preferred_id),
            Err(error) => handles_for_response.show_error(&error_heading, error),
        }
    });
}

fn update_name_validation(
    alert: &adw::AlertDialog,
    validation: &gtk::Label,
    controller: &SettingsController,
    name: &str,
    excluded_id: Option<NowPlayingPresetId>,
) {
    let error = validate_name(controller, name, excluded_id);
    alert.set_response_enabled("confirm", error.is_none());
    validation.set_label(error.as_deref().unwrap_or(""));
    validation.set_visible(error.is_some());
}

fn validate_name(
    controller: &SettingsController,
    name: &str,
    excluded_id: Option<NowPlayingPresetId>,
) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return Some(gettext("Enter a preset name."));
    }
    if name.chars().count() > NOW_PLAYING_PRESET_NAME_MAX_CHARS {
        return Some(
            gettext("Preset names can contain at most {max} characters.")
                .replace("{max}", &NOW_PLAYING_PRESET_NAME_MAX_CHARS.to_string()),
        );
    }

    let folded_name = name.to_lowercase();
    controller
        .presets()
        .items()
        .iter()
        .any(|preset| {
            Some(preset.id) != excluded_id && preset.name.trim().to_lowercase() == folded_name
        })
        .then(|| gettext("A preset with this name already exists."))
}

fn present_update_confirmation(handles: &ManagerHandles) {
    let Some(preset) = handles.selected() else {
        return;
    };
    let Some(parent) = handles.dialog.upgrade() else {
        return;
    };
    let alert = adw::AlertDialog::builder()
        .heading(gettext("Update preset?"))
        .body(
            gettext("Replace “{name}” with the current Now Playing settings?")
                .replace("{name}", &preset.name),
        )
        .close_response("cancel")
        .default_response("cancel")
        .build();
    alert.add_responses(&[
        ("cancel", &gettext("_Cancel")),
        ("update", &gettext("_Update")),
    ]);

    let handles_for_response = handles.clone();
    alert.choose(Some(&parent), None::<&gio::Cancellable>, move |response| {
        if response != "update" {
            return;
        }
        match handles_for_response.controller.update_preset(preset.id) {
            Ok(()) => handles_for_response.refresh(Some(preset.id)),
            Err(error) => {
                handles_for_response.show_error(&gettext("Could not update preset"), error)
            }
        }
    });
}

fn present_delete_confirmation(handles: &ManagerHandles) {
    let Some(preset) = handles.selected() else {
        return;
    };
    let Some(parent) = handles.dialog.upgrade() else {
        return;
    };
    let alert = adw::AlertDialog::builder()
        .heading(gettext("Delete preset?"))
        .body(gettext("Delete “{name}”? This cannot be undone.").replace("{name}", &preset.name))
        .close_response("cancel")
        .default_response("cancel")
        .build();
    alert.add_responses(&[
        ("cancel", &gettext("_Cancel")),
        ("delete", &gettext("_Delete")),
    ]);
    alert.set_response_appearance("delete", adw::ResponseAppearance::Destructive);

    let handles_for_response = handles.clone();
    alert.choose(Some(&parent), None::<&gio::Cancellable>, move |response| {
        if response != "delete" {
            return;
        }
        match handles_for_response.controller.delete_preset(preset.id) {
            Ok(()) => handles_for_response.refresh(None),
            Err(error) => {
                handles_for_response.show_error(&gettext("Could not delete preset"), error)
            }
        }
    });
}

fn selected_summary(selection: &gtk::SingleSelection) -> Option<PresetSummary> {
    let item = selection
        .selected_item()?
        .downcast::<glib::BoxedAnyObject>()
        .ok()?;
    let preset = item.try_borrow::<PresetSummary>().ok()?;
    Some(preset.clone())
}

fn preset_error_message(error: PresetError) -> String {
    match error {
        PresetError::EmptyName => gettext("Enter a preset name."),
        PresetError::NameTooLong { max_chars } => {
            gettext("Preset names can contain at most {max} characters.")
                .replace("{max}", &max_chars.to_string())
        }
        PresetError::DuplicateName => gettext("A preset with this name already exists."),
        PresetError::NotFound => gettext("The selected preset no longer exists."),
        PresetError::IdExhausted => gettext("A new preset could not be assigned an identifier."),
    }
}

#[cfg(test)]
mod tests {
    use super::{PresetManagerDialog, SettingsController};
    use crate::core::preferences::{DisplayMode, NowPlayingPreferences, NowPlayingPresetCatalog};
    use adw::prelude::*;

    #[test]
    #[ignore = "requires a GTK display; run with G_DEBUG=fatal-criticals"]
    fn loading_a_focused_virtualized_preset_row_is_focus_safe() {
        let _serial = crate::MAIN_CONTEXT_TEST_LOCK.lock().unwrap();
        adw::init().unwrap();

        let mut saved = NowPlayingPreferences::default();
        saved.display_mode = DisplayMode::Cinema;
        let mut presets = NowPlayingPresetCatalog::default();
        presets.create("Cinema", saved).unwrap();
        let controller =
            SettingsController::new_with_presets(NowPlayingPreferences::default(), presets, None);
        let parent = gtk::Window::new();
        let manager = PresetManagerDialog::new(controller.clone());

        parent.present();
        manager.present(&parent);
        while glib::MainContext::default().iteration(false) {}

        assert!(manager.list.grab_focus());
        manager
            .handles
            .load
            .upgrade()
            .expect("load button")
            .emit_clicked();
        while glib::MainContext::default().iteration(false) {}

        assert_eq!(controller.settings(), saved);
        parent.destroy();
    }
}
