//! Converts core-decoded artwork into GTK presentation objects.

use crate::core::artwork::Artwork;

pub(super) use crate::core::artwork::ArtworkBackground as Background;

/// A texture and palette sharing the decoded pixels owned by [`Artwork`].
#[derive(Clone)]
pub(super) struct PreparedArtwork {
    pub(super) texture: gdk::MemoryTexture,
    pub(super) background: Background,
}

/// Builds a GTK memory texture without decoding the compressed cover again.
pub(super) fn prepare_artwork(artwork: &Artwork) -> PreparedArtwork {
    PreparedArtwork {
        texture: crate::gui::artwork::texture(artwork),
        background: artwork.background(),
    }
}
