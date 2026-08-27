//! GTK adapter for artwork decoded and validated by the core.

use crate::core::artwork::Artwork;

/// Creates a texture that shares the core-owned RGBA pixels.
pub(crate) fn texture(artwork: &Artwork) -> gdk::MemoryTexture {
    let pixels = glib::Bytes::from_owned(artwork.rgba());
    gdk::MemoryTexture::new(
        i32::try_from(artwork.width()).expect("validated artwork width fits i32"),
        i32::try_from(artwork.height()).expect("validated artwork height fits i32"),
        gdk::MemoryFormat::R8g8b8a8,
        &pixels,
        artwork.stride(),
    )
}
