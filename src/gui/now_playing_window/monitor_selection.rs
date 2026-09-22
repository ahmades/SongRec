//! Live monitor discovery, selection, and fullscreen-target resolution.

use crate::core::preferences::FullscreenMonitorTarget;
use adw::prelude::*;
use gettextrs::gettext;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Clone)]
struct MonitorChoice {
    target: Option<FullscreenMonitorTarget>,
}

struct MonitorSelectorInner {
    dropdown: gtk::DropDown,
    choices: RefCell<Vec<MonitorChoice>>,
    target: RefCell<Option<FullscreenMonitorTarget>>,
    applying: Cell<bool>,
    connected_count: Cell<u32>,
    visible_when_multiple: RefCell<Vec<gtk::Widget>>,
    sensitive_when_multiple: RefCell<Vec<gtk::Widget>>,
    topology_changed_handlers: RefCell<Vec<Box<dyn Fn()>>>,
}

/// A dropdown backed by GDK's live monitor list.
///
/// Each view owns its GTK widget while GDK supplies the same live `ListModel`.
/// Rebuilding is guarded so hot-plug events never overwrite a remembered but
/// temporarily unavailable target.
#[derive(Clone)]
pub(super) struct MonitorSelector {
    inner: Rc<MonitorSelectorInner>,
    _monitors: gio::ListModel,
}

impl MonitorSelector {
    pub(super) fn new(target: Option<FullscreenMonitorTarget>) -> Self {
        let dropdown = gtk::DropDown::from_strings(&[&gettext("Automatic")]);
        dropdown.set_enable_search(true);
        dropdown.set_halign(gtk::Align::End);
        dropdown.set_valign(gtk::Align::Center);

        let inner = Rc::new(MonitorSelectorInner {
            dropdown,
            choices: RefCell::new(Vec::new()),
            target: RefCell::new(target),
            applying: Cell::new(false),
            connected_count: Cell::new(0),
            visible_when_multiple: RefCell::new(Vec::new()),
            sensitive_when_multiple: RefCell::new(Vec::new()),
            topology_changed_handlers: RefCell::new(Vec::new()),
        });
        let display = gdk::Display::default().expect("GTK display must exist");
        let monitors = display.monitors();
        inner.rebuild(&monitors);

        let weak_inner = Rc::downgrade(&inner);
        monitors.connect_items_changed(move |model, _, _, _| {
            if let Some(inner) = weak_inner.upgrade() {
                inner.rebuild(model);
                inner.notify_topology_changed();
            }
        });

        Self {
            inner,
            _monitors: monitors,
        }
    }

    pub(super) fn widget(&self) -> &gtk::DropDown {
        &self.inner.dropdown
    }

    pub(super) fn set_target(&self, target: Option<FullscreenMonitorTarget>) {
        *self.inner.target.borrow_mut() = target;
        let display = self.inner.dropdown.display();
        self.inner.rebuild(&display.monitors());
    }

    pub(super) fn connect_changed<F>(&self, callback: F)
    where
        F: Fn(Option<FullscreenMonitorTarget>) + 'static,
    {
        let weak_inner = Rc::downgrade(&self.inner);
        self.inner
            .dropdown
            .connect_selected_notify(move |dropdown| {
                let Some(inner) = weak_inner.upgrade() else {
                    return;
                };
                if inner.applying.get() {
                    return;
                }
                let target = inner
                    .choices
                    .borrow()
                    .get(dropdown.selected() as usize)
                    .and_then(|choice| choice.target.clone());
                if *inner.target.borrow() == target {
                    return;
                }
                *inner.target.borrow_mut() = target.clone();
                inner.update_bound_widgets();
                callback(target);
            });
    }

    /// Context-menu rows are noise when there is no alternative display.
    pub(super) fn show_only_with_multiple_monitors(&self, widget: &impl IsA<gtk::Widget>) {
        self.inner
            .visible_when_multiple
            .borrow_mut()
            .push(widget.as_ref().clone());
        self.inner.update_bound_widgets();
    }

    /// Preferences keeps the row discoverable. A remembered target remains
    /// editable with one connected display so the user can return to Automatic.
    pub(super) fn enable_only_with_multiple_monitors(&self, widget: &impl IsA<gtk::Widget>) {
        self.inner
            .sensitive_when_multiple
            .borrow_mut()
            .push(widget.as_ref().clone());
        self.inner.update_bound_widgets();
    }

    pub(super) fn connect_topology_changed<F>(&self, callback: F)
    where
        F: Fn() + 'static,
    {
        self.inner
            .topology_changed_handlers
            .borrow_mut()
            .push(Box::new(callback));
    }
}

impl MonitorSelectorInner {
    fn rebuild(&self, model: &gio::ListModel) {
        let monitors = connected_monitors(model);
        let targets = monitors.iter().map(target_from_monitor).collect::<Vec<_>>();
        let connected_count = targets.len() as u32;
        let selected_target = self.target.borrow().clone();
        let selected_connected_index = selected_target
            .as_ref()
            .and_then(|target| matching_target_index(target, &targets));

        let mut labels = vec![gettext("Automatic")];
        let mut monitor_labels = targets
            .iter()
            .enumerate()
            .map(|(index, target)| monitor_label(target, index))
            .collect::<Vec<_>>();
        disambiguate_labels(&mut monitor_labels, &targets);
        labels.extend(monitor_labels);

        let mut choices = vec![MonitorChoice { target: None }];
        choices.extend(targets.iter().cloned().map(|target| MonitorChoice {
            target: Some(target),
        }));

        let selected = if let Some(index) = selected_connected_index {
            index as u32 + 1
        } else if let Some(target) = selected_target {
            let unavailable = format!(
                "{} ({})",
                target_name(&target, connected_count as usize),
                gettext("Unavailable")
            );
            labels.push(unavailable);
            choices.push(MonitorChoice {
                target: Some(target),
            });
            choices.len() as u32 - 1
        } else {
            0
        };

        let label_references = labels.iter().map(String::as_str).collect::<Vec<_>>();
        let string_list = gtk::StringList::new(&label_references);
        let was_applying = self.applying.replace(true);
        self.dropdown.set_model(Some(&string_list));
        self.dropdown.set_selected(selected);
        *self.choices.borrow_mut() = choices;
        self.connected_count.set(connected_count);
        self.applying.set(was_applying);
        self.update_bound_widgets();
    }

    fn update_bound_widgets(&self) {
        let multiple = self.connected_count.get() > 1;
        for widget in self.visible_when_multiple.borrow().iter() {
            widget.set_visible(multiple);
        }
        let useful_choice = multiple || self.target.borrow().is_some();
        for widget in self.sensitive_when_multiple.borrow().iter() {
            widget.set_sensitive(useful_choice);
        }
    }

    fn notify_topology_changed(&self) {
        for callback in self.topology_changed_handlers.borrow().iter() {
            callback();
        }
    }
}

fn connected_monitors(model: &gio::ListModel) -> Vec<gdk::Monitor> {
    (0..model.n_items())
        .filter_map(|index| model.item(index))
        .filter_map(|object| object.downcast::<gdk::Monitor>().ok())
        .filter(|monitor| monitor.is_valid())
        .collect()
}

fn optional_text(value: Option<glib::GString>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn target_from_monitor(monitor: &gdk::Monitor) -> FullscreenMonitorTarget {
    let geometry = monitor.geometry();
    let scale = monitor.scale().max(1.0);
    FullscreenMonitorTarget {
        connector: optional_text(monitor.connector()),
        description: optional_text(monitor.description()),
        manufacturer: optional_text(monitor.manufacturer()),
        model: optional_text(monitor.model()),
        width: (f64::from(geometry.width().max(0)) * scale).round() as u32,
        height: (f64::from(geometry.height().max(0)) * scale).round() as u32,
    }
}

fn dimensions_match(saved: &FullscreenMonitorTarget, candidate: &FullscreenMonitorTarget) -> bool {
    if saved.width == 0 || saved.height == 0 || candidate.width == 0 || candidate.height == 0 {
        return false;
    }
    (saved.width == candidate.width && saved.height == candidate.height)
        || (saved.width == candidate.height && saved.height == candidate.width)
}

fn hardware_fields_match(
    saved: &FullscreenMonitorTarget,
    candidate: &FullscreenMonitorTarget,
) -> bool {
    saved
        .manufacturer
        .as_ref()
        .is_none_or(|value| candidate.manufacturer.as_ref() == Some(value))
        && saved
            .model
            .as_ref()
            .is_none_or(|value| candidate.model.as_ref() == Some(value))
}

fn identity_fields_do_not_conflict(
    saved: &FullscreenMonitorTarget,
    candidate: &FullscreenMonitorTarget,
) -> bool {
    [
        (&saved.description, &candidate.description),
        (&saved.manufacturer, &candidate.manufacturer),
        (&saved.model, &candidate.model),
    ]
    .into_iter()
    .all(|(saved, candidate)| match (saved, candidate) {
        (Some(saved), Some(candidate)) => saved == candidate,
        _ => true,
    })
}

fn unique_match(
    targets: &[FullscreenMonitorTarget],
    predicate: impl Fn(&FullscreenMonitorTarget) -> bool,
) -> Option<usize> {
    let mut matches = targets
        .iter()
        .enumerate()
        .filter_map(|(index, target)| predicate(target).then_some(index));
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

fn matching_target_index(
    saved: &FullscreenMonitorTarget,
    targets: &[FullscreenMonitorTarget],
) -> Option<usize> {
    let has_hardware_name = saved.manufacturer.is_some() || saved.model.is_some();
    if has_hardware_name {
        if let Some(index) = unique_match(targets, |target| hardware_fields_match(saved, target)) {
            return Some(index);
        }

        if let Some(connector) = saved.connector.as_deref()
            && let Some(index) = unique_match(targets, |target| {
                hardware_fields_match(saved, target)
                    && target.connector.as_deref() == Some(connector)
            })
        {
            return Some(index);
        }

        if let Some(description) = saved.description.as_ref()
            && let Some(index) = unique_match(targets, |target| {
                hardware_fields_match(saved, target)
                    && target.description.as_ref() == Some(description)
            })
        {
            return Some(index);
        }

        if let Some(index) = unique_match(targets, |target| {
            hardware_fields_match(saved, target) && dimensions_match(saved, target)
        }) {
            return Some(index);
        }
    }

    if let Some(description) = saved.description.as_ref()
        && let Some(index) = unique_match(targets, |target| {
            target.description.as_ref() == Some(description)
        })
    {
        return Some(index);
    }

    if let Some(connector) = saved.connector.as_deref()
        && let Some(index) = unique_match(targets, |target| {
            target.connector.as_deref() == Some(connector)
                && identity_fields_do_not_conflict(saved, target)
        })
    {
        return Some(index);
    }

    let has_text_identity = saved.connector.is_some()
        || saved.description.is_some()
        || saved.manufacturer.is_some()
        || saved.model.is_some();
    (!has_text_identity)
        .then(|| unique_match(targets, |target| dimensions_match(saved, target)))
        .flatten()
}

fn monitor_label(target: &FullscreenMonitorTarget, index: usize) -> String {
    let name = target_name(target, index);
    if target.width > 0 && target.height > 0 {
        format!("{name} — {} × {}", target.width, target.height)
    } else {
        name
    }
}

fn target_name(target: &FullscreenMonitorTarget, index: usize) -> String {
    target
        .description
        .clone()
        .or_else(|| match (&target.manufacturer, &target.model) {
            (Some(manufacturer), Some(model)) if manufacturer != model => {
                Some(format!("{manufacturer} {model}"))
            }
            (Some(manufacturer), _) => Some(manufacturer.clone()),
            (_, Some(model)) => Some(model.clone()),
            _ => None,
        })
        .or_else(|| target.connector.clone())
        .unwrap_or_else(|| format!("{} {}", gettext("Display"), index + 1))
}

fn disambiguate_labels(labels: &mut [String], targets: &[FullscreenMonitorTarget]) {
    let duplicate = labels
        .iter()
        .map(|label| {
            labels
                .iter()
                .filter(|candidate| *candidate == label)
                .count()
                > 1
        })
        .collect::<Vec<_>>();

    for (index, duplicate) in duplicate.into_iter().enumerate() {
        if duplicate {
            let suffix = targets[index]
                .connector
                .clone()
                .unwrap_or_else(|| format!("{} {}", gettext("Display"), index + 1));
            labels[index] = format!("{} ({suffix})", labels[index]);
        }
    }
}

fn resolve_monitor(
    display: &gdk::Display,
    target: &FullscreenMonitorTarget,
) -> Option<gdk::Monitor> {
    let monitors = connected_monitors(&display.monitors());
    let targets = monitors.iter().map(target_from_monitor).collect::<Vec<_>>();
    matching_target_index(target, &targets)
        .and_then(|index| monitors.get(index).cloned())
        .filter(gdk::prelude::MonitorExt::is_valid)
}

fn request_fullscreen(window: &gtk::Window, target: Option<&FullscreenMonitorTarget>) {
    let display = gtk::prelude::WidgetExt::display(window);
    if let Some(monitor) = target.and_then(|target| resolve_monitor(&display, target)) {
        window.fullscreen_on_monitor(&monitor);
    } else {
        window.fullscreen();
    }
}

/// Shared by F11, canvas double-click, and the context-menu action.
pub(super) fn toggle_fullscreen(window: &gtk::Window, target: Option<&FullscreenMonitorTarget>) {
    if window.is_fullscreen() {
        window.unfullscreen();
    } else {
        request_fullscreen(window, target);
    }
}

/// Moves an already-fullscreen window when the preferred monitor changes.
pub(super) fn reapply_fullscreen_target(
    window: &gtk::Window,
    target: Option<&FullscreenMonitorTarget>,
) {
    if window.is_fullscreen() {
        request_fullscreen(window, target);
    }
}

#[cfg(test)]
mod tests {
    use super::{disambiguate_labels, matching_target_index, monitor_label};
    use crate::core::preferences::FullscreenMonitorTarget;

    fn target(
        connector: Option<&str>,
        manufacturer: Option<&str>,
        model: Option<&str>,
        width: u32,
        height: u32,
    ) -> FullscreenMonitorTarget {
        FullscreenMonitorTarget {
            connector: connector.map(str::to_owned),
            description: None,
            manufacturer: manufacturer.map(str::to_owned),
            model: model.map(str::to_owned),
            width,
            height,
        }
    }

    #[test]
    fn connector_disambiguates_identical_monitors() {
        let saved = target(Some("DP-2"), Some("Dell"), Some("U2723QE"), 3840, 2160);
        let candidates = vec![
            target(Some("DP-1"), Some("Dell"), Some("U2723QE"), 3840, 2160),
            target(Some("DP-2"), Some("Dell"), Some("U2723QE"), 3840, 2160),
        ];
        assert_eq!(matching_target_index(&saved, &candidates), Some(1));
    }

    #[test]
    fn hardware_identity_outweighs_a_reused_connector() {
        let saved = target(Some("DP-2"), Some("Dell"), Some("U2723QE"), 3840, 2160);
        let candidates = vec![
            target(Some("DP-2"), Some("Other"), Some("Display"), 1920, 1080),
            target(Some("DP-4"), Some("Dell"), Some("U2723QE"), 2560, 1440),
        ];
        assert_eq!(matching_target_index(&saved, &candidates), Some(1));
    }

    #[test]
    fn hardware_fallback_survives_connector_changes_and_rotation() {
        let saved = target(Some("DP-2"), Some("Dell"), Some("U2723QE"), 3840, 2160);
        let candidates = vec![
            target(Some("DP-4"), Some("Dell"), Some("U2723QE"), 2160, 3840),
            target(Some("HDMI-1"), Some("LG"), Some("TV"), 3840, 2160),
        ];
        assert_eq!(matching_target_index(&saved, &candidates), Some(0));
    }

    #[test]
    fn hardware_fallback_survives_resolution_changes() {
        let saved = target(Some("DP-2"), Some("Dell"), Some("U2723QE"), 3840, 2160);
        let candidates = vec![
            target(Some("DP-4"), Some("Dell"), Some("U2723QE"), 2560, 1440),
            target(Some("HDMI-1"), Some("LG"), Some("TV"), 3840, 2160),
        ];
        assert_eq!(matching_target_index(&saved, &candidates), Some(0));
    }

    #[test]
    fn ambiguous_fallback_does_not_guess() {
        let saved = target(None, Some("Dell"), Some("U2723QE"), 3840, 2160);
        let candidates = vec![saved.clone(), saved.clone()];
        assert_eq!(matching_target_index(&saved, &candidates), None);
    }

    #[test]
    fn missing_target_falls_back_without_losing_the_saved_identity() {
        let saved = target(Some("HDMI-2"), Some("LG"), Some("TV"), 3840, 2160);
        let candidates = vec![target(
            Some("eDP-1"),
            Some("Framework"),
            Some("Display"),
            2256,
            1504,
        )];
        assert_eq!(matching_target_index(&saved, &candidates), None);
        assert_eq!(saved.connector.as_deref(), Some("HDMI-2"));
    }

    #[test]
    fn metadata_free_monitor_uses_unambiguous_dimensions() {
        let saved = target(None, None, None, 1920, 1080);
        let candidates = vec![
            target(None, None, None, 2560, 1440),
            target(None, None, None, 1920, 1080),
        ];
        assert_eq!(matching_target_index(&saved, &candidates), Some(1));
    }

    #[test]
    fn every_duplicate_label_is_disambiguated() {
        let targets = vec![
            target(Some("DP-1"), Some("Dell"), Some("U2723QE"), 3840, 2160),
            target(Some("DP-2"), Some("Dell"), Some("U2723QE"), 3840, 2160),
        ];
        let mut labels = targets
            .iter()
            .enumerate()
            .map(|(index, target)| monitor_label(target, index))
            .collect::<Vec<_>>();

        disambiguate_labels(&mut labels, &targets);

        assert!(labels[0].ends_with("(DP-1)"));
        assert!(labels[1].ends_with("(DP-2)"));
        assert_ne!(labels[0], labels[1]);
    }
}
