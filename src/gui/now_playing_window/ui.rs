//! GTK widget construction for the Now Playing window.

use super::style::{
    ALBUM_CSS_CLASS, ARTIST_CSS_CLASS, DETAILS_CSS_CLASS, TITLE_CSS_CLASS, font_css_for_size,
};
use super::{AlbumCoverSize, TRANSITION_DURATION_DEFAULT_MS};
use adw::prelude::*;
use gettextrs::gettext;
use std::cell::Cell;
use std::rc::Rc;

const WINDOW_WIDTH: i32 = 720;
const WINDOW_HEIGHT: i32 = 820;
const MIN_WINDOW_WIDTH: i32 = 360;
const MIN_WINDOW_HEIGHT: i32 = 410;
const MIN_ARTWORK_SIZE: i32 = 135;
const ARTWORK_MARGIN_PX: i32 = 24;
const ARTWORK_CORNER_RADIUS_PX: i32 = 18;
const ROOT_SPACING: i32 = 18;
const INFO_BOX_SPACING: i32 = 2;
const BACKGROUND_PADDING_PX: i32 = 96;
const BACKGROUND_CSS_CLASS: &str = "now-playing-background";

/// Sizes and centers album artwork without changing the space reserved for
/// metadata in the outer vertical layout.
#[derive(Clone)]
pub(super) struct AlbumCoverLayout {
    container: gtk::Overlay,
    size: Rc<Cell<AlbumCoverSize>>,
}

impl AlbumCoverLayout {
    fn new(artwork_overlay: &gtk::Overlay) -> Self {
        // The main child is the only child that participates in measuring the
        // slot. The scaled artwork is an unmeasured overlay child, so changing
        // its allocation can never move the metadata below this frame.
        let container = gtk::Overlay::builder().hexpand(true).vexpand(true).build();
        let reservation = gtk::Box::builder().hexpand(true).vexpand(true).build();
        container.set_child(Some(&reservation));

        container.add_overlay(artwork_overlay);
        container.set_measure_overlay(artwork_overlay, false);
        container.set_clip_overlay(artwork_overlay, true);

        let size = Rc::new(Cell::new(AlbumCoverSize::default()));
        let size_for_position = size.clone();
        let artwork_widget = artwork_overlay.clone().upcast::<gtk::Widget>();
        container.connect_get_child_position(move |_, child| {
            if child != &artwork_widget {
                return None;
            }

            // The rectangle is relative to the main child, which is the
            // stable artwork reservation inside the outer aspect frame.
            let width = reservation.width().max(0);
            let height = reservation.height().max(0);
            let side = (f64::from(width.min(height)) * size_for_position.get().layout_fraction())
                .round() as i32;

            Some(gdk::Rectangle::new(
                (width - side) / 2,
                (height - side) / 2,
                side,
                side,
            ))
        });

        Self { container, size }
    }

    /// Changes only the artwork allocation; the outer frame keeps the metadata position stable.
    pub(super) fn set_size(&self, size: AlbumCoverSize) {
        if self.size.replace(size) != size {
            self.container.queue_allocate();
        }
    }
}

pub(super) struct NowPlayingWidgets {
    pub(super) window: gtk::Window,
    pub(super) artwork: gtk::Picture,
    pub(super) artwork_overlay: gtk::Overlay,
    pub(super) album_cover_layout: AlbumCoverLayout,
    pub(super) artwork_placeholder: gtk::Label,
    pub(super) title_label: gtk::Label,
    pub(super) artist_label: gtk::Label,
    pub(super) album_label: gtk::Label,
    pub(super) details_label: gtk::Label,
    pub(super) info_box: gtk::Box,
    pub(super) background_area: gtk::DrawingArea,
    pub(super) content_revealer: gtk::Revealer,
}

/// Builds the window, artwork presentation, metadata widgets, and static CSS providers.
pub(super) fn build_ui() -> (NowPlayingWidgets, gtk::CssProvider) {
    let window = gtk::Window::builder()
        .title("SongRec")
        .default_width(WINDOW_WIDTH)
        .default_height(WINDOW_HEIGHT)
        .resizable(true)
        .hide_on_close(true)
        .build();
    window.set_size_request(MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT);

    let header = gtk::HeaderBar::new();
    header.set_title_widget(Some(&gtk::Label::new(Some(&gettext("Now playing")))));
    window.set_titlebar(Some(&header));

    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .vexpand(true)
        .spacing(ROOT_SPACING)
        .build();
    root.add_css_class(BACKGROUND_CSS_CLASS);

    let cover_frame = gtk::AspectFrame::builder()
        .ratio(1.0)
        .obey_child(false)
        .hexpand(true)
        .vexpand(true)
        .build();
    cover_frame.set_margin_top(ARTWORK_MARGIN_PX);
    cover_frame.set_margin_bottom(ARTWORK_MARGIN_PX);
    cover_frame.set_margin_start(ARTWORK_MARGIN_PX);
    cover_frame.set_margin_end(ARTWORK_MARGIN_PX);
    cover_frame.set_size_request(MIN_ARTWORK_SIZE, MIN_ARTWORK_SIZE);

    let cover_picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Contain)
        .can_shrink(true)
        .hexpand(true)
        .vexpand(true)
        .build();

    let artwork_placeholder = gtk::Label::builder()
        .label(&gettext("Listening..."))
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .visible(false)
        .build();
    let cover_overlay = gtk::Overlay::new();
    cover_overlay.set_child(Some(&cover_picture));
    cover_overlay.set_overflow(gtk::Overflow::Hidden);
    cover_overlay.add_css_class("now-playing-artwork-rounded");
    let album_cover_layout = AlbumCoverLayout::new(&cover_overlay);
    cover_frame.set_child(Some(&album_cover_layout.container));

    let title_label = metadata_label(TITLE_CSS_CLASS);
    let artist_label = metadata_label(ARTIST_CSS_CLASS);
    let album_label = metadata_label(ALBUM_CSS_CLASS);
    let details_label = metadata_label(DETAILS_CSS_CLASS);

    let info_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(INFO_BOX_SPACING)
        .halign(gtk::Align::Center)
        .build();
    info_box.append(&title_label);
    info_box.append(&artist_label);
    info_box.append(&album_label);
    info_box.append(&details_label);

    root.append(&cover_frame);
    root.append(&info_box);

    let content_revealer = gtk::Revealer::builder()
        .reveal_child(true)
        .transition_duration(TRANSITION_DURATION_DEFAULT_MS as u32)
        .transition_type(gtk::RevealerTransitionType::Crossfade)
        .hexpand(true)
        .vexpand(true)
        .build();
    content_revealer.set_child(Some(&root));

    let background_area = gtk::DrawingArea::new();
    background_area.set_hexpand(true);
    background_area.set_vexpand(true);
    background_area.set_can_target(false);

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&background_area));
    overlay.add_overlay(&content_revealer);
    overlay.add_overlay(&artwork_placeholder);
    artwork_placeholder.set_halign(gtk::Align::Center);
    artwork_placeholder.set_valign(gtk::Align::Center);
    artwork_placeholder.set_css_classes(&[TITLE_CSS_CLASS]);
    window.set_child(Some(&overlay));

    let background_css = gtk::CssProvider::new();
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &background_css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
    background_css.load_from_string(&format!(
        ".{BACKGROUND_CSS_CLASS} {{ background-color: transparent; color: #ffffff; padding: {BACKGROUND_PADDING_PX}px; }}\n             .{BACKGROUND_CSS_CLASS} > label {{ color: #ffffff; }}\n             .now-playing-artwork-rounded {{ border-radius: {ARTWORK_CORNER_RADIUS_PX}px; }}"
    ));

    let text_css = gtk::CssProvider::new();
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &text_css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
    text_css.load_from_string(&font_css_for_size((WINDOW_WIDTH, WINDOW_HEIGHT)));

    let key_controller = gtk::EventControllerKey::new();
    let window_for_key = window.clone();
    key_controller.connect_key_pressed(move |_, key, _, _| {
        if key == gtk::gdk::Key::F11 {
            if window_for_key.is_fullscreen() {
                window_for_key.unfullscreen();
            } else {
                window_for_key.fullscreen();
            }
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(key_controller);

    (
        NowPlayingWidgets {
            window,
            artwork: cover_picture,
            artwork_overlay: cover_overlay,
            album_cover_layout,
            artwork_placeholder,
            title_label,
            artist_label,
            album_label,
            details_label,
            info_box,
            background_area,
            content_revealer,
        },
        text_css,
    )
}

fn metadata_label(css_class: &str) -> gtk::Label {
    gtk::Label::builder()
        .halign(gtk::Align::Center)
        .wrap(false)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes([css_class])
        .build()
}
