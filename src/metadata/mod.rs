//! Metadata handling: policy, extraction normalization, EXIF
//! resolution/re-serialization, orientation bake-in and container
//! embedding/muxing helpers.
//!
//! # Payload convention
//!
//! [`ImageMetadata::exif`] always holds the **raw TIFF byte stream**:
//! the content of a JPEG APP1 segment after its `b"Exif\0\0"` header, the
//! body of a PNG `eXIf` chunk, the HEIF `Exif` item, or the JXL `Exif` box
//! payload minus its 4-byte offset prefix. Every boundary to a container
//! that needs the marker prefix (JPEG APP1, WebP `EXIF` chunk) crosses the
//! normalizers in this module — no encoder ever sees mixed conventions.

pub mod policy;

#[cfg(feature = "exif")]
pub mod exif;
#[cfg(feature = "exif")]
pub mod orientation;
pub mod riff;
pub mod sink;

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

/// JPEG APP1-style Exif header (`"Exif\0\0"`) that some containers prefix
/// in front of the TIFF stream.
const EXIF_HEADER: &[u8] = b"Exif\0\0";

/// Normalizes a decoder/container payload to the raw TIFF stream convention:
/// strips the `b"Exif\0\0"` marker prefix when present and drops empty
/// payloads.
#[must_use]
pub fn normalize_exif_payload(raw: Vec<u8>) -> Option<Vec<u8>> {
    let payload = if raw.len() >= EXIF_HEADER.len() && raw[..EXIF_HEADER.len()] == *EXIF_HEADER {
        raw[EXIF_HEADER.len()..].to_vec()
    } else {
        raw
    };
    if payload.is_empty() {
        None
    } else {
        Some(payload)
    }
}

/// Builds the Exif box payload for JPEG XL (WS2): a 4-byte TIFF-header
/// offset prefix followed by four padding bytes and the TIFF stream.
///
/// The offset is a **big-endian** `u32`; both libjxl (`LoadBE32`, TIFF
/// header at `box_payload[4 + offset]`) and jxl-oxide (`tiff_header_offset`,
/// TIFF header at `payload()[offset]` = `box_payload[4 + offset]`) measure
/// it from *behind* the 4-byte offset field. With the four padding bytes
/// used here (keeping the TIFF header 8-byte aligned inside the box) the
/// correct offset value is `4`.
///
/// Plain bytes helper, deliberately available without the `exif` feature.
#[must_use]
pub fn exif_for_jxl(tiff_payload: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(8 + tiff_payload.len());
    payload.extend_from_slice(&4u32.to_be_bytes());
    payload.extend_from_slice(&[0, 0, 0, 0]);
    payload.extend_from_slice(tiff_payload);
    payload
}

/// Resolves the EXIF policy against a source image's metadata.
///
/// Returns the payload to embed (raw TIFF convention) or `None`.
/// See [`metadata::exif::resolve`] for the exact rules; without the `exif`
/// feature this always returns `None` (strip semantics).
#[cfg(feature = "exif")]
#[must_use]
pub fn resolve(policy: &policy::ExifPolicy, meta: &ImageMetadata) -> Option<Vec<u8>> {
    exif::resolve(policy, meta)
}

/// Feature-less fallback of [`resolve`]: metadata is always stripped.
#[cfg(not(feature = "exif"))]
#[must_use]
pub fn resolve(_policy: &policy::ExifPolicy, _meta: &ImageMetadata) -> Option<Vec<u8>> {
    None
}

/// Extraction fallback for formats whose decoder does not surface the raw
/// EXIF payload:
/// - WebP: RIFF scan for the `EXIF` chunk (decode-side fallback),
/// - TIFF: container scan via `kamadak-exif`, re-serialized into a compact
///   raw TIFF payload (falling back to the whole file bytes, whose offsets
///   stay valid, if the writer refuses).
///
/// TODO(WS1 HEIF/HEIC): extraction is wired through
/// `handle.metadata_block_ids(b"Exif")` + `handle.metadata(id)` once the
/// libheif input module exists; it must fill
/// [`ImageMetadata::exif`] (normalized to raw TIFF) and set
/// `exif_applied_orientation` per whether the decoder baked the transform in.
///
/// WS2 JXL: extraction is implemented in `input::jxl` via
/// `aux_boxes().first_exif()` minus the offset prefix; the box payload
/// convention is documented in [`exif_for_jxl`].
#[cfg(feature = "exif")]
#[must_use]
pub fn extract_exif_fallback(
    path: &std::path::Path,
    source_format: &crate::format::ImageFormat,
) -> Option<Vec<u8>> {
    use crate::format::ImageFormat;
    let bytes = std::fs::read(path).ok()?;
    match source_format {
        ImageFormat::Webp => riff::extract_exif_payload(&bytes),
        ImageFormat::Tiff => {
            let parsed = exif::parse_with_container(&bytes)?;
            match exif::serialize_all(&parsed) {
                Ok(payload) if !payload.is_empty() => Some(payload),
                _ => normalize_exif_payload(bytes),
            }
        }
        _ => None,
    }
}

/// Feature-less fallback of [`extract_exif_fallback`].
#[cfg(not(feature = "exif"))]
#[must_use]
pub fn extract_exif_fallback(
    _path: &std::path::Path,
    _source_format: &crate::format::ImageFormat,
) -> Option<Vec<u8>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_strips_marker_prefix_and_empties() {
        let tiff = b"II*\0\x08\0\0\0\0\0".to_vec();
        let mut prefixed = EXIF_HEADER.to_vec();
        prefixed.extend_from_slice(&tiff);
        assert_eq!(
            normalize_exif_payload(prefixed).as_deref(),
            Some(tiff.as_slice())
        );
        assert_eq!(
            normalize_exif_payload(tiff.clone()).as_deref(),
            Some(tiff.as_slice())
        );
        assert_eq!(normalize_exif_payload(Vec::new()), None);
        assert_eq!(normalize_exif_payload(EXIF_HEADER.to_vec()), None);
        assert_eq!(
            normalize_exif_payload(b"Exif".to_vec()).as_deref(),
            Some(&b"Exif"[..])
        );
    }

    #[test]
    fn jxl_payload_uses_offset_prefix_convention() {
        let tiff = b"II*\0\x08\0\0\0";
        let payload = exif_for_jxl(tiff);
        // the offset is big-endian and measured from behind the offset
        // field (libjxl `box_payload[4 + offset]`, jxl-oxide convention);
        // with the 4 padding bytes the TIFF header sits at payload byte 8
        assert_eq!(&payload[..4], &4u32.to_be_bytes());
        assert_eq!(&payload[4..8], &[0, 0, 0, 0]);
        assert_eq!(&payload[8..], tiff);
        // a reader following the offset must land exactly on the TIFF header
        let offset = u32::from_be_bytes(payload[..4].try_into().expect("4 bytes")) as usize;
        assert_eq!(&payload[4 + offset..], tiff);
    }
}
