//! Validated, decoded artwork shared by recognition and presentation layers.

use std::fmt;
use std::io::Cursor;
use std::sync::Arc;

pub const MAX_ARTWORK_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_ARTWORK_DIMENSION_PX: u32 = 4_096;

/// One validated cover image, decoded once for downstream consumers.
///
/// Encoded bytes are retained for consumers that need the original image, while
/// shared RGBA pixels let presentation layers avoid decoding it again.
#[derive(Clone)]
pub struct Artwork {
    encoded: Arc<[u8]>,
    rgba: Arc<[u8]>,
    width: u32,
    height: u32,
    stride: usize,
}

impl fmt::Debug for Artwork {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Artwork")
            .field("encoded_bytes", &self.encoded.len())
            .field("rgba_bytes", &self.rgba.len())
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

impl Artwork {
    /// Validates and decodes a PNG or JPEG response.
    pub fn decode(encoded: Vec<u8>) -> Option<Self> {
        if encoded.is_empty() || encoded.len() > MAX_ARTWORK_BYTES {
            return None;
        }

        let format = image::guess_format(&encoded).ok()?;
        if !matches!(format, image::ImageFormat::Jpeg | image::ImageFormat::Png) {
            return None;
        }

        let mut limits = image::Limits::default();
        limits.max_image_width = Some(MAX_ARTWORK_DIMENSION_PX);
        limits.max_image_height = Some(MAX_ARTWORK_DIMENSION_PX);
        // A validated RGBA image at the maximum dimensions occupies 64 MiB;
        // leave room for decoder scratch buffers while retaining a hard cap.
        limits.max_alloc = Some(128 * 1024 * 1024);

        let mut reader = image::ImageReader::with_format(Cursor::new(&encoded), format);
        reader.limits(limits);
        let image = reader.decode().ok()?;
        if image.width() == 0
            || image.height() == 0
            || image.width() > MAX_ARTWORK_DIMENSION_PX
            || image.height() > MAX_ARTWORK_DIMENSION_PX
        {
            return None;
        }

        let rgba = image.to_rgba8();
        let width = rgba.width();
        let height = rgba.height();
        let stride = usize::try_from(width).ok()?.checked_mul(4)?;

        Some(Self {
            encoded: encoded.into(),
            rgba: rgba.into_raw().into(),
            width,
            height,
            stride,
        })
    }

    pub fn encoded(&self) -> &[u8] {
        &self.encoded
    }

    pub fn rgba(&self) -> Arc<[u8]> {
        self.rgba.clone()
    }

    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }

    pub const fn stride(&self) -> usize {
        self.stride
    }

    /// Approximate retained payload size used by the in-memory artwork cache.
    pub fn storage_bytes(&self) -> usize {
        self.encoded.len().saturating_add(self.rgba.len())
    }
}

#[cfg(test)]
mod tests {
    use super::{Artwork, MAX_ARTWORK_DIMENSION_PX};
    use image::{DynamicImage, ImageFormat};
    use std::io::Cursor;

    fn encoded_png() -> Vec<u8> {
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::new_rgba8(2, 2)
            .write_to(&mut encoded, ImageFormat::Png)
            .unwrap();
        encoded.into_inner()
    }

    #[test]
    fn artwork_is_validated_and_decoded_once() {
        let artwork = Artwork::decode(encoded_png()).unwrap();
        assert_eq!((artwork.width(), artwork.height()), (2, 2));
        assert_eq!(artwork.stride(), 8);
        assert_eq!(artwork.rgba().len(), 16);
        assert!(Artwork::decode(Vec::new()).is_none());
        assert!(Artwork::decode(b"<html>not artwork</html>".to_vec()).is_none());
    }

    #[test]
    fn oversized_dimensions_are_rejected_before_allocating_the_decoded_image() {
        let image = DynamicImage::new_rgba8(MAX_ARTWORK_DIMENSION_PX + 1, 1);
        let mut encoded = Cursor::new(Vec::new());
        image.write_to(&mut encoded, ImageFormat::Png).unwrap();

        assert!(Artwork::decode(encoded.into_inner()).is_none());
    }
}
