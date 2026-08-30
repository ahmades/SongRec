//! GTK widget construction for the Now Playing window.

use super::style::{
    ALBUM_CSS_CLASS, ARTIST_CSS_CLASS, DETAILS_CSS_CLASS, TITLE_CSS_CLASS, font_css_for_size,
};
use super::track::transition_leg_duration_ms;
use super::{AlbumCoverSize, DisplayMode, TRANSITION_DURATION_DEFAULT_MS, TrackInfoAlignment};
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
const CLASSIC_PADDING_MIN_PX: i32 = 32;
const CLASSIC_PADDING_MAX_PX: i32 = 96;
const IMMERSIVE_MARGIN_MIN_PX: i32 = 28;
const IMMERSIVE_MARGIN_MAX_PX: i32 = 96;
const CINEMA_CROP_RETENTION_MINIMUM: f64 = 0.70;
pub(super) const AMBIENT_FOREGROUND_OPACITY: f64 = 0.3;
const SECONDARY_METADATA_OPACITY: f64 = 0.72;
const BACKGROUND_CSS_CLASS: &str = "now-playing-background";
const IMMERSIVE_INFO_CSS_CLASS: &str = "now-playing-immersive-info";

/// How Cinema mode frames artwork for the current source and viewport aspect ratios.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) enum CinemaFraming {
    /// A cover crop retains enough of the artwork to use it edge-to-edge.
    #[default]
    Cover,
    /// Preserve the complete artwork on the right and use Ambient fill on the left.
    Wide,
    /// Preserve the complete artwork above the metadata and use Ambient fill below it.
    Tall,
}

/// Full-bleed artwork with an automatic non-destructive fallback for extreme aspect ratios.
#[derive(Clone)]
pub(super) struct CinemaArtworkLayout {
    pub(super) container: gtk::Overlay,
    backdrop: gtk::Picture,
    foreground: gtk::Picture,
    source_dimensions: Rc<Cell<(i32, i32)>>,
}

impl CinemaArtworkLayout {
    fn new() -> Self {
        let backdrop = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .can_shrink(true)
            .hexpand(true)
            .vexpand(true)
            .build();
        backdrop.set_can_target(false);

        let foreground = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .can_shrink(true)
            .hexpand(true)
            .vexpand(true)
            .build();
        foreground.set_can_target(false);

        let container = gtk::Overlay::builder().hexpand(true).vexpand(true).build();
        container.set_child(Some(&backdrop));
        container.add_overlay(&foreground);
        container.set_measure_overlay(&foreground, false);
        container.set_clip_overlay(&foreground, true);
        container.set_can_target(false);

        let source_dimensions = Rc::new(Cell::new((0, 0)));
        let source_dimensions_for_position = source_dimensions.clone();
        let foreground_widget = foreground.clone().upcast::<gtk::Widget>();
        let backdrop_for_position = backdrop.clone();
        container.connect_get_child_position(move |_, child| {
            if child != &foreground_widget {
                return None;
            }

            Some(cinema_artwork_rect(
                backdrop_for_position.width(),
                backdrop_for_position.height(),
                source_dimensions_for_position.get(),
            ))
        });

        Self {
            container,
            backdrop,
            foreground,
            source_dimensions,
        }
    }

    /// Updates both Cinema layers without regenerating pixels during window resizes.
    pub(super) fn set_artwork(
        &self,
        original: Option<&gdk::MemoryTexture>,
        ambient: Option<&gdk::MemoryTexture>,
    ) {
        if let Some(original) = original {
            self.source_dimensions
                .set((original.width(), original.height()));
            self.foreground.set_paintable(Some(original));
        } else {
            self.source_dimensions.set((0, 0));
            self.foreground.set_paintable(Option::<&gdk::Texture>::None);
        }
        self.backdrop.set_paintable(ambient);
        self.container.queue_allocate();
    }

    pub(super) fn framing(&self, width: i32, height: i32) -> CinemaFraming {
        cinema_framing(width, height, self.source_dimensions.get())
    }
}

/// Ambient fill with a subdued complete-cover layer that remains recognizable
/// when a square album cover is presented on a wide or tall display.
#[derive(Clone)]
pub(super) struct AmbientArtworkLayout {
    pub(super) container: gtk::Overlay,
    backdrop: gtk::Picture,
    foreground: gtk::Picture,
}

impl AmbientArtworkLayout {
    fn new() -> Self {
        let backdrop = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .can_shrink(true)
            .hexpand(true)
            .vexpand(true)
            .build();
        backdrop.set_can_target(false);

        let foreground = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .can_shrink(true)
            .hexpand(true)
            .vexpand(true)
            .opacity(AMBIENT_FOREGROUND_OPACITY)
            .build();
        foreground.set_can_target(false);

        let container = gtk::Overlay::builder().hexpand(true).vexpand(true).build();
        container.set_child(Some(&backdrop));
        container.add_overlay(&foreground);
        container.set_measure_overlay(&foreground, false);
        container.set_clip_overlay(&foreground, true);
        container.set_can_target(false);

        Self {
            container,
            backdrop,
            foreground,
        }
    }

    /// Updates both layers together so track changes never expose an empty frame.
    pub(super) fn set_artwork(
        &self,
        original: Option<&gdk::MemoryTexture>,
        ambient: Option<&gdk::MemoryTexture>,
    ) {
        self.backdrop.set_paintable(ambient);
        if let Some(original) = original {
            self.foreground.set_paintable(Some(original));
        } else {
            self.foreground.set_paintable(Option::<&gdk::Texture>::None);
        }
    }
}

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
    pub(super) classic_content: gtk::Box,
    pub(super) artwork: gtk::Picture,
    pub(super) artwork_overlay: gtk::Overlay,
    pub(super) album_cover_layout: AlbumCoverLayout,
    pub(super) cinema_artwork: CinemaArtworkLayout,
    pub(super) ambient_artwork: AmbientArtworkLayout,
    pub(super) scrim_area: gtk::DrawingArea,
    pub(super) artwork_placeholder: gtk::Label,
    pub(super) title_label: gtk::Label,
    pub(super) artist_label: gtk::Label,
    pub(super) album_label: gtk::Label,
    pub(super) details_label: gtk::Label,
    pub(super) info_box: gtk::Box,
    pub(super) immersive_title_label: gtk::Label,
    pub(super) immersive_artist_label: gtk::Label,
    pub(super) immersive_album_label: gtk::Label,
    pub(super) immersive_details_label: gtk::Label,
    pub(super) immersive_info_box: gtk::Box,
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
    configure_classic_content(&root, WINDOW_WIDTH, WINDOW_HEIGHT);

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

    let immersive_title_label = metadata_label(TITLE_CSS_CLASS);
    let immersive_artist_label = metadata_label(ARTIST_CSS_CLASS);
    let immersive_album_label = metadata_label(ALBUM_CSS_CLASS);
    let immersive_details_label = metadata_label(DETAILS_CSS_CLASS);
    immersive_title_label.add_css_class(IMMERSIVE_INFO_CSS_CLASS);
    immersive_artist_label.add_css_class(IMMERSIVE_INFO_CSS_CLASS);
    immersive_album_label.add_css_class(IMMERSIVE_INFO_CSS_CLASS);
    immersive_details_label.add_css_class(IMMERSIVE_INFO_CSS_CLASS);
    let immersive_info_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(INFO_BOX_SPACING)
        .halign(gtk::Align::Start)
        .valign(gtk::Align::End)
        .visible(false)
        .build();
    immersive_info_box.append(&immersive_title_label);
    immersive_info_box.append(&immersive_artist_label);
    immersive_info_box.append(&immersive_album_label);
    immersive_info_box.append(&immersive_details_label);

    // A permanent reservation lets Classic and immersive content occupy the
    // same revealer without reparenting the metadata widgets on mode changes.
    let content_layer = gtk::Overlay::builder().hexpand(true).vexpand(true).build();
    let content_reservation = gtk::Box::builder()
        .hexpand(true)
        .vexpand(true)
        .width_request(MIN_WINDOW_WIDTH)
        .height_request(MIN_WINDOW_HEIGHT)
        .build();
    content_layer.set_child(Some(&content_reservation));
    content_layer.add_overlay(&root);
    content_layer.set_measure_overlay(&root, false);
    content_layer.add_overlay(&immersive_info_box);
    content_layer.set_measure_overlay(&immersive_info_box, false);

    let content_revealer = gtk::Revealer::builder()
        .reveal_child(true)
        .transition_duration(transition_leg_duration_ms(TRANSITION_DURATION_DEFAULT_MS))
        .transition_type(gtk::RevealerTransitionType::Crossfade)
        .hexpand(true)
        .vexpand(true)
        .build();
    content_revealer.set_child(Some(&content_layer));

    let background_area = gtk::DrawingArea::new();
    background_area.set_hexpand(true);
    background_area.set_vexpand(true);
    background_area.set_can_target(false);

    let cinema_artwork = CinemaArtworkLayout::new();
    cinema_artwork.container.set_visible(false);
    let ambient_artwork = AmbientArtworkLayout::new();
    ambient_artwork.container.set_visible(false);
    let scrim_area = gtk::DrawingArea::builder()
        .hexpand(true)
        .vexpand(true)
        .visible(false)
        .build();
    scrim_area.set_can_target(false);

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&background_area));
    overlay.add_overlay(&cinema_artwork.container);
    overlay.add_overlay(&ambient_artwork.container);
    overlay.add_overlay(&scrim_area);
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
        ".{BACKGROUND_CSS_CLASS} {{ background-color: transparent; color: #ffffff; }}
         .{TITLE_CSS_CLASS}, .{ARTIST_CSS_CLASS} {{ color: #ffffff; }}
         .{ALBUM_CSS_CLASS}, .{DETAILS_CSS_CLASS} {{ color: rgba(255, 255, 255, {SECONDARY_METADATA_OPACITY}); }}
         .{IMMERSIVE_INFO_CSS_CLASS} {{ text-shadow: 0 1px 4px rgba(0, 0, 0, 0.95); }}
         .now-playing-artwork-rounded {{ border-radius: {ARTWORK_CORNER_RADIUS_PX}px; }}"
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
    let window_for_key = window.downgrade();
    key_controller.connect_key_pressed(move |_, key, _, _| {
        if key == gtk::gdk::Key::F11
            && let Some(window) = window_for_key.upgrade()
        {
            if window.is_fullscreen() {
                window.unfullscreen();
            } else {
                window.fullscreen();
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
            classic_content: root,
            artwork: cover_picture,
            artwork_overlay: cover_overlay,
            album_cover_layout,
            cinema_artwork,
            ambient_artwork,
            scrim_area,
            artwork_placeholder,
            title_label,
            artist_label,
            album_label,
            details_label,
            info_box,
            immersive_title_label,
            immersive_artist_label,
            immersive_album_label,
            immersive_details_label,
            immersive_info_box,
            background_area,
            content_revealer,
        },
        text_css,
    )
}

fn metadata_label(css_class: &str) -> gtk::Label {
    gtk::Label::builder()
        .halign(gtk::Align::Center)
        .hexpand(true)
        .wrap(false)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes([css_class])
        .build()
}

/// Applies Classic metadata alignment to both the block and the text inside it.
///
/// Label alignment alone only moves a label widget. `xalign` and `justify`
/// ensure that text in an expanding label follows the selected edge as well.
pub(super) fn apply_classic_track_info_alignment(
    info_box: &gtk::Box,
    labels: [&gtk::Label; 4],
    alignment: TrackInfoAlignment,
) {
    let (widget_alignment, text_alignment, xalign) = match alignment {
        TrackInfoAlignment::Left => (gtk::Align::Start, gtk::Justification::Left, 0.0),
        TrackInfoAlignment::Center => (gtk::Align::Center, gtk::Justification::Center, 0.5),
        TrackInfoAlignment::Right => (gtk::Align::End, gtk::Justification::Right, 1.0),
    };

    info_box.set_halign(widget_alignment);
    for label in labels {
        label.set_halign(widget_alignment);
        label.set_justify(text_alignment);
        label.set_xalign(xalign);
    }
}

/// Keeps Classic's desktop spacing while fitting its declared minimum viewport.
pub(super) fn configure_classic_content(content: &gtk::Box, width: i32, height: i32) {
    let padding = classic_padding_for_size(width, height);
    content.set_margin_start(padding);
    content.set_margin_end(padding);
    content.set_margin_top(padding);
    content.set_margin_bottom(padding);
}

fn classic_padding_for_size(width: i32, height: i32) -> i32 {
    let minimum_dimension = width.max(0).min(height.max(0));
    let interpolation_range = f64::from(WINDOW_WIDTH - MIN_WINDOW_WIDTH);
    let progress =
        (f64::from(minimum_dimension - MIN_WINDOW_WIDTH) / interpolation_range).clamp(0.0, 1.0);

    (f64::from(CLASSIC_PADDING_MIN_PX)
        + f64::from(CLASSIC_PADDING_MAX_PX - CLASSIC_PADDING_MIN_PX) * progress)
        .round() as i32
}

/// Updates the fixed immersive metadata layout for a mode and viewport.
pub(super) fn configure_immersive_info(
    info_box: &gtk::Box,
    labels: [&gtk::Label; 4],
    mode: DisplayMode,
    cinema_framing: CinemaFraming,
    width: i32,
    height: i32,
) {
    let width = width.max(1);
    let height = height.max(1);
    let margin = ((width.min(height) as f64 * 0.065).round() as i32)
        .clamp(IMMERSIVE_MARGIN_MIN_PX, IMMERSIVE_MARGIN_MAX_PX);
    let (alignment, vertical_alignment, width_fraction, maximum_width_chars) =
        match (mode, cinema_framing) {
            (DisplayMode::FullBleed, CinemaFraming::Wide) => {
                (gtk::Align::Start, gtk::Align::Center, 0.38, 28)
            }
            (DisplayMode::FullBleed, _) => (gtk::Align::Start, gtk::Align::End, 0.78, 40),
            (DisplayMode::Ambient | DisplayMode::LightsOff, _) => {
                (gtk::Align::Center, gtk::Align::Center, 0.82, 40)
            }
            (DisplayMode::Classic, _) => (gtk::Align::Center, gtk::Align::End, 0.82, 40),
        };
    let available_width = (width - margin * 2).max(1);
    let info_width = ((width as f64 * width_fraction).round() as i32)
        .min(available_width)
        .max(1);

    info_box.set_halign(alignment);
    info_box.set_valign(vertical_alignment);
    info_box.set_margin_start(margin);
    info_box.set_margin_end(margin);
    info_box.set_margin_top(margin);
    info_box.set_margin_bottom(margin);
    info_box.set_size_request(info_width, -1);
    for label in labels {
        label.set_halign(alignment);
        label.set_max_width_chars(maximum_width_chars);
        label.set_justify(if matches!(alignment, gtk::Align::Center) {
            gtk::Justification::Center
        } else {
            gtk::Justification::Left
        });
    }
}

/// Chooses whether a full cover crop is acceptable for this viewport.
pub(super) fn cinema_framing(
    view_width: i32,
    view_height: i32,
    source_dimensions: (i32, i32),
) -> CinemaFraming {
    let (source_width, source_height) = source_dimensions;
    if view_width <= 0 || view_height <= 0 || source_width <= 0 || source_height <= 0 {
        return CinemaFraming::Cover;
    }

    let view_aspect = f64::from(view_width) / f64::from(view_height);
    let source_aspect = f64::from(source_width) / f64::from(source_height);
    let retained_fraction = (view_aspect / source_aspect)
        .min(source_aspect / view_aspect)
        .clamp(0.0, 1.0);
    if retained_fraction >= CINEMA_CROP_RETENTION_MINIMUM {
        CinemaFraming::Cover
    } else if view_aspect > source_aspect {
        CinemaFraming::Wide
    } else {
        CinemaFraming::Tall
    }
}

fn cinema_artwork_rect(
    view_width: i32,
    view_height: i32,
    source_dimensions: (i32, i32),
) -> gdk::Rectangle {
    let (source_width, source_height) = source_dimensions;
    match cinema_framing(view_width, view_height, source_dimensions) {
        CinemaFraming::Cover => gdk::Rectangle::new(0, 0, view_width.max(0), view_height.max(0)),
        CinemaFraming::Wide => {
            let artwork_width = (f64::from(view_height) * f64::from(source_width)
                / f64::from(source_height))
            .round() as i32;
            let artwork_width = artwork_width.clamp(1, view_width.max(1));
            gdk::Rectangle::new(
                view_width.saturating_sub(artwork_width),
                0,
                artwork_width,
                view_height.max(1),
            )
        }
        CinemaFraming::Tall => {
            let artwork_height = (f64::from(view_width) * f64::from(source_height)
                / f64::from(source_width))
            .round() as i32;
            let artwork_height = artwork_height.clamp(1, view_height.max(1));
            gdk::Rectangle::new(0, 0, view_width.max(1), artwork_height)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CinemaFraming, cinema_artwork_rect, cinema_framing, classic_padding_for_size};

    #[test]
    fn classic_padding_adapts_between_minimum_and_desktop_sizes() {
        assert_eq!(classic_padding_for_size(360, 410), 32);
        assert_eq!(classic_padding_for_size(540, 820), 64);
        assert_eq!(classic_padding_for_size(720, 820), 96);
        assert_eq!(classic_padding_for_size(1_920, 1_080), 96);
    }

    #[test]
    fn cinema_uses_cover_when_most_of_a_square_artwork_is_retained() {
        assert_eq!(
            cinema_framing(720, 820, (1_000, 1_000)),
            CinemaFraming::Cover
        );
        assert_eq!(
            cinema_artwork_rect(720, 820, (1_000, 1_000)),
            gdk::Rectangle::new(0, 0, 720, 820)
        );
    }

    #[test]
    fn cinema_preserves_complete_square_artwork_on_extreme_viewports() {
        assert_eq!(
            cinema_framing(1_920, 1_080, (1_000, 1_000)),
            CinemaFraming::Wide
        );
        assert_eq!(
            cinema_artwork_rect(1_920, 1_080, (1_000, 1_000)),
            gdk::Rectangle::new(840, 0, 1_080, 1_080)
        );
        assert_eq!(
            cinema_framing(1_080, 1_920, (1_000, 1_000)),
            CinemaFraming::Tall
        );
        assert_eq!(
            cinema_artwork_rect(1_080, 1_920, (1_000, 1_000)),
            gdk::Rectangle::new(0, 0, 1_080, 1_080)
        );
    }

    #[test]
    fn cinema_framing_handles_unallocated_widgets() {
        assert_eq!(cinema_framing(0, 0, (0, 0)), CinemaFraming::Cover);
    }
}
