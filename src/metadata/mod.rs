//! Metadata containers decoded from input images.
//!
//! Seeded as a placeholder for WS4 (EXIF/ICC/XMP round-tripping); the input
//! layer already fills these fields so later workstreams only need to
//! consume them.

/// Container-independent metadata of a source image.
#[derive(Default, Clone)]
pub struct ImageMetadata {
    /// Raw TIFF/Exif payload, if present in the source container.
    pub exif: Option<Vec<u8>>,
    /// Raw ICC color profile, if present in the source container.
    pub icc: Option<Vec<u8>>,
    /// Raw XMP packet, if present in the source container.
    pub xmp: Option<Vec<u8>>,
    /// Whether the pixel data has already been rotated upright
    /// according to the EXIF orientation tag.
    pub exif_applied_orientation: bool,
}
